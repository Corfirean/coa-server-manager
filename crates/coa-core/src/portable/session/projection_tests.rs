//! Runtime sessions of a character above the realm's level cap (Phase 8): the Owner and the Host as two stores, a simulated realm at
//! cap 60 between them. The core's decision is supplied (the real core is exercised by the live suite).

use super::fake::*;
use super::protocol::*;
use super::*;
use crate::portable::capabilities::*;
use crate::portable::ids::{CharacterId, SessionId};
use crate::portable::model::{PortableCharacter, Ruleset};
use crate::portable::projection::testing::*;
use crate::portable::projection::{settings, SuppliedDecision};
use crate::portable::realm::project::{plan_with, remember};
use crate::portable::store::Store;

const SERVER: &str = "realm-60";
const GUID: u32 = 777;

struct World {
    owner: Store,
    host: Store,
    realm: FakeRealm,
    id: CharacterId,
    session: SessionId,
    c0: PortableCharacter,
}

fn caps(cap: u32, signature: &str) -> RealmCapabilities {
    let content = ContentProfile {
        ruleset: Ruleset::Coa,
        character_formats: ContentProfile::manager_formats(),
        online_import_job_formats: vec![2],
        session_protocol: PROTOCOL_VERSION,
        collection_protocol: PROTOCOL_VERSION,
        features: [Feature::RuntimeSessions, Feature::LevelProjection]
            .into_iter()
            .collect(),
        collection_kinds: Default::default(),
        extensions: vec![],
        client_catalog: Default::default(),
    };
    RealmCapabilities::build(
        None,
        content,
        Some(crate::portable::capabilities::Progression {
            max_player_level: cap,
            projection_protocol: 1,
            projection_policy_version: 1,
            progression_signature: signature.into(),
            scaling_enabled: true,
        }),
    )
    .unwrap()
}

/// What the realm's core rewrites at its first load: the build record in its own order and padding.
fn rewrites_the_record(r: &mut FakeRealm) {
    if let Some(v) = r.memory.settings.get("core.ascension_slot.0").cloned() {
        let mut rec =
            settings::Record::parse(settings::SettingKind::Slot, &v).expect("a build record");
        rec.entry_order.reverse();
        let mut out = rec.write();
        out.extend([0, 0, 0]);
        r.memory
            .settings
            .insert("core.ascension_slot.0".into(), out);
    }
}

fn world() -> World {
    let mut owner = Store::open_in_memory().unwrap();
    let mut host = Store::open_in_memory().unwrap();
    let profile = owner.default_profile().unwrap();
    let canonical = canonical();
    let id = owner
        .create_character(profile, canonical.clone(), "realm-80")
        .unwrap();
    let c0 = owner.load_current(id).unwrap();
    let offer = OwnerService::new(&mut owner).offer(id, SERVER).unwrap();
    let caps = caps(60, SIGNATURE);
    host.set_realm_profile(SERVER, &caps, "live").unwrap();
    let hold = hold_for(&c0);
    let plan = plan_with(
        &c0,
        offer.canonical_revision,
        Some(&caps),
        Some(&SuppliedDecision(hold)),
    )
    .unwrap();
    let host_profile = host.default_profile().unwrap();
    let mut realm = FakeRealm::new(GUID);
    realm.normalize = rewrites_the_record;
    {
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        h.accept_offer(host_profile, &offer).unwrap();
        realm
            .import_model(
                &mut host,
                id,
                SERVER,
                offer.canonical_revision,
                offer.session_id,
                plan.view.clone(),
            )
            .unwrap();
        remember(&mut host, id, SERVER, &plan).unwrap();
        HostService::new(&mut host, SERVER, HostConfig::default())
            .bind(offer.session_id, GUID)
            .unwrap();
    }
    World {
        owner,
        host,
        realm,
        id,
        session: offer.session_id,
        c0,
    }
}

fn deliver(
    host: &mut HostService,
    owner: &mut OwnerService,
    realm: &mut FakeRealm,
) -> Vec<OwnerAck> {
    let mut acks = Vec::new();
    for m in host.outbox().unwrap() {
        let ack = if m.started {
            owner.handle_started(&m.bytes)
        } else {
            owner.handle_checkpoint(&m.bytes)
        }
        .unwrap();
        host.receive_ack(realm, &ack).unwrap();
        acks.push(ack);
    }
    acks
}

