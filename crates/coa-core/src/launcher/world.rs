//! The world server and the bug-report relay beside it (`manage.py start_world`, `watch_world`, `wait_relay`),
//! and the detailed-logging switch (`debug_logs`).
//!
//! A world is started through a supervisor process that outlives the caller: it starts `worldserver.exe`, keeps
//! the relay running while the world runs, and stops the relay when the world ends. The supervisor is
//! `coa-launcher.exe watch-world`, or `manage.py watch-world` on a repack that does not ship it yet (both write
//! the same records). The relay itself is still the Python `BugReport/relay.py`, run through the repack's
//! Python, until it is ported too.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Error, ErrorCode, Result};
use crate::launcher::services::{self, ensure_free, mysql, spawn, start_mysql, wait_ready, Progress};
use crate::launcher::spawn::Proc;
use crate::launcher::{settings::Settings, squid, state};

fn stop_relay_flag(root: &Path) -> std::path::PathBuf {
    state::dir(root).join("stop-relay")
}

/// The relay is up and has finished starting (it records itself as `relay-ready` once it serves).
pub fn wait_relay(root: &Path) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if let (Some(relay), Some(ready)) = (state::running(root, "relay"), state::running(root, "relay-ready")) {
            if relay == ready {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(Error::Invalid("Bug-report relay is not ready; check BugReport/Logs/relay.log.".into()))
}

/// The process that runs `watch-world` for a started world.
#[derive(Debug, Clone)]
pub enum Supervisor {
    /// `coa-launcher.exe` (this crate's launcher).
    Launcher(PathBuf),
    /// The repack's Python with `Scripts/manage.py`.
    Python { python: PathBuf, script: PathBuf },
}

impl Supervisor {
    /// The supervisor a repack ships: `Scripts/coa-launcher.exe` when present, otherwise its Python launcher.
    pub fn of(root: &Path) -> Result<Supervisor> {
        let exe = root.join("Scripts/coa-launcher.exe");
        if exe.is_file() {
            return Ok(Supervisor::Launcher(exe));
        }
        let python = squid::package_root(root).join("Runtime/python/python.exe");
        let script = root.join("Scripts/manage.py");
        if python.is_file() && script.is_file() {
            return Ok(Supervisor::Python { python, script });
        }
        Err(Error::Service(
            ErrorCode::ServerFilesIncomplete,
            "This server has no world supervisor (Scripts/coa-launcher.exe or Scripts/manage.py). Repair its program files.".into(),
        ))
    }
}

/// How long a world may take to open both of its listeners, together: slow worlds (many bots, a cold disk) are
/// common, and the limit must not depend on the bot module.
const WORLD_STARTUP_SECONDS: u64 = 1500;

/// Wait until the world's console answers a command (it can listen before it accepts logins).
fn wait_console(root: &Path, cfg: &Settings, until: Instant) -> Result<()> {
    let mut last = String::new();
    while Instant::now() < until {
        if state::running(root, "world").is_none() {
            return Err(Error::Service(
                ErrorCode::StartupFailed,
                "World startup failed: worldserver stopped before its console accepted commands.".into(),
            ));
        }
        match crate::ra::Ra::connect_to(cfg.ra_port, &cfg.ra_username, &cfg.ra_password).and_then(|mut ra| ra.run("server info")) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(Error::Service(ErrorCode::StartupFailed, format!("World startup failed: its console is still starting. {last}")))
}

fn seconds_until(until: Instant) -> u64 {
    until.saturating_duration_since(Instant::now()).as_secs()
}

/// Start the world (and its database, unless it is shared), and wait until its console accepts commands.
pub fn start_world(root: &Path, cfg: &Settings, offset: i32, supervisor: &Supervisor, progress: Progress) -> Result<()> {
    start_mysql(root, cfg, progress)?;
    squid::validate_bots(root)?;
    let until = Instant::now() + Duration::from_secs(squid::startup_timeout(root).max(WORLD_STARTUP_SECONDS));
    if state::running(root, "world").is_some() {
        if state::running(root, "supervisor").is_none() {
            return Err(Error::Service(
                ErrorCode::WorldserverAlreadyRunning,
                "Worldserver has no repack supervisor. Use Stop_All_Server.bat before restarting it.".into(),
            ));
        }
        wait_ready(root, "world", cfg.world_port, seconds_until(until))?;
        wait_relay(root)?;
        wait_console(root, cfg, until)?;
        progress("Worldserver and bug-report relay are already running.");
        return Ok(());
    }
    ensure_free(cfg.world_port, progress)?;
    ensure_free(cfg.ra_port, progress)?;
    if state::running(root, "supervisor").is_some() || state::running(root, "relay").is_some() {
        return Err(Error::Service(
            ErrorCode::OperationInProgress,
            "The previous world/relay shutdown is still finishing. Try again shortly.".into(),
        ));
    }
    squid::prepare_playerbots(root, cfg.mysql_port, &|sql| mysql(root, Some(sql), None), progress)?;
    progress("Starting worldserver and its bug-report relay...");
    let _ = std::fs::remove_file(stop_relay_flag(root));
    let offset_text = offset.to_string();
    let root_text = root.to_string_lossy().into_owned();
    let log = root.join("Core/Logs/supervisor.log");
    match supervisor {
        Supervisor::Launcher(exe) => {
            spawn(root, "supervisor", exe, &["--root", &root_text, "--port-offset", &offset_text, "watch-world"], root, &log, &cfg.env_remove)?
        }
        Supervisor::Python { python, script } => {
            let script = script.to_string_lossy().into_owned();
            spawn(root, "supervisor", python, &["-B", &script, "--port-offset", &offset_text, "watch-world"], root, &log, &cfg.env_remove)?
        }
    };
    let first = Instant::now() + Duration::from_secs(20);
    while state::running(root, "world").is_none() && Instant::now() < first {
        if state::running(root, "supervisor").is_none() {
            return Err(Error::Service(ErrorCode::StartupFailed, "World startup failed; check Core/Logs/supervisor.log.".into()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    wait_ready(root, "world", cfg.world_port, seconds_until(until))?;
    wait_ready(root, "world", cfg.ra_port, seconds_until(until))?;
    wait_relay(root)?;
    wait_console(root, cfg, until)?;
    progress("Worldserver and automatic bug reporting are ready.");
    Ok(())
}

fn spawn_child(root: &Path, name: &str, exe: &Path, args: &[&str], cwd: &Path, log: &Path) -> Result<Proc> {
    // The supervisor was started with the environment its services should have.
    let child = crate::launcher::spawn::start(exe, args, cwd, log, &[])?;
    state::record(root, name, child.pid)?;
    Ok(child)
}

/// The relay worker: the repack's Python running `Scripts/manage.py relay-worker` (BugReport/relay.py).
fn spawn_relay(root: &Path) -> Result<Proc> {
    let python = squid::package_root(root).join("Runtime/python/python.exe");
    let manage = root.join("Scripts/manage.py");
    if !python.is_file() || !manage.is_file() {
        return Err(Error::Invalid("The bug-report relay needs the repack's Python and Scripts/manage.py.".into()));
    }
    let manage = manage.to_string_lossy().into_owned();
    spawn_child(root, "relay", &python, &["-B", &manage, "relay-worker"], root, &root.join("BugReport/Logs/relay.log"))
}

/// The supervisor (`watch-world`): run the world, keep the relay alive while it runs, stop the relay after it.
pub fn watch_world(root: &Path, log: Progress) -> Result<()> {
    squid::validate_bots(root)?;
    let world = spawn_child(root, "world", &root.join("Core/worldserver.exe"), &["-c", "configs/worldserver.conf"], &root.join("Core"), &root.join("Core/Logs/world-console.log"))?;
    let mut relay: Option<Proc> = None;
    while world.try_wait().is_none() {
        let dead = relay.as_ref().is_none_or(|r| r.try_wait().is_some());
        if dead {
            match spawn_relay(root) {
                Ok(r) => relay = Some(r),
                Err(e) => {
                    log(&format!("Report relay could not start ({e}); retrying."));
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    std::fs::write(stop_relay_flag(root), b"")?;
    if let Some(r) = relay {
        let until = Instant::now() + Duration::from_secs(40);
        while r.try_wait().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
        }
        if r.try_wait().is_none() {
            // An interrupted POST stays uncertain in the relay's journal; it is not resent.
            if let Some(record) = state::running(root, "relay") {
                services::terminate(&record)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Detailed logging (Debug_Logs_On.bat / Debug_Logs_Off.bat)
// ---------------------------------------------------------------------------------------------------------------

const DEBUG_TEMPLATES: [&str; 2] = ["worldserver.conf.template", "authserver.conf.template"];

/// One template line with every logger forced to Trace and the console/server appenders to the most detail.
fn debug_line(line: &str) -> String {
    let body = line.strip_prefix('#').unwrap_or(line).trim_start();
    for appender in ["Appender.Console", "Appender.Server"] {
        if let Some(rest) = body.strip_prefix(appender).and_then(|r| r.strip_prefix('=')) {
            let parts: Vec<&str> = rest.splitn(4, ',').collect();
            if parts.len() == 4 && parts[..3].iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
                return format!("{appender}={},6,7,{}", parts[0], parts[3]);
            }
        }
    }
    if let Some(rest) = body.strip_prefix("Logger.") {
        if let Some((name, value)) = rest.split_once('=') {
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.') {
                if let Some((level, tail)) = value.split_once(',') {
                    if !level.is_empty() && level.bytes().all(|b| b.is_ascii_digit()) {
                        return format!("Logger.{name}=6,{tail}");
                    }
                }
            }
        }
    }
    let t = line.trim_end();
    if let Some(v) = t.strip_prefix("Log.Async.Enable") {
        let v = v.trim_start();
        if let Some(n) = v.strip_prefix('=') {
            let n = n.trim();
            if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
                return "Log.Async.Enable = 1".into();
            }
        }
    }
    line.to_string()
}

/// Turn detailed logging on or off (the templates; services apply it at their next start). Returns the changed
/// templates.
pub fn debug_logs(root: &Path, enable: bool) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for name in DEBUG_TEMPLATES {
        let path = root.join("Settings").join(name);
        let original = root.join("Settings").join(format!("{name}.original"));
        if !original.exists() {
            std::fs::write(&original, std::fs::read(&path)?)?;
        }
        let new_bytes = if enable {
            let text = String::from_utf8(std::fs::read(&original)?).map_err(|_| Error::Invalid(format!("{name} is not UTF-8.")))?;
            let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let mut out = text.lines().map(debug_line).collect::<Vec<_>>().join(newline);
            if text.ends_with('\n') {
                out.push_str(newline);
            }
            out.into_bytes()
        } else {
            std::fs::read(&original)?
        };
        if std::fs::read(&path)? != new_bytes {
            std::fs::write(&path, &new_bytes)?;
            changed.push(name.to_string());
        }
    }
    state::write_json(&state::dir(root).join("debug-logs.json"), &serde_json::json!({ "enabled": enable }))?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detailed_logging_rewrites_loggers_and_restores_the_original() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Settings")).unwrap();
        let world = "Appender.Console=1,3,0\r\nAppender.Server=2,2,0,Server.log,w\r\n#Logger.module=4,Console Server\r\nLogger.root=5,Console Server\r\nLog.Async.Enable = 0\r\nOther = 1\r\n";
        std::fs::write(d.path().join("Settings/worldserver.conf.template"), world).unwrap();
        std::fs::write(d.path().join("Settings/authserver.conf.template"), "Logger.root=2,Console\n").unwrap();
        let changed = debug_logs(d.path(), true).unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(
            std::fs::read_to_string(d.path().join("Settings/worldserver.conf.template")).unwrap(),
            "Appender.Console=1,3,0\r\nAppender.Server=2,6,7,Server.log,w\r\nLogger.module=6,Console Server\r\nLogger.root=6,Console Server\r\nLog.Async.Enable = 1\r\nOther = 1\r\n"
        );
        assert_eq!(debug_logs(d.path(), true).unwrap().len(), 0, "already on");
        debug_logs(d.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(d.path().join("Settings/worldserver.conf.template")).unwrap(), world);
        assert_eq!(services::status(d.path())["debugLogs"], false);
    }
}
