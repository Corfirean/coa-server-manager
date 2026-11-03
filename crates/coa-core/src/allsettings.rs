//! Every setting of the world server's configuration, for people who want more than the curated list: the documented
//! settings come from `worldserver.conf.dist` and `modules/coa.conf.dist`, the current
//! values from the configuration the next start will use. Anything that holds a password, a port, an address, a folder or
//! the database connection is left out; the Manager owns those.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::config::parser::ConfFile;
use crate::config::{take_snapshot, targets, Scope, Targets};
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
        || k == "console.enable"
        || k.starts_with("ra.")
        || k.starts_with("logger.")
        || k.starts_with("appender.")
        || k.starts_with("updates.")
        || k.starts_with("logindatabase.")
        || k.starts_with("worlddatabase.")
        || k.starts_with("characterdatabase.")
        || [
            "coa.classmodel",
            "coa.realmtype",
            "coa.gamemodemask",
            "coa.mapclass10towarrior",
            "coa.clientbooleanconfigs",
        ]
        .contains(&k.as_str())
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

fn sources(root: &Path) -> Result<Vec<(std::path::PathBuf, Targets)>> {
    let mut result = vec![(dist_path(root), targets(root, Scope::Server)?)];
    let dist = root.join("Core/configs/modules/coa.conf.dist");
    if dist.is_file() {
        let active = root.join("Core/configs/modules/coa.conf");
        result.push((
            dist,
            Targets {
                read: active.clone(),
                writes: vec![active],
                generated: None,
            },
        ));
    }
    Ok(result)
}

pub fn list(root: &Path) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for (dist_path, t) in sources(root)? {
        let dist = fs::read_to_string(&dist_path)?;
        let active = ConfFile::parse_bytes(&if t.read.exists() {
            fs::read(&t.read)?
        } else {
            dist.as_bytes().to_vec()
        })?;
        items.extend(
            parse_dist(&dist)
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
                .collect::<Vec<_>>(),
        );
    }
    Ok(items)
}

