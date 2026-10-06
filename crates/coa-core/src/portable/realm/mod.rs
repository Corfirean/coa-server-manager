//! Phase 2: reading portable characters out of a realm's characters database. **Read-only, offline characters only.**
//!
//! * Goes through the Manager's existing [`Db`] abstraction (the bundled `mysql` client, or the same client inside a
//!   Docker database container). No new connection type, no driver, no published port.
//! * Fixed SQL, one numeric input (the local guid); see [`script`].
//! * One consistent read-only snapshot per character.
//! * A character that is online, deleted, a bot, in an active challenge/game mode/Manastorm state, or that has data in
//!   a table this Manager does not know is **refused** (see [`Blocker`]), not exported half way.
//!
//! Writing into a realm is Phase 3 and will use prepared statements / the realm's own import path.

pub mod blockers;
pub mod export;
pub mod import;
pub mod online;
pub mod plan;
pub mod policy;
pub mod reconcile;
pub mod registry;
pub mod script;
pub mod sqlenc;
pub mod update;

use std::collections::HashMap;

use crate::db::Db;
use crate::realms::Mode;

use super::error::{PortableError, Result};
use super::ids::{CharacterId, PortableItemId, PortablePetId, ProfileId};
use super::model::Ruleset;
use super::store::{RealmRegistration, Store};

pub use blockers::Blocker;
pub use export::{build, session_row, ExportRequest, Exported};
pub use import::{import_character, import_character_in_session, preflight, recover_imports, resolve_import, ImportOptions, ImportOutcome, ImportProblem, PreflightReport, Resolution};
pub use reconcile::{begin_session, reconcile_session, resolve_import_update, update_realm_character, ReconcileOutcome, SessionStart, UpdateOutcome};
pub use script::SchemaProbe;

/// The ruleset a database belongs to is decided by which realm profile it is (`realms::Mode`), never by the caller.
pub fn ruleset_of(db: &Db) -> Ruleset {
    match db.realm() {
        Mode::Coa => Ruleset::Coa,
        Mode::Wildcard => Ruleset::Wildcard,
    }
}

fn realm_error(e: crate::Error) -> PortableError {
    let text = e.to_string();
    if text.contains("Unknown column") || text.contains("doesn't exist") || text.contains("Unknown table") {
        PortableError::SchemaMismatch(text)
    } else {
        PortableError::RealmRead(text)
    }
}

