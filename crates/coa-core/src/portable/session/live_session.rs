//! Phase 5 against a **real worldserver** (the core fork's `feat/portable-session-bridge` build with `PortableSession.Enable = 1`)
//! and two real disposable MySQL servers. `#[ignore]`d.
//!
//! ```text
//! COA_PORTABLE_LIVE="<bin>|13998|<pw>"  COA_PORTABLE_LIVE_B="<bin>|13997|<pw>"
//! COA_PORTABLE_LIVE_SERVER_B="<Core dir>|<RA port>|<path of the repack.json that holds the RA login>"
//! cargo test -p coa-core portable::session::live_session -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The test starts the worldserver itself (a child process it owns and stops), logs the character in through the bot module
//! (`botcmd spawnbot`, the real `HandlePlayerLoginFromDB`), and never uses `.saveall`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::db::Db;
use crate::portable::ids::ImportId;
use crate::portable::store::Store;
use crate::portable::realm::live_import::{make, number, opts, realms, reset_b, ACCOUNT, B};
use crate::portable::realm::import_character_in_session;
use crate::ra::Ra;

use super::bridge::*;
use super::live::{read_session_row, LiveBridge};
use super::protocol::*;
use super::*;

pub(super) struct Spec {
    pub(super) core: PathBuf,
    pub(super) ra_port: u16,
    pub(super) user: String,
    pub(super) password: String,
}

pub(super) fn spec() -> Option<Spec> {
    let raw = std::env::var("COA_PORTABLE_LIVE_SERVER_B").ok()?;
    let p: Vec<&str> = raw.split('|').collect();
    assert_eq!(p.len(), 3, "COA_PORTABLE_LIVE_SERVER_B = <Core dir>|<RA port>|<repack.json>");
    let text = std::fs::read_to_string(p[2]).expect("repack.json");
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    Some(Spec { core: PathBuf::from(p[0]), ra_port: p[1].parse().unwrap(), user: json["raUsername"].as_str().unwrap().to_string(), password: json["raPassword"].as_str().unwrap().to_string() })
}

/// A worldserver this test started and owns.
pub(super) struct Server {
    child: Child,
    pub(super) spec: Spec,
}

impl Server {
    pub(super) fn start(spec: Spec) -> Server {
        Self::start_with(spec, None)
    }

