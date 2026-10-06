use super::ids::CharacterId;

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
