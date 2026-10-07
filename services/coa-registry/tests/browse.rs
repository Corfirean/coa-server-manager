//! The public, read-only list: pagination at scale, filters, sorts, caching, and what it must never show.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use coa_registry::api::ApiConfig;
use coa_registry::limits::Rate;
use coa_registry::store::{MemoryStore, RealmRow};
use coa_registry_proto::caps::Ruleset;
use coa_registry_proto::*;
use common::*;

fn lax() -> ApiConfig {
    ApiConfig { general: Rate { burst: 1.0e9, refill: 1.0e9 }, register: Rate { burst: 1.0e9, refill: 1.0e9 }, heartbeat: Rate { burst: 1.0e9, refill: 1.0e9 }, browse: Rate { burst: 1.0e9, refill: 1.0e9 }, cache_secs: 0, ..ApiConfig::default() }
}

const NAMES: [&str; 8] = ["Descension", "Alpha Realm", "bravo", "Delta Force", "Echo", "Zulu Base", "Ünïcode Réalm", "Яндекс Мир"];

/// A deterministic synthetic realm. Most are published and online; every 7th is offline, every 11th unpublished, every 13th a protocol-1 record.
fn row(i: u32, now: i64, with_caps: &AdvertisedCapabilities) -> RealmRow {
    let mut id_bytes = *RealmId::new().as_uuid().as_bytes();
    id_bytes[10..14].copy_from_slice(&i.to_be_bytes());
    let realm_id = RealmId::from_uuid(uuid_from(id_bytes)).unwrap();
    let cap = [60, 70, 80, 255][(i % 4) as usize];
    let mut caps = with_caps.clone();
    if let Some(p) = caps.progression.as_mut() {
        p.max_player_level = cap;
    }
    caps.content.ruleset = if i % 5 == 0 { Ruleset::Wildcard } else { Ruleset::Coa };
    caps.content_profile_hash = caps.content.hash();
    let listing = Listing {
        display_name: format!("{} {i:05}", NAMES[(i % 8) as usize]),
        description: format!("Synthetic realm {i}. {}", "lorem ipsum ".repeat((i % 30) as usize)),
        language: ["en", "ru", "de"][(i % 3) as usize].into(),
        region: (i % 2 == 0).then(|| ["EU", "NA", "RU"][(i % 3) as usize].to_string()),
        rates: Rates { xp_kill: Some(1.0 + (i % 5) as f64), xp_quest: None, xp_explore: None, loot: Some(1.0), money: None, reputation: None, honor: None },
        modules: [("playerbots", i % 3 == 0), ("content-scaling", i % 4 == 0), ("random-events", i % 5 == 0)].iter().map(|(id, on)| ModuleEntry { id: (*id).into(), version: None, enabled: *on }).collect(),
        account_provisioning: AccountProvisioning { automatic: i % 2 == 0, existing_only: i % 2 == 1 },
        manager_version: "0.6.6".into(),
    };
    RealmRow {
        realm_id,
        public_key: *ed25519_dalek::SigningKey::from_bytes(&[(i % 250 + 1) as u8; 32]).verifying_key().as_bytes(),
        created_at: now - 1_000_000 + i64::from(i) * 17,
        updated_at: now,
        last_seen_at: if i % 7 == 0 { now - 3600 } else { now - i64::from(i % 100) },
        published: i % 11 != 0,
        listing_hash: listing.hash(),
        listing,
        ruleset: caps.content.ruleset,
        level_cap: caps.level_cap(),
        capabilities_hash: caps.advert_hash(),
        capabilities: caps,
        population: Population { players: (i * 7) % 200, bots: (i * 3) % 90, capacity: Some(200) },
        metadata_revision: 1,
        last_request_ts: 0,
        advert_version: if i % 13 == 0 { 1 } else { 2 },
    }
}

fn uuid_from(bytes: [u8; 16]) -> uuid::Uuid {
    uuid::Uuid::from_bytes(bytes)
}

fn visible(i: u32) -> bool {
    i % 11 != 0 && i % 13 != 0
}
fn online(i: u32) -> bool {
    visible(i) && i % 7 != 0
}