/// The structure of the realm's characters schema (table and per-character column names only).
pub fn probe(db: &Db) -> Result<SchemaProbe> {
    let schema = db.realm_schema(script::CHARACTERS_SCHEMA);
    let probe = script::parse_probe(&db.query(&script::probe_sql(schema)?).map_err(realm_error)?)?;
    let missing = probe.missing_required();
    if !missing.is_empty() {
        return Err(PortableError::SchemaMismatch(format!("these tables are missing: {}", missing.join(", "))));
    }
    Ok(probe)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmCharacter {
    pub local_guid: u32,
    pub name: String,
    pub race: u32,
    pub class: u32,
    pub level: u32,
    pub account: u32,
    pub username: Option<String>,
    /// Empty = can be made portable as far as can be told from the listing (unknown tables are checked at export).
    pub blockers: Vec<Blocker>,
}

impl RealmCharacter {
    pub fn eligible(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// "Inspect realm characters": every character of a non-bot account, with the reasons it cannot be exported yet.
pub fn inspect_characters(db: &Db) -> Result<Vec<RealmCharacter>> {
    let probe = probe(db)?;
    let output = db.query(&script::list_sql(&probe)).map_err(realm_error)?;
    parse_listing(&output)
}

pub fn parse_listing(output: &str) -> Result<Vec<RealmCharacter>> {
    let mut out = Vec::new();
    for line in output.lines().filter(|l| !l.is_empty()) {
        let cells: Vec<&str> = line.split('\t').collect();
        if cells.len() != script::LIST_COLUMNS.len() {
            return Err(PortableError::CorruptSnapshot(format!("character list row has {} columns", cells.len())));
        }
        let num = |i: usize| cells[i].parse::<u64>().map_err(|_| PortableError::CorruptSnapshot(format!("character list: {:?} is not a number", cells[i])));
        let text = |i: usize| -> Result<Option<String>> {
            match cells[i] {
                "NULL" | "-" => Ok(None),
                c => {
                    let hex = c.strip_prefix('x').ok_or_else(|| PortableError::CorruptSnapshot("character list: not a hex value".into()))?;
                    let bytes = hex::decode(hex).map_err(|_| PortableError::CorruptSnapshot("character list: invalid hex".into()))?;
                    Ok(Some(String::from_utf8(bytes).map_err(|_| PortableError::CorruptSnapshot("character list: not UTF-8".into()))?))
                }
            }
        };
        let username = text(7)?;
        let mut blockers = Vec::new();
        if num(5)? != 0 {
            blockers.push(Blocker::Online);
        }
        if num(8)? != 0 {
            blockers.push(Blocker::Deleted);
        }
        if username.as_deref().is_some_and(blockers::is_internal_account) {
            blockers.push(Blocker::BotAccount);
        }
        for (i, b) in [(9, Blocker::ActiveChallenge), (10, Blocker::ActiveGameMode), (11, Blocker::ActiveCustomTrial), (12, Blocker::PendingManastormCaches)] {
            if num(i)? != 0 {
                blockers.push(b);
            }
        }
        out.push(RealmCharacter {
            local_guid: u32::try_from(num(0)?).map_err(|_| PortableError::CorruptSnapshot("guid out of range".into()))?,
            name: text(1)?.unwrap_or_default(),
            race: num(2)? as u32,
            class: num(3)? as u32,
            level: num(4)? as u32,
            account: num(6)? as u32,
            username,
            blockers,
        });
    }
    Ok(out)
}

/// Read one character in one consistent snapshot. Returns the raw answer and the statements that produced it.
pub fn read_raw(db: &Db, local_guid: u32, probe: &SchemaProbe) -> Result<script::RawExport> {
    let queries = script::queries(local_guid, probe)?;
    let output = db.query(&script::snapshot_script(&queries)).map_err(realm_error)?;
    script::parse_output(&output, &queries)
}

/// "Export snapshot": the portable model of one offline character, plus the item observations that go with it.
/// `character_id` and `prior_items` are `None`/empty for a character that is not portable yet.
pub fn export_character(db: &Db, local_guid: u32, character_id: Option<CharacterId>, prior_items: &HashMap<u32, (PortableItemId, String)>) -> Result<Exported> {
    export_character_with_pets(db, local_guid, character_id, prior_items, &HashMap::new())
}

/// The same, for a character that already has pet mappings on this realm: its pets keep their portable ids.
pub fn export_character_with_pets(db: &Db, local_guid: u32, character_id: Option<CharacterId>, prior_items: &HashMap<u32, (PortableItemId, String)>, prior_pets: &HashMap<u32, (PortablePetId, String)>) -> Result<Exported> {
    let probe = probe(db)?;
    let raw = read_raw(db, local_guid, &probe)?;
    export::build(&raw, &ExportRequest { ruleset: ruleset_of(db), local_guid, character_id, prior_items, prior_pets, allow_online_session: false })
}

/// Read a character that may be online, because the core holds a portable session row for it: the character as it was last saved,
/// and the marker of the same consistent snapshot.
pub fn read_session_character(db: &Db, local_guid: u32, character_id: CharacterId, prior_items: &HashMap<u32, (PortableItemId, String)>, prior_pets: &HashMap<u32, (PortablePetId, String)>) -> Result<Exported> {
    let probe = probe(db)?;
    let raw = read_raw(db, local_guid, &probe)?;
    export::build(&raw, &ExportRequest { ruleset: ruleset_of(db), local_guid, character_id: Some(character_id), prior_items, prior_pets, allow_online_session: true })
}

#[derive(Debug)]
pub struct MadePortable {
    pub character_id: CharacterId,
    pub revision: u64,
    pub warnings: Vec<String>,
}

/// "Make Portable": export an offline character and register it in the local store (revision 1, bound to this realm,
/// items mapped) in one transaction. Writes only to the local store, never to the realm.
pub fn make_portable(db: &Db, store: &mut Store, profile: ProfileId, server_id: &str, local_guid: u32) -> Result<MadePortable> {
    if let Some(existing) = store.find_by_local(server_id, local_guid)? {
        return Err(PortableError::AlreadyPortable { server_id: server_id.to_string(), local_guid, character: existing });
    }
    let exported = export_character(db, local_guid, None, &HashMap::new())?;
    register(store, profile, server_id, exported)
}

/// The store half of [`make_portable`], separated so it can be tested with recorded realm answers.
pub fn register(store: &mut Store, profile: ProfileId, server_id: &str, exported: Exported) -> Result<MadePortable> {
    let character_id = store.register_realm_character(RealmRegistration {
        profile,
        model: exported.model,
        source_server_id: server_id,
        server_id,
        local_guid: exported.local_guid,
        observations: &exported.observations,
        pets: &exported.pet_observations,
    })?;
    Ok(MadePortable { character_id, revision: 1, warnings: exported.warnings })
}

#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod live;
#[cfg(test)]
pub(crate) mod live_import;
#[cfg(test)]
mod live_roundtrip;
#[cfg(test)]
mod live_update;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod update_tests;
