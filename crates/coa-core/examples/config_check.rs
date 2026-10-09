//! Read-only sanity check of the config engine against a real server folder.
use coa_core::config::{self, parser::ConfFile, Scope};
use std::path::Path;

fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: config_check <server folder>");
    let root = Path::new(&root);

    let mut files = 0;
    for dir in ["Core/configs", "Core/configs/modules", "Settings"] {
        let Ok(rd) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
            if !p.is_file() || !(name.contains(".conf") || name.contains(".template")) {
                continue;
            }
            let bytes = std::fs::read(&p).unwrap();
            match ConfFile::parse_bytes(&bytes) {
                Ok(c) => {
                    assert_eq!(
                        c.to_text().as_bytes(),
                        &bytes[..],
                        "round-trip mismatch: {}",
                        p.display()
                    );
                    files += 1;
                }
                Err(e) => println!("skipped {}: {e}", p.display()),
            }
        }
    }
    println!("round-trip identical for {files} files");

    for scope in [Scope::Bots, Scope::Server] {
        match config::load(root, scope) {
            Ok(v) => {
                let present = v.settings.iter().filter(|s| s.present).count();
                let problems: Vec<_> = v
                    .settings
                    .iter()
                    .filter(|s| s.problem.is_some())
                    .map(|s| (&s.meta.key, s.problem.clone().unwrap()))
                    .collect();
                let missing: Vec<_> = v
                    .settings
                    .iter()
                    .filter(|s| !s.present)
                    .map(|s| s.meta.key.as_str())
                    .collect();
                let non_default = v
                    .settings
                    .iter()
                    .filter(|s| s.present && !s.is_default)
                    .count();
                println!(
                    "{scope:?}: {} settings, {present} present, {non_default} non-default, {} unknown keys, drift {:?}\n  problems {:?}\n  missing {:?}",
                    v.settings.len(), v.unknown_keys, v.drift_keys, problems, missing
                );
                for p in scope.presets() {
                    let pv = config::preview_preset(root, scope, &p.id).unwrap();
                    println!(
                        "  preset {:<14} would change {} settings",
                        p.id,
                        pv.changes.len()
                    );
                }
            }
            Err(e) => println!("{scope:?}: {e}"),
        }
    }
}
