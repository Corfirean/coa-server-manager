//! Outbound Host <-> Game Relay tunnel link (Phase 13).
//! Maintains outbound connectivity from Host Manager to the Game Relay,
//! opening local TCP connections strictly to 127.0.0.1:3724 (Auth) and 127.0.0.1:8085 (World).

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
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(RelayStatus::default()));
        let (alloc_tx, alloc_rx) = channel();

        let s = stop.clone();
        let st = status.clone();
        let join = std::thread::Builder::new()
            .name("control-relay-host".into())
            .spawn(move || run(&url, realm_id, &key, alloc_rx, &s, &st))
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
) {
    let mut failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let began = Instant::now();
        let result = session(url, realm, key, &alloc_rx, stop, status);
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
        if last_ping.elapsed() > Duration::from_secs(30) {
            let _ = ws.send(Message::Text(
                serde_json::to_string(&TunnelMsg::Ping).unwrap().into(),
            ));
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
                            // Strictly predefined local targets: Auth (3724) or World (8085)
                            let local_port = target.local_port();
                            match TcpStream::connect(("127.0.0.1", local_port)) {
                                Ok(tcp) => {
                                    let (stream_tx, stream_rx) = channel::<TunnelMsg>();
                                    let ws_tx = ws_out_tx.clone();

                                    // Spawn worker for this stream
                                    spawn_local_tcp_bridge(stream_id, tcp, stream_rx, ws_tx);
                                    streams.insert(stream_id, StreamWorker { tx: stream_tx });

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
                            if let Some(w) = streams.remove(&stream_id) {
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
                        TunnelMsg::Pong => {}
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
