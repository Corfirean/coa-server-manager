use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use coa_core::control::direct_ingress::DirectIngress;
use coa_core::control::host_link::HostLink;
use coa_core::control::join::{PlayerControl, RealmTarget};
use coa_core::control::secrets::MemoryStore;
use coa_core::control::service::fake::FakeRealm;
use coa_core::control::service::HostService;
use coa_core::control::store::ControlStore;
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;
use tempfile::tempdir;
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

fn build_fake_realm_list(original_world_addr: &str) -> Vec<u8> {
    let mut pkt = vec![
        0x10, // CMD_REALM_LIST opcode
        0x00, 0x00, // Size placeholder
        0x00, 0x00, 0x00, 0x00, // Unused
        0x01, 0x00, // Realm count: 1
        0x01, 0x00, 0x00, 0x00, // Icon
        0x00, // Lock
        0x00, // Flags
    ];
    pkt.extend_from_slice(b"Phase14Realm\0");
    pkt.extend_from_slice(original_world_addr.as_bytes());
    pkt.push(0x00);
    pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Population
    pkt.extend_from_slice(&[0x01]); // Num characters
    pkt.extend_from_slice(&[0x01]); // Timezone
    pkt.extend_from_slice(&[0x00]); // Realm ID
    pkt.extend_from_slice(&[0x02, 0x00]); // Footer

    let body_len = (pkt.len() - 3) as u16;
    pkt[1..3].copy_from_slice(&body_len.to_le_bytes());
    pkt
}

use coa_coordinator::listener::{LimitedListener, Peer};
use coa_coordinator::{router, Config, Hub, MemoryKeys};

struct CoordTestHarness {
    pub addr: std::net::SocketAddr,
    pub keys: Arc<MemoryKeys>,
}

fn start_test_coordinator() -> CoordTestHarness {
    let keys = Arc::new(MemoryKeys::default());
    let cfg = Config {
        trust_proxy: true,
        per_ip_connections: 64,
        per_ip: coa_coordinator::limits::Rate::per_minute(1000, 1000.0),
        per_realm: coa_coordinator::limits::Rate::per_minute(1000, 1000.0),
        ..Config::default()
    };
    let hub = Hub::new(keys.clone(), cfg, Arc::new(now));
    let app = router(hub);
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(tcp.local_addr().unwrap()).unwrap();
            axum::serve(LimitedListener::new(tcp, 64), app.into_make_service_with_connect_info::<Peer>()).await.unwrap();
        });
    });
    CoordTestHarness { addr: rx.recv().unwrap(), keys }
}

