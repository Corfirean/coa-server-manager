//! Phase 6 on the simulated realm: the selected appearances ride in the character checkpoints, the account collections travel on
//! their own channel and only when their hash changed, and a realm that lacks an unlock never removes it.

use super::fake::*;
use super::protocol::*;
use super::*;
use crate::portable::collection::IdSet;
use crate::portable::fixtures::geared_level_eighty;
use crate::portable::model::PortableCharacter;
use crate::portable::ids::{CharacterId, SessionId};
use crate::portable::realm::knowledge::RealmKnowledge;
use crate::portable::store::Store;

const SERVER: &str = "realm-b";
const GUID: u32 = 4242;
const APPEARANCE: &str = "coa:appearance";
const VANITY: &str = "coa:vanity";

struct Rig {
    owner: Store,
    host: Store,
    realm: FakeRealm,
    id: CharacterId,
    session: SessionId,
}

fn ids(range: std::ops::RangeInclusive<u32>) -> IdSet {
    IdSet::from_ids(range).unwrap()
}

fn knows(appearances: IdSet, vanity: IdSet) -> RealmKnowledge {
    RealmKnowledge::new(appearances, vanity)
}

fn rig(wardrobe: impl FnOnce(&mut PortableCharacter)) -> Rig {
    let mut owner = Store::open_in_memory().unwrap();
    let mut host = Store::open_in_memory().unwrap();
    let profile = owner.default_profile().unwrap();
    let mut model = geared_level_eighty();
    wardrobe(&mut model);
    let id = owner.create_character(profile, model, "realm-a").unwrap();
    let offer = OwnerService::new(&mut owner).offer(id, SERVER).unwrap();
    let host_profile = host.default_profile().unwrap();
    let mut realm = FakeRealm::new(GUID);
    {
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        h.accept_offer(host_profile, &offer).unwrap();
        realm.import(&mut host, id, SERVER, offer.canonical_revision, offer.session_id).unwrap();
        HostService::new(&mut host, SERVER, HostConfig::default()).bind(offer.session_id, GUID).unwrap();
    }
    Rig { owner, host, realm, id, session: offer.session_id }
}

fn deliver_characters(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) {
    for m in h.outbox().unwrap() {
        let ack = if m.started { o.handle_started(&m.bytes) } else { o.handle_checkpoint(&m.bytes) }.unwrap();
        h.receive_ack(realm, &ack).unwrap();
    }
}

/// Carry every queued collection message to the Owner and its answer back. Returns the acknowledgements.
fn deliver_collections(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) -> Vec<CollectionAck> {
    let mut acks = Vec::new();
    for m in h.collection_outbox().unwrap() {
        let ack = o.handle_collection(&m.bytes).unwrap();
        h.receive_collection_ack(realm, m.account, &ack).unwrap();
        acks.push(ack);
    }
    acks
}

fn start(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) -> Vec<HostEvent> {
    realm.login();
    let events = h.tick(realm, 0).unwrap();
    deliver_characters(h, o, realm);
    events
}

fn queued(events: &[HostEvent]) -> Vec<(String, usize)> {
    events.iter().filter_map(|e| if let HostEvent::CollectionQueued { kind, count } = e { Some((kind.clone(), *count)) } else { None }).collect()
}

#[test]
fn the_selected_appearance_travels_in_the_checkpoints_and_no_item_is_created_or_deleted() {
    let Rig { mut owner, mut host, mut realm, id, session } = rig(|m| {
        m.wardrobe.active.insert(1, 100);
    });
    let c0 = owner.load_current(id).unwrap();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    assert_eq!(start(&mut h, &mut o, &mut realm).iter().filter(|e| matches!(e, HostEvent::BaselineTaken { .. })).count(), 1);

    realm.play(|m| {
        m.wardrobe.active.insert(3, 303);
        m.wardrobe.active.insert(56, 5600);
        m.wardrobe.outfits.insert("Sunday".into(), vec![100, 0, 303]);
        m.wardrobe.can_see_item = false;
    });
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    let c1 = o.store().load_current(id).unwrap();
    assert_eq!(c1.wardrobe.active, [(1, 100), (3, 303), (56, 5600)].into_iter().collect());
    assert_eq!(c1.wardrobe.outfits["Sunday"], vec![100, 0, 303]);
    assert!(!c1.wardrobe.can_see_item && c1.wardrobe.can_see_spell);
    let normalised = |m: &crate::portable::model::PortableCharacter| m.items.iter().map(|i| (i.entry.clone(), i.slot, i.count)).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(normalised(&c1), normalised(&c0), "applying an appearance never creates, changes or deletes a gameplay item");

    // deselecting a category, deleting an outfit, then logout
    realm.play(|m| {
        m.wardrobe.active.remove(&56);
        m.wardrobe.outfits.clear();
    });
    realm.logout();
    h.tick(&mut realm, 200).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    let c2 = o.store().load_current(id).unwrap();
    assert_eq!(c2.wardrobe.active, [(1, 100), (3, 303)].into_iter().collect());
    assert!(c2.wardrobe.outfits.is_empty());
    assert_eq!(o.session(session).unwrap().unwrap().0, "closed");
    assert_eq!(normalised(&c2), normalised(&c0));
}

