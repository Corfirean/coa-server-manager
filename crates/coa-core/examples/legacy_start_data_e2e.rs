use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::{
    backup,
    db::Db,
    driver::{self, Verb},
    fsx,
    manifest::{Kind, Migration},
    package::{self, BuildOptions},
    pkgsource::Source,
    registry::MetaDir,
    update::{self, RepackEnv},
    Result,
};
use ed25519_dalek::{Signer, SigningKey};
use std::{collections::BTreeMap, path::PathBuf};
fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("qualified disposable fixture"),
    );
    let sql = std::fs::read(std::env::args().nth(2).expect("corrective SQL"))?;
    assert_eq!(
        root.file_name().unwrap(),
        "coa-schema-fixture-stability-upgrade-76-85-20261030"
    );
    assert!(root.join(".release-schema-fixture").is_file());
    assert!(!root.join("legacy-start-data-e2e-started").exists());
    fsx::atomic_write(
        &root.join("legacy-start-data-e2e-started"),
        b"disposable regression test",
    )?;
    let result = std::panic::catch_unwind(|| -> Result<()> {
        let meta = coa_core::registry::metadata_dir_for(&root)?;
        let mut identities = String::new();
        let mut custom = String::new();
        backup::with_database(&root, |db| {
            assert_eq!(
                db.query(
                    "SELECT COUNT(*) FROM acore_world.playercreateinfo WHERE race IN (33,45);"
                )?
                .trim(),
                "0"
            );
            let base: u32 = db
                .query("SELECT MAX(guid)+100 FROM acore_characters.characters;")?
                .trim()
                .parse()
                .unwrap();
            clone_character(db, base, 33, 12, "Thinprobe")?;
            clone_character(db, base + 1, 45, 30, "Leperprobe")?;
            identities = db.query(&format!(
                "SELECT * FROM acore_characters.characters WHERE guid IN ({base},{});",
                base + 1
            ))?;
            let missing = coa_core::character_start_check::missing(db)?;
            assert!(missing
                .iter()
                .any(|c| c.guid == base && c.race == 33 && c.class == 12));
            assert!(missing
                .iter()
                .any(|c| c.guid == base + 1 && c.race == 45 && c.class == 30));
            db.query("INSERT INTO acore_world.playercreateinfo SELECT 33,class,map,zone,position_x+0.125,position_y,position_z,orientation FROM acore_world.playercreateinfo WHERE race=1 AND class=32;")?;
            custom =
                db.query("SELECT * FROM acore_world.playercreateinfo WHERE race=33 AND class=32;")?;
            fsx::atomic_write(
                &root.join("legacy-probe-guids.json"),
                serde_json::to_string(&[base, base + 1])?.as_bytes(),
            )?;
            println!("PASS: reproduced missing starting data for existing race 33/class 12 and race 45/class 30 characters");
            Ok(())
        })?;
        let source = root.join("legacy-start-data-source");
        let pkg = root.join("legacy-start-data-package");
        fsx::atomic_write(
            &source.join("Core/legacy-start-data-proof.txt"),
            b"corrective migration acceptance",
        )?;
        fsx::atomic_write(
            &source.join("_migrations/world/stabilization_legacy_starts.sql"),
            &sql,
        )?;
        let manifest = package::build(
            &source,
            &pkg,
            &BuildOptions {
                kind: Kind::Update,
                version: "2.0.0".into(),
                core_commit: None,
                bots_commit: None,
                built_at: "private regression acceptance".into(),
                part_size: 1 << 20,
                migrations: vec![Migration {
                    id: "stabilization_legacy_starts".into(),
                    db: "world".into(),
                    sha256: fsx::sha256_bytes(&sql),
                    compatible_sha256: vec![],
                    destructive: false,
                }],
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
                resolutions: BTreeMap::new(),
                env: &env,
                fail_after_ops: None,
            },
            &|step, pct| println!("{pct}% {step}"),
        )?;
        assert_eq!(out.txn.state, update::State::Committed);
        assert_eq!(
            MetaDir::open(&meta)?.1.core.version.as_deref(),
            Some("2.0.0")
        );
        backup::with_database(&root, |db| {
            let guids: Vec<u32> =
                serde_json::from_slice(&std::fs::read(root.join("legacy-probe-guids.json"))?)?;
            assert_eq!(
                db.query(&format!(
                    "SELECT * FROM acore_characters.characters WHERE guid IN ({},{});",
                    guids[0], guids[1]
                ))?,
                identities
            );
            assert!(coa_core::character_start_check::missing(db)?
                .iter()
                .all(|c| !guids.contains(&c.guid)));
            assert_eq!(
                db.query("SELECT * FROM acore_world.playercreateinfo WHERE race=33 AND class=32;")?,
                custom
            );
            assert!(coa_core::migrations::status(db, &manifest.migrations)?
                .iter()
                .all(|m| m.status == coa_core::migrations::Status::Applied));
            let before = counts(db)?;
            db.query(&format!(
                "USE acore_world; {}",
                String::from_utf8_lossy(&sql)
            ))?;
            assert_eq!(counts(db)?, before);
            assert_eq!(
                db.query("SELECT * FROM acore_world.playercreateinfo WHERE race=33 AND class=32;")?,
                custom
            );
            println!("PASS: signed Manager update restored legacy starts, preserved complete character rows and custom starts, and remained idempotent on a second SQL execution");
            Ok(())
        })?;
        Ok(())
    });
    let stopped = driver::run(&root, Verb::StopAll)?;
    assert!(stopped.ok, "{}", stopped.output);
    result.map_err(|_| {
        coa_core::Error::Invalid("Legacy start regression failed; fixture stopped".into())
    })?
}
fn counts(db: &Db) -> Result<String> {
    db.query("SELECT COUNT(*) FROM acore_world.playercreateinfo WHERE race IN (33,45); SELECT COUNT(*) FROM acore_world.playercreateinfo_item WHERE race IN (33,45); SELECT COUNT(*) FROM acore_world.playercreateinfo_action WHERE race IN (33,45); SELECT COUNT(*) FROM acore_world.player_race_stats WHERE Race IN (33,45); SELECT COUNT(*) FROM acore_world.ascension_custom_class_race WHERE race IN (33,45);")
}
fn clone_character(db: &Db, guid: u32, race: u32, class: u32, name: &str) -> Result<()> {
    let columns = db.query("SHOW COLUMNS FROM acore_characters.characters;")?;
    let names: Vec<_> = columns
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    let values: Vec<String> = names
        .iter()
        .map(|c| match *c {
            "guid" => guid.to_string(),
            "name" => format!("'{name}'"),
            "race" => race.to_string(),
            "class" => class.to_string(),
            "online" | "at_login" => "0".into(),
            _ => format!("`{c}`"),
        })
        .collect();
    db.query(&format!("INSERT INTO acore_characters.characters ({}) SELECT {} FROM acore_characters.characters WHERE class={class} LIMIT 1;",names.iter().map(|c|format!("`{c}`")).collect::<Vec<_>>().join(","),values.join(",")))?;
    Ok(())
}
