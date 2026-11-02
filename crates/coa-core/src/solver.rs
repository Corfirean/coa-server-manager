//! CoA Dynamic Plugin Compatibility Solver & Update Planner.
//!
//! Evaluates compatibility between server hosts (modular AzerothCore), dynamic DLL plugins,
//! database schema requirements, and client patch requirements.
//!
//! Core Rule: "Unknown compatibility must never silently become compatible."

use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostManifest {
    pub version: String,
    pub commit: String,
    pub abi_fingerprint: String,
    pub sdk_contract_version: String,
    pub schema_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleArtifactInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub required_abi_fingerprint: Option<String>,
    pub required_sdk_contract: Option<String>,
    pub min_host_version: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    pub required_schema_version: Option<String>,
    #[serde(default)]
    pub has_source: bool,
    #[serde(default)]
    pub is_dynamic_dll: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", content = "details")]
pub enum SolverVerdict {
    CompatibleAsIs,
    RequiresModuleRebuild { modules: Vec<String> },
    RequiresModuleUpdate { updates: Vec<(String, String)> },
    RequiresAdditionalDependency { missing_dependencies: Vec<String> },
    RequiresDisablingConflictingModule { conflicting_modules: Vec<String> },
    Unsupported { reason: String },
    Unknown { reason: String },
}

pub struct CompatibilityGraph<'a> {
    pub installed_host: &'a HostManifest,
    pub candidate_host: &'a HostManifest,
    pub installed_modules: &'a [ModuleArtifactInfo],
    pub available_modules: &'a [ModuleArtifactInfo],
    pub current_schema_version: &'a str,
}

