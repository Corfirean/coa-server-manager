use super::*;
use crate::portable::fixtures::{self, character_id};

fn fresh() -> (Store, ProfileId) {
    let mut store = Store::open_in_memory().unwrap();
    let profile = store.default_profile().unwrap();
    (store, profile)
}

fn created(store: &mut Store, profile: ProfileId) -> CharacterId {
    store.create_character(profile, fixtures::geared_level_eighty(), "realm-a").unwrap()
}

fn with_money(mut model: PortableCharacter, money: u32) -> PortableCharacter {
    model.progression.money = money;
    model
}

#[test]
fn create_character_identity() {
    let (mut store, profile) = fresh();
    let model = fixtures::geared_level_eighty();
    let id = store.create_character(profile, model.clone(), "realm-a").unwrap();

    // a fresh UUIDv7, not the placeholder carried by the model
    assert_ne!(id, model.character_id);
    assert_eq!(id.as_uuid().get_version_num(), 7);

    let record = store.character(id).unwrap();
    assert_eq!(record.revision, 1);
    assert_eq!((record.name.as_str(), record.level, record.ruleset), ("Geared", 80, Ruleset::Coa));
    assert_eq!(record.race, "coa:race:1");
    assert_eq!(record.class, "coa:class:21");
    assert_eq!(record.profile_id, profile);
    assert!(!record.archived);

    let loaded = store.load_current(id).unwrap();
    let mut expected = model.normalized();
    expected.character_id = id;
    assert_eq!(loaded, expected, "the stored revision 1 is the model that was passed in");

    let revisions = store.list_revisions(id).unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!((revisions[0].revision, revisions[0].source_server_id.as_str(), revisions[0].note.as_deref()), (1, "realm-a", Some("created")));
    assert_eq!(store.list_characters(profile).unwrap().len(), 1);
}

#[test]
fn two_new_characters_never_share_an_id() {
    let (mut store, profile) = fresh();
    let a = created(&mut store, profile);
    let b = created(&mut store, profile);
    assert_ne!(a, b);
    assert!(a < b, "ids are time-ordered");
}

#[test]
fn revision_increments_monotonically() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let mut model = store.load_current(id).unwrap();
    for expected in 2..=6u64 {
        model.progression.xp += 100;
        let revision = store.commit_snapshot(id, expected - 1, model.clone(), "realm-b", Some("checkpoint")).unwrap();
        assert_eq!(revision, expected);
        assert_eq!(store.character(id).unwrap().revision, expected);
    }
    let revisions: Vec<u64> = store.list_revisions(id).unwrap().iter().map(|r| r.revision).collect();
    assert_eq!(revisions, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(store.load_current(id).unwrap().progression.xp, model.progression.xp);
    assert_eq!(store.character(id).unwrap().level, 80);
}

#[test]
fn commit_updates_the_summary_row() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let mut model = store.load_current(id).unwrap();
    model.identity.name = "Renamed".into();
    model.progression.level = 81;
    store.commit_snapshot(id, 1, model, "realm-a", None).unwrap();
    let record = store.character(id).unwrap();
    assert_eq!((record.name.as_str(), record.level, record.revision), ("Renamed", 81, 2));
}

#[test]
fn stale_revision_is_rejected_and_nothing_is_written() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let base = store.load_current(id).unwrap();

    // a fresher state is committed first
    store.commit_snapshot(id, 1, with_money(base.clone(), 500), "realm-a", None).unwrap();
    let before = store.list_revisions(id).unwrap();

    // a writer that still believes revision 1 is current must lose
    let result = store.commit_snapshot(id, 1, with_money(base.clone(), 999_999), "realm-b", None);
    assert!(matches!(result, Err(PortableError::StaleRevision { expected: 1, current: 2 })), "{result:?}");
    // so must one that claims a revision from the future
    let result = store.commit_snapshot(id, 7, with_money(base, 1), "realm-b", None);
    assert!(matches!(result, Err(PortableError::StaleRevision { expected: 7, current: 2 })), "{result:?}");

    assert_eq!(store.list_revisions(id).unwrap(), before, "a rejected commit leaves no trace");
    assert_eq!(store.load_current(id).unwrap().progression.money, 500);
    assert_eq!(store.character(id).unwrap().revision, 2);
}

#[test]
fn two_stores_on_one_file_cannot_both_win() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::open(dir.path()).unwrap();
    let mut b = Store::open(dir.path()).unwrap();
    let profile = a.default_profile().unwrap();
    let id = created(&mut a, profile);
    let base = b.load_current(id).unwrap();

    a.commit_snapshot(id, 1, with_money(base.clone(), 10), "realm-a", None).unwrap();
    let late = b.commit_snapshot(id, 1, with_money(base, 20), "realm-b", None);
    assert!(matches!(late, Err(PortableError::StaleRevision { .. })), "{late:?}");
    assert_eq!(b.load_current(id).unwrap().progression.money, 10);
}

#[test]
fn duplicate_uuid_is_rejected_and_the_original_is_untouched() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let original = store.load_current(id).unwrap();

    let mut impostor = fixtures::naked_level_one();
    impostor.character_id = id;
    let result = store.create_character_with_id(profile, impostor, "realm-x");
    assert!(matches!(result, Err(PortableError::DuplicateCharacter(dup)) if dup == id), "{result:?}");

    assert_eq!(store.load_current(id).unwrap(), original);
    assert_eq!(store.list_revisions(id).unwrap().len(), 1);
    assert_eq!(store.list_characters(profile).unwrap().len(), 1);
}

#[test]
fn a_restored_character_keeps_its_own_identity() {
    let (mut store, profile) = fresh();
    let model = fixtures::naked_level_one();
    let id = store.create_character_with_id(profile, model.clone(), "realm-a").unwrap();
    assert_eq!(id, model.character_id);
    assert_eq!(id, character_id(1));
}

#[test]
fn wrong_character_ruleset_and_unknown_character_are_refused() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let other = store.create_character(profile, fixtures::naked_level_one(), "realm-a").unwrap();

    // a snapshot of another character cannot be committed under this id
    let foreign = store.load_current(other).unwrap();
    assert!(matches!(store.commit_snapshot(id, 1, foreign, "realm-a", None), Err(PortableError::WrongCharacter { .. })));

    // CoA and Wildcard never mix
    let mut wildcard = store.load_current(id).unwrap();
    wildcard.ruleset = Ruleset::Wildcard;
    assert!(matches!(store.commit_snapshot(id, 1, wildcard, "realm-a", None), Err(PortableError::RulesetChange { .. })));

    let missing = CharacterId::new();
    assert!(matches!(store.character(missing), Err(PortableError::UnknownCharacter(_))));
    assert!(matches!(store.list_revisions(missing), Err(PortableError::UnknownCharacter(_))));
    assert_eq!(store.character(id).unwrap().revision, 1);
}