/// Change settings that exist in the documented file. A value is one line of at most 500 characters. Returns the keys
/// that actually changed; the previous files are snapshotted first (and can be restored from Settings).
pub fn save(root: &Path, meta: &Path, changes: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let _lock = crate::update::operation_lock(meta)?;
    crate::update::ensure_recovered(meta)?;
    let sources = sources(root)?;
    let mut known = BTreeMap::new();
    let mut originals: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
    let mut confs: Vec<ConfFile> = Vec::new();
    let mut owners = Vec::new();
    for (index, (dist, t)) in sources.iter().enumerate() {
        let defaults = fs::read(dist)?;
        for (key, default, _) in parse_dist(&String::from_utf8_lossy(&defaults)) {
            if known.insert(key.clone(), (default, index)).is_some() {
                return Err(Error::Invalid(format!(
                    "{key} is documented in more than one configuration file."
                )));
            }
        }
        for p in &t.writes {
            let bytes = if p.exists() {
                fs::read(p)?
            } else {
                defaults.clone()
            };
            confs.push(ConfFile::parse_bytes(&bytes)?);
            originals.push((p.clone(), bytes));
            owners.push(index);
        }
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
        let (default, owner) = &known[key];
        if default.parse::<f64>().is_ok() && !value.parse::<f64>().is_ok_and(f64::is_finite)
            || key == "CoA.LegacyMountFlight" && !matches!(value, "0" | "1")
        {
            return Err(Error::Invalid(format!(
                "The value of {key} must be a number matching its documented default."
            )));
        }
        let same = confs
            .iter()
            .zip(&owners)
            .filter(|(_, index)| *index == owner)
            .all(|(c, _)| match c.get(key) {
                Some(v) => v.trim() == value,
                None => default == value,
            });
        if !same {
            changed.push((key.clone(), value.to_string()));
        }
    }
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    for (c, owner) in confs.iter_mut().zip(&owners) {
        for (k, v) in &changed {
            if known[k].1 == *owner {
                c.set(k, v, &["Added by CoA Server Manager"]);
            }
        }
    }
    let texts: Vec<String> = confs.iter().map(ConfFile::to_text).collect();
    for (text, owner) in texts.iter().zip(&owners) {
        let reread = ConfFile::parse(text);
        for (k, v) in &changed {
            if known[k].1 == *owner && reread.get(k).map(str::trim) != Some(v.as_str()) {
                return Err(Error::Invalid(format!("internal check failed for {k}")));
            }
        }
    }
    let affected: std::collections::BTreeSet<_> =
        changed.iter().map(|(key, _)| known[key].1).collect();
    let (originals, texts): (Vec<_>, Vec<_>) = originals
        .into_iter()
        .zip(texts)
        .zip(owners)
        .filter(|(_, owner)| affected.contains(owner))
        .map(|(pair, _)| pair)
        .unzip();
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

    #[test]
    fn unrelated_saves_do_not_create_coa_files_and_existing_realm_values_are_preserved() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let module = root.join("Core/configs/modules");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join("coa.conf.dist"),
            "CoA.LegacyMountFlight = 0\nCoA.ClassModel = \"coa\"\n",
        )
        .unwrap();
        save(
            &root,
            &meta,
            &BTreeMap::from([("Rate.Drop.Item.Rare".into(), "1.5".into())]),
        )
        .unwrap();
        assert!(!module.join("coa.conf").exists());
        fs::write(
            module.join("coa.conf"),
            "# Reborn realm\nCoA.ClassModel = \"wcr\"\nCustom.Preserved = 42\n",
        )
        .unwrap();
        assert!(save(
            &root,
            &meta,
            &BTreeMap::from([("CoA.ClassModel".into(), "\"coa\"".into())])
        )
        .is_err());
        save(
            &root,
            &meta,
            &BTreeMap::from([("CoA.LegacyMountFlight".into(), "1".into())]),
        )
        .unwrap();
        let actual = fs::read_to_string(module.join("coa.conf")).unwrap();
        assert!(
            actual.contains("CoA.ClassModel = \"wcr\"") && actual.contains("Custom.Preserved = 42")
        );
        assert!(actual.contains("# Reborn realm"));
    }

    #[test]
    fn coa_settings_are_found_saved_to_the_module_and_snapshotted_without_profile_overrides() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let module = root.join("Core/configs/modules");
        fs::create_dir_all(&module).unwrap();
        let defaults = "[worldserver]\n# Mount flight rules\nCoA.LegacyMountFlight = 0\nCoA.ClassModel = \"coa\"\nCoA.RealmType = \"live\"\n";
        fs::write(module.join("coa.conf.dist"), defaults).unwrap();
        let original_world = fs::read(root.join("Core/configs/worldserver.conf")).unwrap();
        let items = list(&root).unwrap();
        assert!(items
            .iter()
            .any(|i| i.key == "CoA.LegacyMountFlight" && i.value == "0"));
        assert!(!items
            .iter()
            .any(|i| i.key == "CoA.ClassModel" || i.key == "CoA.RealmType"));
        for value in ["2", "invalid", "1\nInjected = 1"] {
            assert!(save(
                &root,
                &meta,
                &BTreeMap::from([("CoA.LegacyMountFlight".into(), value.into())])
            )
            .is_err());
        }
        assert!(!module.join("coa.conf").exists());
        save(
            &root,
            &meta,
            &BTreeMap::from([
                ("CoA.LegacyMountFlight".into(), "1".into()),
                ("Rate.Drop.Item.Rare".into(), "1.5".into()),
            ]),
        )
        .unwrap();
        assert!(fs::read_to_string(module.join("coa.conf"))
            .unwrap()
            .contains("CoA.LegacyMountFlight = 1"));
        let world = fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap();
        assert!(!world.contains("CoA.LegacyMountFlight"));
        assert!(world.contains("Rate.Drop.Item.Rare = 1.5"));
        assert!(!String::from_utf8(original_world)
            .unwrap()
            .contains("CoA.LegacyMountFlight"));
        assert_eq!(
            list(&root)
                .unwrap()
                .iter()
                .find(|i| i.key == "CoA.LegacyMountFlight")
                .unwrap()
                .value,
            "1"
        );
        assert!(!crate::config::list_snapshots(&meta).is_empty());
        let saved = crate::config::list_snapshots(&meta).remove(0);
        crate::config::restore_snapshot(&meta, &saved.id).unwrap();
        assert_eq!(
            list(&root)
                .unwrap()
                .iter()
                .find(|i| i.key == "CoA.LegacyMountFlight")
                .unwrap()
                .value,
            "0"
        );
    }

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
            "Console.Enable",
            "console.enable",
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
