//! SQUID Playerbots on a repack (`Scripts/squid_playerbots.py`): the bot modules must not both run, how long a
//! SQUID world may take to start, and its database, provisioned before the world starts.
//!
//! The SQL journals are the ones the Python script keeps, unchanged: `coa_squid_migrations(path, sha256)` and
//! `coa_squid_pending(path, sha256)` in every target schema, and the SQUID installer's `coa_bots_installed`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha1::Digest as _;

use crate::error::{Error, Result};

/// `key = value` lines of a module config (comments and quotes dropped), as `options()` reads them.
pub fn options(path: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(bytes) = std::fs::read(path) else { return out };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    for line in text.lines() {
        let line = line.trim_start();
        let Some((key, rest)) = line.split_once('=') else { continue };
        let key = key.trim_end();
        if key.is_empty() || key.starts_with('#') || key.contains(char::is_whitespace) {
            continue;
        }
        let value = rest.split('#').next().unwrap_or("").trim();
        out.insert(key.to_string(), value.trim_matches('"').to_string());
    }
    out
}

/// A secondary world shares the primary's package files.
pub fn package_root(root: &Path) -> PathBuf {
    let secondary = root.file_name().is_some_and(|n| n == "secondary")
        && root.parent().and_then(|p| p.file_name()).is_some_and(|n| n == ".realms");
    if secondary {
        root.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf())
    } else {
        root.to_path_buf()
    }
}

pub fn module_options(root: &Path, name: &str) -> (PathBuf, BTreeMap<String, String>) {
    let folder = root.join("Core/configs/modules");
    let active = folder.join(name);
    let mut path = if active.exists() { active } else { folder.join(format!("{name}.dist")) };
    if !path.exists() && name == "playerbots.conf" {
        path = package_root(root).join("Core/configs/modules/playerbots.conf.dist");
    }
    let values = options(&path);
    (path, values)
}

/// Invalid booleans count as the modules' compiled default: enabled.
pub fn enabled(values: &BTreeMap<String, String>, key: &str) -> bool {
    !matches!(values.get(key).map(|v| v.to_ascii_lowercase()).as_deref(), Some("0" | "false" | "no" | "off"))
}

pub fn validate_bots(root: &Path) -> Result<()> {
    let (a, av) = module_options(root, "mod_coa_playerbots.conf");
    let (b, bv) = module_options(root, "playerbots.conf");
    if a.exists() && b.exists() && enabled(&av, "CoaBots.Enable") && enabled(&bv, "AiPlayerbot.Enabled") {
        return Err(Error::Invalid(
            "CoA Companions and SQUID Playerbots cannot both be enabled. Disable one bot module in Modules or its config.".into(),
        ));
    }
    Ok(())
}

/// Seconds a world may take to open its port: SQUID loads its bots first.
pub fn startup_timeout(root: &Path) -> u64 {
    let (path, values) = module_options(root, "playerbots.conf");
    if path.exists() && enabled(&values, "AiPlayerbot.Enabled") {
        1500
    } else {
        180
    }
}

/// Replace (or append) `key = value` lines; the file is written with "\n" line ends.
pub fn set_options(path: &Path, values: &[(&str, String)]) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(&text).replace("\r\n", "\n");
    for (key, value) in values {
        let line = format!("{key} = {value}");
        let mut found = false;
        let lines: Vec<String> = text
            .split('\n')
            .map(|l| {
                let t = l.trim_start();
                let matches = t.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
                if matches {
                    found = true;
                    line.clone()
                } else {
                    l.to_string()
                }
            })
            .collect();
        text = lines.join("\n");
        if !found {
            text.push('\n');
            text.push_str(&line);
            text.push('\n');
        }
    }
    crate::fsx::atomic_write(path, text.as_bytes())
}

fn valid_relative(rel: &str) -> bool {
    !rel.is_empty() && rel.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'))
}

