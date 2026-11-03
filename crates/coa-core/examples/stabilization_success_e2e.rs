use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::{
    backup, driver, fsx,
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
            .expect("disposable recovery fixture"),
    );
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("coa-update-recovery-fixture-"));
    assert!(root.join("recovery-e2e-started").is_file());
    let cycles_only = std::env::args().nth(2).as_deref() == Some("--cycles-only");
    if !cycles_only {
        assert!(!root.join("success-e2e-started").exists());
        fsx::atomic_write(
            &root.join("success-e2e-started"),
            b"disposable success acceptance",
        )?;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if cycles_only {
            restart_cycles(&root)
        } else {
            exercise(&root)
        }
    }));
    assert!(
        driver::run(&root, driver::Verb::StopAll)?.ok,
        "fixture shutdown"
    );
    result.map_err(|_| {
        coa_core::Error::Invalid(
            "Success acceptance assertion failed; fixture services stopped".into(),
        )
    })?
}

fn exercise(root: &std::path::Path) -> Result<()> {
    let meta = coa_core::registry::metadata_dir_for(root)?;
    assert!(update::unfinished(&meta).is_none());
    assert!(driver::run(root, driver::Verb::StopAll)?.ok);
    let source = root.join("success-source");
    let pkg = root.join("success-package");
    fsx::atomic_write(
        &source.join("Core/stabilization-success.txt"),
        b"verified update",
    )?;
    let sql = b"UPDATE manager_recovery_probe SET id=44 WHERE id=43;";
    fsx::atomic_write(
        &source.join("_migrations/world/stabilization_success.sql"),
        sql,
    )?;
    let manifest = package::build(
        &source,
        &pkg,
        &BuildOptions {
            kind: Kind::Update,
            version: "2.0.0".into(),
            core_commit: None,
            bots_commit: None,
            built_at: "disposable acceptance".into(),
            part_size: 1 << 20,
            migrations: vec![Migration {
                id: "stabilization_success".into(),
                db: "world".into(),
                sha256: fsx::sha256_bytes(sql),
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
        root,
        meta_dir: &meta,
    };
    let outcome = update::apply(
        &update::Params {
            root,
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
    assert_eq!(outcome.txn.state, update::State::Committed);
    assert_eq!(
        MetaDir::open(&meta)?.1.core.version.as_deref(),
        Some("2.0.0")
    );
    assert!(update::unfinished(&meta).is_none());
    assert_eq!(
        std::fs::read(root.join("Core/stabilization-success.txt"))?,
        b"verified update"
    );
    backup::with_database(root, |db| {
        assert_eq!(
            db.query("SELECT id FROM acore_characters.manager_recovery_probe;")?,
            "42"
        );
        assert_eq!(
            db.query("SELECT id FROM acore_characters.manager_recovery_view;")?,
            "42"
        );
        assert_eq!(
            db.query("SELECT id FROM acore_world.manager_recovery_probe;")?,
            "44"
        );
        assert!(coa_core::migrations::status(db, &manifest.migrations)?
            .iter()
            .all(|m| m.status == coa_core::migrations::Status::Applied));
        Ok(())
    })?;
    restart_cycles(root)?;
    println!("PASS: signed update committed after failed rehearsal and recovery; SQL, files, metadata and player data verified; two native restart cycles passed");
    Ok(())
}

fn restart_cycles(root: &std::path::Path) -> Result<()> {
    for cycle in 0..2 {
        let stopped = driver::run(root, driver::Verb::StopAll)?;
        assert!(
            stopped.ok,
            "Stop cycle {cycle}: {}",
            coa_core::diag::redact(&stopped.output)
        );
        let started = driver::run(root, driver::Verb::StartAll)?;
        assert!(started.ok, "{}", started.output);
        let pop = coa_core::population::query(root)?;
        println!(
            "Restart cycle {cycle}: world ready, players={}, bots={}",
            pop.players_online, pop.bots_online
        );
    }
    println!("PASS: two native restart cycles finished");
    Ok(())
}
