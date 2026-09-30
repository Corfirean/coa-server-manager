//! Companion (bot) support data. The bot module creates each bot by copying a level 80 "template" character of the
//! same class, so a server with no such characters can not create any bot. A freshly installed server ships with a
//! small set (one per CoA class); older or imported servers can have it added on request.

use std::fs;
use std::path::Path;

use crate::db::Db;
use crate::error::Result;

/// Owner of the template characters. Not account 1 (the bot module skips it) and not a bot-host account; no such
/// account exists in the login database, so nobody can ever log in to these characters.
pub const TEMPLATE_ACCOUNT: u32 = 900_000;

const SQL: &str = include_str!("../data/companion-templates.sql");

/// The seed script with its two variables set: `base_guid` is added to every character guid.
pub fn seed_sql(base_guid: &str) -> String {
    format!("SET @G = {base_guid}; SET @ACC = {TEMPLATE_ACCOUNT};\n{SQL}")
}

/// Number of characters the bot module could use as templates (level 80, a CoA class, not account 1).
pub fn template_count(db: &Db) -> Result<u32> {
    let n = db.query("SELECT COUNT(*) FROM acore_characters.characters WHERE account<>1 AND level>=80 AND class BETWEEN 12 AND 32;")?;
    Ok(n.trim().parse().unwrap_or(0))
}

fn run_seed(db: &Db, base_guid: &str) -> Result<()> {
    let file = std::env::temp_dir().join(format!("coa-companion-templates-{}.sql", std::process::id()));
    fs::write(&file, seed_sql(base_guid))?;
    let r = db.run_sql_file("acore_characters", &file);
    let _ = fs::remove_file(&file);
    r
}

/// Fresh databases (release tooling): the original guids.
pub fn seed_fresh(db: &Db) -> Result<()> {
    run_seed(db, "0")
}

/// An existing server without templates: add them far above the highest guid in use, so a running world can never
/// hand a new character one of their guids. Returns whether anything was added.
pub fn ensure_templates(db: &Db) -> Result<bool> {
    if template_count(db)? > 0 {
        return Ok(false);
    }
    run_seed(db, "(SELECT IFNULL(MAX(guid),0) FROM acore_characters.characters) + 100000")?;
    Ok(true)
}

pub fn has_seed_file(_root: &Path) -> bool {
    !SQL.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_has_one_template_per_class_and_uses_only_variables_for_ids() {
        let s = seed_sql("0");
        assert!(s.starts_with("SET @G = 0; SET @ACC = 900000;"));
        let chars = s.matches("INSERT IGNORE INTO `characters`").count();
        assert_eq!(chars, 21);
        assert_eq!(s.matches("VALUES (@G+").count(), s.matches("INSERT IGNORE").count(), "every row takes its guid from @G");
        assert!(s.contains("@ACC"));
        assert!(!s.contains("INSERT INTO "), "rows that exist are kept");
    }
}
