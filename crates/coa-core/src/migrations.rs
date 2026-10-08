//! Tracked SQL migrations. The core's own database updater is disabled in this project, so schema changes
//! shipped with a release are applied here, one file at a time, recorded in a ledger table, and never twice.

use std::path::Path;

use serde::Serialize;

use crate::db::{self, Db};
use crate::error::{Error, Result};
use crate::fsx;
use crate::manifest::Migration;

const LEDGER_SCHEMA: &str = "acore_world";
const LEDGER_TABLE: &str = "coa_manager_migrations";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Applied,
    Pending,
    Failed,
    Running,
}

#[derive(Debug, Clone)]
pub struct LedgerRow {
    pub db: String,
    pub id: String,
    pub sha256: String,
    pub status: Status,
    pub error: Option<String>,
    pub baseline: bool,
}

/// Storage for the ledger and the ability to run a SQL file; implemented by the real database and by tests.
pub trait Store {
    fn load(&self) -> Result<Vec<LedgerRow>>;
    fn load_readonly(&self) -> Result<Vec<LedgerRow>> { self.load() }
    fn put(&self, row: &LedgerRow) -> Result<()>;
    fn run_file(&self, schema: &str, file: &Path) -> Result<()>;
}

fn hex_of(s: &str) -> String {
    hex::encode(s.as_bytes())
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 150 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

impl Store for Db {
    fn load(&self) -> Result<Vec<LedgerRow>> {
        self.query(&format!(
            "CREATE TABLE IF NOT EXISTS `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` (`db` VARCHAR(32) NOT NULL, `id` VARCHAR(190) NOT NULL, `sha256` CHAR(64) NOT NULL, `status` VARCHAR(16) NOT NULL, `applied_at` DATETIME NOT NULL, `error` TEXT NULL, `baseline` TINYINT NOT NULL DEFAULT 0, PRIMARY KEY (`db`,`id`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"
        ))?;
        self.load_readonly()
    }

    fn load_readonly(&self) -> Result<Vec<LedgerRow>> {
        let out = if self.tables(LEDGER_SCHEMA)?.iter().any(|name| name == LEDGER_TABLE) {
            self.query(&format!("SELECT `db`,`id`,`sha256`,`status`,`baseline`,COALESCE(HEX(`error`),'') FROM `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` ORDER BY `db`,`id`;"))?
        } else { String::new() };
        let mut rows = Vec::new();
        // Files the core's own updater (or whoever prepared this database) already recorded count as applied.
        for (kind, schema) in db::SCHEMAS {
            if self.tables(schema)?.iter().any(|name| name == "updates") {
                let names = self.query(&format!("SELECT name FROM `{schema}`.`updates`;"))?;
                for name in names.lines().filter(|n| n.ends_with(".sql")) {
                    rows.push(LedgerRow { db: kind.into(), id: name.trim_end_matches(".sql").into(), sha256: "0".repeat(64), status: Status::Applied, error: None, baseline: true });
                }
            }
        }
        for line in out.lines().filter(|l| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 5 {
                return Err(Error::Invalid("The database migration history is malformed; no SQL was replayed.".into()));
            }
            if !matches!(f[3], "applied" | "failed" | "running") || !ident_ok(f[1]) || f[2].len() != 64 || !f[2].chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(Error::Invalid("The database migration history is invalid; no SQL was replayed.".into()));
            }
            let err = f.get(5).filter(|h| !h.is_empty()).and_then(|h| hex::decode(h).ok()).map(|b| String::from_utf8_lossy(&b).into_owned());
            rows.push(LedgerRow {
                db: f[0].into(),
                id: f[1].into(),
                sha256: f[2].into(),
                status: match f[3] { "applied" => Status::Applied, "running" => Status::Running, _ => Status::Failed },
                baseline: f[4] == "1",
                error: err,
            });
        }
        Ok(rows)
    }

    fn put(&self, r: &LedgerRow) -> Result<()> {
        if !ident_ok(&r.id) || !ident_ok(&r.db) || r.sha256.len() != 64 || !r.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Invalid("migration record has an invalid identifier".into()));
        }
        let status = match r.status { Status::Applied => "applied", Status::Running => "running", _ => "failed" };
        let err = r.error.as_deref().map(|e| format!("UNHEX('{}')", hex_of(e))).unwrap_or_else(|| "NULL".into());
        self.query(&format!(
            "REPLACE INTO `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` (`db`,`id`,`sha256`,`status`,`applied_at`,`error`,`baseline`) VALUES ('{}','{}','{}','{status}',NOW(),{err},{});",
            r.db, r.id, r.sha256, r.baseline as u8
        ))?;
        Ok(())
    }

    fn run_file(&self, schema: &str, file: &Path) -> Result<()> {
        self.run_sql_file(schema, file)
    }
}

