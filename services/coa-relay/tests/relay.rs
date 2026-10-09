use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine;
use coa_control_proto::relay::*;

use coa_registry_proto::RealmId;
use coa_relay::hub::{Hub, MemoryKeys};
use coa_relay::router;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
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

struct TestServer {
    addr: SocketAddr,
    hub: Arc<Hub<Arc<MemoryKeys>>>,
    keys: Arc<MemoryKeys>,
}

async fn start_test_relay(min_port: u16, max_port: u16) -> TestServer {
    let keys = Arc::new(MemoryKeys::default());
    let hub = Arc::new(Hub::new(
        "127.0.0.1".into(),
        min_port,
        max_port,
        keys.clone(),
        Arc::new(now),
    ));
    let app = router(hub.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    TestServer { addr, hub, keys }
}

#[tokio::test]
async fn test_relay_health_endpoint() {
    let srv = start_test_relay(45000, 45010).await;
    let mut stream = TcpStream::connect(srv.addr).await.unwrap();
    stream.write_all(b"GET /relay/v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(resp.contains("HTTP/1.1 200 OK"));
    assert!(resp.contains("coa-relay"));
}

#[tokio::test]
async fn test_relay_host_auth_and_allocation_and_game_traffic() {
    let srv = start_test_relay(45100, 45110).await;
    let realm_id = RealmId::new();
    let signing_key = random_key();
    srv.keys.set(realm_id, &signing_key.verifying_key());

    // 1. Connect Host WebSocket to relay
    let ws_url = format!("ws://{}/relay/v1/host", srv.addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut ws_sink, mut ws_src) = ws_stream.split();

    // 2. Receive challenge
    let challenge_msg = ws_src.next().await.unwrap().unwrap();
    let challenge_txt = challenge_msg.to_text().unwrap();
    let challenge: RelayChallenge = serde_json::from_str(challenge_txt).unwrap();

    // 3. Send HostHello
    let sig = sign_relay_challenge(&signing_key, &realm_id, &challenge.nonce);
    let hello = HostHello { realm_id, signature: sig };
    ws_sink.send(Message::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();

    // 4. Receive welcome
    let welcome_msg = ws_src.next().await.unwrap().unwrap();
    let welcome: RelayWelcome = serde_json::from_str(welcome_msg.to_text().unwrap()).unwrap();
    assert!(welcome.ok, "Host welcome should be ok");

    // 5. Host asks for allocation
    let player_id = Uuid::new_v4();
    let alloc_req = TunnelMsg::Allocate { request_id: 1, player_id };
    ws_sink.send(Message::Text(serde_json::to_string(&alloc_req).unwrap().into())).await.unwrap();

    let alloc_msg = ws_src.next().await.unwrap().unwrap();
    let TunnelMsg::AllocateOk { request_id, auth_port, world_port, token, .. } = serde_json::from_str(alloc_msg.to_text().unwrap()).unwrap() else {
        panic!("expected AllocateOk");
    };
    assert_eq!(request_id, 1);
    assert_eq!(auth_port, 45100);
    assert_eq!(world_port, 45101);

    // 7. Client connects to Relay AUTH port

    let client_task = tokio::spawn(async move {
        let mut client_auth = TcpStream::connect(format!("127.0.0.1:{auth_port}")).await.unwrap();
        // Client sends auth packet
        client_auth.write_all(b"CLIENT_AUTH_HELLO").await.unwrap();

        // Read response (expected REALM_LIST rewritten)
        let mut resp = vec![0u8; 1024];
        let n = client_auth.read(&mut resp).await.unwrap();
        resp.truncate(n);
        resp
    });

    // 8. Host tunnel receives Connect for AUTH
    let connect_msg = ws_src.next().await.unwrap().unwrap();
    let TunnelMsg::Connect { stream_id, target, token: conn_token } = serde_json::from_str(connect_msg.to_text().unwrap()).unwrap() else {
        panic!("expected Connect");
    };
    assert_eq!(target, RelayTarget::Auth);
    assert_eq!(conn_token, token);

    // Host acknowledges connect
    ws_sink.send(Message::Text(serde_json::to_string(&TunnelMsg::ConnectOk { stream_id }).unwrap().into())).await.unwrap();


    // Relay forwards client data to Host
    let data_msg = ws_src.next().await.unwrap().unwrap();
    let TunnelMsg::Data { chunk, .. } = serde_json::from_str(data_msg.to_text().unwrap()).unwrap() else {
        panic!("expected Data");
    };
    let payload = base64::engine::general_purpose::STANDARD.decode(chunk).unwrap();
    assert_eq!(payload, b"CLIENT_AUTH_HELLO");

    // Local auth backend sends REALM_LIST packet
    let mut pkt = Vec::new();
    pkt.push(0x10); // Opcode
    pkt.extend_from_slice(&[0, 0]); // Length
    pkt.extend_from_slice(&[0, 0, 0, 0]); // Unused
    pkt.extend_from_slice(&1u16.to_le_bytes()); // Realm count
    pkt.push(1); pkt.push(0); pkt.push(0);
    pkt.extend_from_slice(b"CoA\0");
    pkt.extend_from_slice(b"127.0.0.1:8085\0"); // Old address
    pkt.extend_from_slice(&0.5f32.to_le_bytes());
    pkt.push(1); pkt.push(1); pkt.push(1);
    pkt.push(0x10); pkt.push(0x00);
    let body_len = (pkt.len() - 3) as u16;
    pkt[1..3].copy_from_slice(&body_len.to_le_bytes());

    // Host sends this packet through tunnel to Relay
    let b64_pkt = base64::engine::general_purpose::STANDARD.encode(&pkt);
    ws_sink.send(Message::Text(serde_json::to_string(&TunnelMsg::Data { stream_id, chunk: b64_pkt }).unwrap().into())).await.unwrap();

    // Client receives rewritten packet
    let client_recvd = client_task.await.unwrap();
    assert_eq!(client_recvd[0], 0x10);
    // Must contain rewritten address (127.0.0.1:45101)
    let expected_addr = format!("127.0.0.1:{world_port}").into_bytes();
    assert!(client_recvd.windows(expected_addr.len()).any(|w| w == expected_addr), "client received rewritten world address");

    // 9. Client connects to Relay WORLD port
    let client_world_task = tokio::spawn(async move {
        let mut client_world = TcpStream::connect(format!("127.0.0.1:{world_port}")).await.unwrap();
        client_world.write_all(b"PING_WORLD_PACKET").await.unwrap();
        let mut resp = [0u8; 17];
        client_world.read_exact(&mut resp).await.unwrap();
        resp
    });

    // Host receives Connect for WORLD (skipping any Close from stream 1)
    let (world_sid, world_target) = loop {
        let msg = ws_src.next().await.unwrap().unwrap();
        match serde_json::from_str::<TunnelMsg>(msg.to_text().unwrap()).unwrap() {
            TunnelMsg::Connect { stream_id, target, .. } => break (stream_id, target),
            TunnelMsg::Close { .. } => continue,
            other => panic!("unexpected msg awaiting Connect: {other:?}"),
        }
    };
    assert_eq!(world_target, RelayTarget::World);

    // Host acknowledges
    ws_sink.send(Message::Text(serde_json::to_string(&TunnelMsg::ConnectOk { stream_id: world_sid }).unwrap().into())).await.unwrap();

    // Host receives world data (skipping any Close from stream 1)
    let world_chunk = loop {
        let msg = ws_src.next().await.unwrap().unwrap();
        match serde_json::from_str::<TunnelMsg>(msg.to_text().unwrap()).unwrap() {
            TunnelMsg::Data { stream_id, chunk } if stream_id == world_sid => break chunk,
            TunnelMsg::Close { .. } => continue,
            other => panic!("unexpected msg awaiting Data: {other:?}"),
        }
    };
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(world_chunk).unwrap(), b"PING_WORLD_PACKET");


    // Host echoes back
    let pong_b64 = base64::engine::general_purpose::STANDARD.encode(b"PONG_WORLD_PACKET");
    ws_sink.send(Message::Text(serde_json::to_string(&TunnelMsg::Data { stream_id: world_sid, chunk: pong_b64 }).unwrap().into())).await.unwrap();

    let client_world_recvd = client_world_task.await.unwrap();
    assert_eq!(&client_world_recvd, b"PONG_WORLD_PACKET");
}

#[tokio::test]
async fn test_relay_rejects_unauthorized_host() {
    let srv = start_test_relay(45200, 45210).await;
    let realm_id = RealmId::new();
    let signing_key = random_key();
    // Do NOT register realm in keys

    let ws_url = format!("ws://{}/relay/v1/host", srv.addr);
    let (ws_stream, _) = connect_async(&ws_url).await.unwrap();
    let (mut ws_sink, mut ws_src) = ws_stream.split();

    let challenge_msg = ws_src.next().await.unwrap().unwrap();
    let challenge: RelayChallenge = serde_json::from_str(challenge_msg.to_text().unwrap()).unwrap();

    let sig = sign_relay_challenge(&signing_key, &realm_id, &challenge.nonce);
    let hello = HostHello { realm_id, signature: sig };
    ws_sink.send(Message::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();

    let welcome_msg = ws_src.next().await.unwrap().unwrap();
    let welcome: RelayWelcome = serde_json::from_str(welcome_msg.to_text().unwrap()).unwrap();
    assert!(!welcome.ok, "Unauthorized host must be rejected");
}

#[tokio::test]
async fn test_relay_allocation_bounds() {
    // Port pool only has 2 pairs (4 ports)
    let srv = start_test_relay(45300, 45303).await;
    let realm = RealmId::new();

    let alloc1 = srv.hub.allocate(realm, Uuid::new_v4()).unwrap();
    assert_eq!(alloc1.auth_port, 45300);
    assert_eq!(alloc1.world_port, 45301);

    let alloc2 = srv.hub.allocate(realm, Uuid::new_v4()).unwrap();
    assert_eq!(alloc2.auth_port, 45302);
    assert_eq!(alloc2.world_port, 45303);

    // Third allocation exhausts ports
    let alloc3 = srv.hub.allocate(realm, Uuid::new_v4());
    assert!(alloc3.is_err(), "must fail when ports exhausted");
}

