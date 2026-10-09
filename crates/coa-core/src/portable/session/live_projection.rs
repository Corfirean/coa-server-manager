//! Phase 8 against a **real worldserver** whose `MaxPlayerLevel` is below the character's level, and real disposable MySQL servers.
//! `#[ignore]`d. Environment as for `live_session`, and
//!
//! ```text
//! COA_PORTABLE_LIVE_SERVER_B        the cap-60 worldserver of realm B (its job directory is COA_PORTABLE_LIVE_JOBDIR)
//! COA_PORTABLE_LIVE_SERVER_B_CONF   another configuration of the same realm with MaxPlayerLevel = 70 (the cap changes under the character)
//! COA_PORTABLE_LIVE_CANONICAL       optional, "<Manager store dir>|<character id>": a real character to project instead of the fixture's
//! ```

use std::path::Path;
use std::time::Instant;

use super::bridge::RowState;
use crate::portable::capabilities::RealmCapabilities;
use crate::portable::extension::ExtensionRegistry;
use crate::portable::ids::{CharacterId, ImportId, ProfileId};
use crate::portable::projection::{settings, Decision, ProjectionContext};
use crate::portable::realm::live_import::{number, opts, realms, reset_b, Realms, ACCOUNT, B};
use crate::portable::realm::online::{import_character_online, job_bytes};
use crate::portable::realm::profile::probe_capabilities;
use crate::portable::realm::project::decide_with_core;
use crate::portable::store::Store;

use super::live::LiveBridge;
use super::live_session::*;
use super::protocol::*;
use super::*;

const DATA: &str = "C:/games/coa-schema-fixture-20261005/Data";

const HELMET: u64 = crate::portable::realm::live_projection::HELMET;
const LEGS: u64 = crate::portable::realm::live_projection::LEGS;

fn caps_of(r: &Realms, server: &Server) -> RealmCapabilities {
    probe_capabilities(
        &r.b,
        Some(Path::new(DATA)),
        Some(&mut server.ra()),
        &ExtensionRegistry::new(),
    )
    .unwrap()
}

fn options(caps: &RealmCapabilities) -> crate::portable::realm::ImportOptions {
    let mut o = opts();
    o.capabilities = Some(std::sync::Arc::new(caps.clone()));
    o
}

/// The canonical character the tests project: a real one when asked, else the fixture's level-80 ranger with a helmet that only a level-80
/// can wear, legwraps of the same kind in the backpack, and abilities that only exist above level 60.
fn canonical_character(r: &Realms, owner: &mut Store, profile: ProfileId) -> CharacterId {
    if let Ok(spec) = std::env::var("COA_PORTABLE_LIVE_CANONICAL") {
        let (dir, id) = spec.split_once('|').expect("<store dir>|<character id>");
        let source = Store::open(Path::new(dir)).unwrap();
        let model = source.load_current(id.parse().unwrap()).unwrap();
        return owner.create_character(profile, model, "realm-a").unwrap();
    }
    crate::portable::realm::live_projection::fixture_character(r, owner, profile)
}

fn context_of(store: &Store, id: CharacterId) -> ProjectionContext {
    store
        .projection_context(id, B)
        .unwrap()
        .expect("the character is projected on this realm")
}

/// Mails of the realm's loader about items it could not equip or store (what an item above the cap would cause); the realm's own reward mails
/// (achievements) are not counted.
fn equip_problem_mails(db: &crate::db::Db, guid: u32) -> u64 {
    number(db, &format!("SELECT COUNT(*) FROM acore_characters.mail WHERE receiver = {guid} AND subject LIKE '%problems with equipping%'"))
}