#[test]
fn an_invalid_model_is_refused_without_side_effects() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let mut broken = store.load_current(id).unwrap();
    broken.progression.money = u32::MAX;
    assert!(store.commit_snapshot(id, 1, broken, "realm-a", None).is_err());
    assert!(store.commit_snapshot(id, 1, store.load_current(id).unwrap(), "bad server id!", None).is_err());
    assert_eq!(store.character(id).unwrap().revision, 1);
    assert_eq!(store.list_revisions(id).unwrap().len(), 1);
}

#[test]
fn rollback_creates_a_new_revision_with_the_old_state() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let rev1 = store.load_current(id).unwrap();
    store.commit_snapshot(id, 1, with_money(rev1.clone(), 111), "realm-a", None).unwrap();
    store.commit_snapshot(id, 2, with_money(rev1.clone(), 222), "realm-a", None).unwrap();

    let new = store.rollback_to(id, 3, 1, "manager").unwrap();
    assert_eq!(new, 4, "revision numbers never go down");
    assert_eq!(store.character(id).unwrap().revision, 4);
    assert_eq!(store.load_current(id).unwrap(), rev1, "the state is revision 1's");

    let revisions = store.list_revisions(id).unwrap();
    assert_eq!(revisions.len(), 4, "history is kept, nothing is rewritten");
    assert_eq!(revisions[3].note.as_deref(), Some("rollback to revision 1"));
    assert_eq!(revisions[3].content_hash, revisions[0].content_hash);
    assert_eq!(store.load_snapshot(id, 3).unwrap().progression.money, 222, "the state before the rollback is still there");

    // a rollback is itself a revision: a stale caller cannot roll back on top of newer state
    assert!(matches!(store.rollback_to(id, 3, 1, "manager"), Err(PortableError::StaleRevision { .. })));
    assert!(matches!(store.rollback_to(id, 4, 99, "manager"), Err(PortableError::UnknownRevision { revision: 99, .. })));
    assert_eq!(store.character(id).unwrap().revision, 4);
}

#[test]
fn history_is_capped_and_the_current_revision_survives() {
    let (mut store, profile) = fresh();
    assert_eq!(store.history_keep().unwrap(), DEFAULT_HISTORY_KEEP);
    store.set_history_keep(3).unwrap();
    assert_eq!(store.history_keep().unwrap(), 3);
    let id = created(&mut store, profile);
    let mut model = store.load_current(id).unwrap();
    for rev in 1..=9u64 {
        model.progression.xp = rev as u32;
        store.commit_snapshot(id, rev, model.clone(), "realm-a", None).unwrap();
    }
    let revisions: Vec<u64> = store.list_revisions(id).unwrap().iter().map(|r| r.revision).collect();
    assert_eq!(revisions, vec![8, 9, 10], "only the newest three remain");
    assert_eq!(store.character(id).unwrap().revision, 10);
    assert_eq!(store.load_current(id).unwrap().progression.xp, 9);
    assert!(matches!(store.load_snapshot(id, 1), Err(PortableError::UnknownRevision { .. })));

    // lowering the limit and pruning never removes the current revision
    store.set_history_keep(1).unwrap();
    assert_eq!(store.prune(id).unwrap(), 2);
    assert_eq!(store.list_revisions(id).unwrap().len(), 1);
    assert_eq!(store.load_current(id).unwrap().progression.xp, 9);
    assert_eq!(store.prune(id).unwrap(), 0);

    assert!(store.set_history_keep(0).is_err());
    assert!(store.set_history_keep(MAX_HISTORY_KEEP + 1).is_err());
}

#[test]
fn a_damaged_snapshot_is_detected_not_trusted() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    // flip bytes in the stored payload
    store.conn.execute("UPDATE snapshot SET payload = CAST(substr(payload, 1, 20) || zeroblob(40) AS BLOB) WHERE character_id = ?1", [id.to_string()]).unwrap();
    assert!(store.load_current(id).is_err());
    // a payload that decodes but does not match the recorded hash
    let (mut other, profile) = fresh();
    let a = created(&mut other, profile);
    let b = other.create_character(profile, fixtures::naked_level_one(), "realm-a").unwrap();
    other.conn.execute(
        "UPDATE snapshot SET payload = (SELECT payload FROM snapshot WHERE character_id = ?2) WHERE character_id = ?1",
        params![a.to_string(), b.to_string()],
    )
    .unwrap();
    assert!(matches!(other.load_current(a), Err(PortableError::CorruptSnapshot(_))));
}

#[test]
fn data_survives_reopening_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let id;
    {
        let mut store = Store::open(dir.path()).unwrap();
        let profile = store.default_profile().unwrap();
        id = created(&mut store, profile);
        let model = store.load_current(id).unwrap();
        store.commit_snapshot(id, 1, with_money(model, 4242), "realm-a", None).unwrap();
        store.set_history_keep(7).unwrap();
    }
    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.character(id).unwrap().revision, 2);
    assert_eq!(store.load_current(id).unwrap().progression.money, 4242);
    assert_eq!(store.history_keep().unwrap(), 7);
    let profile = store.default_profile().unwrap();
    assert_eq!(store.list_characters(profile).unwrap().len(), 1);
    assert!(Store::path_of(dir.path()).is_file());
}

#[test]
fn migrations_are_idempotent_and_newer_databases_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }
    // opening again changes nothing
    assert_eq!(Store::open(dir.path()).unwrap().schema_version().unwrap(), MIGRATIONS.len() as i64);

    // a database written by a newer Manager is refused, not "migrated" downwards
    {
        let conn = Connection::open(Store::path_of(dir.path())).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    assert!(matches!(Store::open(dir.path()), Err(PortableError::NewerDatabase { found: 99, .. })));
}

#[test]
fn a_foreign_sqlite_file_is_not_adopted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(DATABASE_FILE);
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch("CREATE TABLE somebody_elses (x INTEGER); INSERT INTO somebody_elses VALUES (1);").unwrap();
    }
    assert!(matches!(Store::open(dir.path()), Err(PortableError::NotPortableDatabase)));
    let conn = Connection::open(&file).unwrap();
    let rows: i64 = conn.query_row("SELECT count(*) FROM somebody_elses", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "the foreign file is left alone");
}

#[test]
fn foreign_keys_are_enforced() {
    let (store, _) = fresh();
    let result = store.conn.execute(
        "INSERT INTO character_server_mapping(character_id, server_id, local_guid, last_revision, state, updated_at) VALUES (?1, 'r', 1, 1, 'pending', 'x')",
        [CharacterId::new().to_string()],
    );
    assert!(result.is_err(), "a mapping for a character that does not exist must be impossible");
}

