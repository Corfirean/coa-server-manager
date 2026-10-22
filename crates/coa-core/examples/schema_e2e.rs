//! Check full schema contracts and blocking custom columns, only against a disposable fixture.
use coa_core::{backup, realms::Mode, schema_check, Error, Result};
use std::path::PathBuf;
fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| Error::Invalid("fixture path required".into()))?,
    );
    if !root
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains("fixture"))
    {
        return Err(Error::Invalid("Refusing a non-fixture server.".into()));
    }
    backup::with_database(&root, |db| {
        let coa = db.clone().for_realm(Mode::Coa);
        let wildcard = db.clone().for_realm(Mode::Wildcard);
        assert!(schema_check::check(&coa, &root)?.is_empty());
        assert!(schema_check::check(&wildcard, &root)?.is_empty());
        let reference = tempfile::tempdir()?;
        schema_check::capture(&coa, reference.path())?;
        assert!(schema_check::check(&coa, reference.path())?.is_empty());
        wildcard.query("ALTER TABLE acore_characters.character_action ADD COLUMN manager_required INT NOT NULL;")?;
        let result = (|| {
            assert!(schema_check::check(&wildcard, &root)?
                .iter()
                .any(|p| p.table == "character_action" && p.column == "manager_required"));
            assert!(schema_check::check(&coa, &root)?.is_empty());
            Ok(())
        })();
        wildcard
            .query("ALTER TABLE acore_characters.character_action DROP COLUMN manager_required;")?;
        result
    })?;
    println!("Full schema contract and a blocking NOT NULL column verified; second-realm corruption does not affect CoA checks.");
    Ok(())
}
