use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::{
    backup,
    driver::{self, Verb},
    fsx,
    manifest::{Kind, Migration},
    package::{self, BuildOptions},
    pkgsource::Source,
    update::{self, RepackEnv},
    Result,
};
use ed25519_dalek::{Signer, SigningKey};
use std::{collections::BTreeMap, path::PathBuf};
fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    let core = PathBuf::from(std::env::args().nth(2).unwrap());
    let release = PathBuf::from(std::env::args().nth(3).unwrap());
    assert!(root.join("recovery-e2e-started").is_file());
    let result = std::panic::catch_unwind(|| -> Result<()> {
        let meta = coa_core::registry::metadata_dir_for(&root)?;
        let source = root.join("schema-sync-source-v2");
        let pkg = root.join("schema-sync-package-v2");
        assert!(!pkg.exists());
        fsx::atomic_write(
            &source.join("Core/schema-sync-proof.txt"),
            b"official schema migration acceptance",
        )?;
        let published =
            coa_core::manifest::Manifest::parse(&std::fs::read(release.join("manifest.json"))?)?;
        let contract_path = PathBuf::from("C:/games/source/coa-schema-fixture-stability-upgrade-76-85-20261030/Scripts/database-schema.json");
        let contract = std::fs::read(contract_path)?;
        let expected = published
            .files
            .iter()
            .find(|f| f.path == "Scripts/database-schema.json")
            .unwrap();
        assert_eq!(fsx::sha256_bytes(&contract), expected.sha256);
        fsx::atomic_write(&source.join("Scripts/database-schema.json"), &contract)?;
        let mut migrations: Vec<Migration> = Vec::new();
        for id in [
            "rev_1793212891571528600",
            "rev_20261027_18_coa_strange_piece_of_scrap_faction_diamond",
        ] {
            let m = published
                .migrations
                .iter()
                .find(|m| m.id == id)
                .unwrap()
                .clone();
            let sql = std::fs::read(
                core.join(format!("data/sql/updates/pending_db_{}/{}.sql", m.db, m.id)),
            )?;
            assert_eq!(fsx::sha256_bytes(&sql), m.sha256);
            fsx::atomic_write(
                &source.join(format!("_migrations/{}/{}.sql", m.db, m.id)),
                &sql,
            )?;
            migrations.push(m);
        }
        package::build(
            &source,
            &pkg,
            &BuildOptions {
                kind: Kind::Update,
                version: "3.0.0".into(),
                core_commit: None,
                bots_commit: None,
                built_at: "private native fixture schema sync".into(),
                part_size: 1 << 20,
                migrations,
            },
            &|_| {},
        )?;
        let key = SigningKey::generate(&mut rand_core::OsRng);
        fsx::atomic_write(
            &pkg.join("manifest.json.sig"),
            STANDARD
                .encode(
                    key.sign(&std::fs::read(pkg.join("manifest.json"))?)
                        .to_bytes(),
                )
                .as_bytes(),
        )?;
        let trusted = STANDARD.encode(key.verifying_key().to_bytes());
        let env = RepackEnv {
            root: &root,
            meta_dir: &meta,
        };
        let out = update::apply(
            &update::Params {
                root: &root,
                meta_dir: &meta,
                source: Source::Dir(pkg),
                trusted_key: &trusted,
                cancel: Default::default(),
                resolutions: BTreeMap::from([(
                    "Scripts/database-schema.json".into(),
                    update::Resolution::Replace,
                )]),
                env: &env,
                fail_after_ops: None,
            },
            &|step, pct| println!("{pct}% {step}"),
        )?;
        assert_eq!(out.txn.state, update::State::Committed);
        backup::with_database(&root, |db| {
            assert_eq!(
                db.query("SELECT name,at_login FROM acore_characters.characters WHERE guid=3030;")?
                    .trim(),
                "Christian Shadowheart\t1"
            );
            assert_eq!(
                db.query("SELECT id FROM acore_characters.manager_recovery_probe;")?
                    .trim(),
                "42"
            );
            assert_eq!(
                db.query("SELECT id FROM acore_world.manager_recovery_probe;")?
                    .trim(),
                "44"
            );
            Ok(())
        })?;
        println!("PASS: the exact two published migrations brought the old fixture schema forward through the Manager updater while preserving the rejected companion and recovery probes");
        Ok(())
    });
    let stop = driver::run(&root, Verb::StopAll)?;
    assert!(stop.ok, "{}", stop.output);
    result.map_err(|_| {
        coa_core::Error::Invalid("Schema sync acceptance failed; fixture stopped".into())
    })?
}
