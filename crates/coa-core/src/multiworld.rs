//! A second supervised world with isolated configs, logs and ports; auth and MySQL remain shared.
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}};
use crate::{Error, Result, fsx, realms::{self, Mode, RealmState}, driver::{self, Verb}, process::{self, ServiceState}};

pub fn root(main: &Path) -> PathBuf { main.join(".realms/secondary") }

pub fn is_running(main: &Path) -> bool {
    let second = root(main);
    if !second.exists() { return false; }
    let status = process::observe(&second, &crate::layout::read_ports(&second));
    status.world.state != ServiceState::Stopped
        || ["supervisor", "relay"].iter().any(|n| process::read_state_record(&second, n).is_some_and(|p| process::is_alive(&p)))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == "Logs" || entry.file_name() == "reports" || entry.file_name() == "__pycache__" { continue; }
        if entry.file_type()?.is_symlink() { return Err(Error::Invalid("A linked runtime file cannot be used for a second realm.".into())); }
        let target = fsx::ensure_within(to, &to.join(entry.file_name()))?;
        if entry.file_type()?.is_dir() { copy_tree(&entry.path(), &target)?; }
        else if !target.is_file() || fsx::sha256_file(&entry.path())? != fsx::sha256_file(&target)? { fs::copy(entry.path(), target)?; }
    }
    Ok(())
}

