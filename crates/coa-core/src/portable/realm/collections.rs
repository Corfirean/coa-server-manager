//! Phase 6: the account collections of a realm (`account_appearance_collection`, `account_vanity_collection`).
//!
//! * **Read**: a cheap fingerprint of the realm rows (count, maximum, sum, checksum) says whether anything changed; the ids are
//!   read only when it did. Nothing here ever writes while reading.
//! * **Write**: `INSERT IGNORE` only, exactly like the core itself, so it is safe on a running realm (the core loads an
//!   account's collection once at login and never deletes a row). Only ids the destination's client data knows are written;
//!   the rest stay canonical. `source_item` is bookkeeping the core never reads back, so it is written as 0.
//! * The four bank vanity items open the personal Ascension bank, which is out of scope: they are neither read nor written.

use crate::db::Db;

use super::super::collection::IdSet;
use super::super::error::{PortableError, Result};
use super::knowledge::{is_bank_vanity_item, RealmKnowledge};
use super::plan::FAIL;
use super::ruleset_of;
use super::super::model::Ruleset;

/// How many rows one INSERT carries.
const CHUNK: usize = 2_000;

/// `(table, id column, columns written)` of a collection kind.
fn layout(kind: &str) -> Result<(&'static str, &'static str, bool)> {
    match kind {
        "coa:appearance" => Ok(("account_appearance_collection", "appearance_id", true)),
        "coa:vanity" => Ok(("account_vanity_collection", "item_id", false)),
        other => Err(PortableError::Invalid(format!("collection kind {other:?} is not carried"))),
    }
}

fn realm_error(e: crate::Error) -> PortableError {
    PortableError::RealmRead(e.to_string())
}

fn coa_only(db: &Db) -> Result<()> {
    if ruleset_of(db) != Ruleset::Coa {
        return Err(PortableError::Invalid("collections are carried for CoA realms only (Wildcard transfer is disabled)".into()));
    }
    Ok(())
}

/// The game account that owns a character.
pub fn account_of(db: &Db, local_guid: u32) -> Result<Option<u32>> {
    let out = db.query(&format!("SELECT account FROM acore_characters.characters WHERE guid = {local_guid}")).map_err(realm_error)?;
    out.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().parse::<u32>().map_err(|_| PortableError::CorruptSnapshot(format!("{l:?} is not an account id")))).transpose()
}

/// Count, maximum, sum and checksum of the account's rows of this kind: one cheap aggregate over the primary key.
pub fn fingerprint(db: &Db, account: u32, kind: &str) -> Result<String> {
    coa_only(db)?;
    let (table, column, _) = layout(kind)?;
    let out = db
        .query(&format!("SELECT COUNT(*), IFNULL(MAX(`{column}`), 0), IFNULL(SUM(`{column}`), 0), IFNULL(BIT_XOR(CRC32(`{column}`)), 0) FROM acore_characters.`{table}` WHERE `account_id` = {account}"))
        .map_err(realm_error)?;
    let cells: Vec<&str> = out.trim().split('\t').collect();
    if cells.len() != 4 || cells.iter().any(|c| c.is_empty() || !c.bytes().all(|b| b.is_ascii_digit())) {
        return Err(PortableError::CorruptSnapshot(format!("unexpected collection fingerprint {out:?}")));
    }
    Ok(cells.join(":"))
}

/// The ids of the account in this realm, without the bank vanity items.
pub fn read_set(db: &Db, account: u32, kind: &str) -> Result<IdSet> {
    coa_only(db)?;
    let (table, column, _) = layout(kind)?;
    let out = db.query(&format!("SELECT `{column}` FROM acore_characters.`{table}` WHERE `account_id` = {account} ORDER BY `{column}`")).map_err(realm_error)?;
    let mut ids = Vec::new();
    for line in out.lines().filter(|l| !l.trim().is_empty()) {
        let id: u32 = line.trim().parse().map_err(|_| PortableError::CorruptSnapshot(format!("{line:?} is not an id")))?;
        if kind == "coa:vanity" && is_bank_vanity_item(id) {
            continue;
        }
        ids.push(id);
    }
    IdSet::from_ids(ids)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Ids written to the realm now.
    pub inserted: usize,
    /// Canonical ids the realm's client data does not know: kept canonical, not written.
    pub unknown: usize,
    /// Canonical ids the realm already had.
    pub already: usize,
}

/// Union the canonical set into the account's rows: only ids the realm knows and does not hold yet are inserted.
pub fn apply_set(db: &Db, account: u32, kind: &str, canonical: &IdSet, knowledge: &RealmKnowledge) -> Result<Applied> {
    coa_only(db)?;
    let (table, column, has_source) = layout(kind)?;
    let known = |id: u32| match kind {
        "coa:appearance" => knowledge.knows_appearance(id),
        _ => knowledge.knows_vanity(id) && !is_bank_vanity_item(id),
    };
    let held = read_set(db, account, kind)?;
    let mut applied = Applied::default();
    let mut wanted = Vec::new();
    for id in canonical.ids().iter().copied().filter(|id| !(kind == "coa:vanity" && is_bank_vanity_item(*id))) {
        if !known(id) {
            applied.unknown += 1;
        } else if held.contains(id) {
            applied.already += 1;
        } else {
            wanted.push(id);
        }
    }
    if wanted.is_empty() {
        return Ok(applied);
    }
    let mut script = String::new();
    script.push_str("START TRANSACTION;\n");
    script.push_str(&format!("DO IF((SELECT COUNT(*) FROM acore_auth.account WHERE id = {account}) = 1, 1, {FAIL});\n"));
    for chunk in wanted.chunks(CHUNK) {
        let (columns, rows) = if has_source {
            ("`account_id`, `appearance_id`, `source_item`", chunk.iter().map(|id| format!("({account}, {id}, 0)")).collect::<Vec<_>>().join(","))
        } else {
            ("`account_id`, `item_id`", chunk.iter().map(|id| format!("({account}, {id})")).collect::<Vec<_>>().join(","))
        };
        script.push_str(&format!("INSERT IGNORE INTO acore_characters.`{table}` ({columns}) VALUES {rows};\n"));
    }
    script.push_str("COMMIT;\n");
    let _ = column;
    db.query(&script).map_err(realm_error)?;
    applied.inserted = wanted.len();
    Ok(applied)
}
