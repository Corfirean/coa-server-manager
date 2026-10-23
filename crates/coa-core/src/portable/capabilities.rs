//! What a realm can take (Phase 7): the **content profile** of a destination, transport-neutral and small, so that a Registry can
//! later publish it unchanged and a Manager can decide before it writes anything.
//!
//! ```text
//! RealmCapabilities
//!   core         who built the realm's core (commit, branch, date): identity, not content; not part of the hash
//!   content      ContentProfile: everything that decides what can be applied, and the only thing the hash covers
//!   content_profile_hash   SHA-256 of the canonical JSON of `content`
//! ```
//!
//! The client-data catalog is carried as the **SHA-256 and record count of each client table** the portable features depend on
//! (`Appearances.dbc`, `ItemAppearances.dbc`, `VanityCollection.dbc`, `ItemSet.dbc`), never as the tens of thousands of ids in them:
//! two realms with the same hashes know exactly the same appearances. The ids themselves are read from the realm's own data directory
//! by the side that writes to the realm ([`RealmKnowledge`](super::realm::knowledge::RealmKnowledge)), and that read is checked
//! against these hashes.
//!
//! A change of anything in `content` changes `content_profile_hash`; the Manager remembers the hash a realm had when a character was last
//! synchronised with it and re-evaluates what it held back when the hash moves, even if the canonical revision did not.
//!
//! **Progression (Phase 8)** is not part of the content hash: it says how far a character may progress on the realm (`MaxPlayerLevel`),
//! under which rules the core projects a higher-level character down to it, and an opaque signature of everything those rules read.
//! Profile version 1 had no such block; a stored v1 profile is *migrated* (its content and hash are unchanged) with no progression, which
//! every operation reads as "the realm must be probed again" and never as "no cap".

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::{PortableError, Result};
use super::model::Ruleset;

/// Version of this profile's shape.
pub const PROFILE_VERSION: u32 = 2;
/// The profile version this Manager migrates from.
pub const PROFILE_VERSION_V1: u32 = 1;
/// The most extension namespaces a profile lists.
pub const MAX_PROFILE_EXTENSIONS: usize = 64;
/// The largest serialised profile accepted from outside.
pub const MAX_PROFILE_BYTES: usize = 256 * 1024;

/// A portable feature a realm can support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// A running session: the core holds the player until the baseline is taken, saves one character on request.
    RuntimeSessions,
    /// The selected appearances of a character (`PortableCharacter::wardrobe`).
    Wardrobe,
    /// Account collections (`coa:appearance`, `coa:vanity`).
    Collections,
    /// The core projects a character above its level cap down to it (`portable project`, the job's `projection` header).
    LevelProjection,
}

impl Feature {
    pub fn as_str(&self) -> &'static str {
        match self {
            Feature::RuntimeSessions => "runtime_sessions",
            Feature::Wardrobe => "wardrobe",
            Feature::Collections => "collections",
            Feature::LevelProjection => "level_projection",
        }
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An inclusive range of format versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatRange {
    pub min: u32,
    pub max: u32,
}

impl FormatRange {
    pub fn new(min: u32, max: u32) -> Self {
        Self { min, max }
    }
    pub fn single(v: u32) -> Self {
        Self { min: v, max: v }
    }
    pub fn contains(&self, v: u32) -> bool {
        self.min <= v && v <= self.max
    }
    fn valid(&self) -> bool {
        self.min >= 1 && self.min <= self.max
    }
}

impl std::fmt::Display for FormatRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{}..{}", self.min, self.max)
        }
    }
}

/// The portable character formats a side can read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterFormats {
    pub readable: FormatRange,
    pub writable: FormatRange,
}

/// One client table: the SHA-256 of the file and its record count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub sha256: String,
    pub records: u32,
}

/// The client tables the portable features depend on, by file name. A table the realm does not have is not listed.
pub type ClientCatalog = BTreeMap<String, CatalogEntry>;

/// The client tables a profile is about.
pub const CATALOG_TABLES: [&str; 4] = [
    "Appearances.dbc",
    "ItemAppearances.dbc",
    "VanityCollection.dbc",
    "ItemSet.dbc",
];

/// An extension namespace a realm can apply and the format versions it understands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSupport {
    pub namespace: String,
    pub module_version: String,
    pub formats: FormatRange,
}

