//! Request and response bodies of Registry protocol 1. Every request type refuses unknown fields and validates its own bounds.

use serde::{Deserialize, Serialize};

use crate::caps::{AdvertisedCapabilities, Ruleset};
use crate::sign::RealmId;
use crate::{invalid, Result, MAX_DESCRIPTION_BYTES, MAX_DISPLAY_NAME_CHARS, MAX_LANGUAGE_BYTES, MAX_PLAYER_NUMBER, MAX_VERSION_BYTES, REGISTRY_PROTOCOL_VERSION};

fn bidi_or_control(c: char, allow_newline: bool) -> bool {
    (c.is_control() && !(allow_newline && c == '\n')) || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{2028}' | '\u{2029}')
}

/// Plain text only: the Registry stores text and never HTML, and a consumer must render it as text.
pub fn validate_display_name(s: &str) -> Result<()> {
    let n = s.chars().count();
    if n == 0 || n > MAX_DISPLAY_NAME_CHARS {
        return invalid(format!("the display name must be 1 to {MAX_DISPLAY_NAME_CHARS} characters"));
    }
    if s != s.trim() || s.chars().any(|c| bidi_or_control(c, false)) {
        return invalid("the display name has control characters, direction overrides or surrounding spaces");
    }
    Ok(())
}

pub fn validate_description(s: &str) -> Result<()> {
    if s.len() > MAX_DESCRIPTION_BYTES {
        return invalid(format!("the description is limited to {MAX_DESCRIPTION_BYTES} bytes"));
    }
    if s.chars().any(|c| bidi_or_control(c, true)) {
        return invalid("the description has control characters or direction overrides");
    }
    Ok(())
}

/// A short language tag such as `en`, `ru`, `pt-BR`.
pub fn validate_language(s: &str) -> Result<()> {
    let ok = !s.is_empty() && s.len() <= MAX_LANGUAGE_BYTES && s.split('-').enumerate().all(|(i, part)| !part.is_empty() && part.len() <= 8 && part.bytes().all(|b| b.is_ascii_alphanumeric()) && (i > 0 || part.bytes().all(|b| b.is_ascii_lowercase()) && part.len() >= 2));
    if !ok {
        return invalid("the language is not a short language tag");
    }
    Ok(())
}

pub fn validate_version(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > MAX_VERSION_BYTES || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_')) {
        return invalid("the Manager version is not a short version string");
    }
    Ok(())
}

pub fn validate_players(count: Option<u32>, capacity: Option<u32>) -> Result<()> {
    if count.is_some_and(|n| n > MAX_PLAYER_NUMBER) || capacity.is_some_and(|n| n > MAX_PLAYER_NUMBER) {
        return invalid("a player number is out of range");
    }
    if let (Some(c), Some(cap)) = (count, capacity) {
        if c > cap {
            return invalid("more players than capacity");
        }
    }
    Ok(())
}

fn check_version(v: u32) -> Result<()> {
    if v != REGISTRY_PROTOCOL_VERSION {
        return invalid(format!("protocol version {v} is not supported"));
    }
    Ok(())
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub protocol_version: u32,
    pub realm_id: RealmId,
    /// Base64url (no padding) of the 32-byte Ed25519 public key.
    pub public_key: String,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub ruleset: Ruleset,
    pub manager_version: String,
    pub capabilities: AdvertisedCapabilities,
    /// [`AdvertisedCapabilities::advert_hash`]
    pub capabilities_hash: String,
    #[serde(default)]
    pub player_count: Option<u32>,
    #[serde(default)]
    pub player_capacity: Option<u32>,
}

