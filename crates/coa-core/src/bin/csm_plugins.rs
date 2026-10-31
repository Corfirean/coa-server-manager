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
                if !pass.is_empty() {
                    cmd.env("MYSQL_PWD", &pass);
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
        _ => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}
