//! Read-only database structure checks. A release can ship a contract captured from its clean database.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::{db::Db, error::Result, fsx};

pub const CONTRACT: &str = "Scripts/database-schema.json";
pub const WILDCARD_TABLES: &[(&str, &[&str])] = &[
    ("world", &["ascension_wildcard_boss_marks", "ascension_wildcard_tooltip_links"]),
    ("characters", &["coa_wildcard_skill_card", "coa_wildcard_skill_card_pending", "coa_wildcard_skill_card_account", "coa_wildcard_skill_card_purchase", "coa_wildcard_specialization_cache"]),
];
const WILDCARD_COLUMNS: &[(&str, &str, &[&str])] = &[
    ("world", "ascension_wildcard_boss_marks", &["CreatureEntry", "Amount"]),
    ("world", "ascension_wildcard_tooltip_links", &["Talent", "Ability"]),
    ("characters", "coa_wildcard_skill_card", &["account", "card", "progress"]),
    ("characters", "coa_wildcard_skill_card_pending", &["account", "id", "card"]),
    ("characters", "coa_wildcard_skill_card_account", &["account", "bonus_progress"]),
    ("characters", "coa_wildcard_skill_card_purchase", &["account", "type", "count"]),
    ("characters", "coa_wildcard_specialization_cache", &["account", "claimed_at"]),
];
type Columns = BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>;

#[derive(Debug, Serialize, Deserialize)]
pub struct Contract {
    pub schema: u32,
    pub columns: Columns,
}

pub fn require_release_contract(root: &Path) -> Result<Contract> {
    let contract: Contract = fsx::read_json(&root.join(CONTRACT))?;
    if contract.schema != 1 || ["auth", "characters", "world"].iter().any(|kind| {
        contract.columns.get(*kind).is_none_or(|tables| tables.is_empty() || tables.values().any(|columns| columns.is_empty()))
    }) {
        return Err(crate::Error::Invalid("The release must include a complete supported database schema contract.".into()));
    }
    Ok(contract)
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
    // Older releases omit a schema contract. Still verify the feature's actual tables,
    // independently of its migration history, before declaring an update healthy.
    if std::fs::read(root.join("Core/worldserver.exe")).is_ok_and(|bytes| bytes.windows(b"Wildcard synergy settings".len()).any(|w| w == b"Wildcard synergy settings")) {
        for (kind, tables) in WILDCARD_TABLES {
            let actual = read_columns(db, kind)?;
            for table in *tables {
                if !actual.contains_key(*table) {
                    problems.push(Problem { database: (*kind).into(), table: (*table).into(), column: String::new(), detail: "Missing required Wildcard table; migration history is not proof of its presence".into() });
                } else {
                    for (_, _, columns) in WILDCARD_COLUMNS.iter().filter(|(k, t, _)| k == kind && t == table) {
                        for column in *columns {
                            if !actual[*table].contains_key(*column) {
                                problems.push(Problem { database: (*kind).into(), table: (*table).into(), column: (*column).into(), detail: "Missing required Wildcard column".into() });
                            }
                        }
                    }
                }
            }
        }
    }
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
    // The any-race/class migration supports every standard playable race for all CoA classes.
    // A class existing for just one race does not protect character creation or enumeration.
    if db.realm() == crate::realms::Mode::Coa {
        let rows = db.query("SELECT race,class FROM acore_world.playercreateinfo;")?;
        problems.extend(missing_coa_starts(&rows));
    }
    // Check owned characters too, including legacy classes and Wildcard characters. Their rows
    // must not silently disappear from character selection after an otherwise healthy update.
    let missing = db.query("SELECT c.race,c.class,COUNT(*) FROM acore_characters.characters c LEFT JOIN acore_world.playercreateinfo p ON p.race=c.race AND p.class=c.class WHERE p.race IS NULL GROUP BY c.race,c.class;")?;
    for row in missing.lines() {
        let fields: Vec<_> = row.split('\t').collect();
        if fields.len() == 3 {
            problems.push(Problem { database: "world".into(), table: "playercreateinfo".into(), column: String::new(), detail: format!("{} existing characters have no starting data for race {} / class {} and would be hidden from character selection", fields[2], fields[0], fields[1]) });
        }
    }
    Ok(problems)
}

fn missing_coa_starts(rows: &str) -> Vec<Problem> {
    let pairs: BTreeSet<_> = rows.lines().filter_map(|l| l.split_once('\t')).map(|(a,b)| (a.to_owned(),b.to_owned())).collect();
    let mut problems = Vec::new();
    for race in [1, 2, 3, 4, 5, 6, 7, 8, 10, 11] {
        for class in 12..=32 {
            if !pairs.contains(&(race.to_string(), class.to_string())) {
                problems.push(Problem { database: "world".into(), table: "playercreateinfo".into(), column: String::new(), detail: format!("No starting data for CoA race {race} / class {class}") });
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_cannot_use_a_partial_or_empty_schema_contract() {
        let dir = tempfile::tempdir().unwrap();
        assert!(require_release_contract(dir.path()).is_err());
        fsx::atomic_write_json(&dir.path().join(CONTRACT), &Contract { schema: 1, columns: Columns::new() }).unwrap();
        assert!(require_release_contract(dir.path()).is_err());
        let mut columns = Columns::new();
        for kind in ["auth", "characters", "world"] {
            columns.insert(kind.into(), BTreeMap::from([("table".into(), BTreeMap::from([("id".into(), "int|NO|<NULL>|<NONE>".into())]))]));
        }
        fsx::atomic_write_json(&dir.path().join(CONTRACT), &Contract { schema: 1, columns }).unwrap();
        assert!(require_release_contract(dir.path()).is_ok());
    }

    #[test]
    fn detects_a_missing_orc_class_pair_even_when_the_class_exists_for_other_races() {
        let mut rows = String::new();
        for race in [1, 2, 3, 4, 5, 6, 7, 8, 10, 11] {
            for class in 12..=32 { rows.push_str(&format!("{race}\t{class}\n")); }
        }
        assert!(missing_coa_starts(&rows).is_empty());
        let incomplete = rows.lines().filter(|line| *line != "2\t30").collect::<Vec<_>>().join("\n");
        let problems = missing_coa_starts(&incomplete);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].detail.contains("race 2 / class 30"));
    }
}
