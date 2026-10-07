//! Request and response bodies of Registry protocol 2. Every request type refuses unknown fields and validates its own bounds.
//!
//! A realm's advertisement has three parts with three lifetimes: the **listing** (what a player reads: name, description, language, region, rates, modules,
//! how accounts are made; changes rarely), the **capabilities** (what the realm can take, hash-checked; changes rarely) and the **population** (players and
//! bots; changes every heartbeat). The level cap and the ruleset are not claimed separately: the Registry derives them from the hash-checked capabilities,
//! which come from the running core.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::caps::{AdvertisedCapabilities, Ruleset};
use crate::sign::RealmId;
use crate::{invalid, Result, MAX_DESCRIPTION_BYTES, MAX_DISPLAY_NAME_CHARS, MAX_LANGUAGE_BYTES, MAX_MODULES, MAX_PLAYER_NUMBER, MAX_RATE, MAX_REGION_BYTES, MAX_VERSION_BYTES, REGISTRY_PROTOCOL_VERSION};

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

/// A coarse region such as `EU`, `NA`, `RU`, `EU-West`.
pub fn validate_region(s: &str) -> Result<()> {
    let ok = (2..=MAX_REGION_BYTES).contains(&s.len()) && s.split('-').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if !ok {
        return invalid("the region is not a short region tag");
    }
    Ok(())
}

pub fn validate_version(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > MAX_VERSION_BYTES || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_')) {
        return invalid("a version is not a short version string");
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

/// What the realm's configuration says; `None` is "not known" and is never a guess.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rates {
    /// `Rate.XP.Kill`
    #[serde(default)]
    pub xp_kill: Option<f64>,
    /// `Rate.XP.Quest`
    #[serde(default)]
    pub xp_quest: Option<f64>,
    /// `Rate.XP.Explore`
    #[serde(default)]
    pub xp_explore: Option<f64>,
    /// `Rate.Drop.Item.Normal`
    #[serde(default)]
    pub loot: Option<f64>,
    /// `Rate.Drop.Money`
    #[serde(default)]
    pub money: Option<f64>,
    /// `Rate.Reputation.Gain`
    #[serde(default)]
    pub reputation: Option<f64>,
    /// `Rate.Honor`
    #[serde(default)]
    pub honor: Option<f64>,
}

impl Rates {
    pub fn validate(&self) -> Result<()> {
        for v in [self.xp_kill, self.xp_quest, self.xp_explore, self.loot, self.money, self.reputation, self.honor].into_iter().flatten() {
            if !v.is_finite() || !(0.0..=MAX_RATE).contains(&v) {
                return invalid(format!("a rate is not a number between 0 and {MAX_RATE}"));
            }
        }
        Ok(())
    }
}

/// A module the realm's server build contains. The Registry carries the stable id and the version only; a Player Manager renders names and descriptions
/// from its own module catalog, and shows an id it does not know as plain text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleEntry {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    pub enabled: bool,
}

pub fn validate_module_id(s: &str) -> Result<()> {
    let ok = !s.is_empty() && s.len() <= 64 && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'));
    if !ok {
        return invalid("a module id is not a short lower-case token");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Population {
    /// Human players with a game session.
    pub players: u32,
    /// Characters played by the bot subsystem.
    pub bots: u32,
    /// The player limit, when the realm has one.
    #[serde(default)]
    pub capacity: Option<u32>,
}

impl Population {
    pub fn validate(&self) -> Result<()> {
        if self.players > MAX_PLAYER_NUMBER || self.bots > MAX_PLAYER_NUMBER || self.capacity.is_some_and(|c| c > MAX_PLAYER_NUMBER) {
            return invalid("a population number is out of range");
        }
        if self.capacity.is_some_and(|c| self.players > c) {
            return invalid("more players than capacity");
        }
        Ok(())
    }
}

/// How a player gets a game account on this realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountProvisioning {
    /// The Host creates the account when a player joins.
    pub automatic: bool,
    /// A player can link an account the realm already has.
    pub existing_only: bool,
}

impl AccountProvisioning {
    pub fn validate(&self) -> Result<()> {
        if !self.automatic && !self.existing_only {
            return invalid("account provisioning offers no way to get an account");
        }
        Ok(())
    }
}

/// What a player reads about a realm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub display_name: String,
    pub description: String,
    pub language: String,
    #[serde(default)]
    pub region: Option<String>,
    pub rates: Rates,
    pub modules: Vec<ModuleEntry>,
    pub account_provisioning: AccountProvisioning,
    pub manager_version: String,
}

impl Listing {
    pub fn validate(&self) -> Result<()> {
        validate_display_name(&self.display_name)?;
        validate_description(&self.description)?;
        validate_language(&self.language)?;
        if let Some(r) = &self.region {
            validate_region(r)?;
        }
        validate_version(&self.manager_version)?;
        self.rates.validate()?;
        self.account_provisioning.validate()?;
        if self.modules.len() > MAX_MODULES {
            return invalid("too many modules");
        }
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.modules {
            validate_module_id(&m.id)?;
            if let Some(v) = &m.version {
                validate_version(v)?;
            }
            if !seen.insert(&m.id) {
                return invalid("a module is listed twice");
            }
        }
        Ok(())
    }

