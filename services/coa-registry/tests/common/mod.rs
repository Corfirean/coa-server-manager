//! A Registry running in-process on an ephemeral port with a clock the test moves, and a signing client built from the protocol crate.
#![allow(dead_code)]

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use coa_registry::api::{router, ApiConfig, AppState};
use coa_registry::listener::Peer;
use coa_registry::store::Store;
use coa_registry_proto::caps::{AdvertisedCapabilities, CatalogEntry, CharacterFormats, ContentProfile, CoreIdentity, ExtensionSupport, Feature, FormatRange, Progression, Ruleset};
use coa_registry_proto::sign::{encode_public_key, sign_request};
use coa_registry_proto::*;
use ed25519_dalek::SigningKey;

pub struct TestServer {
    pub base: String,
    pub now: Arc<AtomicI64>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

pub const START: i64 = 1_790_000_000;

pub fn spawn<S: Store>(store: S, cfg: ApiConfig) -> TestServer {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let addr = std_listener.local_addr().unwrap();
    let now = Arc::new(AtomicI64::new(START));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(AppState::new(store, clock, cfg));
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            let app = router(state);
            let _ = axum::serve(listener, app.into_make_service_with_connect_info::<Peer>()).with_graceful_shutdown(async move { let _ = stopped.await; }).await;
        });
    });
    TestServer { base: format!("http://{addr}"), now, _stop: stop }
}

impl TestServer {
    pub fn set_time(&self, t: i64) {
        self.now.store(t, Ordering::SeqCst);
    }
    pub fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
    pub fn time(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}

pub fn caps(max_level: u32) -> AdvertisedCapabilities {
    let content = ContentProfile {
        ruleset: Ruleset::Coa,
        character_formats: CharacterFormats { readable: FormatRange { min: 1, max: 2 }, writable: FormatRange { min: 2, max: 2 } },
        online_import_job_formats: vec![2],
        session_protocol: 2,
        collection_protocol: 1,
        features: [Feature::RuntimeSessions, Feature::Wardrobe, Feature::LevelProjection].into_iter().collect(),
        collection_kinds: ["coa:appearance".to_string()].into_iter().collect(),
        extensions: vec![ExtensionSupport { namespace: "coa".into(), module_version: "1".into(), formats: FormatRange { min: 1, max: 1 } }],
        client_catalog: [("Appearances.dbc".to_string(), CatalogEntry { sha256: "a".repeat(64), records: 5 })].into_iter().collect(),
    };
    let content_profile_hash = content.hash();
    AdvertisedCapabilities {
        profile_version: 2,
        core: Some(CoreIdentity { commit: "abc123".into(), branch: "main".into(), date: "2026-10-01".into() }),
        content,
        content_profile_hash,
        progression: Some(Progression { max_player_level: max_level, projection_protocol: 1, projection_policy_version: 1, progression_signature: "b".repeat(64), scaling_enabled: false }),
    }
}

/// One realm's signing client.
pub struct Host {
    pub base: String,
    pub key: SigningKey,
    pub realm: RealmId,
    pub ts: i64,
    http: reqwest::blocking::Client,
}

fn uuid_bytes() -> [u8; 16] {
    *RealmId::new().as_uuid().as_bytes()
}

pub struct Reply {
    pub status: u16,
    pub body: serde_json::Value,
}

impl Reply {
    pub fn code(&self) -> String {
        self.body["error"]["code"].as_str().unwrap_or("").to_string()
    }
}

impl Host {
    pub fn new(server: &TestServer, seed: u8) -> Self {
        Self { base: server.base.clone(), key: SigningKey::from_bytes(&[seed; 32]), realm: RealmId::new(), ts: server.time(), http: reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(10)).build().unwrap() }
    }

    /// A client for a Registry that is not in this process (a real node): a random key, the real clock.
    pub fn remote(base: &str) -> Self {
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(uuid_bytes().as_slice());
        seed[16..].copy_from_slice(uuid_bytes().as_slice());
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        Self { base: base.trim_end_matches('/').to_string(), key: SigningKey::from_bytes(&seed), realm: RealmId::new(), ts, http: reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(20)).build().unwrap() }
    }

    /// Bring the timestamp up to the real clock (after a pause); it never goes backwards.
    pub fn sync_clock(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        self.ts = self.ts.max(now);
    }

    pub fn register_body(&self, name: &str, level: u32) -> RegisterRequest {
        let capabilities = caps(level);
        RegisterRequest {
            protocol_version: 1,
            realm_id: self.realm,
            public_key: encode_public_key(&self.key.verifying_key()),
            display_name: name.into(),
            description: "A test realm.".into(),
            language: "en".into(),
            ruleset: Ruleset::Coa,
            manager_version: "0.6.6".into(),
            capabilities_hash: capabilities.advert_hash(),
            capabilities,
            player_count: Some(1),
            player_capacity: Some(50),
        }
    }

    pub fn heartbeat_body(&self, level: u32, with_caps: bool) -> HeartbeatRequest {
        let capabilities = caps(level);
        HeartbeatRequest { protocol_version: 1, capabilities_hash: capabilities.advert_hash(), capabilities: with_caps.then_some(capabilities), manager_version: None, player_count: Some(2), player_capacity: Some(50), display_name: None, description: None, language: None }
    }

    /// A fully signed request; the timestamp advances by one for every call.
    pub fn send(&mut self, method: &str, path: &str, body: &[u8]) -> Reply {
        self.ts += 1;
        let headers = sign_request(&self.key, method, path, &self.realm, self.ts, body);
        self.send_with(method, path, body, &headers.pairs())
    }

    pub fn send_with(&self, method: &str, path: &str, body: &[u8], headers: &[(&str, String)]) -> Reply {
        let url = format!("{}{}", self.base, path);
        let mut req = match method {
            "GET" => self.http.get(&url),
            _ => self.http.post(&url).body(body.to_vec()),
        };
        for (n, v) in headers {
            req = req.header(*n, v);
        }
        let resp = req.send().expect("the Registry answers");
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(serde_json::Value::Null) }
    }

    pub fn register(&mut self, name: &str, level: u32) -> Reply {
        let body = serde_json::to_vec(&self.register_body(name, level)).unwrap();
        self.send("POST", PATH_REGISTER, &body)
    }

    pub fn heartbeat(&mut self, level: u32, with_caps: bool) -> Reply {
        let body = serde_json::to_vec(&self.heartbeat_body(level, with_caps)).unwrap();
        let path = path_heartbeat(&self.realm);
        self.send("POST", &path, &body)
    }

    pub fn unpublish(&mut self) -> Reply {
        let body = serde_json::to_vec(&UnpublishRequest { protocol_version: 1 }).unwrap();
        let path = path_unpublish(&self.realm);
        self.send("POST", &path, &body)
    }

    pub fn me(&mut self) -> Reply {
        let path = path_self(&self.realm);
        self.send("GET", &path, b"")
    }
}
