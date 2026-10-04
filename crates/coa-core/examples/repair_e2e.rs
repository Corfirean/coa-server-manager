//! Signed-package repair smoke test. Only a disposable fixture may be used.
use std::path::PathBuf;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use coa_core::{Result, Error, fsx, package::{self, BuildOptions}, manifest::{Kind, Migration}, registry::{InstallKind, InstallMeta, MetaDir, metadata_dir_for}, pkgsource::Source, realms::Mode};

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or_else(|| Error::Invalid("fixture path required".into()))?);
    if !root.file_name().is_some_and(|n| n.to_string_lossy().contains("fixture")) { return Err(Error::Invalid("Refusing a non-fixture server.".into())); }
    let work = tempfile::tempdir()?;
    let base_tree = work.path().join("base-tree");
    let update_tree = work.path().join("update-tree");
    let base_pkg = work.path().join("base-pkg");
    let update_pkg = work.path().join("update-pkg");
    let probe = "Core/reference/repair-e2e.txt";
    fsx::atomic_write(&base_tree.join(probe), b"base")?;
    fsx::atomic_write(&update_tree.join(probe), b"correct official file")?;
    let sql = "CREATE TABLE IF NOT EXISTS acore_world.manager_repair_probe (id INT PRIMARY KEY); INSERT IGNORE INTO acore_world.manager_repair_probe VALUES (42);";
    let id = "20261004_repair_e2e";
    fsx::atomic_write(&update_tree.join(format!("_migrations/world/{id}.sql")), sql.as_bytes())?;
    let options = |kind, version: &str, migrations| BuildOptions { kind, version: version.into(), core_commit: None, built_at: "fixture".into(), part_size: 1 << 20, bots_commit: None, migrations };
    let base = package::build(&base_tree, &base_pkg, &options(Kind::Base, "1.0.0", vec![]), &|_| {})?;
    let update = package::build(&update_tree, &update_pkg, &options(Kind::Update, "1.0.1", vec![Migration { id: id.into(), db: "world".into(), sha256: fsx::sha256_bytes(sql.as_bytes()), destructive: false }]), &|_| {})?;
    // A test-only trust key, unrelated to production signing material.
    let key = SigningKey::from_bytes(&[23; 32]);
    let public = STANDARD.encode(key.verifying_key().as_bytes());
    for pkg in [&base_pkg, &update_pkg] {
        fsx::atomic_write(&pkg.join("manifest.json.sig"), STANDARD.encode(key.sign(&std::fs::read(pkg.join("manifest.json"))?).to_bytes()).as_bytes())?;
    }
    let dir = metadata_dir_for(&root)?;
    let mut meta = InstallMeta::new(InstallKind::New, &root);
    meta.core.version = Some(update.version.clone());
    MetaDir::create(&root, &meta)?;
    fsx::atomic_write_json(&dir.join("manifests/base.json"), &base)?;
    fsx::atomic_write_json(&dir.join("manifests/update-1.0.1.json"), &update)?;
    fsx::atomic_write(&root.join(probe), b"corrupted")?;
    let settings = std::fs::read(root.join("Settings/repack.json"))?;
    let counts = coa_core::backup::with_database(&root, |db| db.query("SELECT COUNT(*) FROM acore_auth.account; SELECT COUNT(*) FROM acore_characters.characters; SELECT COUNT(*) FROM acore_characters_wildcard.characters;"))?;
    let report = coa_core::repair::run(&root, &dir, &Source::Dir(base_pkg), &Source::Dir(update_pkg), &public, &|s,p| println!("{p}% {s}"))?;
    assert_eq!(std::fs::read(root.join(probe))?, b"correct official file");
    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(report.applied.len(), 2, "Migration must run in both worlds");
    assert!(report.database.iter().all(|d| d.problems.is_empty() && d.migrations.iter().all(|m| m.status == coa_core::migrations::Status::Applied)));
    assert_eq!(std::fs::read(root.join("Settings/repack.json"))?, settings);
    assert!(coa_core::backup::verify(&dir, &report.backup)?.ok);
    coa_core::backup::with_database(&root, |db| {
        assert_eq!(db.query("SELECT COUNT(*) FROM acore_auth.account; SELECT COUNT(*) FROM acore_characters.characters; SELECT COUNT(*) FROM acore_characters_wildcard.characters;")?, counts);
        for mode in [Mode::Coa, Mode::Wildcard] {
            assert_eq!(db.clone().for_realm(mode).query("SELECT id FROM acore_world.manager_repair_probe;")?, "42");
        }
        // Corrupt only the second realm, proving that checks read its actual schema.
        let wildcard = db.clone().for_realm(Mode::Wildcard);
        wildcard.query("ALTER TABLE acore_characters.character_action DROP COLUMN type;")?;
        let problems = coa_core::schema_check::check(&wildcard, &root)?;
        assert!(problems.iter().any(|p| p.table == "character_action" && p.column == "type"));
        assert!(coa_core::schema_check::check(&db.clone().for_realm(Mode::Coa), &root)?.is_empty());
        wildcard.query("ALTER TABLE acore_characters.character_action ADD COLUMN type TINYINT UNSIGNED NOT NULL DEFAULT 0;")?;
        Ok(())
    })?;
    println!("Repair verified: signed file restored, both realms migrated, backup valid, accounts/characters/settings preserved, missing column isolated to its realm.");
    Ok(())
}
