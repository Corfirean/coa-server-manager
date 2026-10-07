//! The Coordinator: control-plane routing between Host Managers and Player Managers (Phase 12). **It never reads what it routes.**
//!
//! A Host Manager keeps one outbound WebSocket to `/coord/v1/host` and proves it holds the key the Registry publishes for its realm. A Player Manager opens
//! `/coord/v1/player?realm=<id>` and proves it holds its own player key. After that both sides run a Noise channel through the Coordinator; the Coordinator only
//! sees the realm id, a connection number, sizes and times. Everything it could be asked to store is in memory, bounded, and gone when the connection ends.
//! It is not the Relay: it carries no game traffic.

pub mod limits;
pub mod listener;

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use coa_control_proto::coord::{self, limits as lim, ErrorCode, Frame};
use coa_registry_proto::RealmId;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use tokio::sync::mpsc;

use limits::{Limiter, Rate};
use listener::Peer;

/// Where a realm's Host key comes from: the Registry's table (the key it was registered with).
pub trait KeyLookup: Send + Sync + 'static {
    fn host_key(&self, realm: &RealmId) -> impl Future<Output = Option<VerifyingKey>> + Send;
}

#[derive(Default)]
pub struct MemoryKeys(Mutex<HashMap<RealmId, [u8; 32]>>);

impl MemoryKeys {
    pub fn set(&self, realm: RealmId, key: &VerifyingKey) {
        self.0.lock().unwrap().insert(realm, key.to_bytes());
    }
}

impl KeyLookup for MemoryKeys {
    async fn host_key(&self, realm: &RealmId) -> Option<VerifyingKey> {
        self.0.lock().unwrap().get(realm).and_then(|b| VerifyingKey::from_bytes(b).ok())
    }
}

impl<T: KeyLookup> KeyLookup for Arc<T> {
    fn host_key(&self, realm: &RealmId) -> impl Future<Output = Option<VerifyingKey>> + Send {
        (**self).host_key(realm)
    }
}

pub struct PgKeys {
    pool: deadpool_postgres::Pool,
}

impl PgKeys {
    pub fn new(pool: deadpool_postgres::Pool) -> Self {
        Self { pool }
    }
}

impl KeyLookup for PgKeys {
    async fn host_key(&self, realm: &RealmId) -> Option<VerifyingKey> {
        let c = self.pool.get().await.ok()?;
        let row = c.query_opt("SELECT public_key FROM realms WHERE realm_id = $1::text::uuid AND published AND advert_version = 2", &[&realm.to_string()]).await.ok()??;
        let bytes: Vec<u8> = row.get(0);
        VerifyingKey::from_bytes(&bytes.try_into().ok()?).ok()
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub trust_proxy: bool,
    pub per_host_connections: usize,
    pub per_ip_connections: usize,
    pub per_realm: Rate,
    pub per_ip: Rate,
    pub bytes_per_connection: usize,
    pub lifetime: Duration,
    pub idle: Duration,
    pub host_answer: Duration,
    pub hello_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trust_proxy: false,
            per_host_connections: lim::PER_HOST_CONNECTIONS,
            per_ip_connections: lim::PER_IP_CONNECTIONS,
            per_realm: Rate::per_minute(lim::PER_REALM_PER_MINUTE, f64::from(lim::PER_REALM_PER_MINUTE)),
            per_ip: Rate::per_minute(lim::PER_IP_PER_MINUTE, f64::from(lim::PER_IP_PER_MINUTE)),
            bytes_per_connection: lim::BYTES_PER_CONNECTION,
            lifetime: Duration::from_secs(lim::CONNECTION_LIFETIME_SECS),
            idle: Duration::from_secs(lim::IDLE_SECS),
            host_answer: Duration::from_secs(lim::HOST_ANSWER_SECS),
            hello_timeout: Duration::from_secs(coord::HELLO_TIMEOUT_SECS),
        }
    }
}

enum HostMsg {
    Text(String),
    Binary(Vec<u8>),
    Stop,
}

enum PlayerMsg {
    Binary(Vec<u8>),
    Close(Option<String>),
}

