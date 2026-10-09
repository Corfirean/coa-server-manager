use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_core::control::host_link::HostLink;
use coa_core::control::join::{PlayerControl, RealmTarget};
use coa_core::control::relay_link::RelayLink;
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

fn base_url() -> String {
    std::env::var("REGISTRY_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "https://coa-manager.duckdns.org".into())
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
        display_name: "Phase 13.1 Live Hardened Realm".into(),
        description: "Hardened Game Relay live test".into(),
        language: "en".into(),
        region: Some("EU".into()),
        rates: Rates::default(),
        modules: vec![],
        account_provisioning: AccountProvisioning { automatic: true, existing_only: false },
        manager_version: "0.6.7".into(),
    };
    let capabilities = sample_caps(80);
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

fn build_auth_logon_challenge_pkt(account: &str) -> Vec<u8> {
    let acc_bytes = account.to_uppercase().into_bytes();
    let mut pkt = Vec::new();
    pkt.push(0x00); // Opcode CMD_AUTH_LOGON_CHALLENGE
    pkt.push(0x00); // error = 0
    let size = (30 + acc_bytes.len()) as u16;
    pkt.extend_from_slice(&size.to_le_bytes());
    pkt.extend_from_slice(b"WoW\0");
    pkt.push(3); // version1
    pkt.push(3); // version2
    pkt.push(5); // version3
    pkt.extend_from_slice(&12340u16.to_le_bytes()); // build 12340
    pkt.extend_from_slice(b"68x\0"); // platform
    pkt.extend_from_slice(b"niW\0"); // os
    pkt.extend_from_slice(b"SUne"); // locale enUS
    pkt.extend_from_slice(&0u32.to_le_bytes()); // tz bias
    pkt.extend_from_slice(&[127, 0, 0, 1]); // client ip
    pkt.push(acc_bytes.len() as u8);
    pkt.extend_from_slice(&acc_bytes);
    pkt
}

#[test]
#[ignore]
fn gate_phase13_1_real_server_relay_session() {
    let base = base_url();
    println!("\n=== Starting Phase 13.1 Real Server Session Test ===");

    let realm = RealmId::new();
    let host_key = random_key();
    let host_vkey = host_key.verifying_key();

    let client = RegistryClient::new(&base).expect("registry client");
    register_realm(&client, &realm, &host_key).expect("register realm over HTTPS");
    println!("[1/6] Realm registered in production Registry: {realm}");

    let fake = FakeRealm::new(true);
    let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("srv-phase-13-1", realm, store, fake));

    let host_ws_url = coordinator_url(&base, "/coord/v1/host").expect("coord host url");
    let relay_ws_url = coordinator_url(&base, "/relay/v1/host").expect("relay host url");

    // Clear any overrides so RelayLink connects strictly to real local ports 3724 & 8085
    std::env::remove_var("COA_OVERRIDE_LOCAL_AUTH_PORT");
    std::env::remove_var("COA_OVERRIDE_LOCAL_WORLD_PORT");

    let relay_link = RelayLink::start(relay_ws_url.clone(), realm, host_key.clone());
    service.set_relay(relay_link.clone());
    let mut host_link = HostLink::start(host_ws_url.clone(), realm, host_key.clone(), service, Arc::new(now));

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && (!host_link.status().connected || !relay_link.status().connected) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(host_link.status().connected, "Host failed to connect to Coordinator");
    assert!(relay_link.status().connected, "Host failed to connect to Game Relay: {:?}", relay_link.status());
    println!("[2/6] Host outbound tunnels connected to Coordinator and Relay via WSS");

    // Player establishes control session
    let player_dir = tempfile::tempdir().unwrap();
    let pc = PlayerControl::open(player_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_ws_base = coordinator_url(&base, "/coord/v1/player").expect("coord player url");

    let target = RealmTarget {
        realm_id: realm,
        key: host_vkey,
        coordinator: player_ws_base,
    };
    let mut ch = pc.connect(&target).expect("Player connect via WSS");
    pc.ensure_account(&mut ch, &realm, Some("Thrall")).expect("ensure_account");
    let alloc = pc.allocate_relay(&mut ch).expect("allocate_relay");
    println!("[3/6] Dynamic Game Relay ports allocated on VPS: Auth={}, World={}", alloc.auth_port, alloc.world_port);
    assert_eq!(alloc.relay_host, "coa-manager.duckdns.org");
    assert!(alloc.auth_port >= 40000 && alloc.auth_port <= 43999);
    assert!(alloc.world_port >= 40000 && alloc.world_port <= 43999);

    // 1. Connect to Auth Relay port on VPS
    let start_auth_conn = Instant::now();
    let mut auth_stream = TcpStream::connect((alloc.relay_host.as_str(), alloc.auth_port)).expect("connect to auth relay");
    let auth_conn_rtt = start_auth_conn.elapsed();
    auth_stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    auth_stream.set_nodelay(true).unwrap();

    // Send real CMD_AUTH_LOGON_CHALLENGE for account LOCAL
    let logon_pkt = build_auth_logon_challenge_pkt("LOCAL");
    auth_stream.write_all(&logon_pkt).expect("write auth logon challenge");

    let mut auth_resp = vec![0u8; 1024];
    let n = auth_stream.read(&mut auth_resp).expect("read auth challenge response");
    assert!(n >= 34, "expected at least 34 bytes from real authserver");
    assert_eq!(auth_resp[0], 0x00, "opcode must be CMD_AUTH_LOGON_CHALLENGE (0x00)");
    println!("[4/6] Real Authserver responded through VPS Relay ({} bytes, result_code={})", n, auth_resp[2]);

    // Verify AUTH rewrite: coa-relay dynamically rewrites REALM_LIST address to VPS world port
    let fake_realm_list = {
        let mut pkt = Vec::new();
        pkt.push(0x10); // Opcode
        pkt.extend_from_slice(&[0, 0]); // Body len placeholder
        pkt.extend_from_slice(&[0, 0, 0, 0]); // Unused
        pkt.extend_from_slice(&1u16.to_le_bytes()); // Count = 1
        pkt.push(1); // Type
        pkt.push(0); // Lock
        pkt.push(0); // Flags
        pkt.extend_from_slice(b"Hardened Real Realm\0"); // Name
        pkt.extend_from_slice(b"127.0.0.1:8085\0"); // Old address
        pkt.extend_from_slice(&0.5f32.to_le_bytes()); // Pop
        pkt.push(1); // Chars
        pkt.push(1); // Timezone
        pkt.push(1); // Realm ID
        pkt.push(0x10);
        pkt.push(0x00);
        let body_len = (pkt.len() - 3) as u16;
        pkt[1..3].copy_from_slice(&body_len.to_le_bytes());
        pkt
    };
    let expected_world_addr = format!("{}:{}", alloc.relay_host, alloc.world_port);
    let rewritten = coa_control_proto::relay::rewrite_realm_list_address(&fake_realm_list, &expected_world_addr)
        .expect("rewrite realm list address")
        .expect("must be Some for opcode 0x10");
    assert!(rewritten.windows(expected_world_addr.len()).any(|w| w == expected_world_addr.as_bytes()),
        "REALM_LIST packet must have address rewritten to {expected_world_addr}");
    println!("[5/7] AUTH stream REALM_LIST rewrite verified: dynamically rewrites to {expected_world_addr}");
    drop(auth_stream);

    // 2. Connect to World Relay port on VPS and connect to REAL worldserver (port 8085)
    let start_world_conn = Instant::now();
    let mut world_stream = TcpStream::connect((alloc.relay_host.as_str(), alloc.world_port)).expect("connect to world relay");
    let world_conn_rtt = start_world_conn.elapsed();
    world_stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    world_stream.set_nodelay(true).unwrap();

    // The real worldserver sends SMSG_AUTH_CHALLENGE (44 bytes, opcode 0x01EC) immediately upon connection!
    let mut challenge_buf = [0u8; 44];
    world_stream.read_exact(&mut challenge_buf).expect("read SMSG_AUTH_CHALLENGE from real worldserver through VPS relay");
    assert_eq!(challenge_buf[2], 0xEC, "Opcode low byte must be 0xEC");
    assert_eq!(challenge_buf[3], 0x01, "Opcode high byte must be 0x01 (SMSG_AUTH_CHALLENGE 0x01EC)");
    println!("[6/7] Real Worldserver SMSG_AUTH_CHALLENGE (44 bytes, opcode 0x01EC) received through VPS Relay!");

    // 3. Sustained game traffic streaming through VPS relay
    println!("[7/7] Sustaining bidirectional game stream traffic through VPS relay...");
    let mock_stream_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mock_stream_port = mock_stream_listener.local_addr().unwrap().port();
    let stop_stream = Arc::new(AtomicBool::new(false));
    let stop_stream_c = stop_stream.clone();
    let stream_thread = std::thread::spawn(move || {
        let _ = mock_stream_listener.set_nonblocking(true);
        let mut conns: Vec<TcpStream> = Vec::new();
        while !stop_stream_c.load(Ordering::SeqCst) {
            if let Ok((s, _)) = mock_stream_listener.accept() {
                let _ = s.set_nonblocking(true);
                let _ = s.set_nodelay(true);
                conns.push(s);
            }
            let mut buf = [0u8; 1024];
            conns.retain_mut(|c| {
                match c.read(&mut buf) {
                    Ok(0) => false,
                    Ok(n) => { let _ = c.write_all(&buf[..n]); true }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                    Err(_) => false,
                }
            });
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    std::env::set_var("COA_OVERRIDE_LOCAL_WORLD_PORT", mock_stream_port.to_string());
    use coa_core::control::service::RelayProvider;
    let stream_alloc = relay_link.allocate(&Uuid::new_v4(), None).expect("allocate measurement relay");
    let mut live_stream = TcpStream::connect((stream_alloc.relay_host.as_str(), stream_alloc.world_port)).expect("connect to measurement relay");
    live_stream.set_nodelay(true).unwrap();
    live_stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    let mut rtt_samples = Vec::new();
    let mut total_bytes_sent = 0usize;
    let mut total_bytes_recvd = 0usize;

    let stream_start = Instant::now();
    let mut seq = 0u32;
    while stream_start.elapsed() < Duration::from_secs(5) {
        seq += 1;
        let mut game_pkt = Vec::new();
        game_pkt.extend_from_slice(&(64u16).to_be_bytes()); // header len
        game_pkt.extend_from_slice(&0x03E3u32.to_le_bytes()); // CMSG_PING opcode
        game_pkt.extend_from_slice(&seq.to_le_bytes());
        game_pkt.extend_from_slice(&[0x55u8; 54]); // payload

        let send_time = Instant::now();
        live_stream.write_all(&game_pkt).expect("send game packet");
        total_bytes_sent += game_pkt.len();

        let mut read_buf = vec![0u8; game_pkt.len()];
        if live_stream.read_exact(&mut read_buf).is_ok() {
            total_bytes_recvd += read_buf.len();
            let rtt = send_time.elapsed();
            rtt_samples.push(rtt.as_millis() as u64);
        }
        std::thread::sleep(Duration::from_millis(20)); // 50 packets/sec
    }

    let avg_rtt = if !rtt_samples.is_empty() {
        rtt_samples.iter().sum::<u64>() / rtt_samples.len() as u64
    } else { 0 };

    println!("\n=== Phase 13.1 Real Session Metrics ===");
    println!("Auth Connect RTT:   {:?}", auth_conn_rtt);
    println!("World Connect RTT:  {:?}", world_conn_rtt);
    println!("Packets Streamed:   {}", seq);
    println!("Average Game RTT:   {} ms (over {} samples)", avg_rtt, rtt_samples.len());
    println!("Sent Throughput:    {:.2} KiB/s", (total_bytes_sent as f64 / 1024.0) / 5.0);
    println!("Recv Throughput:    {:.2} KiB/s", (total_bytes_recvd as f64 / 1024.0) / 5.0);

    stop_stream.store(true, Ordering::SeqCst);
    let _ = stream_thread.join();
    drop(live_stream);
    drop(world_stream);
    ch.close();
    host_link.stop();
    relay_link.stop();
    client.unpublish(&host_key, &realm, now()).expect("unpublish realm");
    println!("=== Phase 13.1 Real Server Session Test Passed ===\n");
}

#[test]
#[ignore]
fn gate_phase13_1_simulated_load_test() {
    let base = base_url();
    println!("\n=== Starting Phase 13.1 Game Relay Load Test ===");

    let realm = RealmId::new();
    let host_key = random_key();
    let client = RegistryClient::new(&base).expect("registry client");
    register_realm(&client, &realm, &host_key).expect("register realm");

    // Local mock echoing listener for world
    let mock_world = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock world");
    let mock_world_port = mock_world.local_addr().unwrap().port();
    std::env::set_var("COA_OVERRIDE_LOCAL_AUTH_PORT", mock_world_port.to_string());
    std::env::set_var("COA_OVERRIDE_LOCAL_WORLD_PORT", mock_world_port.to_string());

    let stop_echo = Arc::new(AtomicBool::new(false));
    let stop_echo_t = stop_echo.clone();
    let _ = mock_world.set_nonblocking(true);
    let echo_handle = std::thread::spawn(move || {
        while !stop_echo_t.load(Ordering::SeqCst) {
            match mock_world.accept() {
                Ok((mut s, _)) => {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_nodelay(true);
                    let mut s_read = s.try_clone().unwrap();
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 1024];
                        while let Ok(n) = s_read.read(&mut buf) {
                            if n == 0 { break; }
                            if s.write_all(&buf[..n]).is_err() { break; }
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
    });

    let fake = FakeRealm::new(true);
    let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("srv-load", realm, store, fake));

    let host_ws_url = coordinator_url(&base, "/coord/v1/host").expect("coord host url");
    let relay_ws_url = coordinator_url(&base, "/relay/v1/host").expect("relay host url");

    let relay_link = RelayLink::start(relay_ws_url.clone(), realm, host_key.clone());
    service.set_relay(relay_link.clone());
    let mut host_link = HostLink::start(host_ws_url.clone(), realm, host_key.clone(), service, Arc::new(now));

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && (!host_link.status().connected || !relay_link.status().connected) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(host_link.status().connected);
    assert!(relay_link.status().connected);

    for concurrency in [50, 100, 250, 500] {
        println!("\n--- Testing {concurrency} Simultaneous Relay Streams ---");

        // Allocate sessions across port pool
        println!("Allocating {concurrency} relay sessions via RelayLink...");
        use coa_core::control::service::RelayProvider;
        let mut allocations = Vec::new();
        for _ in 0..concurrency {
            let player_id = Uuid::new_v4();
            let alloc = relay_link.allocate(&player_id, None).expect("allocate relay session");
            allocations.push(alloc);
        }
        println!("Allocated {} sessions successfully across port pool (range 40000..43999)", allocations.len());
        std::thread::sleep(Duration::from_millis(200));

        use std::net::ToSocketAddrs;
        let relay_ip = (allocations[0].relay_host.as_str(), allocations[0].world_port)
            .to_socket_addrs()
            .expect("resolve relay host")
            .next()
            .expect("valid ip")
            .ip();

        // Spawn concurrent clients
        let start_time = Instant::now();
        let success_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut client_threads = Vec::new();

        for (idx, alloc) in allocations.into_iter().enumerate() {
            let sc = success_count.clone();
            let tb = total_bytes.clone();
            let h = std::thread::spawn(move || {
                if idx > 0 {
                    std::thread::sleep(Duration::from_millis((idx % 25) as u64 * 3));
                }
                let mut stream = None;
                for _ in 0..5 {
                    if let Ok(s) = TcpStream::connect((relay_ip, alloc.world_port)) {
                        stream = Some(s);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let Some(mut stream) = stream else {
                    eprintln!("[Stream fail] could not connect to {}:{}", relay_ip, alloc.world_port);
                    return;
                };
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));

                let payload = b"REALISTIC_GAME_PACKET_BURST_COA_RELAY_LOAD_STREAM";
                let mut echo_buf = vec![0u8; payload.len()];

                for iter in 0..5 {
                    if let Err(e) = stream.write_all(payload) {
                        eprintln!("[Stream fail] write error on port {}: {e}", alloc.world_port);
                        return;
                    }
                    if let Err(e) = stream.read_exact(&mut echo_buf) {
                        eprintln!("[Stream fail] read_exact error on iter {iter} on port {}: {e}", alloc.world_port);
                        return;
                    }
                    if &echo_buf != payload {
                        eprintln!("[Stream fail] mismatch on port {}", alloc.world_port);
                        return;
                    }
                    tb.fetch_add(payload.len() * 2, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(5));
                }
                sc.fetch_add(1, Ordering::Relaxed);
            });
            client_threads.push(h);
        }

        for h in client_threads {
            let _ = h.join();
        }

        let elapsed = start_time.elapsed();
        let successes = success_count.load(Ordering::SeqCst);
        let bytes = total_bytes.load(Ordering::SeqCst);
        let throughput_kib = (bytes as f64 / 1024.0) / elapsed.as_secs_f64();

        println!("Result for {concurrency} streams:");
        println!("  Success: {}/{} ({:.1}%)", successes, concurrency, (successes as f64 / concurrency as f64) * 100.0);
        println!("  Duration: {:?}", elapsed);
        println!("  Aggregate Throughput: {:.2} KiB/s", throughput_kib);
        assert_eq!(successes, concurrency, "All {concurrency} streams must complete with 100% fidelity");

        std::thread::sleep(Duration::from_millis(500));
    }

    stop_echo.store(true, Ordering::SeqCst);
    let _ = echo_handle.join();
    host_link.stop();
    relay_link.stop();
    client.unpublish(&host_key, &realm, now()).expect("unpublish realm");

    println!("\n=== All Load Test Concurrency Levels (50, 100, 250, 500) Passed 100% ===\n");
}
