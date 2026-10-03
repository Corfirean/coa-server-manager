//! Recovery points: configuration + database dumps stored under `<install>.manager/backups/<id>/`.
//!
//! Rules enforced here: a recovery point only becomes visible once complete (built in a `.partial` folder),
//! every artefact is checksummed, restore never drops anything (the replaced database is kept under a new name)
//! and restoring a database requires the game servers to be stopped.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::{self, Account, Db};
use crate::driver::{self, Verb};
use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::read_ports;
use crate::process::{self, ServiceState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// characters + auth databases, configs, manager metadata.
    Quick,
    /// Quick + the (large) world database.
    Full,
    Config,
    /// characters + auth databases only.
    Database,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Manual,
    Automatic,
    BeforeUpdate,
    BeforeRestore,
    BeforeMigration,
    BeforeBots,
    BeforeRepair,
    BeforeDangerousChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    /// "characters" | "auth" | "world" | "configs"
    pub name: String,
    /// File (databases) or folder (configs), relative to the recovery point.
    pub path: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub tables: Option<usize>,
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPoint {
    pub schema: u32,
    pub id: String,
    pub kind: Kind,
    pub trigger: Trigger,
    pub label: Option<String>,
    pub created_at: String,
    pub manager_version: String,
    pub components: Vec<Component>,
    #[serde(default)]
    pub realm: crate::realms::Mode,
}

fn backups_dir(meta: &Path) -> PathBuf {
    meta.join("backups")
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() < 100 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !id.contains("..")
}

fn point_dir(meta: &Path, id: &str) -> Result<PathBuf> {
    if !id_ok(id) {
        return Err(Error::PathRejected(format!("bad backup id {id:?}")));
    }
    Ok(backups_dir(meta).join(id))
}

pub fn list(meta: &Path) -> Vec<RecoveryPoint> {
    let mut out: Vec<RecoveryPoint> = fs::read_dir(backups_dir(meta))
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| fsx::read_json(&e.path().join("backup.json")).ok()).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

pub fn get(meta: &Path, id: &str) -> Result<RecoveryPoint> {
    fsx::read_json(&point_dir(meta, id)?.join("backup.json")).map_err(|_| Error::Invalid(format!("backup {id} was not found")))
}

/// Files that make up "configuration": everything a user or the Manager may have changed, minus secrets.
pub fn config_files(root: &Path) -> Vec<String> {
    const MAX: u64 = 8 * 1024 * 1024;
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(root, &p, out);
            } else if md.len() <= MAX {
                let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
                if name.ends_with(".bak") || name.ends_with(".orig") || name.ends_with(".old") || name.contains(".bak-") {
                    continue;
                }
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    walk(root, &root.join("Core/configs"), &mut out);
    if let Ok(rd) = fs::read_dir(root.join("Settings")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // database.json holds generated database passwords; it is launcher state, not configuration.
            if e.path().is_file() && (n.contains(".template") || n == "repack.json") {
                out.push(format!("Settings/{n}"));
            }
        }
    }
    if root.join("RELEASE.json").is_file() {
        out.push("RELEASE.json".into());
    }
    walk(root, &root.join("Settings/realm-profiles"), &mut out);
    if root.join("Settings/realm-profile.json").is_file() { out.push("Settings/realm-profile.json".into()); }
    out.sort();
    out
}

fn copy_configs(root: &Path, dir: &Path) -> Result<Component> {
    let files = config_files(root);
    let target = dir.join("files");
    let mut bytes = 0;
    for rel in &files {
        let src = fsx::safe_join(root, rel)?;
        let dst = fsx::safe_join(&target, rel)?;
        fs::create_dir_all(dst.parent().unwrap())?;
        bytes += fs::copy(&src, &dst)?;
    }
    Ok(Component { name: "configs".into(), path: "files".into(), bytes, sha256: None, tables: None, files: Some(files) })
}

/// Make sure the database is up for `f`. If we had to start it (and nothing else is running), stop it again.
pub fn with_database<T>(root: &Path, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    let db = Db::from_repack(root, Account::Admin)?;
    let observed = process::observe(root, &read_ports(root));
    let mysql_was_up = observed.mysql.state == ServiceState::Running && db.ping();
    if !mysql_was_up {
        let out = driver::run(root, Verb::StartMysql)?;
        if !out.ok {
            return Err(Error::Invalid(out.human.map(|h| h.title.to_string()).unwrap_or_else(|| "The database could not be started.".into())));
        }
    }
    let result = f(&db);
    if !mysql_was_up {
        let now = process::observe(root, &read_ports(root));
        if now.world.state == ServiceState::Stopped && now.auth.state == ServiceState::Stopped {
            let _ = driver::run(root, Verb::StopAll);
        }
    }
    result
}

