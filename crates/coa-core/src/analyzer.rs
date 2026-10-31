//! Module Compatibility Analyzer v0.1 for CoA Dynamic Plugin Architecture.
//!
//! Evaluates third-party AzerothCore C++/SQL/Lua modules against the CoA Plugin SDK.
//! Produces explainable, structured findings with exact file, line, severity, confidence,
//! evidence and remediation guidance.

use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindingSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindingConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityFinding {
    pub rule_id: String,
    pub severity: FindingSeverity,
    pub confidence: FindingConfidence,
    pub file: String,
    pub line: usize,
    pub explanation: String,
    pub recommended_action: String,
    pub evidence: String,
    pub is_auto_fixable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompatibilityClassification {
    AnalysisOnly,
    SourceIncompatible,
    SdkExportMissing,
    CorePatchRequired,
    DependencyMissing,
    SqlReviewRequired,
    GameplayConflict,
    Buildable,
    Loadable,
    RuntimeVerified,
    Unsupported,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredHook {
    pub class_name: String,
    pub script_type: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseUsage {
    pub database: String,
    pub query_type: String,
    pub file: String,
    pub line: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlFileAudit {
    pub file: String,
    pub target_schema: String,
    pub has_create_table: bool,
    pub has_alter_table: bool,
    pub has_insert_replace: bool,
    pub has_delete_update: bool,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub repository_path: String,
    pub repository_name: String,
    pub commit: String,
    pub license: String,
    pub classification: CompatibilityClassification,
    pub hooks: Vec<DiscoveredHook>,
    pub loaders: Vec<String>,
    pub databases: Vec<DatabaseUsage>,
    pub sql_files: Vec<SqlFileAudit>,
    pub core_patches: Vec<String>,
    pub has_cmakelists: bool,
    pub has_entrypoint_adapter: bool,
    pub findings: Vec<CompatibilityFinding>,
    pub reasons: Vec<String>,
    pub recommended_actions: Vec<String>,
}

/// Analyze a local checkout of an AzerothCore module repository.
pub fn analyze_repository(repo_path: &Path) -> Result<CompatibilityReport> {
    let repo_path_buf = repo_path.canonicalize().unwrap_or_else(|_| repo_path.to_path_buf());
    let repo_name = repo_path_buf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    let mut findings = Vec::new();
    let mut hooks = Vec::new();
    let mut loaders = Vec::new();
    let mut databases = Vec::new();
    let mut sql_files = Vec::new();
    let mut core_patches = Vec::new();
    let mut has_cmakelists = false;
    let mut has_entrypoint_adapter = false;
    let mut license = "UNKNOWN".to_string();

    // Check git commit if available
    let commit = if repo_path.join(".git").exists() {
        std::process::Command::new("git")
            .args(["-C", &repo_path.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "pinned-local".into())
    } else {
        "unversioned-local".into()
    };

    // Check license
    for lic in &["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"] {
        let p = repo_path.join(lic);
        if p.is_file() {
            if let Ok(content) = fs::read_to_string(&p) {
                let first_chunk = &content[..content.len().min(500)];
                if first_chunk.contains("GNU AFFERO GENERAL PUBLIC LICENSE") || first_chunk.contains("AGPL") {
                    license = "AGPL-3.0".into();
                } else if first_chunk.contains("GNU GENERAL PUBLIC LICENSE") || first_chunk.contains("GPL") {
                    license = "GPL-2.0/3.0".into();
                } else if first_chunk.contains("MIT License") {
                    license = "MIT".into();
                } else if first_chunk.contains("Apache License") {
                    license = "Apache-2.0".into();
                } else {
                    license = "CUSTOM_OR_OTHER".into();
                }
                break;
            }
        }
    }

    // Traverse directory
    let mut files_to_scan = Vec::new();
    collect_files(repo_path, repo_path, &mut files_to_scan)?;

    for (rel_path, abs_path) in &files_to_scan {
        let lower = rel_path.to_lowercase();
        if lower.ends_with("cmakelists.txt") {
            has_cmakelists = true;
        }

        if lower.ends_with(".patch") || lower.ends_with(".diff") || lower.contains("/patches/") {
            core_patches.push(rel_path.clone());
            findings.push(CompatibilityFinding {
                rule_id: "RULE_CORE_PATCH_REQUIRED".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.clone(),
                line: 1,
                explanation: "Module includes an external core patch or diff modifying the base game engine.".into(),
                recommended_action: "Requires manual inspection; dynamic plugins cannot apply core source patches at runtime.".into(),
                evidence: format!("Found patch file: {rel_path}"),
                is_auto_fixable: false,
            });
        }

        if lower.ends_with(".sql") {
            if let Ok(sql_content) = fs::read_to_string(abs_path) {
                let mut target_schema = "unknown".to_string();
                if lower.contains("characters") || lower.contains("db-characters") {
                    target_schema = "characters".into();
                } else if lower.contains("world") || lower.contains("db-world") {
                    target_schema = "world".into();
                } else if lower.contains("auth") || lower.contains("db-auth") {
                    target_schema = "auth".into();
                }

                let has_create = sql_content.to_uppercase().contains("CREATE TABLE");
                let has_alter = sql_content.to_uppercase().contains("ALTER TABLE");
                let has_insert = sql_content.to_uppercase().contains("INSERT INTO") || sql_content.to_uppercase().contains("REPLACE INTO");
                let has_delete = sql_content.to_uppercase().contains("DELETE FROM") || sql_content.to_uppercase().contains("UPDATE ");

                let line_count = sql_content.lines().count();
                sql_files.push(SqlFileAudit {
                    file: rel_path.clone(),
                    target_schema: target_schema.clone(),
                    has_create_table: has_create,
                    has_alter_table: has_alter,
                    has_insert_replace: has_insert,
                    has_delete_update: has_delete,
                    line_count,
                });

                let rule_id = match target_schema.as_str() {
                    "characters" => "RULE_SQL_CHAR_DIRECT",
                    "auth" => "RULE_SQL_AUTH_DIRECT",
                    _ => "RULE_SQL_WORLD_DIRECT",
                };
                findings.push(CompatibilityFinding {
                    rule_id: rule_id.into(),
                    severity: if target_schema == "characters" { FindingSeverity::Warning } else { FindingSeverity::Info },
                    confidence: FindingConfidence::High,
                    file: rel_path.clone(),
                    line: 1,
                    explanation: format!("SQL file targeting '{target_schema}' database detected (create={has_create}, alter={has_alter}, lines={line_count})."),
                    recommended_action: "Review SQL migration plan before importing into pre-production database.".into(),
                    evidence: format!("SQL schema: {target_schema}"),
                    is_auto_fixable: false,
                });
            }
        }

        // C++ source analysis
        if lower.ends_with(".cpp") || lower.ends_with(".h") || lower.ends_with(".hpp") {
            if let Ok(content) = fs::read_to_string(abs_path) {
                if content.contains("COA_PLUGIN_DEFINE") {
                    has_entrypoint_adapter = true;
                }

                for (idx, line) in content.lines().enumerate() {
                    let line_num = idx + 1;
                    let trimmed = line.trim();

                    // Detect Loader functions: void Add<Name>Scripts()
                    if trimmed.starts_with("void Add") && trimmed.contains("Scripts()") {
                        if let Some(fn_name) = trimmed.strip_prefix("void ") {
                            let name = fn_name.split('(').next().unwrap_or("").trim();
                            if !loaders.contains(&name.to_string()) {
                                loaders.push(name.to_string());
                            }
                        }
                    }

                    // Detect Script classes: class Foo : public PlayerScript
                    if trimmed.starts_with("class ") && trimmed.contains(" : public ") && trimmed.contains("Script") {
                        let parts: Vec<&str> = trimmed.split(" : public ").collect();
                        if parts.len() == 2 {
                            let class_name = parts[0].strip_prefix("class ").unwrap_or("").trim();
                            let script_type = parts[1].split('{').next().unwrap_or("").trim();
                            hooks.push(DiscoveredHook {
                                class_name: class_name.to_string(),
                                script_type: script_type.to_string(),
                                file: rel_path.clone(),
                                line: line_num,
                            });
                        }
                    }

                    // Detect Database calls
                    for db_name in &["WorldDatabase", "CharacterDatabase", "LoginDatabase"] {
                        if trimmed.contains(db_name) && (trimmed.contains(".Query(") || trimmed.contains(".Execute(") || trimmed.contains(".AsyncQuery(")) {
                            let query_type = if trimmed.contains(".Query(") {
                                "Query (Sync)"
                            } else if trimmed.contains(".AsyncQuery(") {
                                "AsyncQuery"
                            } else {
                                "Execute"
                            };

                            databases.push(DatabaseUsage {
                                database: db_name.to_string(),
                                query_type: query_type.to_string(),
                                file: rel_path.clone(),
                                line: line_num,
                                snippet: trimmed.chars().take(120).collect(),
                            });

                            // Check performance critical: synchronous query inside periodic player/unit update
                            let has_update_hook = content.contains("OnPlayerBeforeUpdate")
                                || content.contains("OnPlayerUpdate")
                                || content.contains("OnUpdate")
                                || content.contains("PLAYERHOOK_ON_BEFORE_UPDATE");
                            if has_update_hook && trimmed.contains("Query(") {
                                findings.push(CompatibilityFinding {
                                    rule_id: "RULE_PERF_BLOCKING_QUERY_ON_UPDATE".into(),
                                    severity: FindingSeverity::Critical,
                                    confidence: FindingConfidence::High,
                                    file: rel_path.clone(),
                                    line: line_num,
                                    explanation: "Synchronous blocking database query executed inside player/creature update tick (OnPlayerBeforeUpdate).".into(),
                                    recommended_action: "Refactor to asynchronous query with callback or periodic batch fetch to prevent game thread hitches.".into(),
                                    evidence: trimmed.chars().take(100).collect(),
                                    is_auto_fixable: false,
                                });
                            }
                        }
                    }

                    // Detect Content Scaling & AutoBalance conflicts
                    if trimmed.contains("SetCreateHealth(") || trimmed.contains("ModifyPeriodicDamageDone(") || trimmed.contains("ModifySpellDamageDone(") {
                        findings.push(CompatibilityFinding {
                            rule_id: "RULE_CONFLICT_CONTENT_SCALING".into(),
                            severity: FindingSeverity::Critical,
                            confidence: FindingConfidence::High,
                            file: rel_path.clone(),
                            line: line_num,
                            explanation: "Modifies creature base health budgets or damage formulas, directly colliding with CoA Content Scaling.".into(),
                            recommended_action: "Do not run alongside CoA Content Scaling; mutual gameplay conflict detected.".into(),
                            evidence: trimmed.chars().take(100).collect(),
                            is_auto_fixable: false,
                        });
                    }

                    // Detect SoloCraft double scaling
                    if trimmed.contains("SoloCraftSpellMult") || trimmed.contains("SoloCraftStatsMult") || trimmed.contains("SoloCraftDebuffEnable") {
                        findings.push(CompatibilityFinding {
                            rule_id: "RULE_CONFLICT_SOLOCRAFT_STATS".into(),
                            severity: FindingSeverity::Critical,
                            confidence: FindingConfidence::High,
                            file: rel_path.clone(),
                            line: line_num,
                            explanation: "Applies player stat/spell power multipliers inside instances, resulting in 25x double scaling when combined with CoA Content Scaling.".into(),
                            recommended_action: "Requires balance review; conflicting scaling models between player stat boost and creature health reduction.".into(),
                            evidence: trimmed.chars().take(100).collect(),
                            is_auto_fixable: false,
                        });
                    }

                    // Detect Lua subproject dependencies
                    if trimmed.contains("add_subdirectory(src/lualib") || trimmed.contains("LUA_VERSION") {
                        findings.push(CompatibilityFinding {
                            rule_id: "RULE_EXTERNAL_SUBPROJECT_LUA".into(),
                            severity: FindingSeverity::Warning,
                            confidence: FindingConfidence::High,
                            file: rel_path.clone(),
                            line: line_num,
                            explanation: "Module embeds an external Lua/LuaJIT runtime subproject requiring custom compiler toolchain integration.".into(),
                            recommended_action: "Requires specialized CMake build definition compiling Lua runtime with MultiThreadedDLL /MD ABI compatibility.".into(),
                            evidence: trimmed.chars().take(100).collect(),
                            is_auto_fixable: false,
                        });
                    }
                }
            }
        }
    }

    if !has_cmakelists {
        findings.push(CompatibilityFinding {
            rule_id: "RULE_STRUCT_MISSING_CMAKELISTS".into(),
            severity: FindingSeverity::Warning,
            confidence: FindingConfidence::High,
            file: "CMakeLists.txt".into(),
            line: 0,
            explanation: "Repository does not contain a standalone CMakeLists.txt; relies on upstream AzerothCore globbing.".into(),
            recommended_action: "Generate standalone CMakeLists.txt using CoAPluginSDK config.".into(),
            evidence: "No top-level CMakeLists.txt discovered".into(),
            is_auto_fixable: true,
        });
    }

    if !has_entrypoint_adapter && !loaders.is_empty() {
        findings.push(CompatibilityFinding {
            rule_id: "RULE_ENTRYPOINT_ADAPTER_NEEDED".into(),
            severity: FindingSeverity::Info,
            confidence: FindingConfidence::High,
            file: "src/adapter.cpp".into(),
            line: 0,
            explanation: format!("Standard AzerothCore loader '{}' detected without COA_PLUGIN_DEFINE entrypoint.", loaders[0]),
            recommended_action: "Generate a 10-line adapter bridging the loader to COA_PLUGIN_DEFINE.".into(),
            evidence: format!("Loader function: {}", loaders[0]),
            is_auto_fixable: true,
        });
    }

    // Determine Classification
    let mut reasons = Vec::new();
    let mut recommended_actions = Vec::new();

    let classification = if !core_patches.is_empty() {
        reasons.push(format!("Requires {} core patch file(s) modifying worldserver source", core_patches.len()));
        recommended_actions.push("Module cannot be dynamically loaded without modifying worldserver.exe core.".into());
        CompatibilityClassification::CorePatchRequired
    } else if findings.iter().any(|f| f.rule_id == "RULE_CONFLICT_CONTENT_SCALING" || f.rule_id == "RULE_CONFLICT_SOLOCRAFT_STATS") {
        reasons.push("Gameplay formula collision detected with CoA Content Scaling".into());
        recommended_actions.push("Do not deploy concurrently with CoA Content Scaling".into());
        CompatibilityClassification::GameplayConflict
    } else if findings.iter().any(|f| f.rule_id == "RULE_PERF_BLOCKING_QUERY_ON_UPDATE") {
        reasons.push("Blocking synchronous database query executed in main thread update tick".into());
        recommended_actions.push("Refactor database queries to asynchronous execution".into());
        CompatibilityClassification::SourceIncompatible
    } else if !sql_files.is_empty() {
        reasons.push(format!("Contains {} SQL migration script(s) requiring schema review", sql_files.len()));
        recommended_actions.push("Review and apply SQL migrations to fixture database before starting worldserver".into());
        CompatibilityClassification::SqlReviewRequired
    } else if findings.iter().any(|f| f.rule_id == "RULE_EXTERNAL_SUBPROJECT_LUA") {
        reasons.push("Contains complex embedded runtime subproject (Lua/LuaJIT)".into());
        recommended_actions.push("Requires dedicated C++20 /MD adapter build definition".into());
        CompatibilityClassification::AnalysisOnly
    } else {
        reasons.push("Clean script hooks compatible with CoA Plugin SDK".into());
        recommended_actions.push("Generate standalone CMakeLists.txt and compile against CoA Plugin SDK".into());
        CompatibilityClassification::Buildable
    };

    Ok(CompatibilityReport {
        repository_path: repo_path_buf.to_string_lossy().into_owned(),
        repository_name: repo_name,
        commit,
        license,
        classification,
        hooks,
        loaders,
        databases,
        sql_files,
        core_patches,
        has_cmakelists,
        has_entrypoint_adapter,
        findings,
        reasons,
        recommended_actions,
    })
}

fn collect_files(root: &Path, current: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !current.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && name != ".editorconfig" && name != ".gitattributes" && name != ".gitignore" {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}