    /// Start with another configuration file (`None`: the realm's own).
    pub(super) fn start_with(spec: Spec, conf: Option<&std::path::Path>) -> Server {
        let conf = conf.map(|c| c.to_path_buf()).unwrap_or_else(|| spec.core.join("configs/worldserver.conf"));
        let child = Command::new(spec.core.join("worldserver.exe"))
            .args(["-c", conf.to_str().unwrap()])
            .current_dir(&spec.core)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(spec.core.join("test-out.log")).map(Stdio::from).unwrap_or_else(|_| Stdio::null()))
            .stderr(Stdio::null())
            .spawn()
            .expect("the worldserver starts");
        let mut server = Server { child, spec };
        let end = Instant::now() + Duration::from_secs(240);
        loop {
            if let Some(code) = server.child.try_wait().unwrap() {
                panic!("the worldserver exited early with {code}");
            }
            if Ra::connect_to(server.spec.ra_port, &server.spec.user, &server.spec.password).is_ok() {
                return server;
            }
            assert!(Instant::now() < end, "the worldserver did not open its console in time");
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// What the worldserver has printed since it started.
    pub(super) fn output(&self) -> String {
        let read = |name: &str| std::fs::read(self.spec.core.join(name)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        read("Logs/Server.log") + &read("test-out.log")
    }

    pub(super) fn ra(&self) -> Ra {
        Ra::connect_to(self.spec.ra_port, &self.spec.user, &self.spec.password).expect("RA")
    }

    pub(super) fn crash(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub(super) fn stop(mut self) {
        let _ = self.ra().run("server shutdown 1");
        let end = Instant::now() + Duration::from_secs(120);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < end, "the worldserver did not stop in time");
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(super) fn wait_until(what: &str, seconds: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(300));
    }
}

pub(super) fn sql(db: &Db, text: &str) -> String {
    db.query(text).unwrap_or_else(|e| panic!("{e}\n{text}"))
}


pub(super) fn name_of(db: &Db, guid: u32) -> String {
    sql(db, &format!("SELECT name FROM acore_characters.characters WHERE guid = {guid}")).trim().to_string()
}

pub(super) fn state_of(db: &Db, guid: u32) -> Option<SessionRow> {
    read_session_row(db, guid).unwrap()
}

pub(super) fn online(db: &Db, guid: u32) -> bool {
    number(db, &format!("SELECT online FROM acore_characters.characters WHERE guid = {guid}")) != 0
}

pub(super) fn deliver(host: &mut HostService, owner: &mut OwnerService, bridge: &mut LiveBridge) -> Vec<OwnerAck> {
    let mut acks = Vec::new();
    for m in host.outbox().unwrap() {
        let ack = if m.started { owner.handle_started(&m.bytes) } else { owner.handle_checkpoint(&m.bytes) }.unwrap();
        host.receive_ack(bridge, &ack).unwrap();
        acks.push(ack);
    }
    acks
}

pub(super) fn tick_until(host: &mut HostService, bridge: &mut LiveBridge, clock: &Instant, what: &str, want: impl Fn(&HostEvent) -> bool) -> Vec<HostEvent> {
    let end = Instant::now() + Duration::from_secs(60);
    let mut all = Vec::new();
    loop {
        let events = host.tick(bridge, clock.elapsed().as_secs()).unwrap();
        let hit = events.iter().any(&want);
        all.extend(events);
        if hit {
            return all;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; events so far: {all:?}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn characters_written(log: &str) -> std::collections::BTreeSet<u32> {
    let mut out = std::collections::BTreeSet::new();
    for line in log.lines() {
        if !line.contains("UPDATE characters SET") {
            continue;
        }
        if let Some(at) = line.rfind("WHERE guid") {
            let digits: String = line[at + "WHERE guid".len()..].trim_start_matches([' ', '=']).chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(g) = digits.parse() {
                out.insert(g);
            }
        }
    }
    out
}

pub(super) struct Rig {
    owner: Store,
    host: Store,
    id: crate::portable::ids::CharacterId,
    session: crate::portable::ids::SessionId,
    guid: u32,
    _dir: tempfile::TempDir,
}

pub(super) fn arrive(r: &crate::portable::realm::live_import::Realms) -> Rig {
    reset_b(&r.b);
    let dir = tempfile::tempdir().unwrap();
    let mut owner = Store::open_file(&dir.path().join("owner.db")).unwrap();
    let mut host = Store::open_file(&dir.path().join("host.db")).unwrap();
    let profile = owner.default_profile().unwrap();
    let host_profile = host.default_profile().unwrap();
    let id = make(r, &mut owner, profile, 1005);
    let offer = OwnerService::new(&mut owner).offer(id, B).unwrap();
    HostService::new(&mut host, B, HostConfig::default()).accept_offer(host_profile, &offer).unwrap();
    let imported = import_character_in_session(&r.b, &mut host, id, B, ACCOUNT, &opts(), Some(offer.session_id)).unwrap();
    HostService::new(&mut host, B, HostConfig::default()).bind(offer.session_id, imported.local_guid).unwrap();
    assert_eq!(state_of(&r.b, imported.local_guid).unwrap().state, RowState::WaitingBaseline, "the import armed the session");
    Rig { owner, host, id, session: offer.session_id, guid: imported.local_guid, _dir: dir }
}

#[test]
#[ignore]
fn a_running_worldserver_takes_its_own_baseline_and_checkpoints_one_character_without_a_logout() {
    let (Some(r), Some(spec)) = (realms(), spec()) else { return };
    let mut rig = arrive(&r);
    let guid = rig.guid;
    let name = name_of(&r.b, guid);
    let c0 = rig.owner.load_current(rig.id).unwrap();
    let log = tempfile::tempdir().unwrap().keep().join("general.log");
    let server = Server::start(spec);
    let ra = &mut server.ra();
    let clock = Instant::now();

    for other in [112u32, 113] {
        ra.run(&format!("botcmd spawnbot {other}")).unwrap();
    }
    ra.run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the core's baseline marker", 90, || state_of(&r.b, guid).is_some_and(|row| row.state == RowState::BaselineReady));
    assert!(ra.run(&format!("portable status {guid}")).unwrap().contains("gated 1"), "the core holds the player until the Host has taken B0");

    let mut host = HostService::new(&mut rig.host, B, HostConfig { checkpoint_interval_secs: 1, ..HostConfig::default() });
    let mut owner = OwnerService::new(&mut rig.owner);
    let mut bridge = LiveBridge::new(&r.b, server.ra());
    tick_until(&mut host, &mut bridge, &clock, "the baseline", |e| matches!(e, HostEvent::BaselineTaken { .. }));
    let acks = deliver(&mut host, &mut owner, &mut bridge);
    assert_eq!(acks[0].outcome, AckOutcome::Applied);
    assert!(ra.run(&format!("portable status {guid}")).unwrap().contains("gated 0"), "released after the baseline was persisted");
    assert_eq!(state_of(&r.b, guid).unwrap().state, RowState::Active);
    let id = rig.id;
    let revision = |o: &OwnerService| o.store().character(id).unwrap().revision;
    assert_eq!(revision(&owner), 1, "a baseline changes nothing canonical");
    let level = c0.progression.level;

    sql(&r.b, &format!("SET GLOBAL log_output = 'FILE'; SET GLOBAL general_log_file = '{}'; SET GLOBAL general_log = 'ON'", log.to_str().unwrap().replace('\\', "/")));
    for step in 1..=3u8 {
        ra.run(&format!("character level {name} {}", level + step)).unwrap();
        let before = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
        let events = tick_until(&mut host, &mut bridge, &clock, "a checkpoint", |e| matches!(e, HostEvent::CheckpointQueued { .. }));
        let written = std::fs::read(&log).map(|b| String::from_utf8_lossy(&b[before as usize..]).into_owned()).unwrap_or_default();
        let guids = characters_written(&written);
        assert!(guids.contains(&guid), "step {step}: the checkpoint saved the character ({guids:?}); events {events:?}");
        assert_eq!(guids.len(), 1, "step {step}: one checkpoint writes one character, not everybody: {guids:?}");
        let acks = deliver(&mut host, &mut owner, &mut bridge);
        assert!(acks.iter().all(|a| a.outcome == AckOutcome::Applied), "{acks:?}");
        let canonical = owner.store().load_current(id).unwrap();
        assert_eq!(canonical.progression.level, level + step, "step {step}");
        assert_eq!(revision(&owner), 1 + step as u64);
        assert!(online(&r.b, guid), "the character was online for every checkpoint");
    }
    sql(&r.b, "SET GLOBAL general_log = 'OFF'");

    let canonical = owner.store().load_current(id).unwrap();
    assert_eq!(canonical.progression.chosen_title, c0.progression.chosen_title);
    assert!(c0.build.spells.iter().all(|s| canonical.build.spells.contains(s)), "nothing the realm deleted at load was deleted from the canonical character");

    ra.run(&format!("botcmd despawn {guid}")).unwrap();
    wait_until("the logout save", 60, || state_of(&r.b, guid).is_some_and(|row| row.state == RowState::Ended));
    wait_until("the character to be offline", 60, || !online(&r.b, guid));
    tick_until(&mut host, &mut bridge, &clock, "the final checkpoint", |e| matches!(e, HostEvent::CheckpointQueued { final_checkpoint: true, .. }));
    let acks = deliver(&mut host, &mut owner, &mut bridge);
    let next = acks[0].next_session.clone().expect("the final acknowledgement names the next session");
    assert_eq!(owner.session(rig.session).unwrap().unwrap().0, "closed");
    let row = state_of(&r.b, guid).unwrap();
    assert_eq!((row.session_id, row.state), (next.session_id, RowState::WaitingBaseline), "the realm is armed for the next login");

    ra.run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the second baseline marker", 90, || state_of(&r.b, guid).is_some_and(|row| row.state == RowState::BaselineReady));
    tick_until(&mut host, &mut bridge, &clock, "the second baseline", |e| matches!(e, HostEvent::BaselineTaken { .. }));
    deliver(&mut host, &mut owner, &mut bridge);
    assert_eq!(owner.session(next.session_id).unwrap().unwrap().0, "open");
    assert_eq!(owner.store().load_current(id).unwrap().progression.level, level + 3);

    ra.run(&format!("character level {name} {}", level + 4)).unwrap();
    tick_until(&mut host, &mut bridge, &clock, "a checkpoint before the restart", |e| matches!(e, HostEvent::CheckpointQueued { .. }));
    deliver(&mut host, &mut owner, &mut bridge);
    drop(bridge);
    server.crash();

    let server = Server::start(self::spec().unwrap());
    let mut bridge = LiveBridge::new(&r.b, server.ra());
    sql(&r.b, "UPDATE acore_characters.characters SET online = 0");
    server.ra().run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the character to continue", 90, || online(&r.b, guid));
    wait_until("the core to register the session", 60, || server.ra().run(&format!("portable status {guid}")).unwrap().contains("SESSION"));
    let status = server.ra().run(&format!("portable status {guid}")).unwrap();
    assert!(status.contains("gated 0"), "a running session is not held again: {status}");
    assert_eq!(state_of(&r.b, guid).unwrap().session_id, next.session_id);
    server.ra().run(&format!("character level {name} {}", level + 5)).unwrap();
    tick_until(&mut host, &mut bridge, &clock, "a checkpoint after the restart", |e| matches!(e, HostEvent::CheckpointQueued { .. }));
    let acks = deliver(&mut host, &mut owner, &mut bridge);
    assert!(acks.iter().all(|a| a.outcome == AckOutcome::Applied));
    assert_eq!(owner.store().load_current(id).unwrap().progression.level, level + 5);

    server.ra().run(&format!("character level {name} {}", level + 6)).unwrap();
    drop(bridge);
    server.stop();

    let mut bridge = LiveBridge::without_console(&r.b);
    wait_until("the shutdown logout save", 30, || state_of(&r.b, guid).is_some_and(|row| row.state == RowState::Ended));
    tick_until(&mut host, &mut bridge, &clock, "the final checkpoint of a clean shutdown", |e| matches!(e, HostEvent::CheckpointQueued { final_checkpoint: true, .. }));
    let acks = deliver(&mut host, &mut owner, &mut bridge);
    let third = acks[0].next_session.clone().expect("a clean shutdown ends the session like a logout");
    assert_eq!(owner.store().load_current(id).unwrap().progression.level, level + 6);
    assert_eq!(state_of(&r.b, guid).unwrap().session_id, third.session_id);
}

#[test]
#[ignore]
fn a_character_whose_host_never_answers_is_never_released_and_the_core_disconnects_it() {
    let (Some(r), Some(spec)) = (realms(), spec()) else { return };
    let rig = arrive(&r);
    let guid = rig.guid;
    let server_log = spec.core.join("Logs/Server.log");
    let server = Server::start(spec);
    server.ra().run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the baseline marker", 90, || state_of(&r.b, guid).is_some_and(|row| row.state == RowState::BaselineReady));
    wait_until("the core to give up on the Host", 90, || std::fs::read_to_string(&server_log).is_ok_and(|log| log.contains(&format!("the baseline of character {guid} was not confirmed in time"))));
    let row = state_of(&r.b, guid).unwrap();
    assert_eq!(row.state, RowState::BaselineReady, "the session was never released");
    assert_eq!(row.checkpoint_seq, 0);
    assert!(server.ra().run(&format!("portable status {guid}")).unwrap().contains("gated 1"), "the character stays held");
    server.stop();
}

// ---- the production importer: into a RUNNING realm, through the core's PortableImportService ------------------------------------

pub(super) fn job_dir() -> Option<PathBuf> {
    std::env::var("COA_PORTABLE_LIVE_JOBDIR").ok().map(PathBuf::from)
}

fn read_back_online(r: &crate::portable::realm::live_import::Realms, store: &Store, id: crate::portable::ids::CharacterId) -> crate::portable::model::PortableCharacter {
    let guid = store.server_mappings(id).unwrap().into_iter().find(|m| m.server_id == B).unwrap().local_guid;
    crate::portable::realm::export_character_with_pets(&r.b, guid, Some(id), &store.active_item_lookup(id, B).unwrap(), &store.active_pet_lookup(id, B).unwrap()).unwrap().model
}

#[test]
#[ignore]
fn the_core_imports_characters_into_a_running_realm_like_the_offline_importer_does() {
    let (Some(r), Some(spec), Some(jobs)) = (realms(), spec(), job_dir()) else { return };
    reset_b(&r.b);
    std::fs::create_dir_all(&jobs).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_file(&dir.path().join("portable.db")).unwrap();
    let profile = store.default_profile().unwrap();
    let server = Server::start(spec);
    let mut ra = server.ra();
    let chars_before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
    let next_guid = number(&r.b, "SELECT MAX(guid) + 1 FROM acore_characters.characters");
    sql(&r.b, &format!("REPLACE INTO acore_auth.realmcharacters (realmid, acctid, numchars) VALUES (1, {ACCOUNT}, 0)"));
    let numchars_before = 0;

    let mut imported = Vec::new();
    for a_guid in [1002u32, 1004, 1005, 1001] {
        let id = make(&r, &mut store, profile, a_guid);
        let canonical = store.load_current(id).unwrap();
        let outcome = crate::portable::realm::online::import_character_online(&mut ra, &mut store, id, B, ACCOUNT, &opts(), &jobs, None).unwrap_or_else(|e| panic!("import of {a_guid}: {e}"));
        assert_eq!(outcome.local_guid as u64, next_guid + imported.len() as u64, "the core allocates from its own live generator");
        let back = read_back_online(&r, &store, id);
        assert_eq!(back, crate::portable::realm::live_import::expected_after_a_trip(&canonical, &back), "character {a_guid} is what the offline importer would have made of it");
        imported.push((id, outcome.local_guid, outcome.final_name, canonical));
    }
    assert!(std::fs::read_dir(&jobs).unwrap().next().is_none(), "job and result files are removed after each import");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), chars_before + 4);
    assert!(store.open_imports(B).unwrap().is_empty());

    // the realm itself knows them now: its name cache (lookups by name), the account's character count, its id generators
    let (id, guid, name, canonical) = &imported[2];
    let reply = ra.run(&format!("character level {name} {}", canonical.progression.level + 1)).unwrap();
    assert!(reply.contains("changed level"), "the name cache knows the new character: {reply}");
    assert_eq!(number(&r.b, &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}")), canonical.progression.level as u64 + 1);
    let numchars = number(&r.b, &format!("SELECT numchars FROM acore_auth.realmcharacters WHERE acctid = {ACCOUNT} AND realmid = 1"));
    assert_eq!(numchars, number(&r.b, &format!("SELECT COUNT(*) FROM acore_characters.characters WHERE account = {ACCOUNT}")), "the realm character count of the account was updated to what the account really has");
    assert!(numchars > numchars_before);
    let _ = id;
    ra.run(&format!("botcmd spawnbot {guid}")).unwrap();
    wait_until("the imported character to log in", 90, || online(&r.b, *guid));
    ra.run(&format!("botcmd despawn {guid}")).unwrap();
    wait_until("the logout", 60, || !online(&r.b, *guid));

    // the same job id again changes nothing (idempotent), a hostile or impossible job is refused whole
    let id = make(&r, &mut store, profile, 1003);
    let record = store.character(id).unwrap();
    let model = store.load_snapshot(id, record.revision).unwrap();
    let ticket = store.begin_import(id, B, record.revision, &crate::portable::realm::import::planned_items(&model), &crate::portable::realm::import::planned_pets(&model)).unwrap();
    let bytes = crate::portable::realm::online::job_bytes(ticket.import_id, ticket.nonce, ACCOUNT, record.revision, 10, &model, None, None, None).unwrap();
    let file = jobs.join(format!("{}.job", ticket.import_id));
    std::fs::write(&file, &bytes).unwrap();
    let before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
    assert!(ra.portable_import(ticket.import_id).is_ok());
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), before + 1);
    assert!(ra.portable_import(ticket.import_id).is_ok(), "the same job id again is answered, not repeated");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), before + 1, "no second character");
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(jobs.join(format!("{}.result", ticket.import_id)));
    store.abort_import(ticket.import_id, "test").ok();

