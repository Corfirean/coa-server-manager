//! A second supervised world with isolated configs, logs and ports; auth and MySQL remain shared.
use crate::{
    driver::{self, Verb},
    fsx,
    process::{self, ServiceState},
    realms::{self, Mode, RealmState},
    Error, Result,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// A world that runs next to the primary one. Each has its own folder, configs, logs and ports; auth and MySQL stay shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    /// Wildcard next to Conquest of Azeroth (or Conquest of Azeroth next to Wildcard when Wildcard is the primary).
    Second,
    /// Warcraft Reborn next to Conquest of Azeroth.
    Reborn,
}

impl Tree {
    pub const ALL: [Tree; 2] = [Tree::Second, Tree::Reborn];

    fn dir(self) -> &'static str {
        match self {
            Tree::Second => ".realms/secondary",
            Tree::Reborn => ".realms/reborn",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tree::Second => "second realm",
            Tree::Reborn => "Warcraft Reborn realm",
        }
    }

    fn mode(self, active: Mode) -> Mode {
        match self {
            Tree::Second if active == Mode::Coa => Mode::Wildcard,
            Tree::Second => Mode::Coa,
            Tree::Reborn => Mode::Reborn,
        }
    }

    fn enabled(self, s: &RealmState) -> bool {
        match self {
            Tree::Second => s.simultaneous,
            Tree::Reborn => s.with_reborn,
        }
    }

    fn ports(self, s: &RealmState) -> (Option<u16>, Option<u16>) {
        match self {
            Tree::Second => (s.secondary_world_port, s.secondary_ra_port),
            Tree::Reborn => (s.reborn_world_port, s.reborn_ra_port),
        }
    }
}

pub fn tree_root(main: &Path, tree: Tree) -> PathBuf {
    main.join(tree.dir())
}

/// The folder of the second realm (Wildcard next to Conquest of Azeroth).
pub fn root(main: &Path) -> PathBuf {
    tree_root(main, Tree::Second)
}

pub fn tree_running(main: &Path, tree: Tree) -> bool {
    let second = tree_root(main, tree);
    if !second.exists() {
        return false;
    }
    let status = process::observe(&second, &crate::layout::read_ports(&second));
    status.world.state != ServiceState::Stopped
        || ["supervisor", "relay"]
            .iter()
            .any(|n| process::read_state_record(&second, n).is_some_and(|p| process::is_alive(&p)))
}

/// The folder whose world server runs `mode`: the main folder, or the folder of the extra world that runs that realm.
/// Its settings hold the console port that belongs to that world.
pub fn world_root(main: &Path, mode: Mode) -> PathBuf {
    let Ok(s) = realms::state(main) else {
        return main.to_path_buf();
    };
    if mode == s.active {
        return main.to_path_buf();
    }
    Tree::ALL
        .into_iter()
        .find(|t| t.enabled(&s) && t.mode(s.active) == mode)
        .map_or_else(|| main.to_path_buf(), |t| tree_root(main, t))
}