/// Table names a base file creates (`CREATE TABLE [IF NOT EXISTS] name`).
fn created_tables(sql: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let upper = sql.to_ascii_uppercase();
    let mut from = 0;
    while let Some(i) = upper[from..].find("CREATE") {
        let start = from + i + "CREATE".len();
        from = start;
        let rest = &sql[start..];
        let mut words = rest.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("TABLE")) {
            continue;
        }
        let mut name = words.next().unwrap_or("");
        if name.eq_ignore_ascii_case("IF") {
            let _ = words.next(); // NOT
            let _ = words.next(); // EXISTS
            name = words.next().unwrap_or("");
        }
        let name: String = name.trim_start_matches('`').chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::fsx::sha256_bytes(bytes)
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut h = sha1::Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn rows(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|r| r.split_once('\t')).map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// Provision an enabled SQUID without replaying imported base tables (`prepare_playerbots`). `mysql(sql)` runs
/// SQL with the repack's administrator login and returns its output.
pub fn prepare_playerbots(root: &Path, mysql_port: u16, mysql: &dyn Fn(&str) -> Result<String>, progress: &dyn Fn(&str)) -> Result<()> {
    let (mut path, values) = module_options(root, "playerbots.conf");
    if !path.exists() || !enabled(&values, "AiPlayerbot.Enabled") {
        return Ok(());
    }
    validate_bots(root)?;
    let sql_root = package_root(root).join("Extras/SquidPlayerbots/sql");
    if !sql_root.join("playerbots/base").is_dir() {
        return Err(Error::Invalid("SQUID Playerbots database files are missing. Repair the server installation.".into()));
    }
    let world = options(&root.join("Core/configs/worldserver.conf"));
    let mut schemas: Vec<(&str, String)> = Vec::new();
    for (kind, key) in [("characters", "CharacterDatabaseInfo"), ("world", "WorldDatabaseInfo")] {
        let schema = world.get(key).and_then(|v| v.rsplit(';').next()).unwrap_or("").to_string();
        let ok = matches!(schema.as_str(), "acore_world" | "acore_characters" | "acore_world_wildcard" | "acore_characters_wildcard");
        if !ok {
            return Err(Error::Invalid("Unsupported world/characters database for SQUID Playerbots.".into()));
        }
        schemas.push((kind, schema));
    }
    schemas.push(("playerbots", "acore_playerbots".into()));

    let list = |schema: &str| -> Result<BTreeSet<String>> {
        Ok(mysql(&format!("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='{schema}';"))?
            .lines()
            .map(str::to_string)
            .collect())
    };
    let existing_playerbots = list("acore_playerbots")?;
    let bookkeeping = ["updates", "updates_include", "version_db_playerbots", "coa_squid_migrations", "coa_bots_installed", "coa_squid_pending"];
    let data_tables: Vec<&String> = existing_playerbots.iter().filter(|t| !bookkeeping.contains(&t.as_str())).collect();
    let mut installer_history = BTreeSet::new();
    if existing_playerbots.contains("coa_bots_installed") {
        for name in mysql("SELECT file FROM `acore_playerbots`.coa_bots_installed;")?.lines() {
            let mut n = name.replace('\\', "/");
            while n.contains("//") {
                n = n.replace("//", "/");
            }
            let n = n.trim_matches('/').to_string();
            if n.to_ascii_lowercase().ends_with(".sql") {
                installer_history.insert(n);
            }
        }
    }
    let mut tables: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut histories: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    let mut ledgers: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    for (kind, schema) in &schemas {
        let t = if *kind == "playerbots" { existing_playerbots.clone() } else { list(schema)? };
        if t.contains("coa_squid_pending") {
            let pending = mysql(&format!("SELECT path FROM `{schema}`.coa_squid_pending;"))?;
            let pending = pending.trim();
            if !pending.is_empty() {
                return Err(Error::Invalid(format!(
                    "A SQUID base import was interrupted. Restore the database backup or repair the incomplete import before continuing; no SQL was applied: {pending}"
                )));
            }
        }
        let mut history = BTreeMap::new();
        if t.contains("updates") {
            for (name, digest) in rows(&mysql(&format!("SELECT name,hash FROM `{schema}`.updates;"))?) {
                history.insert(name, digest.to_ascii_lowercase());
            }
        }
        let mut ledger = BTreeMap::new();
        if t.contains("coa_squid_migrations") {
            for (name, digest) in rows(&mysql(&format!("SELECT path,sha256 FROM `{schema}`.coa_squid_migrations;"))?) {
                ledger.insert(name, digest);
            }
        }
        tables.insert(kind, t);
        histories.insert(kind, history);
        ledgers.insert(kind, ledger);
    }
    if !data_tables.is_empty() && installer_history.is_empty() && histories["playerbots"].is_empty() && ledgers["playerbots"].is_empty() {
        for table in data_tables {
            let quoted = table.replace('`', "``");
            if mysql(&format!("SELECT EXISTS(SELECT 1 FROM `acore_playerbots`.`{quoted}` LIMIT 1);"))?.trim() == "1" {
                return Err(Error::Invalid(
                    "Existing SQUID data has no recorded installation history. Repair the imported database history before continuing; no SQL was applied.".into(),
                ));
            }
        }
    }
    mysql("CREATE DATABASE IF NOT EXISTS acore_playerbots CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;")?;
    for host in ["localhost", "127.0.0.1"] {
        mysql(&format!("GRANT ALL PRIVILEGES ON acore_playerbots.* TO 'acore'@'{host}';"))?;
    }
    let (_, password) = crate::db::credentials(root)?;
    if password.len() != 48 || !password.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Error::Invalid("Invalid packaged database configuration.".into()));
    }
    if path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".dist")) {
        let active = root.join("Core/configs/modules/playerbots.conf");
        std::fs::create_dir_all(active.parent().unwrap())?;
        std::fs::write(&active, std::fs::read(&path)?)?;
        path = active;
    }
    set_options(
        &path,
        &[
            ("PlayerbotsDatabaseInfo", format!("\"127.0.0.1;{mysql_port};acore;{password};acore_playerbots\"")),
            ("Playerbots.Updates.EnableDatabases", "0".into()),
        ],
    )?;
    for (kind, schema) in &schemas {
        mysql(&format!("CREATE TABLE IF NOT EXISTS `{schema}`.coa_squid_migrations (path VARCHAR(240) PRIMARY KEY, sha256 CHAR(64) NOT NULL);"))?;
        mysql(&format!("CREATE TABLE IF NOT EXISTS `{schema}`.coa_squid_pending (path VARCHAR(240) PRIMARY KEY, sha256 CHAR(64) NOT NULL);"))?;
        let history = &histories[kind];
        let recorded = ledgers[kind].clone();
        for stage in ["base", "updates", "custom"] {
            let mut sources: Vec<PathBuf> = std::fs::read_dir(sql_root.join(kind).join(stage))
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "sql")).collect())
                .unwrap_or_default();
            sources.sort();
            for source in sources {
                let relative = source.strip_prefix(&sql_root).unwrap_or(&source).to_string_lossy().replace('\\', "/");
                if !valid_relative(&relative) {
                    return Err(Error::Invalid("Invalid SQUID SQL filename.".into()));
                }
                let bytes = std::fs::read(&source)?;
                let digest = sha256_hex(&bytes);
                if let Some(previous) = recorded.get(&relative) {
                    if *previous != digest {
                        return Err(Error::Invalid(format!("A previously applied SQUID database migration changed: {relative}")));
                    }
                    if stage != "base" {
                        continue;
                    }
                }
                let ledger_sql = if recorded.contains_key(&relative) {
                    String::new()
                } else {
                    format!("INSERT INTO `{schema}`.coa_squid_migrations VALUES ('{relative}','{digest}');")
                };
                let text = String::from_utf8_lossy(&bytes);
                let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
                if stage == "base" {
                    let definitions = created_tables(&text);
                    let present: BTreeSet<String> = definitions.intersection(&tables[kind]).cloned().collect();
                    if !present.is_empty() && present != definitions {
                        return Err(Error::Invalid(format!("SQUID base file has only some of its tables. Repair the incomplete database before continuing: {relative}")));
                    }
                    if definitions.is_empty() {
                        return Err(Error::Invalid(format!("SQUID base file has no recognizable table definitions; repair is required: {relative}")));
                    }
                    if present == definitions {
                        if !ledger_sql.is_empty() {
                            mysql(&ledger_sql)?;
                        }
                        continue;
                    }
                    mysql(&format!("INSERT INTO `{schema}`.coa_squid_pending VALUES ('{relative}','{digest}'); COMMIT;"))?;
                    let complete = format!("DELETE FROM `{schema}`.coa_squid_pending WHERE path='{relative}' AND sha256='{digest}'; COMMIT;");
                    progress(&format!("Importing SQUID {relative} into {schema} ({} bytes); large imports may take several minutes...", bytes.len()));
                    mysql(&format!("USE `{schema}`;\n{text}\n{ledger_sql}\n{complete}")).map_err(|_| {
                        Error::Invalid(format!(
                            "SQUID import failed: {relative} in {schema}. The world server was not started. The incomplete import is recorded; restore a database backup or repair that import before retrying."
                        ))
                    })?;
                    progress(&format!("Imported SQUID {relative}."));
                    if let Some(t) = tables.get_mut(kind) {
                        t.extend(definitions);
                    }
                    continue;
                }
                if installer_history.iter().any(|n| *n == relative || n.ends_with(&format!("/{relative}"))) {
                    mysql(&ledger_sql)?;
                    continue;
                }
                let file_name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
                if let Some(upstream) = history.get(&file_name) {
                    let lf = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
                    let accepted = [sha1_hex(&bytes), sha1_hex(lf.as_bytes())];
                    if !upstream.is_empty() && !accepted.contains(upstream) {
                        return Err(Error::Invalid(format!("SQUID update history does not match the shipped SQL: {relative}")));
                    }
                    mysql(&ledger_sql)?;
                    continue;
                }
                mysql(&format!("USE `{schema}`;\n{text}\n{ledger_sql}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_lines_are_read_like_the_python_script() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("playerbots.conf");
        std::fs::write(&p, "\u{feff}# comment = 1\nAiPlayerbot.Enabled = 0 # off\n  Key.Two=\"quoted\"\nnot a line\nEmpty =\n").unwrap();
        let o = options(&p);
        assert_eq!(o.get("AiPlayerbot.Enabled").map(String::as_str), Some("0"));
        assert_eq!(o.get("Key.Two").map(String::as_str), Some("quoted"));
        assert_eq!(o.get("Empty").map(String::as_str), Some(""));
        assert!(!o.contains_key("#"));
        assert!(!enabled(&o, "AiPlayerbot.Enabled"));
        assert!(enabled(&o, "Missing"));
    }

    #[test]
    fn both_bot_modules_on_is_refused_and_squid_gets_a_long_startup() {
        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("Core/configs/modules");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("mod_coa_playerbots.conf"), "CoaBots.Enable = 1\n").unwrap();
        std::fs::write(m.join("playerbots.conf.dist"), "AiPlayerbot.Enabled = 1\n").unwrap();
        assert!(validate_bots(d.path()).is_err());
        assert_eq!(startup_timeout(d.path()), 1500);
        std::fs::write(m.join("playerbots.conf"), "AiPlayerbot.Enabled = 0\n").unwrap();
        validate_bots(d.path()).unwrap();
        assert_eq!(startup_timeout(d.path()), 180);
    }

    #[test]
    fn options_are_replaced_or_appended() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("playerbots.conf");
        std::fs::write(&p, "A = 1\r\n  Playerbots.Updates.EnableDatabases = 1\r\n").unwrap();
        set_options(&p, &[("Playerbots.Updates.EnableDatabases", "0".into()), ("PlayerbotsDatabaseInfo", "\"x\"".into())]).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "A = 1\nPlayerbots.Updates.EnableDatabases = 0\n\nPlayerbotsDatabaseInfo = \"x\"\n");
    }

    #[test]
    fn base_tables_are_found() {
        let t = created_tables("CREATE TABLE IF NOT EXISTS `ai_playerbot_names` (\n id int);\ncreate table playerbots_random_bots(x int);\n-- CREATE INDEX i ON t(x);");
        assert_eq!(t, ["ai_playerbot_names".to_string(), "playerbots_random_bots".to_string()].into_iter().collect());
    }

    #[test]
    fn a_secondary_world_uses_the_primary_package() {
        assert_eq!(package_root(Path::new("C:/S/.realms/secondary")), Path::new("C:/S"));
        assert_eq!(package_root(Path::new("C:/S")), Path::new("C:/S"));
    }
}
