//! The Coordinator end to end over real WebSockets: authentication of both sides, routing by connection number, limits, and that it forwards bytes it cannot read.

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use coa_control_proto::coord::{self, ErrorCode, Frame};
use coa_coordinator::listener::{LimitedListener, Peer};
use coa_coordinator::{router, Config, Hub, MemoryKeys};
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Server {
    addr: std::net::SocketAddr,
    keys: Arc<MemoryKeys>,
    hub: Arc<Hub<Arc<MemoryKeys>>>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn start(cfg: Config) -> Server {
    let keys = Arc::new(MemoryKeys::default());
    let hub = Hub::new(keys.clone(), cfg, Arc::new(now));
    let app = router(hub.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(tcp.local_addr().unwrap()).unwrap();
            axum::serve(LimitedListener::new(tcp, 64), app.into_make_service_with_connect_info::<Peer>()).await.unwrap();
        });
    });
    Server { addr: rx.recv().unwrap(), keys, hub }
}

fn quick() -> Config {
    Config { per_ip_connections: 64, per_ip: coa_coordinator::limits::Rate::per_minute(1000, 1000.0), per_realm: coa_coordinator::limits::Rate::per_minute(1000, 1000.0), ..Config::default() }
}

fn connect(s: &Server, path: &str) -> Ws {
    let (ws, _) = tungstenite::connect(format!("ws://{}{path}", s.addr)).expect("connect");
    if let MaybeTlsStream::Plain(t) = ws.get_ref() {
        t.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    }
    ws
}

fn text(ws: &mut Ws) -> Frame {
    loop {
        match ws.read().expect("a frame") {
            Message::Text(t) => return coord::parse_text(t.as_str()).expect("a valid frame"),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected text, got {other:?}"),
        }
    }
}

fn nonce(ws: &mut Ws) -> String {
    match text(ws) {
        Frame::Challenge { nonce, .. } => nonce,
        f => panic!("{f:?}"),
    }
}

fn send(ws: &mut Ws, f: &Frame) {
    ws.send(Message::Text(coord::to_text(f).into())).unwrap();
}

fn expect_error(ws: &mut Ws, code: ErrorCode) {
    match text(ws) {
        Frame::Error { code: c, .. } => assert_eq!(c, code),
        f => panic!("expected {code:?}, got {f:?}"),
    }
}

fn host(s: &Server, realm: RealmId, key: &SigningKey) -> Ws {
    s.keys.set(realm, &key.verifying_key());
    let mut ws = connect(s, "/coord/v1/host");
    let n = nonce(&mut ws);
    send(&mut ws, &coord::sign_host_hello(key, &realm, &n, now()));
    assert!(matches!(text(&mut ws), Frame::HostReady { .. }));
    ws
}

fn player(s: &Server, realm: RealmId, key: &SigningKey) -> Ws {
    let mut ws = connect(s, &format!("/coord/v1/player?realm={realm}"));
    let n = nonce(&mut ws);
    send(&mut ws, &coord::sign_player_hello(key, &Uuid::now_v7(), &realm, &n, now()));
    ws
}

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

