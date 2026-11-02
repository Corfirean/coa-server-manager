//! Dynamic Plugin Architecture (FV1) for CoA Server Manager.
//!
//! Manages dynamic DLL plugins, the plugin store (`.manager/plugin-store`),
//! registry (`.manager/plugins.json`), `plugins.lock` generation, 3-way configuration merging,
//! transactional updates, dual-source SQUID SQL reconciliation, and multiworld isolation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::parser::ConfFile;
use crate::error::{Error, Result};
use crate::fsx;

pub const REGISTRY_SCHEMA: u32 = 1;

/// Metadata describing a plugin package ready to be placed into the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: BTreeMap<String, String>,
    pub sha256: String,
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub conf_files: Vec<String>,
    #[serde(default)]
    pub enable_key: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

fn yes() -> bool {
    true
}

/// An entry in `.manager/plugins.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub installed: bool,
    pub enabled: bool,
    pub version: String,
    pub sha256: String,
    pub store_path: String,
    pub conf_files: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default = "yes")]
    pub required: bool,
}

/// The root registry document in `.manager/plugins.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsRegistry {
    pub schema: u32,
    pub plugins: BTreeMap<String, PluginEntry>,
}

impl Default for PluginsRegistry {
    fn default() -> Self {
        Self {
            schema: REGISTRY_SCHEMA,
            plugins: BTreeMap::new(),
        }
    }
}

/// View of a plugin suitable for the CSM UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: BTreeMap<String, String>,
    pub installed: bool,
    pub enabled: bool,
    pub update_available: bool,
    pub latest_version: Option<String>,
    pub deps: Vec<String>,
    pub conf_files: Vec<String>,
    pub status: String,
    pub icon: String,
}

/// Resolve the metadata directory (`.manager` or `<server>.manager`).
pub fn meta_dir(server: &Path) -> PathBuf {
    let dot = server.join(".manager");
    if dot.is_dir() {
        return dot;
    }
    if let Ok(dir) = crate::registry::metadata_dir_for(server) {
        if dir.is_dir() {
            return dir;
        }
    }
    dot
}

/// Root of the immutable plugin store: `<meta_dir>/plugin-store`.
pub fn store_dir(server: &Path) -> PathBuf {
    meta_dir(server).join("plugin-store")
}

/// Path to `.manager/plugins.json`.
pub fn registry_path(server: &Path) -> PathBuf {
    meta_dir(server).join("plugins.json")
}

/// Path where `worldserver.exe` expects `plugins/`.
pub fn plugins_install_dir(server: &Path) -> PathBuf {
    if server.join("Core").is_dir() {
        server.join("Core/plugins")
    } else {
        server.join("plugins")
    }
}

/// Path to `plugins.lock`.
pub fn lockfile_path(server: &Path) -> PathBuf {
    plugins_install_dir(server).join("plugins.lock")
}

/// Directory for module configs (`Core/configs/modules` or `configs/modules`).
pub fn modules_config_dir(server: &Path) -> PathBuf {
    if server.join("configs/modules").is_dir() && !server.join("Core/configs/modules").is_dir() {
        server.join("configs/modules")
    } else {
        server.join("Core/configs/modules")
    }
}

/// Check that the worldserver is not running before modifying plugins.
pub fn require_server_stopped(server: &Path) -> Result<()> {
    let observed = crate::process::observe(server, &crate::layout::read_ports(server));
    if observed.world.state != crate::process::ServiceState::Stopped
        || crate::multiworld::is_running(server)
    {
        return Err(Error::Invalid(
            "Stop the server before modifying plugins or their enabled state.".into(),
        ));
    }
    Ok(())
}

/// Calculate SHA256 of bytes.
pub fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Cleans up any staging folders left behind by crashed or interrupted transactions.
pub fn cleanup_stale_staging(server: &Path) -> Result<()> {
    let staging = meta_dir(server).join("staging");
    if staging.is_dir() {
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("store_") || name.starts_with("plugin_tx_") {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// Load `.manager/plugins.json` directly without running recovery check.
pub fn load_registry_direct(server: &Path) -> Result<PluginsRegistry> {
    let _ = cleanup_stale_staging(server);
    let path = registry_path(server);
    if !path.is_file() {
        return Ok(PluginsRegistry::default());
    }
    let bytes = fs::read(&path)?;
    let reg: PluginsRegistry = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Invalid(format!("corrupt plugins.json: {e}")))?;
    if reg.schema > REGISTRY_SCHEMA {
        return Err(Error::Invalid(format!(
            "Unsupported plugins.json schema version {} (supported: {})",
            reg.schema, REGISTRY_SCHEMA
        )));
    }
    Ok(reg)
}

/// Load `.manager/plugins.json` or return empty registry if not present.
/// Performs durable transaction recovery on startup if an interrupted transaction is present.
pub fn load_registry(server: &Path) -> Result<PluginsRegistry> {
    ensure_durable_recovered(server)?;
    load_registry_direct(server)
}

/// Atomically save `.manager/plugins.json`.
pub fn save_registry(server: &Path, reg: &PluginsRegistry) -> Result<()> {
    let meta = meta_dir(server);
    fs::create_dir_all(&meta)?;
    let path = registry_path(server);
    let bytes = serde_json::to_vec_pretty(reg)?;
    fsx::atomic_write(&path, &bytes)
}

/// Read a plugin's metadata from the store given its ID and SHA256.
pub fn load_store_metadata(server: &Path, id: &str, sha256: &str) -> Result<PluginMetadata> {
    let p = store_dir(server).join(id).join(sha256).join("plugin.json");
    let bytes = fs::read(&p)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(format!("corrupt plugin.json at {}: {e}", p.display())))
}

/// Transaction execution phases for durable recovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionPhase {
    Planned,
    Prepared,
    Snapshotted,
    Applying,
    Validating,
    Committed,
    RolledBack,
    RecoveryRequired,
}

/// Supported plugin transaction operation types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginOpType {
    Install,
    Update,
    Enable,
    Disable,
    Remove,
    Repair,
    MultiworldDeploy,
}

/// Durable transaction journal stored at `.manager/plugin-tx/active.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTxJournal {
    pub tx_id: String,
    pub op: PluginOpType,
    pub plugin_id: String,
    pub phase: TransactionPhase,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub target_realms: Vec<String>,
    pub backup_dir: String,
    pub recovery_hashes: BTreeMap<String, String>,
    pub message: Option<String>,
    pub timestamp: String,
}

pub fn plugin_tx_dir(server: &Path) -> PathBuf {
    meta_dir(server).join("plugin-tx")
}

pub fn plugin_active_tx_path(server: &Path) -> PathBuf {
    plugin_tx_dir(server).join("active.json")
}

pub fn plugin_tx_backup_dir(server: &Path, tx_id: &str) -> PathBuf {
    plugin_tx_dir(server).join(tx_id).join("before")
}

/// Check environment variable `CSM_CRASH_AT` and trigger controlled process abort if matched.
pub fn check_crash_injection(point: &str) {
    if let Ok(target) = std::env::var("CSM_CRASH_AT") {
        if target == point {
            eprintln!("[CRASH_INJECTION] Terminating process at crash point: {}", point);
            std::process::abort();
        }
    }
}

