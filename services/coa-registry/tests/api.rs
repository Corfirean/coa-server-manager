//! The protocol and its refusals, against a real HTTP server. The same checks run on the in-memory store and, when `REGISTRY_TEST_PG` is set, on PostgreSQL.

mod common;

use coa_registry::api::ApiConfig;
use coa_registry::limits::Rate;
use coa_registry::store::{MemoryStore, RealmRow, ADVERT_VERSION};
use coa_registry_proto::sign::{sign_request, HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_VERSION};
use coa_registry_proto::caps::Ruleset;
use coa_registry_proto::*;
use common::*;
use ed25519_dalek::SigningKey;
use serde_json::json;

/// Limits high enough to stay out of the way of the functional checks (the limits have their own test).
fn lax() -> ApiConfig {
    ApiConfig { general: Rate { burst: 100_000.0, refill: 1000.0 }, register: Rate { burst: 100_000.0, refill: 1000.0 }, heartbeat: Rate { burst: 100_000.0, refill: 1000.0 }, ..ApiConfig::default() }
}

fn memory() -> TestServer {
    spawn(MemoryStore::new(), lax())
}

/// 1, 2, 4: a fresh realm registers, registers again as itself, and a heartbeat updates last_seen.
fn lifecycle(s: &TestServer) {
    let mut h = Host::new(s, 1);
    let r = h.register("Friends' realm", 60);
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!((r.body["created"].as_bool(), r.body["metadata_revision"].as_u64(), r.body["published"].as_bool()), (Some(true), Some(1), Some(true)));
    assert_eq!((r.body["heartbeat_interval_secs"].as_u64(), r.body["online_ttl_secs"].as_u64()), (Some(30), Some(120)));
    assert_eq!(r.body["protocol_version"], 2);

    let me = h.me();
    assert_eq!(me.status, 200, "{:?}", me.body);
    assert_eq!((me.body["online"].as_bool(), me.body["listing"]["display_name"].as_str(), me.body["last_seen_at"].as_i64()), (Some(true), Some("Friends' realm"), Some(s.time())));
    assert_eq!(me.body["capabilities"]["progression"]["max_player_level"], 60);
    assert_eq!((me.body["level_cap"].as_u64(), me.body["ruleset"].as_str()), (Some(60), Some("coa")), "derived from the capabilities");
    assert_eq!((me.body["population"]["players"].as_u64(), me.body["population"]["bots"].as_u64()), (Some(1), Some(4)), "players and bots are separate");
    assert_eq!(me.body["listing"]["rates"]["xp_kill"], 2.0);
    assert!(me.body["listing"]["rates"]["xp_explore"].is_null(), "an unknown rate stays null");
    assert_eq!(me.body["listing"]["modules"][0]["id"], "playerbots");

    s.advance(30);
    h.ts = s.time();
    let hb = h.heartbeat(60, false);
    assert_eq!(hb.status, 200, "{:?}", hb.body);
    assert_eq!((hb.body["metadata_revision"].as_u64(), hb.body["resend_capabilities"].as_bool(), hb.body["resend_listing"].as_bool()), (Some(1), Some(false), Some(false)));
    let me = h.me();
    assert_eq!((me.body["last_seen_at"].as_i64(), me.body["population"]["players"].as_u64(), me.body["population"]["bots"].as_u64()), (Some(s.time()), Some(2), Some(4)), "the heartbeat moved last_seen and the counts");
    assert_eq!(me.body["metadata_revision"], 1, "a heartbeat of unchanged metadata is not a metadata update");

    // the same key announcing itself again (a restart) keeps the identity; unchanged metadata keeps the revision
    let again = h.register("Friends' realm", 60);
    assert_eq!((again.status, again.body["created"].as_bool(), again.body["metadata_revision"].as_u64()), (200, Some(false), Some(1)));
    let renamed = h.register("Friends' realm II", 60);
    assert_eq!((renamed.status, renamed.body["metadata_revision"].as_u64()), (200, Some(2)));
}

/// 5: a missing heartbeat makes the realm offline after the TTL; it is not deleted and the next heartbeat brings it back.
fn presence(s: &TestServer) {
    let mut h = Host::new(s, 2);
    assert_eq!(h.register("Presence", 60).status, 200);
    s.advance(100);
    h.ts = s.time();
    assert_eq!(h.me().body["online"], true, "inside the TTL");
    s.advance(21);
    h.ts = s.time();
    let me = h.me();
    assert_eq!((me.status, me.body["online"].as_bool(), me.body["published"].as_bool()), (200, Some(false), Some(true)), "offline after the TTL, still there and still published");
    let hb = h.heartbeat(60, false);
    assert_eq!(hb.status, 200);
    assert_eq!(h.me().body["online"], true);
}

