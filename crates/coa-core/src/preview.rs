//! CSM Modules Preview Orchestration Layer.
//!
//! Provides the end-to-end workflows for:
//! - Automated Toolchain discovery
//! - GitHub module intake and compatibility qualification
//! - Controlled Build Engine execution
//! - Transactional plugin installation with Stop-Gate and SQL Review Gate
//! - Plugin lifecycle operations (enable, disable, remove, repair)
//! - Sanitized diagnostic report generation

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyzer::{CompatibilityReport, SqlFileAudit};
use crate::error::{Error, Result};
use crate::plugins::{install, load_registry, remove, require_server_stopped, set_enabled, PluginMetadata};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlReviewPlan {
    pub module_id: String,
    pub total_sql_files: usize,
    pub files: Vec<SqlFileAudit>,
    pub target_schemas: Vec<String>,
    pub contains_schema_mutations: bool,
    pub warning: String,
    pub user_confirmation_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewInstallResult {
    pub success: bool,
    pub module_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub sql_applied: bool,
    pub message: String,
    pub sql_plan: Option<SqlReviewPlan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPreviewPluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub sha256: String,
    pub store_path: String,
    pub conf_files: Vec<String>,
    #[serde(default)]
    pub source_commit: Option<String>,
}

/// Lists all dynamically installed plugins for a realm from .manager/plugins.json.
pub fn list_installed_preview_plugins(realm_root: &Path) -> Result<Vec<InstalledPreviewPluginView>> {
    let reg = load_registry(realm_root)?;
    let mut views = Vec::new();
    for (id, entry) in reg.plugins {
        let mut display_name = id.clone();
        let mut source_commit = None;

        // Attempt to read display_name or metadata from plugin.json in store
        let store_dir = realm_root.join(".manager").join(&entry.store_path);
        let plugin_json_path = store_dir.join("plugin.json");
        if plugin_json_path.is_file() {
            if let Ok(content) = fs::read_to_string(&plugin_json_path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(dn) = val.get("display_name").and_then(|v| v.as_str()) {
                        display_name = dn.to_string();
                    } else if let Some(n) = val.get("name").and_then(|v| v.as_str()) {
                        if n.len() != 40 || !n.chars().all(|c| c.is_ascii_hexdigit()) {
                            display_name = n.to_string();
                        }
                    }
                    if let Some(sc) = val.get("source_commit").and_then(|v| v.as_str()) {
                        source_commit = Some(sc.to_string());
                    }
                }
            }
        }

        // If still a 40-char SHA, resolve via known configs or loader
        if display_name.len() == 40 && display_name.chars().all(|c| c.is_ascii_hexdigit()) {
            source_commit = Some(display_name.clone());
            if entry.conf_files.iter().any(|c| c.contains("npc_buffer")) {
                display_name = "NPC Buffer".to_string();
            } else if entry.conf_files.iter().any(|c| c.contains("ah_bot") || c.contains("ahbot")) {
                display_name = "Auction House Bot".to_string();
            } else if entry.conf_files.iter().any(|c| c.contains("enchanter")) {
                display_name = "NPC Enchanter".to_string();
            } else if entry.conf_files.iter().any(|c| c.contains("congrats")) {
                display_name = "Congrats On Level".to_string();
            } else if entry.conf_files.iter().any(|c| c.contains("reward_played_time")) {
                display_name = "Reward Played Time".to_string();
            } else {
                display_name = format!("Module ({})", &display_name[..8]);
            }
        }

        views.push(InstalledPreviewPluginView {
            id,
            name: display_name,
            version: entry.version,
            enabled: entry.enabled,
            sha256: entry.sha256,
            store_path: entry.store_path,
            conf_files: entry.conf_files,
            source_commit,
        });
    }
    Ok(views)
}

/// Generates an SQL review plan for a given module source directory.
pub fn plan_sql_review(module_id: &str, report: &CompatibilityReport) -> Option<SqlReviewPlan> {
    if report.sql_files.is_empty() {
        return None;
    }

    let mut schemas = Vec::new();
    let mut mutations = false;

    for s in &report.sql_files {
        if !schemas.contains(&s.target_schema) {
            schemas.push(s.target_schema.clone());
        }
        if s.has_create_table || s.has_alter_table {
            mutations = true;
        }
    }

    Some(SqlReviewPlan {
        module_id: module_id.to_string(),
        total_sql_files: report.sql_files.len(),
        files: report.sql_files.clone(),
        target_schemas: schemas,
        contains_schema_mutations: mutations,
        warning: if mutations {
            "Module contains DDL schema mutations (CREATE/ALTER TABLE). DDL cannot be rolled back via standard transactions. Verify against test database before proceeding.".into()
        } else {
            "Module contains DML data inserts or queries.".into()
        },
        user_confirmation_required: true,
    })
}

