//! The Host Manager's side of the control plane: one outbound connection per published realm to the Coordinator, over which Player Managers reach the
//! realm's [`HostService`] through an end-to-end channel the Coordinator cannot read.
//!
//! The Host connects out, so it needs no open port. A player's channel goes through these steps (see `docs/CONTROL_PROTOCOL.md`):
//! `Open{conn}` -> Noise XX handshake (3 messages) -> the Host sends its proof (realm key over the handshake hash) -> the Player sends its proof (player key over the
//! same hash) -> requests and responses. A channel that breaks a step, speaks too much or stays too long is closed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use coa_control_proto::app::{self, AppError, Response};
use coa_control_proto::coord::{self, Frame};
use coa_control_proto::noise::{self, Channel, PlayerProof, Responder};
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;
use serde::Serialize;
use tungstenite::Message;
use uuid::Uuid;

use super::service::{HostService, Session};
use super::transport::{self, LinkError};

const MAX_CHANNELS: usize = 16;
const CHANNEL_LIFETIME: Duration = Duration::from_secs(300);
const CHANNEL_IDLE: Duration = Duration::from_secs(45);
const POLL: Duration = Duration::from_millis(500);

enum Stage {
    AwaitFirst,
    AwaitThird(Responder),
    AwaitPlayerProof(Channel),
    Ready { channel: Channel, player: Uuid, public_key: String, session: Session },
}

struct Conn {
    stage: Option<Stage>,
    opened: Instant,
    last: Instant,
    client_ip: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LinkStatus {
    pub connected: bool,
    pub last_error: Option<String>,
    pub channels: usize,
    pub requests: u64,
}

pub struct HostLink {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<LinkStatus>>,
    join: Option<JoinHandle<()>>,
}

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

impl HostLink {
    /// `url` is the Coordinator's host endpoint, e.g. `wss://registry.example/coord/v1/host`.
    pub fn start(url: String, realm_id: RealmId, key: SigningKey, service: Arc<HostService>, clock: Clock) -> HostLink {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(LinkStatus::default()));
        let (s, st) = (stop.clone(), status.clone());
        let join = std::thread::Builder::new().name("control-host".into()).spawn(move || run(&url, realm_id, &key, &service, &clock, &s, &st)).ok();
        HostLink { stop, status, join }
    }