/// 6: new capabilities and a new listing arrive with a heartbeat and move the metadata revision; a changed hash without the part asks for it.
fn metadata_changes(s: &TestServer) {
    let mut h = Host::new(s, 3);
    assert_eq!(h.register("Caps", 60).status, 200);
    let hb = h.heartbeat(70, false);
    assert_eq!(hb.status, 200);
    assert_eq!((hb.body["resend_capabilities"].as_bool(), hb.body["metadata_revision"].as_u64()), (Some(true), Some(1)), "the hash changed but the capabilities were not sent");
    let me = h.me();
    assert_eq!(me.body["capabilities"]["progression"]["max_player_level"], 60, "nothing was applied without the capabilities");
    let hb = h.heartbeat(70, true);
    assert_eq!((hb.status, hb.body["resend_capabilities"].as_bool(), hb.body["metadata_revision"].as_u64()), (200, Some(false), Some(2)));
    let me = h.me();
    assert_eq!((me.body["capabilities"]["progression"]["max_player_level"].as_u64(), me.body["level_cap"].as_u64(), me.body["metadata_revision"].as_u64()), (Some(70), Some(70), Some(2)), "the level cap follows the capabilities");
    let hb = h.heartbeat(70, true);
    assert_eq!(hb.body["metadata_revision"], 2, "the same capabilities again change nothing");
    // a changed listing: first only its hash (asks for it), then the listing
    let mut body = h.heartbeat_body(70, false);
    let renamed = h.listing("Renamed");
    body.listing_hash = renamed.hash();
    let path = path_heartbeat(&h.realm);
    let r = h.send("POST", &path, &serde_json::to_vec(&body).unwrap());
    assert_eq!((r.status, r.body["resend_listing"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(true), Some(2)));
    h.name = "Renamed".into();
    let body = h.heartbeat_with_listing(70, "Renamed");
    let r = h.send("POST", &path, &serde_json::to_vec(&body).unwrap());
    assert_eq!((r.status, r.body["resend_listing"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(false), Some(3)));
    assert_eq!(h.me().body["listing"]["display_name"], "Renamed");
    // players and bots move without touching the revision
    let mut body = h.heartbeat_body(70, false);
    body.population = Population { players: 18, bots: 46, capacity: None };
    let r = h.send("POST", &path, &serde_json::to_vec(&body).unwrap());
    assert_eq!((r.status, r.body["metadata_revision"].as_u64()), (200, Some(3)));
    let me = h.me();
    assert_eq!((me.body["population"]["players"].as_u64(), me.body["population"]["bots"].as_u64(), me.body["metadata_revision"].as_u64()), (Some(18), Some(46), Some(3)));
}

/// 7: an unrelated key can neither take a realm id over nor speak for it.
fn takeover(s: &TestServer) {
    let mut owner = Host::new(s, 4);
    assert_eq!(owner.register("Mine", 60).status, 200);
    let mut thief = Host::new(s, 5);
    thief.realm = owner.realm;
    thief.ts = s.time() + 10;
    let r = thief.register("Stolen", 60);
    assert_eq!((r.status, r.code().as_str()), (403, "realm_key_mismatch"), "{:?}", r.body);
    let r = thief.heartbeat(60, true);
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"));
    let r = thief.unpublish();
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"));
    let r = thief.me();
    assert_eq!(r.status, 401);
    let me = owner.me();
    assert_eq!((me.body["listing"]["display_name"].as_str(), me.body["published"].as_bool()), (Some("Mine"), Some(true)), "the owner's record is untouched");
}

/// 8, 9, 10: tampering, time, and protocol version.
fn authentication(s: &TestServer) {
    let mut h = Host::new(s, 6);
    assert_eq!(h.register("Auth", 60).status, 200);
    let path = path_heartbeat(&h.realm);
    let body = serde_json::to_vec(&h.heartbeat_body(60, false)).unwrap();

    h.ts += 1;
    let headers = sign_request(&h.key, "POST", &path, &h.realm, h.ts, &body);
    let mut tampered = body.clone();
    tampered.extend_from_slice(b" ");
    let r = h.send_with("POST", &path, &tampered, &headers.pairs());
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"));
    let r = h.send_with("POST", &path_unpublish(&h.realm), &serde_json::to_vec(&UnpublishRequest { protocol_version: 2 }).unwrap(), &headers.pairs());
    assert_eq!((r.status, r.code().as_str()), (401, "invalid_signature"), "a signature for one endpoint is not valid for another");
    let r = h.send_with("POST", &path, &body, &headers.pairs());
    assert_eq!(r.status, 200, "the untampered request is fine: {:?}", r.body);
    let r = h.send_with("POST", &path, &body, &headers.pairs());
    assert_eq!((r.status, r.code().as_str()), (401, "timestamp_not_monotonic"), "a captured request cannot be replayed");

    for skew in [-400, 400] {
        h.ts = s.time() + skew;
        let r = h.send("POST", &path, &body);
        assert_eq!((r.status, r.code().as_str()), (401, "bad_timestamp"), "skew {skew}");
    }
    h.ts = s.time() + 100;
    assert_eq!(h.send("POST", &path, &body).status, 200);
    h.ts = s.time() + 50;
    let r = h.send("POST", &path, &body);
    assert_eq!((r.status, r.code().as_str()), (401, "timestamp_not_monotonic"));
    h.ts = s.time() + 101;
    assert_eq!(h.send("POST", &path, &body).status, 200, "later ones are accepted again");

    // protocol version in the header and in the body (version 1 is refused like any other)
    for wrong in ["1", "3"] {
        h.ts += 1;
        let mut headers = sign_request(&h.key, "POST", &path, &h.realm, h.ts, &body).pairs();
        headers[0] = (HEADER_VERSION, wrong.into());
        let r = h.send_with("POST", &path, &body, &headers);
        assert_eq!((r.status, r.code().as_str()), (400, "unsupported_protocol_version"), "header {wrong}");
    }
    let mut v1 = h.heartbeat_body(60, false);
    v1.protocol_version = 1;
    let r = h.send("POST", &path, &serde_json::to_vec(&v1).unwrap());
    assert_eq!((r.status, r.code().as_str()), (400, "unsupported_protocol_version"));
    let r = h.send_with("POST", &path, &body, &[]);
    assert_eq!(r.status, 400);
    let mut bad = sign_request(&h.key, "POST", &path, &h.realm, h.ts + 1, &body).pairs();
    bad[3] = (HEADER_SIGNATURE, "not-a-signature".into());
    assert_eq!(h.send_with("POST", &path, &body, &bad).status, 400);
    let mut bad = sign_request(&h.key, "POST", &path, &h.realm, h.ts + 1, &body).pairs();
    bad[2] = (HEADER_TIMESTAMP, "yesterday".into());
    assert_eq!(h.send_with("POST", &path, &body, &bad).status, 400);
}

/// The one documented policy for protocol 1: it is refused, explicitly, on every endpoint, and nothing is stored or silently migrated.
fn old_protocol_is_refused(s: &TestServer) {
    let mut h = Host::new(s, 11);
    for (method, path) in [("POST", "/registry/v1/realms/register"), ("POST", &*format!("/registry/v1/realms/{}/heartbeat", h.realm)), ("POST", &*format!("/registry/v1/realms/{}/unpublish", h.realm)), ("GET", &*format!("/registry/v1/realms/{}", h.realm)), ("GET", "/registry/v1/healthz"), ("GET", "/registry/v1")] {
        let r = h.send(method, path, if method == "GET" { b"" } else { br#"{"protocol_version":1}"# });
        assert_eq!((r.status, r.code().as_str()), (400, "unsupported_protocol_version"), "{method} {path}: {:?}", r.body);
        assert!(r.body["error"]["message"].as_str().unwrap().contains("protocol version 1 is no longer supported"));
    }
    let mut check = Host::new(s, 11);
    check.realm = h.realm;
    check.ts = h.ts + 10;
    assert_eq!(check.me().status, 404, "a refused v1 request created nothing");
}

/// 11: hostile input is refused safely and stores nothing.
fn hostile_input(s: &TestServer) {
    let mut h = Host::new(s, 7);
    let big = vec![b'x'; MAX_REQUEST_BYTES + 1];
    let r = h.send("POST", PATH_REGISTER, &big);
    assert_eq!((r.status, r.code().as_str()), (413, "request_too_large"));
    for body in [b"not json".as_slice(), b"[]", b"null", &b"{\"a\":".repeat(1)] {
        let r = h.send("POST", PATH_REGISTER, body);
        assert_eq!(r.status, 400, "{:?}", String::from_utf8_lossy(body));
    }
    let nested = format!("{}1{}", "[".repeat(30_000), "]".repeat(30_000));
    assert_eq!(h.send("POST", PATH_REGISTER, nested.as_bytes()).status, 400);
    let base = serde_json::to_value(h.register_body("Hostile", 60)).unwrap();
    let cases: Vec<(Vec<&str>, serde_json::Value)> = vec![
        (vec!["listing", "display_name"], json!("x".repeat(81))),
        (vec!["listing", "display_name"], json!("")),
        (vec!["listing", "display_name"], json!("evil\u{202E}name")),
        (vec!["listing", "display_name"], json!("new\nline")),
        (vec!["listing", "description"], json!("d".repeat(1025))),
        (vec!["listing", "language"], json!("<script>")),
        (vec!["listing", "region"], json!("<img src=x>")),
        (vec!["listing", "manager_version"], json!("1.0 <b>")),
        (vec!["listing", "rates", "xp_kill"], json!(-1.0)),
        (vec!["listing", "rates", "loot"], json!(1.0e9)),
        (vec!["listing", "modules"], json!([{"id": "<script>alert(1)</script>", "enabled": true}])),
        (vec!["listing", "modules"], json!([{"id": "a", "enabled": true}, {"id": "a", "enabled": false}])),
        (vec!["listing", "modules"], json!((0..65).map(|i| json!({"id": format!("m{i}"), "enabled": true})).collect::<Vec<_>>())),
        (vec!["listing", "modules"], json!([{"id": "ok", "version": "v <1>", "enabled": true}])),
        (vec!["listing", "account_provisioning"], json!({"automatic": false, "existing_only": false})),
        (vec!["population", "players"], json!(1_000_000)),
        (vec!["population", "bots"], json!(-5)),
        (vec!["listing_hash"], json!("0".repeat(64))),
        (vec!["capabilities_hash"], json!("0".repeat(64))),
        (vec!["public_key"], json!("short")),
    ];
    for (path, value) in cases {
        let mut v = base.clone();
        let mut at = &mut v;
        for key in &path[..path.len() - 1] {
            at = &mut at[*key];
        }
        at[path[path.len() - 1]] = value;
        let r = h.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap());
        assert!(r.status == 400, "{path:?}: {} {:?}", r.status, r.body);
    }
    for extra in ["character", "snapshot", "password", "ra_password", "db_credentials", "canonical_revision", "ruleset", "level_cap", "game_mode", "ping"] {
        let mut v = base.clone();
        v[extra] = json!("secret");
        let r = h.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap());
        assert_eq!((r.status, r.code().as_str()), (400, "malformed_request"), "{extra}");
    }
    let mut v = base.clone();
    v["listing"]["rates"]["ping"] = json!(5.0);
    assert_eq!(h.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap()).status, 400, "no unknown rate");
    let mut v = base.clone();
    v["capabilities"]["content"]["client_catalog"]["../../etc/passwd"] = json!({"sha256": "a".repeat(64), "records": 1});
    assert_eq!(h.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap()).status, 400, "capabilities are checked field by field");
    let mut v = base.clone();
    v["capabilities"]["notes"] = json!("free text to store");
    assert_eq!(h.send("POST", PATH_REGISTER, &serde_json::to_vec(&v).unwrap()).status, 400, "no free-form storage in the capabilities");
    let mut other = Host::new(s, 7);
    other.ts = h.ts;
    let r = h.send("GET", "/registry/v2/realms/not-a-uuid/self", b"");
    assert_eq!(r.status, 400);
    let path = format!("/registry/v2/realms/{}/self", other.realm.to_string().to_uppercase());
    assert_eq!(h.send("GET", &path, b"").status, 400, "upper case is not the canonical spelling");
    assert_eq!(h.send("GET", &format!("{}?x=1", path_self(&h.realm)), b"").status, 400, "no query on a signed endpoint");
    let r = h.send("GET", &path_self(&other.realm), b"");
    assert_eq!(r.status, 400, "the realm header names another realm than the path");
    let r = h.send("GET", "/registry/v2/realms/018f2d9e-5c3a-4b21-8c4d-0e5f6a7b8c9d/self", b"");
    assert_eq!(r.status, 400, "a version-4 UUID is not a realm id");
    let mut clean = Host::new(s, 7);
    clean.realm = h.realm;
    clean.ts = h.ts + 100;
    assert_eq!(clean.me().status, 404, "the hostile registrations left no realm behind");
}

