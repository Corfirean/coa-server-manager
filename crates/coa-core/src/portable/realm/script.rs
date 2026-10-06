//! The read-only SQL that is sent to a realm, and the parser of its answer.
//!
//! * **Fixed SQL.** Every statement is a literal in this file. The only input is the character's local guid, a `u32`
//!   formatted as a number; no text from anywhere (names, settings, other databases) is ever put into SQL. The only
//!   other identifiers, table and column names of unknown tables, come from `information_schema`, are checked as plain
//!   identifiers and are only ever used in `SELECT COUNT(*)`.
//! * **One consistent snapshot.** All statements run in *one* `mysql` session inside
//!   `START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY` at `REPEATABLE READ`: every table is read as of the same
//!   moment, and the session cannot write.
//! * **Unambiguous output.** Every text or binary column is hex-encoded (`x` + hex), so tabs, newlines, quotes and
//!   backslashes in names, item texts, settings or macros cannot confuse the parser.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::super::error::{PortableError, Result};
use super::registry::{classify, is_character_column, TableClass};

/// Schema names as the Manager writes them; `Db` routes them to the Wildcard schemas when needed.
pub const CHARACTERS_SCHEMA: &str = "acore_characters";
pub const AUTH_SCHEMA: &str = "acore_auth";

pub const SECTION_PREFIX: &str = "#T:";

/// Column names the schema probe asks about, to find per-character tables of unknown modules.
const CHARACTER_COLUMN_NAMES: &[&str] = &["guid", "owner_guid", "owner", "character_guid", "char_guid", "characterguid", "charguid", "player_guid", "playerguid", "owner_id", "bot_guid", "character_id"];

macro_rules! hex {
    ($c:expr) => {
        concat!("CONCAT('x',HEX(", $c, "))")
    };
}
macro_rules! hex_or_n {
    ($c:expr) => {
        concat!("IF(", $c, " IS NULL,'N',CONCAT('x',HEX(", $c, ")))")
    };
}

fn plain_identifier(s: &str) -> bool {
    crate::db::valid_identifier(s)
}

// ---- schema probe -----------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaProbe {
    pub tables: BTreeSet<String>,
    /// table -> its per-character columns (see [`is_character_column`]).
    pub character_columns: BTreeMap<String, Vec<String>>,
}

/// `schema` is the real schema name (`Db::realm_schema`); it appears in a string literal, which `Db` does not rewrite.
pub fn probe_sql(schema: &str) -> Result<String> {
    if !plain_identifier(schema) {
        return Err(PortableError::Invalid(format!("invalid schema name {schema:?}")));
    }
    let names = CHARACTER_COLUMN_NAMES.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(",");
    Ok(format!(
        "SELECT 'T', table_name, '-' FROM information_schema.tables WHERE table_schema = '{schema}' AND table_type = 'BASE TABLE' \
         UNION ALL \
         SELECT 'C', table_name, column_name FROM information_schema.columns WHERE table_schema = '{schema}' AND LOWER(column_name) IN ({names}) ORDER BY 1, 2, 3;"
    ))
}

pub fn parse_probe(output: &str) -> Result<SchemaProbe> {
    let mut probe = SchemaProbe::default();
    for line in output.lines().filter(|l| !l.is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 3 {
            return Err(PortableError::CorruptSnapshot(format!("unexpected schema probe row {line:?}")));
        }
        match cols[0] {
            "T" => {
                probe.tables.insert(cols[1].to_string());
            }
            "C" => probe.character_columns.entry(cols[1].to_string()).or_default().push(cols[2].to_string()),
            other => return Err(PortableError::CorruptSnapshot(format!("unexpected schema probe row kind {other:?}"))),
        }
    }
    Ok(probe)
}

impl SchemaProbe {
    /// Portable tables the realm does not have: the schema is too old or too different to export from.
    pub fn missing_required(&self) -> Vec<&'static str> {
        super::registry::TABLES.iter().filter(|(_, c)| *c == TableClass::Portable).map(|(t, _)| *t).filter(|t| !self.tables.contains(*t)).collect()
    }

    /// Tables this Manager does not know that carry a per-character column: `(table, column)`.
    pub fn unclassified_character_tables(&self) -> Vec<(String, String)> {
        self.character_columns
            .iter()
            .filter(|(table, _)| classify(table).is_none())
            .filter_map(|(table, columns)| columns.iter().find(|c| is_character_column(c)).map(|c| (table.clone(), c.clone())))
            .collect()
    }

    pub fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }
}

