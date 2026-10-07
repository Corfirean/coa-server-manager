//! The HTTP surface of Registry protocol 1.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use coa_registry_proto::sign::{self, SignedHeaders};
use coa_registry_proto::*;
use std::result::Result;
use serde::Serialize;

use crate::limits::{Limiter, Rate};
use crate::listener::Peer;
use crate::store::{RealmRow, Store, StoreError};

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub online_ttl_secs: u64,
    /// Behind a reverse proxy the client address is the last entry of `X-Forwarded-For` (the one the proxy itself appended).
    pub trust_proxy: bool,
    pub general: Rate,
    pub register: Rate,
    pub heartbeat: Rate,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            online_ttl_secs: ONLINE_TTL_SECS,
            trust_proxy: false,
            general: Rate::per_minute(120, 120.0),
            register: Rate::per_hour(10, 10.0),
            heartbeat: Rate { burst: 4.0, refill: 1.0 / 10.0 },
        }
    }
}

pub struct AppState<S: Store> {
    pub store: S,
    pub clock: Clock,
    pub cfg: ApiConfig,
    ip_general: Limiter<IpAddr>,
    ip_register: Limiter<IpAddr>,
    realm_heartbeat: Limiter<RealmId>,
    started: Instant,
}

impl<S: Store> AppState<S> {
    pub fn new(store: S, clock: Clock, cfg: ApiConfig) -> Self {
        Self {
            ip_general: Limiter::new(cfg.general, 100_000),
            ip_register: Limiter::new(cfg.register, 100_000),
            realm_heartbeat: Limiter::new(cfg.heartbeat, 100_000),
            store,
            clock,
            cfg,
            started: Instant::now(),
        }
    }
    fn now(&self) -> i64 {
        (self.clock)()
    }
    fn mono(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

pub struct ApiError {
    status: StatusCode,
    code: ErrorCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
    fn malformed(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::MalformedRequest, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorBody { error: ErrorDetail { code: self.code, message: self.message } })).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Unknown => Self::new(StatusCode::NOT_FOUND, ErrorCode::UnknownRealm, "this realm is not registered"),
            StoreError::KeyMismatch => Self::new(StatusCode::FORBIDDEN, ErrorCode::RealmKeyMismatch, "this realm id belongs to another key"),
            StoreError::Stale => Self::new(StatusCode::UNAUTHORIZED, ErrorCode::TimestampNotMonotonic, "the timestamp is not later than the last request of this realm"),
            StoreError::NotPublished => Self::new(StatusCode::CONFLICT, ErrorCode::NotPublished, "the realm is not published; register it again"),
            StoreError::Backend(m) => {
                tracing::error!(error = %m, "storage failure");
                Self::new(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable, "storage is unavailable")
            }
        }
    }
}

pub fn router<S: Store>(state: Arc<AppState<S>>) -> Router {
    Router::new()
        .route("/registry/v1/healthz", get(healthz::<S>))
        .route("/registry/v1/realms/register", post(register::<S>))
        .route("/registry/v1/realms/{realm_id}/heartbeat", post(heartbeat::<S>))
        .route("/registry/v1/realms/{realm_id}/unpublish", post(unpublish::<S>))
        .route("/registry/v1/realms/{realm_id}", get(self_info::<S>))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, ErrorCode::MalformedRequest, "no such endpoint") })
        .with_state(state)
}

fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        if let Some(last) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit(',').next()) {
            if let Ok(ip) = last.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    peer.ip()
}

fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or("")
}

fn log(endpoint: &str, realm: Option<&RealmId>, started: Instant, result: &Result<(StatusCode, String), ApiError>) {
    let (status, caps) = match result {
        Ok((s, c)) => (s.as_u16(), c.as_str()),
        Err(e) => (e.status.as_u16(), ""),
    };
    let code = result.as_ref().err().map(|e| format!("{:?}", e.code)).unwrap_or_default();
    tracing::info!(endpoint, realm = realm.map(|r| r.to_string()).unwrap_or_default(), status, error = %code, latency_ms = started.elapsed().as_millis() as u64, caps = caps, "request");
}

