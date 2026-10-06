//! Live tests of the offline importer against **two real disposable MySQL servers**: realm A (the source, loaded with
//! `testdata/realm-fixture.sql`) and realm B (the stopped destination, loaded with `testdata/realm-b-fixture.sql`; both
//! are copies of the CoA schema fixture, so B has the real CoA world data). `#[ignore]`d; see `live.rs` for how to run:
//!
//! ```text
//! COA_PORTABLE_LIVE="<bin>|13998|<pw>"  COA_PORTABLE_LIVE_B="<bin>|13997|<pw>"  cargo test -p coa-core portable::realm::live_import -- --ignored --test-threads=1
//! ```
//!
//! Realm B must also have a database user `acore` (password `acore-test`, hosts `localhost` and `127.0.0.1`) to stand in for a running game server.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use crate::db::Db;
use crate::portable::ids::ContentId;
use crate::portable::model::*;
use crate::portable::store::{ImportState, Store};
use crate::realms::Mode;

use super::import::*;
use super::plan::{build_plan, PlanContext};
use super::*;

pub(super) const B: &str = "realm-b";
pub(super) const ACCOUNT: u32 = 7001;

pub(super) fn tools(var: &str) -> Option<(PathBuf, u16, String)> {
    let spec = std::env::var(var).ok()?;
    let p: Vec<&str> = spec.split('|').collect();
    assert_eq!(p.len(), 3, "{var} = <bin dir>|<port>|<password>");
    Some((PathBuf::from(p[0]), p[1].parse().expect("port"), p[2].to_string()))
}

pub(super) struct Realms {
    pub(super) a: Db,
    pub(super) b: Db,
    /// A session of the game server's database user, to stand in for a running realm.
    pub(super) server_user: Db,
}

pub(super) fn realms() -> Option<Realms> {
    let (bin_a, port_a, pw_a) = tools("COA_PORTABLE_LIVE")?;
    let (bin_b, port_b, pw_b) = tools("COA_PORTABLE_LIVE_B")?;
    Some(Realms {
        a: Db::with_tools(bin_a, port_a, "root", &pw_a, Mode::Coa),
        server_user: Db::with_tools(bin_b.clone(), port_b, "acore", "acore-test", Mode::Coa),
        b: Db::with_tools(bin_b, port_b, "root", &pw_b, Mode::Coa),
    })
}

pub(super) fn opts() -> ImportOptions {
    ImportOptions { recovery_grace: Duration::ZERO, lock_wait_seconds: 2, ..ImportOptions::default() }
}

