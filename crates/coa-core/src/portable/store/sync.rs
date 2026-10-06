//! What the store remembers between the sessions of one character on one realm (Phase 4):
//!
//! * stable **pet mappings** (portable pet id <-> the realm's pet number), with the same lifecycle and content identity as
//!   item mappings, so a recycled pet number never inherits an old pet;
//! * the **synced snapshot** of a realm: the canonical state the realm was last brought to, the *base* of the next in-place
//!   update;
//! * the **session baseline**: `C0` (canonical at join) and `B0` (the realm after its own first load/save, before any
//!   progression), persisted so a reconciliation survives restarts and history pruning;
//! * `commit_reconciled`: the one local transaction that turns a reconciled character into the next canonical revision.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use super::*;
use crate::portable::ids::PortablePetId;

// ---- pets ---------------------------------------------------------------------------------------------------------------

/// What a realm currently holds for one pet of a character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetObservation {
    pub portable_pet_id: PortablePetId,
    pub local_pet_number: u32,
    pub identity: String,
}

/// What cannot change during a pet's life: its creature, kind and the spell that summoned it. The name, level and spells
/// change all the time.
pub fn pet_identity(entry: &ContentId, pet_type: u8, created_by_spell: u32) -> String {
    let mut h = Sha256::new();
    h.update(b"pet-v1\0");
    h.update(entry.to_string().as_bytes());
    h.update([0, pet_type]);
    h.update(created_by_spell.to_be_bytes());
    format!("pet-v1:{}", hex::encode(&h.finalize()[..16]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetMapping {
    pub portable_pet_id: PortablePetId,
    pub local_pet_number: u32,
    pub identity: String,
    pub active: bool,
    pub presence: Presence,
    pub created_revision: u64,
    pub confirmed_revision: u64,
    pub retired_revision: Option<u64>,
    pub retired_reason: Option<RetireReason>,
}

#[derive(Debug, Clone, Default)]
pub struct PetProtection {
    pub canonical_pets: HashSet<PortablePetId>,
    pub realm_local_pets: HashSet<PortablePetId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PetSyncReport {
    pub confirmed: usize,
    pub added: usize,
    pub number_reused: usize,
    pub moved: usize,
    pub absent: usize,
    pub filtered: usize,
}

fn pet_presence(protection: &PetProtection, id: PortablePetId) -> Presence {
    if protection.realm_local_pets.contains(&id) {
        Presence::RealmLocal
    } else {
        Presence::Present
    }
}

fn retire_pet(tx: &Transaction<'_>, mapping_id: i64, reason: RetireReason, revision: u64, at: &str) -> Result<()> {
    tx.execute(
        "UPDATE pet_mapping SET state = 'retired', retired_revision = ?1, retired_reason = ?2, updated_at = ?3 WHERE mapping_id = ?4",
        params![revision as i64, reason.as_str(), at, mapping_id],
    )?;
    Ok(())
}

pub(super) fn retire_all_pets(tx: &Transaction<'_>, id: CharacterId, server_id: &str, reason: RetireReason, revision: u64) -> Result<usize> {
    Ok(tx.execute(
        "UPDATE pet_mapping SET state = 'retired', retired_revision = ?1, retired_reason = ?2, updated_at = ?3 WHERE character_id = ?4 AND server_id = ?5 AND state = 'active'",
        params![revision as i64, reason.as_str(), now(), id.to_string(), server_id],
    )?)
}

/// The pet twin of `reconcile_with`: bring the pet mappings in line with what the realm shows.
pub(super) fn sync_pets_in_tx(tx: &Transaction<'_>, id: CharacterId, server_id: &str, revision: u64, observations: &[PetObservation], protection: &PetProtection) -> Result<PetSyncReport> {
    let mut seen_pets = HashSet::new();
    let mut seen_numbers = HashSet::new();
    for o in observations {
        if !seen_pets.insert(o.portable_pet_id) || !seen_numbers.insert(o.local_pet_number) || o.identity.is_empty() {
            return Err(PortableError::Invalid("pet observations must be unique and carry an identity".into()));
        }
    }
    let at = now();
    let mut report = PetSyncReport::default();
    for o in observations {
        let by_number: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT mapping_id, portable_pet_id, identity FROM pet_mapping WHERE character_id = ?1 AND server_id = ?2 AND local_pet_number = ?3 AND state = 'active'",
                params![id.to_string(), server_id, o.local_pet_number],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((mapping_id, pet, identity)) = &by_number {
            if *pet == o.portable_pet_id.to_string() && *identity == o.identity {
                tx.execute("UPDATE pet_mapping SET confirmed_revision = ?1, updated_at = ?2, presence = ?4 WHERE mapping_id = ?3", params![revision as i64, at, mapping_id, pet_presence(protection, o.portable_pet_id).as_str()])?;
                report.confirmed += 1;
                continue;
            }
            retire_pet(tx, *mapping_id, RetireReason::GuidReused, revision, &at)?;
            report.number_reused += 1;
        }
        let by_pet: Option<i64> = tx
            .query_row(
                "SELECT mapping_id FROM pet_mapping WHERE character_id = ?1 AND server_id = ?2 AND portable_pet_id = ?3 AND state = 'active'",
                params![id.to_string(), server_id, o.portable_pet_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(mapping_id) = by_pet {
            retire_pet(tx, mapping_id, RetireReason::Moved, revision, &at)?;
            report.moved += 1;
        }
        tx.execute(
            "INSERT INTO pet_mapping(character_id, server_id, portable_pet_id, local_pet_number, identity, state, presence, created_revision, confirmed_revision, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7, ?8, ?8)",
            params![id.to_string(), server_id, o.portable_pet_id.to_string(), o.local_pet_number, o.identity, pet_presence(protection, o.portable_pet_id).as_str(), revision as i64, at],
        )?;
        report.added += 1;
    }
    let mut stmt = tx.prepare("SELECT mapping_id, portable_pet_id FROM pet_mapping WHERE character_id = ?1 AND server_id = ?2 AND state = 'active'")?;
    let active: Vec<(i64, String)> = stmt.query_map(params![id.to_string(), server_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for (mapping_id, pet) in active {
        if observations.iter().any(|o| o.portable_pet_id.to_string() == pet) {
            continue;
        }
        let owned = pet.parse::<PortablePetId>().map(|p| protection.canonical_pets.contains(&p)).unwrap_or(false);
        if owned {
            tx.execute("UPDATE pet_mapping SET presence = 'filtered', updated_at = ?2 WHERE mapping_id = ?1", params![mapping_id, at])?;
            report.filtered += 1;
        } else {
            retire_pet(tx, mapping_id, RetireReason::Absent, revision, &at)?;
            report.absent += 1;
        }
    }
    Ok(report)
}

/// Mappings for pets that are new on a realm (an import or an update added them): no observation of the others.
pub(super) fn add_pet_mappings(tx: &Transaction<'_>, id: CharacterId, server_id: &str, revision: u64, pets: &[(PortablePetId, u32, String)]) -> Result<()> {
    let at = now();
    for (pet, number, identity) in pets {
        tx.execute(
            "INSERT INTO pet_mapping(character_id, server_id, portable_pet_id, local_pet_number, identity, state, presence, created_revision, confirmed_revision, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', 'present', ?6, ?6, ?7, ?7)",
            params![id.to_string(), server_id, pet.to_string(), number, identity, revision as i64, at],
        )?;
    }
    Ok(())
}

pub(super) fn retire_pets_by_id(tx: &Transaction<'_>, id: CharacterId, server_id: &str, pets: &[PortablePetId], revision: u64) -> Result<()> {
    let at = now();
    for pet in pets {
        tx.execute(
            "UPDATE pet_mapping SET state = 'retired', retired_revision = ?1, retired_reason = 'absent', updated_at = ?2
             WHERE character_id = ?3 AND server_id = ?4 AND portable_pet_id = ?5 AND state = 'active'",
            params![revision as i64, at, id.to_string(), server_id, pet.to_string()],
        )?;
    }
    Ok(())
}

pub(super) fn retire_items_by_id(tx: &Transaction<'_>, id: CharacterId, server_id: &str, items: &[PortableItemId], revision: u64) -> Result<()> {
    let at = now();
    for item in items {
        tx.execute(
            "UPDATE item_mapping SET state = 'retired', retired_revision = ?1, retired_reason = 'absent', updated_at = ?2
             WHERE character_id = ?3 AND server_id = ?4 AND portable_item_id = ?5 AND state = 'active'",
            params![revision as i64, at, id.to_string(), server_id, item.to_string()],
        )?;
    }
    Ok(())
}

/// Item mappings for items that are new on a realm.
pub(super) fn add_item_mappings(tx: &Transaction<'_>, id: CharacterId, server_id: &str, revision: u64, items: &[(PortableItemId, u32, ContentId, String)]) -> Result<()> {
    let at = now();
    for (item, guid, entry, identity) in items {
        tx.execute(
            "INSERT INTO item_mapping(character_id, server_id, portable_item_id, local_item_guid, entry, identity, state, presence, created_revision, confirmed_revision, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 'present', ?7, ?7, ?8, ?8)",
            params![id.to_string(), server_id, item.to_string(), guid, entry.to_string(), identity, revision as i64, at],
        )?;
    }
    Ok(())
}

fn read_pet_mappings(conn: &Connection, id: CharacterId, server_id: &str, include_retired: bool) -> Result<Vec<PetMapping>> {
    let sql = format!(
        "SELECT portable_pet_id, local_pet_number, identity, state, presence, created_revision, confirmed_revision, retired_revision, retired_reason FROM pet_mapping
         WHERE character_id = ?1 AND server_id = ?2 {} ORDER BY mapping_id",
        if include_retired { "" } else { "AND state = 'active'" }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![id.to_string(), server_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, Option<i64>>(7)?, r.get::<_, Option<String>>(8)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (pet, number, identity, state, presence, created, confirmed, retired, reason) = row?;
        out.push(PetMapping {
            portable_pet_id: pet.parse()?,
            local_pet_number: number as u32,
            identity,
            active: state == "active",
            presence: Presence::parse(&presence)?,
            created_revision: created as u64,
            confirmed_revision: confirmed as u64,
            retired_revision: retired.map(|r| r as u64),
            retired_reason: reason.as_deref().map(RetireReason::parse).transpose()?,
        });
    }
    Ok(out)
}

// ---- synced snapshot ----------------------------------------------------------------------------------------------------------

pub(super) fn set_synced_in_tx(tx: &Transaction<'_>, id: CharacterId, server_id: &str, encoded: &EncodedSnapshot) -> Result<()> {
    let changed = tx.execute(
        "UPDATE character_server_mapping SET synced_hash = ?3, synced_payload = ?4, updated_at = ?5 WHERE character_id = ?1 AND server_id = ?2",
        params![id.to_string(), server_id, encoded.content_hash.as_slice(), encoded.payload, now()],
    )?;
    if changed == 0 {
        return Err(PortableError::Invalid(format!("character {id} is not bound to server {server_id}")));
    }
    Ok(())
}

// ---- baselines ------------------------------------------------------------------------------------------------------------------

/// The persisted base of a session reconciliation.
#[derive(Debug, Clone)]
pub struct Baseline {
    pub character_id: CharacterId,
    pub server_id: String,
    /// The canonical revision at join.
    pub c0_revision: u64,
    /// The canonical revision this session has produced so far.
    pub head_revision: u64,
    /// Canonical at join.
    pub c0: PortableCharacter,
    /// The realm after its own first load/save normalisation, before any progression.
    pub b0: PortableCharacter,
    pub created_at: String,
}

/// What the realm showed at the baseline.
pub struct BaselineInput<'a> {
    pub b0: &'a PortableCharacter,
    pub items: &'a [ItemObservation],
    pub pets: &'a [PetObservation],
}

impl Store {
    pub fn sync_pet_mappings(&mut self, id: CharacterId, server_id: &str, revision: u64, observations: &[PetObservation], protection: &PetProtection) -> Result<PetSyncReport> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let report = sync_pets_in_tx(&tx, id, server_id, revision, observations, protection)?;
        tx.commit()?;
        Ok(report)
    }

    pub fn active_pet_lookup(&self, id: CharacterId, server_id: &str) -> Result<HashMap<u32, (PortablePetId, String)>> {
        Ok(read_pet_mappings(&self.conn, id, server_id, false)?.into_iter().map(|m| (m.local_pet_number, (m.portable_pet_id, m.identity))).collect())
    }

    pub fn pet_mappings(&self, id: CharacterId, server_id: &str) -> Result<Vec<PetMapping>> {
        read_pet_mappings(&self.conn, id, server_id, false)
    }

    pub fn pet_mapping_history(&self, id: CharacterId, server_id: &str) -> Result<Vec<PetMapping>> {
        read_pet_mappings(&self.conn, id, server_id, true)
    }

    /// The canonical snapshot a realm was last synchronised with: the base of the next in-place update.
    pub fn synced_model(&self, id: CharacterId, server_id: &str) -> Result<Option<PortableCharacter>> {
        let row: Option<(Option<Vec<u8>>, Option<Vec<u8>>)> = self
            .conn
            .query_row("SELECT synced_hash, synced_payload FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match row {
            Some((Some(hash), Some(payload))) => Ok(Some(snapshot::decode(&payload, Some(&hash_from_blob(hash)?))?)),
            Some(_) => Ok(None),
            None => Err(PortableError::Invalid(format!("character {id} is not bound to server {server_id}"))),
        }
    }

    /// Persist the baseline of a session: `C0` (the snapshot the realm was synchronised with) and `B0` (what the realm shows
    /// after its own normalisation). Classifies the realm's items and pets: owned by the character but not shown by the realm
    /// = **filtered** (kept, mapping stays active); shown by the realm but not owned = **realm-local** (never merged).
    pub fn capture_baseline(&mut self, id: CharacterId, server_id: &str, input: BaselineInput<'_>) -> Result<Baseline> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let baseline = capture_baseline_in_tx(&tx, id, server_id, &input)?;
        tx.commit()?;
        Ok(baseline)
    }

    pub fn open_baseline(&self, id: CharacterId, server_id: &str) -> Result<Option<Baseline>> {
        read_open_baseline(&self.conn, id, server_id)
    }

    /// End the session: the baseline is kept for the record but no longer used.
    pub fn close_baseline(&mut self, id: CharacterId, server_id: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE realm_baseline SET state = 'closed', updated_at = ?3 WHERE character_id = ?1 AND server_id = ?2 AND state = 'open'",
            params![id.to_string(), server_id, now()],
        )?;
        Ok(changed > 0)
    }

    /// Turn a reconciled character into the next canonical revision, in **one** local transaction: the new snapshot, the
    /// session head, the realm's item/pet mappings as of `B1`, and the realm's synced snapshot.
    /// The canonical character must still be exactly what this session produced last (no foreign progress in between).
    pub fn commit_reconciled(&mut self, id: CharacterId, server_id: &str, merged: PortableCharacter, b1_items: &[ItemObservation], b1_pets: &[PetObservation], note: Option<&str>) -> Result<u64> {
        check_server_id(server_id)?;
        if merged.character_id != id {
            return Err(PortableError::WrongCharacter { expected: id, found: merged.character_id });
        }
        let encoded = snapshot::encode(&merged)?;
        let keep = self.history_keep()?;
        let tx = self.write_tx()?;
        let baseline = read_open_baseline(&tx, id, server_id)?.ok_or_else(|| PortableError::Invalid("there is no open session baseline for this character on this realm".into()))?;
        let record = read_character(&tx, id)?;
        if record.ruleset != merged.ruleset {
            return Err(PortableError::RulesetChange { from: record.ruleset.to_string(), to: merged.ruleset.to_string() });
        }
        if record.revision != baseline.head_revision {
            return Err(PortableError::StaleRevision { expected: baseline.head_revision, current: record.revision });
        }
        let at = now();
        // a session that changed nothing (or only what the realm normalised) makes no new revision
        let head_hash: Vec<u8> = tx.query_row("SELECT content_hash FROM snapshot WHERE character_id = ?1 AND revision = ?2", params![id.to_string(), record.revision as i64], |r| r.get(0))?;
        let revision = if head_hash.as_slice() == encoded.content_hash.as_slice() {
            record.revision
        } else {
            let revision = record.revision + 1;
            insert_snapshot(&tx, id, revision, &encoded, server_id, note, &at)?;
            update_character(&tx, id, &merged, revision, &at)?;
            prune_tx(&tx, id, revision, keep)?;
            tx.execute(
                "UPDATE realm_baseline SET head_revision = ?4, updated_at = ?5 WHERE character_id = ?1 AND server_id = ?2 AND c0_revision = ?3",
                params![id.to_string(), server_id, baseline.c0_revision as i64, revision as i64, at],
            )?;
            revision
        };

        let owned_items: HashSet<PortableItemId> = merged.items.iter().map(|i| i.id).collect();
        let protection = Protection { realm_local_items: b1_items.iter().map(|o| o.portable_item_id).filter(|i| !owned_items.contains(i)).collect(), canonical_items: owned_items };
        reconcile_with(&tx, id, server_id, revision, b1_items, &protection)?;
        let owned_pets: HashSet<PortablePetId> = merged.pets.iter().map(|p| p.id).collect();
        let pet_protection = PetProtection { realm_local_pets: b1_pets.iter().map(|o| o.portable_pet_id).filter(|p| !owned_pets.contains(p)).collect(), canonical_pets: owned_pets };
        sync_pets_in_tx(&tx, id, server_id, revision, b1_pets, &pet_protection)?;

        tx.execute(
            "UPDATE character_server_mapping SET last_revision = ?3, state = 'active', updated_at = ?4 WHERE character_id = ?1 AND server_id = ?2",
            params![id.to_string(), server_id, revision as i64, at],
        )?;
        set_synced_in_tx(&tx, id, server_id, &encoded)?;
        tx.commit()?;
        Ok(revision)
    }
}

pub(super) fn synced_model_in(conn: &Connection, id: CharacterId, server_id: &str) -> Result<Option<PortableCharacter>> {
    let row: Option<(Option<Vec<u8>>, Option<Vec<u8>>)> = conn
        .query_row("SELECT synced_hash, synced_payload FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    match row {
        Some((Some(hash), Some(payload))) => Ok(Some(snapshot::decode(&payload, Some(&hash_from_blob(hash)?))?)),
        Some(_) => Ok(None),
        None => Err(PortableError::Invalid(format!("character {id} is not bound to server {server_id}"))),
    }
}

/// Persist the baseline of a session inside a transaction: `C0` is the snapshot the realm was synchronised with, `B0` what the
/// realm shows after its own normalisation. Items and pets the canonical character owns but the realm does not show are
/// **filtered** (kept, mapping stays active); shown but not owned = **realm-local** (never merged).
pub(super) fn capture_baseline_in_tx(tx: &Transaction<'_>, id: CharacterId, server_id: &str, input: &BaselineInput<'_>) -> Result<Baseline> {
    let c0 = synced_model_in(tx, id, server_id)?.ok_or_else(|| PortableError::Invalid("the realm has no synchronised snapshot to use as the session base".into()))?;
    if input.b0.character_id != id || input.b0.ruleset != c0.ruleset {
        return Err(PortableError::WrongCharacter { expected: id, found: input.b0.character_id });
    }
    let record = read_character(tx, id)?;
    let mapping_revision: i64 = tx.query_row("SELECT last_revision FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| r.get(0))?;
    if record.revision != mapping_revision as u64 {
        return Err(PortableError::StaleRevision { expected: mapping_revision as u64, current: record.revision });
    }
    let open: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM realm_baseline WHERE character_id = ?1 AND server_id = ?2 AND state = 'open')", params![id.to_string(), server_id], |r| r.get(0))?;
    if open {
        return Err(PortableError::Invalid("this character already has an open session baseline on this realm".into()));
    }
    let c0_enc = snapshot::encode(&c0)?;
    let b0_enc = snapshot::encode(input.b0)?;
    let c0_items: HashSet<PortableItemId> = c0.items.iter().map(|i| i.id).collect();
    let c0_pets: HashSet<PortablePetId> = c0.pets.iter().map(|p| p.id).collect();
    let protection = Protection { realm_local_items: input.items.iter().map(|o| o.portable_item_id).filter(|i| !c0_items.contains(i)).collect(), canonical_items: c0_items };
    reconcile_with(tx, id, server_id, record.revision, input.items, &protection)?;
    let pet_protection = PetProtection { realm_local_pets: input.pets.iter().map(|o| o.portable_pet_id).filter(|p| !c0_pets.contains(p)).collect(), canonical_pets: c0_pets };
    sync_pets_in_tx(tx, id, server_id, record.revision, input.pets, &pet_protection)?;
    let at = now();
    tx.execute(
        "INSERT INTO realm_baseline(character_id, server_id, c0_revision, head_revision, state, c0_hash, c0_payload, b0_hash, b0_payload, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3, 'open', ?4, ?5, ?6, ?7, ?8, ?8)",
        params![id.to_string(), server_id, record.revision as i64, c0_enc.content_hash.as_slice(), c0_enc.payload, b0_enc.content_hash.as_slice(), b0_enc.payload, at],
    )?;
    Ok(Baseline { character_id: id, server_id: server_id.to_string(), c0_revision: record.revision, head_revision: record.revision, c0, b0: input.b0.clone(), created_at: at })
}

pub(super) fn read_open_baseline(conn: &Connection, id: CharacterId, server_id: &str) -> Result<Option<Baseline>> {
    type Row = (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String);
    let row: Option<Row> = conn
        .query_row(
            "SELECT c0_revision, head_revision, c0_hash, c0_payload, b0_hash, b0_payload, created_at FROM realm_baseline WHERE character_id = ?1 AND server_id = ?2 AND state = 'open'",
            params![id.to_string(), server_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()?;
    let Some((c0_revision, head, c0_hash, c0_payload, b0_hash, b0_payload, created_at)) = row else { return Ok(None) };
    Ok(Some(Baseline {
        character_id: id,
        server_id: server_id.to_string(),
        c0_revision: c0_revision as u64,
        head_revision: head as u64,
        c0: snapshot::decode(&c0_payload, Some(&hash_from_blob(c0_hash)?))?,
        b0: snapshot::decode(&b0_payload, Some(&hash_from_blob(b0_hash)?))?,
        created_at,
    }))
}