/// Who built the realm's core. Identity for logs and support; it does not decide what can be applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreIdentity {
    pub commit: String,
    pub branch: String,
    pub date: String,
}

/// Everything that decides what a realm can be given. Its hash is the `content_profile_hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentProfile {
    pub ruleset: Ruleset,
    /// The character formats the Manager side that serves this realm reads and writes.
    pub character_formats: CharacterFormats,
    /// The job formats the realm's core accepts for an online import (empty: no online import, or the core was not asked).
    pub online_import_job_formats: Vec<u32>,
    /// Version of the session protocol (`PortableSessionStarted`, `PortableCheckpoint`, `OwnerAck`).
    pub session_protocol: u32,
    /// Version of the collection messages (`CollectionObserved`, `CollectionState`, `CollectionAck`).
    pub collection_protocol: u32,
    pub features: BTreeSet<Feature>,
    pub collection_kinds: BTreeSet<String>,
    /// Sorted by namespace.
    pub extensions: Vec<ExtensionSupport>,
    pub client_catalog: ClientCatalog,
}

impl ContentProfile {
    /// SHA-256 of the canonical JSON (fixed field order, sorted sets and maps).
    pub fn hash(&self) -> Result<[u8; 32]> {
        let mut canonical = self.clone();
        canonical
            .extensions
            .sort_by(|a, b| a.namespace.cmp(&b.namespace));
        let json = serde_json::to_vec(&canonical)?;
        let mut h = Sha256::new();
        h.update(b"coa-content-profile-v1\0");
        h.update(json);
        Ok(h.finalize().into())
    }

    pub fn supports(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }

    pub fn extension(&self, namespace: &str) -> Option<&ExtensionSupport> {
        self.extensions.iter().find(|e| e.namespace == namespace)
    }

    /// The character formats a Manager of this version reads and writes.
    pub fn manager_formats() -> CharacterFormats {
        CharacterFormats {
            readable: FormatRange::new(
                super::versions::PORTABLE_CHARACTER_MIN_READ_VERSION,
                super::versions::PORTABLE_CHARACTER_FORMAT_VERSION,
            ),
            writable: FormatRange::single(super::versions::PORTABLE_CHARACTER_FORMAT_VERSION),
        }
    }
}

/// How far a character may progress on a realm and what its core projects to get there (Phase 8). From the running core, never guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progression {
    /// `MaxPlayerLevel` of the running core. It cannot change while the core runs.
    pub max_player_level: u32,
    /// Version of the projection exchange (`portable project`, the job's `projection` header).
    pub projection_protocol: u32,
    /// Version of the projection policy (what is held at a level, and how).
    pub projection_policy_version: u32,
    /// Opaque, deterministic: moves whenever the cap, the content-scaling layout, the progression tables or the effective item levels move.
    pub progression_signature: String,
    pub scaling_enabled: bool,
}

impl Progression {
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| PortableError::Invalid(format!("realm progression: {what}"));
        if self.max_player_level == 0 || self.max_player_level > 255 {
            return Err(bad("the level cap is out of range"));
        }
        if self.projection_protocol == 0 || self.projection_policy_version == 0 {
            return Err(bad("a version is 0"));
        }
        if self.progression_signature.len() != 64
            || !self
                .progression_signature
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(bad("the signature is not a SHA-256"));
        }
        Ok(())
    }

    /// The words the core stores in `coa.portable.pin` for this progression (see [`super::projection::ProgressionPin::words`]).
    pub fn pin_words(&self) -> Vec<u32> {
        super::projection::ProgressionPin::native(self, &"0".repeat(64)).words()
    }
}

/// A realm's capabilities as they are published and compared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmCapabilities {
    pub profile_version: u32,
    pub core: Option<CoreIdentity>,
    pub content: ContentProfile,
    /// Lower-case hex of [`ContentProfile::hash`].
    pub content_profile_hash: String,
    /// `None`: not known (a profile migrated from version 1, a stopped realm never probed through its core). Never read as "no cap".
    #[serde(default)]
    pub progression: Option<Progression>,
}

/// Profile version 1, as it was stored before Phase 8.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RealmCapabilitiesV1 {
    #[allow(dead_code)]
    profile_version: u32,
    core: Option<CoreIdentity>,
    content: ContentProfileV1,
    content_profile_hash: String,
}

/// Content of a version 1 profile: no `level_projection` feature can be in it.
type ContentProfileV1 = ContentProfile;

