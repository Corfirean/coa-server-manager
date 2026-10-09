//! Verify published checksum compatibility on a disposable real database, without running SQL migrations.
use coa_core::{
    backup,
    db::Db,
    migrations::{self, LedgerRow, Status, Store},
    pkgsource::{self, Source},
    Result,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Observed<'a> {
    db: &'a Db,
    executions: AtomicUsize,
}
impl Store for Observed<'_> {
    fn load(&self) -> Result<Vec<LedgerRow>> {
        self.db.load()
    }
    fn put(&self, row: &LedgerRow) -> Result<()> {
        self.db.put(row)
    }
    fn run_file(&self, _: &str, _: &Path) -> Result<()> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        panic!("Historical SQL must never be replayed");
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let root = PathBuf::from(&args[0]);
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("coa-schema-fixture-chaos-"));
    assert_eq!(
        std::fs::read(root.join(".coa-chaos-fixture"))?,
        b"disposable update chaos fixture"
    );
    assert_eq!(
        std::fs::read(root.join(".release-schema-fixture"))?,
        b"disposable release-schema fixture"
    );
    let source = Source::Dir(PathBuf::from(&args[1]));
    let package = pkgsource::fetch_manifest(&source, coa_core::signing::EMBEDDED_PUBLIC_KEY)?.0;
    let staging = tempfile::tempdir()?;
    let parts = pkgsource::fetch_parts(
        &source,
        &package,
        &staging.path().join("download"),
        &Default::default(),
        &|_, _| {},
    )?;
    coa_core::package::extract(&parts, &package, &staging.path().join("tree"), &|_, _| {})?;
    migrations::verify_files(
        &package.migrations,
        &staging.path().join("tree/_migrations"),
    )?;
    println!(
        "PASS: all {} signed-package SQL artifacts verified before any database mutation",
        package.migrations.len()
    );
    let migration = package
        .migrations
        .iter()
        .find(|m| m.id == "manager_repair__20261005_missing_wildcard_tables")
        .unwrap();
    let hashes = [
        "c1d8dbf2271234283a48a39e4b4aea1106b83b70581f1e3c4aa9fff438e8e4d2",
        "9e4a36245c3f83255415c122c01f08ceb3e3b83898d3d427ac4e2fbcf61e9638",
    ];
    for hash in hashes {
        assert!(migration.sha256 == hash || migration.compatible_sha256.iter().any(|h| h == hash));
    }
    let fixture_meta = coa_core::registry::metadata_dir_for(&root)?;
    let test_meta = tempfile::tempdir()?;
    coa_core::fsx::atomic_write(
        &test_meta.path().join("install.json"),
        &std::fs::read(fixture_meta.join("install.json"))?,
    )?;
    coa_core::registry::MetaDir::open(test_meta.path())?;
    std::fs::create_dir_all(test_meta.path().join("updates"))?;
    let result = backup::with_database(&root, |db| {
        let mut modes = vec![coa_core::realms::Mode::Coa];
        if coa_core::realms::state(&root)?.wildcard_created {
            modes.push(coa_core::realms::Mode::Wildcard);
        }
        for mode in modes {
            let db = db.clone().for_realm(mode);
            let saved = db
                .load()?
                .into_iter()
                .find(|row| row.id == migration.id && row.db == migration.db)
                .expect("Published repair must already be applied");
            assert_eq!(saved.status, Status::Applied);
            let counts = db.query("SELECT CONCAT((SELECT COUNT(*) FROM acore_characters.characters),':',(SELECT COUNT(*) FROM acore_characters.item_instance),':',(SELECT COUNT(*) FROM acore_auth.account));")?;
            let check = || -> Result<()> {
                for hash in hashes {
                    db.put(&LedgerRow {
                        sha256: hash.into(),
                        ..saved.clone()
                    })?;
                    let observed = Observed {
                        db: &db,
                        executions: AtomicUsize::new(0),
                    };
                    migrations::preflight(&db, std::slice::from_ref(migration))?;
                    let report = migrations::apply_pending(
                        &observed,
                        std::slice::from_ref(migration),
                        &root.join(".unused-hotfix-sql"),
                        &|| panic!("Already applied SQL needs no snapshot"),
                    )?;
                    assert!(report.applied.is_empty());
                    assert_eq!(observed.executions.load(Ordering::SeqCst), 0);
                    assert_eq!(
                        db.load()?
                            .into_iter()
                            .find(|r| r.id == migration.id && r.db == migration.db)
                            .unwrap()
                            .sha256,
                        hash
                    );
                }
                let meta = test_meta.path();
                let file_hash = coa_core::fsx::sha256_file(&root.join("Core/worldserver.exe"))?;
                let install_hash = coa_core::fsx::sha256_file(&meta.join("install.json"))?;
                let transaction_count = std::fs::read_dir(meta.join("updates"))?.count();
                for status in [Status::Running, Status::Failed] {
                    db.put(&LedgerRow {
                        status,
                        ..saved.clone()
                    })?;
                    let env = coa_core::update::RepackEnv {
                        root: &root,
                        meta_dir: meta,
                    };
                    let rejected = coa_core::update::apply(
                        &coa_core::update::Params {
                            root: &root,
                            meta_dir: meta,
                            source: Source::Dir(PathBuf::from(&args[1])),
                            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
                            cancel: Default::default(),
                            resolutions: Default::default(),
                            env: &env,
                            fail_after_ops: None,
                        },
                        &|_, _| {},
                    );
                    let message = rejected.unwrap_err().to_string();
                    assert!(
                        message.contains("partially applied"),
                        "Unexpected rejection: {message}"
                    );
                    assert_eq!(
                        std::fs::read_dir(meta.join("updates"))?.count(),
                        transaction_count
                    );
                    assert_eq!(
                        coa_core::fsx::sha256_file(&root.join("Core/worldserver.exe"))?,
                        file_hash
                    );
                    assert_eq!(
                        coa_core::fsx::sha256_file(&meta.join("install.json"))?,
                        install_hash
                    );
                }
                db.put(&saved)?;
                let env = coa_core::update::RepackEnv {
                    root: &root,
                    meta_dir: meta,
                };
                let pending_before = env.pending_migrations(&package)?;
                db.query("DELETE FROM acore_world.coa_manager_migrations WHERE db='characters' AND id='manager_repair__20261005_missing_wildcard_tables';")?;
                assert_eq!(env.pending_migrations(&package)?, pending_before + 1,
                    "Missing history in either realm must remain visible even with identical server files");
                db.put(&saved)?;
                assert_eq!(env.pending_migrations(&package)?, pending_before);
                Ok(())
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(check));
            db.put(&saved)?;
            result.map_err(|_| {
                coa_core::Error::Invalid(
                    "History regression assertion failed; the injected ledger row was restored."
                        .into(),
                )
            })??;
            assert_eq!(db.query("SELECT CONCAT((SELECT COUNT(*) FROM acore_characters.characters),':',(SELECT COUNT(*) FROM acore_characters.item_instance),':',(SELECT COUNT(*) FROM acore_auth.account));")?, counts);
            println!("PASS: {} published checksums; Running/Failed rejected before staging; missing migration counted; zero SQL replays; history and player counts preserved", mode.name());
        }
        Ok(())
    });
    assert!(coa_core::driver::run(&root, coa_core::driver::Verb::StopAll)?.ok);
    result?;
    Ok(())
}
