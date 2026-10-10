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
    /// The name an older server build gave that file (used when `conf` is not there).
    #[serde(default)]
    pub alt_conf: Option<String>,
    /// The setting that switches the whole module on or off (1 / 0). Empty for a part that cannot be switched off.
    #[serde(default)]
    pub enable_key: String,
    /// Can be turned on and off here. A part the game client needs to talk to the server is only configured.
    #[serde(default = "yes")]
    pub switchable: bool,
    /// What the module is when its setting is not in the file: a server from before the setting existed runs it.
    #[serde(default = "yes")]
    pub default_on: bool,
    /// How far along it is: `early` (experimental, may change or break), `beta` (works, still being tested) or
    /// `release` (stable); `soon` is a place held for a module that is planned but not part of any build yet.
    #[serde(default = "release")]
    pub status: String,
    /// Which icon the page draws (a name the page knows).
    #[serde(default)]
    pub icon: String,
    /// A page of the Manager that only makes sense while the module is on (it is hidden while it is off).
    #[serde(default)]
    pub page: Option<String>,
    /// Not shown on the Modules page; its settings are offered elsewhere (a server setting card).
    #[serde(default)]
    pub hidden: bool,
}

fn yes() -> bool {
    true
}

fn release() -> String {
    "release".into()
}

#[derive(Deserialize)]
struct Catalog {
    modules: Vec<Entry>,
}

pub fn catalog() -> Vec<Entry> {
    serde_json::from_str::<Catalog>(CATALOG)
        .map(|c| c.modules)
        .unwrap_or_default()
}

fn entry_in(cat: &[Entry], id: &str) -> Result<Entry> {
    cat.iter()
        .find(|e| e.id == id)
        .cloned()
        .ok_or_else(|| Error::Invalid("That module is not known to the Manager.".into()))
}

fn dir(root: &Path) -> PathBuf {
    root.join("Core/configs/modules")
}

fn read(path: &Path) -> Option<ConfFile> {
    fs::read(path)
        .ok()
        .and_then(|b| ConfFile::parse_bytes(&b).ok())
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().trim_matches('"').to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Debug, Serialize)]
pub struct ModuleView {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: BTreeMap<String, String>,
    pub repo: String,
    /// The module's configuration is present on this server (otherwise it is not part of this server build).
    pub installed: bool,
    /// At least one editable setting besides the master switch exists in this server build.
    pub has_settings: bool,
    pub enabled: bool,
    pub switchable: bool,
    pub status: String,
    pub icon: String,
    pub page: Option<String>,
    pub hidden: bool,
    pub compatibility: String,
}

pub fn list(root: &Path) -> Vec<ModuleView> {
    list_in(&catalog(), root)
}

/// The configuration file name this server uses for the module: the current one, or the name an older build gave it.
fn conf_name(root: &Path, e: &Entry) -> String {
    let there =
        |n: &str| dir(root).join(n).is_file() || dir(root).join(format!("{n}.dist")).is_file();
    if there(&e.conf) {
        return e.conf.clone();
    }
    match &e.alt_conf {
        Some(alt) if there(alt) => alt.clone(),
        _ => e.conf.clone(),
    }
}