    pub fn status(&self) -> LinkStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for HostLink {
    fn drop(&mut self) {
        self.stop();
    }
}

fn set(status: &Mutex<LinkStatus>, f: impl FnOnce(&mut LinkStatus)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

fn run(url: &str, realm: RealmId, key: &SigningKey, service: &HostService, clock: &Clock, stop: &AtomicBool, status: &Mutex<LinkStatus>) {
    let mut failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let began = Instant::now();
        let result = session(url, realm, key, service, clock, stop, status);
        set(status, |s| {
            s.connected = false;
            s.channels = 0;
            if let Err(e) = &result {
                s.last_error = Some(e.to_string());
            }
        });
        if stop.load(Ordering::SeqCst) {
            return;
        }
        // a link that held for a while starts the backoff over
        failures = if began.elapsed() > Duration::from_secs(60) { 1 } else { failures.saturating_add(1) };
        let wait = super::super::realm_registry::host::backoff(failures, Duration::from_secs(2), Duration::from_secs(60), 0.5);
        let end = Instant::now() + wait;
        while Instant::now() < end && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn session(url: &str, realm: RealmId, key: &SigningKey, service: &HostService, clock: &Clock, stop: &AtomicBool, status: &Mutex<LinkStatus>) -> Result<(), LinkError> {
    let mut ws = transport::connect(url, POLL)?;
    let nonce = match transport::next_frame(&mut ws, Duration::from_secs(15))? {
        Frame::Challenge { nonce, .. } => nonce,
        _ => return Err(LinkError::Protocol("a challenge was expected".into())),
    };
    transport::send_text(&mut ws, &coord::sign_host_hello(key, &realm, &nonce, clock()))?;
    match transport::next_frame(&mut ws, Duration::from_secs(15))? {
        Frame::HostReady { .. } => {}
        _ => return Err(LinkError::Protocol("the Coordinator did not accept the hello".into())),
    }
    set(status, |s| {
        s.connected = true;
        s.last_error = None;
    });
    tracing::info!(%realm, "control link up");
    let mut conns: HashMap<u32, Conn> = HashMap::new();
    let mut last_ping = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        match transport::poll(&mut ws)? {
            None => {}
            Some(Message::Text(t)) => match coord::parse_text(t.as_str()).map_err(LinkError::from)? {
                Frame::Open { conn, client_ip } => {
                    if conns.len() >= MAX_CHANNELS || conns.contains_key(&conn) {
                        transport::send_text(&mut ws, &Frame::Close { conn, reason: Some("busy".into()) })?;
                    } else {
                        conns.insert(conn, Conn { stage: Some(Stage::AwaitFirst), opened: Instant::now(), last: Instant::now(), client_ip });
                    }
                }
                Frame::Close { conn, .. } => {
                    conns.remove(&conn);
                }
                Frame::Error { code, message } => return Err(transport::from_error_frame(code, &message)),
                _ => return Err(LinkError::Protocol("an unexpected control frame".into())),
            },
            Some(Message::Binary(data)) => {
                let (conn, frame) = coord::split_binary(&data).map_err(LinkError::from)?;
                let Some(c) = conns.get_mut(&conn) else { continue };
                c.last = Instant::now();
                match step(c, frame, realm, key, service, status) {
                    Ok(out) => {
                        for f in out {
                            transport::send_binary(&mut ws, coord::binary(conn, &f))?;
                        }
                    }
                    Err(why) => {
                        tracing::debug!(%realm, conn, reason = %why, "channel closed");
                        conns.remove(&conn);
                        transport::send_text(&mut ws, &Frame::Close { conn, reason: Some("closed".into()) })?;
                    }
                }
            }
            Some(Message::Close(_)) => return Err(LinkError::Io("the Coordinator closed the connection".into())),
            Some(_) => {}
        }
        let now = Instant::now();
        let expired: Vec<u32> = conns.iter().filter(|(_, c)| now.duration_since(c.opened) > CHANNEL_LIFETIME || now.duration_since(c.last) > CHANNEL_IDLE).map(|(k, _)| *k).collect();
        for conn in expired {
            conns.remove(&conn);
            transport::send_text(&mut ws, &Frame::Close { conn, reason: Some("expired".into()) })?;
        }
        if last_ping.elapsed() > Duration::from_secs(20) {
            ws.send(Message::Ping(Vec::new().into())).map_err(|e| LinkError::Io(e.to_string()))?;
            last_ping = Instant::now();
        }
        set(status, |s| s.channels = conns.len());
    }
    let _ = ws.close(None);
    Ok(())
}

fn seal_all(channel: &mut Channel, message: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    channel.seal(message).map_err(|e| e.to_string())
}

/// Advance one channel by one frame; the frames to send back, or the reason to close it.
fn step(c: &mut Conn, frame: &[u8], realm: RealmId, key: &SigningKey, service: &HostService, status: &Mutex<LinkStatus>) -> Result<Vec<Vec<u8>>, String> {
    let stage = c.stage.take().ok_or("a channel in no state")?;
    match stage {
        Stage::AwaitFirst => {
            let (responder, msg2) = Responder::start(frame).map_err(|e| e.to_string())?;
            c.stage = Some(Stage::AwaitThird(responder));
            Ok(vec![msg2])
        }
        Stage::AwaitThird(responder) => {
            let mut channel = responder.finish(frame).map_err(|e| e.to_string())?;
            let proof = noise::host_proof(key, &realm, &channel);
            let frames = seal_all(&mut channel, &serde_json::to_vec(&proof).map_err(|e| e.to_string())?)?;
            c.stage = Some(Stage::AwaitPlayerProof(channel));
            Ok(frames)
        }
        Stage::AwaitPlayerProof(mut channel) => {
            let Some(message) = channel.open_frame(frame).map_err(|e| e.to_string())? else {
                c.stage = Some(Stage::AwaitPlayerProof(channel));
                return Ok(vec![]);
            };
            let proof: PlayerProof = serde_json::from_slice(&message).map_err(|_| "a player proof is not valid".to_string())?;
            noise::verify_player_proof(&proof, &realm, &channel).map_err(|e| e.to_string())?;
            if !service.key_matches(&proof.player_id, &proof.public_key) {
                return Err("the player id belongs to another key".into());
            }
            c.stage = Some(Stage::Ready { channel, player: proof.player_id, public_key: proof.public_key, session: Session::with_client_ip(c.client_ip.clone()) });
            Ok(vec![])
        }
        Stage::Ready { mut channel, player, public_key, mut session } => {
            let message = channel.open_frame(frame).map_err(|e| e.to_string())?;
            let out = match message {
                None => vec![],
                Some(bytes) => {
                    let response = match app::decode_request(&bytes) {
                        Ok(request) => service.handle(&mut session, &player, &public_key, request),
                        Err(_) => Response::Error { code: AppError::Invalid, message: "That request is not valid.".into() },
                    };
                    set(status, |s| s.requests += 1);
                    seal_all(&mut channel, &app::encode_response(&response))?
                }
            };
            c.stage = Some(Stage::Ready { channel, player, public_key, session });
            Ok(out)
        }
    }
}
