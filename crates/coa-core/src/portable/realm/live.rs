//! Tests against a **real, disposable** MySQL server. They are `#[ignore]`d: the normal `cargo test` never needs a
//! database. To run them, start a throw-away MySQL with the schema of a CoA characters + auth database and load
//! `testdata/realm-fixture.sql` into it (`tools/gen_portable_fixture.py` writes that file), then:
//!
//! ```text
//! COA_PORTABLE_LIVE="<dir with mysql.exe>|<port>|<root password>" cargo test -p coa-core portable::realm::live -- --ignored --test-threads=1
//! COA_PORTABLE_BLESS=1 ...   # rewrite the recorded answers in testdata/golden/ instead of comparing
//! ```
//!
//! Never point this at a real server: some tests modify the fixture rows (and restore them).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::db::Db;
use crate::portable::snapshot;
use crate::portable::store::Store;
use crate::realms::Mode;

use super::script;
use super::*;

const FIXTURE_GUIDS: std::ops::RangeInclusive<u32> = 1001..=1012;

fn live_db() -> Option<Db> {
    let spec = std::env::var("COA_PORTABLE_LIVE").ok()?;
    let parts: Vec<&str> = spec.split('|').collect();
    assert_eq!(
        parts.len(),
        3,
        "COA_PORTABLE_LIVE = <bin dir>|<port>|<password>"
    );
    Some(Db::with_tools(
        PathBuf::from(parts[0]),
        parts[1].parse().expect("port"),
        "root",
        parts[2],
        Mode::Coa,
    ))
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/portable/realm/testdata/golden")
}

fn bless() -> bool {
    std::env::var("COA_PORTABLE_BLESS").is_ok()
}

/// Compare with (or, when blessing, rewrite) a recorded answer. Line endings are normalised so a Windows checkout
/// compares equal.
fn golden(name: &str, actual: &str) {
    // the Windows mysql client prints CRLF
    let actual = &actual.replace("\r\n", "\n");
    let path = golden_dir().join(name);
    if bless() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, format!("{}\n", actual.trim_end())).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!("missing golden file {name}; run once with COA_PORTABLE_BLESS=1")
        })
        .replace("\r\n", "\n");
    assert_eq!(
        actual.trim_end(),
        expected.trim_end(),
        "the realm's answer for {name} changed (run with COA_PORTABLE_BLESS=1 if that is intended)"
    );
}

fn probe_text(db: &Db) -> String {
    db.query(&script::probe_sql(db.realm_schema(script::CHARACTERS_SCHEMA)).unwrap())
        .unwrap()
}

#[test]
#[ignore]
fn record_or_verify_the_recorded_answers() {
    let Some(db) = live_db() else { return };
    let probe_out = probe_text(&db);
    golden("probe.txt", &probe_out);
    let probe = script::parse_probe(&probe_out).unwrap();
    assert!(probe.missing_required().is_empty());

    for guid in FIXTURE_GUIDS {
        let queries = script::queries(guid, &probe).unwrap();
        let output = db.query(&script::snapshot_script(&queries)).unwrap();
        golden(&format!("export-{guid}.txt"), &output);
    }

    // the character list also contains whatever else the disposable database holds; only the fixture rows are recorded
    let listing = db.query(&script::list_sql(&probe)).unwrap();
    let ours: Vec<&str> = listing
        .lines()
        .filter(|l| {
            l.split('\t')
                .next()
                .and_then(|g| g.parse::<u32>().ok())
                .is_some_and(|g| FIXTURE_GUIDS.contains(&g))
        })
        .collect();
    golden("list.txt", &ours.join("\n"));
}

#[test]
#[ignore]
fn the_exporter_reads_through_the_real_db_layer() {
    let Some(db) = live_db() else { return };
    let exported = export_character(&db, 1002, None, &HashMap::new()).unwrap();
    assert_eq!(exported.model.identity.name, "Geared");
    let encoded = snapshot::encode(&exported.model).unwrap();
    assert_eq!(
        snapshot::decode(&encoded.payload, Some(&encoded.content_hash)).unwrap(),
        exported.model
    );
    // a character that is online is refused by the real layer too
    assert!(
        matches!(export_character(&db, 1006, None, &HashMap::new()), Err(PortableError::NotExportable(b)) if b == vec![Blocker::Online])
    );
    assert!(matches!(
        export_character(&db, 999_999, None, &HashMap::new()),
        Err(PortableError::NoSuchRealmCharacter(999_999))
    ));
    // the listing agrees
    let listing = inspect_characters(&db).unwrap();
    let find = |g: u32| listing.iter().find(|c| c.local_guid == g).unwrap();
    assert!(find(1001).eligible() && find(1012).eligible());
    assert_eq!(find(1006).blockers, vec![Blocker::Online]);
    assert!(
        listing.iter().all(|c| c.local_guid != 1010),
        "bot accounts are not listed"
    );
}

