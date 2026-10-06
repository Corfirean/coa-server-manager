//! The real realm behind `RealmBridge`: the Manager's `Db` for reads and for arming, RA for the typed checkpoint and release.

use std::collections::HashMap;

use crate::db::Db;
use crate::ra::Ra;

use super::super::error::{PortableError, Result};
use super::super::ids::{CharacterId, PortableItemId, PortablePetId, SessionId};
use super::super::collection::IdSet;
use super::super::realm::collections::{self, Applied};
use super::super::realm::knowledge::RealmKnowledge;
use super::super::realm::{probe, read_session_character};
use super::bridge::*;

pub struct LiveBridge<'a> {
    db: &'a Db,
    ra: Option<Ra>,
    knowledge: Option<std::sync::Arc<RealmKnowledge>>,
}

impl<'a> LiveBridge<'a> {
    pub fn new(db: &'a Db, ra: Ra) -> Self {
        Self { db, ra: Some(ra), knowledge: None }
    }

    pub fn without_console(db: &'a Db) -> Self {
        Self { db, ra: None, knowledge: None }
    }

    /// What the realm's client data knows: without it no account collection is ever written to the realm.
    pub fn with_knowledge(mut self, knowledge: Option<std::sync::Arc<RealmKnowledge>>) -> Self {
        self.knowledge = knowledge;
        self
    }

    fn console(&mut self) -> Result<&mut Ra> {
        self.ra.as_mut().ok_or_else(|| PortableError::RealmRead("the realm's console is not connected".into()))
    }
}

fn realm_error(e: crate::Error) -> PortableError {
    PortableError::RealmRead(e.to_string())
}

/// The core's session row of one character, read straight from the realm's database.
pub fn read_session_row(db: &Db, local_guid: u32) -> Result<Option<SessionRow>> {
    let sql = format!(
        "SELECT guid, CONCAT('x', HEX(session_id)), CONCAT('x', HEX(character_id)), imported_revision, baseline_generation, state, checkpoint_seq, save_seq FROM acore_characters.coa_portable_session WHERE guid = {local_guid}"
    );
    let out = db.query(&sql).map_err(realm_error)?;
    let Some(line) = out.lines().find(|l| !l.trim().is_empty()) else { return Ok(None) };
    let cells: Vec<&str> = line.trim_end().split('\t').collect();
    if cells.len() != 8 {
        return Err(PortableError::CorruptSnapshot("the portable session row has an unexpected shape".into()));
    }
    let text = |cell: &str| -> Result<String> {
        let hex = cell.strip_prefix('x').ok_or_else(|| PortableError::CorruptSnapshot("a session cell is not hex".into()))?;
        String::from_utf8(hex::decode(hex).map_err(|_| PortableError::CorruptSnapshot("a session cell is not hex".into()))?).map_err(|_| PortableError::CorruptSnapshot("a session cell is not UTF-8".into()))
    };
    let num = |i: usize| -> Result<u64> { cells[i].trim().parse().map_err(|_| PortableError::CorruptSnapshot(format!("{:?} is not a number", cells[i]))) };
    Ok(Some(SessionRow {
        guid: num(0)? as u32,
        session_id: text(cells[1])?.parse()?,
        character_id: text(cells[2])?.parse()?,
        imported_revision: num(3)?,
        generation: num(4)? as u32,
        state: RowState::from_code(num(5)?).ok_or_else(|| PortableError::CorruptSnapshot("unknown session state".into()))?,
        checkpoint_seq: num(6)?,
        save_seq: num(7)?,
    }))
}

impl RealmBridge for LiveBridge<'_> {
    fn session_row(&mut self, local_guid: u32) -> Result<Option<SessionRow>> {
        read_session_row(self.db, local_guid)
    }

    fn read(&mut self, local_guid: u32, prior_items: &HashMap<u32, (PortableItemId, String)>, prior_pets: &HashMap<u32, (PortablePetId, String)>) -> Result<RealmRead> {
        let row = self.session_row(local_guid)?;
        let character = row.as_ref().map(|r| r.character_id).ok_or_else(|| PortableError::Invalid("the character has no portable session row".into()))?;
        let _ = probe(self.db)?;
        let exported = read_session_character(self.db, local_guid, character, prior_items, prior_pets)?;
        let session = exported.session.clone();
        Ok(RealmRead { exported, session, online: true })
    }

    fn request_checkpoint(&mut self, local_guid: u32, session: SessionId, sequence: u64) -> Result<CheckpointReply> {
        self.console()?.portable_checkpoint(local_guid, session, sequence).map_err(|e| PortableError::RealmRead(e.to_string()))
    }

    fn release(&mut self, session: SessionId) -> Result<()> {
        self.console()?.portable_release(session).map_err(|e| PortableError::RealmRead(e.to_string()))
    }

    fn arm(&mut self, local_guid: u32, session: SessionId, character: CharacterId, revision: u64, generation: u32) -> Result<()> {
        let sql = format!(
            "START TRANSACTION;
DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE guid = {local_guid} AND online = 0) = 1, 1, (SELECT 1 UNION ALL SELECT 2));
             INSERT INTO acore_characters.coa_portable_session (guid, session_id, character_id, imported_revision, baseline_generation, state, checkpoint_seq, save_seq, updated_at)
             VALUES ({local_guid}, '{session}', '{character}', {revision}, {generation}, 0, 0, 0, UNIX_TIMESTAMP())
             ON DUPLICATE KEY UPDATE session_id = VALUES(session_id), character_id = VALUES(character_id), imported_revision = VALUES(imported_revision), baseline_generation = VALUES(baseline_generation), state = 0, checkpoint_seq = 0, updated_at = VALUES(updated_at);
COMMIT;"
        );
        self.db.query(&sql).map_err(realm_error)?;
        Ok(())
    }

    fn account_of(&mut self, local_guid: u32) -> Result<Option<u32>> {
        collections::account_of(self.db, local_guid)
    }

    fn collection_fingerprint(&mut self, account: u32, kind: &str) -> Result<String> {
        collections::fingerprint(self.db, account, kind)
    }

    fn read_collection(&mut self, account: u32, kind: &str) -> Result<IdSet> {
        collections::read_set(self.db, account, kind)
    }

    fn apply_collection(&mut self, account: u32, kind: &str, canonical: &IdSet) -> Result<Applied> {
        let knowledge = self.knowledge.clone().ok_or_else(|| PortableError::Invalid("the realm's client data is not known: no collection can be written".into()))?;
        collections::apply_set(self.db, account, kind, canonical, &knowledge)
    }
}
