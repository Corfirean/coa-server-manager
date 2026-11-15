//! Start/stop of a repack-shaped installation through the repack launcher in this crate
//! ([`crate::launcher`]): config rendering, the services, the world supervisor, graceful shutdown and readiness.
//! It writes the same `.state` records as the repack's `Scripts/manage.py`, so the repack's own launcher and
//! this Manager can be used on the same folder. Observation stays native (see `process`).

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::error::{Error, ErrorCode, Result};
use crate::fsx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verb {
    StartAll,
    StartMysql,
    StartWorld,
    StopAll,
}

impl Verb {
    fn action(self) -> crate::launcher::ops::Action {
        use crate::launcher::ops::Action;
        match self {
            Verb::StartAll => Action::StartAll,
            Verb::StartMysql => Action::StartMysql,
            Verb::StartWorld => Action::StartWorld,
            Verb::StopAll => Action::StopAll,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DriverOutcome {
    pub ok: bool,
    pub exit_code: Option<i32>,
    /// Human-facing classification of a failure.
    pub code: Option<ErrorCode>,
    pub human: Option<crate::error::Human>,
    pub output: String,
}

fn has_launcher_settings(root: &Path) -> Result<()> {
    if !root.join("Settings/repack.json").is_file() {
        return Err(Error::Invalid(
            "This server has no CoA Repack launcher; start/stop is not available for it yet."
                .into(),
        ));
    }
    Ok(())
}

/// Run a launcher verb to completion (blocking; call from a worker thread).
pub fn run(root: &Path, verb: Verb) -> Result<DriverOutcome> {
    tracing::info!(?verb, root = %root.display(), "driver: launcher requested");
    let result = run_inner(root, verb, false);
    if let Err(error) = &result {
        tracing::warn!(?verb, root = %root.display(), %error, "driver: launcher request failed before completion");
    }
    result
}

pub(crate) fn validate_update(root: &Path) -> Result<DriverOutcome> {
    run_inner(root, Verb::StartAll, true)
}

pub fn force_stop(root: &Path) -> Result<DriverOutcome> {
    if crate::docker::is_docker(root) {
        return Err(Error::Invalid(
            "Forced shutdown is only available for Windows repacks.".into(),
        ));
    }
    for tree in crate::multiworld::Tree::ALL {
        let path = crate::multiworld::tree_root(root, tree);
        if crate::multiworld::tree_running(root, tree)
            || !crate::process::find_by_exe(&path.join("Core/worldserver.exe")).is_empty()
        {
            let child = fsx::ensure_within(root, &path)?;
            let out = run_inner_mode(&child, Verb::StopAll, false, true)?;
            if !out.ok || crate::multiworld::tree_running(root, tree) {
                return Err(Error::Invalid(format!(
                    "An extra world could not stop; the shared database was left running. {}",
                    out.output
                )));
            }
        }
    }
    run_inner_mode(root, Verb::StopAll, false, true)
}

fn recover_world_for_forced_stop(root: &Path) -> Result<()> {
    let worlds = crate::process::find_by_exe(&root.join("Core/worldserver.exe"));
    if worlds.len() > 1 {
        return Err(Error::Invalid(
            "More than one world process belongs to this folder; its database was left running."
                .into(),
        ));
    }
    if let Some(world) = worlds.first() {
        if !crate::process::is_alive(world) {
            return Err(Error::Invalid(
                "The world process identity changed; retry the stop.".into(),
            ));
        }
        if crate::process::read_state_record(root, "world")
            .is_none_or(|saved| !saved.same_as(world))
        {
            let record = root.join(".state/world.json");
            let previous = root.join(".state/world-before-forced-stop-recovery.json");
            if record.is_file() && !previous.exists() {
                fsx::atomic_write(&previous, &std::fs::read(&record)?)?;
            }
            fsx::atomic_write_json(&record, world)?;
            tracing::warn!(pid = world.pid, root = %root.display(), "Recovered owned world identity before an explicitly requested forced stop");
        }
    }
    Ok(())
}

fn run_inner(root: &Path, verb: Verb, validating_update: bool) -> Result<DriverOutcome> {
    run_inner_mode(root, verb, validating_update, false)
}

fn run_inner_mode(
    root: &Path,
    verb: Verb,
    validating_update: bool,
    force: bool,
) -> Result<DriverOutcome> {
    let root = fsx::canonicalize_lenient(root)?;
    let _update_lock = if !validating_update && matches!(verb, Verb::StartAll | Verb::StartWorld) {
        Some(crate::update::operation_lock_for(
            &crate::registry::metadata_dir_for(&root)?,
            "starting server",
            true,
        )?)
    } else {
        None
    };
    if !validating_update && matches!(verb, Verb::StartAll | Verb::StartWorld) {
        crate::update::ensure_recovered(&crate::registry::metadata_dir_for(&root)?)?;
    }
    if matches!(verb, Verb::StartAll | Verb::StartWorld) {
        crate::modules::ensure_bot_exclusivity(&root)?;
    }
    if crate::docker::is_docker(&root) {
        prepare_headless_world(&root, verb)?;
        return crate::docker::run(&root, verb);
    }
    if verb == Verb::StopAll && !force {
        crate::multiworld::stop(&root)?;
    }
    if verb == Verb::StartAll {
        crate::realms::before_start(&root)?;
        if root.join("Settings/realm-profile.json").exists() {
            let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin)?;
            if !db.ping() {
                let out = run(&root, Verb::StartMysql)?;
                if !out.ok {
                    return Ok(out);
                }
            }
            crate::realms::setup_realmlist(&root)?;
        }
    }
    if matches!(verb, Verb::StartAll | Verb::StartWorld) {
        crate::modules::ensure_bot_exclusivity(&root)?;
    }
    has_launcher_settings(&root)?;
    prepare_headless_world(&root, verb)?;
    if matches!(verb, Verb::StopAll | Verb::StartMysql | Verb::StartAll) {
        recover_mysql_launcher_state(&root, verb != Verb::StopAll)?;
    }
    if force {
        recover_world_for_forced_stop(&root)?;
    }
    let action = if force {
        crate::launcher::ops::Action::ForceStopAll
    } else {
        verb.action()
    };
    tracing::info!(?verb, root = %root.display(), "driver: starting launcher action");
    let lines = Mutex::new(Vec::<String>::new());
    let progress = |line: &str| {
        tracing::info!(?verb, "launcher: {line}");
        lines.lock().unwrap_or_else(|e| e.into_inner()).push(line.to_string());
    };
    let result = crate::launcher::Settings::load(&root, 0).and_then(|mut cfg| {
        if validating_update {
            // Isolated startup must use the port from its temporary auth config.
            cfg.env_remove.push("AC_REALM_SERVER_PORT".into());
        }
        crate::launcher::ops::run(&root, &cfg, 0, action, None, &progress)
    });
    let mut output = lines.into_inner().unwrap_or_else(|e| e.into_inner()).join("\n");
    let ok = result.is_ok();
    let code = match &result {
        Ok(()) => None,
        Err(error) => {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("ERROR: {error}"));
            // Log evidence beats a generic "stopped during startup".
            match error.code() {
                ErrorCode::StartupFailed | ErrorCode::Unknown => Some(
                    crate::health::diagnose_installation(&root).unwrap_or(error.code()),
                ),
                other => Some(other),
            }
        }
    };
    let exit_code = Some(if ok { 0 } else { 1 });
    if code == Some(ErrorCode::PortInUse) {
        let observed = crate::process::observe(&root, &crate::layout::read_ports(&root));
        for service in [&observed.mysql, &observed.auth, &observed.world]
            .into_iter()
            .chain(observed.extra_worlds())
        {
            if let Some(conflict) = &service.conflict {
                output.push_str(&format!(
                    "\nPort {} is used by process {} ({}).",
                    conflict.port,
                    conflict.pid,
                    conflict.exe.as_deref().unwrap_or("executable unavailable")
                ));
            }
        }
    }
    tracing::info!(?verb, ok, ?code, "driver: launcher verb finished");
    if !ok && !output.is_empty() {
        let redacted = crate::diag::redact(&output);
        let tail: Vec<_> = redacted.lines().rev().take(12).collect();
        tracing::warn!(?verb, exit_code, output = %tail.into_iter().rev().collect::<Vec<_>>().join("\n"), "driver: launcher verb failed");
    }
    if ok && verb == Verb::StartAll {
        crate::multiworld::start(&root)?;
    }
    Ok(DriverOutcome {
        ok,
        exit_code,
        code,
        human: code.map(ErrorCode::human),
        output,
    })
}

/// Background servers receive no interactive stdin. Keep CLI EOF from stopping the world server;
/// the Manager's console uses remote access instead. The template survives launcher regeneration.
fn prepare_headless_world(root: &Path, verb: Verb) -> Result<()> {
    if !matches!(verb, Verb::StartAll | Verb::StartWorld) {
        return Ok(());
    }
    std::fs::create_dir_all(root.join("Core/Crashes"))?;
    let probe = tempfile::NamedTempFile::new_in(root.join("Core/Crashes"))?;
    probe.close()?;
    for relative in [
        "Settings/worldserver.conf.template",
        "Core/configs/worldserver.conf",
    ] {
        let path = root.join(relative);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut config = crate::config::parser::ConfFile::parse_bytes(&bytes)?;
        config.set(
            "Console.Enable",
            "0",
            &["Local console is disabled for background startup; the Manager uses remote access."],
        );
        let text = config.to_text();
        if text.as_bytes() != bytes {
            fsx::atomic_write(&path, text.as_bytes())?;
        }
    }
    Ok(())
}

/// A lost launcher record must not hide this installation's running MySQL from
/// graceful shutdown. Adopt only the exact executable owning its configured port.
fn recover_mysql_launcher_state(root: &Path, prefer_listener: bool) -> Result<()> {
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if observed.mysql.state != crate::process::ServiceState::Running || !observed.mysql.port_ready {
        return Ok(());
    }
    let Some(pid) = observed.mysql.pid else {
        return Ok(());
    };
    let Some(identity) = crate::process::identity(pid) else {
        return Ok(());
    };
    let (_, _, executable) = crate::layout::executables(root);
    if !crate::process::find_by_exe(&executable)
        .iter()
        .any(|candidate| candidate.same_as(&identity))
        || !crate::process::listeners().contains(&(observed.mysql.port, pid))
    {
        return Ok(());
    }
    if let Some(saved) = crate::process::read_state_record(root, "mysql") {
        if saved.same_as(&identity) {
            return Ok(());
        }
        if crate::process::is_alive(&saved) {
            // A valid record may describe the MySQL supervisor rather than its
            // listening child. The launcher can gracefully stop either record.
            if crate::process::find_by_exe(&executable)
                .iter()
                .any(|candidate| candidate.same_as(&saved))
            {
                if !prefer_listener {
                    return Ok(());
                }
            } else {
                return Err(Error::Invalid("The MySQL launcher record identifies a different live process. No database action was requested.".into()));
            }
        }
    }
    let state = root.join(".state/mysql.json");
    let previous = root.join(".state/mysql-before-shutdown-recovery.json");
    if state.is_file() && !previous.exists() {
        fsx::atomic_write(&previous, &std::fs::read(&state)?)?;
    }
    fsx::atomic_write_json(&state, &identity)?;
    tracing::warn!(pid, root = %root.display(), "Recovered MySQL launcher identity from the installed executable owning its port");
    Ok(())
}

pub(crate) fn startup_failure(root: &Path, started: &DriverOutcome) -> String {
    let code = started
        .code
        .filter(|c| *c != ErrorCode::Unknown)
        .or_else(|| crate::health::diagnose_installation(root));
    let title = code
        .map(|c| c.human().title)
        .unwrap_or("The server did not become ready");
    let output = crate::diag::redact(&started.output);
    let lines: Vec<_> = output.lines().rev().take(12).collect();
    let mut reason = if lines.is_empty() {
        title.into()
    } else {
        format!(
            "{title}\n{}",
            lines.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
    };
    for relative in [
        "Core/Logs/world-console.log",
        "Core/Logs/auth-console.log",
        "mysql/logs/mysql-error.log",
    ] {
        let tail = crate::health::tail(&root.join(relative), 64 * 1024);
        let failures: Vec<_> = tail
            .lines()
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                crate::health::diagnose(line).is_some()
                    || lower.contains("fatal")
                    || lower.contains("error")
                    || lower.contains("aborting")
            })
            .rev()
            .take(8)
            .collect();
        if !failures.is_empty() {
            reason.push_str(&format!(
                "\n{relative}:\n{}",
                crate::diag::redact(&failures.into_iter().rev().collect::<Vec<_>>().join("\n"))
            ));
        }
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn shutdown_recovery_never_adopts_a_foreign_port_listener() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        fsx::atomic_write_json(
            &root.path().join("Settings/repack.json"),
            &serde_json::json!({"mysqlPort":listener.local_addr().unwrap().port()}),
        )
        .unwrap();
        recover_mysql_launcher_state(root.path(), false).unwrap();
        recover_mysql_launcher_state(root.path(), true).unwrap();
        assert!(!root.path().join(".state/mysql.json").exists());
        assert!(crate::process::identity(std::process::id()).is_some());
    }