#[test]
fn routes_both_ways_by_connection_number_and_never_interprets() {
    let s = start(quick());
    let (realm, hk) = (RealmId::new(), key(1));
    let mut h = host(&s, realm, &hk);
    let mut p1 = player(&s, realm, &key(10));
    let mut p2 = player(&s, realm, &key(11));
    let Frame::Open { conn: c1, .. } = text(&mut h) else { panic!("open 1") };
    let Frame::Open { conn: c2, .. } = text(&mut h) else { panic!("open 2") };
    assert_ne!(c1, c2);
    assert!(matches!(text(&mut p1), Frame::PlayerReady { conn, .. } if conn == c1));
    assert!(matches!(text(&mut p2), Frame::PlayerReady { conn, .. } if conn == c2));
    // bytes that are not a Noise message at all are forwarded untouched: the Coordinator has nothing to parse
    let blob = vec![0xA5u8; 300];
    p1.send(Message::Binary(blob.clone().into())).unwrap();
    p2.send(Message::Binary(b"second".to_vec().into())).unwrap();
    let mut got = std::collections::HashMap::new();
    for _ in 0..2 {
        match h.read().unwrap() {
            Message::Binary(b) => {
                let (c, f) = coord::split_binary(&b).unwrap();
                got.insert(c, f.to_vec());
            }
            m => panic!("{m:?}"),
        }
    }
    assert_eq!(got[&c1], blob);
    assert_eq!(got[&c2], b"second");
    h.send(Message::Binary(coord::binary(c2, b"to-two").into())).unwrap();
    h.send(Message::Binary(coord::binary(c1, b"to-one").into())).unwrap();
    assert_eq!(p1.read().unwrap(), Message::Binary(b"to-one".to_vec().into()));
    assert_eq!(p2.read().unwrap(), Message::Binary(b"to-two".to_vec().into()));
    // the player hangs up; the host is told
    p1.close(None).ok();
    loop {
        if let Frame::Close { conn, .. } = text(&mut h) {
            assert_eq!(conn, c1);
            break;
        }
    }
    // the host closes one; that player is told and the other is unaffected
    send(&mut h, &Frame::Close { conn: c2, reason: None });
    assert!(matches!(p2.read(), Ok(Message::Text(_) | Message::Close(_)) | Err(_)));
}

#[test]
fn a_host_must_hold_the_registered_key() {
    let s = start(quick());
    let realm = RealmId::new();
    s.keys.set(realm, &key(1).verifying_key());
    let mut ws = connect(&s, "/coord/v1/host");
    let n = nonce(&mut ws);
    send(&mut ws, &coord::sign_host_hello(&key(2), &realm, &n, now()));
    expect_error(&mut ws, ErrorCode::BadHello);

    let mut ws = connect(&s, "/coord/v1/host");
    let n = nonce(&mut ws);
    send(&mut ws, &coord::sign_host_hello(&key(1), &RealmId::new(), &n, now()));
    expect_error(&mut ws, ErrorCode::UnknownRealm);

    // a hello signed for another challenge is a replay
    let mut a = connect(&s, "/coord/v1/host");
    let na = nonce(&mut a);
    let hello = coord::sign_host_hello(&key(1), &realm, &na, now());
    let mut b = connect(&s, "/coord/v1/host");
    let _ = nonce(&mut b);
    send(&mut b, &hello);
    expect_error(&mut b, ErrorCode::BadHello);

    // a stale timestamp
    let mut c = connect(&s, "/coord/v1/host");
    let nc = nonce(&mut c);
    send(&mut c, &coord::sign_host_hello(&key(1), &realm, &nc, now() - 3600));
    expect_error(&mut c, ErrorCode::BadHello);
    assert_eq!(s.hub.connected_hosts(), 0);
}

#[test]
fn a_player_needs_a_signature_the_right_realm_and_an_online_host() {
    let s = start(quick());
    let realm = RealmId::new();
    // host offline
    let mut p = player(&s, realm, &key(5));
    expect_error(&mut p, ErrorCode::HostOffline);
    // a hello for another realm than the URL's
    let mut p = connect(&s, &format!("/coord/v1/player?realm={realm}"));
    let n = nonce(&mut p);
    send(&mut p, &coord::sign_player_hello(&key(5), &Uuid::now_v7(), &RealmId::new(), &n, now()));
    expect_error(&mut p, ErrorCode::BadHello);
    // a key that did not sign
    let mut p = connect(&s, &format!("/coord/v1/player?realm={realm}"));
    let n = nonce(&mut p);
    let Frame::PlayerHello { player_id, realm_id, ts, sig, .. } = coord::sign_player_hello(&key(5), &Uuid::now_v7(), &realm, &n, now()) else { unreachable!() };
    send(&mut p, &Frame::PlayerHello { player_id, public_key: coord::encode_public_key(&key(6).verifying_key()), realm_id, ts, sig });
    expect_error(&mut p, ErrorCode::BadHello);
    // garbage and the wrong frame
    let mut p = connect(&s, &format!("/coord/v1/player?realm={realm}"));
    let _ = nonce(&mut p);
    p.send(Message::Text("not json".into())).unwrap();
    assert!(!matches!(p.read(), Ok(Message::Text(t)) if matches!(coord::parse_text(t.as_str()), Ok(Frame::PlayerReady { .. }))));
    assert!(tungstenite::connect(format!("ws://{}/coord/v1/player?realm=nope", s.addr)).is_err());
}