/// Any of the extra worlds is running or still stopping.
pub fn is_running(main: &Path) -> bool {
    Tree::ALL.into_iter().any(|t| tree_running(main, t))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == "Logs"
            || entry.file_name() == "reports"
            || entry.file_name() == "__pycache__"
        {
            continue;
        }
        if entry.file_type()?.is_symlink() {
            return Err(Error::Invalid(
                "A linked runtime file cannot be used for a second realm.".into(),
            ));
        }
        let target = fsx::ensure_within(to, &to.join(entry.file_name()))?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if !target.is_file()
            || fsx::sha256_file(&entry.path())? != fsx::sha256_file(&target)?
        {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Start Wildcard together with Conquest of Azeroth.
pub fn set_enabled(main: &Path, enabled: bool) -> Result<realms::View> {
    set_tree(main, Tree::Second, enabled)
}

/// Start Warcraft Reborn together with Conquest of Azeroth.
pub fn set_reborn_enabled(main: &Path, enabled: bool) -> Result<realms::View> {
    set_tree(main, Tree::Reborn, enabled)
}

fn set_tree(main: &Path, tree: Tree, enabled: bool) -> Result<realms::View> {
    if enabled && !cfg!(windows) {
        return Err(Error::Invalid(
            "Simultaneous realms currently require Windows.".into(),
        ));
    }
    if enabled {
        let active = realms::state(main)?.active;
        let view = realms::view(main)?;
        match tree {
            Tree::Second => {
                if active == Mode::Reborn {
                    return Err(Error::Invalid(
                        "Wildcard starts together with Conquest of Azeroth. Select Conquest of Azeroth first."
                            .into(),
                    ));
                }
                if !view.supported {
                    return Err(Error::Invalid(
                        "Update the server to a build with Wildcard support first.".into(),
                    ));
                }
            }
            Tree::Reborn => {
                if active != Mode::Coa {
                    return Err(Error::Invalid(
                        "Warcraft Reborn starts together with Conquest of Azeroth. Select Conquest of Azeroth first."
                            .into(),
                    ));
                }
                if !view.reborn_supported {
                    return Err(Error::Invalid(
                        "Update the server to a build with Warcraft Reborn support first.".into(),
                    ));
                }
            }
        }
        secondary_launcher(&fs::read_to_string(main.join("Scripts/manage.py"))?)?;
    }
    let status = process::observe(main, &crate::layout::read_ports(main));
    if status.world.state != ServiceState::Stopped
        || status.auth.state != ServiceState::Stopped
        || is_running(main)
    {
        return Err(Error::Invalid(
            "Stop all realms before changing which realms start together.".into(),
        ));
    }
    if enabled {
        realms::ensure_created(main, tree.mode(realms::state(main)?.active))?;
    }
    let mut s = realms::state(main)?;
    if enabled {
        let p = crate::layout::read_ports(main);
        let mut reserved = vec![p.mysql, p.auth, p.world, p.ra];
        // Ports already saved for any other extra world stay reserved, so two worlds never get the same one.
        for other in Tree::ALL.into_iter().filter(|t| *t != tree) {
            let (world, ra) = other.ports(&s);
            reserved.extend(world.into_iter().chain(ra));
        }
        let mut choose = |saved: Option<u16>, near: u16| -> Result<u16> {
            if let Some(port) = saved.filter(|p| *p > 0 && !reserved.contains(p)) {
                reserved.push(port);
                return Ok(port);
            }
            let port = (near.saturating_add(1)..=u16::MAX)
                .find(|p| {
                    !reserved.contains(p) && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()
                })
                .ok_or_else(|| Error::Invalid(format!("No free port for the {}.", tree.label())))?;
            reserved.push(port);
            Ok(port)
        };
        let (world, ra) = tree.ports(&s);
        let (world, ra) = (choose(world, p.world)?, choose(ra, p.ra)?);
        match tree {
            Tree::Second => {
                s.secondary_world_port = Some(world);
                s.secondary_ra_port = Some(ra);
            }
            Tree::Reborn => {
                s.reborn_world_port = Some(world);
                s.reborn_ra_port = Some(ra);
            }
        }
    }
    match tree {
        Tree::Second => s.simultaneous = enabled,
        Tree::Reborn => s.with_reborn = enabled,
    }
    fsx::atomic_write_json(&main.join("Settings/realm-profile.json"), &s)?;
    realms::view(main)
}

fn prepare(main: &Path, tree: Tree) -> Result<PathBuf> {
    if tree_running(main, tree) {
        return Err(Error::Invalid(format!(
            "The {} is still running; stop it before preparing its files.",
            tree.label()
        )));
    }
    let s = realms::state(main)?;
    let mode = tree.mode(s.active);
    let second = fsx::ensure_within(main, &tree_root(main, tree))?;
    for folder in ["Runtime", "BugReport", "mysql/bin"] {
        copy_tree(
            &main.join(folder),
            &fsx::ensure_within(&second, &second.join(folder))?,
        )?;
    }
    if main.join("Scripts/squid_playerbots.py").is_file() {
        fs::create_dir_all(second.join("Scripts"))?;
        fs::copy(
            main.join("Scripts/squid_playerbots.py"),
            second.join("Scripts/squid_playerbots.py"),
        )?;
    }
    if main.join("Core/reference").is_dir() {
        copy_tree(
            &main.join("Core/reference"),
            &fsx::ensure_within(&second, &second.join("Core/reference"))?,
        )?;
    }
    for entry in fs::read_dir(main.join("Core"))? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && (entry.file_name() == "worldserver.exe"
                || entry.path().extension().is_some_and(|x| x == "dll"))
        {
            fs::create_dir_all(second.join("Core"))?;
            fs::copy(
                entry.path(),
                fsx::ensure_within(&second, &second.join("Core").join(entry.file_name()))?,
            )?;
        }
    }
    let files: BTreeMap<String, Vec<u8>> =
        fsx::read_json(&main.join(format!("Settings/realm-profiles/{}.json", mode.name())))?;
    // A previous secondary run may have used the other realm's module files.
    for path in crate::backup::config_files(&second) {
        if path.starts_with("Core/configs/")
            && path.ends_with(".conf")
            && !files.contains_key(&path)
        {
            fs::remove_file(fsx::ensure_within(
                &second,
                &fsx::safe_join(&second, &path)?,
            )?)?;
        }
    }
    for (path, bytes) in files {
        if !(path.starts_with("Core/configs/") && path.ends_with(".conf")
            || path.starts_with("Settings/") && path.ends_with(".template"))
        {
            return Err(Error::Invalid(
                "Invalid secondary realm configuration path.".into(),
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::Invalid("Invalid realm config encoding.".into()))?;
        let shared = main.to_string_lossy().replace('\\', "/");
        let text = text
            .replace("@DATA@", &format!("{shared}/Data"))
            .replace("@ASCENSION_DBC@", &format!("{shared}/Data/dbc/Ascension"))
            .replace("@MYSQL_EXE@", &format!("{shared}/mysql/bin/mysql.exe"));
        let text = if path == "Settings/worldserver.conf.template" {
            let main_conf =
                crate::config::parser::ConfFile::parse_bytes(&fs::read(main.join(&path))?)?;
            let mut other_conf = crate::config::parser::ConfFile::parse_bytes(text.as_bytes())?;
            if let Some(bind) = main_conf.get("BindIP") {
                other_conf.set("BindIP", bind, &[]);
            }
            other_conf.set("Ra.IP", "\"127.0.0.1\"", &[]);
            other_conf.to_text()
        } else if path == "Core/configs/modules/coa.conf" {
            let mut conf = crate::config::parser::ConfFile::parse_bytes(text.as_bytes())?;
            if crate::friends::bind_is_open(main) {
                conf.set("CoA.AllowRemoteClients", "1", &[]);
            }
            conf.to_text()
        } else {
            text
        };
        fsx::atomic_write(
            &fsx::ensure_within(&second, &fsx::safe_join(&second, &path)?)?,
            text.as_bytes(),
        )?;
    }
    let mut cfg: serde_json::Value = fsx::read_json(&main.join("Settings/repack.json"))?;
    let (world_port, ra_port) = tree.ports(&s);
    cfg["worldPort"] = world_port
        .ok_or_else(|| Error::Invalid(format!("The {} world port is missing.", tree.label())))?
        .into();
    cfg["raPort"] = ra_port
        .ok_or_else(|| Error::Invalid(format!("The {} console port is missing.", tree.label())))?
        .into();
    fsx::atomic_write_json(&second.join("Settings/repack.json"), &cfg)?;
    fsx::atomic_write(
        &second.join("Settings/database.json"),
        &fs::read(main.join("Settings/database.json"))?,
    )?;
    fsx::atomic_write_json(
        &second.join("Settings/realm-profile.json"),
        &RealmState {
            active: mode,
            wildcard_created: true,
            reborn_created: true,
            ..Default::default()
        },
    )?;
    let source = fs::read_to_string(main.join("Scripts/manage.py"))?;
    let source = secondary_launcher(&source)?;
    fsx::atomic_write(&second.join("Scripts/manage.py"), source.as_bytes())?;
    Ok(second)
}

fn secondary_launcher(source: &str) -> Result<String> {
    let marker = "def start_world(config, offset):\n    start_mysql(config)";
    let source = source.replace("\r\n", "\n");
    if !source.contains(marker) {
        return Err(Error::Invalid(
            "The launcher does not support a shared database world. Update the server first."
                .into(),
        ));
    }
    Ok(source.replace(
        marker,
        "def start_world(config, offset):\n    mysql(admin=\"ping\")",
    ))
}

fn start_tree(main: &Path, tree: Tree) -> Result<()> {
    if tree_running(main, tree) {
        let second = tree_root(main, tree);
        if process::observe(&second, &crate::layout::read_ports(&second))
            .world
            .state
            == ServiceState::Running
        {
            return Ok(());
        }
        return Err(Error::Invalid(format!(
            "The {} is still starting or stopping.",
            tree.label()
        )));
    }
    let second = prepare(main, tree)?;
    let out = driver::run(&second, Verb::StartWorld)?;
    if !out.ok {
        return Err(Error::Invalid(format!(
            "The {} did not start: {}",
            tree.label(),
            out.output
        )));
    }
    Ok(())
}

/// Start every extra world that is switched on.
pub fn start(main: &Path) -> Result<()> {
    let s = realms::state(main)?;
    for tree in Tree::ALL.into_iter().filter(|t| t.enabled(&s)) {
        start_tree(main, tree)?;
    }
    Ok(())
}

fn stop_tree(main: &Path, tree: Tree) -> Result<()> {
    let second = tree_root(main, tree);
    if !second.join("Scripts/manage.py").exists() {
        if tree_running(main, tree) {
            return Err(Error::Invalid(format!(
                "The running {}'s launcher is missing. The shared database was left running.",
                tree.label()
            )));
        }
        return Ok(());
    }
    // Its state has no auth/MySQL records: the shared database remains up until the primary world stops.
    if tree_running(main, tree) {
        let out = driver::run(&second, Verb::StopAll)?;
        if !out.ok || tree_running(main, tree) {
            return Err(Error::Invalid(format!(
                "The {} could not stop; the database was left running: {}",
                tree.label(),
                out.output
            )));
        }
    }
    Ok(())
}

/// Stop every extra world; one that fails does not keep the others running.
pub fn stop(main: &Path) -> Result<()> {
    let mut first = None;
    for tree in Tree::ALL {
        if let Err(e) = stop_tree(main, tree) {
            first.get_or_insert(e);
        }
    }
    first.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_extra_world_has_its_own_folder_realm_and_ports() {
        let mut s = RealmState::default();
        assert_eq!(Tree::Second.mode(Mode::Coa), Mode::Wildcard);
        assert_eq!(Tree::Second.mode(Mode::Wildcard), Mode::Coa);
        assert_eq!(Tree::Reborn.mode(Mode::Coa), Mode::Reborn);
        assert_ne!(Tree::Second.dir(), Tree::Reborn.dir());
        s.secondary_world_port = Some(8086);
        s.reborn_world_port = Some(8087);
        assert_eq!(Tree::Second.ports(&s).0, Some(8086));
        assert_eq!(Tree::Reborn.ports(&s).0, Some(8087));
        assert!(!Tree::Second.enabled(&s) && !Tree::Reborn.enabled(&s));
        s.with_reborn = true;
        assert!(!Tree::Second.enabled(&s) && Tree::Reborn.enabled(&s));
    }

    #[test]
    fn a_realm_is_reached_through_the_folder_of_its_own_world() {
        let t = tempfile::tempdir().unwrap();
        let main = t.path();
        fsx::atomic_write(
            &main.join("Settings/realm-profile.json"),
            br#"{"active":"coa","wildcard_created":true,"reborn_created":true,"simultaneous":true,"with_reborn":true}"#,
        )
        .unwrap();
        assert_eq!(world_root(main, Mode::Coa), main);
        assert_eq!(world_root(main, Mode::Wildcard), main.join(".realms/secondary"));
        assert_eq!(world_root(main, Mode::Reborn), main.join(".realms/reborn"));
        // A realm that is not switched on has no world of its own: the main folder answers.
        fsx::atomic_write(
            &main.join("Settings/realm-profile.json"),
            br#"{"active":"coa","wildcard_created":true,"reborn_created":true,"with_reborn":true}"#,
        )
        .unwrap();
        assert_eq!(world_root(main, Mode::Wildcard), main);
        assert_eq!(world_root(main, Mode::Reborn), main.join(".realms/reborn"));
    }

    #[test]
    fn reborn_starts_together_with_conquest_of_azeroth_only() {
        let t = tempfile::tempdir().unwrap();
        fsx::atomic_write(
            &t.path().join("Core/worldserver.exe"),
            b"Warcraft Reborn class spells are unavailable",
        )
        .unwrap();
        fsx::atomic_write(
            &t.path().join("Settings/realm-profile.json"),
            br#"{"active":"wildcard","wildcard_created":true}"#,
        )
        .unwrap();
        let err = set_reborn_enabled(t.path(), true).unwrap_err().to_string();
        assert!(err.contains("Select Conquest of Azeroth first"), "{err}");
        assert!(!realms::state(t.path()).unwrap().with_reborn);
    }

    #[test]
    fn secondary_never_starts_the_shared_database() {
        let patched = secondary_launcher("def start_world(config, offset):\r\n    start_mysql(config)\r\n    wait_ready('world', 8086)\r\n").unwrap();
        assert!(patched.contains("mysql(admin=\"ping\")"));
        assert!(!patched.contains("start_mysql(config)"));
        assert!(secondary_launcher("def start_world(): pass").is_err());
    }
}