/// Statements that can lose data. Detection is deliberately conservative: a false positive only costs a backup.
pub fn looks_destructive(sql: &str) -> bool {
    let upper: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--") && !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase();
    ["DROP TABLE", "DROP DATABASE", "DROP SCHEMA", "TRUNCATE", "DELETE FROM", "DROP COLUMN", "DROP INDEX", "DROP PRIMARY", "DROP FOREIGN"].iter().any(|k| upper.contains(k))
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: String,
    pub db: String,
    pub status: Status,
    pub destructive: bool,
    pub baseline: bool,
    pub error: Option<String>,
}

pub fn status(store: &dyn Store, list: &[Migration]) -> Result<Vec<Item>> {
    let rows = store.load()?;
    Ok(list
        .iter()
        .map(|m| {
            // Manager records are appended after the core's updates table and are authoritative.
            let row = recorded(&rows, m);
            let changed = row.is_some_and(|r| hash_changed(r, m));
            Item {
                id: m.id.clone(),
                db: m.db.clone(),
                status: if changed || row.is_some_and(|r| r.status == Status::Running) { Status::Failed } else { row.map(|r| r.status).unwrap_or(Status::Pending) },
                destructive: m.destructive,
                baseline: row.map(|r| r.baseline).unwrap_or(false),
                error: if changed { Some("An already applied SQL file changed. Publish a new migration instead of replaying it.".into()) } else { row.and_then(|r| r.error.clone()) },
            }
        })
        .collect())
}

fn recorded<'a>(rows: &'a [LedgerRow], m: &Migration) -> Option<&'a LedgerRow> {
    rows.iter().rev().find(|r| r.db == m.db && r.id == m.id)
}

/// Migrations that were published with different bytes but have the same effect, so a server that applied either
/// version accepts the other. The Wildcard table repair went out as `c1d8…` (0.261005.0, 0.261006.1) and as `9e4a…`
/// (0.261005.22, 0.261007.24); the packages carry no list of compatible versions, so the Manager knows this pair.
const SAME_EFFECT: &[(&str, &[&str])] = &[(
    "manager_repair__20261005_missing_wildcard_tables",
    &[
        "c1d8dbf2271234283a48a39e4b4aea1106b83b70581f1e3c4aa9fff438e8e4d2",
        "9e4a36245c3f83255415c122c01f08ceb3e3b83898d3d427ac4e2fbcf61e9638",
    ],
)];

fn same_effect(id: &str, a: &str, b: &str) -> bool {
    SAME_EFFECT.iter().any(|(known, hashes)| *known == id
        && hashes.iter().any(|h| h.eq_ignore_ascii_case(a)) && hashes.iter().any(|h| h.eq_ignore_ascii_case(b)))
}

fn hash_changed(row: &LedgerRow, m: &Migration) -> bool {
    row.status == Status::Applied && row.sha256 != "0".repeat(64) && !row.sha256.eq_ignore_ascii_case(&m.sha256)
        && !m.compatible_sha256.iter().any(|hash| hash.eq_ignore_ascii_case(&row.sha256))
        && !same_effect(&m.id, &row.sha256, &m.sha256)
}

/// Inspect recorded history without creating a ledger or executing migrations.
pub fn preflight(store: &dyn Store, list: &[Migration]) -> Result<()> {
    check_history(&store.load_readonly()?, list)
}

pub fn pending_count(store: &dyn Store, list: &[Migration]) -> Result<usize> {
    let rows = store.load_readonly()?;
    check_history(&rows, list)?;
    Ok(list.iter().filter(|m| recorded(&rows, m).is_none_or(|r| r.status != Status::Applied)).count())
}

