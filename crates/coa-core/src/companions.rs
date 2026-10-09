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
    let file = std::env::temp_dir().join(format!(
        "coa-companion-templates-{}.sql",
        std::process::id()
    ));
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
    run_seed(
        db,
        "(SELECT IFNULL(MAX(guid),0) FROM acore_characters.characters) + 100000",
    )?;
    Ok(true)
}

/// The bot module's offline factory ships with the server package; it creates fully equipped bots straight in the
/// database while the world server is stopped (fast and safe for large batches).
pub const OFFLINE_FACTORY: &str = "Extras/CoABotTools/offline_bot_factory.py";

/// Batches at least this large are created offline when the server happens to be stopped.
pub const OFFLINE_MIN: u32 = 100;

pub fn offline_factory(root: &Path) -> Option<std::path::PathBuf> {
    let p = root.join(OFFLINE_FACTORY);
    (p.is_file()
        && root.join("Runtime/python/python.exe").is_file()
        && root.join("mysql/bin/mysql.exe").is_file()
        && root.join("mysql/admin-client.ini").is_file())
    .then_some(p)
}

/// Run the offline factory for `count` leveled bots. The database must be running and the world/auth servers stopped.
/// Returns the tail of its report. Credentials are passed as a file path only, never on the command line.
pub fn offline_create(root: &Path, log: &Path, count: u32) -> Result<String> {
    use std::process::{Command, Stdio};
    let script = offline_factory(root).ok_or_else(|| {
        crate::error::Error::Invalid("The offline bot factory is not part of this server.".into())
    })?;
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    let out = fs::File::create(log)?;
    let err = out.try_clone()?;
    let mut child = Command::new(root.join("Runtime/python/python.exe"))
        .arg(&script)
        .args(["--count", &count.to_string(), "--leveled", "--mysql-exe"])
        .arg(root.join("mysql/bin/mysql.exe"))
        .arg("--defaults-file")
        .arg(root.join("mysql/admin-client.ini"))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if started.elapsed() > std::time::Duration::from_secs(45 * 60) {
            let _ = child.kill();
            return Err(crate::error::Error::Invalid(
                "Creating the companions took too long and was stopped.".into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let text = fs::read_to_string(log).unwrap_or_default();
    let tail: Vec<&str> = text
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if status.success() {
        Ok(tail.join("\n"))
    } else {
        Err(crate::error::Error::Invalid(format!(
            "The bot factory reported an error: {}",
            tail.join(" | ")
        )))
    }
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
        assert_eq!(
            s.matches("VALUES (@G+").count(),
            s.matches("INSERT IGNORE").count(),
            "every row takes its guid from @G"
        );
        assert!(s.contains("@ACC"));
        assert!(!s.contains("INSERT INTO "), "rows that exist are kept");
    }
}
