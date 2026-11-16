//! The Rust launcher on a DISPOSABLE repack, side by side with Scripts/manage.py:
//!
//! 1. install the stable server into a new folder on unused ports;
//! 2. coa-launcher start-all, then manage.py status-json must see every service;
//! 3. manage.py stop-all stops what coa-launcher started;
//! 4. manage.py start-all, coa-launcher status-json sees it, coa-launcher stop-all stops it;
//! 5. both stop-alls leave nothing of the repack running.
//!
//! usage: launcher_e2e <new folder ending in -fixture> <coa-launcher.exe>
use std::path::Path;
use std::process::Command;

use coa_core::install::{install_base, Params};
use coa_core::registry::Registry;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn run(exe: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(exe).args(args).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

fn launcher(root: &Path, exe: &Path, action: &str) -> (bool, String) {
    let r = root.to_string_lossy().into_owned();
    let t = std::time::Instant::now();
    let out = run(exe, &["--root", &r, action]);
    println!("[coa-launcher {action}] ok={} in {:?}\n{}", out.0, t.elapsed(), indent(&out.1));
    out
}

fn manage(root: &Path, action: &str) -> (bool, String) {
    let python = root.join("Runtime/python/python.exe");
    let script = root.join("Scripts/manage.py").to_string_lossy().into_owned();
    let t = std::time::Instant::now();
    let out = run(&python, &["-B", &script, action]);
    println!("[manage.py {action}] ok={} in {:?}\n{}", out.0, t.elapsed(), indent(&out.1));
    out
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

fn status(out: &str) -> serde_json::Value {
    let line = out.lines().rev().find(|l| l.trim_start().starts_with('{')).expect("a JSON status line");
    serde_json::from_str(line).unwrap()
}

fn all(v: &serde_json::Value, running: bool) -> bool {
    ["mysql", "auth", "world", "relay"].iter().all(|k| v[*k] == running)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&a[0]);
    let exe = Path::new(&a[1]);
    assert!(a[0].ends_with("-fixture"), "a folder ending in -fixture is required");
    if root.join("Core/worldserver.exe").is_file() {
        println!("reusing the installed fixture");
    } else {
        install(root);
    }
    test(root, exe);
}

fn install(root: &Path) {
    assert!(!root.exists(), "a new folder is required");
    let reg_dir = std::env::temp_dir().join("coa-launcher-e2e-registry");
    let _ = std::fs::remove_dir_all(&reg_dir);
    let reg = Registry::at(reg_dir.join("installs.json"));
    // what the Manager installs: the stable release's installation image
    let image = coa_core::install::Source::Installation(Box::new(coa_core::channels::resolve("stable").expect("stable channel")));
    let installed = install_base(
        &Params { source: image, dest: root.into(), trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY, registry: &reg, cancel: Default::default() },
        &|s| eprintln!("  {:>3}% {}", s.percent, s.step),
    )
    .expect("install");
    println!("installed {}", installed.version);
    let repack = root.join("Settings/repack.json");
    let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&repack).unwrap()).unwrap();
    for k in ["mysqlPort", "authPort", "worldPort", "raPort"] {
        cfg[k] = free_port().into();
    }
    std::fs::write(&repack, serde_json::to_vec_pretty(&cfg).unwrap()).unwrap();
}

fn test(root: &Path, exe: &Path) {
    // 2. Rust starts, Python sees it
    assert!(launcher(root, exe, "start-all").0, "coa-launcher start-all");
    let seen = status(&manage(root, "status-json").1);
    assert!(all(&seen, true), "manage.py must see the services coa-launcher started: {seen}");
    // 3. Python stops what Rust started
    assert!(manage(root, "stop-all").0, "manage.py stop-all");
    assert!(all(&status(&launcher(root, exe, "status-json").1), false));

    // 4. Python starts, Rust sees and stops it
    assert!(manage(root, "start-all").0, "manage.py start-all");
    let seen = status(&launcher(root, exe, "status-json").1);
    assert!(all(&seen, true), "coa-launcher must see the services manage.py started: {seen}");
    assert!(launcher(root, exe, "stop-all").0, "coa-launcher stop-all");
    assert!(all(&status(&manage(root, "status-json").1), false));

    // 5. Rust alone, a second time (start on a server it stopped)
    assert!(launcher(root, exe, "start-all").0);
    assert!(launcher(root, exe, "stop-all").0);
    let left = coa_core::process::find_by_exe(&root.join("mysql/bin/mysqld.exe")).len()
        + coa_core::process::find_by_exe(&root.join("Core/worldserver.exe")).len()
        + coa_core::process::find_by_exe(&root.join("Core/authserver.exe")).len();
    assert_eq!(left, 0, "nothing of the repack may be left running");
    println!("LAUNCHER E2E PASS");
}
