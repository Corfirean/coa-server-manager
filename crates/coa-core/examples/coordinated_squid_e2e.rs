use std::path::{Path, PathBuf};
use coa_core::{backup, driver, fsx, Result};

fn set(root: &Path, filename: &str, values: &[(&str, &str)]) -> Result<()> {
    let path = root.join("Core/configs/modules").join(filename);
    let mut config = coa_core::config::parser::ConfFile::parse_bytes(&std::fs::read(&path)?)?;
    for (key, value) in values { config.set(key, value, &[]); }
    fsx::atomic_write(&path, config.to_text().as_bytes())
}

fn ledgers(root: &Path) -> Result<Vec<String>> {
    backup::with_database(root, |db| ["acore_playerbots", "acore_world", "acore_characters"].into_iter()
        .map(|schema| db.query(&format!("SELECT path,sha256 FROM `{schema}`.coa_squid_migrations ORDER BY path;"))).collect())
}

fn exercise(root: &Path) -> Result<()> {
    let meta = coa_core::registry::metadata_dir_for(root)?;
    let point = backup::create(root, &meta, backup::Kind::Full, backup::Trigger::Manual, Some("Disposable SQUID acceptance".into()), &|stage| println!("{stage}"))?;
    assert!(backup::verify(&meta, &point.id)?.ok);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        backup::with_database(root, |db| db.query("DROP DATABASE IF EXISTS acore_playerbots;").map(|_| ()))?;
        assert!(driver::run(root, driver::Verb::StartAll)?.ok, "Disabled SQUID must start without its database");
        assert!(driver::run(root, driver::Verb::StopAll)?.ok);
        coa_core::multiworld::set_enabled(root, false)?;
        coa_core::realms::select(root, coa_core::realms::Mode::Coa)?;
        set(root, "mod_coa_playerbots.conf", &[("CoaBots.Enable", "0")])?;
        set(root, "playerbots.conf", &[("AiPlayerbot.Enabled", "1"), ("AiPlayerbot.RandomBotAutologin", "0"), ("AiPlayerbot.MinRandomBots", "0"), ("AiPlayerbot.MaxRandomBots", "0"), ("AiPlayerbot.RandomBotGuildCount", "0"), ("AiPlayerbot.RandomBotAccountCount", "0")])?;
        let started = driver::run(root, driver::Verb::StartAll)?;
        assert!(started.ok, "Fresh SQUID startup: {}", coa_core::diag::redact(&started.output));
        let before = ledgers(root)?;
        assert!(!before[0].is_empty(), "Fresh SQUID history missing");
        assert!(driver::run(root, driver::Verb::StopAll)?.ok);
        let started = driver::run(root, driver::Verb::StartAll)?;
        assert!(started.ok, "Repeated SQUID startup: {}", coa_core::diag::redact(&started.output));
        assert_eq!(ledgers(root)?, before, "SQUID replay changed its migration history");
        println!("PASS: SQUID-disabled dual startup without its database; fresh enabled SQUID import and startup; repeated startup preserves all three histories");
        Ok(())
    }));
    assert!(driver::run(root, driver::Verb::StopAll)?.ok);
    restore(root, &meta, &point)?;
    result.map_err(|_| coa_core::Error::Invalid("SQUID acceptance failed; disposable databases and configurations restored".into()))?
}

fn restore(root: &Path, meta: &Path, point: &backup::RecoveryPoint) -> Result<()> {
    assert!(backup::verify(meta, &point.id)?.ok);
    assert!(driver::run(root, driver::Verb::StopAll)?.ok);
    coa_core::realms::select(root, point.realm)?;
    for component in point.components.iter().filter(|component| component.name != "configs") {
        backup::restore_database(root, meta, &point.id, &component.name)?;
    }
    backup::restore_configs(root, meta, &point.id)?;
    assert!(driver::run(root, driver::Verb::StopAll)?.ok);
    println!("PASS: disposable SQUID acceptance databases and original realm configurations restored");
    Ok(())
}

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-schema-fixture-chaos-064-transition-"));
    assert_eq!(std::fs::read(root.join(".transition-fixture"))?, b"disposable published-064 transition fixture");
    assert!(!root.join("Data").symlink_metadata()?.file_type().is_symlink());
    assert!(driver::run(&root, driver::Verb::StopAll)?.ok);
    if let Some(id) = std::env::args().nth(2) {
        let meta = coa_core::registry::metadata_dir_for(&root)?;
        return restore(&root, &meta, &backup::get(&meta, &id)?);
    }
    let result = exercise(&root);
    assert!(driver::run(&root, driver::Verb::StopAll)?.ok);
    result
}