/// Put realm B back to its baseline (the characters of `realm-b-fixture.sql` and the template characters): removes every
/// character with a guid above the fixture's and what hangs off it. Every live test starts with this, so the tests do not
/// depend on each other or on their order.
pub(super) fn reset_b(db: &Db) {
    let mut sql = vec!["DROP TRIGGER IF EXISTS acore_characters.coa_test_fail;".to_string()];
    for (table, column) in super::plan::CHARACTER_KEYED {
        sql.push(format!("DELETE FROM acore_characters.`{table}` WHERE `{column}` > 3010;"));
    }
    sql.push("DELETE FROM acore_characters.characters WHERE guid > 3010;".into());
    sql.push("DELETE FROM acore_characters.pet_spell WHERE guid NOT IN (SELECT id FROM acore_characters.character_pet);".into());
    sql.push("DELETE FROM acore_characters.character_pet_declinedname WHERE id NOT IN (SELECT id FROM acore_characters.character_pet);".into());
    sql.push("DELETE FROM acore_characters.mail_items WHERE item_guid >= 99000;".into());
    sql.push("UPDATE acore_characters.characters SET online = 0;".into());
    db.query(&sql.join("
")).unwrap();
}

pub(super) fn on_b(store: &Store, id: CharacterId) -> Vec<crate::portable::store::MappingRecord> {
    store.server_mappings(id).unwrap().into_iter().filter(|m| m.server_id == B).collect()
}

pub(super) fn first(db: &Db, sql: &str) -> String {
    db.query(sql).unwrap().lines().next().unwrap_or("").to_string()
}

pub(super) fn number(db: &Db, sql: &str) -> u64 {
    first(db, sql).trim().parse().unwrap_or_else(|_| panic!("not a number: {sql}"))
}

/// Row counts of every table of the characters schema: the "nothing changed" proof.
pub(super) fn counts(db: &Db) -> BTreeMap<String, u64> {
    let tables = db.tables("acore_characters").unwrap();
    let sql = tables.iter().map(|t| format!("SELECT '{t}', COUNT(*) FROM acore_characters.`{t}`")).collect::<Vec<_>>().join(" UNION ALL ");
    db.query(&sql).unwrap().lines().map(|l| {
        let (t, n) = l.trim().split_once('\t').unwrap();
        (t.to_string(), n.trim().parse().unwrap())
    }).collect()
}

pub(super) fn fresh_store() -> (Store, crate::portable::ids::ProfileId) {
    let mut store = Store::open_in_memory().unwrap();
    let profile = store.default_profile().unwrap();
    (store, profile)
}

pub(super) fn make(r: &Realms, store: &mut Store, profile: crate::portable::ids::ProfileId, guid: u32) -> CharacterId {
    make_portable(&r.a, store, profile, "realm-a", guid).unwrap().character_id
}

/// What a trip through a realm legitimately does not preserve. Everything else must come back identical.
pub(super) fn expected_after_a_trip(canonical: &PortableCharacter, back: &PortableCharacter) -> PortableCharacter {
    let mut c = canonical.clone();
    c.identity.name = back.identity.name.clone(); // a taken or reserved name arrives under a temporary one
    for item in &mut c.items {
        item.creator_name = None; // the crafter is a character of another realm and is not stored at the destination
    }
    c.extensions.clear(); // quarantined settings are kept in the snapshot, not written to the realm
    c.build.talents.clear(); // stock talent rows are never written: a bad row makes the realm assert
    for (mine, theirs) in c.pets.iter_mut().zip(&back.pets) {
        mine.id = theirs.id; // pets get new portable ids on every read until pet mappings exist
    }
    c.normalized()
}

pub(super) fn read_back(r: &Realms, store: &Store, id: CharacterId) -> PortableCharacter {
    let guid = store.server_mappings(id).unwrap().iter().find(|m| m.server_id == B).unwrap().local_guid;
    let prior = store.active_item_lookup(id, B).unwrap();
    let pets = store.active_pet_lookup(id, B).unwrap();
    export_character_with_pets(&r.b, guid, Some(id), &prior, &pets).unwrap().model
}

#[test]
#[ignore]
fn characters_survive_a_trip_into_a_second_realm() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    for guid in [1001u32, 1002, 1003, 1004, 1005, 1012] {
        let id = make(&r, &mut store, profile, guid);
        let canonical = store.load_current(id).unwrap();
        let (max_guid, max_item) = (number(&r.b, "SELECT MAX(guid) FROM acore_characters.characters"), number(&r.b, "SELECT GREATEST(IFNULL((SELECT MAX(guid) FROM acore_characters.item_instance),0), IFNULL((SELECT MAX(item_guid) FROM acore_characters.mail_items),0))"));

        let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap_or_else(|e| panic!("import of {guid}: {e}"));

        // allocation: a new guid above everything in B, items right above everything that names an item
        assert_eq!(outcome.local_guid as u64, max_guid + 1, "{guid}");
        assert_ne!(outcome.local_guid, guid, "the guid of the source realm is never reused");
        let mapping = on_b(&store, id).remove(0);
        assert_eq!((mapping.local_guid, mapping.last_revision), (outcome.local_guid, 1));
        let mapped = store.item_mappings(id, B).unwrap();
        assert_eq!(mapped.len(), canonical.items.len());
        if let Some(min) = mapped.iter().map(|m| m.local_item_guid as u64).min() {
            assert!(min > max_item, "{guid}: items start at {min}, above {max_item}");
            assert_eq!(mapped.iter().map(|m| m.local_item_guid as u64).max().unwrap() - min + 1, mapped.len() as u64, "contiguous");
        }

        // every reference in the realm points inside the new character
        let g = outcome.local_guid;
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {g}")) as usize, canonical.items.len());
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_inventory ci LEFT JOIN acore_characters.item_instance ii ON ii.guid = ci.item AND ii.owner_guid = {g} WHERE ci.guid = {g} AND ii.guid IS NULL")), 0);
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_inventory ci WHERE ci.guid = {g} AND ci.bag <> 0 AND ci.bag NOT IN (SELECT item FROM acore_characters.character_inventory WHERE guid = {g})")), 0);
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {g} AND (creatorGuid <> 0 OR giftCreatorGuid <> 0)")), 0, "no guid of another character is carried");
        if !canonical.pets.is_empty() {
            assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_pet WHERE owner = {g}")) as usize, canonical.pets.len());
            assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.pet_spell ps JOIN acore_characters.character_pet p ON p.id = ps.guid WHERE p.owner = {g}")) as usize, canonical.pets.iter().map(|p| p.spells.len()).sum::<usize>());
        }

        // a fresh arrival: not online, no first-login rewards, the intro seen, standing at the realm's own start position
        assert_eq!(first(&r.b, &format!("SELECT online, cinematic, at_login & 32 FROM acore_characters.characters WHERE guid = {g}")), "0\t1\t0");
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.characters c JOIN acore_characters.character_homebind h ON h.guid = c.guid JOIN acore_world.playercreateinfo p ON p.race = c.race AND p.class = c.class WHERE c.guid = {g} AND c.map = p.map AND h.mapId = p.map AND h.posX = p.position_x")), 1);

        // the character is what it was: read it back out of the realm and compare
        let back = read_back(&r, &store, id);
        assert_eq!(back, expected_after_a_trip(&canonical, &back), "character {guid} changed in the realm");
        assert_eq!(back.character_id, id);

        // names: taken (case-insensitively) or reserved names arrive renamed
        match guid {
            1001 | 1012 => {
                assert!(outcome.renamed, "{guid}");
                assert_eq!(outcome.final_name, format!("{}{:X}", canonical.identity.name, g));
                assert_eq!(number(&r.b, &format!("SELECT at_login & 1 FROM acore_characters.characters WHERE guid = {g}")), 1);
            }
            _ => {
                assert!(!outcome.renamed, "{guid}");
                assert_eq!(outcome.final_name, canonical.identity.name);
            }
        }
        assert!(!outcome.not_applied.is_empty() || guid != 1004, "1004's unapplied settings are reported");
        // the same character cannot be imported into the same realm twice
        assert!(matches!(import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()), Err(PortableError::AlreadyOnRealm { .. })));
        let entry = store.open_imports(B).unwrap();
        assert!(entry.is_empty(), "nothing is left unfinished");
    }
}

