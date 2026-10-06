//! The Owner and the Host as two separate stores, a simulated realm between them, and the messages carried by hand.

use super::bridge::*;
use super::fake::*;
use super::protocol::*;
use super::*;
use crate::portable::fixtures::geared_level_eighty;
use crate::portable::ids::{CharacterId, PortableItemId, SessionId};
use crate::portable::model::*;
use crate::portable::store::{HostState, Store};

const SERVER: &str = "realm-b";
const GUID: u32 = 4242;

struct Setup {
    owner: Store,
    host: Store,
    realm: FakeRealm,
    id: CharacterId,
    session: SessionId,
    c0: PortableCharacter,
}

/// What a realm does by itself when it loads a character: default spells, resets, items it holds back, items of its own.
fn normalizes(r: &mut FakeRealm) {
    r.memory.build.spells.extend([(6654, 255), (23250, 255)]);
    r.memory.progression.honor.today_honor = 0;
    r.memory.progression.chosen_title = 0;
    r.memory.progression.known_currencies |= 1 << 40;
    let held: Vec<PortableItemId> = plain(&r.memory).into_iter().take(4).collect();
    r.memory.items.retain(|i| !held.contains(&i.id));
    r.give_item(new_internal_item(1, 30, 66_001));
    r.give_item(new_internal_item(2, 31, 66_002));
}

fn plain(m: &PortableCharacter) -> Vec<PortableItemId> {
    let containers: std::collections::HashSet<PortableItemId> = m.items.iter().filter_map(|i| i.container).collect();
    m.items.iter().filter(|i| i.container.is_none() && !containers.contains(&i.id) && (23..40).contains(&i.slot)).map(|i| i.id).collect()
}

/// Canonical revision 10 on the Owner; the realm has the character imported and armed, not yet logged in.
fn setup() -> Setup {
    setup_in(Store::open_in_memory().unwrap(), Store::open_in_memory().unwrap())
}

fn setup_in(mut owner: Store, mut host: Store) -> Setup {
    let profile = owner.default_profile().unwrap();
    let id = owner.create_character(profile, geared_level_eighty(), "realm-a").unwrap();
    for _ in 2..=10 {
        let mut m = owner.load_current(id).unwrap();
        m.progression.xp += 1;
        let rev = owner.character(id).unwrap().revision;
        owner.commit_snapshot(id, rev, m, "realm-a", None).unwrap();
    }
    assert_eq!(owner.character(id).unwrap().revision, 10);
    let c0 = owner.load_current(id).unwrap();
    let offer = OwnerService::new(&mut owner).offer(id, SERVER).unwrap();
    let host_profile = host.default_profile().unwrap();
    let mut realm = FakeRealm::new(GUID);
    realm.normalize = normalizes;
    {
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        h.accept_offer(host_profile, &offer).unwrap();
        realm.import(&mut host, id, SERVER, offer.canonical_revision, offer.session_id).unwrap();
        HostService::new(&mut host, SERVER, HostConfig::default()).bind(offer.session_id, GUID).unwrap();
    }
    Setup { owner, host, realm, id, session: offer.session_id, c0 }
}

/// Carry every queued message to the Owner and its answer back.
fn deliver(host: &mut HostService, owner: &mut OwnerService, realm: &mut FakeRealm) -> Vec<OwnerAck> {
    let mut acks = Vec::new();
    for m in host.outbox().unwrap() {
        let ack = if m.started { owner.handle_started(&m.bytes) } else { owner.handle_checkpoint(&m.bytes) }.unwrap();
        host.receive_ack(realm, &ack).unwrap();
        acks.push(ack);
    }
    acks
}

fn revision(owner: &OwnerService, id: CharacterId) -> u64 {
    owner.store().character(id).unwrap().revision
}

/// One checkpoint: request, then wait for the marker and read.
fn checkpoint(host: &mut HostService, owner: &mut OwnerService, realm: &mut FakeRealm, now: u64) -> Vec<HostEvent> {
    let mut events = host.tick(realm, now).unwrap();
    events.extend(host.tick(realm, now + 1).unwrap());
    deliver(host, owner, realm);
    events
}

