//! The names players see in the realm list, chosen by the server owner.
//!
//! The launcher writes `acore_auth.realmlist.name` at every start, so a name typed into the database (or into
//! an old `manage.py`) did not last. The owner's choice lives in `Settings/realm-names.json` of the server's main
//! folder, keyed by realm id, and every start and every realm-list setup uses it; extra worlds read their main
//! folder's file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;

const FILE: &str = "Settings/realm-names.json";

/// `acore_auth.realmlist.name` is `varchar(32)`.
pub const MAX_CHARS: usize = 32;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Names {
    #[serde(default)]
    names: BTreeMap<u32, String>,
}

/// The folder that owns the names: an extra world (`<server>/.realms/<name>`) belongs to `<server>`.
fn main_folder(root: &Path) -> PathBuf {
    if crate::launcher::settings::is_extra_world(root) {
        if let Some(main) = root.parent().and_then(Path::parent) {
            return main.to_path_buf();
        }
    }
    root.to_path_buf()
}

fn read(root: &Path) -> Names {
    let path = main_folder(root).join(FILE);
    if !path.is_file() {
        return Names::default();
    }
    match fsx::read_json(&path) {
        Ok(names) => names,
        Err(error) => {
            tracing::warn!(%error, "realm names could not be read; the default names are used");
            Names::default()
        }
    }
}

/// A trimmed name the realm list can hold: 1 to 32 characters, no control characters.
pub fn validate(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("The realm name cannot be empty.".into()));
    }
    if name.chars().count() > MAX_CHARS {
        return Err(Error::Invalid(format!(
            "The realm name can have at most {MAX_CHARS} characters."
        )));
    }
    // The game client stores the realm as `SET realmName "<name>"` in Config.wtf.
    if name.chars().any(|c| c.is_control() || c == '"') {
        return Err(Error::Invalid(
            "The realm name cannot contain quotation marks or control characters.".into(),
        ));
    }
    Ok(name.to_string())
}

/// The owner's name for realm `id`, if one was chosen.
pub fn custom(root: &Path, id: u32) -> Option<String> {
    read(root)
        .names
        .remove(&id)
        .and_then(|name| validate(&name).ok())
}

/// The name realm `id` shows: the owner's choice, or `default`.
pub fn name_or(root: &Path, id: u32, default: &str) -> String {
    custom(root, id).unwrap_or_else(|| default.to_string())
}

/// The name the realm of `mode` shows.
pub fn for_mode(root: &Path, mode: crate::realms::Mode) -> String {
    name_or(root, mode.realm_id(), mode.label())
}

/// Every chosen name.
pub fn all(root: &Path) -> BTreeMap<u32, String> {
    read(root)
        .names
        .into_iter()
        .filter_map(|(id, name)| validate(&name).ok().map(|name| (id, name)))
        .collect()
}

/// Choose a name for realm `id`; `None` (or an empty name) returns it to its default.
pub fn set(root: &Path, id: u32, name: Option<&str>) -> Result<Option<String>> {
    let mut names = read(root);
    let chosen = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => Some(validate(name)?),
        None => None,
    };
    match &chosen {
        Some(name) => names.names.insert(id, name.clone()),
        None => names.names.remove(&id),
    };
    fsx::atomic_write_json(&main_folder(root).join(FILE), &names)?;
    Ok(chosen)
}

/// A realm of this server and the name it shows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RealmName {
    pub id: u32,
    pub name: String,
    pub default_name: String,
    pub custom: bool,
}

/// The realms of a server with their names: Conquest of Azeroth (or the plain repack's "AzerothCore") and the
/// Wildcard and Warcraft Reborn realms that were created.
pub fn list(root: &Path) -> Result<Vec<RealmName>> {
    let main = main_folder(root);
    let mut realms: Vec<(u32, &'static str)> = Vec::new();
    if main.join("Settings/realm-profile.json").exists() {
        let state = crate::realms::state(&main)?;
        realms.extend(state.modes().into_iter().map(|m| (m.realm_id(), m.label())));
    } else {
        realms.push((1, "AzerothCore"));
    }
    realms.sort_unstable();
    realms.dedup();
    Ok(realms
        .into_iter()
        .map(|(id, default)| {
            let custom = custom(&main, id);
            RealmName {
                id,
                name: custom.clone().unwrap_or_else(|| default.to_string()),
                default_name: default.to_string(),
                custom: custom.is_some(),
            }
        })
        .collect())
}

/// Choose a name and, when the database runs, write it to the realm list at once (the auth server shows it
/// within its realm refresh interval). Returns the realms with their names.
pub fn rename(root: &Path, id: u32, name: Option<&str>) -> Result<Vec<RealmName>> {
    let main = main_folder(root);
    let known = list(&main)?;
    let realm = known
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| Error::Invalid(format!("This server has no realm {id}.")))?;
    let shown = set(&main, id, name)?.unwrap_or_else(|| realm.default_name.clone());
    crate::backup::with_running_database(&main, |db| {
        db.query(&format!(
            "UPDATE acore_auth.realmlist SET name={} WHERE id={id};",
            sql_literal(&shown)
        ))
        .map(|_| ())
    })?;
    list(&main)
}

/// `text` as a MySQL string literal (quotes and backslashes escaped).
pub fn sql_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated_trimmed_and_reset() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(name_or(d.path(), 1, "Conquest of Azeroth"), "Conquest of Azeroth");
        assert_eq!(set(d.path(), 1, Some("  Owner's Realm  ")).unwrap().as_deref(), Some("Owner's Realm"));
        assert_eq!(name_or(d.path(), 1, "x"), "Owner's Realm");
        assert!(set(d.path(), 2, Some(&"a".repeat(33))).is_err());
        assert!(set(d.path(), 2, Some("bad\nname")).is_err());
        assert_eq!(set(d.path(), 2, Some("Царство")).unwrap().as_deref(), Some("Царство"));
        assert_eq!(all(d.path()).len(), 2);
        assert_eq!(set(d.path(), 1, Some("   ")).unwrap(), None);
        assert_eq!(name_or(d.path(), 1, "Conquest of Azeroth"), "Conquest of Azeroth");
    }

    #[test]
    fn an_extra_world_reads_its_main_folders_names() {
        let d = tempfile::tempdir().unwrap();
        set(d.path(), 2, Some("Wild Side")).unwrap();
        let extra = d.path().join(".realms/secondary");
        std::fs::create_dir_all(&extra).unwrap();
        assert_eq!(custom(&extra, 2).as_deref(), Some("Wild Side"));
    }

    #[test]
    fn names_are_safe_in_sql() {
        assert_eq!(sql_literal(r"O'Neil \ realm"), r"'O''Neil \\ realm'");
    }
}
