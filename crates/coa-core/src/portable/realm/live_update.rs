//! Live tests of Phase 4 against the two real disposable MySQL servers of `live_import.rs` (realm A = source, realm B =
//! destination). No worldserver is involved here; `live_roundtrip.rs` adds the real servers.
//!
//! ```text
//! COA_PORTABLE_LIVE="<bin>|13998|<pw>"  COA_PORTABLE_LIVE_B="<bin>|13997|<pw>"  cargo test -p coa-core portable::realm::live_update -- --ignored --test-threads=1
//! ```

use std::collections::BTreeMap;

use crate::db::Db;
use crate::portable::ids::{CharacterId, PortableItemId, ProfileId};
use crate::portable::merge::{merge3, Mode};
use crate::portable::model::*;
use crate::portable::store::{ImportState, Presence, Store, UpdatePlan};

use super::live_import::{counts, expected_after_a_trip, first, fresh_store, json_diff, make, number, opts, read_back, realms, reset_b, Realms, ACCOUNT, B};
use super::reconcile::resolve_import_update;
use super::update::{build_update, UpdateContext};
use super::*;

const EMPTY_ENCHANTS: &str = "0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 ";

fn sql(db: &Db, text: &str) {
    db.query(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
}

/// Make `a_guid` portable on realm A and import it into B; returns (portable id, local guid on B).
fn join(r: &Realms, store: &mut Store, profile: ProfileId, a_guid: u32) -> (CharacterId, u32) {
    let id = make(r, store, profile, a_guid);
    let outcome = import_character(&r.b, store, id, B, ACCOUNT, &opts()).unwrap_or_else(|e| panic!("import of {a_guid}: {e}"));
    (id, outcome.local_guid)
}

/// Everything about a character that is the realm's own business: where it stands, where it is bound, its auras,
/// cooldowns and instance saves. An in-place update must leave this exactly as it is.
fn world_local(db: &Db, guid: u32) -> Vec<String> {
    let one = |sql: String| db.query(&sql).unwrap().trim().to_string();
    vec![
        one(format!("SELECT CONCAT_WS('|', guid, account, map, zone, position_x, position_y, position_z, orientation, online, at_login, totaltime, logout_time, instance_id, cinematic, extra_flags) FROM acore_characters.characters WHERE guid = {guid}")),
        one(format!("SELECT CONCAT_WS('|', mapId, zoneId, posX, posY, posZ) FROM acore_characters.character_homebind WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', spell, item, time) ORDER BY spell) FROM acore_characters.character_spell_cooldown WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', quest, time) ORDER BY quest) FROM acore_characters.character_queststatus_daily WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', instance, permanent) ORDER BY instance) FROM acore_characters.character_instance WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', spell, remainTime) ORDER BY spell) FROM acore_characters.character_aura WHERE guid = {guid}")),
    ]
}

/// Give a character realm-local state that the portable model does not carry and that differs from anything the import wrote.
fn make_local(db: &Db, guid: u32) {
    sql(
        db,
        &format!(
            "UPDATE acore_characters.characters SET map = 1, zone = 14, position_x = -618.5, position_y = -4251.75, position_z = 38.25, orientation = 1.5, totaltime = 7777, logout_time = 1700000000 WHERE guid = {guid};
             UPDATE acore_characters.character_homebind SET mapId = 1, zoneId = 14, posX = -600.5, posY = -4200.25, posZ = 40.5 WHERE guid = {guid};
             INSERT INTO acore_characters.character_spell_cooldown (guid, spell, category, item, time, needSend) VALUES ({guid}, 2096, 0, 0, 2000000000, 0);
             INSERT INTO acore_characters.character_queststatus_daily (guid, quest, time) VALUES ({guid}, 9999, 1700000000);
             INSERT INTO acore_characters.character_instance (guid, instance, permanent, extended) VALUES ({guid}, 5, 1, 0);
             INSERT INTO acore_characters.character_aura (guid, casterGuid, itemGuid, spell, effectMask, recalculateMask, stackCount, maxDuration, remainTime) VALUES ({guid}, 0, 0, 1459, 1, 0, 1, 3600000, 3000000);"
        ),
    );
}

fn free_slot(db: &Db, guid: u32) -> u8 {
    let used: Vec<u64> = db.query(&format!("SELECT slot FROM acore_characters.character_inventory WHERE guid = {guid} AND bag = 0")).unwrap().lines().filter_map(|l| l.trim().parse().ok()).collect();
    (23u8..=38).chain(39..=66).find(|s| !used.contains(&(*s as u64))).expect("a free slot")
}

/// A new item of the realm's own (what a realm hands out, or a player loots), written the way the realm writes it.
fn realm_gives(db: &Db, guid: u32, entry: u64) -> u32 {
    let item = number(db, "SELECT MAX(guid) + 1 FROM acore_characters.item_instance") as u32;
    let slot = free_slot(db, guid);
    sql(
        db,
        &format!(
            "INSERT INTO acore_characters.item_instance (guid, itemEntry, owner_guid, count, enchantments) VALUES ({item}, {entry}, {guid}, 1, '{EMPTY_ENCHANTS}');
             INSERT INTO acore_characters.character_inventory (guid, bag, slot, item) VALUES ({guid}, 0, {slot}, {item});"
        ),
    );
    item
}

/// Remove an item from a realm character the way a realm does (inventory row and instance).
fn realm_takes(db: &Db, guid: u32, item: u32) {
    sql(db, &format!("DELETE FROM acore_characters.character_inventory WHERE guid = {guid} AND item = {item}; DELETE FROM acore_characters.item_instance WHERE guid = {item} AND owner_guid = {guid};"));
}

/// The local item guid a portable item has on B.
fn local_item(store: &Store, id: CharacterId, item: PortableItemId) -> u32 {
    store.item_mappings(id, B).unwrap().into_iter().find(|m| m.portable_item_id == item && m.active).unwrap_or_else(|| panic!("item {item} is not mapped")).local_item_guid
}

/// Plain items of a model that hold nothing (safe to take away or hold back).
fn plain(m: &PortableCharacter) -> Vec<PortableItemId> {
    let containers: std::collections::HashSet<PortableItemId> = m.items.iter().filter_map(|i| i.container).collect();
    m.items.iter().filter(|i| i.container.is_none() && !containers.contains(&i.id) && i.slot >= 23).map(|i| i.id).collect()
}

fn state_of(store: &Store, id: CharacterId) -> (u64, u64) {
    (store.character(id).unwrap().revision, store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == B).unwrap().last_revision)
}