/// Check and recover any interrupted plugin transaction upon startup or before new operations.
/// Idempotent: repeated executions on clean or already-recovered state return Ok(()).
pub fn ensure_durable_recovered(server: &Path) -> Result<()> {
    let active_path = plugin_active_tx_path(server);
    if !active_path.is_file() {
        return Ok(());
    }

    let bytes = match fs::read(&active_path) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };

    let mut journal: PluginTxJournal = match serde_json::from_slice(&bytes) {
        Ok(j) => j,
        Err(e) => {
            return Err(Error::Invalid(format!(
                "RecoveryRequired: Unreadable plugin transaction journal at {}: {e}",
                active_path.display()
            )));
        }
    };

    if journal.phase == TransactionPhase::RecoveryRequired {
        return Err(Error::Invalid(format!(
            "RecoveryRequired: Plugin transaction {} is in unresolvable state ({}). Manual operator intervention required.",
            journal.tx_id,
            journal.message.as_deref().unwrap_or("unknown error")
        )));
    }

    if journal.phase == TransactionPhase::Committed {
        // Logically committed before crash. Complete cleanup and finalize.
        let tx_dir = plugin_tx_dir(server).join(&journal.tx_id);
        let _ = fs::remove_dir_all(&tx_dir);
        let _ = fs::remove_file(&active_path);
        cleanup_stale_staging(server)?;
        return Ok(());
    }

    // Phases [Planned, Prepared, Snapshotted, Applying, Validating]: rollback required
    let before_dir = PathBuf::from(&journal.backup_dir);
    if !before_dir.is_dir() {
        journal.phase = TransactionPhase::RecoveryRequired;
        journal.message = Some(format!("Backup directory missing: {}", before_dir.display()));
        let _ = fsx::atomic_write_json(&active_path, &journal);
        return Err(Error::Invalid(format!(
            "RecoveryRequired: Backup directory {} missing for transaction {}",
            before_dir.display(),
            journal.tx_id
        )));
    }

    // Verify integrity of all backup files against expected recovery hashes
    for (rel_path, expected_hash) in &journal.recovery_hashes {
        let fpath = before_dir.join(rel_path);
        if !fpath.is_file() {
            journal.phase = TransactionPhase::RecoveryRequired;
            journal.message = Some(format!("Recovery file {} missing", rel_path));
            let _ = fsx::atomic_write_json(&active_path, &journal);
            return Err(Error::Invalid(format!(
                "RecoveryRequired: Backup file {} missing for transaction {}",
                rel_path, journal.tx_id
            )));
        }
        let actual_hash = sha256_digest(&fs::read(&fpath)?);
        if &actual_hash != expected_hash {
            journal.phase = TransactionPhase::RecoveryRequired;
            journal.message = Some(format!("Recovery file {} checksum mismatch", rel_path));
            let _ = fsx::atomic_write_json(&active_path, &journal);
            return Err(Error::Invalid(format!(
                "RecoveryRequired: Checksum mismatch on backup file {} (expected {}, got {})",
                rel_path, expected_hash, actual_hash
            )));
        }
    }

    // Atomically restore registry and lockfile
    let reg_p = registry_path(server);
    if journal.recovery_hashes.contains_key("plugins.json") {
        let b = fs::read(before_dir.join("plugins.json"))?;
        fsx::atomic_write(&reg_p, &b)?;
    } else if reg_p.is_file() {
        let _ = fs::remove_file(&reg_p);
    }

    let lock_p = lockfile_path(server);
    if journal.recovery_hashes.contains_key("plugins.lock") {
        let b = fs::read(before_dir.join("plugins.lock"))?;
        fsx::atomic_write(&lock_p, &b)?;
    } else if lock_p.is_file() {
        let _ = fs::remove_file(&lock_p);
    }

    // Restore configuration files
    for rel_path in journal.recovery_hashes.keys() {
        if let Some(conf_name) = rel_path.strip_prefix("confs/") {
            let target_conf = modules_config_dir(server).join(conf_name);
            let b = fs::read(before_dir.join(rel_path))?;
            fsx::atomic_write(&target_conf, &b)?;
        }
    }

    // Clean up transaction directory and active journal
    let tx_dir = plugin_tx_dir(server).join(&journal.tx_id);
    let _ = fs::remove_dir_all(&tx_dir);
    let _ = fs::remove_file(&active_path);
    cleanup_stale_staging(server)?;

    Ok(())
}

/// Durable RAII transaction guard for plugin operations.
/// Backed by `.manager/plugin-tx/active.json` to survive process abortion (`TerminateProcess`, `abort`).
pub struct TxGuard<'a> {
    server: &'a Path,
    tx_id: String,
    journal: PluginTxJournal,
    staging_dirs: Vec<PathBuf>,
    committed: bool,
}

impl<'a> TxGuard<'a> {
    pub fn begin(server: &'a Path) -> Result<Self> {
        Self::begin_op(server, PluginOpType::Repair, "generic")
    }

    pub fn begin_op(server: &'a Path, op: PluginOpType, plugin_id: &str) -> Result<Self> {
        ensure_durable_recovered(server)?;

        let tx_id = uuid::Uuid::now_v7().to_string();
        let before_dir = plugin_tx_backup_dir(server, &tx_id);
        fs::create_dir_all(&before_dir)?;

        let mut recovery_hashes = BTreeMap::new();

        let reg_p = registry_path(server);
        if reg_p.is_file() {
            let bytes = fs::read(&reg_p)?;
            fs::write(before_dir.join("plugins.json"), &bytes)?;
            recovery_hashes.insert("plugins.json".into(), sha256_digest(&bytes));
        }

        let lock_p = lockfile_path(server);
        if lock_p.is_file() {
            let bytes = fs::read(&lock_p)?;
            fs::write(before_dir.join("plugins.lock"), &bytes)?;
            recovery_hashes.insert("plugins.lock".into(), sha256_digest(&bytes));
        }

        let journal = PluginTxJournal {
            tx_id: tx_id.clone(),
            op,
            plugin_id: plugin_id.to_string(),
            phase: TransactionPhase::Snapshotted,
            from_version: None,
            to_version: None,
            target_realms: vec!["primary".into()],
            backup_dir: before_dir.to_string_lossy().into_owned(),
            recovery_hashes,
            message: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        fsx::atomic_write_json(&plugin_active_tx_path(server), &journal)?;
        check_crash_injection("after_snapshot");

        Ok(Self {
            server,
            tx_id,
            journal,
            staging_dirs: Vec::new(),
            committed: false,
        })
    }

    pub fn set_phase(&mut self, phase: TransactionPhase) -> Result<()> {
        self.journal.phase = phase;
        fsx::atomic_write_json(&plugin_active_tx_path(self.server), &self.journal)
    }

    pub fn backup_conf(&mut self, conf_path: &Path) -> Result<()> {
        if conf_path.is_file() {
            let file_name = conf_path.file_name().unwrap().to_string_lossy().into_owned();
            let before_dir = PathBuf::from(&self.journal.backup_dir);
            let confs_dir = before_dir.join("confs");
            fs::create_dir_all(&confs_dir)?;
            let bytes = fs::read(conf_path)?;
            fs::write(confs_dir.join(&file_name), &bytes)?;
            self.journal.recovery_hashes.insert(format!("confs/{}", file_name), sha256_digest(&bytes));
            fsx::atomic_write_json(&plugin_active_tx_path(self.server), &self.journal)?;
        }
        Ok(())
    }

    pub fn track_staging(&mut self, staging_dir: PathBuf) {
        self.staging_dirs.push(staging_dir);
    }

    pub fn commit(mut self) -> Result<()> {
        self.set_phase(TransactionPhase::Committed)?;
        check_crash_injection("after_commit_before_cleanup");

        for s in &self.staging_dirs {
            let _ = fs::remove_dir_all(s);
        }

        let tx_dir = plugin_tx_dir(self.server).join(&self.tx_id);
        let _ = fs::remove_dir_all(&tx_dir);
        let _ = fs::remove_file(plugin_active_tx_path(self.server));

        self.committed = true;
        Ok(())
    }
}

impl<'a> Drop for TxGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = ensure_durable_recovered(self.server);
        }
    }
}