fn verified_file(m: &Migration, dir: &Path) -> Result<(&'static str, std::path::PathBuf, bool)> {
    if !ident_ok(&m.id) { return Err(Error::InvalidManifest(format!("migration id {:?} is not allowed", m.id))); }
    let schema = db::schema_of(&m.db)?;
    let path = fsx::safe_join(dir, &format!("{}/{}.sql", m.db, m.id))?;
    let bytes = std::fs::read(&path).map_err(|_| Error::Invalid(format!("migration file {} is missing", m.id)))?;
    let actual = fsx::sha256_bytes(&bytes);
    if !actual.eq_ignore_ascii_case(&m.sha256) { return Err(Error::HashMismatch { path: path.display().to_string(), expected: m.sha256.clone(), actual }); }
    Ok((schema, path, m.destructive || looks_destructive(&String::from_utf8_lossy(&bytes))))
}

/// Check every migration artifact, including applied history, before replacing server files.
pub fn verify_files(list: &[Migration], dir: &Path) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for m in list {
        if !seen.insert((&m.db, &m.id)) { return Err(Error::InvalidManifest(format!("duplicate migration {} ({})", m.id, m.db))); }
        verified_file(m, dir)?;
    }
    Ok(())
}

fn check_history(rows: &[LedgerRow], list: &[Migration]) -> Result<()> {
    for m in list {
        if recorded(rows, m).is_some_and(|r| matches!(r.status, Status::Running | Status::Failed)) {
            return Err(Error::Invalid(format!("Database update {} ({}) failed or was interrupted and may be partially applied. Restore its recovery point before retrying.", m.id, m.db)));
        }
        if recorded(rows, m).is_some_and(|r| hash_changed(r, m)) {
            return Err(Error::Invalid(format!("Applied database update {} ({}) has a different checksum. A new migration is required; it was not replayed.", m.id, m.db)));
        }
    }
    Ok(())
}

/// Record migrations as already present without running them (adopting an existing server whose database
/// was brought up to date by hand).
pub fn baseline(store: &dyn Store, list: &[Migration]) -> Result<usize> {
    let rows = store.load()?;
    let mut n = 0;
    for m in list {
        if rows.iter().any(|r| r.db == m.db && r.id == m.id && r.status == Status::Applied) {
            continue;
        }
        store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Applied, error: None, baseline: true })?;
        n += 1;
    }
    Ok(n)
}

#[derive(Debug, Serialize)]
pub struct ApplyReport {
    pub applied: Vec<String>,
    pub failed: Option<(String, String)>,
    pub snapshot: Option<String>,
}

