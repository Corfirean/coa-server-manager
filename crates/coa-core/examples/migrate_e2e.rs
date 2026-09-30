//! Bring a DISPOSABLE fixture database up to the core checkout's SQL, through the tracked migration runner.
//! usage: migrate_e2e <fixture folder> <core checkout>
use coa_core::backup;
use coa_core::db::{Account, Db};
use coa_core::fsx;
use coa_core::manifest::Migration;
use coa_core::migrations::{apply_pending, status, Status};
use std::path::{Path, PathBuf};

fn sql_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "sql").unwrap_or(false)).collect()).unwrap_or_default();
    v.sort();
    v
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a[0].to_lowercase().contains("fixture"), "refusing: not a fixture folder");
    let (root, core) = (PathBuf::from(&a[0]), PathBuf::from(&a[1]));
    let staging = std::env::temp_dir().join("coa-migrate-e2e");
    let _ = std::fs::remove_dir_all(&staging);

    // Release order: released updates, then pending ones, then module updates.
    let mut list: Vec<Migration> = Vec::new();
    let mut origin: std::collections::HashMap<(String, String), String> = Default::default();
    let mut add = |db: &str, id: String, src: &Path| {
        origin.insert((db.to_string(), id.clone()), src.strip_prefix(&core).unwrap().to_string_lossy().replace('\\', "/"));
        let bytes = std::fs::read(src).unwrap();
        let dst = staging.join(db).join(format!("{id}.sql"));
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, &bytes).unwrap();
        list.push(Migration { id, db: db.into(), sha256: fsx::sha256_bytes(&bytes), destructive: false });
    };
    for (kind, dirs) in [("auth", ["db_auth", "pending_db_auth"]), ("characters", ["db_characters", "pending_db_characters"]), ("world", ["db_world", "pending_db_world"])] {
        for d in dirs {
            for f in sql_files(&core.join("data/sql/updates").join(d)) {
                add(kind, f.file_stem().unwrap().to_string_lossy().into_owned(), &f);
            }
        }
    }
    for m in std::fs::read_dir(core.join("modules")).unwrap().flatten() {
        let mname = m.file_name().to_string_lossy().replace('-', "_");
        for kind in ["auth", "characters", "world"] {
            let base = m.path().join(format!("data/sql/db-{kind}"));
            for f in sql_files(&base.join("base")) {
                add(kind, format!("mod_{mname}__base__{}", f.file_stem().unwrap().to_string_lossy()), &f);
            }
            for f in sql_files(&base) {
                add(kind, format!("mod_{mname}__{}", f.file_stem().unwrap().to_string_lossy()), &f);
            }
        }
    }
    // Application order = the order the files were introduced in git history (a pending core script may rely on a
    // table created by a module script that arrived earlier). The release pipeline computes exactly this.
    let out = std::process::Command::new("git")
        .current_dir(&core)
        .args(["log", "--reverse", "--diff-filter=A", "--name-only", "--pretty=format:", "--", "data/sql/updates", "modules"])
        .output()
        .unwrap();
    let mut rank = std::collections::HashMap::new();
    for (i, path) in String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.ends_with(".sql")).enumerate() {
        rank.entry(path.to_string()).or_insert(i);
    }
    let key = |m: &Migration| -> usize {
        origin.get(&(m.db.clone(), m.id.clone())).and_then(|p| rank.get(p)).copied().unwrap_or(usize::MAX)
    };
    list.sort_by_key(|m| key(m));
    println!("candidate migrations: {} ({} not found in history)", list.len(), list.iter().filter(|m| key(m) == usize::MAX).count());

    let admin = Db::from_repack(&root, Account::Admin).unwrap();
    let _ = &admin;
    let t = std::time::Instant::now();
    let report = backup::with_database(&root, |db| {
        let before = status(db, &list)?;
        let pending = before.iter().filter(|i| i.status == Status::Pending).count();
        println!("already recorded by the database: {}; pending: {pending}", list.len() - pending);
        let pending_list: Vec<Migration> = list.iter().zip(&before).filter(|(_, i)| i.status == Status::Pending).map(|(m, _)| m.clone()).collect();
        apply_pending(db, &pending_list, &staging, &|| Ok("(fixture: no backup)".into()))
    })
    .expect("apply");
    println!("applied {} in {:?}; failed: {:?}", report.applied.len(), t.elapsed(), report.failed);
}
