//! The local canonical store (SQLite, WAL). This is the **source of truth** for portable characters: realms only
//! hold working copies. Every write is one transaction; every mutation of a character is checked against the revision
//! the caller saw (`StaleRevision`), so a stale writer can never overwrite newer state.
//!
//! Nothing here touches a realm database.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::collection::{valid_kind, IdSet};
use super::error::{PortableError, Result};
use super::ids::{CharacterId, PortableItemId, ProfileId};
use super::model::{PortableCharacter, Ruleset};
use super::snapshot::{self, EncodedSnapshot};
use super::versions::{PORTABLE_COLLECTION_FORMAT_VERSION, SNAPSHOT_FORMAT_VERSION};

pub const DATABASE_FILE: &str = "portable.db";
/// `PRAGMA application_id`: "COAP". Refuses to adopt an unrelated SQLite file.
const APPLICATION_ID: i64 = 0x434F_4150;
const MIGRATIONS: &[&str] = &[include_str!("migrations/001_init.sql")];

pub const DEFAULT_HISTORY_KEEP: u32 = 20;
pub const MAX_HISTORY_KEEP: u32 = 1_000;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterRecord {
    pub character_id: CharacterId,
    pub profile_id: ProfileId,
    pub ruleset: Ruleset,
    pub name: String,
    pub race: String,
    pub class: String,
    pub gender: u8,
    pub level: u8,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionInfo {
    pub revision: u64,
    pub created_at: String,
    pub source_server_id: String,
    pub content_hash: [u8; 32],
    pub uncompressed_size: u64,
    pub stored_size: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingState {
    /// Bound, nothing imported yet.
    Pending,
    /// The character is (or may be) in play on that realm.
    Active,
    /// The realm's copy equals `last_revision`.
    Synced,
    /// The realm's copy is older than canonical.
    Stale,
}

impl MappingState {
    pub fn as_str(self) -> &'static str {
        match self {
            MappingState::Pending => "pending",
            MappingState::Active => "active",
            MappingState::Synced => "synced",
            MappingState::Stale => "stale",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => MappingState::Pending,
            "active" => MappingState::Active,
            "synced" => MappingState::Synced,
            "stale" => MappingState::Stale,
            other => return Err(PortableError::Invalid(format!("unknown mapping state {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingRecord {
    pub character_id: CharacterId,
    pub server_id: String,
    pub local_guid: u32,
    pub last_revision: u64,
    pub state: MappingState,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionInfo {
    pub kind: String,
    pub revision: u64,
    pub hash: [u8; 32],
    pub count: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionMerge {
    /// `false` when the incoming set added nothing: no revision was created.
    pub changed: bool,
    pub added: usize,
    /// `None` only when nothing is stored and nothing was added.
    pub info: Option<CollectionInfo>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn valid_server_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

fn check_server_id(id: &str) -> Result<()> {
    if valid_server_id(id) {
        Ok(())
    } else {
        Err(PortableError::Invalid(format!("invalid server id {id:?}")))
    }
}

fn hash_from_blob(blob: Vec<u8>) -> Result<[u8; 32]> {
    blob.try_into().map_err(|_| PortableError::CorruptSnapshot("a stored hash is not 32 bytes".into()))
}

impl Store {
    /// Open (creating it if necessary) `<dir>/portable.db`.
    pub fn open(dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(dir)?;
        Store::open_file(&dir.join(DATABASE_FILE))
    }

    pub fn open_file(file: &Path) -> Result<Store> {
        let conn = Connection::open(file)?;
        // WAL keeps readers out of writers' way and survives a crash mid-write; FULL sync keeps the canonical state
        // durable even on power loss.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let application: i64 = self.conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let supported = MIGRATIONS.len() as i64;
        if application != APPLICATION_ID {
            let objects: i64 = self.conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
            if application != 0 || version != 0 || objects != 0 {
                return Err(PortableError::NotPortableDatabase);
            }
        }
        if version > supported {
            return Err(PortableError::NewerDatabase { found: version, supported });
        }
        for next in version..supported {
            let tx = self.conn.transaction()?;
            tx.execute_batch(MIGRATIONS[next as usize])?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", next + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn write_tx(&mut self) -> Result<Transaction<'_>> {
        Ok(self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
    }

    // ---- settings -------------------------------------------------------------------------------------------

    /// How many revisions of one character are kept (the newest ones).
    pub fn history_keep(&self) -> Result<u32> {
        let value: Option<String> = self.conn.query_row("SELECT value FROM setting WHERE key = 'history_keep'", [], |r| r.get(0)).optional()?;
        Ok(value.and_then(|v| v.parse().ok()).filter(|n| (1..=MAX_HISTORY_KEEP).contains(n)).unwrap_or(DEFAULT_HISTORY_KEEP))
    }

    pub fn set_history_keep(&mut self, keep: u32) -> Result<()> {
        if !(1..=MAX_HISTORY_KEEP).contains(&keep) {
            return Err(PortableError::Invalid(format!("history_keep must be between 1 and {MAX_HISTORY_KEEP}")));
        }
        self.conn.execute("INSERT INTO setting(key, value) VALUES ('history_keep', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [keep.to_string()])?;
        Ok(())
    }

    // ---- profiles -------------------------------------------------------------------------------------------

    pub fn create_profile(&mut self) -> Result<ProfileId> {
        let id = ProfileId::new();
        self.conn.execute(
            "INSERT INTO profile(profile_id, created_at, format_version) VALUES (?1, ?2, ?3)",
            params![id.to_string(), now(), super::versions::PORTABLE_CHARACTER_FORMAT_VERSION],
        )?;
        Ok(id)
    }

    /// The profile of this Manager: the oldest one, created on first use.
    pub fn default_profile(&mut self) -> Result<ProfileId> {
        let tx = self.write_tx()?;
        let existing: Option<String> = tx.query_row("SELECT profile_id FROM profile ORDER BY profile_id LIMIT 1", [], |r| r.get(0)).optional()?;
        let id = match existing {
            Some(text) => text.parse()?,
            None => {
                let id = ProfileId::new();
                tx.execute(
                    "INSERT INTO profile(profile_id, created_at, format_version) VALUES (?1, ?2, ?3)",
                    params![id.to_string(), now(), super::versions::PORTABLE_CHARACTER_FORMAT_VERSION],
                )?;
                id
            }
        };
        tx.commit()?;
        Ok(id)
    }

    // ---- characters -----------------------------------------------------------------------------------------

    /// Create a character with a **new** UUIDv7 (the one in `model.character_id` is replaced). Revision 1.
    pub fn create_character(&mut self, profile: ProfileId, mut model: PortableCharacter, source_server_id: &str) -> Result<CharacterId> {
        model.character_id = CharacterId::new();
        self.create_character_with_id(profile, model, source_server_id)
    }

    /// Create a character that already has its identity (restoring or receiving an existing portable character).
    /// An id that exists is refused and nothing is written.
    pub fn create_character_with_id(&mut self, profile: ProfileId, model: PortableCharacter, source_server_id: &str) -> Result<CharacterId> {
        check_server_id(source_server_id)?;
        let encoded = snapshot::encode(&model)?;
        let id = model.character_id;
        let tx = self.write_tx()?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM character WHERE character_id = ?1)", [id.to_string()], |r| r.get(0))?;
        if exists {
            return Err(PortableError::DuplicateCharacter(id));
        }
        let profile_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM profile WHERE profile_id = ?1)", [profile.to_string()], |r| r.get(0))?;
        if !profile_exists {
            return Err(PortableError::Invalid(format!("profile {profile} does not exist")));
        }
        let at = now();
        tx.execute(
            "INSERT INTO character(character_id, profile_id, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, 0)",
            params![
                id.to_string(),
                profile.to_string(),
                model.ruleset.as_str(),
                model.identity.name,
                model.identity.race.to_string(),
                model.identity.class.to_string(),
                model.identity.gender,
                model.progression.level,
                at
            ],
        )?;
        insert_snapshot(&tx, id, 1, &encoded, source_server_id, Some("created"), &at)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn character(&self, id: CharacterId) -> Result<CharacterRecord> {
        read_character(&self.conn, id)
    }

    pub fn list_characters(&self, profile: ProfileId) -> Result<Vec<CharacterRecord>> {
        let mut stmt = self.conn.prepare("SELECT character_id FROM character WHERE profile_id = ?1 ORDER BY character_id")?;
        let ids: Vec<String> = stmt.query_map([profile.to_string()], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        ids.iter().map(|text| read_character(&self.conn, text.parse()?)).collect()
    }

    pub fn set_archived(&mut self, id: CharacterId, archived: bool) -> Result<()> {
        let changed = self.conn.execute("UPDATE character SET archived = ?2 WHERE character_id = ?1", params![id.to_string(), archived])?;
        if changed == 0 {
            return Err(PortableError::UnknownCharacter(id));
        }
        Ok(())
    }

    /// Store the next canonical revision. `expected_revision` is the revision the caller based its change on; if the
    /// canonical revision has moved on, nothing is written and `StaleRevision` is returned. Returns the new revision.
    pub fn commit_snapshot(&mut self, id: CharacterId, expected_revision: u64, model: PortableCharacter, source_server_id: &str, note: Option<&str>) -> Result<u64> {
        check_server_id(source_server_id)?;
        if model.character_id != id {
            return Err(PortableError::WrongCharacter { expected: id, found: model.character_id });
        }
        let encoded = snapshot::encode(&model)?;
        let keep = self.history_keep()?;
        let tx = self.write_tx()?;
        let current = read_character(&tx, id)?;
        if current.ruleset != model.ruleset {
            return Err(PortableError::RulesetChange { from: current.ruleset.to_string(), to: model.ruleset.to_string() });
        }
        if current.revision != expected_revision {
            return Err(PortableError::StaleRevision { expected: expected_revision, current: current.revision });
        }
        let revision = current.revision + 1;
        let at = now();
        insert_snapshot(&tx, id, revision, &encoded, source_server_id, note, &at)?;
        update_character(&tx, id, &model, revision, &at)?;
        prune_tx(&tx, id, revision, keep)?;
        tx.commit()?;
        Ok(revision)
    }

    /// Make an older revision current **as a new revision** (revision numbers never go down, so freshness stays a
    /// plain integer comparison). Returns the new revision.
    pub fn rollback_to(&mut self, id: CharacterId, expected_revision: u64, target_revision: u64, source_server_id: &str) -> Result<u64> {
        check_server_id(source_server_id)?;
        let keep = self.history_keep()?;
        let tx = self.write_tx()?;
        let current = read_character(&tx, id)?;
        if current.revision != expected_revision {
            return Err(PortableError::StaleRevision { expected: expected_revision, current: current.revision });
        }
        let (model, encoded) = read_snapshot_row(&tx, id, target_revision)?;
        let revision = current.revision + 1;
        let at = now();
        insert_snapshot(&tx, id, revision, &encoded, source_server_id, Some(&format!("rollback to revision {target_revision}")), &at)?;
        update_character(&tx, id, &model, revision, &at)?;
        prune_tx(&tx, id, revision, keep)?;
        tx.commit()?;
        Ok(revision)
    }

    pub fn list_revisions(&self, id: CharacterId) -> Result<Vec<RevisionInfo>> {
        read_character(&self.conn, id)?;
        let mut stmt = self.conn.prepare(
            "SELECT revision, created_at, source_server_id, content_hash, uncompressed_size, length(payload), note
             FROM snapshot WHERE character_id = ?1 ORDER BY revision",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, Option<String>>(6)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (revision, created_at, source_server_id, hash, uncompressed, stored, note) = row?;
            out.push(RevisionInfo {
                revision: revision as u64,
                created_at,
                source_server_id,
                content_hash: hash_from_blob(hash)?,
                uncompressed_size: uncompressed as u64,
                stored_size: stored as u64,
                note,
            });
        }
        Ok(out)
    }

    /// One stored revision, verified (hash, limits, structure).
    pub fn load_snapshot(&self, id: CharacterId, revision: u64) -> Result<PortableCharacter> {
        Ok(read_snapshot_row(&self.conn, id, revision)?.0)
    }

    pub fn load_current(&self, id: CharacterId) -> Result<PortableCharacter> {
        let record = read_character(&self.conn, id)?;
        self.load_snapshot(id, record.revision)
    }

    /// Remove revisions beyond the history limit. The current revision is never removed. Returns how many went.
    pub fn prune(&mut self, id: CharacterId) -> Result<usize> {
        let keep = self.history_keep()?;
        let tx = self.write_tx()?;
        let current = read_character(&tx, id)?;
        let removed = prune_tx(&tx, id, current.revision, keep)?;
        tx.commit()?;
        Ok(removed)
    }

    // ---- realm mappings -------------------------------------------------------------------------------------

    /// Record (or update) the local guid of a portable character on a realm. A local guid can belong to one portable
    /// character only. Changing the local guid of an existing binding drops that binding's item mappings (they named
    /// items of the previous local character).
    pub fn bind_server(&mut self, id: CharacterId, server_id: &str, local_guid: u32, last_revision: u64, state: MappingState) -> Result<()> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let record = read_character(&tx, id)?;
        if last_revision > record.revision {
            return Err(PortableError::Invalid(format!("revision {last_revision} is newer than the canonical revision {}", record.revision)));
        }
        let holder: Option<String> = tx
            .query_row("SELECT character_id FROM character_server_mapping WHERE server_id = ?1 AND local_guid = ?2", params![server_id, local_guid], |r| r.get(0))
            .optional()?;
        if holder.is_some_and(|h| h != id.to_string()) {
            return Err(PortableError::LocalGuidTaken { server_id: server_id.to_string(), local_guid });
        }
        let previous: Option<i64> = tx
            .query_row("SELECT local_guid FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| r.get(0))
            .optional()?;
        if previous.is_some_and(|p| p != local_guid as i64) {
            tx.execute("DELETE FROM item_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id])?;
        }
        tx.execute(
            "INSERT INTO character_server_mapping(character_id, server_id, local_guid, last_revision, state, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(character_id, server_id) DO UPDATE SET local_guid = excluded.local_guid, last_revision = excluded.last_revision,
                                                              state = excluded.state, updated_at = excluded.updated_at",
            params![id.to_string(), server_id, local_guid, last_revision as i64, state.as_str(), now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The portable character a realm's local character belongs to. Checkpoints arrive keyed by the local guid.
    pub fn find_by_local(&self, server_id: &str, local_guid: u32) -> Result<Option<CharacterId>> {
        let found: Option<String> = self
            .conn
            .query_row("SELECT character_id FROM character_server_mapping WHERE server_id = ?1 AND local_guid = ?2", params![server_id, local_guid], |r| r.get(0))
            .optional()?;
        found.map(|text| text.parse()).transpose()
    }

    pub fn server_mappings(&self, id: CharacterId) -> Result<Vec<MappingRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT server_id, local_guid, last_revision, state, updated_at FROM character_server_mapping WHERE character_id = ?1 ORDER BY server_id",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (server_id, guid, revision, state, updated_at) = row?;
            out.push(MappingRecord { character_id: id, server_id, local_guid: guid as u32, last_revision: revision as u64, state: MappingState::parse(&state)?, updated_at });
        }
        Ok(out)
    }

    /// Replace the item mappings of one (character, realm) pair. Every portable item id and every local item guid may
    /// appear once. The pair must already be bound with [`Store::bind_server`].
    pub fn set_item_mappings(&mut self, id: CharacterId, server_id: &str, mappings: &[(PortableItemId, u32)]) -> Result<()> {
        check_server_id(server_id)?;
        let mut portable = std::collections::HashSet::new();
        let mut local = std::collections::HashSet::new();
        for (item, guid) in mappings {
            if !portable.insert(*item) {
                return Err(PortableError::Invalid(format!("portable item {item} is mapped twice")));
            }
            if !local.insert(*guid) {
                return Err(PortableError::ItemGuidConflict { server_id: server_id.to_string(), local_item_guid: *guid });
            }
        }
        let tx = self.write_tx()?;
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2)",
            params![id.to_string(), server_id],
            |r| r.get(0),
        )?;
        if !bound {
            return Err(PortableError::Invalid(format!("character {id} is not bound to server {server_id}")));
        }
        tx.execute("DELETE FROM item_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id])?;
        {
            let mut insert = tx.prepare("INSERT INTO item_mapping(character_id, server_id, portable_item_id, local_item_guid) VALUES (?1, ?2, ?3, ?4)")?;
            for (item, guid) in mappings {
                insert.execute(params![id.to_string(), server_id, item.to_string(), guid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn item_mappings(&self, id: CharacterId, server_id: &str) -> Result<Vec<(PortableItemId, u32)>> {
        let mut stmt = self.conn.prepare("SELECT portable_item_id, local_item_guid FROM item_mapping WHERE character_id = ?1 AND server_id = ?2 ORDER BY local_item_guid")?;
        let rows = stmt.query_map(params![id.to_string(), server_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (item, guid) = row?;
            out.push((item.parse()?, guid as u32));
        }
        Ok(out)
    }

    // ---- collections ----------------------------------------------------------------------------------------

    /// `stored = stored U incoming`. Nothing is ever removed. When the union equals what is stored, no revision is
    /// created and the hash stays the same.
    pub fn merge_collection(&mut self, profile: ProfileId, kind: &str, incoming: &IdSet) -> Result<CollectionMerge> {
        if !valid_kind(kind) {
            return Err(PortableError::Invalid(format!("invalid collection kind {kind:?}")));
        }
        let tx = self.write_tx()?;
        let profile_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM profile WHERE profile_id = ?1)", [profile.to_string()], |r| r.get(0))?;
        if !profile_exists {
            return Err(PortableError::Invalid(format!("profile {profile} does not exist")));
        }
        let stored = read_collection(&tx, profile, kind)?;
        let (current_set, revision) = match &stored {
            Some((info, set)) => (set.clone(), info.revision),
            None => (IdSet::new(), 0),
        };
        let added = current_set.count_new(incoming);
        if added == 0 {
            tx.commit()?;
            return Ok(CollectionMerge { changed: false, added: 0, info: stored.map(|(info, _)| info) });
        }
        let merged = current_set.union(incoming);
        let hash = merged.hash(kind);
        let payload = merged.encode();
        let at = now();
        tx.execute(
            "INSERT INTO collection(profile_id, kind, collection_revision, collection_hash, format_version, item_count, updated_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(profile_id, kind) DO UPDATE SET collection_revision = excluded.collection_revision, collection_hash = excluded.collection_hash,
                format_version = excluded.format_version, item_count = excluded.item_count, updated_at = excluded.updated_at, payload = excluded.payload",
            params![profile.to_string(), kind, (revision + 1) as i64, hash.as_slice(), PORTABLE_COLLECTION_FORMAT_VERSION, merged.len() as i64, at, payload],
        )?;
        tx.commit()?;
        Ok(CollectionMerge {
            changed: true,
            added,
            info: Some(CollectionInfo { kind: kind.to_string(), revision: revision + 1, hash, count: merged.len(), updated_at: at }),
        })
    }

    /// Revision, hash and size without loading the set: the "did it change?" check.
    pub fn collection_info(&self, profile: ProfileId, kind: &str) -> Result<Option<CollectionInfo>> {
        let row = self
            .conn
            .query_row(
                "SELECT collection_revision, collection_hash, item_count, updated_at FROM collection WHERE profile_id = ?1 AND kind = ?2",
                params![profile.to_string(), kind],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)),
            )
            .optional()?;
        row.map(|(revision, hash, count, updated_at)| Ok(CollectionInfo { kind: kind.to_string(), revision: revision as u64, hash: hash_from_blob(hash)?, count: count as usize, updated_at }))
            .transpose()
    }

    pub fn collection(&self, profile: ProfileId, kind: &str) -> Result<Option<(CollectionInfo, IdSet)>> {
        read_collection(&self.conn, profile, kind)
    }

    pub fn path_of(dir: &Path) -> PathBuf {
        dir.join(DATABASE_FILE)
    }
}

fn read_collection(conn: &Connection, profile: ProfileId, kind: &str) -> Result<Option<(CollectionInfo, IdSet)>> {
    let row = conn
        .query_row(
            "SELECT collection_revision, collection_hash, item_count, updated_at, payload FROM collection WHERE profile_id = ?1 AND kind = ?2",
            params![profile.to_string(), kind],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Vec<u8>>(4)?)),
        )
        .optional()?;
    let Some((revision, hash, count, updated_at, payload)) = row else { return Ok(None) };
    let set = IdSet::decode(&payload)?;
    let hash = hash_from_blob(hash)?;
    if set.hash(kind) != hash || set.len() != count as usize {
        return Err(PortableError::CorruptSnapshot(format!("collection {kind} does not match its recorded hash")));
    }
    Ok(Some((CollectionInfo { kind: kind.to_string(), revision: revision as u64, hash, count: set.len(), updated_at }, set)))
}

fn read_character(conn: &Connection, id: CharacterId) -> Result<CharacterRecord> {
    let row = conn
        .query_row(
            "SELECT profile_id, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived FROM character WHERE character_id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, u8>(5)?,
                    r.get::<_, u8>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, bool>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((profile, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived)) = row else {
        return Err(PortableError::UnknownCharacter(id));
    };
    Ok(CharacterRecord {
        character_id: id,
        profile_id: profile.parse()?,
        ruleset: Ruleset::parse(&ruleset)?,
        name,
        race,
        class,
        gender,
        level,
        revision: revision as u64,
        created_at,
        updated_at,
        archived,
    })
}

fn insert_snapshot(tx: &Transaction<'_>, id: CharacterId, revision: u64, encoded: &EncodedSnapshot, source_server_id: &str, note: Option<&str>, at: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO snapshot(character_id, revision, snapshot_format_version, created_at, source_server_id, content_hash, uncompressed_size, payload, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id.to_string(), revision as i64, SNAPSHOT_FORMAT_VERSION, at, source_server_id, encoded.content_hash.as_slice(), encoded.uncompressed_size as i64, encoded.payload, note],
    )?;
    Ok(())
}

fn update_character(tx: &Transaction<'_>, id: CharacterId, model: &PortableCharacter, revision: u64, at: &str) -> Result<()> {
    tx.execute(
        "UPDATE character SET name = ?2, race = ?3, class = ?4, gender = ?5, level = ?6, revision = ?7, updated_at = ?8 WHERE character_id = ?1",
        params![id.to_string(), model.identity.name, model.identity.race.to_string(), model.identity.class.to_string(), model.identity.gender, model.progression.level, revision as i64, at],
    )?;
    Ok(())
}

/// Delete everything older than the newest `keep` revisions; `current` is always among the newest.
fn prune_tx(tx: &Transaction<'_>, id: CharacterId, current: u64, keep: u32) -> Result<usize> {
    let cutoff = current as i64 - keep as i64;
    if cutoff < 1 {
        return Ok(0);
    }
    Ok(tx.execute("DELETE FROM snapshot WHERE character_id = ?1 AND revision <= ?2 AND revision < ?3", params![id.to_string(), cutoff, current as i64])?)
}

/// Read one revision and verify it against its recorded hash. Also returns the re-encoded form so a rollback can store
/// exactly the same payload.
fn read_snapshot_row(conn: &Connection, id: CharacterId, revision: u64) -> Result<(PortableCharacter, EncodedSnapshot)> {
    let row = conn
        .query_row(
            "SELECT snapshot_format_version, content_hash, uncompressed_size, payload FROM snapshot WHERE character_id = ?1 AND revision = ?2",
            params![id.to_string(), revision as i64],
            |r| Ok((r.get::<_, u32>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?, r.get::<_, Vec<u8>>(3)?)),
        )
        .optional()?;
    let Some((format, hash, size, payload)) = row else {
        read_character(conn, id)?;
        return Err(PortableError::UnknownRevision { character: id, revision });
    };
    if format > SNAPSHOT_FORMAT_VERSION {
        return Err(PortableError::UnsupportedFormat { found: format, supported: SNAPSHOT_FORMAT_VERSION });
    }
    let hash = hash_from_blob(hash)?;
    let model = snapshot::decode(&payload, Some(&hash))?;
    if model.character_id != id {
        return Err(PortableError::CorruptSnapshot("a stored snapshot names another character".into()));
    }
    Ok((model, EncodedSnapshot { content_hash: hash, uncompressed_size: size as u64, payload }))
}

#[cfg(test)]
mod tests;