/// 12: an unpublished realm stops being published and cannot heartbeat; registering again publishes it.
fn unpublishing(s: &TestServer) {
    let mut h = Host::new(s, 8);
    assert_eq!(h.register("Brief", 60).status, 200);
    let r = h.unpublish();
    assert_eq!((r.status, r.body["published"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(false), Some(2)));
    let me = h.me();
    assert_eq!((me.body["published"].as_bool(), me.body["online"].as_bool()), (Some(false), Some(false)), "stopped, and not online even though it was just heard of");
    let hb = h.heartbeat(60, false);
    assert_eq!((hb.status, hb.code().as_str()), (409, "not_published"));
    let r = h.unpublish();
    assert_eq!((r.status, r.body["metadata_revision"].as_u64()), (200, Some(2)), "unpublishing twice changes nothing");
    let r = h.register("Brief", 60);
    assert_eq!((r.status, r.body["created"].as_bool(), r.body["published"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(false), Some(true), Some(3)));
    assert_eq!(h.heartbeat(60, false).status, 200);
    let unknown = {
        let mut u = Host::new(s, 9);
        u.ts = s.time();
        u.unpublish()
    };
    assert_eq!((unknown.status, unknown.code().as_str()), (404, "unknown_realm"));
}

fn healthz(s: &TestServer) {
    let r = reqwest::blocking::get(format!("{}/registry/v2/healthz", s.base)).unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().unwrap();
    assert_eq!((v["status"].as_str(), v["protocol_version"].as_u64()), (Some("ok"), Some(2)));
    assert_eq!(reqwest::blocking::get(format!("{}/registry/v2/nothing", s.base)).unwrap().status(), 404);
    assert_eq!(reqwest::blocking::get(format!("{}/admin", s.base)).unwrap().status(), 404, "and no admin surface");
}

fn all(s: &TestServer) {
    healthz(s);
    lifecycle(s);
    presence(s);
    metadata_changes(s);
    takeover(s);
    authentication(s);
    old_protocol_is_refused(s);
    hostile_input(s);
    unpublishing(s);
}

#[test]
fn the_protocol_on_the_memory_store() {
    all(&memory());
}

/// A record left by protocol 1 keeps its identity, key and creation time when its Host republishes under protocol 2.
#[test]
fn a_protocol_1_record_is_upgraded_in_place() {
    let store = std::sync::Arc::new(MemoryStore::new());
    let s = spawn(store.clone(), lax());
    let mut h = Host::new(&s, 60);
    let caps = caps(60);
    let created = START - 86_400;
    let old = RealmRow {
        realm_id: h.realm,
        public_key: *h.key.verifying_key().as_bytes(),
        created_at: created,
        updated_at: created,
        last_seen_at: created,
        published: true,
        listing: Listing { display_name: "Old realm".into(), description: "from protocol 1".into(), language: "en".into(), region: None, rates: Rates::default(), modules: vec![], account_provisioning: AccountProvisioning { automatic: false, existing_only: true }, manager_version: "0.6.6".into() },
        listing_hash: "f".repeat(64),
        ruleset: Ruleset::Coa,
        level_cap: Some(60),
        capabilities: caps.clone(),
        capabilities_hash: caps.advert_hash(),
        population: Population::default(),
        metadata_revision: 5,
        last_request_ts: s.time() - 1000,
        advert_version: 1,
    };
    store.put_for_test(old);
    // a heartbeat of the new protocol with only a hash cannot upgrade it: the listing is asked for
    h.ts = s.time();
    h.name = "Old realm".into();
    let r = h.heartbeat(60, false);
    assert_eq!((r.status, r.body["resend_listing"].as_bool()), (200, Some(true)), "{:?}", r.body);
    assert_eq!(store.get_for_test(&h.realm).unwrap().advert_version, 1, "nothing was applied");
    let body = h.heartbeat_with_listing(60, "Old realm");
    let path = path_heartbeat(&h.realm);
    let r = h.send("POST", &path, &serde_json::to_vec(&body).unwrap());
    assert_eq!((r.status, r.body["resend_listing"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(false), Some(6)), "{:?}", r.body);
    let row = store.get_for_test(&h.realm).unwrap();
    assert_eq!((row.advert_version, ADVERT_VERSION, row.created_at, row.public_key), (2, 2, created, *h.key.verifying_key().as_bytes()), "identity, key and creation time are kept");
    assert_eq!(row.listing.rates.xp_kill, Some(2.0));

    // and the other way: registering again as itself
    let mut h2 = Host::new(&s, 61);
    let old2 = RealmRow { realm_id: h2.realm, public_key: *h2.key.verifying_key().as_bytes(), advert_version: 1, last_request_ts: s.time() - 1000, ..store.get_for_test(&h.realm).unwrap() };
    store.put_for_test(old2);
    h2.ts = s.time();
    let r = h2.register("Republished", 60);
    assert_eq!((r.status, r.body["created"].as_bool(), r.body["metadata_revision"].as_u64()), (200, Some(false), Some(7)), "{:?}", r.body);
    assert_eq!(store.get_for_test(&h2.realm).unwrap().created_at, created);
}

#[test]
fn a_flood_is_limited() {
    let cfg = ApiConfig { register: Rate { burst: 3.0, refill: 0.0 }, heartbeat: Rate { burst: 2.0, refill: 0.0 }, general: Rate { burst: 1000.0, refill: 0.0 }, ..ApiConfig::default() };
    let s = spawn(MemoryStore::new(), cfg);
    let mut limited = 0;
    for i in 0..6 {
        let mut h = Host::new(&s, 20 + i);
        if h.register("Flood", 60).status == 429 {
            limited += 1;
        }
    }
    assert_eq!(limited, 3, "three registrations from one address, then a limit");

    let s = spawn(MemoryStore::new(), ApiConfig { heartbeat: Rate { burst: 2.0, refill: 0.0 }, ..ApiConfig::default() });
    let mut h = Host::new(&s, 30);
    assert_eq!(h.register("Beat", 60).status, 200);
    let a = h.heartbeat(60, false).status;
    let b = h.heartbeat(60, false).status;
    let c = h.heartbeat(60, false);
    assert_eq!((a, b, c.status, c.code().as_str()), (200, 200, 429, "rate_limited"), "heartbeats are limited per realm");
    let mut other = Host::new(&s, 31);
    assert_eq!(other.register("Other", 60).status, 200);
    assert_eq!(other.heartbeat(60, false).status, 200, "another realm has its own allowance");

    let s = spawn(MemoryStore::new(), ApiConfig { general: Rate { burst: 12.0, refill: 0.0 }, heartbeat: Rate { burst: 2.0, refill: 0.0 }, ..ApiConfig::default() });
    let mut victim = Host::new(&s, 40);
    assert_eq!(victim.register("Victim", 60).status, 200);
    let mut forger = Host::new(&s, 41);
    forger.realm = victim.realm;
    forger.ts = s.time() + 5;
    let mut statuses = Vec::new();
    for _ in 0..6 {
        statuses.push(forger.heartbeat(60, false).status);
    }
    assert_eq!(&statuses[..2], &[401, 401]);
    assert_eq!(*statuses.last().unwrap(), 429, "the forger runs out of allowance: {statuses:?}");
}

#[test]
fn a_realm_key_is_what_identifies_it_not_its_address_or_name() {
    let s = memory();
    let mut a = Host::new(&s, 50);
    let mut b = Host::new(&s, 51);
    assert_eq!(a.register("Same name", 60).status, 200);
    assert_eq!(b.register("Same name", 60).status, 200, "names are not identities");
    assert_ne!(a.realm, b.realm);
    let wrong = SigningKey::from_bytes(&[52; 32]);
    let path = path_heartbeat(&a.realm);
    let body = serde_json::to_vec(&a.heartbeat_body(60, false)).unwrap();
    let h = sign_request(&wrong, "POST", &path, &a.realm, s.time() + 5, &body);
    assert_eq!(a.send_with("POST", &path, &body, &h.pairs()).status, 401);
}

#[test]
#[ignore]
fn the_protocol_on_postgresql() {
    let Ok(url) = std::env::var("REGISTRY_TEST_PG") else { return };
    // host:port:dbname:user:password
    let parts: Vec<&str> = url.splitn(5, ':').collect();
    let cfg = coa_registry::pg::PgConfig { host: parts[0].into(), port: parts[1].parse().unwrap(), dbname: parts[2].into(), user: parts[3].into(), password: parts[4].into(), pool_size: 4, statement_timeout_ms: 5000 };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = rt.block_on(async {
        let s = coa_registry::pg::PgStore::connect(&cfg).await.unwrap();
        s.migrate().await.unwrap();
        s.migrate().await.unwrap();
        s
    });
    let s = spawn(store, lax());
    all(&s);
    std::mem::forget(rt);
}
