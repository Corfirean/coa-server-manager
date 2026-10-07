use super::ids::{CharacterId, ImportId};
use super::realm::{Blocker, ImportProblem};

pub type Result<T> = std::result::Result<T, PortableError>;

#[derive(Debug, thiserror::Error)]
pub enum PortableError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("character {0} already exists")]
    DuplicateCharacter(CharacterId),
    #[error("character {0} does not exist")]
    UnknownCharacter(CharacterId),
    #[error("revision {revision} of character {character} does not exist (it may have been pruned)")]
    UnknownRevision { character: CharacterId, revision: u64 },
    #[error("stale revision: the caller expected {expected} but the canonical revision is {current}")]
    StaleRevision { expected: u64, current: u64 },
    #[error("a character's ruleset cannot change ({from} -> {to})")]
    RulesetChange { from: String, to: String },
    #[error("the snapshot belongs to character {found}, not {expected}")]
    WrongCharacter { expected: CharacterId, found: CharacterId },

    #[error("local character {local_guid} on server {server_id} is already bound to another portable character")]
    LocalGuidTaken { server_id: String, local_guid: u32 },
    #[error("local item {local_item_guid} on server {server_id} is mapped twice for one character")]
    ItemGuidConflict { server_id: String, local_item_guid: u32 },

    #[error("local character {local_guid} on server {server_id} is already portable ({character})")]
    AlreadyPortable { server_id: String, local_guid: u32, character: CharacterId },

    #[error("this character cannot be made portable: {}", .0.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("; "))]
    NotExportable(Vec<Blocker>),
    #[error("there is no character {0} on this realm")]
    NoSuchRealmCharacter(u32),
    #[error("the realm database does not have the expected structure: {0}")]
    SchemaMismatch(String),
    #[error("could not read the realm: {0}")]
    RealmRead(String),
    #[error("the realm database is a {found} realm but a {expected} character was requested")]
    WrongRealm { expected: String, found: String },

    #[error("character {character} is already on server {server_id}")]
    AlreadyOnRealm { character: CharacterId, server_id: String },
    #[error("import {import_id} of this character into this server has not finished; recover it first")]
    ImportInProgress { import_id: ImportId },
    #[error("import {0} does not exist")]
    UnknownImport(ImportId),
    #[error("import {import_id} is {state}, which does not allow this")]
    ImportState { import_id: ImportId, state: String },
    #[error("this realm cannot take the {operation}; nothing was written: {}", .reasons.join("; "))]
    Incompatible { operation: String, reasons: Vec<String> },
    #[error("this character is level {character_level} and the realm's cap is {cap}: it must be projected, and only a running core can say what a projection holds (start the realm, or supply a projection made by its core); nothing was written")]
    ProjectionNeedsRunningCore { character_level: u32, cap: u32 },
    #[error("the realm's progression profile is not the one this was projected for: {0}")]
    ProgressionChanged(String),
    #[error("the import was refused before anything was written: {}", .0.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("; "))]
    ImportRefused(Vec<ImportProblem>),
    #[error("import {import_id} needs attention: {detail}")]
    ImportNeedsAttention { import_id: ImportId, detail: String },

    #[error("this character has no open session on this realm: capture the baseline after the realm's first load and before it is played")]
    NoBaseline,
    #[error("this character has an open session on this realm: reconcile it first")]
    SessionOpen,
    #[error("the character was changed on the realm and in the canonical store in conflicting ways; nothing was written: {}", .0.join("; "))]
    UpdateConflicts(Vec<String>),

    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("stored snapshot is damaged: {0}")]
    CorruptSnapshot(String),
    #[error("data format version {found} is newer than the supported version {supported}")]
    UnsupportedFormat { found: u32, supported: u32 },
    #[error("the portable database was written by a newer Manager (schema {found}, this Manager understands {supported})")]
    NewerDatabase { found: i64, supported: i64 },
    #[error("this file is not a Manager portable database")]
    NotPortableDatabase,
}

impl From<PortableError> for crate::Error {
    fn from(e: PortableError) -> Self {
        crate::Error::Invalid(e.to_string())
    }
}