#[test]
fn an_appearance_the_realm_cannot_show_stays_canonical_through_the_whole_session() {
    // the canonical character wears 999, which this realm never showed: its B0 and B1 do not have it
    let Rig { mut owner, mut host, mut realm, id, .. } = rig(|m| {
        m.wardrobe.active.insert(1, 100);
        m.wardrobe.active.insert(2, 999);
        m.wardrobe.outfits.insert("Far away".into(), vec![999, 1000]);
    });
    realm.db.wardrobe = Default::default();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| {
        m.wardrobe.active.insert(3, 303);
    });
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    realm.logout();
    h.tick(&mut realm, 200).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    let c = o.store().load_current(id).unwrap();
    assert_eq!(c.wardrobe.active.get(&2), Some(&999), "kept");
    assert_eq!(c.wardrobe.active.get(&3), Some(&303), "and what the player chose here is added");
    assert!(c.wardrobe.outfits.contains_key("Far away"));
}

#[test]
fn a_collection_is_sent_only_when_its_hash_changed_and_never_with_a_checkpoint() {
    let Rig { mut owner, mut host, mut realm, .. } = rig(|_| {});
    realm.knowledge = knows(ids(1..=10_000), ids(1..=100));
    realm.unlock(APPEARANCE, 1..=5);
    realm.unlock(VANITY, [7, 9]);
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);

    // the session starts: both collections are looked at once and sent
    let events = start_collecting(&mut h, &mut o, &mut realm);
    assert_eq!(queued(&events), vec![(APPEARANCE.to_string(), 5), (VANITY.to_string(), 2)]);
    let acks = deliver_collections(&mut h, &mut o, &mut realm);
    assert!(acks.iter().all(|a| a.outcome == CollectionOutcome::Applied && a.canonical.is_none()), "{acks:?}");
    assert_eq!(acks[0].collection_revision, 1);
    let reads = realm.full_reads;
    assert!(h.collection_outbox().unwrap().is_empty());

    // a 60-second checkpoint cadence never touches the collections: the interval is 300 s and nothing changed
    for now in [100, 160, 220] {
        h.tick(&mut realm, now).unwrap();
        deliver_characters(&mut h, &mut o, &mut realm);
    }
    assert_eq!(realm.full_reads, reads, "nothing was read");

    // due, but the fingerprint is the same: still nothing read, nothing sent
    let before = realm.fingerprints;
    let events = h.tick(&mut realm, 400).unwrap();
    assert!(realm.fingerprints > before, "the cheap look happened");
    assert_eq!(realm.full_reads, reads, "an unchanged fingerprint reads nothing");
    assert!(queued(&events).is_empty() && h.collection_outbox().unwrap().is_empty());

    // the player unlocks more: the next look reads and sends the compact set once
    realm.unlock(APPEARANCE, [6, 77]);
    let events = h.tick(&mut realm, 800).unwrap();
    assert_eq!(queued(&events), vec![(APPEARANCE.to_string(), 7)]);
    let acks = deliver_collections(&mut h, &mut o, &mut realm);
    assert_eq!((acks.len(), &acks[0].outcome, acks[0].collection_revision), (1, &CollectionOutcome::Applied, 2));
    assert!(queued(&h.tick(&mut realm, 1200).unwrap()).is_empty(), "and then it is quiet again");

    // no checkpoint message contains a collection
    for m in h.store().host_message(rig_session(&h), 1).unwrap().into_iter() {
        let text = String::from_utf8(m).unwrap();
        assert!(!text.contains("collection") && !text.contains("\"ids\""), "a checkpoint carries no wardrobe");
    }
}