fn fixture(n: u32) -> (Arc<MemoryStore>, TestServer) {
    let store = Arc::new(MemoryStore::new());
    let server = spawn(store.clone(), lax());
    let caps = caps(60);
    for i in 1..=n {
        store.put_for_test(row(i, START, &caps));
    }
    (store, server)
}

fn get(server: &TestServer, path_and_query: &str) -> (u16, serde_json::Value) {
    let r = reqwest::blocking::get(format!("{}{}", server.base, path_and_query)).unwrap();
    let status = r.status().as_u16();
    (status, r.json().unwrap_or(serde_json::Value::Null))
}

/// Walk every page of a query; returns all summaries in order.
fn walk(server: &TestServer, query: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..1000 {
        let mut q = format!("{PATH_LIST}?limit=100&{query}");
        if let Some(c) = &cursor {
            q += &format!("&cursor={c}");
        }
        let (status, body) = get(server, &q);
        assert_eq!(status, 200, "{q}: {body}");
        out.extend(body["realms"].as_array().unwrap().iter().cloned());
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => return out,
        }
    }
    panic!("the pages never ended");
}

#[test]
fn thousands_of_realms_paginate_in_every_order_without_gaps_or_repeats() {
    let (_store, server) = fixture(5000);
    let expected = (1..=5000).filter(|i| online(*i)).count();
    let started = std::time::Instant::now();
    for (sort, order) in [("players", "desc"), ("players", "asc"), ("name", "asc"), ("name", "desc"), ("cap", "desc"), ("cap", "asc"), ("created", "desc"), ("created", "asc")] {
        let all = walk(&server, &format!("sort={sort}&order={order}"));
        assert_eq!(all.len(), expected, "{sort} {order}: every online published realm exactly once");
        let ids: BTreeSet<&str> = all.iter().map(|r| r["realm_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), expected, "{sort} {order}: no repeats");
        let key = |r: &serde_json::Value| -> (String, i64) {
            match sort {
                "name" => (r["display_name"].as_str().unwrap().to_lowercase(), 0),
                "players" => (String::new(), r["population"]["players"].as_i64().unwrap()),
                "cap" => (String::new(), r["level_cap"].as_i64().unwrap_or(0)),
                _ => (String::new(), r["created_at"].as_i64().unwrap()),
            }
        };
        for w in all.windows(2) {
            let (a, b) = (key(&w[0]), key(&w[1]));
            let ok = if order == "asc" { a <= b } else { a >= b };
            assert!(ok, "{sort} {order}: {a:?} then {b:?}");
        }
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 60, "eight full walks of 5000 realms took {elapsed:?}");
    println!("8 full walks of {expected} realms in {elapsed:?}");
}

#[test]
fn filters_and_the_status_switch() {
    let (_store, server) = fixture(1000);
    let ids = |q: &str| walk(&server, q);
    let coa_cap70 = ids("ruleset=coa&cap_min=70&cap_max=70");
    assert!(!coa_cap70.is_empty() && coa_cap70.iter().all(|r| r["ruleset"] == "coa" && r["level_cap"] == 70));
    let bots = ids("module=playerbots&sort=name");
    assert!(!bots.is_empty() && bots.iter().all(|r| r["modules"].as_array().unwrap().iter().any(|m| m["id"] == "playerbots" && m["enabled"] == true)), "only realms that have the module ON");
    let busy = ids("players_min=150");
    assert!(!busy.is_empty() && busy.iter().all(|r| r["population"]["players"].as_u64().unwrap() >= 150));
    let found = ids("q=descension");
    assert!(!found.is_empty() && found.iter().all(|r| r["display_name"].as_str().unwrap().to_lowercase().contains("descension")), "a case-insensitive search of the name");
    assert!(!ids("q=%D1%8F%D0%BD%D0%B4%D0%B5%D0%BA%D1%81").is_empty(), "and of non-Latin names");
    let ru = ids("language=ru&region=RU");
    assert!(ru.iter().all(|r| r["language"] == "ru" && r["region"] == "RU"));
    let online_n = ids("").len();
    let offline_n = ids("status=offline").len();
    let all_n = ids("status=all").len();
    assert_eq!(online_n + offline_n, all_n);
    assert_eq!(all_n, (1..=1000).filter(|i| visible(*i)).count(), "unpublished and protocol-1 records are in no list, whatever the status");
    assert!(ids("status=offline").iter().all(|r| r["online"] == false));
    assert!(ids("").iter().all(|r| r["online"] == true), "by default only online realms");
}

#[test]
fn the_list_shows_discovery_metadata_and_nothing_else() {
    let (store, server) = fixture(40);
    let (status, page) = get(&server, &format!("{PATH_LIST}?limit=5"));
    assert_eq!(status, 200);
    let text = page.to_string().to_lowercase();
    for word in ["public_key", "capabilities\"", "password", "credential", "secret", "snapshot", "inventory", "canonical", "character_id", "ra_user", "mysql", "private", "last_request_ts", "token"] {
        assert!(!text.contains(word), "the list must not carry {word}: {text}");
    }
    let first = &page["realms"][0];
    for field in ["realm_id", "display_name", "description", "language", "ruleset", "level_cap", "rates", "modules", "population", "account_provisioning", "online"] {
        assert!(first.get(field).is_some(), "{field}");
    }
    assert!(first["description"].as_str().unwrap().chars().count() <= 160);
    assert!(first["population"]["players"].is_number() && first["population"]["bots"].is_number(), "players and bots apart");

    // the detail of a published realm has the capabilities (for the compatibility check) and the key (for the later JOIN)
    let id = first["realm_id"].as_str().unwrap().to_string();
    let (status, d) = get(&server, &format!("{PATH_LIST}/{id}"));
    assert_eq!(status, 200);
    assert!(d["capabilities"]["content_profile_hash"].is_string() && d["public_key"].is_string());
    let text = d.to_string().to_lowercase();
    for word in ["password", "credential", "secret", "snapshot", "canonical", "mysql", "private", "last_request_ts"] {
        assert!(!text.contains(word), "{word}");
    }
    // unpublished, protocol-1 and unknown realms have no public detail
    let hidden: Vec<RealmRow> = (1..=40).filter(|i| !visible(*i)).map(|i| store.get_for_test(&row(i, START, &caps(60)).realm_id).unwrap_or_else(|| row(i, START, &caps(60)))).collect();
    assert!(!hidden.is_empty());
    for h in hidden {
        assert_eq!(get(&server, &format!("{PATH_LIST}/{}", h.realm_id)).0, 404, "{:?}", h.realm_id);
    }
    assert_eq!(get(&server, &format!("{PATH_LIST}/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d")).0, 404);
    assert_eq!(get(&server, &format!("{PATH_LIST}/not-a-uuid")).0, 400);
    assert_eq!(get(&server, &format!("{PATH_LIST}/{id}?x=1")).0, 400);
}

#[test]
fn the_public_endpoints_are_read_only() {
    let (_store, server) = fixture(10);
    let http = reqwest::blocking::Client::new();
    let url = format!("{}{PATH_LIST}", server.base);
    for method in [reqwest::Method::POST, reqwest::Method::PUT, reqwest::Method::DELETE, reqwest::Method::PATCH] {
        let r = http.request(method.clone(), &url).body("{}").send().unwrap();
        assert_eq!(r.status().as_u16(), 405, "{method} on the list");
    }
    let (_, page) = get(&server, PATH_LIST);
    let id = page["realms"][0]["realm_id"].as_str().unwrap().to_string();
    for method in [reqwest::Method::POST, reqwest::Method::PUT, reqwest::Method::DELETE] {
        let r = http.request(method.clone(), format!("{url}/{id}")).body("{}").send().unwrap();
        assert!(r.status().as_u16() == 405 || r.status().as_u16() == 404, "{method} on a detail: {}", r.status());
    }
    // a GET changes nothing
    let (_, again) = get(&server, PATH_LIST);
    assert_eq!(page, again);
}

#[test]
fn hostile_text_is_returned_as_plain_data_and_unknown_modules_stay_plain_ids() {
    let store = Arc::new(MemoryStore::new());
    let server = spawn(store.clone(), lax());
    let mut r = row(1, START, &caps(60));
    r.last_seen_at = START;
    r.listing.display_name = "<img src=x onerror=alert(1)>".into();
    r.listing.description = "<script>alert('x')</script>\n[link](javascript:alert(1)) &amp; ${jndi:ldap://x}".into();
    r.listing.modules = vec![ModuleEntry { id: "mystery-mod".into(), version: Some("9.9.9".into()), enabled: true }];
    store.put_for_test(r.clone());
    let resp = reqwest::blocking::get(format!("{}{PATH_LIST}", server.base)).unwrap();
    assert_eq!(resp.headers()["content-type"], "application/json", "never HTML");
    let page: serde_json::Value = resp.json().unwrap();
    assert_eq!(page["realms"][0]["display_name"], "<img src=x onerror=alert(1)>", "stored and returned as the text it is");
    assert!(page["realms"][0]["description"].as_str().unwrap().contains("<script>"));
    assert_eq!(page["realms"][0]["modules"][0]["id"], "mystery-mod");
}

#[test]
fn bad_queries_are_refused_with_a_reason() {
    let (_store, server) = fixture(10);
    for bad in ["limit=0", "limit=1000", "sort=ping", "ruleset=normal", "cap_min=999", "unknown=1", "q=a&q=b", "cursor=garbage", "status=banned", "module=%3Cb%3E"] {
        let (status, body) = get(&server, &format!("{PATH_LIST}?{bad}"));
        assert_eq!((status, body["error"]["code"].as_str()), (400, Some("malformed_request")), "{bad}: {body}");
    }
}

#[test]
fn answers_are_cached_briefly_and_carry_an_etag() {
    let store = Arc::new(MemoryStore::new());
    let cfg = ApiConfig { cache_secs: 30, ..lax() };
    let server = spawn(store.clone(), cfg);
    let caps = caps(60);
    let mut r = row(1, START, &caps);
    r.last_seen_at = START;
    store.put_for_test(r.clone());
    let http = reqwest::blocking::Client::new();
    let url = format!("{}{PATH_LIST}", server.base);
    let first = http.get(&url).send().unwrap();
    assert_eq!(first.headers()["cache-control"], "public, max-age=30");
    let etag = first.headers()["etag"].to_str().unwrap().to_string();
    let body1 = first.text().unwrap();
    // the database changes, the cached answer stays for its lifetime
    let mut r2 = r.clone();
    r2.listing.display_name = "Changed".into();
    store.put_for_test(r2);
    let second = http.get(&url).send().unwrap();
    assert_eq!(second.headers()["etag"].to_str().unwrap(), etag);
    assert_eq!(second.text().unwrap(), body1, "served from memory, the store was not asked again");
    // a revalidation costs nothing
    let not_modified = http.get(&url).header("if-none-match", &etag).send().unwrap();
    assert_eq!(not_modified.status().as_u16(), 304);
    assert!(not_modified.text().unwrap().is_empty());
    // another query is another entry
    assert!(http.get(format!("{url}?sort=name")).send().unwrap().headers()["etag"].to_str().unwrap().len() > 10);
}

#[test]
fn browsing_is_rate_limited_per_address() {
    let store = Arc::new(MemoryStore::new());
    let server = spawn(store, ApiConfig { browse: Rate { burst: 5.0, refill: 0.0 }, general: Rate { burst: 1000.0, refill: 0.0 }, cache_secs: 0, ..ApiConfig::default() });
    let codes: Vec<u16> = (0..8).map(|_| get(&server, PATH_LIST).0).collect();
    assert_eq!(codes.iter().filter(|c| **c == 200).count(), 5, "{codes:?}");
    assert!(codes.iter().filter(|c| **c == 429).count() == 3, "{codes:?}");
}

#[test]
#[ignore]
fn the_list_on_postgresql() {
    let Ok(url) = std::env::var("REGISTRY_TEST_PG") else { return };
    let parts: Vec<&str> = url.splitn(5, ':').collect();
    let cfg = coa_registry::pg::PgConfig { host: parts[0].into(), port: parts[1].parse().unwrap(), dbname: parts[2].into(), user: parts[3].into(), password: parts[4].into(), pool_size: 4, statement_timeout_ms: 5000 };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let store = rt.block_on(async {
        let s = coa_registry::pg::PgStore::connect(&cfg).await.unwrap();
        s.migrate().await.unwrap();
        s
    });
    let server = spawn(store, lax());
    let (status, page) = get(&server, &format!("{PATH_LIST}?limit=3&sort=name"));
    assert_eq!(status, 200, "{page}");
    std::mem::forget(rt);
}
