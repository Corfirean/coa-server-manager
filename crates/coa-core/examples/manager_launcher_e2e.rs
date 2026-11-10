//! The Manager's start/stop (`driver`) on the Rust launcher, on a DISPOSABLE repack:
//!
//! 1. driver start-all: every service runs, the console answers, manage.py is not touched and sees the services;
//! 2. driver stop-all stops everything;
//! 3. manage.py start-all, then driver stop-all stops what Python started;
//! 4. a taken world port fails with the typed "port in use" code (no stderr parsing), then stop-all;
//! 5. driver start-all, then the forced stop;
//! 6. with a realm profile: the realm row gets the profile's name, the .bat launcher is adapted as before;
//! 7. nothing of the repack is left running.
//!
//! usage: manager_launcher_e2e <existing folder ending in -fixture>
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use coa_core::driver::{self, Verb};
use coa_core::error::ErrorCode;
use coa_core::launcher::{services, settings::Settings, state};

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

fn drive(root: &Path, verb: Verb) -> driver::DriverOutcome {
    let t = Instant::now();
    let out = driver::run(root, verb).expect("driver");
    println!("[driver {verb:?}] ok={} code={:?} in {:?}\n{}", out.ok, out.code, t.elapsed(), indent(&out.output));
    out
}

fn manage(root: &Path, action: &str) -> (bool, String) {
    let python = root.join("Runtime/python/python.exe");
    let script = root.join("Scripts/manage.py").to_string_lossy().into_owned();
    let t = Instant::now();
    let out = Command::new(python).args(["-B", &script, action]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    println!("[manage.py {action}] ok={} in {:?}\n{}", out.status.success(), t.elapsed(), indent(&text));
    (out.status.success(), text)
}

fn python_status(root: &Path) -> serde_json::Value {
    let (ok, out) = manage(root, "status-json");
    assert!(ok);
    let line = out.lines().rev().find(|l| l.trim_start().starts_with('{')).expect("a JSON status line");
    serde_json::from_str(line).unwrap()
}

fn all(v: &serde_json::Value, running: bool) -> bool {
    ["mysql", "auth", "world", "relay"].iter().all(|k| v[*k] == running)
}

fn nothing_running(root: &Path) -> bool {
    ["mysql", "auth", "world", "supervisor", "relay"].iter().all(|n| state::running(root, n).is_none())
        && ["mysql/bin/mysqld.exe", "Core/authserver.exe", "Core/worldserver.exe"]
            .iter()
            .all(|exe| coa_core::process::find_by_exe(&root.join(exe)).is_empty())
}

fn console(root: &Path) -> String {
    let cfg = Settings::load(root, 0).unwrap();
    let mut ra = coa_core::ra::Ra::connect_to(cfg.ra_port, &cfg.ra_username, &cfg.ra_password).unwrap();
    ra.run("server info").unwrap()
}

fn realm_name(root: &Path) -> String {
    services::mysql(root, Some("SELECT name FROM acore_auth.realmlist WHERE id=1;"), None).unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&a[0]);
    assert!(a[0].ends_with("-fixture"), "a folder ending in -fixture is required");
    assert!(nothing_running(root), "the fixture must be stopped before the test");
    let manage_py = root.join("Scripts/manage.py");
    let original = std::fs::read(&manage_py).unwrap();
    let profile = root.join("Settings/realm-profile.json");
    assert!(!profile.exists(), "the fixture starts without a realm profile");

    // 1
    let started = drive(root, Verb::StartAll);
    assert!(started.ok, "start-all");
    assert!(all(&python_status(root), true), "manage.py sees what the Manager started");
    let info = console(root);
    println!("[console] {}", info.lines().next().unwrap_or(""));
    assert_eq!(std::fs::read(&manage_py).unwrap(), original, "manage.py is not rewritten");
    assert_eq!(realm_name(root), "AzerothCore", "no profile: the launcher's own realm name");

    let shipped = root.join("Scripts/coa-launcher.exe");
    let supervisor = state::running(root, "supervisor").expect("a supervisor");
    println!("[supervisor] {}", supervisor.exe);
    let expected = if shipped.is_file() { "coa-launcher.exe" } else { "python.exe" };
    assert!(supervisor.exe.to_lowercase().ends_with(expected), "the world is supervised by {expected}");
    let relay = state::running(root, "relay").expect("a bug-report relay");
    println!("[relay] {}", relay.exe);
    assert!(relay.exe.to_lowercase().ends_with(expected), "the relay is run by {expected}");
    assert!(state::running(root, "relay-ready").is_some_and(|r| r.pid == relay.pid), "the relay reported ready");

    // 2
    assert!(drive(root, Verb::StopAll).ok, "stop-all");
    assert!(nothing_running(root), "1-2 left something running");

    // 2b: the repack's Python launcher stops a world the Manager started
    assert!(drive(root, Verb::StartAll).ok);
    assert!(manage(root, "stop-all").0, "manage.py stops what the Manager started");
    assert!(nothing_running(root), "2b left something running");

    // 3
    assert!(manage(root, "start-all").0);
    assert!(drive(root, Verb::StopAll).ok, "driver stops what manage.py started");
    assert!(nothing_running(root), "3 left something running");

    // 4
    let cfg = Settings::load(root, 0).unwrap();
    let blocker = std::net::TcpListener::bind(("127.0.0.1", cfg.world_port)).unwrap();
    let refused = drive(root, Verb::StartAll);
    assert!(!refused.ok);
    assert_eq!(refused.code, Some(ErrorCode::PortInUse), "typed port error");
    assert!(refused.output.contains(&format!("Port {}", cfg.world_port)));
    drop(blocker);
    assert!(drive(root, Verb::StopAll).ok);
    assert!(nothing_running(root), "4 left something running");

    // 5
    assert!(drive(root, Verb::StartAll).ok);
    let t = Instant::now();
    let forced = driver::force_stop(root).expect("force stop");
    println!("[driver force_stop] ok={} in {:?}\n{}", forced.ok, t.elapsed(), indent(&forced.output));
    assert!(forced.ok && forced.output.contains("Force-stopped"));
    assert!(nothing_running(root), "5 left something running");

    // 6
    std::fs::write(&profile, br#"{"active":"coa","wildcard_created":false}"#).unwrap();
    let result = std::panic::catch_unwind(|| {
        assert!(drive(root, Verb::StartAll).ok, "start-all with a realm profile");
        assert_eq!(realm_name(root), "Conquest of Azeroth");
        let patched = std::fs::read_to_string(&manage_py).unwrap();
        assert!(patched.contains("# coa-manager-realm-profiles-v2"), "the .bat launcher still follows the realm");
        let fix = "sys.path.insert(0, str(Path(__file__).resolve().parent))";
        assert_eq!(
            patched.matches(fix).count(),
            String::from_utf8_lossy(&original).matches(fix).count(),
            "no bridge import fix added"
        );
        assert!(drive(root, Verb::StopAll).ok);
    });
    std::fs::remove_file(&profile).unwrap();
    std::fs::write(&manage_py, &original).unwrap();
    if result.is_err() {
        let _ = driver::run(root, Verb::StopAll);
        panic!("step 6 failed");
    }

    // 7
    assert!(nothing_running(root), "something of the repack is still running");
    println!("PASS: the Manager starts and stops through the Rust launcher, interoperates with manage.py, typed errors");
}