#[test]
fn mapping_unique_constraints() {
    let (mut store, profile) = fresh();
    let a = created(&mut store, profile);
    let b = store.create_character(profile, fixtures::naked_level_one(), "realm-a").unwrap();

    store.bind_server(a, "realm-a", 154, 1, MappingState::Active).unwrap();
    assert_eq!(store.find_by_local("realm-a", 154).unwrap(), Some(a));
    assert_eq!(store.find_by_local("realm-a", 155).unwrap(), None);
    assert_eq!(store.find_by_local("realm-z", 154).unwrap(), None, "guids are per realm");

    // the same local guid cannot be claimed by another portable character
    let taken = store.bind_server(b, "realm-a", 154, 1, MappingState::Active);
    assert!(matches!(taken, Err(PortableError::LocalGuidTaken { local_guid: 154, .. })), "{taken:?}");
    assert_eq!(store.find_by_local("realm-a", 154).unwrap(), Some(a));

    // but the same number on another realm is a different character
    store.bind_server(b, "realm-b", 154, 1, MappingState::Pending).unwrap();
    assert_eq!(store.find_by_local("realm-b", 154).unwrap(), Some(b));

    // one character, many realms, different local guids (the point of the whole design)
    store.bind_server(a, "realm-b", 8421, 1, MappingState::Pending).unwrap();
    store.bind_server(a, "realm-c", 391, 1, MappingState::Pending).unwrap();
    let guids: Vec<(String, u32)> = store.server_mappings(a).unwrap().into_iter().map(|m| (m.server_id, m.local_guid)).collect();
    assert_eq!(guids, vec![("realm-a".into(), 154), ("realm-b".into(), 8421), ("realm-c".into(), 391)]);

    // rebinding updates in place
    store.bind_server(a, "realm-a", 154, 1, MappingState::Synced).unwrap();
    assert_eq!(store.server_mappings(a).unwrap()[0].state, MappingState::Synced);

    // a binding cannot name a revision that does not exist yet, nor an unknown character
    assert!(store.bind_server(a, "realm-a", 154, 5, MappingState::Synced).is_err());
    assert!(matches!(store.bind_server(CharacterId::new(), "realm-a", 1, 1, MappingState::Pending), Err(PortableError::UnknownCharacter(_))));
}

fn obs(item: PortableItemId, guid: u32, entry: u64, identity: &str) -> ItemObservation {
    ItemObservation { portable_item_id: item, local_item_guid: guid, entry: ContentId::new("coa", "item", entry).unwrap(), identity: identity.to_string() }
}

/// A character bound to `realm-a` with three portable items to play with.
fn bound() -> (Store, CharacterId, [PortableItemId; 3]) {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    store.bind_server(id, "realm-a", 154, 1, MappingState::Active).unwrap();
    let model = store.load_current(id).unwrap();
    let items = [model.items[0].id, model.items[1].id, model.items[2].id];
    (store, id, items)
}

#[test]
fn item_mapping_needs_a_binding_and_enforces_uniqueness_among_active_rows() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let items: Vec<PortableItemId> = store.load_current(id).unwrap().items.iter().take(4).map(|i| i.id).collect();

    // an item mapping needs a server binding first
    assert!(store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 1, 10, "v1:a")]).is_err());
    store.bind_server(id, "realm-a", 154, 1, MappingState::Active).unwrap();

    let report = store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 9001, 10, "v1:a"), obs(items[1], 9002, 11, "v1:b"), obs(items[2], 9003, 12, "v1:c")]).unwrap();
    assert_eq!(report, ReconcileReport { added: 3, ..Default::default() });
    let active = store.item_mappings(id, "realm-a").unwrap();
    assert_eq!(active.iter().map(|m| (m.portable_item_id, m.local_item_guid)).collect::<Vec<_>>(), vec![(items[0], 9001), (items[1], 9002), (items[2], 9003)]);
    assert!(active.iter().all(|m| m.active && m.created_revision == 1 && m.entry.starts_with("coa:item:")));

    // input that maps one local guid twice, or one portable item twice, is refused before anything changes
    let clash = store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 9001, 10, "v1:a"), obs(items[1], 9001, 11, "v1:b")]);
    assert!(matches!(clash, Err(PortableError::ItemGuidConflict { local_item_guid: 9001, .. })), "{clash:?}");
    assert!(store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 1, 10, "v1:a"), obs(items[0], 2, 10, "v1:a")]).is_err());
    assert!(store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 1, 10, "")]).is_err(), "an observation without an identity is useless");
    assert!(store.reconcile_item_mappings(id, "realm-a", 9, &[]).is_err(), "revision from the future");
    assert_eq!(store.item_mappings(id, "realm-a").unwrap().len(), 3);

    // the database itself enforces UNIQUE(character_id, server_id, local_item_guid) for active rows
    let direct = store.conn.execute(
        "INSERT INTO item_mapping(character_id, server_id, portable_item_id, local_item_guid, entry, identity, state, created_revision, confirmed_revision, created_at, updated_at)
         VALUES (?1, 'realm-a', ?2, 9001, 'x', 'x', 'active', 1, 1, 'x', 'x')",
        params![id.to_string(), items[3].to_string()],
    );
    assert!(direct.is_err());

    // the same local item guid on another realm is another item
    store.bind_server(id, "realm-b", 8421, 1, MappingState::Active).unwrap();
    store.reconcile_item_mappings(id, "realm-b", 1, &[obs(items[0], 9001, 10, "v1:a")]).unwrap();
    assert_eq!(store.item_mappings(id, "realm-b").unwrap().len(), 1);
}

#[test]
fn a_recycled_local_guid_is_not_the_old_item() {
    let (mut store, id, items) = bound();
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 5000, 10, "v1:sword")]).unwrap();

    // later the sword is destroyed; the realm gives guid 5000 to a different item with another identity
    let resolutions = store.resolve_item_ids(id, "realm-a", &[(5000, "v1:potion"), (5000, "v1:sword"), (5001, "v1:sword")]).unwrap();
    assert_eq!(resolutions, vec![ItemResolution::Reused { previous: items[0] }, ItemResolution::Known(items[0]), ItemResolution::Unmapped]);

    // the new item gets a new portable id; the old mapping is retired, not overwritten
    let fresh_item = PortableItemId::new();
    let report = store.reconcile_item_mappings(id, "realm-a", 1, &[obs(fresh_item, 5000, 77, "v1:potion")]).unwrap();
    assert_eq!((report.guid_reused, report.added, report.absent), (1, 1, 0));

    let active = store.item_mappings(id, "realm-a").unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!((active[0].portable_item_id, active[0].local_item_guid), (fresh_item, 5000));
    let history = store.item_mapping_history(id, "realm-a").unwrap();
    assert_eq!(history.len(), 2);
    assert!(!history[0].active);
    assert_eq!(history[0].portable_item_id, items[0]);
    assert_eq!(history[0].retired_reason, Some(RetireReason::GuidReused));
    assert_eq!(history[0].retired_revision, Some(1));

    // the old portable item is not resurrected by the recycled guid
    assert_eq!(store.resolve_item_ids(id, "realm-a", &[(5000, "v1:sword")]).unwrap(), vec![ItemResolution::Reused { previous: fresh_item }]);
}

