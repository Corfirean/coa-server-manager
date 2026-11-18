//! CLI tool for CoA Server Manager plugin operations (testing, qualification and automation).
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use coa_core::plugins::{self, PluginMetadata};

fn print_usage() {
    eprintln!(
        r#"csm_plugins CLI
Usage:
    csm_plugins install <server> <id> <name> <version> <dll> [--pdb <pdb>] [--conf <dist_confs>] [--deps <deps>] [--enable] [--crash-at <point>]
    csm_plugins update <server> <id> <name> <version> <dll> [--pdb <pdb>] [--conf <dist_confs>] [--crash-at <point>]
    csm_plugins enable <server> <id> [--crash-at <point>]
    csm_plugins disable <server> <id> [--crash-at <point>]
    csm_plugins remove <server> <id> [--crash-at <point>]
    csm_plugins repair <server> [--crash-at <point>]
    csm_plugins recover <server>
    csm_plugins prune <server>
    csm_plugins list <server>
    csm_plugins reconcile_squid <server> [char_schema]
    csm_plugins multiworld <server> <second_root> [--allow <a,b>] [--crash-at <point>]
    csm_plugins analyze <local_repository_path> [--json]
    csm_plugins analyze-github <url> [--cache-dir <dir>] [--contract <v02|v03>] [--json]
    csm_plugins verify-artifact-provenance <worldserver_path> [--sdk-json <path>] [--json]
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        print_usage();
        std::process::exit(1);
    }

    // Inspect for --crash-at argument
    for i in 0..args.len() {
        if args[i] == "--crash-at" && i + 1 < args.len() {
            std::env::set_var("CSM_CRASH_AT", &args[i + 1]);
        }
    }

    let cmd = &args[1];
    let server = Path::new(&args[2]);

    match cmd.as_str() {
        "install" => {
            if args.len() < 7 {
                eprintln!("Usage: install <server> <id> <name> <version> <dll> [--pdb <pdb>] [--conf <dist_confs>] [--deps <deps>] [--enable]");
                std::process::exit(1);
            }
            let id = &args[3];
            let name = &args[4];
            let version = &args[5];
            let dll_path = Path::new(&args[6]);
            let dll_bytes = fs::read(dll_path)?;
            let digest = plugins::sha256_digest(&dll_bytes);

            let mut pdb_bytes = None;
            let mut conf_list = Vec::new();
            let mut deps = Vec::new();
            let mut enable = false;

            let mut i = 7;
            while i < args.len() {
                match args[i].as_str() {
                    "--pdb" => {
                        i += 1;
                        if i < args.len() {
                            pdb_bytes = Some(fs::read(&args[i])?);
                        }
                    }
                    "--conf" => {
                        i += 1;
                        if i < args.len() {
                            for item in args[i].split(',') {
                                if !item.is_empty() {
                                    let p = Path::new(item);
                                    let file_name = p.file_name().unwrap().to_string_lossy().into_owned();
                                    let bytes = fs::read(p)?;
                                    conf_list.push((file_name, bytes));
                                }
                            }
                        }
                    }
                    "--deps" => {
                        i += 1;
                        if i < args.len() {
                            deps = args[i].split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                        }
                    }
                    "--enable" => {
                        enable = true;
                    }
                    _ => {}
                }
                i += 1;
            }

            let conf_names: Vec<String> = conf_list.iter().map(|(n, _)| n.clone()).collect();
            let dist_refs: Vec<(String, &[u8])> = conf_list.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();

            let meta = PluginMetadata {
                id: id.clone(),
                name: name.clone(),
                version: version.clone(),
                description: BTreeMap::new(),
                sha256: digest,
                required: true,
                deps,
                conf_files: conf_names,
                enable_key: None,
                status: Some("release".into()),
                icon: None,
            };

            plugins::install(server, meta, &dll_bytes, pdb_bytes.as_deref(), &dist_refs, enable)?;
            println!("Installed plugin '{}' successfully (enabled: {})", id, enable);
        }
        "update" => {
            if args.len() < 7 {
                eprintln!("Usage: update <server> <id> <name> <version> <dll> [--pdb <pdb>] [--conf <dist_confs>]");
                std::process::exit(1);
            }
            let id = &args[3];
            let name = &args[4];
            let version = &args[5];
            let dll_path = Path::new(&args[6]);
            let dll_bytes = fs::read(dll_path)?;
            let digest = plugins::sha256_digest(&dll_bytes);

            let mut pdb_bytes = None;
            let mut conf_list = Vec::new();

            let mut i = 7;
            while i < args.len() {
                match args[i].as_str() {
                    "--pdb" => {
                        i += 1;
                        if i < args.len() {
                            pdb_bytes = Some(fs::read(&args[i])?);
                        }
                    }
                    "--conf" => {
                        i += 1;
                        if i < args.len() {
                            for item in args[i].split(',') {
                                if !item.is_empty() {
                                    let p = Path::new(item);
                                    let file_name = p.file_name().unwrap().to_string_lossy().into_owned();
                                    let bytes = fs::read(p)?;
                                    conf_list.push((file_name, bytes));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                i += 1;
            }

            let conf_names: Vec<String> = conf_list.iter().map(|(n, _)| n.clone()).collect();
            let dist_refs: Vec<(String, &[u8])> = conf_list.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();

            let meta = PluginMetadata {
                id: id.clone(),
                name: name.clone(),
                version: version.clone(),
                description: BTreeMap::new(),
                sha256: digest,
                required: true,
                deps: vec![],
                conf_files: conf_names,
                enable_key: None,
                status: Some("release".into()),
                icon: None,
            };

            plugins::update(server, meta, &dll_bytes, pdb_bytes.as_deref(), &dist_refs)?;
            println!("Updated plugin '{}' to version {} successfully", id, version);
        }
        "enable" => {
            if args.len() < 4 {
                eprintln!("Usage: enable <server> <id>");
                std::process::exit(1);
            }
            let id = &args[3];
            plugins::set_enabled(server, id, true)?;
            println!("Enabled plugin '{}' successfully", id);
        }
        "disable" => {
            if args.len() < 4 {
                eprintln!("Usage: disable <server> <id>");
                std::process::exit(1);
            }
            let id = &args[3];
            plugins::set_enabled(server, id, false)?;
            println!("Disabled plugin '{}' successfully", id);
        }
        "remove" => {
            if args.len() < 4 {
                eprintln!("Usage: remove <server> <id>");
                std::process::exit(1);
            }
            let id = &args[3];
            plugins::remove(server, id)?;
            println!("Removed plugin '{}' successfully", id);
        }
        "repair" => {
            let restored = plugins::repair(server)?;
            println!("Repair complete. Restored {} plugin(s): {:?}", restored.len(), restored);
        }
        "recover" => {
            plugins::ensure_durable_recovered(server)?;
            println!("Durable transaction recovery complete (clean state restored).");
        }
        "prune" => {
            let pruned = plugins::prune_store(server)?;
            println!("Pruned {} unused store version(s): {:?}", pruned.len(), pruned);
        }
        "reconcile_squid" => {
            let char_schema = if args.len() >= 4 { &args[3] } else { "acore_characters" };
            let port_str = std::env::var("MYSQL_PORT").unwrap_or_else(|_| "3417".into());
            let user = std::env::var("MYSQL_USER").unwrap_or_else(|_| "root".into());
            let pass = std::env::var("MYSQL_PWD").unwrap_or_default();
            let mysql_exe = std::env::var("MYSQL_EXE").unwrap_or_else(|_| "mysql".into());

            let query_fn = |sql: &str| -> Result<String, coa_core::error::Error> {
                let mut cmd = std::process::Command::new(&mysql_exe);
                cmd.args(["-h", "127.0.0.1", "-P", &port_str, "-u", &user, "-e", sql]);
                if !pass.is_empty() && pass != "trap" {
                    cmd.env("MYSQL_PWD", &pass);
                } else {
                    cmd.env_remove("MYSQL_PWD");
                }
                let out = cmd.output().map_err(coa_core::error::Error::Io)?;
                if !out.status.success() {
                    return Err(coa_core::error::Error::Invalid(String::from_utf8_lossy(&out.stderr).into_owned()));
                }
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            };
            let audit = plugins::reconcile_squid_migrations(server, char_schema, Some(&query_fn))?;
            println!("SQUID Reconciliation: status={:?}, applied={}", audit.db_status, audit.effective_applied);
        }
        "list" => {
            let reg = plugins::load_registry(server)?;
            println!("Plugins schema {}:", reg.schema);
            for (id, p) in reg.plugins {
                println!(
                    " - {}: enabled={}, version={}, sha256={}",
                    id, p.enabled, p.version, p.sha256
                );
            }
        }
        "multiworld" => {
            if args.len() < 4 {
                eprintln!("Usage: multiworld <server> <second_root> [--allow <a,b>]");
                std::process::exit(1);
            }
            let second_root = Path::new(&args[3]);
            let mut allowlist = None;
            if args.len() >= 6 && args[4] == "--allow" {
                allowlist = Some(args[5].split(',').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>());
            }
            plugins::prepare_multiworld_realm(server, second_root, allowlist.as_deref())?;
            println!("Prepared multiworld realm at {} successfully", second_root.display());
        }
        "analyze" => {
            let repo_path = Path::new(&args[2]);
            let as_json = args.iter().any(|a| a == "--json");
            let contract = if args.iter().any(|a| a == "--sdk-v02") {
                coa_core::analyzer::SdkExportContract::v02()
            } else {
                coa_core::analyzer::SdkExportContract::v03()
            };
            let report = coa_core::analyzer::analyze_repository_with_contract(repo_path, &contract)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("=== Module Compatibility Report v0.2 ===");
                println!("Repository: {}", report.repository_name);
                println!("Commit    : {}", report.commit);
                println!("License   : {}", report.license_info.primary_license);
                if let Some(ref lf) = report.license_info.license_file {
                    println!("License File: {}", lf);
                }
                if !report.license_info.vendored_licenses.is_empty() {
                    println!("Vendored Licenses: {:?}", report.license_info.vendored_licenses);
                }
                println!("Verdict   : {:?}", report.classification);
                println!("Status L0-L6: static={}, compile={}, link={}, contract={}, boot={}, gameplay={}, teardown={}",
                    report.verification_status.l0_static,
                    report.verification_status.l1_compile,
                    report.verification_status.l2_link,
                    report.verification_status.l3_contract,
                    report.verification_status.l4_engine_boot,
                    report.verification_status.l5_gameplay,
                    report.verification_status.l6_teardown);
                println!("Hooks ({}):", report.hooks.len());
                for h in &report.hooks {
                    println!("  - {} : {} ({}:{})", h.class_name, h.script_type, h.file, h.line);
                }
                println!("SQL Files ({}):", report.sql_files.len());
                for s in &report.sql_files {
                    println!("  - {} [schema: {}] (create={}, alter={}, lines={})", s.file, s.target_schema, s.has_create_table, s.has_alter_table, s.line_count);
                }
                println!("Findings ({}):", report.findings.len());
                for f in &report.findings {
                    println!("  [{:?}] {} (at {}:{}): {}", f.severity, f.rule_id, f.file, f.line, f.explanation);
                    println!("     Guidance: {}", f.recommended_action);
                }
                println!("Summary Reasons: {:?}", report.reasons);
            }
        }
        "analyze-github" => {
            let url = &args[2];
            let mut cache_dir = None;
            let mut json_out = false;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--cache-dir" => {
                        i += 1;
                        if i < args.len() {
                            cache_dir = Some(std::path::PathBuf::from(&args[i]));
                        }
                    }
                    "--json" => {
                        json_out = true;
                    }
                    _ => {}
                }
                i += 1;
            }

            match coa_core::analyzer::analyze_github_repository(url, cache_dir.as_deref()) {
                Ok(report) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        println!("Repository: {}", report.repository_name);
                        println!("Classification: {:?}", report.classification);
                        println!("License: {}", report.license_info.primary_license);
                        println!("Cache Hit: {:?}", report.cache_hit);
                        println!("Tree Digest: {:?}", report.tree_digest);
                        println!("Commit: {:?}", report.commit);
                        println!("Loaders: {:?}", report.script_loaders);
                        println!("Hooks: {}", report.hooks.len());
                        println!("SQL Files: {}", report.sql_files.len());
                        println!("Findings: {}", report.findings.len());
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "generate-adapter" => {
            let repo_path = Path::new(&args[2]);
            let res = coa_core::analyzer::generate_adapter(repo_path)?;
            match res {
                coa_core::analyzer::AdapterGenerationResult::Success { canonical_loader, adapter_source, delegated_count } => {
                    println!("Canonical Master Loader: {} (delegates to {} scripts)", canonical_loader, delegated_count);
                    if let Some(pos) = args.iter().position(|a| a == "--output") {
                        if pos + 1 < args.len() {
                            let out_path = Path::new(&args[pos + 1]);
                            if let Some(p) = out_path.parent() {
                                std::fs::create_dir_all(p)?;
                            }
                            std::fs::write(out_path, &adapter_source)?;
                            println!("Adapter successfully written to: {}", out_path.display());
                        }
                    } else {
                        println!("--- Generated Adapter Source ---");
                        println!("{}", adapter_source);
                    }
                }
                coa_core::analyzer::AdapterGenerationResult::ManualAdapterRequired { reason, candidate_loaders } => {
                    eprintln!("MANUAL_ADAPTER_REQUIRED: {}", reason);
                    eprintln!("Candidate Loaders: {:?}", candidate_loaders);
                    std::process::exit(1);
                }
                coa_core::analyzer::AdapterGenerationResult::NoLoaderFound => {
                    eprintln!("NO_LOADER_FOUND: Unable to discover any Add*Scripts() functions.");
                    std::process::exit(1);
                }
            }
        }
        "verify-artifact-provenance" => {
            let exe_path = Path::new(&args[2]);
            let mut sdk_json = None;
            let mut json_out = false;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--sdk-json" => {
                        i += 1;
                        if i < args.len() {
                            sdk_json = Some(Path::new(&args[i]));
                        }
                    }
                    "--json" => {
                        json_out = true;
                    }
                    _ => {}
                }
                i += 1;
            }

            match coa_core::provenance::verify_worldserver_provenance(exe_path, sdk_json) {
                Ok(rec) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&rec)?);
                    } else {
                        println!("Artifact: {}", rec.file_path);
                        println!("SHA-256:  {}", rec.file_sha256);
                        println!("Arch:     {}", rec.machine_architecture);
                        println!("Exports:  {}", rec.total_export_count);
                        println!("Embedded ABI: {:?}", rec.embedded_abi_fingerprint);
                        println!("Status:   {}", rec.provenance_status);
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}
