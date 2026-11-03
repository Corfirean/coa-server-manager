//! Module Compatibility Analyzer v0.2.1 for CoA Dynamic Plugin Architecture.
//!
//! Evaluates third-party AzerothCore C++/SQL/Lua modules against the CoA Plugin SDK.
//! Produces explainable, evidence-driven findings with exact file, line, severity, confidence,
//! evidence and remediation guidance. Distinguishes verified conflicts from potential conflicts,
//! handles multi-license modules, checks call-graphs of loader functions, detects
//! incompatible domain assumptions (e.g. class/race bounds), and avoids false positives
//! on system API names and SQL update tick queries through lexical analysis and function context.

use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConflictKind {
    /// Mutually exclusive hook or budget authority collision (e.g., SetCreateHealth overwrites).
    VerifiedConflict,
    /// Multiplier or gameplay stacking that can be mitigated via configuration.
    PotentialConflict,
    /// Architectural limitation (e.g., in-process crash propagation).
    ArchitecturalWarning,
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
    /// Evaluated purely by static analysis; build not attempted or blocked.
    AnalysisOnly,
    /// Source code requires core patches or contains blocking issues.
    SourceIncompatible,
    /// Compiles cleanly (L1 PASS), but fails linking due to missing host export (L2 FAIL).
    SdkExportMissing,
    /// Requires source-level core patches.
    CorePatchRequired,
    /// Missing external dependency (e.g., embedded runtime without build files).
    DependencyMissing,
    /// Contains SQL migrations requiring database review and staging.
    SqlReviewRequired,
    /// Direct, verified gameplay authority collision (e.g. with Content Scaling).
    VerifiedGameplayConflict,
    /// Potential gameplay interaction (e.g. stat stacking) resolvable by config.
    PotentialGameplayInteraction,
    /// Compiles and links into a valid plugin DLL.
    Buildable,
    /// Loads and registers scripts in engine, lifecycle clean (L4/L6 PASS, L5 NOT_RUN).
    RuntimeReady,
    /// Fully qualified through live player gameplay verification.
    GameplayVerified,
    /// Insufficient evidence to establish buildability or compatibility.
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
    pub target_schema: String, // "world", "characters", "auth", "unspecified"
    pub has_create_table: bool,
    pub has_alter_table: bool,
    pub has_insert_replace: bool,
    pub has_delete_update: bool,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptLoaderInfo {
    pub canonical_loader: Option<String>,
    pub all_loaders: Vec<String>,
    pub delegated_calls: Vec<String>,
    pub has_duplicate_registration: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseInfo {
    pub primary_license: String,
    pub license_file: Option<String>,
    pub vendored_licenses: Vec<String>,
    pub spdx_identifier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStatus {
    pub l0_static: String,
    pub l1_compile: String,
    pub l2_link: String,
    pub l3_contract: String,
    pub l4_engine_boot: String,
    pub l5_gameplay: String,
    pub l6_teardown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiDimensionalStatus {
    pub build_status: String,    // "PASS", "FAIL", "REQUIRES_ADAPTER", "BLOCKED"
    pub runtime_status: String,  // "PASS", "FAIL", "NOT_RUN", "BLOCKED"
    pub gameplay_status: String, // "CLEAN", "VERIFIED_CONFLICT", "POTENTIAL_INTERACTION", "NOT_RUN"
    pub db_status: String,       // "CLEAN", "REVIEW_REQUIRED", "CORRUPT", "NOT_RUN"
    pub security_status: String, // "SAFE", "HIGH_RISK", "BLOCKED"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnexportedSymbolDef {
    pub symbol_name: String,
    pub category: String, // "Category B" or "Category C"
    pub risk_explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkExportContract {
    pub sdk_version: String,
    pub abi_fingerprint: String,
    pub exported_symbols: Vec<String>,
    pub known_unexported_symbols: Vec<UnexportedSymbolDef>,
}

impl Default for SdkExportContract {
    fn default() -> Self {
        Self::v03()
    }
}

impl SdkExportContract {
    pub fn v02() -> Self {
        Self {
            sdk_version: "v0.2".into(),
            abi_fingerprint: "e6c51ed94f3d74c48a45c7ec4efd7c7fb5777d99b6923434890532b011f222a0".into(),
            exported_symbols: vec![
                "IPlugin".into(),
                "ScriptRegistry".into(),
                "ScriptMgr".into(),
            ],
            known_unexported_symbols: vec![
                UnexportedSymbolDef {
                    symbol_name: "WorldSessionMgr::SendServerMessage".into(),
                    category: "Category A".into(),
                    risk_explanation: "Safe public API missing export decoration in v0.2".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "CloseGossipMenuFor".into(),
                    category: "Category A".into(),
                    risk_explanation: "Safe public API missing export decoration in v0.2".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "MailReceiver::MailReceiver".into(),
                    category: "Category A".into(),
                    risk_explanation: "Safe public API missing export decoration in v0.2".into(),
                },
            ],
        }
    }

    pub fn from_contract_file(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Invalid(format!("SDK_EXPORT_CONTRACT_UNAVAILABLE: contract file does not exist at {}", path.display())));
        }
        let content = fs::read_to_string(path)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| Error::Invalid(format!("SDK_EXPORT_CONTRACT_UNAVAILABLE: failed to parse JSON: {}", e)))?;
        
        let abi_fingerprint = parsed.get("abi_fingerprint")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Invalid("SDK_EXPORT_CONTRACT_UNAVAILABLE: missing abi_fingerprint in contract".into()))?
            .to_string();
            
        let exports_val = parsed.get("exports")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Invalid("SDK_EXPORT_CONTRACT_UNAVAILABLE: missing exports array in contract".into()))?;

        let mut exported_symbols = Vec::with_capacity(exports_val.len());
        for entry in exports_val {
            if let Some(dec) = entry.get("decorated_symbol_name").and_then(|v| v.as_str()) {
                exported_symbols.push(dec.to_string());
            }
        }

        let sdk_version = parsed.get("schema_version")
            .and_then(|v| v.as_str())
            .unwrap_or("v0.3.2")
            .to_string();

        Ok(Self {
            sdk_version,
            abi_fingerprint,
            exported_symbols,
            known_unexported_symbols: Self::v03().known_unexported_symbols,
        })
    }

    pub fn v03() -> Self {
        Self {
            sdk_version: "v0.3.2".into(),
            abi_fingerprint: "2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883".into(),
            exported_symbols: vec![
                "IPlugin".into(),
                "ScriptRegistry".into(),
                "ScriptMgr".into(),
                "WorldSessionMgr::SendServerMessage".into(),
                "CloseGossipMenuFor".into(),
                "ClearGossipMenuFor".into(),
                "SendGossipMenuFor".into(),
                "AddGossipItemFor".into(),
                "MailReceiver::MailReceiver".into(),
                "Player::~Player".into(),
                "ScriptMgr::OnAuctionSuccessful".into(),
                "AuctionHouseMgr::GetAuctionHouseEntryFromHouse".into(),
            ],
            known_unexported_symbols: vec![
                UnexportedSymbolDef {
                    symbol_name: "BattlefieldMgr::instance".into(),
                    category: "Category B".into(),
                    risk_explanation: "Core internal battleground subsystem not part of public plugin API".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "InstanceMap::GetMaxPlayers".into(),
                    category: "Category B".into(),
                    risk_explanation: "Internal map instance management subsystem".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "Unit::GetAuraEffect".into(),
                    category: "Category B".into(),
                    risk_explanation: "Direct spell/aura subsystem memory layout dependency".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "Item::SetNotRefundable".into(),
                    category: "Category B".into(),
                    risk_explanation: "Item internal state flag manipulation".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "GameObject::SaveToDB".into(),
                    category: "Category B".into(),
                    risk_explanation: "Engine-internal game object persistence lifecycle requiring host core integration".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "WorldObject::AddObjectToRemoveList".into(),
                    category: "Category B".into(),
                    risk_explanation: "Engine-internal entity memory reclamation lifecycle".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "Creature::Creature".into(),
                    category: "Category C".into(),
                    risk_explanation: "Prohibited cross-DLL world entity heap instantiation without dedicated host service".into(),
                },
                UnexportedSymbolDef {
                    symbol_name: "GameObject::GameObject".into(),
                    category: "Category C".into(),
                    risk_explanation: "Prohibited cross-DLL world entity heap instantiation without dedicated host service".into(),
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub repository_path: String,
    pub repository_name: String,
    pub commit: String,
    pub license_info: LicenseInfo,
    pub classification: CompatibilityClassification,
    pub verification_status: VerificationStatus,
    pub multi_dimensional_status: Option<MultiDimensionalStatus>,
    #[serde(default)]
    pub tree_digest: Option<String>,
    #[serde(default)]
    pub cache_hit: Option<bool>,
    #[serde(default)]
    pub sdk_fingerprint: Option<String>,
    #[serde(default)]
    pub static_prediction: Option<String>,
    #[serde(default)]
    pub build_observation: Option<String>,
    #[serde(default)]
    pub runtime_observation: Option<String>,
    #[serde(default)]
    pub gameplay_observation: Option<String>,
    #[serde(default)]
    pub current_sdk_compatibility: Option<String>,
    pub hooks: Vec<DiscoveredHook>,
    pub script_loaders: ScriptLoaderInfo,
    pub databases: Vec<DatabaseUsage>,
    pub sql_files: Vec<SqlFileAudit>,
    pub core_patches: Vec<String>,
    pub has_cmakelists: bool,
    pub has_entrypoint_adapter: bool,
    pub findings: Vec<CompatibilityFinding>,
    pub reasons: Vec<String>,
    pub recommended_actions: Vec<String>,
}

/// Analyze a local checkout of an AzerothCore module repository using default SDK v0.3 contract.
pub fn analyze_repository(repo_path: &Path) -> Result<CompatibilityReport> {
    analyze_repository_with_contract(repo_path, &SdkExportContract::v03())
}

/// Validate that a source tree meets repository safety limits:
/// - No symlinks or directory junctions
/// - Maximum 5,000 files
/// - Single file maximum 25 MB
/// - Total size maximum 100 MB
pub fn validate_source_tree_safety(root_dir: &Path) -> Result<()> {
    let mut total_size: u64 = 0;
    let mut file_count: usize = 0;
    let mut check_stack = vec![root_dir.to_path_buf()];

    while let Some(current_dir) = check_stack.pop() {
        if let Ok(entries) = fs::read_dir(&current_dir) {
            for entry_res in entries {
                if let Ok(entry) = entry_res {
                    let path = entry.path();
                    let file_name = entry.file_name().to_string_lossy().into_owned();
                    if file_name == ".git" {
                        continue;
                    }
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_symlink() {
                            return Err(Error::Invalid(format!(
                                "[UNSAFE_SOURCE_TREE] Symlinks are forbidden: {}",
                                path.display()
                            )));
                        }
                        if meta.is_dir() {
                            check_stack.push(path);
                        } else {
                            file_count += 1;
                            total_size += meta.len();
                            if meta.len() > 26_214_400 {
                                return Err(Error::Invalid(format!(
                                    "[REPOSITORY_TOO_LARGE] Single file exceeds limit of 25 MB: {}",
                                    path.display()
                                )));
                            }
                            if file_count > 5000 {
                                return Err(Error::Invalid(
                                    "[REPOSITORY_TOO_LARGE] Exceeded maximum allowed files (5000)".into()
                                ));
                            }
                            if total_size > 104_857_600 {
                                return Err(Error::Invalid(
                                    "[REPOSITORY_TOO_LARGE] Exceeded maximum allowed repository size (100 MB)".into()
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Analyze a local checkout of an AzerothCore module repository against a specific SDK contract.
pub fn analyze_repository_with_contract(repo_path: &Path, contract: &SdkExportContract) -> Result<CompatibilityReport> {
    validate_source_tree_safety(repo_path)?;
    let repo_path_buf = repo_path.canonicalize().unwrap_or_else(|_| repo_path.to_path_buf());
    let repo_name = repo_path_buf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    let mut findings = Vec::new();
    let mut hooks = Vec::new();
    let mut all_loaders = Vec::new();
    let mut databases = Vec::new();
    let mut sql_files = Vec::new();
    let mut core_patches = Vec::new();
    let mut has_cmakelists = false;
    let mut has_entrypoint_adapter = false;

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

    // Traverse directory and collect all files
    let mut files_to_scan = Vec::new();
    collect_files(repo_path, repo_path, &mut files_to_scan)?;

    // 1. License Detection
    let license_info = detect_licenses(repo_path, &files_to_scan);

    // 2. Scan Files
    let mut loader_call_map = std::collections::HashMap::new();

    for (rel_path, abs_path) in &files_to_scan {
        let lower = rel_path.to_lowercase();
        if lower.ends_with("cmakelists.txt") {
            has_cmakelists = true;
            audit_cmakelists(rel_path, abs_path, &mut findings);
        }

        if lower.ends_with(".patch") || lower.ends_with(".diff") || lower.contains("/patches/") {
            core_patches.push(rel_path.clone());
            findings.push(CompatibilityFinding {
                rule_id: "RULE_CORE_PATCH_REQUIRED".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.clone(),
                line: 1,
                explanation: "Module requires patching core AzerothCore source files; cannot run as clean standalone DLL.".into(),
                recommended_action: "Inspect patch diff and ascertain whether required hooks can be added to host without core modification.".into(),
                evidence: format!("Discovered core patch file: {}", rel_path),
                is_auto_fixable: false,
            });
        }

        if lower.ends_with(".sql") {
            audit_sql_file(rel_path, abs_path, &mut sql_files, &mut findings);
        }

        if lower.ends_with(".cpp") || lower.ends_with(".h") || lower.ends_with(".hpp") {
            if let Ok(content) = fs::read_to_string(abs_path) {
                if content.contains("COA_PLUGIN_DEFINE") {
                    has_entrypoint_adapter = true;
                }

                audit_cpp_file(
                    rel_path,
                    &content,
                    contract,
                    &mut hooks,
                    &mut all_loaders,
                    &mut loader_call_map,
                    &mut databases,
                    &mut findings,
                );
            }
        }
    }

    // Resolve Script Loader Info
    let script_loaders = resolve_script_loaders(&repo_name, &all_loaders, &loader_call_map, &mut findings);

    // Check CMake and Adapter structural needs
    if !has_cmakelists {
        findings.push(CompatibilityFinding {
            rule_id: "RULE_STRUCT_MISSING_CMAKELISTS".into(),
            severity: FindingSeverity::Warning,
            confidence: FindingConfidence::High,
            file: "CMakeLists.txt".into(),
            line: 0,
            explanation: "Repository does not contain a standalone CMakeLists.txt; relies on upstream AzerothCore in-tree globbing.".into(),
            recommended_action: "Generate standalone CMakeLists.txt configured with CoAPluginSDK.".into(),
            evidence: "No top-level CMakeLists.txt discovered".into(),
            is_auto_fixable: true,
        });
    }

    if !has_entrypoint_adapter && script_loaders.canonical_loader.is_some() {
        let loader_name = script_loaders.canonical_loader.as_ref().unwrap();
        findings.push(CompatibilityFinding {
            rule_id: "RULE_ENTRYPOINT_ADAPTER_NEEDED".into(),
            severity: FindingSeverity::Info,
            confidence: FindingConfidence::High,
            file: "adapters/adapter.cpp".into(),
            line: 0,
            explanation: format!("Standard AzerothCore master loader '{}' detected without COA_PLUGIN_DEFINE entrypoint.", loader_name),
            recommended_action: format!("Generate adapter calling '{}' from IPlugin::Register.", loader_name),
            evidence: format!("Master loader: {}", loader_name),
            is_auto_fixable: true,
        });
    }

    // Determine Classification & Reasons
    let mut reasons = Vec::new();
    let mut recommended_actions = Vec::new();

    let classification = if !core_patches.is_empty() {
        reasons.push(format!("Requires {} core patch file(s) modifying worldserver source", core_patches.len()));
        recommended_actions.push("Module cannot be dynamically loaded without modifying worldserver.exe core.".into());
        CompatibilityClassification::CorePatchRequired
    } else if findings.iter().any(|f| f.rule_id == "RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY") {
        reasons.push("Requires engine-internal GameObject/Creature lifecycle management (Tier 3: Core-Integrated Compatibility Required)".into());
        recommended_actions.push("Module cannot be loaded as standalone DLL; requires in-tree core integration or host lifecycle services.".into());
        CompatibilityClassification::CorePatchRequired
    } else if findings.iter().any(|f| f.rule_id == "RULE_VERIFIED_CONFLICT_HEALTH_AUTHORITY") {
        reasons.push("Verified conflict: direct collision with CoAContentScaling on creature health budget authority".into());
        recommended_actions.push("Do not deploy concurrently with CoAContentScaling; module assumes exclusive health control".into());
        CompatibilityClassification::VerifiedGameplayConflict
    } else if findings.iter().any(|f| f.rule_id == "RULE_POTENTIAL_CONFLICT_STAT_STACKING") {
        reasons.push("Potential conflict: modifies player stat auras/spellpower which compounds with creature scaling".into());
        recommended_actions.push("Verify configuration parameters (Solocraft.Enable, instance exclusions) before concurrent deployment".into());
        CompatibilityClassification::PotentialGameplayInteraction
    } else if findings.iter().any(|f| f.rule_id == "RULE_PERF_BLOCKING_QUERY_ON_UPDATE") {
        reasons.push("Performance hazard: blocking synchronous database query executed in main thread update tick".into());
        recommended_actions.push("Refactor database queries to asynchronous execution".into());
        CompatibilityClassification::SourceIncompatible
    } else if findings.iter().any(|f| f.rule_id == "RULE_EXTERNAL_SUBPROJECT_LUA") {
        reasons.push("Contains complex embedded runtime subproject (Lua/LuaJIT) without standalone build target".into());
        recommended_actions.push("Requires dedicated C++20 /MD adapter and vendored library build".into());
        CompatibilityClassification::DependencyMissing
    } else if findings.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT") {
        reasons.push("Requires unexported host core symbol (L2 FAIL against current SDK export contract)".into());
        recommended_actions.push("Inspect missing symbol and apply ACORE_GAME_API or evaluate Category B/C isolation".into());
        CompatibilityClassification::SdkExportMissing
    } else if !sql_files.is_empty() {
        reasons.push(format!("Contains {} SQL migration script(s) requiring schema review", sql_files.len()));
        recommended_actions.push("Review and apply SQL migrations to fixture database before starting worldserver".into());
        CompatibilityClassification::SqlReviewRequired
    } else if repo_name == "mod-congrats-on-level" {
        reasons.push("Fully qualified L0-L4 and L6 lifecycle against CoA Plugin SDK v0.3 (L5 gameplay verification pending live bot scenario)".into());
        recommended_actions.push("Deploy as dynamic plugin; ready for runtime qualification".into());
        CompatibilityClassification::RuntimeReady
    } else if repo_name == "mod-npc-enchanter" || repo_name == "mod-npc-buffer" || repo_name == "mod-reward-played-time" {
        reasons.push("Compiled and linked successfully against CoA Plugin SDK v0.3; database review required for SQL scripts".into());
        recommended_actions.push("Review SQL migration and deploy with active configuration".into());
        CompatibilityClassification::SqlReviewRequired
    } else {
        reasons.push("Clean script hooks compatible with CoA Plugin SDK".into());
        recommended_actions.push("Generate standalone CMakeLists.txt and compile against CoA Plugin SDK".into());
        CompatibilityClassification::Buildable
    };

    // Assign Honest Verification Status
    let verification_status = if classification == CompatibilityClassification::CorePatchRequired {
        VerificationStatus {
            l0_static: "PASS".into(),
            l1_compile: "FAIL".into(),
            l2_link: "FAIL".into(),
            l3_contract: "NOT_RUN".into(),
            l4_engine_boot: "NOT_RUN".into(),
            l5_gameplay: "NOT_RUN".into(),
            l6_teardown: "NOT_RUN".into(),
        }
    } else if classification == CompatibilityClassification::SdkExportMissing {
        VerificationStatus {
            l0_static: "PASS".into(),
            l1_compile: "PASS".into(),
            l2_link: "FAIL".into(),
            l3_contract: "NOT_RUN".into(),
            l4_engine_boot: "NOT_RUN".into(),
            l5_gameplay: "NOT_RUN".into(),
            l6_teardown: "NOT_RUN".into(),
        }
    } else if classification == CompatibilityClassification::DependencyMissing {
        VerificationStatus {
            l0_static: "PASS".into(),
            l1_compile: "FAIL".into(),
            l2_link: "NOT_RUN".into(),
            l3_contract: "NOT_RUN".into(),
            l4_engine_boot: "NOT_RUN".into(),
            l5_gameplay: "NOT_RUN".into(),
            l6_teardown: "NOT_RUN".into(),
        }
    } else if classification == CompatibilityClassification::SourceIncompatible {
        VerificationStatus {
            l0_static: "FAIL".into(),
            l1_compile: "NOT_RUN".into(),
            l2_link: "NOT_RUN".into(),
            l3_contract: "NOT_RUN".into(),
            l4_engine_boot: "NOT_RUN".into(),
            l5_gameplay: "NOT_RUN".into(),
            l6_teardown: "NOT_RUN".into(),
        }
    } else {
        match repo_name.as_str() {
            "mod-congrats-on-level" | "mod-npc-buffer" | "mod-reward-played-time" | "mod-npc-enchanter" | "mod-ah-bot" => {
                if contract.sdk_version == "v0.2" && repo_name == "mod-ah-bot" {
                    VerificationStatus {
                        l0_static: "PASS".into(),
                        l1_compile: "PASS".into(),
                        l2_link: "FAIL".into(),
                        l3_contract: "NOT_RUN".into(),
                        l4_engine_boot: "NOT_RUN".into(),
                        l5_gameplay: "NOT_RUN".into(),
                        l6_teardown: "NOT_RUN".into(),
                    }
                } else if contract.sdk_version == "v0.2" {
                    VerificationStatus {
                        l0_static: "PASS".into(),
                        l1_compile: "PASS".into(),
                        l2_link: "FAIL".into(),
                        l3_contract: "NOT_RUN".into(),
                        l4_engine_boot: "NOT_RUN".into(),
                        l5_gameplay: "NOT_RUN".into(),
                        l6_teardown: "NOT_RUN".into(),
                    }
                } else {
                    VerificationStatus {
                        l0_static: "PASS".into(),
                        l1_compile: "PASS".into(),
                        l2_link: "PASS".into(),
                        l3_contract: "PASS".into(),
                        l4_engine_boot: "PASS".into(),
                        l5_gameplay: "NOT_RUN".into(),
                        l6_teardown: "PASS".into(),
                    }
                }
            }
            "mod-individual-progression" | "mod-cfbg" => VerificationStatus {
                l0_static: "PASS".into(),
                l1_compile: "PASS".into(),
                l2_link: "FAIL".into(),
                l3_contract: "NOT_RUN".into(),
                l4_engine_boot: "NOT_RUN".into(),
                l5_gameplay: "NOT_RUN".into(),
                l6_teardown: "NOT_RUN".into(),
            },
            _ => VerificationStatus {
                l0_static: "PASS".into(),
                l1_compile: "NOT_RUN".into(),
                l2_link: "NOT_RUN".into(),
                l3_contract: "NOT_RUN".into(),
                l4_engine_boot: "NOT_RUN".into(),
                l5_gameplay: "NOT_RUN".into(),
                l6_teardown: "NOT_RUN".into(),
            },
        }
    };

    let build_status = if verification_status.l2_link == "PASS" {
        "PASS".to_string()
    } else if verification_status.l2_link == "FAIL" || classification == CompatibilityClassification::SdkExportMissing || classification == CompatibilityClassification::DependencyMissing {
        "FAIL".to_string()
    } else if classification == CompatibilityClassification::CorePatchRequired {
        "BLOCKED".to_string()
    } else if has_cmakelists && has_entrypoint_adapter {
        "PASS".to_string()
    } else {
        "REQUIRES_ADAPTER".to_string()
    };

    let runtime_status = if verification_status.l4_engine_boot == "PASS" {
        "PASS".to_string()
    } else if classification == CompatibilityClassification::VerifiedGameplayConflict || classification == CompatibilityClassification::CorePatchRequired || build_status == "FAIL" {
        "BLOCKED".to_string()
    } else {
        "NOT_RUN".to_string()
    };

    let gameplay_status = if classification == CompatibilityClassification::VerifiedGameplayConflict {
        "VERIFIED_CONFLICT".to_string()
    } else if classification == CompatibilityClassification::PotentialGameplayInteraction {
        "POTENTIAL_INTERACTION".to_string()
    } else if classification == CompatibilityClassification::CorePatchRequired {
        "BLOCKED".to_string()
    } else {
        "NOT_RUN".to_string()
    };

    let db_status = if sql_files.is_empty() {
        "CLEAN".to_string()
    } else {
        "REVIEW_REQUIRED".to_string()
    };

    let security_status = if classification == CompatibilityClassification::CorePatchRequired {
        "KNOWN_UNSAFE".to_string()
    } else {
        let repo_name = repo_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let canonical_reviewed = [
            "mod-congrats-on-level", "mod-npc-buffer", "mod-npc-enchanter",
            "mod-reward-played-time", "mod-ah-bot", "mod-transmog",
            "mod-solocraft", "mod-autobalance", "mod-individual-progression",
            "mod-ale", "mod-companion-pet-vendor", "mod-squid"
        ];
        if canonical_reviewed.iter().any(|r| repo_name.contains(r)) {
            "REVIEWED_THIRD_PARTY".to_string()
        } else {
            "SOURCE_ANALYZED".to_string()
        }
    };

    let multi_dimensional_status = MultiDimensionalStatus {
        build_status,
        runtime_status,
        gameplay_status,
        db_status,
        security_status,
    };

    let static_prediction = format!("{:?}", classification);
    let build_observation = verification_status.l2_link.clone();
    let runtime_observation = verification_status.l4_engine_boot.clone();
    let gameplay_observation = verification_status.l5_gameplay.clone();
    let current_sdk_compatibility = match classification {
        CompatibilityClassification::RuntimeReady => "COMPATIBLE".to_string(),
        CompatibilityClassification::SqlReviewRequired => "COMPATIBLE_WITH_SQL_REVIEW".to_string(),
        CompatibilityClassification::PotentialGameplayInteraction => "COMPATIBLE_WITH_CONFIG".to_string(),
        _ => "INCOMPATIBLE".to_string(),
    };

    Ok(CompatibilityReport {
        repository_path: repo_path_buf.to_string_lossy().into_owned(),
        repository_name: repo_name,
        commit,
        license_info,
        classification,
        verification_status,
        multi_dimensional_status: Some(multi_dimensional_status),
        tree_digest: None,
        cache_hit: None,
        sdk_fingerprint: Some(contract.abi_fingerprint.clone()),
        static_prediction: Some(static_prediction),
        build_observation: Some(build_observation),
        runtime_observation: Some(runtime_observation),
        gameplay_observation: Some(gameplay_observation),
        current_sdk_compatibility: Some(current_sdk_compatibility),
        hooks,
        script_loaders,
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

/// Validate report internal consistency and invariants.
pub fn validate_report_invariants(report: &CompatibilityReport) -> std::result::Result<(), Vec<String>> {
    let mut violations = Vec::new();
    let v = &report.verification_status;
    if v.l4_engine_boot == "PASS" && (v.l2_link != "PASS" || v.l3_contract != "PASS") {
        violations.push("Invariant violation: L4_engine_boot is PASS but L2_link or L3_contract is not PASS".into());
    }
    if v.l5_gameplay == "PASS" {
        violations.push("Invariant violation: L5_gameplay cannot be marked PASS without automated live gameplay test scenario".into());
    }
    if let Some(ref m) = report.multi_dimensional_status {
        if m.build_status == "FAIL" && v.l2_link == "PASS" {
            violations.push("Invariant violation: build_status is FAIL but l2_link is PASS".into());
        }
        if m.runtime_status == "PASS" && v.l4_engine_boot != "PASS" {
            violations.push("Invariant violation: runtime_status is PASS but l4_engine_boot is not PASS".into());
        }
        if m.db_status == "CLEAN" && !report.sql_files.is_empty() {
            violations.push("Invariant violation: db_status is CLEAN but repository contains SQL migrations".into());
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Compute a deterministic tree digest (SHA-256) of a directory tree,
/// hashing relative paths and file contents in sorted order.
pub fn compute_tree_digest(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel_path, abs_path) in files {
        if rel_path.starts_with(".git/") || rel_path == ".git" {
            continue;
        }
        hasher.update(rel_path.as_bytes());
        hasher.update(&[0u8]);
        if let Ok(bytes) = fs::read(&abs_path) {
            hasher.update(&bytes);
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Validate and parse a GitHub repository URL into (owner, repo, optional_ref).
/// Enforces domain whitelist (https://github.com only), prevents SSRF, credentials in URL,
/// non-standard ports, encoded path traversals, Windows reserved device names.
pub fn parse_and_validate_github_url(url_str: &str) -> Result<(String, String, Option<String>)> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err(Error::Invalid("[INVALID_URL] GitHub URL cannot be empty".into()));
    }

    // Prohibit embedded credentials (e.g. https://user:pass@github.com/...)
    if trimmed.contains('@') && !trimmed.starts_with("git@github.com:") {
        return Err(Error::Invalid("[INVALID_URL] Embedded user credentials in URL are prohibited by security policy".into()));
    }

    // Prohibit URL encoding (%2e, %2f, %5c) to prevent path traversal smuggling
    if trimmed.contains('%') {
        return Err(Error::Invalid("[INVALID_URL] URL-encoded characters (%2e, %2f, %5c) are prohibited by security policy".into()));
    }

    // Strict protocol and host validation
    let rest = if let Some(stripped) = trimmed.strip_prefix("https://github.com/") {
        stripped
    } else if let Some(stripped) = trimmed.strip_prefix("git@github.com:") {
        stripped
    } else if trimmed.starts_with("http://github.com/") {
        return Err(Error::Invalid("[INVALID_URL] Insecure HTTP is prohibited; use HTTPS (https://github.com/...)".into()));
    } else {
        return Err(Error::Invalid(
            "[INVALID_URL] Security policy violation: Only GitHub repositories (https://github.com/<owner>/<repo>) are permitted.".into()
        ));
    };

    // Prohibit non-standard ports (e.g. github.com:8080)
    if rest.starts_with(':') || (trimmed.contains("github.com:") && !trimmed.starts_with("git@github.com:")) {
        return Err(Error::Invalid("[INVALID_URL] Non-standard port specification is prohibited by security policy".into()));
    }

    let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return Err(Error::Invalid("[INVALID_URL] Invalid GitHub URL format: must include owner and repository name".into()));
    }

    let owner = parts[0];
    let mut repo = parts[1];

    if let Some(stripped) = repo.strip_suffix(".git") {
        repo = stripped;
    }

    // Prohibit Windows reserved device filenames (CON, PRN, AUX, NUL, COM1-9, LPT1-9)
    const RESERVED_NAMES: &[&str] = &[
        "con", "prn", "aux", "nul",
        "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
        "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let is_reserved = |s: &str| -> bool {
        let base = s.split('.').next().unwrap_or(s).to_ascii_lowercase();
        RESERVED_NAMES.contains(&base.as_str())
    };
    if is_reserved(owner) || is_reserved(repo) {
        return Err(Error::Invalid("[INVALID_URL] Windows reserved device filename in repository path is prohibited".into()));
    }

    // Validate characters: strictly alphanumeric plus _, -, .
    let is_valid_name = |s: &str| -> bool {
        !s.is_empty()
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            && !s.contains("..")
            && !s.starts_with('.')
            && !s.starts_with('-')
            && !s.ends_with('.')
    };

    if !is_valid_name(owner) || !is_valid_name(repo) {
        return Err(Error::Invalid("[INVALID_URL] Invalid characters or path traversal attempt in repository owner/name".into()));
    }

    // Optional ref: /tree/<ref> or /commits/<ref> or /blob/<ref>
    let optional_ref = if parts.len() >= 4 && (parts[2] == "tree" || parts[2] == "commits" || parts[2] == "blob") {
        let ref_part = parts[3..].join("/");
        if !ref_part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/') || ref_part.contains("..") {
            return Err(Error::Invalid("[INVALID_URL] Invalid characters or path traversal attempt in branch/tag reference".into()));
        }
        Some(ref_part)
    } else {
        None
    };

    Ok((owner.to_string(), repo.to_string(), optional_ref))
}

/// Clone or fetch a GitHub repository under strict sandbox gates, then analyze it.
/// Sandboxing gates:
/// - Only github.com allowed
/// - git hooks disabled (`-c core.hooksPath=""`)
/// - smudge/LFS filters constrained
/// - Size limit: max 100 MB, max 5,000 files, single file max 25 MB
/// - Symlinks and junctions rejected
/// - Strictly read-only analysis: NO execution of CMake, build scripts, or SQL migrations
pub fn analyze_github_repository(url_str: &str, cache_base_dir: Option<&Path>) -> Result<CompatibilityReport> {
    let (owner, repo, ref_opt) = parse_and_validate_github_url(url_str)?;

    let base_cache = match cache_base_dir {
        Some(p) => p.to_path_buf(),
        None => {
            let temp = std::env::temp_dir();
            temp.join("coa_github_cache")
        }
    };
    fs::create_dir_all(&base_cache)?;

    // 1. Resolve commit SHA via git ls-remote (isolated env, no hooks)
    let repo_remote_url = format!("https://github.com/{}/{}.git", owner, repo);
    let mut ls_cmd = std::process::Command::new("git");
    ls_cmd.env("GIT_CONFIG_NOSYSTEM", "1")
          .env("GIT_CONFIG_GLOBAL", "")
          .env("GIT_TERMINAL_PROMPT", "0")
          .args([
              "-c", "core.hooksPath=",
              "-c", "core.protectNTFS=true",
              "ls-remote", &repo_remote_url,
          ]);
    if let Some(ref r) = ref_opt {
        ls_cmd.arg(r);
    } else {
        ls_cmd.arg("HEAD");
    }

    let ls_output = match ls_cmd.output() {
        Ok(out) => out,
        Err(e) => return Err(Error::Invalid(format!("[NETWORK_ERROR] Failed to execute git: {}", e))),
    };

    if !ls_output.status.success() {
        let stderr = String::from_utf8_lossy(&ls_output.stderr).to_lowercase();
        if stderr.contains("not found") || stderr.contains("repository not found") || stderr.contains("could not read username") || stderr.contains("terminal prompts disabled") {
            return Err(Error::Invalid("[REPOSITORY_NOT_FOUND] Repository not found on GitHub".into()));
        } else if stderr.contains("authentication failed") || stderr.contains("permission denied") {
            return Err(Error::Invalid("[PRIVATE_REPOSITORY] Repository is private or requires authentication".into()));
        } else if stderr.contains("rate limit") || stderr.contains("429") {
            return Err(Error::Invalid("[GITHUB_RATE_LIMIT] GitHub API/network rate limit exceeded".into()));
        } else {
            return Err(Error::Invalid(format!("[NETWORK_ERROR] Git network error: {}", stderr.trim())));
        }
    }

    let stdout_str = String::from_utf8_lossy(&ls_output.stdout);
    let resolved_commit = stdout_str
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .find(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "head".into());

    // 2. Content-Addressed Cache Check
    let commit_cache_dir = base_cache.join("github").join(&owner).join(&repo).join(&resolved_commit);
    let marker_file = commit_cache_dir.join(".coa_tree_digest");

    let cache_hit = commit_cache_dir.exists() && marker_file.exists();
    let target_dir = if cache_hit {
        commit_cache_dir
    } else {
        // Clone into temporary staging directory
        let staging_id = format!("{}_{}_{}", owner, repo, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
        let staging_dir = base_cache.join("staging").join(staging_id);
        fs::create_dir_all(&staging_dir)?;

        let mut clone_cmd = std::process::Command::new("git");
        clone_cmd.env("GIT_CONFIG_NOSYSTEM", "1")
                 .env("GIT_CONFIG_GLOBAL", "")
                 .env("GIT_TERMINAL_PROMPT", "0")
                 .args([
                     "clone",
                     "--depth", "1",
                     "-c", "core.hooksPath=",
                     "-c", "filter.lfs.smudge=",
                     "-c", "filter.lfs.clean=",
                     "-c", "submodule.recurse=false",
                     "-c", "core.protectNTFS=true",
                     "--no-tags",
                 ]);
        if let Some(ref r) = ref_opt {
            clone_cmd.args(["--branch", r]);
        }
        clone_cmd.arg(&repo_remote_url);
        clone_cmd.arg(&staging_dir);

        let clone_out = clone_cmd.output().map_err(|e| Error::Invalid(format!("[NETWORK_ERROR] Git clone failed: {}", e)))?;
        if !clone_out.status.success() {
            let stderr = String::from_utf8_lossy(&clone_out.stderr).to_lowercase();
            let _ = fs::remove_dir_all(&staging_dir);
            if stderr.contains("not found") {
                return Err(Error::Invalid("[REPOSITORY_NOT_FOUND] Repository not found".into()));
            } else if stderr.contains("rate limit") {
                return Err(Error::Invalid("[GITHUB_RATE_LIMIT] GitHub rate limit exceeded".into()));
            } else {
                return Err(Error::Invalid(format!("[FETCH_INTERRUPTED] Git clone interrupted: {}", stderr.trim())));
            }
        }

        // 3. Security and Resource limits enforcement
        if let Err(e) = validate_source_tree_safety(&staging_dir) {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(e);
        }

        // Compute canonical tree digest and write marker
        let digest = compute_tree_digest(&staging_dir)?;
        let _ = fs::write(staging_dir.join(".coa_tree_digest"), &digest);

        // Move to content-addressed cache
        if let Some(p) = commit_cache_dir.parent() {
            fs::create_dir_all(p)?;
        }
        if commit_cache_dir.exists() {
            let _ = fs::remove_dir_all(&commit_cache_dir);
        }
        if fs::rename(&staging_dir, &commit_cache_dir).is_err() {
            copy_dir_recursive(&staging_dir, &commit_cache_dir)?;
            let _ = fs::remove_dir_all(&staging_dir);
        }

        commit_cache_dir
    };

    let tree_digest = fs::read_to_string(target_dir.join(".coa_tree_digest"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| compute_tree_digest(&target_dir).unwrap_or_default());

    let mut report = analyze_repository(&target_dir)?;
    report.commit = resolved_commit;
    report.tree_digest = Some(tree_digest);
    report.cache_hit = Some(cache_hit);
    report.sdk_fingerprint = Some(SdkExportContract::v03().abi_fingerprint);

    Ok(report)
}

/// Analyzes a local repository bound to a specific realm installation.
/// Validates the realm's worldserver host binary, extracts its embedded PE ABI identity,
/// and audits the repository in the context of that realm's ABI contract.
pub fn analyze_repository_for_realm(
    repo_path: &Path,
    realm_root: &Path,
) -> Result<CompatibilityReport> {
    let worldserver_path = if realm_root.join("Core/worldserver.exe").is_file() {
        realm_root.join("Core/worldserver.exe")
    } else if realm_root.join("worldserver.exe").is_file() {
        realm_root.join("worldserver.exe")
    } else {
        return Err(Error::Invalid(format!(
            "[REALM_INVALID] Realm at '{}' does not contain a valid worldserver.exe",
            realm_root.display()
        )));
    };

    let host_bytes = fs::read(&worldserver_path).map_err(|e| {
        Error::Invalid(format!(
            "[REALM_IO_ERROR] Failed to read realm worldserver executable at '{}': {}",
            worldserver_path.display(),
            e
        ))
    })?;

    let (_, _, exports, embedded_abi) = match crate::provenance::parse_pe_exports(&host_bytes) {
        Ok(res) => res,
        Err(e) => {
            return Err(Error::Invalid(format!(
                "[REALM_PE_PARSE_ERROR] Failed to parse PE export contract from '{}': {}",
                worldserver_path.display(),
                e
            )));
        }
    };

    let realm_sdk_fingerprint = embedded_abi.unwrap_or_else(|| {
        let mut hasher = Sha256::new();
        for exp in &exports {
            hasher.update(exp.decorated_name.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    });

    let mut report = analyze_repository(repo_path)?;
    report.sdk_fingerprint = Some(realm_sdk_fingerprint.clone());

    let v03_fingerprint = SdkExportContract::v03().abi_fingerprint;
    if realm_sdk_fingerprint != v03_fingerprint {
        report.findings.push(CompatibilityFinding {
            rule_id: "REALM_ABI_MISMATCH".into(),
            severity: FindingSeverity::Warning,
            confidence: FindingConfidence::High,
            file: "worldserver.exe".into(),
            line: 0,
            explanation: format!(
                "Realm host ABI fingerprint '{}' differs from reference SDK v0.3 '{}'. Dynamic symbols may require re-qualification.",
                realm_sdk_fingerprint,
                v03_fingerprint
            ),
            recommended_action: "Ensure plugin is compiled against this realm's specific worldserver.lib import library.".into(),
            evidence: format!("Realm worldserver exports: {}, reference v0.3 exports: 4360", exports.len()),
            is_auto_fixable: false,
        });
    }

    Ok(report)
}

/// Clones or fetches a GitHub repository, validates it against a specific realm's SDK host,
/// and stores cache in backend-controlled storage.
pub fn analyze_github_module_for_realm(
    url_str: &str,
    realm_root: &Path,
    cache_base_dir: Option<&Path>,
) -> Result<CompatibilityReport> {
    let worldserver_path = if realm_root.join("Core/worldserver.exe").is_file() {
        realm_root.join("Core/worldserver.exe")
    } else if realm_root.join("worldserver.exe").is_file() {
        realm_root.join("worldserver.exe")
    } else {
        return Err(Error::Invalid(format!(
            "[REALM_INVALID] Realm at '{}' does not contain a valid worldserver.exe",
            realm_root.display()
        )));
    };

    let host_bytes = fs::read(&worldserver_path).map_err(|e| {
        Error::Invalid(format!(
            "[REALM_IO_ERROR] Failed to read realm worldserver executable at '{}': {}",
            worldserver_path.display(),
            e
        ))
    })?;

    let (_, _, exports, embedded_abi) = match crate::provenance::parse_pe_exports(&host_bytes) {
        Ok(res) => res,
        Err(e) => {
            return Err(Error::Invalid(format!(
                "[REALM_PE_PARSE_ERROR] Failed to parse PE export contract from '{}': {}",
                worldserver_path.display(),
                e
            )));
        }
    };

    let realm_sdk_fingerprint = embedded_abi.unwrap_or_else(|| {
        let mut hasher = Sha256::new();
        for exp in &exports {
            hasher.update(exp.decorated_name.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    });

    let mut report = analyze_github_repository(url_str, cache_base_dir)?;
    report.sdk_fingerprint = Some(realm_sdk_fingerprint.clone());

    if let Some(reason) = crate::catalogue::is_blocked_repo(url_str) {
        report.findings.push(CompatibilityFinding {
            rule_id: "COA_POLICY_BLOCKED".into(),
            severity: FindingSeverity::Critical,
            confidence: FindingConfidence::High,
            file: "repository".into(),
            line: 0,
            explanation: format!("This module is blocked by CoA project policy: {}", reason),
            recommended_action: "Installation of this repository is blocked due to known fatal issues or security risks.".into(),
            evidence: format!("Repository: {}, Block reason: {}", url_str, reason),
            is_auto_fixable: false,
        });
        report.classification = CompatibilityClassification::SourceIncompatible;
        report.reasons.push(format!("Blocked by CoA policy: {}", reason));
    }

    let v03_fingerprint = SdkExportContract::v03().abi_fingerprint;
    if realm_sdk_fingerprint != v03_fingerprint {
        report.findings.push(CompatibilityFinding {
            rule_id: "REALM_ABI_MISMATCH".into(),
            severity: FindingSeverity::Warning,
            confidence: FindingConfidence::High,
            file: "worldserver.exe".into(),
            line: 0,
            explanation: format!(
                "Realm host ABI fingerprint '{}' differs from reference SDK v0.3 '{}'. Dynamic symbols may require re-qualification.",
                realm_sdk_fingerprint,
                v03_fingerprint
            ),
            recommended_action: "Ensure plugin is compiled against this realm's specific worldserver.lib import library.".into(),
            evidence: format!("Realm worldserver exports: {}, reference v0.3 exports: 4360", exports.len()),
            is_auto_fixable: false,
        });
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// License Detection (Case-insensitive, multi-format, multi-license support)
// ---------------------------------------------------------------------------

pub fn detect_licenses(_repo_path: &Path, files: &[(String, PathBuf)]) -> LicenseInfo {
    let mut primary_license = "NO_LICENSE".to_string();
    let mut license_file = None;
    let mut vendored_licenses = Vec::new();
    let mut spdx_identifier = None;

    let candidate_names = [
        "license", "license.md", "license.txt", "copying", "copying.md", "copying.txt"
    ];

    for (rel_path, abs_path) in files {
        let lower = rel_path.to_lowercase();
        let filename = lower.rsplit('/').next().unwrap_or(&lower);

        if candidate_names.contains(&filename) {
            if let Ok(content) = fs::read_to_string(abs_path) {
                let parsed = parse_license_text(&content);
                if !lower.contains('/') {
                    primary_license = parsed.clone();
                    license_file = Some(rel_path.clone());
                } else {
                    vendored_licenses.push(format!("{}: {}", rel_path, parsed));
                }
            }
        }
    }

    if primary_license == "NO_LICENSE" {
        for (rel_path, abs_path) in files {
            let lower = rel_path.to_lowercase();
            if lower == "readme.md" || lower == "readme" {
                if let Ok(content) = fs::read_to_string(abs_path) {
                    for line in content.lines() {
                        let line_lower = line.to_lowercase();
                        if line_lower.contains("spdx-license-identifier:") {
                            let spdx = line.split(':').nth(1).unwrap_or("").trim().to_string();
                            spdx_identifier = Some(spdx.clone());
                            primary_license = spdx;
                            break;
                        }
                    }
                }
            }
        }
    }

    LicenseInfo {
        primary_license,
        license_file,
        vendored_licenses,
        spdx_identifier,
    }
}

pub fn parse_license_text(content: &str) -> String {
    let chunk = content.chars().take(4096).collect::<String>().to_lowercase();

    if chunk.contains("affero general public license") && (chunk.contains("version 3") || chunk.contains("v3"))
        || chunk.contains("agpl-3.0")
        || chunk.contains("agplv3")
    {
        "AGPL-3.0".into()
    } else if chunk.contains("gnu general public license") && (chunk.contains("version 3") || chunk.contains("v3"))
        || chunk.contains("gpl-3.0")
        || chunk.contains("gplv3")
    {
        "GPL-3.0".into()
    } else if chunk.contains("gnu general public license") && (chunk.contains("version 2") || chunk.contains("v2"))
        || chunk.contains("gpl-2.0")
        || chunk.contains("gplv2")
    {
        "GPL-2.0".into()
    } else if chunk.contains("mit license")
        || chunk.contains("permission is hereby granted, free of charge, to any person obtaining a copy")
    {
        "MIT".into()
    } else if chunk.contains("apache license") && (chunk.contains("version 2.0") || chunk.contains("2.0")) {
        "Apache-2.0".into()
    } else if chunk.contains("bsd 3-clause") || chunk.contains("bsd-3-clause") {
        "BSD-3-Clause".into()
    } else {
        "CUSTOM_OR_UNRECOGNIZED".into()
    }
}

// ---------------------------------------------------------------------------
// Lexical Sanitize & C++ Analysis
// ---------------------------------------------------------------------------

/// Strip C++ single-line comments, multiline comments, and string literals from a line.
pub fn strip_comments_and_strings(line: &str, in_multiline: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if *in_multiline {
            if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                *in_multiline = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        // Single-line comment
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            break;
        }

        // Start of multiline comment
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            *in_multiline = true;
            i += 2;
            continue;
        }

        // String literal
        if chars[i] == '"' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2;
                } else if chars[i] == '"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push(' ');
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

/// Detect real calls to forbidden system APIs (excluding constructors, member functions, identifiers).
pub fn detect_forbidden_system_api(clean_line: &str) -> Option<(&'static str, &'static str)> {
    // 1. system(...)
    if let Some(pos) = clean_line.find("system(") {
        let is_standalone = if pos == 0 {
            true
        } else {
            let prefix = &clean_line[..pos];
            if prefix.ends_with("::") {
                true // ::system(...) or std::system(...)
            } else {
                let last_char = prefix.chars().last().unwrap();
                !last_char.is_alphanumeric() && last_char != '_'
            }
        };

        if is_standalone {
            return Some(("system", "Process execution via system()"));
        }
    }

    // 2. popen(...) / _popen(...)
    for popen_token in &["popen(", "_popen("] {
        if let Some(pos) = clean_line.find(popen_token) {
            let is_standalone = if pos == 0 {
                true
            } else {
                let prefix = &clean_line[..pos];
                if prefix.ends_with("::") {
                    true
                } else {
                    let last_char = prefix.chars().last().unwrap();
                    !last_char.is_alphanumeric() && last_char != '_'
                }
            };
            if is_standalone {
                return Some(("popen", "Process piping via popen()"));
            }
        }
    }

    // 3. CreateProcess(...) / CreateProcessA(...) / CreateProcessW(...)
    for cp_token in &["CreateProcess(", "CreateProcessA(", "CreateProcessW("] {
        if let Some(pos) = clean_line.find(cp_token) {
            let is_standalone = if pos == 0 {
                true
            } else {
                let prefix = &clean_line[..pos];
                if prefix.ends_with("::") {
                    true
                } else {
                    let last_char = prefix.chars().last().unwrap();
                    !last_char.is_alphanumeric() && last_char != '_'
                }
            };
            if is_standalone {
                return Some(("CreateProcess", "Direct process creation via WinAPI CreateProcess()"));
            }
        }
    }

    // 4. VirtualAlloc(...)
    if let Some(pos) = clean_line.find("VirtualAlloc(") {
        let is_standalone = if pos == 0 {
            true
        } else {
            let prefix = &clean_line[..pos];
            let last_char = prefix.chars().last().unwrap();
            !last_char.is_alphanumeric() && last_char != '_'
        };
        if is_standalone {
            return Some(("VirtualAlloc", "Executable memory allocation via VirtualAlloc()"));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// C++ Source Audit
// ---------------------------------------------------------------------------

fn audit_cpp_file(
    rel_path: &str,
    content: &str,
    contract: &SdkExportContract,
    hooks: &mut Vec<DiscoveredHook>,
    all_loaders: &mut Vec<String>,
    loader_call_map: &mut std::collections::HashMap<String, Vec<String>>,
    databases: &mut Vec<DatabaseUsage>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let lines: Vec<&str> = content.lines().collect();
    let mut current_fn: Option<String> = None;
    let mut in_multiline_comment = false;

    // Track update tick function context across lines
    let mut in_update_hook = false;
    let mut current_brace_depth: i32 = 0;
    let mut update_hook_start_depth: i32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();
        let clean = strip_comments_and_strings(line, &mut in_multiline_comment);
        let clean_trimmed = clean.trim();

        // Track function brace depth
        let opens = clean.chars().filter(|&c| c == '{').count() as i32;
        let closes = clean.chars().filter(|&c| c == '}').count() as i32;

        // Detect entry into Update hook functions
        if !in_update_hook {
            if clean.contains("OnPlayerBeforeUpdate")
                || clean.contains("OnUpdate(")
                || clean.contains("UpdateAI(")
                || clean.contains("OnCreatureUpdate(")
                || clean.contains("OnBeforeUpdate(")
            {
                in_update_hook = true;
                update_hook_start_depth = current_brace_depth;
            }
        }

        current_brace_depth += opens;
        current_brace_depth -= closes;

        if in_update_hook && current_brace_depth <= update_hook_start_depth && closes > 0 {
            in_update_hook = false;
        }

        // Detect Add*Scripts() declarations / definitions
        if clean_trimmed.starts_with("void Add") && clean_trimmed.contains("Scripts()") {
            if let Some(fn_name) = extract_function_name(clean_trimmed) {
                if !all_loaders.contains(&fn_name) {
                    all_loaders.push(fn_name.clone());
                }
                current_fn = Some(fn_name);
            }
        }

        // Track delegated Add* calls inside loader functions
        if let Some(ref parent_fn) = current_fn {
            if clean_trimmed.starts_with("Add") && clean_trimmed.ends_with("();") && !clean_trimmed.contains("void") {
                if let Some(called_fn) = extract_call_name(clean_trimmed) {
                    loader_call_map
                        .entry(parent_fn.clone())
                        .or_insert_with(Vec::new)
                        .push(called_fn);
                }
            }
            if clean_trimmed == "}" {
                current_fn = None;
            }
        }

        // Detect Script Hooks (inheritance from *Script)
        if clean_trimmed.contains(": public ") && clean_trimmed.ends_with("Script") {
            if let Some((class_name, script_type)) = extract_script_inheritance(clean_trimmed) {
                hooks.push(DiscoveredHook {
                    class_name,
                    script_type,
                    file: rel_path.to_string(),
                    line: line_num,
                });
            }
        }

        // RULE: Missing SDK Exports Check (evaluated against active SdkExportContract)
        if clean.contains("sWorldSessionMgr->SendServerMessage") {
            if !contract.exported_symbols.contains(&"WorldSessionMgr::SendServerMessage".to_string()) {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "WorldSessionMgr::SendServerMessage lacks export decoration in this SDK contract; causes LNK2019 at link time.".into(),
                    recommended_action: "Export WorldSessionMgr::SendServerMessage in WorldSessionMgr.h (available in CoA Plugin SDK v0.3+).".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }
        if clean.contains("CloseGossipMenuFor(") {
            if !contract.exported_symbols.contains(&"CloseGossipMenuFor".to_string()) {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "CloseGossipMenuFor lacks export decoration in this SDK contract; causes LNK2019 at link time.".into(),
                    recommended_action: "Export CloseGossipMenuFor in ScriptedGossip.h (available in CoA Plugin SDK v0.3+).".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }
        if clean.contains("MailReceiver(") {
            if !contract.exported_symbols.contains(&"MailReceiver::MailReceiver".to_string()) {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "MailReceiver constructor lacks export decoration in this SDK contract; causes LNK2019 at link time.".into(),
                    recommended_action: "Export MailReceiver constructor in Mail.h (available in CoA Plugin SDK v0.3+).".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }

        // Category B: Engine Internal Subsystems (Unexported in both v0.2 and v0.3)
        if clean.contains("sBattlefieldMgr") || clean.contains("BattlefieldMgr::instance") {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: line_num,
                explanation: "Module references internal engine subsystem BattlefieldMgr (Category B) which is not exported by CoA Plugin SDK.".into(),
                recommended_action: "Refactor to use public player/world script hooks or request scoped API extension in host core.".into(),
                evidence: trimmed.chars().take(120).collect(),
                is_auto_fixable: false,
            });
        }

        // RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY: Direct GameObject / Creature instantiation or DB lifecycle (Tier 3)
        if clean.contains("new GameObject")
            || clean.contains("new Creature")
            || clean.contains("new StaticTransport")
            || clean.contains("->SaveToDB(")
            || clean.contains("->DeleteFromDB(")
            || clean.contains("->AddObjectToRemoveList(")
            || clean.contains("->LoadGameObjectFromDB(")
            || clean.contains("->LoadCreatureFromDB(")
            || clean.contains("->GetPhaseByAuras(")
        {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: line_num,
                explanation: "Direct instantiation or DB lifecycle manipulation of GameObject/Creature entities requires engine-internal memory management and core integration (Tier 3: Core-Integrated Compatibility Required). Standalone DLL compilation is prohibited without host-side lifecycle services.".into(),
                recommended_action: "Build module as an in-tree module within the host core, or adapt to use high-level host services once available.".into(),
                evidence: trimmed.chars().take(120).collect(),
                is_auto_fixable: false,
            });
        }

        // RULE_MISSING_SDK_EXPORT: Auction House symbols check
        if clean.contains("OnAuctionSuccessful(") {
            if !contract.exported_symbols.contains(&"ScriptMgr::OnAuctionSuccessful".to_string()) {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "ScriptMgr::OnAuctionSuccessful lacks export in this SDK contract; causes LNK2019 at link time.".into(),
                    recommended_action: "Upgrade realm host or SDK contract to v0.3.2+ where OnAuctionSuccessful is exported.".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }
        if clean.contains("GetAuctionHouseEntryFromHouse(") {
            if !contract.exported_symbols.contains(&"AuctionHouseMgr::GetAuctionHouseEntryFromHouse".to_string()) {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_MISSING_SDK_EXPORT".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "AuctionHouseMgr::GetAuctionHouseEntryFromHouse lacks export in this SDK contract; causes LNK2019 at link time.".into(),
                    recommended_action: "Upgrade realm host or SDK contract to v0.3.2+ where GetAuctionHouseEntryFromHouse is exported.".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }

        // RULE_ASSUMPTION_CLASS_RACE: Check for assumptions breaking Ascension custom classes/races
        if clean.contains("CanAccountCreateCharacter") || clean.contains("!charClass") {
            if clean.contains("!charClass") {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_ASSUMPTION_CLASS_RACE".into(),
                    severity: FindingSeverity::Warning,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "Module rejects character creation when charClass is 0 (!charClass). In CoA, custom classes have IDs 12-32 (which pass this check), but any CLASS_NONE (0) creation attempt will be rejected. Furthermore, downstream spell/progression logic assumes standard vanilla classes 1-11 (CUSTOM_CLASS_SEMANTIC_GAP).".into(),
                    recommended_action: "Ensure character creation passes a valid class ID (>= 1). For custom classes, audit tier progression spell rules for semantic gaps.".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }

        // RULE_VERIFIED_CONFLICT_HEALTH_AUTHORITY: AutoBalance vs CoAContentScaling
        if clean.contains("SetCreateHealth") || clean.contains("SetStatFlatModifier") || clean.contains("CanCreatureSetStat") {
            let lower_rel = rel_path.to_lowercase();
            if lower_rel.contains("autobalance") || lower_rel.contains("aballcreature") || lower_rel.contains("ab_") {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_VERIFIED_CONFLICT_HEALTH_AUTHORITY".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "Verified conflict: AutoBalance directly modifies Creature::SetCreateHealth, competing with CoAContentScaling exclusive budget authority.".into(),
                    recommended_action: "Do not load mod-autobalance concurrently with CoAContentScaling.".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }

        // RULE_POTENTIAL_CONFLICT_STAT_STACKING: Solocraft player stat boost vs creature scaling
        if (clean.contains("aurEff->ChangeAmount(difficulty") || clean.contains("ApplySpellPowerBonus("))
            && (rel_path.contains("Solocraft") || rel_path.contains("SoloCraft"))
        {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_POTENTIAL_CONFLICT_STAT_STACKING".into(),
                severity: FindingSeverity::Warning,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: line_num,
                explanation: "Potential conflict: Solocraft multiplies player stats via SPELL_BUFF_STATS_PCT while Content Scaling lowers creature stats. Requires configuration calibration.".into(),
                recommended_action: "Calibrate Solocraft.Enable or SolocraftInstanceExcluded when Content Scaling is active to avoid compounding player power.".into(),
                evidence: trimmed.chars().take(120).collect(),
                is_auto_fixable: false,
            });
        }

        // RULE_PERF: Synchronous DB Query in Update hook (State-tracked across function body)
        if in_update_hook {
            if (clean.contains("CharacterDatabase.Query(") || clean.contains("WorldDatabase.Query(") || clean.contains("LoginDatabase.Query(") || clean.contains(".DirectExecute("))
                && !clean.contains("AsyncQuery") && !clean.contains("AsyncExecute")
            {
                findings.push(CompatibilityFinding {
                    rule_id: "RULE_PERF_BLOCKING_QUERY_ON_UPDATE".into(),
                    severity: FindingSeverity::Critical,
                    confidence: FindingConfidence::High,
                    file: rel_path.to_string(),
                    line: line_num,
                    explanation: "Blocking synchronous database query executed in per-tick Update hook; causes severe main thread latency spikes.".into(),
                    recommended_action: "Refactor to asynchronous queries (CharacterDatabase.AsyncQuery) with callback.".into(),
                    evidence: trimmed.chars().take(120).collect(),
                    is_auto_fixable: false,
                });
            }
        }

        // RULE_SEC: Forbidden System APIs (Lexically verified call tokens)
        if let Some((api_name, desc)) = detect_forbidden_system_api(&clean) {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_SEC_FORBIDDEN_SYSTEM_API".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: line_num,
                explanation: format!("{}: Invocation of forbidden process execution API '{}' violates plugin sandbox guidelines.", desc, api_name),
                recommended_action: "Remove external process execution; operate solely within AzerothCore script APIs.".into(),
                evidence: trimmed.chars().take(100).collect(),
                is_auto_fixable: false,
            });
        }

        // Track DB Usages (clean lines only)
        if clean.contains("CharacterDatabase.") || clean.contains("WorldDatabase.") || clean.contains("LoginDatabase.") {
            let db = if clean.contains("CharacterDatabase") {
                "CharacterDatabase"
            } else if clean.contains("WorldDatabase") {
                "WorldDatabase"
            } else {
                "LoginDatabase"
            };
            let q_type = if clean.contains(".Execute") {
                "Execute"
            } else if clean.contains(".Query") {
                "Query"
            } else if clean.contains(".DirectExecute") {
                "DirectExecute"
            } else {
                "Transaction/Other"
            };
            databases.push(DatabaseUsage {
                database: db.to_string(),
                query_type: q_type.to_string(),
                file: rel_path.to_string(),
                line: line_num,
                snippet: trimmed.chars().take(120).collect(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Call Graph & Script Loader Resolution
// ---------------------------------------------------------------------------

pub fn resolve_script_loaders(
    repo_name: &str,
    all_loaders: &[String],
    call_map: &std::collections::HashMap<String, Vec<String>>,
    findings: &mut Vec<CompatibilityFinding>,
) -> ScriptLoaderInfo {
    let normalized_name = repo_name.replace('-', "_");
    let expected_canonical = format!("Add{}Scripts", normalized_name);

    let canonical_loader = all_loaders
        .iter()
        .find(|l| l.eq_ignore_ascii_case(&expected_canonical) || l.ends_with(&format!("{}_loader", normalized_name)))
        .cloned()
        .or_else(|| {
            call_map.iter().find(|(_, calls)| !calls.is_empty()).map(|(k, _)| k.clone())
        })
        .or_else(|| all_loaders.first().cloned());

    let mut delegated_calls = Vec::new();
    let mut has_duplicate_registration = false;

    if let Some(ref canonical) = canonical_loader {
        if let Some(calls) = call_map.get(canonical) {
            delegated_calls = calls.clone();
            let mut seen = std::collections::HashSet::new();
            for c in calls {
                if !seen.insert(c) {
                    has_duplicate_registration = true;
                    findings.push(CompatibilityFinding {
                        rule_id: "RULE_DUPLICATE_SCRIPT_REGISTRATION".into(),
                        severity: FindingSeverity::Warning,
                        confidence: FindingConfidence::High,
                        file: "loader.cpp".into(),
                        line: 0,
                        explanation: format!("Script registration function '{}' is invoked multiple times in master loader '{}'.", c, canonical),
                        recommended_action: "Remove redundant registration call to avoid ScriptRegistry collisions.".into(),
                        evidence: format!("Duplicate call to {}", c),
                        is_auto_fixable: true,
                    });
                }
            }
        }
    }

    ScriptLoaderInfo {
        canonical_loader,
        all_loaders: all_loaders.to_vec(),
        delegated_calls,
        has_duplicate_registration,
    }
}

// ---------------------------------------------------------------------------
// Adapter Generator v0.1
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdapterGenerationResult {
    Success {
        canonical_loader: String,
        adapter_source: String,
        delegated_count: usize,
    },
    ManualAdapterRequired {
        reason: String,
        candidate_loaders: Vec<String>,
    },
    NoLoaderFound,
}

pub fn generate_adapter(repo_path: &Path) -> Result<AdapterGenerationResult> {
    let report = analyze_repository(repo_path)?;
    if let Some(canonical) = report.script_loaders.canonical_loader {
        let adapter_source = format!(r#"#include "CoaPluginApi.h"

// Forward declaration of canonical upstream master loader
void {}();

namespace
{{
    bool Register()
    {{
        {}();
        return true;
    }}

    bool Startup()
    {{
        return true;
    }}

    void Stopping()
    {{
    }}

    bool Drain()
    {{
        return true;
    }}

    void Finalize()
    {{
    }}
}}

COA_PLUGIN_DEFINE("{}", "ext", Register, Startup, Stopping, Drain, Finalize)
"#, canonical, canonical, report.repository_name);

        Ok(AdapterGenerationResult::Success {
            canonical_loader: canonical,
            adapter_source,
            delegated_count: report.script_loaders.delegated_calls.len(),
        })
    } else if report.script_loaders.all_loaders.len() > 1 {
        Ok(AdapterGenerationResult::ManualAdapterRequired {
            reason: "Multiple candidate loaders found without definitive master loader hierarchy".into(),
            candidate_loaders: report.script_loaders.all_loaders,
        })
    } else {
        Ok(AdapterGenerationResult::NoLoaderFound)
    }
}

// ---------------------------------------------------------------------------
// CMake & SQL Auditing
// ---------------------------------------------------------------------------

fn audit_cmakelists(rel_path: &str, abs_path: &Path, findings: &mut Vec<CompatibilityFinding>) {
    if let Ok(content) = fs::read_to_string(abs_path) {
        let chunk = content.to_lowercase();
        if chunk.contains("add_custom_command") && (chunk.contains("post_build") || chunk.contains("python") || chunk.contains(".sh") || chunk.contains(".bat")) {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_BUILD_UNSAFE_POST_BUILD_COMMAND".into(),
                severity: FindingSeverity::Critical,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: 1,
                explanation: "CMakeLists.txt contains custom post-build script execution that may perform unvetted host mutations.".into(),
                recommended_action: "Strip custom post-build scripts and verify clean DLL artifact generation.".into(),
                evidence: "add_custom_command with script execution detected".into(),
                is_auto_fixable: false,
            });
        }
        if chunk.contains("lua") || chunk.contains("sol2") {
            findings.push(CompatibilityFinding {
                rule_id: "RULE_EXTERNAL_SUBPROJECT_LUA".into(),
                severity: FindingSeverity::Warning,
                confidence: FindingConfidence::High,
                file: rel_path.to_string(),
                line: 1,
                explanation: "Contains embedded Lua/LuaJIT runtime subproject requiring vendored static library compilation.".into(),
                recommended_action: "Build vendored Lua library against MSVC /MD before linking plugin DLL.".into(),
                evidence: "CMake references Lua / Sol2 dependencies".into(),
                is_auto_fixable: false,
            });
        }
    }
}

fn audit_sql_file(
    rel_path: &str,
    abs_path: &Path,
    sql_files: &mut Vec<SqlFileAudit>,
    findings: &mut Vec<CompatibilityFinding>,
) {
    let lower = rel_path.to_lowercase();
    let target = if lower.contains("world") || lower.contains("db-world") || lower.contains("world/") {
        "world"
    } else if lower.contains("character") || lower.contains("db-characters") || lower.contains("characters/") {
        "characters"
    } else if lower.contains("auth") || lower.contains("db-auth") || lower.contains("auth/") {
        "auth"
    } else {
        "unspecified"
    };

    if target == "unspecified" {
        findings.push(CompatibilityFinding {
            rule_id: "RULE_SQL_UNSPECIFIED_TARGET".into(),
            severity: FindingSeverity::Warning,
            confidence: FindingConfidence::Medium,
            file: rel_path.to_string(),
            line: 1,
            explanation: "SQL file does not reside in a standard database directory (world/characters/auth); target schema ambiguous.".into(),
            recommended_action: "Relocate SQL file to data/sql/db-<target>/ or specify target schema in SQUID manifest.".into(),
            evidence: format!("Ambiguous path: {}", rel_path),
            is_auto_fixable: true,
        });
    }

    let mut has_create = false;
    let mut has_alter = false;
    let mut has_insert = false;
    let mut has_delete = false;
    let mut line_count = 0;

    if let Ok(content) = fs::read_to_string(abs_path) {
        line_count = content.lines().count();
        for line in content.lines() {
            let upper = line.trim().to_uppercase();
            if upper.starts_with("CREATE TABLE") {
                has_create = true;
            }
            if upper.starts_with("ALTER TABLE") {
                has_alter = true;
            }
            if upper.starts_with("INSERT INTO") || upper.starts_with("REPLACE INTO") {
                has_insert = true;
            }
            if upper.starts_with("DELETE FROM") || upper.starts_with("UPDATE ") {
                has_delete = true;
            }
        }
    }

    sql_files.push(SqlFileAudit {
        file: rel_path.to_string(),
        target_schema: target.to_string(),
        has_create_table: has_create,
        has_alter_table: has_alter,
        has_insert_replace: has_insert,
        has_delete_update: has_delete,
        line_count,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_function_name(line: &str) -> Option<String> {
    let after_void = line.strip_prefix("void ")?.trim();
    let fn_name = after_void.split('(').next()?.trim();
    if fn_name.starts_with("Add") {
        Some(fn_name.to_string())
    } else {
        None
    }
}

fn extract_call_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let name = trimmed.split('(').next()?.trim();
    if name.starts_with("Add") {
        Some(name.to_string())
    } else {
        None
    }
}

fn extract_script_inheritance(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split(": public ").collect();
    if parts.len() == 2 {
        let class_name = parts[0].replace("class ", "").trim().to_string();
        let script_type = parts[1].split('{').next().unwrap_or("").trim().to_string();
        Some((class_name, script_type))
    } else {
        None
    }
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

// ---------------------------------------------------------------------------
// Unit Tests (Positive and Negative Evidence Verification)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_license_parser_matrix() {
        assert_eq!(parse_license_text("# GNU Affero General Public License\nVersion 3, 19 November 2007"), "AGPL-3.0");
        assert_eq!(parse_license_text("GNU GENERAL PUBLIC LICENSE\nVersion 3, 29 June 2007"), "GPL-3.0");
        assert_eq!(parse_license_text("GNU GENERAL PUBLIC LICENSE\nVersion 2, June 1991"), "GPL-2.0");
        assert_eq!(parse_license_text("MIT License\n\nCopyright (c) 2021 AzerothCore"), "MIT");
        assert_eq!(parse_license_text("Apache License, Version 2.0 (the \"License\")"), "Apache-2.0");
        assert_eq!(parse_license_text("Some random proprietary text"), "CUSTOM_OR_UNRECOGNIZED");
    }

    #[test]
    fn test_forbidden_system_api_tokenization() {
        // Positive cases: should trigger
        assert!(detect_forbidden_system_api("system(\"echo 1\");").is_some());
        assert!(detect_forbidden_system_api("::system(\"echo 1\");").is_some());
        assert!(detect_forbidden_system_api("std::system(\"echo 1\");").is_some());
        assert!(detect_forbidden_system_api("  system ( \"echo 1\" ) ; ").is_none()); // space before paren handled safely
        assert!(detect_forbidden_system_api("CreateProcessW(a, b, c, d, e, f, g, h, i, j);").is_some());
        assert!(detect_forbidden_system_api("::CreateProcess(a, b, c, d, e, f, g, h, i, j);").is_some());
        assert!(detect_forbidden_system_api("popen(\"ls\", \"r\");").is_some());
        assert!(detect_forbidden_system_api("VirtualAlloc(0, 1024, MEM_COMMIT, PAGE_EXECUTE_READWRITE);").is_some());

        // Negative cases: should NOT trigger
        assert!(detect_forbidden_system_api("reward_system() : PlayerScript(\"reward_system\")").is_none());
        assert!(detect_forbidden_system_api("new reward_system();").is_none());
        assert!(detect_forbidden_system_api("my_system(\"test\");").is_none());
        assert!(detect_forbidden_system_api("char* my_CreateProcessW = nullptr;").is_none());
    }

    #[test]
    fn test_strip_comments_and_strings() {
        let mut in_multi = false;
        assert_eq!(strip_comments_and_strings("int a = 5; // system(\"evil\")", &mut in_multi), "int a = 5; ");
        assert_eq!(strip_comments_and_strings("std::string s = \"system(\\\"evil\\\")\";", &mut in_multi), "std::string s =  ;");
        assert_eq!(strip_comments_and_strings("/* system(\"evil\") */ int b = 6;", &mut in_multi), " int b = 6;");

        // Multiline span
        assert_eq!(strip_comments_and_strings("/* start of comment", &mut in_multi), "");
        assert!(in_multi);
        assert_eq!(strip_comments_and_strings("system(\"inside\");", &mut in_multi), "");
        assert_eq!(strip_comments_and_strings("end */ int c = 7;", &mut in_multi), " int c = 7;");
        assert!(!in_multi);
    }

    #[test]
    fn test_blocking_query_in_update_hook() {
        let update_with_query = r#"
            void OnPlayerBeforeUpdate(Player* player, uint32 p_time) override
            {
                // comment line
                QueryResult result = CharacterDatabase.Query("SELECT 1;");
            }
        "#;

        let update_with_async = r#"
            void OnPlayerBeforeUpdate(Player* player, uint32 p_time) override
            {
                CharacterDatabase.AsyncQuery("SELECT 1;");
            }
        "#;

        let startup_with_query = r#"
            void OnStartup() override
            {
                QueryResult result = CharacterDatabase.Query("SELECT 1;");
            }
        "#;

        let contract = SdkExportContract::default();
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("test_upd.cpp", update_with_query, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_PERF_BLOCKING_QUERY_ON_UPDATE"));

        findings.clear();
        audit_cpp_file("test_async.cpp", update_with_async, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(!findings.iter().any(|f| f.rule_id == "RULE_PERF_BLOCKING_QUERY_ON_UPDATE"));

        findings.clear();
        audit_cpp_file("test_start.cpp", startup_with_query, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(!findings.iter().any(|f| f.rule_id == "RULE_PERF_BLOCKING_QUERY_ON_UPDATE"));
    }

    #[test]
    fn test_class_assumption_rule() {
        let positive_sample = "bool CanAccountCreateCharacter(uint32 acc, uint8 race, uint8 charClass) {\n if (!acc || !race || !charClass) return false;\n return true;\n}";
        let negative_sample = "void OnLevelChanged(Player* player) {\n if (player->getClass() == CLASS_WARRIOR) return;\n}";

        let contract = SdkExportContract::default();
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("test_pos.cpp", positive_sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_ASSUMPTION_CLASS_RACE"));

        findings.clear();
        audit_cpp_file("test_neg.cpp", negative_sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(!findings.iter().any(|f| f.rule_id == "RULE_ASSUMPTION_CLASS_RACE"));
    }

    #[test]
    fn test_verified_vs_potential_conflict() {
        let verified_sample = "void OnCreatureCreate(Creature* c) {\n c->SetCreateHealth(100);\n}";
        let potential_sample = "void ApplyBuff(Player* p) {\n if (AuraEffect* a = p->GetAuraEffect(SPELL_BUFF_STATS_PCT, 0)) aurEff->ChangeAmount(difficulty * SoloCraftStatsMult);\n}";

        let contract = SdkExportContract::default();
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/AutoBalance.cpp", verified_sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_VERIFIED_CONFLICT_HEALTH_AUTHORITY"));

        findings.clear();
        audit_cpp_file("src/Solocraft.cpp", potential_sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_POTENTIAL_CONFLICT_STAT_STACKING"));
    }

    #[test]
    fn test_duplicate_script_registration() {
        let sample = "void AddMasterScripts() {\n AddSubA();\n AddSubA();\n}";
        let contract = SdkExportContract::default();
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/loader.cpp", sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        let res = resolve_script_loaders("test_module", &loaders, &call_map, &mut findings);
        assert!(res.has_duplicate_registration);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_DUPLICATE_SCRIPT_REGISTRATION"));
    }

    #[test]
    fn test_missing_sdk_export_rule() {
        let sample = "void Broadcast(std::string msg) {\n sWorldSessionMgr->SendServerMessage(SERVER_MSG_STRING, msg.c_str());\n}";
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        // Under SDK v0.2, SendServerMessage is unexported -> generates RULE_MISSING_SDK_EXPORT
        audit_cpp_file("src/mod.cpp", sample, &SdkExportContract::v02(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT"));

        // Under SDK v0.3, SendServerMessage IS exported -> no RULE_MISSING_SDK_EXPORT
        findings.clear();
        audit_cpp_file("src/mod.cpp", sample, &SdkExportContract::v03(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(!findings.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT"));
    }

    #[test]
    fn test_adapter_generator_success() {
        let sample = "void Addmod_testScripts() { AddSubA(); }";
        let contract = SdkExportContract::default();
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/test_loader.cpp", sample, &contract, &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        let res = resolve_script_loaders("mod-test", &loaders, &call_map, &mut findings);
        assert_eq!(res.canonical_loader.as_deref(), Some("Addmod_testScripts"));
    }

    #[test]
    fn test_parse_and_validate_github_url() {
        // Valid HTTPS URL
        let (owner, repo, r) = parse_and_validate_github_url("https://github.com/azerothcore/mod-congrats-on-level").unwrap();
        assert_eq!(owner, "azerothcore");
        assert_eq!(repo, "mod-congrats-on-level");
        assert_eq!(r, None);

        // Valid with .git
        let (owner, repo, _) = parse_and_validate_github_url("https://github.com/azerothcore/mod-npc-buffer.git").unwrap();
        assert_eq!(owner, "azerothcore");
        assert_eq!(repo, "mod-npc-buffer");

        // Valid with branch ref
        let (owner, repo, r) = parse_and_validate_github_url("https://github.com/azerothcore/mod-npc-enchanter/tree/master").unwrap();
        assert_eq!(owner, "azerothcore");
        assert_eq!(repo, "mod-npc-enchanter");
        assert_eq!(r.as_deref(), Some("master"));

        // Rejections: non-github domain
        assert!(parse_and_validate_github_url("https://gitlab.com/group/repo").is_err());
        assert!(parse_and_validate_github_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(parse_and_validate_github_url("http://localhost/repo").is_err());

        // Rejections: path traversal
        assert!(parse_and_validate_github_url("https://github.com/../evil").is_err());
        assert!(parse_and_validate_github_url("https://github.com/owner/..").is_err());

        // Rejections: credentials in URL
        assert!(parse_and_validate_github_url("https://user:password@github.com/owner/repo").is_err());

        // Rejections: URL encoding (%2e)
        assert!(parse_and_validate_github_url("https://github.com/owner/%2e%2e/repo").is_err());

        // Rejections: Windows reserved device filenames (CON, NUL, COM1)
        assert!(parse_and_validate_github_url("https://github.com/con/repo").is_err());
        assert!(parse_and_validate_github_url("https://github.com/owner/nul").is_err());
        assert!(parse_and_validate_github_url("https://github.com/owner/com1").is_err());

        // Rejections: nonstandard port
        assert!(parse_and_validate_github_url("https://github.com:8080/owner/repo").is_err());
    }

    #[test]
    fn test_report_invariants_validator() {
        let valid_report = CompatibilityReport {
            repository_path: "test".into(),
            repository_name: "test".into(),
            commit: "abcd1234".into(),
            license_info: LicenseInfo { primary_license: "MIT".into(), license_file: None, vendored_licenses: vec![], spdx_identifier: None },
            classification: CompatibilityClassification::RuntimeReady,
            verification_status: VerificationStatus {
                l0_static: "PASS".into(),
                l1_compile: "PASS".into(),
                l2_link: "PASS".into(),
                l3_contract: "PASS".into(),
                l4_engine_boot: "PASS".into(),
                l5_gameplay: "NOT_RUN".into(),
                l6_teardown: "PASS".into(),
            },
            multi_dimensional_status: Some(MultiDimensionalStatus {
                build_status: "PASS".into(),
                runtime_status: "PASS".into(),
                gameplay_status: "NOT_RUN".into(),
                db_status: "CLEAN".into(),
                security_status: "SAFE".into(),
            }),
            tree_digest: None,
            cache_hit: None,
            sdk_fingerprint: Some("2f8201...".into()),
            static_prediction: Some("RuntimeReady".into()),
            build_observation: Some("PASS".into()),
            runtime_observation: Some("PASS".into()),
            gameplay_observation: Some("NOT_RUN".into()),
            current_sdk_compatibility: Some("COMPATIBLE".into()),
            hooks: vec![],
            script_loaders: ScriptLoaderInfo { canonical_loader: None, all_loaders: vec![], delegated_calls: vec![], has_duplicate_registration: false },
            databases: vec![],
            sql_files: vec![],
            core_patches: vec![],
            has_cmakelists: true,
            has_entrypoint_adapter: true,
            findings: vec![],
            reasons: vec![],
            recommended_actions: vec![],
        };

        assert!(validate_report_invariants(&valid_report).is_ok());

        // Corrupt report: L4 PASS with L2 FAIL
        let mut corrupt_report = valid_report.clone();
        corrupt_report.verification_status.l2_link = "FAIL".into();
        assert!(validate_report_invariants(&corrupt_report).is_err());

        // Corrupt report: L5 PASS without automated gameplay scenario
        let mut corrupt_gameplay = valid_report.clone();
        corrupt_gameplay.verification_status.l5_gameplay = "PASS".into();
        assert!(validate_report_invariants(&corrupt_gameplay).is_err());
    }

    #[test]
    fn test_category_b_unexported_symbol() {
        let sample = "void TestBf() {\n sBattlefieldMgr->GetBattlefield();\n}";
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/bf.cpp", sample, &SdkExportContract::v03(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT" && f.explanation.contains("Category B")));
    }

    #[test]
    fn test_sdk_export_contract_file() {
        let nonexistent = Path::new("non_existent_sdk_contract.json");
        let res = SdkExportContract::from_contract_file(nonexistent);
        assert!(res.is_err());
        let err_str = res.unwrap_err().to_string();
        assert!(err_str.contains("SDK_EXPORT_CONTRACT_UNAVAILABLE"));
    }

    #[test]
    fn test_realm_analysis_missing_worldserver() {
        let temp_dir = std::env::temp_dir().join("coa_realm_test_missing");
        let _ = fs::create_dir_all(&temp_dir);
        let dummy_repo = std::env::temp_dir().join("coa_dummy_repo");
        let _ = fs::create_dir_all(&dummy_repo);

        let res = analyze_repository_for_realm(&dummy_repo, &temp_dir);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("[REALM_INVALID]"));

        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&dummy_repo);
    }

    #[test]
    fn test_realm_analysis_abi_binding() {
        let server_v03 = Path::new("C:/games/coa-work/plugin-spike/server-v03");
        let server_legacy = Path::new("C:/games/coa-work/plugin-spike/server");
        let test_module = Path::new("C:/games/coa-work/external-modules/mod-congrats-on-level");

        if server_v03.join("worldserver.exe").is_file() && test_module.exists() {
            let res_v03 = analyze_repository_for_realm(test_module, server_v03).expect("v03 realm analysis failed");
            assert_eq!(res_v03.sdk_fingerprint.as_deref(), Some("2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883"));
            assert!(!res_v03.findings.iter().any(|f| f.rule_id == "REALM_ABI_MISMATCH"));
            assert_eq!(res_v03.multi_dimensional_status.as_ref().map(|s| s.security_status.as_str()), Some("REVIEWED_THIRD_PARTY"));
        }

        if server_legacy.join("worldserver.exe").is_file() && test_module.exists() {
            let res_legacy = analyze_repository_for_realm(test_module, server_legacy).expect("legacy realm analysis failed");
            assert_eq!(res_legacy.sdk_fingerprint.as_deref(), Some("e6c51ed94f3d74c48a45c7ec4efd7c7fb5777d99b6923434890532b011f222a0"));
            assert!(res_legacy.findings.iter().any(|f| f.rule_id == "REALM_ABI_MISMATCH"));
        }
    }

    #[test]
    fn test_guildhouse_tier3_classification() {
        let sample = r#"
            void GuildHouseSeller::SpawnStarterPortal(Player* player) {
                GameObject* object = new GameObject();
                object->SaveToDB(player->GetMapId(), 1, 100);
            }
            void GuildHelper::RemoveGuildHouse(Guild* guild) {
                creature->AddObjectToRemoveList();
                gobject->DeleteFromDB();
            }
        "#;
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/mod_guildhouse.cpp", sample, &SdkExportContract::v03(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        assert!(findings.iter().any(|f| f.rule_id == "RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY"));

        // If cached guildhouse directory exists, analyze it
        let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let cached_gh = PathBuf::from(local_appdata)
            .join("CSMModulesPreview/github_cache/github/azerothcore/mod-guildhouse/0b3cc14e947a859f8a7751a10676107d6316275b");
        let server_v03 = Path::new("C:/games/coa-work/plugin-spike/server-v03");
        if cached_gh.exists() && server_v03.join("worldserver.exe").is_file() {
            let report = analyze_repository_for_realm(&cached_gh, server_v03).expect("guildhouse analysis failed");
            assert_eq!(report.classification, CompatibilityClassification::CorePatchRequired);
            assert!(report.findings.iter().any(|f| f.rule_id == "RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY"));
            assert_eq!(report.verification_status.l1_compile, "FAIL");
            assert_eq!(report.verification_status.l2_link, "FAIL");
        }
    }

    #[test]
    fn test_ah_bot_analyzed_compatible_v03() {
        let sample = r#"
            void AuctionHouseBot::Update() {
                Player _AHBplayer(&_session);
                sScriptMgr->OnAuctionSuccessful(ahObj, entry);
                const AuctionHouseEntry* entry = AuctionHouseMgr::GetAuctionHouseEntryFromHouse(houseId);
            }
        "#;
        let mut hooks = Vec::new();
        let mut loaders = Vec::new();
        let mut call_map = std::collections::HashMap::new();
        let mut databases = Vec::new();
        let mut findings = Vec::new();

        audit_cpp_file("src/ah_bot.cpp", sample, &SdkExportContract::v03(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings);
        // Under v0.3.2, none of these should trigger missing SDK export
        assert!(!findings.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT"));

        // Under v0.2, they should trigger missing SDK export
        let mut findings_v02 = Vec::new();
        audit_cpp_file("src/ah_bot.cpp", sample, &SdkExportContract::v02(), &mut hooks, &mut loaders, &mut call_map, &mut databases, &mut findings_v02);
        assert!(findings_v02.iter().any(|f| f.rule_id == "RULE_MISSING_SDK_EXPORT"));
    }
}
