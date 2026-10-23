//! The import journal: how a realm commit and a local commit stay consistent although they cannot be one transaction.
//!
//! ```text
//!   begin_import / begin_update   local commit 1   journal row "prepared" (nonce, the plan, the canonical snapshot synced)
//!   realm transaction                              writes the character (or its update) AND the marker
//!                                                  `coa.portable.import = <nonce> <revision>`
//!   finish_import                 local commit 2   mappings + binding + synced snapshot + journal "committed", in ONE tx
//! ```
//!
//! A crash can fall in two places. Before the realm commit: no marker exists, nothing happened in the realm; the entry is
//! aborted. After the realm commit but before `finish_import`: the marker exists in the realm, so the import *did*
//! happen and `finish_import` is simply run again from what the marker and the realm's rows say. The marker is part of
//! the realm transaction, so "marker present" and "import committed" are the same fact.
//!
//! An **update** (an existing realm character brought to a newer canonical revision, in place) is journaled the same way;
//! it additionally records which items and pets it retires from the realm's mappings.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::*;
use crate::portable::ids::{ImportId, PortablePetId};

/// One item of the plan, as far as the mapping needs to know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedItem {
    pub id: PortableItemId,
    pub entry: ContentId,
    pub identity: String,
}

/// One pet of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPet {
    pub id: PortablePetId,
    pub entry: ContentId,
    pub identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    /// A new character is created in the realm.
    Import,
    /// An existing realm character is updated in place.
    Update,
}

impl JournalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JournalKind::Import => "import",
            JournalKind::Update => "update",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        match s {
            "import" => Ok(JournalKind::Import),
            "update" => Ok(JournalKind::Update),
            other => Err(PortableError::Invalid(format!(
                "unknown journal kind {other:?}"
            ))),
        }
    }
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
            other => {
                return Err(PortableError::Invalid(format!(
                    "unknown import state {other:?}"
                )))
            }
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
    pub kind: JournalKind,
    pub character_id: CharacterId,
    pub server_id: String,
    pub revision: u64,
    pub marker: String,
    pub state: ImportState,
    /// Items the realm gets: item `i` is realm guid `item_base + i`.
    pub items: Vec<PlannedItem>,
    /// Pets the realm gets: pet `i` is `pet_base + i`.
    pub pets: Vec<PlannedPet>,
    /// An update only: items / pets removed from the realm.
    pub retired_items: Vec<PortableItemId>,
    pub retired_pets: Vec<PortablePetId>,
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
    format!(
        "{} {} {} {} {} ",
        nonce[0], nonce[1], nonce[2], nonce[3], revision
    )
}

/// What an update changes in the realm's mappings.
#[derive(Debug, Clone, Default)]
pub struct UpdatePlan {
    pub added_items: Vec<PlannedItem>,
    pub added_pets: Vec<PlannedPet>,
    pub retired_items: Vec<PortableItemId>,
    pub retired_pets: Vec<PortablePetId>,
}

impl Store {
    /// Record the intention to import `character` at its current canonical `revision` into `server_id`.
    /// Refused when the revision is stale, when the character is already on that server, or when another import of the
    /// same pair is unfinished.
    pub fn begin_import(
        &mut self,
        character: CharacterId,
        server_id: &str,
        revision: u64,
        items: &[PlannedItem],
        pets: &[PlannedPet],
    ) -> Result<ImportTicket> {
        self.begin(
            JournalKind::Import,
            character,
            server_id,
            revision,
            UpdatePlan {
                added_items: items.to_vec(),
                added_pets: pets.to_vec(),
                ..UpdatePlan::default()
            },
        )
    }

    /// Record the intention to bring the existing realm character to canonical `revision`, in place.
    pub fn begin_update(
        &mut self,
        character: CharacterId,
        server_id: &str,
        revision: u64,
        plan: UpdatePlan,
    ) -> Result<ImportTicket> {
        self.begin(JournalKind::Update, character, server_id, revision, plan)
    }