// ---- queries ------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub name: String,
    pub columns: Vec<&'static str>,
    pub sql: String,
}

fn query(name: &str, columns: &[(&'static str, &str)], from: &str, guid: u32) -> Query {
    let select = columns.iter().map(|(_, expr)| *expr).collect::<Vec<_>>().join(", ");
    Query {
        name: name.to_string(),
        columns: columns.iter().map(|(alias, _)| *alias).collect(),
        sql: format!("SELECT {select} FROM {}", from.replace("{guid}", &guid.to_string())),
    }
}

/// All statements for one character, in a fixed order. `probe` decides which optional modules' tables are checked.
pub fn queries(guid: u32, probe: &SchemaProbe) -> Result<Vec<Query>> {
    let mut out = vec![
        query(
            "chars",
            &[
                ("guid", "c.guid"),
                ("account", "c.account"),
                ("name", hex!("c.name")),
                ("race", "c.race"),
                ("class", "c.class"),
                ("gender", "c.gender"),
                ("level", "c.level"),
                ("xp", "c.xp"),
                ("money", "c.money"),
                ("skin", "c.skin"),
                ("face", "c.face"),
                ("hair_style", "c.hairStyle"),
                ("hair_color", "c.hairColor"),
                ("facial_style", "c.facialStyle"),
                ("bank_slots", "c.bankSlots"),
                ("player_flags", "c.playerFlags"),
                ("online", "c.online"),
                ("taxi_mask", hex!("c.taximask")),
                ("arena_points", "c.arenaPoints"),
                ("total_honor", "c.totalHonorPoints"),
                ("today_honor", "c.todayHonorPoints"),
                ("yesterday_honor", "c.yesterdayHonorPoints"),
                ("total_kills", "c.totalKills"),
                ("today_kills", "c.todayKills"),
                ("yesterday_kills", "c.yesterdayKills"),
                ("chosen_title", "c.chosenTitle"),
                ("known_currencies", "c.knownCurrencies"),
                ("watched_faction", "c.watchedFaction"),
                ("talent_groups", "c.talentGroupsCount"),
                ("active_group", "c.activeTalentGroup"),
                ("explored_zones", hex!("c.exploredZones")),
                ("known_titles", hex!("c.knownTitles")),
                ("action_bars", "c.actionBars"),
                ("extra_talents", "c.extraBonusTalentCount"),
                ("reset_cost", "c.resettalents_cost"),
                ("stable_slots", "c.stable_slots"),
                ("deleted", "IF(c.deleteDate IS NULL,0,1)"),
                ("username", "IFNULL((SELECT CONCAT('x',HEX(a.username)) FROM acore_auth.account a WHERE a.id = c.account),'-')"),
            ],
            "acore_characters.characters c WHERE c.guid = {guid}",
            guid,
        ),
        query("inventory", &[("bag", "ci.bag"), ("slot", "ci.slot"), ("item", "ci.item")], "acore_characters.character_inventory ci WHERE ci.guid = {guid} ORDER BY ci.bag, ci.slot, ci.item", guid),
        query(
            "items",
            &[
                ("guid", "ii.guid"),
                ("entry", "ii.itemEntry"),
                ("count", "ii.count"),
                ("duration", "ii.duration"),
                ("charges", hex_or_n!("ii.charges")),
                ("flags", "ii.flags"),
                ("enchantments", hex!("ii.enchantments")),
                ("random_property", "ii.randomPropertyId"),
                ("durability", "ii.durability"),
                ("played_time", "ii.playedTime"),
                ("text", hex_or_n!("ii.text")),
                ("creator_guid", "ii.creatorGuid"),
                ("creator_name", "IFNULL((SELECT CONCAT('x',HEX(cc.name)) FROM acore_characters.characters cc WHERE cc.guid = ii.creatorGuid),'-')"),
            ],
            "acore_characters.character_inventory ci JOIN acore_characters.item_instance ii ON ii.guid = ci.item WHERE ci.guid = {guid} ORDER BY ii.guid",
            guid,
        ),
        query("gifts", &[("item_guid", "g.item_guid"), ("entry", "g.entry"), ("flags", "g.flags")], "acore_characters.character_gifts g WHERE g.guid = {guid} ORDER BY g.item_guid", guid),
        query("spells", &[("spell", "s.spell"), ("spec_mask", "s.specMask")], "acore_characters.character_spell s WHERE s.guid = {guid} ORDER BY s.spell", guid),
        query("talents", &[("spell", "t.spell"), ("spec_mask", "t.specMask")], "acore_characters.character_talent t WHERE t.guid = {guid} ORDER BY t.spell", guid),
        query("skills", &[("skill", "s.skill"), ("value", "s.value"), ("max", "s.`max`")], "acore_characters.character_skills s WHERE s.guid = {guid} ORDER BY s.skill", guid),
        query(
            "glyphs",
            &[("talent_group", "g.talentGroup"), ("g1", "g.glyph1"), ("g2", "g.glyph2"), ("g3", "g.glyph3"), ("g4", "g.glyph4"), ("g5", "g.glyph5"), ("g6", "g.glyph6")],
            "acore_characters.character_glyphs g WHERE g.guid = {guid} ORDER BY g.talentGroup",
            guid,
        ),
        query("reputation", &[("faction", "r.faction"), ("standing", "r.standing"), ("flags", "r.flags")], "acore_characters.character_reputation r WHERE r.guid = {guid} ORDER BY r.faction", guid),
        query(
            "quests",
            &[
                ("quest", "q.quest"),
                ("status", "q.status"),
                ("explored", "q.explored"),
                ("timer", "q.timer"),
                ("mob1", "q.mobcount1"),
                ("mob2", "q.mobcount2"),
                ("mob3", "q.mobcount3"),
                ("mob4", "q.mobcount4"),
                ("item1", "q.itemcount1"),
                ("item2", "q.itemcount2"),
                ("item3", "q.itemcount3"),
                ("item4", "q.itemcount4"),
                ("item5", "q.itemcount5"),
                ("item6", "q.itemcount6"),
                ("player_count", "q.playercount"),
            ],
            "acore_characters.character_queststatus q WHERE q.guid = {guid} ORDER BY q.quest",
            guid,
        ),
        // the realm itself loads only rows with `active = 1` (`CHAR_SEL_CHARACTER_QUESTSTATUSREW`)
        query("rewarded", &[("quest", "q.quest")], "acore_characters.character_queststatus_rewarded q WHERE q.guid = {guid} AND q.active = 1 ORDER BY q.quest", guid),
        query("actions", &[("spec", "a.spec"), ("button", "a.button"), ("action", "a.action"), ("type", "a.`type`")], "acore_characters.character_action a WHERE a.guid = {guid} ORDER BY a.spec, a.button", guid),
        query(
            "pets",
            &[
                ("id", "p.id"),
                ("entry", "p.entry"),
                ("model", "p.modelid"),
                ("created_by", "p.CreatedBySpell"),
                ("pet_type", "p.PetType"),
                ("level", "p.level"),
                ("exp", "p.exp"),
                ("react", "p.Reactstate"),
                ("name", hex!("p.name")),
                ("renamed", "p.renamed"),
                ("slot", "p.slot"),
                ("health", "p.curhealth"),
                ("mana", "p.curmana"),
                ("happiness", "p.curhappiness"),
                ("abdata", hex_or_n!("p.abdata")),
            ],
            "acore_characters.character_pet p WHERE p.owner = {guid} ORDER BY p.id",
            guid,
        ),
        query(
            "pet_spells",
            &[("pet", "ps.guid"), ("spell", "ps.spell"), ("active", "ps.active")],
            "acore_characters.pet_spell ps JOIN acore_characters.character_pet p ON p.id = ps.guid WHERE p.owner = {guid} ORDER BY ps.guid, ps.spell",
            guid,
        ),
        query(
            "pet_declined",
            &[("id", "d.id"), ("n1", hex!("d.genitive")), ("n2", hex!("d.dative")), ("n3", hex!("d.accusative")), ("n4", hex!("d.instrumental")), ("n5", hex!("d.prepositional"))],
            "acore_characters.character_pet_declinedname d WHERE d.owner = {guid} ORDER BY d.id",
            guid,
        ),
        query("settings", &[("source", hex!("s.source")), ("data", hex!("s.data"))], "acore_characters.character_settings s WHERE s.guid = {guid} ORDER BY s.source", guid),
        // macros are per-character account-data type 5 (`PER_CHARACTER_MACROS_CACHE`)
        query("macros", &[("time", "d.`time`"), ("data", hex!("d.data"))], "acore_characters.character_account_data d WHERE d.guid = {guid} AND d.`type` = 5", guid),
    ];

    // Blockers of optional modules: only asked for when the module's table exists.
    if probe.has("coa_character_challenge") {
        out.push(query("block:challenge", &[("n", "COUNT(*)")], "acore_characters.coa_character_challenge WHERE guid = {guid}", guid));
    }
    if probe.has("coa_character_gamemode") {
        out.push(query("block:gamemode", &[("n", "COUNT(*)")], "acore_characters.coa_character_gamemode WHERE guid = {guid} AND gameMode <> 0", guid));
    }
    if probe.has("coa_custom_trial_active") {
        out.push(query("block:trial", &[("n", "COUNT(*)")], "acore_characters.coa_custom_trial_active WHERE guid = {guid}", guid));
    }
    if probe.has("ascension_manastorm_cache") {
        out.push(query("block:manastorm", &[("n", "COUNT(*)")], "acore_characters.ascension_manastorm_cache WHERE guid = {guid}", guid));
    }
    // Per-character tables this Manager has never heard of.
    for (table, column) in probe.unclassified_character_tables() {
        if !plain_identifier(&table) || !plain_identifier(&column) {
            return Err(PortableError::Invalid(format!("table or column name of an unknown table is not a plain identifier: {table}.{column}")));
        }
        out.push(Query {
            name: format!("unclassified:{table}"),
            columns: vec!["n"],
            sql: format!("SELECT COUNT(*) FROM acore_characters.`{table}` WHERE `{column}` = {guid}"),
        });
    }
    Ok(out)
}

/// The whole script for one character: one session, one consistent read-only snapshot.
pub fn snapshot_script(queries: &[Query]) -> String {
    let mut script = String::from("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;\nSTART TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;\n");
    for q in queries {
        script.push_str(&format!("SELECT '{SECTION_PREFIX}{}';\n{};\n", q.name, q.sql));
    }
    script.push_str("COMMIT;\n");
    script
}

// ---- listing characters -------------------------------------------------------------------------------------------

pub const LIST_COLUMNS: &[&str] = &["guid", "name", "race", "class", "level", "online", "account", "username", "deleted", "challenge", "gamemode", "trial", "manastorm"];

/// Characters of non-bot accounts, with the blocker facts that can be computed cheaply for all of them at once.
pub fn list_sql(probe: &SchemaProbe) -> String {
    let exists = |table: &str, cond: &str| {
        if probe.has(table) {
            format!("EXISTS(SELECT 1 FROM acore_characters.{table} x WHERE x.guid = c.guid{cond})")
        } else {
            "0".to_string()
        }
    };
    format!(
        "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;\n\
         START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;\n\
         SELECT c.guid, CONCAT('x',HEX(c.name)), c.race, c.class, c.level, c.online, c.account, IFNULL(CONCAT('x',HEX(a.username)),'-'), IF(c.deleteDate IS NULL,0,1), {}, {}, {}, {} \
         FROM acore_characters.characters c LEFT JOIN acore_auth.account a ON a.id = c.account \
         WHERE a.username IS NULL OR (UPPER(a.username) NOT LIKE 'COABOT%' AND UPPER(a.username) <> 'COAMANAGER') \
         ORDER BY c.guid;\n\
         COMMIT;\n",
        exists("coa_character_challenge", ""),
        exists("coa_character_gamemode", " AND x.gameMode <> 0"),
        exists("coa_custom_trial_active", ""),
        exists("ascension_manastorm_cache", ""),
    )
}

// ---- parsing ------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Rows {
    pub columns: Vec<&'static str>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct RawExport {
    sections: HashMap<String, Rows>,
}

impl RawExport {
    pub fn section(&self, name: &str) -> Result<&Rows> {
        self.sections.get(name).ok_or_else(|| PortableError::CorruptSnapshot(format!("the realm answer has no section {name}")))
    }
    pub fn has(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }
}

/// `output` is exactly what `Db::query` returned for [`snapshot_script`].
pub fn parse_output(output: &str, queries: &[Query]) -> Result<RawExport> {
    let by_name: HashMap<&str, &Query> = queries.iter().map(|q| (q.name.as_str(), q)).collect();
    let mut sections: HashMap<String, Rows> = HashMap::new();
    let mut current: Option<String> = None;
    for line in output.lines() {
        if let Some(name) = line.strip_prefix(SECTION_PREFIX) {
            let query = by_name.get(name).ok_or_else(|| PortableError::CorruptSnapshot(format!("unexpected section {name:?} in the realm answer")))?;
            if sections.insert(name.to_string(), Rows { columns: query.columns.clone(), rows: Vec::new() }).is_some() {
                return Err(PortableError::CorruptSnapshot(format!("section {name} appears twice")));
            }
            current = Some(name.to_string());
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let name = current.as_ref().ok_or_else(|| PortableError::CorruptSnapshot("data before the first section marker".into()))?;
        let rows = sections.get_mut(name).expect("inserted with the marker");
        let cells: Vec<String> = line.split('\t').map(str::to_string).collect();
        if cells.len() != rows.columns.len() {
            return Err(PortableError::CorruptSnapshot(format!("section {name}: expected {} columns, got {}", rows.columns.len(), cells.len())));
        }
        rows.rows.push(cells);
    }
    for q in queries {
        if !sections.contains_key(&q.name) {
            return Err(PortableError::CorruptSnapshot(format!("the realm answer is incomplete: section {} is missing", q.name)));
        }
    }
    Ok(RawExport { sections })
}

impl Rows {
    pub fn iter(&self) -> impl Iterator<Item = Row<'_>> {
        self.rows.iter().map(|cells| Row { columns: &self.columns, cells })
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[derive(Clone, Copy)]
pub struct Row<'a> {
    columns: &'a [&'static str],
    cells: &'a [String],
}

impl Row<'_> {
    fn cell(&self, column: &str) -> Result<&str> {
        let index = self.columns.iter().position(|c| *c == column).ok_or_else(|| PortableError::Invalid(format!("internal: no column {column}")))?;
        Ok(&self.cells[index])
    }

    pub fn u64(&self, column: &str) -> Result<u64> {
        let text = self.cell(column)?;
        text.parse().map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {text:?} is not an unsigned integer")))
    }

    pub fn i64(&self, column: &str) -> Result<i64> {
        let text = self.cell(column)?;
        text.parse().map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {text:?} is not an integer")))
    }

    pub fn u32(&self, column: &str) -> Result<u32> {
        let v = self.u64(column)?;
        u32::try_from(v).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {v} does not fit 32 bits")))
    }

    pub fn u16(&self, column: &str) -> Result<u16> {
        let v = self.u64(column)?;
        u16::try_from(v).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {v} does not fit 16 bits")))
    }

    pub fn u8(&self, column: &str) -> Result<u8> {
        let v = self.u64(column)?;
        u8::try_from(v).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {v} does not fit 8 bits")))
    }

    pub fn i32(&self, column: &str) -> Result<i32> {
        let v = self.i64(column)?;
        i32::try_from(v).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: {v} does not fit a signed 32-bit integer")))
    }

    /// Bytes of a hex column; `None` for SQL NULL (`NULL`, `N` or `-` markers).
    pub fn opt_bytes(&self, column: &str) -> Result<Option<Vec<u8>>> {
        let text = self.cell(column)?;
        if matches!(text, "NULL" | "N" | "-") {
            return Ok(None);
        }
        let hex = text.strip_prefix('x').ok_or_else(|| PortableError::CorruptSnapshot(format!("column {column}: not a hex value")))?;
        hex::decode(hex).map(Some).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: invalid hex")))
    }

    pub fn bytes(&self, column: &str) -> Result<Vec<u8>> {
        Ok(self.opt_bytes(column)?.unwrap_or_default())
    }

    pub fn opt_text(&self, column: &str) -> Result<Option<String>> {
        self.opt_bytes(column)?
            .map(|b| String::from_utf8(b).map_err(|_| PortableError::CorruptSnapshot(format!("column {column}: not valid UTF-8"))))
            .transpose()
    }

    /// A plain (not hex) text cell such as a number or a keyword.
    pub fn plain(&self, column: &str) -> Result<String> {
        Ok(self.cell(column)?.to_string())
    }

    pub fn text(&self, column: &str) -> Result<String> {
        Ok(self.opt_text(column)?.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_with(tables: &[&str]) -> SchemaProbe {
        SchemaProbe { tables: tables.iter().map(|t| t.to_string()).collect(), character_columns: BTreeMap::new() }
    }

    fn script_for(guid: u32, probe: &SchemaProbe) -> String {
        snapshot_script(&queries(guid, probe).unwrap())
    }

    #[test]
    fn the_script_is_one_read_only_consistent_snapshot() {
        let script = script_for(1002, &probe_with(&["coa_character_challenge", "coa_character_gamemode", "coa_custom_trial_active", "ascension_manastorm_cache"]));
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[0], "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;");
        assert_eq!(lines[1], "START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;");
        assert_eq!(*lines.last().unwrap(), "COMMIT;");
        // every other statement is a plain SELECT
        for line in &lines[2..lines.len() - 1] {
            assert!(line.starts_with("SELECT "), "not a SELECT: {line}");
        }
    }

    #[test]
    fn nothing_in_the_script_can_write() {
        let mut probe = probe_with(&["coa_character_challenge"]);
        probe.character_columns.insert("mod_unknown_state".into(), vec!["guid".into()]);
        let script = script_for(7, &probe).to_ascii_uppercase();
        for forbidden in ["INSERT ", "UPDATE ", "DELETE ", "REPLACE ", "DROP ", "ALTER ", "CREATE ", "TRUNCATE ", "GRANT ", "SET GLOBAL", "LOAD DATA", "INTO OUTFILE", "CALL ", "LOCK TABLES", "FOR UPDATE", "FOR SHARE"] {
            assert!(!script.contains(forbidden), "the script contains {forbidden:?}");
        }
        // the only SET is the isolation level, before the transaction starts
        assert_eq!(script.matches("SET ").count(), 1);
    }

    #[test]
    fn the_only_input_is_the_guid_as_a_number() {
        let a = script_for(1002, &probe_with(&[]));
        let b = script_for(1003, &probe_with(&[]));
        assert_eq!(a.replace("1002", "G"), b.replace("1003", "G"), "scripts for two guids differ only in the number");
        assert!(!a.contains("{guid}"));
        assert!(a.contains("c.guid = 1002"));
    }

    #[test]
    fn optional_module_checks_follow_the_schema() {
        let none = queries(1, &probe_with(&[])).unwrap();
        assert!(none.iter().all(|q| !q.name.starts_with("block:")));
        let all = queries(1, &probe_with(&["coa_character_challenge", "coa_character_gamemode", "coa_custom_trial_active", "ascension_manastorm_cache"])).unwrap();
        let names: Vec<&str> = all.iter().filter(|q| q.name.starts_with("block:")).map(|q| q.name.as_str()).collect();
        assert_eq!(names, ["block:challenge", "block:gamemode", "block:trial", "block:manastorm"]);
    }

    #[test]
    fn unknown_character_tables_get_a_count_query() {
        let mut probe = probe_with(&[]);
        probe.character_columns.insert("mod_unknown_state".into(), vec!["guid".into()]);
        probe.character_columns.insert("item_instance".into(), vec!["guid".into(), "owner_guid".into()]);
        probe.character_columns.insert("mod_other".into(), vec!["account".into()]);
        let qs = queries(55, &probe).unwrap();
        let unknown: Vec<&Query> = qs.iter().filter(|q| q.name.starts_with("unclassified:")).collect();
        assert_eq!(unknown.len(), 1, "known tables and tables without a per-character column are not counted");
        assert_eq!(unknown[0].sql, "SELECT COUNT(*) FROM acore_characters.`mod_unknown_state` WHERE `guid` = 55");
    }

    #[test]
    fn a_hostile_table_name_from_the_schema_is_refused() {
        let mut probe = probe_with(&[]);
        probe.character_columns.insert("x`; DROP TABLE characters; --".into(), vec!["guid".into()]);
        assert!(queries(1, &probe).is_err());
    }

    #[test]
    fn the_probe_names_the_schema_literally_and_refuses_odd_schema_names() {
        let sql = probe_sql("acore_characters_wildcard").unwrap();
        assert!(sql.contains("table_schema = 'acore_characters_wildcard'"));
        assert!(probe_sql("a'; DROP").is_err());
    }

    #[test]
    fn probe_output_is_parsed_and_checked_against_the_registry() {
        let out = "T\tcharacters\t\nT\tmod_new\t\nC\tmod_new\tguid\nC\tcharacters\tguid\nC\tmod_acct\taccount\n";
        let probe = parse_probe(out).unwrap();
        assert!(probe.has("characters") && probe.has("mod_new"));
        assert_eq!(probe.unclassified_character_tables(), vec![("mod_new".to_string(), "guid".to_string())]);
        assert!(probe.missing_required().contains(&"item_instance"));
        assert!(parse_probe("X\ta\tb\n").is_err());
        assert!(parse_probe("T\ta\n").is_err());
    }

    #[test]
    fn output_is_split_into_sections_by_marker() {
        let probe = probe_with(&[]);
        let qs: Vec<Query> = queries(1, &probe).unwrap().into_iter().filter(|q| matches!(q.name.as_str(), "spells" | "reputation")).collect();
        let out = "#T:spells\n10\t1\n11\t3\n#T:reputation\n";
        let raw = parse_output(out, &qs).unwrap();
        let spells: Vec<(u32, u8)> = raw.section("spells").unwrap().iter().map(|r| (r.u32("spell").unwrap(), r.u8("spec_mask").unwrap())).collect();
        assert_eq!(spells, vec![(10, 1), (11, 3)]);
        assert!(raw.section("reputation").unwrap().is_empty());
    }

    #[test]
    fn malformed_answers_are_refused() {
        let qs: Vec<Query> = queries(1, &probe_with(&[])).unwrap().into_iter().filter(|q| q.name == "spells").collect();
        assert!(parse_output("#T:spells\n1\n", &qs).is_err(), "wrong column count");
        assert!(parse_output("#T:other\n", &qs).is_err(), "unknown section");
        assert!(parse_output("5\t1\n", &qs).is_err(), "data before a marker");
        assert!(parse_output("#T:spells\n#T:spells\n", &qs).is_err(), "twice");
        assert!(parse_output("", &qs).is_err(), "missing section");
        let raw = parse_output("#T:spells\nabc\t1\n", &qs).unwrap();
        assert!(raw.section("spells").unwrap().iter().next().unwrap().u32("spell").is_err());
    }

    #[test]
    fn text_columns_are_hex_and_survive_any_content() {
        let qs: Vec<Query> = queries(1, &probe_with(&[])).unwrap().into_iter().filter(|q| q.name == "settings").collect();
        let tricky = "tab\there \"quote\" 'q' back\\slash\nnewline NULL and \u{4e2d}\u{6587}";
        let out = format!("#T:settings\nx{}\tx{}\n", hex::encode(tricky), hex::encode("1 2 3 "));
        let raw = parse_output(&out, &qs).unwrap();
        let row = raw.section("settings").unwrap().iter().next().unwrap();
        assert_eq!(row.text("source").unwrap(), tricky);
        assert_eq!(row.text("data").unwrap(), "1 2 3 ");
        // SQL NULL and the 'no value' markers
        let none = parse_output("#T:settings\nNULL\tN\n", &qs).unwrap();
        let row = none.section("settings").unwrap().iter().next().unwrap();
        assert_eq!((row.opt_text("source").unwrap(), row.opt_text("data").unwrap()), (None, None));
        let bad = parse_output("#T:settings\nxZZ\tx\n", &qs).unwrap();
        assert!(bad.section("settings").unwrap().iter().next().unwrap().text("source").is_err());
    }

    #[test]
    fn the_list_query_hides_bots_and_is_read_only_too() {
        let sql = list_sql(&probe_with(&["coa_character_gamemode"]));
        assert!(sql.contains("COABOT%") && sql.contains("COAMANAGER"));
        assert!(sql.contains("READ ONLY"));
        assert!(sql.contains("gameMode <> 0"));
    }
}
