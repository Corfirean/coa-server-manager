//! Host WebSocket tunnel handling and game TCP listener proxying.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use base64::Engine;
use coa_control_proto::relay::*;
use futures_util::SinkExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::hub::{Hub, KeyLookup};
use crate::limits::*;

pub async fn handle_host_websocket<K: KeyLookup>(mut ws: WebSocket, hub: Arc<Hub<K>>) {

    // 1. Send challenge
    let nonce = Uuid::new_v4().to_string();
    let challenge = RelayChallenge { nonce: nonce.clone() };
    let msg = Message::Text(serde_json::to_string(&challenge).unwrap().into());
    if ws.send(msg).await.is_err() {
        return;
    }

    // 2. Wait for HostHello
    let hello_res = tokio::time::timeout(HANDSHAKE_TIMEOUT, ws.recv()).await;
    let Ok(Some(Ok(Message::Text(hello_text)))) = hello_res else {
        tracing::warn!("handshake timed out or failed");
        return;
    };

    let Ok(hello) = serde_json::from_str::<HostHello>(&hello_text) else {
        tracing::warn!("invalid HostHello payload");
        return;
    };

    // 3. Verify signature
    let Some(key) = hub.keys.host_key(&hello.realm_id).await else {
        tracing::warn!(realm_id = %hello.realm_id, "unknown or unpublished realm");
        let _ = ws.send(Message::Text(serde_json::to_string(&RelayWelcome { ok: false, message: "unauthorized".into() }).unwrap().into())).await;
        let _ = ws.close().await;
        return;
    };

    if !verify_relay_challenge(&key, &hello.realm_id, &nonce, &hello.signature) {
        tracing::warn!(realm_id = %hello.realm_id, "invalid challenge signature");
        let _ = ws.send(Message::Text(serde_json::to_string(&RelayWelcome { ok: false, message: "invalid signature".into() }).unwrap().into())).await;
        let _ = ws.close().await;
        return;
    }

    // 4. Welcome host
    let welcome = RelayWelcome { ok: true, message: "welcome".into() };
    if ws.send(Message::Text(serde_json::to_string(&welcome).unwrap().into())).await.is_err() {
        return;
    }

    let realm_id = hello.realm_id;
    let (tx_to_host, mut rx_to_host) = mpsc::channel::<TunnelMsg>(MAX_STREAMS_PER_REALM * 4);
    hub.register_host(realm_id, tx_to_host.clone());

    // 5. Multiplex loop: handle messages between Host WS and Relay tasks
    let (mut ws_sink, mut ws_stream) = ws.split();

    // Outbound task: messages to host over WS
    let ws_send_task = tokio::spawn(async move {
        use futures_util::SinkExt;
        while let Some(msg) = rx_to_host.recv().await {
            let json = serde_json::to_string(&msg).unwrap();
            if ws_sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Inbound loop: messages from host over WS
    use futures_util::StreamExt;
    while let Some(Ok(msg)) = ws_stream.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(tunnel_msg) = serde_json::from_str::<TunnelMsg>(&text) else { continue };

        match tunnel_msg {
            TunnelMsg::Allocate { request_id, player_id, expected_client_ip } => {
                match hub.allocate(realm_id, player_id, expected_client_ip) {
                    Ok(alloc) => {
                        let token = alloc.token.clone();
                        let auth_port = alloc.auth_port;
                        let world_port = alloc.world_port;
                        let expires_at = alloc.expires_at;
                        let relay_host = hub.relay_host.clone();

                        // Spawn TCP listeners for the allocation
                        spawn_game_listeners(hub.clone(), alloc);

                        let reply = TunnelMsg::AllocateOk {
                            request_id,
                            token,
                            relay_host,
                            auth_port,
                            world_port,
                            expires_at,
                        };
                        let _ = tx_to_host.send(reply).await;
                    }
                    Err(err) => {
                        let reply = TunnelMsg::AllocateErr { request_id, error: err };
                        let _ = tx_to_host.send(reply).await;
                    }
                }
            }
            TunnelMsg::ConnectOk { stream_id } => {
                if let Some(tx) = hub.stream_tx(stream_id) {
                    let _ = tx.send(TunnelMsg::ConnectOk { stream_id }).await;
                }
            }
            TunnelMsg::ConnectErr { stream_id, error } => {
                if let Some(tx) = hub.stream_tx(stream_id) {
                    let _ = tx.send(TunnelMsg::ConnectErr { stream_id, error }).await;
                }
            }
            TunnelMsg::Data { stream_id, chunk } => {
                if let Some(tx) = hub.stream_tx(stream_id) {
                    let _ = tx.send(TunnelMsg::Data { stream_id, chunk }).await;
                }
            }
            TunnelMsg::Close { stream_id } => {
                if let Some(tx) = hub.stream_tx(stream_id) {
                    let _ = tx.send(TunnelMsg::Close { stream_id }).await;
                }
            }
            TunnelMsg::Reset { stream_id } => {
                if let Some(tx) = hub.stream_tx(stream_id) {
                    let _ = tx.send(TunnelMsg::Reset { stream_id }).await;
                }
            }
            TunnelMsg::Ping => {
                let _ = tx_to_host.send(TunnelMsg::Pong).await;
            }
            TunnelMsg::Pong => {}
            _ => {}
        }
    }

    ws_send_task.abort();
    hub.unregister_host(&realm_id);
}

fn spawn_game_listeners<K: KeyLookup>(hub: Arc<Hub<K>>, alloc: crate::hub::Allocation) {
    let hub_auth = hub.clone();
    let alloc_auth = alloc.clone();
    tokio::spawn(async move {
        listen_game_port(hub_auth, alloc_auth, RelayTarget::Auth).await;
    });

    let hub_world = hub.clone();
    let alloc_world = alloc.clone();
    tokio::spawn(async move {
        listen_game_port(hub_world, alloc_world, RelayTarget::World).await;
    });
}

async fn listen_game_port<K: KeyLookup>(hub: Arc<Hub<K>>, alloc: crate::hub::Allocation, target: RelayTarget) {
    let port = match target {
        RelayTarget::Auth => alloc.auth_port,
        RelayTarget::World => alloc.world_port,
    };

    let Ok(socket) = tokio::net::TcpSocket::new_v4() else {
        tracing::error!(port, ?target, "failed to create tcp socket");
        hub.release_port(port);
        return;
    };
    let _ = socket.set_reuseaddr(true);
    let Ok(_) = socket.bind(std::net::SocketAddr::from(([0, 0, 0, 0], port))) else {
        tracing::error!(port, ?target, "failed to bind relay game port");
        hub.release_port(port);
        return;
    };
    let Ok(listener) = socket.listen(1024) else {
        tracing::error!(port, ?target, "failed to listen on relay game port");
        hub.release_port(port);
        return;
    };

    tracing::info!(port, ?target, "relay game listener active");

    // Accept incoming connection within allocation window
    let mut accept_stream = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs((alloc.expires_at.saturating_sub((hub.clock)())).max(1) as u64);
    while tokio::time::Instant::now() < deadline {
        // If host disconnected, terminate listener immediately
        if hub.host_tx(&alloc.realm_id).is_none() {
            hub.release_port(port);
            return;
        }
        match tokio::time::timeout(Duration::from_millis(500), listener.accept()).await {
            Ok(Ok((stream, peer))) => {
                accept_stream = Some((stream, peer));
                break;
            }
            Ok(Err(e)) => {
                tracing::debug!(port, ?target, error = %e, "accept failed");
                hub.release_port(port);
                return;
            }
            Err(_) => {
                // Timeout on 500ms tick, loop and check host liveness
            }
        }
    }

    let Some((stream, peer)) = accept_stream else {
        tracing::debug!(port, ?target, "relay listener timed out or accept failed");
        hub.release_port(port);
        return;
    };

    // If an expected client IP was specified, verify source IP matches
    if let Some(expected_ip_str) = &alloc.expected_client_ip {
        if let Ok(expected_ip) = expected_ip_str.parse::<std::net::IpAddr>() {
            let peer_ip = peer.ip();
            let matches = match (expected_ip, peer_ip) {
                (std::net::IpAddr::V4(a), std::net::IpAddr::V4(b)) => a == b,
                (std::net::IpAddr::V6(a), std::net::IpAddr::V6(b)) => a == b,
                (std::net::IpAddr::V4(a), std::net::IpAddr::V6(b)) => b.to_ipv4_mapped() == Some(a),
                (std::net::IpAddr::V6(a), std::net::IpAddr::V4(b)) => a.to_ipv4_mapped() == Some(b),
            };
            if !matches {
                tracing::warn!(port, ?target, peer = %peer_ip, expected = %expected_ip, "rejected connection from unexpected client IP");
                drop(stream);
                hub.release_port(port);
                return;
            }
        }
    }

    let Some(host_tx) = hub.host_tx(&alloc.realm_id) else {
        tracing::warn!(realm_id = %alloc.realm_id, "host disconnected before game stream opened");
        hub.release_port(port);
        return;
    };

    let stream_id = hub.next_stream_id.fetch_add(1, Ordering::Relaxed);
    let (stream_tx, mut stream_rx) = mpsc::channel::<TunnelMsg>(STREAM_CHANNEL_CAPACITY);
    hub.register_stream(stream_id, stream_tx);

    // Send Connect request to Host
    let connect_req = TunnelMsg::Connect {
        stream_id,
        target,
        token: alloc.token.clone(),
    };
    if host_tx.send(connect_req).await.is_err() {
        hub.unregister_stream(stream_id);
        hub.release_port(port);
        return;
    }

    // Wait for ConnectOk
    let connect_ack = tokio::time::timeout(CONNECT_TIMEOUT, stream_rx.recv()).await;
    match connect_ack {
        Ok(Some(TunnelMsg::ConnectOk { .. })) => {
            tracing::info!(stream_id, port, ?target, "relay stream connected to host target");
        }
        _ => {
            tracing::warn!(stream_id, port, ?target, "connect to host target failed or timed out");
            hub.unregister_stream(stream_id);
            hub.release_port(port);
            return;
        }
    }

    // Proxy stream
    let (mut tcp_read, mut tcp_write) = stream.into_split();
    let host_tx_data = host_tx.clone();

    // Client TCP -> Host Tunnel
    let read_task = tokio::spawn(async move {
        let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => {
                    let _ = host_tx_data.send(TunnelMsg::Close { stream_id }).await;
                    break;
                }
                Ok(n) => {
                    let chunk = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                    if host_tx_data.send(TunnelMsg::Data { stream_id, chunk }).await.is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = host_tx_data.send(TunnelMsg::Reset { stream_id }).await;
                    break;
                }
            }
        }
    });

    // Host Tunnel -> Client TCP
    let relay_host = hub.relay_host.clone();
    let world_port = alloc.world_port;
    let write_task = tokio::spawn(async move {
        while let Some(msg) = stream_rx.recv().await {
            match msg {
                TunnelMsg::Data { chunk, .. } => {
                    let Ok(data) = base64::engine::general_purpose::STANDARD.decode(&chunk) else { continue };
                    let to_write = if target == RelayTarget::Auth {
                        // Check if REALM_LIST packet and rewrite realm address
                        match rewrite_realm_list_address(&data, &format!("{relay_host}:{world_port}")) {
                            Ok(Some(rewritten)) => rewritten,
                            _ => data,
                        }
                    } else {
                        data
                    };

                    if tcp_write.write_all(&to_write).await.is_err() {
                        break;
                    }
                }
                TunnelMsg::Close { .. } => {
                    let _ = tcp_write.shutdown().await;
                    break;
                }
                TunnelMsg::Reset { .. } => {
                    break;
                }
                _ => {}
            }
        }
    });

    let _ = tokio::join!(read_task, write_task);
    hub.unregister_stream(stream_id);
    hub.release_port(port);
    tracing::info!(stream_id, port, ?target, "relay stream finished and port released");
}