struct Link {
    tx: mpsc::Sender<PlayerMsg>,
    started: Instant,
    last: Mutex<Instant>,
    bytes: AtomicUsize,
    answered: AtomicBool,
}

struct HostEntry {
    generation: u64,
    tx: mpsc::Sender<HostMsg>,
    links: Mutex<HashMap<u32, Arc<Link>>>,
    next_conn: AtomicU32,
}

pub struct Hub<K: KeyLookup> {
    keys: K,
    cfg: Config,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    hosts: Mutex<HashMap<RealmId, Arc<HostEntry>>>,
    ip_open: Mutex<HashMap<IpAddr, usize>>,
    ip_rate: Limiter<IpAddr>,
    ip_hello: Limiter<IpAddr>,
    realm_rate: Limiter<RealmId>,
    generation: AtomicU64,
    started: Instant,
}

impl<K: KeyLookup> Hub<K> {
    pub fn new(keys: K, cfg: Config, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            keys,
            ip_rate: Limiter::new(cfg.per_ip, 100_000),
            ip_hello: Limiter::new(Rate::per_minute(60, 60.0), 100_000),
            realm_rate: Limiter::new(cfg.per_realm, 100_000),
            cfg,
            clock,
            hosts: Mutex::new(HashMap::new()),
            ip_open: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            started: Instant::now(),
        })
    }

    fn mono(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn connected_hosts(&self) -> usize {
        self.hosts.lock().map(|h| h.len()).unwrap_or(0)
    }
}

/// Counts a connection against its address for as long as it lives.
struct IpGuard<K: KeyLookup> {
    hub: Arc<Hub<K>>,
    ip: IpAddr,
}

impl<K: KeyLookup> IpGuard<K> {
    fn take(hub: &Arc<Hub<K>>, ip: IpAddr) -> Option<Self> {
        let mut open = hub.ip_open.lock().ok()?;
        let n = open.entry(ip).or_insert(0);
        if *n >= hub.cfg.per_ip_connections {
            return None;
        }
        *n += 1;
        Some(Self { hub: hub.clone(), ip })
    }
}

impl<K: KeyLookup> Drop for IpGuard<K> {
    fn drop(&mut self) {
        if let Ok(mut open) = self.hub.ip_open.lock() {
            if let Some(n) = open.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    open.remove(&self.ip);
                }
            }
        }
    }
}

fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        if let Some(ip) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit(',').next()).and_then(|v| v.trim().parse::<IpAddr>().ok()) {
            return ip;
        }
    }
    peer.ip()
}

pub fn router<K: KeyLookup>(hub: Arc<Hub<K>>) -> Router {
    Router::new()
        .route("/coord/v1/health", get(health::<K>))
        .route("/coord/v1/host", get(host_ws::<K>))
        .route("/coord/v1/player", get(player_ws::<K>))
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .with_state(hub)
}

async fn health<K: KeyLookup>(State(hub): State<Arc<Hub<K>>>) -> Response {
    Json(serde_json::json!({ "status": "ok", "protocol_version": coa_control_proto::CONTROL_PROTOCOL_VERSION, "hosts": hub.connected_hosts() })).into_response()
}

async fn host_ws<K: KeyLookup>(State(hub): State<Arc<Hub<K>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let ip = client_ip(&headers, peer, hub.cfg.trust_proxy);
    if !hub.ip_hello.take(&ip, 1.0, hub.mono()) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response();
    }
    let Some(guard) = IpGuard::take(&hub, ip) else { return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response() };
    ws.max_message_size(coord::CONN_PREFIX + coord::MAX_FRAME_BYTES).max_frame_size(coord::CONN_PREFIX + coord::MAX_FRAME_BYTES).on_upgrade(move |socket| async move {
        run_host(hub, socket, ip).await;
        drop(guard);
    })
}

#[derive(Deserialize)]
struct PlayerQuery {
    realm: String,
}

