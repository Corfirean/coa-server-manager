//! The Player Manager's read side of the Registry: the public list and one realm's detail. Plain HTTP GETs with hard limits; nothing is signed and
//! nothing about the player is sent (no identity, no character, no account: the Registry is asked the same question by everybody).

use std::io::Read;
use std::time::Duration;

use coa_registry_proto::*;
use serde::de::DeserializeOwned;

use super::client::ClientError;

/// The largest answer read (a full page of 100 summaries is well under this).
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;

pub struct BrowseClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl BrowseClient {
    pub fn new(base: &str) -> crate::Result<Self> {
        let base = super::settings::validate_url(base)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("coa-manager/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        Ok(Self { base, http })
    }

    fn get<T: DeserializeOwned>(&self, path_and_query: &str) -> std::result::Result<T, ClientError> {
        let resp = self.http.get(format!("{}{}", self.base, path_and_query)).send().map_err(|e| ClientError::Transient(e.without_url().to_string()))?;
        let status = resp.status();
        let mut bytes = Vec::new();
        resp.take(MAX_ANSWER_BYTES).read_to_end(&mut bytes).map_err(|e| ClientError::Transient(e.to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes).map_err(|_| ClientError::Protocol("the answer cannot be read".into()));
        }
        match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(e) if status.as_u16() == 429 || status.as_u16() == 503 => Err(ClientError::Transient(format!("{:?}", e.error.code))),
            Ok(e) => Err(ClientError::Rejected { code: e.error.code, status: status.as_u16(), message: e.error.message }),
            Err(_) if status.is_server_error() || status.as_u16() == 429 => Err(ClientError::Transient(format!("HTTP {status}"))),
            Err(_) => Err(ClientError::Protocol(format!("HTTP {status}"))),
        }
    }

    /// One page of the public list. The query is checked and normalised by the protocol's own parser before it is sent.
    pub fn list(&self, query: &ListQuery) -> std::result::Result<RealmPage, ClientError> {
        self.get(&format!("{PATH_LIST}?{}", query.canonical()))
    }

    pub fn detail(&self, realm: &RealmId) -> std::result::Result<RealmDetail, ClientError> {
        self.get(&path_detail(realm))
    }
}

/// What the browser's controls ask for; every field optional, checked by [`ListQuery::parse`].
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct BrowseParams {
    pub q: Option<String>,
    pub ruleset: Option<String>,
    pub cap_min: Option<u32>,
    pub cap_max: Option<u32>,
    pub module: Option<String>,
    pub players_min: Option<u32>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl BrowseParams {
    pub fn to_query(&self) -> crate::Result<ListQuery> {
        let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
        let mut parts = Vec::new();
        let mut text = |name: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
                parts.push(format!("{name}={}", enc(v)));
            }
        };
        text("q", &self.q);
        text("ruleset", &self.ruleset);
        text("module", &self.module);
        text("status", &self.status);
        text("sort", &self.sort);
        text("order", &self.order);
        text("cursor", &self.cursor);
        let mut num = |name: &str, v: Option<u32>| {
            if let Some(v) = v {
                parts.push(format!("{name}={v}"));
            }
        };
        num("cap_min", self.cap_min);
        num("cap_max", self.cap_max);
        num("players_min", self.players_min);
        num("limit", self.limit);
        ListQuery::parse(Some(&parts.join("&"))).map_err(|e| crate::Error::Invalid(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_become_a_checked_query() {
        let p = BrowseParams { q: Some("Desc ension".into()), ruleset: Some("coa".into()), cap_min: Some(60), module: Some("playerbots".into()), sort: Some("name".into()), ..Default::default() };
        let q = p.to_query().unwrap();
        assert_eq!((q.q.as_deref(), q.cap_min, q.module.as_deref()), (Some("Desc ension"), Some(60), Some("playerbots")));
        assert!(q.canonical().contains("q=Desc%20ension"));
        assert!(BrowseParams { ruleset: Some("normal".into()), ..Default::default() }.to_query().is_err());
        assert!(BrowseParams { module: Some("<script>".into()), ..Default::default() }.to_query().is_err());
        assert!(BrowseParams { limit: Some(0), ..Default::default() }.to_query().is_err());
        assert!(BrowseParams::default().to_query().is_ok());
    }
}
