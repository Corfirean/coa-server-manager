//! Outbound Host <-> Game Relay tunnel link (Phase 13).
//! Maintains outbound connectivity from Host Manager to the Game Relay,
//! opening local TCP connections only to the installed realm's configured loopback Auth and World ports.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use coa_control_proto::relay::*;
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;
use serde::Serialize;
use tungstenite::Message;
use uuid::Uuid;

use super::service::RelayProvider;
use super::transport::{self, LinkError};

const POLL: Duration = Duration::from_millis(2);

#[derive(Clone, Debug, Default, Serialize)]
pub struct RelayStatus {
    pub connected: bool,
    pub last_error: Option<String>,
    pub active_streams: usize,
}

pub struct RelayLink {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<RelayStatus>>,
    alloc_tx: Sender<(
        Uuid,
        Option<String>,
        Sender<Result<RelayAllocationInfo, LinkError>>,
    )>,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct RelayAllocationInfo {
    pub relay_host: String,
    pub auth_port: u16,
    pub world_port: u16,
    pub token: String,
    pub expires_at: i64,
}

impl RelayProvider for RelayLink {
    fn allocate(
        &self,
        player_id: &Uuid,
        client_ip: Option<&str>,
    ) -> crate::error::Result<RelayAllocationInfo> {
        let (tx, rx) = channel();
        self.alloc_tx
            .send((*player_id, client_ip.map(|s| s.to_string()), tx))
            .map_err(|_| {
                crate::error::Error::Invalid("relay link worker thread not running".to_string())
            })?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| crate::error::Error::Invalid("relay allocation timed out".to_string()))?
            .map_err(|e| crate::error::Error::Invalid(e.to_string()))
    }
}

impl RelayLink {
    pub fn start(url: String, realm_id: RealmId, key: SigningKey) -> Arc<Self> {
        Self::start_with_ports(url, realm_id, key, None)
    }

    pub fn start_with_ports(
        url: String,
        realm_id: RealmId,
        key: SigningKey,
        local_world_port: Option<u16>,
    ) -> Arc<Self> {
        Self::start_with_local_ports(
            url,
            realm_id,
            key,
            RelayTarget::Auth.local_port(),
            local_world_port,
        )
    }

    pub fn start_with_local_ports(
        url: String,
        realm_id: RealmId,
        key: SigningKey,
        local_auth_port: u16,
        local_world_port: Option<u16>,
    ) -> Arc<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(RelayStatus::default()));
        let (alloc_tx, alloc_rx) = channel();

        let s = stop.clone();
        let st = status.clone();
        let join = std::thread::Builder::new()
            .name("control-relay-host".into())
            .spawn(move || {
                run(
                    &url,
                    realm_id,
                    &key,
                    alloc_rx,
                    &s,
                    &st,
                    local_auth_port,
                    local_world_port,
                )
            })
            .ok();

        Arc::new(RelayLink {
            stop,
            status,
            alloc_tx,
            join: Mutex::new(join),
        })
    }

    pub fn status(&self) -> RelayStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut lock) = self.join.lock() {
            if let Some(j) = lock.take() {
                let _ = j.join();
            }
        }
    }
}

impl Drop for RelayLink {
    fn drop(&mut self) {
        self.stop();
    }
}