/// Solves compatibility for updating the host and/or plugins.
pub fn solve_compatibility(graph: &CompatibilityGraph) -> SolverVerdict {
    let mut updates: Vec<(String, String)> = Vec::new();
    let mut rebuilds: Vec<String> = Vec::new();
    let mut missing_deps: BTreeSet<String> = BTreeSet::new();
    let mut conflicting_mods: BTreeSet<String> = BTreeSet::new();

    let installed_ids: BTreeSet<String> = graph
        .installed_modules
        .iter()
        .map(|m| m.id.clone())
        .collect();

    // 1. Schema compatibility check
    if graph.candidate_host.schema_version.as_str() > graph.current_schema_version {
        // Database migration is required; check if supported
        if graph.candidate_host.schema_version.is_empty() {
            return SolverVerdict::Unknown {
                reason: "Candidate host schema version metadata is missing or corrupted.".into(),
            };
        }
    }

    // 2. Evaluate each installed module against candidate host
    for m in graph.installed_modules {
        // Unknown compatibility check
        if m.is_dynamic_dll && m.required_abi_fingerprint.is_none() && m.required_sdk_contract.is_none() {
            return SolverVerdict::Unknown {
                reason: format!(
                    "Module '{}' has no declared ABI contract or fingerprint. Unverified binaries cannot be safely loaded.",
                    m.id
                ),
            };
        }

        // Mutual exclusion / Conflicts
        for conflict in &m.conflicts {
            if installed_ids.contains(conflict) {
                conflicting_mods.insert(conflict.clone());
                conflicting_mods.insert(m.id.clone());
            }
        }
        // Built-in mutual exclusion: companions vs playerbots
        if m.id == "companions" && installed_ids.contains("playerbots") {
            conflicting_mods.insert("companions".into());
            conflicting_mods.insert("playerbots".into());
        }

        // Dependencies
        for dep in &m.dependencies {
            if !installed_ids.contains(dep) {
                missing_deps.insert(dep.clone());
            }
        }

        // ABI Fingerprint compatibility
        if m.is_dynamic_dll {
            if let Some(ref req_abi) = m.required_abi_fingerprint {
                if req_abi != &graph.candidate_host.abi_fingerprint {
                    // ABI mismatch! Look for replacement in available modules
                    let replacement = graph.available_modules.iter().find(|avail| {
                        avail.id == m.id
                            && avail
                                .required_abi_fingerprint
                                .as_ref()
                                .map(|a| a == &graph.candidate_host.abi_fingerprint)
                                .unwrap_or(false)
                    });

                    if let Some(rep) = replacement {
                        updates.push((m.id.clone(), rep.version.clone()));
                    } else if m.has_source {
                        rebuilds.push(m.id.clone());
                    } else {
                        return SolverVerdict::Unsupported {
                            reason: format!(
                                "Module '{}' (v{}) requires ABI fingerprint {} which does not match candidate host ABI {}. No compatible update or source is available.",
                                m.id, m.version, req_abi, graph.candidate_host.abi_fingerprint
                            ),
                        };
                    }
                }
            }
        }
    }

    if !conflicting_mods.is_empty() {
        return SolverVerdict::RequiresDisablingConflictingModule {
            conflicting_modules: conflicting_mods.into_iter().collect(),
        };
    }

    if !missing_deps.is_empty() {
        return SolverVerdict::RequiresAdditionalDependency {
            missing_dependencies: missing_deps.into_iter().collect(),
        };
    }

    if !updates.is_empty() {
        return SolverVerdict::RequiresModuleUpdate { updates };
    }

    if !rebuilds.is_empty() {
        return SolverVerdict::RequiresModuleRebuild { modules: rebuilds };
    }

    SolverVerdict::CompatibleAsIs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_old_host() -> HostManifest {
        HostManifest {
            version: "0.7.0".into(),
            commit: "1111111111111111111111111111111111111111".into(),
            abi_fingerprint: "abi_old_fingerprint_01".into(),
            sdk_contract_version: "0.3.1".into(),
            schema_version: "2026_10_20_00".into(),
        }
    }

    fn sample_new_host() -> HostManifest {
        HostManifest {
            version: "0.7.1".into(),
            commit: "2222222222222222222222222222222222222222".into(),
            abi_fingerprint: "abi_new_fingerprint_02".into(),
            sdk_contract_version: "0.3.2".into(),
            schema_version: "2026_10_28_00".into(),
        }
    }

    #[test]
    fn test_scenario_old_host_old_plugin() {
        let host = sample_old_host();
        let plugin = ModuleArtifactInfo {
            id: "content-scaling".into(),
            name: "Content Scaling".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: Some("abi_old_fingerprint_01".into()),
            required_sdk_contract: Some("0.3.1".into()),
            min_host_version: Some("0.7.0".into()),
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &host,
            candidate_host: &host,
            installed_modules: &[plugin],
            available_modules: &[],
            current_schema_version: "2026_10_20_00",
        };

        assert_eq!(solve_compatibility(&graph), SolverVerdict::CompatibleAsIs);
    }

    #[test]
    fn test_scenario_old_host_incompatible_plugin() {
        let host = sample_old_host();
        let plugin = ModuleArtifactInfo {
            id: "custom-plugin".into(),
            name: "Custom Plugin".into(),
            version: "2.0.0".into(),
            required_abi_fingerprint: Some("abi_future_fingerprint_99".into()),
            required_sdk_contract: Some("0.4.0".into()),
            min_host_version: Some("0.8.0".into()),
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &host,
            candidate_host: &host,
            installed_modules: &[plugin],
            available_modules: &[],
            current_schema_version: "2026_10_20_00",
        };

        match solve_compatibility(&graph) {
            SolverVerdict::Unsupported { reason } => {
                assert!(reason.contains("does not match candidate host ABI"));
            }
            other => panic!("Expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_scenario_new_host_incompatible_old_plugin_with_replacement() {
        let old_host = sample_old_host();
        let new_host = sample_new_host();
        let old_plugin = ModuleArtifactInfo {
            id: "companions".into(),
            name: "CoA Companions".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: Some("abi_old_fingerprint_01".into()),
            required_sdk_contract: Some("0.3.1".into()),
            min_host_version: Some("0.7.0".into()),
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let replacement_plugin = ModuleArtifactInfo {
            id: "companions".into(),
            name: "CoA Companions".into(),
            version: "1.1.0".into(),
            required_abi_fingerprint: Some("abi_new_fingerprint_02".into()),
            required_sdk_contract: Some("0.3.2".into()),
            min_host_version: Some("0.7.1".into()),
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &old_host,
            candidate_host: &new_host,
            installed_modules: &[old_plugin],
            available_modules: &[replacement_plugin],
            current_schema_version: "2026_10_28_00",
        };

        assert_eq!(
            solve_compatibility(&graph),
            SolverVerdict::RequiresModuleUpdate {
                updates: vec![("companions".into(), "1.1.0".into())],
            }
        );
    }

    #[test]
    fn test_scenario_new_host_incompatible_old_plugin_with_rebuild() {
        let old_host = sample_old_host();
        let new_host = sample_new_host();
        let old_plugin = ModuleArtifactInfo {
            id: "npc-buffer".into(),
            name: "NPC Buffer".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: Some("abi_old_fingerprint_01".into()),
            required_sdk_contract: Some("0.3.1".into()),
            min_host_version: Some("0.7.0".into()),
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: true, // source available in workspace!
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &old_host,
            candidate_host: &new_host,
            installed_modules: &[old_plugin],
            available_modules: &[],
            current_schema_version: "2026_10_28_00",
        };

        assert_eq!(
            solve_compatibility(&graph),
            SolverVerdict::RequiresModuleRebuild {
                modules: vec!["npc-buffer".into()],
            }
        );
    }

    #[test]
    fn test_scenario_unknown_compatibility_never_silently_compatible() {
        let host = sample_old_host();
        let unknown_plugin = ModuleArtifactInfo {
            id: "mystery-dll".into(),
            name: "Mystery DLL".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: None, // No ABI metadata!
            required_sdk_contract: None,
            min_host_version: None,
            dependencies: vec![],
            conflicts: vec![],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &host,
            candidate_host: &host,
            installed_modules: &[unknown_plugin],
            available_modules: &[],
            current_schema_version: "2026_10_20_00",
        };

        match solve_compatibility(&graph) {
            SolverVerdict::Unknown { reason } => {
                assert!(reason.contains("Unverified binaries cannot be safely loaded"));
            }
            other => panic!("Expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_scenario_conflicting_modules_detected() {
        let host = sample_old_host();
        let companions = ModuleArtifactInfo {
            id: "companions".into(),
            name: "CoA Companions".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: Some("abi_old_fingerprint_01".into()),
            required_sdk_contract: Some("0.3.1".into()),
            min_host_version: Some("0.7.0".into()),
            dependencies: vec![],
            conflicts: vec!["playerbots".into()],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let playerbots = ModuleArtifactInfo {
            id: "playerbots".into(),
            name: "SQUID Playerbots".into(),
            version: "1.0.0".into(),
            required_abi_fingerprint: Some("abi_old_fingerprint_01".into()),
            required_sdk_contract: Some("0.3.1".into()),
            min_host_version: Some("0.7.0".into()),
            dependencies: vec![],
            conflicts: vec!["companions".into()],
            required_schema_version: None,
            has_source: false,
            is_dynamic_dll: true,
        };

        let graph = CompatibilityGraph {
            installed_host: &host,
            candidate_host: &host,
            installed_modules: &[companions, playerbots],
            available_modules: &[],
            current_schema_version: "2026_10_20_00",
        };

        match solve_compatibility(&graph) {
            SolverVerdict::RequiresDisablingConflictingModule { conflicting_modules } => {
                assert!(conflicting_modules.contains(&"companions".to_string()));
                assert!(conflicting_modules.contains(&"playerbots".to_string()));
            }
            other => panic!("Expected RequiresDisablingConflictingModule, got {:?}", other),
        }
    }
}