/// Apply every pending migration in list order, stopping at the first failure. `snapshot` runs once, before the
/// first pending migration, when any of them is (or looks) destructive.
pub fn apply_pending(store: &dyn Store, list: &[Migration], dir: &Path, snapshot: &dyn Fn() -> Result<String>) -> Result<ApplyReport> {
    let rows = store.load()?;
    // The list order is the release's order of application (released updates, then pending, then modules).
    check_history(&rows, list)?;
    let todo: Vec<&Migration> = list.iter().filter(|m| recorded(&rows, m).is_none_or(|r| r.status != Status::Applied)).collect();
    tracing::info!(total = list.len(), pending = todo.len(), skipped = list.len() - todo.len(), "database migration plan");

    // Resolve and verify every file before running anything.
    let mut files = Vec::new();
    let mut any_destructive = false;
    for m in &todo {
        let (schema, path, destructive) = verified_file(m, dir)?;
        any_destructive |= destructive;
        files.push((*m, schema, path));
    }

    let mut report = ApplyReport { applied: Vec::new(), failed: None, snapshot: None };
    if files.is_empty() {
        return Ok(report);
    }
    if any_destructive {
        report.snapshot = Some(snapshot().map_err(|e| Error::Invalid(format!("No database update was applied because the safety backup failed: {e}")))?);
    }
    for (m, schema, path) in files {
        tracing::info!(migration = %m.id, database = %m.db, "database migration starting");
        store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Running, error: Some("Interrupted SQL must be recovered before replay.".into()), baseline: false })?;
        match store.run_file(schema, &path) {
            Ok(()) => {
                store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Applied, error: None, baseline: false })?;
                report.applied.push(m.id.clone());
                tracing::info!(migration = %m.id, database = %m.db, "database migration applied");
            }
            Err(e) => {
                tracing::error!(migration = %m.id, database = %m.db, "database migration failed; details saved in migration history");
                let msg = e.to_string();
                let _ = store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Failed, error: Some(msg.clone()), baseline: false });
                report.failed = Some((m.id.clone(), msg));
                break;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_two_published_versions_of_the_wildcard_table_repair_are_interchangeable() {
        let (old, new) = ("c1d8dbf2271234283a48a39e4b4aea1106b83b70581f1e3c4aa9fff438e8e4d2", "9e4a36245c3f83255415c122c01f08ceb3e3b83898d3d427ac4e2fbcf61e9638");
        let id = "manager_repair__20261005_missing_wildcard_tables";
        let row = |id: &str, sha: &str| LedgerRow { db: "characters".into(), id: id.into(), sha256: sha.into(), status: Status::Applied, error: None, baseline: false };
        let migration = |id: &str, sha: &str| Migration { compatible_sha256: vec![], id: id.into(), db: "characters".into(), sha256: sha.into(), destructive: false };
        assert!(!hash_changed(&row(id, old), &migration(id, new)), "applied as c1d8, offered as 9e4a");
        assert!(!hash_changed(&row(id, new), &migration(id, old)), "applied as 9e4a, offered as c1d8");
        assert!(hash_changed(&row(id, &"a".repeat(64)), &migration(id, new)), "an unknown version is still a change");
        assert!(hash_changed(&row("other_migration", old), &migration("other_migration", new)), "only the known pair is exempt");
    }

    #[derive(Default)]
    struct Mem {
        rows: RefCell<Vec<LedgerRow>>,
        ran: RefCell<Vec<String>>,
        fail_on: Option<&'static str>,
    }

    impl Store for Mem {
        fn load(&self) -> Result<Vec<LedgerRow>> {
            Ok(self.rows.borrow().clone())
        }
        fn put(&self, r: &LedgerRow) -> Result<()> {
            let mut rows = self.rows.borrow_mut();
            rows.retain(|x| !(x.db == r.db && x.id == r.id));
            rows.push(r.clone());
            Ok(())
        }
        fn run_file(&self, _schema: &str, file: &Path) -> Result<()> {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            if self.fail_on == Some(Box::leak(name.clone().into_boxed_str())) {
                return Err(Error::Invalid("syntax error near 'FOO'".into()));
            }
            self.ran.borrow_mut().push(name);
            Ok(())
        }
    }

    fn setup(files: &[(&str, &str, &str)]) -> (tempfile::TempDir, Vec<Migration>) {
        let d = tempfile::tempdir().unwrap();
        let mut list = Vec::new();
        for (db, id, sql) in files {
            std::fs::create_dir_all(d.path().join(db)).unwrap();
            std::fs::write(d.path().join(db).join(format!("{id}.sql")), sql).unwrap();
            list.push(Migration { compatible_sha256: vec![], id: id.to_string(), db: db.to_string(), sha256: fsx::sha256_bytes(sql.as_bytes()), destructive: false });
        }
        (d, list)
    }

    fn no_snapshot() -> Result<String> {
        panic!("no snapshot expected")
    }

    #[test]
    fn manager_failure_overrides_an_old_core_update_record() {
        let (d, list) = setup(&[("characters", "same_id", "SELECT 1;")]);
        let store = Mem::default();
        let old = LedgerRow { db: "characters".into(), id: "same_id".into(), sha256: "0".repeat(64), status: Status::Applied, error: None, baseline: true };
        let failed = LedgerRow { sha256: list[0].sha256.clone(), status: Status::Failed, error: Some("previous failure".into()), baseline: false, ..old.clone() };
        store.rows.borrow_mut().extend([old, failed]);
        assert_eq!(status(&store, &list).unwrap()[0].status, Status::Failed);
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).unwrap_err().to_string().contains("partially applied"));
        assert!(store.ran.borrow().is_empty());
    }

    #[test]
    fn changed_applied_sql_is_reported_and_never_replayed() {
        let (d, list) = setup(&[("characters", "same_id", "SELECT 1;")]);
        let store = Mem::default();
        store.rows.borrow_mut().push(LedgerRow { db: "characters".into(), id: "same_id".into(), sha256: "a".repeat(64), status: Status::Applied, error: None, baseline: false });
        assert_eq!(status(&store, &list).unwrap()[0].status, Status::Failed);
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).is_err());
        assert!(store.ran.borrow().is_empty());
    }

    #[test]
    fn staged_files_reject_missing_corrupt_duplicate_and_invalid_migrations() {
        let (dir, list) = setup(&[("characters", "repair", "SELECT 1;\n")]);
        verify_files(&list, dir.path()).unwrap();
        let mut bad = list.clone();
        bad[0].id = "../outside".into();
        assert!(verify_files(&bad, dir.path()).is_err());
        bad = list.clone();
        bad.push(list[0].clone());
        assert!(verify_files(&bad, dir.path()).is_err());
        std::fs::write(dir.path().join("characters/repair.sql"), b"SELECT 2;\n").unwrap();
        assert!(verify_files(&list, dir.path()).is_err());
        std::fs::remove_file(dir.path().join("characters/repair.sql")).unwrap();
        assert!(verify_files(&list, dir.path()).is_err());
    }

    #[test]
    fn preflight_uses_readonly_history_and_rejects_conflicts_without_writes() {
        struct ReadOnly<'a>(&'a Mem);
        impl Store for ReadOnly<'_> {
            fn load(&self) -> Result<Vec<LedgerRow>> { panic!("preflight must use readonly access") }
            fn load_readonly(&self) -> Result<Vec<LedgerRow>> { self.0.load() }
            fn put(&self, _: &LedgerRow) -> Result<()> { panic!("preflight must not rewrite history") }
            fn run_file(&self, _: &str, _: &Path) -> Result<()> { panic!("preflight must not run SQL") }
        }
        let (_d, list) = setup(&[("characters", "repair", "SELECT 1;\n")]);
        for state in [Status::Applied, Status::Running, Status::Failed] {
            let store = Mem::default();
            store.rows.borrow_mut().push(LedgerRow { db: "characters".into(), id: "repair".into(), sha256: "a".repeat(64), status: state, error: None, baseline: false });
            assert!(preflight(&ReadOnly(&store), &list).is_err());
            assert_eq!(store.rows.borrow()[0].sha256, "a".repeat(64));
            assert_eq!(store.rows.borrow()[0].status, state);
        }
    }

    #[test]
    fn published_compatibility_preserves_history_and_never_replays_sql() {
        let (d, mut list) = setup(&[("characters", "repair", "SELECT 1;\n")]);
        let store = Mem::default();
        let old = fsx::sha256_bytes(b"SELECT 1;\r\n");
        store.rows.borrow_mut().push(LedgerRow { db: "characters".into(), id: "repair".into(), sha256: old.clone(), status: Status::Applied, error: None, baseline: false });
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).is_err());
        list[0].compatible_sha256.push(old.clone());
        assert_eq!(status(&store, &list).unwrap()[0].status, Status::Applied);
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).unwrap().applied.is_empty());
        assert!(store.ran.borrow().is_empty());
        assert_eq!(store.rows.borrow()[0].sha256, old);
        store.rows.borrow_mut()[0].status = Status::Running;
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).is_err());
        store.rows.borrow_mut()[0].status = Status::Failed;
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).is_err());
        assert!(store.ran.borrow().is_empty());
    }

    #[test]
    fn applies_in_order_once_and_records_the_ledger() {
        let (d, list) = setup(&[("world", "2026_09_01_00_a", "CREATE TABLE t (id INT);"), ("world", "2026_09_02_00_b", "ALTER TABLE t ADD c INT;")]);
        let store = Mem::default();
        let r = apply_pending(&store, &list, d.path(), &no_snapshot).unwrap();
        assert_eq!(r.applied, ["2026_09_01_00_a", "2026_09_02_00_b"], "manifest order is application order");
        assert_eq!(*store.ran.borrow(), ["2026_09_01_00_a", "2026_09_02_00_b"]);
        let again = apply_pending(&store, &list, d.path(), &no_snapshot).unwrap();
        assert!(again.applied.is_empty(), "never applied twice");
        assert!(status(&store, &list).unwrap().iter().all(|i| i.status == Status::Applied));
    }

    #[test]
    fn a_failure_stops_the_batch_and_is_remembered() {
        let (d, list) = setup(&[("world", "m1", "SELECT 1;"), ("world", "m2", "BROKEN;"), ("world", "m3", "SELECT 3;")]);
        let store = Mem { fail_on: Some("m2"), ..Default::default() };
        let r = apply_pending(&store, &list, d.path(), &no_snapshot).unwrap();
        assert_eq!(r.applied, ["m1"]);
        assert_eq!(r.failed.as_ref().unwrap().0, "m2");
        assert_eq!(*store.ran.borrow(), ["m1"], "m3 must not run after m2 failed");
        let st = status(&store, &list).unwrap();
        assert_eq!(st.iter().map(|i| i.status).collect::<Vec<_>>(), [Status::Applied, Status::Failed, Status::Pending]);
        assert!(st[1].error.as_deref().unwrap().contains("syntax error"));
    }

    #[test]
    fn destructive_migrations_force_a_backup_first_and_a_failed_backup_blocks_everything() {
        let (d, list) = setup(&[("characters", "d1", "-- cleanup\nDELETE FROM old_stuff WHERE 1;")]);
        let store = Mem::default();
        let r = apply_pending(&store, &list, d.path(), &|| Ok("backup-1".into())).unwrap();
        assert_eq!(r.snapshot.as_deref(), Some("backup-1"));
        assert_eq!(r.applied, ["d1"]);

        let store = Mem::default();
        let e = apply_pending(&store, &list, d.path(), &|| Err(Error::Invalid("disk full".into()))).unwrap_err();
        assert!(e.to_string().contains("safety backup failed"));
        assert!(store.ran.borrow().is_empty(), "nothing ran without a backup");
    }

    #[test]
    fn tampered_or_missing_files_are_refused_before_anything_runs() {
        let (d, mut list) = setup(&[("world", "t1", "SELECT 1;"), ("world", "t2", "SELECT 2;")]);
        list[1].sha256 = "0".repeat(64);
        let store = Mem::default();
        assert!(matches!(apply_pending(&store, &list, d.path(), &no_snapshot), Err(Error::HashMismatch { .. })));
        assert!(store.ran.borrow().is_empty(), "t1 must not run when t2 is bad");
        std::fs::remove_file(d.path().join("world/t1.sql")).unwrap();
        assert!(apply_pending(&store, &list[..1], d.path(), &no_snapshot).is_err());
    }

    #[test]
    fn interrupted_sql_is_reported_and_never_automatically_replayed() {
        let (d, list) = setup(&[("world", "partial", "DROP TABLE valuable;")]);
        let store = Mem::default();
        store.rows.borrow_mut().push(LedgerRow { db: "world".into(), id: "partial".into(), sha256: list[0].sha256.clone(), status: Status::Running, error: Some("interrupted".into()), baseline: false });
        assert_eq!(status(&store, &list).unwrap()[0].status, Status::Failed);
        assert!(apply_pending(&store, &list, d.path(), &no_snapshot).unwrap_err().to_string().contains("interrupted"));
        assert!(store.ran.borrow().is_empty());
    }

    #[test]
    fn baseline_marks_without_running_and_hostile_ids_are_rejected() {
        let (d, list) = setup(&[("world", "b1", "SELECT 1;")]);
        let store = Mem::default();
        assert_eq!(baseline(&store, &list).unwrap(), 1);
        assert_eq!(baseline(&store, &list).unwrap(), 0);
        assert!(store.ran.borrow().is_empty());
        assert!(status(&store, &list).unwrap()[0].baseline);
        let mut evil = list.clone();
        evil[0].id = "../../x".into();
        assert!(apply_pending(&Mem::default(), &evil, d.path(), &no_snapshot).is_err());
    }

    #[test]
    fn destructive_detection_ignores_comments() {
        assert!(looks_destructive("ALTER TABLE t DROP COLUMN c;"));
        assert!(looks_destructive("truncate table t;"));
        assert!(!looks_destructive("-- DROP TABLE nothing\nINSERT INTO t VALUES (1);"));
        assert!(!looks_destructive("CREATE TABLE IF NOT EXISTS t (id INT);"));
    }
}