#[test]
fn a_host_serves_a_bounded_number_of_players() {
    let s = start(Config { per_host_connections: 2, ..quick() });
    let realm = RealmId::new();
    let mut h = host(&s, realm, &key(1));
    let mut a = player(&s, realm, &key(2));
    let mut b = player(&s, realm, &key(3));
    assert!(matches!(text(&mut a), Frame::PlayerReady { .. }));
    assert!(matches!(text(&mut b), Frame::PlayerReady { .. }));
    let mut c = player(&s, realm, &key(4));
    expect_error(&mut c, ErrorCode::HostBusy);
    // when one leaves there is room again
    a.close(None).ok();
    loop {
        if matches!(text(&mut h), Frame::Close { .. }) {
            break;
        }
    }
    let mut d = player(&s, realm, &key(7));
    assert!(matches!(text(&mut d), Frame::PlayerReady { .. }));
}

#[test]
fn a_connection_that_exceeds_its_byte_budget_is_cut() {
    let s = start(Config { bytes_per_connection: 1000, ..quick() });
    let realm = RealmId::new();
    let mut h = host(&s, realm, &key(1));
    let mut p = player(&s, realm, &key(2));
    assert!(matches!(text(&mut p), Frame::PlayerReady { .. }));
    let Frame::Open { .. } = text(&mut h) else { panic!() };
    for _ in 0..5 {
        if p.send(Message::Binary(vec![1u8; 400].into())).is_err() {
            break;
        }
    }
    let mut cut = false;
    for _ in 0..10 {
        match p.read() {
            Ok(Message::Text(t)) if matches!(coord::parse_text(t.as_str()), Ok(Frame::Error { code: ErrorCode::TooLarge, .. })) => {
                cut = true;
                break;
            }
            Ok(Message::Close(_)) | Err(_) => {
                cut = true;
                break;
            }
            _ => {}
        }
    }
    assert!(cut, "the budget is enforced");
}