fn spells_of(r: &Realms, guid: u32) -> Vec<u32> {
    r.a.query(&format!("SELECT spell FROM acore_characters.character_spell WHERE guid = {guid} ORDER BY spell")).unwrap().lines().filter_map(|l| l.trim().parse().ok()).collect()
}

#[test]
#[ignore]
fn an_update_changes_the_portable_subset_in_place_and_keeps_everything_local() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    make_local(&r.b, g);
    let local_before = world_local(&r.b, g);
    let counts_before = counts(&r.b);
    let max_item_before = number(&r.b, "SELECT MAX(guid) FROM acore_characters.item_instance");
    let c0 = store.load_current(id).unwrap();

    // the canonical character moves on (it was played somewhere else): more money and xp, one item gone, one new, a spell, a quest
    let extra_spell = *spells_of(&r, 1004).iter().find(|s| !c0.build.spells.iter().any(|(k, _)| k == *s)).expect("a spell 1002 lacks");
    let gone = plain(&c0)[0];
    let mut c1 = c0.clone();
    c1.progression.money += 123_456;
    c1.progression.xp += 1_000;
    c1.progression.honor.total_kills += 7;
    c1.items.retain(|i| i.id != gone);
    let mut loot = c0.items.iter().find(|i| i.id == plain(&c0)[1]).unwrap().clone();
    loot.id = PortableItemId::new();
    loot.slot = 66;
    c1.items.push(loot.clone());
    c1.build.spells.push((extra_spell, 255));
    c1.quests.rewarded.push(4242);
    c1.reputation.push(ReputationEntry { faction: 69, standing: 300, flags: 1 });
    let c1 = c1.normalized();
    store.commit_snapshot(id, 1, c1.clone(), "realm-a", Some("played elsewhere")).unwrap();

    let outcome = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(outcome.updated);
    assert_eq!((outcome.from_revision, outcome.to_revision), (1, 2));
    assert_eq!((outcome.counts.items_added, outcome.counts.items_removed), (1, 1), "{:?}", outcome.counts);

    // the same character, in place: guid, mapping, and every piece of realm-local state are untouched
    assert_eq!(world_local(&r.b, g), local_before, "position, homebind, auras, cooldowns and instance saves are the realm's own");
    assert_eq!(store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == B).unwrap().local_guid, g);
    assert_eq!(state_of(&store, id), (2, 2));
    let after = counts(&r.b);
    let changed: BTreeMap<_, _> = after.iter().filter(|(t, n)| counts_before[*t] != **n).map(|(t, n)| (t.clone(), (counts_before[t], *n))).collect();
    assert_eq!(
        changed.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["character_queststatus_rewarded", "character_reputation", "character_settings", "character_spell"],
        "only the tables the change is about gained rows: {changed:?}"
    );
    assert_eq!(after["characters"], counts_before["characters"], "the character is never deleted or re-created");
    assert_eq!(after["item_instance"], counts_before["item_instance"], "one item out, one in");

    // the realm now holds exactly the new canonical character
    let back = read_back(&r, &store, id);
    let mut diffs = Vec::new();
    json_diff("", &serde_json::to_value(expected_after_a_trip(&c1, &back)).unwrap(), &serde_json::to_value(&back).unwrap(), &mut diffs);
    assert!(diffs.is_empty(), "{diffs:#?}");
    // the removed item's mapping is retired, the new one is mapped to a guid above everything the realm names
    let mapped = store.item_mappings(id, B).unwrap();
    assert!(mapped.iter().all(|m| m.portable_item_id != gone));
    assert!(local_item(&store, id, loot.id) as u64 > max_item_before, "the new item's guid is allocated above every item guid of the realm");
    // nothing is left unfinished, and asking again is a no-op
    assert!(store.open_imports(B).unwrap().is_empty());
    let again = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(!again.updated);
    assert_eq!(counts(&r.b), after);
}

