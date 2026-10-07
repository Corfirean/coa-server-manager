//! Phase 9 against a real cap-60 worldserver and real disposable databases: the whole of portable play through the service the application
//! uses, with a bot standing in for the game client. `#[ignore]`d; environment as for `session::live_projection`.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::portable::realm::live_import::{number, realms, reset_b, tools};
use crate::portable::session::live_session::{job_dir, name_of, online, spec, sql, wait_until, Server};
use crate::portable::store::HostState;

use super::access::{Descriptor, Kind, Redacted};
use super::*;

const REALM: &str = "scratch-cap60";
const DATA: &str = "C:/games/coa-schema-fixture-20261005/Data";

fn descriptor(dir: &Path) -> std::path::PathBuf {
    let (bin, port, pw) = tools("COA_PORTABLE_LIVE_B").expect("COA_PORTABLE_LIVE_B");
    let spec = spec().expect("COA_PORTABLE_LIVE_SERVER_B");
    let jobs = job_dir().expect("COA_PORTABLE_LIVE_JOBDIR");
    let d = Descriptor {
        schema: 1,
        id: REALM.into(),
        name: "Scratch realm (cap 60)".into(),
        address: "127.0.0.1".into(),
        realm_name: None,
        mysql_bin: bin.to_string_lossy().into(),
        db_port: port,
        db_user: "root".into(),
        db_password: Redacted::new(pw),
        ra_port: spec.ra_port,
        ra_user: spec.user.clone(),
        ra_password: Redacted::new(spec.password.clone()),
        job_dir: jobs.to_string_lossy().into(),
        data_dir: DATA.into(),
        game_server_users: vec!["acore".into()],
        server_root: None,
    };
    let file = dir.join("scratch-cap60.json");
    std::fs::write(&file, serde_json::to_vec(&d).unwrap()).unwrap();
    file
}