#[test]
#[ignore]
fn leftovers_of_a_deleted_character_with_the_same_guid_do_not_attach_to_the_newcomer() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1005);
    let next = number(&r.b, "SELECT MAX(guid) + 1 FROM acore_characters.characters");
    // a deleted character with this very guid left rows behind (module tables are not cleaned by the realm's own delete)
    let empty_enchants = "0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 ";
    r.b.query(&format!(
        "INSERT INTO acore_characters.character_spell (guid, spell, specMask) VALUES ({next}, 99999, 1);
         INSERT INTO acore_characters.coa_character_condition (guid, flag) VALUES ({next}, 'orphan');
         INSERT INTO acore_characters.character_settings (guid, source, data) VALUES ({next}, 'core.ascension_build.1', '5 ');
         INSERT INTO acore_characters.item_instance (guid, itemEntry, owner_guid, enchantments) VALUES (99001, 30000, {next}, '{empty_enchants}');
         INSERT INTO acore_characters.character_inventory (guid, bag, slot, item) VALUES ({next}, 0, 5, 99001);
         INSERT INTO acore_characters.mail_items (mail_id, item_guid, receiver) VALUES (1, 99500, 3001);
         INSERT INTO acore_characters.character_pet (id, entry, owner) VALUES (3999, 416, {next});
         INSERT INTO acore_characters.pet_spell (guid, spell, active) VALUES (3999, 1, 1);"
    ))
    .unwrap();

    let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap();
    let g = outcome.local_guid;
    assert_eq!(g as u64, next);

    // the leftovers keyed by the character guid are gone
    for sql in [
        format!("SELECT COUNT(*) FROM acore_characters.character_spell WHERE guid = {g} AND spell = 99999"),
        format!("SELECT COUNT(*) FROM acore_characters.coa_character_condition WHERE guid = {g}"),
        format!("SELECT COUNT(*) FROM acore_characters.character_settings WHERE guid = {g} AND source = 'core.ascension_build.1'"),
        "SELECT COUNT(*) FROM acore_characters.item_instance WHERE guid = 99001".to_string(),
        format!("SELECT COUNT(*) FROM acore_characters.character_inventory WHERE guid = {g} AND item = 99001"),
        "SELECT COUNT(*) FROM acore_characters.character_pet WHERE id = 3999".to_string(),
    ] {
        assert_eq!(number(&r.b, &sql), 0, "{sql}");
    }
    // the newcomer's items start above every item guid anything still names (the mail row names 99500)
    let items = store.item_mappings(id, B).unwrap();
    assert!(items.iter().all(|m| m.local_item_guid > 99_500));
    // and its pets above the stale pet number: the stale pet_spell row cannot become the newcomer's
    let pet_numbers: Vec<u64> = r.b.query(&format!("SELECT id FROM acore_characters.character_pet WHERE owner = {g}")).unwrap().lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(pet_numbers.len(), 3);
    assert!(pet_numbers.iter().all(|n| *n > 3999), "{pet_numbers:?}");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.pet_spell WHERE guid = 3999"), 1, "unrelated rows are not touched");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.mail_items WHERE item_guid = 99500"), 1);
    r.b.query("DELETE FROM acore_characters.pet_spell WHERE guid = 3999; DELETE FROM acore_characters.mail_items WHERE item_guid = 99500").unwrap();
}

