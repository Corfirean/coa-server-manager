//! Companion (bot) support data. The bot module creates each bot by copying a level 80 "template" character of the
//! same class, so a server with no such characters can not create any bot. A freshly installed server ships with a
//! small set (one per CoA class); older or imported servers can have it added on request.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
use crate::fsx;

/// Owner of the template characters. Not account 1 (the bot module skips it) and not a bot-host account; no such
/// account exists in the login database, so nobody can ever log in to these characters.
pub const TEMPLATE_ACCOUNT: u32 = 900_000;

const SQL: &str = include_str!("../data/companion-templates.sql");

/// The seed script with its two variables set: `base_guid` is added to every character guid.
pub fn seed_sql(base_guid: &str) -> String {
    format!("SET @G = {base_guid}; SET @ACC = {TEMPLATE_ACCOUNT};\n{SQL}")
}

/// Number of characters the bot module could use as templates.
pub fn template_count(db: &Db) -> Result<u32> {
    template_count_for_schema(db, "acore_characters")
}

pub fn template_count_for_schema(db: &Db, chars_db: &str) -> Result<u32> {
    let n = db.query(&format!("SELECT COUNT(*) FROM {chars_db}.characters WHERE account<>1 AND level>=80;"))?;
    Ok(n.trim().parse().unwrap_or(0))
}

fn run_seed(db: &Db, chars_db: &str, base_guid: &str) -> Result<()> {
    let file = std::env::temp_dir().join(format!(
        "coa-companion-templates-{}.sql",
        std::process::id()
    ));
    fs::write(&file, seed_sql(base_guid))?;
    let r = db.run_sql_file(chars_db, &file);
    let _ = fs::remove_file(&file);
    r
}

/// Fresh databases (release tooling): the original guids.
pub fn seed_fresh(db: &Db) -> Result<()> {
    run_seed(db, "acore_characters", "0")
}

/// An existing server without templates: add them far above the highest guid in use, so a running world can never
/// hand a new character one of their guids. Returns whether anything was added.
pub fn ensure_templates(db: &Db) -> Result<bool> {
    ensure_templates_for_schema(db, "acore_characters")
}

pub fn ensure_templates_for_schema(db: &Db, chars_db: &str) -> Result<bool> {
    if template_count_for_schema(db, chars_db)? > 0 {
        return Ok(false);
    }
    run_seed(
        db,
        chars_db,
        &format!("(SELECT IFNULL(MAX(guid),0) FROM {chars_db}.characters) + 100000"),
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

/// Real runtime wildcard archetype metadata directly from coa_build_creator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WildcardArchetypeDto {
    pub id: String,
    pub name: String,
    pub roles: u32,
    pub primary_stat: u32,
    pub author: String,
    pub difficulty: u32,
    pub category: u32,
}

/// Query coa_build_creator in the target characters DB for curated and player-authored archetypes.
pub fn list_wildcard_archetypes(db: &Db, chars_db: &str) -> Result<Vec<WildcardArchetypeDto>> {
    let sql = format!(
        "SELECT id, name, roles, primary_stat, author, difficulty, category \
         FROM {chars_db}.coa_build_creator \
         WHERE needs_repairs = 0 \
         ORDER BY upvotes DESC, name ASC \
         LIMIT 100;"
    );

    let mut archetypes = Vec::new();
    if let Ok(out) = db.query(&sql) {
        for line in out.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 7 {
                archetypes.push(WildcardArchetypeDto {
                    id: parts[0].trim().to_string(),
                    name: parts[1].trim().to_string(),
                    roles: parts[2].trim().parse().unwrap_or(8),
                    primary_stat: parts[3].trim().parse().unwrap_or(0),
                    author: parts[4].trim().to_string(),
                    difficulty: parts[5].trim().parse().unwrap_or(0),
                    category: parts[6].trim().parse().unwrap_or(0),
                });
            }
        }
    }

    // Curated fallback if DB query returned zero rows (e.g. initial setup)
    if archetypes.is_empty() {
        archetypes = vec![
            WildcardArchetypeDto {
                id: "b9bc2e11-cd62-4d37-b05a-fff2a2ada475".into(),
                name: "Kinyaz Druid Tank Bear".into(),
                roles: 2,
                primary_stat: 0, // Strength
                author: "Official".into(),
                difficulty: 1,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "c556625e-b5f1-40a9-b545-2bcaef883350".into(),
                name: "Holy Paladin".into(),
                roles: 4,
                primary_stat: 3, // Intellect
                author: "Official".into(),
                difficulty: 1,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "05add9af-94ee-4094-b091-f9afcb79c102".into(),
                name: "Idum's Fire Sage".into(),
                roles: 4,
                primary_stat: 4, // Spirit
                author: "Idum".into(),
                difficulty: 2,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "aaf370e5-fa02-4a02-abee-01cc4058aace".into(),
                name: "Idum's Forest Warden".into(),
                roles: 4,
                primary_stat: 4, // Spirit
                author: "Idum".into(),
                difficulty: 2,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "20f08cc1-b2c4-47ca-b5b1-d5cbcbd3d2f8".into(),
                name: "Blood DK DPS".into(),
                roles: 8,
                primary_stat: 0, // Strength
                author: "Official".into(),
                difficulty: 1,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "8f1df486-d97f-4424-9b2f-8a032d88bbd6".into(),
                name: "Bleed Berserker".into(),
                roles: 8,
                primary_stat: 1, // Agility
                author: "Official".into(),
                difficulty: 2,
                category: 1,
            },
            WildcardArchetypeDto {
                id: "45fbe2e6-231a-4a25-827a-8d7b3da4501a".into(),
                name: "Mur-Lock".into(),
                roles: 8,
                primary_stat: 3, // Intellect
                author: "Official".into(),
                difficulty: 2,
                category: 1,
            },
        ];
    }

    Ok(archetypes)
}

