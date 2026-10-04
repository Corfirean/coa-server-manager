//! Read-only database structure checks. A release can ship a contract captured from its clean database.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::{db::Db, error::Result, fsx};

pub const CONTRACT: &str = "Scripts/database-schema.json";
type Columns = BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>;

#[derive(Debug, Serialize, Deserialize)]
pub struct Contract {
    pub schema: u32,
    pub columns: Columns,
}

#[derive(Debug, Serialize)]
pub struct Problem { pub database: String, pub table: String, pub column: String, pub detail: String }

fn read_columns(db: &Db, kind: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let schema = db.realm_schema(crate::db::schema_of(kind)?);
    let text = db.query(&format!("SELECT TABLE_NAME,COLUMN_NAME,CONCAT(COLUMN_TYPE,'|',IS_NULLABLE,'|',IF(COLUMN_DEFAULT IS NULL,'<NULL>',HEX(COLUMN_DEFAULT)),'|',IF(EXTRA='','<NONE>',HEX(EXTRA))) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='{schema}' ORDER BY TABLE_NAME,ORDINAL_POSITION;"))?;
    let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() == 3 { tables.entry(fields[0].into()).or_default().insert(fields[1].into(), fields[2].into()); }
    }
    Ok(tables)
}

pub fn capture(db: &Db, root: &Path) -> Result<()> {
    let mut columns = Columns::new();
    for kind in ["auth", "characters", "world"] { columns.insert(kind.into(), read_columns(db, kind)?); }
    fsx::atomic_write_json(&root.join(CONTRACT), &Contract { schema: 1, columns })
}

pub fn check(db: &Db, root: &Path) -> Result<Vec<Problem>> {
    let contract = if root.join(CONTRACT).is_file() {
        let contract: Contract = fsx::read_json(&root.join(CONTRACT))?;
        if contract.schema != 1 { return Err(crate::Error::Invalid("Unsupported database schema contract.".into())); }
        contract
    } else {
        // Older packages have no full contract. Check the columns used by initial character saves.
        let tables: BTreeMap<String, Vec<String>> = serde_json::from_str(include_str!("character-save-columns.json"))?;
        Contract { schema: 1, columns: BTreeMap::from([("characters".into(), tables.into_iter().map(|(t, cols)| (t, cols.into_iter().map(|c| (c, String::new())).collect())).collect())]) }
    };
    let mut problems = Vec::new();
    for (kind, expected) in &contract.columns {
        let actual = read_columns(db, kind)?;
        for (table, cols) in expected {
            if matches!(table.as_str(), "updates" | "updates_include" | "coa_manager_migrations") { continue; }
            if !actual.contains_key(table) {
                problems.push(Problem { database: kind.clone(), table: table.clone(), column: String::new(), detail: "Missing table".into() });
                continue;
            }
            for (column, want) in cols {
                let found = actual[table].get(column);
                if found.is_none() || (!want.is_empty() && found != Some(want)) {
                    problems.push(Problem { database: kind.clone(), table: table.clone(), column: column.clone(), detail: found.map(|s| format!("Expected {want}, found {s}")).unwrap_or_else(|| "Missing column".into()) });
                }
            }
        }
    }
    // A custom NOT NULL column without a default can reject every INSERT even when all standard columns exist.
    let supplied: BTreeMap<String, Vec<String>> = serde_json::from_str(include_str!("character-save-columns.json"))?;
    let schema = db.realm_schema("acore_characters");
    let required = db.query(&format!("SELECT TABLE_NAME,COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='{schema}' AND IS_NULLABLE='NO' AND COLUMN_DEFAULT IS NULL AND EXTRA NOT LIKE '%auto_increment%' AND EXTRA NOT LIKE '%GENERATED%';"))?;
    for (table, column) in required.lines().filter_map(|l| l.split_once('\t')) {
        if supplied.get(table).is_some_and(|cols| !cols.iter().any(|c| c == column)) {
            problems.push(Problem { database: "characters".into(), table: table.into(), column: column.into(), detail: "Required column has no default and is not supplied by character creation".into() });
        }
    }
    // Check every CoA class represented in the starting data, rather than one arbitrary pair.
    if db.realm() == crate::realms::Mode::Coa {
        let rows = db.query("SELECT race,class FROM acore_world.playercreateinfo;")?;
        let pairs: BTreeSet<_> = rows.lines().filter_map(|l| l.split_once('\t')).map(|(a,b)| (a.to_owned(),b.to_owned())).collect();
        for class in 12..=32 {
            if !pairs.iter().any(|(_, c)| c == &class.to_string()) {
                problems.push(Problem { database: "world".into(), table: "playercreateinfo".into(), column: String::new(), detail: format!("No starting data for CoA class {class}") });
            }
        }
    }
    Ok(problems)
}