async fn player_ws<K: KeyLookup>(State(hub): State<Arc<Hub<K>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, headers: HeaderMap, Query(q): Query<PlayerQuery>, ws: WebSocketUpgrade) -> Response {
    let ip = client_ip(&headers, peer, hub.cfg.trust_proxy);
    let Ok(realm) = RealmId::parse(&q.realm) else { return (StatusCode::BAD_REQUEST, "bad realm").into_response() };
    if !hub.ip_hello.take(&ip, 1.0, hub.mono()) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response();
    }
    let Some(guard) = IpGuard::take(&hub, ip) else { return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response() };
    ws.max_message_size(coord::CONN_PREFIX + coord::MAX_FRAME_BYTES).max_frame_size(coord::CONN_PREFIX + coord::MAX_FRAME_BYTES).on_upgrade(move |socket| async move {
        run_player(hub, socket, ip, realm).await;
        drop(guard);
    })
}

async fn send_frame(sock: &mut WebSocket, f: &Frame) -> bool {
    sock.send(Message::Text(coord::to_text(f).into())).await.is_ok()
}

async fn refuse(sock: &mut WebSocket, code: ErrorCode, message: &str) {
    let _ = send_frame(sock, &Frame::Error { code, message: message.to_string() }).await;
    let _ = sock.send(Message::Close(None)).await;
    // let the peer read the error and answer the close; dropping at once can reset the connection and discard the error before it is read
    let _ = tokio::time::timeout(Duration::from_millis(500), async { while let Some(Ok(_)) = sock.recv().await {} }).await;
}

/// The next text frame within `timeout` (pings and pongs are the transport's business).
async fn next_text(sock: &mut WebSocket, timeout: Duration) -> Option<Frame> {
    let end = Instant::now() + timeout;
    loop {
        let left = end.checked_duration_since(Instant::now())?;
        match tokio::time::timeout(left, sock.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => return coord::parse_text(t.as_str()).ok(),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            _ => return None,
        }
    }
}

async fn run_host<K: KeyLookup>(hub: Arc<Hub<K>>, mut sock: WebSocket, ip: IpAddr) {
    let nonce = coord::challenge_nonce();
    if !send_frame(&mut sock, &Frame::Challenge { protocol: coa_control_proto::CONTROL_PROTOCOL_VERSION, nonce: nonce.clone() }).await {
        return;
    }
    let Some(hello) = next_text(&mut sock, hub.cfg.hello_timeout).await else { return refuse(&mut sock, ErrorCode::Timeout, "no valid hello").await };
    let Frame::HostHello { realm_id: realm, .. } = &hello else { return refuse(&mut sock, ErrorCode::BadHello, "a host hello was expected").await };
    let realm = *realm;
    let Some(key) = hub.keys.host_key(&realm).await else { return refuse(&mut sock, ErrorCode::UnknownRealm, "this realm is not published").await };
    if coord::verify_host_hello(&hello, &key, None, &nonce, (hub.clock)()).is_err() {
        tracing::warn!(%realm, %ip, "host hello refused");
        return refuse(&mut sock, ErrorCode::BadHello, "the hello does not prove the realm key").await;
    }
    let (tx, mut rx) = mpsc::channel::<HostMsg>(256);
    let entry = Arc::new(HostEntry { generation: hub.generation.fetch_add(1, Ordering::SeqCst), tx, links: Mutex::new(HashMap::new()), next_conn: AtomicU32::new(1) });
    if let Some(old) = hub.hosts.lock().ok().and_then(|mut h| h.insert(realm, entry.clone())) {
        let _ = old.tx.try_send(HostMsg::Stop);
    }
    if !send_frame(&mut sock, &Frame::HostReady { max_connections: hub.cfg.per_host_connections as u32, max_frame: coord::MAX_FRAME_BYTES as u32 }).await {
        remove_host(&hub, &realm, &entry);
        return;
    }
    tracing::info!(%realm, %ip, "host connected");
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            incoming = sock.recv() => match incoming {
                Some(Ok(Message::Binary(b))) => {
                    let Ok((conn, frame)) = coord::split_binary(&b) else { break };
                    let link = entry.links.lock().ok().and_then(|l| l.get(&conn).cloned());
                    let Some(link) = link else { continue };
                    if link.bytes.fetch_add(frame.len(), Ordering::Relaxed) + frame.len() > hub.cfg.bytes_per_connection {
                        let _ = link.tx.try_send(PlayerMsg::Close(Some("byte limit".into())));
                        continue;
                    }
                    link.answered.store(true, Ordering::Relaxed);
                    if let Ok(mut l) = link.last.lock() { *l = Instant::now(); }
                    if link.tx.send(PlayerMsg::Binary(frame.to_vec())).await.is_err() {
                        if let Ok(mut l) = entry.links.lock() { l.remove(&conn); }
                    }
                }
                Some(Ok(Message::Text(t))) => match coord::parse_text(t.as_str()) {
                    Ok(Frame::Close { conn, reason }) => {
                        let link = entry.links.lock().ok().and_then(|mut l| l.remove(&conn));
                        if let Some(link) = link { let _ = link.tx.try_send(PlayerMsg::Close(reason)); }
                    }
                    _ => break,
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => break,
            },
            out = rx.recv() => match out {
                Some(HostMsg::Text(t)) => if sock.send(Message::Text(t.into())).await.is_err() { break },
                Some(HostMsg::Binary(b)) => if sock.send(Message::Binary(b.into())).await.is_err() { break },
                Some(HostMsg::Stop) | None => break,
            },
            _ = tick.tick() => {
                if sock.send(Message::Ping(Vec::new().into())).await.is_err() { break }
                expire(&hub, &entry);
            }
        }
    }
    remove_host(&hub, &realm, &entry);
    tracing::info!(%realm, "host disconnected");
}

