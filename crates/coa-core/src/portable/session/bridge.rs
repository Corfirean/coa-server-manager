//! What the Host needs from a realm to run a session: the core's session marker, the checkpoint/release operations and a
//! read-only export. `LiveBridge` (see `live.rs`) implements it with the Manager's `Db` and RA; tests implement it with a
//! simulated realm.

use std::collections::HashMap;

use super::super::collection::IdSet;
use super::super::error::Result;
use super::super::ids::{CharacterId, PortableItemId, PortablePetId, SessionId};
use super::super::realm::collections::Applied;
use super::super::realm::Exported;

/// The core's `coa_portable_session.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// Armed by the importer/updater: the character's first load will take the baseline.
    WaitingBaseline,
    /// The baseline transaction was written; the player is held until the Host releases the session.
    BaselineReady,
    /// Released: the session runs.
    Active,
    /// The logout save was written.
    Ended,
}

impl RowState {
    pub fn from_code(code: u64) -> Option<RowState> {
        Some(match code {
            0 => RowState::WaitingBaseline,
            1 => RowState::BaselineReady,
            2 => RowState::Active,
            3 => RowState::Ended,
            _ => return None,
        })
    }
}

/// One row of the core's session table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub guid: u32,
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub imported_revision: u64,
    pub generation: u32,
    pub state: RowState,
    pub checkpoint_seq: u64,
    pub save_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointReply {
    /// The save was queued. This is **not** persistence: the marker row has to be seen.
    Queued,
    /// The character is being teleported or otherwise cannot be saved right now: try again.
    Busy,
    NotOnline,
    /// The core refused (wrong session, stale sequence, not a portable character).
    Refused(String),
}

/// A read of the realm: the character as committed, and the session row **of the same consistent snapshot**.
pub struct RealmRead {
    pub exported: Exported,
    pub session: Option<SessionRow>,
    pub online: bool,
}

pub trait RealmBridge {
    /// Cheap poll of one character's session row.
    fn session_row(&mut self, local_guid: u32) -> Result<Option<SessionRow>>;
    /// Read-only export of the committed character. Allowed while the character is online **if** it has a session row.
    fn read(
        &mut self,
        local_guid: u32,
        prior_items: &HashMap<u32, (PortableItemId, String)>,
        prior_pets: &HashMap<u32, (PortablePetId, String)>,
    ) -> Result<RealmRead>;
    /// `portable checkpoint <local_guid> <session_id> <sequence>`
    fn request_checkpoint(
        &mut self,
        local_guid: u32,
        session: SessionId,
        sequence: u64,
    ) -> Result<CheckpointReply>;
    /// `portable release <session_id>`
    fn release(&mut self, session: SessionId) -> Result<()>;
    /// Put the character's marker back to "waiting for baseline" with a new session (the character is offline).
    fn arm(
        &mut self,
        local_guid: u32,
        session: SessionId,
        character: CharacterId,
        revision: u64,
        generation: u32,
    ) -> Result<()>;

    // ---- account collections (Phase 6) --------------------------------------------------------------------------------------

    /// The game account that owns the character.
    fn account_of(&mut self, local_guid: u32) -> Result<Option<u32>>;
    /// A cheap fingerprint of the account's rows of one collection kind: equal fingerprints mean nothing changed.
    fn collection_fingerprint(&mut self, account: u32, kind: &str) -> Result<String>;
    /// The account's ids of one kind.
    fn read_collection(&mut self, account: u32, kind: &str) -> Result<IdSet>;
    /// Union the canonical ids into the account: only ids the realm knows and lacks are written.
    fn apply_collection(&mut self, account: u32, kind: &str, canonical: &IdSet) -> Result<Applied>;
}
