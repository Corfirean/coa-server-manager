//! The import journal: how a realm commit and a local commit stay consistent although they cannot be one transaction.
//!
//! ```text
//!   begin_import      local commit 1   journal row "prepared" (nonce, the plan: portable ids, entries, identities)
//!   realm transaction                  inserts the character AND the marker `coa.portable.import = <nonce> <revision>`
//!   finish_import     local commit 2   mappings + binding + journal "committed", in ONE local transaction
//! ```
//!
//! A crash can fall in two places. Before the realm commit: no marker exists, nothing happened in the realm; the entry is
//! aborted. After the realm commit but before `finish_import`: the marker exists in the realm, so the import *did*
//! happen and `finish_import` is simply run again from what the marker and the realm's rows say. The marker is part of
//! the realm transaction, so "marker present" and "import committed" are the same fact.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::*;
use crate::portable::ids::{ImportId, PortablePetId};

/// One item of the import plan, as far as the mapping needs to know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedItem {
    pub id: PortableItemId,
    pub entry: ContentId,
    pub identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportState {
    /// Written before the realm transaction; the realm may or may not have committed.
    Prepared,
    Committed,
    /// The realm did not commit: nothing of this import exists there.
    Aborted,
    /// The realm and the journal disagree in a way that must not be guessed about.
    NeedsAttention,
}

impl ImportState {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportState::Prepared => "prepared",
            ImportState::Committed => "committed",
            ImportState::Aborted => "aborted",
            ImportState::NeedsAttention => "needs_attention",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "prepared" => ImportState::Prepared,
            "committed" => ImportState::Committed,
            "aborted" => ImportState::Aborted,
            "needs_attention" => ImportState::NeedsAttention,
            other => return Err(PortableError::Invalid(format!("unknown import state {other:?}"))),
        })
    }
}

/// What a begun import hands to the realm step.
#[derive(Debug, Clone)]
pub struct ImportTicket {
    pub import_id: ImportId,
    pub nonce: [u32; 4],
    pub revision: u64,
    pub marker: String,
}

/// What the realm allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportAllocation {
    pub local_guid: u32,
    pub item_base: u32,
    pub pet_base: u32,
}

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub import_id: ImportId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub revision: u64,
    pub marker: String,
    pub state: ImportState,
    pub items: Vec<PlannedItem>,
    pub pet_ids: Vec<PortablePetId>,
    pub allocation: Option<ImportAllocation>,
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn nonce() -> [u32; 4] {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let word = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    [word(0), word(4), word(8), word(12)]
}

/// The marker text for a nonce and revision. Kept in step with `realm::plan::marker_data`.
pub fn marker_text(nonce: [u32; 4], revision: u64) -> String {
    format!("{} {} {} {} {} ", nonce[0], nonce[1], nonce[2], nonce[3], revision)
}