impl RealmCapabilities {
    pub fn new(core: Option<CoreIdentity>, content: ContentProfile) -> Result<Self> {
        Self::build(core, content, None)
    }

    /// A profile with the progression of the realm's core (or without it, for a realm whose core could not be asked).
    pub fn build(
        core: Option<CoreIdentity>,
        mut content: ContentProfile,
        progression: Option<Progression>,
    ) -> Result<Self> {
        content
            .extensions
            .sort_by(|a, b| a.namespace.cmp(&b.namespace));
        let content_profile_hash = hex::encode(content.hash()?);
        let caps = Self {
            profile_version: PROFILE_VERSION,
            core,
            content,
            content_profile_hash,
            progression,
        };
        caps.validate()?;
        Ok(caps)
    }

    pub fn with_progression(mut self, progression: Progression) -> Result<Self> {
        progression.validate()?;
        self.progression = Some(progression);
        Ok(self)
    }

    /// The realm's progression is not known: every operation that depends on the cap needs a probe through the core first.
    pub fn needs_reprobe(&self) -> bool {
        self.progression.is_none()
    }

    pub fn progression_signature(&self) -> Option<&str> {
        self.progression
            .as_ref()
            .map(|p| p.progression_signature.as_str())
    }

    pub fn hash(&self) -> &str {
        &self.content_profile_hash
    }

    /// Structural validation, and the hash must be the one of the content: a profile cannot claim another content's hash.
    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| PortableError::Invalid(format!("realm capabilities: {what}"));
        if self.profile_version != PROFILE_VERSION {
            return Err(PortableError::UnsupportedFormat {
                found: self.profile_version,
                supported: PROFILE_VERSION,
            });
        }
        let c = &self.content;
        if !c.character_formats.readable.valid() || !c.character_formats.writable.valid() {
            return Err(bad("a character format range is empty or starts below 1"));
        }
        if c.online_import_job_formats.contains(&0)
            || c.online_import_job_formats.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(bad(
                "the online import job formats must be ascending, unique and at least 1",
            ));
        }
        if c.session_protocol == 0 || c.collection_protocol == 0 {
            return Err(bad("a protocol version is 0"));
        }
        if c.extensions.len() > MAX_PROFILE_EXTENSIONS {
            return Err(bad("too many extension namespaces"));
        }
        for (i, e) in c.extensions.iter().enumerate() {
            if !super::ids::valid_namespace(&e.namespace)
                || !e.formats.valid()
                || e.module_version.is_empty()
                || e.module_version.len() > 64
            {
                return Err(bad(&format!("extension {:?} is not valid", e.namespace)));
            }
            if i > 0 && c.extensions[i - 1].namespace >= e.namespace {
                return Err(bad("extensions must be sorted by namespace and unique"));
            }
        }
        for kind in &c.collection_kinds {
            if !super::collection::valid_kind(kind) {
                return Err(bad(&format!("collection kind {kind:?} is not valid")));
            }
        }
        for (name, entry) in &c.client_catalog {
            let known = CATALOG_TABLES.contains(&name.as_str());
            let hex_ok = entry.sha256.len() == 64
                && entry
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !known || !hex_ok {
                return Err(bad(&format!("client table {name:?} is not valid")));
            }
        }
        if hex::encode(c.hash()?) != self.content_profile_hash {
            return Err(bad("the content profile hash does not match the content"));
        }
        match &self.progression {
            Some(p) => p.validate()?,
            None if c.supports(Feature::LevelProjection) => {
                return Err(bad(
                    "level projection is claimed without a progression block",
                ))
            }
            None => {}
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(PortableError::LimitExceeded(format!(
                "a profile is limited to {MAX_PROFILE_BYTES} bytes"
            )));
        }
        Ok(bytes)
    }

    /// Parse and validate a profile from outside: size first, version before anything else is trusted, unknown fields refused. A version 1
    /// profile is migrated (see the module documentation); a newer one than this Manager knows is refused.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(PortableError::LimitExceeded(format!(
                "a profile is limited to {MAX_PROFILE_BYTES} bytes"
            )));
        }
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value
            .get("profile_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| PortableError::Invalid("the profile has no version".into()))?;
        if version == PROFILE_VERSION_V1 as u64 {
            return Self::migrate_v1(value);
        }
        if version != PROFILE_VERSION as u64 {
            return Err(PortableError::UnsupportedFormat {
                found: version.min(u32::MAX as u64) as u32,
                supported: PROFILE_VERSION,
            });
        }
        let caps: RealmCapabilities = serde_json::from_value(value)?;
        caps.validate()?;
        Ok(caps)
    }

    fn migrate_v1(value: serde_json::Value) -> Result<Self> {
        let old: RealmCapabilitiesV1 = serde_json::from_value(value)?;
        if old.content.supports(Feature::LevelProjection) {
            return Err(PortableError::Invalid(
                "a version 1 profile cannot claim level projection".into(),
            ));
        }
        let caps = Self {
            profile_version: PROFILE_VERSION,
            core: old.core,
            content: old.content,
            content_profile_hash: old.content_profile_hash,
            progression: None,
        };
        caps.validate()?;
        Ok(caps)
    }
}