#[test]
fn the_whole_session_from_an_automatic_baseline_to_the_final_checkpoint() {
    let Setup { mut owner, mut host, mut realm, id, session, c0 } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);

    // the player logs in; the realm normalises and holds the player; the Host takes B0 without anybody doing anything
    realm.login();
    assert!(realm.gated, "the core holds the player until B0 exists");
    let events = h.tick(&mut realm, 0).unwrap();
    assert_eq!(events, vec![HostEvent::BaselineTaken { session }]);
    assert!(!realm.gated, "released only after B0 was persisted");
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!((acks.len(), &acks[0].outcome), (1, &AckOutcome::Applied));
    assert_eq!(o.session(session).unwrap().unwrap().0, "open");
    assert_eq!(revision(&o, id), 10, "a baseline changes nothing canonical");

    // play, checkpoint #1 -> revision 11
    realm.play(|m| {
        m.progression.money += 5_000;
        m.progression.honor.total_kills += 3;
        m.build.spells.push((1111, 255));
        m.quests.rewarded.push(777);
        m.pets[0].exp += 100;
    });
    let bonus = new_internal_item(3, 32, 77_001);
    realm.give_item(bonus.clone());
    realm.memory.items.retain(|i| i.entry.id() != 66_002);
    let events = checkpoint(&mut h, &mut o, &mut realm, 100);
    assert!(events.contains(&HostEvent::CheckpointRequested { session, sequence: 1 }) && events.contains(&HostEvent::CheckpointQueued { session, sequence: 1, final_checkpoint: false }), "{events:?}");
    assert_eq!(revision(&o, id), 11);
    let c1 = o.store().load_current(id).unwrap();
    assert_eq!(c1.progression.money, c0.progression.money + 5_000);
    assert_eq!(c1.progression.honor.total_kills, c0.progression.honor.total_kills + 3);
    assert!(c1.build.spells.contains(&(1111, 255)) && c1.quests.rewarded.contains(&777));
    assert_eq!(c1.pets[0].exp, c0.pets[0].exp + 100);
    assert!(c1.items.iter().any(|i| i.entry.id() == 77_001), "the new item is a session gain");
    assert!(c1.items.iter().all(|i| i.entry.id() != 66_001 && i.entry.id() != 66_002), "the realm's own items never become canonical");
    // realm normalisation did not propagate, and what the realm held back is still canonical
    assert!(!c1.build.spells.contains(&(6654, 255)) && !c1.build.spells.contains(&(23250, 255)));
    assert_eq!(c1.progression.honor.today_honor, c0.progression.honor.today_honor);
    assert_eq!(c1.progression.chosen_title, c0.progression.chosen_title);
    assert_eq!(c1.progression.known_currencies, c0.progression.known_currencies);
    assert_eq!(c1.items.len(), c0.items.len() + 1, "four items the realm held back stay, one session gain");
    let first = h.outbox().unwrap();
    assert!(first.is_empty(), "the acknowledgement emptied the outbox");

    // the same checkpoint delivered again: nothing happens
    let sent = host_outbox_history(&h, session, 1);
    let again = o.handle_checkpoint(&sent).unwrap();
    assert_eq!(again.outcome, AckOutcome::Duplicate);
    assert_eq!(revision(&o, id), 11);
    assert_eq!(again.canonical_revision, 11);

    // more play, checkpoint #2 -> revision 12; money counted once
    realm.play(|m| m.progression.money += 250);
    checkpoint(&mut h, &mut o, &mut realm, 200);
    assert_eq!(revision(&o, id), 12);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 5_250);

    // the delayed #1 arrives after #2: ignored, the original answer is repeated
    let late = o.handle_checkpoint(&sent).unwrap();
    assert!(matches!(late.outcome, AckOutcome::Duplicate) && late.canonical_revision == 11);
    assert_eq!(revision(&o, id), 12);

    // logout -> final checkpoint -> the owner's final revision, the session closes, the next one is armed
    realm.logout();
    let events = h.tick(&mut realm, 300).unwrap();
    assert!(events.contains(&HostEvent::CheckpointQueued { session, sequence: 3, final_checkpoint: true }), "{events:?}");
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!(acks.len(), 1);
    let next = acks[0].next_session.clone().expect("the final acknowledgement names the next session");
    assert_eq!(o.session(session).unwrap().unwrap().0, "closed");
    assert!(revision(&o, id) >= 12);
    let row = realm.row.clone().unwrap();
    assert_eq!((row.session_id, row.state, row.generation), (next.session_id, RowState::WaitingBaseline, 2), "the realm is armed for the next login");
    let hs = h.store().host_session(session).unwrap().unwrap();
    assert_eq!(hs.state, HostState::Closed);
    let synced = h.store().synced_model(id, SERVER).unwrap().unwrap();
    assert_eq!(synced, o.store().load_current(id).unwrap(), "the Host's realm is synchronised with what the Owner now holds");
}

