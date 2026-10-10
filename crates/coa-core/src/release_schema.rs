//! Database acceptance for release packages, using a freshly extracted signed base fixture.
use std::collections::HashSet;
use std::path::Path;
use crate::{backup, fsx, install, manifest::{Kind, Migration}, migrations, package, pkgsource, release::{self, SqlFile}, schema_check, Error, Result};

const MARKER: &str = ".release-schema-fixture";

struct QualificationEnv<'a>(crate::update::RepackEnv<'a>);

impl crate::update::Env for QualificationEnv<'_> {
    fn activate(&self) -> Result<()> { Ok(()) }
    fn ensure_stopped(&self) -> Result<()> { self.0.ensure_stopped() }
    fn preflight(&self, manifest: &crate::manifest::Manifest) -> Result<()> { self.0.preflight(manifest) }
    fn snapshot(&self) -> Result<String> { self.0.snapshot() }
    fn verify_snapshot(&self, id: &str) -> Result<()> { self.0.verify_snapshot(id) }
    fn rehearse(&self, manifest: &crate::manifest::Manifest, tree: &Path, items: &[crate::update::PlanItem], point: &str) -> Result<()> { self.0.rehearse(manifest, tree, items, point) }
    fn restore_snapshot(&self, id: &str) -> Result<()> { self.0.restore_snapshot(id) }
    fn migrate(&self, manifest: &crate::manifest::Manifest, staged: &Path) -> Result<crate::migrations::ApplyReport> { self.0.migrate(manifest, staged) }
    fn validate(&self) -> Result<()> { self.0.validate() }
    fn validate_recovery(&self) -> Result<()> { self.0.validate_recovery() }
    fn recover_validation(&self) -> Result<()> { self.0.recover_validation() }
    fn automatic_rollback(&self) -> bool { true }
    // Health checks use the normal private validation; qualification never opens game ports after commit.
}

pub fn qualify_upgrade(base: &Path, source: &Path, candidate: &Path, fixture: &Path, trusted_key: &str) -> Result<serde_json::Value> {
    use crate::update::Env;
    if !cfg!(windows) { return Err(Error::Invalid("Manager upgrade qualification requires a Windows fixture.".into())); }
    if fixture.exists() || !fixture.file_name().is_some_and(|name| name.to_string_lossy().starts_with("coa-schema-fixture-")) {
        return Err(Error::Invalid("Upgrade qualification requires a new disposable coa-schema-fixture-* directory.".into()));
    }
    let metadata = crate::registry::metadata_dir_for(fixture)?;
    if metadata.exists() { return Err(Error::Invalid("Upgrade fixture metadata already exists.".into())); }
    let (base_manifest, base_bytes) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(base.into()), trusted_key)?;
    let (source_manifest, source_bytes) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(source.into()), trusted_key)?;
    let (candidate_manifest, _) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(candidate.into()), trusted_key)?;
    if candidate_manifest.kind != Kind::Update || !matches!(source_manifest.kind, Kind::Base | Kind::Update)
        || (source_manifest.kind == Kind::Base && fsx::sha256_bytes(&base_bytes) != fsx::sha256_bytes(&source_bytes)) {
        return Err(Error::Invalid("Invalid source or candidate package for upgrade qualification.".into()));
    }
    extract_base(base, fixture, trusted_key)?;
    package::extract_selected(base, &base_manifest, fixture, &|path| path.starts_with("Data/"), &|_, _| {})?;
    let mut meta = crate::registry::InstallMeta::new(crate::registry::InstallKind::New, fixture);
    meta.core.version = Some(base_manifest.version.clone());
    meta.core.commit = base_manifest.core.commit.clone();
    meta.original_hashes = base_manifest.files.iter().map(|file| (file.path.clone(), file.sha256.clone())).collect();
    crate::registry::MetaDir::create(fixture, &meta)?;
    let env = QualificationEnv(crate::update::RepackEnv { root: fixture, meta_dir: &metadata });
    let apply = |package: &Path| -> Result<()> {
        let outcome = crate::update::apply(&crate::update::Params {
            root: fixture, meta_dir: &metadata, source: pkgsource::Source::Dir(package.into()),
            trusted_key, cancel: crate::download::Cancel::default(), resolutions: Default::default(),
            env: &env, fail_after_ops: None,
        }, &|_, _| {})?;
        if outcome.txn.state != crate::update::State::Committed {
            return Err(Error::Invalid("Upgrade qualification did not commit.".into()));
        }
        env.ensure_stopped()
    };
    install::with_scratch_ports(fixture, || {
        if source_manifest.kind == Kind::Update { apply(source)?; }
        let user_file = fixture.join("Core/configs/coa-qualification-user.conf");
        let user_bytes = b"Qualification.UserSetting = 7391\r\n";
        fsx::atomic_write(&user_file, user_bytes)?;
        apply(candidate)?;
        if std::fs::read(&user_file)?.as_slice() != user_bytes {
            return Err(Error::Invalid("The update changed a user configuration file.".into()));
        }
        let (_, installed) = crate::registry::MetaDir::open(&metadata)?;
        if installed.core.version.as_deref() != Some(candidate_manifest.version.as_str())
            || crate::update_isolation::pending(&metadata) || crate::update::pending_checked(&metadata)?.is_some() {
            return Err(Error::Invalid("The update left inconsistent metadata or an unfinished transaction.".into()));
        }
        Ok(())
    })?;
    Ok(serde_json::json!({
        "schema": 1, "result": "passed", "fromVersion": source_manifest.version,
        "toVersion": candidate_manifest.version, "fromManifestSha256": fsx::sha256_bytes(&source_bytes),
        "fromSchemaSha256": source_manifest.files.iter().find(|file| file.path == schema_check::CONTRACT).map(|file| &file.sha256),
        "checks": ["manager-transaction", "private-user-database-rehearsal", "database-schema", "private-auth-world-startup", "existing-identities", "user-file-preservation"],
    }))
}

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
    // A Linux computer has no Windows database program: the fixture is a Docker server over the package's data directory.
    if cfg!(windows) {
        install::bootstrap_database(fixture)
    } else {
        crate::docker::fixture::prepare(fixture)
    }
}