#[test]
#[ignore]
fn a_failed_update_changes_not_one_row() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    make_local(&r.b, g);
    let before = counts(&r.b);
    let local_before = world_local(&r.b, g);
    let mut c1 = store.load_current(id).unwrap();
    c1.progression.money += 5;
    c1.build.spells.push((500_090, 255));
    store.commit_snapshot(id, 1, c1.normalized(), "realm-a", None).unwrap();

    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    r.b.query("DELIMITER //\nCREATE TRIGGER acore_characters.coa_test_fail BEFORE INSERT ON acore_characters.character_spell FOR EACH ROW BEGIN IF NEW.spell = 500090 THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'injected failure'; END IF; END//\nDELIMITER ;").unwrap();
    let result = update_realm_character(&r.b, &mut store, id, B, &opts());
    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    let error = result.expect_err("the injected failure must fail the update").to_string();
    assert!(error.contains("injected failure") || error.contains("database command failed"), "{error}");

    assert_eq!(counts(&r.b), before);
    assert_eq!(world_local(&r.b, g), local_before);
    assert_eq!(number(&r.b, &format!("SELECT money FROM acore_characters.characters WHERE guid = {g}")), store.load_snapshot(id, 1).unwrap().progression.money as u64, "the money change that ran before the failure was rolled back");
    assert_eq!(state_of(&store, id), (2, 1), "no mapping or revision moved");
    assert!(store.open_imports(B).unwrap().is_empty(), "the journal entry is closed");
    let entries = r.b.query("SELECT GET_LOCK('coa_portable_import', 0)").unwrap();
    assert_eq!(entries.trim(), "1", "the import lock was released");
    r.b.query("DO RELEASE_LOCK('coa_portable_import')").unwrap();
    // and the update works once the cause is gone
    let outcome = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(outcome.updated);
    assert_eq!(state_of(&store, id), (2, 2));
}

/// The update flow of `update_realm_character` up to (not including) the realm run, so a test can interrupt it.
fn prepare_update(r: &Realms, store: &mut Store, id: CharacterId) -> (crate::portable::ids::ImportId, String) {
    let guid = store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == B).unwrap().local_guid;
    let prior_items = store.active_item_lookup(id, B).unwrap();
    let prior_pets = store.active_pet_lookup(id, B).unwrap();
    let exported = export_character_with_pets(&r.b, guid, Some(id), &prior_items, &prior_pets).unwrap();
    let synced = store.synced_model(id, B).unwrap().unwrap();
    let canonical = store.load_current(id).unwrap();
    let merged = merge3(&exported.model, &synced, &canonical, Mode::Strict).unwrap();
    assert!(merged.conflicts.is_empty());
    let items = exported.observations.iter().map(|o| (o.portable_item_id, o.local_item_guid)).collect();
    let pets = exported.pet_observations.iter().map(|o| (o.portable_pet_id, o.local_pet_number)).collect();
    let schema = probe(&r.b).unwrap();
    let users = opts().game_server_users;
    let revision = canonical_revision(store, id);
    let ctx = |nonce| UpdateContext { ruleset: Ruleset::Coa, local_guid: guid, revision, nonce, game_server_users: &users, probe: &schema, items: &items, pets: &pets, session: None };
    let draft = build_update(&exported.model, &merged.model, &ctx([0; 4])).unwrap();
    let by_id: std::collections::HashMap<_, _> = merged.model.items.iter().map(|i| (i.id, i)).collect();
    let plan = UpdatePlan {
        added_items: draft.added_items.iter().map(|i| crate::portable::store::PlannedItem { id: *i, entry: by_id[i].entry.clone(), identity: crate::portable::identity::item_identity(&by_id[i].entry, by_id[i].random_property_id) }).collect(),
        retired_items: draft.removed_items.clone(),
        ..UpdatePlan::default()
    };
    let ticket = store.begin_update(id, B, revision, plan).unwrap();
    let script = build_update(&exported.model, &merged.model, &ctx(ticket.nonce)).unwrap();
    (ticket.import_id, script.script)
}

fn canonical_revision(store: &Store, id: CharacterId) -> u64 {
    store.character(id).unwrap().revision
}

