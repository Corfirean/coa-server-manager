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

#[test]
#[ignore]
fn gate_phase12_1_live_vps_remote_transfer() {
    use std::collections::BTreeMap;
    use sha2::{Digest, Sha256};

    let Some(base) = base_url() else { return };

    let realm = RealmId::new();
    let host_key = random_key();
    let host_vkey = host_key.verifying_key();

    let client = RegistryClient::new(&base).expect("registry client");
    register_realm(&client, &realm, &host_key).expect("register realm over HTTPS");
    println!("GATE 12.1 PASS  registered realm {realm} in Registry over HTTPS");

    let fake = FakeRealm::new(true);
    let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("srv-gate-12-1", realm, store, fake.clone()));

    let host_ws_url = coordinator_url(&base, "/coord/v1/host").expect("coordinator host url");
    let mut link = HostLink::start(host_ws_url.clone(), realm, host_key.clone(), service, Arc::new(now));

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !link.status().connected {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(link.status().connected, "Host failed to connect to Coordinator via WSS");

    let player_dir = tempfile::tempdir().unwrap();
    let pc = PlayerControl::open(player_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_ws_base = coordinator_url(&base, "/coord/v1/player").expect("coordinator player url");

    let target = RealmTarget {
        realm_id: realm,
        key: host_vkey,
        coordinator: player_ws_base,
    };

    let mut ch = pc.connect(&target).expect("Player connect via WSS");
    pc.ensure_account(&mut ch, &realm, Some("VarianWrynn")).expect("ensure_account");

    let cid = Uuid::now_v7();
    let payload = b"REMOTE_PORTABLE_CHARACTER_VARIAN_PAYLOAD_V1".to_vec();
    let hash: [u8; 32] = Sha256::digest(&payload).into();
    let collections = BTreeMap::from([("coa:appearance".to_string(), vec![101, 202])]);

    // 1. Remote transfer over Coordinator WSS channel
    let outcome = pc.transfer_character(&mut ch, cid, 1, &payload, hash, collections.clone()).expect("transfer_character");
    assert!(outcome.local_guid > 0);
    assert_ne!(outcome.session_id, Uuid::nil());
    println!("GATE 12.1 PASS  remote transfer succeeded over WSS: local_guid={}, session_id={}", outcome.local_guid, outcome.session_id);

    // 2. Idempotent retry with same payload discovers existing commit
    let retry_outcome = pc.transfer_character(&mut ch, cid, 1, &payload, hash, collections.clone()).expect("retry transfer");
    assert_eq!(outcome.local_guid, retry_outcome.local_guid, "idempotent retry must return existing guid");
    assert_eq!(outcome.session_id, retry_outcome.session_id, "idempotent retry must return existing session");
    println!("GATE 12.1 PASS  idempotent retry returned existing commit without duplication");

    // 3. Update with newer revision 2 updates working copy without duplicating character
    let payload2 = b"REMOTE_PORTABLE_CHARACTER_VARIAN_PAYLOAD_V2".to_vec();
    let hash2: [u8; 32] = Sha256::digest(&payload2).into();
    let update_outcome = pc.transfer_character(&mut ch, cid, 2, &payload2, hash2, collections).expect("update transfer");
    assert_eq!(outcome.local_guid, update_outcome.local_guid, "revision update must preserve guid");
    println!("GATE 12.1 PASS  revision 2 update succeeded without character duplication");

    // 4. Incompatible realm rejects before mutation
    let incomp_id = Uuid::now_v7();
    fake.incompatible.lock().unwrap().insert(incomp_id);
    let err = pc.transfer_character(&mut ch, incomp_id, 1, b"incomp", [0u8; 32], BTreeMap::new()).expect_err("should reject");
    assert_eq!(err.code(), "incompatible");
    println!("GATE 12.1 PASS  incompatible character rejected cleanly");

    ch.close();
    link.stop();
    client.unpublish(&host_key, &realm, now()).expect("unpublish realm");
    println!("GATE 12.1 PASS  remote transfer gate passed and cleaned up");
}

#[test]
#[ignore]
fn gate_phase13_live_vps_relay() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use coa_core::control::relay_link::RelayLink;

    let Some(base) = base_url() else { return };

    let realm = RealmId::new();
    let host_key = random_key();
    let host_vkey = host_key.verifying_key();

    let client = RegistryClient::new(&base).expect("registry client");
    register_realm(&client, &realm, &host_key).expect("register realm over HTTPS");
    println!("GATE 13 PASS  registered realm {realm} in Registry over HTTPS");

    // Start local mock Auth server
    let mock_auth_listener = TcpListener::bind("127.0.0.1:0").expect("bind mock auth");
    let mock_auth_port = mock_auth_listener.local_addr().unwrap().port();

    // Start local mock World server
    let mock_world_listener = TcpListener::bind("127.0.0.1:0").expect("bind mock world");
    let mock_world_port = mock_world_listener.local_addr().unwrap().port();

    std::env::set_var("COA_OVERRIDE_LOCAL_AUTH_PORT", mock_auth_port.to_string());
    std::env::set_var("COA_OVERRIDE_LOCAL_WORLD_PORT", mock_world_port.to_string());

    // Spawn mock Auth worker: sends REALM_LIST packet when connection arrives
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = mock_auth_listener.accept() else { return };
        // Build mock REALM_LIST packet
        let mut pkt = Vec::new();
        pkt.push(0x10); // Opcode
        pkt.extend_from_slice(&[0, 0]); // Body len placeholder
        pkt.extend_from_slice(&[0, 0, 0, 0]); // Unused
        pkt.extend_from_slice(&1u16.to_le_bytes()); // Count = 1
        pkt.push(1); // Type
        pkt.push(0); // Lock
        pkt.push(0); // Flags
        pkt.extend_from_slice(b"Live Relay Realm\0"); // Name
        pkt.extend_from_slice(b"127.0.0.1:8085\0"); // Old address
        pkt.extend_from_slice(&0.5f32.to_le_bytes()); // Pop
        pkt.push(1); // Chars
        pkt.push(1); // Timezone
        pkt.push(1); // Realm ID
        pkt.push(0x10);
        pkt.push(0x00);
        let body_len = (pkt.len() - 3) as u16;
        pkt[1..3].copy_from_slice(&body_len.to_le_bytes());

        let _ = stream.write_all(&pkt);
        let mut buf = [0u8; 64];
        let _ = stream.read(&mut buf);
    });

    // Spawn mock World worker: echoes back received data
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = mock_world_listener.accept() else { return };
        let mut buf = [0u8; 128];
        if let Ok(n) = stream.read(&mut buf) {
            let _ = stream.write_all(&buf[..n]);
        }
    });

    let fake = FakeRealm::new(true);
    let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("srv-gate-13", realm, store, fake));

    let host_ws_url = coordinator_url(&base, "/coord/v1/host").expect("coordinator host url");
    let relay_ws_url = coordinator_url(&base, "/relay/v1/host").expect("relay host url");

    let relay_link = RelayLink::start(relay_ws_url.clone(), realm, host_key.clone());
    service.set_relay(relay_link.clone());

    let mut host_link = HostLink::start(host_ws_url.clone(), realm, host_key.clone(), service, Arc::new(now));

    // Wait for HostLink and RelayLink to connect
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && (!host_link.status().connected || !relay_link.status().connected) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(host_link.status().connected, "Host failed to connect to Coordinator");
    assert!(relay_link.status().connected, "Host failed to connect to Game Relay: {:?}", relay_link.status());
    println!("GATE 13 PASS  Host connected to Coordinator and Game Relay via outbound WSS");

    // Player connects
    let player_dir = tempfile::tempdir().unwrap();
    let pc = PlayerControl::open(player_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_ws_base = coordinator_url(&base, "/coord/v1/player").expect("coordinator player url");

    let target = RealmTarget {
        realm_id: realm,
        key: host_vkey,
        coordinator: player_ws_base,
    };

    let mut ch = pc.connect(&target).expect("Player connect via WSS");
    pc.ensure_account(&mut ch, &realm, Some("Thrall")).expect("ensure_account");

    // Allocate relay
    let alloc = pc.allocate_relay(&mut ch).expect("allocate_relay");
    println!("GATE 13 PASS  allocated relay: host={}, auth_port={}, world_port={}, token={}", alloc.relay_host, alloc.auth_port, alloc.world_port, alloc.token);
    assert_eq!(alloc.relay_host, "coa-manager.duckdns.org");
    assert!(alloc.auth_port >= 40000 && alloc.auth_port <= 40050);
    assert!(alloc.world_port >= 40000 && alloc.world_port <= 40050);
    assert_ne!(alloc.auth_port, alloc.world_port);

    // Test AUTH traffic through VPS Relay: verify REALM_LIST address rewrite
    let mut auth_client = TcpStream::connect((alloc.relay_host.as_str(), alloc.auth_port)).expect("connect to auth relay");
    auth_client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut pkt_buf = vec![0u8; 512];
    let n = auth_client.read(&mut pkt_buf).expect("read REALM_LIST response from auth relay");
    assert!(n > 10, "expected realm list packet");
    assert_eq!(pkt_buf[0], 0x10, "opcode must be 0x10 (REALM_LIST)");

    let expected_world_addr = format!("{}:{}", alloc.relay_host, alloc.world_port);
    assert!(pkt_buf[..n].windows(expected_world_addr.len()).any(|w| w == expected_world_addr.as_bytes()),
        "REALM_LIST packet must have address rewritten to {expected_world_addr}");
    println!("GATE 13 PASS  AUTH stream rewrote REALM_LIST address to VPS world port {expected_world_addr}");
    drop(auth_client);

    // Test WORLD traffic through VPS Relay: verify dumb byte tunneling
    let mut world_client = TcpStream::connect((alloc.relay_host.as_str(), alloc.world_port)).expect("connect to world relay");
    world_client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let test_payload = b"WORLD_TEST_PAYLOAD_FROM_CLIENT_THROUGH_VPS_RELAY";
    world_client.write_all(test_payload).expect("send data to world relay");

    let mut echo_buf = vec![0u8; test_payload.len()];
    world_client.read_exact(&mut echo_buf).expect("read echo from world relay");
    assert_eq!(&echo_buf, test_payload, "world relay must tunnel bytes faithfully without alteration");
    println!("GATE 13 PASS  WORLD stream tunneled game bytes faithfully end-to-end");
    drop(world_client);

    // Clean up
    ch.close();
    host_link.stop();
    relay_link.stop();
    client.unpublish(&host_key, &realm, now()).expect("unpublish realm");
    println!("GATE 13 PASS  game relay gate passed and cleaned up");
}