fn rig_session(h: &HostService) -> SessionId {
    h.store().host_live_sessions(SERVER).unwrap().into_iter().next().unwrap().session_id
}

fn start_collecting(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) -> Vec<HostEvent> {
    start(h, o, realm)
}

#[test]
fn a_realm_that_lacks_an_unlock_never_removes_it_and_gets_what_it_knows_back() {
    let Rig { mut owner, mut host, mut realm, .. } = rig(|_| {});
    // the Owner already holds a big wardrobe from other realms
    let profile = owner.default_profile().unwrap();
    owner.merge_collection(profile, APPEARANCE, &ids(1..=500)).unwrap();
    owner.merge_collection(profile, VANITY, &ids(1000..=1010)).unwrap();
    // this realm knows only part of it and holds only a few
    realm.knowledge = knows(ids(1..=300), ids(1000..=1005));
    realm.unlock(APPEARANCE, [5, 6, 700]);
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    let acks = deliver_collections(&mut h, &mut o, &mut realm);

    // the Owner unioned the realm's ids (700 included: the realm showed it) and kept everything else
    let appearance_ack = acks.iter().find(|a| a.kind == APPEARANCE).unwrap();
    assert_eq!(appearance_ack.outcome, CollectionOutcome::Applied);
    let (_, canonical) = o.store().collection(profile, APPEARANCE).unwrap().unwrap();
    assert_eq!(canonical.len(), 501);
    assert!(canonical.contains(700) && canonical.contains(1) && canonical.contains(500), "nothing was removed");

    // the realm received what its client data knows: 1..=300 minus what it had; the ids it cannot show stay canonical only
    assert!((1..=300).all(|id| realm.appearance_rows.contains(id)));
    assert!(!realm.appearance_rows.contains(301) && !realm.appearance_rows.contains(500), "unknown to this realm: not written");
    assert!(realm.appearance_rows.contains(700), "its own row is never removed");
    assert!((1000..=1005).all(|id| realm.vanity_rows.contains(id)) && !realm.vanity_rows.contains(1006));
}

#[test]
fn an_account_with_unlocks_arrives_on_a_second_realm_from_the_owners_state_alone() {
    let mut owner = Store::open_in_memory().unwrap();
    let profile = owner.default_profile().unwrap();
    owner.merge_collection(profile, APPEARANCE, &ids(1..=40)).unwrap();
    owner.merge_collection(profile, VANITY, &ids(50..=60)).unwrap();
    let states = OwnerService::new(&mut owner).collection_states().unwrap();
    assert_eq!(states.len(), 2);

    let mut host = Store::open_in_memory().unwrap();
    let mut realm = FakeRealm::new(GUID);
    realm.knowledge = knows(ids(1..=30), ids(50..=60));
    realm.unlock(APPEARANCE, [3]);
    let mut h = HostService::new(&mut host, "realm-c", HostConfig::default());
    for state in &states {
        let applied = h.receive_collection_state(&mut realm, 1, state).unwrap().unwrap();
        assert_eq!(applied.unknown, if state.kind == APPEARANCE { 10 } else { 0 });
    }
    assert_eq!(realm.appearance_rows.len(), 30);
    assert_eq!(realm.vanity_rows.len(), 11);
    // the same state again writes nothing
    let writes = realm.collection_writes;
    for state in &states {
        assert_eq!(h.receive_collection_state(&mut realm, 1, state).unwrap().unwrap().inserted, 0);
    }
    assert_eq!(realm.collection_writes, writes);
}