#[test]
#[ignore]
fn an_update_whose_answer_was_lost_is_finished_by_recovery_and_one_that_never_ran_is_aborted() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    let c0 = store.load_current(id).unwrap();
    let gone = plain(&c0)[0];
    let mut c1 = c0.clone();
    c1.progression.money += 10;
    c1.items.retain(|i| i.id != gone);
    let mut loot = c0.items.iter().find(|i| i.id == plain(&c0)[1]).unwrap().clone();
    loot.id = PortableItemId::new();
    loot.slot = 66;
    c1.items.push(loot.clone());
    store.commit_snapshot(id, 1, c1.normalized(), "realm-a", None).unwrap();

    // (a) the realm commits, the Manager never hears of it
    let (import_id, script) = prepare_update(&r, &mut store, id);
    assert!(matches!(store.begin_update(id, B, 2, UpdatePlan::default()), Err(PortableError::ImportInProgress { .. })), "one unfinished update per pair");
    r.b.query(&script).unwrap();
    assert_eq!(state_of(&store, id), (2, 1), "the Manager still believes the realm is at revision 1");
    assert!(store.item_mappings(id, B).unwrap().iter().any(|m| m.portable_item_id == gone), "and still maps the removed item");
    let reports = recover_imports(&r.b, &mut store, B, &opts()).unwrap();
    assert_eq!(reports.len(), 1);
    assert!(matches!(reports[0].resolution, Resolution::Committed(a) if a.local_guid == g), "{:?}", reports[0].resolution);
    assert_eq!(state_of(&store, id), (2, 2));
    assert!(store.item_mappings(id, B).unwrap().iter().all(|m| m.portable_item_id != gone));
    let new_guid = local_item(&store, id, loot.id);
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE guid = {new_guid} AND owner_guid = {g}")), 1, "the recorded allocation is the realm's");
    assert_eq!(store.import_entry(import_id).unwrap().state, ImportState::Committed);
    assert!(recover_imports(&r.b, &mut store, B, &opts()).unwrap().is_empty());

    // (b) the Manager journaled the update and died before the realm ran it
    let mut c2 = store.load_current(id).unwrap();
    c2.progression.money += 1;
    store.commit_snapshot(id, 2, c2, "realm-a", None).unwrap();
    let before = counts(&r.b);
    let (import_id, _script_never_run) = prepare_update(&r, &mut store, id);
    let reports = recover_imports(&r.b, &mut store, B, &opts()).unwrap();
    assert!(matches!(reports[0].resolution, Resolution::Aborted), "{:?}", reports[0].resolution);
    assert_eq!(store.import_entry(import_id).unwrap().state, ImportState::Aborted);
    assert_eq!(counts(&r.b), before);
    assert_eq!(state_of(&store, id), (3, 2));
    // the update can simply be done again
    assert!(update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap().updated);
    assert_eq!(state_of(&store, id), (3, 3));
    // a resolved entry resolves the same way again
    assert!(matches!(resolve_import_update(&r.b, &mut store, import_id, &opts(), false).unwrap(), Resolution::Aborted));
}

#[test]
#[ignore]
fn an_update_is_refused_while_the_realm_runs_the_character_is_online_or_a_session_is_open() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    let mut c1 = store.load_current(id).unwrap();
    c1.progression.money += 1;
    store.commit_snapshot(id, 1, c1.normalized(), "realm-a", None).unwrap();
    let before = counts(&r.b);

    // a session of the game server's database user = the realm is running
    let refused = std::thread::scope(|s| {
        let holder = s.spawn(|| {
            let _ = r.server_user.query("SELECT SLEEP(6)");
        });
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let result = update_realm_character(&r.b, &mut store, id, B, &opts());
        holder.join().unwrap();
        result
    });
    assert!(matches!(&refused, Err(PortableError::ImportRefused(p)) if p.iter().any(|p| matches!(p, ImportProblem::RealmRunning { .. }))), "{refused:?}");
    assert_eq!(counts(&r.b), before);
    assert!(store.open_imports(B).unwrap().is_empty(), "refused before anything was journaled");

    // a character that is online cannot be read, so it cannot be updated either
    sql(&r.b, &format!("UPDATE acore_characters.characters SET online = 1 WHERE guid = {g}"));
    assert!(matches!(update_realm_character(&r.b, &mut store, id, B, &opts()), Err(PortableError::NotExportable(_))));
    sql(&r.b, "UPDATE acore_characters.characters SET online = 0");

    // a session cannot start from an old base: the realm is at revision 1, the canonical character at 2
    let started = begin_session(&r.b, &mut store, id, B);
    assert!(matches!(started, Err(PortableError::StaleRevision { expected: 1, current: 2 })), "{started:?}");
    assert!(update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap().updated);

    // an open session blocks updates: reconcile first
    begin_session(&r.b, &mut store, id, B).unwrap();
    let mut c = store.load_current(id).unwrap();
    c.progression.money += 1;
    store.commit_snapshot(id, 2, c.normalized(), "realm-a", None).unwrap();
    assert!(matches!(update_realm_character(&r.b, &mut store, id, B, &opts()), Err(PortableError::SessionOpen)));
    // and a session whose canonical character moved under it cannot be reconciled
    let r2 = reconcile_session(&r.b, &mut store, id, B, true, None);
    assert!(matches!(r2, Err(PortableError::StaleRevision { expected: 2, current: 3 })), "{r2:?}");
    assert_eq!(store.character(id).unwrap().revision, 3);
}

