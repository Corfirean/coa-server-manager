//! The Host Manager's publishing lifecycle (coa-core) against the real Registry code over HTTP, on virtual time.

#[path = "../../coa-registry/tests/common/mod.rs"]
mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_core::realm_registry::host::{Clock, PublishState, RegistryHost, Timing};
use coa_core::realm_registry::{AdvertSource, FileKeyStore, KeyStore, LocalAdvert};
use coa_registry::api::ApiConfig;
use coa_registry::limits::Rate;
use coa_registry::store::MemoryStore;
use coa_registry_proto::{AccountProvisioning, ModuleEntry, Population, Rates};
use common::*;

struct Source(Mutex<LocalAdvert>);

impl Source {
    fn new(level: u32) -> Arc<Self> {
        Arc::new(Self(Mutex::new(LocalAdvert { running: true, capabilities: Some(caps(level)), population: Population { players: 3, bots: 7, capacity: None }, rates: Rates { xp_kill: Some(2.0), ..Rates::default() }, modules: vec![ModuleEntry { id: "playerbots".into(), version: Some("1.4.2".into()), enabled: true }], account_provisioning: AccountProvisioning { automatic: false, existing_only: true }, manager_version: "0.6.6".into() })))
    }
    fn set(&self, f: impl FnOnce(&mut LocalAdvert)) {
        f(&mut self.0.lock().unwrap());
    }
}

impl AdvertSource for Source {
    fn current(&self, _local_id: &str) -> Option<LocalAdvert> {
        Some(self.0.lock().unwrap().clone())
    }
}

fn lax() -> ApiConfig {
    ApiConfig { general: Rate { burst: 100_000.0, refill: 1000.0 }, register: Rate { burst: 100_000.0, refill: 1000.0 }, heartbeat: Rate { burst: 100_000.0, refill: 1000.0 }, ..ApiConfig::default() }
}

struct Rig {
    server: TestServer,
    store: Arc<MemoryStore>,
    dir: tempfile::TempDir,
    source: Arc<Source>,
    t0: Instant,
    elapsed: Duration,
}

impl Rig {
    fn new() -> Self {
        let store = Arc::new(MemoryStore::new());
        let server = spawn(store.clone(), lax());
        Self { server, store, dir: tempfile::tempdir().unwrap(), source: Source::new(60), t0: Instant::now(), elapsed: Duration::ZERO }
    }

    fn clock(&self) -> Clock {
        let now = self.server.now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn keys(&self) -> Arc<dyn KeyStore> {
        Arc::new(FileKeyStore::new(self.dir.path().join("keys")))
    }

    /// A Manager start: everything is read back from the folder.
    fn open(&self) -> RegistryHost {
        let mut host = RegistryHost::open(self.dir.path(), self.keys(), self.source.clone(), self.clock(), Timing::default(), self.t0 + self.elapsed).unwrap();
        host.set_url(Some(&self.server.base), self.t0 + self.elapsed).unwrap();
        host
    }

    /// Let `secs` pass for the Registry's clock and for the Host's timers, then run one pass.
    fn pass(&mut self, host: &mut RegistryHost, secs: u64) {
        self.server.advance(secs as i64);
        self.elapsed += Duration::from_secs(secs);
        host.tick(self.t0 + self.elapsed);
    }

    fn now(&self) -> Instant {
        self.t0 + self.elapsed
    }
}

fn state(host: &RegistryHost, rig: &Rig) -> PublishState {
    host.status(rig.now()).realms[0].state
}

#[test]
fn a_fresh_realm_makes_an_identity_registers_and_heartbeats() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Friends' realm", "Plain text.", "en", None, rig.now()).unwrap();
    let realm = host.realm_id("srv-1").expect("an identity was made");
    assert!(rig.dir.path().join("keys").join(format!("{realm}.key")).is_file());
    rig.pass(&mut host, 0);
    assert_eq!(state(&host, &rig), PublishState::Online);

    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.realm_id, rec.listing.display_name.as_str(), rec.published, rec.online, rec.metadata_revision), (realm, "Friends' realm", true, true, 1));
    assert_eq!(Some(rec.public_key.clone()), host.public_key("srv-1"));
    assert_eq!((rec.population.players, rec.population.bots), (3, 7), "players and bots arrive separately");
    assert_eq!(rec.listing.rates.xp_kill, Some(2.0));
    assert_eq!(rec.level_cap, Some(60));

    let before = rec.last_seen_at;
    rig.pass(&mut host, 35);
    let rec = host.registry_record("srv-1").unwrap();
    assert!(rec.last_seen_at > before, "the heartbeat moved last_seen");
    assert_eq!(rec.metadata_revision, 1, "an unchanged realm is not a metadata update");

    // nothing secret in anything the Host writes or shows
    let key_text = std::fs::read_to_string(rig.dir.path().join("keys").join(format!("{realm}.key"))).unwrap();
    let status = serde_json::to_string(&host.status(rig.now())).unwrap();
    let settings = std::fs::read_to_string(rig.dir.path().join("registry.json")).unwrap();
    for text in [&status, &settings] {
        assert!(!text.contains(key_text.trim()), "the private key is only in its own file");
    }
}

