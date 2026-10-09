//! Live tests of Phase 6 against the two real disposable MySQL servers of `live_import.rs`: the selected appearances travel
//! with the character, the account collections are unioned and written with `INSERT IGNORE`, and what the destination does
//! not know stays canonical. `#[ignore]`d; see `live_import.rs` for how to run.
//!
//! The test that brings the result back to realm A rewrites a fixture character of A: it only runs with
//! `COA_PORTABLE_LIVE_MUTATE_A=1` (reset A afterwards with `tools/reset_live_realm.ps1`).

use std::sync::Arc;

use crate::db::Db;
use crate::portable::collection::IdSet;
use crate::portable::ids::CharacterId;
use crate::portable::store::Store;

use super::collections::{apply_set, fingerprint, read_set};
use super::knowledge::RealmKnowledge;
use super::live_import::{counts, first, fresh_store, make, number, realms, reset_b, ACCOUNT, B};
use super::*;

const APPEARANCE: &str = "coa:appearance";
const VANITY: &str = "coa:vanity";

fn knows(appearances: &[u32], vanity: &[u32]) -> Arc<RealmKnowledge> {
    Arc::new(RealmKnowledge::new(
        IdSet::from_ids(appearances.iter().copied()).unwrap(),
        IdSet::from_ids(vanity.iter().copied()).unwrap(),
    ))
}

fn options(knowledge: Arc<RealmKnowledge>) -> ImportOptions {
    ImportOptions {
        knowledge: Some(knowledge),
        ..super::live_import::opts()
    }
}

