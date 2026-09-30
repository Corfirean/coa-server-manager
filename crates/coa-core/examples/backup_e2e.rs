//! Full backup/restore scenario against a DISPOSABLE fixture (never point this at a real server).
use coa_core::backup::{self, Kind, Trigger};
use std::path::Path;

fn main() {
    let root = std::env::args().nth(1).expect("usage: backup_e2e <fixture folder>");
    assert!(root.to_lowercase().contains("fixture"), "refusing: not a fixture folder");
    let root = Path::new(&root);
    let meta = coa_core::registry::metadata_dir_for(root).unwrap();
    std::fs::create_dir_all(&meta).unwrap();

    let t = std::time::Instant::now();
    let point = backup::create(root, &meta, Kind::Quick, Trigger::Manual, Some("e2e".into()), &|s| println!("  .. {s}")).expect("backup");
    println!("quick backup {} in {:?}: {:?}", point.id, t.elapsed(), point.components.iter().map(|c| (&c.name, c.bytes, c.tables)).collect::<Vec<_>>());
    assert!(backup::verify(&meta, &point.id).unwrap().ok);

    // Change live state after the backup.
    backup::with_database(root, |db| {
        db.query("CREATE TABLE acore_characters.manager_marker (id INT); INSERT INTO acore_characters.manager_marker VALUES (42);")?;
        assert_eq!(db.query("SELECT COUNT(*) FROM acore_characters.manager_marker;")?, "1");
        Ok(())
    })
    .unwrap();

    let r = backup::restore_database(root, &meta, &point.id, "characters").expect("restore");
    println!("restored {} tables; previous database kept as {}; safety backup {}", r.tables_restored, r.previous_schema, r.safety_backup);

    backup::with_database(root, |db| {
        let live = db.tables("acore_characters")?;
        assert!(!live.contains(&"manager_marker".to_string()), "marker must be gone from the restored database");
        let old = db.tables(&r.previous_schema)?;
        assert!(old.contains(&"manager_marker".to_string()), "the replaced database keeps the marker");
        assert_eq!(db.query(&format!("SELECT id FROM {}.manager_marker;", r.previous_schema))?, "42");
        assert_eq!(live.len(), r.tables_restored);
        println!("OK: live tables {}, old schema tables {}", live.len(), old.len());
        Ok(())
    })
    .unwrap();

    // A rejected restore (server "running" is simulated by a damaged backup) must change nothing.
    let victim = meta.join("backups").join(&point.id).join("characters.sql.zst");
    let mut bytes = std::fs::read(&victim).unwrap();
    bytes[10] ^= 0xFF;
    std::fs::write(&victim, bytes).unwrap();
    assert!(backup::verify(&meta, &point.id).unwrap().problems.len() == 1);
    assert!(backup::restore_database(root, &meta, &point.id, "characters").is_err());
    println!("damaged backup correctly refused");
}
