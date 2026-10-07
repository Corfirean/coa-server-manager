//! The Phase-10 gate against a real node (`REGISTRY_URL=http://<VPS_IP>`), through the public path: Caddy, the Registry container, PostgreSQL.
//! `#[ignore]`d. The infrastructure items (container restarts, port scan, database contents) are driven by `tools/registry_gate.ps1` / `.sh`.

#[path = "../../coa-registry/tests/common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use coa_core::realm_registry::host::{system_clock, PublishState, RegistryHost, Timing};
use coa_core::realm_registry::{AdvertSource, FileKeyStore, KeyStore, LocalAdvert};
use coa_registry_proto::sign::sign_request;
use coa_registry_proto::*;
use common::*;
use serde_json::json;

fn base() -> Option<String> {
    std::env::var("REGISTRY_URL").ok().filter(|u| !u.is_empty())
}

struct Fixed(LocalAdvert);

impl AdvertSource for Fixed {
    fn current(&self, _: &str) -> Option<LocalAdvert> {
        Some(self.0.clone())
    }
}

fn pass(n: u32, what: &str) {
    println!("GATE {n:>2} PASS  {what}");
}

#[test]
#[ignore]
fn gate_1_to_12_against_the_node() {
    let Some(base) = base() else { return };

    // 1. a fresh realm generates its identity and registers (through the Host Manager code)
    let dir = tempfile::tempdir().unwrap();
    let keys: Arc<dyn KeyStore> = Arc::new(FileKeyStore::new(dir.path().join("keys")));
    let source = Arc::new(Fixed(LocalAdvert { running: true, capabilities: Some(caps(60)), player_count: Some(2), player_capacity: None, manager_version: "0.6.6".into() }));
    let t0 = Instant::now();
    let mut elapsed = Duration::ZERO;
    let mut host = RegistryHost::open(dir.path(), keys.clone(), source.clone(), system_clock(), Timing::default(), t0).unwrap();
    host.set_url(Some(&base), t0).unwrap();
    host.publish("srv-gate", "Gate realm", "Phase 10 gate", "en", t0).unwrap();
    host.tick(t0);
    assert_eq!(host.status(t0).realms[0].state, PublishState::Online, "{:?}", host.status(t0));
    let realm = host.realm_id("srv-gate").unwrap();
    let key = host.public_key("srv-gate").unwrap();
    let rec = host.registry_record("srv-gate").unwrap();
    assert_eq!((rec.realm_id, rec.published, rec.online, rec.metadata_revision), (realm, true, true, 1));
    pass(1, &format!("fresh realm {realm} generated an identity and registered"));

    // 3. a Host Manager restart keeps the same RealmId and key
    drop(host);
    let mut host = RegistryHost::open(dir.path(), keys.clone(), source.clone(), system_clock(), Timing::default(), t0).unwrap();
    host.set_url(Some(&base), t0).unwrap();
    assert_eq!((host.realm_id("srv-gate"), host.public_key("srv-gate")), (Some(realm), Some(key.clone())));
    std::thread::sleep(Duration::from_millis(1200));
    host.tick(t0);
    assert_eq!(host.status(t0).realms[0].state, PublishState::Online);
    let rec2 = host.registry_record("srv-gate").unwrap();
    assert_eq!((rec2.realm_id, rec2.public_key.clone(), rec2.metadata_revision), (realm, key, 1));
    pass(3, "after a Manager restart: same RealmId, same key, registered again as itself, revision unchanged");

    // 4. a valid heartbeat updates last_seen
    std::thread::sleep(Duration::from_millis(2100));
    elapsed += Duration::from_secs(31);
    host.tick(t0 + elapsed + Duration::from_secs(10));
    let rec3 = host.registry_record("srv-gate").unwrap();
    assert!(rec3.last_seen_at > rec2.last_seen_at, "{} -> {}", rec2.last_seen_at, rec3.last_seen_at);
    pass(4, &format!("heartbeat moved last_seen_at {} -> {}", rec2.last_seen_at, rec3.last_seen_at));

    // 6. a capability change updates the record with the next heartbeat
    let source2 = Arc::new(Fixed(LocalAdvert { running: true, capabilities: Some(caps(70)), player_count: Some(3), player_capacity: None, manager_version: "0.6.6".into() }));
    drop(host);
    let mut host = RegistryHost::open(dir.path(), keys.clone(), source2, system_clock(), Timing::default(), t0).unwrap();
    host.set_url(Some(&base), t0).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    host.tick(t0);
    std::thread::sleep(Duration::from_millis(1200));
    elapsed += Duration::from_secs(31);
    host.tick(t0 + elapsed + Duration::from_secs(40));
    let rec4 = host.registry_record("srv-gate").unwrap();
    assert_eq!(rec4.capabilities.progression.as_ref().unwrap().max_player_level, 70, "{rec4:?}");
    assert!(rec4.metadata_revision > rec3.metadata_revision);
    pass(6, &format!("capability change (cap 60 -> 70) applied; metadata revision {} -> {}", rec3.metadata_revision, rec4.metadata_revision));

    // 12. unpublish stops publication (the Manager's path)
    host.unpublish("srv-gate", t0 + elapsed).unwrap();
    host.tick(t0 + elapsed);
    let rec5 = host.registry_record("srv-gate").unwrap();
    assert!(!rec5.published && !rec5.online);
    pass(12, "unpublish: published=false, online=false");

    // 7-11 on a separate realm with the raw protocol
    let mut h = Host::remote(&base);
    assert_eq!(h.register("Raw realm", 60).status, 200);
    let mut thief = Host::remote(&base);
    thief.realm = h.realm;
    let r = thief.register("Stolen", 60);
    assert_eq!((r.status, r.code().as_str()), (403, "realm_key_mismatch"));
    let r = thief.heartbeat(60, true);
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"));
    assert_eq!(h.me().body["display_name"], "Raw realm");
    pass(7, "same RealmId signed by another key: register 403 realm_key_mismatch, heartbeat 401 invalid_signature, record untouched");

    h.sync_clock();
    let path = path_heartbeat(&h.realm);
    let body = serde_json::to_vec(&h.heartbeat_body(60, false)).unwrap();
    h.ts += 1;
    let headers = sign_request(&h.key, "POST", &path, &h.realm, h.ts, &body);
    let mut tampered = body.clone();
    tampered.push(b' ');
    let r = h.send_with("POST", &path, &tampered, &headers.pairs());
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"));
    pass(8, "body tampered after signing: 401 invalid_signature");

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    for skew in [-3600, 3600] {
        h.ts = now + skew;
        let r = h.send("POST", &path, &body);
        assert_eq!((r.status, r.code().as_str()), (401, "bad_timestamp"), "skew {skew}");
    }
    h.ts = now;
    h.sync_clock();
    let ok = h.send("POST", &path, &body);
    assert_eq!(ok.status, 200, "{:?}", ok.body);
    h.ts -= 5;
    let r = h.send("POST", &path, &body);
    assert_eq!((r.status, r.code().as_str()), (401, "timestamp_not_monotonic"));
    pass(9, "old (-1h) and future (+1h) timestamps: 401 bad_timestamp; a non-increasing one: 401 timestamp_not_monotonic");

    h.sync_clock();
    let mut hv = sign_request(&h.key, "POST", &path, &h.realm, h.ts + 1, &body).pairs();
    hv[0] = (sign_header_version(), "2".into());
    let r = h.send_with("POST", &path, &body, &hv);
    assert_eq!((r.status, r.code().as_str()), (400, "unsupported_protocol_version"));
    let mut v2 = h.heartbeat_body(60, false);
    v2.protocol_version = 2;
    let r = h.send("POST", &path, &serde_json::to_vec(&v2).unwrap());
    assert_eq!((r.status, r.code().as_str()), (400, "unsupported_protocol_version"));
    pass(10, "protocol version 2 (header and body): 400 unsupported_protocol_version");

    let big = vec![b'x'; MAX_REQUEST_BYTES + 1];
    let r = h.send("POST", PATH_REGISTER, &big);
    assert_eq!((r.status, r.code().as_str()), (413, "request_too_large"));
    let mut hostile = Host::remote(&base);
    let base_body = serde_json::to_value(hostile.register_body("Hostile", 60)).unwrap();
    for (field, value) in [("display_name", json!("x".repeat(500))), ("description", json!("d".repeat(5000))), ("language", json!("<script>alert(1)</script>")), ("display_name", json!("evil\u{202E}name"))] {
        let mut v = base_body.clone();
        v[field] = value;
        let r = hostile.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap());
        assert_eq!(r.status, 400, "{field}");
    }
    for extra in ["character", "snapshot", "password", "ra_password", "db_credentials", "canonical_revision"] {
        let mut v = base_body.clone();
        v[extra] = json!("secret");
        let r = hostile.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap());
        assert_eq!((r.status, r.code().as_str()), (400, "malformed_request"), "{extra}");
    }
    let nested = format!("{}1{}", "[".repeat(30_000), "]".repeat(30_000));
    assert_eq!(hostile.send("POST", PATH_REGISTER, nested.as_bytes()).status, 400);
    let mut clean = Host::remote(&base);
    clean.realm = hostile.realm;
    assert_eq!(clean.me().status, 404, "nothing was stored by the hostile requests");
    pass(11, "oversized body 413; hostile/oversized metadata, unknown fields, JSON nesting bomb: 400; nothing stored");

    // the per-realm allowance (burst 4, one more every 10 s) was used up by the refusals above
    std::thread::sleep(Duration::from_secs(22));
    h.sync_clock();
    assert_eq!(h.unpublish().status, 200);
    std::thread::sleep(Duration::from_secs(12));
    h.sync_clock();
    let r = h.heartbeat(60, false);
    assert_eq!((r.status, r.code().as_str()), (409, "not_published"));
    pass(12, "raw unpublish: heartbeat afterwards 409 not_published");

    println!("GATE_REALM {realm}");
}

