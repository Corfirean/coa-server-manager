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
use super::super::snapshot;

pub const PROTOCOL_VERSION: u32 = 1;
/// A message carries at most one compressed snapshot (base64: 4/3) plus a little structure.
pub const MAX_MESSAGE_BYTES: usize = MAX_COMPRESSED_BYTES * 4 / 3 + 64 * 1024;
/// The most item/pet ids an acknowledgement lists.
pub const MAX_ACK_IDS: usize = 20_000;

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
        Ok(Envelope { content_hash: hex::encode(encoded.content_hash), payload: base64::engine::general_purpose::STANDARD.encode(&encoded.payload) })
    }

    pub fn from_encoded(encoded: &snapshot::EncodedSnapshot) -> Envelope {
        Envelope { content_hash: hex::encode(encoded.content_hash), payload: base64::engine::general_purpose::STANDARD.encode(&encoded.payload) }
    }

    pub fn hash(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.content_hash).map_err(|_| PortableError::Invalid("the content hash is not hex".into()))?;
        <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| PortableError::Invalid("the content hash is not 32 bytes".into()))
    }

    /// Decode and fully verify (size caps while inflating, hash, structure).
    pub fn open(&self) -> Result<PortableCharacter> {
        if self.payload.len() > MAX_COMPRESSED_BYTES * 4 / 3 + 8 {
            return Err(PortableError::LimitExceeded("the snapshot payload is over the limit".into()));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(&self.payload).map_err(|_| PortableError::Invalid("the snapshot payload is not base64".into()))?;
        snapshot::decode(&bytes, Some(&self.hash()?))
    }

    /// The raw compressed bytes, for storage next to their hash.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD.decode(&self.payload).map_err(|_| PortableError::Invalid("the snapshot payload is not base64".into()))
    }
}

/// Owner -> Host: a character to put on the realm, and the session that will follow it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOffer {
    pub protocol_version: u32,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub canonical_revision: u64,
    pub snapshot: Envelope,
}

/// Host -> Owner: the realm's own first load/save of the character was captured, before any gameplay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSessionStarted {
    pub protocol_version: u32,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub base_canonical_revision: u64,
    pub baseline_generation: u32,
    pub b0: Envelope,
    pub content_hash: String,
}

/// Host -> Owner: the realm's state now. `sequence` is monotonic per session; `final_checkpoint` ends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableCheckpoint {
    pub protocol_version: u32,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub base_canonical_revision: u64,
    pub sequence: u64,
    pub final_checkpoint: bool,
    pub realm_snapshot: Envelope,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "reason", rename_all = "snake_case")]
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
}

pub fn to_json<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!("a message is limited to {MAX_MESSAGE_BYTES} bytes")));
    }
    Ok(bytes)
}

/// Parse one message. The size is checked before the bytes are looked at, unknown fields are refused, the protocol version
/// is checked before anything else is trusted.
pub fn from_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(PortableError::LimitExceeded(format!("a message is limited to {MAX_MESSAGE_BYTES} bytes")));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value.get("protocol_version").and_then(|v| v.as_u64()).ok_or_else(|| PortableError::Invalid("the message has no protocol version".into()))?;
    if version != PROTOCOL_VERSION as u64 {
        return Err(PortableError::UnsupportedFormat { found: version.min(u32::MAX as u64) as u32, supported: PROTOCOL_VERSION });
    }
    Ok(serde_json::from_value(value)?)
}

impl PortableSessionStarted {
    pub fn new(session_id: SessionId, character_id: CharacterId, server_id: &str, base_canonical_revision: u64, baseline_generation: u32, b0: &PortableCharacter) -> Result<Self> {
        let b0 = Envelope::seal(b0)?;
        Ok(Self { protocol_version: PROTOCOL_VERSION, session_id, character_id, server_id: server_id.to_string(), base_canonical_revision, baseline_generation, content_hash: b0.content_hash.clone(), b0 })
    }
}

impl PortableCheckpoint {
    pub fn new(session_id: SessionId, character_id: CharacterId, server_id: &str, base_canonical_revision: u64, sequence: u64, final_checkpoint: bool, b1: &PortableCharacter) -> Result<Self> {
        let realm_snapshot = Envelope::seal(b1)?;
        Ok(Self { protocol_version: PROTOCOL_VERSION, session_id, character_id, server_id: server_id.to_string(), base_canonical_revision, sequence, final_checkpoint, content_hash: realm_snapshot.content_hash.clone(), realm_snapshot })
    }
}