#[test]
fn a_manager_restart_keeps_the_realm_id_and_the_key() {
    let mut rig = Rig::new();
    let (realm, key) = {
        let mut host = rig.open();
        host.publish("srv-1", "Restarting", "", "en", None, rig.now()).unwrap();
        rig.pass(&mut host, 0);
        assert_eq!(state(&host, &rig), PublishState::Online);
        (host.realm_id("srv-1").unwrap(), host.public_key("srv-1").unwrap())
    };
    rig.server.advance(5);
    let mut host = rig.open();
    assert_eq!((host.realm_id("srv-1"), host.public_key("srv-1")), (Some(realm), Some(key.clone())), "the same identity after the restart");
    rig.pass(&mut host, 0);
    assert_eq!(state(&host, &rig), PublishState::Online, "it resumes by itself");
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.realm_id, rec.public_key), (realm, key));
    assert_eq!(rec.metadata_revision, 1, "registering again as itself changed nothing");
    assert_eq!(rig.dir.path().join("keys").read_dir().unwrap().count(), 1, "no second key was made");
}

#[test]
fn changed_capabilities_reach_the_registry_with_the_next_heartbeat() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Caps", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    rig.source.set(|a| a.capabilities = Some(caps(70)));
    rig.pass(&mut host, 35);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.capabilities.progression.unwrap().max_player_level, rec.metadata_revision), (70, 2));
    rig.pass(&mut host, 35);
    assert_eq!(host.registry_record("srv-1").unwrap().metadata_revision, 2, "and then nothing is rewritten");
    // a rename
    host.publish("srv-1", "Renamed", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    rig.pass(&mut host, 0);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!(rec.listing.display_name, "Renamed");
}

#[test]
fn missing_heartbeats_make_the_realm_offline_and_it_comes_back() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Gone and back", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    // the Manager is closed: nothing is sent
    rig.server.advance(125);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.online, rec.published), (false, true), "offline, not deleted");
    drop(host);
    let mut host = rig.open();
    rig.pass(&mut host, 0);
    assert!(host.registry_record("srv-1").unwrap().online, "the restarted Manager brings it back");
}

#[test]
fn a_stopped_realm_sends_no_heartbeats_and_goes_offline() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Stoppable", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    rig.source.set(|a| a.running = false);
    for _ in 0..5 {
        rig.pass(&mut host, 35);
    }
    assert_eq!(state(&host, &rig), PublishState::RealmStopped);
    assert!(!host.registry_record("srv-1").unwrap().online, "no worldserver, no presence");
    rig.source.set(|a| a.running = true);
    rig.pass(&mut host, 35);
    assert!(host.registry_record("srv-1").unwrap().online);
}

#[test]
fn unpublishing_stops_the_publication_and_the_identity_is_kept() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Brief", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    let realm = host.realm_id("srv-1").unwrap();
    host.unpublish("srv-1", rig.now()).unwrap();
    rig.pass(&mut host, 0);
    assert_eq!(state(&host, &rig), PublishState::Disabled);
    assert!(!host.registry_record("srv-1").unwrap().published);
    rig.pass(&mut host, 60);
    assert!(!host.registry_record("srv-1").unwrap().published, "no heartbeat republishes it");
    host.publish("srv-1", "Brief", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.realm_id, rec.published, rec.online), (realm, true, true), "the same realm id, published again");
}

#[test]
fn a_registry_that_is_down_is_retried_with_a_growing_delay_and_nothing_else_happens() {
    let mut rig = Rig::new();
    let mut host = RegistryHost::open(rig.dir.path(), rig.keys(), rig.source.clone(), rig.clock(), Timing::default(), rig.now()).unwrap();
    host.set_url(Some("http://127.0.0.1:9"), rig.now()).unwrap();
    host.publish("srv-1", "Offline registry", "", "en", None, rig.now()).unwrap();
    let mut waits = Vec::new();
    for _ in 0..6 {
        host.tick(rig.now());
        let s = host.status(rig.now());
        assert_eq!(s.realms[0].state, PublishState::Retrying);
        assert!(s.realms[0].last_error.as_deref().unwrap_or("").contains("could not be reached"));
        let wait = s.realms[0].retry_in_secs.unwrap();
        waits.push(wait);
        rig.elapsed += Duration::from_secs(wait + 1);
    }
    assert!(waits.windows(2).filter(|w| w[1] >= w[0]).count() >= 4, "the delay grows: {waits:?}");
    assert!(waits.iter().all(|w| *w <= 300 * 5 / 4 + 1), "and is capped: {waits:?}");
    // the Registry comes back: the same Host recovers by itself
    host.set_url(Some(&rig.server.base), rig.now()).unwrap();
    rig.pass(&mut host, 0);
    assert_eq!(state(&host, &rig), PublishState::Online);
}