/// The bytes of a message the Host queued (kept in the outbox table after acknowledgement).
fn host_outbox_history(h: &HostService, session: SessionId, sequence: u64) -> Vec<u8> {
    h.store().host_message(session, sequence).unwrap().expect("the message is kept")
}

// ---- the matrix ----------------------------------------------------------------------------------------------------------------------

fn start(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) {
    realm.login();
    if realm.gated {
        h.tick(realm, 0).unwrap();
        deliver(h, o, realm);
    }
}

fn tamper(bytes: &[u8], f: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    f(&mut v);
    serde_json::to_vec(&v).unwrap()
}

fn sealed_checkpoint(host: &Store, session: SessionId, sequence: u64) -> Vec<u8> {
    host.host_message(session, sequence).unwrap().expect("queued")
}

#[test]
fn a_host_that_never_answers_never_lets_the_player_play_and_the_baseline_is_retaken_at_the_next_login() {
    let Setup { mut owner, mut host, mut realm, id, session, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    realm.login();
    assert!(realm.gated);
    realm.gate_timeout();
    assert!(!realm.online && realm.row.as_ref().unwrap().state == RowState::BaselineReady && realm.released.is_empty());
    realm.login();
    assert!(realm.gated);
    assert_eq!(h.tick(&mut realm, 0).unwrap(), vec![HostEvent::BaselineTaken { session }]);
    deliver(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 1);
    assert_eq!(revision(&o, id), 10);
}

#[test]
fn a_failed_intermediate_checkpoint_changes_nothing_canonical_and_is_retried() {
    let Setup { mut owner, mut host, mut realm, id, session, c0 } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 100);
    realm.busy = 2;
    let e1 = h.tick(&mut realm, 100).unwrap();
    let e2 = h.tick(&mut realm, 101).unwrap();
    assert!(e1.contains(&HostEvent::CheckpointBusy { session, sequence: 1 }) && e2.contains(&HostEvent::CheckpointBusy { session, sequence: 1 }), "{e1:?} {e2:?}");
    assert_eq!(revision(&o, id), 10, "a failed checkpoint changes nothing");
    assert!(h.outbox().unwrap().is_empty());
    h.tick(&mut realm, 102).unwrap();
    h.tick(&mut realm, 103).unwrap();
    deliver(&mut h, &mut o, &mut realm);
    assert_eq!(revision(&o, id), 11);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 100);
    assert_eq!(h.store().host_session(session).unwrap().unwrap().acked_sequence, 1, "the retry used the same sequence number");
}

#[test]
fn a_lost_acknowledgement_is_healed_by_redelivery_and_nothing_is_counted_twice() {
    let Setup { mut owner, mut host, mut realm, id, c0, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 700);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    let msg = h.outbox().unwrap().remove(0);
    let lost = o.handle_checkpoint(&msg.bytes).unwrap();
    assert_eq!((lost.outcome.clone(), revision(&o, id)), (AckOutcome::Applied, 11));
    assert_eq!(h.outbox().unwrap().len(), 1, "the Host still holds the message");
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!(acks[0].outcome, AckOutcome::Duplicate);
    assert_eq!(revision(&o, id), 11);
    assert!(h.outbox().unwrap().is_empty());
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 700);
}