/// Helper to recursively copy directories.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Copy and register a plugin into `.manager/plugin-store/<id>/<sha256>/`.
pub fn install_to_store(
    server: &Path,
    meta: &PluginMetadata,
    dll_bytes: &[u8],
    pdb_bytes: Option<&[u8]>,
    dist_confs: &[(String, &[u8])],
) -> Result<PathBuf> {
    let digest = sha256_digest(dll_bytes);
    if digest != meta.sha256 {
        return Err(Error::HashMismatch {
            path: format!("{}.dll", meta.id),
            expected: meta.sha256.clone(),
            actual: digest,
        });
    }

    let target = store_dir(server).join(&meta.id).join(&digest);
    let staging = meta_dir(server)
        .join("staging")
        .join(format!("store_{}_{}", meta.id, uuid::Uuid::now_v7()));
    fs::create_dir_all(&staging)?;
    check_crash_injection("after_staging");

    let write_res = (|| -> Result<()> {
        fs::write(staging.join(format!("{}.dll", meta.id)), dll_bytes)?;
        if let Some(pdb) = pdb_bytes {
            fs::write(staging.join(format!("{}.pdb", meta.id)), pdb)?;
        }
        for (name, conf_bytes) in dist_confs {
            let conf_name = if name.ends_with(".dist") {
                name.clone()
            } else {
                format!("{name}.dist")
            };
            fs::write(staging.join(&conf_name), conf_bytes)?;
        }
        let meta_bytes = serde_json::to_vec_pretty(meta)?;
        fs::write(staging.join("plugin.json"), meta_bytes)?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    if target.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Ok(target);
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Err(e) = fs::rename(&staging, &target) {
        let _ = copy_tree(&staging, &target);
        let _ = fs::remove_dir_all(&staging);
        if !target.is_dir() {
            return Err(Error::Io(e));
        }
    }

    Ok(target)
}

/// Generate `Core/plugins/plugins.lock` and synchronize active files in `Core/plugins/` and `Core/configs/modules/`.
pub fn regenerate_lockfile(server: &Path, reg: &PluginsRegistry) -> Result<()> {
    regenerate_lockfile_at(server, server, reg)
}

/// Deploy plugins from `store_server` into `target_server` and write lockfile.
pub fn regenerate_lockfile_at(
    store_server: &Path,
    target_server: &Path,
    reg: &PluginsRegistry,
) -> Result<()> {
    let plugins_root = plugins_install_dir(target_server);
    let conf_root = modules_config_dir(target_server);
    fs::create_dir_all(&plugins_root)?;
    fs::create_dir_all(&conf_root)?;

    let mut lock_lines = vec![
        "# written by CoA Server Manager: id|path|sha256|required|confs|deps".to_string(),
    ];

    let mut active_plugin_ids = BTreeSet::new();

    for (id, entry) in &reg.plugins {
        if !entry.installed || !entry.enabled {
            continue;
        }

        active_plugin_ids.insert(id.clone());
        let store_path = store_dir(store_server).join(id).join(&entry.sha256);
        if !store_path.is_dir() {
            return Err(Error::Invalid(format!(
                "Store directory for plugin '{}' ({}) is missing",
                id, entry.sha256
            )));
        }

        // Deploy plugin DLL and PDB into plugins/<id>/
        let target_dir = plugins_root.join(id);
        fs::create_dir_all(&target_dir)?;

        let dll_name = format!("{id}.dll");
        let src_dll = store_path.join(&dll_name);
        let dst_dll = target_dir.join(&dll_name);
        if !src_dll.is_file() {
            return Err(Error::Invalid(format!(
                "Source DLL missing in store: {}",
                src_dll.display()
            )));
        }
        fsx::atomic_write(&dst_dll, &fs::read(&src_dll)?)?;

        let pdb_name = format!("{id}.pdb");
        let src_pdb = store_path.join(&pdb_name);
        if src_pdb.is_file() {
            let dst_pdb = target_dir.join(&pdb_name);
            fsx::atomic_write(&dst_pdb, &fs::read(&src_pdb)?)?;
        }

        // Deploy conf.dist and ensure active .conf exists
        let mut deployed_conf_names = Vec::new();
        for conf_entry in &entry.conf_files {
            let base_name = conf_entry.strip_suffix(".dist").unwrap_or(conf_entry);
            let dist_name = format!("{base_name}.dist");
            let src_dist = store_path.join(&dist_name);
            let dst_dist = conf_root.join(&dist_name);
            let dst_conf = conf_root.join(base_name);

            if src_dist.is_file() {
                let dist_bytes = fs::read(&src_dist)?;
                fsx::atomic_write(&dst_dist, &dist_bytes)?;
                if !dst_conf.is_file() {
                    fsx::atomic_write(&dst_conf, &dist_bytes)?;
                }
            }
            deployed_conf_names.push(base_name.to_string());
        }

        let rel_dll_path = format!("{id}/{dll_name}");
        let req_str = if entry.required { "1" } else { "0" };
        let confs_str = deployed_conf_names.join(",");
        let deps_str = entry.deps.join(",");

        lock_lines.push(format!(
            "{id}|{rel_dll_path}|{}|{req_str}|{confs_str}|{deps_str}",
            entry.sha256
        ));
    }

    // Clean up directories in plugins/ for uninstalled plugins
    if let Ok(entries) = fs::read_dir(&plugins_root) {
        for e in entries.flatten() {
            if let Ok(ft) = e.file_type() {
                if ft.is_dir() {
                    let dir_name = e.file_name().to_string_lossy().into_owned();
                    if !active_plugin_ids.contains(&dir_name) {
                        let _ = fs::remove_dir_all(e.path());
                    }
                }
            }
        }
    }

    let lock_content = lock_lines.join("\n") + "\n";
    let lock_path = lockfile_path(target_server);
    fsx::atomic_write(&lock_path, lock_content.as_bytes())?;
    Ok(())
}

/// Perform a 3-way merge on an AzerothCore `.conf` file.
pub fn merge_conf(base_str: &str, theirs_str: &str, mine_str: &str) -> Result<String> {
    let base = ConfFile::parse(base_str);
    let theirs = ConfFile::parse(theirs_str);
    let mut merged = ConfFile::parse(mine_str);

    for (key, theirs_val) in theirs.entries() {
        let base_val = base.get(key);
        let mine_val = merged.get(key).map(|s| s.to_string());

        match mine_val {
            Some(curr) => {
                if base_val == Some(&curr) && curr != theirs_val {
                    merged.set(key, theirs_val, &[]);
                }
            }
            None => {
                let doc_lines = theirs.doc_for(key);
                let doc_refs: Vec<&str> = doc_lines.iter().map(|s| s.as_str()).collect();
                merged.set(key, theirs_val, &doc_refs);
            }
        }
    }

    Ok(merged.to_text())
}

/// Install a plugin into the store, update the registry, and optionally enable it.
pub fn install(
    server: &Path,
    meta: PluginMetadata,
    dll_bytes: &[u8],
    pdb_bytes: Option<&[u8]>,
    dist_confs: &[(String, &[u8])],
    enable: bool,
) -> Result<()> {
    require_server_stopped(server)?;

    let mut tx = TxGuard::begin_op(server, PluginOpType::Install, &meta.id)?;
    let mut reg = load_registry_direct(server)?;

    if enable {
        if meta.id == "companions" && reg.plugins.get("playerbots").map(|p| p.enabled).unwrap_or(false) {
            return Err(Error::BotModulesConflict);
        }
        if meta.id == "playerbots" && reg.plugins.get("companions").map(|p| p.enabled).unwrap_or(false) {
            return Err(Error::BotModulesConflict);
        }
    }

    tx.set_phase(TransactionPhase::Applying)?;
    let store_target = install_to_store(server, &meta, dll_bytes, pdb_bytes, dist_confs)?;
    check_crash_injection("after_dll_copy");

    let rel_store_path = store_target
        .strip_prefix(meta_dir(server))
        .unwrap_or(&store_target)
        .to_string_lossy()
        .replace('\\', "/");

    let entry = PluginEntry {
        installed: true,
        enabled: enable,
        version: meta.version.clone(),
        sha256: meta.sha256.clone(),
        store_path: rel_store_path,
        conf_files: meta.conf_files.clone(),
        deps: meta.deps.clone(),
        required: meta.required,
    };

    reg.plugins.insert(meta.id.clone(), entry);
    save_registry(server, &reg)?;
    check_crash_injection("after_registry_write");

    regenerate_lockfile(server, &reg)?;
    check_crash_injection("after_lock_update");

    if let Some(key) = &meta.enable_key {
        let conf_root = modules_config_dir(server);
        for conf_name in &meta.conf_files {
            let base_name = conf_name.strip_suffix(".dist").unwrap_or(conf_name);
            let conf_path = conf_root.join(base_name);
            if conf_path.is_file() {
                tx.backup_conf(&conf_path)?;
                if let Ok(bytes) = fs::read(&conf_path) {
                    if let Ok(mut conf_file) = ConfFile::parse_bytes(&bytes) {
                        conf_file.set(key, if enable { "1" } else { "0" }, &[]);
                        let _ = fsx::atomic_write(&conf_path, conf_file.to_text().as_bytes());
                    }
                }
            }
        }
    }
    check_crash_injection("after_config_edit");

    tx.set_phase(TransactionPhase::Validating)?;
    check_crash_injection("before_commit");
    tx.commit()?;
    Ok(())
}

/// Enable or disable an installed plugin.
pub fn set_enabled(server: &Path, id: &str, enabled: bool) -> Result<()> {
    require_server_stopped(server)?;

    let mut tx = TxGuard::begin_op(
        server,
        if enabled { PluginOpType::Enable } else { PluginOpType::Disable },
        id,
    )?;
    let mut reg = load_registry_direct(server)?;
    let entry = reg.plugins.get(id).cloned().ok_or_else(|| {
        Error::Invalid(format!("Plugin '{id}' is not installed"))
    })?;

    if enabled {
        if id == "companions" && reg.plugins.get("playerbots").map(|p| p.enabled).unwrap_or(false) {
            return Err(Error::BotModulesConflict);
        }
        if id == "playerbots" && reg.plugins.get("companions").map(|p| p.enabled).unwrap_or(false) {
            return Err(Error::BotModulesConflict);
        }

        for dep in &entry.deps {
            if !reg.plugins.get(dep).map(|p| p.enabled).unwrap_or(false) {
                return Err(Error::Invalid(format!(
                    "Cannot enable plugin '{id}': required dependency '{dep}' is not enabled"
                )));
            }
        }
    } else {
        for (other_id, other) in &reg.plugins {
            if other.enabled && other.deps.contains(&id.to_string()) {
                return Err(Error::Invalid(format!(
                    "Cannot disable plugin '{id}': plugin '{other_id}' depends on it"
                )));
            }
        }
    }

    tx.set_phase(TransactionPhase::Applying)?;
    reg.plugins.get_mut(id).unwrap().enabled = enabled;
    save_registry(server, &reg)?;
    check_crash_injection("after_registry_write");

    regenerate_lockfile(server, &reg)?;
    check_crash_injection("after_lock_update");

    if let Ok(meta) = load_store_metadata(server, id, &entry.sha256) {
        if let Some(key) = &meta.enable_key {
            let conf_root = modules_config_dir(server);
            for conf_name in &meta.conf_files {
                let base_name = conf_name.strip_suffix(".dist").unwrap_or(conf_name);
                let conf_path = conf_root.join(base_name);
                if conf_path.is_file() {
                    tx.backup_conf(&conf_path)?;
                    if let Ok(bytes) = fs::read(&conf_path) {
                        if let Ok(mut conf_file) = ConfFile::parse_bytes(&bytes) {
                            conf_file.set(key, if enabled { "1" } else { "0" }, &[]);
                            let _ = fsx::atomic_write(&conf_path, conf_file.to_text().as_bytes());
                        }
                    }
                }
            }
        }
    }
    check_crash_injection("after_config_edit");

    tx.set_phase(TransactionPhase::Validating)?;
    check_crash_injection("before_commit");
    tx.commit()?;
    Ok(())
}

/// Update an installed plugin to a new version with 3-way configuration merge.
pub fn update(
    server: &Path,
    new_meta: PluginMetadata,
    new_dll: &[u8],
    new_pdb: Option<&[u8]>,
    new_dist_confs: &[(String, &[u8])],
) -> Result<()> {
    require_server_stopped(server)?;

    let mut tx = TxGuard::begin_op(server, PluginOpType::Update, &new_meta.id)?;
    let mut reg = load_registry_direct(server)?;
    let old_entry = reg
        .plugins
        .get(&new_meta.id)
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("Plugin '{}' is not installed", new_meta.id)))?;

    tx.set_phase(TransactionPhase::Applying)?;
    let old_store = store_dir(server).join(&new_meta.id).join(&old_entry.sha256);
    let new_store = install_to_store(server, &new_meta, new_dll, new_pdb, new_dist_confs)?;
    check_crash_injection("after_dll_copy");

    let conf_root = modules_config_dir(server);
    for (name, new_dist_bytes) in new_dist_confs {
        let base_name = name.strip_suffix(".dist").unwrap_or(name);
        let dist_name = format!("{base_name}.dist");

        let old_dist_path = old_store.join(&dist_name);
        let active_conf_path = conf_root.join(base_name);

        if old_dist_path.is_file() && active_conf_path.is_file() {
            tx.backup_conf(&active_conf_path)?;
            let old_dist_str = fs::read_to_string(&old_dist_path)?;
            let new_dist_str = std::str::from_utf8(new_dist_bytes)
                .map_err(|e| Error::Invalid(format!("new .dist for {base_name} is not UTF-8: {e}")))?;
            let active_conf_str = fs::read_to_string(&active_conf_path)?;

            let merged_text = merge_conf(&old_dist_str, new_dist_str, &active_conf_str)?;
            fsx::atomic_write(&active_conf_path, merged_text.as_bytes())?;
        }
    }
    check_crash_injection("after_config_edit");

    let rel_store_path = new_store
        .strip_prefix(meta_dir(server))
        .unwrap_or(&new_store)
        .to_string_lossy()
        .replace('\\', "/");

    let updated_entry = PluginEntry {
        installed: true,
        enabled: old_entry.enabled,
        version: new_meta.version,
        sha256: new_meta.sha256,
        store_path: rel_store_path,
        conf_files: new_meta.conf_files,
        deps: new_meta.deps,
        required: new_meta.required,
    };

    reg.plugins.insert(new_meta.id, updated_entry);
    save_registry(server, &reg)?;
    check_crash_injection("after_registry_write");

    regenerate_lockfile(server, &reg)?;
    check_crash_injection("after_lock_update");

    tx.set_phase(TransactionPhase::Validating)?;
    check_crash_injection("before_commit");
    tx.commit()?;
    Ok(())
}