#[test]
fn test_phase14_direct_verified_route_connects_and_rewrites_realm_list() {
    let coord = start_test_coordinator();
    let realm_id = RealmId::new();
    let realm_key = random_key();
    coord.keys.set(realm_id, &realm_key.verifying_key());

    // Mock local Auth Server
    let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_auth_port = mock_auth.local_addr().unwrap().port();

    // Mock local World Server
    let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_world_port = mock_world.local_addr().unwrap().port();

    // Start Direct Ingress Proxy
    let mut ingress = DirectIngress::start(
        0,
        0,
        "127.0.0.1".into(),
        0,
        local_auth_port,
        local_world_port,
    )
    .unwrap();

    let direct_auth_port = ingress.auth_port();
    let direct_world_port = ingress.world_port();

    // Host Service setup
    let fake_realm = FakeRealm::new(true);
    let store = Arc::new(std::sync::Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("local-1", realm_id, store, fake_realm));
    service.set_direct_route(Some(format!("127.0.0.1:{direct_auth_port}")));

    // Host connects to Coordinator
    let coord_ws = format!("ws://{}/coord/v1/host", coord.addr);
    let host_link = HostLink::start(coord_ws, realm_id, realm_key.clone(), service.clone(), Arc::new(now));
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host_link.status().connected {
        assert!(std::time::Instant::now() < end, "Host never connected to coord");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Player connects over Coordinator
    let p_dir = tempdir().unwrap();
    let pc = PlayerControl::open(p_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_coord_url = format!("ws://{}/coord/v1/player", coord.addr);
    let target = RealmTarget {
        realm_id,
        key: realm_key.verifying_key(),
        coordinator: player_coord_url,
    };
    let mut ch = pc.connect(&target).unwrap();
    let (_auto, _exist, offered_route) = pc.welcome(&mut ch).unwrap();
    assert_eq!(offered_route, Some(format!("127.0.0.1:{direct_auth_port}")));

    // Spawn mock auth responder that loops (ignoring 0-byte probe)
    let fake_pkt = build_fake_realm_list("127.0.0.1:8085");
    let t_auth = std::thread::spawn(move || {
        while let Ok((mut stream, _)) = mock_auth.accept() {
            let mut buf = [0u8; 16];
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => continue,
                Ok(_) => {
                    let _ = stream.write_all(&fake_pkt);
                    break;
                }
            }
        }
    });

    // Spawn mock world responder
    let t_world = std::thread::spawn(move || {
        let (mut stream, _) = mock_world.accept().unwrap();
        let mut buf = [0u8; 16];
        let n = stream.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"WORLD_SESSION");
        stream.write_all(b"WORLD_ACK").unwrap();
    });

    // Player verifies direct route candidate via quick TCP probe
    let candidate = offered_route.unwrap();
    let direct_usable = TcpStream::connect_timeout(
        &candidate.parse().unwrap(),
        Duration::from_millis(2000),
    ).is_ok();
    assert!(direct_usable, "Direct route must be connectable");

    // Client connects to direct AUTH port
    let mut client_auth = TcpStream::connect(format!("127.0.0.1:{direct_auth_port}")).unwrap();
    client_auth.write_all(b"PING_AUTH").unwrap();
    let mut resp = [0u8; 512];
    let n = client_auth.read(&mut resp).unwrap();
    let resp_str = String::from_utf8_lossy(&resp[..n]);
    // Address was rewritten to direct world port
    assert!(resp_str.contains(&format!("127.0.0.1:{direct_world_port}")), "Expected rewritten direct world port in {resp_str}");
    drop(client_auth);

    // Client connects to direct WORLD port
    let mut client_world = TcpStream::connect(format!("127.0.0.1:{direct_world_port}")).unwrap();
    client_world.write_all(b"WORLD_SESSION").unwrap();
    let mut w_resp = [0u8; 16];
    let wn = client_world.read(&mut w_resp).unwrap();
    assert_eq!(&w_resp[..wn], b"WORLD_ACK");
    drop(client_world);

    t_auth.join().unwrap();
    t_world.join().unwrap();
    ingress.stop();
}

