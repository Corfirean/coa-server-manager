//! What a destination realm **knows** (Phase 6): the appearance ids and vanity items of its client data.
//!
//! The appearance and vanity systems of CoA are driven by client tables the core loads at startup, not by database rows:
//! `Appearances.dbc` (field 0 is the appearance id) and `VanityCollection.dbc` (field 1 is the vanity item entry), both in
//! `<DataDir>/dbc`. An id outside those tables cannot be shown or selected by the realm, so it is never **written** to that
//! realm; it stays in the canonical character and the profile collection, and is written when the character or the
//! account reaches a realm that does know it.
//!
//! Woodworking appearances are synthesised by the core at startup and are not in the table: this Manager treats them as
//! unknown to a destination (they are kept, never applied), which is the documented limitation of Phase 6.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::super::capabilities::{CatalogEntry, ClientCatalog, CATALOG_TABLES};
use super::super::collection::IdSet;
use super::super::error::{PortableError, Result};

/// The four vanity items that open the personal Ascension bank. The bank is out of scope, so they are not part of the
/// portable vanity collection (`AscensionCollectionService::BankVanityItems`).
pub const BANK_VANITY_ITEMS: [u32; 4] = [110_000, 134_985, 509_892, 1_180_097];

pub fn is_bank_vanity_item(item: u32) -> bool {
    BANK_VANITY_ITEMS.contains(&item)
}

const HEADER: usize = 20;
/// Largest DBC read (the tables are a few MB).
const MAX_DBC_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Default)]
pub struct RealmKnowledge {
    appearances: IdSet,
    vanity: IdSet,
    /// The SHA-256 and record count of the client tables this knowledge was read from (empty when it was built by hand): compared with
    /// the catalog of the realm's content profile, so that what the Manager reads is what the realm loaded.
    catalog: ClientCatalog,
}

impl std::fmt::Debug for RealmKnowledge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RealmKnowledge({} appearances, {} vanity items)", self.appearances.len(), self.vanity.len())
    }
}

impl RealmKnowledge {
    pub fn new(appearances: IdSet, vanity: IdSet) -> Self {
        Self { appearances, vanity, catalog: ClientCatalog::new() }
    }

    pub fn catalog(&self) -> &ClientCatalog {
        &self.catalog
    }

    #[cfg(test)]
    pub(crate) fn set_catalog_for_test(&mut self, table: &str, sha256: &str, records: u32) {
        self.catalog.insert(table.to_string(), CatalogEntry { sha256: sha256.to_string(), records });
    }

    /// Read `<data_dir>/dbc/Appearances.dbc` and `VanityCollection.dbc`.
    pub fn from_data_dir(data_dir: &Path) -> Result<Self> {
        let dbc = data_dir.join("dbc");
        let (appearances, appearances_entry) = read_table(&dbc.join("Appearances.dbc"), 0, 9)?;
        let (vanity, vanity_entry) = read_table(&dbc.join("VanityCollection.dbc"), 1, 77)?;
        let mut catalog = ClientCatalog::new();
        catalog.insert("Appearances.dbc".into(), appearances_entry);
        catalog.insert("VanityCollection.dbc".into(), vanity_entry);
        for name in CATALOG_TABLES {
            if !catalog.contains_key(name) {
                if let Some(entry) = table_entry(&dbc.join(name)) {
                    catalog.insert(name.to_string(), entry);
                }
            }
        }
        Ok(Self { appearances: IdSet::from_ids(appearances)?, vanity: IdSet::from_ids(vanity)?, catalog })
    }

    pub fn knows_appearance(&self, id: u32) -> bool {
        self.appearances.contains(id)
    }

    pub fn knows_vanity(&self, item: u32) -> bool {
        self.vanity.contains(item)
    }

    pub fn appearance_count(&self) -> usize {
        self.appearances.len()
    }

    pub fn vanity_count(&self) -> usize {
        self.vanity.len()
    }
}