/// The IP-level admission every request passes before anything is parsed.
fn admit<S: Store>(st: &AppState<S>, ip: IpAddr, cost: f64) -> Result<(), ApiError> {
    if st.ip_general.take(&ip, cost, st.mono()) {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited, "too many requests"))
    }
}

async fn read_body(body: Body) -> Result<Vec<u8>, ApiError> {
    match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(b) => Ok(b.to_vec()),
        Err(_) => Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, ErrorCode::RequestTooLarge, format!("a request is limited to {MAX_REQUEST_BYTES} bytes"))),
    }
}

/// Headers, id and timestamp: everything that can be checked before the key is known.
fn prelude<S: Store>(st: &AppState<S>, headers: &HeaderMap, uri: &Uri, path_realm: Option<&str>) -> Result<(SignedHeaders, RealmId), ApiError> {
    if uri.query().is_some() {
        return Err(ApiError::malformed("a signed endpoint takes no query string"));
    }
    let h = SignedHeaders::parse(|n| headers.get(n).and_then(|v| v.to_str().ok())).map_err(|e| ApiError::malformed(e.to_string()))?;
    if h.version != REGISTRY_PROTOCOL_VERSION {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::UnsupportedProtocolVersion, format!("protocol version {} is not supported; this Registry speaks {REGISTRY_PROTOCOL_VERSION}", h.version)));
    }
    let realm = match path_realm {
        Some(text) => RealmId::parse(text).map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidRealmId, "the realm id in the path is not a canonical UUIDv7"))?,
        None => h.realm,
    };
    if h.realm != realm {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidRealmId, "the realm header and the realm id disagree"));
    }
    if (h.timestamp - st.now()).abs() > MAX_CLOCK_SKEW_SECS {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, ErrorCode::BadTimestamp, format!("the timestamp is more than {MAX_CLOCK_SKEW_SECS} seconds from the Registry's clock")));
    }
    Ok((h, realm))
}

fn check_signature<S: Store>(st: &AppState<S>, ip: IpAddr, key: &[u8; 32], h: &SignedHeaders, method: &Method, path: &str, body: &[u8]) -> Result<(), ApiError> {
    let ok = ed25519_dalek::VerifyingKey::from_bytes(key).map(|k| sign::verify_request(&k, h, method.as_str(), path, body)).unwrap_or(false);
    if ok {
        return Ok(());
    }
    // a forged request costs its sender more than an honest one
    st.ip_general.take(&ip, 4.0, st.mono());
    Err(ApiError::new(StatusCode::UNAUTHORIZED, ErrorCode::InvalidSignature, "the signature does not match"))
}

fn realm_admit<S: Store>(st: &AppState<S>, realm: &RealmId) -> Result<(), ApiError> {
    if st.realm_heartbeat.take(realm, 1.0, st.mono()) {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited, "this realm sends requests too fast"))
    }
}

async fn healthz<S: Store>(State(st): State<Arc<AppState<S>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, req: Request<Body>) -> Response {
    let ip = client_ip(req.headers(), peer, st.cfg.trust_proxy);
    if let Err(e) = admit(&st, ip, 1.0) {
        return e.into_response();
    }
    if st.store.ping().await {
        Json(Health { status: "ok".into(), protocol_version: REGISTRY_PROTOCOL_VERSION }).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(Health { status: "degraded".into(), protocol_version: REGISTRY_PROTOCOL_VERSION })).into_response()
    }
}

fn json<T: Serialize>(value: &T) -> (StatusCode, Response) {
    (StatusCode::OK, Json(value).into_response())
}