#[test]
#[ignore]
fn a_failure_in_the_middle_rolls_everything_back_and_leaves_nothing_mapped() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let before = counts(&r.b);

    // a trigger that makes one spell row fail: the failure happens after the character and its items were already inserted
    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    r.b.query("DELIMITER //\nCREATE TRIGGER acore_characters.coa_test_fail BEFORE INSERT ON acore_characters.character_spell FOR EACH ROW BEGIN IF NEW.spell = 500090 THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'injected failure'; END IF; END//\nDELIMITER ;\n").unwrap();

    let result = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts());
    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    let error = result.expect_err("the injected failure must fail the import").to_string();
    assert!(error.contains("injected failure") || error.contains("database command failed"), "{error}");

    assert_eq!(counts(&r.b), before, "not one row of one table may have changed");
    assert!(on_b(&store, id).is_empty(), "no Manager mapping after a failed realm transaction");
    assert!(store.item_mappings(id, B).unwrap().is_empty());
    assert!(store.open_imports(B).unwrap().is_empty(), "the journal entry is closed");
    let journal = r.b.query("SELECT 1").unwrap();
    assert_eq!(journal.trim(), "1");

    // and the realm is fine afterwards: the same import now works (the lock was released, the journal allows a retry)
    let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap();
    assert_eq!(on_b(&store, id)[0].local_guid, outcome.local_guid);
}