#[test]
fn hostile_or_broken_collection_messages_change_nothing() {
    let mut owner = Store::open_in_memory().unwrap();
    let profile = owner.default_profile().unwrap();
    let mut o = OwnerService::new(&mut owner);
    let good = collection_to_json(&CollectionObserved::new(SERVER, APPEARANCE, &ids(1..=10)).unwrap()).unwrap();
    assert_eq!(o.handle_collection(&good).unwrap().outcome, CollectionOutcome::Applied);
    let state_of = |o: &OwnerService| o.store().collection_info(profile, APPEARANCE).unwrap().unwrap();
    let before = state_of(&o);

    // garbage, wrong version, unknown field, over the size cap: errors, nothing changes
    assert!(o.handle_collection(b"not json").is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["protocol_version"] = 99.into();
    assert!(o.handle_collection(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["extra"] = 1.into();
    assert!(o.handle_collection(&serde_json::to_vec(&v).unwrap()).is_err());
    assert!(o.handle_collection(&vec![b' '; MAX_COLLECTION_MESSAGE_BYTES + 1]).is_err());

    // parsed but not acceptable: answered with a rejection, never merged
    for (what, edit) in [
        ("a hash that does not match", Box::new(|v: &mut serde_json::Value| v["set"]["hash"] = "00".repeat(32).into()) as Box<dyn Fn(&mut serde_json::Value)>),
        ("a count that does not match", Box::new(|v: &mut serde_json::Value| v["set"]["count"] = 3.into())),
        ("a kind that is not carried", Box::new(|v: &mut serde_json::Value| v["kind"] = "coa:mounts".into())),
        ("a payload that is not base64", Box::new(|v: &mut serde_json::Value| v["set"]["ids"] = "***".into())),
        ("a payload with trailing bytes", Box::new(|v: &mut serde_json::Value| v["set"]["ids"] = "AQoBAQEBAQEBAQEBAQEBAAAA".into())),
    ] {
        let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
        edit(&mut v);
        let ack = o.handle_collection(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(matches!(ack.outcome, CollectionOutcome::Rejected(_)), "{what}: {ack:?}");
    }
    assert_eq!(state_of(&o), before, "nothing was merged by any of them");

    // a set that decodes to more ids than the cap allows is refused before it is built
    let mut huge = vec![1u8];
    huge.extend([0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    let message = serde_json::json!({"protocol_version": 1, "server_id": SERVER, "kind": APPEARANCE, "set": {"hash": "00".repeat(32), "count": 1, "ids": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, huge)}});
    assert!(matches!(o.handle_collection(&serde_json::to_vec(&message).unwrap()).unwrap().outcome, CollectionOutcome::Rejected(_)));
}

#[test]
fn ten_thousand_and_fifty_thousand_ids_stay_cheap() {
    for count in [10_000u32, 50_000] {
        let started = std::time::Instant::now();
        let Rig { mut owner, mut host, mut realm, .. } = rig(|_| {});
        // scattered ids, as a real wardrobe is
        let scattered: Vec<u32> = (0..count).map(|i| 1 + i * 7 + (i * i) % 5).collect();
        realm.knowledge = knows(IdSet::from_ids(scattered.iter().copied()).unwrap(), IdSet::new());
        realm.unlock(APPEARANCE, scattered.iter().copied());
        let profile = owner.default_profile().unwrap();
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        let mut o = OwnerService::new(&mut owner);
        let events = start(&mut h, &mut o, &mut realm);
        assert_eq!(queued(&events), vec![(APPEARANCE.to_string(), count as usize), (VANITY.to_string(), 0)], "the first look reports every kind, even an empty one");

        let messages: Vec<_> = h.collection_outbox().unwrap().into_iter().filter(|m| m.kind == APPEARANCE).collect();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].bytes.len() < count as usize * 4, "{} ids -> {} bytes: compact, not a JSON array", count, messages[0].bytes.len());
        assert!(!String::from_utf8_lossy(&messages[0].bytes).contains(&format!("{},", scattered[1])), "no verbose id list");
        let ack = o.handle_collection(&messages[0].bytes).unwrap();
        assert_eq!(ack.outcome, CollectionOutcome::Applied);
        h.receive_collection_ack(&mut realm, messages[0].account, &ack).unwrap();

        // unchanged: a look costs one fingerprint and reads nothing, however big the collection is
        let reads = realm.full_reads;
        h.tick(&mut realm, 400).unwrap();
        h.tick(&mut realm, 900).unwrap();
        assert_eq!(realm.full_reads, reads);
        let stored = o.store().collection_info(profile, APPEARANCE).unwrap().unwrap();
        assert_eq!(stored.count, count as usize);

        // the Owner's answer to a realm with nothing is the whole compact set, once
        let mut empty_host = Store::open_in_memory().unwrap();
        let mut empty_realm = FakeRealm::new(GUID);
        empty_realm.knowledge = realm.knowledge.clone();
        let state = o.collection_states().unwrap().into_iter().find(|s| s.kind == APPEARANCE).unwrap();
        let applied = HostService::new(&mut empty_host, "realm-c", HostConfig::default()).receive_collection_state(&mut empty_realm, 1, &state).unwrap().unwrap();
        assert_eq!(applied.inserted, count as usize);
        assert!(started.elapsed().as_secs() < 20, "{count} ids took {:?}", started.elapsed());
    }
}

#[test]
fn a_lost_acknowledgement_resends_the_same_set_and_the_owner_counts_it_once() {
    let Rig { mut owner, mut host, mut realm, .. } = rig(|_| {});
    realm.knowledge = knows(ids(1..=100), IdSet::new());
    realm.unlock(APPEARANCE, 1..=20);
    let profile = owner.default_profile().unwrap();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    // the message reaches the Owner but its answer is lost
    let m = h.collection_outbox().unwrap().into_iter().find(|m| m.kind == APPEARANCE).unwrap();
    assert_eq!(o.handle_collection(&m.bytes).unwrap().outcome, CollectionOutcome::Applied);
    assert!(h.collection_outbox().unwrap().iter().any(|p| p.kind == APPEARANCE), "the Host still holds it");
    // delivered again: the union adds nothing
    let ack = o.handle_collection(&m.bytes).unwrap();
    assert_eq!(ack.outcome, CollectionOutcome::Unchanged);
    h.receive_collection_ack(&mut realm, m.account, &ack).unwrap();
    assert!(!h.collection_outbox().unwrap().iter().any(|p| p.kind == APPEARANCE));
    assert_eq!(o.store().collection_info(profile, APPEARANCE).unwrap().unwrap().revision, 1, "one revision, not two");
}

#[test]
fn what_the_host_has_not_had_acknowledged_survives_a_restart_and_what_it_has_acknowledged_is_not_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let owner_dir = dir.path().join("owner");
    let host_dir = dir.path().join("host");
    let Rig { owner: _, host: _, mut realm, .. } = rig(|_| {});
    realm.knowledge = knows(ids(1..=100), IdSet::new());
    realm.unlock(APPEARANCE, 1..=10);
    // the same character on file-backed stores
    let mut owner = Store::open(&owner_dir).unwrap();
    let mut host = Store::open(&host_dir).unwrap();
    let profile = owner.default_profile().unwrap();
    let id = owner.create_character(profile, geared_level_eighty(), "realm-a").unwrap();
    let offer = OwnerService::new(&mut owner).offer(id, SERVER).unwrap();
    let host_profile = host.default_profile().unwrap();
    HostService::new(&mut host, SERVER, HostConfig::default()).accept_offer(host_profile, &offer).unwrap();
    realm.import(&mut host, id, SERVER, offer.canonical_revision, offer.session_id).unwrap();
    {
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        h.bind(offer.session_id, GUID).unwrap();
        realm.login();
        h.tick(&mut realm, 0).unwrap();
        assert!(h.collection_outbox().unwrap().iter().any(|m| m.kind == APPEARANCE));
    }
    drop(host);

    // the Host process restarts: the unacknowledged message is still there, the realm is not read again for it
    let mut host = Store::open(&host_dir).unwrap();
    let reads = realm.full_reads;
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    let pending: Vec<_> = h.collection_outbox().unwrap();
    assert!(pending.iter().any(|m| m.kind == APPEARANCE));
    for m in &pending {
        let ack = o.handle_collection(&m.bytes).unwrap();
        h.receive_collection_ack(&mut realm, m.account, &ack).unwrap();
    }
    assert!(h.collection_outbox().unwrap().is_empty());
    assert_eq!(realm.full_reads, reads);

    // another restart: acknowledged is acknowledged, and the unchanged fingerprint means no read and no message
    drop(h);
    drop(host);
    let mut host = Store::open(&host_dir).unwrap();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let events = h.tick(&mut realm, 5000).unwrap();
    assert!(queued(&events).is_empty() && h.collection_outbox().unwrap().is_empty());
    assert_eq!(realm.full_reads, reads, "nothing was read again after the first look of the two kinds");
}

#[test]
fn a_session_that_changes_nothing_on_a_format_1_store_makes_no_new_revision() {
    let Rig { mut owner, mut host, mut realm, id, .. } = rig(|_| {});
    owner.downgrade_head_to_format_1(id).unwrap();
    let before = owner.character(id).unwrap().revision;
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    realm.logout();
    h.tick(&mut realm, 200).unwrap();
    deliver_characters(&mut h, &mut o, &mut realm);
    assert_eq!(o.store().character(id).unwrap().revision, before, "the same character in a newer format is not a change");
}