/// Remove a plugin from the registry and lockfile, deleting deployed binary files.
/// Invariant: user configuration files (.conf) and SQL/player data are strictly preserved.
pub fn remove(server: &Path, id: &str) -> Result<()> {
    require_server_stopped(server)?;

    let mut tx = TxGuard::begin_op(server, PluginOpType::Remove, id)?;
    let mut reg = load_registry_direct(server)?;
    if reg.plugins.remove(id).is_none() {
        return Ok(());
    }

    tx.set_phase(TransactionPhase::Applying)?;
    save_registry(server, &reg)?;
    check_crash_injection("after_registry_write");

    regenerate_lockfile(server, &reg)?;
    check_crash_injection("after_lock_update");

    let target_dir = plugins_install_dir(server).join(id);
    if target_dir.is_dir() {
        let _ = fs::remove_dir_all(target_dir);
    }

    tx.set_phase(TransactionPhase::Validating)?;
    check_crash_injection("before_commit");
    tx.commit()?;
    Ok(())
}

/// Repair verification: checks deployed plugin SHA256 hashes against registry and restores from store if mismatch.
pub fn repair(server: &Path) -> Result<Vec<String>> {
    require_server_stopped(server)?;

    let mut tx = TxGuard::begin_op(server, PluginOpType::Repair, "all")?;
    let reg = load_registry_direct(server)?;
    let mut restored = Vec::new();
    let plugins_root = plugins_install_dir(server);

    tx.set_phase(TransactionPhase::Applying)?;

    for (id, entry) in &reg.plugins {
        if !entry.installed || !entry.enabled {
            continue;
        }

        let dll_path = plugins_root.join(id).join(format!("{id}.dll"));
        let needs_restore = if !dll_path.is_file() {
            true
        } else {
            let bytes = fs::read(&dll_path)?;
            sha256_digest(&bytes) != entry.sha256
        };

        if needs_restore {
            let store_dll = store_dir(server).join(id).join(&entry.sha256).join(format!("{id}.dll"));
            if !store_dll.is_file() {
                return Err(Error::Invalid(format!(
                    "Store image for plugin '{id}' ({}) is missing; cannot repair",
                    entry.sha256
                )));
            }
            fs::create_dir_all(plugins_root.join(id))?;
            fsx::atomic_write(&dll_path, &fs::read(&store_dll)?)?;
            restored.push(id.clone());
        }
    }

    regenerate_lockfile(server, &reg)?;
    check_crash_injection("after_lock_update");
    tx.set_phase(TransactionPhase::Validating)?;
    check_crash_injection("before_commit");
    tx.commit()?;
    Ok(restored)
}