#[test]
fn an_oversized_frame_is_refused() {
    let s = start(quick());
    let realm = RealmId::new();
    let _h = host(&s, realm, &key(1));
    let mut p = player(&s, realm, &key(2));
    assert!(matches!(text(&mut p), Frame::PlayerReady { .. }));
    let _ = p.send(Message::Binary(vec![0u8; coord::MAX_FRAME_BYTES + 100].into()));
    let mut closed = false;
    for _ in 0..5 {
        match p.read() {
            Ok(Message::Close(_)) | Err(_) => {
                closed = true;
                break;
            }
            Ok(Message::Text(t)) if matches!(coord::parse_text(t.as_str()), Ok(Frame::Error { .. })) => {
                closed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(closed);
}

#[test]
fn a_reconnecting_host_replaces_the_old_connection_and_its_players_are_dropped() {
    let s = start(quick());
    let realm = RealmId::new();
    let mut h1 = host(&s, realm, &key(1));
    let mut p = player(&s, realm, &key(2));
    assert!(matches!(text(&mut p), Frame::PlayerReady { .. }));
    let Frame::Open { .. } = text(&mut h1) else { panic!() };
    let mut h2 = host(&s, realm, &key(1));
    // the old host connection ends, and its player is told the host went away
    assert!(h1.read().is_err() || matches!(h1.read(), Ok(Message::Close(_)) | Err(_)));
    let mut told = false;
    for _ in 0..5 {
        match p.read() {
            Ok(Message::Text(t)) if matches!(coord::parse_text(t.as_str()), Ok(Frame::Error { code: ErrorCode::HostOffline, .. })) => {
                told = true;
                break;
            }
            Ok(Message::Close(_)) | Err(_) => {
                told = true;
                break;
            }
            _ => {}
        }
    }
    assert!(told);
    // a new player reaches the new host
    let mut p2 = player(&s, realm, &key(3));
    assert!(matches!(text(&mut p2), Frame::PlayerReady { .. }));
    assert!(matches!(text(&mut h2), Frame::Open { .. }));
    assert_eq!(s.hub.connected_hosts(), 1);
}

#[test]
fn connections_per_address_and_per_realm_are_limited() {
    let s = start(Config { per_ip_connections: 3, ..quick() });
    let realm = RealmId::new();
    let _h = host(&s, realm, &key(1));
    let mut held = Vec::new();
    for i in 0..2 {
        let mut p = player(&s, realm, &key(20 + i));
        assert!(matches!(text(&mut p), Frame::PlayerReady { .. }));
        held.push(p);
    }
    // the host holds one, two players make three: the fourth from this address is refused at the upgrade
    let r = tungstenite::connect(format!("ws://{}/coord/v1/player?realm={realm}", s.addr));
    assert!(r.is_err(), "the per-address limit applies");

    let s = start(Config { per_realm: coa_coordinator::limits::Rate::per_minute(2, 0.0001), ..quick() });
    let realm = RealmId::new();
    let _h = host(&s, realm, &key(1));
    let mut ok = 0;
    let mut limited = 0;
    let mut keep = Vec::new();
    for i in 0..4 {
        let mut p = player(&s, realm, &key(30 + i));
        match text(&mut p) {
            Frame::PlayerReady { .. } => ok += 1,
            Frame::Error { code: ErrorCode::RateLimited, .. } => limited += 1,
            f => panic!("{f:?}"),
        }
        keep.push(p);
    }
    assert_eq!((ok, limited), (2, 2));
}

/// One HTTP response read until its body is complete (keep-alive: a server that closes right after answering can reset the connection on Windows before the client reads).
fn read_response(t: &mut TcpStream) -> String {
    use std::io::Read;
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = t.read(&mut buf).expect("a response");
        assert!(n > 0, "closed before a full response");
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data).into_owned();
        if let Some(split) = text.find("\r\n\r\n") {
            let len = text[..split].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
            if text.len() >= split + 4 + len {
                return text;
            }
        }
    }
}

#[test]
fn the_health_endpoint_answers_and_unknown_paths_do_not() {
    let s = start(quick());
    let mut t = TcpStream::connect(s.addr).unwrap();
    use std::io::Write;
    t.write_all(b"GET /coord/v1/health HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let out = read_response(&mut t);
    assert!(out.starts_with("HTTP/1.1 200") && out.contains("\"status\":\"ok\""), "{out}");
    let mut t = TcpStream::connect(s.addr).unwrap();
    t.write_all(b"GET /admin HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let out = read_response(&mut t);
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
}

#[test]
fn probe_endpoint_verifies_reachable_and_unreachable_ports() {
    let s = start(quick());
    let l1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p1 = l1.local_addr().unwrap().port();
    let p2 = 45999; // assumed free port

    use std::io::Write;
    let mut t = TcpStream::connect(s.addr).unwrap();
    let req = format!("GET /coord/v1/probe?ports={p1},{p2} HTTP/1.1\r\nHost: x\r\n\r\n");
    t.write_all(req.as_bytes()).unwrap();
    let out = read_response(&mut t);
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains(&format!("\"{p1}\":true")), "{out}");
    assert!(out.contains(&format!("\"{p2}\":false")), "{out}");
    assert!(out.contains("\"all_reachable\":false"), "{out}");

    // Now test with both listening
    let l2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p2_live = l2.local_addr().unwrap().port();
    let mut t2 = TcpStream::connect(s.addr).unwrap();
    let req2 = format!("GET /coord/v1/probe?ports={p1},{p2_live} HTTP/1.1\r\nHost: x\r\n\r\n");
    t2.write_all(req2.as_bytes()).unwrap();
    let out2 = read_response(&mut t2);
    assert!(out2.starts_with("HTTP/1.1 200"), "{out2}");
    assert!(out2.contains(&format!("\"{p1}\":true")), "{out2}");
    assert!(out2.contains(&format!("\"{p2_live}\":true")), "{out2}");
    assert!(out2.contains("\"all_reachable\":true"), "{out2}");
}
