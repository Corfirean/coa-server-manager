//! Reproduce applied migration history with five missing tables on a disposable MySQL fixture.
use std::path::PathBuf;
use coa_core::{backup, fsx, manifest::Migration, migrations::{self, LedgerRow, Status, Store}, release_schema, schema_check, Error, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or_else(|| Error::Invalid("fixture path required".into()))?);
    let old_sql = PathBuf::from(args.next().ok_or_else(|| Error::Invalid("core SQL directory required".into()))?);
    let repairs = PathBuf::from(args.next().ok_or_else(|| Error::Invalid("repair SQL directory required".into()))?);
    if !root.file_name().is_some_and(|n| n.to_string_lossy().starts_with("coa-update-recovery-fixture-"))
        || !root.join("recovery-e2e-started").exists() {
        return Err(Error::Invalid("Refusing a non-disposable recovery fixture.".into()));
    }
    let tree = tempfile::tempdir()?;
    backup::with_database(&root, |db| {
        let db = db.clone().for_realm(coa_core::realms::Mode::Coa);
        let characters = db.query("SELECT COUNT(*) FROM acore_characters.characters;")?;
        db.load()?;
        // Record exactly the old file hashes, as in the user's all-applied report.
        let mut migrations = Vec::new();
        for name in ["rev_20260929_92_coa_wildcard_skill_cards", "rev_20260929_93_coa_wildcard_skill_card_duplicates", "rev_20260929_94_coa_wildcard_specialization_cache"] {
            let path = old_sql.join(format!("{name}.sql"));
            let hash = fsx::sha256_file(&path)?;
            db.put(&LedgerRow { db: "characters".into(), id: name.into(), sha256: hash.clone(), status: Status::Applied, error: None, baseline: false })?;
            migrations.push(Migration { compatible_sha256: vec![], db: "characters".into(), id: name.into(), sha256: hash, destructive: false });
        }
        for (_, tables) in schema_check::WILDCARD_TABLES.iter().filter(|(kind,_)| *kind == "characters") {
            for table in *tables { db.query(&format!("DROP TABLE IF EXISTS acore_characters.`{table}`;"))?; }
        }
        assert!(migrations::status(&db, &migrations)?.iter().all(|m| m.status == Status::Applied));
        assert!(!db.tables("acore_characters")?.iter().any(|t| t == "coa_wildcard_skill_card"));
        let empty_core = tempfile::tempdir()?;
        let files = release_schema::collect(empty_core.path(), None, Some(&repairs))?;
        assert_eq!(files.len(), 1);
        let repair = &files[0];
        migrations.push(Migration { compatible_sha256: vec![], db: repair.db.clone(), id: repair.id.clone(), sha256: repair.sha256.clone(), destructive: false });
        let staged = tree.path().join("sql/characters");
        fsx::atomic_write(&staged.join(format!("{}.sql", repair.id)), &std::fs::read(&repair.abs)?)?;
        let report = migrations::apply_pending(&db, &migrations, &tree.path().join("sql"), &|| Ok("disposable fixture".into()))?;
        assert!(report.failed.is_none());
        assert_eq!(report.applied, [repair.id.clone()]);
        assert_eq!(db.query("SELECT COUNT(*) FROM acore_characters.characters;")?, characters);
        db.query("INSERT INTO acore_characters.coa_wildcard_skill_card VALUES (77,123,456);")?;
        db.run_sql_file("acore_characters", &repair.abs)?;
        assert_eq!(db.query("SELECT progress FROM acore_characters.coa_wildcard_skill_card WHERE account=77 AND card=123;")?, "456");
        assert!(migrations::apply_pending(&db, &migrations, &tree.path().join("sql"), &|| Ok("disposable fixture".into()))?.applied.is_empty());
        schema_check::capture(&db, tree.path())?;
        db.query("DROP TABLE acore_characters.coa_wildcard_skill_card_purchase;")?;
        assert!(schema_check::check(&db, tree.path())?.iter().any(|p| p.table == "coa_wildcard_skill_card_purchase" && p.detail == "Missing table"));
        db.run_sql_file("acore_characters", &repair.abs)?;
        db.query("ALTER TABLE acore_characters.coa_wildcard_skill_card DROP COLUMN progress;")?;
        assert!(schema_check::check(&db, tree.path())?.iter().any(|p| p.table == "coa_wildcard_skill_card" && p.column == "progress"));
        db.query("ALTER TABLE acore_characters.coa_wildcard_skill_card ADD COLUMN progress INT UNSIGNED NOT NULL DEFAULT 0;")?;
        Ok(())
    })?;
    println!("Applied history with all five tables missing repaired; characters and existing card rows preserved; full contract detects missing tables and columns.");
    Ok(())
}