#[test]
#[ignore]
fn both_sides_changing_the_same_thing_differently_is_a_conflict_and_nothing_is_written() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    let c0 = store.load_current(id).unwrap();
    let faction = c0.reputation[0].faction;
    let standing = c0.reputation[0].standing;
    // the realm raised a reputation (played without a session) while the canonical character raised it differently
    sql(&r.b, &format!("UPDATE acore_characters.character_reputation SET standing = {} WHERE guid = {g} AND faction = {faction}", standing + 111));
    let mut c1 = c0.clone();
    c1.reputation.iter_mut().find(|e| e.faction == faction).unwrap().standing = standing + 222;
    c1.progression.money += 77;
    store.commit_snapshot(id, 1, c1.normalized(), "realm-a", None).unwrap();
    let before = counts(&r.b);
    let result = update_realm_character(&r.b, &mut store, id, B, &opts());
    match &result {
        Err(PortableError::UpdateConflicts(list)) => assert!(list.iter().any(|c| c.contains("reputation")), "{list:?}"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(counts(&r.b), before);
    assert_eq!(number(&r.b, &format!("SELECT money FROM acore_characters.characters WHERE guid = {g}")), c0.progression.money as u64, "not even the money, which would not have conflicted, was written");
    assert!(store.open_imports(B).unwrap().is_empty());
    assert_eq!(state_of(&store, id), (2, 1));

    // where only one side changed a thing, both changes survive: the realm's own reputation change and the canonical money
    let mut c2 = c0.clone();
    c2.progression.money += 77;
    store.commit_snapshot(id, 2, c2.normalized(), "realm-a", None).unwrap();
    let out = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(out.updated);
    assert_eq!(number(&r.b, &format!("SELECT money FROM acore_characters.characters WHERE guid = {g}")), c0.progression.money as u64 + 77);
    assert_eq!(first(&r.b, &format!("SELECT standing FROM acore_characters.character_reputation WHERE guid = {g} AND faction = {faction}")).trim().parse::<i64>().unwrap(), (standing + 111) as i64, "the realm's own unreconciled change is left alone");
}

/// The central scenario: a realm filters items before the baseline, the player later gets rid of others and finds new ones.
#[test]
#[ignore]
fn filtered_items_stay_canonical_and_deleted_ones_do_not_and_a_checkpoint_never_counts_twice() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    let c0 = store.load_current(id).unwrap();
    let candidates = plain(&c0);
    let (held_back, sold_later, kept) = (&candidates[0..3], candidates[3], candidates[4]);

    // --- the realm's own first load/save: it holds three items back, hands out one of its own, adds default spells, resets honor ---
    let extra_spells: Vec<u32> = spells_of(&r, 1004).into_iter().filter(|s| !c0.build.spells.iter().any(|(k, _)| k == s)).take(2).collect();
    for item in held_back {
        let guid = local_item(&store, id, *item);
        sql(&r.b, &format!("DELETE FROM acore_characters.character_inventory WHERE guid = {g} AND item = {guid}"));
        // a realm mails what it cannot place: the instance row stays, owned by the character, in a mail
        sql(&r.b, &format!("INSERT INTO acore_characters.mail_items (mail_id, item_guid, receiver) VALUES (99000, {guid}, {g})"));
    }
    let realm_own = realm_gives(&r.b, g, c0.items.iter().find(|i| i.id == kept).unwrap().entry.id());
    for s in &extra_spells {
        sql(&r.b, &format!("INSERT INTO acore_characters.character_spell (guid, spell, specMask) VALUES ({g}, {s}, 255)"));
    }
    sql(&r.b, &format!("UPDATE acore_characters.characters SET todayHonorPoints = 0, yesterdayHonorPoints = 0, chosenTitle = 0 WHERE guid = {g}"));

    let start = begin_session(&r.b, &mut store, id, B).unwrap();
    assert_eq!(start.c0_revision, 1);
    assert_eq!(start.items_filtered, 3, "the three held-back items are recognised as filtered, not as deleted");
    assert_eq!(start.items_realm_local, 1, "the realm's own item is recognised as the realm's");
    let mapped = store.item_mappings(id, B).unwrap();
    for item in held_back {
        let m = mapped.iter().find(|m| m.portable_item_id == *item).unwrap();
        assert!(m.active && m.presence == Presence::Filtered, "an item that is absent from the inventory at B0 is not retired");
    }
    assert!(mapped.iter().any(|m| m.local_item_guid == realm_own && m.presence == Presence::RealmLocal));

    // --- the player plays: gets rid of one item, finds one, earns money ---
    let sold = local_item(&store, id, sold_later);
    realm_takes(&r.b, g, sold);
    let loot_entry = c0.items.iter().find(|i| i.id == candidates[5]).unwrap().entry.id();
    let loot_guid = realm_gives(&r.b, g, loot_entry);
    sql(&r.b, &format!("UPDATE acore_characters.characters SET money = money + 1000, totalKills = totalKills + 4 WHERE guid = {g}"));

    let first = reconcile_session(&r.b, &mut store, id, B, false, Some("checkpoint 1")).unwrap();
    assert_eq!(first.revision, 2);
    let c1 = store.load_current(id).unwrap();
    assert!(held_back.iter().all(|h| c1.items.iter().any(|i| i.id == *h)), "everything the realm held back is still canonical");
    assert!(c1.items.iter().all(|i| i.id != sold_later), "the item the player got rid of is gone");
    assert!(c1.items.iter().any(|i| i.entry.id() == loot_entry && !c0.items.iter().any(|o| o.id == i.id)), "the loot is canonical");
    assert_eq!(c1.items.len(), c0.items.len() - 1 + 1, "three filtered stay, one sold, one found; the realm's own item does not count");
    assert_eq!(c1.progression.money, c0.progression.money + 1000);
    assert_eq!(c1.progression.honor.total_kills, c0.progression.honor.total_kills + 4);
    for s in &extra_spells {
        assert!(!c1.build.spells.iter().any(|(k, _)| k == s), "a spell the realm added by itself is normalisation, not progress");
    }
    assert_eq!(c1.progression.honor.today_honor, c0.progression.honor.today_honor, "the realm's honor reset does not overwrite the canonical value");
    assert_eq!(c1.progression.chosen_title, c0.progression.chosen_title, "the realm's title normalisation does not overwrite the canonical value");

    // --- more play, a second checkpoint: only the whole B0 -> B1 delta counts, once ---
    sql(&r.b, &format!("UPDATE acore_characters.characters SET money = money + 50 WHERE guid = {g}"));
    let second = reconcile_session(&r.b, &mut store, id, B, false, Some("checkpoint 2")).unwrap();
    assert_eq!(second.revision, 3);
    let c2 = store.load_current(id).unwrap();
    assert_eq!(c2.progression.money, c0.progression.money + 1050, "1000 was not counted again");
    assert_eq!(c2.items.len(), c1.items.len());
    // nothing played: no new revision
    assert_eq!(reconcile_session(&r.b, &mut store, id, B, false, None).unwrap().revision, 3);
    assert_eq!(store.list_revisions(id).unwrap().len(), 3);

    // --- the item the player found is mapped, the realm's own one stays the realm's, the filtered ones stay mapped ---
    let mapped = store.item_mappings(id, B).unwrap();
    assert!(mapped.iter().any(|m| m.local_item_guid == loot_guid && m.presence == Presence::Present));
    assert!(mapped.iter().any(|m| m.local_item_guid == realm_own && m.presence == Presence::RealmLocal));
    assert!(held_back.iter().all(|h| mapped.iter().any(|m| m.portable_item_id == *h && m.active && m.presence == Presence::Filtered)));
    assert!(mapped.iter().all(|m| m.portable_item_id != sold_later || !m.active));

    // --- closing the session, then an in-place update with a newer canonical state ---
    let last = reconcile_session(&r.b, &mut store, id, B, true, Some("end")).unwrap();
    assert_eq!(last.revision, 3);
    assert!(store.open_baseline(id, B).unwrap().is_none());
    let mut c3 = c2.clone();
    c3.progression.money += 5;
    store.commit_snapshot(id, 3, c3.normalized(), "realm-a", Some("played on another realm")).unwrap();
    let before_items = number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {g}"));
    let updated = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(updated.updated);
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {g}")), before_items, "nothing was added or removed");
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE guid = {realm_own} AND owner_guid = {g}")), 1, "the realm's own item survived the update");
    for item in held_back {
        let guid = local_item(&store, id, *item);
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.mail_items WHERE item_guid = {guid}")), 1, "a held-back item is not resurrected into the inventory by an update");
        assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_inventory WHERE item = {guid}")), 0);
    }
}

