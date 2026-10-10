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

    fn timeout(self, root: &Path) -> Duration {
        match self {
            Verb::StartAll | Verb::StartWorld if squid_enabled(root) => Duration::from_secs(2700),
            Verb::StartAll | Verb::StartWorld => Duration::from_secs(420),
            Verb::StartMysql => Duration::from_secs(120),
            Verb::StopAll => Duration::from_secs(240),
        }
    }
}

fn squid_enabled(root: &Path) -> bool {
    let active = root.join("Core/configs/modules/playerbots.conf");
    let path = if active.exists() {
        active
    } else {
        root.join("Core/configs/modules/playerbots.conf.dist")
    };
    std::fs::read(path)
        .ok()
        .and_then(|bytes| crate::config::parser::ConfFile::parse_bytes(&bytes).ok())
        .is_some_and(|config| {
            config.get("AiPlayerbot.Enabled").is_none_or(|value| {
                !matches!(
                    value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
        })
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
    } else if o.contains("another start/stop action is in progress")
        || o.contains("shutdown is still finishing")
    {
        ErrorCode::OperationInProgress
    } else if o.contains("packaged database is missing")
        || o.contains("extract the complete repack")
    {
        ErrorCode::ServerFilesIncomplete
    } else if o.contains("stopped during startup")
        || o.contains("is still starting")
        || o.contains("world startup failed")
    {
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
            "This server has no CoA Repack launcher; start/stop is not available for it yet."
                .into(),
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
        Some(crate::update::operation_lock(
            &crate::registry::metadata_dir_for(&root)?,
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
    if verb == Verb::StopAll {
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
    let (python, script) = launcher(&root)?;
    prepare_headless_world(&root, verb)?;
    let original = std::fs::read_to_string(&script)?;
    let patched = patch_launcher_imports(&original)?;
    if original != patched {
        fsx::atomic_write(&script, patched.as_bytes())?;
        if let Ok(dir) = crate::registry::metadata_dir_for(&root) {
            if let Ok((_, mut meta)) = crate::registry::MetaDir::open(&dir) {
                if meta.original_hashes.get("Scripts/manage.py")
                    == Some(&fsx::sha256_bytes(original.as_bytes()))
                {
                    meta.original_hashes.insert(
                        "Scripts/manage.py".into(),
                        fsx::sha256_bytes(patched.as_bytes()),
                    );
                    fsx::atomic_write_json(&dir.join("install.json"), &meta)?;
                }
            }
        }
    }
    discard_damaged_launcher_state(&root);
    let mut cmd = Command::new(&python);
    cmd.arg("-B")
        .arg("-c")
        .arg(LAUNCH_SCRIPT)
        .arg(&script)
        .arg(verb.arg())
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let deadline = Instant::now() + verb.timeout(&root);
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
        let base = if status.is_none() {
            ErrorCode::StartupFailed
        } else {
            translate(&output)
        };
        match base {
            ErrorCode::StartupFailed | ErrorCode::Unknown => {
                Some(crate::health::diagnose_installation(&root).unwrap_or(base))
            }
            other => Some(other),
        }
    };
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

/// The launcher keeps one JSON record per service in `.state`. A crash or power loss while it writes one can leave
/// a file of the right length that holds only zero bytes (NTFS keeps the length but not the data), and the launcher
/// then dies on every start with an unreadable record. Such a file carries no information, so remove it: the
/// launcher treats a missing record as "not running". Only empty or all-zero `*.json` files are touched.
pub(crate) fn discard_damaged_launcher_state(root: &Path) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join(".state")) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("json"))
            || !path.is_file()
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.iter().all(|b| *b == 0) && std::fs::remove_file(&path).is_ok() {
            tracing::warn!(file = %path.display(), bytes = bytes.len(), "driver: removed a launcher state file damaged by an interrupted write");
            removed.push(path);
        }
    }
    removed
}

const LAUNCH_SCRIPT: &str = "import runpy,sys;from pathlib import Path;script=sys.argv[1];sys.path.insert(0,str(Path(script).resolve().parent));sys.argv=sys.argv[1:];runpy.run_path(script,run_name='__main__')";

pub(crate) fn patch_launcher_imports(text: &str) -> Result<String> {
    let line = "sys.path.insert(0, str(Path(__file__).resolve().parent))";
    if !text.contains("from squid_playerbots import") || text.contains(line) {
        return Ok(text.into());
    }
    let root = "ROOT = Path(__file__).resolve().parents[1]";
    if !text.contains(root) || !text.lines().any(|line| line.trim() == "import sys") {
        return Err(Error::Invalid(
            "The server launcher cannot load its integration scripts. Repair its program files."
                .into(),
        ));
    }
    Ok(text.replacen(root, &format!("{root}\n{line}"), 1))
}

pub(crate) fn launcher_matches(signed: &str, actual: &[u8]) -> bool {
    if signed.as_bytes() == actual {
        return true;
    }
    if patch_launcher_imports(signed).is_ok_and(|s| s.as_bytes() == actual) {
        return true;
    }
    crate::realms::patch_launcher(signed).is_ok_and(|s| {
        s.as_bytes() == actual || patch_launcher_imports(&s).is_ok_and(|p| p.as_bytes() == actual)
    })
}

/// Recover legacy metadata only when undoing supported Manager edits reproduces the recorded hash.
pub(crate) fn launcher_matches_recorded(recorded: &str, actual: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(actual) else {
        return false;
    };
    let without_import = text.replacen(
        "ROOT = Path(__file__).resolve().parents[1]\nsys.path.insert(0, str(Path(__file__).resolve().parent))",
        "ROOT = Path(__file__).resolve().parents[1]", 1,
    );
    for candidate in [
        Some(without_import.clone()),
        crate::realms::unpatch_launcher(&without_import),
    ]
    .into_iter()
    .flatten()
    {
        if crate::fsx::sha256_bytes(candidate.as_bytes()).eq_ignore_ascii_case(recorded)
            && launcher_matches(&candidate, actual)
        {
            return true;
        }
    }
    false
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
    if lines.is_empty() {
        title.into()
    } else {
        format!(
            "{title}\n{}",
            lines.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn large_squid_provisioning_has_time_to_finish_without_extending_shutdown() {
        let folder = tempfile::tempdir().unwrap();
        let modules = folder.path().join("Core/configs/modules");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(
            modules.join("playerbots.conf.dist"),
            "AiPlayerbot.Enabled = 1\n",
        )
        .unwrap();
        assert_eq!(
            Verb::StartAll.timeout(folder.path()),
            Duration::from_secs(2700)
        );
        assert_eq!(
            Verb::StopAll.timeout(folder.path()),
            Duration::from_secs(240)
        );
        std::fs::write(modules.join("playerbots.conf"), "AiPlayerbot.Enabled = 0\n").unwrap();
        assert_eq!(
            Verb::StartWorld.timeout(folder.path()),
            Duration::from_secs(420)
        );
    }

    #[test]
    fn zero_filled_launcher_state_is_discarded_and_valid_state_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let state = folder.path().join(".state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("mysql.json"), vec![0u8; 116]).unwrap();
        std::fs::write(state.join("configuration.json"), Vec::<u8>::new()).unwrap();
        std::fs::write(state.join("world.json"), "{\"pid\": 4}\n").unwrap();
        std::fs::write(state.join("stop-relay"), Vec::<u8>::new()).unwrap();
        std::fs::write(state.join("auth.json"), "{\"pid\": 0}").unwrap();
        let mut removed: Vec<_> = discard_damaged_launcher_state(folder.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        removed.sort();
        assert_eq!(removed, ["configuration.json", "mysql.json"]);
        assert!(
            state.join("world.json").exists()
                && state.join("auth.json").exists()
                && state.join("stop-relay").exists()
        );
        assert!(discard_damaged_launcher_state(folder.path()).is_empty());
        assert!(discard_damaged_launcher_state(&folder.path().join("missing")).is_empty());
    }

    #[test]
    fn translates_known_launcher_messages() {
        assert_eq!(
            translate("RuntimeError: Port 8085 is already used. Stop the other server"),
            ErrorCode::PortInUse
        );
        assert_eq!(
            translate("Another start/stop action is in progress."),
            ErrorCode::OperationInProgress
        );
        assert_eq!(
            translate("The packaged database is missing. Extract the complete repack."),
            ErrorCode::ServerFilesIncomplete
        );
        assert_eq!(
            translate("world stopped during startup. Check its log."),
            ErrorCode::StartupFailed
        );
        assert_eq!(translate("Exit code -1073741819"), ErrorCode::Unknown);
    }

    #[test]
    fn refuses_folder_without_launcher() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), Verb::StartAll).is_err());
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

    #[test]
    fn launcher_import_fix_is_idempotent_and_retains_other_code() {
        let text = "import sys\nROOT = Path(__file__).resolve().parents[1]\nfrom squid_playerbots import validate_bots\ncustom = 42\n";
        let fixed = patch_launcher_imports(text).unwrap();
        assert!(fixed.contains("sys.path.insert(0, str(Path(__file__).resolve().parent))"));
        assert!(fixed.ends_with("custom = 42\n"));
        assert_eq!(patch_launcher_imports(&fixed).unwrap(), fixed);
    }
}