#[test]
fn a_refusal_that_repeating_cannot_fix_stops_the_publication() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Contested", "", "en", None, rig.now()).unwrap();
    // somebody else got to this realm id first with another key
    let mut squatter = Host::new(&rig.server, 99);
    squatter.realm = host.realm_id("srv-1").unwrap();
    assert_eq!(squatter.register("Squatter", 60).status, 200);
    rig.pass(&mut host, 0);
    let s = host.status(rig.now());
    assert_eq!(s.realms[0].state, PublishState::Rejected);
    assert!(s.realms[0].retry_in_secs.is_none(), "no further attempt is scheduled");
    assert!(s.realms[0].last_error.as_deref().unwrap_or("").contains("RealmKeyMismatch"));
    let seen = rig.store.get_for_test(&squatter.realm);
    rig.pass(&mut host, 600);
    rig.pass(&mut host, 600);
    assert_eq!(state(&host, &rig), PublishState::Rejected, "still stopped");
    assert_eq!(rig.store.get_for_test(&squatter.realm), seen, "and nothing was sent in the meantime");
}

#[test]
fn a_registry_that_lost_its_data_gets_the_realm_again() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Amnesia", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    rig.store.forget_all();
    rig.pass(&mut host, 35);
    assert_eq!(state(&host, &rig), PublishState::Retrying, "the heartbeat of an unknown realm is refused");
    rig.pass(&mut host, 2);
    assert_eq!(state(&host, &rig), PublishState::Online, "and it registers again as itself");
    assert!(host.registry_record("srv-1").unwrap().online);
}

#[test]
fn the_host_never_publishes_what_is_not_discovery_metadata() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Plain", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    let rec = host.registry_record("srv-1").unwrap();
    let json = serde_json::to_string(&rec).unwrap().to_lowercase();
    for word in ["password", "credential", "secret", "snapshot", "inventory", "canonical", "character_id", "ra_user", "mysql", "token"] {
        assert!(!json.contains(word), "the Registry's record mentions {word}: {json}");
    }
    assert!(host.publish("srv-1", &"x".repeat(81), "", "en", None, rig.now()).is_err());
    assert!(host.publish("srv-1", "ok", "", "not a language", None, rig.now()).is_err());
}

#[test]
fn a_realm_whose_key_is_gone_is_stopped_and_publishing_again_makes_a_new_identity() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Lost key", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    let old = host.realm_id("srv-1").unwrap();
    drop(host);
    std::fs::remove_file(rig.dir.path().join("keys").join(format!("{old}.key"))).unwrap();
    let mut host = rig.open();
    rig.pass(&mut host, 0);
    let s = host.status(rig.now());
    assert_eq!(s.realms[0].state, PublishState::Rejected);
    assert!(s.realms[0].last_error.as_deref().unwrap_or("").contains("key is missing"), "{:?}", s.realms[0].last_error);
    host.publish("srv-1", "Lost key", "", "en", None, rig.now()).unwrap();
    rig.pass(&mut host, 0);
    assert_eq!(state(&host, &rig), PublishState::Online);
    assert_ne!(host.realm_id("srv-1").unwrap(), old, "a new identity, not the old id with a guessed key");
}

#[test]
fn a_changed_rate_or_module_reaches_the_registry_with_the_next_heartbeat_and_counts_move_without_a_revision() {
    let mut rig = Rig::new();
    let mut host = rig.open();
    host.publish("srv-1", "Rates", "", "en", Some("EU"), rig.now()).unwrap();
    rig.pass(&mut host, 0);
    let first = host.registry_record("srv-1").unwrap();
    assert_eq!((first.listing.region.as_deref(), first.metadata_revision), (Some("EU"), 1));
    rig.source.set(|a| {
        a.rates.xp_kill = Some(5.0);
        a.modules.push(ModuleEntry { id: "content-scaling".into(), version: None, enabled: true });
    });
    rig.pass(&mut host, 35);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.listing.rates.xp_kill, rec.listing.modules.len(), rec.metadata_revision), (Some(5.0), 2, 2));
    rig.source.set(|a| a.population = Population { players: 18, bots: 46, capacity: Some(100) });
    rig.pass(&mut host, 35);
    let rec = host.registry_record("srv-1").unwrap();
    assert_eq!((rec.population.players, rec.population.bots, rec.population.capacity, rec.metadata_revision), (18, 46, Some(100), 2), "players and bots separately; population is not a metadata change");
}
