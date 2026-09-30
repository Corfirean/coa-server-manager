//! Full clean-install scenario: package a DISPOSABLE fixture, sign it, install it into a new folder.
//! usage: install_e2e <fixture folder> <package output> <install destination>
use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::db::{Account, Db};
use coa_core::download::Cancel;
use coa_core::install::{install_base, Params, Source};
use coa_core::package::{build, BuildOptions};
use coa_core::registry::Registry;
use ed25519_dalek::{Signer, SigningKey};
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (fixture, out, dest) = (PathBuf::from(&a[0]), PathBuf::from(&a[1]), PathBuf::from(&a[2]));
    assert!(a[0].to_lowercase().contains("fixture"), "refusing: source is not a fixture");
    assert!(std::fs::symlink_metadata(fixture.join("Data")).unwrap().file_type().is_symlink(), "Data junction must be skipped by the packager");

    let t = std::time::Instant::now();
    let m = build(&fixture, &out, &BuildOptions { version: "0.1.0".into(), core_commit: None, built_at: chrono::Utc::now().to_rfc3339(), part_size: 256 * 1024 * 1024 }, &|s| eprintln!("  pack: {s}")).expect("build");
    let arch = m.archive.as_ref().unwrap();
    println!("packaged {} files, {:.2} GB -> {} parts, {:.2} GB in {:?}", m.files.len(), arch.unpacked_size as f64 / 1e9, arch.parts.len(), arch.parts.iter().map(|p| p.size).sum::<u64>() as f64 / 1e9, t.elapsed());

    let key_path = format!("{}/.coa-manager/signing/manifest-signing.key", std::env::var("USERPROFILE").unwrap());
    let seed: [u8; 32] = STANDARD.decode(std::fs::read_to_string(key_path).unwrap().trim()).unwrap().try_into().unwrap();
    let sig = SigningKey::from_bytes(&seed).sign(&std::fs::read(out.join("manifest.json")).unwrap());
    std::fs::write(out.join("manifest.json.sig"), STANDARD.encode(sig.to_bytes())).unwrap();

    let reg_dir = std::env::temp_dir().join("coa-install-e2e");
    let _ = std::fs::remove_dir_all(&reg_dir);
    let registry = Registry::at(reg_dir.join("installs.json"));
    let t = std::time::Instant::now();
    let installed = install_base(
        &Params { source: Source::Dir(out.clone()), dest: dest.clone(), trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY, registry: &registry, cancel: Cancel::default() },
        &|s| eprintln!("  install: {:>3}% {} {}", s.percent, s.step, s.detail.unwrap_or_default()),
    )
    .expect("install");
    println!("installed {} at {} in {:?}", installed.version, installed.path, t.elapsed());

    assert!(!dest.with_file_name("coa-installtest.installing").exists());
    assert!(!dest.join("Settings/database.bootstrap.json").exists(), "bootstrap credentials are removed");
    assert!(dest.join("Core/worldserver.exe").is_file());
    assert!(dest.with_file_name("coa-installtest.manager/install.json").is_file());
    assert_eq!(registry.list().unwrap().len(), 1);

    // The rotated credentials must differ from the packaged ones and work.
    let packaged: serde_json::Value = serde_json::from_slice(&std::fs::read(fixture.join("Settings/database.json")).unwrap()).unwrap();
    let now: serde_json::Value = serde_json::from_slice(&std::fs::read(dest.join("Settings/database.json")).unwrap()).unwrap();
    assert_ne!(packaged["rootPassword"], now["rootPassword"]);
    assert_ne!(packaged["appPassword"], now["appPassword"]);
    coa_core::backup::with_database(&dest, |db: &Db| {
        assert!(db.ping());
        assert_eq!(db.tables("acore_auth")?.len() > 10, true);
        let app = Db::from_repack(&dest, Account::App)?;
        assert!(app.query("SELECT 1;")? == "1", "the game account works with the new password");
        Ok(())
    })
    .expect("database after install");
    println!("OK: install verified (rotated credentials work, no leftovers)");
}
