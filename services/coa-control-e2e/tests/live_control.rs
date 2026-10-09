//! Real gate for Phase 12 against the production VPS over HTTPS and WSS:
//! Host connects to Coordinator via WSS, Player connects to Host through Coordinator via WSS,
//! automatic provisioning, existing-account linking, and character claim.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_core::control::host_link::HostLink;
use coa_core::control::join::{PlayerControl, RealmTarget};
use coa_core::control::secrets::MemoryStore;
use coa_core::control::service::fake::FakeRealm;
use coa_core::control::service::HostService;
use coa_core::control::store::ControlStore;
use coa_core::control::transport::coordinator_url;
use coa_core::realm_registry::client::RegistryClient;
use coa_registry_proto::caps::*;
use coa_registry_proto::sign::encode_public_key;
use coa_registry_proto::*;
use ed25519_dalek::SigningKey;
use uuid::Uuid;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn random_key() -> SigningKey {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    SigningKey::from_bytes(&seed)
}

fn base_url() -> Option<String> {
    std::env::var("REGISTRY_URL").ok().filter(|u| !u.is_empty())
}

fn sample_caps(max_level: u32) -> AdvertisedCapabilities {
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

fn register_realm(client: &RegistryClient, realm: &RealmId, key: &SigningKey) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let listing = Listing {
        display_name: "Live Control Gate".into(),
        description: "TLS Phase 12 gate".into(),
        language: "en".into(),
        region: Some("EU".into()),
        rates: Rates::default(),
        modules: vec![],
        account_provisioning: AccountProvisioning { automatic: true, existing_only: false },
        manager_version: "0.6.7".into(),
    };
    let capabilities = sample_caps(60);
    let req = RegisterRequest {
        protocol_version: REGISTRY_PROTOCOL_VERSION,
        realm_id: *realm,
        public_key: encode_public_key(&key.verifying_key()),
        listing_hash: listing.hash(),
        listing,
        capabilities_hash: capabilities.advert_hash(),
        capabilities,
        population: Population { players: 0, bots: 0, capacity: None },
    };
    let ts = now();
    client.register(key, &req, ts)?;
    Ok(())
}

#[test]
#[ignore]
fn gate_phase12_live_vps_control_channel() {
    let Some(base) = base_url() else { return };

    let realm = RealmId::new();
    let host_key = random_key();
    let host_vkey = host_key.verifying_key();

    let client = RegistryClient::new(&base).expect("registry client");

    // 1. Register the realm in the production Registry over HTTPS
    register_realm(&client, &realm, &host_key).expect("register realm over HTTPS");
    println!("GATE 12 PASS  registered realm {realm} in Registry over HTTPS");

    // 2. Start HostService and HostLink over WSS
    let fake = FakeRealm::new(true);
    let legacy_account_id = fake.add_account("OLDHERO", "SECRET123");
    fake.add_char(legacy_account_id, 42, "LegacyChar", true);
    fake.at_select(legacy_account_id, true, 0);

    let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("srv-gate", realm, store, fake.clone()));

    let host_ws_url = coordinator_url(&base, "/coord/v1/host").expect("coordinator host url");
    assert!(host_ws_url.starts_with("wss://"), "Coordinator URL must use WSS: {host_ws_url}");

    let mut link = HostLink::start(host_ws_url.clone(), realm, host_key.clone(), service, Arc::new(now));

    // Wait for Host to connect to Coordinator
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !link.status().connected {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(link.status().connected, "Host failed to connect to Coordinator via WSS at {host_ws_url}: {:?}", link.status());
    println!("GATE 12 PASS  Coordinator Host connection established over WSS: {host_ws_url}");

    // 3. Player connects to Host via Coordinator over WSS
    let player_dir = tempfile::tempdir().unwrap();
    let pc = PlayerControl::open(player_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_ws_base = coordinator_url(&base, "/coord/v1/player").expect("coordinator player url");
    assert!(player_ws_base.starts_with("wss://"), "Player Coordinator URL must use WSS: {player_ws_base}");

    let target = RealmTarget {
        realm_id: realm,
        key: host_vkey,
        coordinator: player_ws_base,
    };

    let mut ch = pc.connect(&target).expect("Player connect via WSS");
    println!("GATE 12 PASS  Player -> Coordinator -> Host control channel established over WSS");

    let (auto, existing_only, route) = pc.welcome(&mut ch).expect("welcome handshake");
    assert!(auto);
    assert!(!existing_only);
    assert!(route.is_none());
    println!("GATE 12 PASS  handshake welcome received (auto={auto}, existing_only={existing_only})");

    // 4. Automatic account provisioning over WSS
    let outcome = pc.ensure_account(&mut ch, &realm, Some("GateWarrior")).expect("ensure_account");
    assert_eq!(outcome.username, "GATEWARRIOR");
    assert!(outcome.created);
    println!("GATE 12 PASS  automatic account provisioning succeeded: {}", outcome.username);

    // 5. A second player tests existing-account linking over WSS
    let player2_dir = tempfile::tempdir().unwrap();
    let pc2 = PlayerControl::open(player2_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let mut ch2 = pc2.connect(&target).expect("Player 2 connect via WSS");

    let linked = pc2.link_existing(&mut ch2, &realm, "OLDHERO", "SECRET123").expect("link_existing");
    assert_eq!(linked, "OLDHERO");
    println!("GATE 12 PASS  existing-account linking succeeded: {linked}");

    // 6. Character claim over WSS
    let chars = pc2.list_characters(&mut ch2).expect("list characters");
    assert_eq!(chars.len(), 1, "expected 1 character for OLDHERO");
    assert_eq!(chars[0].name, "LegacyChar");
    assert!(chars[0].eligible);

    let claimed = pc2.claim(&mut ch2, chars[0].token).expect("claim character");
    assert_eq!(claimed.payload, b"payload-42");
    pc2.acknowledge(&mut ch2, claimed.character_id, &realm, "LegacyChar").expect("acknowledge claim");
    println!("GATE 12 PASS  character claim and acknowledge succeeded: character_id={}", claimed.character_id);

    // 7. Clean up
    ch.close();
    ch2.close();
    link.stop();
    client.unpublish(&host_key, &realm, now()).expect("unpublish realm");
    println!("GATE 12 PASS  realm unpublished and links closed cleanly");
}