/// What the realm's core says about itself in answer to `portable capabilities` (RA). Strict: the Manager trusts nothing it does not
/// recognise.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreReport {
    pub report_version: u32,
    pub ruleset: String,
    pub core: CoreIdentity,
    pub portable: CorePortable,
    /// Report version 2 and later. A version 1 core cannot say what its cap is.
    #[serde(default)]
    pub progression: Option<Progression>,
    pub extension_namespaces: Vec<String>,
    pub client_data: BTreeMap<String, Option<CatalogEntry>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorePortable {
    pub job_formats: Vec<u32>,
    pub character_formats: Vec<u32>,
    pub session_marker_version: u32,
    pub features: Vec<String>,
}

impl CoreReport {
    /// Parse the single JSON line of the console's answer. Anything else in the answer is ignored; the JSON object must be there.
    pub fn parse(answer: &str) -> Result<CoreReport> {
        let line = answer
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with('{'))
            .ok_or_else(|| {
                PortableError::Invalid(
                    "the core did not answer with a capability report (an older core?)".into(),
                )
            })?;
        if line.len() > MAX_PROFILE_BYTES {
            return Err(PortableError::LimitExceeded(
                "the core's capability report is too large".into(),
            ));
        }
        let report: CoreReport = serde_json::from_str(line).map_err(|e| {
            PortableError::Invalid(format!("the core's capability report is not valid: {e}"))
        })?;
        if report.report_version != 1 && report.report_version != 2 {
            return Err(PortableError::UnsupportedFormat {
                found: report.report_version,
                supported: 2,
            });
        }
        match (&report.progression, report.report_version) {
            (Some(p), 2) => p.validate()?,
            (None, 1) => {}
            (Some(_), _) => {
                return Err(PortableError::Invalid(
                    "a version 1 report cannot carry a progression block".into(),
                ))
            }
            (None, _) => {
                return Err(PortableError::Invalid(
                    "the core's capability report has no progression block".into(),
                ))
            }
        }
        if report.ruleset != "coa" {
            return Err(PortableError::Invalid(format!(
                "the core reports ruleset {:?}; only coa is carried",
                report.ruleset
            )));
        }
        for (name, entry) in &report.client_data {
            if !CATALOG_TABLES.contains(&name.as_str()) {
                return Err(PortableError::Invalid(format!(
                    "the core reports an unknown client table {name:?}"
                )));
            }
            if let Some(e) = entry {
                if e.sha256.len() != 64
                    || !e
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(PortableError::Invalid(format!(
                        "the hash of {name} is not a SHA-256"
                    )));
                }
            }
        }
        Ok(report)
    }

    pub fn has_feature(&self, name: &str) -> bool {
        self.portable.features.iter().any(|f| f == name)
    }

    /// The catalog as the core loaded it (tables it could not read are absent).
    pub fn catalog(&self) -> ClientCatalog {
        self.client_data
            .iter()
            .filter_map(|(k, v)| v.clone().map(|e| (k.clone(), e)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ContentProfile {
        let mut catalog = ClientCatalog::new();
        catalog.insert(
            "Appearances.dbc".into(),
            CatalogEntry {
                sha256: "ab".repeat(32),
                records: 42_903,
            },
        );
        catalog.insert(
            "VanityCollection.dbc".into(),
            CatalogEntry {
                sha256: "cd".repeat(32),
                records: 10_764,
            },
        );
        ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: CharacterFormats {
                readable: FormatRange::new(1, 2),
                writable: FormatRange::single(2),
            },
            online_import_job_formats: vec![2],
            session_protocol: 1,
            collection_protocol: 1,
            features: [
                Feature::RuntimeSessions,
                Feature::Wardrobe,
                Feature::Collections,
            ]
            .into_iter()
            .collect(),
            collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()]
                .into_iter()
                .collect(),
            extensions: vec![ExtensionSupport {
                namespace: "mod:fake".into(),
                module_version: "1.2.0".into(),
                formats: FormatRange::new(1, 3),
            }],
            client_catalog: catalog,
        }
    }

    #[test]
    fn the_profile_round_trips_and_its_hash_covers_the_content_but_not_the_core_identity() {
        let a = RealmCapabilities::new(
            Some(CoreIdentity {
                commit: "aaaa".into(),
                branch: "x".into(),
                date: "d1".into(),
            }),
            sample(),
        )
        .unwrap();
        let b = RealmCapabilities::new(
            Some(CoreIdentity {
                commit: "bbbb".into(),
                branch: "y".into(),
                date: "d2".into(),
            }),
            sample(),
        )
        .unwrap();
        assert_eq!(
            a.content_profile_hash, b.content_profile_hash,
            "a new core build with the same content is the same content profile"
        );
        assert_ne!(a, b);
        let back = RealmCapabilities::from_json(&a.to_json().unwrap()).unwrap();
        assert_eq!(back, a);

        // every part of the content moves the hash
        let base = a.content_profile_hash.clone();
        let mut changed: Vec<ContentProfile> = Vec::new();
        let mut c = sample();
        c.features.remove(&Feature::Wardrobe);
        changed.push(c);
        let mut c = sample();
        c.client_catalog.get_mut("Appearances.dbc").unwrap().sha256 = "ee".repeat(32);
        changed.push(c);
        let mut c = sample();
        c.extensions[0].formats = FormatRange::new(1, 4);
        changed.push(c);
        let mut c = sample();
        c.online_import_job_formats = vec![2, 3];
        changed.push(c);
        let mut c = sample();
        c.collection_kinds.insert("coa:mounts".into());
        changed.push(c);
        let mut c = sample();
        c.session_protocol = 2;
        changed.push(c);
        for c in changed {
            assert_ne!(
                RealmCapabilities::new(None, c)
                    .unwrap()
                    .content_profile_hash,
                base
            );
        }
    }

    #[test]
    fn a_profile_from_outside_is_validated_and_a_forged_hash_is_refused() {
        let caps = RealmCapabilities::new(None, sample()).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&caps.to_json().unwrap()).unwrap();
        let bytes = |v: &serde_json::Value| serde_json::to_vec(v).unwrap();

        let mut forged = value.clone();
        forged["content_profile_hash"] = "00".repeat(32).into();
        assert!(
            RealmCapabilities::from_json(&bytes(&forged)).is_err(),
            "the hash must be the content's"
        );
        let mut edited = value.clone();
        edited["content"]["features"] = serde_json::json!(["wardrobe"]);
        assert!(
            RealmCapabilities::from_json(&bytes(&edited)).is_err(),
            "content edited without its hash"
        );
        let mut unknown = value.clone();
        unknown["extra"] = 1.into();
        assert!(RealmCapabilities::from_json(&bytes(&unknown)).is_err());
        let mut newer = value.clone();
        newer["profile_version"] = 3.into();
        assert!(matches!(
            RealmCapabilities::from_json(&bytes(&newer)),
            Err(PortableError::UnsupportedFormat { .. })
        ));
        value["content"]["client_catalog"]["Appearances.dbc"]["sha256"] = "not hex".into();
        assert!(RealmCapabilities::from_json(&bytes(&value)).is_err());
        assert!(RealmCapabilities::from_json(&vec![b' '; MAX_PROFILE_BYTES + 1]).is_err());
        assert!(RealmCapabilities::from_json(b"[]").is_err());

        let mut bad = sample();
        bad.extensions.push(ExtensionSupport {
            namespace: "mod:fake".into(),
            module_version: "1".into(),
            formats: FormatRange::single(1),
        });
        assert!(
            RealmCapabilities::new(None, bad).is_err(),
            "a namespace twice"
        );
        let mut bad = sample();
        bad.character_formats.readable = FormatRange::new(3, 2);
        assert!(RealmCapabilities::new(None, bad).is_err());
        let mut bad = sample();
        bad.client_catalog.insert(
            "Whatever.dbc".into(),
            CatalogEntry {
                sha256: "ab".repeat(32),
                records: 1,
            },
        );
        assert!(RealmCapabilities::new(None, bad).is_err());
    }

    #[test]
    fn a_version_one_profile_is_migrated_with_no_progression_and_never_with_a_made_up_one() {
        let caps = RealmCapabilities::new(None, sample()).unwrap();
        let mut old: serde_json::Value = serde_json::from_slice(&caps.to_json().unwrap()).unwrap();
        old["profile_version"] = 1.into();
        old.as_object_mut().unwrap().remove("progression");
        let migrated = RealmCapabilities::from_json(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(migrated.profile_version, PROFILE_VERSION);
        assert!(migrated.needs_reprobe() && migrated.progression.is_none());
        assert_eq!(
            migrated.content_profile_hash, caps.content_profile_hash,
            "the content, and its hash, are the same"
        );
        let mut with_block = old.clone();
        with_block["progression"] = serde_json::json!({"max_player_level": 60});
        assert!(
            RealmCapabilities::from_json(&serde_json::to_vec(&with_block).unwrap()).is_err(),
            "a v1 profile has no progression to carry"
        );
    }

    #[test]
    fn progression_is_validated_and_its_pin_words_are_stable() {
        let p = Progression {
            max_player_level: 60,
            projection_protocol: 1,
            projection_policy_version: 1,
            progression_signature: "ab".repeat(32),
            scaling_enabled: true,
        };
        let caps = RealmCapabilities::build(None, sample(), Some(p.clone())).unwrap();
        assert_eq!(
            RealmCapabilities::from_json(&caps.to_json().unwrap()).unwrap(),
            caps
        );
        assert_eq!(
            p.pin_words(),
            vec![
                1, 60, 0xabababab, 0xabababab, 0xabababab, 0xabababab, 0xabababab, 0xabababab,
                0xabababab, 0xabababab
            ]
        );
        for bad in [
            Progression {
                max_player_level: 0,
                ..p.clone()
            },
            Progression {
                projection_protocol: 0,
                ..p.clone()
            },
            Progression {
                progression_signature: "zz".into(),
                ..p.clone()
            },
        ] {
            assert!(RealmCapabilities::build(None, sample(), Some(bad)).is_err());
        }
        let mut claims = sample();
        claims.features.insert(Feature::LevelProjection);
        assert!(
            RealmCapabilities::build(None, claims, None).is_err(),
            "level projection is claimed only with the progression block"
        );
    }

    #[test]
    fn the_core_report_is_parsed_strictly() {
        let line = r#"{"report_version":1,"ruleset":"coa","core":{"commit":"abc","branch":"feat/x","date":"2026-10-06"},"portable":{"job_formats":[2],"character_formats":[2],"session_marker_version":1,"features":["runtime_sessions","wardrobe"]},"extension_namespaces":[],"client_data":{"Appearances.dbc":{"sha256":"abababababababababababababababababababababababababababababababab","records":3},"ItemSet.dbc":null}}"#;
        let report = CoreReport::parse(&format!("noise\n{line}\n")).unwrap();
        assert!(report.has_feature("wardrobe") && !report.has_feature("collections"));
        assert_eq!(
            report.catalog().len(),
            1,
            "a table the core could not read is not in the catalog"
        );
        assert!(
            CoreReport::parse("Unknown command").is_err(),
            "an older core has no such command"
        );
        assert!(
            CoreReport::parse(&line.replace("\"report_version\":1", "\"report_version\":3"))
                .is_err()
        );
        assert!(
            CoreReport::parse(&line.replace("\"report_version\":1", "\"report_version\":2"))
                .is_err(),
            "a version 2 report must carry its progression block"
        );
        assert!(CoreReport::parse(&line.replace("\"coa\"", "\"wildcard\"")).is_err());
        assert!(
            CoreReport::parse(&line.replace("\"records\":3", "\"records\":3,\"x\":1")).is_err()
        );
        assert!(CoreReport::parse(&line.replace("Appearances.dbc", "Secret.dbc")).is_err());
        assert!(CoreReport::parse(&line.replace(
            "abababababababababababababababababababababababababababababababab",
            "zz"
        ))
        .is_err());
    }
}
