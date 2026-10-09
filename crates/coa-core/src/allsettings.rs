//! Every setting of the world server's configuration, for people who want more than the curated list: the documented
//! settings come from `worldserver.conf.dist` (the file that ships with the server and explains each one), the current
//! values from the configuration the next start will use. Anything that holds a password, a port, an address, a folder or
//! the database connection is left out; the Manager owns those.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::config::parser::ConfFile;
use crate::config::{take_snapshot, targets, Scope};
use crate::error::{Error, Result};
use crate::fsx;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Item {
    pub key: String,
    /// What the server uses now (the documented default when the file does not mention the setting).
    pub value: String,
    pub default: String,
    pub doc: String,
    /// The file sets it to something other than the documented default.
    pub changed: bool,
}

/// Settings the Manager (or the launcher) owns.
fn hidden(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("password")
        || k.contains("databaseinfo")
        || k.ends_with("port")
        || k.starts_with("ra.")
        || k.starts_with("logger.")
        || k.starts_with("appender.")
        || k.starts_with("updates.")
        || k.starts_with("logindatabase.")
        || k.starts_with("worlddatabase.")
        || k.starts_with("characterdatabase.")
        || [
            "bindip",
            "realmid",
            "datadir",
            "logsdir",
            "tempdir",
            "sourcedirectory",
            "mysqlexecutable",
            "cmakecommand",
            "builddirectory",
            "pidfile",
            "vmapdir",
            "mmapdir",
        ]
        .contains(&k.as_str())
}

fn doc_of(block: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for l in block {
        let body = l.trim_start_matches('#').trim();
        if body.is_empty() {
            continue;
        }
        // the lines that only name the settings the block describes
        let named = l.starts_with("#    ")
            && !l.starts_with("#     ")
            && body
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if named {
            continue;
        }
        parts.push(body.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    let mut doc = parts.join(" ");
    if doc.chars().count() > 900 {
        doc = doc.chars().take(900).collect::<String>() + "…";
    }
    doc
}

/// The documented settings of a `.dist` file: (key, default, description), in file order.
fn parse_dist(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut block: Vec<String> = Vec::new();
    let mut in_comment = false;
    let mut doc = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            if !in_comment {
                block.clear();
                in_comment = true;
            }
            block.push(line.trim_end().to_string());
            continue;
        }
        if in_comment {
            doc = doc_of(&block);
            in_comment = false;
        }
        if t.is_empty() || t.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let k = k.trim();
            if !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                out.push((k.to_string(), v.trim().to_string(), doc.clone()));
            }
        }
    }
    out
}

fn dist_path(root: &Path) -> std::path::PathBuf {
    root.join("Core/configs/worldserver.conf.dist")
}

pub fn list(root: &Path) -> Result<Vec<Item>> {
    let dist = fs::read_to_string(dist_path(root)).map_err(|_| {
        Error::Invalid(
            "The documented settings file (worldserver.conf.dist) was not found on this server."
                .into(),
        )
    })?;
    let t = targets(root, Scope::Server)?;
    let active = ConfFile::parse_bytes(&fs::read(&t.read)?)?;
    Ok(parse_dist(&dist)
        .into_iter()
        .filter(|(k, _, _)| !hidden(k))
        .map(|(key, default, doc)| {
            let value = active
                .get(&key)
                .map(|v| v.trim().to_string())
                .unwrap_or_else(|| default.clone());
            let changed = value != default;
            Item {
                key,
                value,
                default,
                doc,
                changed,
            }
        })
        .collect())
}

