//! The messages of a runtime portable session. Transport-neutral: a Registry/Relay later only has to carry these bytes.
//!
//! Every message is canonical JSON with unknown fields refused; snapshots travel in the existing snapshot envelope
//! (zstd of canonical JSON, SHA-256 of the canonical JSON). Size limits are checked before anything is parsed.

use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::super::error::{PortableError, Result};
use super::super::ids::{CharacterId, PortableItemId, PortablePetId, SessionId};
use super::super::model::{limits::MAX_COMPRESSED_BYTES, PortableCharacter};
use super::super::projection::{ProgressionPin, ProjectionContext};
use super::super::snapshot;

/// Version 3: `SessionOffer`, `PortableSessionStarted`, `PortableCheckpoint`, and `OwnerAck` require explicit `ruleset`.
/// Version 2 (Phase 8): `PortableSessionStarted` carried progression and pins.
pub const SESSION_PROTOCOL_VERSION: u32 = 3;
/// Protocol version for collection synchronization (`CollectionObserved`, `CollectionState`, `CollectionAck`).
pub const COLLECTION_PROTOCOL_VERSION: u32 = 2;
/// Backward-compatibility alias for session protocol version.
pub const PROTOCOL_VERSION: u32 = SESSION_PROTOCOL_VERSION;
/// A message carries at most one compressed snapshot (base64: 4/3) plus a little structure.
pub const MAX_MESSAGE_BYTES: usize = MAX_COMPRESSED_BYTES * 4 / 3 + 64 * 1024;
/// The most item/pet ids an acknowledgement lists.
pub const MAX_ACK_IDS: usize = 20_000;
/// The largest serialized collection message accepted.
pub const MAX_COLLECTION_MESSAGE_BYTES: usize = 1024 * 1024;

/// A sealed snapshot: the hash of the canonical JSON and the compressed bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub content_hash: String,
    pub payload: String,
}

impl Envelope {
    pub fn seal(model: &PortableCharacter) -> Result<Envelope> {
        let encoded = snapshot::encode(model)?;
        Ok(Envelope {
            content_hash: hex::encode(encoded.content_hash),
            payload: base64::engine::general_purpose::STANDARD.encode(&encoded.payload),
        })
    }

    pub fn from_encoded(encoded: &snapshot::EncodedSnapshot) -> Envelope {
        Envelope {
            content_hash: hex::encode(encoded.content_hash),
            payload: base64::engine::general_purpose::STANDARD.encode(&encoded.payload),
        }
    }

    pub fn hash(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.content_hash)
            .map_err(|_| PortableError::Invalid("the content hash is not hex".into()))?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| PortableError::Invalid("the content hash is not 32 bytes".into()))
    }

    /// Decode and fully verify (size caps while inflating, hash, structure).
    pub fn open(&self) -> Result<PortableCharacter> {
        if self.payload.len() > MAX_COMPRESSED_BYTES * 4 / 3 + 8 {
            return Err(PortableError::LimitExceeded(
                "the snapshot payload is over the limit".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.payload)
            .map_err(|_| PortableError::Invalid("the snapshot payload is not base64".into()))?;
        snapshot::decode(&bytes, Some(&self.hash()?))
    }

    /// The raw compressed bytes, for storage next to their hash.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.payload)
            .map_err(|_| PortableError::Invalid("the snapshot payload is not base64".into()))
    }
}

/// Owner -> Host: a character to put on the realm, and the session that will follow it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOffer {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub canonical_revision: u64,
    pub snapshot: Envelope,
}

/// The progression a session runs under: what the realm's cap and rules were when it started, and, when the character is above the cap,
/// the projection the working copy was made with. Nothing in a session may change it: a message under another pin is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProgression {
    pub pin: ProgressionPin,
    pub projection: Option<ProjectionContext>,
}

impl SessionProgression {
    pub fn validate(&self) -> Result<()> {
        self.pin.validate()?;
        match (&self.projection, self.pin.projected) {
            (Some(ctx), true) => {
                ctx.validate()?;
                if ctx.pin() != self.pin {
                    return Err(PortableError::Invalid(
                        "the projection is not the one the pin names".into(),
                    ));
                }
            }
            (None, false) => {}
            _ => {
                return Err(PortableError::Invalid(
                    "a projected pin needs its projection and a native one has none".into(),
                ))
            }
        }
        Ok(())
    }
}

/// Host -> Owner: the realm's own first load/save of the character was captured, before any gameplay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSessionStarted {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub base_canonical_revision: u64,
    pub baseline_generation: u32,
    pub b0: Envelope,
    pub content_hash: String,
    /// The progression the session runs under; `None` when the Host does not know its realm's progression (no profile).
    pub progression: Option<SessionProgression>,
}