impl RegisterRequest {
    pub fn validate(&self) -> Result<()> {
        check_version(self.protocol_version)?;
        crate::sign::decode_public_key(&self.public_key)?;
        validate_display_name(&self.display_name)?;
        validate_description(&self.description)?;
        validate_language(&self.language)?;
        validate_version(&self.manager_version)?;
        validate_players(self.player_count, self.player_capacity)?;
        self.capabilities.validate()?;
        if self.capabilities.content.ruleset != self.ruleset {
            return invalid("the ruleset is not the ruleset of the capabilities");
        }
        if !hex64(&self.capabilities_hash) || self.capabilities_hash != self.capabilities.advert_hash() {
            return invalid("the capabilities hash is not the hash of the capabilities");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatRequest {
    pub protocol_version: u32,
    /// The hash of the capabilities the Host advertises now.
    pub capabilities_hash: String,
    /// Present when the Host's hash differs from the one the Registry last acknowledged.
    #[serde(default)]
    pub capabilities: Option<AdvertisedCapabilities>,
    #[serde(default)]
    pub manager_version: Option<String>,
    #[serde(default)]
    pub player_count: Option<u32>,
    #[serde(default)]
    pub player_capacity: Option<u32>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

impl HeartbeatRequest {
    pub fn validate(&self) -> Result<()> {
        check_version(self.protocol_version)?;
        if !hex64(&self.capabilities_hash) {
            return invalid("the capabilities hash is not a SHA-256");
        }
        if let Some(c) = &self.capabilities {
            c.validate()?;
            if c.advert_hash() != self.capabilities_hash {
                return invalid("the capabilities hash is not the hash of the capabilities");
            }
        }
        if let Some(v) = &self.manager_version {
            validate_version(v)?;
        }
        if let Some(n) = &self.display_name {
            validate_display_name(n)?;
        }
        if let Some(d) = &self.description {
            validate_description(d)?;
        }
        if let Some(l) = &self.language {
            validate_language(l)?;
        }
        validate_players(self.player_count, self.player_capacity)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnpublishRequest {
    pub protocol_version: u32,
}

impl UnpublishRequest {
    pub fn validate(&self) -> Result<()> {
        check_version(self.protocol_version)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub protocol_version: u32,
    pub realm_id: RealmId,
    pub created: bool,
    pub metadata_revision: u64,
    pub published: bool,
    pub capabilities_hash: String,
    pub server_time: i64,
    pub heartbeat_interval_secs: u64,
    pub online_ttl_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub protocol_version: u32,
    pub metadata_revision: u64,
    pub published: bool,
    /// The hash the Registry holds now.
    pub capabilities_hash: String,
    /// The heartbeat carried another hash without the capabilities: send them with the next one.
    pub resend_capabilities: bool,
    pub server_time: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpublishResponse {
    pub protocol_version: u32,
    pub published: bool,
    pub metadata_revision: u64,
    pub server_time: i64,
}

/// What the Registry holds about one realm (the authenticated self endpoint; there is no public list yet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfResponse {
    pub protocol_version: u32,
    pub realm_id: RealmId,
    pub public_key: String,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub ruleset: Ruleset,
    pub manager_version: String,
    pub capabilities: AdvertisedCapabilities,
    pub capabilities_hash: String,
    pub metadata_revision: u64,
    pub published: bool,
    /// Published and heard of within the online TTL.
    pub online: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    pub player_count: Option<u32>,
    pub player_capacity: Option<u32>,
    pub server_time: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    MalformedRequest,
    UnsupportedProtocolVersion,
    RequestTooLarge,
    InvalidMetadata,
    InvalidRealmId,
    InvalidSignature,
    BadTimestamp,
    TimestampNotMonotonic,
    RealmKeyMismatch,
    UnknownRealm,
    NotPublished,
    RateLimited,
    Unavailable,
    Internal,
}

impl ErrorCode {
    /// Errors that repeating the same request cannot fix (the Host stops instead of retrying).
    pub fn is_permanent(self) -> bool {
        matches!(self, ErrorCode::MalformedRequest | ErrorCode::UnsupportedProtocolVersion | ErrorCode::RequestTooLarge | ErrorCode::InvalidMetadata | ErrorCode::InvalidRealmId | ErrorCode::InvalidSignature | ErrorCode::RealmKeyMismatch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub protocol_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::tests::sample;

    pub(crate) fn register() -> RegisterRequest {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let capabilities = sample();
        RegisterRequest {
            protocol_version: 1,
            realm_id: RealmId::parse("018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d").unwrap(),
            public_key: crate::sign::encode_public_key(&key.verifying_key()),
            display_name: "Friends' realm".into(),
            description: "A small realm.\nPlain text.".into(),
            language: "en".into(),
            ruleset: Ruleset::Coa,
            manager_version: "0.6.6".into(),
            capabilities_hash: capabilities.advert_hash(),
            capabilities,
            player_count: Some(2),
            player_capacity: Some(100),
        }
    }

    #[test]
    fn a_sound_registration_validates() {
        register().validate().unwrap();
    }

    #[test]
    fn text_is_plain_and_bounded() {
        for bad in ["", " lead", "trail ", "tab\there", "ctl\u{0007}", "rtl\u{202E}name", &"x".repeat(81)] {
            assert!(validate_display_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_display_name(&"я".repeat(80)).is_ok(), "80 characters, not bytes");
        assert!(validate_description(&"x".repeat(1025)).is_err());
        assert!(validate_description(&"я".repeat(600)).is_err(), "bytes, not characters");
        assert!(validate_description("a\nb").is_ok());
        assert!(validate_description("a\r\nb").is_err());
        assert!(validate_description("\u{2066}x").is_err());
        assert!(validate_language("pt-BR").is_ok() && validate_language("en").is_ok());
        for bad in ["", "E", "EN", "en_US", "en-", "-en", "<b>", "x".repeat(17).as_str()] {
            assert!(validate_language(bad).is_err(), "{bad:?}");
        }
        assert!(validate_version("0.6.6").is_ok() && validate_version("0.6.6-rc.1+b2").is_ok());
        assert!(validate_version("1 2").is_err() && validate_version("").is_err());
        assert!(validate_players(Some(5), Some(4)).is_err() && validate_players(Some(100_001), None).is_err() && validate_players(None, None).is_ok());
    }

    #[test]
    fn the_request_refuses_what_it_does_not_know_and_what_does_not_add_up() {
        let mut r = register();
        r.protocol_version = 2;
        assert!(r.validate().is_err());
        let mut r = register();
        r.ruleset = Ruleset::Wildcard;
        assert!(r.validate().is_err(), "ruleset and capabilities disagree");
        let mut r = register();
        r.capabilities_hash = "0".repeat(64);
        assert!(r.validate().is_err());
        let mut value = serde_json::to_value(register()).unwrap();
        value.as_object_mut().unwrap().insert("character".into(), serde_json::json!({"name": "x"}));
        assert!(serde_json::from_value::<RegisterRequest>(value).is_err(), "no field for a character, and none is accepted");
        let mut value = serde_json::to_value(register()).unwrap();
        value["realm_id"] = serde_json::json!("018F2D9E-5C3A-7B21-8C4D-0E5F6A7B8C9D");
        assert!(serde_json::from_value::<RegisterRequest>(value).is_err());
    }

    #[test]
    fn a_heartbeat_with_capabilities_must_hash_to_its_claim() {
        let caps = sample();
        let mut h = HeartbeatRequest { protocol_version: 1, capabilities_hash: caps.advert_hash(), capabilities: Some(caps.clone()), manager_version: None, player_count: None, player_capacity: None, display_name: None, description: None, language: None };
        h.validate().unwrap();
        h.capabilities_hash = "1".repeat(64);
        assert!(h.validate().is_err());
        h.capabilities = None;
        h.validate().unwrap();
    }

    #[test]
    fn permanent_and_transient_errors_are_told_apart() {
        assert!(ErrorCode::InvalidSignature.is_permanent() && ErrorCode::RealmKeyMismatch.is_permanent() && ErrorCode::UnsupportedProtocolVersion.is_permanent());
        assert!(!ErrorCode::RateLimited.is_permanent() && !ErrorCode::Unavailable.is_permanent() && !ErrorCode::BadTimestamp.is_permanent() && !ErrorCode::UnknownRealm.is_permanent());
    }
}
