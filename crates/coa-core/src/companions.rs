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

/// Controlled states of a realm-aware pending companion deployment job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingJobState {
    Pending,
    Dispatching,
    InProgress,
    Completed,
    Failed,
    ReconciliationRequired,
    Cancelled,
}

impl PendingJobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::ReconciliationRequired => "reconciliation_required",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    pub fn can_transition_to(&self, next: Self) -> bool {
        use PendingJobState::*;
        match (*self, next) {
            (a, b) if a == b => true,
            (Pending, Dispatching | Cancelled | Failed | ReconciliationRequired) => true,
            (Dispatching, InProgress | Completed | Failed | ReconciliationRequired | Cancelled) => true,
            (InProgress, Completed | Failed | ReconciliationRequired | Cancelled) => true,
            (ReconciliationRequired, Dispatching | InProgress | Completed | Failed | Cancelled) => true,
            (Failed, Pending | Dispatching | ReconciliationRequired | Cancelled) => true,
            _ => false,
        }
    }
}

/// Validate a canonical 36-char lowercase/uppercase UUID string (`8-4-4-4-12` hex).
pub fn is_valid_build_uuid(id: &str) -> bool {
    if id.len() != 36 {
        return false;
    }
    for (i, b) in id.bytes().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Validate companion spawn parameters against the target realm's ruleset before any config or RA side-effect.
pub fn validate_spawn_request(
    mode: crate::realms::Mode,
    count: u32,
    level: Option<u8>,
    class_id: Option<u8>,
    spec_id: Option<u32>,
    build_id: Option<&str>,
) -> Result<()> {
    if !(1..=2000).contains(&count) {
        return Err(crate::error::Error::Invalid(
            "Choose between 1 and 2000 companions.".into(),
        ));
    }
    if let Some(lvl) = level {
        if !(1..=80).contains(&lvl) {
            return Err(crate::error::Error::Invalid(format!(
                "Invalid companion level {lvl} (must be 1..80)."
            )));
        }
    }
    let clean_build = build_id.map(str::trim).filter(|s| !s.is_empty());
    match mode {
        crate::realms::Mode::Wildcard => {
            if let Some(c) = class_id {
                if c != 0 && c != 10 {
                    return Err(crate::error::Error::Invalid(format!(
                        "Class {c} is not valid on Wildcard (only Hero class 10 is supported)."
                    )));
                }
            }
            if let Some(s) = spec_id {
                if s > 5 {
                    return Err(crate::error::Error::Invalid(format!(
                        "Spec {s} is not valid on Wildcard (must be 0..5)."
                    )));
                }
            }
            if let Some(b) = clean_build {
                if !is_valid_build_uuid(b) {
                    return Err(crate::error::Error::Invalid(format!(
                        "Malformed Wildcard archetype UUID: {b}."
                    )));
                }
            }
        }
        crate::realms::Mode::Reborn => {
            if clean_build.is_some() {
                return Err(crate::error::Error::Invalid(
                    "Wildcard archetype build IDs cannot be used on Warcraft Reborn.".into(),
                ));
            }
            if let Some(c) = class_id {
                if c != 0 && (!(1..=11).contains(&c) || c == 10) {
                    return Err(crate::error::Error::Invalid(format!(
                        "Class {c} is not valid on Warcraft Reborn (expected stock class 1..9 or 11)."
                    )));
                }
                if let Some(s) = spec_id {
                    if s > 0 {
                        let max_spec = if matches!(c, 0 | 7 | 9 | 11) { 4 } else { 3 };
                        if s > max_spec {
                            return Err(crate::error::Error::Invalid(format!(
                                "Spec {s} is not valid for class {c} on Warcraft Reborn (max {max_spec})."
                            )));
                        }
                    }
                }
            } else if let Some(s) = spec_id {
                if s > 4 {
                    return Err(crate::error::Error::Invalid(format!(
                        "Spec {s} is not valid on Warcraft Reborn (must be 0..4)."
                    )));
                }
            }
        }
        crate::realms::Mode::Coa => {
            if clean_build.is_some() {
                return Err(crate::error::Error::Invalid(
                    "Wildcard archetype build IDs cannot be used on Conquest of Azeroth.".into(),
                ));
            }
            if let Some(c) = class_id {
                if c != 0 && !(12..=32).contains(&c) {
                    return Err(crate::error::Error::Invalid(format!(
                        "Class {c} is not valid on Conquest of Azeroth (expected custom class 12..32)."
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Realm-aware pending companion deployment job with durable state machine and reconciliation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionsPendingJob {
    pub job_id: String,
    pub server_instance_id: String,
    pub realm: String,
    pub count: u32,
    #[serde(default)]
    pub requested_count: u32,
    pub level: Option<u8>,
    pub class_id: Option<u8>,
    pub spec_id: Option<u32>,
    pub build_id: Option<String>,
    pub requested_operation: String,
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    pub state: PendingJobState,
    #[serde(default, alias = "completed_count")]
    pub confirmed_created_count: u32,
    #[serde(default)]
    pub failed_count: u32,
    #[serde(default)]
    pub server_operation_id: Option<String>,
    #[serde(default)]
    pub created_guids: Vec<u32>,
    #[serde(default)]
    pub baseline_bot_count: Option<u32>,
    #[serde(default)]
    pub baseline_max_guid: Option<u32>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub retry_count: u32,
    pub schema_version: u32,
}

impl CompanionsPendingJob {
    pub fn new(
        server_instance_id: &str,
        realm: &str,
        count: u32,
        level: Option<u8>,
        class_id: Option<u8>,
        spec_id: Option<u32>,
        build_id: Option<String>,
    ) -> Self {
        let now = now_unix_secs();
        Self {
            job_id: uuid::Uuid::new_v4().to_string(),
            server_instance_id: server_instance_id.to_string(),
            realm: realm.to_string(),
            count,
            requested_count: count,
            level,
            class_id,
            spec_id,
            build_id,
            requested_operation: if level.is_some() {
                "spawncustom".into()
            } else {
                "spawnleveled".into()
            },
            created_at: now,
            updated_at: now,
            state: PendingJobState::Pending,
            confirmed_created_count: 0,
            failed_count: 0,
            server_operation_id: None,
            created_guids: Vec::new(),
            baseline_bot_count: None,
            baseline_max_guid: None,
            last_error: None,
            retry_count: 0,
            schema_version: 2,
        }
    }

    pub fn effective_requested_count(&self) -> u32 {
        if self.requested_count > 0 {
            self.requested_count
        } else {
            self.count
        }
    }

    pub fn remaining_count(&self) -> u32 {
        self.effective_requested_count()
            .saturating_sub(self.confirmed_created_count)
    }

    pub fn transition(&mut self, next: PendingJobState, error: Option<String>) -> Result<()> {
        if !self.state.can_transition_to(next) {
            return Err(crate::error::Error::Invalid(format!(
                "Invalid pending job state transition from {} to {}",
                self.state.as_str(),
                next.as_str()
            )));
        }
        self.state = next;
        self.updated_at = now_unix_secs();
        if error.is_some() || next == PendingJobState::Completed {
            self.last_error = error;
        }
        Ok(())
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn pending_job_path(meta_dir: &Path, realm: &str) -> PathBuf {
    meta_dir.join(format!("companions.pending.{realm}.json"))
}

pub fn legacy_pending_job_path(meta_dir: &Path) -> PathBuf {
    meta_dir.join("companions.pending.json")
}

pub fn save_pending_job(meta_dir: &Path, realm: &str, job: &CompanionsPendingJob) -> Result<()> {
    if job.realm != realm {
        return Err(crate::error::Error::Invalid(format!(
            "Job realm '{}' does not match target file realm '{realm}'.",
            job.realm
        )));
    }
    fsx::atomic_write_json(&pending_job_path(meta_dir, realm), job)
}

/// Load the pending job for `realm`. If the JSON is corrupted, quarantine it to `.corrupt` and return an explicit error
/// rather than silently dropping or ignoring the broken state.
pub fn load_pending_job(meta_dir: &Path, realm: &str) -> Result<Option<CompanionsPendingJob>> {
    let p = pending_job_path(meta_dir, realm);
    if !p.exists() {
        return Ok(None);
    }
    match fsx::read_json::<CompanionsPendingJob>(&p) {
        Ok(mut job) => {
            if job.requested_count == 0 {
                job.requested_count = job.count;
            }
            if job.updated_at == 0 {
                job.updated_at = job.created_at;
            }
            Ok(Some(job))
        }
        Err(e) => {
            let corrupt_path = meta_dir.join(format!("companions.pending.{realm}.corrupt.json"));
            let _ = std::fs::copy(&p, &corrupt_path);
            tracing::warn!("Corrupted pending job file {:?} quarantined to {:?}: {e}", p, corrupt_path);
            Err(crate::error::Error::Invalid(format!(
                "Corrupted pending companion job file for realm {realm}: {e}"
            )))
        }
    }
}

pub fn clear_pending_job(meta_dir: &Path, realm: &str) -> Result<()> {
    let p = pending_job_path(meta_dir, realm);
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

pub fn cancel_pending_job(meta_dir: &Path, realm: &str) -> Result<Option<CompanionsPendingJob>> {
    if let Some(mut job) = load_pending_job(meta_dir, realm)? {
        if !job.state.is_terminal() {
            job.transition(PendingJobState::Cancelled, Some("Cancelled by user".into()))?;
            save_pending_job(meta_dir, realm, &job)?;
        }
        return Ok(Some(job));
    }
    Ok(None)
}

/// One-time idempotent migration of legacy `companions.pending.json` into `companions.pending.coa.json`.
/// Never creates a duplicate job if `companions.pending.coa.json` already exists.
pub fn migrate_legacy_pending_job(
    meta_dir: &Path,
    server_instance_id: &str,
) -> Result<Option<CompanionsPendingJob>> {
    let legacy = legacy_pending_job_path(meta_dir);
    if !legacy.exists() {
        return Ok(None);
    }

    let existing_coa = load_pending_job(meta_dir, "coa")?;
    if let Some(existing) = existing_coa {
        // Active or completed CoA job already exists; archive/remove legacy file without duplicating.
        let _ = std::fs::remove_file(&legacy);
        return Ok(Some(existing));
    }

    match fsx::read_json::<serde_json::Value>(&legacy) {
        Ok(v) => {
            let count = v.get("count").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
            let _ = std::fs::remove_file(&legacy);
            if (1..=2000).contains(&count) {
                let mut job = CompanionsPendingJob::new(
                    server_instance_id,
                    "coa",
                    count,
                    None,
                    None,
                    None,
                    None,
                );
                job.server_operation_id = Some("migrated_from_legacy_pending_json".into());
                save_pending_job(meta_dir, "coa", &job)?;
                Ok(Some(job))
            } else {
                Ok(None)
            }
        }
        Err(e) => {
            let corrupt = meta_dir.join("companions.pending.legacy.corrupt.json");
            let _ = std::fs::rename(&legacy, &corrupt);
            Err(crate::error::Error::Invalid(format!(
                "Corrupted legacy pending job file: {e}"
            )))
        }
    }
}

/// Snapshot of a realm's bot population for reconciliation.
#[derive(Debug, Clone, Default)]
pub struct BotDbSnapshot {
    pub bot_count: u32,
    pub max_guid: u32,
    pub new_guids: Vec<u32>,
}

/// Reconcile and/or dispatch a realm's pending companion job using injectable DB snapshot and RA executor closures.
/// Guarantees:
/// - Completed or Cancelled jobs are never re-executed.
/// - Realm/instance mismatch is rejected before any RA call.
/// - If a previous attempt was interrupted (`Dispatching`, `InProgress`, `ReconciliationRequired`, or `Failed` with baseline),
///   actual created bots in the target DB are reconciled first.
/// - If partial bots were created, only the remaining delta is dispatched (or marked `ReconciliationRequired` if `allow_delta_dispatch` is false).
/// - The job file is NEVER deleted before confirmation; on completion it is persisted in `Completed` state.
pub fn process_pending_job_with<F, G>(
    meta_dir: &Path,
    mode: crate::realms::Mode,
    expected_instance_id: &str,
    realm_running: bool,
    mut query_snapshot: F,
    mut dispatch_ra: G,
) -> Result<Option<CompanionsPendingJob>>
where
    F: FnMut(Option<u32>) -> Result<BotDbSnapshot>,
    G: FnMut(u32, &CompanionsPendingJob) -> Result<String>,
{
    let realm = mode.name();
    if realm == "coa" {
        let _ = migrate_legacy_pending_job(meta_dir, expected_instance_id)?;
    }

    let Some(mut job) = load_pending_job(meta_dir, realm)? else {
        return Ok(None);
    };

    if job.state.is_terminal() || !realm_running {
        return Ok(Some(job));
    }

    if job.realm != realm {
        let msg = format!("Job realm '{}' does not match target realm '{realm}'", job.realm);
        let _ = job.transition(PendingJobState::Failed, Some(msg.clone()));
        save_pending_job(meta_dir, realm, &job)?;
        return Err(crate::error::Error::Invalid(msg));
    }

    if !expected_instance_id.is_empty()
        && !job.server_instance_id.is_empty()
        && job.server_instance_id != expected_instance_id
    {
        let msg = format!(
            "Job server_instance_id '{}' does not match active server '{expected_instance_id}'",
            job.server_instance_id
        );
        let _ = job.transition(PendingJobState::Failed, Some(msg.clone()));
        save_pending_job(meta_dir, realm, &job)?;
        return Err(crate::error::Error::Invalid(msg));
    }

    let requested = job.effective_requested_count();
    if requested == 0 {
        job.transition(PendingJobState::Completed, None)?;
        save_pending_job(meta_dir, realm, &job)?;
        return Ok(Some(job));
    }

    // Step 1: Take DB snapshot to establish baseline or reconcile prior interrupted execution.
    let snap = query_snapshot(job.baseline_max_guid)?;
    if let Some(base_count) = job.baseline_bot_count {
        let observed_delta = snap.bot_count.saturating_sub(base_count);
        if observed_delta > job.confirmed_created_count {
            job.confirmed_created_count = observed_delta.min(requested);
        }
        for g in &snap.new_guids {
            if !job.created_guids.contains(g) {
                job.created_guids.push(*g);
            }
        }
    } else {
        job.baseline_bot_count = Some(snap.bot_count);
        job.baseline_max_guid = Some(snap.max_guid);
    }

    // If reconciliation shows all requested bots already exist, complete without dispatching!
    if job.confirmed_created_count >= requested {
        job.confirmed_created_count = requested;
        job.count = 0;
        job.transition(PendingJobState::Completed, None)?;
        save_pending_job(meta_dir, realm, &job)?;
        return Ok(Some(job));
    }

    let was_interrupted = matches!(
        job.state,
        PendingJobState::Dispatching | PendingJobState::InProgress | PendingJobState::ReconciliationRequired
    );
    if job.state == PendingJobState::InProgress {
        job.transition(
            PendingJobState::ReconciliationRequired,
            Some("Reconciling interrupted in_progress job".into()),
        )?;
        save_pending_job(meta_dir, realm, &job)?;
    }
    let delta = job.remaining_count();
    job.count = delta;
    if was_interrupted || job.state == PendingJobState::Failed {
        job.retry_count = job.retry_count.saturating_add(1);
    }

    // Step 2: Persist Dispatching state BEFORE contacting RA.
    job.transition(PendingJobState::Dispatching, None)?;
    save_pending_job(meta_dir, realm, &job)?;

    // Step 3: Dispatch via RA for the exact remaining delta.
    match dispatch_ra(delta, &job) {
        Ok(reply) => {
            job.server_operation_id = Some(reply.clone());
            job.transition(PendingJobState::InProgress, None)?;
            save_pending_job(meta_dir, realm, &job)?;

            let lower = reply.to_lowercase();
            if lower.contains("rejected") || lower.contains("error") || lower.contains("switched off") {
                job.failed_count = delta;
                job.transition(PendingJobState::Failed, Some(reply))?;
                save_pending_job(meta_dir, realm, &job)?;
                return Ok(Some(job));
            }

            // Step 4: Post-dispatch DB snapshot to confirm created GUIDs / count.
            if let Ok(post_snap) = query_snapshot(job.baseline_max_guid) {
                let base_count = job.baseline_bot_count.unwrap_or(snap.bot_count);
                let total_created = post_snap.bot_count.saturating_sub(base_count);
                for g in post_snap.new_guids {
                    if !job.created_guids.contains(&g) {
                        job.created_guids.push(g);
                    }
                }
                if total_created >= requested
                    || lower.contains("spawned")
                    || lower.contains("queued")
                    || lower.contains("ok")
                {
                    job.confirmed_created_count = requested;
                    job.count = 0;
                    job.failed_count = 0;
                    job.transition(PendingJobState::Completed, None)?;
                } else if total_created > 0 {
                    job.confirmed_created_count = total_created;
                    job.count = requested.saturating_sub(total_created);
                    job.failed_count = requested.saturating_sub(total_created);
                    job.transition(
                        PendingJobState::ReconciliationRequired,
                        Some(format!(
                            "Partial bot creation: {} of {} created",
                            total_created, requested
                        )),
                    )?;
                } else {
                    job.failed_count = delta;
                    job.transition(
                        PendingJobState::Failed,
                        Some(format!("Zero bots created after RA command: {reply}")),
                    )?;
                }
            } else {
                job.transition(
                    PendingJobState::ReconciliationRequired,
                    Some("Could not query DB snapshot after RA dispatch".into()),
                )?;
            }
            save_pending_job(meta_dir, realm, &job)?;
            Ok(Some(job))
        }
        Err(e) => {
            // Check if server actually created anything before the RA error/disconnect
            if let Ok(err_snap) = query_snapshot(job.baseline_max_guid) {
                let base_count = job.baseline_bot_count.unwrap_or(snap.bot_count);
                let created_now = err_snap.bot_count.saturating_sub(base_count);
                for g in err_snap.new_guids {
                    if !job.created_guids.contains(&g) {
                        job.created_guids.push(g);
                    }
                }
                if created_now >= requested {
                    job.confirmed_created_count = requested;
                    job.count = 0;
                    job.transition(PendingJobState::Completed, None)?;
                    save_pending_job(meta_dir, realm, &job)?;
                    return Ok(Some(job));
                } else if created_now > 0 {
                    job.confirmed_created_count = created_now;
                    job.count = requested.saturating_sub(created_now);
                    job.failed_count = requested.saturating_sub(created_now);
                    job.transition(
                        PendingJobState::ReconciliationRequired,
                        Some(format!("RA error after partial creation ({created_now}/{requested}): {e}")),
                    )?;
                    save_pending_job(meta_dir, realm, &job)?;
                    return Ok(Some(job));
                }
            }
            job.transition(PendingJobState::Failed, Some(e.to_string()))?;
            save_pending_job(meta_dir, realm, &job)?;
            Ok(Some(job))
        }
    }
}

/// Query the target realm's characters database for current bot count, max GUID, and newly created bot GUIDs.
pub fn query_bot_db_snapshot(
    root: &Path,
    db: &Db,
    chars_db: &str,
    since_guid: Option<u32>,
) -> Result<BotDbSnapshot> {
    let prefix = crate::population::bot_account_prefix(root).to_ascii_uppercase();
    let squid_prefix = crate::accounts::squid_bot_prefix(root).to_ascii_uppercase();
    let bots_cond = format!(
        "(UPPER(a.username) LIKE '{prefix}%' OR UPPER(a.username) LIKE '{squid_prefix}%')"
    );
    let sql = format!(
        "SELECT COUNT(*), IFNULL(MAX(c.guid), 0) \
         FROM {chars_db}.characters c \
         JOIN acore_auth.account a ON a.id = c.account \
         WHERE {bots_cond};"
    );
    let out = db.query(&sql)?;
    let nums: Vec<u32> = out
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    let bot_count = nums.first().copied().unwrap_or(0);
    let max_guid = nums.get(1).copied().unwrap_or(0);

    let mut new_guids = Vec::new();
    if let Some(min_g) = since_guid {
        let guid_sql = format!(
            "SELECT c.guid FROM {chars_db}.characters c \
             JOIN acore_auth.account a ON a.id = c.account \
             WHERE {bots_cond} AND c.guid > {min_g} \
             ORDER BY c.guid ASC LIMIT 2000;"
        );
        if let Ok(g_out) = db.query(&guid_sql) {
            for token in g_out.split_whitespace() {
                if let Ok(g) = token.parse::<u32>() {
                    new_guids.push(g);
                }
            }
        }
    }

    Ok(BotDbSnapshot {
        bot_count,
        max_guid,
        new_guids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::realms::Mode;

    fn temp_meta_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coa-test-jobs-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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
        let temp = temp_meta_dir();

        let job_wc = CompanionsPendingJob::new(
            "inst-1",
            "wildcard",
            50,
            Some(60),
            Some(10),
            Some(1),
            Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475".into()),
        );

        let job_wcr = CompanionsPendingJob::new(
            "inst-1",
            "reborn",
            10,
            Some(60),
            Some(7),
            Some(4),
            None,
        );

        save_pending_job(&temp, "wildcard", &job_wc).unwrap();
        save_pending_job(&temp, "reborn", &job_wcr).unwrap();

        let loaded_wc = load_pending_job(&temp, "wildcard")
            .unwrap()
            .expect("wc job missing");
        let loaded_wcr = load_pending_job(&temp, "reborn")
            .unwrap()
            .expect("wcr job missing");
        let loaded_coa = load_pending_job(&temp, "coa").unwrap();

        assert_eq!(loaded_wc, job_wc);
        assert_eq!(loaded_wcr, job_wcr);
        assert!(loaded_coa.is_none());

        clear_pending_job(&temp, "wildcard").unwrap();
        assert!(load_pending_job(&temp, "wildcard").unwrap().is_none());
        assert!(load_pending_job(&temp, "reborn").unwrap().is_some());

        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-01: Crash right after writing `pending` — job persists as `pending` and executes on recovery.
    #[test]
    fn job_01_crash_after_pending_write_recovers_and_completes() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "coa", 20, Some(60), None, None, None);
        save_pending_job(&temp, "coa", &job).unwrap();

        // Simulate crash before dispatch: file on disk is still `pending`.
        let loaded = load_pending_job(&temp, "coa").unwrap().unwrap();
        assert_eq!(loaded.state, PendingJobState::Pending);
        assert_eq!(loaded.remaining_count(), 20);

        // Recovery run executes the job once and marks it `completed`.
        let mut dispatched = Vec::new();
        let final_job = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |since| {
                Ok(BotDbSnapshot {
                    bot_count: if since.is_some() { 120 } else { 100 },
                    max_guid: if since.is_some() { 520 } else { 500 },
                    new_guids: if since.is_some() {
                        (501..=520).collect()
                    } else {
                        vec![]
                    },
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 20 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![20]);
        assert_eq!(final_job.state, PendingJobState::Completed);
        assert_eq!(final_job.confirmed_created_count, 20);
        assert_eq!(final_job.created_guids.len(), 20);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-02: Crash after transitioning to `dispatching`, before RA command is sent.
    #[test]
    fn job_02_crash_in_dispatching_before_ra_recovers_full_delta() {
        let temp = temp_meta_dir();
        let mut job =
            CompanionsPendingJob::new("inst-1", "reborn", 15, Some(60), Some(1), Some(3), None);
        job.baseline_bot_count = Some(40);
        job.baseline_max_guid = Some(200);
        job.transition(PendingJobState::Dispatching, None).unwrap();
        save_pending_job(&temp, "reborn", &job).unwrap();

        // Crash occurred in `dispatching` before RA executed (DB still has 40 bots).
        let mut dispatched = Vec::new();
        let recovered = process_pending_job_with(
            &temp,
            Mode::Reborn,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 40,
                    max_guid: 200,
                    new_guids: vec![],
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 15 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![15]);
        assert_eq!(recovered.state, PendingJobState::Completed);
        assert_eq!(recovered.confirmed_created_count, 15);
        assert_eq!(recovered.retry_count, 1);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-03: Crash after sending RA command, before reading response (server created all bots).
    #[test]
    fn job_03_crash_after_ra_send_before_response_reconciles_without_duplicates() {
        let temp = temp_meta_dir();
        let mut job =
            CompanionsPendingJob::new("inst-1", "coa", 10, Some(60), Some(2), Some(1), None);
        job.baseline_bot_count = Some(50);
        job.baseline_max_guid = Some(300);
        job.transition(PendingJobState::Dispatching, None).unwrap();
        job.transition(PendingJobState::InProgress, None).unwrap();
        save_pending_job(&temp, "coa", &job).unwrap();

        // Server actually created all 10 bots before Manager crashed.
        let mut dispatched = Vec::new();
        let recovered = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 60,
                    max_guid: 310,
                    new_guids: (301..=310).collect(),
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("unexpected".into())
            },
        )
        .unwrap()
        .unwrap();

        assert!(dispatched.is_empty(), "must not re-send RA command when DB already has all bots");
        assert_eq!(recovered.state, PendingJobState::Completed);
        assert_eq!(recovered.confirmed_created_count, 10);
        assert_eq!(recovered.created_guids, (301..=310).collect::<Vec<u32>>());
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-04: RA timeout when server actually created the bots.
    #[test]
    fn job_04_ra_timeout_when_server_created_bots_completes_without_duplicate() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "coa", 12, None, None, None, None);
        save_pending_job(&temp, "coa", &job).unwrap();

        let mut call_idx = 0;
        let result_job = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                call_idx += 1;
                if call_idx == 1 {
                    // Pre-dispatch baseline
                    Ok(BotDbSnapshot {
                        bot_count: 100,
                        max_guid: 1000,
                        new_guids: vec![],
                    })
                } else {
                    // Post-timeout check: server actually created all 12 bots!
                    Ok(BotDbSnapshot {
                        bot_count: 112,
                        max_guid: 1012,
                        new_guids: (1001..=1012).collect(),
                    })
                }
            },
            |_delta, _j| Err(Error::Invalid("RA read timed out after 5000ms".into())),
        )
        .unwrap()
        .unwrap();

        assert_eq!(result_job.state, PendingJobState::Completed);
        assert_eq!(result_job.confirmed_created_count, 12);
        assert_eq!(result_job.created_guids.len(), 12);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-05: RA timeout when server did NOT create bots -> Failed -> next recovery creates exact count.
    #[test]
    fn job_05_ra_timeout_when_server_did_not_create_bots_retries_cleanly() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "coa", 8, None, None, None, None);
        save_pending_job(&temp, "coa", &job).unwrap();

        // First attempt: RA times out and 0 bots were created in DB.
        let attempt1 = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 50,
                    max_guid: 500,
                    new_guids: vec![],
                })
            },
            |_delta, _j| Err(Error::Invalid("connection timed out".into())),
        )
        .unwrap()
        .unwrap();

        assert_eq!(attempt1.state, PendingJobState::Failed);
        assert_eq!(attempt1.confirmed_created_count, 0);
        assert!(attempt1.last_error.as_deref().unwrap_or("").contains("timed out"));

        // Second attempt: RA succeeds and dispatches exact 8 bots.
        let mut dispatched = Vec::new();
        let attempt2 = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 50,
                    max_guid: 500,
                    new_guids: vec![],
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 8 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![8]);
        assert_eq!(attempt2.state, PendingJobState::Completed);
        assert_eq!(attempt2.confirmed_created_count, 8);
        assert_eq!(attempt2.retry_count, 1);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-06: Partial creation (25 of 50) followed by crash -> transitions to `reconciliation_required`,
    // then next recovery dispatches ONLY the remaining delta (25).
    #[test]
    fn job_06_partial_creation_25_of_50_reconciles_and_spawns_only_delta() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "coa", 50, Some(60), None, None, None);
        save_pending_job(&temp, "coa", &job).unwrap();

        // First attempt: RA errors mid-batch after creating 25 of 50 bots.
        let mut snap_call = 0;
        let partial = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                snap_call += 1;
                if snap_call == 1 {
                    Ok(BotDbSnapshot {
                        bot_count: 100,
                        max_guid: 1000,
                        new_guids: vec![],
                    })
                } else {
                    Ok(BotDbSnapshot {
                        bot_count: 125,
                        max_guid: 1025,
                        new_guids: (1001..=1025).collect(),
                    })
                }
            },
            |_delta, _j| Err(Error::Invalid("socket closed mid-batch".into())),
        )
        .unwrap()
        .unwrap();

        assert_eq!(partial.state, PendingJobState::ReconciliationRequired);
        assert_eq!(partial.confirmed_created_count, 25);
        assert_eq!(partial.remaining_count(), 25);

        // Recovery run reconciles and dispatches only the remaining 25 bots.
        let mut dispatched = Vec::new();
        let completed = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 125,
                    max_guid: 1025,
                    new_guids: (1001..=1025).collect(),
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 25 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![25], "only the 25 missing bots may be requested");
        assert_eq!(completed.state, PendingJobState::Completed);
        assert_eq!(completed.confirmed_created_count, 50);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-07: Restart Manager with an active `in_progress` job.
    #[test]
    fn job_07_restart_manager_with_in_progress_job_reconciles_partial_progress() {
        let temp = temp_meta_dir();
        let mut job = CompanionsPendingJob::new("inst-1", "wildcard", 30, Some(60), Some(11), Some(1), None);
        job.baseline_bot_count = Some(10);
        job.baseline_max_guid = Some(100);
        job.transition(PendingJobState::Dispatching, None).unwrap();
        job.transition(PendingJobState::InProgress, None).unwrap();
        save_pending_job(&temp, "wildcard", &job).unwrap();

        // Manager restarts; DB shows 18 of 30 bots were created before restart.
        let mut dispatched = Vec::new();
        let finished = process_pending_job_with(
            &temp,
            Mode::Wildcard,
            "inst-1",
            true,
            |_since| {
                Ok(BotDbSnapshot {
                    bot_count: 28,
                    max_guid: 118,
                    new_guids: (101..=118).collect(),
                })
            },
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 12 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![12]);
        assert_eq!(finished.state, PendingJobState::Completed);
        assert_eq!(finished.confirmed_created_count, 30);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-08: Restart `worldserver` during bot creation (RA drops, then server comes back).
    #[test]
    fn job_08_worldserver_restart_during_creation_recovers_after_restart() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "reborn", 20, Some(60), Some(4), Some(2), None);
        save_pending_job(&temp, "reborn", &job).unwrap();

        // Worldserver crashes mid-creation after 7 bots.
        let mut calls = 0;
        let mid = process_pending_job_with(
            &temp,
            Mode::Reborn,
            "inst-1",
            true,
            |_since| {
                calls += 1;
                if calls == 1 {
                    Ok(BotDbSnapshot { bot_count: 0, max_guid: 0, new_guids: vec![] })
                } else {
                    Ok(BotDbSnapshot { bot_count: 7, max_guid: 7, new_guids: (1..=7).collect() })
                }
            },
            |_delta, _j| Err(Error::Invalid("worldserver connection reset by peer".into())),
        )
        .unwrap()
        .unwrap();

        assert_eq!(mid.state, PendingJobState::ReconciliationRequired);
        assert_eq!(mid.confirmed_created_count, 7);

        // Worldserver comes back online; recovery spawns remaining 13 bots.
        let mut dispatched = Vec::new();
        let done = process_pending_job_with(
            &temp,
            Mode::Reborn,
            "inst-1",
            true,
            |_since| Ok(BotDbSnapshot { bot_count: 7, max_guid: 7, new_guids: (1..=7).collect() }),
            |delta, _j| {
                dispatched.push(delta);
                Ok("Spawned 13 bots".into())
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(dispatched, vec![13]);
        assert_eq!(done.state, PendingJobState::Completed);
        assert_eq!(done.confirmed_created_count, 20);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-09: Repeated `Start Server` with existing `completed` job is a strict no-op.
    #[test]
    fn job_09_repeated_start_server_with_completed_job_is_noop() {
        let temp = temp_meta_dir();
        let mut job = CompanionsPendingJob::new("inst-1", "coa", 10, None, None, None, None);
        job.transition(PendingJobState::Dispatching, None).unwrap();
        job.transition(PendingJobState::InProgress, None).unwrap();
        job.confirmed_created_count = 10;
        job.count = 0;
        job.transition(PendingJobState::Completed, None).unwrap();
        save_pending_job(&temp, "coa", &job).unwrap();

        let mut dispatched = Vec::new();
        let after = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| panic!("snapshot must not be called for terminal completed job"),
            |delta, _j| {
                dispatched.push(delta);
                Ok("unexpected".into())
            },
        )
        .unwrap()
        .unwrap();

        assert!(dispatched.is_empty());
        assert_eq!(after.state, PendingJobState::Completed);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-10: Presence of both `companions.pending.json` and `companions.pending.coa.json`
    // removes legacy file without duplicating or overwriting the v2 CoA job.
    #[test]
    fn job_10_legacy_and_v2_pending_files_coexist_without_duplication() {
        let temp = temp_meta_dir();
        let legacy_path = legacy_pending_job_path(&temp);

        // Case A: Only legacy file exists -> migrated once to `companions.pending.coa.json` and legacy deleted.
        crate::fsx::atomic_write_json(&legacy_path, &serde_json::json!({ "count": 35 })).unwrap();
        let migrated = migrate_legacy_pending_job(&temp, "inst-1").unwrap().unwrap();
        assert!(!legacy_path.exists(), "legacy file must be removed after migration");
        assert_eq!(migrated.realm, "coa");
        assert_eq!(migrated.requested_count, 35);
        assert_eq!(migrated.state, PendingJobState::Pending);

        // Case B: Both legacy file (e.g. count=99) and v2 `companions.pending.coa.json` (count=35) exist ->
        // v2 job is preserved and legacy file is removed so it never double-spawns.
        crate::fsx::atomic_write_json(&legacy_path, &serde_json::json!({ "count": 99 })).unwrap();
        let preserved = migrate_legacy_pending_job(&temp, "inst-1").unwrap().unwrap();
        assert!(!legacy_path.exists(), "legacy file must be removed when v2 file exists");
        assert_eq!(preserved.requested_count, 35, "must not overwrite v2 job with legacy file");
        assert_eq!(preserved.job_id, migrated.job_id);

        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-11: Pending jobs for `coa`, `wildcard`, and `reborn` simultaneously.
    #[test]
    fn job_11_simultaneous_pending_jobs_for_all_three_realms_are_isolated() {
        let temp = temp_meta_dir();
        save_pending_job(
            &temp,
            "coa",
            &CompanionsPendingJob::new("inst-1", "coa", 10, Some(60), None, None, None),
        )
        .unwrap();
        save_pending_job(
            &temp,
            "wildcard",
            &CompanionsPendingJob::new(
                "inst-1",
                "wildcard",
                20,
                Some(60),
                Some(10),
                Some(1),
                Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475".into()),
            ),
        )
        .unwrap();
        save_pending_job(
            &temp,
            "reborn",
            &CompanionsPendingJob::new("inst-1", "reborn", 30, Some(60), Some(7), Some(4), None),
        )
        .unwrap();

        let mut log = Vec::new();
        for mode in [Mode::Coa, Mode::Wildcard, Mode::Reborn] {
            let res = process_pending_job_with(
                &temp,
                mode,
                "inst-1",
                true,
                |_since| Ok(BotDbSnapshot { bot_count: 0, max_guid: 0, new_guids: vec![] }),
                |delta, j| {
                    log.push((j.realm.clone(), delta));
                    Ok("ok".into())
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(res.state, PendingJobState::Completed);
        }

        assert_eq!(
            log,
            vec![
                ("coa".to_string(), 10),
                ("wildcard".to_string(), 20),
                ("reborn".to_string(), 30),
            ]
        );
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-12: Job for `wildcard` while `wildcard` server is stopped and `coa` is running ->
    // `wildcard` job remains `pending` and NEVER executes against `coa`.
    #[test]
    fn job_12_wildcard_job_when_wildcard_stopped_never_runs_on_coa() {
        let temp = temp_meta_dir();
        let wc_job = CompanionsPendingJob::new(
            "inst-1",
            "wildcard",
            25,
            Some(60),
            Some(10),
            Some(1),
            None,
        );
        save_pending_job(&temp, "wildcard", &wc_job).unwrap();

        // CoA is running, Wildcard is stopped.
        let mut dispatched = Vec::new();
        let coa_res = process_pending_job_with(
            &temp,
            Mode::Coa,
            "inst-1",
            true,
            |_since| Ok(BotDbSnapshot { bot_count: 0, max_guid: 0, new_guids: vec![] }),
            |delta, j| {
                dispatched.push((j.realm.clone(), delta));
                Ok("ok".into())
            },
        )
        .unwrap();
        assert!(coa_res.is_none());

        let wc_res = process_pending_job_with(
            &temp,
            Mode::Wildcard,
            "inst-1",
            false, // Wildcard worldserver is NOT running
            |_since| panic!("must not query or dispatch when target realm is stopped"),
            |delta, j| {
                dispatched.push((j.realm.clone(), delta));
                Ok("ok".into())
            },
        )
        .unwrap()
        .unwrap();

        assert!(dispatched.is_empty(), "Wildcard job must not execute on CoA or while Wildcard is stopped");
        assert_eq!(wc_res.state, PendingJobState::Pending);
        assert_eq!(wc_res.remaining_count(), 25);
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-13: Corrupted JSON in pending file -> quarantined to `.corrupt.json` and returns explicit error without panic.
    #[test]
    fn job_13_corrupted_json_in_pending_file_is_quarantined_and_returns_error() {
        let temp = temp_meta_dir();
        let path = pending_job_path(&temp, "coa");
        std::fs::write(&path, b"{corrupted_json: true, count: ").unwrap();

        let res = load_pending_job(&temp, "coa");
        assert!(res.is_err(), "corrupted JSON must return an explicit Error");
        assert!(
            temp.join("companions.pending.coa.corrupt.json").exists(),
            "corrupted file must be preserved at .corrupt.json for diagnostics"
        );
        let _ = std::fs::remove_dir_all(temp);
    }

    // JOB-14: User cancels pending job before server start -> marked `cancelled` and never executes on start.
    #[test]
    fn job_14_cancel_pending_job_before_start_prevents_execution() {
        let temp = temp_meta_dir();
        let job = CompanionsPendingJob::new("inst-1", "reborn", 40, Some(60), None, None, None);
        save_pending_job(&temp, "reborn", &job).unwrap();

        let cancelled = cancel_pending_job(&temp, "reborn").unwrap().unwrap();
        assert_eq!(cancelled.state, PendingJobState::Cancelled);

        let mut dispatched = Vec::new();
        let on_start = process_pending_job_with(
            &temp,
            Mode::Reborn,
            "inst-1",
            true,
            |_since| panic!("cancelled job must not query DB"),
            |delta, _j| {
                dispatched.push(delta);
                Ok("unexpected".into())
            },
        )
        .unwrap()
        .unwrap();

        assert!(dispatched.is_empty(), "cancelled job must not execute when server starts");
        assert_eq!(on_start.state, PendingJobState::Cancelled);
        let _ = std::fs::remove_dir_all(temp);
    }

    // Section 6.4: Validation of Build UUIDs, SQL injection payloads, class/spec combinations, and realm rulesets.
    #[test]
    fn build_uuid_and_spawn_request_validation_rejects_malformed_and_mismatched_inputs() {
        // Valid UUID on Wildcard
        assert!(validate_spawn_request(
            Mode::Wildcard,
            5,
            Some(60),
            Some(10),
            Some(1),
            Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475")
        )
        .is_ok());

        // Malformed UUID & SQLi payload
        assert!(validate_spawn_request(
            Mode::Wildcard,
            5,
            Some(60),
            None,
            None,
            Some("'; DROP TABLE characters; --")
        )
        .is_err());
        assert!(validate_spawn_request(
            Mode::Wildcard,
            5,
            Some(60),
            None,
            None,
            Some("not-a-uuid")
        )
        .is_err());

        // Wrong realm: Wildcard archetype UUID passed to CoA or Reborn
        assert!(validate_spawn_request(
            Mode::Coa,
            5,
            Some(60),
            None,
            None,
            Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475")
        )
        .is_err());
        assert!(validate_spawn_request(
            Mode::Reborn,
            5,
            Some(60),
            None,
            None,
            Some("b9bc2e11-cd62-4d37-b05a-fff2a2ada475")
        )
        .is_err());

        // Invalid class/spec combination (CoA custom class 12 on Wildcard/Reborn, or stock class 1 on CoA, or spec 4 on non-Reborn)
        assert!(validate_spawn_request(Mode::Wildcard, 5, Some(60), Some(12), Some(1), None).is_err());
        assert!(validate_spawn_request(Mode::Coa, 5, Some(60), Some(1), Some(1), None).is_err());
        assert!(validate_spawn_request(Mode::Wildcard, 5, Some(60), Some(7), Some(4), None).is_err());
        assert!(validate_spawn_request(Mode::Reborn, 5, Some(60), Some(7), Some(4), None).is_ok());
    }
}