/// Host -> Owner: the realm's state now. `sequence` is monotonic per session; `final_checkpoint` ends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableCheckpoint {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub base_canonical_revision: u64,
    pub sequence: u64,
    pub final_checkpoint: bool,
    pub realm_snapshot: Envelope,
    pub content_hash: String,
    /// The pin the session started under (`None` as the start said).
    pub pin: Option<ProgressionPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "status",
    content = "reason",
    rename_all = "snake_case"
)]
pub enum AckOutcome {
    Applied,
    /// The same sequence with the same content was applied before; the original result is repeated.
    Duplicate,
    /// An older sequence than the Owner already holds: ignored.
    StaleSequence,
    /// The session was superseded (another session or revision moved the character): nothing is accepted.
    StaleSession,
    Rejected(String),
}

impl AckOutcome {
    /// The Owner holds this state (applied now or before).
    pub fn accepted(&self) -> bool {
        matches!(self, AckOutcome::Applied | AckOutcome::Duplicate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextSession {
    pub session_id: SessionId,
    pub canonical_revision: u64,
}

/// Owner -> Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAck {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub session_id: SessionId,
    /// 0 acknowledges `PortableSessionStarted`.
    pub sequence: u64,
    pub outcome: AckOutcome,
    pub canonical_revision: u64,
    pub canonical_hash: String,
    /// The items and pets the canonical character owns after this checkpoint (what the Host maps as `present`).
    pub owned_items: Vec<PortableItemId>,
    pub owned_pets: Vec<PortablePetId>,
    /// Only on the acknowledgement of a final checkpoint: the canonical character, which the realm is now synchronised with.
    pub canonical: Option<Envelope>,
    /// Only on the acknowledgement of a final checkpoint: the session that follows.
    pub next_session: Option<NextSession>,
    /// The pin the Owner holds for this session (`None` before the start was accepted, or when the session has none).
    pub pin: Option<ProgressionPin>,
}

pub fn to_json<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!(
            "a message is limited to {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

/// Parse one message. The size is checked before the bytes are looked at, unknown fields are refused, the protocol version
/// is checked before anything else is trusted.
pub fn from_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!(
            "a message is limited to {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("protocol_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| PortableError::Invalid("the message has no protocol version".into()))?;
    if version != PROTOCOL_VERSION as u64 {
        return Err(PortableError::UnsupportedFormat {
            found: version.min(u32::MAX as u64) as u32,
            supported: PROTOCOL_VERSION,
        });
    }
    Ok(serde_json::from_value(value)?)
}

impl PortableSessionStarted {
    pub fn new(
        session_id: SessionId,
        character_id: CharacterId,
        server_id: &str,
        base_canonical_revision: u64,
        baseline_generation: u32,
        b0: &PortableCharacter,
        progression: Option<SessionProgression>,
    ) -> Result<Self> {
        if let Some(p) = &progression {
            p.validate()?;
        }
        let sealed_b0 = Envelope::seal(b0)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            ruleset: b0.ruleset,
            session_id,
            character_id,
            server_id: server_id.to_string(),
            base_canonical_revision,
            baseline_generation,
            content_hash: sealed_b0.content_hash.clone(),
            b0: sealed_b0,
            progression,
        })
    }
}

impl PortableCheckpoint {
    pub fn new(
        session_id: SessionId,
        character_id: CharacterId,
        server_id: &str,
        base_canonical_revision: u64,
        sequence: u64,
        final_checkpoint: bool,
        b1: &PortableCharacter,
        pin: Option<ProgressionPin>,
    ) -> Result<Self> {
        let realm_snapshot = Envelope::seal(b1)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            ruleset: b1.ruleset,
            session_id,
            character_id,
            server_id: server_id.to_string(),
            base_canonical_revision,
            sequence,
            final_checkpoint,
            content_hash: realm_snapshot.content_hash.clone(),
            realm_snapshot,
            pin,
        })
    }
}

// ---- account collections (Phase 6) --------------------------------------------------------------------------------------------
//
// A collection is a set of ids that only grows (the wardrobe, the vanity items). It never rides in a character checkpoint: the
// Host sends it when the hash of what its realm shows changed, and the Owner answers with the canonical set only when that has
// ids the realm lacks. The set travels in its compact form (sorted, delta + varint, base64), never as a JSON array of ids.

use super::super::collection::{valid_kind, IdSet, MAX_ENCODED_BYTES};

/// The collection kinds the protocol carries.
pub const COLLECTION_KINDS: [&str; 6] = [
    "coa:appearance",
    "coa:vanity",
    "wildcard:appearance",
    "wildcard:vanity",
    "reborn:appearance",
    "reborn:vanity",
];

