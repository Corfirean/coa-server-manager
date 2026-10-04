//! Repair official files and pending SQL from signed packages without replacing player databases or settings.
use std::{collections::BTreeMap, fs, path::Path};
use serde::Serialize;
use crate::{Error, Result, fsx, manifest::{Manifest, Kind, FileEntry, ReplacePolicy}, pkgsource::{self, Source}, registry::{InstallMeta, MetaDir}, update::{Env, RepackEnv}, download::Cancel};

#[derive(Debug, Serialize)]
pub struct DatabaseCheck {
    pub realm: crate::realms::Mode,
    pub migrations: Vec<crate::migrations::Item>,
    pub problems: Vec<crate::schema_check::Problem>,
    pub full_schema: bool,
}

fn installed_manifest(meta: &InstallMeta, dir: &Path) -> Result<Manifest> {
    let version = meta.core.version.as_deref().ok_or_else(|| Error::Invalid("This imported server has no verified package version. Install an official server package first.".into()))?;
    let update = dir.join("manifests").join(format!("update-{version}.json"));
    let bytes = fs::read(if update.exists() { update } else { dir.join("manifests/base.json") })?;
    let m = Manifest::parse(&bytes)?;
    if m.version != version { return Err(Error::Invalid("The installed package manifest is missing. Update the server to restore its package information.".into())); }
    Ok(m)
}

pub fn check(root: &Path, dir: &Path) -> Result<Vec<DatabaseCheck>> {
    let (_, meta) = MetaDir::open(dir)?;
    let m = installed_manifest(&meta, dir)?;
    let checks = crate::backup::with_database(root, |db| {
        let mut modes = vec![crate::realms::Mode::Coa];
        if crate::realms::state(root)?.wildcard_created { modes.push(crate::realms::Mode::Wildcard); }
        let mut checks = Vec::new();
        for mode in modes {
            let db = db.clone().for_realm(mode);
            let migrations: Vec<_> = m.migrations.iter().filter(|m| mode == crate::realms::Mode::Coa || m.db != "auth").cloned().collect();
            checks.push(DatabaseCheck { realm: mode, migrations: crate::migrations::status(&db, &migrations)?, problems: crate::schema_check::check(&db, root)?, full_schema: root.join(crate::schema_check::CONTRACT).exists() });
        }
        Ok(checks)
    })?;
    fsx::atomic_write_json(&dir.join("logs/database-checks.json"), &checks)?;
    Ok(checks)
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub restored: Vec<String>,
    pub applied: Vec<String>,
    pub backup: String,
    pub database: Vec<DatabaseCheck>,
    pub error: Option<String>,
}

fn repairable(f: &FileEntry) -> bool {
    let path = f.path.to_lowercase();
    f.owner != crate::manifest::Owner::User && matches!(f.policy, ReplacePolicy::Replace | ReplacePolicy::ReplaceIfPristine)
        && !path.starts_with("settings/") && !path.starts_with("core/configs/")
        && !path.starts_with("mysql/data/") && !path.starts_with("_migrations/")
}

fn restore_files(root: &Path, work: &Path, before: &Path, broken: &[(&FileEntry, bool)]) -> Result<Vec<String>> {
    let mut plan = Vec::new();
    // Back up every target before changing the first one, and retain a crash-recovery inventory.
    for (entry, _) in broken {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &entry.path)?)?;
        let existed = target.is_file();
        if existed {
            let saved = fsx::safe_join(before, &entry.path)?;
            fs::create_dir_all(saved.parent().unwrap())?;
            fs::copy(&target, saved)?;
        }
        plan.push((entry.path.clone(), existed));
    }
    fsx::atomic_write_json(&before.join("files.json"), &plan)?;
    let mut restored = Vec::new();
    let result: Result<()> = (|| {
        for (entry, latest) in broken {
            let target = fsx::ensure_within(root, &fsx::safe_join(root, &entry.path)?)?;
            let source = fsx::safe_join(&work.join(if *latest { "update" } else { "base" }), &entry.path)?;
            fsx::atomic_write(&target, &fs::read(&source)?)?;
            restored.push(entry.path.clone());
        }
        Ok(())
    })();
    if let Err(e) = result {
        for (path, existed) in &plan {
            let target = fsx::ensure_within(root, &fsx::safe_join(root, path)?)?;
            if *existed { fsx::atomic_write(&target, &fs::read(fsx::safe_join(before, path)?)?)?; }
            else if target.is_file() { fs::remove_file(target)?; }
        }
        return Err(e);
    }
    Ok(restored)
}