/// Change settings that exist in the documented file. A value is one line of at most 500 characters. Returns the keys
/// that actually changed; the previous files are snapshotted first (and can be restored from Settings).
pub fn save(root: &Path, meta: &Path, changes: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let known: BTreeMap<String, String> =
        parse_dist(&fs::read_to_string(dist_path(root)).map_err(|_| {
            Error::Invalid("worldserver.conf.dist was not found on this server.".into())
        })?)
        .into_iter()
        .map(|(k, d, _)| (k, d))
        .collect();
    let t = targets(root, Scope::Server)?;
    let mut originals: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
    let mut confs: Vec<ConfFile> = Vec::new();
    for p in &t.writes {
        let bytes = fs::read(p)?;
        confs.push(ConfFile::parse_bytes(&bytes)?);
        originals.push((p.clone(), bytes));
    }
    let mut changed = Vec::new();
    for (key, value) in changes {
        let value = value.trim();
        if hidden(key) || !known.contains_key(key) {
            return Err(Error::Invalid(format!(
                "{key} is not a setting that can be changed here."
            )));
        }
        if value.is_empty() || value.len() > 500 || value.chars().any(char::is_control) {
            return Err(Error::Invalid(format!(
                "The value of {key} must be one line of text."
            )));
        }
        let same = confs.iter().all(|c| match c.get(key) {
            Some(v) => v.trim() == value,
            None => known[key] == value,
        });
        if !same {
            changed.push((key.clone(), value.to_string()));
        }
    }
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    for c in &mut confs {
        for (k, v) in &changed {
            c.set(k, v, &["Added by CoA Server Manager"]);
        }
    }
    let texts: Vec<String> = confs.iter().map(ConfFile::to_text).collect();
    for text in &texts {
        let reread = ConfFile::parse(text);
        for (k, v) in &changed {
            if reread.get(k).map(str::trim) != Some(v.as_str()) {
                return Err(Error::Invalid(format!("internal check failed for {k}")));
            }
        }
    }
    take_snapshot(
        meta,
        Scope::Server,
        &format!(
            "before changing {} setting(s) in the full list",
            changed.len()
        ),
        &originals,
    )?;
    let mut written = 0;
    for (i, (path, _)) in originals.iter().enumerate() {
        if let Err(e) = fsx::atomic_write(path, texts[i].as_bytes()) {
            for (p, bytes) in originals.iter().take(written) {
                let _ = fsx::atomic_write(p, bytes);
            }
            return Err(e);
        }
        written += 1;
    }
    Ok(changed.into_iter().map(|(k, _)| k).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIST: &str = "[worldserver]\r\n\r\n###################\r\n# SECTION\r\n###################\r\n\r\n#\r\n#    Rate.Drop.Item.Epic\r\n#    Rate.Drop.Item.Rare\r\n#        Description: Drop rates by quality.\r\n#        Default:     1\r\n#\r\n\r\nRate.Drop.Item.Rare = 1\r\nRate.Drop.Item.Epic = 1\r\n\r\n#\r\n#    LoginDatabaseInfo\r\n#        Description: secret\r\n#\r\n\r\nLoginDatabaseInfo = \"127.0.0.1;3306;a;b;c\"\r\n\r\n#\r\n#    Instance.ResetTimeHour\r\n#        Description: Hour of the reset.\r\n#        Important:   Use 0-23.\r\n#        Default:     4\r\n#\r\n\r\nInstance.ResetTimeHour = 4\r\n";

    fn server(d: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let (root, meta) = (d.join("srv"), d.join("meta"));
        fs::create_dir_all(root.join("Core/configs")).unwrap();
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::create_dir_all(&meta).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf.dist"), DIST).unwrap();
        let conf = "[worldserver]\r\nRate.Drop.Item.Epic = 3\r\nLoginDatabaseInfo = \"x\"\r\n";
        fs::write(root.join("Settings/worldserver.conf.template"), conf).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), conf).unwrap();
        (root, meta)
    }

    #[test]
    fn the_documentation_of_a_block_is_shared_by_its_settings_and_secrets_are_left_out() {
        let d = tempfile::tempdir().unwrap();
        let (root, _) = server(d.path());
        let items = list(&root).unwrap();
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "Rate.Drop.Item.Rare",
                "Rate.Drop.Item.Epic",
                "Instance.ResetTimeHour"
            ],
            "the database connection is hidden"
        );
        assert_eq!(
            items[0].doc,
            "Description: Drop rates by quality. Default: 1"
        );
        assert_eq!(items[0].doc, items[1].doc);
        assert_eq!(
            items[2].doc,
            "Description: Hour of the reset. Important: Use 0-23. Default: 4"
        );
        assert!(
            !items[0].changed && items[0].value == "1",
            "absent from the file: documented default"
        );
        assert!(items[1].changed && items[1].value == "3");
    }

    #[test]
    fn only_documented_visible_settings_with_one_line_values_can_be_saved_and_both_files_change() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let ok = BTreeMap::from([("Instance.ResetTimeHour".to_string(), "6".to_string())]);
        assert_eq!(save(&root, &meta, &ok).unwrap(), ["Instance.ResetTimeHour"]);
        for f in [
            "Settings/worldserver.conf.template",
            "Core/configs/worldserver.conf",
        ] {
            let t = fs::read_to_string(root.join(f)).unwrap();
            assert!(
                t.contains("Instance.ResetTimeHour = 6") && t.contains("Rate.Drop.Item.Epic = 3"),
                "{f}: {t}"
            );
        }
        assert!(
            save(&root, &meta, &ok).unwrap().is_empty(),
            "same value: nothing to do"
        );
        let same_as_default =
            BTreeMap::from([("Rate.Drop.Item.Rare".to_string(), "1".to_string())]);
        assert!(
            save(&root, &meta, &same_as_default).unwrap().is_empty(),
            "the default is already in effect"
        );
        for bad in [
            ("LoginDatabaseInfo", "\"evil\""),
            ("Nope.Key", "1"),
            ("Rate.Drop.Item.Rare", ""),
            ("Rate.Drop.Item.Rare", "1\n2"),
        ] {
            assert!(
                save(
                    &root,
                    &meta,
                    &BTreeMap::from([(bad.0.to_string(), bad.1.to_string())])
                )
                .is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn what_belongs_to_the_manager_is_hidden() {
        for k in [
            "LoginDatabaseInfo",
            "WorldServerPort",
            "BindIP",
            "Ra.Enable",
            "Ra.Password",
            "Logger.root",
            "Appender.Console",
            "DataDir",
            "Updates.EnableDatabases",
            "LoginDatabase.WorkerThreads",
            "InstanceServerPort",
        ] {
            assert!(hidden(k), "{k}");
        }
        for k in [
            "Rate.XP.Kill",
            "PlayerLimit",
            "SupportEnabled",
            "Instance.ResetTimeHour",
            "Warden.Enabled",
            "CharacterCreating.Disabled",
        ] {
            assert!(!hidden(k), "{k}");
        }
    }
}