async fn register<S: Store>(State(st): State<Arc<AppState<S>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, req: Request<Body>) -> Response {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let ip = client_ip(&parts.headers, peer, st.cfg.trust_proxy);
    let mut realm_for_log = None;
    let result: Result<(StatusCode, Response, String), ApiError> = async {
        admit(&st, ip, 1.0)?;
        if !st.ip_register.take(&ip, 1.0, st.mono()) {
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited, "too many registrations from this address"));
        }
        let bytes = read_body(body).await?;
        let (h, realm) = prelude(&st, &parts.headers, &parts.uri, None)?;
        realm_for_log = Some(realm);
        let req: RegisterRequest = serde_json::from_slice(&bytes).map_err(|_| ApiError::malformed("the body is not a registration"))?;
        if req.protocol_version != REGISTRY_PROTOCOL_VERSION {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::UnsupportedProtocolVersion, format!("protocol version {} is not supported", req.protocol_version)));
        }
        if req.realm_id != realm {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidRealmId, "the realm header and the body disagree"));
        }
        req.validate().map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidMetadata, e.to_string()))?;
        let public = sign::decode_public_key(&req.public_key).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidMetadata, e.to_string()))?;
        check_signature(&st, ip, public.as_bytes(), &h, &parts.method, parts.uri.path(), &bytes)?;
        let now = st.now();
        let (row, created) = st.store.register(&req, *public.as_bytes(), now, h.timestamp).await?;
        let resp = RegisterResponse {
            protocol_version: REGISTRY_PROTOCOL_VERSION,
            realm_id: realm,
            created,
            metadata_revision: row.metadata_revision,
            published: row.published,
            capabilities_hash: row.capabilities_hash.clone(),
            server_time: now,
            heartbeat_interval_secs: HEARTBEAT_INTERVAL_SECS,
            online_ttl_secs: st.cfg.online_ttl_secs,
        };
        let (status, response) = json(&resp);
        Ok((status, response, row.capabilities_hash))
    }
    .await;
    finish("register", realm_for_log.as_ref(), started, result)
}

fn finish(endpoint: &str, realm: Option<&RealmId>, started: Instant, result: Result<(StatusCode, Response, String), ApiError>) -> Response {
    match result {
        Ok((status, response, caps)) => {
            log(endpoint, realm, started, &Ok((status, short(&caps).to_string())));
            response
        }
        Err(e) => {
            log(endpoint, realm, started, &Err(ApiError { status: e.status, code: e.code, message: String::new() }));
            e.into_response()
        }
    }
}

/// Shared by heartbeat, unpublish and the self endpoint: an existing realm, a valid signature, and (for writes) the per-realm rate.
async fn authenticate<S: Store>(st: &AppState<S>, ip: IpAddr, parts: &axum::http::request::Parts, path_realm: &str, bytes: &[u8], limited: bool) -> Result<(RealmId, i64), ApiError> {
    let (h, realm) = prelude(st, &parts.headers, &parts.uri, Some(path_realm))?;
    let key = st.store.key_of(&realm).await?.ok_or(StoreError::Unknown)?;
    check_signature(st, ip, &key, &h, &parts.method, parts.uri.path(), bytes)?;
    if limited {
        realm_admit(st, &realm)?;
    }
    Ok((realm, h.timestamp))
}

async fn heartbeat<S: Store>(State(st): State<Arc<AppState<S>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, Path(realm_text): Path<String>, req: Request<Body>) -> Response {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let ip = client_ip(&parts.headers, peer, st.cfg.trust_proxy);
    let mut realm_for_log = None;
    let result: Result<(StatusCode, Response, String), ApiError> = async {
        admit(&st, ip, 1.0)?;
        let bytes = read_body(body).await?;
        let (realm, ts) = authenticate(&st, ip, &parts, &realm_text, &bytes, true).await?;
        realm_for_log = Some(realm);
        let hb: HeartbeatRequest = serde_json::from_slice(&bytes).map_err(|_| ApiError::malformed("the body is not a heartbeat"))?;
        if hb.protocol_version != REGISTRY_PROTOCOL_VERSION {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::UnsupportedProtocolVersion, format!("protocol version {} is not supported", hb.protocol_version)));
        }
        hb.validate().map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidMetadata, e.to_string()))?;
        let now = st.now();
        let out = st.store.heartbeat(&realm, &hb, now, ts).await?;
        let resp = HeartbeatResponse { protocol_version: REGISTRY_PROTOCOL_VERSION, metadata_revision: out.metadata_revision, published: true, capabilities_hash: out.capabilities_hash.clone(), resend_capabilities: out.resend_capabilities, server_time: now };
        let (status, response) = json(&resp);
        Ok((status, response, out.capabilities_hash))
    }
    .await;
    finish("heartbeat", realm_for_log.as_ref(), started, result)
}