#[test]
#[ignore]
fn pets_keep_their_ids_across_sessions_and_a_deleted_pet_leaves_the_realm_on_update() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1005);
    let c0 = store.load_current(id).unwrap();
    assert_eq!(c0.pets.len(), 3);
    let numbers: Vec<u32> = store.pet_mappings(id, B).unwrap().iter().map(|m| m.local_pet_number).collect();
    assert_eq!(numbers.len(), 3);

    begin_session(&r.b, &mut store, id, B).unwrap();
    // the player levels one pet, abandons another
    sql(&r.b, &format!("UPDATE acore_characters.character_pet SET level = level + 1, exp = 99, name = 'Renamed' WHERE id = {}", numbers[0]));
    sql(&r.b, &format!("DELETE FROM acore_characters.character_pet WHERE id = {0}; DELETE FROM acore_characters.pet_spell WHERE guid = {0};", numbers[1]));
    let out = reconcile_session(&r.b, &mut store, id, B, true, None).unwrap();
    assert_eq!(out.revision, 2);
    let c1 = store.load_current(id).unwrap();
    assert_eq!(c1.pets.len(), 2);
    let pet0 = c1.pets.iter().find(|p| p.id == c0.pets[0].id).expect("the same portable pet");
    assert_eq!((pet0.level, pet0.exp, pet0.name.as_str()), (c0.pets[0].level + 1, 99, "Renamed"), "the same pet id, with the played changes");
    assert!(c1.pets.iter().all(|p| p.id != c0.pets[1].id), "the abandoned pet is gone");
    assert!(c1.pets.iter().any(|p| p.id == c0.pets[2].id));
    assert_eq!(store.active_pet_lookup(id, B).unwrap().len(), 2);

    // canonical pet 0 moves on elsewhere; the update changes the same realm pet row in place
    let mut c2 = c1.clone();
    c2.pets.iter_mut().find(|p| p.id == c0.pets[0].id).unwrap().exp = 500;
    store.commit_snapshot(id, 2, c2, "realm-a", None).unwrap();
    assert!(update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap().updated);
    assert_eq!(number(&r.b, &format!("SELECT exp FROM acore_characters.character_pet WHERE id = {}", numbers[0])), 500, "the same pet number, updated in place");
    assert_eq!(number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.character_pet WHERE owner = {g}")), 2);

    // a pet number the realm recycles is not the old pet
    sql(&r.b, &format!("DELETE FROM acore_characters.character_pet WHERE id = {0}; DELETE FROM acore_characters.pet_spell WHERE guid = {0};", numbers[0]));
    sql(&r.b, &format!("INSERT INTO acore_characters.character_pet (id, entry, owner, modelid, CreatedBySpell, PetType, level, exp, Reactstate, name, renamed, slot, curhealth, curmana, curhappiness, savetime) VALUES ({}, 42, {g}, 1, 1515, 0, 5, 0, 1, 'Stranger', 0, 4, 100, 0, 0, 1)", numbers[0]));
    let c = store.load_current(id).unwrap();
    let before = c.pets.len();
    store.commit_snapshot(id, 2, c, "realm-a", None).ok();
    let exported = export_character_with_pets(&r.b, g, Some(id), &store.active_item_lookup(id, B).unwrap(), &store.active_pet_lookup(id, B).unwrap()).unwrap();
    assert!(exported.model.pets.iter().all(|p| p.id != c0.pets[0].id), "the recycled number does not inherit the old pet's portable id");
    assert_eq!(exported.model.pets.len(), before);
}