pub fn set_enabled(main: &Path, enabled: bool) -> Result<realms::View> {
    if enabled && !cfg!(windows) { return Err(Error::Invalid("Simultaneous realms currently require Windows.".into())); }
    if enabled {
        if !realms::view(main)?.supported { return Err(Error::Invalid("Update the server to a build with Wildcard support first.".into())); }
        secondary_launcher(&fs::read_to_string(main.join("Scripts/manage.py"))?)?;
    }
    let status = process::observe(main, &crate::layout::read_ports(main));
    if status.world.state != ServiceState::Stopped || status.auth.state != ServiceState::Stopped || is_running(main) {
        return Err(Error::Invalid("Stop both realms before changing simultaneous startup.".into()));
    }
    let original = realms::state(main)?.active;
    if enabled && !realms::state(main)?.wildcard_created {
        realms::select(main, Mode::Wildcard)?;
        if original != Mode::Wildcard { realms::select(main, original)?; }
    }
    let mut s = realms::state(main)?;
    if enabled {
        let p = crate::layout::read_ports(main);
        let mut reserved = vec![p.mysql, p.auth, p.world, p.ra];
        let mut choose = |saved: Option<u16>, near: u16| -> Result<u16> {
            if let Some(port) = saved.filter(|p| *p > 0 && !reserved.contains(p)) { reserved.push(port); return Ok(port); }
            let port = (near.saturating_add(1)..=u16::MAX).find(|p| !reserved.contains(p) && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
                .ok_or_else(|| Error::Invalid("No free port for the second realm.".into()))?;
            reserved.push(port); Ok(port)
        };
        s.secondary_world_port = Some(choose(s.secondary_world_port, p.world)?);
        s.secondary_ra_port = Some(choose(s.secondary_ra_port, p.ra)?);
    }
    s.simultaneous = enabled;
    fsx::atomic_write_json(&main.join("Settings/realm-profile.json"), &s)?;
    realms::view(main)
}

fn prepare(main: &Path) -> Result<PathBuf> {
    if is_running(main) { return Err(Error::Invalid("The second realm is still running; stop it before preparing its files.".into())); }
    let s = realms::state(main)?;
    let mode = if s.active == Mode::Coa { Mode::Wildcard } else { Mode::Coa };
    let second = fsx::ensure_within(main, &root(main))?;
    for folder in ["Runtime", "BugReport", "mysql/bin"] { copy_tree(&main.join(folder), &fsx::ensure_within(&second, &second.join(folder))?)?; }
    if main.join("Core/reference").is_dir() { copy_tree(&main.join("Core/reference"), &fsx::ensure_within(&second, &second.join("Core/reference"))?)?; }
    for entry in fs::read_dir(main.join("Core"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() && (entry.file_name() == "worldserver.exe" || entry.path().extension().is_some_and(|x| x == "dll")) {
            fs::create_dir_all(second.join("Core"))?;
            fs::copy(entry.path(), fsx::ensure_within(&second, &second.join("Core").join(entry.file_name()))?)?;
        }
    }
    let files: BTreeMap<String, Vec<u8>> = fsx::read_json(&main.join(format!("Settings/realm-profiles/{}.json", mode.name())))?;
    // A previous secondary run may have used the other realm's module files.
    for path in crate::backup::config_files(&second) {
        if path.starts_with("Core/configs/") && path.ends_with(".conf") && !files.contains_key(&path) {
            fs::remove_file(fsx::ensure_within(&second, &fsx::safe_join(&second, &path)?)?)?;
        }
    }
    for (path, bytes) in files {
        if !(path.starts_with("Core/configs/") && path.ends_with(".conf") || path.starts_with("Settings/") && path.ends_with(".template")) {
            return Err(Error::Invalid("Invalid secondary realm configuration path.".into()));
        }
        let text = String::from_utf8(bytes).map_err(|_| Error::Invalid("Invalid realm config encoding.".into()))?;
        let shared = main.to_string_lossy().replace('\\', "/");
        let text = text.replace("@DATA@", &format!("{shared}/Data"))
            .replace("@ASCENSION_DBC@", &format!("{shared}/Data/dbc/Ascension"))
            .replace("@MYSQL_EXE@", &format!("{shared}/mysql/bin/mysql.exe"));
        let text = if path == "Settings/worldserver.conf.template" {
            let main_conf = crate::config::parser::ConfFile::parse_bytes(&fs::read(main.join(&path))?)?;
            let mut other_conf = crate::config::parser::ConfFile::parse_bytes(text.as_bytes())?;
            if let Some(bind) = main_conf.get("BindIP") { other_conf.set("BindIP", bind, &[]); }
            other_conf.set("Ra.IP", "\"127.0.0.1\"", &[]);
            other_conf.to_text()
        } else if path == "Core/configs/modules/coa.conf" {
            let mut conf = crate::config::parser::ConfFile::parse_bytes(text.as_bytes())?;
            if crate::friends::bind_is_open(main) { conf.set("CoA.AllowRemoteClients", "1", &[]); }
            conf.to_text()
        } else { text };
        fsx::atomic_write(&fsx::ensure_within(&second, &fsx::safe_join(&second, &path)?)?, text.as_bytes())?;
    }
    let mut cfg: serde_json::Value = fsx::read_json(&main.join("Settings/repack.json"))?;
    cfg["worldPort"] = s.secondary_world_port.ok_or_else(|| Error::Invalid("Second realm world port is missing.".into()))?.into();
    cfg["raPort"] = s.secondary_ra_port.ok_or_else(|| Error::Invalid("Second realm console port is missing.".into()))?.into();
    fsx::atomic_write_json(&second.join("Settings/repack.json"), &cfg)?;
    fsx::atomic_write(&second.join("Settings/database.json"), &fs::read(main.join("Settings/database.json"))?)?;
    fsx::atomic_write_json(&second.join("Settings/realm-profile.json"), &RealmState { active: mode, wildcard_created: true, ..Default::default() })?;
    let source = fs::read_to_string(main.join("Scripts/manage.py"))?;
    let source = secondary_launcher(&source)?;
    fsx::atomic_write(&second.join("Scripts/manage.py"), source.as_bytes())?;
    Ok(second)
}

fn secondary_launcher(source: &str) -> Result<String> {
    let marker = "def start_world(config, offset):\n    start_mysql(config)";
    let source = source.replace("\r\n", "\n");
    if !source.contains(marker) { return Err(Error::Invalid("The launcher does not support a shared database world. Update the server first.".into())); }
    Ok(source.replace(marker, "def start_world(config, offset):\n    mysql(admin=\"ping\")"))
}

pub fn start(main: &Path) -> Result<()> {
    if !realms::state(main)?.simultaneous { return Ok(()); }
    if is_running(main) {
        let second = root(main);
        if process::observe(&second, &crate::layout::read_ports(&second)).world.state == ServiceState::Running { return Ok(()); }
        return Err(Error::Invalid("The second realm is still starting or stopping.".into()));
    }
    let second = prepare(main)?;
    let out = driver::run(&second, Verb::StartWorld)?;
    if !out.ok { return Err(Error::Invalid(format!("The second realm did not start: {}", out.output))); }
    Ok(())
}

pub fn stop(main: &Path) -> Result<()> {
    let second = root(main);
    if !second.join("Scripts/manage.py").exists() {
        if is_running(main) { return Err(Error::Invalid("The running second realm's launcher is missing. The shared database was left running.".into())); }
        return Ok(());
    }
    // Its state has no auth/MySQL records: the shared database remains up until the primary world stops.
    if is_running(main) {
        let out = driver::run(&second, Verb::StopAll)?;
        if !out.ok || is_running(main) { return Err(Error::Invalid(format!("The second realm could not stop; the database was left running: {}", out.output))); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secondary_never_starts_the_shared_database() {
        let patched = secondary_launcher("def start_world(config, offset):\r\n    start_mysql(config)\r\n    wait_ready('world', 8086)\r\n").unwrap();
        assert!(patched.contains("mysql(admin=\"ping\")"));
        assert!(!patched.contains("start_mysql(config)"));
        assert!(secondary_launcher("def start_world(): pass").is_err());
    }
}