pub fn run(root: &Path, dir: &Path, base_source: &Source, update_source: &Source, key: &str, progress: &dyn Fn(&str, u8)) -> Result<Report> {
    let env = RepackEnv { root, meta_dir: dir };
    env.ensure_stopped()?;
    if crate::update::unfinished(dir).is_some() { return Err(Error::Invalid("Resolve the unfinished update before repairing the server.".into())); }
    let (_, mut meta) = MetaDir::open(dir)?;
    progress("Checking signed packages", 2);
    let (update, _) = pkgsource::fetch_manifest(update_source, key)?;
    if update.kind != Kind::Update || !update.compatible_with_manager(crate::MANAGER_VERSION) || meta.core.version.as_deref() != Some(&update.version) {
        return Err(Error::Invalid("Update the server to the latest compatible package before repairing it. Repair never changes its version.".into()));
    }
    let (base, _) = pkgsource::fetch_manifest(base_source, key)?;
    let installed_base = Manifest::parse(&fs::read(dir.join("manifests/base.json"))?)?;
    if base.kind != Kind::Base || base.version != installed_base.version || base.core.commit != installed_base.core.commit {
        return Err(Error::Invalid("The matching base package is no longer available. Your server was left unchanged.".into()));
    }
    let work = dir.join("staging").join(format!("repair-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| {
        let mut files: BTreeMap<String, (&FileEntry, bool)> = base.files.iter().filter(|f| repairable(f)).map(|f| (f.path.clone(), (f, false))).collect();
        for f in update.files.iter().filter(|f| repairable(f)) { files.insert(f.path.clone(), (f, true)); }
        let mut broken = Vec::new();
        for (path, (entry, latest)) in files {
            let target = fsx::ensure_within(root, &fsx::safe_join(root, &path)?)?;
            if fsx::sha256_file(&target).ok().as_deref() != Some(&entry.sha256) { broken.push((entry, latest)); }
        }
        let cancel = Cancel::default();
        let update_parts = pkgsource::fetch_parts(update_source, &update, &dir.join("staging/download"), &cancel, &|f,_| progress("Downloading repair files", 5 + (f * 30.0) as u8))?;
        crate::package::extract(&update_parts, &update, &work.join("update"), &|_,_| {})?;
        if broken.iter().any(|(_,latest)| !latest) {
            let base_parts = pkgsource::fetch_parts(base_source, &base, &dir.join("staging/repair-base-download"), &cancel, &|f,_| progress("Downloading base files", 35 + (f * 25.0) as u8))?;
            crate::package::extract(&base_parts, &base, &work.join("base"), &|_,_| {})?;
        }
        // Verify all sources before taking a backup or replacing any target.
        for (entry, latest) in &broken {
            let src = fsx::safe_join(&work.join(if *latest { "update" } else { "base" }), &entry.path)?;
            if fsx::sha256_file(&src)? != entry.sha256 { return Err(Error::Invalid("A repair file failed verification.".into())); }
        }
        progress("Backing up before repair", 65);
        let backup = crate::backup::create(root, dir, crate::backup::Kind::Full, crate::backup::Trigger::Manual, Some("before repair".into()), &|_| {})?.id;
        let before = dir.join("repairs").join(&backup);
        fs::create_dir_all(&before)?;
        fsx::require_space(root, broken.iter().map(|(e,_)| e.size.saturating_mul(2)).sum())?;
        let restored = restore_files(root, &work, &before, &broken)?;
        for (entry, _) in &broken {
            meta.original_hashes.insert(entry.path.clone(), entry.sha256.clone());
        }
        fsx::atomic_write_json(&dir.join("install.json"), &meta)?;
        progress("Repairing pending database updates", 80);
        let (applied, error) = match env.migrate(&update, &work.join("update/_migrations")) {
            Ok(r) => (r.applied, r.failed.map(|(id,why)| format!("{id}: {why}"))),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        crate::config::create_missing_module_configs(root)?;
        progress("Checking database after repair", 95);
        let database = check(root, dir)?;
        let report = Report { restored, applied, backup, database, error };
        fsx::atomic_write_json(&before.join("report.json"), &report)?;
        progress("Done", 100);
        Ok(report)
    })();
    // Only the UUID staging directory created above is discarded; backups are retained.
    let _ = fs::remove_dir_all(work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repair_never_replaces_player_data_or_settings() {
        for path in ["mysql/data/acore_characters/characters.ibd", "Settings/repack.json", "Core/configs/worldserver.conf", "_migrations/world/x.sql"] {
            let entry = FileEntry { path: path.into(), sha256: "a".repeat(64), size: 1, owner: crate::manifest::Owner::Core, policy: ReplacePolicy::Replace };
            assert!(!repairable(&entry));
        }
    }

    #[test]
    fn file_failure_restores_every_original_and_keeps_recovery_inventory() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("server");
        let work = d.path().join("work");
        let before = d.path().join("before");
        fsx::atomic_write(&root.join("Core/a.dll"), b"original").unwrap();
        fsx::atomic_write(&work.join("update/Core/a.dll"), b"repaired").unwrap();
        let entries: Vec<_> = ["Core/a.dll", "Core/missing.dll"].into_iter().map(|path| FileEntry { path: path.into(), sha256: "a".repeat(64), size: 1, owner: crate::manifest::Owner::Core, policy: ReplacePolicy::Replace }).collect();
        assert!(restore_files(&root, &work, &before, &[(&entries[0], true), (&entries[1], true)]).is_err());
        assert_eq!(fs::read(root.join("Core/a.dll")).unwrap(), b"original");
        assert!(!root.join("Core/missing.dll").exists());
        assert!(before.join("files.json").is_file());
    }
}
