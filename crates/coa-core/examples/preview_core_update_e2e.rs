//! A core update with a NEW plugin ABI on a DISPOSABLE preview server that already runs modules: the production
//! updater must rebuild every enabled module for the new core before the trial start. With `break <module>` that
//! module's sources are replaced (for this server only) by a copy that does not compile: the core update must
//! still commit, the module must be switched off with a reason, and the world must start without it.
//!
//! usage: preview_core_update_e2e <existing folder ending in -fixture> <preview package dir> [break <module id>] [same-core]
//! `same-core`: the package keeps the server's plugin ABI (a module the manager switched off is retried).
//! The shared source cache is never modified: the broken copy lives next to the fixture.
use std::path::{Path, PathBuf};

use coa_core::install::Source;
use coa_core::update::{self, Env, RepackEnv};

fn sql(root: &Path, q: &str) -> String {
    coa_core::backup::with_database(root, |db| db.query(q)).unwrap_or_else(|e| panic!("{q}: {e}"))
}

/// Accounts and every character row, read the same way before and after.
fn data(root: &Path) -> String {
    sql(root, "SELECT CONCAT((SELECT COUNT(*) FROM acore_auth.account), '|', COUNT(*), '|', \
        COALESCE(SUM(CRC32(CONCAT_WS('|', guid, account, name, level, xp, money))), 0)) FROM acore_characters.characters")
        .trim()
        .to_string()
}

fn host_fingerprint(root: &Path) -> Option<String> {
    let t = std::fs::read_to_string(root.join("Core/plugins/plugin-boot-report.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&t).ok()?;
    v["hostAbiFingerprint"].as_str().map(str::to_string)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

fn first_cpp(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.iter().find(|p| p.extension().is_some_and(|x| x == "cpp")).cloned()
        .or_else(|| entries.iter().filter(|p| p.is_dir()).find_map(|p| first_cpp(p)))
}

/// Points this server's build recipe for `id` at a copy of its sources with a compile error.
fn break_module(root: &Path, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let recipe_path = coa_core::plugins::meta_dir(root).join("build-recipes").join(format!("{id}.json"));
    let mut recipe: serde_json::Value = serde_json::from_slice(&std::fs::read(&recipe_path)?)?;
    let src = PathBuf::from(recipe["source_dir"].as_str().ok_or("recipe has no source_dir")?);
    // Next to the fixture, never inside the shared cache. Same <repo>/<commit> tail as the cache, because the
    // manager reads the module's identity from it: the build itself must be what fails.
    let tail: PathBuf = src.components().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
    let broken = root.parent().unwrap().join("brk").join(tail);
    if src == broken {
        println!("{id} already builds from the broken copy {}", broken.display());
        return Ok(());
    }
    copy_dir(&src, &broken)?; // a fresh copy over any earlier one
    let cpp = ["src", "module/src"].iter().find_map(|d| first_cpp(&broken.join(d))).ok_or("no .cpp in the module sources")?;
    let text = std::fs::read_to_string(&cpp)?;
    std::fs::write(&cpp, format!("#error coa e2e: this module deliberately does not build\n{text}"))?;
    println!("broke {id}: {} (recipe now -> {})", cpp.display(), broken.display());
    recipe["source_dir"] = broken.to_string_lossy().into_owned().into();
    std::fs::write(&recipe_path, serde_json::to_vec_pretty(&recipe)?)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let same_core = a.last().is_some_and(|x| x == "same-core");
    let a: Vec<String> = if same_core { a[..a.len() - 1].to_vec() } else { a };
    assert!(a.len() == 2 || (a.len() == 4 && a[2] == "break"), "usage: preview_core_update_e2e <fixture> <package dir> [break <id>] [same-core]");
    let root = Path::new(&a[0]);
    assert!(a[0].ends_with("-fixture") && root.is_dir(), "an existing folder ending in -fixture is required");
    let broken = a.get(3).cloned();
    let meta_dir = coa_core::registry::metadata_dir_for(root)?;
    let env = RepackEnv { root, meta_dir: &meta_dir };
    env.ensure_stopped()?;

    let fp_before = host_fingerprint(root);
    let data_before = data(root);
    let lock_before = std::fs::read_to_string(root.join("Core/plugins/plugins.lock")).unwrap_or_default();
    println!("before: host {fp_before:?}, data {data_before}\n--- plugins.lock before\n{lock_before}");
    if let Some(id) = &broken {
        break_module(root, id)?;
    }

    let t = std::time::Instant::now();
    let outcome = update::apply(
        &update::Params {
            root,
            meta_dir: &meta_dir,
            source: Source::Dir(a[1].clone().into()),
            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
            cancel: Default::default(),
            resolutions: Default::default(),
            env: &env,
            fail_after_ops: None,
        },
        &|phase, percent| println!("{percent}% {phase}"),
    );
    let stopped = coa_core::driver::run(root, coa_core::driver::Verb::StopAll);
    println!("shutdown after update: {:?}", stopped.map(|r| r.ok));
    let outcome = outcome?;
    println!("update {:?} to {:?} in {:?}; modules {:#?}", outcome.txn.state, outcome.txn.to_version, t.elapsed(), outcome.modules);

    let fp_after = host_fingerprint(root);
    let data_after = data(root);
    let lock_after = std::fs::read_to_string(root.join("Core/plugins/plugins.lock")).unwrap_or_default();
    println!("after: host {fp_after:?}, data {data_after}\n--- plugins.lock after\n{lock_after}");
    if let Ok(t) = std::fs::read_to_string(root.join("Core/plugins/plugin-boot-report.json")) {
        println!("--- boot report\n{t}");
    }
    let reg = coa_core::plugins::load_registry(root)?;
    for (id, e) in &reg.plugins {
        println!("registry {id}: enabled={} disabled_reason={:?}", e.enabled, e.disabled_reason);
    }
    env.ensure_stopped()?;

    assert_eq!(outcome.txn.state, update::State::Committed, "the core update must always commit");
    if same_core {
        assert_eq!(fp_before, fp_after, "same-core: the host fingerprint changed");
    } else {
        assert_ne!(fp_before, fp_after, "the host fingerprint did not change");
    }
    assert_eq!(data_before, data_after, "accounts or characters changed");
    let shown: Vec<String> = outcome.modules.iter().map(|m| format!("{m:?}")).collect();
    for (id, e) in reg.plugins.iter().filter(|(_, e)| e.installed) {
        let line = shown.iter().find(|m| m.contains(&format!("\"{id}\""))).cloned().unwrap_or_default();
        if broken.as_deref() == Some(id.as_str()) {
            assert!(line.starts_with("Disabled") && !e.enabled && e.disabled_reason.is_some(), "{id} must be switched off with a reason: {line}");
            assert!(line.contains("does not build"), "{id} must be switched off because it does not build: {line}");
            assert!(!lock_after.contains(&format!("{id}|")), "{id} is still in plugins.lock");
        } else if lock_before.contains(&format!("{id}|")) {
            let ok = line.starts_with("Rebuilt") || line.starts_with("Reused") || (same_core && line.starts_with("UpToDate"));
            assert!(ok, "{id} was not rebuilt for the new core: {line}");
            assert!(e.enabled && lock_after.contains(&format!("{id}|")), "{id} is not loaded after the update");
        }
    }
    let report = std::fs::read_to_string(root.join("Core/plugins/plugin-boot-report.json"))?;
    assert!(report.contains("\"anyFaulted\": false"), "the trial start reported a faulted plugin");
    println!("PASS");
    Ok(())
}
