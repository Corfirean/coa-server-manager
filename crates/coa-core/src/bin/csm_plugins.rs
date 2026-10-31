//! CLI tool for CoA Server Manager plugin operations (testing and automation).
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use coa_core::plugins::{
    self, PluginMetadata,
};

fn print_usage() {
    eprintln!(
        r#"csm_plugins CLI
Usage:
    csm_plugins install <server> <id> <name> <version> <dll> [--pdb <pdb>] [--conf <dist_confs>] [--deps <deps>] [--enable]
    csm_plugins enable <server> <id>
    csm_plugins disable <server> <id>
    csm_plugins repair <server>
    csm_plugins list <server>
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        print_usage();
        std::process::exit(1);
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
        "repair" => {
            let restored = plugins::repair(server)?;
            println!("Repair complete. Restored {} plugin(s): {:?}", restored.len(), restored);
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
        _ => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}
