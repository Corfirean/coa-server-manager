//! The HOST side of a runtime portable session: what the realm's Manager remembers about the sessions it runs.
//!
//! The Host keeps a **copy** of the canonical character at the revision the Owner handed over (it needs it to import, to map
//! items and to know what the realm was synchronised with); it never produces a new canonical revision. It keeps its own
//! sequence numbers and an outbox of messages the Owner has not acknowledged.

use rusqlite::{params, OptionalExtension};

use super::*;
use crate::portable::ids::{PortablePetId, SessionId};
use crate::portable::session::protocol::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    /// The realm holds the marker; `B0` has not been read yet.
    Armed,
    /// `B0` was read and sent; checkpoints run.
    Open,
    Closed,
}

impl HostState {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "armed" => HostState::Armed,
            "open" => HostState::Open,
            "closed" => HostState::Closed,
            other => return Err(PortableError::Invalid(format!("unknown host session state {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSession {
    pub session_id: SessionId,
    pub character_id: CharacterId,
    pub server_id: String,
    pub local_guid: Option<u32>,
    pub base_revision: u64,
    pub generation: u32,
    pub state: HostState,
    pub next_sequence: u64,
    pub pending_sequence: Option<u64>,
    pub acked_sequence: u64,
    pub owned_items: Vec<PortableItemId>,
    pub owned_pets: Vec<PortablePetId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    pub session_id: SessionId,
    pub sequence: u64,
    pub started: bool,
    pub bytes: Vec<u8>,
}

/// What an acknowledgement changed on the Host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckEffect {
    /// Nothing was waiting for it (an acknowledgement delivered twice).
    Ignored,
    Applied,
    /// The final checkpoint was applied: the realm is now synchronised with the canonical character, and this session follows.
    Finished { next_session: SessionId, next_revision: u64 },
    /// The Owner will never accept this message (stale or rejected); it was dropped from the outbox.
    Dropped(String),
}

fn read_host_session(conn: &Connection, session: SessionId) -> Result<Option<HostSession>> {
    type Raw = (String, String, Option<i64>, i64, i64, String, i64, Option<i64>, i64, String, String);
    let raw: Option<Raw> = conn
        .query_row(
            "SELECT character_id, server_id, local_guid, base_revision, generation, state, next_sequence, pending_sequence, acked_sequence, owned_items, owned_pets FROM host_session WHERE session_id = ?1",
            [session.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
        )
        .optional()?;
    let Some((character, server_id, guid, base, generation, state, next, pending, acked, items, pets)) = raw else { return Ok(None) };
    Ok(Some(HostSession {
        session_id: session,
        character_id: character.parse()?,
        server_id,
        local_guid: guid.map(|g| g as u32),
        base_revision: base as u64,
        generation: generation as u32,
        state: HostState::parse(&state)?,
        next_sequence: next as u64,
        pending_sequence: pending.map(|p| p as u64),
        acked_sequence: acked as u64,
        owned_items: serde_json::from_str(&items)?,
        owned_pets: serde_json::from_str(&pets)?,
    }))
}

fn insert_host_session(tx: &Transaction<'_>, session: SessionId, id: CharacterId, server_id: &str, guid: Option<u32>, base: u64, generation: u32, model: &PortableCharacter) -> Result<()> {
    let items: Vec<PortableItemId> = model.items.iter().map(|i| i.id).collect();
    let pets: Vec<PortablePetId> = model.pets.iter().map(|p| p.id).collect();
    let at = now();
    tx.execute(
        "INSERT INTO host_session(session_id, character_id, server_id, local_guid, base_revision, generation, state, next_sequence, acked_sequence, owned_items, owned_pets, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'armed', 1, 0, ?7, ?8, ?9, ?9)",
        params![session.to_string(), id.to_string(), server_id, guid, base as i64, generation, serde_json::to_string(&items)?, serde_json::to_string(&pets)?, at],
    )?;
    Ok(())
}

/// Install (or advance to) the canonical character the Owner handed over, as the Host's copy. The revision is the Owner's.
fn install_copy_in_tx(tx: &Transaction<'_>, profile: ProfileId, model: &PortableCharacter, revision: u64, source_server_id: &str) -> Result<()> {
    let encoded = snapshot::encode(model)?;
    let id = model.character_id;
    let at = now();
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM character WHERE character_id = ?1)", [id.to_string()], |r| r.get(0))?;
    if !exists {
        tx.execute(
            "INSERT INTO character(character_id, profile_id, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 0)",
            params![id.to_string(), profile.to_string(), model.ruleset.as_str(), model.identity.name, model.identity.race.to_string(), model.identity.class.to_string(), model.identity.gender, model.progression.level, revision as i64, at],
        )?;
        insert_snapshot(tx, id, revision, &encoded, source_server_id, Some("received"), &at)?;
        return Ok(());
    }
    let record = read_character(tx, id)?;
    if record.ruleset != model.ruleset {
        return Err(PortableError::RulesetChange { from: record.ruleset.to_string(), to: model.ruleset.to_string() });
    }
    if revision < record.revision {
        return Err(PortableError::StaleRevision { expected: revision, current: record.revision });
    }
    if revision == record.revision {
        let (_, have) = read_snapshot_row(tx, id, revision)?;
        if have.content_hash != encoded.content_hash {
            return Err(PortableError::Invalid("the Host already holds a different canonical state at this revision".into()));
        }
        return Ok(());
    }
    insert_snapshot(tx, id, revision, &encoded, source_server_id, Some("received"), &at)?;
    update_character(tx, id, model, revision, &at)?;
    Ok(())
}

/// Presence of every active mapping after the Owner said what the canonical character owns: owned and shown = present, owned and
/// not shown = filtered, shown and not owned = realm-local, neither = retired.
fn reclassify_in_tx(tx: &Transaction<'_>, id: CharacterId, server_id: &str, revision: u64, items: &[PortableItemId], pets: &[PortablePetId]) -> Result<()> {
    let at = now();
    for (table, column, owned) in [("item_mapping", "portable_item_id", items.iter().map(|i| i.to_string()).collect::<std::collections::HashSet<_>>()), ("pet_mapping", "portable_pet_id", pets.iter().map(|p| p.to_string()).collect())] {
        let mut stmt = tx.prepare(&format!("SELECT mapping_id, {column}, presence FROM {table} WHERE character_id = ?1 AND server_id = ?2 AND state = 'active'"))?;
        let rows: Vec<(i64, String, String)> = stmt.query_map(params![id.to_string(), server_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
        drop(stmt);
        for (mapping, portable, presence) in rows {
            let observed = presence != "filtered";
            let is_owned = owned.contains(&portable);
            match (is_owned, observed) {
                (true, true) => tx.execute(&format!("UPDATE {table} SET presence = 'present', updated_at = ?2 WHERE mapping_id = ?1"), params![mapping, at])?,
                (true, false) => 0,
                (false, true) => tx.execute(&format!("UPDATE {table} SET presence = 'realm_local', updated_at = ?2 WHERE mapping_id = ?1"), params![mapping, at])?,
                (false, false) => tx.execute(&format!("UPDATE {table} SET state = 'retired', retired_revision = ?3, retired_reason = 'absent', updated_at = ?2 WHERE mapping_id = ?1"), params![mapping, at, revision as i64])?,
            };
        }
    }
    Ok(())
}

impl Store {
    /// The Host's copy of the character the Owner handed over (creates it, or advances it to a newer revision).
    pub fn host_install_copy(&mut self, profile: ProfileId, model: &PortableCharacter, revision: u64, owner_server_id: &str) -> Result<()> {
        check_server_id(owner_server_id)?;
        let tx = self.write_tx()?;
        install_copy_in_tx(&tx, profile, model, revision, owner_server_id)?;
        tx.commit()?;
        Ok(())
    }

    /// Record that a session is about to be armed on the realm (before the import runs, so a crash in between is recoverable).
    pub fn host_prepare_session(&mut self, offer: &SessionOffer, model: &PortableCharacter) -> Result<HostSession> {
        let tx = self.write_tx()?;
        if let Some(existing) = read_host_session(&tx, offer.session_id)? {
            return Ok(existing);
        }
        insert_host_session(&tx, offer.session_id, offer.character_id, &offer.server_id, None, offer.canonical_revision, 1, model)?;
        tx.commit()?;
        Ok(read_host_session(&self.conn, offer.session_id)?.expect("just inserted"))
    }

    /// The import committed: the session now belongs to this local character.
    pub fn host_bind_session(&mut self, session: SessionId, local_guid: u32) -> Result<()> {
        let changed = self.conn.execute("UPDATE host_session SET local_guid = ?2, updated_at = ?3 WHERE session_id = ?1 AND (local_guid IS NULL OR local_guid = ?2)", params![session.to_string(), local_guid, now()])?;
        if changed == 0 {
            return Err(PortableError::Invalid(format!("session {session} does not exist or is bound to another character")));
        }
        Ok(())
    }

    pub fn host_session(&self, session: SessionId) -> Result<Option<HostSession>> {
        read_host_session(&self.conn, session)
    }

    /// Armed and open sessions on one realm, oldest first.
    pub fn host_live_sessions(&self, server_id: &str) -> Result<Vec<HostSession>> {
        let mut stmt = self.conn.prepare("SELECT session_id FROM host_session WHERE server_id = ?1 AND state IN ('armed', 'open') ORDER BY session_id")?;
        let ids: Vec<String> = stmt.query_map([server_id], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        ids.iter().map(|s| Ok(read_host_session(&self.conn, s.parse()?)?.expect("it was just listed"))).collect()
    }

    /// Reserve the sequence of the next checkpoint **before** the realm is asked to save. Asking again returns the same number
    /// until the checkpoint is queued, so a restart resumes the same checkpoint.
    pub fn host_begin_checkpoint(&mut self, session: SessionId) -> Result<u64> {
        let tx = self.write_tx()?;
        let s = read_host_session(&tx, session)?.ok_or_else(|| PortableError::Invalid(format!("unknown session {session}")))?;
        if s.state != HostState::Open {
            return Err(PortableError::Invalid("a checkpoint needs an open session".into()));
        }
        let sequence = s.pending_sequence.unwrap_or(s.next_sequence);
        tx.execute("UPDATE host_session SET pending_sequence = ?2, updated_at = ?3 WHERE session_id = ?1", params![session.to_string(), sequence as i64, now()])?;
        tx.commit()?;
        Ok(sequence)
    }

    /// `B0` was read from the realm: persist the baseline (presence of items and pets), queue `PortableSessionStarted`, open.
    pub fn host_queue_started(&mut self, session: SessionId, msg: &PortableSessionStarted, b0: &PortableCharacter, items: &[ItemObservation], pets: &[PetObservation]) -> Result<()> {
        let tx = self.write_tx()?;
        let s = read_host_session(&tx, session)?.ok_or_else(|| PortableError::Invalid(format!("unknown session {session}")))?;
        match s.state {
            HostState::Armed => {}
            _ => return Err(PortableError::Invalid("this session already has its baseline".into())),
        }
        capture_baseline_in_tx(&tx, s.character_id, &s.server_id, &BaselineInput { b0, items, pets })?;
        let bytes = to_json(msg)?;
        tx.execute("INSERT INTO host_outbox(session_id, sequence, kind, message, state, created_at) VALUES (?1, 0, 'started', ?2, 'pending', ?3)", params![session.to_string(), bytes, now()])?;
        tx.execute("UPDATE host_session SET state = 'open', generation = ?2, updated_at = ?3 WHERE session_id = ?1", params![session.to_string(), msg.baseline_generation, now()])?;
        tx.commit()?;
        Ok(())
    }

    /// `B1` was read after the realm's checkpoint marker appeared: register what the realm shows (so new items keep their ids in
    /// the next checkpoint) and queue the message.
    pub fn host_queue_checkpoint(&mut self, session: SessionId, msg: &PortableCheckpoint, items: &[ItemObservation], pets: &[PetObservation]) -> Result<()> {
        let tx = self.write_tx()?;
        let s = read_host_session(&tx, session)?.ok_or_else(|| PortableError::Invalid(format!("unknown session {session}")))?;
        if s.state != HostState::Open || s.pending_sequence != Some(msg.sequence) {
            return Err(PortableError::Invalid("this checkpoint was not reserved".into()));
        }
        let baseline = read_open_baseline(&tx, s.character_id, &s.server_id)?.ok_or(PortableError::NoBaseline)?;
        let (c0_items, b0_items): (std::collections::HashSet<_>, std::collections::HashSet<_>) = (baseline.c0.items.iter().map(|i| i.id).collect(), baseline.b0.items.iter().map(|i| i.id).collect());
        let (c0_pets, b0_pets): (std::collections::HashSet<_>, std::collections::HashSet<_>) = (baseline.c0.pets.iter().map(|p| p.id).collect(), baseline.b0.pets.iter().map(|p| p.id).collect());
        // what the Owner is expected to own: what it owned at the last acknowledgement plus what is new since B0
        let mut canonical_items: std::collections::HashSet<_> = s.owned_items.iter().copied().collect();
        canonical_items.extend(items.iter().map(|o| o.portable_item_id).filter(|i| !b0_items.contains(i) && !c0_items.contains(i)));
        let mut canonical_pets: std::collections::HashSet<_> = s.owned_pets.iter().copied().collect();
        canonical_pets.extend(pets.iter().map(|o| o.portable_pet_id).filter(|p| !b0_pets.contains(p) && !c0_pets.contains(p)));
        let mapping_revision: i64 = tx.query_row("SELECT last_revision FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![s.character_id.to_string(), s.server_id], |r| r.get(0))?;
        let protection = Protection { realm_local_items: items.iter().map(|o| o.portable_item_id).filter(|i| !canonical_items.contains(i)).collect(), canonical_items };
        reconcile_with(&tx, s.character_id, &s.server_id, mapping_revision as u64, items, &protection)?;
        let pet_protection = PetProtection { realm_local_pets: pets.iter().map(|o| o.portable_pet_id).filter(|p| !canonical_pets.contains(p)).collect(), canonical_pets };
        sync_pets_in_tx(&tx, s.character_id, &s.server_id, mapping_revision as u64, pets, &pet_protection)?;
        let bytes = to_json(msg)?;
        tx.execute("INSERT INTO host_outbox(session_id, sequence, kind, message, state, created_at) VALUES (?1, ?2, 'checkpoint', ?3, 'pending', ?4)", params![session.to_string(), msg.sequence as i64, bytes, now()])?;
        tx.execute("UPDATE host_session SET next_sequence = ?2, pending_sequence = NULL, updated_at = ?3 WHERE session_id = ?1", params![session.to_string(), msg.sequence as i64 + 1, now()])?;
        tx.commit()?;
        Ok(())
    }

    /// The bytes of one queued message, acknowledged or not.
    pub fn host_message(&self, session: SessionId, sequence: u64) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.query_row("SELECT message FROM host_outbox WHERE session_id = ?1 AND sequence = ?2", params![session.to_string(), sequence as i64], |r| r.get(0)).optional()?)
    }

    /// Messages the Owner has not acknowledged yet, in the order they must be delivered.
    pub fn host_outbox_pending(&self, server_id: &str) -> Result<Vec<OutboxMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT o.session_id, o.sequence, o.kind, o.message FROM host_outbox o JOIN host_session s ON s.session_id = o.session_id
             WHERE s.server_id = ?1 AND o.state = 'pending' ORDER BY o.created_at, o.sequence",
        )?;
        let rows = stmt.query_map([server_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (session, sequence, kind, bytes) = row?;
            out.push(OutboxMessage { session_id: session.parse()?, sequence: sequence as u64, started: kind == "started", bytes });
        }
        Ok(out)
    }

    /// Apply the Owner's acknowledgement.
    pub fn host_receive_ack(&mut self, ack: &OwnerAck) -> Result<AckEffect> {
        let tx = self.write_tx()?;
        let Some(s) = read_host_session(&tx, ack.session_id)? else { return Ok(AckEffect::Ignored) };
        let waiting: Option<String> = tx
            .query_row("SELECT state FROM host_outbox WHERE session_id = ?1 AND sequence = ?2", params![ack.session_id.to_string(), ack.sequence as i64], |r| r.get(0))
            .optional()?;
        match waiting.as_deref() {
            None => return Ok(AckEffect::Ignored),
            Some("acked") => return Ok(AckEffect::Ignored),
            Some(_) => {}
        }
        let at = now();
        if !ack.outcome.accepted() {
            tx.execute("UPDATE host_outbox SET state = 'acked' WHERE session_id = ?1 AND sequence = ?2", params![ack.session_id.to_string(), ack.sequence as i64])?;
            if matches!(ack.outcome, AckOutcome::StaleSession) {
                tx.execute("UPDATE host_session SET state = 'closed', updated_at = ?2 WHERE session_id = ?1", params![ack.session_id.to_string(), at])?;
            }
            tx.commit()?;
            return Ok(AckEffect::Dropped(format!("{:?}", ack.outcome)));
        }
        tx.execute("UPDATE host_outbox SET state = 'acked' WHERE session_id = ?1 AND sequence = ?2", params![ack.session_id.to_string(), ack.sequence as i64])?;
        if ack.sequence > 0 {
            tx.execute(
                "UPDATE host_session SET acked_sequence = max(acked_sequence, ?2), owned_items = ?3, owned_pets = ?4, updated_at = ?5 WHERE session_id = ?1",
                params![ack.session_id.to_string(), ack.sequence as i64, serde_json::to_string(&ack.owned_items)?, serde_json::to_string(&ack.owned_pets)?, at],
            )?;
            reclassify_in_tx(&tx, s.character_id, &s.server_id, ack.canonical_revision, &ack.owned_items, &ack.owned_pets)?;
        }
        let (Some(canonical), Some(next)) = (&ack.canonical, &ack.next_session) else {
            tx.commit()?;
            return Ok(AckEffect::Applied);
        };

        // the final checkpoint: the realm is now synchronised with the canonical character, and the next session follows
        let model = canonical.open()?;
        if hex::encode(canonical.hash()?) != ack.canonical_hash || model.character_id != s.character_id {
            return Err(PortableError::Invalid("the final acknowledgement carries another character than its session".into()));
        }
        let profile: String = tx.query_row("SELECT profile_id FROM character WHERE character_id = ?1", [s.character_id.to_string()], |r| r.get(0))?;
        install_copy_in_tx(&tx, profile.parse()?, &model, ack.canonical_revision, "owner")?;
        let encoded = snapshot::encode(&model)?;
        tx.execute("UPDATE character_server_mapping SET last_revision = ?3, state = 'synced', updated_at = ?4 WHERE character_id = ?1 AND server_id = ?2", params![s.character_id.to_string(), s.server_id, ack.canonical_revision as i64, at])?;
        set_synced_in_tx(&tx, s.character_id, &s.server_id, &encoded)?;
        tx.execute("UPDATE realm_baseline SET state = 'closed', updated_at = ?3 WHERE character_id = ?1 AND server_id = ?2 AND state = 'open'", params![s.character_id.to_string(), s.server_id, at])?;
        tx.execute("UPDATE host_session SET state = 'closed', updated_at = ?2 WHERE session_id = ?1", params![ack.session_id.to_string(), at])?;
        insert_host_session(&tx, next.session_id, s.character_id, &s.server_id, s.local_guid, next.canonical_revision, s.generation + 1, &model)?;
        tx.commit()?;
        Ok(AckEffect::Finished { next_session: next.session_id, next_revision: next.canonical_revision })
    }
}

// ---- account collections (Phase 6) ---------------------------------------------------------------------------------------------

/// What the Host remembers of one realm account's collection of one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCollection {
    pub fingerprint: String,
    pub observed_hash: Option<[u8; 32]>,
    pub acked_hash: Option<[u8; 32]>,
    pub canonical_revision: u64,
    pub canonical_hash: Option<[u8; 32]>,
    pub pending: Option<Vec<u8>>,
    pub checked_at: u64,
}

/// A collection message waiting for the Owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionOutboxMessage {
    pub account: u32,
    pub kind: String,
    pub bytes: Vec<u8>,
}

fn opt_hash(blob: Option<Vec<u8>>) -> Result<Option<[u8; 32]>> {
    blob.map(|b| <[u8; 32]>::try_from(b.as_slice()).map_err(|_| PortableError::CorruptSnapshot("a stored collection hash is not 32 bytes".into()))).transpose()
}

impl Store {
    pub fn host_collection(&self, server_id: &str, account: u32, kind: &str) -> Result<Option<HostCollection>> {
        type Raw = (String, Option<Vec<u8>>, Option<Vec<u8>>, i64, Option<Vec<u8>>, Option<Vec<u8>>, i64);
        let raw: Option<Raw> = self
            .conn
            .query_row(
                "SELECT fingerprint, observed_hash, acked_hash, canonical_revision, canonical_hash, pending, checked_at FROM host_collection WHERE server_id = ?1 AND account = ?2 AND kind = ?3",
                params![server_id, account, kind],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .optional()?;
        raw.map(|(fingerprint, observed, acked, revision, canonical, pending, checked_at)| {
            Ok(HostCollection { fingerprint, observed_hash: opt_hash(observed)?, acked_hash: opt_hash(acked)?, canonical_revision: revision as u64, canonical_hash: opt_hash(canonical)?, pending, checked_at: checked_at as u64 })
        })
        .transpose()
    }

    /// The realm account was read: remember the fingerprint and the hash of what it showed, and queue `pending` for the Owner
    /// (or none when the Owner already acknowledged exactly this set). A newer observation replaces an older pending one.
    pub fn host_collection_observe(&mut self, server_id: &str, account: u32, kind: &str, fingerprint: &str, observed: &[u8; 32], pending: Option<&[u8]>, checked_at: u64) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute(
            "INSERT INTO host_collection(server_id, account, kind, fingerprint, observed_hash, pending, checked_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(server_id, account, kind) DO UPDATE SET fingerprint = excluded.fingerprint, observed_hash = excluded.observed_hash, pending = excluded.pending,
                checked_at = excluded.checked_at, updated_at = excluded.updated_at",
            params![server_id, account, kind, fingerprint, observed.as_slice(), pending, checked_at as i64, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The Owner acknowledged `acked` (the hash of the realm set it was sent, or of the set the realm holds after the canonical
    /// ids were applied): the pending message is dropped and the canonical revision remembered.
    #[allow(clippy::too_many_arguments)]
    pub fn host_collection_acknowledge(&mut self, server_id: &str, account: u32, kind: &str, fingerprint: Option<&str>, observed: Option<&[u8; 32]>, acked: &[u8; 32], canonical_revision: u64, canonical_hash: Option<&[u8; 32]>) -> Result<()> {
        let tx = self.write_tx()?;
        let at = now();
        tx.execute(
            "INSERT INTO host_collection(server_id, account, kind, fingerprint, observed_hash, acked_hash, canonical_revision, canonical_hash, pending, updated_at)
             VALUES (?1, ?2, ?3, COALESCE(?4, ''), ?5, ?6, ?7, ?8, NULL, ?9)
             ON CONFLICT(server_id, account, kind) DO UPDATE SET fingerprint = COALESCE(?4, fingerprint), observed_hash = COALESCE(?5, observed_hash), acked_hash = excluded.acked_hash,
                canonical_revision = excluded.canonical_revision, canonical_hash = COALESCE(excluded.canonical_hash, canonical_hash), pending = NULL, updated_at = excluded.updated_at",
            params![server_id, account, kind, fingerprint, observed.map(|h| h.as_slice()), acked.as_slice(), canonical_revision as i64, canonical_hash.map(|h| h.as_slice()), at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Drop a pending message the Owner will never accept.
    pub fn host_collection_drop_pending(&mut self, server_id: &str, account: u32, kind: &str) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute("UPDATE host_collection SET pending = NULL, updated_at = ?4 WHERE server_id = ?1 AND account = ?2 AND kind = ?3", params![server_id, account, kind, now()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn host_collection_outbox(&self, server_id: &str) -> Result<Vec<CollectionOutboxMessage>> {
        let mut stmt = self.conn.prepare("SELECT account, kind, pending FROM host_collection WHERE server_id = ?1 AND pending IS NOT NULL ORDER BY account, kind")?;
        let rows = stmt.query_map([server_id], |r| Ok(CollectionOutboxMessage { account: r.get::<_, i64>(0)? as u32, kind: r.get(1)?, bytes: r.get(2)? }))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
