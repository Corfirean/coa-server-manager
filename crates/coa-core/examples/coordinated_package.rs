use std::{fs, path::PathBuf};
use coa_core::{manifest::{Kind, Manifest}, package::{self, BuildOptions}, pkgsource::{self, Source}, release, signing, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = Source::Dir(args[1].clone().into());
    let (previous, _) = pkgsource::fetch_manifest(&source, signing::EMBEDDED_PUBLIC_KEY)?;
    let tree = PathBuf::from(&args[2]);
    if args[0] == "extract" {
        assert!(!tree.exists(), "Use a new release tree");
        package::extract(&PathBuf::from(&args[1]), &previous, &tree, &|_, _| {})?;
        println!("Verified and extracted {}: {} historical migrations", previous.version, previous.migrations.len());
        return Ok(());
    }
    assert_eq!(args[0], "pack");
    let out = PathBuf::from(&args[3]);
    assert!(!out.exists(), "Release addresses must be immutable");
    for migration in &previous.migrations {
        let entry = previous.files.iter().find(|file| file.path.starts_with("_migrations/") && file.sha256 == migration.sha256).expect("Historical SQL entry");
        assert_eq!(coa_core::fsx::sha256_file(&tree.join(&entry.path))?, entry.sha256, "Historical SQL changed: {}", migration.id);
    }
    let mut manifest = package::build(&tree, &out, &BuildOptions {
        kind: Kind::Update, version: args[4].clone(), core_commit: Some(args[5].clone()),
        bots_commit: previous.bots.and_then(|revision| revision.commit),
        built_at: chrono::Utc::now().to_rfc3339(), part_size: package::DEFAULT_PART_SIZE,
        migrations: previous.migrations.clone(),
    }, &|line| println!("{line}"))?;
    manifest.min_manager_version = "0.6.5".into();
    manifest.validate()?;
    coa_core::fsx::atomic_write(&out.join("manifest.json"), &serde_json::to_vec_pretty(&manifest)?)?;
    let key_path = PathBuf::from(std::env::var("USERPROFILE").unwrap()).join(".coa-manager/signing/manifest-signing.key");
    release::sign_manifest(&out, &fs::read_to_string(key_path)?)?;
    coa_core::release_schema::verify_package(&out, signing::EMBEDDED_PUBLIC_KEY)?;
    let reread: Manifest = serde_json::from_slice(&fs::read(out.join("manifest.json"))?)?;
    assert_eq!(serde_json::to_value(reread.migrations)?, serde_json::to_value(previous.migrations)?);
    println!("PASS: signed {}; all historical migration bytes, checksums, aliases and order preserved", manifest.version);
    Ok(())
}