#[test]
fn the_host_manager_can_restart_between_the_request_and_the_read_and_between_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let host_file = dir.path().join("host.db");
    let Setup { mut owner, host, mut realm, id, session, c0 } = setup_in(Store::open_in_memory().unwrap(), Store::open_file(&host_file).unwrap());
    drop(host);
    let mut o = OwnerService::new(&mut owner);
    {
        let mut hs = Store::open_file(&host_file).unwrap();
        let mut h = HostService::new(&mut hs, SERVER, HostConfig::default());
        start(&mut h, &mut o, &mut realm);
        realm.play(|m| m.progression.money += 40);
        h.tick(&mut realm, 100).unwrap();
        assert_eq!(realm.row.as_ref().unwrap().checkpoint_seq, 1, "the realm saved at the request");
    }
    let mut hs = Store::open_file(&host_file).unwrap();
    assert_eq!(hs.host_session(session).unwrap().unwrap().pending_sequence, Some(1), "the reserved sequence survived");
    {
        let mut h = HostService::new(&mut hs, SERVER, HostConfig::default());
        h.tick(&mut realm, 101).unwrap();
        deliver(&mut h, &mut o, &mut realm);
        assert_eq!(revision(&o, id), 11);
        realm.play(|m| m.progression.money += 60);
        h.tick(&mut realm, 200).unwrap();
        h.tick(&mut realm, 201).unwrap();
        assert_eq!(h.outbox().unwrap().len(), 1);
    }
    drop(hs);
    let mut hs = Store::open_file(&host_file).unwrap();
    let mut h = HostService::new(&mut hs, SERVER, HostConfig::default());
    assert_eq!(h.outbox().unwrap().len(), 1, "the outbox is persistent");
    deliver(&mut h, &mut o, &mut realm);
    assert_eq!(revision(&o, id), 12);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 100);
}

#[test]
fn the_owner_manager_can_restart_at_any_point() {
    let dir = tempfile::tempdir().unwrap();
    let owner_file = dir.path().join("owner.db");
    let Setup { owner, mut host, mut realm, id, session, c0 } = setup_in(Store::open_file(&owner_file).unwrap(), Store::open_in_memory().unwrap());
    drop(owner);
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    {
        let mut os = Store::open_file(&owner_file).unwrap();
        let mut o = OwnerService::new(&mut os);
        start(&mut h, &mut o, &mut realm);
    }
    realm.play(|m| m.progression.money += 5);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    {
        let mut os = Store::open_file(&owner_file).unwrap();
        let mut o = OwnerService::new(&mut os);
        assert_eq!(o.session(session).unwrap().unwrap().0, "open", "the baseline survived the restart");
        deliver(&mut h, &mut o, &mut realm);
        assert_eq!(revision(&o, id), 11);
    }
    realm.play(|m| m.progression.money += 6);
    h.tick(&mut realm, 200).unwrap();
    h.tick(&mut realm, 201).unwrap();
    let mut os = Store::open_file(&owner_file).unwrap();
    let mut o = OwnerService::new(&mut os);
    deliver(&mut h, &mut o, &mut realm);
    assert_eq!(revision(&o, id), 12);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 11);
}

#[test]
fn a_worldserver_restart_continues_the_same_session_without_a_new_baseline() {
    let Setup { mut owner, mut host, mut realm, id, session, c0 } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 10);
    checkpoint(&mut h, &mut o, &mut realm, 100);
    assert_eq!(revision(&o, id), 11);
    realm.play(|m| m.progression.money += 99);
    realm.crash();
    assert!(h.tick(&mut realm, 150).unwrap().is_empty(), "an offline character with a running session needs nothing");
    assert_eq!(revision(&o, id), 11, "the last acknowledged checkpoint wins");
    realm.login();
    assert!(!realm.gated);
    assert_eq!(realm.row.as_ref().unwrap().session_id, session);
    realm.play(|m| m.progression.money += 5);
    checkpoint(&mut h, &mut o, &mut realm, 200);
    assert_eq!(revision(&o, id), 12);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 15, "the lost 99 stayed lost, the rest was counted once");
}

