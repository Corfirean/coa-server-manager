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

#[test]
fn item_mapping_unique_per_character_and_server() {
    let (mut store, profile) = fresh();
    let id = created(&mut store, profile);
    let model = store.load_current(id).unwrap();
    let items: Vec<PortableItemId> = model.items.iter().take(4).map(|i| i.id).collect();

    // an item mapping needs a server binding first
    assert!(store.set_item_mappings(id, "realm-a", &[(items[0], 1)]).is_err());
    store.bind_server(id, "realm-a", 154, 1, MappingState::Active).unwrap();

    store.set_item_mappings(id, "realm-a", &[(items[0], 9001), (items[1], 9002), (items[2], 9003)]).unwrap();
    assert_eq!(store.item_mappings(id, "realm-a").unwrap(), vec![(items[0], 9001), (items[1], 9002), (items[2], 9003)]);

    // UNIQUE(character_id, server_id, local_item_guid): one local item guid cannot stand for two portable items
    let clash = store.set_item_mappings(id, "realm-a", &[(items[0], 9001), (items[1], 9001)]);
    assert!(matches!(clash, Err(PortableError::ItemGuidConflict { local_item_guid: 9001, .. })), "{clash:?}");
    // and the same portable item cannot have two local guids
    assert!(store.set_item_mappings(id, "realm-a", &[(items[0], 1), (items[0], 2)]).is_err());
    // a failed replacement leaves the previous mappings intact
    assert_eq!(store.item_mappings(id, "realm-a").unwrap().len(), 3);

    // the database itself enforces the same rule, not just the Rust check
    let direct = store.conn.execute(
        "INSERT INTO item_mapping(character_id, server_id, portable_item_id, local_item_guid) VALUES (?1, 'realm-a', ?2, 9001)",
        params![id.to_string(), items[3].to_string()],
    );
    assert!(direct.is_err(), "UNIQUE(character_id, server_id, local_item_guid)");

    // the same local item guid on another realm is fine
    store.bind_server(id, "realm-b", 8421, 1, MappingState::Active).unwrap();
    store.set_item_mappings(id, "realm-b", &[(items[0], 9001)]).unwrap();
    assert_eq!(store.item_mappings(id, "realm-b").unwrap(), vec![(items[0], 9001)]);

    // if the local character changes (deleted and imported again), the old item guids are meaningless
    store.bind_server(id, "realm-a", 777, 1, MappingState::Active).unwrap();
    assert!(store.item_mappings(id, "realm-a").unwrap().is_empty());
    assert_eq!(store.item_mappings(id, "realm-b").unwrap().len(), 1, "other realms are untouched");
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