async fn unpublish<S: Store>(State(st): State<Arc<AppState<S>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, Path(realm_text): Path<String>, req: Request<Body>) -> Response {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let ip = client_ip(&parts.headers, peer, st.cfg.trust_proxy);
    let mut realm_for_log = None;
    let result: Result<(StatusCode, Response, String), ApiError> = async {
        admit(&st, ip, 1.0)?;
        let bytes = read_body(body).await?;
        let (realm, ts) = authenticate(&st, ip, &parts, &realm_text, &bytes, true).await?;
        realm_for_log = Some(realm);
        let un: UnpublishRequest = serde_json::from_slice(&bytes).map_err(|_| ApiError::malformed("the body is not an unpublish request"))?;
        if un.protocol_version != REGISTRY_PROTOCOL_VERSION {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::UnsupportedProtocolVersion, format!("protocol version {} is not supported", un.protocol_version)));
        }
        let now = st.now();
        let revision = st.store.unpublish(&realm, now, ts).await?;
        let (status, response) = json(&UnpublishResponse { protocol_version: REGISTRY_PROTOCOL_VERSION, published: false, metadata_revision: revision, server_time: now });
        Ok((status, response, String::new()))
    }
    .await;
    finish("unpublish", realm_for_log.as_ref(), started, result)
}

fn self_response(row: RealmRow, now: i64, ttl: u64) -> SelfResponse {
    let online = row.published && now - row.last_seen_at <= ttl as i64;
    SelfResponse {
        protocol_version: REGISTRY_PROTOCOL_VERSION,
        realm_id: row.realm_id,
        public_key: sign::encode_public_key(&ed25519_dalek::VerifyingKey::from_bytes(&row.public_key).expect("a stored key was validated on the way in")),
        display_name: row.display_name,
        description: row.description,
        language: row.language,
        ruleset: row.ruleset,
        manager_version: row.manager_version,
        capabilities: row.capabilities,
        capabilities_hash: row.capabilities_hash,
        metadata_revision: row.metadata_revision,
        published: row.published,
        online,
        created_at: row.created_at,
        updated_at: row.updated_at,
        last_seen_at: row.last_seen_at,
        player_count: row.player_count,
        player_capacity: row.player_capacity,
        server_time: now,
    }
}

/// The authenticated read of a realm's own record. It does not advance the replay counter (a read changes nothing), so it is accepted with any timestamp inside the window.
async fn self_info<S: Store>(State(st): State<Arc<AppState<S>>>, ConnectInfo(Peer(peer)): ConnectInfo<Peer>, Path(realm_text): Path<String>, req: Request<Body>) -> Response {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let ip = client_ip(&parts.headers, peer, st.cfg.trust_proxy);
    let mut realm_for_log = None;
    let result: Result<(StatusCode, Response, String), ApiError> = async {
        admit(&st, ip, 1.0)?;
        let bytes = read_body(body).await?;
        if !bytes.is_empty() {
            return Err(ApiError::malformed("a read has no body"));
        }
        let (realm, _) = authenticate(&st, ip, &parts, &realm_text, &bytes, false).await?;
        realm_for_log = Some(realm);
        let row = st.store.get(&realm).await?.ok_or(StoreError::Unknown)?;
        let caps = row.capabilities_hash.clone();
        let (status, response) = json(&self_response(row, st.now(), st.cfg.online_ttl_secs));
        Ok((status, response, caps))
    }
    .await;
    finish("self", realm_for_log.as_ref(), started, result)
}