fn pump(svc: &mut PortableService, seconds: u64, what: &str, mut done: impl FnMut(&PortableService) -> bool) {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        svc.tick();
        if done(svc) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state: {:?}", svc.state().characters.iter().map(|c| (&c.name, c.status, c.copies.iter().map(|x| x.status).collect::<Vec<_>>())).collect::<Vec<_>>());
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn status(svc: &PortableService) -> PlayStatus {
    svc.state().characters[0].copies.first().map(|c| c.status).unwrap_or(PlayStatus::Ready)
}

#[test]
#[ignore]
fn one_click_play_of_a_level_eighty_on_a_cap_sixty_realm_survives_navigation_a_restart_and_the_logout() {
    let (Some(r), Some(sp), Some(jobs)) = (realms(), spec(), job_dir()) else { return };
    reset_b(&r.b);
    sql(&r.b, "DELETE FROM acore_characters.mail_items WHERE receiver > 3010; DELETE FROM acore_characters.mail WHERE receiver > 3010;");
    std::fs::create_dir_all(&jobs).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = descriptor(dir.path());
    let mut svc = PortableService::open(&dir.path().join("portable"), vec![]).unwrap();

    // a fresh player: no server of their own, one prepared realm, no characters yet
    assert!(svc.state().characters.is_empty() && svc.state().realms.is_empty());
    let realm = svc.add_prepared_realm(&file).unwrap();
    assert_eq!((realm.id.as_str(), realm.kind), (REALM, Kind::Prepared));
    assert!(!realm.online, "the realm is not running yet");

    // a level-80 character is already in the player's hands (made portable elsewhere)
    let profile = svc.owner_profile();
    let id = crate::portable::realm::live_projection::fixture_character(&r, svc.owner_mut(), profile);
    let name = svc.owner().character(id).unwrap().name;
    assert_eq!(svc.state().characters[0].level, 80);
    assert!(svc.state().characters[0].copies.is_empty(), "no realm copy yet");

    // the realm is not running: Play is refused before anything is written, in words
    let id_text = id.to_string();
    assert_eq!(svc.preflight(&id_text, REALM).unwrap().step, Step::Offline);
    assert_eq!(svc.play(&id_text, REALM, Some("x")).unwrap_err().code, "realm_offline");
    assert!(svc.host().server_mappings(id).unwrap().is_empty());

    let server = Server::start(sp);
    svc.tick();
    svc.tick();
    sql(&r.b, "REPLACE INTO acore_auth.realmcharacters (realmid, acctid, numchars) VALUES (1, 7001, 0)");
    let account = name_of_account(&r.b, 7001);

    // the compatibility check says what will happen, before anything is written
    let pre = svc.preflight(&id_text, REALM).unwrap();
    assert_eq!((pre.verdict, pre.step, pre.projection, pre.needs_account), (Verdict::Degraded, Step::Prepare, Some((80, 60)), true));
    assert!(pre.notes.iter().any(|n| n.code == "projection" && n.params["from"] == "80" && n.params["to"] == "60"), "{:?}", pre.notes);
    assert!(svc.host().server_mappings(id).unwrap().is_empty(), "looking changes nothing");
    assert_eq!(svc.play(&id_text, REALM, None).unwrap_err().code, "needs_account");
    assert_eq!(svc.play(&id_text, REALM, Some("NOSUCHACCOUNT")).unwrap_err().code, "account_missing");

    // one click
    let played = svc.play(&id_text, REALM, Some(&account)).unwrap();
    assert_eq!((played.status, played.projection), (PlayStatus::CompatWarning, Some((80, 60))));
    let guid = svc.host().server_mappings(id).unwrap().into_iter().find(|m| m.server_id == REALM).unwrap().local_guid;
    assert_eq!(number(&r.b, &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}")), 60);
    assert_eq!(svc.state().characters[0].copies[0].projected_level, Some(60));
    assert_eq!(status(&svc), PlayStatus::WaitingLogin);
    assert!(svc.state().characters[0].copies[0].degraded, "a degraded realm plays, with its warning");
    let again = svc.preflight(&id_text, REALM).unwrap();
    assert_eq!(again.step, Step::Resume, "a prepared session is resumed, not made twice");

    // the game's login (a bot stands in for the client): the baseline is taken without anybody doing anything
    let ra = &mut server.ra();
    ra.run(&format!("botcmd spawnbot {guid}")).unwrap();
    pump(&mut svc, 90, "the player to be playing", |s| status(s) == PlayStatus::Playing);
    assert_eq!(svc.state().characters[0].active_realm.as_deref(), Some("Scratch realm (cap 60)"));
    assert_eq!(svc.state().runtime.sessions_open, 1);

    // the interface navigates elsewhere and back: nothing depends on it, ticks go on, a checkpoint is made
    let char_name = name_of(&r.b, guid);
    ra.run(&format!("character level {char_name} 59")).unwrap();
    let before = svc.owner().character(id).unwrap().revision;
    pump(&mut svc, 90, "a checkpoint", |s| s.owner().character(id).unwrap().revision > before || s.host().host_live_sessions(REALM).unwrap().iter().any(|x| x.acked_sequence >= 1));
    assert_eq!(svc.owner().load_current(id).unwrap().progression.level, 80, "the realm's level is not the canonical one");

    // the Manager restarts in the middle of the session
    let sessions_before: Vec<_> = svc.host().host_live_sessions(REALM).unwrap().iter().map(|s| (s.session_id, s.state)).collect();
    assert_eq!(sessions_before[0].1, HostState::Open);
    drop(svc);
    let mut svc = PortableService::open(&dir.path().join("portable"), vec![]).unwrap();
    assert_eq!(svc.state().characters.len(), 1, "the character is still there");
    svc.tick();
    pump(&mut svc, 60, "the session to be recognised again", |s| status(s) == PlayStatus::Playing);
    let sessions_after: Vec<_> = svc.host().host_live_sessions(REALM).unwrap().iter().map(|s| (s.session_id, s.state)).collect();
    assert_eq!(sessions_before, sessions_after, "the same session continues");
    let acked = svc.host().host_live_sessions(REALM).unwrap()[0].acked_sequence;
    pump(&mut svc, 90, "a checkpoint after the restart", |s| s.host().host_live_sessions(REALM).unwrap().iter().any(|x| x.acked_sequence > acked));

    // logout: the final sync, and the character is ready for the next login
    ra.run(&format!("botcmd despawn {guid}")).unwrap();
    wait_until("the character to leave", 60, || !online(&r.b, guid));
    pump(&mut svc, 90, "the final sync", |s| s.saved(&id_text, REALM).is_some());
    let (revision, _) = svc.saved(&id_text, REALM).unwrap();
    assert_eq!(revision, svc.owner().character(id).unwrap().revision, "the interface can say which revision the logout produced");
    pump(&mut svc, 30, "the next session to wait for a login", |s| status(s) == PlayStatus::WaitingLogin);
    let c = svc.owner().load_current(id).unwrap();
    assert_eq!(c.progression.level, 80);
    let history = svc.history(&id_text).unwrap();
    assert!(history.len() >= 2 && history[0].revision == svc.owner().character(id).unwrap().revision);
    let diag = serde_json::to_string(&svc.diagnostics()).unwrap();
    assert!(!diag.contains(&descriptor_secret()) && diag.contains(&id_text) && diag.contains("progression_pin"), "{diag}");

    // a realm that cannot take the character is refused before anything is changed
    let mut other = svc.owner().load_current(id).unwrap();
    other.character_id = crate::portable::CharacterId::new();
    other.ruleset = crate::portable::model::Ruleset::Wildcard;
    other.content_namespace = "wildcard".into();
    let wild = svc.owner_mut().create_character(profile, other, "elsewhere").unwrap();
    let pre = svc.preflight(&wild.to_string(), REALM).unwrap();
    assert_eq!((pre.verdict, pre.step), (Verdict::Incompatible, Step::Blocked));
    let err = svc.play(&wild.to_string(), REALM, Some(&account)).unwrap_err();
    assert_eq!(err.code, "incompatible");
    assert!(svc.host().server_mappings(wild).unwrap().is_empty(), "nothing was written for the character a realm cannot take");
    let _ = (name, server);
}

#[test]
#[ignore]
fn a_character_made_portable_on_a_running_realm_is_armed_on_it_without_a_restart() {
    let (Some(r), Some(sp), Some(jobs)) = (realms(), spec(), job_dir()) else { return };
    reset_b(&r.b);
    std::fs::create_dir_all(&jobs).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = descriptor(dir.path());
    let mut svc = PortableService::open(&dir.path().join("portable"), vec![]).unwrap();
    svc.add_prepared_realm(&file).unwrap();
    let server = Server::start(sp);
    svc.tick();
    svc.tick();

    let local = svc.local_characters(REALM).unwrap();
    let pick = local.iter().find(|c| c.eligible).expect("an eligible character on the realm");
    let made = svc.make_portable(REALM, pick.token).unwrap();
    let pre = svc.preflight(&made.id, REALM).unwrap();
    assert_eq!(pre.step, Step::Arm, "the realm's own character is current: nothing to update, nothing to restart; {:?}", pre.notes);
    svc.play(&made.id, REALM, None).unwrap();
    assert_eq!(status(&svc), PlayStatus::WaitingLogin);
    drop(server);
}

#[test]
#[ignore]
fn a_manager_restart_with_nothing_to_report_makes_no_revision_and_a_real_change_makes_exactly_one() {
    let (Some(r), Some(sp), Some(jobs)) = (realms(), spec(), job_dir()) else { return };
    reset_b(&r.b);
    sql(&r.b, "DELETE FROM acore_characters.mail_items WHERE receiver > 3010; DELETE FROM acore_characters.mail WHERE receiver > 3010;");
    std::fs::create_dir_all(&jobs).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = descriptor(dir.path());
    let quick = crate::portable::session::HostConfig { checkpoint_interval_secs: 4, collection_interval_secs: 300 };
    let open = |dir: &Path| {
        let mut svc = PortableService::open(&dir.join("portable"), vec![]).unwrap();
        svc.set_host_config(quick.clone());
        svc
    };
    let mut svc = open(dir.path());
    svc.add_prepared_realm(&file).unwrap();
    let profile = svc.owner_profile();
    let id = crate::portable::realm::live_projection::fixture_character(&r, svc.owner_mut(), profile);
    let id_text = id.to_string();
    let server = Server::start(sp);
    svc.tick();
    svc.tick();
    sql(&r.b, "REPLACE INTO acore_auth.realmcharacters (realmid, acctid, numchars) VALUES (1, 7001, 0)");
    let account = name_of_account(&r.b, 7001);
    svc.play(&id_text, REALM, Some(&account)).unwrap();
    let guid = svc.host().server_mappings(id).unwrap().into_iter().find(|m| m.server_id == REALM).unwrap().local_guid;
    server.ra().run(&format!("botcmd spawnbot {guid}")).unwrap();
    pump(&mut svc, 90, "playing", |s| status(s) == PlayStatus::Playing);
    server.ra().run(&format!("botcmd suspend {guid}")).unwrap();

    let acked = |s: &PortableService| s.host().host_live_sessions(REALM).unwrap().first().map(|x| x.acked_sequence).unwrap_or(0);
    let revision = |s: &PortableService| s.owner().character(id).unwrap().revision;

    // whatever the realm does by itself at the first login arrives with the first checkpoints; then the character is quiet
    pump(&mut svc, 90, "the first checkpoints", |s| acked(s) >= 1);
    let from = acked(&svc);
    pump(&mut svc, 90, "two quiet checkpoints", |s| acked(s) >= from + 2);
    let settled = svc.owner().load_current(id).unwrap();
    let n = revision(&svc);

    // the Manager restarts; the new Host has no memory of its last checkpoint and checkpoints at once
    drop(svc);
    let mut svc = open(dir.path());
    let before = acked(&svc);
    pump(&mut svc, 60, "the Host to be recognised again", |s| status(s) == PlayStatus::Playing);
    pump(&mut svc, 60, "the immediate checkpoint after the restart", |s| acked(s) > before);
    assert_eq!(revision(&svc), n, "a restart with nothing to report is not a new revision");
    let after = svc.owner().load_current(id).unwrap();
    assert_eq!(after, settled, "field by field: {:?}", super::delta_probe::diff(&settled, &after));
    assert_eq!(crate::portable::snapshot::content_hash(&after).unwrap(), crate::portable::snapshot::content_hash(&settled).unwrap(), "by hash");
    let again = acked(&svc);
    pump(&mut svc, 60, "another quiet checkpoint", |s| acked(s) > again);
    assert_eq!(revision(&svc), n);

    // a real change in the game: exactly one revision, however many checkpoints follow
    server.ra().run(&format!("botcmd professiontrainer {guid}")).unwrap();
    let mark = acked(&svc);
    pump(&mut svc, 90, "the checkpoints after the change", |s| acked(s) >= mark + 3);
    assert_eq!(revision(&svc), n + 1, "{:?}", super::delta_probe::diff(&settled, &svc.owner().load_current(id).unwrap()));
    let changed = svc.owner().load_current(id).unwrap();
    assert!(changed.build.skills.len() > settled.build.skills.len() || changed.build.spells.len() > settled.build.spells.len(), "the change is the professions");
    server.ra().run(&format!("botcmd despawn {guid}")).unwrap();
}

fn name_of_account(db: &crate::db::Db, id: u32) -> String {
    sql(db, &format!("SELECT username FROM acore_auth.account WHERE id = {id}")).trim().to_string()
}

fn descriptor_secret() -> String {
    tools("COA_PORTABLE_LIVE_B").map(|t| t.2).unwrap_or_else(|| "n/a".into())
}