fn wanted_databases(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Quick | Kind::Database => &["characters", "auth"],
        Kind::Full => &["characters", "auth", "world"],
        Kind::Config => &[],
    }
}

/// Create a recovery point. The server may be running: dumps are consistent snapshots (`--single-transaction`).
pub fn create(root: &Path, meta: &Path, kind: Kind, trigger: Trigger, label: Option<String>, progress: &dyn Fn(&str)) -> Result<RecoveryPoint> {
    let id = format!("{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), match trigger {
        Trigger::Manual => "manual",
        Trigger::Automatic => "auto",
        Trigger::BeforeUpdate => "before-update",
        Trigger::BeforeRestore => "before-restore",
        Trigger::BeforeMigration => "before-migration",
        Trigger::BeforeBots => "before-bots",
        Trigger::BeforeRepair => "before-repair",
        Trigger::BeforeDangerousChange => "before-change",
    });
    let final_dir = point_dir(meta, &id)?;
    let partial = backups_dir(meta).join(format!("{id}.partial"));
    fs::create_dir_all(&partial)?;

    let build = || -> Result<Vec<Component>> {
        let mut components = Vec::new();
        let realm = crate::realms::state(root)?;
        let mut databases: Vec<(String, &str)> = wanted_databases(kind).iter()
            .map(|name| Ok((name.to_string(), realm.active.schema(name)?))).collect::<Result<_>>()?;
        if realm.wildcard_created && kind != Kind::Config {
            let other = if realm.active == crate::realms::Mode::Coa { crate::realms::Mode::Wildcard } else { crate::realms::Mode::Coa };
            databases.push((format!("{}-characters", other.name()), other.schema("characters")?));
            if kind == Kind::Full { databases.push((format!("{}-world", other.name()), other.schema("world")?)); }
        }
        for (name, schema) in databases {
            progress(&format!("Backing up {name} database"));
            let component = with_database(root, |db| {
                let db = db.clone().for_realm(crate::realms::Mode::Coa);
                let size = db.schema_bytes(schema)?;
                fsx::require_space(&partial, size / 2 + 32 * 1024 * 1024)?;
                let tables = db.tables(schema)?.len();
                let file = format!("{name}.sql.zst");
                let (bytes, sha) = db.dump_to(schema, &partial.join(&file))?;
                Ok(Component { name, path: file, bytes, sha256: Some(sha), tables: Some(tables), files: None })
            })?;
            components.push(component);
        }
        if kind != Kind::Database {
            progress("Saving configuration");
            components.push(copy_configs(root, &partial)?);
        }
        Ok(components)
    };

    let components = match build() {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_dir_all(&partial); // our own unfinished folder
            return Err(e);
        }
    };
    let point = RecoveryPoint {
        realm: crate::realms::state(root)?.active,
        schema: 1,
        id: id.clone(),
        kind,
        trigger,
        label,
        created_at: chrono::Utc::now().to_rfc3339(),
        manager_version: crate::MANAGER_VERSION.into(),
        components,
    };
    fsx::atomic_write_json(&partial.join("backup.json"), &point)?;
    fs::rename(&partial, &final_dir)?;
    tracing::info!(%id, ?kind, ?trigger, "recovery point created");
    if trigger == Trigger::Automatic {
        prune_automatic(meta, 10);
    }
    Ok(point)
}

