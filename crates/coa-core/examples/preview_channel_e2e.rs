//! The published Modules Preview channel, end to end on a DISPOSABLE server, through the same source selection
//! the Manager uses when the owner does not choose a package: install from the stable channel's installation
//! image, check for updates (the preview channel), apply the preview update (modules are built on this PC and
//! the plugin host starts with them), then check again: the server must now be current.
//!
//! usage: preview_channel_e2e <new folder ending in -fixture>
//! The server gets its own free ports before anything starts; it never uses another server's database.
use std::path::Path;

use coa_core::install::{install_base, Params, Source};
use coa_core::registry::{MetaDir, Registry};
use coa_core::update::{self, DatabaseAccess, Env, RepackEnv};

const KEY: &str = coa_core::signing::EMBEDDED_PUBLIC_KEY;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn sql(root: &Path, q: &str) -> String {
    coa_core::backup::with_database(root, |db| db.query(q)).unwrap_or_else(|e| panic!("{q}: {e}"))
}

/// The Manager's "up to date" rule (src/lib/serverUpdateStatus.ts).
fn is_current(p: &update::Preview) -> bool {
    p.from_version.as_deref() == Some(p.to_version.as_str())
        && p.items.iter().all(|i| format!("{:?}", i.action).eq_ignore_ascii_case("skip"))
        && (p.database_check_error.is_some() || !p.database_checked || p.pending_migrations == 0)
}

fn check(root: &Path, meta_dir: &Path) -> Result<(Source, update::Preview), Box<dyn std::error::Error>> {
    let (_, meta) = MetaDir::open(meta_dir)?;
    let selection = coa_core::update_route::select(&meta, coa_core::channels::preview_update_source()?, KEY)?;
    let preview = update::preview_with(root, &meta, &selection.source, KEY, &Default::default(), DatabaseAccess::Start)?;
    Ok((selection.source, preview))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert_eq!(a.len(), 1, "usage: preview_channel_e2e <new-folder-fixture>");
    let root = Path::new(&a[0]);
    assert!(a[0].ends_with("-fixture") && !root.exists(), "a new folder ending in -fixture is required");
    let reg_dir = std::env::temp_dir().join("coa-preview-channel-e2e-registry");
    let _ = std::fs::remove_dir_all(&reg_dir);
    let reg = Registry::at(reg_dir.join("installs.json"));

    // 1. New server: the Manager installs the stable channel's image (the preview publishes updates only).
    let image = coa_core::channels::stable_source()?;
    println!("installation image: {image:?}");
    let t = std::time::Instant::now();
    let installed = install_base(
        &Params { source: Source::Installation(Box::new(image)), dest: root.into(), trusted_key: KEY, registry: &reg, cancel: Default::default() },
        &|s| eprintln!("  {:>3}% {}", s.percent, s.step),
    )?;
    println!("installed {} in {:?}", installed.version, t.elapsed());

    let repack = root.join("Settings/repack.json");
    let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&repack)?)?;
    for k in ["mysqlPort", "authPort", "worldPort", "raPort"] {
        cfg[k] = free_port().into();
    }
    std::fs::write(&repack, serde_json::to_vec_pretty(&cfg)?)?;
    sql(root, "INSERT INTO acore_auth.account (username, salt, verifier, email, reg_mail) \
        VALUES ('E2ECHANNEL', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32)), '', '')");
    let accounts = sql(root, "SELECT COUNT(*) FROM acore_auth.account").trim().to_string();

    // 2. Check for updates: the preview channel offers the preview.
    let meta_dir = coa_core::registry::metadata_dir_for(root)?;
    let (source, preview) = check(root, &meta_dir)?;
    println!("check: {:?} -> {} from {source:?}, {} items, {} migrations", preview.from_version, preview.to_version, preview.items.len(), preview.migrations);
    assert!(!is_current(&preview), "a fresh stable server must be offered the preview");

    // 3. Apply it, exactly as the Update button does.
    let env = RepackEnv { root, meta_dir: &meta_dir };
    let t = std::time::Instant::now();
    let outcome = update::apply(
        &update::Params { root, meta_dir: &meta_dir, source, trusted_key: KEY, cancel: Default::default(), resolutions: Default::default(), env: &env, fail_after_ops: None },
        &|phase, percent| println!("{percent}% {phase}"),
    );
    let _ = coa_core::driver::run(root, coa_core::driver::Verb::StopAll);
    let outcome = outcome?;
    println!("update {:?} to {:?} in {:?}; modules {:?}", outcome.txn.state, outcome.txn.to_version, t.elapsed(), outcome.modules);
    if let Ok(t) = std::fs::read_to_string(root.join("Core/plugins/plugin-boot-report.json")) {
        println!("--- boot report\n{t}");
    }
    assert_eq!(outcome.txn.state, update::State::Committed);

    // 4. Check again: nothing newer, nothing to change.
    let (_, again) = check(root, &meta_dir)?;
    let changing: Vec<_> = again.items.iter().filter(|i| !format!("{:?}", i.action).eq_ignore_ascii_case("skip")).map(|i| format!("{i:?}")).collect();
    println!("second check: {:?} -> {}, pending migrations {}, changing items {changing:?}", again.from_version, again.to_version, again.pending_migrations);
    assert_eq!(sql(root, "SELECT COUNT(*) FROM acore_auth.account").trim(), accounts, "accounts changed");
    env.ensure_stopped()?;
    assert!(is_current(&again), "the updated server is not reported as current");
    println!("PASS");
    Ok(())
}