#[test]
fn the_same_portable_id_with_a_changed_identity_is_treated_as_reuse_too() {
    let (mut store, id, items) = bound();
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 5000, 10, "v1:old")]).unwrap();
    let report = store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 5000, 10, "v1:new")]).unwrap();
    assert_eq!((report.confirmed, report.guid_reused, report.added), (0, 1, 1));
    let history = store.item_mapping_history(id, "realm-a").unwrap();
    assert_eq!((history.iter().filter(|m| m.active).count(), history.len()), (1, 2));
}

#[test]
fn moved_and_absent_items_are_retired_with_their_reason() {
    let (mut store, id, items) = bound();
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 100, 1, "v1:a"), obs(items[1], 101, 2, "v1:b"), obs(items[2], 102, 3, "v1:c")]).unwrap();

    // item 0 got a new local guid (re-imported), item 1 is gone, item 2 is unchanged
    let report = store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 200, 1, "v1:a"), obs(items[2], 102, 3, "v1:c")]).unwrap();
    assert_eq!(report, ReconcileReport { confirmed: 1, added: 1, guid_reused: 0, moved: 1, absent: 1, filtered: 0 });

    let active: Vec<(PortableItemId, u32)> = store.item_mappings(id, "realm-a").unwrap().into_iter().map(|m| (m.portable_item_id, m.local_item_guid)).collect();
    assert_eq!(active, vec![(items[2], 102), (items[0], 200)]);
    let reasons: Vec<Option<RetireReason>> = store.item_mapping_history(id, "realm-a").unwrap().into_iter().filter(|m| !m.active).map(|m| m.retired_reason).collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons.contains(&Some(RetireReason::Moved)) && reasons.contains(&Some(RetireReason::Absent)));
}

#[test]
fn rebinding_to_another_local_character_retires_its_item_mappings() {
    let (mut store, id, items) = bound();
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 100, 1, "v1:a")]).unwrap();
    store.bind_server(id, "realm-a", 777, 1, MappingState::Active).unwrap();
    assert!(store.item_mappings(id, "realm-a").unwrap().is_empty());
    let history = store.item_mapping_history(id, "realm-a").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].retired_reason, Some(RetireReason::CharacterRebound));
    // binding again to the same local guid changes nothing
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 300, 1, "v1:a")]).unwrap();
    store.bind_server(id, "realm-a", 777, 1, MappingState::Synced).unwrap();
    assert_eq!(store.item_mappings(id, "realm-a").unwrap().len(), 1);
}

#[test]
fn a_failed_reconcile_leaves_the_mappings_untouched() {
    let (mut store, id, items) = bound();
    store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 100, 1, "v1:a"), obs(items[1], 101, 2, "v1:b")]).unwrap();
    let before = store.item_mapping_history(id, "realm-a").unwrap();
    assert!(store.reconcile_item_mappings(id, "realm-a", 1, &[obs(items[0], 100, 1, "v1:a"), obs(items[0], 5, 1, "v1:a")]).is_err());
    assert_eq!(store.item_mapping_history(id, "realm-a").unwrap(), before);
}

#[test]
fn schema_1_item_mappings_survive_the_migration_as_unverified() {
    let dir = tempfile::tempdir().unwrap();
    let file = Store::path_of(dir.path());
    let (character, item);
    {
        // a schema-1 database, exactly as Phase 1 wrote it: only the first migration applied
        let conn = Connection::open(&file).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "application_id", APPLICATION_ID).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        let profile = ProfileId::new();
        character = CharacterId::new();
        item = PortableItemId::new();
        conn.execute("INSERT INTO profile VALUES (?1, 'x', 1)", [profile.to_string()]).unwrap();
        conn.execute("INSERT INTO character VALUES (?1, ?2, 'coa', 'n', 'r', 'c', 0, 1, 1, 'x', 'x', 0)", params![character.to_string(), profile.to_string()]).unwrap();
        conn.execute("INSERT INTO character_server_mapping VALUES (?1, 'realm-a', 154, 1, 'active', 'x')", [character.to_string()]).unwrap();
        conn.execute("INSERT INTO item_mapping VALUES (?1, 'realm-a', ?2, 4242)", params![character.to_string(), item.to_string()]).unwrap();
    }
    let store = Store::open_file(&file).unwrap();
    assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as i64);
    let mappings = store.item_mappings(character, "realm-a").unwrap();
    assert_eq!(mappings.len(), 1);
    assert_eq!((mappings[0].portable_item_id, mappings[0].local_item_guid, mappings[0].identity.as_str()), (item, 4242, ""));
    // an unverified mapping never matches a real identity, so it is treated as a recycled guid, not trusted
    assert_eq!(store.resolve_item_ids(character, "realm-a", &[(4242, "v1:real")]).unwrap(), vec![ItemResolution::Reused { previous: item }]);
}

#[test]
fn collection_union_never_removes_and_unchanged_means_no_revision() {
    let (mut store, profile) = fresh();
    let kind = "coa:wardrobe";
    assert!(store.collection(profile, kind).unwrap().is_none());

    // nothing stored, nothing incoming: nothing is created
    let empty = store.merge_collection(profile, kind, &IdSet::new()).unwrap();
    assert!(!empty.changed && empty.info.is_none());

    let first = store.merge_collection(profile, kind, &IdSet::from_ids([10, 20, 30]).unwrap()).unwrap();
    assert!(first.changed);
    assert_eq!(first.added, 3);
    let info1 = first.info.unwrap();
    assert_eq!((info1.revision, info1.count), (1, 3));

    // another realm returns a subset: nothing is lost, nothing changes
    let subset = store.merge_collection(profile, kind, &IdSet::from_ids([20]).unwrap()).unwrap();
    assert!(!subset.changed);
    assert_eq!(subset.info.as_ref().unwrap(), &info1, "same revision, same hash");
    let same = store.merge_collection(profile, kind, &IdSet::from_ids([10, 20, 30]).unwrap()).unwrap();
    assert!(!same.changed);

    // a realm with new unlocks (and without the old ones) only adds
    let more = store.merge_collection(profile, kind, &IdSet::from_ids([30, 40, 5]).unwrap()).unwrap();
    assert!(more.changed);
    assert_eq!(more.added, 2);
    let info2 = more.info.unwrap();
    assert_eq!((info2.revision, info2.count), (2, 5));
    assert_ne!(info2.hash, info1.hash);

    let (info, set) = store.collection(profile, kind).unwrap().unwrap();
    assert_eq!(set.ids(), &[5, 10, 20, 30, 40]);
    assert_eq!(info, info2);
    assert_eq!(store.collection_info(profile, kind).unwrap().unwrap(), info2);

    // kinds are separate collections
    assert!(store.collection_info(profile, "coa:vanity").unwrap().is_none());
    assert!(store.merge_collection(profile, "wardrobe", &IdSet::new()).is_err(), "kinds must be namespaced");
}