    let mut refused = |bytes: Vec<u8>, why: &str| {
        let job = ImportId::new();
        let mut text = String::from_utf8(bytes).unwrap();
        text = text.replacen(&ticket.import_id.to_string(), &job.to_string(), 1);
        std::fs::write(jobs.join(format!("{job}.job")), text).unwrap();
        let counts = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters") + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance");
        let reply = ra.portable_import(job);
        assert!(reply.is_err(), "{why}: {reply:?}");
        assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters") + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance"), counts, "{why}: nothing was written");
        let _ = std::fs::remove_file(jobs.join(format!("{job}.job")));
        let _ = std::fs::remove_file(jobs.join(format!("{job}.result")));
    };
    let mut impossible = imported[0].3.clone();
    impossible.items[0].entry = crate::portable::ids::ContentId::new("coa", "item", 4_000_000_000).unwrap();
    refused(crate::portable::realm::online::job_bytes(ticket.import_id, [9, 9, 9, 9], ACCOUNT, 1, 10, &impossible, None, None, None).unwrap(), "an item the realm does not know");
    refused(crate::portable::realm::online::job_bytes(ticket.import_id, [9, 9, 9, 8], 99_999_999, 1, 10, &model, None, None, None).unwrap(), "an account that does not exist");
    let mut tampered = crate::portable::realm::online::job_bytes(ticket.import_id, [9, 9, 9, 7], ACCOUNT, 1, 10, &model, None, None, None).unwrap();
    let last = tampered.len() - 3;
    tampered[last] ^= 1;
    refused(tampered, "a snapshot that does not match its hash");
    refused(b"{\"job_id\":\"x\"}\nnot json".to_vec(), "garbage");
    server.stop();
}