fn checkpoint(host: &mut HostService, owner: &mut OwnerService, realm: &mut FakeRealm, now: u64) {
    host.tick(realm, now).unwrap();
    host.tick(realm, now + 1).unwrap();
    deliver(host, owner, realm);
}

#[test]
fn a_session_of_a_level_eighty_on_a_cap_sixty_realm_keeps_the_canonical_character_whole() {
    let World {
        mut owner,
        mut host,
        mut realm,
        id,
        session,
        c0,
        ..
    } = world();
    assert_eq!(
        realm.db.progression.level, 60,
        "the realm was given the working copy"
    );
    assert!(
        realm
            .db
            .items
            .iter()
            .all(|i| !hold_for(&c0).held_items.contains(&i.id)),
        "a held item never reaches the realm"
    );
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);

    realm.login();
    h.tick(&mut realm, 0).unwrap();
    let started: PortableSessionStarted = from_json(&h.outbox().unwrap()[0].bytes).unwrap();
    let progression = started
        .progression
        .clone()
        .expect("the session carries its progression");
    assert!(progression.pin.projected && progression.projection.is_some());
    assert_eq!(
        (
            progression.pin.max_player_level,
            progression.projection.as_ref().unwrap().canonical_level
        ),
        (60, 80)
    );
    let acks = deliver(&mut h, &mut o, &mut realm);
    assert_eq!(acks[0].outcome, AckOutcome::Applied);
    assert_eq!(acks[0].pin, Some(progression.pin.clone()));

    realm.play(|m| {
        m.progression.money += 9_000;
        m.progression.xp = 4_321;
        m.reputation[0].standing += 100;
        m.quests.rewarded.push(31_337);
        m.settings
            .insert("core.ascension_build.61".into(), vec![2, 1001, 4001]);
    });
    realm.give_item(new_internal_item(9, 40, 88_001));
    checkpoint(&mut h, &mut o, &mut realm, 100);
    let c1 = o.store().load_current(id).unwrap();
    assert_eq!(
        (c1.progression.level, c1.progression.xp),
        (80, c0.progression.xp),
        "the realm's level and xp are not the canonical character's"
    );
    assert_eq!(c1.progression.money, c0.progression.money + 9_000);
    assert!(
        c1.quests.rewarded.contains(&31_337) && c1.items.iter().any(|i| i.entry.id() == 88_001)
    );
    let hold = hold_for(&c0);
    for held in &hold.held_items {
        assert!(
            c1.items.iter().any(|i| i.id == *held),
            "the held item is still canonical"
        );
    }
    for spell in &hold.held_spells {
        assert!(
            c1.build.spells.iter().any(|(s, _)| s == spell),
            "the held ability is still canonical"
        );
    }
    let slot = settings::Record::parse(
        settings::SettingKind::Slot,
        &c1.settings["core.ascension_slot.0"],
    )
    .unwrap();
    assert_eq!(
        slot.entries.keys().copied().collect::<Vec<_>>(),
        vec![100, 200, 300],
        "the realm's rewrite of the build record does not erase what it could not hold"
    );
    let build = settings::Record::parse(
        settings::SettingKind::Build,
        &c1.settings["core.ascension_build.61"],
    )
    .unwrap();
    assert_eq!(
        build.entries.keys().copied().collect::<Vec<_>>(),
        vec![100, 200, 300, 400],
        "a pick made at the cap arrives"
    );

    // the checkpoint repeated, and more of the same play: nothing is counted twice, the held state is still there
    checkpoint(&mut h, &mut o, &mut realm, 200);
    assert_eq!(o.store().load_current(id).unwrap(), c1);
    realm.play(|m| m.progression.money += 1);
    checkpoint(&mut h, &mut o, &mut realm, 300);
    assert_eq!(
        o.store().load_current(id).unwrap().progression.money,
        c1.progression.money + 1
    );

    // logout: the final checkpoint, the next session follows, and the projection is still the realm's
    realm.logout();
    h.tick(&mut realm, 400).unwrap();
    let acks = deliver(&mut h, &mut o, &mut realm);
    let last = acks.last().unwrap();
    assert!(
        last.next_session.is_some() && last.canonical.is_some(),
        "{acks:?}"
    );
    let c2 = o.store().load_current(id).unwrap();
    assert_eq!(c2.progression.level, 80);
    assert_eq!(c2.items.len(), c1.items.len());
    let ctx = h
        .store()
        .projection_context(id, SERVER)
        .unwrap()
        .expect("the projection is still remembered");
    assert_eq!(
        ctx.canonical_revision,
        o.store().character(id).unwrap().revision,
        "the context follows the acknowledged revision"
    );
    assert_eq!(o.session(session).unwrap().unwrap().0, "closed");
}

