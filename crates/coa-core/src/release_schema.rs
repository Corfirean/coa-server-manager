//! Database acceptance for release packages, using a freshly extracted signed base fixture.
use std::collections::HashSet;
use std::path::Path;
use crate::{backup, fsx, install, manifest::{Kind, Migration}, migrations, package, pkgsource, release::{self, SqlFile}, schema_check, Error, Result};

const MARKER: &str = ".release-schema-fixture";

pub fn collect(core: &Path, bots: Option<&Path>, repairs: Option<&Path>) -> Result<Vec<SqlFile>> {
    let mut files = release::collect_core_sql(core)?;
    if let Some(bots) = bots { files.extend(release::collect_bots_sql(bots)?); }
    if let Some(repairs) = repairs {
        for kind in ["auth", "characters", "world"] {
            let dir = repairs.join(kind);
            if !dir.exists() { continue; }
            let mut paths: Vec<_> = std::fs::read_dir(&dir)?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<_>>()?;
            paths.sort();
            for path in paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "sql")) {
                let stem = path.file_stem().unwrap().to_string_lossy();
                files.push(SqlFile { db: kind.into(), id: format!("manager_repair__{stem}"), origin: format!("{kind}/{stem}.sql"), sha256: fsx::sha256_file(&path)?, abs: path });
            }
        }
    }
    let mut seen = HashSet::new();
    for file in &files {
        if !seen.insert((&file.db, &file.id)) { return Err(Error::Invalid(format!("Duplicate release migration: {} / {}", file.db, file.id))); }
    }
    Ok(files)
}

pub fn extract_base(source: &Path, fixture: &Path, trusted_key: &str) -> Result<()> {
    if fixture.exists() || !fixture.file_name().is_some_and(|n| n.to_string_lossy().starts_with("coa-schema-fixture-")) {
        return Err(Error::Invalid("Use a new coa-schema-fixture-* directory for release validation.".into()));
    }
    let (manifest, _) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(source.into()), trusted_key)?;
    if manifest.kind != Kind::Base { return Err(Error::Invalid("Release schema validation needs a signed base package.".into())); }
    package::extract_selected(source, &manifest, fixture, &|path| {
        let path = path.to_ascii_lowercase();
        ["mysql/", "settings/", "runtime/", "scripts/", "core/", "bugreport/"].iter().any(|prefix| path.starts_with(prefix))
    }, &|_, _| {})?;
    fsx::atomic_write(&fixture.join(MARKER), b"disposable release-schema fixture")?;
    install::bootstrap_database(fixture)
}

pub fn capture_release(fixture: &Path, tree: &Path, sql: &[SqlFile]) -> Result<()> {
    if !fixture.file_name().is_some_and(|n| n.to_string_lossy().starts_with("coa-schema-fixture-"))
        || std::fs::read(fixture.join(MARKER)).ok().as_deref() != Some(b"disposable release-schema fixture") {
        return Err(Error::Invalid("Refusing to apply release SQL to a non-fixture server.".into()));
    }
    let staging = tempfile::tempdir()?;
    let migrations: Vec<_> = sql.iter().map(|f| Migration { db: f.db.clone(), id: f.id.clone(), sha256: f.sha256.clone(), destructive: false }).collect();
    for file in sql {
        let path = fsx::safe_join(staging.path(), &format!("{}/{}.sql", file.db, file.id))?;
        fsx::atomic_write(&path, &std::fs::read(&file.abs)?)?;
    }
    install::with_scratch_ports(fixture, || backup::with_database(fixture, |db| {
        let db = db.clone().for_realm(crate::realms::Mode::Coa);
        let report = migrations::apply_pending(&db, &migrations, staging.path(), &|| Ok("disposable release fixture".into()))?;
        if let Some((id, why)) = report.failed { return Err(Error::Invalid(format!("Release migration {id} failed: {why}"))); }
        // Validate known critical feature effects before capturing; history may be stale in the base.
        let previous = tree.join(schema_check::CONTRACT);
        if previous.exists() { return Err(Error::Invalid("Release tree already contains a schema contract; generate it from a fresh tree.".into())); }
        let problems = schema_check::check(&db, tree)?;
        if let Some(p) = problems.first() { return Err(Error::Invalid(format!("Release database failed validation: {}.{}.{}: {}", p.database, p.table, p.column, p.detail))); }
        schema_check::capture(&db, tree)?;
        schema_check::require_release_contract(tree)?;
        Ok(())
    }))
}

pub fn verify_package(dir: &Path, trusted_key: &str) -> Result<()> {
    let (manifest, _) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(dir.into()), trusted_key)?;
    if !manifest.files.iter().any(|f| f.path == schema_check::CONTRACT && f.size > 0) {
        return Err(Error::Invalid("The release is missing its database schema contract.".into()));
    }
    let scratch = tempfile::tempdir()?;
    package::extract_selected(dir, &manifest, scratch.path(), &|p| p == schema_check::CONTRACT, &|_, _| {})?;
    schema_check::require_release_contract(scratch.path())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn signed_packages_must_carry_a_real_complete_schema_contract() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let out = dir.path().join("out");
        fsx::atomic_write(&tree.join("Core/worldserver.exe"), b"fixture").unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = STANDARD.encode(key.verifying_key().as_bytes());
        let opts = package::BuildOptions { kind: Kind::Update, version: "1.0.0".into(), core_commit: None, bots_commit: None, built_at: "test".into(), part_size: 1024 * 1024, migrations: vec![] };
        let sign = || {
            let bytes = std::fs::read(out.join("manifest.json")).unwrap();
            fsx::atomic_write(&out.join("manifest.json.sig"), STANDARD.encode(key.sign(&bytes).to_bytes()).as_bytes()).unwrap();
        };
        package::build(&tree, &out, &opts, &|_| {}).unwrap(); sign();
        assert!(verify_package(&out, &public).is_err());
        fsx::atomic_write(&tree.join(schema_check::CONTRACT), br#"{"schema":1,"columns":{}}"#).unwrap();
        package::build(&tree, &out, &opts, &|_| {}).unwrap(); sign();
        assert!(verify_package(&out, &public).is_err());
        fsx::atomic_write(&tree.join(schema_check::CONTRACT), br#"{"schema":1,"columns":{"auth":{"account":{"id":"int"}},"characters":{"characters":{"guid":"int"}},"world":{"creature":{"id":"int"}}}}"#).unwrap();
        package::build(&tree, &out, &opts, &|_| {}).unwrap(); sign();
        verify_package(&out, &public).unwrap();
    }
    #[test]
    fn repair_sql_has_new_ids_and_runs_after_original_sql() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("core");
        let repairs = dir.path().join("repairs");
        fsx::atomic_write(&core.join("data/sql/updates/pending_db_characters/old.sql"), b"SELECT 1;").unwrap();
        fsx::atomic_write(&repairs.join("characters/20261005_wildcard.sql"), b"SELECT 2;").unwrap();
        let files = collect(&core, None, Some(&repairs)).unwrap();
        assert_eq!(files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["old", "manager_repair__20261005_wildcard"]);
        assert_eq!(files[1].db, "characters");
    }
    #[test]
    fn existing_server_cannot_be_used_for_schema_generation() {
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_base(dir.path(), dir.path(), "irrelevant").is_err());
        assert!(capture_release(dir.path(), dir.path(), &[]).is_err());
        assert!(!dir.path().join(schema_check::CONTRACT).exists());
    }
}