#[test]
fn a_superseded_session_is_refused_and_closed() {
    let Setup { mut owner, mut host, mut realm, id, session, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    {
        let mut o = OwnerService::new(&mut owner);
        start(&mut h, &mut o, &mut realm);
    }
    let mut m = owner.load_current(id).unwrap();
    m.progression.money += 1;
    owner.commit_snapshot(id, 10, m, "realm-c", None).unwrap();
    let mut o = OwnerService::new(&mut owner);
    realm.play(|m| m.progression.money += 3);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!(acks[0].outcome, AckOutcome::StaleSession);
    assert_eq!(revision(&o, id), 11, "nothing was merged over the newer state");
    assert_eq!(o.store().load_current(id).unwrap().progression.money, geared_level_eighty().progression.money + 1);
    assert_eq!(h.store().host_session(session).unwrap().unwrap().state, HostState::Closed);
    let msg = sealed_checkpoint(h.store(), session, 1);
    assert_eq!(o.handle_checkpoint(&msg).unwrap().outcome, AckOutcome::StaleSession);
}

#[test]
fn messages_for_the_wrong_character_session_or_realm_and_malformed_ones_change_nothing() {
    let Setup { mut owner, mut host, mut realm, id, session, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 9);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    let good = h.outbox().unwrap().remove(0).bytes;
    let before = revision(&o, id);
    let other_character = CharacterId::new();
    let other_session = SessionId::new();

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("another character", tamper(&good, |v| v["character_id"] = other_character.to_string().into())),
        ("an unknown session", tamper(&good, |v| v["session_id"] = other_session.to_string().into())),
        ("another realm", tamper(&good, |v| v["server_id"] = "realm-z".into())),
        ("another base revision", tamper(&good, |v| v["base_canonical_revision"] = 3.into())),
        ("a wrong content hash", tamper(&good, |v| v["content_hash"] = "00".repeat(32).into())),
        ("sequence zero", tamper(&good, |v| v["sequence"] = 0.into())),
        ("a snapshot of another character", {
            let mut other = geared_level_eighty();
            other.character_id = other_character;
            to_json(&PortableCheckpoint::new(session, id, SERVER, 10, 1, false, &other).unwrap()).unwrap()
        }),
        ("a snapshot that does not match its hash", tamper(&good, |v| {
            let payload = v["realm_snapshot"]["payload"].as_str().unwrap().to_string();
            v["realm_snapshot"]["payload"] = format!("{}AA", &payload[..payload.len() - 4]).into();
        })),
    ];
    for (name, bytes) in cases {
        let ack = o.handle_checkpoint(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(matches!(ack.outcome, AckOutcome::Rejected(_)), "{name}: {:?}", ack.outcome);
        assert_eq!(revision(&o, id), before, "{name} must not change the canonical character");
    }
    for (name, bytes) in [
        ("garbage", b"\x00\x01 not json".to_vec()),
        ("an unknown field", tamper(&good, |v| v["extra"] = 1.into())),
        ("a newer protocol", tamper(&good, |v| v["protocol_version"] = 99.into())),
        ("no protocol version", tamper(&good, |v| {
            v.as_object_mut().unwrap().remove("protocol_version");
        })),
        ("an over-limit message", vec![b' '; MAX_MESSAGE_BYTES + 1]),
    ] {
        assert!(o.handle_checkpoint(&bytes).is_err(), "{name}");
        assert_eq!(revision(&o, id), before, "{name}");
    }
    let bomb = zstd::stream::encode_all(vec![b' '; crate::portable::model::limits::MAX_SNAPSHOT_BYTES + 4096].as_slice(), 19).unwrap();
    let evil = tamper(&good, |v| {
        v["realm_snapshot"]["payload"] = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bomb).into();
        v["realm_snapshot"]["content_hash"] = "11".repeat(32).into();
        v["content_hash"] = "11".repeat(32).into();
    });
    assert!(matches!(o.handle_checkpoint(&evil).unwrap().outcome, AckOutcome::Rejected(_)));
    assert_eq!(revision(&o, id), before);
    assert_eq!(o.handle_checkpoint(&good).unwrap().outcome, AckOutcome::Applied);
    assert_eq!(revision(&o, id), before + 1);
}

#[test]
fn checkpoints_out_of_order_or_with_reused_sequences_are_refused_safely() {
    let Setup { mut owner, mut host, mut realm, id, session, c0 } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 10);
    h.tick(&mut realm, 100).unwrap();
    h.tick(&mut realm, 101).unwrap();
    realm.play(|m| m.progression.money += 20);
    h.tick(&mut realm, 200).unwrap();
    h.tick(&mut realm, 201).unwrap();
    let (one, two) = (sealed_checkpoint(h.store(), session, 1), sealed_checkpoint(h.store(), session, 2));
    assert_eq!(o.handle_checkpoint(&two).unwrap().outcome, AckOutcome::Applied);
    assert_eq!(revision(&o, id), 11);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 30);
    assert_eq!(o.handle_checkpoint(&one).unwrap().outcome, AckOutcome::StaleSequence);
    assert_eq!(revision(&o, id), 11);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 30);
    let reused = tamper(&two, |v| {
        let mut m = geared_level_eighty();
        m.progression.money += 123;
        let other = PortableCheckpoint::new(session, id, SERVER, 10, 2, false, &m).unwrap();
        v["realm_snapshot"] = serde_json::to_value(&other.realm_snapshot).unwrap();
        v["content_hash"] = other.content_hash.into();
    });
    assert!(matches!(o.handle_checkpoint(&reused).unwrap().outcome, AckOutcome::Rejected(_)));
    assert_eq!(revision(&o, id), 11);
}