/// Keep the newest `keep` automatic recovery points; manual and safety ones are never pruned.
pub fn prune_automatic(meta: &Path, keep: usize) {
    let autos: Vec<_> = list(meta).into_iter().filter(|p| p.trigger == Trigger::Automatic).collect();
    for p in autos.into_iter().skip(keep) {
        if let Ok(dir) = point_dir(meta, &p.id) {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub problems: Vec<String>,
}

pub fn verify(meta: &Path, id: &str) -> Result<VerifyReport> {
    let point = get(meta, id)?;
    let dir = point_dir(meta, id)?;
    let mut problems = Vec::new();
    for c in &point.components {
        let path = dir.join(&c.path);
        match (&c.sha256, &c.files) {
            (Some(sha), _) => match fsx::sha256_file(&path) {
                Ok(actual) if actual.eq_ignore_ascii_case(sha) => {}
                Ok(_) => problems.push(format!("{} is damaged (checksum mismatch)", c.name)),
                Err(_) => problems.push(format!("{} is missing", c.name)),
            },
            (None, Some(files)) => {
                for f in files {
                    if !fsx::safe_join(&path, f).map(|p| p.is_file()).unwrap_or(false) {
                        problems.push(format!("configuration file {f} is missing"));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(VerifyReport { ok: problems.is_empty(), problems })
}

/// Delete one recovery point (its own folder only).
pub fn delete(meta: &Path, id: &str) -> Result<()> {
    let dir = point_dir(meta, id)?;
    get(meta, id)?; // must be a real recovery point
    fs::remove_dir_all(dir)?;
    Ok(())
}

/// Put configuration files back. Files the backup does not contain are left alone. A safety point is taken first.
pub fn restore_configs(root: &Path, meta: &Path, id: &str) -> Result<RecoveryPoint> {
    let point = get(meta, id)?;
    if point.realm != crate::realms::state(root)?.active {
        return Err(Error::Invalid("Select the realm this backup belongs to before restoring it.".into()));
    }
    let comp = point.components.iter().find(|c| c.name == "configs").ok_or_else(|| Error::Invalid("this backup has no configuration".into()))?;
    if !verify(meta, id)?.ok {
        return Err(Error::Invalid("This backup is damaged and cannot be restored.".into()));
    }
    let safety = create(root, meta, Kind::Config, Trigger::BeforeRestore, Some(format!("before restoring {id}")), &|_| {})?;
    let src_root = point_dir(meta, id)?.join(&comp.path);
    for rel in comp.files.as_deref().unwrap_or_default() {
        let dst = fsx::ensure_within(root, &fsx::safe_join(root, rel)?)?;
        fsx::atomic_write(&dst, &fs::read(fsx::safe_join(&src_root, rel)?)?)?;
    }
    Ok(safety)
}

#[derive(Debug, Serialize)]
pub struct DbRestore {
    /// The database the restore replaced, kept intact under this name.
    pub previous_schema: String,
    pub safety_backup: String,
    pub tables_restored: usize,
}

/// Restore one database from a recovery point without dropping anything:
/// import into a staging schema, sanity-check it, then swap tables atomically and keep the old schema.
pub fn restore_database(root: &Path, meta: &Path, id: &str, name: &str) -> Result<DbRestore> {
    let point = get(meta, id)?;
    if point.realm != crate::realms::state(root)?.active {
        return Err(Error::Invalid("Select the realm this backup belongs to before restoring it.".into()));
    }
    let comp = point.components.iter().find(|c| c.name == name && c.sha256.is_some()).ok_or_else(|| Error::Invalid(format!("this backup has no {name} database")))?;
    if !verify(meta, id)?.ok {
        return Err(Error::Invalid("This backup is damaged and cannot be restored.".into()));
    }
    let now = process::observe(root, &read_ports(root));
    if now.world.state != ServiceState::Stopped || now.auth.state != ServiceState::Stopped {
        return Err(Error::Invalid("Stop the server before restoring a database.".into()));
    }
    let live = if name.contains('-') { db::schema_of(name)? } else { point.realm.schema(name)? };
    let expected_tables = comp.tables.unwrap_or(0);
    let dump = point_dir(meta, id)?.join(&comp.path);
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();

    // 1. Safety copy of the current state, so even a wrong restore is reversible.
    let safety = create(root, meta, Kind::Database, Trigger::BeforeRestore, Some(format!("before restoring {name} from {id}")), &|_| {})?;

    with_database(root, |db| {
        let db = db.clone().for_realm(crate::realms::Mode::Coa);
        let staging = format!("{live}_restore_{stamp}");
        let old = format!("{live}_before_restore_{stamp}");
        if db.schema_exists(&staging)? || db.schema_exists(&old)? {
            return Err(Error::Invalid("A previous restore left its work schemas behind; try again in a moment.".into()));
        }
        // 2. Import into staging.
        db.query(&format!("CREATE DATABASE `{staging}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"))?;
        db.import_from(&staging, &dump)?;
        // 3. Sanity checks.
        let staged = db.tables(&staging)?;
        if staged.is_empty() || (expected_tables > 0 && staged.len() != expected_tables) {
            return Err(Error::Invalid(format!("The restored copy looks incomplete ({} of {expected_tables} tables); nothing was changed.", staged.len())));
        }
        if db.extra_objects(&staging)? > 0 || db.extra_objects(live)? > 0 {
            return Err(Error::Invalid("This database contains routines, triggers or views; automatic restore does not support that. Nothing was changed.".into()));
        }
        // 4. Atomic swap: live tables move to the `before_restore` schema, staged tables move into place.
        let current = db.tables(live)?;
        db.query(&format!("CREATE DATABASE `{old}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"))?;
        let mut renames = Vec::new();
        for t in &current {
            renames.push(format!("`{live}`.`{t}` TO `{old}`.`{t}`"));
        }
        for t in &staged {
            renames.push(format!("`{staging}`.`{t}` TO `{live}`.`{t}`"));
        }
        db.query(&format!("RENAME TABLE {};", renames.join(", ")))?;
        Ok(DbRestore { previous_schema: old, safety_backup: safety.id.clone(), tables_restored: staged.len() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("srv");
        let meta = dir.path().join("srv.manager");
        crate::layout::testkit::fake_repack(&root);
        fs::write(root.join("Settings/database.json"), r#"{"rootPassword":"secret-root","appPassword":"secret-app"}"#).unwrap_or_else(|_| {
            fs::create_dir_all(root.join("Settings")).unwrap();
            fs::write(root.join("Settings/database.json"), "{}").unwrap();
        });
        fs::create_dir_all(&meta).unwrap();
        (dir, root, meta)
    }

    #[test]
    fn config_backup_excludes_secrets_and_clutter_and_restores_byte_exact() {
        let (_d, root, meta) = setup();
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(root.join("Settings/database.json"), r#"{"rootPassword":"x"}"#).unwrap();
        fs::write(root.join("Settings/worldserver.conf.template"), "A = 1\n").unwrap();
        fs::write(root.join("Core/configs/modules/old.conf.bak"), "junk").unwrap();
        let files = config_files(&root);
        assert!(files.contains(&"Core/configs/worldserver.conf".to_string()));
        assert!(files.contains(&"Settings/worldserver.conf.template".to_string()));
        assert!(!files.iter().any(|f| f.contains("database.json")), "secrets excluded");
        assert!(!files.iter().any(|f| f.ends_with(".bak")));

        let p = create(&root, &meta, Kind::Config, Trigger::Manual, Some("test".into()), &|_| {}).unwrap();
        assert!(verify(&meta, &p.id).unwrap().ok);
        assert_eq!(list(&meta).len(), 1);
        assert!(!backups_dir(&meta).join(format!("{}.partial", p.id)).exists());

        let original = fs::read(root.join("Core/configs/worldserver.conf")).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), "[worldserver]\nRealmID = 99\n").unwrap();
        fs::write(root.join("Core/configs/unrelated_new.conf"), "keep").unwrap();
        let safety = restore_configs(&root, &meta, &p.id).unwrap();
        assert_eq!(fs::read(root.join("Core/configs/worldserver.conf")).unwrap(), original);
        assert!(root.join("Core/configs/unrelated_new.conf").is_file(), "restore never deletes other files");
        assert_eq!(safety.trigger, Trigger::BeforeRestore);
        assert!(list(&meta).len() >= 2);
    }

    #[test]
    fn verify_detects_tampering_and_restore_refuses_damaged_backup() {
        let (_d, root, meta) = setup();
        let p = create(&root, &meta, Kind::Config, Trigger::Manual, None, &|_| {}).unwrap();
        let victim = point_dir(&meta, &p.id).unwrap().join("files/Core/configs/worldserver.conf");
        fs::remove_file(&victim).unwrap();
        let v = verify(&meta, &p.id).unwrap();
        assert!(!v.ok && v.problems[0].contains("worldserver.conf"));
        assert!(restore_configs(&root, &meta, &p.id).is_err());
    }

    #[test]
    fn ids_are_validated_and_delete_only_touches_real_backups() {
        let (d, root, meta) = setup();
        for bad in ["", "..", "../x", "a/b", "a\\b"] {
            assert!(point_dir(&meta, bad).is_err(), "{bad:?}");
        }
        let victim = d.path().join("srv.manager/important");
        fs::create_dir_all(&victim).unwrap();
        assert!(delete(&meta, "important").is_err(), "not a recovery point");
        assert!(victim.exists());
        let p = create(&root, &meta, Kind::Config, Trigger::Manual, None, &|_| {}).unwrap();
        delete(&meta, &p.id).unwrap();
        assert!(list(&meta).is_empty());
    }

    #[test]
    fn automatic_points_are_pruned_but_manual_and_safety_are_kept() {
        let (_d, root, meta) = setup();
        let mk = |t: Trigger, id: &str| {
            let mut p = create(&root, &meta, Kind::Config, t, None, &|_| {}).unwrap();
            // give each a distinct, sortable id by renaming its folder
            let from = point_dir(&meta, &p.id).unwrap();
            p.id = id.to_string();
            fsx::atomic_write_json(&from.join("backup.json"), &p).unwrap();
            fs::rename(from, backups_dir(&meta).join(id)).unwrap();
        };
        mk(Trigger::Manual, "20260101-000000-manual");
        mk(Trigger::BeforeRestore, "20260102-000000-before-restore");
        for i in 0..5 {
            mk(Trigger::Automatic, &format!("2026020{i}-000000-auto"));
        }
        prune_automatic(&meta, 2);
        let left: Vec<String> = list(&meta).into_iter().map(|p| p.id).collect();
        assert_eq!(left.iter().filter(|i| i.ends_with("-auto")).count(), 2);
        assert!(left.contains(&"20260101-000000-manual".to_string()));
        assert!(left.contains(&"20260102-000000-before-restore".to_string()));
        assert!(left.contains(&"20260204-000000-auto".to_string()), "newest automatic kept");
    }
}