#[test]
fn collections_hold_tens_of_thousands_of_ids_compactly() {
    let (mut store, profile) = fresh();
    for (kind, count) in [("coa:wardrobe", 2_000u32), ("coa:vanity", 10_000), ("mod:test:mounts", 50_000)] {
        let ids: Vec<u32> = (0..count).map(|n| n * 7 + (n % 5)).collect();
        let set = IdSet::from_ids(ids).unwrap();
        let merge = store.merge_collection(profile, kind, &set).unwrap();
        assert_eq!(merge.added, count as usize);
        let stored_bytes: i64 = store.conn.query_row("SELECT length(payload) FROM collection WHERE kind = ?1", [kind], |r| r.get(0)).unwrap();
        assert!(stored_bytes < count as i64 * 2 + 16, "{kind}: {count} ids took {stored_bytes} bytes");
        let (_, back) = store.collection(profile, kind).unwrap().unwrap();
        assert_eq!(back, set);
    }
}

#[test]
fn a_tampered_collection_is_detected() {
    let (mut store, profile) = fresh();
    store.merge_collection(profile, "coa:vanity", &IdSet::from_ids([1, 2, 3]).unwrap()).unwrap();
    let forged = IdSet::from_ids([1, 2, 3, 4]).unwrap().encode();
    store.conn.execute("UPDATE collection SET payload = ?1", [forged]).unwrap();
    assert!(matches!(store.collection(profile, "coa:vanity"), Err(PortableError::CorruptSnapshot(_))));
}

// ---- the import journal -------------------------------------------------------------------------------------------------

fn plan_of_current(store: &Store, id: CharacterId) -> Vec<PlannedItem> {
    store
        .load_current(id)
        .unwrap()
        .items
        .iter()
        .map(|i| PlannedItem { id: i.id, entry: i.entry.clone(), identity: crate::portable::identity::item_identity(&i.entry, i.random_property_id) })
        .collect()
}

fn planned_pets_of(store: &Store, id: CharacterId) -> Vec<PlannedPet> {
    store.load_current(id).unwrap().pets.iter().map(|p| PlannedPet { id: p.id, entry: p.entry.clone(), identity: pet_identity(&p.entry, p.pet_type, p.created_by_spell) }).collect()
}

fn alloc(guid: u32) -> ImportAllocation {
    ImportAllocation { local_guid: guid, item_base: 20_064, pet_base: 3_004 }
}

#[test]
fn an_import_is_journaled_first_and_mapped_only_when_finished() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let items = plan_of_current(&store, id);
    let pets = planned_pets_of(&store, id);

    let ticket = store.begin_import(id, "realm-b", 1, &items, &pets).unwrap();
    assert_eq!(ticket.marker, marker_text(ticket.nonce, 1));
    let entry = store.import_entry(ticket.import_id).unwrap();
    assert_eq!((entry.state, entry.revision, entry.items.len(), entry.pets.len()), (ImportState::Prepared, 1, items.len(), pets.len()));
    // nothing is mapped before the realm has committed
    assert!(store.server_mappings(id).unwrap().is_empty());
    assert_eq!(store.find_by_local("realm-b", 1013).unwrap(), None);
    assert_eq!(store.open_imports("realm-b").unwrap().len(), 1);
    assert!(store.open_imports("realm-a").unwrap().is_empty());

    store.finish_import(ticket.import_id, alloc(1013)).unwrap();

    assert_eq!(store.find_by_local("realm-b", 1013).unwrap(), Some(id));
    let mapping = &store.server_mappings(id).unwrap()[0];
    assert_eq!((mapping.local_guid, mapping.last_revision, mapping.state), (1013, 1, MappingState::Synced));
    let mapped = store.item_mappings(id, "realm-b").unwrap();
    assert_eq!(mapped.len(), items.len());
    for (i, planned) in items.iter().enumerate() {
        let m = mapped.iter().find(|m| m.portable_item_id == planned.id).unwrap();
        assert_eq!(m.local_item_guid, 20_064 + i as u32, "item {i} is item_base + {i}");
        assert_eq!((m.identity.as_str(), m.entry.as_str()), (planned.identity.as_str(), planned.entry.to_string().as_str()));
    }
    let done = store.import_entry(ticket.import_id).unwrap();
    assert_eq!((done.state, done.allocation), (ImportState::Committed, Some(alloc(1013))));
    assert!(store.open_imports("realm-b").unwrap().is_empty());

    // finishing twice is harmless (recovery may run after a normal finish)
    store.finish_import(ticket.import_id, alloc(1013)).unwrap();
    assert_eq!(store.item_mappings(id, "realm-b").unwrap().len(), items.len());
    // the character is on that realm now
    assert!(matches!(store.begin_import(id, "realm-b", 1, &items, &pets), Err(PortableError::AlreadyOnRealm { .. })));
}

#[test]
fn begin_import_refuses_stale_revisions_and_a_second_unfinished_import() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let items = plan_of_current(&store, id);
    let model = store.load_current(id).unwrap();
    store.commit_snapshot(id, 1, model, "realm-a", None).unwrap();

    assert!(matches!(store.begin_import(id, "realm-b", 1, &items, &[]), Err(PortableError::StaleRevision { expected: 1, current: 2 })));
    let first = store.begin_import(id, "realm-b", 2, &items, &[]).unwrap();
    assert!(matches!(store.begin_import(id, "realm-b", 2, &items, &[]), Err(PortableError::ImportInProgress { import_id }) if import_id == first.import_id));
    // another realm is independent
    let other = store.begin_import(id, "realm-c", 2, &items, &[]).unwrap();
    assert_ne!(first.nonce, other.nonce, "every import has its own nonce");
    assert!(matches!(store.begin_import(CharacterId::new(), "realm-b", 1, &items, &[]), Err(PortableError::UnknownCharacter(_))));
    assert!(store.begin_import(id, "bad server!", 2, &items, &[]).is_err());
}