    #[test]
    fn a_blocked_start_is_logged_before_any_launcher_is_spawned() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("server");
        std::fs::create_dir(&root).unwrap();
        let metadata = crate::registry::metadata_dir_for(&root).unwrap();
        let _maintenance = crate::update::operation_lock(&metadata).unwrap();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || Capture(Arc::clone(&writer)))
            .finish();
        let error = tracing::subscriber::with_default(subscriber, || run(&root, Verb::StartAll))
            .unwrap_err();
        assert!(error.to_string().contains("in progress"));
        let text = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(text.contains("launcher requested") && text.contains("StartAll"));
        assert!(
            text.contains("launcher request failed before completion")
                && text.contains("in progress")
        );
        assert!(!text.contains("starting launcher action"));
    }

    #[test]
    fn background_start_disables_cli_in_template_and_rendered_config() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::create_dir_all(root.join("Settings")).unwrap();
        std::fs::create_dir_all(root.join("Core/configs")).unwrap();
        let template = root.join("Settings/worldserver.conf.template");
        let rendered = root.join("Core/configs/worldserver.conf");
        let original = "\u{feff}# Custom settings\r\nConsole.Enable = 1\r\nRa.Enable = 1\r\nPlayerLimit = 42\r\n";
        std::fs::write(&template, original).unwrap();
        std::fs::write(&rendered, original).unwrap();
        let auth = root.join("Core/configs/authserver.conf");
        std::fs::write(&auth, "custom auth config").unwrap();
        for verb in [Verb::StartMysql, Verb::StopAll] {
            prepare_headless_world(root, verb).unwrap();
            assert_eq!(std::fs::read_to_string(&template).unwrap(), original);
            assert_eq!(std::fs::read_to_string(&rendered).unwrap(), original);
        }
        for verb in [Verb::StartAll, Verb::StartWorld] {
            std::fs::write(&template, original).unwrap();
            std::fs::write(&rendered, original).unwrap();
            prepare_headless_world(root, verb).unwrap();
            let expected = original.replace("Console.Enable = 1", "Console.Enable = 0");
            assert_eq!(std::fs::read_to_string(&template).unwrap(), expected);
            assert_eq!(std::fs::read_to_string(&rendered).unwrap(), expected);
            prepare_headless_world(root, verb).unwrap();
            assert_eq!(std::fs::read_to_string(&template).unwrap(), expected);
        }
        assert_eq!(std::fs::read_to_string(auth).unwrap(), "custom auth config");
    }

    #[test]
    fn background_start_overrides_an_absent_or_duplicate_console_setting() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        prepare_headless_world(root, Verb::StartAll).unwrap();
        assert!(root.join("Core/Crashes").is_dir());
        std::fs::create_dir_all(root.join("Core/configs")).unwrap();
        let path = root.join("Core/configs/worldserver.conf");
        for original in [
            "Ra.Enable = 1\n",
            "Console.Enable = 0\nConsole.Enable = 1\n",
        ] {
            std::fs::write(&path, original).unwrap();
            prepare_headless_world(root, Verb::StartAll).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let config = crate::config::parser::ConfFile::parse_bytes(&bytes).unwrap();
            assert_eq!(config.get("Console.Enable"), Some("0"));
            prepare_headless_world(root, Verb::StartAll).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn launcher_failures_carry_their_error_code() {
        let e = Error::Service(ErrorCode::PortInUse, "Port 8085 is already used (PID 4).".into());
        assert_eq!(e.code(), ErrorCode::PortInUse);
        assert_eq!(e.to_string(), "Port 8085 is already used (PID 4).");
    }

    #[test]
    fn refuses_folder_without_launcher() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), Verb::StartAll).is_err());
    }

    #[test]
    fn startup_failure_reports_late_world_errors_after_launcher_ready() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Core/Logs")).unwrap();
        std::fs::write(dir.path().join("Core/Logs/world-console.log"), "ERROR [1146] Table 'acore_characters.playerbots_names' doesn't exist\nFATAL Could not prepare statements\n").unwrap();
        let started = DriverOutcome {
            ok: true,
            code: None,
            exit_code: Some(0),
            human: None,
            output: "Worldserver and automatic bug reporting are ready.".into(),
        };
        let reason = startup_failure(dir.path(), &started);
        assert!(reason.contains("playerbots_names"));
        assert!(reason.contains("Core/Logs/world-console.log"));
        assert!(reason.contains("Could not prepare statements"));
    }

    #[test]
    fn startup_failure_keeps_the_launcher_reason_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let started = DriverOutcome {
            ok: false,
            exit_code: Some(1),
            code: Some(ErrorCode::Unknown),
            human: Some(ErrorCode::Unknown.human()),
            output: "appPassword=private\nModuleNotFoundError: No module named 'squid_playerbots'"
                .into(),
        };
        let reason = startup_failure(dir.path(), &started);
        assert!(reason.contains("ModuleNotFoundError"));
        assert!(!reason.contains("private"));
        assert!(!reason.contains("Something went wrong"));
    }
}
