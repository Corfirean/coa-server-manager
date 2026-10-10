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

/// Keep all differences for support; never change a database to match a contract automatically.
pub fn check_with_report(db: &Db, root: &Path) -> Result<Vec<Problem>> {
    let problems = check(db, root)?;
    if problems.is_empty() { return Ok(problems); }
    let report = serde_json::json!({
        "schema": 1, "checkedAt": chrono::Utc::now().to_rfc3339(),
        "realm": db.realm().name(), "problems": problems,
    });
    let path = crate::registry::metadata_dir_for(root)?.join("diagnostics")
        .join(format!("schema-validation-{}-{}.json", db.realm().name(), uuid::Uuid::new_v4()));
    // A log write failure must neither hide the original mismatch nor accept an invalid schema.
    if let Err(error) = fsx::atomic_write_json(&path, &report) {
        tracing::warn!(%error, path = %path.display(), "Could not save database schema report");
    }
    Ok(problems)
}

fn describe_signature(signature: &str) -> String {
    let fields: Vec<_> = signature.split('|').collect();
    if fields.len() != 4 { return signature.into(); }
    let decode = |value: &str| -> String {
        if value == "<NULL>" { return "no default".into(); }
        if value == "<NONE>" { return "none".into(); }
        hex::decode(value).ok().and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| format!("{text:?}")).unwrap_or_else(|| value.into())
    };
    format!("type={}, nullable={}, default={}, extra={}", fields[0], fields[1], decode(fields[2]), decode(fields[3]))
}

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

fn missing_default(expected: &str, actual: &str) -> Option<String> {
    let expected: Vec<_> = expected.split('|').collect();
    let actual: Vec<_> = actual.split('|').collect();
    if expected.len() != 4 || actual.len() != 4 || expected[0] != actual[0]
        || expected[1] != "NO" || actual[1] != "NO" || expected[3] != "<NONE>" || actual[3] != "<NONE>"
        || actual[2] != "<NULL>" || matches!(expected[2], "<NULL>" | "<NONE>") { return None; }
    String::from_utf8(hex::decode(expected[2]).ok()?).ok()
}

/// Only restore a missing literal default from the exact signed target contract. Never alter data or custom defaults.
pub(crate) fn repair_missing_defaults(db: &Db, root: &Path, signed_hash: Option<&str>) -> Result<()> {
    let Some(signed_hash) = signed_hash else { return Ok(()); };
    if fsx::sha256_file(&root.join(CONTRACT))? != signed_hash {
        return Err(crate::Error::Invalid("The target schema contract differs from the signed update.".into()));
    }
    let contract = require_release_contract(root)?;
    let mut repaired = Vec::new();
    let quote = |identifier: &str| format!("`{}`", identifier.replace('`', "``"));
    for (kind, tables) in &contract.columns {
        let schema = db.realm_schema(crate::db::schema_of(kind)?);
        let actual = read_columns(db, kind)?;
        let base_tables: BTreeSet<_> = db.tables(crate::db::schema_of(kind)?)?.into_iter().collect();
        for (table, columns) in tables {
            if !base_tables.contains(table) { continue; }
            if matches!(table.as_str(), "updates" | "updates_include" | "coa_manager_migrations") { continue; }
            for (column, expected) in columns {
                let Some(found) = actual.get(table).and_then(|columns| columns.get(column)) else { continue; };
                let Some(value) = missing_default(expected, found) else { continue; };
                db.query(&format!("SET SESSION sql_mode=CONCAT_WS(',',@@SESSION.sql_mode,'NO_BACKSLASH_ESCAPES'); ALTER TABLE {}.{} ALTER COLUMN {} SET DEFAULT '{}';", quote(schema), quote(table), quote(column), value.replace('\'', "''")))?;
                repaired.push(serde_json::json!({"database":schema,"table":table,"column":column,"action":"restore-missing-default"}));
            }
        }
    }
    if !repaired.is_empty() {
        let path = crate::registry::metadata_dir_for(root)?.join("diagnostics").join(format!("database-repair-{}.json", uuid::Uuid::new_v4()));
        fsx::atomic_write_json(&path, &serde_json::json!({"schema":1,"checkedAt":chrono::Utc::now().to_rfc3339(),"contractSha256":signed_hash,"repairs":repaired}))?;
    }
    Ok(())
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
                    problems.push(Problem { database: kind.clone(), table: table.clone(), column: column.clone(), detail: found.map(|s| format!("Expected {}, found {}", describe_signature(want), describe_signature(s))).unwrap_or_else(|| "Missing column".into()) });
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
    fn default_repair_only_accepts_missing_literal_defaults_without_other_schema_changes() {
        assert_eq!(missing_default("int unsigned|NO|30|<NONE>", "int unsigned|NO|<NULL>|<NONE>"), Some("0".into()));
        for actual in ["int unsigned|NO|3939|<NONE>", "int|NO|<NULL>|<NONE>", "int unsigned|YES|<NULL>|<NONE>", "int unsigned|NO|<NULL>|6175746f5f696e6372656d656e74"] {
            assert!(missing_default("int unsigned|NO|30|<NONE>", actual).is_none());
        }
        assert!(missing_default("timestamp|NO|43555252454e545f54494d455354414d50|44454641554c545f47454e455241544544", "timestamp|NO|<NULL>|<NONE>").is_none());
        assert!(missing_default("int|NO|zz|<NONE>", "int|NO|<NULL>|<NONE>").is_none());
    }

    #[test]
    fn schema_defaults_are_readable_and_not_conflated() {
        assert_eq!(describe_signature("int unsigned|NO|30|<NONE>"), "type=int unsigned, nullable=NO, default=\"0\", extra=none");
        assert!(describe_signature("int unsigned|NO|<NULL>|<NONE>").contains("default=no default"));
        assert!(describe_signature("varchar(8)|YES||<NONE>").contains("default=\"\""));
        assert_eq!(describe_signature("legacy"), "legacy");
    }

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