fn rows(db: &Db, sql: &str) -> Vec<String> {
    db.query(sql)
        .unwrap()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn selected(db: &Db, guid: u32) -> Vec<String> {
    rows(db, &format!("SELECT CONCAT(category_id, '=', appearance_id) FROM acore_characters.character_appearance WHERE guid = {guid} ORDER BY category_id"))
}

fn outfits(db: &Db, guid: u32) -> Vec<String> {
    rows(db, &format!("SELECT CONCAT(name, ':', appearances) FROM acore_characters.character_appearance_outfit WHERE guid = {guid} ORDER BY name"))
}

fn switches(db: &Db, guid: u32) -> Vec<String> {
    rows(db, &format!("SELECT CONCAT(can_see_item, ',', can_see_spell) FROM acore_characters.character_appearance_settings WHERE guid = {guid}"))
}

fn sql(db: &Db, text: &str) {
    db.query(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
}

fn local(store: &Store, id: CharacterId, server: &str) -> u32 {
    store
        .server_mappings(id)
        .unwrap()
        .into_iter()
        .find(|m| m.server_id == server)
        .unwrap()
        .local_guid
}

#[test]
#[ignore]
fn the_selected_appearance_is_exported_and_only_what_the_destination_knows_is_written() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let canonical = store.load_current(id).unwrap();
    assert_eq!(
        canonical.wardrobe.active,
        [(1, 1101), (4, 1104), (56, 1156)].into_iter().collect(),
        "the fixture's selection is in the snapshot"
    );
    assert_eq!(
        canonical.wardrobe.outfits["Sunday best"],
        vec![1101, 0, 0, 1104]
    );
    assert!(canonical.wardrobe.can_see_item && !canonical.wardrobe.can_see_spell);

    // B's client data does not know 1156: that selection stays canonical and is not written
    let imported = import_character(
        &r.b,
        &mut store,
        id,
        B,
        ACCOUNT,
        &options(knows(&[1101, 1104], &[50001])),
    )
    .unwrap();
    let g = imported.local_guid;
    assert_eq!(selected(&r.b, g), ["1=1101", "4=1104"]);
    assert_eq!(outfits(&r.b, g), ["Sunday best:1101 0 0 1104"]);
    assert_eq!(switches(&r.b, g), ["1,0"]);
    assert_eq!(
        store.load_current(id).unwrap().wardrobe,
        canonical.wardrobe,
        "nothing was removed from the canonical character"
    );
    assert!(
        imported.not_applied.is_empty() || !imported.not_applied.contains(&"wardrobe".to_string())
    );

    // no gameplay item is touched by any of this: the realm holds exactly the items of the canonical character
    let back = export_character_with_pets(
        &r.b,
        g,
        Some(id),
        &store.active_item_lookup(id, B).unwrap(),
        &store.active_pet_lookup(id, B).unwrap(),
    )
    .unwrap()
    .model;
    assert_eq!(back.items.len(), canonical.items.len());
    assert_eq!(
        back.wardrobe.active,
        [(1, 1101), (4, 1104)].into_iter().collect()
    );

    // an import without knowledge of the client data writes no appearance at all and says so
    reset_b(&r.b);
    let (mut store2, profile2) = fresh_store();
    let id2 = make(&r, &mut store2, profile2, 1002);
    let plain = import_character(
        &r.b,
        &mut store2,
        id2,
        B,
        ACCOUNT,
        &super::live_import::opts_without_knowledge(),
    )
    .unwrap();
    assert!(
        selected(&r.b, plain.local_guid).is_empty() && outfits(&r.b, plain.local_guid).is_empty()
    );
    assert!(
        plain.not_applied.contains(&"wardrobe".to_string()),
        "{:?}",
        plain.not_applied
    );
}

#[test]
#[ignore]
fn account_collections_are_read_cheaply_unioned_and_written_without_duplicates_or_unknown_ids() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    // realm A's account 5001 holds four appearances and three vanity items (one of them a bank item that is never carried)
    let wardrobe = read_set(&r.a, 5001, APPEARANCE).unwrap();
    let vanity = read_set(&r.a, 5001, VANITY).unwrap();
    assert_eq!(wardrobe.ids(), [1101, 1104, 1156, 1200]);
    assert_eq!(
        vanity.ids(),
        [50001, 50002],
        "the bank vanity item 110000 is not part of the portable collection"
    );

    let (mut store, _) = fresh_store();
    let profile = store.default_profile().unwrap();
    store
        .merge_collection(profile, APPEARANCE, &wardrobe)
        .unwrap();
    store.merge_collection(profile, VANITY, &vanity).unwrap();
    let fingerprint_a = fingerprint(&r.a, 5001, APPEARANCE).unwrap();
    assert_eq!(
        fingerprint(&r.a, 5001, APPEARANCE).unwrap(),
        fingerprint_a,
        "reading the fingerprint is stable"
    );

    // B knows only part of it; the account on B starts empty
    let k = knows(&[1101, 1104], &[50001]);
    let before = counts(&r.b);
    let a = apply_set(&r.b, ACCOUNT, APPEARANCE, &wardrobe, &k).unwrap();
    assert_eq!((a.inserted, a.unknown, a.already), (2, 2, 0));
    let v = apply_set(&r.b, ACCOUNT, VANITY, &vanity, &k).unwrap();
    assert_eq!((v.inserted, v.unknown, v.already), (1, 1, 0));
    assert_eq!(
        read_set(&r.b, ACCOUNT, APPEARANCE).unwrap().ids(),
        [1101, 1104]
    );
    assert_eq!(read_set(&r.b, ACCOUNT, VANITY).unwrap().ids(), [50001]);
    let after = counts(&r.b);
    let changed: Vec<_> = after
        .iter()
        .filter(|(t, n)| before[*t] != **n)
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(
        changed,
        ["account_appearance_collection", "account_vanity_collection"],
        "nothing but the two collection tables moved"
    );
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.account_appearance_collection WHERE account_id = {ACCOUNT} AND source_item <> 0")), 0, "source_item is bookkeeping and written as 0");

    // the same again changes nothing (INSERT IGNORE, and the pre-read skips what is held)
    let again = apply_set(&r.b, ACCOUNT, APPEARANCE, &wardrobe, &k).unwrap();
    assert_eq!((again.inserted, again.already), (0, 2));
    assert_eq!(counts(&r.b), after);
    let fingerprint_b = fingerprint(&r.b, ACCOUNT, APPEARANCE).unwrap();

    // a realm that gains an unlock changes the fingerprint; a destination that later learns more gets exactly the rest
    sql(&r.b, &format!("INSERT INTO acore_characters.account_appearance_collection (account_id, appearance_id, source_item) VALUES ({ACCOUNT}, 9999, 77)"));
    assert_ne!(
        fingerprint(&r.b, ACCOUNT, APPEARANCE).unwrap(),
        fingerprint_b
    );
    let richer = knows(&[1101, 1104, 1156, 1200], &[50001, 50002]);
    let rest = apply_set(&r.b, ACCOUNT, APPEARANCE, &wardrobe, &richer).unwrap();
    assert_eq!((rest.inserted, rest.unknown, rest.already), (2, 0, 2));
    assert_eq!(
        read_set(&r.b, ACCOUNT, APPEARANCE).unwrap().ids(),
        [1101, 1104, 1156, 1200, 9999],
        "a realm's own row is never removed"
    );

    // a collection of a Wildcard realm is refused, and so is an account that does not exist
    assert!(apply_set(&r.b, 99_999_999, APPEARANCE, &wardrobe, &richer).is_err());
    assert_eq!(first(&r.b, "SELECT COUNT(*) FROM acore_characters.account_appearance_collection WHERE account_id = 99999999"), "0");
}