/// The SHA-256 and record count of a client table, or `None` when the realm does not have it (or it is not a WDBC file).
pub fn table_entry(path: &Path) -> Option<CatalogEntry> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_DBC_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < HEADER || &bytes[..4] != b"WDBC" {
        return None;
    }
    Some(CatalogEntry { sha256: hex::encode(Sha256::digest(&bytes)), records: u32::from_le_bytes(bytes[4..8].try_into().ok()?) })
}

fn read_table(path: &Path, id_dword: usize, minimum_dwords: usize) -> Result<(Vec<u32>, CatalogEntry)> {
    let ids = read_ids(path, id_dword, minimum_dwords)?;
    let entry = table_entry(path).ok_or_else(|| PortableError::SchemaMismatch(format!("{}: is not a client table", path.display())))?;
    Ok((ids, entry))
}

/// The ids of one DWORD column of a WDBC file (`WDBC`, record count, field count, record size, string size, records, strings).
/// Zero ids are not records of anything and are skipped, as the core does.
fn read_ids(path: &Path, id_dword: usize, minimum_dwords: usize) -> Result<Vec<u32>> {
    let bad = |what: &str| PortableError::SchemaMismatch(format!("{}: {what}", path.display()));
    let size = std::fs::metadata(path).map_err(|e| bad(&format!("cannot be read ({e})")))?.len();
    if size > MAX_DBC_BYTES {
        return Err(bad("is unreasonably large"));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(&format!("cannot be read ({e})")))?;
    if bytes.len() < HEADER || &bytes[..4] != b"WDBC" {
        return Err(bad("is not a WDBC file"));
    }
    let field = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes")) as u64;
    let (records, record_size, strings) = (field(4), field(12), field(16));
    if HEADER as u64 + records * record_size + strings != bytes.len() as u64 {
        return Err(bad("header does not match the file size"));
    }
    if records > 0 && record_size < (minimum_dwords as u64) * 4 {
        return Err(bad("records are too small for this table"));
    }
    let record_size = record_size as usize;
    let mut ids = Vec::with_capacity(records as usize);
    for row in 0..records as usize {
        let at = HEADER + row * record_size + id_dword * 4;
        let id = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        if id != 0 {
            ids.push(id);
        }
    }
    Ok(ids)
}

#[cfg(test)]
pub(crate) fn write_test_dbc(path: &Path, ids: &[u32], id_dword: usize, dwords: usize) {
    let mut out = Vec::new();
    out.extend_from_slice(b"WDBC");
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    out.extend_from_slice(&(dwords as u32).to_le_bytes());
    out.extend_from_slice(&((dwords * 4) as u32).to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    for id in ids {
        for d in 0..dwords {
            out.extend_from_slice(&(if d == id_dword { *id } else { 7 }).to_le_bytes());
        }
    }
    out.push(0);
    std::fs::write(path, out).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_of_the_two_client_tables_are_read_and_nothing_else_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("dbc")).unwrap();
        write_test_dbc(&dir.path().join("dbc/Appearances.dbc"), &[5, 0, 9, 9, 100], 0, 9);
        write_test_dbc(&dir.path().join("dbc/VanityCollection.dbc"), &[1000, 2000], 1, 77);
        let k = RealmKnowledge::from_data_dir(dir.path()).unwrap();
        assert!(k.knows_appearance(5) && k.knows_appearance(9) && k.knows_appearance(100));
        assert!(!k.knows_appearance(0) && !k.knows_appearance(6));
        assert_eq!((k.appearance_count(), k.vanity_count()), (3, 2));
        assert!(k.knows_vanity(1000) && !k.knows_vanity(1));

        // a truncated file, a wrong magic and a missing file are refused, never half read
        let path = dir.path().join("dbc/Appearances.dbc");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 5);
        std::fs::write(&path, &bytes).unwrap();
        assert!(RealmKnowledge::from_data_dir(dir.path()).is_err());
        std::fs::write(&path, b"NOPE0000000000000000000").unwrap();
        assert!(RealmKnowledge::from_data_dir(dir.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(RealmKnowledge::from_data_dir(dir.path()).is_err());
    }

    #[test]
    fn the_bank_vanity_items_are_recognised() {
        assert!(is_bank_vanity_item(110_000) && is_bank_vanity_item(1_180_097));
        assert!(!is_bank_vanity_item(110_001));
    }
}