/// The real template characters of the schema fixture are genuine CoA characters (level 80, all 21 classes).
#[test]
#[ignore]
fn every_real_template_character_exports_and_roundtrips() {
    let Some(db) = live_db() else { return };
    let mut exported = 0;
    for guid in 112..=132u32 {
        let result = export_character(&db, guid, None, &HashMap::new());
        let Ok(e) = result else {
            panic!(
                "template character {guid} did not export: {:?}",
                result.err()
            );
        };
        let encoded = snapshot::encode(&e.model).unwrap();
        let back = snapshot::decode(&encoded.payload, Some(&encoded.content_hash)).unwrap();
        assert_eq!(back, e.model, "guid {guid}");
        assert_eq!(e.model.progression.level, 80);
        // the template characters carry little more than their CoA gameplay state
        assert!(
            !e.model.settings.is_empty() || !e.model.extensions.is_empty(),
            "guid {guid} exported without any CoA state"
        );
        exported += 1;
    }
    assert_eq!(exported, 21);
}

#[test]
#[ignore]
fn make_portable_registers_the_character_and_writes_nothing_to_the_realm() {
    let Some(db) = live_db() else { return };
    let before = db.query("SELECT COUNT(*), IFNULL(SUM(money),0), IFNULL(SUM(level),0) FROM acore_characters.characters").unwrap();
    let items_before = db
        .query("SELECT COUNT(*) FROM acore_characters.item_instance")
        .unwrap();

    let mut store = Store::open_in_memory().unwrap();
    let profile = store.default_profile().unwrap();
    let made = make_portable(&db, &mut store, profile, "live-realm", 1002).unwrap();
    assert_eq!(made.revision, 1);
    assert!(
        !made.warnings.is_empty(),
        "the orphaned items of the fixture are reported"
    );
    assert_eq!(
        store.find_by_local("live-realm", 1002).unwrap(),
        Some(made.character_id)
    );
    assert!(matches!(
        make_portable(&db, &mut store, profile, "live-realm", 1002),
        Err(PortableError::AlreadyPortable { .. })
    ));

    assert_eq!(db.query("SELECT COUNT(*), IFNULL(SUM(money),0), IFNULL(SUM(level),0) FROM acore_characters.characters").unwrap(), before);
    assert_eq!(
        db.query("SELECT COUNT(*) FROM acore_characters.item_instance")
            .unwrap(),
        items_before
    );
}

#[test]
#[ignore]
fn the_snapshot_script_cannot_write() {
    let Some(db) = live_db() else { return };
    let attempt = db.query("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;\nUPDATE acore_characters.characters SET money = 1 WHERE guid = 1012;\nCOMMIT;\n");
    let error = attempt
        .expect_err("a write inside the read-only snapshot must fail")
        .to_string();
    assert!(
        error.to_lowercase().contains("read only") || error.to_lowercase().contains("read-only"),
        "{error}"
    );
    assert_eq!(
        db.query("SELECT money FROM acore_characters.characters WHERE guid = 1012")
            .unwrap(),
        "0"
    );
}

/// All tables are read as of one moment: a write by someone else in the middle of the script is not seen.
#[test]
#[ignore]
fn the_snapshot_is_consistent_while_the_realm_keeps_writing() {
    let Some(db) = live_db() else { return };
    db.query("UPDATE acore_characters.characters SET money = 100 WHERE guid = 1012")
        .unwrap();
    let reader = {
        let db = db.clone();
        std::thread::spawn(move || {
            db.query(
                "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;
START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;
                 SELECT money FROM acore_characters.characters WHERE guid = 1012;
SELECT SLEEP(3);
                 SELECT money FROM acore_characters.characters WHERE guid = 1012;
COMMIT;
",
            )
            .unwrap()
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(1200));
    db.query("UPDATE acore_characters.characters SET money = 777 WHERE guid = 1012")
        .unwrap();
    let seen = reader.join().unwrap();
    let after = db
        .query("SELECT money FROM acore_characters.characters WHERE guid = 1012")
        .unwrap();
    db.query("UPDATE acore_characters.characters SET money = 0 WHERE guid = 1012")
        .unwrap();
    assert_eq!(after, "777", "the concurrent write really happened");
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(
        (lines.first().copied(), lines.last().copied()),
        (Some("100"), Some("100")),
        "both reads see the state at the start of the snapshot; saw {seen:?}"
    );
}