#[test]
#[ignore]
fn imports_are_refused_before_anything_is_written() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1003);
    let model = store.load_current(id).unwrap();
    let before = counts(&r.b);
    let refused = |store: &mut Store, id: CharacterId, account: u32| -> Vec<ImportProblem> {
        match import_character(&r.b, store, id, B, account, &opts()) {
            Err(PortableError::ImportRefused(p)) => p,
            other => panic!("expected a refusal, got {:?}", other.map(|o| o.local_guid)),
        }
    };

    // accounts
    assert_eq!(refused(&mut store, id, 99_999), vec![ImportProblem::AccountMissing(99_999)]);
    assert_eq!(refused(&mut store, id, 7002), vec![ImportProblem::InternalAccount(7002)]);
    assert_eq!(refused(&mut store, id, 7003), vec![ImportProblem::AccountFull { count: 10, max: 10 }]);

    // the realm is running: a session of the game server's database user exists
    let server = r.server_user.clone();
    let session = std::thread::spawn(move || server.query("SELECT SLEEP(6)").unwrap());
    std::thread::sleep(Duration::from_millis(1500));
    let running = refused(&mut store, id, ACCOUNT);
    assert!(matches!(running.as_slice(), [ImportProblem::RealmRunning { sessions }] if *sessions >= 1), "{running:?}");
    // ... and if the server starts between the preflight and the transaction, the transaction's own guard stops it
    let probe = super::probe(&r.b).unwrap();
    let users = vec!["acore".to_string()];
    let plan = build_plan(&model, &PlanContext { ruleset: Ruleset::Coa, account: ACCOUNT, revision: 1, nonce: [1, 2, 3, 4], max_characters_per_account: 10, game_server_users: &users, probe: &probe }).unwrap();
    assert!(run_realm_import(&r.b, &plan).is_err(), "the in-transaction guard must refuse while the game server's session exists");
    session.join().unwrap();

    // characters marked online
    r.b.query("UPDATE acore_characters.characters SET online = 1 WHERE guid = 2002").unwrap();
    let online = refused(&mut store, id, ACCOUNT);
    r.b.query("UPDATE acore_characters.characters SET online = 0 WHERE guid = 2002").unwrap();
    assert_eq!(online, vec![ImportProblem::OnlineCharacters(1)]);

    assert_eq!(counts(&r.b), before, "refusals write nothing");
    assert!(store.open_imports(B).unwrap().is_empty(), "and leave no journal entry");
    assert!(on_b(&store, id).is_empty());

    // content the realm cannot hold
    let missing_entry = number(&r.b, "SELECT MAX(entry) + 1000 FROM acore_world.item_template") as u32;
    let mut with_unknown_item = export_character(&r.a, 1002, None, &HashMap::new()).unwrap().model;
    with_unknown_item.items[0].entry = ContentId::new("coa", "item", missing_entry as u64).unwrap();
    let id2 = store.create_character(profile, with_unknown_item, "realm-a").unwrap();
    assert_eq!(refused(&mut store, id2, ACCOUNT), vec![ImportProblem::MissingItems(vec![missing_entry])]);

    let mut unknown_class = export_character(&r.a, 1003, None, &HashMap::new()).unwrap().model;
    unknown_class.identity.class = ContentId::new("coa", "class", 99).unwrap();
    let id3 = store.create_character(profile, unknown_class, "realm-a").unwrap();
    assert_eq!(refused(&mut store, id3, ACCOUNT), vec![ImportProblem::UnknownRaceClass { race: 3, class: 99 }]);

    let mut mod_item = export_character(&r.a, 1002, None, &HashMap::new()).unwrap().model;
    mod_item.items[0].entry = ContentId::new("mod:other", "item", 5).unwrap();
    let id4 = store.create_character(profile, mod_item, "realm-a").unwrap();
    assert!(matches!(refused(&mut store, id4, ACCOUNT).as_slice(), [ImportProblem::UnsupportedContent(_)]));

    let mut wildcard = export_character(&r.a, 1003, None, &HashMap::new()).unwrap().model;
    wildcard.ruleset = Ruleset::Wildcard;
    wildcard.content_namespace = "wildcard".into();
    let id5 = store.create_character(profile, wildcard, "realm-a").unwrap();
    assert!(refused(&mut store, id5, ACCOUNT).iter().any(|p| matches!(p, ImportProblem::WrongRuleset { character: Ruleset::Wildcard, realm: Ruleset::Coa })), "CoA and Wildcard never mix");

    assert_eq!(counts(&r.b), before);
    assert!(store.open_imports(B).unwrap().is_empty());
}

