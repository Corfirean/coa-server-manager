//! 0.9.10 install paths of the Modules Preview, end to end on DISPOSABLE servers under <work>:
//!
//! 1. fresh:  a new server goes on to the preview core with the bare core (no module is compiled).
//! 2. copy:   a classic server (stopped) gets a modular copy next to it: accounts, characters, the world and
//!            module settings arrive, the old server's files are byte-for-byte unchanged, the copy takes the
//!            preview core with the modules the old server had switched on.
//! 3. copy-running: the same copy step from a classic server whose database is running.
//!
//! usage: preview_bare_and_copy_e2e <empty work folder> [fresh|copy|all]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coa_core::install::{install_base, Params, Source};
use coa_core::registry::Registry;
use coa_core::update::{self, RepackEnv};

const KEY: &str = coa_core::signing::EMBEDDED_PUBLIC_KEY;
type R<T> = Result<T, Box<dyn std::error::Error>>;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn sql(root: &Path, q: &str) -> String {
    coa_core::backup::with_database(root, |db| db.query(q)).unwrap_or_else(|e| panic!("{q}: {e}"))
}

fn install(root: &Path, reg: &Registry) -> R<()> {
    let image = coa_core::channels::stable_source()?;
    let t = std::time::Instant::now();
    let done = install_base(
        &Params { source: Source::Installation(Box::new(image)), dest: root.into(), trusted_key: KEY, registry: reg, cancel: Default::default() },
        &|s| eprintln!("  {:>3}% {}", s.percent, s.step),
    )?;
    println!("[{}] installed {} in {:?}", root.display(), done.version, t.elapsed());
    Ok(())
}

fn set_free_ports(root: &Path) -> R<()> {
    let repack = root.join("Settings/repack.json");
    let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&repack)?)?;
    for k in ["mysqlPort", "authPort", "worldPort", "raPort"] {
        cfg[k] = free_port().into();
    }
    std::fs::write(&repack, serde_json::to_vec_pretty(&cfg)?)?;
    Ok(())
}

/// The same steps as the Manager's install_preview_core.
fn preview_core(root: &Path) -> R<update::Outcome> {
    let meta_dir = coa_core::registry::metadata_dir_for(root)?;
    let (_, meta) = coa_core::registry::MetaDir::open(&meta_dir)?;
    let source = coa_core::update_route::select(&meta, coa_core::channels::preview_update_source()?, KEY)?.source;
    let env = RepackEnv { root, meta_dir: &meta_dir };
    let t = std::time::Instant::now();
    let out = coa_core::install::with_scratch_ports(root, || update::apply(
        &update::Params { root, meta_dir: &meta_dir, source, trusted_key: KEY, cancel: Default::default(), resolutions: Default::default(), env: &env, fail_after_ops: None },
        &|phase, percent| println!("  {percent}% {phase}"),
    ));
    let _ = coa_core::driver::run(root, coa_core::driver::Verb::StopAll);
    let out = out?;
    println!("[{}] preview core: {:?} in {:?}; modules {:?}", root.display(), out.txn.state, t.elapsed(), out.modules);
    assert_eq!(out.txn.state, update::State::Committed);
    assert!(coa_core::provenance::realm_host_fingerprint(root).is_some(), "the plugin host is installed");
    Ok(out)
}

/// sha256 of every file under the old server, except the launcher's own run state.
fn tree(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, coa_core::fsx::sha256_file(&p).unwrap_or_else(|e| format!("unreadable: {e}")));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn fresh(work: &Path, reg: &Registry) -> R<()> {
    let root = work.join("fresh-fixture");
    install(&root, reg)?; // shipped ports, as in the Manager: another server may be running on them
    coa_core::rebuild::decline_recommended(&root)?;
    let out = preview_core(&root)?;
    assert!(out.modules.is_empty(), "a new server compiles nothing: {:?}", out.modules);
    let registry = coa_core::plugins::load_registry(&root)?;
    assert!(registry.plugins.values().all(|e| !e.installed), "no plugin installed: {:?}", registry.plugins.keys());
    let boot: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("Core/plugins/plugin-boot-report.json"))?)?;
    println!("boot report: {boot}");
    assert_eq!(boot["anyFaulted"], serde_json::json!(false));
    println!("FRESH PASS");
    Ok(())
}

