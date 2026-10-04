//! Start/stop of a repack-shaped installation by delegating to the repack's own supervisor
//! (`Runtime\python\python.exe Scripts\manage.py <verb>`), which already implements config rendering, the
//! bug-report relay, RA graceful shutdown and readiness. Observation stays native (see `process`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    fn arg(self) -> &'static str {
        match self {
            Verb::StartAll => "start-all",
            Verb::StartMysql => "start-mysql",
            Verb::StartWorld => "start-world",
            Verb::StopAll => "stop-all",
        }
    }

    fn timeout(self) -> Duration {
        match self {
            Verb::StartAll | Verb::StartWorld => Duration::from_secs(420),
            Verb::StartMysql => Duration::from_secs(120),
            Verb::StopAll => Duration::from_secs(240),
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

/// Map the launcher's own messages onto stable error codes. Unknown text stays `Unknown` (details remain visible).
pub fn translate(output: &str) -> ErrorCode {
    let o = output.to_lowercase();
    if o.contains("is already used") {
        ErrorCode::PortInUse
    } else if o.contains("another start/stop action is in progress") || o.contains("shutdown is still finishing") {
        ErrorCode::OperationInProgress
    } else if o.contains("packaged database is missing") || o.contains("extract the complete repack") {
        ErrorCode::ServerFilesIncomplete
    } else if o.contains("stopped during startup") || o.contains("is still starting") || o.contains("world startup failed") {
        ErrorCode::StartupFailed
    } else if o.contains("invalid port") || o.contains("unresolved placeholder") {
        ErrorCode::InvalidConfigValue
    } else {
        ErrorCode::Unknown
    }
}

fn launcher(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let python = root.join("Runtime/python/python.exe");
    let script = root.join("Scripts/manage.py");
    if !python.is_file() || !script.is_file() {
        return Err(Error::Invalid(
            "This server has no CoA Repack launcher; start/stop is not available for it yet.".into(),
        ));
    }
    Ok((python, script))
}

/// Run a launcher verb to completion (blocking; call from a worker thread).
pub fn run(root: &Path, verb: Verb) -> Result<DriverOutcome> {
    run_inner(root, verb, false)
}

pub(crate) fn validate_update(root: &Path) -> Result<DriverOutcome> {
    run_inner(root, Verb::StartAll, true)
}

fn run_inner(root: &Path, verb: Verb, validating_update: bool) -> Result<DriverOutcome> {
    let root = fsx::canonicalize_lenient(root)?;
    let _update_lock = if !validating_update && matches!(verb, Verb::StartAll | Verb::StartWorld) {
        Some(crate::update::operation_lock(&crate::registry::metadata_dir_for(&root)?)?)
    } else { None };
    if !validating_update && matches!(verb, Verb::StartAll | Verb::StartWorld) {
        crate::update::ensure_recovered(&crate::registry::metadata_dir_for(&root)?)?;
    }
    if matches!(verb, Verb::StartAll | Verb::StartWorld) { crate::modules::ensure_bot_exclusivity(&root)?; }
    if crate::docker::is_docker(&root) {
        return crate::docker::run(&root, verb);
    }
    if verb == Verb::StopAll { crate::multiworld::stop(&root)?; }
    if verb == Verb::StartAll {
        crate::realms::before_start(&root)?;
        if root.join("Settings/realm-profile.json").exists() {
            let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin)?;
            if !db.ping() {
                let out = run(&root, Verb::StartMysql)?;
                if !out.ok { return Ok(out); }
            }
            crate::realms::setup_realmlist(&root)?;
        }
    }
    if matches!(verb, Verb::StartAll | Verb::StartWorld) { crate::modules::ensure_bot_exclusivity(&root)?; }
    let (python, script) = launcher(&root)?;
    let mut cmd = Command::new(&python);
    cmd.arg("-B").arg(&script).arg(verb.arg()).current_dir(&root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    tracing::info!(?verb, root = %root.display(), "driver: starting launcher verb");
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + verb.timeout();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if Instant::now() > deadline {
            // Never kill the servers themselves: only abandon the launcher process we spawned.
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut output = out_t.join().unwrap_or_default();
    output.push_str(&err_t.join().unwrap_or_default());
    let output = output.trim().to_string();
    let (ok, exit_code) = match status {
        Some(st) => (st.success(), st.code()),
        None => (false, None),
    };
    let code = if ok {
        None
    } else {
        // Log evidence beats the launcher's generic "stopped during startup" message.
        let base = if status.is_none() { ErrorCode::StartupFailed } else { translate(&output) };
        match base {
            ErrorCode::StartupFailed | ErrorCode::Unknown => Some(crate::health::diagnose_installation(&root).unwrap_or(base)),
            other => Some(other),
        }
    };
    tracing::info!(?verb, ok, ?code, "driver: launcher verb finished");
    if ok && verb == Verb::StartAll { crate::multiworld::start(&root)?; }
    Ok(DriverOutcome { ok, exit_code, code, human: code.map(ErrorCode::human), output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_known_launcher_messages() {
        assert_eq!(translate("RuntimeError: Port 8085 is already used. Stop the other server"), ErrorCode::PortInUse);
        assert_eq!(translate("Another start/stop action is in progress."), ErrorCode::OperationInProgress);
        assert_eq!(translate("The packaged database is missing. Extract the complete repack."), ErrorCode::ServerFilesIncomplete);
        assert_eq!(translate("world stopped during startup. Check its log."), ErrorCode::StartupFailed);
        assert_eq!(translate("Exit code -1073741819"), ErrorCode::Unknown);
    }

    #[test]
    fn refuses_folder_without_launcher() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), Verb::StartAll).is_err());
    }
}
