//! Real signed-package transition on an independent, explicitly marked disposable copy.
//! The Env wrapper instruments only launcher isolation; migration/backup/update code is production code.
use std::{collections::BTreeMap, path::{Path, PathBuf}};
use coa_core::{backup, driver, fsx, migrations, pkgsource::{self, Source}, registry::{InstallMeta, MetaDir}, update::{self, Env, RepackEnv}, Result};

struct Offline<'a>(RepackEnv<'a>);
impl Env for Offline<'_> {
    fn ensure_stopped(&self) -> Result<()> { self.0.ensure_stopped() }
    fn preflight(&self, m: &coa_core::manifest::Manifest) -> Result<()> { self.0.preflight(m) }
    fn verify_snapshot(&self, id: &str) -> Result<()> { self.0.verify_snapshot(id) }
    fn snapshot(&self) -> Result<String> { self.0.snapshot() }
    fn restore_snapshot(&self, id: &str) -> Result<()> { self.0.restore_snapshot(id) }
    fn migrate(&self, m: &coa_core::manifest::Manifest, p: &Path) -> Result<migrations::ApplyReport> { self.0.migrate(m,p) }
    fn validate(&self) -> Result<()> {
        let path=self.0.root.join("Scripts/manage.py");
        let source=std::fs::read_to_string(&path)?.replace("\r\n", "\n");
        // Explicit test instrumentation prevents relay transmissions and binds services locally.
        let guarded=source.replace("def spawn(name, command, cwd, log):\n", "def spawn(name, command, cwd, log):\n    for filename in ('worldserver.conf', 'authserver.conf'):\n        path = ROOT / 'Core/configs' / filename\n        if path.exists():\n            text = path.read_text(encoding='utf-8-sig')\n            for key, value in (('BindIP', '\"127.0.0.1\"'), ('Ra.IP', '\"127.0.0.1\"'), ('SOAP.Enabled', '0')):\n                text = re.sub(r'(?m)^\\s*' + re.escape(key) + r'\\s*=.*$', '', text)\n                text += '\\n' + key + ' = ' + value + '\\n'\n            path.write_text(text, encoding='utf-8')\n").replace("def relay_worker():\n", "def relay_worker():\n    save_process('relay-ready', os.getpid())\n    while not (STATE / 'stop-relay').exists():\n        time.sleep(0.5)\n    return\n");
        assert_ne!(source,guarded,"Fixture launcher isolation failed");
        fsx::atomic_write(&path,guarded.as_bytes())?;
        self.0.validate()
    }
}
fn players(db: &coa_core::db::Db) -> Result<String> {
    db.query("SELECT CONCAT(id,':',username,':',HEX(salt),':',HEX(verifier)) FROM acore_auth.account WHERE id=900001; SELECT CONCAT(guid,':',account,':',name,':',level,':',money) FROM acore_characters.characters WHERE guid=900001; SELECT CONCAT(guid,':',itemEntry,':',owner_guid,':',enchantments) FROM acore_characters.item_instance WHERE guid=900001; SELECT CONCAT(guid,':',bag,':',slot,':',item) FROM acore_characters.character_inventory WHERE guid=900001;")
}
fn main() -> Result<()> {
    let args:Vec<_>=std::env::args().skip(1).collect();
    let root=PathBuf::from(&args[0]);
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-schema-fixture-chaos-064-transition-"));
    assert_eq!(std::fs::read(root.join(".transition-fixture"))?,b"disposable published-064 transition fixture");
    if args.get(3).is_some_and(|mode| mode=="legacy-recovery") {
        return run_fixture(&root,||legacy_recovery(&root));
    }
    assert!(!root.join(".transition-started").exists(),"Use a new independent fixture");
    fsx::atomic_write(&root.join(".transition-started"),b"started")?;
    let meta=coa_core::registry::metadata_dir_for(&root)?;
    assert!(!meta.exists(),"Do not reuse experimental or user metadata");
    run_fixture(&root,||exercise(&root,&meta,&args[1],&args[2]))
}
fn run_fixture(root:&Path,operation:impl FnOnce()->Result<()>)->Result<()> {
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
    assert!(driver::run(root,driver::Verb::StopAll)?.ok,"Fixture shutdown failed");
    result.map_err(|_|coa_core::Error::Invalid("Disposable acceptance assertion failed; services were stopped and recovery copies retained.".into()))?
}
fn legacy_recovery(root:&Path)->Result<()> {
    let meta=coa_core::registry::metadata_dir_for(root)?;
    assert!(driver::run(root,driver::Verb::StopAll)?.ok);
    let expected=backup::with_database(root,|db| [coa_core::realms::Mode::Coa,coa_core::realms::Mode::Wildcard].into_iter().map(|mode|players(&db.clone().for_realm(mode))).collect::<Result<Vec<_>>>())?;
    assert!(driver::run(root,driver::Verb::StopAll)?.ok);
    let dirs=std::fs::read_dir(meta.join("updates"))?.collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(dirs.len(),1,"Requires exactly the preceding successful disposable transition");
    let txn_dir=dirs[0].path();
    let journal=txn_dir.join("txn.json");
    let mut legacy:serde_json::Value=fsx::read_json(&journal)?;
    assert_eq!(legacy["state"],"committed");
    let point=legacy["recovery_point"].as_str().unwrap().to_string();
    let point_path=meta.join("backups").join(&point).join("backup.json");
    let mut point_json:serde_json::Value=fsx::read_json(&point_path)?;
    point_json["manager_version"]="0.6.4".into();
    for component in point_json["components"].as_array_mut().unwrap() { component.as_object_mut().unwrap().remove("file_sha256"); }
    fsx::atomic_write_json(&point_path,&point_json)?;
    legacy.as_object_mut().unwrap().remove("recovery_hashes");
    legacy.as_object_mut().unwrap().remove("databases_started");
    legacy["state"]="applied".into();
    fsx::atomic_write_json(&journal,&legacy)?;
    assert!(update::ensure_recovered(&meta).is_err(),"Legacy unfinished update must block startup");
    let id=legacy["id"].as_str().unwrap();
    let env=RepackEnv {root,meta_dir:&meta};
    println!("Recovering legacy-format transaction and recovery point on disposable installation");
    assert_eq!(update::rollback(root,&meta,id,&env)?.state,update::State::RolledBack);
    for op in legacy["ops"].as_array().unwrap() {
        let relative=op["path"].as_str().unwrap();
        if op["had_previous"]==true {
            assert_eq!(fsx::sha256_file(&fsx::safe_join(root,relative)?)?,fsx::sha256_file(&fsx::safe_join(&txn_dir.join("before"),relative)?)?,"Legacy file restoration mismatch: {relative}");
        } else { assert!(!fsx::safe_join(root,relative)?.exists()); }
    }
    assert_eq!(std::fs::read(meta.join("install.json"))?,std::fs::read(txn_dir.join("before/manager-install.json"))?);
    backup::with_database(root,|db| {
        for (index,mode) in [coa_core::realms::Mode::Coa,coa_core::realms::Mode::Wildcard].into_iter().enumerate() {
            let db=db.clone().for_realm(mode);
            assert_eq!(players(&db)?,expected[index]);
            assert_eq!(db.query("SELECT COUNT(*) FROM acore_world.coa_manager_migrations WHERE id IN ('manager_repair__20261005_reconcile_wildcard_tables','manager_repair__20261005_reconcile_missing_class_starts');")?,"0");
            assert_eq!(db.query("SELECT sha256 FROM acore_world.coa_manager_migrations WHERE id='manager_repair__20261005_missing_wildcard_tables';")?,"9e4a36245c3f83255415c122c01f08ceb3e3b83898d3d427ac4e2fbcf61e9638");
        }
        Ok(())
    })?;
    assert!(driver::run(root,driver::Verb::StopAll)?.ok);
    assert!(update::pending_checked(&meta)?.is_none());
    assert!(update::rollback(root,&meta,id,&env).is_err(),"Completed recovery must not run again");
    println!("PASS: legacy journal blocked startup; old files, metadata, migration histories and player fields restored; completed recovery rejects replay. Original baseline had a missing table, so healthy startup is not claimed after rollback.");
    Ok(())
}
fn exercise(root:&Path,meta:&Path,baseline:&str,candidate:&str)->Result<()> {
    let mut install:InstallMeta=fsx::read_json(&PathBuf::from("C:/games/coa-schema-fixture-chaos-20261005-v1.manager/install.json"))?;
    install.server_path=root.to_string_lossy().into_owned();
    install.id=uuid::Uuid::new_v4().to_string();
    install.manager_version="0.6.4".into();
    MetaDir::create(root,&install)?;
    // Private ports, and no inherited PID records or paths to the source database.
    let mut cfg:serde_json::Value=fsx::read_json(&root.join("Settings/repack.json"))?;
    let mut listeners=Vec::new();
    for key in ["mysqlPort","authPort","worldPort","raPort"] {
        let listener=std::net::TcpListener::bind("127.0.0.1:0")?;
        cfg[key]=listener.local_addr()?.port().into(); listeners.push(listener);
    }
    fsx::atomic_write_json(&root.join("Settings/repack.json"),&cfg)?;
    let mut profile:serde_json::Value=fsx::read_json(&root.join("Settings/realm-profile.json"))?;
    for key in ["secondary_world_port","secondary_ra_port"] {
        let listener=std::net::TcpListener::bind("127.0.0.1:0")?;
        profile[key]=listener.local_addr()?.port().into(); listeners.push(listener);
    }
    fsx::atomic_write_json(&root.join("Settings/realm-profile.json"),&profile)?;
    drop(listeners);
    let state=root.join(".state");
    if state.exists() { std::fs::remove_dir_all(&state)?; }
    let secondary_state=root.join(".realms/secondary/.state");
    if secondary_state.exists() { std::fs::remove_dir_all(&secondary_state)?; }
    let source=Source::Dir(baseline.into());
    let (manifest,_)=pkgsource::fetch_manifest(&source,coa_core::signing::EMBEDDED_PUBLIC_KEY)?;
    let temp=tempfile::tempdir()?;
    let parts=pkgsource::fetch_parts(&source,&manifest,&temp.path().join("download"),&Default::default(),&|_,_|{})?;
    let tree=temp.path().join("tree");
    coa_core::package::extract(&parts,&manifest,&tree,&|_,_|{})?;
    for file in &manifest.files {
        if file.path.starts_with("_migrations/") {continue;}
        fsx::atomic_write(&fsx::safe_join(root,&file.path)?,&std::fs::read(fsx::safe_join(&tree,&file.path)?)?)?;
        install.original_hashes.insert(file.path.clone(),file.sha256.clone());
    }
    install.core.version=Some(manifest.version);
    fsx::atomic_write_json(&meta.join("install.json"),&install)?;
    // Both worlds must remain local, including generated secondary configuration.
    for relative in ["Settings/worldserver.conf.template","Settings/authserver.conf.template"] {
        let p=root.join(relative);
        let mut conf=coa_core::config::parser::ConfFile::parse_bytes(&std::fs::read(&p)?)?;
        conf.set("BindIP","\"127.0.0.1\"",&[]); conf.set("Ra.IP","\"127.0.0.1\"",&[]); conf.set("SOAP.Enabled","0",&[]);
        fsx::atomic_write(&p,conf.to_text().as_bytes())?;
    }
    let expected=backup::with_database(root,|db| {
        let mut saved=Vec::new();
        for mode in [coa_core::realms::Mode::Coa,coa_core::realms::Mode::Wildcard] {
            let db=db.clone().for_realm(mode);
            saved.push(players(&db)?);
            db.query("DROP TABLE IF EXISTS acore_characters.coa_wildcard_skill_card; DELETE FROM acore_world.coa_manager_migrations WHERE id IN ('manager_repair__20261005_reconcile_wildcard_tables','manager_repair__20261005_reconcile_missing_class_starts'); UPDATE acore_world.coa_manager_migrations SET sha256='9e4a36245c3f83255415c122c01f08ceb3e3b83898d3d427ac4e2fbcf61e9638' WHERE id='manager_repair__20261005_missing_wildcard_tables' AND status='applied';")?;
        }
        Ok(saved)
    })?;
    let source=Source::Dir(candidate.into());
    let candidate_manifest=pkgsource::fetch_manifest(&source,coa_core::signing::EMBEDDED_PUBLIC_KEY)?.0;
    let choices:BTreeMap<_,_>=update::plan(root,&install,&candidate_manifest,&BTreeMap::new(),None)?.into_iter().filter(|p|p.action==update::Action::Conflict).map(|p|(p.path,update::Resolution::Replace)).collect();
    let env=Offline(RepackEnv{root,meta_dir:meta});
    let outcome=update::apply(&update::Params {root,meta_dir:meta,source,trusted_key:coa_core::signing::EMBEDDED_PUBLIC_KEY,cancel:Default::default(),resolutions:choices,env:&env,fail_after_ops:None},&|stage,pct|println!("{pct}% {stage}"))?;
    assert_eq!(outcome.txn.state,update::State::Committed,"{:?}",outcome.txn.message);
    backup::with_database(root,|db| {
        for (index,mode) in [coa_core::realms::Mode::Coa,coa_core::realms::Mode::Wildcard].into_iter().enumerate() {
            let db=db.clone().for_realm(mode);
            assert_eq!(players(&db)?,expected[index],"Player data changed in {}",mode.name());
            assert!(coa_core::schema_check::check(&db,root)?.is_empty());
            assert_eq!(migrations::pending_count(&db,&candidate_manifest.migrations)?,0);
        }
        Ok(())
    })?;
    println!("PASS: signed historical package to candidate; missing Wildcard table repaired in both realms; player fields and inventory preserved; schema and startup verified. Launcher isolation was test instrumentation, not packaged UI acceptance.");
    Ok(())
}