fn remove_host<K: KeyLookup>(hub: &Hub<K>, realm: &RealmId, entry: &Arc<HostEntry>) {
    if let Ok(mut hosts) = hub.hosts.lock() {
        if hosts.get(realm).is_some_and(|e| e.generation == entry.generation) {
            hosts.remove(realm);
        }
    }
    let links: Vec<Arc<Link>> = entry.links.lock().map(|mut l| l.drain().map(|(_, v)| v).collect()).unwrap_or_default();
    for l in links {
        let _ = l.tx.try_send(PlayerMsg::Close(Some("host offline".into())));
    }
}

/// Close connections that are too old, silent too long, or whose Host never answered.
fn expire<K: KeyLookup>(hub: &Hub<K>, entry: &HostEntry) {
    let now = Instant::now();
    let dead: Vec<(u32, &'static str)> = entry
        .links
        .lock()
        .map(|l| {
            l.iter()
                .filter_map(|(conn, link)| {
                    let last = link.last.lock().map(|t| *t).unwrap_or(now);
                    if now.duration_since(link.started) > hub.cfg.lifetime {
                        Some((*conn, "lifetime"))
                    } else if now.duration_since(last) > hub.cfg.idle {
                        Some((*conn, "idle"))
                    } else if !link.answered.load(Ordering::Relaxed) && now.duration_since(link.started) > hub.cfg.host_answer {
                        Some((*conn, "host did not answer"))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    for (conn, why) in dead {
        let link = entry.links.lock().ok().and_then(|mut l| l.remove(&conn));
        if let Some(link) = link {
            let _ = link.tx.try_send(PlayerMsg::Close(Some(why.into())));
        }
        let _ = entry.tx.try_send(HostMsg::Text(coord::to_text(&Frame::Close { conn, reason: Some(why.into()) })));
    }
}

async fn run_player<K: KeyLookup>(hub: Arc<Hub<K>>, mut sock: WebSocket, ip: IpAddr, realm: RealmId) {
    let nonce = coord::challenge_nonce();
    if !send_frame(&mut sock, &Frame::Challenge { protocol: coa_control_proto::CONTROL_PROTOCOL_VERSION, nonce: nonce.clone() }).await {
        return;
    }
    let Some(hello) = next_text(&mut sock, hub.cfg.hello_timeout).await else { return refuse(&mut sock, ErrorCode::Timeout, "no valid hello").await };
    let Ok((player, hello_realm, _key)) = coord::verify_player_hello(&hello, &nonce, (hub.clock)()) else { return refuse(&mut sock, ErrorCode::BadHello, "the hello does not prove the player key").await };
    if hello_realm != realm {
        return refuse(&mut sock, ErrorCode::BadHello, "the hello is for another realm").await;
    }
    if !hub.ip_rate.take(&ip, 1.0, hub.mono()) || !hub.realm_rate.take(&realm, 1.0, hub.mono()) {
        return refuse(&mut sock, ErrorCode::RateLimited, "too many connections").await;
    }
    let entry = hub.hosts.lock().ok().and_then(|h| h.get(&realm).cloned());
    let Some(entry) = entry else { return refuse(&mut sock, ErrorCode::HostOffline, "the realm's Manager is not connected").await };
    let (ptx, mut prx) = mpsc::channel::<PlayerMsg>(64);
    let link = Arc::new(Link { tx: ptx, started: Instant::now(), last: Mutex::new(Instant::now()), bytes: AtomicUsize::new(0), answered: AtomicBool::new(false) });
    let slot = take_slot(&hub, &entry, &link);
    let Some(conn) = slot else { return refuse(&mut sock, ErrorCode::HostBusy, "the realm's Manager is busy").await };
    if entry.tx.send(HostMsg::Text(coord::to_text(&Frame::Open { conn }))).await.is_err() {
        if let Ok(mut l) = entry.links.lock() {
            l.remove(&conn);
        }
        return refuse(&mut sock, ErrorCode::HostOffline, "the realm's Manager is not connected").await;
    }
    tracing::info!(%realm, conn, %player, %ip, "player connected");
    if !send_frame(&mut sock, &Frame::PlayerReady { conn, max_frame: coord::MAX_FRAME_BYTES as u32 }).await {
        end_link(&entry, conn).await;
        return;
    }
    loop {
        tokio::select! {
            incoming = sock.recv() => match incoming {
                Some(Ok(Message::Binary(b))) => {
                    if b.len() > coord::MAX_FRAME_BYTES || link.bytes.fetch_add(b.len(), Ordering::Relaxed) + b.len() > hub.cfg.bytes_per_connection {
                        refuse(&mut sock, ErrorCode::TooLarge, "a limit was exceeded").await;
                        break;
                    }
                    if let Ok(mut l) = link.last.lock() { *l = Instant::now(); }
                    if entry.tx.send(HostMsg::Binary(coord::binary(conn, &b))).await.is_err() { break }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Text(t))) => {
                    if !matches!(coord::parse_text(t.as_str()), Ok(Frame::Close { .. })) { break }
                    break;
                }
                _ => break,
            },
            out = prx.recv() => match out {
                Some(PlayerMsg::Binary(b)) => if sock.send(Message::Binary(b.into())).await.is_err() { break },
                Some(PlayerMsg::Close(reason)) => {
                    let code = if reason.as_deref() == Some("host offline") { ErrorCode::HostOffline } else { ErrorCode::Timeout };
                    refuse(&mut sock, code, reason.as_deref().unwrap_or("closed")).await;
                    break;
                }
                None => break,
            },
        }
    }
    end_link(&entry, conn).await;
    tracing::info!(%realm, conn, bytes = link.bytes.load(Ordering::Relaxed), seconds = link.started.elapsed().as_secs(), "player disconnected");
}

/// A connection number on this Host, or `None` when it is at its limit.
fn take_slot<K: KeyLookup>(hub: &Hub<K>, entry: &HostEntry, link: &Arc<Link>) -> Option<u32> {
    let mut links = entry.links.lock().ok()?;
    if links.len() >= hub.cfg.per_host_connections {
        return None;
    }
    let conn = entry.next_conn.fetch_add(1, Ordering::SeqCst);
    links.insert(conn, link.clone());
    Some(conn)
}

async fn end_link(entry: &HostEntry, conn: u32) {
    let was = entry.links.lock().ok().and_then(|mut l| l.remove(&conn));
    if was.is_some() {
        let _ = entry.tx.try_send(HostMsg::Text(coord::to_text(&Frame::Close { conn, reason: None })));
    }
}