#[test]
fn a_checkpoint_under_another_pin_is_refused_and_changes_nothing() {
    let World {
        mut owner,
        mut host,
        mut realm,
        id,
        session,
        ..
    } = world();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    realm.login();
    h.tick(&mut realm, 0).unwrap();
    deliver(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 5);
    h.tick(&mut realm, 50).unwrap();
    h.tick(&mut realm, 51).unwrap();
    let pending = h.outbox().unwrap();
    assert_eq!(pending.len(), 1);
    let before = o.store().load_current(id).unwrap();

    let mut forged: PortableCheckpoint = from_json(&pending[0].bytes).unwrap();
    let mut other = forged.pin.clone().unwrap();
    other.progression_signature = "ee".repeat(32);
    forged.pin = Some(other);
    let ack = o.handle_checkpoint(&to_json(&forged).unwrap()).unwrap();
    assert!(
        matches!(ack.outcome, AckOutcome::Rejected(ref why) if why.contains("progression")),
        "{ack:?}"
    );
    forged.pin = None;
    let ack = o.handle_checkpoint(&to_json(&forged).unwrap()).unwrap();
    assert!(
        matches!(ack.outcome, AckOutcome::Rejected(_)),
        "a checkpoint with no pin under a pinned session"
    );
    assert_eq!(
        o.store().load_current(id).unwrap(),
        before,
        "nothing was merged"
    );
    assert_eq!(o.session(session).unwrap().unwrap().1, 0);

    // the genuine one is accepted
    let ack = o.handle_checkpoint(&pending[0].bytes).unwrap();
    assert_eq!(ack.outcome, AckOutcome::Applied);
}

#[test]
fn a_session_started_without_its_projection_or_with_a_wrong_one_is_not_opened() {
    let World {
        mut owner,
        mut host,
        mut realm,
        id,
        ..
    } = world();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    realm.login();
    h.tick(&mut realm, 0).unwrap();
    let genuine = h.outbox().unwrap().remove(0).bytes;

    let mut no_context: PortableSessionStarted = from_json(&genuine).unwrap();
    no_context.progression.as_mut().unwrap().projection = None;
    assert!(
        o.handle_started(&to_json(&no_context).unwrap())
            .unwrap()
            .outcome
            != AckOutcome::Applied,
        "a projected pin needs its projection"
    );

    let mut wrong_level: PortableSessionStarted = from_json(&genuine).unwrap();
    {
        let p = wrong_level.progression.as_mut().unwrap();
        let ctx = p.projection.as_mut().unwrap();
        ctx.canonical_level = 70;
        p.pin = ctx.pin();
    }
    let ack = o.handle_started(&to_json(&wrong_level).unwrap()).unwrap();
    assert!(
        matches!(ack.outcome, AckOutcome::Rejected(ref why) if why.contains("canonical level")),
        "{ack:?}"
    );
    assert_eq!(
        o.session(h.store().host_live_sessions(SERVER).unwrap()[0].session_id)
            .unwrap()
            .unwrap()
            .0,
        "offered"
    );

    // an older protocol is not understood at all
    let old = String::from_utf8(genuine.clone())
        .unwrap()
        .replace("\"protocol_version\":2", "\"protocol_version\":1");
    assert!(o.handle_started(old.as_bytes()).is_err());
    assert_eq!(
        o.handle_started(&genuine).unwrap().outcome,
        AckOutcome::Applied
    );
    assert_eq!(o.store().character(id).unwrap().revision, 1);
}

