//! Optional server modules. They are compiled into the server; what the owner decides is whether each one is on and how it is
//! configured, which lives in `Core/configs/modules/<module>.conf` (the documented defaults are in the `.dist` beside it).
//! The list of known modules is a bundled catalog, so a module is added to the Manager by adding a few lines to it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::parser::ConfFile;
use crate::error::{Error, Result};
use crate::fsx;

static CATALOG: &str = include_str!("../../../modules/catalog.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// language code -> text; "en" is always there.
    pub description: BTreeMap<String, String>,
    pub repo: String,
    /// File in `Core/configs/modules` that holds this module's settings.
    pub conf: String,
    /// The setting that switches the whole module on or off (1 / 0).
    pub enable_key: String,
}

#[derive(Deserialize)]
struct Catalog {
    modules: Vec<Entry>,
}

pub fn catalog() -> Vec<Entry> {
    serde_json::from_str::<Catalog>(CATALOG).map(|c| c.modules).unwrap_or_default()
}

fn entry(id: &str) -> Result<Entry> {
    catalog().into_iter().find(|e| e.id == id).ok_or_else(|| Error::Invalid("That module is not known to the Manager.".into()))
}

fn dir(root: &Path) -> PathBuf {
    root.join("Core/configs/modules")
}

fn read(path: &Path) -> Option<ConfFile> {
    fs::read(path).ok().and_then(|b| ConfFile::parse_bytes(&b).ok())
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().trim_matches('"').to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

#[derive(Debug, Serialize)]
pub struct ModuleView {
    pub id: String,
    pub name: String,
    pub description: BTreeMap<String, String>,
    pub repo: String,
    /// The module's configuration is present on this server (otherwise it is not part of this server build).
    pub installed: bool,
    pub enabled: bool,
}

pub fn list(root: &Path) -> Vec<ModuleView> {
    catalog()
        .into_iter()
        .map(|e| {
            let conf = dir(root).join(&e.conf);
            let dist = dir(root).join(format!("{}.dist", e.conf));
            let installed = conf.is_file() || dist.is_file();
            // the active file decides; without one the documented default is what the server will use
            let enabled = read(&conf).or_else(|| read(&dist)).and_then(|c| c.get(&e.enable_key).map(truthy)).unwrap_or(false);
            ModuleView { id: e.id, name: e.name, description: e.description, repo: e.repo, installed, enabled }
        })
        .collect()
}

/// The active file, created from its `.dist` when it does not exist yet.
fn active_file(root: &Path, e: &Entry) -> Result<PathBuf> {
    let conf = dir(root).join(&e.conf);
    if !conf.is_file() {
        let dist = dir(root).join(format!("{}.dist", e.conf));
        if !dist.is_file() {
            return Err(Error::Invalid("This module is not part of this server.".into()));
        }
        fsx::atomic_write(&conf, &fs::read(&dist)?)?;
    }
    Ok(conf)
}

fn backup(meta: &Path, conf: &Path) -> Result<()> {
    let name = conf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let to = meta.join("backups").join("modules").join(format!("{name}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    fsx::atomic_write(&to, &fs::read(conf)?)
}

pub fn set_enabled(root: &Path, meta: &Path, id: &str, on: bool) -> Result<()> {
    let e = entry(id)?;
    let conf = active_file(root, &e)?;
    let mut file = ConfFile::parse_bytes(&fs::read(&conf)?)?;
    if file.get(&e.enable_key).map(truthy) == Some(on) {
        return Ok(());
    }
    backup(meta, &conf)?;
    file.set(&e.enable_key, if on { "1" } else { "0" }, &["Changed by CoA Server Manager"]);
    fsx::atomic_write(&conf, file.to_text().as_bytes())?;
    tracing::info!(module = id, on, "module switched");
    Ok(())
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub default: Option<String>,
    /// The documentation the module ships above the setting.
    pub doc: String,
}

/// Every active setting of the module with its current value, its documented default and its description.
pub fn settings(root: &Path, id: &str) -> Result<Vec<Setting>> {
    let e = entry(id)?;
    let conf = dir(root).join(&e.conf);
    let dist = read(&dir(root).join(format!("{}.dist", e.conf)));
    let active = read(&conf).or_else(|| dist.clone()).ok_or_else(|| Error::Invalid("This module is not part of this server.".into()))?;
    Ok(active
        .entries()
        .map(|(k, v)| Setting {
            key: k.to_string(),
            value: v.to_string(),
            default: dist.as_ref().and_then(|d| d.get(k)).map(str::to_string),
            doc: dist.as_ref().map(|d| d.doc_for(k)).filter(|d| !d.is_empty()).unwrap_or_else(|| active.doc_for(k)).join(" "),
        })
        .collect())
}

/// Change values of settings the module already has. A value is one line of at most 500 characters.
pub fn save_settings(root: &Path, meta: &Path, id: &str, changes: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let e = entry(id)?;
    let conf = active_file(root, &e)?;
    let mut file = ConfFile::parse_bytes(&fs::read(&conf)?)?;
    let mut changed = Vec::new();
    for (key, value) in changes {
        let value = value.trim();
        if file.get(key).is_none() {
            return Err(Error::Invalid(format!("{key} is not a setting of this module.")));
        }
        if value.is_empty() || value.len() > 500 || value.chars().any(char::is_control) {
            return Err(Error::Invalid(format!("The value of {key} must be one line of text.")));
        }
        if file.get(key).map(str::trim) != Some(value) {
            changed.push((key.clone(), value.to_string()));
        }
    }
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    backup(meta, &conf)?;
    for (k, v) in &changed {
        file.set(k, v, &[]);
    }
    fsx::atomic_write(&conf, file.to_text().as_bytes())?;
    Ok(changed.into_iter().map(|(k, _)| k).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(d: &Path) -> (PathBuf, PathBuf) {
        let (root, meta) = (d.join("srv"), d.join("meta"));
        let m = dir(&root);
        fs::create_dir_all(&m).unwrap();
        fs::create_dir_all(&meta).unwrap();
        fs::write(m.join("war_games.conf.dist"), "# Turns War Games on.\r\n# Second line.\r\nWarGames.Enable = 1\r\n\r\n# How long a challenge stays open.\r\nWarGames.ChallengeSeconds = 60\r\n").unwrap();
        (root, meta)
    }

    #[test]
    fn the_bundled_catalog_is_complete() {
        let c = catalog();
        assert!(c.len() >= 5);
        for e in &c {
            assert!(e.description.contains_key("en") && e.description.len() == 5, "{} has all five descriptions", e.id);
            assert!(e.repo.starts_with("https://github.com/Corfirean/") && e.conf.ends_with(".conf") && e.enable_key.ends_with(".Enable"), "{}", e.id);
        }
        let mut ids: Vec<_> = c.iter().map(|e| e.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len(), "ids are unique");
    }

    #[test]
    fn a_module_is_listed_as_installed_with_its_documented_default_until_a_file_exists() {
        let d = tempfile::tempdir().unwrap();
        let (root, _) = server(d.path());
        let wg = list(&root).into_iter().find(|m| m.id == "war-games").unwrap();
        assert!(wg.installed && wg.enabled, "the .dist says 1");
        assert!(!list(&root).into_iter().find(|m| m.id == "spellbook").unwrap().installed, "no configuration of that module on this server");
    }

    #[test]
    fn switching_a_module_creates_its_file_saves_the_old_one_and_changes_only_that_line() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        set_enabled(&root, &meta, "war-games", false).unwrap();
        let conf = fs::read_to_string(dir(&root).join("war_games.conf")).unwrap();
        assert!(conf.contains("WarGames.Enable = 0") && conf.contains("WarGames.ChallengeSeconds = 60"), "{conf}");
        assert!(!list(&root).into_iter().find(|m| m.id == "war-games").unwrap().enabled);
        set_enabled(&root, &meta, "war-games", false).unwrap();
        assert_eq!(fs::read_dir(meta.join("backups/modules")).unwrap().count(), 1, "no change, no backup");
        set_enabled(&root, &meta, "war-games", true).unwrap();
        assert!(fs::read_to_string(dir(&root).join("war_games.conf")).unwrap().contains("WarGames.Enable = 1"));
        assert!(set_enabled(&root, &meta, "spellbook", true).is_err(), "not part of this server");
        assert!(set_enabled(&root, &meta, "nonsense", true).is_err());
    }

    #[test]
    fn settings_show_the_documentation_and_only_known_keys_with_one_line_values_can_be_saved() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let s = settings(&root, "war-games").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], Setting { key: "WarGames.ChallengeSeconds".into(), value: "60".into(), default: Some("60".into()), doc: "How long a challenge stays open.".into() });
        assert_eq!(s[0].doc, "Turns War Games on. Second line.");

        let ok = BTreeMap::from([("WarGames.ChallengeSeconds".to_string(), "90".to_string())]);
        assert_eq!(save_settings(&root, &meta, "war-games", &ok).unwrap(), ["WarGames.ChallengeSeconds"]);
        assert!(settings(&root, "war-games").unwrap()[1].value == "90");
        assert!(save_settings(&root, &meta, "war-games", &ok).unwrap().is_empty(), "same value: nothing to do");
        for bad in [("Other.Key", "1"), ("WarGames.ChallengeSeconds", ""), ("WarGames.ChallengeSeconds", "1\nInjected = 1")] {
            let c = BTreeMap::from([(bad.0.to_string(), bad.1.to_string())]);
            assert!(save_settings(&root, &meta, "war-games", &c).is_err(), "{bad:?}");
        }
        assert!(!fs::read_to_string(dir(&root).join("war_games.conf")).unwrap().contains("Injected"));
    }
}
