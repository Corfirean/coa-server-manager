//! Keeps installed plugins usable across core changes.
//!
//! A plugin loads only into the core whose ABI fingerprint it was built with, and the core changes almost
//! every day. After a core update (or a rollback) every plugin that should run is brought to the current
//! fingerprint: an earlier build already in the store is reused, otherwise it is rebuilt from the sources
//! it was last built from. A plugin that cannot be brought up to date is switched off with the reason; the
//! core update itself always goes ahead.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::build_engine;
use crate::error::Result;
use crate::plugins;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum ModuleRebuild {
    /// Already built for this core.
    UpToDate { id: String },
    /// An earlier build for this core was still in the store (a rollback to a core seen before).
    Reused { id: String },
    Rebuilt { id: String, seconds: u64 },
    /// Switched off; `also` lists the plugins that depend on it and were switched off with it.
    Disabled { id: String, reason: String, also: Vec<String> },
}

/// Whether a plugin takes part: enabled, or switched off earlier only because a rebuild failed.
fn wanted(entry: &plugins::PluginEntry) -> bool {
    entry.installed && (entry.enabled || entry.disabled_reason.is_some())
}

/// Brings every plugin that should run to the realm's current core. Does nothing for a core without
/// plugin support. Never fails because of one plugin: each outcome is reported.
pub fn rebuild_for_current_core(
    realm_root: &Path,
    progress: &dyn Fn(&str, &str),
) -> Result<Vec<ModuleRebuild>> {
    let Some(host) = crate::provenance::realm_host_fingerprint(realm_root) else {
        return Ok(Vec::new());
    };
    let registry = plugins::load_registry(realm_root)?;
    let mut out = Vec::new();
    for (id, entry) in registry.plugins.iter().filter(|(_, e)| wanted(e)) {
        if entry.enabled && plugins::installed_plugin_fingerprint(realm_root, id).as_deref() == Some(host.as_str()) {
            out.push(ModuleRebuild::UpToDate { id: id.clone() });
            continue;
        }
        if out.iter().any(|o| matches!(o, ModuleRebuild::Disabled { also, .. } if also.contains(id))) {
            continue; // switched off together with a plugin it depends on
        }
        progress(id, "Building the module for the new core");
        let result = bring_up_to_date(realm_root, id, &host, progress).and_then(|done| {
            if !plugins::load_registry(realm_root).map_err(|e| e.to_string())?.plugins.get(id).is_some_and(|e| e.enabled) {
                plugins::set_enabled(realm_root, id, true).map_err(|e| e.to_string())?;
            }
            Ok(done)
        });
        out.push(match result {
            Ok(done) => done,
            Err(reason) => {
                let reason = first_line(&reason);
                progress(id, &format!("Switched off: {reason}"));
                let off = plugins::disable_with_reason(realm_root, id, &reason)?;
                ModuleRebuild::Disabled { id: id.clone(), reason, also: off.into_iter().skip(1).collect() }
            }
        });
    }
    Ok(out)
}

fn bring_up_to_date(
    realm_root: &Path,
    id: &str,
    host: &str,
    progress: &dyn Fn(&str, &str),
) -> std::result::Result<ModuleRebuild, String> {
    if let Some(sha) = stored_build_for(realm_root, id, host) {
        reuse_stored(realm_root, id, &sha).map_err(|e| e.to_string())?;
        return Ok(ModuleRebuild::Reused { id: id.to_string() });
    }
    let recipe = build_engine::read_build_recipe(realm_root, id)
        .ok_or("the manager does not know which sources this module was built from; install it again")?;
    if !recipe.source_dir.is_dir() || recipe.bundled.iter().any(|b| !b.is_dir()) {
        return Err("the module's sources are no longer on this PC; install it again".into());
    }
    let started = std::time::Instant::now();
    let built = build_engine::build_module_with_bundle(&recipe.source_dir, &recipe.bundled, realm_root, None, None)
        .map_err(|e| e.to_string())?;
    if built.module_id != id {
        return Err(format!("the sources now build module '{}', not '{id}'", built.module_id));
    }
    progress(id, "Installing the rebuilt module");
    let (meta, dll, confs) = crate::preview::built_package(realm_root, id, &built.version).map_err(|e| e.to_string())?;
    let confs: Vec<(String, &[u8])> = confs.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();
    plugins::update(realm_root, meta, &dll, None, &confs).map_err(|e| e.to_string())?;
    if plugins::installed_plugin_fingerprint(realm_root, id).as_deref() != Some(host) {
        return Err("the rebuilt module does not match the installed core".into());
    }
    Ok(ModuleRebuild::Rebuilt { id: id.to_string(), seconds: started.elapsed().as_secs() })
}

/// The store keeps every build by content hash: find one made for `host`.
fn stored_build_for(realm_root: &Path, id: &str, host: &str) -> Option<String> {
    let dir = plugins::store_dir(realm_root).join(id);
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let dll = e.path().join(format!("{id}.dll"));
        (crate::provenance::file_abi_fingerprint(&dll).as_deref() == Some(host))
            .then(|| e.file_name().to_string_lossy().into_owned())
    })
}