#[test]
#[ignore]
fn a_session_of_appearance_changes_comes_back_and_a_late_update_restores_what_was_held_back() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let c0 = store.load_current(id).unwrap();
    let narrow = options(knows(&[1101, 1104, 1300], &[]));
    let g = import_character(&r.b, &mut store, id, B, ACCOUNT, &narrow)
        .unwrap()
        .local_guid;

    // the session on B: the realm's own first load changed nothing; the player changes the look
    begin_session(&r.b, &mut store, id, B).unwrap();
    sql(
        &r.b,
        &format!(
            "DELETE FROM acore_characters.character_appearance WHERE guid = {g} AND category_id = 4;
             INSERT INTO acore_characters.character_appearance (guid, category_id, appearance_id) VALUES ({g}, 2, 1104), ({g}, 5, 1300);
             UPDATE acore_characters.character_appearance_settings SET can_see_spell = 1 WHERE guid = {g};
             DELETE FROM acore_characters.character_appearance_outfit WHERE guid = {g};
             INSERT INTO acore_characters.character_appearance_outfit (guid, name, appearances) VALUES ({g}, 'Night', '1104 1300');"
        ),
    );
    let outcome = reconcile_session(&r.b, &mut store, id, B, true, Some("test")).unwrap();
    assert!(outcome.new_revision);
    let c1 = store.load_current(id).unwrap();
    assert_eq!(
        c1.wardrobe.active,
        [(1, 1101), (2, 1104), (5, 1300), (56, 1156)]
            .into_iter()
            .collect(),
        "the new choices are canonical, the selection B could not show is still there"
    );
    assert!(c1.wardrobe.can_see_item && c1.wardrobe.can_see_spell);
    assert_eq!(
        c1.wardrobe.outfits.keys().cloned().collect::<Vec<_>>(),
        ["Night"],
        "the deleted outfit is gone"
    );
    assert_eq!(
        c1.items.len(),
        c0.items.len(),
        "not one gameplay item was created or deleted by the session"
    );

    // the character is updated on B from a newer canonical revision made elsewhere: B's selection follows, its own stays
    let mut c2 = c1.clone();
    c2.wardrobe.active.insert(7, 1101);
    c2.wardrobe.active.insert(9, 7777); // B does not know 7777
    store
        .commit_snapshot(id, 2, c2.normalized(), "realm-a", Some("played elsewhere"))
        .unwrap();
    let updated = update_realm_character(&r.b, &mut store, id, B, &narrow).unwrap();
    assert!(updated.updated);
    assert_eq!(
        selected(&r.b, g),
        ["1=1101", "2=1104", "5=1300", "7=1101"],
        "known ids written, 56 and 9 held back"
    );
    assert_eq!(outfits(&r.b, g), ["Night:1104 1300"]);
    assert_eq!(
        store.load_current(id).unwrap().wardrobe.active.len(),
        6,
        "and canonical keeps all six"
    );
    // nothing but the three appearance tables, the character's marker settings and the journal changed in the realm
    assert!(
        !update_realm_character(&r.b, &mut store, id, B, &narrow)
            .unwrap()
            .updated
    );
}