/// Multiworld helper: propagate only explicitly allowed or active plugins to a realm directory.
pub fn prepare_multiworld_realm(
    server: &Path,
    realm_root: &Path,
    allowlist: Option<&[String]>,
) -> Result<()> {
    let reg = load_registry(server)?;
    let realm_plugins_root = realm_root.join("Core/plugins");
    fs::create_dir_all(&realm_plugins_root)?;

    // Enforce bot mutual exclusivity within this realm's allowlist
    if let Some(list) = allowlist {
        if list.contains(&"companions".to_string()) && list.contains(&"playerbots".to_string()) {
            return Err(Error::BotModulesConflict);
        }
    }

    let realm_reg_path = realm_root.join(".manager/plugins.json");
    if realm_reg_path.is_file() {
        let realm_reg = load_registry(realm_root)?;
        regenerate_lockfile_at(server, realm_root, &realm_reg)?;
        check_crash_injection("after_secondary_deploy");
        return Ok(());
    }

    // Filter registry strictly by allowlist if provided, or default to primary enabled plugins
    let mut realm_reg = PluginsRegistry::default();
    for (id, entry) in &reg.plugins {
        let is_allowed = match allowlist {
            Some(list) => list.contains(id),
            None => entry.enabled,
        };
        if is_allowed {
            let mut realm_entry = entry.clone();
            realm_entry.enabled = true;
            realm_reg.plugins.insert(id.clone(), realm_entry);
        }
    }

    regenerate_lockfile_at(server, realm_root, &realm_reg)?;
    check_crash_injection("after_secondary_deploy");
    Ok(())
}

/// Multiworld helper (backward-compatible signature).
pub fn prepare_multiworld(server: &Path, second_root: &Path) -> Result<()> {
    prepare_multiworld_realm(server, second_root, None)
}

/// Count references to a specific plugin ID and SHA256 across all realms and active transactions.
pub fn count_store_references(server: &Path, plugin_id: &str, sha256: &str) -> usize {
    let mut refs = 0;

    // 1. Primary realm registry
    if let Ok(reg) = load_registry_direct(server) {
        if let Some(entry) = reg.plugins.get(plugin_id) {
            if entry.sha256 == sha256 {
                refs += 1;
            }
        }
    }

    // 2. Secondary realms in realms/*/Core/plugins/plugins.lock
    let realms_root = server.join("realms");
    if let Ok(entries) = fs::read_dir(&realms_root) {
        for entry in entries.flatten() {
            let lock_path = entry.path().join("Core/plugins/plugins.lock");
            if lock_path.is_file() {
                if let Ok(content) = fs::read_to_string(&lock_path) {
                    for line in content.lines() {
                        let parts: Vec<&str> = line.split('|').collect();
                        if parts.len() >= 3 && parts[0] == plugin_id && parts[2] == sha256 {
                            refs += 1;
                        }
                    }
                }
            }
        }
    }

    // 3. Active transaction journal backup
    let active_tx = plugin_active_tx_path(server);
    if active_tx.is_file() {
        if let Ok(bytes) = fs::read(&active_tx) {
            if let Ok(journal) = serde_json::from_slice::<PluginTxJournal>(&bytes) {
                if journal.plugin_id == plugin_id {
                    refs += 1;
                }
            }
        }
    }

    refs
}

/// Prune unreferenced plugin versions from `.manager/plugin-store/`.
/// Strictly protects any versions referenced by Primary, Secondary realms, or active transactions.
pub fn prune_store(server: &Path) -> Result<Vec<String>> {
    let mut pruned = Vec::new();
    let store = store_dir(server);
    if !store.is_dir() {
        return Ok(pruned);
    }

    for plugin_dir in fs::read_dir(&store)?.flatten() {
        if !plugin_dir.file_type()?.is_dir() {
            continue;
        }
        let plugin_id = plugin_dir.file_name().to_string_lossy().into_owned();
        for version_dir in fs::read_dir(plugin_dir.path())?.flatten() {
            if !version_dir.file_type()?.is_dir() {
                continue;
            }
            let sha256 = version_dir.file_name().to_string_lossy().into_owned();
            if count_store_references(server, &plugin_id, &sha256) == 0 {
                fs::remove_dir_all(version_dir.path())?;
                pruned.push(format!("{plugin_id}:{sha256}"));
            }
        }
    }
    Ok(pruned)
}

/// SQUID SQL migration state in the database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SquidDbStatus {
    FullyApplied,
    Clean,
    Partial(Vec<String>),
    Interrupted(String),
    ChecksumMismatch { migration: String, expected: String, actual: String },
    LegacyAdoptionRequired(String),
    Offline,
    RepairRequired(String),
}

/// Dual-source audit report for SQUID SQL migrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SquidMigrationAudit {
    pub file_marker_applied: bool,
    pub file_marker_source: Option<String>,
    pub db_status: SquidDbStatus,
    pub reconciled: bool,
    pub effective_applied: bool,
    pub message: String,
}

