//! The Host's HTTP client of Registry protocol 1. Blocking, with hard timeouts; every mutating request is signed with the realm's own key.

use std::time::Duration;

use coa_registry_proto::sign::sign_request;
use coa_registry_proto::*;
use ed25519_dalek::SigningKey;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::result::Result;

/// Why a request did not succeed, in the terms the retry policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The network, a timeout, a 5xx or a rate limit: the same request may work later.
    Transient(String),
    /// The Registry understood the request and refused it with this code.
    Rejected {
        code: ErrorCode,
        status: u16,
        message: String,
    },
    /// The answer is not Registry protocol 1.
    Protocol(String),
}

impl ClientError {
    /// Repeating cannot help: the Host stops instead of retrying.
    pub fn is_permanent(&self) -> bool {
        match self {
            ClientError::Transient(_) => false,
            ClientError::Rejected { code, .. } => code.is_permanent(),
            ClientError::Protocol(_) => true,
        }
    }
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            ClientError::Rejected { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Transient(m) => write!(f, "the Registry could not be reached: {m}"),
            ClientError::Rejected { code, status, message } => write!(f, "the Registry refused the request ({status} {code:?}): {message}"),
            ClientError::Protocol(m) => write!(f, "the Registry answered something that is not protocol {REGISTRY_PROTOCOL_VERSION}: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

pub struct RegistryClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl RegistryClient {
    pub fn new(base: &str) -> crate::Result<Self> {
        let base = super::settings::validate_url(base)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("coa-manager/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        Ok(Self { base, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn send<R: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        key: &SigningKey,
        realm: &RealmId,
        ts: i64,
    ) -> Result<R, ClientError> {
        let headers = sign_request(key, method, path, realm, ts, &body);
        let url = format!("{}{}", self.base, path);
        let mut req = if method == "GET" {
            self.http.get(url)
        } else {
            self.http
                .post(url)
                .header("content-type", "application/json")
                .body(body)
        };
        for (name, value) in headers.pairs() {
            req = req.header(name, value);
        }
        let resp = req
            .send()
            .map_err(|e| ClientError::Transient(e.without_url().to_string()))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .map_err(|e| ClientError::Transient(e.without_url().to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map_err(|_| ClientError::Protocol("the answer cannot be read".into()));
        }
        match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(e) if status.as_u16() == 429 || status.as_u16() == 503 => Err(
                ClientError::Transient(format!("{:?}: {}", e.error.code, e.error.message)),
            ),
            Ok(e) => Err(ClientError::Rejected {
                code: e.error.code,
                status: status.as_u16(),
                message: e.error.message,
            }),
            Err(_)
                if status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429 =>
            {
                Err(ClientError::Transient(format!("HTTP {status}")))
            }
            Err(_) => Err(ClientError::Protocol(format!(
                "HTTP {status} without a Registry error body"
            ))),
        }
    }

    fn post<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        key: &SigningKey,
        realm: &RealmId,
        ts: i64,
    ) -> Result<R, ClientError> {
        let bytes = serde_json::to_vec(body).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.send("POST", path, bytes, key, realm, ts)
    }

    pub fn register(
        &self,
        key: &SigningKey,
        req: &RegisterRequest,
        ts: i64,
    ) -> Result<RegisterResponse, ClientError> {
        self.post(PATH_REGISTER, req, key, &req.realm_id, ts)
    }

    pub fn heartbeat(
        &self,
        key: &SigningKey,
        realm: &RealmId,
        req: &HeartbeatRequest,
        ts: i64,
    ) -> Result<HeartbeatResponse, ClientError> {
        self.post(&path_heartbeat(realm), req, key, realm, ts)
    }

    pub fn unpublish(
        &self,
        key: &SigningKey,
        realm: &RealmId,
        ts: i64,
    ) -> Result<UnpublishResponse, ClientError> {
        self.post(
            &path_unpublish(realm),
            &UnpublishRequest {
                protocol_version: REGISTRY_PROTOCOL_VERSION,
            },
            key,
            realm,
            ts,
        )
    }

    /// The Registry's own record of this realm (the authenticated read).
    pub fn self_info(
        &self,
        key: &SigningKey,
        realm: &RealmId,
        ts: i64,
    ) -> Result<RealmDetail, ClientError> {
        self.send("GET", &path_self(realm), Vec::new(), key, realm, ts)
    }
}