pub fn collection_kinds(ruleset: super::super::model::Ruleset) -> &'static [&'static str] {
    match ruleset {
        super::super::model::Ruleset::Coa => &["coa:appearance", "coa:vanity"],
        super::super::model::Ruleset::Wildcard => &["wildcard:appearance", "wildcard:vanity"],
        super::super::model::Ruleset::Reborn => &["reborn:appearance", "reborn:vanity"],
    }
}

pub fn carried_kind(kind: &str) -> bool {
    valid_kind(kind) && COLLECTION_KINDS.contains(&kind)
}

pub fn carried_kind_for(kind: &str, ruleset: super::super::model::Ruleset) -> bool {
    valid_kind(kind) && collection_kinds(ruleset).contains(&kind)
}

/// A compact id set as it travels: hash of the kind and content, number of ids, base64 of [`IdSet::encode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactSet {
    pub hash: String,
    pub count: u32,
    pub ids: String,
}

impl CompactSet {
    pub fn seal(kind: &str, set: &IdSet) -> Result<CompactSet> {
        Ok(CompactSet {
            hash: hex::encode(set.hash(kind)),
            count: u32::try_from(set.len())
                .map_err(|_| PortableError::LimitExceeded("a collection is too large".into()))?,
            ids: base64::engine::general_purpose::STANDARD.encode(set.encode()),
        })
    }

    /// Decode and verify against the kind: size cap before decoding, then count and hash must match what was announced.
    pub fn open(&self, kind: &str) -> Result<IdSet> {
        if self.ids.len() > MAX_ENCODED_BYTES * 4 / 3 + 8 {
            return Err(PortableError::LimitExceeded(
                "the collection payload is over the limit".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.ids)
            .map_err(|_| PortableError::Invalid("the collection payload is not base64".into()))?;
        let set = IdSet::decode(&bytes)?;
        if set.len() != self.count as usize {
            return Err(PortableError::Invalid(
                "the collection does not have the announced number of ids".into(),
            ));
        }
        if hex::encode(set.hash(kind)) != self.hash {
            return Err(PortableError::Invalid(
                "the collection does not match its hash".into(),
            ));
        }
        Ok(set)
    }

    pub fn hash_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.hash)
            .map_err(|_| PortableError::Invalid("the collection hash is not hex".into()))?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| PortableError::Invalid("the collection hash is not 32 bytes".into()))
    }
}

/// Host -> Owner: the realm account shows this set of one kind. Sent only when its hash changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionObserved {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub server_id: String,
    pub kind: String,
    pub set: CompactSet,
}

/// Owner -> Host: the canonical set of one kind at a revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionState {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub kind: String,
    pub collection_revision: u64,
    pub set: CompactSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "status",
    content = "reason",
    rename_all = "snake_case"
)]
pub enum CollectionOutcome {
    /// The union added ids: a new collection revision.
    Applied,
    /// The Owner already held everything the realm showed.
    Unchanged,
    Rejected(String),
}

/// Owner -> Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionAck {
    pub protocol_version: u32,
    pub ruleset: super::super::model::Ruleset,
    pub kind: String,
    pub outcome: CollectionOutcome,
    pub collection_revision: u64,
    pub collection_hash: String,
    /// Present only when the canonical set has ids the realm did not show: the realm applies what it knows of them.
    pub canonical: Option<CollectionState>,
}