impl Store {
    /// Record the intention to import `character` at its current canonical `revision` into `server_id`.
    /// Refused when the revision is stale, when the character is already on that server, or when another import of the
    /// same pair is unfinished.
    pub fn begin_import(&mut self, character: CharacterId, server_id: &str, revision: u64, items: &[PlannedItem], pet_ids: &[PortablePetId]) -> Result<ImportTicket> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let record = read_character(&tx, character)?;
        if record.revision != revision {
            return Err(PortableError::StaleRevision { expected: revision, current: record.revision });
        }
        let open: Option<String> = tx
            .query_row("SELECT import_id FROM import_journal WHERE character_id = ?1 AND server_id = ?2 AND state = 'prepared'", params![character.to_string(), server_id], |r| r.get(0))
            .optional()?;
        if let Some(open) = open {
            return Err(PortableError::ImportInProgress { import_id: open.parse()? });
        }
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2)",
            params![character.to_string(), server_id],
            |r| r.get(0),
        )?;
        if bound {
            return Err(PortableError::AlreadyOnRealm { character, server_id: server_id.to_string() });
        }
        let import_id = ImportId::new();
        let nonce = nonce();
        let marker = marker_text(nonce, revision);
        let at = now();
        tx.execute(
            "INSERT INTO import_journal(import_id, character_id, server_id, revision, marker, state, items, pet_ids, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'prepared', ?6, ?7, ?8, ?8)",
            params![import_id.to_string(), character.to_string(), server_id, revision as i64, marker, serde_json::to_string(items)?, serde_json::to_string(pet_ids)?, at],
        )?;
        tx.commit()?;
        Ok(ImportTicket { import_id, nonce, revision, marker })
    }

    /// The realm committed: record the binding and the item mappings and close the journal entry, in **one** local
    /// transaction. Safe to call again for an entry that is already committed (it does nothing).
    pub fn finish_import(&mut self, import_id: ImportId, allocation: ImportAllocation) -> Result<()> {
        let tx = self.write_tx()?;
        let entry = read_journal(&tx, import_id)?;
        match entry.state {
            ImportState::Committed => return Ok(()),
            ImportState::Prepared => {}
            other => return Err(PortableError::ImportState { import_id, state: other.as_str().into() }),
        }
        let mut observations = Vec::with_capacity(entry.items.len());
        for (i, planned) in entry.items.iter().enumerate() {
            let guid = u32::try_from(allocation.item_base as u64 + i as u64).map_err(|_| PortableError::Invalid("item guid out of range".into()))?;
            observations.push(ItemObservation { portable_item_id: planned.id, local_item_guid: guid, entry: planned.entry.clone(), identity: planned.identity.clone() });
        }
        bind_in_tx(&tx, entry.character_id, &entry.server_id, allocation.local_guid, entry.revision, MappingState::Synced)?;
        reconcile_in_tx(&tx, entry.character_id, &entry.server_id, entry.revision, &observations)?;
        tx.execute(
            "UPDATE import_journal SET state = 'committed', local_guid = ?2, item_base = ?3, pet_base = ?4, updated_at = ?5 WHERE import_id = ?1",
            params![import_id.to_string(), allocation.local_guid, allocation.item_base, allocation.pet_base, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The realm did not commit.
    pub fn abort_import(&mut self, import_id: ImportId, detail: &str) -> Result<()> {
        self.close_import(import_id, ImportState::Aborted, detail)
    }

    /// The realm and the journal disagree; a person has to look. The entry stays visible and blocks nothing else.
    pub fn flag_import(&mut self, import_id: ImportId, detail: &str) -> Result<()> {
        self.close_import(import_id, ImportState::NeedsAttention, detail)
    }

    fn close_import(&mut self, import_id: ImportId, state: ImportState, detail: &str) -> Result<()> {
        let tx = self.write_tx()?;
        let entry = read_journal(&tx, import_id)?;
        if entry.state != ImportState::Prepared {
            return Err(PortableError::ImportState { import_id, state: entry.state.as_str().into() });
        }
        tx.execute("UPDATE import_journal SET state = ?2, detail = ?3, updated_at = ?4 WHERE import_id = ?1", params![import_id.to_string(), state.as_str(), detail, now()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn import_entry(&self, import_id: ImportId) -> Result<JournalEntry> {
        read_journal(&self.conn, import_id)
    }

    /// Unfinished imports into one server, oldest first.
    pub fn open_imports(&self, server_id: &str) -> Result<Vec<JournalEntry>> {
        let mut stmt = self.conn.prepare("SELECT import_id FROM import_journal WHERE server_id = ?1 AND state = 'prepared' ORDER BY import_id")?;
        let ids: Vec<String> = stmt.query_map([server_id], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        ids.iter().map(|id| read_journal(&self.conn, id.parse()?)).collect()
    }
}

fn read_journal(conn: &Connection, import_id: ImportId) -> Result<JournalEntry> {
    type Row = (String, String, i64, String, String, String, String, Option<i64>, Option<i64>, Option<i64>, Option<String>, String, String);
    let row: Option<Row> = conn
        .query_row(
            "SELECT character_id, server_id, revision, marker, state, items, pet_ids, local_guid, item_base, pet_base, detail, created_at, updated_at FROM import_journal WHERE import_id = ?1",
            [import_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?)),
        )
        .optional()?;
    let Some((character, server_id, revision, marker, state, items, pets, guid, item_base, pet_base, detail, created_at, updated_at)) = row else {
        return Err(PortableError::UnknownImport(import_id));
    };
    let allocation = match (guid, item_base, pet_base) {
        (Some(g), Some(i), Some(p)) => Some(ImportAllocation { local_guid: g as u32, item_base: i as u32, pet_base: p as u32 }),
        _ => None,
    };
    Ok(JournalEntry {
        import_id,
        character_id: character.parse()?,
        server_id,
        revision: revision as u64,
        marker,
        state: ImportState::parse(&state)?,
        items: serde_json::from_str(&items)?,
        pet_ids: serde_json::from_str(&pets)?,
        allocation,
        detail,
        created_at,
        updated_at,
    })
}
