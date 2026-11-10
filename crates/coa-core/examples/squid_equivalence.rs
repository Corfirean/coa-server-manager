//! SQUID database provisioning in Rust gives exactly what Scripts/squid_playerbots.py gives, on a DISPOSABLE repack:
//! the same journals (coa_squid_migrations, coa_squid_pending) in every schema, the same acore_playerbots tables
//! with the same row counts, the same playerbots.conf. Each side starts from the same state (SQUID never
//! provisioned), and a second run of each must change nothing.
//!
//! usage: squid_equivalence <fixture ending in -fixture> <coa-launcher.exe>
use std::path::Path;
use std::process::Command;

use coa_core::launcher::{services, squid};

fn sql(root: &Path, q: &str) -> String {
    services::mysql(root, Some(q), None).unwrap_or_else(|e| panic!("{q}: {e}"))
}

/// What provisioning leaves behind, comparable between runs.
fn snapshot(root: &Path) -> String {
    let mut out = Vec::new();
    for schema in ["acore_characters", "acore_world", "acore_playerbots"] {
        let tables = sql(root, &format!("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='{schema}' ORDER BY TABLE_NAME;"));
        for t in tables.lines() {
            if schema != "acore_playerbots" && !t.starts_with("coa_squid") {
                continue;
            }
            let n = sql(root, &format!("SELECT COUNT(*) FROM `{schema}`.`{t}`;"));
            out.push(format!("{schema}.{t} rows={n}"));
            if t == "coa_squid_migrations" || t == "coa_squid_pending" {
                for row in sql(root, &format!("SELECT path, sha256 FROM `{schema}`.`{t}` ORDER BY path;")).lines() {
                    out.push(format!("  {row}"));
                }
            }
        }
    }
    let conf = std::fs::read_to_string(root.join("Core/configs/modules/playerbots.conf")).unwrap();
    out.push(format!("playerbots.conf:\n{}", conf.replace("\r\n", "\n")));
    out.join("\n")
}

fn reset(root: &Path, conf: &[u8]) {
    sql(root, "DROP DATABASE IF EXISTS acore_playerbots;");
    for schema in ["acore_characters", "acore_world"] {
        sql(root, &format!("DROP TABLE IF EXISTS `{schema}`.coa_squid_migrations, `{schema}`.coa_squid_pending;"));
    }
    std::fs::write(root.join("Core/configs/modules/playerbots.conf"), conf).unwrap();
}

fn python(root: &Path) {
    let harness = root.join(".state/squid_harness.py");
    std::fs::write(
        &harness,
        "import sys, pathlib\nroot = pathlib.Path(sys.argv[1])\nsys.path.insert(0, str(root / 'Scripts'))\nsys.argv = ['manage.py']\nimport manage\nfrom squid_playerbots import prepare_playerbots\nprepare_playerbots(manage.ROOT, manage.settings(), manage.mysql)\n",
    )
    .unwrap();
    let out = Command::new(root.join("Runtime/python/python.exe")).arg("-B").arg(&harness).arg(root).output().unwrap();
    assert!(out.status.success(), "python: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    print!("{}", String::from_utf8_lossy(&out.stdout));
}

fn rust(root: &Path) {
    let cfg = coa_core::launcher::Settings::load(root, 0).unwrap();
    squid::prepare_playerbots(root, cfg.mysql_port, &|q| services::mysql(root, Some(q), None), &|m| println!("{m}")).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&a[0]);
    let exe = Path::new(&a[1]);
    assert!(a[0].ends_with("-fixture"));
    let modules = root.join("Core/configs/modules");
    // SQUID on, Companions off
    let companions = std::fs::read_to_string(modules.join("mod_coa_playerbots.conf")).unwrap();
    std::fs::write(modules.join("mod_coa_playerbots.conf"), companions.replace("CoaBots.Enable = 1", "CoaBots.Enable = 0")).unwrap();
    let squid_conf = std::fs::read_to_string(modules.join("playerbots.conf")).unwrap().replace("AiPlayerbot.Enabled = 0", "AiPlayerbot.Enabled = 1");
    let r = root.to_string_lossy().into_owned();
    let started = Command::new(exe).args(["--root", &r, "start-mysql"]).output().unwrap();
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));

    reset(root, squid_conf.as_bytes());
    let t = std::time::Instant::now();
    python(root);
    println!("python provisioned in {:?}", t.elapsed());
    let by_python = snapshot(root);
    python(root);
    assert_eq!(snapshot(root), by_python, "a second Python run changed something");

    reset(root, squid_conf.as_bytes());
    let t = std::time::Instant::now();
    rust(root);
    println!("rust provisioned in {:?}", t.elapsed());
    let by_rust = snapshot(root);
    rust(root);
    assert_eq!(snapshot(root), by_rust, "a second Rust run changed something");

    if by_python != by_rust {
        std::fs::write(root.join(".state/squid-python.txt"), &by_python).unwrap();
        std::fs::write(root.join(".state/squid-rust.txt"), &by_rust).unwrap();
        panic!("Rust and Python differ: see .state/squid-python.txt and squid-rust.txt");
    }
    println!("{} lines of journal/table/config state are identical", by_rust.lines().count());
    let stopped = Command::new(exe).args(["--root", &r, "stop-all"]).output().unwrap();
    assert!(stopped.status.success());
    println!("SQUID EQUIVALENCE PASS");
}
