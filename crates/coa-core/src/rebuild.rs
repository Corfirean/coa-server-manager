//! Keeps installed plugins usable across core changes.
//!
//! A plugin loads only into the core whose ABI fingerprint it was built with, and the core changes almost
//! every day. After a core update (or a rollback) every plugin that should run is brought to the current
//! fingerprint: an earlier build already in the store is reused, otherwise it is rebuilt from the sources
//! it was last built from. A plugin that cannot be brought up to date is switched off with the reason; the
//! core update itself always goes ahead.

use std::fs;
use std::path::Path;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

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
    /// A module the core recommends, built and switched on for the first time (the move from a core
    /// with the module built in to a core that loads it as a plugin).
    Installed { id: String, seconds: u64 },
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
    let mut out = install_recommended(realm_root, progress);
    let registry = plugins::load_registry(realm_root)?;
    for (id, entry) in registry.plugins.iter().filter(|(_, e)| wanted(e)) {
        if out.iter().any(|o| matches!(o, ModuleRebuild::Installed { id: done, .. } if done == id)) {
            continue;
        }
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
    if !built.success {
        // Never fall through to a package left by an earlier build: that one was made for the old core.
        return Err(build_failure_reason(&built.errors));
    }
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

/// Why a module did not build for the new core, from the compiler's first error.
fn build_failure_reason(errors: &[build_engine::BuildErrorDiagnostic]) -> String {
    match errors.first() {
        Some(e) if !e.file.is_empty() => {
            let file = e.file.rsplit(['/', '\\']).next().unwrap_or(&e.file);
            format!("it does not build for the new core: {} ({file}:{}, {})", first_line(&e.message), e.line, e.error_code)
        }
        Some(e) => format!("it does not build for the new core: {}", first_line(&e.message)),
        None => "it does not build for the new core".into(),
    }
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

// ---------------------------------------------------------------------------------------------------
// Modules the core recommends
// ---------------------------------------------------------------------------------------------------

static SOURCE_CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Where module sources are downloaded (the app's own cache); set once at startup.
pub fn set_source_cache(dir: PathBuf) {
    let _ = SOURCE_CACHE.set(dir);
}

fn source_cache() -> PathBuf {
    SOURCE_CACHE.get().cloned().unwrap_or_else(|| std::env::temp_dir().join("coa_github_cache"))
}

/// `Core/plugins/recommended.json`, shipped with a plugin-host core: the modules a server of the
/// monolithic release had built in, at the commits this core was qualified with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recommended {
    pub schema: u32,
    pub modules: Vec<RecommendedModule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecommendedModule {
    pub id: String,
    /// `owner/name` on GitHub.
    pub repo: String,
    pub commit: String,
    /// Source trees compiled into the same plugin: a folder of this repository, or of another one.
    #[serde(default)]
    pub bundled: Vec<BundledSource>,
    /// When to install it: "always" (default), "config:<file>:<key>=<value>" (the owner had it switched
    /// on in that module config), or "unless:<id>" (a module that excludes it is switched on).
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundledSource {
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub path: String,
}

fn recommended(realm_root: &Path) -> Option<Recommended> {
    let path = plugins::plugins_install_dir(realm_root).join("recommended.json");
    let r: Recommended = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (r.schema == 1).then_some(r)
}

/// `.manager/recommended-modules.json`: recommended modules already dealt with (installed, or not wanted
/// by their rule), so a module the owner later removes is not installed again.
fn handled_path(realm_root: &Path) -> PathBuf {
    plugins::meta_dir(realm_root).join("recommended-modules.json")
}

fn load_handled(realm_root: &Path) -> BTreeMap<String, String> {
    fs::read(handled_path(realm_root)).ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| serde_json::from_value(v.get("handled")?.clone()).ok())
        .unwrap_or_default()
}

fn save_handled(realm_root: &Path, handled: &BTreeMap<String, String>) {
    let body = serde_json::json!({ "schema": 1, "handled": handled });
    let _ = fs::create_dir_all(plugins::meta_dir(realm_root));
    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
    let _ = crate::fsx::atomic_write(&handled_path(realm_root), text.as_bytes());
}

/// Whether `rule` asks for the module on this server, given the plugins already switched on.
fn rule_allows(realm_root: &Path, rule: Option<&str>, enabled: &[String]) -> bool {
    let rule = rule.unwrap_or("always");
    if rule == "always" {
        return true;
    }
    if let Some(other) = rule.strip_prefix("unless:") {
        return !enabled.iter().any(|e| e == other);
    }
    if let Some(spec) = rule.strip_prefix("config:") {
        let Some((file, kv)) = spec.split_once(':') else { return false };
        let Some((key, value)) = kv.split_once('=') else { return false };
        if file.contains('/') || file.contains('\\') || file.contains("..") {
            return false;
        }
        let path = plugins::modules_config_dir(realm_root).join(file);
        return fs::read(&path).ok()
            .and_then(|b| crate::config::parser::ConfFile::parse_bytes(&b).ok())
            .and_then(|c| c.get(key).map(|v| v.trim().trim_matches('"') == value))
            .unwrap_or(false);
    }
    false
}

fn split_repo(repo: &str) -> std::result::Result<(&str, &str), String> {
    repo.split_once('/')
        .filter(|(o, r)| !o.is_empty() && !r.is_empty() && !r.contains('/'))
        .ok_or_else(|| format!("invalid repository '{repo}'"))
}

fn install_recommended(realm_root: &Path, progress: &dyn Fn(&str, &str)) -> Vec<ModuleRebuild> {
    let Some(rec) = recommended(realm_root) else { return Vec::new() };
    let mut handled = load_handled(realm_root);
    let mut out = Vec::new();
    for m in &rec.modules {
        let registry = plugins::load_registry(realm_root).unwrap_or_default();
        if handled.contains_key(&m.id) || registry.plugins.contains_key(&m.id) {
            continue;
        }
        let enabled: Vec<String> = registry.plugins.iter().filter(|(_, e)| e.enabled).map(|(k, _)| k.clone()).collect();
        if !rule_allows(realm_root, m.when.as_deref(), &enabled) {
            handled.insert(m.id.clone(), format!("not wanted ({})", m.when.as_deref().unwrap_or("")));
            continue;
        }
        progress(&m.id, "Downloading and building the module for this core");
        let started = std::time::Instant::now();
        match install_one(realm_root, m) {
            Ok(()) => {
                handled.insert(m.id.clone(), m.commit.clone());
                out.push(ModuleRebuild::Installed { id: m.id.clone(), seconds: started.elapsed().as_secs() });
            }
            // Not marked as handled: the next core change tries again.
            Err(reason) => out.push(ModuleRebuild::Disabled { id: m.id.clone(), reason: first_line(&reason), also: Vec::new() }),
        }
    }
    save_handled(realm_root, &handled);
    out
}

fn install_one(realm_root: &Path, m: &RecommendedModule) -> std::result::Result<(), String> {
    let cache = source_cache();
    let (owner, name) = split_repo(&m.repo)?;
    let primary = crate::module_sources::fetch_pinned(owner, name, &m.commit, &cache).map_err(|e| e.to_string())?;
    let mut bundled = Vec::new();
    for b in &m.bundled {
        let base = match (&b.repo, &b.commit) {
            (Some(repo), Some(commit)) => {
                let (o, n) = split_repo(repo)?;
                crate::module_sources::fetch_pinned(o, n, commit, &cache).map_err(|e| e.to_string())?
            }
            (None, None) => primary.clone(),
            _ => return Err(format!("a bundled source of '{}' needs both repo and commit", m.id)),
        };
        let dir = if b.path.is_empty() { base } else { crate::fsx::safe_join(&base, &b.path).map_err(|e| e.to_string())? };
        if !dir.is_dir() {
            return Err(format!("bundled source '{}' is missing", b.path));
        }
        bundled.push(dir);
    }
    let built = build_engine::build_module_with_bundle(&primary, &bundled, realm_root, None, None).map_err(|e| e.to_string())?;
    if built.module_id != m.id {
        return Err(format!("the sources build module '{}', not '{}'", built.module_id, m.id));
    }
    let (meta, dll, confs) = crate::preview::built_package(realm_root, &m.id, &built.version).map_err(|e| e.to_string())?;
    let confs: Vec<(String, &[u8])> = confs.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();
    plugins::install(realm_root, meta, &dll, None, &confs, true).map_err(|e| e.to_string())
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
    fn recommended_modules_follow_what_the_owner_used() {
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path();
        fs::create_dir_all(server.join("Core/configs/modules")).unwrap();
        fs::create_dir_all(server.join("Core/plugins")).unwrap();
        fs::write(server.join("Core/worldserver.exe"), image("worldserver", &"c".repeat(64))).unwrap();
        let conf = server.join("Core/configs/modules/playerbots.conf");

        fs::write(&conf, "AiPlayerbot.Enabled = 1\n").unwrap();
        assert!(rule_allows(server, Some("config:playerbots.conf:AiPlayerbot.Enabled=1"), &[]));
        fs::write(&conf, "AiPlayerbot.Enabled = 0\n").unwrap();
        assert!(!rule_allows(server, Some("config:playerbots.conf:AiPlayerbot.Enabled=1"), &[]));
        assert!(!rule_allows(server, Some("config:../x.conf:A=1"), &[]));
        assert!(!rule_allows(server, Some("unless:playerbots"), &["playerbots".into()]));
        assert!(rule_allows(server, Some("unless:playerbots"), &["companions".into()]));
        assert!(rule_allows(server, None, &[]));
        assert!(!rule_allows(server, Some("sometimes"), &[]));

        // SQUID was off on the old server: it is not installed, and is not offered again later. A module
        // whose sources cannot be fetched is reported and retried at the next core change.
        fs::write(server.join("Core/plugins/recommended.json"), r#"{"schema":1,"modules":[
            {"id":"playerbots","repo":"Zyth45/mod-playerbots","commit":"0000000000000000000000000000000000000000","when":"config:playerbots.conf:AiPlayerbot.Enabled=1"},
            {"id":"companions","repo":"not-a-repo","commit":"0000000000000000000000000000000000000000","when":"unless:playerbots"}]}"#).unwrap();
        let out = rebuild_for_current_core(server, &|_, _| {}).unwrap();
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], ModuleRebuild::Disabled { id, reason, .. } if id == "companions" && reason.contains("invalid repository")));
        let handled = load_handled(server);
        assert!(handled["playerbots"].starts_with("not wanted"));
        assert!(!handled.contains_key("companions"));
    }

    /// Real run: COA_TEST_RECOMMENDED_REALM is a plugin-host realm with Core/plugins/recommended.json (or
    /// plugins/recommended.json); the modules are downloaded from GitHub, built and switched on.
    #[test]
    fn recommended_modules_install_on_a_real_realm() {
        let Ok(realm) = std::env::var("COA_TEST_RECOMMENDED_REALM") else { return };
        let realm = Path::new(&realm);
        let out = rebuild_for_current_core(realm, &|id, what| println!("{id}: {what}")).unwrap();
        println!("{out:#?}");
        let host = crate::provenance::realm_host_fingerprint(realm).unwrap();
        for o in &out {
            let ModuleRebuild::Installed { id, .. } = o else { panic!("not installed: {o:?}") };
            assert_eq!(plugins::installed_plugin_fingerprint(realm, id).as_deref(), Some(host.as_str()));
        }
        let lock = fs::read_to_string(plugins::lockfile_path(realm)).unwrap();
        println!("{lock}");
    }

    #[test]
    fn a_failed_build_names_the_compiler_error() {
        let e = build_engine::BuildErrorDiagnostic {
            file: "C:\\brk\\src\\PlayerbotAIConfig.cpp".into(),
            line: 1,
            error_code: "C1189".into(),
            message: "#error: coa e2e: this module deliberately does not build\nmore".into(),
            explanation: None,
            resolution: None,
        };
        assert_eq!(
            build_failure_reason(&[e]),
            "it does not build for the new core: #error: coa e2e: this module deliberately does not build (PlayerbotAIConfig.cpp:1, C1189)"
        );
        assert_eq!(build_failure_reason(&[]), "it does not build for the new core");
    }

    #[test]
    fn a_monolithic_core_needs_no_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Core")).unwrap();
        fs::write(dir.path().join("Core/worldserver.exe"), b"MZ no plugin host").unwrap();
        assert!(rebuild_for_current_core(dir.path(), &|_, _| {}).unwrap().is_empty());
    }
}