#[test]
fn an_aborted_import_can_be_retried_and_closed_entries_cannot_be_reopened() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let items = plan_of_current(&store, id);
    let first = store.begin_import(id, "realm-b", 1, &items, &[]).unwrap();
    store.abort_import(first.import_id, "the realm never committed").unwrap();
    let entry = store.import_entry(first.import_id).unwrap();
    assert_eq!((entry.state, entry.detail.as_deref()), (ImportState::Aborted, Some("the realm never committed")));
    assert!(store.server_mappings(id).unwrap().is_empty() && store.item_mappings(id, "realm-b").unwrap().is_empty());

    // closed entries stay closed
    assert!(matches!(store.finish_import(first.import_id, alloc(1)), Err(PortableError::ImportState { .. })));
    assert!(matches!(store.abort_import(first.import_id, "again"), Err(PortableError::ImportState { .. })));

    let second = store.begin_import(id, "realm-b", 1, &items, &[]).unwrap();
    assert_ne!(first.import_id, second.import_id);
    store.flag_import(second.import_id, "the realm has 3 items where 60 were planned").unwrap();
    assert_eq!(store.import_entry(second.import_id).unwrap().state, ImportState::NeedsAttention);
    assert!(store.open_imports("realm-b").unwrap().is_empty(), "a flagged entry is not 'open'");
    assert!(matches!(store.finish_import(second.import_id, alloc(1)), Err(PortableError::ImportState { .. })));
    assert!(matches!(store.import_entry(crate::portable::ids::ImportId::new()), Err(PortableError::UnknownImport(_))));
}

#[test]
fn a_failing_finish_changes_nothing_locally() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let other = store.create_character(profile, fixtures::naked_level_one(), "realm-a").unwrap();
    store.bind_server(other, "realm-b", 1013, 1, MappingState::Synced).unwrap();
    let items = plan_of_current(&store, id);
    let ticket = store.begin_import(id, "realm-b", 1, &items, &[]).unwrap();

    // the realm claims a local guid that already belongs to another portable character: the local step must roll back whole
    let result = store.finish_import(ticket.import_id, alloc(1013));
    assert!(matches!(result, Err(PortableError::LocalGuidTaken { .. })), "{result:?}");
    assert_eq!(store.import_entry(ticket.import_id).unwrap().state, ImportState::Prepared, "still recoverable");
    assert!(store.server_mappings(id).unwrap().is_empty());
    assert!(store.item_mappings(id, "realm-b").unwrap().is_empty());
}

// ---- Phase 4: presence, pet mappings, baselines, reconciliation, updates ---------------------------------------------------------

use crate::portable::ids::PortablePetId;
use crate::portable::merge::{merge3, Mode};
use crate::portable::model::PortableItem;

const ITEM_BASE: u32 = 20_064;
const PET_BASE: u32 = 3_004;

/// The character joined `realm-b` by an import and is synced there at revision 1.
fn joined() -> (Store, CharacterId, PortableCharacter) {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let items = plan_of_current(&store, id);
    let pets = planned_pets_of(&store, id);
    let ticket = store.begin_import(id, "realm-b", 1, &items, &pets).unwrap();
    store.finish_import(ticket.import_id, alloc(1013)).unwrap();
    let c0 = store.load_current(id).unwrap();
    (store, id, c0)
}

fn pet_obs(m: &PortableCharacter) -> Vec<PetObservation> {
    m.pets.iter().enumerate().map(|(i, p)| PetObservation { portable_pet_id: p.id, local_pet_number: PET_BASE + i as u32, identity: pet_identity(&p.entry, p.pet_type, p.created_by_spell) }).collect()
}

/// Items the realm would filter away: plain ones that hold nothing.
fn filterable(m: &PortableCharacter, n: usize) -> Vec<PortableItemId> {
    let containers: std::collections::HashSet<PortableItemId> = m.items.iter().filter_map(|i| i.container).collect();
    m.items.iter().filter(|i| i.container.is_none() && !containers.contains(&i.id) && i.slot < 40).map(|i| i.id).take(n).collect()
}

fn realm_item(n: u64) -> PortableItem {
    PortableItem {
        id: PortableItemId::from_uuid(fixtures::id7(600_000 + n)).unwrap(),
        container: None,
        slot: 60 + n as u8,
        entry: ContentId::new("coa", "item", 66_000 + n).unwrap(),
        count: 1,
        duration: 0,
        charges: vec![],
        flags: 0,
        enchantments: vec![],
        random_property_id: 0,
        durability: 10,
        played_time: 0,
        text: None,
        creator_name: None,
        gift: None,
    }
}

/// What the realm shows after its own first load/save: two items held back, one of its own added.
fn b0_of(c0: &PortableCharacter) -> (PortableCharacter, Vec<PortableItemId>, PortableItemId) {
    let filtered = filterable(c0, 2);
    let mut b0 = c0.clone();
    b0.items.retain(|i| !filtered.contains(&i.id));
    let own = realm_item(1);
    let own_id = own.id;
    b0.items.push(own);
    b0.progression.honor.today_honor = 0;
    (b0.normalized(), filtered, own_id)
}

/// The realm's item guids: the import's `ITEM_BASE + index` for imported items, `99_000 + n` for the realm's own.
fn observed(c0: &PortableCharacter, model: &PortableCharacter) -> Vec<ItemObservation> {
    model
        .items
        .iter()
        .map(|it| ItemObservation {
            portable_item_id: it.id,
            local_item_guid: match c0.items.iter().position(|o| o.id == it.id) {
                Some(i) => ITEM_BASE + i as u32,
                None => 99_000 + (it.entry.id() % 1000) as u32,
            },
            entry: it.entry.clone(),
            identity: crate::portable::identity::item_identity(&it.entry, it.random_property_id),
        })
        .collect()
}

#[test]
fn a_baseline_tells_filtered_items_from_the_realms_own_and_keeps_both_mapped() {
    let (mut store, id, c0) = joined();
    let (b0, filtered, own) = b0_of(&c0);

    let baseline = store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) }).unwrap();
    assert_eq!((baseline.c0_revision, baseline.head_revision), (1, 1));
    assert_eq!(baseline.c0, c0, "C0 is the snapshot the realm was synced with");
    assert_eq!(baseline.b0, b0);

    let mappings = store.item_mappings(id, "realm-b").unwrap();
    let presence = |item: PortableItemId| mappings.iter().find(|m| m.portable_item_id == item && m.active).map(|m| m.presence);
    for f in &filtered {
        assert_eq!(presence(*f), Some(Presence::Filtered), "an item the realm held back stays mapped, as filtered, not retired");
    }
    assert_eq!(presence(own), Some(Presence::RealmLocal), "an item the canonical character does not own is realm-local");
    assert_eq!(presence(c0.items.iter().find(|i| !filtered.contains(&i.id)).unwrap().id), Some(Presence::Present));
    assert!(mappings.iter().all(|m| m.active), "nothing was retired");

    // the baseline is persisted and one per realm at a time
    let again = store.open_baseline(id, "realm-b").unwrap().unwrap();
    assert_eq!((again.c0, again.b0), (c0.clone(), b0.clone()));
    assert!(store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) }).is_err());
    assert!(store.close_baseline(id, "realm-b").unwrap());
    assert!(store.open_baseline(id, "realm-b").unwrap().is_none());
    assert!(!store.close_baseline(id, "realm-b").unwrap(), "closing twice is harmless");
}