pub fn capture_release(fixture: &Path, tree: &Path, sql: &[SqlFile]) -> Result<()> {
    if !fixture.file_name().is_some_and(|n| n.to_string_lossy().starts_with("coa-schema-fixture-"))
        || std::fs::read(fixture.join(MARKER)).ok().as_deref() != Some(b"disposable release-schema fixture") {
        return Err(Error::Invalid("Refusing to apply release SQL to a non-fixture server.".into()));
    }
    let staging = tempfile::tempdir()?;
    let migrations: Vec<_> = sql.iter().map(|f| Migration { compatible_sha256: vec![], db: f.db.clone(), id: f.id.clone(), sha256: f.sha256.clone(), destructive: false }).collect();
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

pub fn validate_startup(fixture: &Path, tree: &Path, base: &Path, trusted_key: &str) -> Result<()> {
    use crate::update::Env;
    if !cfg!(windows) {
        return Err(Error::Invalid("Repack startup qualification requires a Windows runner.".into()));
    }
    if !fixture.file_name().is_some_and(|n| n.to_string_lossy().starts_with("coa-schema-fixture-"))
        || std::fs::read(fixture.join(MARKER)).ok().as_deref() != Some(b"disposable release-schema fixture") {
        return Err(Error::Invalid("Startup qualification requires a disposable release-schema fixture.".into()));
    }
    let metadata = crate::registry::metadata_dir_for(fixture)?;
    if metadata.exists() { return Err(Error::Invalid("Startup fixture already has Manager metadata; use a fresh fixture.".into())); }
    schema_check::require_release_contract(tree)?;
    for name in ["worldserver.exe", "authserver.exe"] {
        if !tree.join("Core").join(name).is_file() {
            return Err(Error::Invalid(format!("Startup candidate is missing {name}.")));
        }
    }
    let (manifest, _) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(base.into()), trusted_key)?;
    if manifest.kind != Kind::Base { return Err(Error::Invalid("Startup qualification requires a signed base package.".into())); }
    let mut directories = vec![tree.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() { return Err(Error::Invalid("Startup release tree must not contain symbolic links.".into())); }
            let path = entry.path();
            fsx::ensure_within(tree, &path)?;
            if kind.is_dir() { directories.push(path); continue; }
            if !kind.is_file() { return Err(Error::Invalid("Startup release tree contains an unsupported file type.".into())); }
            let relative = path.strip_prefix(tree).map_err(|e| Error::Invalid(e.to_string()))?.to_path_buf();
            let first = relative.components().next().unwrap().as_os_str().to_string_lossy();
            if !matches!(first.as_ref(), "Core" | "Scripts" | "Data" | "Extras" | "Licenses") {
                return Err(Error::Invalid("Startup release tree contains installation-specific files.".into()));
            }
            fsx::ensure_within(fixture, &fixture.join(&relative))?;
            files.push(relative);
        }
    }
    package::extract_selected(base, &manifest, fixture, &|path| path.starts_with("Data/"), &|_, _| {})?;
    for relative in files {
        let target = fsx::ensure_within(fixture, &fixture.join(&relative))?;
        fsx::atomic_write(&target, &std::fs::read(tree.join(relative))?)?;
    }
    let meta = crate::registry::InstallMeta::new(crate::registry::InstallKind::New, fixture);
    crate::registry::MetaDir::create(fixture, &meta)?;
    let env = crate::update::RepackEnv { root: fixture, meta_dir: &metadata };
    env.validate()?;
    env.ensure_stopped()?;
    if crate::update_isolation::pending(&metadata) {
        return Err(Error::Invalid("Startup qualification left unfinished isolated validation.".into()));
    }
    Ok(())
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