/// Rows of the realm that carry an item of one of these entries: in the character's items, or attached to a mail it received.
fn traces(db: &crate::db::Db, guid: u32, entries: &[u64]) -> u64 {
    let list = entries
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if entries.is_empty() {
        return 0;
    }
    number(db, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE itemEntry IN ({list}) AND (owner_guid = {guid} OR guid IN (SELECT item_guid FROM acore_characters.mail_items WHERE receiver = {guid}))"))
}

fn item_entries(db: &crate::db::Db, guid: u32) -> Vec<u64> {
    sql(db, &format!("SELECT itemEntry FROM acore_characters.item_instance WHERE owner_guid = {guid} ORDER BY itemEntry")).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn spells_of(db: &crate::db::Db, guid: u32) -> std::collections::BTreeSet<u32> {
    sql(
        db,
        &format!("SELECT spell FROM acore_characters.character_spell WHERE guid = {guid}"),
    )
    .lines()
    .filter_map(|l| l.trim().parse().ok())
    .collect()
}

fn pin_words(db: &crate::db::Db, guid: u32) -> String {
    sql(db, &format!("SELECT data FROM acore_characters.character_settings WHERE guid = {guid} AND source = 'coa.portable.pin'")).trim().to_string()
}

#[test]
#[ignore]
fn a_level_eighty_character_is_projected_onto_a_cap_sixty_worldserver_and_nothing_canonical_is_lost(
) {
    let (Some(r), Some(spec), Some(jobs)) = (realms(), spec(), job_dir()) else {
        return;
    };
    reset_b(&r.b);
    sql(&r.b, "DELETE FROM acore_characters.mail_items WHERE receiver > 3010; DELETE FROM acore_characters.mail WHERE receiver > 3010;");
    std::fs::create_dir_all(&jobs).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut owner = Store::open_file(&dir.path().join("owner.db")).unwrap();
    let mut host = Store::open_file(&dir.path().join("host.db")).unwrap();
    let profile = owner.default_profile().unwrap();
    let host_profile = host.default_profile().unwrap();
    let id = canonical_character(&r, &mut owner, profile);
    let c0 = owner.load_current(id).unwrap();
    assert_eq!(c0.progression.level, 80);

    let server = Server::start(spec);
    let mut ra = server.ra();
    let caps = caps_of(&r, &server);
    let progression = caps
        .progression
        .clone()
        .expect("the core reports its progression");
    assert_eq!(progression.max_player_level, 60);
    assert!(caps
        .content
        .supports(crate::portable::capabilities::Feature::LevelProjection));
    host.set_realm_profile(B, &caps, "live").unwrap();
    let o = options(&caps);

    // the core refuses a level-80 character that was not projected, and a projection prepared for another progression
    let record = owner.character(id).unwrap();
    let bad = ImportId::new();
    let unprojected = job_bytes(
        bad,
        [5, 5, 5, 5],
        ACCOUNT,
        record.revision,
        10,
        &c0,
        None,
        None,
        None,
    )
    .unwrap();
    std::fs::write(jobs.join(format!("{bad}.job")), unprojected).unwrap();
    let before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters")
        + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance");
    assert!(ra.portable_import(bad).is_err());
    assert!(std::fs::read_to_string(jobs.join(format!("{bad}.result")))
        .unwrap()
        .contains("above_level_cap"));
    assert_eq!(
        number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters")
            + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance"),
        before,
        "nothing was written"
    );
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.job")));
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.result")));

    // the core's decision for this character
    let decision = decide_with_core(&mut ra, &jobs, &c0).unwrap();
    let Decision::Projected(hold) = decision else {
        panic!("a level-80 character is projected at cap 60")
    };
    assert_eq!(
        (
            hold.max_player_level,
            hold.canonical_level,
            hold.projected_level
        ),
        (60, 80, 60)
    );
    assert_eq!(
        hold.progression_signature,
        progression.progression_signature
    );
    assert!(
        !hold.held_spells.is_empty(),
        "abilities of the character need more than level 60"
    );
    if std::env::var("COA_PORTABLE_LIVE_CANONICAL").is_err() {
        let helmet = c0.items.iter().find(|i| i.entry.id() == HELMET).unwrap().id;
        let legs = c0.items.iter().find(|i| i.entry.id() == LEGS).unwrap().id;
        assert!(
            hold.held_items.contains(&helmet),
            "the worn level-80 helmet is held"
        );
        assert!(!hold.held_items.contains(&legs), "the same kind of item in the backpack is not: the realm loads it and refuses to wear it by itself");
        for spell in crate::portable::realm::live_projection::HIGH_SPELLS {
            assert!(hold.held_spells.contains(&spell), "{spell} is held at 60");
        }
    }

    // the session starts: the Owner offers the full canonical character, the Host projects it for its realm and imports the working copy
    let offer = OwnerService::new(&mut owner).offer(id, B).unwrap();
    HostService::new(&mut host, B, HostConfig::default())
        .accept_offer(host_profile, &offer)
        .unwrap();
    let imported = import_character_online(
        &mut ra,
        &mut host,
        id,
        B,
        ACCOUNT,
        &o,
        &jobs,
        Some(offer.session_id),
    )
    .unwrap();
    HostService::new(&mut host, B, HostConfig::default())
        .bind(offer.session_id, imported.local_guid)
        .unwrap();
    let guid = imported.local_guid;
    let ctx = context_of(&host, id);
    assert_eq!(
        (
            ctx.canonical_level,
            ctx.projected_level,
            ctx.canonical_revision
        ),
        (80, 60, record.revision)
    );
    let name = name_of(&r.b, guid);

    assert_eq!(
        number(
            &r.b,
            &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}")
        ),
        60
    );
    assert_eq!(
        number(
            &r.b,
            &format!("SELECT xp FROM acore_characters.characters WHERE guid = {guid}")
        ),
        0
    );
    assert_eq!(
        item_entries(&r.b, guid).len(),
        c0.items.len()
            - hold
                .held_items
                .iter()
                .filter(|h| c0.items.iter().any(|i| i.id == **h))
                .count()
            - c0.items
                .iter()
                .filter(|i| hold.held_items.contains(&i.container.unwrap_or(i.id))
                    && i.container.is_some())
                .count(),
        "only what the realm can hold was written"
    );
    let on_realm = spells_of(&r.b, guid);
    assert!(
        hold.held_spells.iter().all(|s| !on_realm.contains(s)),
        "a held ability is absent from the realm's tables"
    );
    let held_entries: Vec<u64> = hold
        .held_items
        .iter()
        .filter_map(|h| c0.items.iter().find(|i| i.id == *h))
        .map(|i| i.entry.id())
        .collect();
    assert_eq!(
        traces(&r.b, guid, &held_entries),
        0,
        "a held item is neither in the realm's tables nor in a mail"
    );
    let real = std::env::var("COA_PORTABLE_LIVE_CANONICAL").is_ok();
    if real {
        assert_eq!(equip_problem_mails(&r.b, guid), 0);
    }
    assert_eq!(
        pin_words(&r.b, guid),
        progression
            .pin_words()
            .iter()
            .map(|w| w.to_string())
            .collect::<Vec<_>>()
            .join(" "),
        "the core stores the progression the character was prepared for"
    );
    for source in hold.settings.iter().map(|s| &s.source) {
        let value: Vec<u32> = sql(&r.b, &format!("SELECT data FROM acore_characters.character_settings WHERE guid = {guid} AND source = '{source}'")).split_whitespace().filter_map(|w| w.parse().ok()).collect();
        let held = hold.settings.iter().find(|s| &s.source == source).unwrap();
        let record = settings::Record::parse(held.kind, &value).expect("a build record");
        assert!(
            held.entries.iter().all(|e| !record.entries.contains_key(e))
                && held.buttons.iter().all(|b| !record.buttons.contains_key(b)),
            "{source}: nothing held was written"
        );
    }

    // a projected character that is not what the core would project is refused (a held ability put back)
    let mut view = crate::portable::projection::apply(&c0, &hold);
    view.build.spells.push((hold.held_spells[0], 255));
    let bad = ImportId::new();
    let pin = context_of(&host, id).pin();
    std::fs::write(
        jobs.join(format!("{bad}.job")),
        job_bytes(
            bad,
            [6, 6, 6, 6],
            ACCOUNT,
            record.revision,
            10,
            &view.normalized(),
            None,
            None,
            Some(&pin),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(ra.portable_import(bad).is_err());
    assert!(std::fs::read_to_string(jobs.join(format!("{bad}.result")))
        .unwrap()
        .contains("projection_inconsistent"));
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.job")));
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.result")));
    let mut other_profile = pin.clone();
    other_profile.progression_signature = "ab".repeat(32);
    let bad = ImportId::new();
    std::fs::write(
        jobs.join(format!("{bad}.job")),
        job_bytes(
            bad,
            [7, 7, 7, 7],
            ACCOUNT,
            record.revision,
            10,
            &crate::portable::projection::apply(&c0, &hold),
            None,
            None,
            Some(&other_profile),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(ra.portable_import(bad).is_err());
    assert!(std::fs::read_to_string(jobs.join(format!("{bad}.result")))
        .unwrap()
        .contains("progression_changed"));
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.job")));
    let _ = std::fs::remove_file(jobs.join(format!("{bad}.result")));

    // the real login: the realm loads the working copy (no mail, no removed items), the Host takes B0 under the pin
    let clock = Instant::now();
    ra.run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the baseline marker", 90, || {
        state_of(&r.b, guid).is_some_and(|row| row.state == RowState::BaselineReady)
    });
    let mut h = HostService::new(
        &mut host,
        B,
        HostConfig {
            checkpoint_interval_secs: 1,
            ..HostConfig::default()
        },
    );
    h.observe_profile(&caps, "live").unwrap();
    let mut o_service = OwnerService::new(&mut owner);
    let mut bridge = LiveBridge::new(&r.b, server.ra());
    tick_until(&mut h, &mut bridge, &clock, "the baseline", |e| {
        matches!(e, HostEvent::BaselineTaken { .. })
    });
    let acks = deliver(&mut h, &mut o_service, &mut bridge);
    assert_eq!(acks[0].outcome, AckOutcome::Applied);
    assert!(acks[0]
        .pin
        .as_ref()
        .is_some_and(|p| p.projected && p.max_player_level == 60));
    assert_eq!(
        traces(&r.b, guid, &held_entries),
        0,
        "the realm's loader found nothing above the cap to mail"
    );
    if real {
        assert_eq!(
            equip_problem_mails(&r.b, guid),
            0,
            "the realm mailed no item it could not wear: nothing above the cap was written to it"
        );
    }
    let level_on_realm = |db: &crate::db::Db| {
        number(
            db,
            &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}"),
        )
    };
    assert_eq!(level_on_realm(&r.b), 60);

    // play: the realm moves the level (down and up again), checkpoints run; the canonical character never leaves 80
    ra.run(&format!("character level {name} 59")).unwrap();
    tick_until(
        &mut h,
        &mut bridge,
        &clock,
        "a checkpoint at level 59",
        |e| matches!(e, HostEvent::CheckpointQueued { .. }),
    );
    deliver(&mut h, &mut o_service, &mut bridge);
    assert_eq!(level_on_realm(&r.b), 59);
    let c1 = o_service.store().load_current(id).unwrap();
    assert_eq!(
        (c1.progression.level, c1.progression.xp),
        (80, c0.progression.xp),
        "the realm's level is not the canonical one"
    );
    for held in &hold.held_items {
        assert!(
            c1.items.iter().any(|i| i.id == *held),
            "a held item is still canonical"
        );
    }
    for spell in &hold.held_spells {
        assert!(
            c1.build.spells.iter().any(|(s, _)| s == spell),
            "a held ability is still canonical"
        );
    }
    for s in &hold.settings {
        let value = c1
            .settings
            .get(&s.source)
            .expect("the record is still canonical");
        let record = settings::Record::parse(s.kind, value).expect("a build record");
        assert!(
            s.entries.iter().all(|e| record.entries.contains_key(e)),
            "{}: what the realm could not hold is still in the record",
            s.source
        );
    }
    ra.run(&format!("character level {name} 60")).unwrap();

    // logout: the final checkpoint; canonical level unchanged; the next session is armed on the same working copy
    ra.run(&format!("botcmd despawn {guid}")).unwrap();
    wait_until("the logout save", 60, || {
        state_of(&r.b, guid).is_some_and(|row| row.state == RowState::Ended)
    });
    wait_until("the character to be offline", 60, || !online(&r.b, guid));
    tick_until(&mut h, &mut bridge, &clock, "the final checkpoint", |e| {
        matches!(
            e,
            HostEvent::CheckpointQueued {
                final_checkpoint: true,
                ..
            }
        )
    });
    let acks = deliver(&mut h, &mut o_service, &mut bridge);
    let next = acks[0].next_session.clone().expect("the next session");
    let c2 = o_service.store().load_current(id).unwrap();
    assert_eq!(c2.progression.level, 80);
    assert_eq!(traces(&r.b, guid, &held_entries), 0);
    assert_eq!(state_of(&r.b, guid).unwrap().session_id, next.session_id);
    drop(bridge);
    let revision_before_cap_change = o_service.store().character(id).unwrap().revision;

    // ---- the cap moves under the character (60 -> 70): nobody plays it under the new rules, its session ends under the old pin ----
    server.stop();
    let Some(conf) = std::env::var("COA_PORTABLE_LIVE_SERVER_B_CONF")
        .ok()
        .map(std::path::PathBuf::from)
    else {
        return;
    };
    let server70 = Server::start_with(self::spec().unwrap(), Some(&conf));
    let caps70 = caps_of(&r, &server70);
    assert_eq!(caps70.progression.as_ref().unwrap().max_player_level, 70);
    assert_ne!(
        caps70.progression.as_ref().unwrap().progression_signature,
        progression.progression_signature
    );
    let (change, _) = h.observe_profile(&caps70, "live").unwrap();
    assert!(change.progression_changed());

    // the session is open under the old pin and the character's rows are the cap-60 working copy: the core refuses it at login
    let status_before = state_of(&r.b, guid).unwrap();
    server70
        .ra()
        .run(&format!("botcmd spawnbot {guid}"))
        .unwrap();
    wait_until("the core to refuse the login", 60, || {
        server70
            .output()
            .contains(&format!("character {guid} of session"))
            && server70
                .output()
                .contains("was prepared for another progression profile")
    });
    assert!(
        server70
            .ra()
            .run(&format!("portable status {guid}"))
            .unwrap()
            .contains("NONE"),
        "a refused character is not tracked as a session"
    );
    assert_eq!(
        state_of(&r.b, guid).unwrap().state,
        status_before.state,
        "the session row was not advanced by a login that was refused: no baseline was taken"
    );
    server70
        .ra()
        .run(&format!("botcmd despawn {guid}"))
        .unwrap();
    wait_until("the refused bot to leave", 60, || !online(&r.b, guid));

    // the Host ends the session (the realm's state under the old rules is what the checkpoint carries), and does not arm the next one
    let mut bridge = LiveBridge::new(&r.b, server70.ra());
    let events = h.tick(&mut bridge, clock.elapsed().as_secs()).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HostEvent::ProfileMoved { .. })),
        "{events:?}"
    );
    let acks = deliver(&mut h, &mut o_service, &mut bridge);
    assert!(
        acks.iter()
            .all(|a| a.outcome == AckOutcome::Applied || a.outcome == AckOutcome::Duplicate),
        "{acks:?}"
    );
    let c3 = o_service.store().load_current(id).unwrap();
    assert_eq!(c3.progression.level, 80);
    assert!(
        c3.build.spells.len() >= c2.build.spells.len(),
        "ending a session loses nothing"
    );
    let _ = revision_before_cap_change;

    // the Manager asks the running core for the decision at the new cap, stops the realm, and updates the working copy in place
    let decision70 = {
        let canonical = o_service.store().load_current(id).unwrap();
        let mut ra70 = server70.ra();
        decide_with_core(&mut ra70, &jobs, &canonical).unwrap()
    };
    drop(bridge);
    server70.stop();
    let Decision::Projected(hold70) = decision70 else {
        panic!("level 80 is above 70")
    };
    assert_eq!(hold70.max_player_level, 70);
    assert!(
        hold70.held_spells.len() <= hold.held_spells.len(),
        "a higher cap holds back no more than a lower one"
    );
    assert!(hold70.held_items.len() <= hold.held_items.len());
    let mut o70 = options(&caps70);
    o70.projection = Some(crate::portable::projection::Oracle(std::sync::Arc::new(
        crate::portable::projection::SuppliedDecision(hold70.clone()),
    )));
    let outcome = crate::portable::realm::reproject_session(&r.b, &mut host, id, B, &o70)
        .unwrap()
        .expect("a session waited for its working copy");
    assert!(outcome.updated);
    assert_eq!(
        number(
            &r.b,
            &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}")
        ),
        70
    );
    let on_realm = spells_of(&r.b, guid);
    for spell in &hold.held_spells {
        assert_eq!(
            on_realm.contains(spell),
            !hold70.held_spells.contains(spell),
            "{spell}: restored exactly when the new cap allows it"
        );
    }
    assert_eq!(
        traces(
            &r.b,
            guid,
            &held_entries
                .iter()
                .copied()
                .filter(|e| hold70
                    .held_items
                    .iter()
                    .any(|h| c0.items.iter().any(|i| i.id == *h && i.entry.id() == *e)))
                .collect::<Vec<_>>()
        ),
        0,
        "what the new cap still holds is still not on the realm"
    );
    assert_eq!(
        pin_words(&r.b, guid),
        caps70
            .progression
            .as_ref()
            .unwrap()
            .pin_words()
            .iter()
            .map(|w| w.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let ctx = context_of(&host, id);
    assert_eq!((ctx.projected_level, ctx.canonical_level), (70, 80));
    assert!(
        host.host_live_sessions(B)
            .unwrap()
            .iter()
            .all(|s| !s.reproject),
        "the working copy was projected again, the session may run"
    );
}
