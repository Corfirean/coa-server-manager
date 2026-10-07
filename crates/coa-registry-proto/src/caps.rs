//! The form of `RealmCapabilities` (Manager profile version 2) that a realm advertises to the Registry.
//!
//! The types mirror `coa_core::portable::capabilities` field for field (same names, same order: the content hash is taken over the JSON,
//! so the order is part of the contract), with the limits of a public, hostile-input boundary added. `coa-core` has a test that a real
//! profile parses into this type and keeps its hash, so the two cannot drift apart unnoticed. What the Registry stores is host-advertised
//! discovery metadata: a future JOIN negotiates the capabilities again with the live Host.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{invalid, Result};

pub const PROFILE_VERSION: u32 = 2;
pub const MAX_EXTENSIONS: usize = 64;
pub const MAX_COLLECTION_KINDS: usize = 16;
pub const MAX_JOB_FORMATS: usize = 16;
pub const MAX_CATALOG_ENTRIES: usize = 8;
const MAX_TOKEN: usize = 64;
const MAX_IDENTITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ruleset {
    Coa,
    Wildcard,
}

impl Ruleset {
    pub fn as_str(self) -> &'static str {
        match self {
            Ruleset::Coa => "coa",
            Ruleset::Wildcard => "wildcard",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    RuntimeSessions,
    Wardrobe,
    Collections,
    LevelProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterFormats {
    pub readable: FormatRange,
    pub writable: FormatRange,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub sha256: String,
    pub records: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSupport {
    pub namespace: String,
    pub module_version: String,
    pub formats: FormatRange,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreIdentity {
    pub commit: String,
    pub branch: String,
    pub date: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentProfile {
    pub ruleset: Ruleset,
    pub character_formats: CharacterFormats,
    pub online_import_job_formats: Vec<u32>,
    pub session_protocol: u32,
    pub collection_protocol: u32,
    pub features: BTreeSet<Feature>,
    pub collection_kinds: BTreeSet<String>,
    pub extensions: Vec<ExtensionSupport>,
    pub client_catalog: BTreeMap<String, CatalogEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progression {
    pub max_player_level: u32,
    pub projection_protocol: u32,
    pub projection_policy_version: u32,
    pub progression_signature: String,
    pub scaling_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvertisedCapabilities {
    pub profile_version: u32,
    pub core: Option<CoreIdentity>,
    pub content: ContentProfile,
    pub content_profile_hash: String,
    #[serde(default)]
    pub progression: Option<Progression>,
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn token(s: &str, what: &str) -> Result<()> {
    if s.is_empty() || s.len() > MAX_TOKEN || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'+')) {
        return invalid(format!("capabilities: {what} is not a short token"));
    }
    Ok(())
}

fn identity_text(s: &str, what: &str) -> Result<()> {
    if s.len() > MAX_IDENTITY || s.chars().any(char::is_control) {
        return invalid(format!("capabilities: {what} is too long or has control characters"));
    }
    Ok(())
}

fn range(r: &FormatRange, what: &str) -> Result<()> {
    if r.min < 1 || r.min > r.max || r.max > 1000 {
        return invalid(format!("capabilities: {what} is not a sane version range"));
    }
    Ok(())
}

impl ContentProfile {
    /// SHA-256 of the canonical JSON, as the Manager computes it (`coa-content-profile-v1\0` + the JSON with the extensions sorted).
    pub fn hash(&self) -> String {
        let mut canonical = self.clone();
        canonical.extensions.sort_by(|a, b| a.namespace.cmp(&b.namespace));
        let json = serde_json::to_vec(&canonical).expect("a profile serialises");
        let mut h = Sha256::new();
        h.update(b"coa-content-profile-v1\0");
        h.update(json);
        hex::encode(h.finalize())
    }
}

impl AdvertisedCapabilities {
    /// Bounds on everything, and the declared hash must be the hash of the content.
    pub fn validate(&self) -> Result<()> {
        if self.profile_version != PROFILE_VERSION {
            return invalid(format!("capabilities: profile version {} is not supported (expected {PROFILE_VERSION})", self.profile_version));
        }
        let c = &self.content;
        range(&c.character_formats.readable, "the readable character formats")?;
        range(&c.character_formats.writable, "the writable character formats")?;
        if c.online_import_job_formats.len() > MAX_JOB_FORMATS || c.online_import_job_formats.iter().any(|v| *v == 0 || *v > 1000) {
            return invalid("capabilities: the import job formats are out of range");
        }
        if c.session_protocol > 1000 || c.collection_protocol > 1000 {
            return invalid("capabilities: a protocol version is out of range");
        }
        if c.collection_kinds.len() > MAX_COLLECTION_KINDS {
            return invalid("capabilities: too many collection kinds");
        }
        for k in &c.collection_kinds {
            token(k, "a collection kind")?;
        }
        if c.extensions.len() > MAX_EXTENSIONS {
            return invalid("capabilities: too many extensions");
        }
        let mut seen = BTreeSet::new();
        for e in &c.extensions {
            token(&e.namespace, "an extension namespace")?;
            token(&e.module_version, "an extension version")?;
            range(&e.formats, "an extension's formats")?;
            if !seen.insert(&e.namespace) {
                return invalid("capabilities: an extension is listed twice");
            }
        }
        if c.client_catalog.len() > MAX_CATALOG_ENTRIES {
            return invalid("capabilities: too many client tables");
        }
        for (name, entry) in &c.client_catalog {
            if name.is_empty() || name.len() > MAX_TOKEN || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
                return invalid("capabilities: a client table name is not a plain file name");
            }
            if !hex64(&entry.sha256) || entry.records > 100_000_000 {
                return invalid("capabilities: a client table entry is malformed");
            }
        }
        if let Some(core) = &self.core {
            identity_text(&core.commit, "the core commit")?;
            identity_text(&core.branch, "the core branch")?;
            identity_text(&core.date, "the core date")?;
        }
        if let Some(p) = &self.progression {
            if p.max_player_level == 0 || p.max_player_level > 255 || p.projection_protocol == 0 || p.projection_protocol > 1000 || p.projection_policy_version == 0 || p.projection_policy_version > 1000 {
                return invalid("capabilities: the progression numbers are out of range");
            }
            if !hex64(&p.progression_signature) {
                return invalid("capabilities: the progression signature is not a SHA-256");
            }
        }
        if !hex64(&self.content_profile_hash) {
            return invalid("capabilities: the content hash is not a SHA-256");
        }
        if self.content.hash() != self.content_profile_hash {
            return invalid("capabilities: the content hash is not the hash of the content");
        }
        Ok(())
    }

    /// The running core's `MaxPlayerLevel`, when the realm has reported its progression.
    pub fn level_cap(&self) -> Option<u32> {
        self.progression.as_ref().map(|p| p.max_player_level)
    }

    /// The hash that identifies this advertisement (the content hash plus the progression signature: both change what a Host can take).
    pub fn advert_hash(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"coa-registry-advert-v1\0");
        h.update(self.content_profile_hash.as_bytes());
        match &self.progression {
            Some(p) => {
                h.update(b"\x01");
                h.update(p.max_player_level.to_be_bytes());
                h.update(p.projection_protocol.to_be_bytes());
                h.update(p.projection_policy_version.to_be_bytes());
                h.update(p.progression_signature.as_bytes());
                h.update([p.scaling_enabled as u8]);
            }
            None => h.update(b"\x00"),
        }
        hex::encode(h.finalize())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> AdvertisedCapabilities {
        let content = ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: CharacterFormats { readable: FormatRange { min: 1, max: 2 }, writable: FormatRange { min: 2, max: 2 } },
            online_import_job_formats: vec![2],
            session_protocol: 2,
            collection_protocol: 1,
            features: [Feature::RuntimeSessions, Feature::Wardrobe, Feature::LevelProjection].into_iter().collect(),
            collection_kinds: ["coa:appearance".to_string()].into_iter().collect(),
            extensions: vec![ExtensionSupport { namespace: "coa".into(), module_version: "1".into(), formats: FormatRange { min: 1, max: 1 } }],
            client_catalog: [("Appearances.dbc".to_string(), CatalogEntry { sha256: "a".repeat(64), records: 5 })].into_iter().collect(),
        };
        let content_profile_hash = content.hash();
        AdvertisedCapabilities {
            profile_version: 2,
            core: Some(CoreIdentity { commit: "abc123".into(), branch: "main".into(), date: "2026-10-01".into() }),
            content,
            content_profile_hash,
            progression: Some(Progression { max_player_level: 60, projection_protocol: 1, projection_policy_version: 1, progression_signature: "b".repeat(64), scaling_enabled: false }),
        }
    }

    #[test]
    fn a_sound_advertisement_validates_and_a_lying_hash_does_not() {
        sample().validate().unwrap();
        let mut c = sample();
        c.content_profile_hash = "0".repeat(64);
        assert!(c.validate().is_err());
        let mut c = sample();
        c.content.session_protocol = 3;
        assert!(c.validate().is_err(), "the content changed under the same hash");
    }

    #[test]
    fn hostile_content_is_bounded() {
        let mut c = sample();
        c.content.extensions = (0..65).map(|i| ExtensionSupport { namespace: format!("ns{i}"), module_version: "1".into(), formats: FormatRange { min: 1, max: 1 } }).collect();
        c.content_profile_hash = c.content.hash();
        assert!(c.validate().is_err());
        let mut c = sample();
        c.content.client_catalog.insert("../../x".into(), CatalogEntry { sha256: "a".repeat(64), records: 1 });
        c.content_profile_hash = c.content.hash();
        assert!(c.validate().is_err());
        let mut c = sample();
        c.core.as_mut().unwrap().branch = "x".repeat(10_000);
        assert!(c.validate().is_err());
        let mut c = sample();
        c.content.collection_kinds.insert("<script>".into());
        c.content_profile_hash = c.content.hash();
        assert!(c.validate().is_err());
        assert!(serde_json::from_str::<AdvertisedCapabilities>(r#"{"profile_version":2,"extra":1}"#).is_err(), "unknown fields");
    }

    #[test]
    fn the_advert_hash_moves_with_the_progression() {
        let a = sample();
        let mut b = sample();
        b.progression.as_mut().unwrap().max_player_level = 70;
        assert_ne!(a.advert_hash(), b.advert_hash());
        assert_eq!(a.advert_hash(), sample().advert_hash());
        let mut c = sample();
        c.progression = None;
        assert_ne!(a.advert_hash(), c.advert_hash());
    }
}