#[test]
fn a_baseline_needs_the_canonical_revision_the_realm_was_synced_with() {
    let (mut store, id, c0) = joined();
    let (b0, _, _) = b0_of(&c0);
    // canonical moved on (another realm's session): the base of this session would be wrong
    store.commit_snapshot(id, 1, with_money(c0.clone(), 5), "realm-a", None).unwrap();
    let r = store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) });
    assert!(matches!(r, Err(PortableError::StaleRevision { expected: 1, current: 2 })), "{r:?}");
    assert!(store.open_baseline(id, "realm-b").unwrap().is_none(), "a refused capture leaves nothing behind");
}

#[test]
fn reconciling_makes_one_new_revision_and_leaves_the_realms_state_in_sync() {
    let (mut store, id, c0) = joined();
    let (b0, filtered, own) = b0_of(&c0);
    store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) }).unwrap();

    // the player earns money, loses a plain item and picks one up
    let sold = filterable(&b0, 6).into_iter().find(|i| !filtered.contains(i) && *i != own).unwrap();
    let mut b1 = b0.clone();
    b1.progression.money += 500;
    b1.items.retain(|i| i.id != sold);
    let loot = realm_item(2);
    let loot_id = loot.id;
    b1.items.push(loot);
    let b1 = b1.normalized();
    let baseline = store.open_baseline(id, "realm-b").unwrap().unwrap();
    let merged = merge3(&baseline.c0, &baseline.b0, &b1, Mode::Lenient).unwrap();

    let revision = store.commit_reconciled(id, "realm-b", merged.model.clone(), &observed(&c0, &b1), &pet_obs(&b1), Some("session")).unwrap();
    assert_eq!(revision, 2);
    let canonical = store.load_current(id).unwrap();
    assert_eq!(canonical.progression.money, c0.progression.money + 500);
    assert!(canonical.items.iter().all(|i| i.id != sold), "what the player got rid of is gone");
    assert!(filtered.iter().all(|f| canonical.items.iter().any(|i| i.id == *f)), "what the realm held back is still canonical");
    assert!(canonical.items.iter().any(|i| i.id == loot_id), "what the player found is canonical now");
    assert!(canonical.items.iter().all(|i| i.id != own), "the realm's own item never becomes canonical");

    let head = store.open_baseline(id, "realm-b").unwrap().unwrap();
    assert_eq!((head.c0_revision, head.head_revision), (1, 2));
    assert_eq!(store.synced_model(id, "realm-b").unwrap().unwrap(), canonical, "the realm is synced with what was just committed");
    assert_eq!(store.server_mappings(id).unwrap()[0].last_revision, 2);

    let mappings = store.item_mappings(id, "realm-b").unwrap();
    let state = |item: PortableItemId| mappings.iter().find(|m| m.portable_item_id == item && m.active).map(|m| m.presence);
    assert_eq!(state(loot_id), Some(Presence::Present));
    assert_eq!(state(own), Some(Presence::RealmLocal));
    for f in &filtered {
        assert_eq!(state(*f), Some(Presence::Filtered));
    }
    assert!(state(sold).is_none(), "the sold item's mapping is retired");

    // a checkpoint later: the WHOLE B0 -> B1' delta is applied to C0 again, so nothing is counted twice
    let mut b2 = b1.clone();
    b2.progression.money += 100;
    let b2 = b2.normalized();
    let merged = merge3(&baseline.c0, &baseline.b0, &b2, Mode::Lenient).unwrap();
    assert_eq!(merged.model.progression.money, c0.progression.money + 600);
    let revision = store.commit_reconciled(id, "realm-b", merged.model.clone(), &observed(&c0, &b2), &pet_obs(&b2), None).unwrap();
    assert_eq!(revision, 3);
    // a checkpoint with no new progress makes no new revision
    let again = store.commit_reconciled(id, "realm-b", merged.model, &observed(&c0, &b2), &pet_obs(&b2), None).unwrap();
    assert_eq!(again, 3);
    assert_eq!(store.list_revisions(id).unwrap().len(), 3);
}

#[test]
fn a_reconcile_refuses_a_canonical_character_that_moved_under_it() {
    let (mut store, id, c0) = joined();
    let (b0, _, _) = b0_of(&c0);
    store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) }).unwrap();
    store.commit_snapshot(id, 1, with_money(c0.clone(), 9), "realm-a", None).unwrap();
    let mut b1 = b0.clone();
    b1.progression.money += 1;
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    let r = store.commit_reconciled(id, "realm-b", merged.model.clone(), &observed(&c0, &b1), &pet_obs(&b1), None);
    assert!(matches!(r, Err(PortableError::StaleRevision { expected: 1, current: 2 })), "{r:?}");
    assert_eq!(store.character(id).unwrap().revision, 2, "nothing was committed");
    // without a baseline there is nothing to reconcile
    assert!(store.close_baseline(id, "realm-b").unwrap());
    assert!(store.commit_reconciled(id, "realm-b", merged.model, &observed(&c0, &b1), &pet_obs(&b1), None).is_err());
}