    fn begin(
        &mut self,
        kind: JournalKind,
        character: CharacterId,
        server_id: &str,
        revision: u64,
        plan: UpdatePlan,
    ) -> Result<ImportTicket> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let record = read_character(&tx, character)?;
        if record.revision != revision {
            return Err(PortableError::StaleRevision {
                expected: revision,
                current: record.revision,
            });
        }
        let open: Option<String> = tx
            .query_row("SELECT import_id FROM import_journal WHERE character_id = ?1 AND server_id = ?2 AND state = 'prepared'", params![character.to_string(), server_id], |r| r.get(0))
            .optional()?;
        if let Some(open) = open {
            return Err(PortableError::ImportInProgress {
                import_id: open.parse()?,
            });
        }
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2)",
            params![character.to_string(), server_id],
            |r| r.get(0),
        )?;
        match (kind, bound) {
            (JournalKind::Import, true) => {
                return Err(PortableError::AlreadyOnRealm {
                    character,
                    server_id: server_id.to_string(),
                })
            }
            (JournalKind::Update, false) => {
                return Err(PortableError::Invalid(format!(
                "character {character} is not on server {server_id}: there is nothing to update"
            )))
            }
            _ => {}
        }
        let (target, _) = read_snapshot_row(&tx, character, revision)?;
        let encoded = snapshot::encode(&target)?;
        let import_id = ImportId::new();
        let nonce = nonce();
        let marker = marker_text(nonce, revision);
        let at = now();
        tx.execute(
            "INSERT INTO import_journal(import_id, character_id, server_id, revision, marker, state, items, pet_ids, kind, retired_items, retired_pets, target_hash, target_payload, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'prepared', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                import_id.to_string(),
                character.to_string(),
                server_id,
                revision as i64,
                marker,
                serde_json::to_string(&plan.added_items)?,
                serde_json::to_string(&plan.added_pets)?,
                kind.as_str(),
                serde_json::to_string(&plan.retired_items)?,
                serde_json::to_string(&plan.retired_pets)?,
                encoded.content_hash.as_slice(),
                encoded.payload,
                at
            ],
        )?;
        tx.commit()?;
        Ok(ImportTicket {
            import_id,
            nonce,
            revision,
            marker,
        })
    }

    /// The realm committed: record the binding (or the update), the item and pet mappings, the synced snapshot and close the
    /// journal entry, in **one** local transaction. Safe to call again for an entry that is already committed.
    pub fn finish_import(
        &mut self,
        import_id: ImportId,
        allocation: ImportAllocation,
    ) -> Result<()> {
        let tx = self.write_tx()?;
        let entry = read_journal(&tx, import_id)?;
        match entry.state {
            ImportState::Committed => return Ok(()),
            ImportState::Prepared => {}
            other => {
                return Err(PortableError::ImportState {
                    import_id,
                    state: other.as_str().into(),
                })
            }
        }
        let guid_of = |base: u32, i: usize| {
            u32::try_from(base as u64 + i as u64)
                .map_err(|_| PortableError::Invalid("a realm id is out of range".into()))
        };
        let item_rows: Vec<(PortableItemId, u32, ContentId, String)> = entry
            .items
            .iter()
            .enumerate()
            .map(|(i, p)| {
                Ok((
                    p.id,
                    guid_of(allocation.item_base, i)?,
                    p.entry.clone(),
                    p.identity.clone(),
                ))
            })
            .collect::<Result<_>>()?;
        let pet_rows: Vec<(PortablePetId, u32, String)> = entry
            .pets
            .iter()
            .enumerate()
            .map(|(i, p)| Ok((p.id, guid_of(allocation.pet_base, i)?, p.identity.clone())))
            .collect::<Result<_>>()?;

        match entry.kind {
            JournalKind::Import => {
                bind_in_tx(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    allocation.local_guid,
                    entry.revision,
                    MappingState::Synced,
                )?;
                let observations: Vec<ItemObservation> = item_rows
                    .iter()
                    .map(|(id, guid, entry, identity)| ItemObservation {
                        portable_item_id: *id,
                        local_item_guid: *guid,
                        entry: entry.clone(),
                        identity: identity.clone(),
                    })
                    .collect();
                reconcile_in_tx(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    entry.revision,
                    &observations,
                )?;
                let pets: Vec<PetObservation> = pet_rows
                    .iter()
                    .map(|(id, number, identity)| PetObservation {
                        portable_pet_id: *id,
                        local_pet_number: *number,
                        identity: identity.clone(),
                    })
                    .collect();
                sync_pets_in_tx(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    entry.revision,
                    &pets,
                    &PetProtection::default(),
                )?;
            }
            JournalKind::Update => {
                let local: i64 = tx.query_row("SELECT local_guid FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![entry.character_id.to_string(), entry.server_id], |r| r.get(0))?;
                if local != allocation.local_guid as i64 {
                    return Err(PortableError::Invalid(format!(
                        "the update reports local character {} but the realm binding is {local}",
                        allocation.local_guid
                    )));
                }
                retire_items_by_id(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    &entry.retired_items,
                    entry.revision,
                )?;
                retire_pets_by_id(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    &entry.retired_pets,
                    entry.revision,
                )?;
                add_item_mappings(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    entry.revision,
                    &item_rows,
                )?;
                add_pet_mappings(
                    &tx,
                    entry.character_id,
                    &entry.server_id,
                    entry.revision,
                    &pet_rows,
                )?;
                tx.execute(
                    "UPDATE character_server_mapping SET last_revision = ?3, state = 'synced', updated_at = ?4 WHERE character_id = ?1 AND server_id = ?2",
                    params![entry.character_id.to_string(), entry.server_id, entry.revision as i64, now()],
                )?;
            }
        }
        let (hash, payload): (Vec<u8>, Vec<u8>) = tx.query_row(
            "SELECT target_hash, target_payload FROM import_journal WHERE import_id = ?1",
            [import_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let target = snapshot::decode(&payload, Some(&hash_from_blob(hash)?))?;
        set_synced_in_tx(
            &tx,
            entry.character_id,
            &entry.server_id,
            &snapshot::encode(&target)?,
        )?;
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

    fn close_import(
        &mut self,
        import_id: ImportId,
        state: ImportState,
        detail: &str,
    ) -> Result<()> {
        let tx = self.write_tx()?;
        let entry = read_journal(&tx, import_id)?;
        if entry.state != ImportState::Prepared {
            return Err(PortableError::ImportState {
                import_id,
                state: entry.state.as_str().into(),
            });
        }
        tx.execute("UPDATE import_journal SET state = ?2, detail = ?3, updated_at = ?4 WHERE import_id = ?1", params![import_id.to_string(), state.as_str(), detail, now()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn import_entry(&self, import_id: ImportId) -> Result<JournalEntry> {
        read_journal(&self.conn, import_id)
    }

    /// Unfinished imports and updates into one server, oldest first.
    pub fn open_imports(&self, server_id: &str) -> Result<Vec<JournalEntry>> {
        let mut stmt = self.conn.prepare("SELECT import_id FROM import_journal WHERE server_id = ?1 AND state = 'prepared' ORDER BY import_id")?;
        let ids: Vec<String> = stmt
            .query_map([server_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        ids.iter()
            .map(|id| read_journal(&self.conn, id.parse()?))
            .collect()
    }
}

fn read_journal(conn: &Connection, import_id: ImportId) -> Result<JournalEntry> {
    type Row = (
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT character_id, server_id, revision, marker, state, items, pet_ids, local_guid, item_base, pet_base, detail, created_at, updated_at, kind, retired_items, retired_pets
             FROM import_journal WHERE import_id = ?1",
            [import_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?, r.get(13)?, r.get(14)?, r.get(15)?)),
        )
        .optional()?;
    let Some((
        character,
        server_id,
        revision,
        marker,
        state,
        items,
        pets,
        guid,
        item_base,
        pet_base,
        detail,
        created_at,
        updated_at,
        kind,
        retired_items,
        retired_pets,
    )) = row
    else {
        return Err(PortableError::UnknownImport(import_id));
    };
    let allocation = match (guid, item_base, pet_base) {
        (Some(g), Some(i), Some(p)) => Some(ImportAllocation {
            local_guid: g as u32,
            item_base: i as u32,
            pet_base: p as u32,
        }),
        _ => None,
    };
    Ok(JournalEntry {
        import_id,
        kind: JournalKind::parse(&kind)?,
        character_id: character.parse()?,
        server_id,
        revision: revision as u64,
        marker,
        state: ImportState::parse(&state)?,
        items: serde_json::from_str(&items)?,
        pets: serde_json::from_str(&pets)?,
        retired_items: serde_json::from_str(&retired_items)?,
        retired_pets: serde_json::from_str(&retired_pets)?,
        allocation,
        detail,
        created_at,
        updated_at,
    })
}