#[test]
#[ignore]
fn the_complete_round_trip_without_a_server() {
    let Some(r) = realms() else { return };
    // this test updates the ORIGINAL character of realm A in place, which changes the fixture the recorded-answer tests compare
    // with: it only runs on request, and realm A has to be reset afterwards (`tools/reset_live_realm.ps1`)
    if std::env::var("COA_PORTABLE_LIVE_MUTATE_A").is_err() {
        return;
    }
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    // A -> B
    let (id, g) = join(&r, &mut store, profile, 1002);
    // realm A's fixture keeps one character flagged online on purpose (it must not be exportable); a stopped realm has none
    sql(&r.a, "UPDATE acore_characters.characters SET online = 0 WHERE guid = 1006");
    let a_guid = store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == "realm-a").unwrap().local_guid;
    make_local(&r.b, g);
    let b_local = world_local(&r.b, g);
    let c0 = store.load_current(id).unwrap();
    begin_session(&r.b, &mut store, id, B).unwrap();
    // play on B
    let new_spell = *spells_of(&r, 1004).iter().find(|s| !c0.build.spells.iter().any(|(k, _)| k == *s)).unwrap();
    sql(&r.b, &format!("UPDATE acore_characters.characters SET money = money + 4000 WHERE guid = {g}; INSERT INTO acore_characters.character_spell (guid, spell, specMask) VALUES ({g}, {new_spell}, 255)"));
    let sold = local_item(&store, id, plain(&c0)[0]);
    realm_takes(&r.b, g, sold);
    // reconcile into canonical, then update A in place
    let out = reconcile_session(&r.b, &mut store, id, B, true, Some("session on B")).unwrap();
    assert_eq!(out.revision, 2);
    let a_before = world_local(&r.a, a_guid);
    let a_counts = counts(&r.a);
    let canonical = store.load_current(id).unwrap();
    assert_eq!(canonical.progression.money, c0.progression.money + 4000);
    // A is the realm the character came from: its mapping is synced at revision 1
    let updated = update_realm_character(&r.a, &mut store, id, "realm-a", &opts()).unwrap();
    assert!(updated.updated, "{updated:?}");
    assert_eq!(world_local(&r.a, a_guid), a_before, "A's own position, homebind and local data are untouched");
    assert_eq!(number(&r.a, &format!("SELECT money FROM acore_characters.characters WHERE guid = {a_guid}")), canonical.progression.money as u64);
    assert_eq!(number(&r.a, &format!("SELECT COUNT(*) FROM acore_characters.character_spell WHERE guid = {a_guid} AND spell = {new_spell}")), 1);
    assert_eq!(counts(&r.a)["characters"], a_counts["characters"]);
    // B is untouched by what happened on A, and still the same local character
    assert_eq!(world_local(&r.b, g), b_local);
    // A holds exactly the canonical character
    let prior = store.active_item_lookup(id, "realm-a").unwrap();
    let back_a = export_character_with_pets(&r.a, a_guid, Some(id), &prior, &store.active_pet_lookup(id, "realm-a").unwrap()).unwrap().model;
    let mut diffs = Vec::new();
    json_diff("", &serde_json::to_value(&canonical).unwrap(), &serde_json::to_value(&back_a).unwrap(), &mut diffs);
    assert!(diffs.is_empty(), "A holds exactly the canonical character: {diffs:#?}");

    // play on A (a session of its own), reconcile, and go back to B: the SAME local character there, updated in place
    begin_session(&r.a, &mut store, id, "realm-a").unwrap();
    sql(&r.a, &format!("UPDATE acore_characters.characters SET money = money + 1000 WHERE guid = {a_guid}"));
    let out = reconcile_session(&r.a, &mut store, id, "realm-a", true, Some("session on A")).unwrap();
    assert_eq!(out.revision, 3);
    let b_counts = counts(&r.b);
    let back = update_realm_character(&r.b, &mut store, id, B, &opts()).unwrap();
    assert!(back.updated);
    assert_eq!((back.from_revision, back.to_revision), (2, 3));
    assert_eq!(world_local(&r.b, g), b_local, "returning to a visited realm keeps its local character, position and homebind");
    assert_eq!(number(&r.b, &format!("SELECT money FROM acore_characters.characters WHERE guid = {g}")), canonical.progression.money as u64 + 1000);
    assert_eq!(counts(&r.b)["characters"], b_counts["characters"], "never deleted and re-created");
    assert_eq!(store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == B).unwrap().local_guid, g);
    assert!(store.open_imports(B).unwrap().is_empty() && store.open_imports("realm-a").unwrap().is_empty());
    sql(&r.a, "UPDATE acore_characters.characters SET online = 1 WHERE guid = 1006");
}