#[test]
#[ignore]
fn a_crash_between_the_two_commits_is_recovered_from_the_marker_in_the_realm() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let model = store.load_current(id).unwrap();
    let probe = super::probe(&r.b).unwrap();

    // the realm commits ... and the Manager "dies" before it records anything
    let ticket = store.begin_import(id, B, 1, &import::planned_items(&model), &[]).unwrap();
    let users = vec!["acore".to_string()];
    let plan = build_plan(&model, &PlanContext { ruleset: Ruleset::Coa, account: ACCOUNT, revision: 1, nonce: ticket.nonce, max_characters_per_account: 10, game_server_users: &users, probe: &probe }).unwrap();
    let (alloc, committed) = run_realm_import(&r.b, &plan).unwrap();
    assert!(committed);
    assert_eq!(store.import_entry(ticket.import_id).unwrap().state, ImportState::Prepared);
    assert!(on_b(&store, id).is_empty(), "the Manager knows nothing yet");

    // the next start of the Manager: recovery finds the marker and completes the local side
    let report = recover_imports(&r.b, &mut store, B, &opts()).unwrap();
    assert_eq!(report.len(), 1);
    // the guid and the item base are recovered exactly; this character has no pets, so there is no pet base to recover
    let Resolution::Committed(recovered) = &report[0].resolution else { panic!("{:?}", report[0].resolution) };
    assert_eq!((recovered.local_guid, recovered.item_base), (alloc.local_guid, alloc.item_base));
    let mapping = &on_b(&store, id)[0];
    assert_eq!((mapping.local_guid, mapping.last_revision), (alloc.local_guid, 1));
    assert_eq!(store.item_mappings(id, B).unwrap().len(), model.items.len());
    // exactly what a normal import would have recorded: reading the realm back with these mappings gives the character
    let back = read_back(&r, &store, id);
    assert_eq!(back, expected_after_a_trip(&model, &back));
    assert!(recover_imports(&r.b, &mut store, B, &opts()).unwrap().is_empty(), "nothing is left to recover");
}

#[test]
#[ignore]
fn a_crash_before_the_realm_commit_is_recovered_as_never_happened() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1003);
    let model = store.load_current(id).unwrap();
    let before = counts(&r.b);

    let ticket = store.begin_import(id, B, 1, &import::planned_items(&model), &[]).unwrap();
    // too recent to be called lost while an orphaned client might still be finishing it
    let patient = ImportOptions { recovery_grace: Duration::from_secs(3600), ..opts() };
    let pending = recover_imports(&r.b, &mut store, B, &patient).unwrap();
    assert!(matches!(pending[0].resolution, Resolution::Pending(_)), "{:?}", pending[0].resolution);
    assert_eq!(store.import_entry(ticket.import_id).unwrap().state, ImportState::Prepared);

    // an import that is still running in the realm holds the lock: recovery must wait for it, not decide
    let holder = r.b.clone();
    let busy = std::thread::spawn(move || holder.query("SELECT GET_LOCK('coa_portable_import', 5), SLEEP(4)").unwrap());
    std::thread::sleep(Duration::from_millis(1000));
    let blocked = recover_imports(&r.b, &mut store, B, &opts()).unwrap();
    assert!(matches!(&blocked[0].resolution, Resolution::Pending(why) if why.contains("still running")), "{:?}", blocked[0].resolution);
    busy.join().unwrap();

    let report = recover_imports(&r.b, &mut store, B, &opts()).unwrap();
    assert_eq!(report[0].resolution, Resolution::Aborted);
    assert_eq!(store.import_entry(ticket.import_id).unwrap().state, ImportState::Aborted);
    assert!(on_b(&store, id).is_empty());
    assert_eq!(counts(&r.b), before);
    // and the import can simply be run again
    assert!(import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).is_ok());
}

