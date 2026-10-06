use std::{collections::BTreeMap, path::PathBuf};
use coa_core::{backup, driver, registry::MetaDir, update::{self, RepackEnv}, Result};

fn players(db: &coa_core::db::Db) -> Result<String> {
    db.query("SELECT CONCAT(id,':',username,':',HEX(salt),':',HEX(verifier)) FROM acore_auth.account WHERE id=900001; SELECT CONCAT(guid,':',account,':',name,':',level,':',money) FROM acore_characters.characters WHERE guid=900001; SELECT CONCAT(guid,':',itemEntry,':',owner_guid,':',enchantments) FROM acore_characters.item_instance WHERE guid=900001; SELECT CONCAT(guid,':',bag,':',slot,':',item) FROM acore_characters.character_inventory WHERE guid=900001;")
}

fn exercise(root: &std::path::Path, package: &str) -> Result<()> {
    let meta_dir = coa_core::registry::metadata_dir_for(root)?;
    let (_, meta) = MetaDir::open(&meta_dir)?;
    let expected = backup::with_database(root, |db| [coa_core::realms::Mode::Coa, coa_core::realms::Mode::Wildcard].into_iter().map(|mode| players(&db.clone().for_realm(mode))).collect::<Result<Vec<_>>>())?;
    assert!(driver::run(root, driver::Verb::StopAll)?.ok);
    let source = coa_core::pkgsource::Source::Dir(package.into());
    let preview = update::preview(root, &meta, &source, coa_core::signing::EMBEDDED_PUBLIC_KEY, &BTreeMap::new())?;
    assert_eq!(preview.pending_migrations, 0, "Historical SQL must not replay");
    assert!(!preview.items.iter().any(|item| item.action == update::Action::Conflict), "Unresolved candidate files: {:?}", preview.items);
    let environment = RepackEnv { root, meta_dir: &meta_dir };
    let outcome = update::apply(&update::Params {
        root, meta_dir: &meta_dir, source, trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
        resolutions: BTreeMap::new(), cancel: Default::default(), env: &environment, fail_after_ops: None,
    }, &|stage, progress| println!("{progress}% {stage}"))?;
    assert_eq!(outcome.txn.state, update::State::Committed, "{:?}", outcome.txn);
    backup::with_database(root, |db| {
        for (index, mode) in [coa_core::realms::Mode::Coa, coa_core::realms::Mode::Wildcard].into_iter().enumerate() {
            let realm = db.clone().for_realm(mode);
            assert_eq!(players(&realm)?, expected[index], "Player data changed in {mode:?}");
            let problems = coa_core::schema_check::check(&realm, root)?;
            assert!(problems.is_empty(), "{problems:?}");
        }
        Ok(())
    })?;
    let release = coa_core::squid::release(root).expect("SQUID release metadata");
    println!("SQUID metadata: {release:?}");
    assert_eq!(coa_core::squid::fields(root)?.len(), 40);
    let (_, installed) = MetaDir::open(&meta_dir)?;
    let repeated = update::preview(root, &installed, &coa_core::pkgsource::Source::Dir(package.into()), coa_core::signing::EMBEDDED_PUBLIC_KEY, &BTreeMap::new())?;
    assert_eq!(repeated.pending_migrations, 0);
    assert!(repeated.items.iter().all(|item| item.action == update::Action::Skip), "{:?}", repeated.items);
    println!("PASS: coordinated update committed, both realms started, player fields preserved, historical SQL skipped, repeated preview clean");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(&args[0]);
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-schema-fixture-chaos-064-transition-"));
    assert_eq!(std::fs::read(root.join(".transition-fixture"))?, b"disposable published-064 transition fixture");
    assert_eq!(std::fs::read(root.join(".release-schema-fixture"))?, b"disposable release-schema fixture");
    assert!(!root.join("Data").symlink_metadata()?.file_type().is_symlink());
    assert!(driver::run(&root, driver::Verb::StopAll)?.ok);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&root, &args[1])));
    assert!(driver::run(&root, driver::Verb::StopAll)?.ok, "Fixture shutdown failed");
    outcome.map_err(|_| coa_core::Error::Invalid("Disposable coordinated acceptance failed; stopped services and retained recovery copies".into()))?
}