    /// SHA-256 over `coa-registry-listing-v2\0` and the JSON of the listing in its fixed field order (the Registry recomputes it from what it parsed).
    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"coa-registry-listing-v2\0");
        h.update(serde_json::to_vec(self).expect("a listing serialises"));
        hex::encode(h.finalize())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub protocol_version: u32,
    pub realm_id: RealmId,
    /// Base64url (no padding) of the 32-byte Ed25519 public key.
    pub public_key: String,
    pub listing: Listing,
    /// [`Listing::hash`]
    pub listing_hash: String,
    pub capabilities: AdvertisedCapabilities,
    /// [`AdvertisedCapabilities::advert_hash`]
    pub capabilities_hash: String,
    pub population: Population,
}

impl RegisterRequest {
    pub fn validate(&self) -> Result<()> {
        check_version(self.protocol_version)?;
        crate::sign::decode_public_key(&self.public_key)?;
        self.listing.validate()?;
        if !hex64(&self.listing_hash) || self.listing_hash != self.listing.hash() {
            return invalid("the listing hash is not the hash of the listing");
        }
        self.capabilities.validate()?;
        if !hex64(&self.capabilities_hash) || self.capabilities_hash != self.capabilities.advert_hash() {
            return invalid("the capabilities hash is not the hash of the capabilities");
        }
        self.population.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatRequest {
    pub protocol_version: u32,
    /// The hash of the listing the Host advertises now.
    pub listing_hash: String,
    /// Present when the Host's listing hash differs from the one the Registry last acknowledged.
    #[serde(default)]
    pub listing: Option<Listing>,
    pub capabilities_hash: String,
    /// Present when the Host's capabilities hash differs from the one the Registry last acknowledged.
    #[serde(default)]
    pub capabilities: Option<AdvertisedCapabilities>,
    pub population: Population,
}

impl HeartbeatRequest {
    pub fn validate(&self) -> Result<()> {
        check_version(self.protocol_version)?;
        if !hex64(&self.listing_hash) || !hex64(&self.capabilities_hash) {
            return invalid("a hash is not a SHA-256");
        }
        if let Some(l) = &self.listing {
            l.validate()?;
            if l.hash() != self.listing_hash {
                return invalid("the listing hash is not the hash of the listing");
            }
        }
        if let Some(c) = &self.capabilities {
            c.validate()?;
            if c.advert_hash() != self.capabilities_hash {
                return invalid("the capabilities hash is not the hash of the capabilities");
            }
        }
        self.population.validate()
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
    pub listing_hash: String,
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
    /// The hashes the Registry holds now.
    pub listing_hash: String,
    pub capabilities_hash: String,
    /// The heartbeat carried another hash without the part: send it with the next one (nothing of it was applied).
    pub resend_listing: bool,
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

/// What the Registry holds about one realm: the public detail, and the realm's own authenticated read (which also works while unpublished).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealmDetail {
    pub protocol_version: u32,
    pub realm_id: RealmId,
    pub public_key: String,
    pub listing: Listing,
    /// Derived from the capabilities, never claimed separately.
    pub ruleset: Ruleset,
    /// The running core's `MaxPlayerLevel`, from the capabilities' progression; `None` when the realm has not reported one.
    pub level_cap: Option<u32>,
    pub population: Population,
    pub capabilities: AdvertisedCapabilities,
    pub capabilities_hash: String,
    pub listing_hash: String,
    pub metadata_revision: u64,
    pub published: bool,
    /// Published and heard of within the online TTL.
    pub online: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
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
pub(crate) mod tests {
    use super::*;
    use crate::caps::tests::sample;

    pub(crate) fn listing() -> Listing {
        Listing {
            display_name: "Friends' realm".into(),
            description: "A small realm.\nPlain text.".into(),
            language: "en".into(),
            region: Some("EU".into()),
            rates: Rates { xp_kill: Some(2.0), xp_quest: Some(2.0), xp_explore: None, loot: Some(1.0), money: Some(1.5), reputation: None, honor: Some(1.0) },
            modules: vec![ModuleEntry { id: "playerbots".into(), version: Some("1.4.2".into()), enabled: true }, ModuleEntry { id: "content-scaling".into(), version: None, enabled: false }],
            account_provisioning: AccountProvisioning { automatic: true, existing_only: false },
            manager_version: "0.6.6".into(),
        }
    }

    pub(crate) fn register() -> RegisterRequest {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let capabilities = sample();
        let listing = listing();
        RegisterRequest {
            protocol_version: 2,
            realm_id: RealmId::parse("018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d").unwrap(),
            public_key: crate::sign::encode_public_key(&key.verifying_key()),
            listing_hash: listing.hash(),
            listing,
            capabilities_hash: capabilities.advert_hash(),
            capabilities,
            population: Population { players: 18, bots: 46, capacity: Some(100) },
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
        assert!(validate_region("EU").is_ok() && validate_region("EU-West").is_ok());
        for bad in ["", "E", "<b>", "EU West", "x".repeat(17).as_str()] {
            assert!(validate_region(bad).is_err(), "{bad:?}");
        }
        assert!(validate_version("0.6.6").is_ok() && validate_version("0.6.6-rc.1+b2").is_ok());
        assert!(validate_version("1 2").is_err() && validate_version("").is_err() && validate_version("1.4 · abc").is_err());
    }

    #[test]
    fn structured_fields_are_bounded() {
        let mut l = listing();
        l.rates.xp_kill = Some(f64::NAN);
        assert!(l.validate().is_err());
        l.rates.xp_kill = Some(-1.0);
        assert!(l.validate().is_err());
        l.rates.xp_kill = Some(1000.5);
        assert!(l.validate().is_err());
        l.rates.xp_kill = Some(1000.0);
        l.validate().unwrap();
        let mut l = listing();
        l.modules.push(ModuleEntry { id: "playerbots".into(), version: None, enabled: true });
        assert!(l.validate().is_err(), "a module twice");
        let mut l = listing();
        l.modules = (0..65).map(|i| ModuleEntry { id: format!("m{i}"), version: None, enabled: true }).collect();
        assert!(l.validate().is_err(), "too many modules");
        for bad in ["", "Bad", "<script>", "a b", "-x", "x".repeat(65).as_str()] {
            assert!(validate_module_id(bad).is_err(), "{bad:?}");
        }
        let mut l = listing();
        l.account_provisioning = AccountProvisioning { automatic: false, existing_only: false };
        assert!(l.validate().is_err());
        assert!(Population { players: 5, bots: 0, capacity: Some(4) }.validate().is_err());
        assert!(Population { players: 100_001, bots: 0, capacity: None }.validate().is_err());
        assert!(Population { players: 3, bots: 500, capacity: Some(10) }.validate().is_ok(), "bots do not count against the player capacity");
    }

    #[test]
    fn the_listing_hash_moves_with_every_field() {
        let base = listing().hash();
        let mut l = listing();
        l.rates.loot = Some(2.0);
        assert_ne!(l.hash(), base);
        let mut l = listing();
        l.modules[1].enabled = true;
        assert_ne!(l.hash(), base);
        let mut l = listing();
        l.region = None;
        assert_ne!(l.hash(), base);
        assert_eq!(listing().hash(), base);
        let reparsed: Listing = serde_json::from_slice(&serde_json::to_vec(&listing()).unwrap()).unwrap();
        assert_eq!(reparsed.hash(), base, "the hash survives a round trip through JSON, including the floats");
    }

    #[test]
    fn the_request_refuses_what_it_does_not_know_and_what_does_not_add_up() {
        let mut r = register();
        r.protocol_version = 1;
        assert!(r.validate().is_err());
        let mut r = register();
        r.listing_hash = "0".repeat(64);
        assert!(r.validate().is_err());
        let mut r = register();
        r.capabilities_hash = "0".repeat(64);
        assert!(r.validate().is_err());
        let mut r = register();
        r.listing.rates.honor = Some(5.0);
        assert!(r.validate().is_err(), "the listing changed under the same hash");
        for extra in ["character", "snapshot", "password", "ra_password", "ruleset", "level_cap", "game_mode"] {
            let mut value = serde_json::to_value(register()).unwrap();
            value.as_object_mut().unwrap().insert(extra.into(), serde_json::json!("x"));
            assert!(serde_json::from_value::<RegisterRequest>(value).is_err(), "{extra}: a field the protocol has no place for");
        }
        let mut value = serde_json::to_value(register()).unwrap();
        value["listing"]["rates"]["ping"] = serde_json::json!(1.0);
        assert!(serde_json::from_value::<RegisterRequest>(value).is_err(), "an unknown rate");
        let mut value = serde_json::to_value(register()).unwrap();
        value["realm_id"] = serde_json::json!("018F2D9E-5C3A-7B21-8C4D-0E5F6A7B8C9D");
        assert!(serde_json::from_value::<RegisterRequest>(value).is_err());
    }

    #[test]
    fn a_heartbeat_with_parts_must_hash_to_its_claim() {
        let caps = sample();
        let l = listing();
        let mut h = HeartbeatRequest { protocol_version: 2, listing_hash: l.hash(), listing: Some(l.clone()), capabilities_hash: caps.advert_hash(), capabilities: Some(caps.clone()), population: Population { players: 1, bots: 2, capacity: None } };
        h.validate().unwrap();
        h.listing_hash = "1".repeat(64);
        assert!(h.validate().is_err());
        h.listing = None;
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