pub fn collection_to_json<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_COLLECTION_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!(
            "a collection message is limited to {MAX_COLLECTION_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

/// Parse a collection message: size first, protocol version second, unknown fields refused.
pub fn collection_from_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_COLLECTION_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!(
            "a collection message is limited to {MAX_COLLECTION_MESSAGE_BYTES} bytes"
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("protocol_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| PortableError::Invalid("the message has no protocol version".into()))?;
    if version != COLLECTION_PROTOCOL_VERSION as u64 {
        return Err(PortableError::UnsupportedFormat {
            found: version.min(u32::MAX as u64) as u32,
            supported: COLLECTION_PROTOCOL_VERSION,
        });
    }
    Ok(serde_json::from_value(value)?)
}

impl CollectionObserved {
    pub fn new(
        server_id: &str,
        ruleset: super::super::model::Ruleset,
        kind: &str,
        set: &IdSet,
    ) -> Result<Self> {
        if !carried_kind_for(kind, ruleset) {
            return Err(PortableError::Invalid(format!(
                "collection kind {kind:?} is not carried for {ruleset:?}"
            )));
        }
        Ok(Self {
            protocol_version: COLLECTION_PROTOCOL_VERSION,
            ruleset,
            server_id: server_id.to_string(),
            kind: kind.to_string(),
            set: CompactSet::seal(kind, set)?,
        })
    }
}

impl CollectionState {
    pub fn new(
        ruleset: super::super::model::Ruleset,
        kind: &str,
        revision: u64,
        set: &IdSet,
    ) -> Result<Self> {
        Ok(Self {
            protocol_version: COLLECTION_PROTOCOL_VERSION,
            ruleset,
            kind: kind.to_string(),
            collection_revision: revision,
            set: CompactSet::seal(kind, set)?,
        })
    }
}

/// Legacy Session Protocol v2 representation (Phase 8), where absence of ruleset is defined to mean CoA.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyV2SessionOffer {
    pub protocol_version: u32,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub canonical_revision: u64,
    pub snapshot: Envelope,
}

impl LegacyV2SessionOffer {
    pub fn into_session_offer(self) -> SessionOffer {
        SessionOffer {
            protocol_version: SESSION_PROTOCOL_VERSION,
            ruleset: super::super::model::Ruleset::Coa,
            session_id: self.session_id,
            character_id: self.character_id,
            server_id: self.server_id,
            canonical_revision: self.canonical_revision,
            snapshot: self.snapshot,
        }
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use crate::portable::Ruleset;

    #[test]
    fn session_protocol_v3_requires_ruleset_and_rejects_missing_or_unknown() {
        let envelope = Envelope {
            content_hash: "00".repeat(32),
            payload: "AA==".to_string(),
        };

        // 1. Valid v3 SessionOffer with explicit ruleset (CoA, Wildcard, Reborn)
        for r in [Ruleset::Coa, Ruleset::Wildcard, Ruleset::Reborn] {
            let offer = SessionOffer {
                protocol_version: SESSION_PROTOCOL_VERSION,
                ruleset: r,
                session_id: SessionId::new(),
                character_id: CharacterId::new(),
                server_id: "realm-1".to_string(),
                canonical_revision: 1,
                snapshot: envelope.clone(),
            };
            let bytes = to_json(&offer).unwrap();
            let parsed: SessionOffer = from_json(&bytes).unwrap();
            assert_eq!(parsed.ruleset, r);
        }

        // 2. Missing ruleset in v3 JSON is rejected (never silently defaults to CoA)
        let missing_ruleset_v3 = serde_json::json!({
            "protocol_version": SESSION_PROTOCOL_VERSION,
            "session_id": SessionId::new().to_string(),
            "character_id": CharacterId::new().to_string(),
            "server_id": "realm-1",
            "canonical_revision": 1,
            "snapshot": {
                "content_hash": "00".repeat(32),
                "payload": "AA=="
            }
        });
        let bytes = serde_json::to_vec(&missing_ruleset_v3).unwrap();
        let err: std::result::Result<SessionOffer, _> = from_json(&bytes);
        assert!(
            err.is_err(),
            "missing ruleset must be rejected in protocol v3"
        );

        // 3. Unknown ruleset in v3 JSON is rejected
        let unknown_ruleset_v3 = serde_json::json!({
            "protocol_version": SESSION_PROTOCOL_VERSION,
            "ruleset": "cyberpunk",
            "session_id": SessionId::new().to_string(),
            "character_id": CharacterId::new().to_string(),
            "server_id": "realm-1",
            "canonical_revision": 1,
            "snapshot": {
                "content_hash": "00".repeat(32),
                "payload": "AA=="
            }
        });
        let bytes = serde_json::to_vec(&unknown_ruleset_v3).unwrap();
        let err: std::result::Result<SessionOffer, _> = from_json(&bytes);
        assert!(err.is_err(), "unknown ruleset must be rejected");

        // 4. Old v2 peer message (protocol_version 2) is rejected by current v3 parser
        let legacy_v2_json = serde_json::json!({
            "protocol_version": 2,
            "session_id": SessionId::new().to_string(),
            "character_id": CharacterId::new().to_string(),
            "server_id": "realm-1",
            "canonical_revision": 1,
            "snapshot": {
                "content_hash": "00".repeat(32),
                "payload": "AA=="
            }
        });
        let bytes = serde_json::to_vec(&legacy_v2_json).unwrap();
        let err: std::result::Result<SessionOffer, _> = from_json(&bytes);
        assert!(
            matches!(
                err,
                Err(PortableError::UnsupportedFormat {
                    found: 2,
                    supported: 3
                })
            ),
            "v2 peer must be rejected with UnsupportedFormat"
        );

        // 5. Explicit LegacyV2SessionOffer decode correctly interprets absence of ruleset as CoA
        let v2_parsed: LegacyV2SessionOffer = serde_json::from_slice(&bytes).unwrap();
        let upgraded = v2_parsed.into_session_offer();
        assert_eq!(upgraded.ruleset, Ruleset::Coa);
        assert_eq!(upgraded.protocol_version, SESSION_PROTOCOL_VERSION);
    }
}
