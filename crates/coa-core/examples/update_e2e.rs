//! Real end-to-end update on a DISPOSABLE fixture: recovery point, file replacement, config merge, a real SQL
//! migration, restart and health check, commit. usage: update_e2e <fixture folder>
use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::download::Cancel;
use coa_core::manifest::{Kind, Migration};
use coa_core::package::{build, BuildOptions};
use coa_core::pkgsource::Source;
use coa_core::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir};
use coa_core::update::{apply, Params, RepackEnv};
use coa_core::{backup, fsx};
use ed25519_dalek::{Signer, SigningKey};
use std::path::PathBuf;

fn put(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("fixture folder"));
    assert!(
        root.to_string_lossy().to_lowercase().contains("fixture"),
        "refusing: not a fixture folder"
    );
    let meta_dir = metadata_dir_for(&root).unwrap();
    if !meta_dir.join("install.json").is_file() {
        MetaDir::create(&root, &InstallMeta::new(InstallKind::Imported, &root)).unwrap();
    }

    let work = std::env::temp_dir().join("coa-update-e2e");
    let _ = std::fs::remove_dir_all(&work);
    let (src, pkg) = (work.join("src"), work.join("pkg"));
    put(
        &src,
        "Core/reference/update-e2e.txt",
        "shipped by the update\n",
    );
    put(
        &src,
        "Settings/worldserver.conf.template",
        "[worldserver]\nUpdateE2E.NewSetting = 7\n",
    );
    let sql = "CREATE TABLE IF NOT EXISTS coa_manager_update_test (id INT PRIMARY KEY); INSERT IGNORE INTO coa_manager_update_test VALUES (1);";
    put(&src, "_migrations/world/2026_09_30_00_update_e2e.sql", sql);
    let mut m = build(
        &src,
        &pkg,
        &BuildOptions {
            kind: Kind::Update,
            version: "0.2.0".into(),
            core_commit: None,
            built_at: chrono::Utc::now().to_rfc3339(),
            part_size: 1 << 20,
            bots_commit: None,
            migrations: vec![],
        },
        &|_| {},
    )
    .unwrap();
    m.migrations.push(Migration {
        compatible_sha256: vec![],
        id: "2026_09_30_00_update_e2e".into(),
        db: "world".into(),
        sha256: fsx::sha256_bytes(sql.as_bytes()),
        destructive: false,
    });
    std::fs::write(
        pkg.join("manifest.json"),
        serde_json::to_vec_pretty(&m).unwrap(),
    )
    .unwrap();
    let key = format!(
        "{}/.coa-manager/signing/manifest-signing.key",
        std::env::var("USERPROFILE").unwrap()
    );
    let seed: [u8; 32] = STANDARD
        .decode(std::fs::read_to_string(key).unwrap().trim())
        .unwrap()
        .try_into()
        .unwrap();
    let sig =
        SigningKey::from_bytes(&seed).sign(&std::fs::read(pkg.join("manifest.json")).unwrap());
    std::fs::write(
        pkg.join("manifest.json.sig"),
        STANDARD.encode(sig.to_bytes()),
    )
    .unwrap();

    let template_before = std::fs::read(root.join("Settings/worldserver.conf.template")).unwrap();
    let env = RepackEnv {
        root: &root,
        meta_dir: &meta_dir,
    };
    let t = std::time::Instant::now();
    let out = apply(
        &Params {
            root: &root,
            meta_dir: &meta_dir,
            source: Source::Dir(pkg),
            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
            cancel: Cancel::default(),
            resolutions: Default::default(),
            env: &env,
            fail_after_ops: None,
        },
        &|s, p| eprintln!("  {p:>3}% {s}"),
    )
    .expect("update");
    println!(
        "update {:?} in {:?}; recovery point {:?}; migrations {:?}",
        out.txn.state,
        t.elapsed(),
        out.txn.recovery_point,
        out.migrations.as_ref().map(|r| &r.applied)
    );

    let tpl = std::fs::read_to_string(root.join("Settings/worldserver.conf.template")).unwrap();
    assert!(tpl.contains("UpdateE2E.NewSetting = 7"), "new key merged");
    assert!(
        tpl.len() > template_before.len()
            && tpl.starts_with(&String::from_utf8_lossy(&template_before).into_owned()),
        "existing template content untouched, key appended"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Core/reference/update-e2e.txt")).unwrap(),
        "shipped by the update\n"
    );
    let n = backup::with_database(&root, |db| {
        db.query("SELECT COUNT(*) FROM acore_world.coa_manager_update_test;")
    })
    .unwrap();
    assert_eq!(n, "1", "the migration ran");
    println!("OK: files, merge, migration and health check verified");
}
