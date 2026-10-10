//! Failure injection on a NEW disposable fixture created by tools/make-fixture.ps1.
//! Uses real MySQL dumps, SQL execution, signed packages and update rollback.
use std::{collections::BTreeMap, path::{Path, PathBuf}};
use coa_core::{backup, db::{Account, Db}, download::Cancel, driver::{self, Verb}, fsx,
    manifest::{Kind, Migration}, package::{self, BuildOptions}, pkgsource::Source,
    registry::{InstallKind, InstallMeta, MetaDir},
    update::{self, Env, RepackEnv}, Error, Result};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).expect("usage: update_recovery_e2e <new fixture>");
    let root = PathBuf::from(arg);
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-update-recovery-fixture-"));
    assert!(!root.join("recovery-e2e-started").exists(), "Use a new fixture; never reuse an owned server");
    fsx::atomic_write(&root.join("recovery-e2e-started"), b"disposable fixture")?;
    let result = exercise(&root);
    let stopped = driver::run(&root, Verb::StopAll)?;
    assert!(stopped.ok);
    result
}

fn exercise(root: &Path) -> Result<()> {
    let meta = coa_core::registry::metadata_dir_for(root)?;
    let mut installed = InstallMeta::new(InstallKind::New, root);
    installed.core.version = Some("1.0.0".into());
    MetaDir::create(root, &installed)?;
    backup::with_database(root, |db| {
        db.query("CREATE TABLE acore_characters.manager_recovery_probe (id INT); INSERT INTO acore_characters.manager_recovery_probe VALUES (42); CREATE TABLE acore_world.manager_recovery_probe (id INT); INSERT INTO acore_world.manager_recovery_probe VALUES (43);")?;
        db.query("CREATE VIEW acore_characters.manager_recovery_view AS SELECT id FROM acore_characters.manager_recovery_probe; CREATE TRIGGER acore_characters.manager_recovery_trigger BEFORE INSERT ON acore_characters.manager_recovery_probe FOR EACH ROW SET NEW.id=NEW.id+1; CREATE PROCEDURE acore_characters.manager_recovery_proc() SELECT id FROM acore_characters.manager_recovery_probe;")?;
        Ok(())
    })?;
    let source = root.join("fixture-update-source");
    fsx::atomic_write(&source.join("Core/recovery-probe.txt"), b"updated")?;
    let mut migrations = Vec::new();
    for (db, id, sql) in [
        ("characters", "probe_chars", "UPDATE manager_recovery_probe SET id=999;"),
        ("world", "probe_world", "DROP TABLE manager_recovery_probe; THIS IS INTENTIONALLY INVALID SQL;"),
    ] {
        fsx::atomic_write(&source.join(format!("_migrations/{db}/{id}.sql")), sql.as_bytes())?;
        migrations.push(Migration { compatible_sha256: vec![], db: db.into(), id: id.into(), sha256: fsx::sha256_bytes(sql.as_bytes()), destructive: false });
    }
    let pkg = root.join("fixture-package");
    let manifest = package::build(&source, &pkg, &BuildOptions {
        kind: Kind::Update, version: "2.0.0".into(), core_commit: None, bots_commit: None,
        built_at: "fixture".into(), part_size: 1 << 20, migrations,
    }, &|_| {})?;
    let key = SigningKey::generate(&mut rand_core::OsRng);
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&std::fs::read(pkg.join("manifest.json"))?).to_bytes());
    fsx::atomic_write(&pkg.join("manifest.json.sig"), signature.as_bytes())?;
    let trusted = base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes());
    let env = RepackEnv { root, meta_dir: &meta };
    let error = update::apply(&update::Params {
        root, meta_dir: &meta, source: Source::Dir(pkg), trusted_key: &trusted,
        cancel: Cancel::default(), resolutions: BTreeMap::new(), env: &env, fail_after_ops: None,
    }, &|step, pct| println!("{pct}% {step}" )).unwrap_err();
    assert!(error.to_string().contains("probe_world"), "{error}");
    assert!(error.to_string().contains("private update rehearsal failed"), "{error}");
    assert!(!root.join("Core/recovery-probe.txt").exists());
    assert!(update::unfinished(&meta).is_none());
    assert_eq!(MetaDir::open(&meta)?.1.core.version.as_deref(), Some("1.0.0"));
    backup::with_database(root, |db| {
        assert_eq!(db.query("SELECT id FROM acore_characters.manager_recovery_probe;")?, "42");
        assert_eq!(db.query("SELECT id FROM acore_world.manager_recovery_probe;")?, "43");
        assert_eq!(db.query("SELECT id FROM acore_characters.manager_recovery_view;")?, "42");
        assert_eq!(db.recovery_objects("acore_characters")?.iter().filter(|object| object["name"].as_str().is_some_and(|name| name.starts_with("manager_recovery_"))).count(), 3);
        assert!(!coa_core::migrations::status(db, &manifest.migrations)?.iter().any(|m| m.status == coa_core::migrations::Status::Applied));
        Ok(())
    })?;
    // A recovery point can be reused if the process died midway through restoring multiple schemas.
    let point = backup::list(&meta).into_iter().find(|p| p.trigger == backup::Trigger::BeforeUpdate).ok_or_else(|| Error::Invalid("missing backup".into()))?;
    env.restore_snapshot(&point.id)?;
    assert_eq!(Db::from_repack(root, Account::Admin)?.realm(), coa_core::realms::Mode::Coa);
    println!("PASS: private rehearsal caught failed SQL before changing the installed world, characters, ledger, objects, files or version; complete recovery succeeded");
    Ok(())
}
