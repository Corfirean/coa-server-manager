//! What the user interface sees of portable play. Everything here is plain data: states, words and numbers a person can read, never
//! a session id, a guid, a manifest or a console command. The ids that are present (`id` of a character, of a realm) are handles the
//! interface hands back; they are not displayed.

use std::collections::BTreeMap;

use serde::Serialize;

/// Where a character is with a realm, in the words the interface uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayStatus {
    /// Nothing prepared yet on this realm, or prepared and idle: pressing Play does the rest.
    Ready,
    /// A message of the session is on its way to the Manager's store.
    Syncing,
    /// The character is being put on the realm.
    Preparing,
    /// Prepared: the game's login is the next step.
    WaitingLogin,
    Playing,
    /// The player left: the last progress is being taken.
    Saving,
    /// The realm cannot be reached.
    Offline,
    /// Playable, with a part of the character left safely in the Manager (the held state).
    CompatWarning,
    /// The realm must be restarted or updated first.
    UpdateRequired,
    /// The realm's copy and the canonical character both changed.
    Conflict,
    /// The realm cannot take this character at all.
    Incompatible,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CopyView {
    pub realm_id: String,
    pub realm_name: String,
    pub status: PlayStatus,
    /// The canonical revision this realm's copy was last brought to.
    pub synced_revision: u64,
    pub behind: bool,
    /// Set when the character is played here at a lower level (the realm's cap).
    pub projected_level: Option<u32>,
    /// The realm plays this character with a part of it left safely in the Manager.
    pub degraded: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CharacterView {
    pub id: String,
    pub name: String,
    pub class_name: String,
    pub race: String,
    pub level: u8,
    pub revision: u64,
    pub updated_at: String,
    /// The realm the character is in play on right now.
    pub active_realm: Option<String>,
    pub status: PlayStatus,
    pub copies: Vec<CopyView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RealmView {
    pub id: String,
    pub name: String,
    pub kind: super::access::Kind,
    pub address: String,
    pub online: bool,
    pub database_ok: bool,
    /// The realm's level cap, once its core has said it.
    pub level_cap: Option<u32>,
    /// `ready`: the realm's core can run portable sessions; `setup`: it can after a setting and a restart; `unknown`: not looked at yet.
    pub portable: String,
    pub game_account: Option<String>,
    pub installed_id: Option<String>,
}

/// One line of the compatibility check, as a code the interface translates and the technical text for diagnostics.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Note {
    pub code: String,
    pub params: BTreeMap<String, String>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Compatible,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Put the character on the realm.
    Prepare,
    /// The character is on the realm and up to date: arm the session.
    Arm,
    /// A session is running or waiting for the game's login.
    Resume,
    /// The realm must be stopped, brought up to date and started again (done by the Manager when it owns the server).
    Restart,
    /// The realm's copy and the canonical character both changed.
    Resolve,
    Blocked,
    Offline,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreflightView {
    pub verdict: Verdict,
    pub step: Step,
    /// `Some((from, to))` when the character is played at a lower level.
    pub projection: Option<(u32, u32)>,
    pub notes: Vec<Note>,
    pub needs_account: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlayView {
    pub status: PlayStatus,
    pub projection: Option<(u32, u32)>,
    pub notes: Vec<Note>,
    pub realm_address: String,
    pub realm_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LocalCharacterView {
    /// A handle the interface hands back (the realm's own numbering; never shown).
    pub token: u32,
    pub name: String,
    pub class_name: String,
    pub level: u32,
    pub account: String,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub revision: u64,
    pub at: String,
    pub source_realm: String,
    pub kind: HistoryKind,
}

/// What a saved version came from, in words a player understands; internal session and checkpoint identifiers never leave the service.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    Created,
    Play,
    Other,
}

impl HistoryKind {
    pub fn of(note: Option<&str>) -> Self {
        match note {
            Some("created") => Self::Created,
            Some(n) if n.starts_with("session ") => Self::Play,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_notes_become_kinds_without_identifiers() {
        assert_eq!(HistoryKind::of(Some("created")), HistoryKind::Created);
        assert_eq!(HistoryKind::of(Some("session 01a11638-9ccf-7111-b5e9-124d1ecf84bb checkpoint 3")), HistoryKind::Play);
        assert_eq!(HistoryKind::of(Some("re-export")), HistoryKind::Other);
        assert_eq!(HistoryKind::of(None), HistoryKind::Other);
        let json = serde_json::to_string(&HistoryEntry { revision: 2, at: "t".into(), source_realm: "r".into(), kind: HistoryKind::of(Some("session 01a11638-9ccf-7111-b5e9-124d1ecf84bb checkpoint 3")) }).unwrap();
        assert!(!json.contains("01a1") && !json.contains("checkpoint"), "{json}");
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorEntry {
    pub at: String,
    pub realm: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OperationView {
    pub character_id: String,
    pub realm_id: String,
    pub status: PlayStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct RuntimeView {
    pub running: bool,
    /// Seconds since the last look at the realms' sessions.
    pub last_tick_secs: Option<u64>,
    pub sessions_open: u32,
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct PortableState {
    pub characters: Vec<CharacterView>,
    pub realms: Vec<RealmView>,
    pub operation: Option<OperationView>,
    pub runtime: RuntimeView,
}

/// The two ways out of a realm copy the Manager does not own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolve {
    /// Make the realm's copy what the canonical character is (what only the realm's copy had is dropped).
    UseCanonical,
    /// Stop managing the realm's copy: it stays on the realm as an ordinary character.
    Detach,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiagnosticsReport {
    pub generated_at: String,
    pub manager_version: String,
    pub realms: Vec<RealmDiagnostics>,
    pub characters: Vec<CharacterDiagnostics>,
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RealmDiagnostics {
    pub server_id: String,
    pub name: String,
    pub kind: super::access::Kind,
    pub online: bool,
    pub capability_profile_hash: Option<String>,
    pub core_commit: Option<String>,
    pub level_cap: Option<u32>,
    pub progression_signature: Option<String>,
    pub session_protocol: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CharacterDiagnostics {
    pub character_id: String,
    pub canonical_revision: u64,
    pub copies: Vec<CopyDiagnostics>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CopyDiagnostics {
    pub server_id: String,
    pub synced_revision: u64,
    pub session_id: Option<String>,
    pub session_state: Option<String>,
    pub last_checkpoint: Option<u64>,
    pub progression_pin: Option<crate::portable::projection::ProgressionPin>,
    pub projected: bool,
    pub compatibility: Option<Verdict>,
}