/// Realm-aware pending companion deployment job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionsPendingJob {
    pub job_id: String,
    pub server_instance_id: String,
    pub realm: String,
    pub count: u32,
    pub level: Option<u8>,
    pub class_id: Option<u8>,
    pub spec_id: Option<u32>,
    pub build_id: Option<String>,
    pub requested_operation: String,
    pub created_at: u64,
    pub state: String, // "pending", "in_progress", "completed", "failed"
    pub completed_count: u32,
    pub failed_count: u32,
    pub last_error: Option<String>,
    pub schema_version: u32,
}

pub fn pending_job_path(meta_dir: &Path, realm: &str) -> PathBuf {
    meta_dir.join(format!("companions.pending.{realm}.json"))
}

pub fn save_pending_job(meta_dir: &Path, realm: &str, job: &CompanionsPendingJob) -> Result<()> {
    fsx::atomic_write_json(&pending_job_path(meta_dir, realm), job)
}

pub fn load_pending_job(meta_dir: &Path, realm: &str) -> Result<Option<CompanionsPendingJob>> {
    let p = pending_job_path(meta_dir, realm);
    if !p.exists() {
        return Ok(None);
    }
    match fsx::read_json::<CompanionsPendingJob>(&p) {
        Ok(job) => Ok(Some(job)),
        Err(e) => {
            tracing::warn!("Corrupted pending job file {:?}: {e}", p);
            Ok(None)
        }
    }
}

pub fn clear_pending_job(meta_dir: &Path, realm: &str) -> Result<()> {
    let p = pending_job_path(meta_dir, realm);
    if p.exists() {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
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

    #[test]
    fn pending_jobs_roundtrip_and_are_isolated_by_realm() {
        let temp = std::env::temp_dir().join(format!("coa-test-jobs-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&temp).unwrap();

        let job_wc = CompanionsPendingJob {
            job_id: "test-wc-1".into(),
            server_instance_id: "inst-1".into(),
            realm: "wildcard".into(),
            count: 50,
            level: Some(60),
            class_id: Some(10),
            spec_id: Some(1),
            build_id: Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475".into()),
            requested_operation: "spawn".into(),
            created_at: 1000,
            state: "pending".into(),
            completed_count: 0,
            failed_count: 0,
            last_error: None,
            schema_version: 1,
        };

        let job_wcr = CompanionsPendingJob {
            job_id: "test-wcr-1".into(),
            server_instance_id: "inst-1".into(),
            realm: "reborn".into(),
            count: 10,
            level: Some(60),
            class_id: Some(7),
            spec_id: Some(4),
            build_id: None,
            requested_operation: "spawn".into(),
            created_at: 1001,
            state: "pending".into(),
            completed_count: 0,
            failed_count: 0,
            last_error: None,
            schema_version: 1,
        };

        save_pending_job(&temp, "wildcard", &job_wc).unwrap();
        save_pending_job(&temp, "reborn", &job_wcr).unwrap();

        let loaded_wc = load_pending_job(&temp, "wildcard").unwrap().expect("wc job missing");
        let loaded_wcr = load_pending_job(&temp, "reborn").unwrap().expect("wcr job missing");
        let loaded_coa = load_pending_job(&temp, "coa").unwrap();

        assert_eq!(loaded_wc, job_wc);
        assert_eq!(loaded_wcr, job_wcr);
        assert!(loaded_coa.is_none());

        clear_pending_job(&temp, "wildcard").unwrap();
        assert!(load_pending_job(&temp, "wildcard").unwrap().is_none());
        assert!(load_pending_job(&temp, "reborn").unwrap().is_some());

        let _ = std::fs::remove_dir_all(temp);
    }
}