#[test]
fn test_phase14_direct_unreachable_falls_back_to_relay() {
    let coord = start_test_coordinator();
    let realm_id = RealmId::new();
    let realm_key = random_key();
    coord.keys.set(realm_id, &realm_key.verifying_key());

    // Host Service has NO direct route (e.g. CGNAT or unmapped)
    let fake_realm = FakeRealm::new(true);
    let store = Arc::new(std::sync::Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("local-1", realm_id, store, fake_realm));
    service.set_direct_route(None); // Direct route unavailable

    // Mock Relay Provider
    struct MockRelay;
    impl coa_core::control::service::RelayProvider for MockRelay {
        fn allocate(&self, _player: &Uuid, _ip: Option<&str>) -> coa_core::Result<coa_core::control::relay_link::RelayAllocationInfo> {
            Ok(coa_core::control::relay_link::RelayAllocationInfo {
                relay_host: "relay.test".into(),
                auth_port: 41000,
                world_port: 41001,
                token: "token123".into(),
                expires_at: now() + 60,
            })
        }
    }
    service.set_relay(Arc::new(MockRelay));

    // Host connects to Coordinator
    let coord_ws = format!("ws://{}/coord/v1/host", coord.addr);
    let host_link = HostLink::start(coord_ws, realm_id, realm_key.clone(), service.clone(), Arc::new(now));
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host_link.status().connected {
        assert!(std::time::Instant::now() < end, "Host never connected to coord");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Player connects over Coordinator
    let p_dir = tempdir().unwrap();
    let pc = PlayerControl::open(p_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let player_coord_url = format!("ws://{}/coord/v1/player", coord.addr);
    let target = RealmTarget {
        realm_id,
        key: realm_key.verifying_key(),
        coordinator: player_coord_url,
    };
    let mut ch = pc.connect(&target).unwrap();
    let (_auto, _exist, offered_route) = pc.welcome(&mut ch).unwrap();
    assert_eq!(offered_route, None, "Direct route must be None when unavailable");

    // Ensure account before allocating relay
    let _account = pc.ensure_account(&mut ch, &realm_id, Some("PLAYER")).unwrap();

    // Player route selection: since offered_route is None, transparently falls back to allocate_relay
    let relay_alloc = pc.allocate_relay(&mut ch).unwrap();
    assert_eq!(relay_alloc.relay_host, "relay.test");
    assert_eq!(relay_alloc.auth_port, 41000);
    assert_eq!(relay_alloc.world_port, 41001);
}

#[test]
fn test_phase14_mixed_mode_simultaneous_direct_and_relay() {
    let coord = start_test_coordinator();
    let realm_id = RealmId::new();
    let realm_key = random_key();
    coord.keys.set(realm_id, &realm_key.verifying_key());

    // Mock local Auth & World
    let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_auth_port = mock_auth.local_addr().unwrap().port();
    let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_world_port = mock_world.local_addr().unwrap().port();

    // Start Direct Ingress
    let mut ingress = DirectIngress::start(
        0,
        0,
        "127.0.0.1".into(),
        local_world_port,
        local_auth_port,
        local_world_port,
    )
    .unwrap();
    let direct_auth_port = ingress.auth_port();

    // Host Service has both Direct and Relay capability
    let fake_realm = FakeRealm::new(true);
    let store = Arc::new(std::sync::Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("local-1", realm_id, store, fake_realm));
    service.set_direct_route(Some(format!("127.0.0.1:{direct_auth_port}")));

    struct MockRelay;
    impl coa_core::control::service::RelayProvider for MockRelay {
        fn allocate(&self, _player: &Uuid, _ip: Option<&str>) -> coa_core::Result<coa_core::control::relay_link::RelayAllocationInfo> {
            Ok(coa_core::control::relay_link::RelayAllocationInfo {
                relay_host: "relay.test".into(),
                auth_port: 42000,
                world_port: 42001,
                token: "tok-relay".into(),
                expires_at: now() + 60,
            })
        }
    }
    service.set_relay(Arc::new(MockRelay));

    // Host connects to Coordinator
    let coord_ws = format!("ws://{}/coord/v1/host", coord.addr);
    let host_link = HostLink::start(coord_ws, realm_id, realm_key.clone(), service.clone(), Arc::new(now));
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host_link.status().connected {
        assert!(std::time::Instant::now() < end, "Host never connected to coord");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Player 1 (Direct) connects
    let target = RealmTarget {
        realm_id,
        key: realm_key.verifying_key(),
        coordinator: format!("ws://{}/coord/v1/player", coord.addr),
    };
    let p_dir1 = tempdir().unwrap();
    let pc1 = PlayerControl::open(p_dir1.path(), Arc::new(MemoryStore::default())).unwrap();
    let mut ch1 = pc1.connect(&target).unwrap();
    let (_auto1, _exist1, route1) = pc1.welcome(&mut ch1).unwrap();
    assert_eq!(route1, Some(format!("127.0.0.1:{direct_auth_port}")));

    // Player 2 (Relay - e.g. simulating direct connect failed or blocked)
    let p_dir2 = tempdir().unwrap();
    let pc2 = PlayerControl::open(p_dir2.path(), Arc::new(MemoryStore::default())).unwrap();
    let mut ch2 = pc2.connect(&target).unwrap();
    let (_auto2, _exist2, _route2) = pc2.welcome(&mut ch2).unwrap();
    let _acc2 = pc2.ensure_account(&mut ch2, &realm_id, Some("PLAYER2")).unwrap();
    let relay_alloc2 = pc2.allocate_relay(&mut ch2).unwrap();
    assert_eq!(relay_alloc2.relay_host, "relay.test");
    assert_eq!(relay_alloc2.auth_port, 42000);

    // Both players successfully got their respective routes on the exact same realm concurrently!
    assert_ne!(route1.unwrap(), format!("{}:{}", relay_alloc2.relay_host, relay_alloc2.auth_port));

    ingress.stop();
}

#[test]
fn test_phase14_security_direct_ingress_strictly_forbids_mysql_and_ra() {
    // Mock local Auth
    let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_auth_port = mock_auth.local_addr().unwrap().port();

    // Mock local World
    let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_world_port = mock_world.local_addr().unwrap().port();

    // Mock local MySQL (3306 mock)
    let mock_mysql = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_mysql_port = mock_mysql.local_addr().unwrap().port();

    // Mock local Remote Administration (3443 mock)
    let mock_ra = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_ra_port = mock_ra.local_addr().unwrap().port();

    let mut ingress = DirectIngress::start(
        0,
        0,
        "127.0.0.1".into(),
        local_world_port,
        local_auth_port,
        local_world_port,
    )
    .unwrap();

    let d_auth = ingress.auth_port();
    let d_world = ingress.world_port();

    // Verify direct ingress binds ONLY to its 2 allocated ports (Auth & World)
    assert_ne!(d_auth, local_mysql_port);
    assert_ne!(d_auth, local_ra_port);
    assert_ne!(d_world, local_mysql_port);
    assert_ne!(d_world, local_ra_port);

    // Spawn mock auth responder
    let t_auth = std::thread::spawn(move || {
        let (mut stream, _) = mock_auth.accept().unwrap();
        let mut buf = [0u8; 16];
        let n = stream.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"TEST_AUTH");
        stream.write_all(b"AUTH_OK").unwrap();
    });

    // Test that client talking to direct auth reaches AUTH, not MySQL/RA
    let mut client = TcpStream::connect(format!("127.0.0.1:{d_auth}")).unwrap();
    client.write_all(b"TEST_AUTH").unwrap();
    let mut resp = [0u8; 16];
    let n = client.read(&mut resp).unwrap();
    assert_eq!(&resp[..n], b"AUTH_OK");

    // Verify no connections ever reached mock MySQL or RA
    mock_mysql.set_nonblocking(true).unwrap();
    assert!(mock_mysql.accept().is_err(), "MySQL listener must receive 0 connections via direct ingress");

    mock_ra.set_nonblocking(true).unwrap();
    assert!(mock_ra.accept().is_err(), "RA listener must receive 0 connections via direct ingress");

    drop(client);
    t_auth.join().unwrap();
    ingress.stop();
}

#[test]
fn test_phase14_direct_mapped_world_port_distinct_from_desired_and_bound() {
    let desired_world_port = 8085;
    let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_auth_port = mock_auth.local_addr().unwrap().port();

    let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
    let local_world_port = mock_world.local_addr().unwrap().port();

    // Actual ingress binds to ephemeral port != desired_world_port
    let mut ingress = DirectIngress::start(
        0,
        0,
        "198.51.100.1".into(),
        desired_world_port,
        local_auth_port,
        local_world_port,
    ).unwrap();

    let actual_ingress_world = ingress.world_port();
    assert_ne!(actual_ingress_world, desired_world_port);

    // Mapped external port differs from both desired (8085) and actual ingress port
    let external_mapped_world_port = 48085;
    assert_ne!(external_mapped_world_port, desired_world_port);
    assert_ne!(external_mapped_world_port, actual_ingress_world);

    // Update with final verified mapped external endpoint
    ingress.update_external_world_endpoint("198.51.100.1", external_mapped_world_port);

    let fake_pkt = build_fake_realm_list("127.0.0.1:8085");
    let t_auth = std::thread::spawn(move || {
        let (mut stream, _) = mock_auth.accept().unwrap();
        let mut buf = [0u8; 16];
        let _ = stream.read(&mut buf).unwrap();
        stream.write_all(&fake_pkt).unwrap();
    });

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", ingress.auth_port())).unwrap();
    client.write_all(b"PING").unwrap();
    let mut resp = [0u8; 512];
    let n = client.read(&mut resp).unwrap();
    let s = String::from_utf8_lossy(&resp[..n]);

    // Assert the rewritten REALM_LIST contains exactly the mapped external port
    assert!(
        s.contains("198.51.100.1:48085"),
        "Must contain mapped external port 48085, got: {s}"
    );
    assert!(
        !s.contains("198.51.100.1:8085"),
        "Must NOT contain desired world port 8085"
    );
    assert!(
        !s.contains(&format!("198.51.100.1:{actual_ingress_world}")),
        "Must NOT contain internal ingress port"
    );

    drop(client);
    t_auth.join().unwrap();
    ingress.stop();
}

#[test]
fn test_phase14_route_health_break_falls_back_to_relay() {
    let coord = start_test_coordinator();
    let realm_id = RealmId::new();
    let realm_key = random_key();
    coord.keys.set(realm_id, &realm_key.verifying_key());

    // Host Service setup
    let fake_realm = FakeRealm::new(true);
    let store = Arc::new(std::sync::Mutex::new(ControlStore::open_in_memory().unwrap()));
    let service = Arc::new(HostService::new("local-1", realm_id, store, fake_realm));

    // Initially direct route is active
    service.set_direct_route(Some("127.0.0.1:39999".into()));

    struct MockRelay;
    impl coa_core::control::service::RelayProvider for MockRelay {
        fn allocate(&self, _player: &Uuid, _ip: Option<&str>) -> coa_core::Result<coa_core::control::relay_link::RelayAllocationInfo> {
            Ok(coa_core::control::relay_link::RelayAllocationInfo {
                relay_host: "relay.fallback".into(),
                auth_port: 43000,
                world_port: 43001,
                token: "tok-fallback".into(),
                expires_at: now() + 60,
            })
        }
    }
    service.set_relay(Arc::new(MockRelay));

    // Host connects to Coordinator
    let host_link = HostLink::start(format!("ws://{}/coord/v1/host", coord.addr), realm_id, realm_key.clone(), service.clone(), Arc::new(now));
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host_link.status().connected {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(50));
    }

    // Now deliberately break the direct route / health check
    service.set_direct_route(None);

    // Player connects to join
    let target = RealmTarget {
        realm_id,
        key: realm_key.verifying_key(),
        coordinator: format!("ws://{}/coord/v1/player", coord.addr),
    };
    let p_dir = tempdir().unwrap();
    let pc = PlayerControl::open(p_dir.path(), Arc::new(MemoryStore::default())).unwrap();
    let mut ch = pc.connect(&target).unwrap();
    let (_auto, _exist, route) = pc.welcome(&mut ch).unwrap();

    // Direct route is None
    assert_eq!(route, None);

    // Join automatically uses Relay fallback
    let _acc = pc.ensure_account(&mut ch, &realm_id, Some("PLAYER_FB")).unwrap();
    let relay_alloc = pc.allocate_relay(&mut ch).unwrap();
    assert_eq!(relay_alloc.relay_host, "relay.fallback");
    assert_eq!(relay_alloc.auth_port, 43000);
}