#[test]
#[ignore]
fn a_realm_that_no_longer_matches_the_plan_is_flagged_not_guessed_about() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let model = store.load_current(id).unwrap();
    let probe = super::probe(&r.b).unwrap();
    let ticket = store.begin_import(id, B, 1, &import::planned_items(&model), &[]).unwrap();
    let users = vec!["acore".to_string()];
    let plan = build_plan(&model, &PlanContext { ruleset: Ruleset::Coa, account: ACCOUNT, revision: 1, nonce: ticket.nonce, max_characters_per_account: 10, game_server_users: &users, probe: &probe }).unwrap();
    let (alloc, _) = run_realm_import(&r.b, &plan).unwrap();
    // someone deletes an item of the new character before the Manager recovers
    r.b.query(&format!("DELETE FROM acore_characters.item_instance WHERE guid = {}", alloc.item_base + 3)).unwrap();

    let result = recover_imports(&r.b, &mut store, B, &opts());
    assert!(matches!(result, Err(PortableError::ImportNeedsAttention { .. })), "{:?}", result.map(|_| ()));
    assert_eq!(store.import_entry(ticket.import_id).unwrap().state, ImportState::NeedsAttention);
    assert!(on_b(&store, id).is_empty(), "no mapping is recorded on a guess");
}

#[test]
#[ignore]
fn hostile_text_in_names_texts_blobs_and_pets_reaches_the_realm_as_data_only() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let tables_before = r.b.tables("acore_characters").unwrap();

    let mut m = export_character(&r.a, 1005, None, &HashMap::new()).unwrap().model;
    m.identity.name = "Bob');DROP TABLE x;--".into();
    m.pets[0].name = "p');DROP TABLE x;--".into();
    m.pets[0].action_bar = "'; DROP TABLE pet_spell; -- \\".into();
    m.pets[1].declined_names = Some(["a'".into(), "b\"".into(), "c\\".into(), "d;".into(), "e--".into()]);
    m.client_data.insert(5, ClientBlob { time: 5, data: Bytes(b"'; DROP DATABASE acore_characters; -- \x00\xff\n\t\\".to_vec()) });
    let hostile_item_text = "\"; DELETE FROM item_instance; -- \\ \n\t' OR '1'='1 \u{1F409}";
    m.items.push(PortableItem {
        id: crate::portable::ids::PortableItemId::new(),
        container: None,
        slot: 23,
        entry: ContentId::new("coa", "item", 30_000).unwrap(),
        count: 1,
        duration: 0,
        charges: vec![],
        flags: 0,
        enchantments: vec![],
        random_property_id: 0,
        durability: 1,
        played_time: 0,
        text: Some(hostile_item_text.into()),
        creator_name: None,
        gift: None,
    });
    let id = store.create_character(profile, m.normalized(), "realm-a").unwrap();
    let canonical = store.load_current(id).unwrap();
    let characters_before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
    let items_before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance");

    import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap();

    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), characters_before + 1, "exactly one character was added, nothing deleted");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance"), items_before + 1);
    assert_eq!(r.b.tables("acore_characters").unwrap(), tables_before, "no table was dropped or created");
    assert!(r.b.schema_exists("acore_characters").unwrap());

    let back = read_back(&r, &store, id);
    assert_eq!(back.identity.name, canonical.identity.name, "the name arrives byte for byte (it was free on the realm)");
    assert_eq!(back.items[0].text.as_deref(), Some(hostile_item_text));
    assert_eq!(back.client_data[&5].data, canonical.client_data[&5].data);
    assert_eq!(back.pets.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), canonical.pets.iter().map(|p| p.name.clone()).collect::<Vec<_>>());
    assert!(back.pets.iter().any(|p| p.declined_names == Some(["a'".into(), "b\"".into(), "c\\".into(), "d;".into(), "e--".into()])));
    assert_eq!(back, expected_after_a_trip(&canonical, &back));
}

#[test]
#[ignore]
fn the_realms_own_start_data_and_rules_decide_the_arrival() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1004);
    let report = preflight(&r.b, &store.load_current(id).unwrap(), ACCOUNT, &opts()).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(!report.will_rename);
    let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap();
    let g = outcome.local_guid;
    // the marker identifies the import in the realm and is not part of what the character is
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_settings WHERE guid = {g} AND source = 'coa.portable.import'")), 1);
    let back = read_back(&r, &store, id);
    assert!(!back.settings.contains_key("coa.portable.import"), "the marker is dropped by the export policy");
    // quarantined settings are in the snapshot and are not in the realm
    assert!(outcome.not_applied.iter().any(|s| s == "coa:unlisted-settings"), "{:?}", outcome.not_applied);
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_settings WHERE guid = {g} AND source LIKE 'core.spell_charge.%'")), 0);
    assert!(store.load_current(id).unwrap().extensions.contains_key("coa:unlisted-settings"), "nothing was destroyed in the canonical snapshot");
}