#[test]
fn pet_mappings_are_stable_and_never_inherit_a_recycled_number() {
    let (mut store, id, c0) = joined();
    let pet = c0.pets[0].clone();
    let number = PET_BASE;
    let active = store.pet_mappings(id, "realm-b").unwrap();
    assert_eq!(active.len(), c0.pets.len());
    assert_eq!((active[0].portable_pet_id, active[0].local_pet_number, active[0].presence), (pet.id, number, Presence::Present));
    let lookup = store.active_pet_lookup(id, "realm-b").unwrap();
    assert_eq!(lookup[&number].0, pet.id, "the export reuses the id for the same pet number");

    // the same pet seen again: confirmed, no new mapping
    let seen = pet_obs(&c0);
    let r = store.sync_pet_mappings(id, "realm-b", 1, &seen, &PetProtection::default()).unwrap();
    assert_eq!((r.confirmed, r.added), (seen.len(), 0));

    // the realm recycles the number for another pet: the old mapping is retired, the newcomer is a new pet
    let stranger = PetObservation { portable_pet_id: PortablePetId::new(), local_pet_number: number, identity: "pet-v1:other".into() };
    let r = store.sync_pet_mappings(id, "realm-b", 1, &[stranger.clone()], &PetProtection { realm_local_pets: [stranger.portable_pet_id].into(), ..Default::default() }).unwrap();
    assert_eq!((r.number_reused, r.added), (1, 1));
    let history = store.pet_mapping_history(id, "realm-b").unwrap();
    let old = history.iter().find(|m| m.portable_pet_id == pet.id).unwrap();
    assert_eq!((old.active, old.retired_reason), (false, Some(RetireReason::GuidReused)));
    let new = history.iter().find(|m| m.portable_pet_id == stranger.portable_pet_id).unwrap();
    assert_eq!((new.active, new.presence), (true, Presence::RealmLocal));

    // a stranger that left is retired; a pet the canonical character owns that the realm does not show would be filtered instead
    let r = store.sync_pet_mappings(id, "realm-b", 1, &[], &PetProtection { canonical_pets: c0.pets.iter().map(|p| p.id).collect(), ..Default::default() }).unwrap();
    assert_eq!(r.absent, 1, "the realm-local stranger is gone");
    assert!(store.pet_mappings(id, "realm-b").unwrap().iter().all(|m| m.portable_pet_id != stranger.portable_pet_id));
    // the same portable pet on a new number is a move
    let moved = PetObservation { portable_pet_id: pet.id, local_pet_number: 7777, identity: seen[0].identity.clone() };
    store.sync_pet_mappings(id, "realm-b", 1, &[moved], &PetProtection::default()).unwrap();
    assert_eq!(store.active_pet_lookup(id, "realm-b").unwrap()[&7777].0, pet.id);
    // duplicate observations are refused
    let dup = PetObservation { portable_pet_id: pet.id, local_pet_number: 1, identity: "x".into() };
    assert!(store.sync_pet_mappings(id, "realm-b", 1, &[dup.clone(), dup], &PetProtection::default()).is_err());
}

#[test]
fn an_update_is_journaled_like_an_import_and_changes_the_mappings_only_when_finished() {
    let (mut store, id, c0) = joined();
    // canonical moves on: one item removed, one new item
    let gone = filterable(&c0, 1)[0];
    let mut c1 = c0.clone();
    c1.items.retain(|i| i.id != gone);
    let fresh_item = realm_item(3);
    let fresh_id = fresh_item.id;
    c1.items.push(fresh_item.clone());
    store.commit_snapshot(id, 1, c1.clone(), "realm-a", None).unwrap();

    let plan = UpdatePlan {
        added_items: vec![PlannedItem { id: fresh_id, entry: fresh_item.entry.clone(), identity: crate::portable::identity::item_identity(&fresh_item.entry, 0) }],
        retired_items: vec![gone],
        ..UpdatePlan::default()
    };
    // updating needs the character to be on the realm already
    assert!(store.begin_update(id, "realm-z", 2, plan.clone()).is_err());
    assert!(matches!(store.begin_update(id, "realm-b", 1, plan.clone()), Err(PortableError::StaleRevision { .. })));
    let ticket = store.begin_update(id, "realm-b", 2, plan.clone()).unwrap();
    let entry = store.import_entry(ticket.import_id).unwrap();
    assert_eq!((entry.kind, entry.state, entry.items.len(), entry.retired_items.clone()), (JournalKind::Update, ImportState::Prepared, 1, vec![gone]));
    assert!(matches!(store.begin_update(id, "realm-b", 2, plan.clone()), Err(PortableError::ImportInProgress { .. })));
    // nothing changed locally before the realm committed
    assert_eq!(store.server_mappings(id).unwrap()[0].last_revision, 1);
    assert!(store.item_mappings(id, "realm-b").unwrap().iter().any(|m| m.portable_item_id == gone));
    assert!(store.synced_model(id, "realm-b").unwrap().unwrap() == c0);

    store.finish_import(ticket.import_id, ImportAllocation { local_guid: 1013, item_base: 88_000, pet_base: 0 }).unwrap();
    let mappings = store.item_mappings(id, "realm-b").unwrap();
    assert!(mappings.iter().all(|m| m.portable_item_id != gone), "the removed item's mapping is retired");
    let added = mappings.iter().find(|m| m.portable_item_id == fresh_id).unwrap();
    assert_eq!((added.local_item_guid, added.presence), (88_000, Presence::Present));
    assert_eq!(store.server_mappings(id).unwrap()[0].last_revision, 2);
    assert_eq!(store.synced_model(id, "realm-b").unwrap().unwrap(), store.load_current(id).unwrap(), "the realm is synced with what it was updated to");
    // the local character is the same one
    assert_eq!(store.server_mappings(id).unwrap()[0].local_guid, 1013);
    // a wrong guid is refused and rolls back
    let second = store.begin_update(id, "realm-b", 2, UpdatePlan::default()).unwrap();
    let r = store.finish_import(second.import_id, ImportAllocation { local_guid: 4040, item_base: 0, pet_base: 0 });
    assert!(r.is_err() && store.import_entry(second.import_id).unwrap().state == ImportState::Prepared);
}

#[test]
fn baselines_mappings_and_the_synced_snapshot_survive_reopening_the_file() {
    let dir = std::env::temp_dir().join(format!("coa-portable-reopen-{}", uuid::Uuid::new_v4()));
    let (id, c0, b0) = {
        let mut store = Store::open(&dir).unwrap();
        let profile = store.default_profile().unwrap();
        let id = store.create_character(profile, fixtures::geared_level_eighty(), "realm-a").unwrap();
        let items = plan_of_current(&store, id);
        let pets = planned_pets_of(&store, id);
        let ticket = store.begin_import(id, "realm-b", 1, &items, &pets).unwrap();
        store.finish_import(ticket.import_id, alloc(1013)).unwrap();
        let c0 = store.load_current(id).unwrap();
        let (b0, _, _) = b0_of(&c0);
        store.capture_baseline(id, "realm-b", BaselineInput { b0: &b0, items: &observed(&c0, &b0), pets: &pet_obs(&b0) }).unwrap();
        (id, c0, b0)
    };
    let mut store = Store::open(&dir).unwrap();
    let baseline = store.open_baseline(id, "realm-b").unwrap().expect("the open session survives a restart");
    assert_eq!((baseline.c0, baseline.b0), (c0.clone(), b0));
    assert_eq!(store.synced_model(id, "realm-b").unwrap().unwrap(), c0);
    assert!(store.item_mappings(id, "realm-b").unwrap().iter().any(|m| m.presence == Presence::Filtered));
    assert_eq!(store.pet_mappings(id, "realm-b").unwrap().len(), c0.pets.len());
    assert!(store.close_baseline(id, "realm-b").unwrap());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