fn set_status(status: &Mutex<RelayStatus>, f: impl FnOnce(&mut RelayStatus)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

fn run(
    url: &str,
    realm: RealmId,
    key: &SigningKey,
    alloc_rx: Receiver<(
        Uuid,
        Option<String>,
        Sender<Result<RelayAllocationInfo, LinkError>>,
    )>,
    stop: &AtomicBool,
    status: &Mutex<RelayStatus>,
    local_auth_port: u16,
    local_world_port: Option<u16>,
) {
    let mut failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let began = Instant::now();
        let result = session(
            url,
            realm,
            key,
            &alloc_rx,
            stop,
            status,
            local_auth_port,
            local_world_port,
        );
        if let Err(error) = &result {
            tracing::warn!(%realm, %error, "game relay connection lost; retrying");
        }
        set_status(status, |s| {
            s.connected = false;
            s.active_streams = 0;
            if let Err(e) = &result {
                s.last_error = Some(e.to_string());
            }
        });
        if stop.load(Ordering::SeqCst) {
            return;
        }

        failures = if began.elapsed() > Duration::from_secs(60) {
            1
        } else {
            failures.saturating_add(1)
        };
        let wait = crate::realm_registry::host::backoff(
            failures,
            Duration::from_secs(2),
            Duration::from_secs(60),
            0.5,
        );
        let end = Instant::now() + wait;
        while Instant::now() < end && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

struct StreamWorker {
    tx: Sender<TunnelMsg>,
    reset_on_drop: bool,
}

impl Drop for StreamWorker {
    fn drop(&mut self) {
        if self.reset_on_drop {
            let _ = self.tx.send(TunnelMsg::Reset { stream_id: 0 });
        }
    }
}

fn session(
    url: &str,
    realm: RealmId,
    key: &SigningKey,
    alloc_rx: &Receiver<(
        Uuid,
        Option<String>,
        Sender<Result<RelayAllocationInfo, LinkError>>,
    )>,
    stop: &AtomicBool,
    status: &Mutex<RelayStatus>,
    local_auth_port: u16,
    local_world_port: Option<u16>,
) -> Result<(), LinkError> {
    let mut ws = transport::connect(url, POLL)?;

    // 1. Read challenge
    let challenge_raw = match transport::poll(&mut ws)? {
        Some(Message::Text(t)) => t,
        _ => {
            // wait up to 10s
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut got = None;
            while Instant::now() < deadline && got.is_none() {
                if let Some(Message::Text(t)) = transport::poll(&mut ws)? {
                    got = Some(t);
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            got.ok_or(LinkError::Timeout)?
        }
    };

    let challenge: RelayChallenge = serde_json::from_str(&challenge_raw)
        .map_err(|e| LinkError::Protocol(format!("invalid challenge: {e}")))?;

    // 2. Send HostHello
    let sig = sign_relay_challenge(key, &realm, &challenge.nonce);
    let hello = HostHello {
        realm_id: realm,
        signature: sig,
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .map_err(|e| LinkError::Io(e.to_string()))?;

    // 3. Receive welcome
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut welcome_ok = false;
    while Instant::now() < deadline {
        if let Some(Message::Text(t)) = transport::poll(&mut ws)? {
            let welcome: RelayWelcome = serde_json::from_str(&t)
                .map_err(|e| LinkError::Protocol(format!("invalid welcome: {e}")))?;
            if welcome.ok {
                welcome_ok = true;
                break;
            } else {
                return Err(LinkError::Auth("relay rejected host".into()));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if !welcome_ok {
        return Err(LinkError::Timeout);
    }

    set_status(status, |s| {
        s.connected = true;
        s.last_error = None;
    });
    tracing::info!(%realm, "relay link active");

    let mut streams: HashMap<u32, StreamWorker> = HashMap::new();
    let mut pending_allocs: HashMap<u64, Sender<Result<RelayAllocationInfo, LinkError>>> =
        HashMap::new();
    let req_counter = AtomicU64::new(1);
    let (ws_out_tx, ws_out_rx) = channel::<TunnelMsg>();

    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        // Drain pending allocation requests
        while let Ok((player_id, expected_client_ip, reply_tx)) = alloc_rx.try_recv() {
            let rid = req_counter.fetch_add(1, Ordering::Relaxed);
            pending_allocs.insert(rid, reply_tx);
            let msg = TunnelMsg::Allocate {
                request_id: rid,
                player_id,
                expected_client_ip,
            };
            let _ = ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()));
        }

        // Drain outgoing messages from streams to ws
        while let Ok(msg) = ws_out_rx.try_recv() {
            let _ = ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()));
        }

        // Keepalive
        if last_pong.elapsed() > Duration::from_secs(90) {
            return Err(LinkError::Timeout);
        }
        if last_ping.elapsed() > Duration::from_secs(30) {
            ws.send(Message::Text(
                serde_json::to_string(&TunnelMsg::Ping).unwrap().into(),
            ))
            .map_err(|error| LinkError::Io(error.to_string()))?;
            last_ping = Instant::now();
        }

        // Poll WS incoming (drain available frames)
        for _ in 0..128 {
            match transport::poll(&mut ws)? {
                None => break,
                Some(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<TunnelMsg>(&text) else {
                        continue;
                    };
                    match msg {
                        TunnelMsg::AllocateOk {
                            request_id,
                            token,
                            relay_host,
                            auth_port,
                            world_port,
                            expires_at,
                        } => {
                            if let Some(tx) = pending_allocs.remove(&request_id) {
                                let _ = tx.send(Ok(RelayAllocationInfo {
                                    relay_host,
                                    auth_port,
                                    world_port,
                                    token,
                                    expires_at,
                                }));
                            }
                        }
                        TunnelMsg::AllocateErr { request_id, error } => {
                            if let Some(tx) = pending_allocs.remove(&request_id) {
                                let _ = tx.send(Err(LinkError::Protocol(error)));
                            }
                        }
                        TunnelMsg::Connect {
                            stream_id, target, ..
                        } => {
                            // Strictly predefined local targets: Auth (3724) or World (8085 or bound realm world port)
                            let local_port = match target {
                                RelayTarget::Auth => local_auth_port,
                                RelayTarget::World => match local_world_port {
                                    Some(p) => p,
                                    None => {
                                        eprintln!("RelayLink cannot connect to World: local_world_port was not provided (fail-closed)");
                                        let _ = ws_out_tx.send(TunnelMsg::ConnectErr {
                                            stream_id,
                                            error: "World port unresolved".into(),
                                        });
                                        continue;
                                    }
                                },
                            };
                            match TcpStream::connect(("127.0.0.1", local_port)) {
                                Ok(tcp) => {
                                    let (stream_tx, stream_rx) = channel::<TunnelMsg>();
                                    let ws_tx = ws_out_tx.clone();

                                    // Spawn worker for this stream
                                    spawn_local_tcp_bridge(stream_id, tcp, stream_rx, ws_tx);
                                    streams.insert(
                                        stream_id,
                                        StreamWorker {
                                            tx: stream_tx,
                                            reset_on_drop: true,
                                        },
                                    );

                                    // Reply ConnectOk
                                    let ok_msg = TunnelMsg::ConnectOk { stream_id };
                                    let _ = ws.send(Message::Text(
                                        serde_json::to_string(&ok_msg).unwrap().into(),
                                    ));
                                }
                                Err(e) => {
                                    let err_msg = TunnelMsg::ConnectErr {
                                        stream_id,
                                        error: e.to_string(),
                                    };
                                    let _ = ws.send(Message::Text(
                                        serde_json::to_string(&err_msg).unwrap().into(),
                                    ));
                                }
                            }
                            set_status(status, |s| s.active_streams = streams.len());
                        }
                        TunnelMsg::Data { stream_id, chunk } => {
                            if let Some(w) = streams.get(&stream_id) {
                                let _ = w.tx.send(TunnelMsg::Data { stream_id, chunk });
                            }
                        }
                        TunnelMsg::Close { stream_id } => {
                            if let Some(mut w) = streams.remove(&stream_id) {
                                w.reset_on_drop = false;
                                let _ = w.tx.send(TunnelMsg::Close { stream_id });
                            }
                            set_status(status, |s| s.active_streams = streams.len());
                        }
                        TunnelMsg::Reset { stream_id } => {
                            if let Some(w) = streams.remove(&stream_id) {
                                let _ = w.tx.send(TunnelMsg::Reset { stream_id });
                            }
                            set_status(status, |s| s.active_streams = streams.len());
                        }
                        TunnelMsg::Pong => {
                            last_pong = Instant::now();
                        }
                        _ => {}
                    }
                }
                Some(Message::Close(_)) => {
                    return Err(LinkError::Io("relay closed connection".into()))
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn spawn_local_tcp_bridge(
    stream_id: u32,
    mut tcp: TcpStream,
    from_tunnel: Receiver<TunnelMsg>,
    to_tunnel: Sender<TunnelMsg>,
) {
    let _ = tcp.set_nonblocking(false);
    let _ = tcp.set_nodelay(true);
    let mut tcp_read = tcp.try_clone().expect("tcp clone");
    let to_tunnel_read = to_tunnel.clone();

    // Local TCP read -> to tunnel
    std::thread::Builder::new()
        .name(format!("relay-stream-{stream_id}-read"))
        .spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match tcp_read.read(&mut buf) {
                    Ok(0) => {
                        let _ = to_tunnel_read.send(TunnelMsg::Close { stream_id });
                        break;
                    }
                    Ok(n) => {
                        let chunk = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        if to_tunnel_read
                            .send(TunnelMsg::Data { stream_id, chunk })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = to_tunnel_read.send(TunnelMsg::Reset { stream_id });
                        break;
                    }
                }
            }
        })
        .ok();

    // From tunnel -> local TCP write
    std::thread::Builder::new()
        .name(format!("relay-stream-{stream_id}-write"))
        .spawn(move || {
            while let Ok(msg) = from_tunnel.recv() {
                match msg {
                    TunnelMsg::Data { chunk, .. } => {
                        if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(&chunk) {
                            if tcp.write_all(&data).is_err() {
                                break;
                            }
                        }
                    }
                    TunnelMsg::Close { .. } => {
                        let _ = tcp.shutdown(std::net::Shutdown::Write);
                        break;
                    }
                    TunnelMsg::Reset { .. } => {
                        let _ = tcp.shutdown(std::net::Shutdown::Both);
                        break;
                    }
                    _ => {}
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn relay_reconnects_after_a_dropped_link_and_clears_its_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let seen = accepted.clone();
        let worker = std::thread::spawn(move || {
            for turn in 0..2 {
                let (tcp, _) = listener.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
                let mut ws = tungstenite::accept(tcp).unwrap();
                ws.send(Message::Text(
                    serde_json::to_string(&RelayChallenge {
                        nonce: "relay-reconnect-test".into(),
                    })
                    .unwrap()
                    .into(),
                ))
                .unwrap();
                let _: HostHello =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                ws.send(Message::Text(
                    serde_json::to_string(&RelayWelcome {
                        ok: true,
                        message: String::new(),
                    })
                    .unwrap()
                    .into(),
                ))
                .unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                if turn == 0 {
                    continue;
                }
                while let Ok(message) = ws.read() {
                    let Ok(text) = message.to_text() else {
                        continue;
                    };
                    if let Ok(TunnelMsg::Allocate { request_id, .. }) = serde_json::from_str(text) {
                        ws.send(Message::Text(
                            serde_json::to_string(&TunnelMsg::AllocateOk {
                                request_id,
                                token: "test-only".into(),
                                relay_host: "127.0.0.1".into(),
                                auth_port: 41001,
                                world_port: 41002,
                                expires_at: i64::MAX,
                            })
                            .unwrap()
                            .into(),
                        ))
                        .unwrap();
                        break;
                    }
                }
            }
        });
        let link = RelayLink::start_with_local_ports(
            format!("ws://{addr}"),
            RealmId::new(),
            SigningKey::from_bytes(&[43; 32]),
            57324,
            Some(58085),
        );
        let until = Instant::now() + Duration::from_secs(20);
        while accepted.load(Ordering::SeqCst) < 2 || !link.status().connected {
            assert!(Instant::now() < until, "{:?}", link.status());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(link.status().last_error.is_none());
        let allocation = link.allocate(&Uuid::now_v7(), None).unwrap();
        assert_eq!(
            (allocation.auth_port, allocation.world_port),
            (41001, 41002)
        );
        link.stop();
        worker.join().unwrap();
    }

    #[test]
    fn dropping_a_tunnel_closes_its_local_tcp_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let (tx, rx) = channel();
        let (out, _) = channel();
        spawn_local_tcp_bridge(7, tcp, rx, out);
        drop(StreamWorker {
            tx,
            reset_on_drop: true,
        });
        match peer.read(&mut [0u8; 1]) {
            Ok(0) => {},
            Err(error) if matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {},
            other => panic!("The abandoned relay stream must close, got {other:?}"),
        }
    }
}