#[test]
fn a_checkpoint_that_changes_nothing_makes_no_revision_and_new_items_keep_their_ids() {
    let Setup { mut owner, mut host, mut realm, id, c0, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.give_item(new_internal_item(9, 40, 88_001));
    realm.play(|m| m.progression.money += 1);
    checkpoint(&mut h, &mut o, &mut realm, 100);
    assert_eq!(revision(&o, id), 11);
    let gained: Vec<PortableItemId> = o.store().load_current(id).unwrap().items.iter().filter(|i| i.entry.id() == 88_001).map(|i| i.id).collect();
    assert_eq!(gained.len(), 1);
    checkpoint(&mut h, &mut o, &mut realm, 200);
    assert_eq!(revision(&o, id), 11, "no new revision for an unchanged character");
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 1, "nothing was counted again");
    realm.play(|m| m.progression.money += 1);
    checkpoint(&mut h, &mut o, &mut realm, 300);
    let again: Vec<PortableItemId> = o.store().load_current(id).unwrap().items.iter().filter(|i| i.entry.id() == 88_001).map(|i| i.id).collect();
    assert_eq!(again, gained, "an item first seen after B0 keeps one identity");
    assert_eq!(revision(&o, id), 12);
}

#[test]
fn a_logout_while_a_checkpoint_is_pending_turns_it_into_the_final_one() {
    let Setup { mut owner, mut host, mut realm, id, session, c0 } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 8);
    h.tick(&mut realm, 100).unwrap();
    realm.play(|m| m.progression.money += 2);
    realm.logout();
    let events = h.tick(&mut realm, 101).unwrap();
    assert!(events.contains(&HostEvent::CheckpointQueued { session, sequence: 1, final_checkpoint: true }), "{events:?}");
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert!(acks[0].next_session.is_some());
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 10, "the final state includes the logout save");
}

#[test]
fn a_lost_final_acknowledgement_is_healed_and_the_realm_is_armed_again_afterwards() {
    let Setup { mut owner, mut host, mut realm, id, session, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 4);
    realm.logout();
    h.tick(&mut realm, 100).unwrap();
    let msg = h.outbox().unwrap().remove(0);
    let lost = o.handle_checkpoint(&msg.bytes).unwrap();
    let next = lost.next_session.clone().unwrap();
    assert_eq!(realm.row.as_ref().unwrap().session_id, session, "the Host never heard the answer: the realm is not re-armed yet");
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!(acks[0].outcome, AckOutcome::Duplicate);
    assert_eq!(acks[0].next_session.as_ref().unwrap().session_id, next.session_id, "the same next session is named again");
    assert_eq!(realm.row.as_ref().unwrap().session_id, next.session_id);
    assert_eq!(o.session(next.session_id).unwrap().unwrap().0, "offered");
    realm.login();
    assert!(realm.gated);
    assert_eq!(h.tick(&mut realm, 200).unwrap(), vec![HostEvent::BaselineTaken { session: next.session_id }]);
    deliver(&mut h, &mut o, &mut realm);
    assert!(matches!(h.store().host_session(next.session_id).unwrap().unwrap().state, HostState::Open));
    assert_eq!(o.session(next.session_id).unwrap().unwrap().0, "open");
    assert_eq!(revision(&o, id), o.store().character(id).unwrap().revision);
}

#[test]
fn a_realm_that_was_never_given_a_session_row_is_left_alone() {
    let Setup { mut host, mut realm, .. } = setup();
    realm.row = None;
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    realm.login();
    assert!(h.tick(&mut realm, 0).unwrap().is_empty());
    assert!(!realm.gated);
}

#[test]
fn an_autosave_between_checkpoints_is_harmless() {
    let Setup { mut owner, mut host, mut realm, id, c0, .. } = setup();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    start(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 7);
    realm.autosave();
    realm.play(|m| m.progression.money += 3);
    checkpoint(&mut h, &mut o, &mut realm, 100);
    assert_eq!(revision(&o, id), 11);
    assert_eq!(o.store().load_current(id).unwrap().progression.money, c0.progression.money + 10);
}