// ---- the real server ------------------------------------------------------------------------------------------------

const SERVER_CHECK_GUIDS: [u32; 6] = [1001, 1002, 1003, 1004, 1005, 1012];

fn check_dir() -> PathBuf {
    PathBuf::from(std::env::var("COA_PORTABLE_SERVER_CHECK_DIR").expect("COA_PORTABLE_SERVER_CHECK_DIR"))
}

/// The staged real-server checks only mean something when run stage by stage with the worldserver in between.
fn staged() -> bool {
    std::env::var("COA_PORTABLE_SERVER_CHECK_DIR").is_ok()
}

/// Paths at which two JSON values differ (capped).
pub(super) fn json_diff(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value::{Array, Null, Object};
    if out.len() >= 60 {
        return;
    }
    match (a, b) {
        (Object(x), Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                json_diff(&format!("{path}.{k}"), x.get(k).unwrap_or(&Null), y.get(k).unwrap_or(&Null), out);
            }
        }
        (Array(x), Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                json_diff(&format!("{path}[{i}]"), p, q, out);
            }
        }
        _ if a != b => out.push(format!("{path}: {} -> {}", a.to_string().chars().take(70).collect::<String>(), b.to_string().chars().take(70).collect::<String>())),
        _ => {}
    }
}

/// Step 1 of the real-server check (run with the realm STOPPED): import the six exportable characters into B and keep the
/// store on disk so that step 3 can compare after the real worldserver has loaded and saved them.
#[test]
#[ignore]
fn server_check_1_import_into_the_stopped_realm() {
    let Some(r) = realms() else { return };
    if !staged() {
        return;
    }
    reset_b(&r.b);
    let dir = check_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = Store::open(&dir).unwrap();
    let profile = store.default_profile().unwrap();
    let mut lines = Vec::new();
    for guid in SERVER_CHECK_GUIDS {
        let id = make(&r, &mut store, profile, guid);
        let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts()).unwrap();
        lines.push(format!("{guid} -> guid {} ({} items, {} pets, renamed: {})", outcome.local_guid, outcome.items, outcome.pets, outcome.renamed));
    }
    std::fs::write(dir.join("imported.txt"), lines.join("\n")).unwrap();
    println!("{}", lines.join("\n"));
}

/// Step 3: after the real worldserver has logged the characters in and saved them on shutdown, read them back and say
/// what the server did to them.
#[test]
#[ignore]
fn server_check_3_what_the_real_server_did_to_them() {
    let Some(r) = realms() else { return };
    if !staged() {
        return;
    }
    let store = Store::open(&check_dir()).unwrap();
    let profile = store.list_characters_all().unwrap();
    let mut report = Vec::new();
    for record in profile {
        let id = record.character_id;
        let canonical = store.load_current(id).unwrap();
        let guid = on_b(&store, id)[0].local_guid;
        let online = number(&r.b, &format!("SELECT online FROM acore_characters.characters WHERE guid = {guid}"));
        let prior = store.active_item_lookup(id, B).unwrap();
        let back = export_character(&r.b, guid, Some(id), &prior);
        match back {
            Err(e) => report.push(format!("{} (guid {guid}, online {online}): export refused: {e}", record.name)),
            Ok(e) => {
                let expected = expected_after_a_trip(&canonical, &e.model);
                let mut diffs = Vec::new();
                json_diff("", &serde_json::to_value(&expected).unwrap(), &serde_json::to_value(&e.model).unwrap(), &mut diffs);
                report.push(format!("{} (guid {guid}, online {online}): {} difference(s) after the server loaded and saved it{}", record.name, diffs.len(), if diffs.is_empty() { String::new() } else { format!(":\n    {}", diffs.join("\n    ")) }));
            }
        }
    }
    println!("{}", report.join("\n"));
    std::fs::write(check_dir().join("report.txt"), report.join("\n")).unwrap();
}