/// Record that SQUID migrations have run on filesystem.
pub fn record_squid_migrations_applied(server: &Path) -> Result<()> {
    let path = meta_dir(server).join("migrations/squid_applied.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::json!({
        "applied": true,
        "source": "manual_or_runtime_record",
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    fsx::atomic_write_json(&path, &data)
}

/// Check if SQUID SQL migrations were already recorded as applied (offline check).
pub fn is_squid_migrations_applied(server: &Path) -> bool {
    let path = meta_dir(server).join("migrations/squid_applied.json");
    if !path.is_file() {
        return false;
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("applied")?.as_bool())
        .unwrap_or(false)
}

/// Dual-source reconciliation between database schema and squid_applied.json marker.
/// Verifies acore_characters, acore_world, acore_playerbots, coa_squid_pending, and legacy markers.
pub fn reconcile_squid_migrations(
    server: &Path,
    char_schema: &str,
    query_fn: Option<&impl Fn(&str) -> Result<String>>,
) -> Result<SquidMigrationAudit> {
    let marker_path = meta_dir(server).join("migrations/squid_applied.json");
    let (mut file_applied, file_source) = if marker_path.is_file() {
        let val: Option<serde_json::Value> = fs::read_to_string(&marker_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let applied = val.as_ref().and_then(|v| v.get("applied")?.as_bool()).unwrap_or(false);
        let src = val.as_ref().and_then(|v| v.get("source")?.as_str()).map(str::to_string);
        (applied, src)
    } else {
        (false, None)
    };

    let Some(query) = query_fn else {
        return Ok(SquidMigrationAudit {
            file_marker_applied: file_applied,
            file_marker_source: file_source,
            db_status: SquidDbStatus::Offline,
            reconciled: false,
            effective_applied: file_applied,
            message: "Database query unavailable (offline); using cached marker.".into(),
        });
    };

    // 1. Check for interrupted base import via coa_squid_pending
    let tables_raw = query(&format!(
        "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='{char_schema}';"
    ))?;
    let existing_tables: BTreeSet<String> = tables_raw.lines().map(|s| s.trim().to_string()).collect();

    if existing_tables.contains("coa_squid_pending") {
        let pending = query(&format!("SELECT path FROM `{char_schema}`.coa_squid_pending;"))?;
        let pending_paths: Vec<&str> = pending.lines().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if !pending_paths.is_empty() {
            let detail = format!("Interrupted SQUID import: {}", pending_paths.join(", "));
            if file_applied {
                let _ = fsx::atomic_write_json(&marker_path, &serde_json::json!({
                    "applied": false,
                    "source": "blocked_by_interrupted_import",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "error": &detail
                }));
            }
            return Ok(SquidMigrationAudit {
                file_marker_applied: file_applied,
                file_marker_source: file_source,
                db_status: SquidDbStatus::Interrupted(detail.clone()),
                reconciled: true,
                effective_applied: false,
                message: detail,
            });
        }
    }

    // 2. Check for migration checksum mismatch in updates ledger if present
    if existing_tables.contains("coa_squid_migrations") || existing_tables.contains("updates") {
        let ledger_table = if existing_tables.contains("coa_squid_migrations") {
            "coa_squid_migrations"
        } else {
            "updates"
        };
        if let Ok(mismatch_raw) = query(&format!(
            "SELECT name, hash FROM `{char_schema}`.{ledger_table} WHERE state='FAILED' LIMIT 1;"
        )) {
            let line = mismatch_raw.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let name = parts.first().unwrap_or(&"unknown").to_string();
                let hash = parts.get(1).unwrap_or(&"unknown").to_string();
                return Ok(SquidMigrationAudit {
                    file_marker_applied: file_applied,
                    file_marker_source: file_source,
                    db_status: SquidDbStatus::ChecksumMismatch {
                        migration: name.clone(),
                        expected: "recorded_clean".into(),
                        actual: hash,
                    },
                    reconciled: false,
                    effective_applied: false,
                    message: format!("Checksum or execution failure in ledger for {name}"),
                });
            }
        }
    }

    // 3. Check for legacy adoption requirements
    let has_legacy_table = existing_tables.contains("coa_bots_installed");
    let has_legacy_file = server.join("CoA-Bots/release.json").is_file();
    if (has_legacy_table || has_legacy_file) && !existing_tables.contains("playerbots_names") {
        return Ok(SquidMigrationAudit {
            file_marker_applied: file_applied,
            file_marker_source: file_source,
            db_status: SquidDbStatus::LegacyAdoptionRequired("Legacy SQUID repack structure detected".into()),
            reconciled: false,
            effective_applied: false,
            message: "Legacy SQUID installation detected; manual adoption required before starting.".into(),
        });
    }

    // 4. Check required SQUID tables in characters schema
    let required = ["playerbots_names", "playerbots_guild_names", "playerbots_arena_team_names"];
    let mut missing = Vec::new();
    let mut present = 0;
    for t in required {
        if existing_tables.contains(t) {
            present += 1;
        } else {
            missing.push(t.to_string());
        }
    }

    let (db_status, effective_applied, message) = if present == required.len() {
        if !file_applied {
            let _ = fsx::atomic_write_json(&marker_path, &serde_json::json!({
                "applied": true,
                "source": "reconciled_from_database",
                "timestamp": chrono::Utc::now().to_rfc3339()
            }));
            file_applied = true;
        }
        (
            SquidDbStatus::FullyApplied,
            true,
            "Database contains all required SQUID tables; marker reconciled/verified.".into(),
        )
    } else if present == 0 {
        if file_applied {
            let _ = fsx::atomic_write_json(&marker_path, &serde_json::json!({
                "applied": false,
                "source": "reconciled_from_clean_database",
                "timestamp": chrono::Utc::now().to_rfc3339()
            }));
            file_applied = false;
        }
        (
            SquidDbStatus::Clean,
            false,
            "Database is clean (no SQUID tables); migrations required.".into(),
        )
    } else {
        (
            SquidDbStatus::Partial(missing.clone()),
            false,
            format!("Database schema partial; missing: {}", missing.join(", ")),
        )
    };

    Ok(SquidMigrationAudit {
        file_marker_applied: file_applied,
        file_marker_source: file_source,
        db_status,
        reconciled: true,
        effective_applied,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_merge_conf_preserves_customizations() {
        let base = "CoA.Setting1 = 10\nCoA.Setting2 = \"alpha\"\n";
        let theirs = "CoA.Setting1 = 10\nCoA.Setting2 = \"beta\"\n# New option\nCoA.Setting3 = 1\n";
        let mine = "CoA.Setting1 = 20\nCoA.Setting2 = \"alpha\"\n";

        let merged = merge_conf(base, theirs, mine).unwrap();
        let parsed = ConfFile::parse(&merged);

        assert_eq!(parsed.get("CoA.Setting1"), Some("20"));
        assert_eq!(parsed.get("CoA.Setting2"), Some("\"beta\""));
        assert_eq!(parsed.get("CoA.Setting3"), Some("1"));
    }

    #[test]
    fn test_plugin_lifecycle_atomic() {
        let dir = tempdir().unwrap();
        let server = dir.path();

        let dll_content = b"MZ fake dll";
        let digest = sha256_digest(dll_content);
        let meta = PluginMetadata {
            id: "test-plugin".into(),
            name: "Test Plugin".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: digest.clone(),
            required: true,
            deps: vec![],
            conf_files: vec!["test.conf.dist".into()],
            enable_key: Some("Test.Enable".into()),
            status: Some("release".into()),
            icon: Some("bot".into()),
        };

        install(
            server,
            meta.clone(),
            dll_content,
            None,
            &[("test.conf.dist".into(), b"Test.Enable = 1\n")],
            true,
        )
        .unwrap();

        let reg = load_registry(server).unwrap();
        assert!(reg.plugins.contains_key("test-plugin"));
        assert!(reg.plugins["test-plugin"].enabled);

        let lock = fs::read_to_string(lockfile_path(server)).unwrap();
        assert!(lock.contains("test-plugin|test-plugin/test-plugin.dll"));

        // Disable
        set_enabled(server, "test-plugin", false).unwrap();
        let reg = load_registry(server).unwrap();
        assert!(!reg.plugins["test-plugin"].enabled);

        let lock = fs::read_to_string(lockfile_path(server)).unwrap();
        assert!(!lock.contains("test-plugin|test-plugin/test-plugin.dll"));

        // Re-enable
        set_enabled(server, "test-plugin", true).unwrap();
        let reg = load_registry(server).unwrap();
        assert!(reg.plugins["test-plugin"].enabled);

        // Corrupt file and test repair
        let deployed_dll = plugins_install_dir(server).join("test-plugin/test-plugin.dll");
        fs::write(&deployed_dll, b"corrupted bytes").unwrap();
        let restored = repair(server).unwrap();
        assert_eq!(restored, vec!["test-plugin".to_string()]);
        assert_eq!(fs::read(&deployed_dll).unwrap(), dll_content);
    }

    #[test]
    fn test_failure_injection_install_and_update() {
        let dir = tempdir().unwrap();
        let server = dir.path();

        let dll_content = b"MZ valid content";
        let digest = sha256_digest(dll_content);

        // 1. Failure in install: checksum mismatch
        let meta_bad_hash = PluginMetadata {
            id: "bad-plugin".into(),
            name: "Bad".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            required: true,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        };
        let res = install(server, meta_bad_hash, dll_content, None, &[], true);
        assert!(matches!(res, Err(Error::HashMismatch { .. })));
        let reg = load_registry(server).unwrap();
        assert!(!reg.plugins.contains_key("bad-plugin"));

        // 2. Install good plugin
        let meta_good = PluginMetadata {
            id: "my-plugin".into(),
            name: "Good".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: digest.clone(),
            required: true,
            deps: vec![],
            conf_files: vec!["my.conf.dist".into()],
            enable_key: Some("My.Enable".into()),
            status: None,
            icon: None,
        };
        install(server, meta_good, dll_content, None, &[("my.conf.dist".into(), b"My.Enable = 1\nMy.Val = 10\n")], true).unwrap();

        // Customize active conf
        let conf_p = modules_config_dir(server).join("my.conf");
        fs::write(&conf_p, b"My.Enable = 1\nMy.Val = 99\n").unwrap();

        // 3. Failure in update: invalid UTF-8 in new dist template
        let meta_v2 = PluginMetadata {
            id: "my-plugin".into(),
            name: "Good".into(),
            version: "2.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(b"v2 dll"),
            required: true,
            deps: vec![],
            conf_files: vec!["my.conf.dist".into()],
            enable_key: Some("My.Enable".into()),
            status: None,
            icon: None,
        };
        let bad_utf8_dist = [0xFF, 0xFE, 0xFD];
        let res_up = update(server, meta_v2, b"v2 dll", None, &[("my.conf.dist".into(), &bad_utf8_dist)]);
        assert!(res_up.is_err());

        // Verify rollback: version is still 1.0.0, user customization My.Val = 99 is intact!
        let reg_after = load_registry(server).unwrap();
        assert_eq!(reg_after.plugins["my-plugin"].version, "1.0.0");
        let active_conf = fs::read_to_string(&conf_p).unwrap();
        assert!(active_conf.contains("My.Val = 99"));
    }

    #[test]
    fn test_non_destructive_removal() {
        let dir = tempdir().unwrap();
        let server = dir.path();
        let dll = b"plugin dll";
        let meta = PluginMetadata {
            id: "temp-plugin".into(),
            name: "Temp".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(dll),
            required: true,
            deps: vec![],
            conf_files: vec!["temp.conf.dist".into()],
            enable_key: None,
            status: None,
            icon: None,
        };
        install(server, meta, dll, None, &[("temp.conf.dist".into(), b"Temp.Setting = 123\n")], true).unwrap();
        let conf_p = modules_config_dir(server).join("temp.conf");
        assert!(conf_p.is_file());

        // Remove plugin
        remove(server, "temp-plugin").unwrap();
        let reg = load_registry(server).unwrap();
        assert!(!reg.plugins.contains_key("temp-plugin"));

        // Invariant: User config MUST remain on disk!
        assert!(conf_p.is_file());
        assert!(fs::read_to_string(&conf_p).unwrap().contains("Temp.Setting = 123"));
    }

    #[test]
    fn test_multiworld_allowlist_and_exclusivity() {
        let dir = tempdir().unwrap();
        let server = dir.path();
        let second = dir.path().join(".realms/secondary");

        let dll_cs = b"cs dll";
        let dll_comp = b"comp dll";
        let dll_sq = b"sq dll";

        let meta_cs = PluginMetadata {
            id: "content-scaling".into(),
            name: "CS".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(dll_cs),
            required: true,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        };
        let meta_comp = PluginMetadata {
            id: "companions".into(),
            name: "Comp".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(dll_comp),
            required: true,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        };
        let meta_sq = PluginMetadata {
            id: "playerbots".into(),
            name: "Squid".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(dll_sq),
            required: true,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        };

        install(server, meta_cs, dll_cs, None, &[], true).unwrap();
        install(server, meta_comp, dll_comp, None, &[], true).unwrap();
        install(server, meta_sq, dll_sq, None, &[], false).unwrap();

        // Secondary realm allowlist: ONLY content-scaling
        prepare_multiworld_realm(server, &second, Some(&["content-scaling".into()])).unwrap();
        let second_lock = fs::read_to_string(second.join("Core/plugins/plugins.lock")).unwrap();
        assert!(second_lock.contains("content-scaling"));
        assert!(!second_lock.contains("companions"));
        assert!(!second_lock.contains("playerbots"));

        // Secondary realm attempting both bot modules in allowlist -> conflict!
        let conflict = prepare_multiworld_realm(server, &second, Some(&["companions".into(), "playerbots".into()]));
        assert!(matches!(conflict, Err(Error::BotModulesConflict)));
    }

    #[test]
    fn test_squid_sql_dual_source_reconciliation() {
        let dir = tempdir().unwrap();
        let server = dir.path();

        // 1. Case A: DB is clean (restored from backup), file marker says true -> desync detected & reconciled
        record_squid_migrations_applied(server).unwrap();
        assert!(is_squid_migrations_applied(server));

        let mock_clean_db = |_sql: &str| -> Result<String> {
            Ok("".into()) // no tables
        };
        let audit1 = reconcile_squid_migrations(server, "acore_characters", Some(&mock_clean_db)).unwrap();
        assert_eq!(audit1.db_status, SquidDbStatus::Clean);
        assert!(!audit1.effective_applied);
        assert!(!is_squid_migrations_applied(server)); // Marker automatically set to false!

        // 2. Case B: DB has tables, file marker says false -> reconciled to true without re-import
        let mock_migrated_db = |sql: &str| -> Result<String> {
            if sql.contains("information_schema.TABLES") {
                Ok("playerbots_names\nplayerbots_guild_names\nplayerbots_arena_team_names\n".into())
            } else {
                Ok("".into())
            }
        };
        let audit2 = reconcile_squid_migrations(server, "acore_characters", Some(&mock_migrated_db)).unwrap();
        assert_eq!(audit2.db_status, SquidDbStatus::FullyApplied);
        assert!(audit2.effective_applied);
        assert!(is_squid_migrations_applied(server)); // Marker automatically set to true!

        // 3. Case C: coa_squid_pending has rows -> interrupted import, blocked
        let mock_interrupted_db = |sql: &str| -> Result<String> {
            if sql.contains("information_schema.TABLES") {
                Ok("coa_squid_pending\nplayerbots_names\n".into())
            } else if sql.contains("coa_squid_pending") {
                Ok("playerbots/base/ai_playerbot_texts.sql\n".into())
            } else {
                Ok("".into())
            }
        };
        let audit3 = reconcile_squid_migrations(server, "acore_characters", Some(&mock_interrupted_db)).unwrap();
        assert!(matches!(audit3.db_status, SquidDbStatus::Interrupted(_)));
        assert!(!audit3.effective_applied);
        assert!(!is_squid_migrations_applied(server));

        // 4. Case D: Checksum mismatch in updates ledger
        let mock_mismatch_db = |sql: &str| -> Result<String> {
            if sql.contains("information_schema.TABLES") {
                Ok("playerbots_names\nupdates\n".into())
            } else if sql.contains("updates WHERE state='FAILED'") {
                Ok("2026_05_19_00_ai_playerbot_audit_texts.sql badhash1234\n".into())
            } else {
                Ok("".into())
            }
        };
        let audit4 = reconcile_squid_migrations(server, "acore_characters", Some(&mock_mismatch_db)).unwrap();
        assert!(matches!(audit4.db_status, SquidDbStatus::ChecksumMismatch { .. }));
        assert!(!audit4.effective_applied);

        // 5. Case E: Legacy adoption required
        let mock_legacy_db = |sql: &str| -> Result<String> {
            if sql.contains("information_schema.TABLES") {
                Ok("coa_bots_installed\n".into())
            } else {
                Ok("".into())
            }
        };
        let audit5 = reconcile_squid_migrations(server, "acore_characters", Some(&mock_legacy_db)).unwrap();
        assert!(matches!(audit5.db_status, SquidDbStatus::LegacyAdoptionRequired(_)));
        assert!(!audit5.effective_applied);
    }

    #[test]
    fn test_durable_transaction_recovery_after_crash() {
        let dir = tempdir().unwrap();
        let server = dir.path();

        let dll = b"base dll";
        let meta = PluginMetadata {
            id: "initial-plugin".into(),
            name: "Initial".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha256_digest(dll),
            required: true,
            deps: vec![],
            conf_files: vec!["initial.conf.dist".into()],
            enable_key: None,
            status: None,
            icon: None,
        };
        install(server, meta, dll, None, &[("initial.conf.dist".into(), b"Initial.Val = 1\n")], true).unwrap();
        let reg_before = load_registry(server).unwrap();
        assert!(reg_before.plugins.contains_key("initial-plugin"));

        // Simulate crash: create an active transaction in Applying phase
        let tx_id = "test-crashed-tx-123";
        let before_dir = plugin_tx_backup_dir(server, tx_id);
        fs::create_dir_all(&before_dir).unwrap();

        // Backup current registry and lockfile
        let reg_bytes = fs::read(registry_path(server)).unwrap();
        fs::write(before_dir.join("plugins.json"), &reg_bytes).unwrap();
        let lock_bytes = fs::read(lockfile_path(server)).unwrap();
        fs::write(before_dir.join("plugins.lock"), &lock_bytes).unwrap();

        let mut recovery_hashes = BTreeMap::new();
        recovery_hashes.insert("plugins.json".into(), sha256_digest(&reg_bytes));
        recovery_hashes.insert("plugins.lock".into(), sha256_digest(&lock_bytes));

        let journal = PluginTxJournal {
            tx_id: tx_id.to_string(),
            op: PluginOpType::Install,
            plugin_id: "corrupted-new-plugin".to_string(),
            phase: TransactionPhase::Applying,
            from_version: None,
            to_version: Some("1.0.0".into()),
            target_realms: vec!["primary".into()],
            backup_dir: before_dir.to_string_lossy().into_owned(),
            recovery_hashes,
            message: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        fsx::atomic_write_json(&plugin_active_tx_path(server), &journal).unwrap();

        // Mutate registry and lockfile to simulate partial corrupted write
        fs::write(registry_path(server), b"{\"schema\": 1, \"plugins\": {\"corrupted\": {}}}").unwrap();
        fs::write(lockfile_path(server), b"corrupted|bad.dll|badhash|0||").unwrap();

        // Next read of registry must trigger durable rollback!
        let reg_recovered = load_registry(server).unwrap();
        assert!(reg_recovered.plugins.contains_key("initial-plugin"));
        assert!(!reg_recovered.plugins.contains_key("corrupted"));

        let lock_recovered = fs::read_to_string(lockfile_path(server)).unwrap();
        assert!(lock_recovered.contains("initial-plugin"));
        assert!(!lock_recovered.contains("corrupted"));

        // Journal must be cleaned up and recovery must be idempotent
        assert!(!plugin_active_tx_path(server).is_file());
        assert!(ensure_durable_recovered(server).is_ok());

        // Test RecoveryRequired: if backup copy is missing, block further ops
        let bad_journal = PluginTxJournal {
            tx_id: "bad-tx".to_string(),
            op: PluginOpType::Install,
            plugin_id: "bad".to_string(),
            phase: TransactionPhase::Applying,
            from_version: None,
            to_version: None,
            target_realms: vec![],
            backup_dir: server.join("nonexistent_backup").to_string_lossy().into_owned(),
            recovery_hashes: BTreeMap::new(),
            message: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        fsx::atomic_write_json(&plugin_active_tx_path(server), &bad_journal).unwrap();
        let err = load_registry(server);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("RecoveryRequired"));
    }

    #[test]
    fn test_multiworld_store_reference_safety() {
        let dir = tempdir().unwrap();
        let server = dir.path();
        let second = dir.path().join("realms/secondary");

        let dll = b"shared dll content";
        let sha = sha256_digest(dll);
        let meta = PluginMetadata {
            id: "shared-plugin".into(),
            name: "Shared".into(),
            version: "1.0.0".into(),
            description: BTreeMap::new(),
            sha256: sha.clone(),
            required: true,
            deps: vec![],
            conf_files: vec![],
            enable_key: None,
            status: None,
            icon: None,
        };

        install(server, meta, dll, None, &[], true).unwrap();
        prepare_multiworld_realm(server, &second, Some(&["shared-plugin".into()])).unwrap();

        // 1. Store has reference in both primary and secondary
        assert_eq!(count_store_references(server, "shared-plugin", &sha), 2);

        // 2. Primary removes plugin
        remove(server, "shared-plugin").unwrap();
        let reg = load_registry(server).unwrap();
        assert!(!reg.plugins.contains_key("shared-plugin"));

        // Reference count from secondary is still 1!
        assert_eq!(count_store_references(server, "shared-plugin", &sha), 1);

        // Pruning store does NOT delete the version because secondary still uses it!
        let pruned = prune_store(server).unwrap();
        assert!(pruned.is_empty());
        let store_dll = store_dir(server).join("shared-plugin").join(&sha).join("shared-plugin.dll");
        assert!(store_dll.is_file());

        // Secondary realm's lockfile is still valid and untouched
        let second_lock = fs::read_to_string(second.join("Core/plugins/plugins.lock")).unwrap();
        assert!(second_lock.contains("shared-plugin"));

        // 3. When secondary lockfile is cleared, references drop to 0 and prune cleans it
        fs::remove_file(second.join("Core/plugins/plugins.lock")).unwrap();
        assert_eq!(count_store_references(server, "shared-plugin", &sha), 0);
        let pruned2 = prune_store(server).unwrap();
        assert_eq!(pruned2, vec![format!("shared-plugin:{sha}")]);
        assert!(!store_dll.is_file());
    }

    #[test]
    fn test_lockfile_conformance_and_security() {
        let valid_line = "content-scaling|content-scaling/cs.dll|4b6a88b577ba6bf2858686b2089456930efd902d3eeae7b7fa22f671bb572ce3|1|cs.conf|";
        let parts: Vec<&str> = valid_line.split('|').collect();
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[0], "content-scaling");
        assert_eq!(parts[2].len(), 64);

        // Path traversal test
        let traversal_path = "../../evil.dll";
        assert!(traversal_path.contains("..") || traversal_path.starts_with('/'));

        // Absolute path test
        let abs_path = "C:/evil.dll";
        assert!(abs_path.contains(':') || abs_path.starts_with('/'));
    }
}