pub fn verify_upgrade_fixture(dir: &Path, trusted_key: &str) -> Result<()> {
    let (manifest, _) = pkgsource::fetch_manifest(&pkgsource::Source::Dir(dir.into()), trusted_key)?;
    if !matches!(manifest.kind, Kind::Base | Kind::Update) {
        return Err(Error::Invalid("Upgrade fixtures must be base or server update packages.".into()));
    }
    let scratch = tempfile::tempdir()?;
    package::extract_selected(dir, &manifest, scratch.path(), &|path| path == schema_check::CONTRACT, &|_, _| {})?;
    if manifest.kind == Kind::Update || manifest.files.iter().any(|file| file.path == schema_check::CONTRACT) {
        schema_check::require_release_contract(scratch.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn startup_qualification_refuses_real_server_folders_before_writes() {
        let directory = tempfile::tempdir().unwrap();
        let server = directory.path().join("live-server");
        std::fs::create_dir(&server).unwrap();
        fsx::atomic_write(&server.join("owner-data"), b"untouched").unwrap();
        assert!(validate_startup(&server, directory.path(), directory.path(), "unused").is_err());
        assert_eq!(std::fs::read(server.join("owner-data")).unwrap(), b"untouched");
        assert!(!crate::registry::metadata_dir_for(&server).unwrap().exists());
    }

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
        assert!(qualify_upgrade(dir.path(), dir.path(), dir.path(), dir.path(), "irrelevant").is_err());
        assert!(!dir.path().join(schema_check::CONTRACT).exists());
    }

    #[test]
    fn only_legacy_base_fixtures_may_omit_the_schema_contract() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let out = dir.path().join("package");
        fsx::atomic_write(&tree.join("Core/example.txt"), b"fixture").unwrap();
        let key = SigningKey::from_bytes(&[29; 32]);
        let public = STANDARD.encode(key.verifying_key().as_bytes());
        let options = package::BuildOptions { kind: Kind::Base, version: "0.2.0".into(), core_commit: None, bots_commit: None, built_at: "test".into(), part_size: 1024 * 1024, migrations: vec![] };
        package::build(&tree, &out, &options, &|_| {}).unwrap();
        let sign = || {
            let bytes = std::fs::read(out.join("manifest.json")).unwrap();
            fsx::atomic_write(&out.join("manifest.json.sig"), STANDARD.encode(key.sign(&bytes).to_bytes()).as_bytes()).unwrap();
        };
        sign();
        verify_upgrade_fixture(&out, &public).unwrap();
        assert!(verify_package(&out, &public).is_err());
        let mut manifest: serde_json::Value = fsx::read_json(&out.join("manifest.json")).unwrap();
        manifest["kind"] = "update".into();
        fsx::atomic_write_json(&out.join("manifest.json"), &manifest).unwrap();
        sign();
        assert!(verify_upgrade_fixture(&out, &public).is_err());
    }
}