/// Installs a qualified build package into the realm's Plugin Store and enables it.
/// Enforces Stop-Gate and SQL Review Gate.
pub fn install_qualified_module(
    realm_root: &Path,
    module_id: &str,
    version: &str,
    apply_sql: bool,
    sql_plan: Option<&SqlReviewPlan>,
) -> Result<PreviewInstallResult> {
    // 1. Enforce Stop-Gate
    require_server_stopped(realm_root)?;

    // 2. Check SQL Review Gate
    if let Some(plan) = sql_plan {
        if !apply_sql && plan.total_sql_files > 0 {
            return Ok(PreviewInstallResult {
                success: false,
                module_id: module_id.to_string(),
                display_name: module_id.to_string(),
                enabled: false,
                sql_applied: false,
                message: "SQL Review required before activating module. User confirmation needed.".into(),
                sql_plan: Some(plan.clone()),
            });
        }
    }

    // 3. Locate package in plugin store
    let store_dir = realm_root.join(".manager").join("plugin-store").join(module_id).join(version);
    let dll_name = format!("{}.dll", module_id);
    let dll_path = store_dir.join(&dll_name);

    if !dll_path.is_file() {
        return Err(Error::Invalid(format!(
            "Plugin package DLL not found in store: {}", dll_path.display()
        )));
    }

    let dll_bytes = fs::read(&dll_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&dll_bytes);
    let sha256 = format!("{:x}", hasher.finalize());

    // Resolve human-readable display_name from plugin.json if available
    let plugin_json_path = store_dir.join("plugin.json");
    let mut display_name = module_id.to_string();
    if plugin_json_path.is_file() {
        if let Ok(content) = fs::read_to_string(&plugin_json_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(dn) = val.get("display_name").and_then(|v| v.as_str()) {
                    display_name = dn.to_string();
                } else if let Some(n) = val.get("name").and_then(|v| v.as_str()) {
                    if n.len() != 40 || !n.chars().all(|c| c.is_ascii_hexdigit()) {
                        display_name = n.to_string();
                    }
                }
            }
        }
    }

    // Collect dist confs
    let mut dist_confs = Vec::new();
    if let Ok(entries) = fs::read_dir(&store_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "dist").unwrap_or(false) {
                if let Ok(bytes) = fs::read(&p) {
                    let name = p.file_name().unwrap().to_string_lossy().into_owned();
                    dist_confs.push((name, bytes));
                }
            }
        }
    }

    if display_name.len() == 40 && display_name.chars().all(|c| c.is_ascii_hexdigit()) {
        if dist_confs.iter().any(|(n, _)| n.contains("npc_buffer")) {
            display_name = "NPC Buffer".to_string();
        } else if dist_confs.iter().any(|(n, _)| n.contains("ah_bot") || n.contains("ahbot")) {
            display_name = "Auction House Bot".to_string();
        } else if dist_confs.iter().any(|(n, _)| n.contains("enchanter")) {
            display_name = "NPC Enchanter".to_string();
        } else if dist_confs.iter().any(|(n, _)| n.contains("congrats")) {
            display_name = "Congrats On Level".to_string();
        } else if dist_confs.iter().any(|(n, _)| n.contains("reward_played_time")) {
            display_name = "Reward Played Time".to_string();
        } else {
            display_name = format!("Module ({})", &display_name[..8]);
        }
    }

    let meta = PluginMetadata {
        id: module_id.to_string(),
        name: display_name.clone(),
        version: version.to_string(),
        description: BTreeMap::new(),
        sha256,
        required: false,
        deps: vec![],
        conf_files: dist_confs.iter().map(|(n, _)| n.clone()).collect(),
        enable_key: None,
        status: Some("release".into()),
        icon: None,
    };

    let dist_confs_ref: Vec<(String, &[u8])> = dist_confs.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();

    // 4. Execute transactional install into store & enable in plugins.lock
    install(realm_root, meta, &dll_bytes, None, &dist_confs_ref, true)?;

    Ok(PreviewInstallResult {
        success: true,
        module_id: module_id.to_string(),
        display_name: display_name.clone(),
        enabled: true,
        sql_applied: apply_sql,
        message: format!("Plugin '{}' v{} installed and enabled successfully in test realm.", display_name, version),
        sql_plan: None,
    })
}

/// Toggles a module's enabled state with Stop-Gate protection.
pub fn toggle_preview_module(realm_root: &Path, module_id: &str, enable: bool) -> Result<String> {
    require_server_stopped(realm_root)?;
    set_enabled(realm_root, module_id, enable)?;
    Ok(format!(
        "Plugin '{}' has been {}",
        module_id,
        if enable { "ENABLED" } else { "DISABLED" }
    ))
}

/// Safely removes a module from the realm. Preserves SQL data.
pub fn remove_preview_module(realm_root: &Path, module_id: &str) -> Result<String> {
    require_server_stopped(realm_root)?;
    remove(realm_root, module_id)?;
    Ok(format!(
        "Plugin '{}' removed from realm plugins. Database tables and user data were preserved.",
        module_id
    ))
}
