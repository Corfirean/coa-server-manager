//! The OWNER side of a runtime portable session: offer a character to a realm, accept the realm's baseline and apply its
//! checkpoints to the canonical character. Nothing here reads a realm, and nothing here knows how the messages travel.
//!
//! Every checkpoint is `merge3(C0, B0, current B1)` with the session's own `C0` and `B0` as anchors, so a checkpoint that
//! is repeated, re-sent or superseded by a later one can never count progress twice.

use rusqlite::{params, OptionalExtension};

use super::*;
use crate::portable::ids::SessionId;
use crate::portable::merge::{merge3, Mode};
use crate::portable::session::protocol::*;

struct OwnerRow {
    session_id: SessionId,
    character_id: CharacterId,
    server_id: String,
    state: String,
    c0_revision: u64,
    c0_hash: [u8; 32],
    c0_payload: Vec<u8>,
    generation: Option<u32>,
    b0_hash: Option<[u8; 32]>,
    b0_payload: Option<Vec<u8>>,
    head_revision: u64,
    last_sequence: u64,
}

fn read_owner_row(conn: &Connection, session: SessionId) -> Result<Option<OwnerRow>> {
    type Raw = (String, String, String, i64, Vec<u8>, Vec<u8>, Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>, i64, i64);
    let raw: Option<Raw> = conn
        .query_row(
            "SELECT character_id, server_id, state, c0_revision, c0_hash, c0_payload, baseline_generation, b0_hash, b0_payload, head_revision, last_sequence FROM owner_session WHERE session_id = ?1",
            [session.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
        )
        .optional()?;
    let Some((character, server_id, state, c0_revision, c0_hash, c0_payload, generation, b0_hash, b0_payload, head, last)) = raw else { return Ok(None) };
    Ok(Some(OwnerRow {
        session_id: session,
        character_id: character.parse()?,
        server_id,
        state,
        c0_revision: c0_revision as u64,
        c0_hash: hash_from_blob(c0_hash)?,
        c0_payload,
        generation: generation.map(|g| g as u32),
        b0_hash: b0_hash.map(hash_from_blob).transpose()?,
        b0_payload,
        head_revision: head as u64,
        last_sequence: last as u64,
    }))
}

fn head_hash(conn: &Connection, id: CharacterId, revision: u64) -> Result<[u8; 32]> {
    let blob: Vec<u8> = conn.query_row("SELECT content_hash FROM snapshot WHERE character_id = ?1 AND revision = ?2", params![id.to_string(), revision as i64], |r| r.get(0)).map_err(|_| PortableError::UnknownRevision { character: id, revision })?;
    hash_from_blob(blob)
}

fn ack(session: SessionId, sequence: u64, outcome: AckOutcome, revision: u64, hash: [u8; 32]) -> OwnerAck {
    OwnerAck { protocol_version: PROTOCOL_VERSION, session_id: session, sequence, outcome, canonical_revision: revision, canonical_hash: hex::encode(hash), owned_items: vec![], owned_pets: vec![], canonical: None, next_session: None }
}

fn owned_ids(model: &PortableCharacter) -> (Vec<PortableItemId>, Vec<crate::portable::ids::PortablePetId>) {
    let items: Vec<_> = model.items.iter().map(|i| i.id).take(MAX_ACK_IDS).collect();
    let pets: Vec<_> = model.pets.iter().map(|p| p.id).take(MAX_ACK_IDS).collect();
    (items, pets)
}

fn offer_from_row(row: &OwnerRow) -> SessionOffer {
    SessionOffer {
        protocol_version: PROTOCOL_VERSION,
        session_id: row.session_id,
        character_id: row.character_id,
        server_id: row.server_id.clone(),
        canonical_revision: row.c0_revision,
        snapshot: Envelope::from_encoded(&EncodedSnapshot { content_hash: row.c0_hash, uncompressed_size: 0, payload: row.c0_payload.clone() }),
    }
}

fn open_offer(tx: &Transaction<'_>, id: CharacterId, server_id: &str) -> Result<SessionOffer> {
    let record = read_character(tx, id)?;
    let (_, enc) = read_snapshot_row(tx, id, record.revision)?;
    let session = SessionId::new();
    let at = now();
    tx.execute(
        "INSERT INTO owner_session(session_id, character_id, server_id, state, c0_revision, c0_hash, c0_payload, head_revision, last_sequence, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'offered', ?4, ?5, ?6, ?4, 0, ?7, ?7)",
        params![session.to_string(), id.to_string(), server_id, record.revision as i64, enc.content_hash.as_slice(), enc.payload, at],
    )?;
    Ok(offer_from_row(&read_owner_row(tx, session)?.expect("just inserted")))
}

impl Store {
    /// Offer the character's current canonical revision to a realm and open the session that will follow it. An offer that was
    /// made and not yet started is repeated as it was; one made against an older revision is superseded by a fresh one.
    pub fn owner_offer_session(&mut self, id: CharacterId, server_id: &str) -> Result<SessionOffer> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let record = read_character(&tx, id)?;
        let live: Option<String> = tx
            .query_row("SELECT session_id FROM owner_session WHERE character_id = ?1 AND server_id = ?2 AND state IN ('offered', 'open')", params![id.to_string(), server_id], |r| r.get(0))
            .optional()?;
        if let Some(live) = live {
            let row = read_owner_row(&tx, live.parse()?)?.expect("it was just found");
            if row.state == "open" {
                return Err(PortableError::SessionOpen);
            }
            if row.c0_revision == record.revision {
                return Ok(offer_from_row(&row));
            }
            tx.execute("UPDATE owner_session SET state = 'superseded', updated_at = ?2 WHERE session_id = ?1", params![live, now()])?;
        }
        let offer = open_offer(&tx, id, server_id)?;
        tx.commit()?;
        Ok(offer)
    }

    /// The Host captured `B0`. Idempotent for the same baseline.
    pub fn owner_session_started(&mut self, msg: &PortableSessionStarted) -> Result<OwnerAck> {
        let tx = self.write_tx()?;
        let Some(row) = read_owner_row(&tx, msg.session_id)? else {
            return Ok(ack(msg.session_id, 0, AckOutcome::Rejected("unknown session".into()), 0, [0; 32]));
        };
        let current_hash = head_hash(&tx, row.character_id, row.head_revision)?;
        let reject = |why: &str| ack(msg.session_id, 0, AckOutcome::Rejected(why.into()), row.head_revision, current_hash);
        if msg.character_id != row.character_id || msg.server_id != row.server_id {
            return Ok(reject("the message does not belong to this session"));
        }
        if msg.base_canonical_revision != row.c0_revision {
            return Ok(reject("the baseline was taken against another canonical revision"));
        }
        let b0_hash = match msg.b0.hash() {
            Ok(h) if hex::encode(h) == msg.content_hash => h,
            _ => return Ok(reject("the content hash does not match the baseline")),
        };
        match row.state.as_str() {
            "open" => {
                return Ok(if row.b0_hash == Some(b0_hash) && row.generation == Some(msg.baseline_generation) {
                    ack(msg.session_id, 0, AckOutcome::Duplicate, row.head_revision, current_hash)
                } else {
                    reject("this session already has a different baseline")
                })
            }
            "offered" => {}
            _ => return Ok(ack(msg.session_id, 0, AckOutcome::StaleSession, row.head_revision, current_hash)),
        }
        let b0 = match msg.b0.open() {
            Ok(m) => m,
            Err(e) => return Ok(reject(&format!("the baseline cannot be read: {e}"))),
        };
        let c0 = snapshot::decode(&row.c0_payload, Some(&row.c0_hash))?;
        if b0.character_id != row.character_id || b0.ruleset != c0.ruleset || b0.content_namespace != c0.content_namespace {
            return Ok(reject("the baseline is another character or ruleset"));
        }
        tx.execute(
            "UPDATE owner_session SET state = 'open', baseline_generation = ?2, b0_hash = ?3, b0_payload = ?4, updated_at = ?5 WHERE session_id = ?1",
            params![msg.session_id.to_string(), msg.baseline_generation, b0_hash.as_slice(), msg.b0.bytes()?, now()],
        )?;
        tx.commit()?;
        Ok(ack(msg.session_id, 0, AckOutcome::Applied, row.head_revision, current_hash))
    }

    /// Apply one checkpoint of the realm: `merge3(C0, B0, B1)`.
    pub fn owner_checkpoint(&mut self, msg: &PortableCheckpoint) -> Result<OwnerAck> {
        let keep = self.history_keep()?;
        let tx = self.write_tx()?;
        let Some(row) = read_owner_row(&tx, msg.session_id)? else {
            return Ok(ack(msg.session_id, msg.sequence, AckOutcome::Rejected("unknown session".into()), 0, [0; 32]));
        };
        let record = read_character(&tx, row.character_id)?;
        let seen_hash = head_hash(&tx, row.character_id, record.revision)?;
        let outcome = |o: AckOutcome| ack(msg.session_id, msg.sequence, o, record.revision, seen_hash);
        let reject = |why: &str| outcome(AckOutcome::Rejected(why.into()));

        if msg.character_id != row.character_id || msg.server_id != row.server_id {
            return Ok(reject("the message does not belong to this session"));
        }
        if msg.base_canonical_revision != row.c0_revision {
            return Ok(reject("the checkpoint was taken against another canonical revision"));
        }
        let b1_hash = match msg.realm_snapshot.hash() {
            Ok(h) if hex::encode(h) == msg.content_hash => h,
            _ => return Ok(reject("the content hash does not match the snapshot")),
        };
        if msg.sequence == 0 {
            return Ok(reject("sequences start at 1"));
        }

        // a sequence the Owner has applied before is answered from its log, whatever the session's state is now
        let logged: Option<(Vec<u8>, i64, i64, Option<String>)> = tx
            .query_row("SELECT content_hash, resulting_revision, final, next_session_id FROM owner_checkpoint WHERE session_id = ?1 AND sequence = ?2", params![msg.session_id.to_string(), msg.sequence as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?;
        if let Some((hash, revision, was_final, next)) = logged {
            if hash.as_slice() != b1_hash.as_slice() {
                return Ok(reject("this sequence was applied with different content"));
            }
            let revision = revision as u64;
            let mut a = ack(msg.session_id, msg.sequence, AckOutcome::Duplicate, revision, head_hash(&tx, row.character_id, revision)?);
            if let Ok((model, _)) = read_snapshot_row(&tx, row.character_id, revision) {
                (a.owned_items, a.owned_pets) = owned_ids(&model);
                if was_final == 1 {
                    a.canonical = Some(Envelope::seal(&model)?);
                    if let Some(next) = next {
                        a.next_session = Some(NextSession { session_id: next.parse()?, canonical_revision: revision });
                    }
                }
            }
            return Ok(a);
        }
        match row.state.as_str() {
            "open" => {}
            "offered" => return Ok(reject("the session has not started")),
            _ => return Ok(outcome(AckOutcome::StaleSession)),
        }
        if msg.sequence < row.last_sequence {
            return Ok(outcome(AckOutcome::StaleSequence));
        }
        if record.revision != row.head_revision {
            tx.execute("UPDATE owner_session SET state = 'superseded', updated_at = ?2 WHERE session_id = ?1", params![msg.session_id.to_string(), now()])?;
            tx.commit()?;
            return Ok(ack(msg.session_id, msg.sequence, AckOutcome::StaleSession, record.revision, head_hash(&self.conn, row.character_id, record.revision)?));
        }

        let b1 = match msg.realm_snapshot.open() {
            Ok(m) => m,
            Err(e) => return Ok(reject(&format!("the snapshot cannot be read: {e}"))),
        };
        let (c0, b0) = (snapshot::decode(&row.c0_payload, Some(&row.c0_hash))?, snapshot::decode(row.b0_payload.as_deref().expect("an open session has B0"), row.b0_hash.as_ref())?);
        if b1.character_id != row.character_id || b1.ruleset != c0.ruleset || b1.content_namespace != c0.content_namespace {
            return Ok(reject("the snapshot is another character or ruleset"));
        }
        let merged = match merge3(&c0, &b0, &b1, Mode::Lenient) {
            Ok(m) => m.model,
            Err(e) => return Ok(reject(&format!("the checkpoint cannot be merged: {e}"))),
        };
        let encoded = match snapshot::encode(&merged) {
            Ok(e) => e,
            Err(e) => return Ok(reject(&format!("the merged character is not valid: {e}"))),
        };

        let at = now();
        let head = head_hash(&tx, row.character_id, record.revision)?;
        let revision = if head == encoded.content_hash {
            record.revision
        } else {
            let revision = record.revision + 1;
            insert_snapshot(&tx, row.character_id, revision, &encoded, &row.server_id, Some(&format!("session {} checkpoint {}", msg.session_id, msg.sequence)), &at)?;
            update_character(&tx, row.character_id, &merged, revision, &at)?;
            prune_tx(&tx, row.character_id, revision, keep)?;
            revision
        };

        let mut a = ack(msg.session_id, msg.sequence, AckOutcome::Applied, revision, encoded.content_hash);
        (a.owned_items, a.owned_pets) = owned_ids(&merged);
        let mut next_id = None;
        if msg.final_checkpoint {
            tx.execute("UPDATE owner_session SET state = 'closed', last_sequence = ?2, head_revision = ?3, updated_at = ?4 WHERE session_id = ?1", params![msg.session_id.to_string(), msg.sequence as i64, revision as i64, at])?;
            let offer = open_offer(&tx, row.character_id, &row.server_id)?;
            a.canonical = Some(Envelope::from_encoded(&encoded));
            a.next_session = Some(NextSession { session_id: offer.session_id, canonical_revision: revision });
            next_id = Some(offer.session_id.to_string());
        } else {
            tx.execute("UPDATE owner_session SET last_sequence = ?2, head_revision = ?3, updated_at = ?4 WHERE session_id = ?1", params![msg.session_id.to_string(), msg.sequence as i64, revision as i64, at])?;
        }
        tx.execute(
            "INSERT INTO owner_checkpoint(session_id, sequence, content_hash, resulting_revision, final, next_session_id, applied_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![msg.session_id.to_string(), msg.sequence as i64, b1_hash.as_slice(), revision as i64, msg.final_checkpoint as i64, next_id, at],
        )?;
        tx.commit()?;
        Ok(a)
    }

    /// What the Owner knows of one session: (state, last applied sequence, head revision).
    pub fn owner_session_info(&self, session: SessionId) -> Result<Option<(String, u64, u64)>> {
        Ok(read_owner_row(&self.conn, session)?.map(|r| (r.state, r.last_sequence, r.head_revision)))
    }
}