#[test]
#[ignore]
fn an_in_place_update_arms_the_next_runtime_session_in_the_same_transaction() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let (id, g) = join(&r, &mut store, profile, 1002);
    let mut c1 = store.load_current(id).unwrap();
    c1.progression.money += 3;
    store.commit_snapshot(id, 1, c1.normalized(), "realm-a", None).unwrap();
    let session = crate::portable::ids::SessionId::new();
    let outcome = update_realm_character_in_session(&r.b, &mut store, id, B, &opts(), Some(session)).unwrap();
    assert!(outcome.updated);
    let row = crate::portable::session::live::read_session_row(&r.b, g).unwrap().expect("the update armed the session");
    assert_eq!((row.session_id, row.character_id, row.state, row.imported_revision), (session, id, crate::portable::session::bridge::RowState::WaitingBaseline, 2));
    // a failed update arms nothing
    let before = counts(&r.b);
    let mut c2 = store.load_current(id).unwrap();
    c2.build.spells.push((500_090, 255));
    store.commit_snapshot(id, 2, c2.normalized(), "realm-a", None).unwrap();
    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    r.b.query("DELIMITER //\nCREATE TRIGGER acore_characters.coa_test_fail BEFORE INSERT ON acore_characters.character_spell FOR EACH ROW BEGIN IF NEW.spell = 500090 THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'injected failure'; END IF; END//\nDELIMITER ;").unwrap();
    let again = crate::portable::ids::SessionId::new();
    assert!(update_realm_character_in_session(&r.b, &mut store, id, B, &opts(), Some(again)).is_err());
    r.b.query("DROP TRIGGER IF EXISTS acore_characters.coa_test_fail").unwrap();
    assert_eq!(counts(&r.b), before);
    assert_eq!(crate::portable::session::live::read_session_row(&r.b, g).unwrap().unwrap().session_id, session, "the failed update left the earlier session in place");
}