fn list_in(cat: &[Entry], root: &Path) -> Vec<ModuleView> {
    let wildcard = crate::realms::state(root)
        .map(|s| s.active == crate::realms::Mode::Wildcard)
        .unwrap_or(true);
    cat.iter()
        .cloned()
        .map(|e| {
            let name = conf_name(root, &e);
            let conf = dir(root).join(&name);
            let dist = dir(root).join(format!("{name}.dist"));
            let installed = conf.is_file() || dist.is_file();
            let has_settings = installed
                && settings_in(cat, root, &e.id)
                    .map(|settings| settings.iter().any(|s| s.key != e.enable_key))
                    .unwrap_or(false);
            // the active file decides; without one the documented default is what the server will use
            let enabled = if matches!(e.id.as_str(), "companions" | "playerbots") {
                bot_enabled(root, &e).unwrap_or(true)
            } else {
                !e.switchable
                    || read(&conf)
                        .or_else(|| read(&dist))
                        .and_then(|c| c.get(&e.enable_key).map(truthy))
                        .unwrap_or(e.default_on)
            };
            let blocked = wildcard && matches!(e.id.as_str(), "companions" | "playerbots");
            let version = if e.id == "playerbots" {
                crate::squid::release(root).map(|release| {
                    [
                        release.tag,
                        release
                            .commit
                            .map(|commit| commit.chars().take(8).collect()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
                })
            } else {
                None
            };
            ModuleView {
                version,
                compatibility: if blocked {
                    "unsupported"
                } else if wildcard && e.id != "client-compat" {
                    "experimental"
                } else {
                    "compatible"
                }
                .into(),
                id: e.id,
                name: e.name,
                description: e.description,
                repo: e.repo,
                installed,
                has_settings,
                enabled: enabled && !blocked,
                switchable: e.switchable && !blocked,
                status: e.status,
                icon: e.icon,
                page: e.page,
                hidden: e.hidden,
            }
        })
        .collect()
}

/// The active file, created from its `.dist` when it does not exist yet.
fn active_file(root: &Path, e: &Entry) -> Result<PathBuf> {
    let name = conf_name(root, e);
    let conf = dir(root).join(&name);
    if !conf.is_file() {
        let dist = dir(root).join(format!("{name}.dist"));
        if !dist.is_file() {
            return Err(Error::Invalid(
                "This module is not part of this server.".into(),
            ));
        }
        fsx::atomic_write(&conf, &fs::read(&dist)?)?;
    }
    Ok(conf)
}

fn backup(meta: &Path, conf: &Path) -> Result<()> {
    let name = conf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let to = meta.join("backups").join("modules").join(format!(
        "{name}-{}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    fsx::atomic_write(&to, &fs::read(conf)?)
}

fn require_bot_server_stopped(root: &Path) -> Result<()> {
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if observed.world.state != crate::process::ServiceState::Stopped
        || crate::multiworld::is_running(root)
    {
        return Err(Error::Invalid(
            "Stop the server before changing the bot system.".into(),
        ));
    }
    Ok(())
}

pub fn set_enabled(root: &Path, meta: &Path, id: &str, on: bool) -> Result<()> {
    let _update_lock = crate::update::operation_lock(meta)?;
    crate::update::ensure_recovered(meta)?;
    let _lock = if matches!(id, "companions" | "playerbots") {
        require_bot_server_stopped(root)?;
        fs::create_dir_all(root.join(".state"))?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".state/control.lock"))?;
        if !fs4::fs_std::FileExt::try_lock_exclusive(&lock)? {
            return Err(Error::Invalid(
                "Another start/stop action is in progress.".into(),
            ));
        }
        require_bot_server_stopped(root)?;
        Some(lock)
    } else {
        None
    };
    if on {
        crate::realms::guard_module(root, id)?;
    }
    if id == "custom-races" {
        if let Ok(meta_content) = fs::read_to_string(meta.join("install.json")) {
            if let Ok(install_meta) =
                serde_json::from_str::<crate::registry::InstallMeta>(&meta_content)
            {
                if let Some(cp) = install_meta.client_path {
                    let client_path = PathBuf::from(cp);
                    if client_path.is_dir() {
                        crate::custom_races::set_client_patch_enabled(&client_path, on)?;
                    }
                }
            }
        }
    }
    set_enabled_in(&catalog(), root, meta, id, on)
}

fn bot_enabled(root: &Path, e: &Entry) -> Result<bool> {
    let name = conf_name(root, e);
    let conf = dir(root).join(&name);
    let dist = dir(root).join(format!("{name}.dist"));
    let path = if conf.exists() { conf } else { dist };
    if !path.exists() {
        return Ok(false);
    }
    let file = ConfFile::parse_bytes(&fs::read(path)?)?;
    // Invalid booleans use the modules' enabled compiled default: fail closed.
    Ok(file
        .get(&e.enable_key)
        .map(|v| {
            !matches!(
                v.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true))
}

pub fn ensure_bot_exclusivity(root: &Path) -> Result<()> {
    let cat = catalog();
    if bot_enabled(root, &entry_in(&cat, "companions")?)?
        && bot_enabled(root, &entry_in(&cat, "playerbots")?)?
    {
        return Err(Error::BotModulesConflict);
    }
    Ok(())
}

fn set_enabled_in(cat: &[Entry], root: &Path, meta: &Path, id: &str, on: bool) -> Result<()> {
    let e = entry_in(cat, id)?;
    if !e.switchable || e.enable_key.is_empty() {
        return Err(Error::Invalid("This part cannot be switched off.".into()));
    }
    if on && matches!(id, "companions" | "playerbots") {
        let other = if id == "companions" {
            "playerbots"
        } else {
            "companions"
        };
        if let Some(other) = cat.iter().find(|e| e.id == other) {
            if bot_enabled(root, other)? {
                return Err(Error::BotModulesConflict);
            }
        }
    }
    let conf = active_file(root, &e)?;
    let mut file = ConfFile::parse_bytes(&fs::read(&conf)?)?;
    if file.get(&e.enable_key).map(truthy).unwrap_or(e.default_on) == on
        && file.get(&e.enable_key).is_some()
    {
        return Ok(());
    }
    backup(meta, &conf)?;
    file.set(
        &e.enable_key,
        if on { "1" } else { "0" },
        &["Changed by CoA Server Manager"],
    );
    fsx::atomic_write(&conf, file.to_text().as_bytes())?;
    tracing::info!(module = id, on, "module switched");
    Ok(())
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub default: Option<String>,
    /// The documentation the module ships above the setting.
    pub doc: String,
    pub field: Option<crate::squid::Field>,
}

/// Every active setting of the module with its current value, its documented default and its description.
pub fn settings(root: &Path, id: &str) -> Result<Vec<Setting>> {
    settings_in(&catalog(), root, id)
}

fn settings_in(cat: &[Entry], root: &Path, id: &str) -> Result<Vec<Setting>> {
    let e = entry_in(cat, id)?;
    let name = conf_name(root, &e);
    let conf = dir(root).join(&name);
    let dist = read(&dir(root).join(format!("{name}.dist")));
    let active = read(&conf)
        .or_else(|| dist.clone())
        .ok_or_else(|| Error::Invalid("This module is not part of this server.".into()))?;
    let fields = if id == "playerbots" {
        crate::squid::fields(root)?
    } else {
        Default::default()
    };
    let mut effective = active.clone();
    if let Some(defaults) = &dist {
        for (key, value) in defaults.entries() {
            if effective.get(key).is_none() {
                effective.set(key, value, &[]);
            }
        }
    }
    Ok(effective
        .entries()
        .map(|(k, v)| Setting {
            key: k.to_string(),
            value: fields
                .get(k)
                .and_then(|field| field.default_if_missing.as_ref())
                .filter(|_| active.get(k).is_none())
                .and_then(crate::squid::scalar)
                .unwrap_or_else(|| v.to_string()),
            default: fields
                .get(k)
                .and_then(|field| crate::squid::scalar(&field.default))
                .or_else(|| dist.as_ref().and_then(|d| d.get(k)).map(str::to_string)),
            field: fields.get(k).cloned(),
            doc: dist
                .as_ref()
                .map(|d| d.doc_for(k))
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| active.doc_for(k))
                .join(" "),
        })
        .collect())
}

/// Change values of settings the module already has. A value is one line of at most 500 characters.
pub fn save_settings(
    root: &Path,
    meta: &Path,
    id: &str,
    changes: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    crate::realms::guard_module(root, id)?;
    save_settings_in(&catalog(), root, meta, id, changes)
}

fn save_settings_in(
    cat: &[Entry],
    root: &Path,
    meta: &Path,
    id: &str,
    changes: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let e = entry_in(cat, id)?;
    let conf = active_file(root, &e)?;
    let mut file = ConfFile::parse_bytes(&fs::read(&conf)?)?;
    let dist = read(&dir(root).join(format!("{}.dist", conf_name(root, &e))));
    let fields = if id == "playerbots" {
        crate::squid::fields(root)?
    } else {
        Default::default()
    };
    let mut changed = Vec::new();
    for (key, value) in changes {
        let value = value.trim();
        if let Some(field) = fields.get(key) {
            crate::squid::validate(field, value)?;
        }
        if e.switchable && key == &e.enable_key {
            return Err(Error::Invalid(
                "Use the module switch to turn this module on or off.".into(),
            ));
        }
        if id == "content-scaling" && key == "CoAContentScaling.Difficulty.DamageMultiplier" {
            let multiplier = value.parse::<f32>().map_err(|_| {
                Error::Invalid("Enemy damage must be a number from 0.25 to 2.0.".into())
            })?;
            if !multiplier.is_finite() || !(0.25..=2.0).contains(&multiplier) {
                return Err(Error::Invalid(
                    "Enemy damage must be a number from 0.25 to 2.0.".into(),
                ));
            }
        }
        if id == "content-scaling" && key == "CoAContentScaling.World.Leech.Percent" {
            let percent = value.parse::<f32>().map_err(|_| {
                Error::Invalid("Life steal must be a number from 0 to 100 percent.".into())
            })?;
            if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
                return Err(Error::Invalid(
                    "Life steal must be a number from 0 to 100 percent.".into(),
                ));
            }
        }
        if id == "content-scaling"
            && matches!(
                key.as_str(),
                "CoAContentScaling.LFG.AllowPartialGroups" | "CoAContentScaling.World.Leech.Enable"
            )
            && !matches!(value, "0" | "1")
        {
            return Err(Error::Invalid(format!("{key} must be 0 or 1.")));
        }
        if id == "ah-bot"
            && key.starts_with("AuctionHouseBot.ListProportion.Category")
            && key.contains(".Quality")
        {
            let weight = value.parse::<u32>().map_err(|_| {
                Error::Invalid(
                    "Auction listing weights must be whole numbers from 0 to 1000.".into(),
                )
            })?;
            if weight > 1000 {
                return Err(Error::Invalid(
                    "Auction listing weights must be whole numbers from 0 to 1000.".into(),
                ));
            }
        }
        if file.get(key).is_none() && dist.as_ref().and_then(|d| d.get(key)).is_none() {
            return Err(Error::Invalid(format!(
                "{key} is not a setting of this module."
            )));
        }
        let documented_empty = dist.as_ref().and_then(|d| d.get(key)).map(str::trim) == Some("");
        if (value.is_empty() && !documented_empty)
            || value.len() > 500
            || value.chars().any(char::is_control)
        {
            return Err(Error::Invalid(format!(
                "The value of {key} must be one line of text."
            )));
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

    fn test_catalog() -> Vec<Entry> {
        let one = |id: &str, name: &str, conf: &str, key: &str| Entry {
            id: id.into(),
            name: name.into(),
            description: ["en", "ru", "de", "fr", "es"]
                .iter()
                .map(|l| (l.to_string(), format!("{name} ({l})")))
                .collect(),
            repo: format!("https://github.com/Corfirean/example/{id}"),
            conf: conf.into(),
            alt_conf: None,
            enable_key: key.into(),
            switchable: true,
            default_on: true,
            status: "beta".into(),
            icon: String::new(),
            page: None,
            hidden: false,
        };
        vec![
            one(
                "war-games",
                "War Games",
                "war_games.conf",
                "WarGames.Enable",
            ),
            one(
                "spellbook",
                "Spellbook",
                "spellbook.conf",
                "Spellbook.Enable",
            ),
        ]
    }

    fn server(d: &Path) -> (PathBuf, PathBuf) {
        let (root, meta) = (d.join("srv"), d.join("meta"));
        let m = dir(&root);
        fs::create_dir_all(&m).unwrap();
        fs::create_dir_all(&meta).unwrap();
        fs::write(m.join("war_games.conf.dist"), "# Turns War Games on.\r\n# Second line.\r\nWarGames.Enable = 1\r\n\r\n# How long a challenge stays open.\r\nWarGames.ChallengeSeconds = 60\r\n").unwrap();
        (root, meta)
    }

    #[test]
    fn squid_missing_keys_use_code_defaults_and_saves_enforce_json_ranges() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(dir(root)).unwrap();
        fs::write(
            dir(root).join("playerbots.conf"),
            "AiPlayerbot.Enabled = 0\n",
        )
        .unwrap();
        fs::write(
            dir(root).join("playerbots.conf.dist"),
            "AiPlayerbot.Enabled = 0\nAiPlayerbot.MinRandomBots = 500\n",
        )
        .unwrap();
        fs::write(dir(root).join("playerbots.conf.settings.json"), r#"{"format":1,"groups":{"population":"Population"},"settings":[{"key":"AiPlayerbot.MinRandomBots","type":"int","group":"population","title":"Minimum bots","description":"Lower bound","default":500,"default_if_missing":50,"min":0,"max":5000}]}"#).unwrap();
        let cat = catalog();
        let options = settings_in(&cat, root, "playerbots").unwrap();
        let option = options
            .iter()
            .find(|setting| setting.key == "AiPlayerbot.MinRandomBots")
            .unwrap();
        assert_eq!(option.value, "50");
        assert_eq!(option.default.as_deref(), Some("500"));
        assert_eq!(
            option.field.as_ref().unwrap().group_title.as_deref(),
            Some("Population")
        );
        let before = fs::read(dir(root).join("playerbots.conf")).unwrap();
        assert!(save_settings_in(
            &cat,
            root,
            root,
            "playerbots",
            &BTreeMap::from([("AiPlayerbot.MinRandomBots".into(), "5001".into())])
        )
        .is_err());
        assert_eq!(fs::read(dir(root).join("playerbots.conf")).unwrap(), before);
        save_settings_in(
            &cat,
            root,
            root,
            "playerbots",
            &BTreeMap::from([("AiPlayerbot.MinRandomBots".into(), "42".into())]),
        )
        .unwrap();
        assert_eq!(
            settings_in(&cat, root, "playerbots")
                .unwrap()
                .iter()
                .find(|setting| setting.key == "AiPlayerbot.MinRandomBots")
                .unwrap()
                .value,
            "42"
        );
    }

    #[test]
    fn bot_systems_are_mutually_exclusive_in_both_directions_and_manual_configs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(dir(root)).unwrap();
        let a = dir(root).join("mod_coa_playerbots.conf");
        let b = dir(root).join("playerbots.conf");
        fs::write(&a, "CoaBots.Enable = 1\n").unwrap();
        fs::write(&b, "AiPlayerbot.Enabled = 0\n").unwrap();
        assert!(ensure_bot_exclusivity(root).is_ok());
        assert!(matches!(
            set_enabled_in(&catalog(), root, root, "playerbots", true),
            Err(Error::BotModulesConflict)
        ));
        assert_eq!(fs::read_to_string(&b).unwrap(), "AiPlayerbot.Enabled = 0\n");
        fs::write(&a, "CoaBots.Enable = 0\n").unwrap();
        set_enabled_in(&catalog(), root, root, "playerbots", true).unwrap();
        assert!(matches!(
            set_enabled_in(&catalog(), root, root, "companions", true),
            Err(Error::BotModulesConflict)
        ));
        fs::write(&a, "CoaBots.Enable = true\n").unwrap();
        assert!(matches!(
            ensure_bot_exclusivity(root),
            Err(Error::BotModulesConflict)
        ));
        fs::write(&b, "# missing key uses the compiled enabled default\n").unwrap();
        assert!(matches!(
            ensure_bot_exclusivity(root),
            Err(Error::BotModulesConflict)
        ));
        set_enabled_in(&catalog(), root, root, "companions", false).unwrap();
        assert!(ensure_bot_exclusivity(root).is_ok());
    }

    #[test]
    fn startup_rejects_conflicting_bots_before_launching_any_service() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir(d.path())).unwrap();
        fs::write(
            dir(d.path()).join("mod_coa_playerbots.conf"),
            "CoaBots.Enable = 1",
        )
        .unwrap();
        fs::write(
            dir(d.path()).join("playerbots.conf"),
            "AiPlayerbot.Enabled = 1",
        )
        .unwrap();
        for verb in [
            crate::driver::Verb::StartAll,
            crate::driver::Verb::StartWorld,
        ] {
            assert!(matches!(
                crate::driver::run(d.path(), verb),
                Err(Error::BotModulesConflict)
            ));
        }
    }

    #[test]
    fn the_bundled_catalog_is_valid() {
        let c = catalog();
        // it may be empty (modules are added to it one by one); whatever is in it must be complete
        for e in &c {
            assert!(
                e.description.contains_key("en") && e.description.len() == 5,
                "{} has all five descriptions",
                e.id
            );
            assert!(
                e.repo.starts_with("https://github.com/") && e.conf.ends_with(".conf"),
                "{}",
                e.id
            );
            assert!(
                !e.switchable
                    || (e.enable_key.ends_with(".Enable") || e.enable_key.ends_with(".Enabled")),
                "{} needs the setting that switches it",
                e.id
            );
            assert!(
                ["soon", "early", "beta", "release"].contains(&e.status.as_str()),
                "{} has an unknown status",
                e.id
            );
            assert!(
                e.status != "soon" || !e.switchable,
                "{} is only announced, so it cannot be switched",
                e.id
            );
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
        let wg = list_in(&test_catalog(), &root)
            .into_iter()
            .find(|m| m.id == "war-games")
            .unwrap();
        assert!(wg.installed && wg.enabled, "the .dist says 1");
        assert!(
            !list_in(&test_catalog(), &root)
                .into_iter()
                .find(|m| m.id == "spellbook")
                .unwrap()
                .installed,
            "no configuration of that module on this server"
        );
    }

    #[test]
    fn switching_a_module_creates_its_file_saves_the_old_one_and_changes_only_that_line() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        set_enabled_in(&test_catalog(), &root, &meta, "war-games", false).unwrap();
        let conf = fs::read_to_string(dir(&root).join("war_games.conf")).unwrap();
        assert!(
            conf.contains("WarGames.Enable = 0") && conf.contains("WarGames.ChallengeSeconds = 60"),
            "{conf}"
        );
        assert!(
            !list_in(&test_catalog(), &root)
                .into_iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .enabled
        );
        set_enabled_in(&test_catalog(), &root, &meta, "war-games", false).unwrap();
        assert_eq!(
            fs::read_dir(meta.join("backups/modules")).unwrap().count(),
            1,
            "no change, no backup"
        );
        set_enabled_in(&test_catalog(), &root, &meta, "war-games", true).unwrap();
        assert!(fs::read_to_string(dir(&root).join("war_games.conf"))
            .unwrap()
            .contains("WarGames.Enable = 1"));
        assert!(
            set_enabled_in(&test_catalog(), &root, &meta, "spellbook", true).is_err(),
            "not part of this server"
        );
        assert!(set_enabled_in(&test_catalog(), &root, &meta, "nonsense", true).is_err());
    }

    #[test]
    fn settings_show_the_documentation_and_only_known_keys_with_one_line_values_can_be_saved() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let s = settings_in(&test_catalog(), &root, "war-games").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(
            s[1],
            Setting {
                key: "WarGames.ChallengeSeconds".into(),
                value: "60".into(),
                default: Some("60".into()),
                doc: "How long a challenge stays open.".into(),
                field: None
            }
        );
        assert_eq!(s[0].doc, "Turns War Games on. Second line.");

        let ok = BTreeMap::from([("WarGames.ChallengeSeconds".to_string(), "90".to_string())]);
        assert_eq!(
            save_settings_in(&test_catalog(), &root, &meta, "war-games", &ok).unwrap(),
            ["WarGames.ChallengeSeconds"]
        );
        assert!(settings_in(&test_catalog(), &root, "war-games").unwrap()[1].value == "90");
        assert!(
            save_settings_in(&test_catalog(), &root, &meta, "war-games", &ok)
                .unwrap()
                .is_empty(),
            "same value: nothing to do"
        );
        for bad in [
            ("Other.Key", "1"),
            ("WarGames.ChallengeSeconds", ""),
            ("WarGames.ChallengeSeconds", "1\nInjected = 1"),
        ] {
            let c = BTreeMap::from([(bad.0.to_string(), bad.1.to_string())]);
            assert!(
                save_settings_in(&test_catalog(), &root, &meta, "war-games", &c).is_err(),
                "{bad:?}"
            );
        }
        assert!(!fs::read_to_string(dir(&root).join("war_games.conf"))
            .unwrap()
            .contains("Injected"));
    }

    #[test]
    fn updated_module_defaults_are_visible_and_can_be_saved_in_an_existing_config() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        fs::write(dir(&root).join("war_games.conf"), "WarGames.Enable = 0\n").unwrap();
        let cat = test_catalog();
        let values = settings_in(&cat, &root, "war-games").unwrap();
        assert_eq!(
            values
                .iter()
                .find(|s| s.key == "WarGames.Enable")
                .unwrap()
                .value,
            "0"
        );
        assert_eq!(
            values
                .iter()
                .find(|s| s.key == "WarGames.ChallengeSeconds")
                .unwrap()
                .value,
            "60"
        );
        let changes = BTreeMap::from([("WarGames.ChallengeSeconds".to_string(), "90".to_string())]);
        save_settings_in(&cat, &root, &meta, "war-games", &changes).unwrap();
        let text = fs::read_to_string(dir(&root).join("war_games.conf")).unwrap();
        assert!(
            text.contains("WarGames.Enable = 0") && text.contains("WarGames.ChallengeSeconds = 90")
        );
    }

    #[test]
    fn module_enable_cannot_be_changed_through_the_settings_editor() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let changes = BTreeMap::from([("WarGames.Enable".into(), "0".into())]);
        assert!(save_settings_in(&test_catalog(), &root, &meta, "war-games", &changes).is_err());
        assert_eq!(
            settings_in(&test_catalog(), &root, "war-games").unwrap()[0].value,
            "1"
        );
    }

    #[test]
    fn settings_button_requires_more_than_the_master_switch() {
        let d = tempfile::tempdir().unwrap();
        let (root, _) = server(d.path());
        let cat = test_catalog();
        assert!(
            list_in(&cat, &root)
                .iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .has_settings
        );
        fs::write(
            dir(&root).join("war_games.conf.dist"),
            "WarGames.Enable = 1\n",
        )
        .unwrap();
        assert!(
            !list_in(&cat, &root)
                .iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .has_settings
        );
        fs::write(
            dir(&root).join("war_games.conf"),
            "WarGames.Enable = 0\nWarGames.ChallengeSeconds = 90\n",
        )
        .unwrap();
        assert!(
            list_in(&cat, &root)
                .iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .has_settings,
            "custom active settings remain editable"
        );
    }

    #[test]
    fn documented_empty_lists_can_be_restored() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        fs::write(
            dir(&root).join("war_games.conf.dist"),
            "WarGames.Enable = 1\nWarGames.ExcludedItems = \n",
        )
        .unwrap();
        fs::write(
            dir(&root).join("war_games.conf"),
            "WarGames.Enable = 1\nWarGames.ExcludedItems = 42\n",
        )
        .unwrap();
        let changes = BTreeMap::from([("WarGames.ExcludedItems".into(), "".into())]);
        save_settings_in(&test_catalog(), &root, &meta, "war-games", &changes).unwrap();
        assert_eq!(
            settings_in(&test_catalog(), &root, "war-games")
                .unwrap()
                .iter()
                .find(|s| s.key == "WarGames.ExcludedItems")
                .unwrap()
                .value,
            ""
        );
    }

    #[test]
    fn auction_type_weights_are_validated_before_any_setting_is_written() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let cat = catalog();
        let key = "AuctionHouseBot.ListProportion.CategoryWeapon.QualityNormal";
        let conf = dir(&root).join("mod_ahbot.conf");
        fs::write(
            &conf,
            format!(
                "AuctionHouseBot.Enable = true\nAuctionHouseBot.ItemsPerCycle = 150\n{key} = 20\n"
            ),
        )
        .unwrap();
        let original = fs::read(&conf).unwrap();
        for bad in ["-1", "1.5", "NaN", "1001", "", "4294967296"] {
            let changes = BTreeMap::from([
                ("AuctionHouseBot.ItemsPerCycle".into(), "25".into()),
                (key.into(), bad.into()),
            ]);
            assert!(
                save_settings_in(&cat, &root, &meta, "ah-bot", &changes).is_err(),
                "{bad}"
            );
            assert_eq!(
                fs::read(&conf).unwrap(),
                original,
                "no partial save for {bad}"
            );
        }
        let off = BTreeMap::from([(key.into(), "0".into())]);
        save_settings_in(&cat, &root, &meta, "ah-bot", &off).unwrap();
        assert_eq!(
            settings_in(&cat, &root, "ah-bot")
                .unwrap()
                .iter()
                .find(|s| s.key == key)
                .unwrap()
                .value,
            "0"
        );
    }

    #[test]
    fn partial_lfg_and_world_leech_settings_round_trip_and_reject_invalid_values() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let cat = catalog();
        let conf = dir(&root).join("mod-coa-content-scaling.conf.dist");
        let partial = "CoAContentScaling.LFG.AllowPartialGroups";
        let enable = "CoAContentScaling.World.Leech.Enable";
        let percent = "CoAContentScaling.World.Leech.Percent";
        let change = |k: &str, v: &str| BTreeMap::from([(k.into(), v.into())]);
        fs::write(&conf, "CoAContentScaling.Enable = 1\n").unwrap();
        for key in [partial, enable, percent] {
            assert!(
                save_settings_in(&cat, &root, &meta, "content-scaling", &change(key, "1")).is_err()
            );
        }
        fs::write(
            &conf,
            format!("CoAContentScaling.Enable = 1\n{partial} = 1\n{enable} = 0\n{percent} = 5.0\n"),
        )
        .unwrap();
        for bad in ["NaN", "inf", "-0.1", "100.1", "abc"] {
            assert!(
                save_settings_in(&cat, &root, &meta, "content-scaling", &change(percent, bad))
                    .is_err()
            );
        }
        for key in [partial, enable] {
            for bad in ["2", "-1", "true", "abc"] {
                assert!(
                    save_settings_in(&cat, &root, &meta, "content-scaling", &change(key, bad))
                        .is_err()
                );
            }
            for good in ["0", "1"] {
                save_settings_in(&cat, &root, &meta, "content-scaling", &change(key, good))
                    .unwrap();
            }
        }
        for good in ["0", "10.5", "100"] {
            save_settings_in(
                &cat,
                &root,
                &meta,
                "content-scaling",
                &change(percent, good),
            )
            .unwrap();
            let settings = settings_in(&cat, &root, "content-scaling").unwrap();
            assert_eq!(
                settings.iter().find(|s| s.key == percent).unwrap().value,
                good
            );
        }
    }

    #[test]
    fn damage_difficulty_is_validated_and_older_server_builds_do_not_accept_it() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        let cat = catalog();
        let conf = dir(&root).join("mod-coa-content-scaling.conf.dist");
        fs::write(&conf, "CoAContentScaling.Enable = 1\n").unwrap();
        let key = "CoAContentScaling.Difficulty.DamageMultiplier";
        let changes = |v: &str| BTreeMap::from([(key.into(), v.into())]);
        assert!(save_settings_in(&cat, &root, &meta, "content-scaling", &changes("0.5")).is_err());
        fs::write(
            &conf,
            format!("CoAContentScaling.Enable = 1\n{key} = 1.0\n"),
        )
        .unwrap();
        for bad in ["NaN", "inf", "0", "0.24", "2.01", "abc"] {
            assert!(
                save_settings_in(&cat, &root, &meta, "content-scaling", &changes(bad)).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            save_settings_in(&cat, &root, &meta, "content-scaling", &changes("0.25")).unwrap(),
            [key]
        );
        assert_eq!(
            save_settings_in(&cat, &root, &meta, "content-scaling", &changes("2")).unwrap(),
            [key]
        );
        assert_eq!(
            settings_in(&cat, &root, "content-scaling")
                .unwrap()
                .iter()
                .find(|s| s.key == key)
                .unwrap()
                .value,
            "2"
        );
    }

    #[test]
    fn an_older_file_name_is_used_when_the_current_one_is_missing_and_a_part_that_cannot_be_switched_stays_on(
    ) {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        fs::write(
            dir(&root).join("old_name.conf"),
            "CoA.Enable = 0\r\nCoA.UnlockAllVanity = 1\r\n",
        )
        .unwrap();
        let compat = Entry {
            id: "compat".into(),
            name: "Compat".into(),
            description: ["en", "ru", "de", "fr", "es"]
                .iter()
                .map(|l| (l.to_string(), "x".to_string()))
                .collect(),
            repo: "https://github.com/Corfirean/x".into(),
            conf: "coa.conf".into(),
            alt_conf: Some("old_name.conf".into()),
            enable_key: String::new(),
            switchable: false,
            default_on: true,
            status: "release".into(),
            icon: String::new(),
            page: None,
            hidden: false,
        };
        let cat = vec![compat];
        let v = list_in(&cat, &root).remove(0);
        assert!(
            v.installed && v.enabled && !v.switchable,
            "found under the old name, never reported as off"
        );
        assert_eq!(settings_in(&cat, &root, "compat").unwrap().len(), 2);
        let change = BTreeMap::from([("CoA.UnlockAllVanity".to_string(), "0".to_string())]);
        assert_eq!(
            save_settings_in(&cat, &root, &meta, "compat", &change).unwrap(),
            ["CoA.UnlockAllVanity"]
        );
        assert!(fs::read_to_string(dir(&root).join("old_name.conf"))
            .unwrap()
            .contains("CoA.UnlockAllVanity = 0"));
        assert!(
            set_enabled_in(&cat, &root, &meta, "compat", false).is_err(),
            "cannot be switched off"
        );
    }

    #[test]
    fn a_module_whose_setting_is_not_in_the_file_counts_as_on_and_can_be_switched_off() {
        let d = tempfile::tempdir().unwrap();
        let (root, meta) = server(d.path());
        fs::write(
            dir(&root).join("war_games.conf"),
            "WarGames.ChallengeSeconds = 60\r\n",
        )
        .unwrap();
        let cat = test_catalog();
        assert!(
            list_in(&cat, &root)
                .into_iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .enabled,
            "a missing key means the default (on)"
        );
        set_enabled_in(&cat, &root, &meta, "war-games", false).unwrap();
        assert!(fs::read_to_string(dir(&root).join("war_games.conf"))
            .unwrap()
            .contains("WarGames.Enable = 0"));
        assert!(
            !list_in(&cat, &root)
                .into_iter()
                .find(|m| m.id == "war-games")
                .unwrap()
                .enabled
        );
    }
}