fn copy(work: &Path, reg: &Registry) -> R<()> {
    // A classic server with data of its own.
    let old = work.join("classic-fixture");
    install(&old, reg)?;
    set_free_ports(&old)?; // the test runs this "old" server; it must not meet a real one on the shipped ports
    sql(&old, "INSERT INTO acore_auth.account (username, salt, verifier, email, reg_mail) \
        VALUES ('E2ECOPY', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32)), '', '')");
    let account = sql(&old, "SELECT id FROM acore_auth.account WHERE username='E2ECOPY'").trim().to_string();
    sql(&old, &format!("SET SESSION sql_mode=''; INSERT INTO acore_characters.characters (guid, account, name, race, class, gender, level, money) \
        VALUES (900001, {account}, 'Copytest', 1, 1, 0, 42, 123456)"));
    let conf = old.join("Core/configs/modules/mod-coa-content-scaling.conf");
    let conf_text = std::fs::read_to_string(&conf).unwrap_or_default() + "\n# e2e owner edit\n";
    std::fs::write(&conf, &conf_text)?;
    let fingerprint = |root: &Path| sql(root, "SELECT CONCAT((SELECT COUNT(*) FROM acore_auth.account),'|',(SELECT COUNT(*) FROM acore_characters.characters),'|',\
        (SELECT CONCAT(name,':',level,':',money) FROM acore_characters.characters WHERE guid=900001))").trim().to_string();
    let expected = fingerprint(&old);
    let _ = coa_core::driver::run(&old, coa_core::driver::Verb::StopAll);
    println!("old server data: {expected}");
    assert!(coa_core::modular_copy::is_classic(&old));

    // 2. copy of the stopped server, then the preview core on the copy.
    let before = tree(&old);
    let new = work.join("copy-fixture");
    install(&new, reg)?;
    let report = coa_core::modular_copy::copy_data(&old, &new, &|w| println!("  copy: {w}"))?;
    println!("copy report: {report:?}");
    assert_eq!(report.read_from, "copy of the stopped database");
    assert_eq!(tree(&old), before, "the old server's files changed");
    let observed = coa_core::process::observe(&new, &coa_core::layout::read_ports(&new));
    assert_eq!(observed.mysql.state, coa_core::process::ServiceState::Stopped, "a database of the copy is still running: {:?}", observed.mysql);
    assert!(!coa_core::registry::metadata_dir_for(&new)?.join("import").exists(), "the private copy of the old data is removed");
    assert_eq!(std::fs::read_to_string(new.join("Core/configs/modules/mod-coa-content-scaling.conf"))?, conf_text);
    let out = preview_core(&new)?;
    println!("modules on the copy: {:?}", out.modules);
    let got = fingerprint(&new);
    let _ = coa_core::driver::run(&new, coa_core::driver::Verb::StopAll);
    assert_eq!(got, expected, "data of the copy after the preview core");
    assert_eq!(tree(&old), before, "the old server's files changed");
    println!("COPY (stopped) PASS");

    // 3. copy while the old database runs: read from it, change nothing.
    let started = coa_core::driver::run(&old, coa_core::driver::Verb::StartMysql)?;
    assert!(started.ok, "{}", started.output);
    let data_before: BTreeMap<String, String> = tree(&old).into_iter().filter(|(k, _)| !k.starts_with("mysql/") && !k.starts_with(".state/") && !k.starts_with("Core/configs/")).collect();
    let rows_before = fingerprint(&old);
    let new2 = work.join("copy-running-fixture");
    install(&new2, reg)?;
    let report = coa_core::modular_copy::copy_data(&old, &new2, &|w| println!("  copy: {w}"));
    let rows_after = fingerprint(&old);
    let _ = coa_core::driver::run(&old, coa_core::driver::Verb::StopAll);
    let report = report?;
    assert_eq!(report.read_from, "running database");
    assert_eq!(rows_after, rows_before);
    let data_after: BTreeMap<String, String> = tree(&old).into_iter().filter(|(k, _)| !k.starts_with("mysql/") && !k.starts_with(".state/") && !k.starts_with("Core/configs/")).collect();
    assert_eq!(data_after, data_before, "files outside the database changed");
    assert_eq!(fingerprint(&new2), expected);
    let _ = coa_core::driver::run(&new2, coa_core::driver::Verb::StopAll);
    println!("COPY (running) PASS");
    Ok(())
}

fn main() -> R<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let work = PathBuf::from(a.first().expect("usage: preview_bare_and_copy_e2e <empty work folder> [fresh|copy|all]"));
    let mode = a.get(1).map(String::as_str).unwrap_or("all");
    assert!(!work.exists() || std::fs::read_dir(&work)?.next().is_none(), "work folder must be new or empty");
    std::fs::create_dir_all(&work)?;
    let reg = Registry::at(work.join("installs.json"));
    if mode == "fresh" || mode == "all" {
        fresh(&work, &reg)?;
    }
    if mode == "copy" || mode == "all" {
        copy(&work, &reg)?;
    }
    println!("ALL PASS ({mode})");
    Ok(())
}