#[test]
#[ignore]
fn the_result_comes_back_to_realm_a_unchanged_gameplay_and_complete_collections() {
    if std::env::var("COA_PORTABLE_LIVE_MUTATE_A").ok().as_deref() != Some("1") {
        return;
    }
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let c0 = store.load_current(id).unwrap();
    let a_guid = local(&store, id, "realm-a");
    let a_items_before = rows(&r.a, &format!("SELECT CONCAT(itemEntry, ':', count) FROM acore_characters.item_instance WHERE owner_guid = {a_guid} ORDER BY guid"));
    let a_known = knows(&[1101, 1104, 1156, 1200, 1300], &[50001, 50002]);
    // the fixture holds a character flagged online (a blocker test case); a realm that is stopped has none
    sql(&r.a, "UPDATE acore_characters.characters SET online = 0");

    // out to B, play there (a new selection and a new unlock), reconcile
    let narrow = options(knows(&[1101, 1104, 1300], &[50001]));
    let g = import_character(&r.b, &mut store, id, B, ACCOUNT, &narrow)
        .unwrap()
        .local_guid;
    begin_session(&r.b, &mut store, id, B).unwrap();
    sql(&r.b, &format!("INSERT INTO acore_characters.character_appearance (guid, category_id, appearance_id) VALUES ({g}, 5, 1300); INSERT INTO acore_characters.account_appearance_collection (account_id, appearance_id, source_item) VALUES ({ACCOUNT}, 1300, 4), ({ACCOUNT}, 1104, 5);"));
    reconcile_session(&r.b, &mut store, id, B, true, None).unwrap();
    for kind in [APPEARANCE, VANITY] {
        store
            .merge_collection(profile, kind, &read_set(&r.b, ACCOUNT, kind).unwrap())
            .unwrap();
        store
            .merge_collection(profile, kind, &read_set(&r.a, 5001, kind).unwrap())
            .unwrap();
    }

    // back to A: the character in place, the account's unlocks by union
    let a_before = read_set(&r.a, 5001, APPEARANCE).unwrap();
    let updated =
        update_realm_character(&r.a, &mut store, id, "realm-a", &options(a_known.clone())).unwrap();
    assert!(updated.updated, "{updated:?}");
    let (_, canonical_wardrobe) = store.collection(profile, APPEARANCE).unwrap().unwrap();
    let applied = apply_set(&r.a, 5001, APPEARANCE, &canonical_wardrobe, &a_known).unwrap();
    assert_eq!(applied.inserted, 1, "only 1300 is new to A");
    assert_eq!(
        read_set(&r.a, 5001, APPEARANCE).unwrap().ids(),
        [1101, 1104, 1156, 1200, 1300],
        "nothing was lost, nothing was duplicated"
    );
    assert!(a_before
        .ids()
        .iter()
        .all(|id| read_set(&r.a, 5001, APPEARANCE).unwrap().contains(*id)));
    assert_eq!(
        selected(&r.a, a_guid),
        ["1=1101", "4=1104", "5=1300", "56=1156"]
    );
    assert_eq!(rows(&r.a, &format!("SELECT CONCAT(itemEntry, ':', count) FROM acore_characters.item_instance WHERE owner_guid = {a_guid} ORDER BY guid")), a_items_before, "the original gameplay items are untouched");
    assert_eq!(c0.items.len(), store.load_current(id).unwrap().items.len());
}

#[test]
#[ignore]
fn ten_thousand_and_fifty_thousand_ids_are_cheap_on_a_real_database() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    for count in [10_000u32, 50_000] {
        sql(&r.b, &format!("DELETE FROM acore_characters.account_appearance_collection WHERE account_id = {ACCOUNT}"));
        let scattered: Vec<u32> = (0..count).map(|i| 100_000 + i * 7 + (i * i) % 5).collect();
        let set = IdSet::from_ids(scattered.iter().copied()).unwrap();
        let k = Arc::new(RealmKnowledge::new(set.clone(), IdSet::new()));

        let started = std::time::Instant::now();
        let applied = apply_set(&r.b, ACCOUNT, APPEARANCE, &set, &k).unwrap();
        let write = started.elapsed();
        assert_eq!(
            (applied.inserted, applied.unknown, applied.already),
            (count as usize, 0, 0)
        );

        // the unchanged look: one aggregate query, whatever the size
        let started = std::time::Instant::now();
        let first = fingerprint(&r.b, ACCOUNT, APPEARANCE).unwrap();
        let look = started.elapsed();
        assert_eq!(fingerprint(&r.b, ACCOUNT, APPEARANCE).unwrap(), first);

        let started = std::time::Instant::now();
        let read = read_set(&r.b, ACCOUNT, APPEARANCE).unwrap();
        let full = started.elapsed();
        assert_eq!(read, set, "{count} ids come back exactly");

        // the second apply reads what is held and writes nothing
        let started = std::time::Instant::now();
        let again = apply_set(&r.b, ACCOUNT, APPEARANCE, &set, &k).unwrap();
        let repeat = started.elapsed();
        assert_eq!((again.inserted, again.already), (0, count as usize));
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.account_appearance_collection WHERE account_id = {ACCOUNT}")), count as u64, "no duplicates");

        let encoded = set.encode().len();
        println!("{count} ids: write {write:?}, fingerprint {look:?}, full read {full:?}, repeat {repeat:?}, compact set {encoded} bytes");
        assert!(
            look < std::time::Duration::from_secs(2),
            "the cheap look took {look:?}"
        );
        assert!(
            write < std::time::Duration::from_secs(30) && full < std::time::Duration::from_secs(30),
            "{write:?} {full:?}"
        );
        assert!(encoded < count as usize * 3);
    }
    sql(&r.b, &format!("DELETE FROM acore_characters.account_appearance_collection WHERE account_id = {ACCOUNT}"));
}