#[test]
fn a_realm_that_changes_its_cap_under_a_session_ends_it_under_the_old_pin_and_does_not_arm_the_next(
) {
    let World {
        mut owner,
        mut host,
        mut realm,
        id,
        session,
        c0,
        ..
    } = world();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    realm.login();
    h.tick(&mut realm, 0).unwrap();
    deliver(&mut h, &mut o, &mut realm);
    realm.play(|m| m.progression.money += 700);
    checkpoint(&mut h, &mut o, &mut realm, 100);
    realm.play(|m| m.progression.money += 300);

    // the realm restarts with another cap (a different progression signature); the player is gone
    realm.crash();
    let moved = caps(70, &"cd".repeat(32));
    let (change, _) = h.observe_profile(&moved, "live").unwrap();
    assert!(
        change.progression_changed() && !change.changed(),
        "only the progression moved, not the content"
    );
    let events = h.tick(&mut realm, 500).unwrap();
    assert!(
        events.contains(&HostEvent::ProfileMoved { session }),
        "{events:?}"
    );
    assert!(
        events.contains(&HostEvent::CheckpointQueued {
            session,
            sequence: 2,
            final_checkpoint: true
        }),
        "{events:?}"
    );
    let started_pin = h
        .store()
        .host_session(session)
        .unwrap()
        .unwrap()
        .pin
        .unwrap();
    assert_eq!(
        started_pin.max_player_level, 60,
        "the session ends under the pin it started under"
    );

    let acks = deliver(&mut h, &mut o, &mut realm);
    let ack = acks.last().unwrap();
    assert_eq!(ack.outcome, AckOutcome::Applied);
    assert_eq!(ack.pin.as_ref().unwrap().max_player_level, 60);
    let c1 = o.store().load_current(id).unwrap();
    assert_eq!(
        c1.progression.level, 80,
        "merged as a projection of the old cap, never as the new one"
    );
    assert_eq!(
        c1.progression.money,
        c0.progression.money + 700,
        "what the realm had saved (the unsaved 300 died with the crash)"
    );
    let next = ack.next_session.as_ref().unwrap().session_id;
    let next_row = h.store().host_session(next).unwrap().unwrap();
    assert!(
        next_row.reproject,
        "the next session waits for the working copy to be projected again"
    );
    assert_eq!(
        h.rearm_pending(&mut realm).unwrap(),
        0,
        "nothing is armed on the old working copy"
    );
    assert_eq!(
        realm.row.as_ref().unwrap().session_id,
        session,
        "the realm was not armed for the next session"
    );

    // later ticks do not end the session twice
    let again = h.tick(&mut realm, 600).unwrap();
    assert!(
        !again
            .iter()
            .any(|e| matches!(e, HostEvent::CheckpointQueued { .. })),
        "{again:?}"
    );
}

#[test]
fn a_character_online_under_the_old_profile_ends_at_its_logout() {
    let World {
        mut owner,
        mut host,
        mut realm,
        session,
        ..
    } = world();
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    realm.login();
    h.tick(&mut realm, 0).unwrap();
    deliver(&mut h, &mut o, &mut realm);
    h.observe_profile(&caps(70, &"cd".repeat(32)), "live")
        .unwrap();
    let events = h.tick(&mut realm, 10).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HostEvent::ProfileWaiting { session: s, .. } if *s == session)),
        "{events:?}"
    );
    assert!(
        h.outbox().unwrap().is_empty(),
        "nothing is sent while the character is online"
    );
}

#[test]
fn what_a_character_is_bound_to_is_remembered_per_realm_and_forgotten_on_request() {
    let World {
        mut host, id, c0, ..
    } = world();
    let pin = host
        .character_pin(id, SERVER)
        .unwrap()
        .expect("the import bound the character");
    assert!(pin.projected && pin.max_player_level == 60 && pin.progression_signature == SIGNATURE);
    let ctx = host.projection_context(id, SERVER).unwrap().unwrap();
    assert_eq!(
        (
            ctx.canonical_level,
            ctx.projected_level,
            ctx.canonical_revision
        ),
        (80, 60, 1)
    );
    assert!(
        host.projection_context(id, "another-realm")
            .unwrap()
            .is_none()
            && host.character_pin(id, "another-realm").unwrap().is_none()
    );

    host.advance_projection_revision(id, SERVER, 7).unwrap();
    assert_eq!(
        host.projection_context(id, SERVER)
            .unwrap()
            .unwrap()
            .canonical_revision,
        7
    );

    assert!(host.clear_projection_context(id, SERVER).unwrap());
    assert!(host.projection_context(id, SERVER).unwrap().is_none());
    host.set_mapping_pin(id, SERVER, None).unwrap();
    assert!(host.character_pin(id, SERVER).unwrap().is_none());
    assert!(
        host.set_mapping_pin(id, "another-realm", None).is_err(),
        "a character that is not on a realm has no pin there"
    );
    let mut bad = pin.clone();
    bad.progression_signature = "zz".into();
    assert!(host.set_mapping_pin(id, SERVER, Some(&bad)).is_err());
    let _ = c0;
}