fn sign_header_version() -> &'static str {
    coa_registry_proto::sign::HEADER_VERSION
}

/// 5. a realm that stops heartbeating goes offline after the TTL and is not deleted (takes just over two minutes).
#[test]
#[ignore]
fn gate_5_missing_heartbeats_mean_offline_not_deleted() {
    let Some(base) = base() else { return };
    let mut h = Host::remote(&base);
    assert_eq!(h.register("TTL realm", 60).status, 200);
    h.sync_clock();
    assert_eq!(h.me().body["online"], true);
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(125));
    h.sync_clock();
    let me = h.me();
    assert_eq!((me.status, me.body["online"].as_bool(), me.body["published"].as_bool()), (200, Some(false), Some(true)), "{:?}", me.body);
    h.sync_clock();
    assert_eq!(h.heartbeat(60, false).status, 200);
    h.sync_clock();
    assert_eq!(h.me().body["online"], true);
    pass(5, &format!("no heartbeat for {}s: online=false, published=true, record kept; a heartbeat brings it back", started.elapsed().as_secs()));
    println!("GATE_REALM {}", h.realm);
}

/// The record a Registry restart (item 2) and a database restart (item 14) are checked against: written once, read back by the driver script.
#[test]
#[ignore]
fn gate_persist_probe_write() {
    let Some(base) = base() else { return };
    let mut h = Host::remote(&base);
    assert_eq!(h.register("Persist realm", 60).status, 200);
    let seed = h.key.to_bytes();
    println!("PERSIST_REALM {} {}", h.realm, hex_of(&seed));
}

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
#[ignore]
fn gate_persist_probe_read() {
    let Some(base) = base() else { return };
    let (Ok(realm), Ok(seed)) = (std::env::var("PERSIST_REALM"), std::env::var("PERSIST_SEED")) else { return };
    let mut h = Host::remote(&base);
    h.realm = RealmId::parse(&realm).unwrap();
    let bytes: Vec<u8> = (0..seed.len() / 2).map(|i| u8::from_str_radix(&seed[2 * i..2 * i + 2], 16).unwrap()).collect();
    h.key = ed25519_dalek::SigningKey::from_bytes(&bytes.try_into().unwrap());
    h.sync_clock();
    let me = h.me();
    assert_eq!((me.status, me.body["display_name"].as_str(), me.body["published"].as_bool()), (200, Some("Persist realm"), Some(true)), "{:?}", me.body);
    h.sync_clock();
    assert_eq!(h.heartbeat(60, false).status, 200, "and it can still heartbeat with its own key");
    println!("PERSIST_OK {realm}");
}

/// Production defaults: ten registrations per hour from one address, then 429 (run once against a freshly started Registry).
#[test]
#[ignore]
fn gate_registration_rate_limit() {
    let Some(base) = base() else { return };
    let mut codes = Vec::new();
    for _ in 0..13 {
        let mut h = Host::remote(&base);
        codes.push(h.register("Limit probe", 60).status);
    }
    let ok = codes.iter().filter(|c| **c == 200).count();
    let limited = codes.iter().filter(|c| **c == 429).count();
    assert!(ok >= 1 && limited >= 1 && ok + limited == 13, "{codes:?}");
    println!("GATE LIMIT PASS  registrations from one address: {ok} accepted, {limited} refused with 429 ({codes:?})");
}