fn reuse_stored(realm_root: &Path, id: &str, sha: &str) -> Result<()> {
    let dir = plugins::store_dir(realm_root).join(id).join(sha);
    let meta = plugins::load_store_metadata(realm_root, id, sha)?;
    let dll = fs::read(dir.join(format!("{id}.dll")))?;
    let mut confs = Vec::new();
    for name in &meta.conf_files {
        if let Ok(bytes) = fs::read(dir.join(name)) {
            confs.push((name.clone(), bytes));
        }
    }
    let confs: Vec<(String, &[u8])> = confs.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();
    plugins::update(realm_root, meta, &dll, None, &confs)
}

fn first_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim();
    if line.chars().count() > 300 { line.chars().take(300).collect::<String>() + "…" } else { line.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{PluginMetadata, PluginsRegistry};
    use std::collections::BTreeMap;

    /// The smallest PE image with a ".coapl" section, as plugins and plugin hosts carry it.
    fn image(id: &str, fp: &str) -> Vec<u8> {
        let text = format!("COAPLUGIN1|{id}|{fp}\0");
        let mut b = vec![0u8; 0x200];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        b[0x40..0x44].copy_from_slice(b"PE\0\0");
        b[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // one section, no optional header
        let sec = 0x40 + 24;
        b[sec..sec + 6].copy_from_slice(b".coapl");
        let len = text.len() as u32;
        b[sec + 8..sec + 12].copy_from_slice(&len.to_le_bytes());
        b[sec + 16..sec + 20].copy_from_slice(&len.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&0x100u32.to_le_bytes());
        b[0x100..0x100 + text.len()].copy_from_slice(text.as_bytes());
        b
    }

    fn meta(id: &str, dll: &[u8]) -> PluginMetadata {
        PluginMetadata {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: plugins::sha256_digest(dll),
            required: false,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        }
    }

    fn registry(server: &Path) -> PluginsRegistry {
        plugins::load_registry(server).unwrap()
    }

    #[test]
    fn plugins_follow_the_core_or_are_switched_off_with_a_reason() {
        let old_fp = "a".repeat(64);
        let new_fp = "b".repeat(64);
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path();
        fs::create_dir_all(server.join("Core")).unwrap();
        fs::write(server.join("Core/worldserver.exe"), image("worldserver", &old_fp)).unwrap();

        // "kept" was built for both cores (the store has both builds); "lost" only for the old one and
        // the manager has no recipe to rebuild it from.
        let kept_new = image("kept", &new_fp);
        let kept_old = image("kept", &old_fp);
        plugins::install(server, meta("kept", &kept_new), &kept_new, None, &[], true).unwrap();
        plugins::update(server, meta("kept", &kept_old), &kept_old, None, &[]).unwrap();
        let lost = image("lost", &old_fp);
        plugins::install(server, meta("lost", &lost), &lost, None, &[], true).unwrap();
        // A plugin the owner switched off stays off and is not rebuilt.
        let idle = image("idle", &old_fp);
        plugins::install(server, meta("idle", &idle), &idle, None, &[], false).unwrap();

        // Same core: nothing to do.
        let same = rebuild_for_current_core(server, &|_, _| {}).unwrap();
        assert_eq!(same, vec![
            ModuleRebuild::UpToDate { id: "kept".into() },
            ModuleRebuild::UpToDate { id: "lost".into() },
        ]);

        // New core.
        fs::write(server.join("Core/worldserver.exe"), image("worldserver", &new_fp)).unwrap();
        let out = rebuild_for_current_core(server, &|_, _| {}).unwrap();
        assert_eq!(out[0], ModuleRebuild::Reused { id: "kept".into() });
        assert!(matches!(&out[1], ModuleRebuild::Disabled { id, reason, .. } if id == "lost" && reason.contains("install it again")));
        assert_eq!(out.len(), 2);
        let reg = registry(server);
        assert!(reg.plugins["kept"].enabled);
        assert_eq!(plugins::installed_plugin_fingerprint(server, "kept").as_deref(), Some(new_fp.as_str()));
        assert!(!reg.plugins["lost"].enabled);
        assert!(reg.plugins["lost"].disabled_reason.is_some());
        assert!(!reg.plugins["idle"].enabled && reg.plugins["idle"].disabled_reason.is_none());
        let lock = fs::read_to_string(plugins::lockfile_path(server)).unwrap();
        assert!(lock.contains("kept") && !lock.contains("\"lost\""), "{lock}");

        // Back to the old core (a rollback): the old builds are put back, "lost" is switched on again.
        fs::write(server.join("Core/worldserver.exe"), image("worldserver", &old_fp)).unwrap();
        let back = rebuild_for_current_core(server, &|_, _| {}).unwrap();
        assert_eq!(back, vec![
            ModuleRebuild::Reused { id: "kept".into() },
            ModuleRebuild::Reused { id: "lost".into() },
        ]);
        let reg = registry(server);
        assert!(reg.plugins["kept"].enabled && reg.plugins["lost"].enabled);
        assert!(reg.plugins["lost"].disabled_reason.is_none());
    }

    #[test]
    fn a_monolithic_core_needs_no_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Core")).unwrap();
        fs::write(dir.path().join("Core/worldserver.exe"), b"MZ no plugin host").unwrap();
        assert!(rebuild_for_current_core(dir.path(), &|_, _| {}).unwrap().is_empty());
    }
}