#[test]
#[ignore]
fn the_core_reports_what_it_is_the_manager_evaluates_before_it_writes_and_the_core_refuses_a_job_format_it_does_not_read() {
    use crate::portable::capabilities::*;
    use crate::portable::extension::ExtensionRegistry;
    use crate::portable::realm::profile::{core_report, probe_capabilities};
    let (Some(r), Some(spec), Some(jobs)) = (realms(), spec(), job_dir()) else { return };
    reset_b(&r.b);
    std::fs::create_dir_all(&jobs).unwrap();
    let data = std::path::Path::new("C:/games/coa-schema-fixture-20261005/Data");
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_file(&dir.path().join("portable.db")).unwrap();
    let profile = store.default_profile().unwrap();
    let server = Server::start(spec);
    let mut ra = server.ra();

    // what the core says of itself, and the profile assembled from it
    let report = core_report(&mut ra).unwrap();
    assert_eq!(report.portable.job_formats, [2]);
    assert_eq!(report.portable.character_formats, [2]);
    assert!(report.has_feature("runtime_sessions") && report.has_feature("wardrobe"));
    assert!(report.core.commit.len() >= 7 && !report.core.branch.is_empty());
    assert!(report.catalog().contains_key("Appearances.dbc") && report.catalog().contains_key("VanityCollection.dbc"));
    let registry = ExtensionRegistry::new();
    let caps = probe_capabilities(&r.b, Some(data), Some(&mut ra), &registry).unwrap();
    assert!(caps.content.supports(Feature::RuntimeSessions) && caps.content.supports(Feature::Wardrobe) && caps.content.supports(Feature::Collections));
    assert_eq!(caps.content.online_import_job_formats, [2]);
    assert_eq!(caps.core.as_ref().unwrap().commit, report.core.commit);
    assert_eq!(probe_capabilities(&r.b, Some(data), Some(&mut ra), &registry).unwrap(), caps, "asking twice gives the same profile");
    assert!(caps.to_json().unwrap().len() < 4096, "a few hundred bytes of hashes, no id lists");
    // the catalog the Manager hashed from the data directory is the one the core loaded: a different directory is refused outright
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(elsewhere.path().join("dbc")).unwrap();
    std::fs::copy(data.join("dbc/Appearances.dbc"), elsewhere.path().join("dbc/Appearances.dbc")).unwrap();
    let mut changed = std::fs::read(elsewhere.path().join("dbc/Appearances.dbc")).unwrap();
    let last = changed.len() - 2;
    changed[last] ^= 1;
    std::fs::write(elsewhere.path().join("dbc/Appearances.dbc"), changed).unwrap();
    assert!(probe_capabilities(&r.b, Some(elsewhere.path()), Some(&mut ra), &registry).is_err(), "another Appearances.dbc than the one the core loaded");

    // an online import is evaluated before the job file is written: a profile whose core reads another job format
    let id = make(&r, &mut store, profile, 1005);
    let mut other = caps.content.clone();
    other.online_import_job_formats = vec![3];
    let other = RealmCapabilities::new(None, other).unwrap();
    let mut o = opts();
    o.capabilities = Some(std::sync::Arc::new(other));
    let before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
    let error = crate::portable::realm::online::import_character_online(&mut ra, &mut store, id, B, ACCOUNT, &o, &jobs, None).expect_err("the core reads job format 3 only according to this profile");
    assert!(matches!(error, crate::portable::PortableError::Incompatible { .. }), "{error}");
    assert!(std::fs::read_dir(&jobs).unwrap().next().is_none(), "no job file was written");
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), before);
    assert!(store.open_imports(B).unwrap().is_empty());

    // with the real profile it goes through, and the character remembers the profile it was synchronised under
    o.capabilities = Some(std::sync::Arc::new(caps.clone()));
    let imported = crate::portable::realm::online::import_character_online(&mut ra, &mut store, id, B, ACCOUNT, &o, &jobs, None).unwrap();
    assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"), before + 1);
    assert_eq!(store.mapping_profile(id, B).unwrap().as_deref(), Some(caps.content_profile_hash.as_str()));
    let _ = imported;

    // the core itself refuses a job whose format it does not read, before it looks at the body: a newer one, one without the field
    // (what Phase 5 wrote), one that is not a number
    let id = make(&r, &mut store, profile, 1003);
    let record = store.character(id).unwrap();
    let model = store.load_snapshot(id, record.revision).unwrap();
    let ticket = store.begin_import(id, B, record.revision, &crate::portable::realm::import::planned_items(&model), &crate::portable::realm::import::planned_pets(&model)).unwrap();
    let good = crate::portable::realm::online::job_bytes(ticket.import_id, ticket.nonce, ACCOUNT, record.revision, 10, &model, None, None, None).unwrap();
    let text = String::from_utf8(good).unwrap();
    assert!(text.starts_with("{\"job_format\":2,"), "{}", &text[..60]);
    let chars = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters") + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance");
    for (why, edited) in [
        ("a newer job format", text.replacen("\"job_format\":2", "\"job_format\":99", 1)),
        ("a job without a format", text.replacen("\"job_format\":2,", "", 1)),
        ("a job whose format is not a number", text.replacen("\"job_format\":2", "\"job_format\":\"2\"", 1)),
        ("the format of Phase 5", text.replacen("\"job_format\":2", "\"job_format\":1", 1)),
        ("a body that is garbage behind a format the core does not read", format!("{{\"job_format\":7}}\n{{{{ not json at all")),
    ] {
        let job = ImportId::new();
        let edited = edited.replacen(&ticket.import_id.to_string(), &job.to_string(), 1);
        std::fs::write(jobs.join(format!("{job}.job")), edited).unwrap();
        let reply = ra.portable_import(job);
        assert!(reply.as_ref().is_err_and(|e| e.to_string().contains("unsupported_job_format")), "{why}: {reply:?}");
        let result = std::fs::read_to_string(jobs.join(format!("{job}.result"))).unwrap();
        assert!(result.contains("unsupported_job_format") && result.contains("\"supported_job_formats\":[2]"), "{why}: {result}");
        assert_eq!(number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters") + number(&r.b, "SELECT COUNT(*) FROM acore_characters.item_instance"), chars, "{why}: nothing was written");
        let _ = std::fs::remove_file(jobs.join(format!("{job}.job")));
        let _ = std::fs::remove_file(jobs.join(format!("{job}.result")));
    }
    store.abort_import(ticket.import_id, "test").ok();
    server.stop();
}
