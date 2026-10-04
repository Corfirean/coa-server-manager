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
        let out = self.query(&format!("SELECT `db`,`id`,`sha256`,`status`,`baseline`,COALESCE(HEX(`error`),'') FROM `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` ORDER BY `db`,`id`;"))?;
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

fn hash_changed(row: &LedgerRow, m: &Migration) -> bool {
    row.status == Status::Applied && row.sha256 != "0".repeat(64) && !row.sha256.eq_ignore_ascii_case(&m.sha256)
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
    for m in list {
        if recorded(&rows, m).is_some_and(|r| matches!(r.status, Status::Running | Status::Failed)) {
            return Err(Error::Invalid(format!("Database update {} ({}) failed or was interrupted and may be partially applied. Restore its recovery point before retrying.", m.id, m.db)));
        }
        if recorded(&rows, m).is_some_and(|r| hash_changed(r, m)) {
            return Err(Error::Invalid(format!("Applied database update {} ({}) has a different checksum. A new migration is required; it was not replayed.", m.id, m.db)));
        }
    }
    let todo: Vec<&Migration> = list.iter().filter(|m| recorded(&rows, m).is_none_or(|r| r.status != Status::Applied)).collect();

    // Resolve and verify every file before running anything.
    let mut files = Vec::new();
    let mut any_destructive = false;
    for m in &todo {
        if !ident_ok(&m.id) {
            return Err(Error::InvalidManifest(format!("migration id {:?} is not allowed", m.id)));
        }
        let schema = db::schema_of(&m.db)?;
        let path = fsx::safe_join(dir, &format!("{}/{}.sql", m.db, m.id))?;
        let bytes = std::fs::read(&path).map_err(|_| Error::Invalid(format!("migration file {} is missing", m.id)))?;
        let actual = fsx::sha256_bytes(&bytes);
        if !actual.eq_ignore_ascii_case(&m.sha256) {
            return Err(Error::HashMismatch { path: path.display().to_string(), expected: m.sha256.clone(), actual });
        }
        any_destructive |= m.destructive || looks_destructive(&String::from_utf8_lossy(&bytes));
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
        store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Running, error: Some("Interrupted SQL must be recovered before replay.".into()), baseline: false })?;
        match store.run_file(schema, &path) {
            Ok(()) => {
                store.put(&LedgerRow { db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), status: Status::Applied, error: None, baseline: false })?;
                report.applied.push(m.id.clone());
            }
            Err(e) => {
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
            list.push(Migration { id: id.to_string(), db: db.to_string(), sha256: fsx::sha256_bytes(sql.as_bytes()), destructive: false });
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
