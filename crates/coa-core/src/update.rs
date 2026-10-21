//! Update transaction: Prepare -> Verify -> Snapshot -> Apply -> Migrate -> Validate -> Commit.
//!
//! Nothing is changed until the signed package is downloaded, verified and extracted into staging. Every file that is
//! about to change is first copied into the transaction's `before/` folder, each operation is journaled, and a crash or
//! failure at any point leaves the installation recoverable: `rollback` restores binaries and configuration exactly.
//! Full recovery points protect database changes as well as files. Interrupted updates must be resolved first.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{merge, parser::ConfFile};
use crate::download::Cancel;
use crate::error::{Error, Result};
use crate::fsx;
use crate::manifest::{FileEntry, Kind, Manifest, ReplacePolicy};
use crate::migrations::ApplyReport;
use crate::package;
use crate::pkgsource::{fetch_manifest, fetch_parts, Source};
use crate::registry::{InstallMeta, MetaDir};

const STAGED_PREFIX: &str = "_migrations/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Prepared,
    Stopping,
    Snapshotting,
    Trialing,
    Applying,
    Migrating,
    Validating,
    Committing,
    Restoring,
    /// Files replaced; waiting for migrations / health check.
    Applied,
    /// Files replaced but the new build did not become healthy; the owner decides (rollback is offered).
    NeedsDecision,
    Committed,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Create,
    Replace,
    MergeConfig,
    Skip,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Op {
    pub path: String,
    pub action: Action,
    pub reason: Option<String>,
    /// sha256 the file will have after the operation.
    pub new_sha256: String,
    pub had_previous: bool,
    pub started: bool,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Keep,
    Replace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition { Updated, Rejected, Restored, RecoveryRequired }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Txn {
    pub id: String,
    pub state: State,
    pub from_version: Option<String>,
    pub to_version: String,
    pub recovery_point: Option<String>,
    #[serde(default = "legacy_databases_started")]
    pub databases_started: bool,
    pub ops: Vec<Op>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<State>,
    #[serde(default)]
    pub manifest_sha256: Option<String>,
    #[serde(default)]
    pub was_running: bool,
    #[serde(default)]
    pub server_stopped: bool,
    #[serde(default)]
    pub recovery_verified: bool,
    #[serde(default)]
    pub trial_completed: bool,
    #[serde(default)]
    pub protected_files: Vec<String>,
    #[serde(default)]
    pub recovery_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub outcome: Option<Disposition>,
}

fn legacy_databases_started() -> bool { true }

/// Everything the transaction needs from the outside world; tests provide a fake.
pub trait Env {
    fn ensure_stopped(&self) -> Result<()>;
    fn was_running(&self) -> Result<bool> { Ok(false) }
    fn stop(&self) -> Result<()> { self.ensure_stopped() }
    fn trial(&self, _manifest: &Manifest, _tree: &Path, _before: &Path, _point: &str, _resolutions: &BTreeMap<String, Resolution>) -> Result<()> { Ok(()) }
    fn check_capacity(&self, _manifest: &Manifest) -> Result<()> { Ok(()) }
    fn reserve_recovery(&self, _before: &Path) -> Result<()> { Ok(()) }
    fn restore_running_state(&self, _running: bool) -> Result<()> { Ok(()) }
    fn verify_restored(&self) -> Result<()> { Ok(()) }
    fn verify_snapshot(&self, _id: &str) -> Result<()> { Ok(()) }
    /// Create a recovery point for the databases and configuration; returns its id.
    fn snapshot(&self) -> Result<String>;
    fn restore_snapshot(&self, id: &str) -> Result<()>;
    fn migrate(&self, manifest: &Manifest, staged_migrations: &Path) -> Result<ApplyReport>;
    /// Start the server and confirm it becomes healthy.
    fn validate(&self) -> Result<()>;
}

fn updates_dir(meta: &Path) -> PathBuf {
    meta.join("updates")
}

fn txn_dir(meta: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err(Error::PathRejected(format!("bad update id {id:?}")));
    }
    fsx::ensure_within(meta, &updates_dir(meta).join(id))
}

fn save(meta: &Path, t: &Txn) -> Result<()> {
    let mut saved = t.clone();
    saved.outcome = match saved.state {
        State::Committed if saved.server_stopped => Some(Disposition::RecoveryRequired),
        State::Committed => Some(Disposition::Updated),
        State::RolledBack => Some(Disposition::Restored),
        State::Restoring => Some(Disposition::RecoveryRequired),
        State::Failed if saved.server_stopped || saved.databases_started || saved.ops.iter().any(|op| op.started) => Some(Disposition::RecoveryRequired),
        State::Failed => Some(Disposition::Rejected),
        _ => None,
    };
    fsx::atomic_write_json(&txn_dir(meta, &t.id)?.join("txn.json"), &saved)
}

pub fn load(meta: &Path, id: &str) -> Result<Txn> {
    read_journal(&txn_dir(meta, id)?.join("txn.json"))
}

fn read_journal(path: &Path) -> Result<Txn> {
    let txn: Txn = fsx::read_json(path)?;
    let directory_id = path.parent().and_then(Path::file_name).and_then(|s| s.to_str());
    if directory_id != Some(txn.id.as_str()) || txn.id.is_empty()
        || !txn.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(Error::Invalid("Update journal identity is inconsistent. Recovery requires intervention.".into()));
    }
    let parent = path.parent().ok_or_else(|| Error::Invalid("Update journal has no directory.".into()))?;
    if let Some(point) = &txn.recovery_point {
        if point.is_empty() || !point.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
            return Err(Error::Invalid("Update journal contains an invalid recovery point identity.".into()));
        }
    }
    let valid_hash = |hash: &str| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if txn.manifest_sha256.as_deref().is_some_and(|hash| !valid_hash(hash))
        || txn.recovery_hashes.values().any(|hash| !valid_hash(hash)) {
        return Err(Error::Invalid("Update journal contains an invalid content hash.".into()));
    }
    let mut paths = std::collections::BTreeSet::new();
    for op in &txn.ops {
        fsx::safe_join(parent, &op.path)?;
        if !paths.insert(op.path.replace('\\', "/").to_ascii_lowercase()) || !valid_hash(&op.new_sha256) {
            return Err(Error::Invalid("Update journal contains duplicate paths or an invalid file hash.".into()));
        }
        if op.done && !op.started && !matches!(op.action, Action::Skip | Action::Conflict) {
            return Err(Error::Invalid("Update journal contains an inconsistent file operation.".into()));
        }
    }
    for protected in &txn.protected_files { fsx::safe_join(parent, protected)?; }
    if txn.state == State::RolledBack && txn.manifest_sha256.is_some() && !txn.recovery_verified {
        return Err(Error::Invalid("Update journal claims an unverified recovery.".into()));
    }
    if txn.state == State::Prepared && (txn.server_stopped || txn.ops.iter().any(|op| op.started)) {
        return Err(Error::Invalid("Update journal marks a changed installation as unstarted.".into()));
    }
    if txn.state == State::Committed && txn.ops.iter().any(|op| !op.done && !matches!(op.action, Action::Skip | Action::Conflict)) {
        return Err(Error::Invalid("Update journal marks incomplete file operations as committed.".into()));
    }
    Ok(txn)
}

pub fn latest(meta: &Path) -> Result<Option<Txn>> {
    let mut transactions = read_journals(meta)?;
    transactions.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(transactions.into_iter().next())
}

#[derive(Serialize, Deserialize)]
struct CleanupMarker { id: String, state: State, server_stopped: bool }

fn cleanup_marker(meta: &Path, id: &str) -> Result<PathBuf> {
    txn_dir(meta, id)?;
    fsx::ensure_within(meta, &updates_dir(meta).join(format!("{id}.cleanup.json")))
}

fn read_journals(meta: &Path) -> Result<Vec<Txn>> {
    let entries = match fs::read_dir(updates_dir(meta)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut transactions = Vec::new();
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() { return Err(Error::Invalid("Linked update journal storage is not supported.".into())); }
        if !kind.is_dir() { continue; }
        let directory = fsx::ensure_within(meta, &entry.path())?;
        let path = directory.join("txn.json");
        if !path.is_file() {
            let id = directory.file_name().and_then(|s| s.to_str()).ok_or_else(|| Error::Invalid("Invalid update directory identity.".into()))?;
            let completed_cleanup = fsx::read_json::<CleanupMarker>(&cleanup_marker(meta, id)?).is_ok_and(|m|
                m.id == id && !m.server_stopped && matches!(m.state, State::Committed | State::RolledBack));
            if completed_cleanup { continue; }
            return Err(Error::Invalid(format!("Update journal {} is missing. Recovery requires intervention; available copies were preserved.", path.display())));
        }
        transactions.push(read_journal(&path).map_err(|_| Error::Invalid("An update journal cannot be read or is inconsistent. Recovery is required.".into()))?);
    }
    Ok(transactions)
}

/// Called while holding the installation lock. Never replays interrupted SQL.
pub(crate) fn recover_before_start(root: &Path, meta: &Path) -> Result<()> {
    if let Some(mut txn) = latest(meta)? {
        // Commit has already happened: resume admission, never replay SQL or undo it.
        if txn.state == State::Committed && txn.server_stopped {
            release_reserve(root, &txn_dir(meta, &txn.id)?.join("before"))?;
            if let Err(e) = (RepackEnv { root, meta_dir: meta }).restore_running_state(txn.was_running) {
                txn.failure_stage = Some(State::Committed);
                txn.message = Some(format!("The update is committed, but restoring the previous running state failed: {e}"));
                save(meta, &txn)?;
                write_diagnostic(root, meta, &txn)?;
                return Err(e);
            }
            txn.server_stopped = false;
            txn.message = Some("Committed update startup was resumed.".into());
            save(meta, &txn)?;
            write_diagnostic(root, meta, &txn)?;
        }
    }
    if let Some(mut txn) = unfinished(meta) {
        if txn.failure_stage.is_none() && txn.state != State::Restoring { txn.failure_stage = Some(txn.state); }
        txn.state = State::Restoring;
        save(meta, &txn)?;
        let env = RepackEnv { root, meta_dir: meta };
        if let Err(e) = restore_transaction(root, meta, &txn_dir(meta, &txn.id)?.join("before"), &mut txn, &env) {
            txn.message = Some(format!("Automatic recovery requires intervention: {e}"));
            save(meta, &txn)?;
            write_diagnostic(root, meta, &txn)?;
            return Err(e);
        }
        txn.state = State::RolledBack;
        txn.message = Some("Interrupted update was automatically restored.".into());
        save(meta, &txn)?;
        write_diagnostic(root, meta, &txn)?;
    }
    ensure_recovered(meta)
}

pub fn recover(root: &Path, meta: &Path) -> Result<Option<Txn>> {
    let _lock = operation_lock(meta)?;
    recover_before_start(root, meta)?;
    latest(meta)
}

/// A transaction that started but never reached a final state (crash, power loss). The UI offers rollback.
pub fn unfinished(meta: &Path) -> Option<Txn> {
    let mut all = read_journals(meta).ok()?;
    all.sort_by(|a, b| b.id.cmp(&a.id));
    all.into_iter().find(|t| !matches!(t.state, State::Committed | State::RolledBack | State::Prepared) && (t.state != State::Failed || t.server_stopped || t.databases_started || t.ops.iter().any(|o| o.started)))
}

pub fn ensure_recovered(meta: &Path) -> Result<()> {
    for txn in read_journals(meta)? {
        if txn.state == State::Committed && txn.server_stopped {
            return Err(Error::Invalid(format!("Update {} is committed but has not restored the previous running state. Resume recovery before changing the server.", txn.id)));
        }
    }
    if let Some(t) = unfinished(meta) {
        return Err(Error::Invalid(format!("Resolve unfinished update {} before changing or starting the server.", t.id)));
    }
    Ok(())
}

fn sha_if_exists(p: &Path) -> Option<String> {
    p.is_file().then(|| fsx::sha256_file(p).ok()).flatten()
}

#[derive(Debug, Serialize)]
pub struct PlanItem {
    pub path: String,
    pub action: Action,
    pub reason: Option<String>,
}

/// What an update would do to this installation. Read-only.
pub fn plan(root: &Path, meta: &InstallMeta, manifest: &Manifest, resolutions: &BTreeMap<String, Resolution>, staged: Option<&Path>) -> Result<Vec<PlanItem>> {
    let mut out = Vec::new();
    for f in manifest.files.iter().filter(|f| !f.path.starts_with(STAGED_PREFIX)) {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &f.path)?)?;
        let cur = sha_if_exists(&target);
        let recorded = meta.original_hashes.get(&f.path);
        let (action, reason) = match (f.policy, &cur) {
            (ReplacePolicy::NeverTouch, _) => (Action::Skip, Some("managed by you".to_string())),
            (_, None) => (Action::Create, None),
            (ReplacePolicy::CreateIfMissing, Some(_)) => (Action::Skip, Some("kept your existing file".into())),
            (ReplacePolicy::MergeConfig, Some(_)) => {
                let would_add = match staged {
                    Some(tree) => {
                        let (a, b) = (fs::read(&target)?, fs::read(fsx::safe_join(tree, &f.path)?)?);
                        !merge::plan(&ConfFile::parse_bytes(&a)?, &ConfFile::parse_bytes(&b)?).added.is_empty()
                    }
                    None => true,
                };
                if would_add { (Action::MergeConfig, Some("new settings are added; your values are kept".into())) } else { (Action::Skip, Some("no new settings".into())) }
            }
            (_, Some(c)) if c.eq_ignore_ascii_case(&f.sha256) => (Action::Skip, Some("already up to date".into())),
            (ReplacePolicy::Replace | ReplacePolicy::ReplaceIfPristine, Some(c)) => {
                if f.path == "Core/worldserver.exe" && meta.kind == crate::registry::InstallKind::Imported && crate::squid::imported_repack(root) {
                    match resolutions.get(&f.path) {
                        Some(Resolution::Replace) => (Action::Replace, Some("Replacing this binary stops the original SquidBots launcher/updater from accepting its hash.".into())),
                        Some(Resolution::Keep) => (Action::Skip, Some("kept the original SquidBots binary".into())),
                        None => (Action::Conflict, Some("This SquidBots repack verifies its worldserver hash. Replacing it stops Start_All_Bots/coa_update from working. Choose explicitly whether to replace this binary.".into())),
                    }
                } else if recorded.map(|r| r.eq_ignore_ascii_case(c)).unwrap_or(false) {
                    (Action::Replace, None)
                } else if f.policy == ReplacePolicy::ReplaceIfPristine {
                    (Action::Skip, Some("modified outside CoA Server Manager; kept".into()))
                } else {
                    match resolutions.get(&f.path) {
                        Some(Resolution::Replace) => (Action::Replace, Some("you chose to replace your modified file".into())),
                        Some(Resolution::Keep) => (Action::Skip, Some("you chose to keep your modified file".into())),
                        None => (Action::Conflict, Some("modified outside CoA Server Manager".into())),
                    }
                }
            }
        };
        out.push(PlanItem { path: f.path.clone(), action, reason });
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Preview {
    pub from_version: Option<String>,
    pub to_version: String,
    pub items: Vec<PlanItem>,
    pub conflicts: Vec<String>,
    pub migrations: usize,
    pub download_bytes: u64,
}

/// Check the signed manifest and describe the update without downloading the payload.
pub fn preview(root: &Path, meta: &InstallMeta, source: &Source, trusted_key: &str, resolutions: &BTreeMap<String, Resolution>) -> Result<Preview> {
    let (m, _) = fetch_manifest(source, trusted_key)?;
    check_manifest(&m)?;
    let items = plan(root, meta, &m, resolutions, None)?;
    Ok(Preview {
        from_version: meta.core.version.clone(),
        to_version: m.version.clone(),
        conflicts: items.iter().filter(|i| i.action == Action::Conflict).map(|i| i.path.clone()).collect(),
        items,
        migrations: m.migrations.len(),
        download_bytes: m.archive.as_ref().map(|a| a.parts.iter().map(|p| p.size).sum()).unwrap_or(0),
    })
}

fn check_manifest(m: &Manifest) -> Result<()> {
    if m.kind != Kind::Update {
        return Err(Error::Invalid("This is not an update package.".into()));
    }
    if !m.compatible_with_manager(crate::MANAGER_VERSION) {
        return Err(Error::Invalid("This update needs a newer version of CoA Server Manager.".into()));
    }
    Ok(())
}

pub struct Params<'a> {
    pub root: &'a Path,
    pub meta_dir: &'a Path,
    pub source: Source,
    pub trusted_key: &'a str,
    pub cancel: Cancel,
    pub resolutions: BTreeMap<String, Resolution>,
    pub env: &'a dyn Env,
    /// Test hook: fail after this many completed file operations.
    pub fail_after_ops: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub txn: Txn,
    pub migrations: Option<ApplyReport>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub disposition: Disposition,
    pub txn: Option<Txn>,
    pub message: Option<String>,
}

pub fn last_execution_result(meta: &Path) -> Result<Option<ExecutionResult>> {
    let path = meta.join("logs/update-result.json");
    if !path.exists() { return Ok(None); }
    fsx::read_json(&path).map(Some)
}

/// UI contract: application errors retain their actual recovery outcome instead of
/// forcing the caller to infer it from a translated error string.
pub fn execute(p: &Params, report: &dyn Fn(&str, u8)) -> Result<ExecutionResult> {
    let previous = latest(p.meta_dir);
    let previous_id = previous.as_ref().ok().and_then(|t| t.as_ref()).map(|t| t.id.clone());
    match apply(p, report) {
        Ok(out) => {
            let result = ExecutionResult { disposition: Disposition::Updated, txn: Some(out.txn), message: None };
            fsx::atomic_write_json(&p.meta_dir.join("logs/update-result.json"), &result)?;
            Ok(result)
        },
        Err(error) => {
            let journal = latest(p.meta_dir);
            let corrupted = previous.is_err() || journal.is_err();
            let txn = journal.ok().flatten().filter(|t| Some(&t.id) != previous_id.as_ref() || t.server_stopped || unfinished(p.meta_dir).is_some());
            let disposition = if corrupted { Disposition::RecoveryRequired } else {
                txn.as_ref().and_then(|t| t.outcome).unwrap_or(Disposition::Rejected)
            };
            let mut message = error.to_string();
            if let Ok((a,b)) = crate::db::credentials(p.root) {
                for secret in [a,b] { if !secret.is_empty() { message = message.replace(&secret, "[redacted]"); } }
            }
            let mut result = ExecutionResult { disposition, txn, message: Some(crate::diag::redact(&message)) };
            if let Some(txn) = &mut result.txn { txn.message = result.message.clone(); }
            // Even signature/network rejection, before any transaction exists, gets a local report.
            fsx::atomic_write_json(&p.meta_dir.join("logs").join(format!("update-result-{}.json", uuid::Uuid::new_v4().simple())), &result)?;
            fsx::atomic_write_json(&p.meta_dir.join("logs/update-result.json"), &result)?;
            Ok(result)
        }
    }
}

fn step(report: &dyn Fn(&str, u8), name: &str, pct: u8) {
    tracing::info!(step = name, percent = pct, "server update progress");
    report(name, pct);
}

/// Run the private trial and apply the same package. Failures before commit restore automatically.
pub fn apply(p: &Params, report: &dyn Fn(&str, u8)) -> Result<Outcome> {
    let (root, meta_dir) = (p.root, p.meta_dir);
    let _lock = operation_lock(meta_dir)?;
    ensure_recovered(meta_dir)?;
    let (md, mut meta) = MetaDir::open(meta_dir)?;
    let _ = md;

    step(report, "Checking the update", 2);
    let pinned = crate::pkgsource::fetch_package(&p.source, p.trusted_key)?;
    let (m, manifest_bytes, signature, source) = (pinned.manifest, pinned.bytes, pinned.signature, pinned.source);
    tracing::info!(root = %root.display(), from_version = ?meta.core.version, to_version = %m.version, migrations = m.migrations.len(), "server update selected");
    check_manifest(&m)?;
    let archive = m.archive.clone().ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
    let id = format!("{}-{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), m.version.replace('.', "_"), uuid::Uuid::new_v4().simple());
    let tdir = txn_dir(meta_dir, &id)?;
    let (tree, before) = (tdir.join("tree"), tdir.join("before"));
    fs::create_dir_all(&before)?;
    crate::signing::verify(&manifest_bytes, &String::from_utf8_lossy(&signature), p.trusted_key)?;
    fsx::atomic_write(&tdir.join("manifest.json"), &manifest_bytes)?;
    fsx::atomic_write(&tdir.join("manifest.json.sig"), &signature)?;
    fsx::atomic_write(&before.join("manager-install.json"), &fs::read(meta_dir.join("install.json"))?)?;
    let previous_manifest = meta_dir.join("manifests").join(format!("update-{}.json", m.version));
    fsx::atomic_write(&before.join("manager-manifest-existed"), if previous_manifest.exists() { b"yes" } else { b"no" })?;
    if previous_manifest.exists() { fsx::atomic_write(&before.join("manager-manifest.json"), &fs::read(previous_manifest)?)?; }

    let mut txn = Txn { id: id.clone(), state: State::Prepared, from_version: meta.core.version.clone(), to_version: m.version.clone(), recovery_point: None, databases_started: false, ops: Vec::new(), message: None, failure_stage: None, manifest_sha256: Some(fsx::sha256_bytes(&manifest_bytes)), was_running: false, server_stopped: false, recovery_verified: false, trial_completed: false, protected_files: Vec::new(), recovery_hashes: BTreeMap::new(), outcome: None };
    txn.recovery_hashes.insert("@install".into(), fsx::sha256_file(&before.join("manager-install.json"))?);
    if before.join("manager-manifest.json").exists() { txn.recovery_hashes.insert("@manifest".into(), fsx::sha256_file(&before.join("manager-manifest.json"))?); }
    save(meta_dir, &txn)?;

    // Nothing has touched the installation yet; failures up to here just discard the staging area.
    let staged = (|| -> Result<PathBuf> {
        step(report, "Downloading the update", 5);
        let parts = fetch_parts(&source, &m, &meta_dir.join("staging").join("download"), &p.cancel, &|f, _| step(report, "Downloading the update", 5 + (f * 35.0) as u8))?;
        step(report, "Verifying the update", 42);
        package::extract(&parts, &m, &tree, &|_, _| {})?;
        Ok(tree.clone())
    })();
    let tree = match staged {
        Ok(t) => t,
        Err(e) => {
            txn.state = State::Failed;
            txn.message = Some(e.to_string());
            let _ = save(meta_dir, &txn);
            let _ = fs::remove_dir_all(&tree);
            return Err(e);
        }
    };

    crate::update_fault::checkpoint(root, "verified");
    let items = plan(root, &meta, &m, &p.resolutions, Some(&tree))?;
    if let Some(c) = items.iter().find(|i| i.action == Action::Conflict) {
        txn.state = State::Failed;
        txn.message = Some(format!("{} needs a decision", c.path));
        save(meta_dir, &txn)?;
        let _ = fs::remove_dir_all(&tree);
        return Err(Error::Invalid(format!("Some files were modified outside CoA Server Manager and need your decision (for example {}).", c.path)));
    }
    let need: u64 = m.files.iter().filter(|f| items.iter().any(|i| i.path == f.path && matches!(i.action, Action::Replace | Action::Create | Action::MergeConfig))).map(|f| f.size).sum();
    fsx::require_space(root, need)?;
    fsx::require_space(meta_dir, need)?;

    p.env.check_capacity(&m)?;
    txn.was_running = p.env.was_running()?;
    let preparation = (|| -> Result<()> {
        // Validation renders files not necessarily listed in the incoming package.
        txn.protected_files = crate::backup::config_files(root);
        txn.protected_files.extend(crate::update_trial::secondary_files(root)?);
        for path in ["Scripts/manage.py", "Settings/realm-profile.json", "Settings/realm-switch.json", "mysql/my.ini", "mysql/admin-client.ini"] {
            if root.join(path).is_file() && !txn.protected_files.iter().any(|p| p == path) { txn.protected_files.push(path.into()); }
        }
        for rel in &txn.protected_files {
            let bytes = fs::read(fsx::safe_join(root, rel)?)?;
            fsx::atomic_write(&fsx::safe_join(&before.join("validation-originals"), rel)?, &bytes)?;
            txn.recovery_hashes.insert(format!("protected:{rel}"), fsx::sha256_bytes(&bytes));
        }
        // Save originals completely before persisting the intent to stop or render files.
        txn.server_stopped = true;
        txn.state = State::Stopping;
        save(meta_dir, &txn)?;
        step(report, "Making sure the server is stopped", 48);
        p.env.stop()?;
        crate::update_fault::checkpoint(root, "stopped");
        txn.state = State::Snapshotting;
        save(meta_dir, &txn)?;
        step(report, "Saving a recovery point", 52);
        txn.recovery_point = Some(p.env.snapshot().map_err(|e| Error::Invalid(format!("The update was not applied because the safety backup failed: {e}")))?);
        let point_metadata = meta_dir.join("backups").join(txn.recovery_point.as_deref().unwrap()).join("backup.json");
        if point_metadata.is_file() { txn.recovery_hashes.insert("@recovery-point".into(), fsx::sha256_file(&point_metadata)?); }
        crate::update_fault::checkpoint(root, "snapshot");
        p.env.reserve_recovery(&before)?;
        save(meta_dir, &txn)?;
        crate::update_fault::checkpoint(root, "snapshot-recorded");
        step(report, "Testing the update on an isolated copy", 54);
        txn.state = State::Trialing;
        save(meta_dir, &txn)?;
        p.env.trial(&m, &tree, &before, txn.recovery_point.as_deref().unwrap(), &p.resolutions)?;
        crate::update_fault::checkpoint(root, "trial");
        txn.trial_completed = true;
        save(meta_dir, &txn)?;
        Ok(())
    })();
    if let Err(e) = preparation {
        if !txn.server_stopped {
            txn.state = State::Failed;
            txn.message = Some(e.to_string());
            save(meta_dir, &txn)?;
            write_diagnostic(root, meta_dir, &txn)?;
            return Err(e);
        }
        return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), None);
    }

    txn.ops = items
        .iter()
        .filter(|i| i.action != Action::Skip)
        .map(|i| {
            let f = m.files.iter().find(|f| f.path == i.path).expect("planned from manifest");
            Op { path: i.path.clone(), action: i.action, reason: i.reason.clone(), new_sha256: f.sha256.clone(), had_previous: fsx::safe_join(root, &i.path).map(|t| t.is_file()).unwrap_or(false), started: false, done: false }
        })
        .collect();
    txn.state = State::Applying;
    save(meta_dir, &txn)?;

    step(report, "Applying the update", 58);
    let applied = apply_ops(root, &tree, &before, &m.files, &mut txn, meta_dir, p.fail_after_ops);
    if let Err(e) = applied {
        step(report, "Undoing the update", 70);
        return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), None);
    }
    crate::update_fault::checkpoint(root, "applied");
    txn.state = State::Applied;
    save(meta_dir, &txn)?;

    // MySQL DDL can commit before a later statement fails. Restore the full recovery point on failure.
    let mut migrated = None;
    if !m.migrations.is_empty() {
        step(report, "Updating the database", 75);
        txn.state = State::Migrating;
        txn.databases_started = true;
        save(meta_dir, &txn)?;
        crate::update_fault::checkpoint(root, "before-migrations");
        match p.env.migrate(&m, &tree.join("_migrations")) {
            Ok(r) if r.failed.is_none() => migrated = Some(r),
            Ok(r) => {
                let (mid, why) = r.failed.clone().unwrap();
                return fail_after_apply(p, &mut txn, &before, &tree, format!("Database update {mid} failed: {why}"), Some(r));
            }
            Err(e) => return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), None),
        }
    }

    crate::update_fault::checkpoint(root, "migrated");
    step(report, "Starting the updated server", 85);
    txn.state = State::Validating;
    // Startup can change database state even when there are no SQL files.
    txn.databases_started = true;
    save(meta_dir, &txn)?;
    if let Err(e) = p.env.validate() {
        return fail_after_apply(p, &mut txn, &before, &tree, format!("The updated server did not start correctly: {e}"), migrated);
    }
    crate::update_fault::checkpoint(root, "validated");
    txn.state = State::Committing;
    save(meta_dir, &txn)?;
    if let Err(e) = finish(root, meta_dir, &mut meta, &m, &tree, &mut txn) {
        return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), migrated);
    }
    // Commit is durable before game connections are allowed again.
    release_reserve(root, &before)?;
    p.env.restore_running_state(txn.was_running)?;
    txn.server_stopped = false;
    save(meta_dir, &txn)?;
    txn = load(meta_dir, &txn.id)?;
    step(report, "Done", 100);
    let _ = archive;
    Ok(Outcome { txn, migrations: migrated })
}

fn finish(root: &Path, meta_dir: &Path, meta: &mut InstallMeta, m: &Manifest, tree: &Path, txn: &mut Txn) -> Result<()> {
    meta.core.version = Some(m.version.clone());
    meta.core.commit = m.core.commit.clone().or(meta.core.commit.clone());
    for op in &txn.ops {
        let hash = if op.path == "Scripts/manage.py" {
            match (fs::read_to_string(tree.join(&op.path)), fs::read(root.join(&op.path))) {
                (Ok(signed), Ok(actual)) if crate::driver::launcher_matches(&signed, &actual) => fsx::sha256_bytes(&actual),
                _ => op.new_sha256.clone(),
            }
        } else { op.new_sha256.clone() };
        meta.original_hashes.insert(op.path.clone(), hash);
        if !meta.managed_files.contains(&op.path) { meta.managed_files.push(op.path.clone()); }
    }
    for file in &m.files {
        if !txn.ops.iter().any(|op| op.path == file.path) {
            let path = fsx::safe_join(root, &file.path)?;
            if path.is_file() && fsx::sha256_file(&path)? == file.sha256 {
                meta.original_hashes.insert(file.path.clone(), file.sha256.clone());
            }
        }
    }
    crate::update_fault::checkpoint(root, "before-commit");
    fsx::atomic_write_json(&meta_dir.join("install.json"), meta)?;
    crate::update_fault::checkpoint(root, "commit-metadata");
    fsx::atomic_write(&meta_dir.join("manifests").join(format!("update-{}.json", m.version)), &serde_json::to_vec_pretty(m)?)?;
    txn.state = State::Committed;
    txn.message = None;
    save(meta_dir, txn)?;
    let _ = fs::remove_dir_all(tree);
    prune(meta_dir, 3);
    Ok(())
}

pub fn retry_validation(root: &Path, meta_dir: &Path, id: &str, fallback: &Source, trusted_key: &str, env: &dyn Env) -> Result<Txn> {
    let _lock = operation_lock(meta_dir)?;
    let mut txn = load(meta_dir, id)?;
    if txn.state != State::NeedsDecision || unfinished(meta_dir).is_none_or(|t| t.id != id) {
        return Err(Error::Invalid("Only an applied update awaiting startup validation can be retried.".into()));
    }
    let dir = txn_dir(meta_dir, id)?;
    let source = if dir.join("manifest.json").is_file() { Source::Dir(dir.clone()) } else { fallback.clone() };
    let (manifest, _) = fetch_manifest(&source, trusted_key)?;
    check_manifest(&manifest)?;
    if manifest.version != txn.to_version || txn.ops.iter().any(|op| !op.done) {
        return Err(Error::Invalid("The saved update does not match the applied transaction.".into()));
    }
    for op in txn.ops.iter().filter(|op| matches!(op.action, Action::Create | Action::Replace)) {
        let path = fsx::safe_join(root, &op.path)?;
        let file = manifest.files.iter().find(|f| f.path == op.path)
            .ok_or_else(|| Error::Invalid("The update journal contains a file absent from the signed package.".into()))?;
        if matches!(file.policy, ReplacePolicy::CreateIfMissing | ReplacePolicy::MergeConfig | ReplacePolicy::NeverTouch) { continue; }
        let bytes = fs::read(&path)?;
        let actual = fsx::sha256_bytes(&bytes);
        let launcher = op.path == "Scripts/manage.py"
            && fs::read_to_string(dir.join("tree").join(&op.path)).ok()
                .filter(|s| fsx::sha256_bytes(s.as_bytes()) == file.sha256)
                .is_some_and(|s| crate::driver::launcher_matches(&s, &bytes));
        if actual != file.sha256 && !launcher {
            return Err(Error::Invalid(format!("{} changed since the update was applied; use repair or rollback.", op.path)));
        }
    }
    env.ensure_stopped()?;
    if let Err(error) = env.validate() {
        txn.message = Some(format!("The updated server did not start correctly: {error}"));
        save(meta_dir, &txn)?;
        return Ok(txn);
    }
    let (_, mut meta) = MetaDir::open(meta_dir)?;
    finish(root, meta_dir, &mut meta, &manifest, &dir.join("tree"), &mut txn)?;
    Ok(txn)
}

fn fail_after_apply(p: &Params, txn: &mut Txn, before: &Path, tree: &Path, why: String, report: Option<ApplyReport>) -> Result<Outcome> {
    tracing::error!(transaction = %txn.id, databases_started = txn.databases_started, "server update failed; restoring recovery point");
    txn.failure_stage = Some(txn.state);
    txn.state = State::Restoring;
    txn.message = Some(why.clone());
    save(p.meta_dir, txn)?;
    let restored = restore_transaction(p.root, p.meta_dir, before, txn, p.env);
    tracing::info!(transaction = %txn.id, recovered = restored.is_ok(), "server update recovery finished");
    txn.state = if restored.is_ok() { State::RolledBack } else { State::Failed };
    txn.message = Some(match &restored { Ok(()) => why.clone(), Err(e) => format!("{why} Recovery failed: {e}") });
    save(p.meta_dir, txn)?;
    write_diagnostic(p.root, p.meta_dir, txn)?;
    if restored.is_ok() { let _ = fs::remove_dir_all(tree); }
    let _ = report;
    Err(Error::Invalid(match restored {
        Ok(()) if txn.databases_started => format!("{why} The server files and databases were restored to the recovery point."),
        Ok(()) => format!("{why} The server files were restored."),
        Err(e) => format!("{why} Recovery failed: {e}. Resolve the unfinished update before starting the server."),
    }))
}

fn restore_transaction(root: &Path, meta: &Path, before: &Path, txn: &mut Txn, env: &dyn Env) -> Result<()> {
    for (key, expected) in &txn.recovery_hashes {
        let saved = match key.as_str() {
            "@recovery-point" => {
                let id = txn.recovery_point.as_deref().ok_or_else(|| Error::Invalid("The recovery point identity is missing.".into()))?;
                fsx::safe_join(&meta.join("backups"), id)?.join("backup.json")
            }
            "@install" => before.join("manager-install.json"),
            "@manifest" => before.join("manager-manifest.json"),
            _ if key.starts_with("protected:") => fsx::safe_join(&before.join("validation-originals"), &key[10..])?,
            _ => fsx::safe_join(before, key)?,
        };
        if fsx::sha256_file(&saved)? != *expected { return Err(Error::Invalid(format!("Recovery copy {key} is damaged. Available copies were preserved."))); }
    }
    if let Some(id) = &txn.recovery_point { env.verify_snapshot(id)?; }
    release_reserve(root, before)?;
    let trial = before.parent().unwrap().join("trial");
    if trial.exists() && trial.join("Scripts/manage.py").is_file() {
        let out = crate::driver::run(&trial, crate::driver::Verb::StopAll)?;
        if !out.ok { return Err(Error::Invalid("The interrupted isolated trial could not be stopped.".into())); }
    }
    env.stop()?;
    crate::update_fault::checkpoint(root, "restore-stopped");
    if txn.databases_started {
        let id = txn.recovery_point.as_deref().ok_or_else(|| Error::Invalid("The database recovery point is missing.".into()))?;
        env.restore_snapshot(id)?;
        crate::update_fault::checkpoint(root, "restore-databases");
    }
    restore_files(root, before, txn)?;
    crate::update_fault::checkpoint(root, "restore-files");
    if !txn.protected_files.is_empty() {
        let mut generated = crate::backup::config_files(root);
        generated.extend(crate::update_trial::secondary_files(root)?);
        if txn.manifest_sha256.is_some() {
            for rel in ["Settings/realm-profile.json", "Settings/realm-switch.json", "mysql/my.ini", "mysql/admin-client.ini"] { if root.join(rel).is_file() { generated.push(rel.into()); } }
        }
        generated.sort();
        generated.dedup();
        for rel in generated {
            if !txn.protected_files.contains(&rel) && !txn.ops.iter().any(|o| o.path == rel && o.had_previous) {
                fs::remove_file(fsx::ensure_within(root, &fsx::safe_join(root, &rel)?)?)?;
            }
        }
        for rel in &txn.protected_files {
            fsx::atomic_write(&fsx::ensure_within(root, &fsx::safe_join(root, rel)?)?, &fs::read(fsx::safe_join(&before.join("validation-originals"), rel)?)?)?;
        }
    }
    let saved_meta = before.join("manager-install.json");
    if saved_meta.exists() {
        fsx::atomic_write(&meta.join("install.json"), &fs::read(&saved_meta)?)?;
    }
    let manifest = meta.join("manifests").join(format!("update-{}.json", txn.to_version));
    match fs::read(before.join("manager-manifest-existed")).ok().as_deref() {
        Some(b"yes") => fsx::atomic_write(&manifest, &fs::read(before.join("manager-manifest.json"))?)?,
        Some(b"no") if manifest.exists() => fs::remove_file(manifest)?,
        _ => {}, // Legacy journals did not record this optional metadata file.
    }
    env.verify_restored()?;
    env.restore_running_state(txn.was_running)?;
    // Database verification and restarting either world regenerate configuration
    // and realm snapshots. Restore those bytes after the last process command.
    // Avoid replacing unchanged runtime binaries that the restarted server holds.
    let finish_protection = (|| -> Result<()> {
        let mut generated = crate::backup::config_files(root);
        generated.extend(crate::update_trial::secondary_files(root)?);
        if txn.manifest_sha256.is_some() {
            for rel in ["Settings/realm-profile.json", "Settings/realm-switch.json", "mysql/my.ini", "mysql/admin-client.ini"] { if root.join(rel).is_file() { generated.push(rel.into()); } }
        }
        generated.sort();
        generated.dedup();
        for rel in generated {
            if !txn.protected_files.contains(&rel) && !txn.ops.iter().any(|o| o.path == rel && o.had_previous) {
                fs::remove_file(fsx::ensure_within(root, &fsx::safe_join(root, &rel)?)?)?;
            }
        }
        for rel in &txn.protected_files {
            let original = fs::read(fsx::safe_join(&before.join("validation-originals"), rel)?)?;
            let target = fsx::ensure_within(root, &fsx::safe_join(root, rel)?)?;
            if !target.is_file() || fsx::sha256_file(&target)? != fsx::sha256_bytes(&original) {
                fsx::atomic_write(&target, &original)?;
            }
        }
        if saved_meta.exists() {
            let original = fs::read(&saved_meta)?;
            if fsx::sha256_file(&meta.join("install.json"))? != fsx::sha256_bytes(&original) {
                fsx::atomic_write(&meta.join("install.json"), &original)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = finish_protection {
        let _ = env.stop();
        return Err(error);
    }
    txn.recovery_verified = true;
    txn.server_stopped = false;
    Ok(())
}

fn release_reserve(root: &Path, before: &Path) -> Result<()> {
    for file in [before.join("recovery.reserve"), root.join(".state/coa-update.reserve")] {
        match fs::remove_file(file) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn write_diagnostic(root: &Path, meta: &Path, txn: &Txn) -> Result<()> {
    let recorded = load(meta, &txn.id)?;
    let mut report = serde_json::to_value(&recorded).map_err(|e| Error::Invalid(e.to_string()))?;
    // Do not put SQL output, configs, or credentials into the automatically collected report.
    if let Some(message) = report.get_mut("message") {
        let mut text = message.as_str().unwrap_or_default().to_string();
        if let Ok((a, b)) = crate::db::credentials(root) {
            for secret in [a, b] { if !secret.is_empty() { text = text.replace(&secret, "[redacted]"); } }
        }
        *message = crate::diag::redact(&text).into();
    }
    fsx::atomic_write_json(&txn_dir(meta, &txn.id)?.join("diagnostic.json"), &report)?;
    let sanitized: Txn = serde_json::from_value(report)?;
    fsx::atomic_write_json(&meta.join("logs/update-result.json"), &ExecutionResult { disposition: sanitized.outcome.unwrap_or(Disposition::RecoveryRequired), message: sanitized.message.clone(), txn: Some(sanitized) })
}

fn apply_ops(root: &Path, tree: &Path, before: &Path, files: &[FileEntry], txn: &mut Txn, meta_dir: &Path, fail_after: Option<usize>) -> Result<()> {
    let mut completed = 0usize;
    for i in 0..txn.ops.len() {
        if fail_after == Some(completed) {
            return Err(Error::Invalid("simulated failure".into()));
        }
        let (path, action) = (txn.ops[i].path.clone(), txn.ops[i].action);
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &path)?)?;
        let staged = fsx::safe_join(tree, &path)?;
        let entry = files.iter().find(|f| f.path == path).expect("listed");
        if target.is_file() {
            let keep = fsx::safe_join(before, &path)?;
            fs::create_dir_all(keep.parent().unwrap())?;
            let bytes = fs::read(&target)?;
            fsx::atomic_write(&keep, &bytes)?;
            txn.recovery_hashes.insert(path.clone(), fsx::sha256_bytes(&bytes));
        }
        // Journal only after the old bytes are safely saved; a crash during backup must not
        // turn an untouched file into a rollback operation with a missing saved copy.
        crate::update_fault::checkpoint(root, &format!("before-file:{i}"));
        txn.ops[i].started = true;
        save(meta_dir, txn)?;
        crate::update_fault::checkpoint(root, &format!("journal-file:{i}"));
        fs::create_dir_all(target.parent().unwrap())?;
        match action {
            Action::Create | Action::Replace => {
                // Same-volume temp file + rename: the target is either the old or the new file, never half of one.
                let tmp = target.with_file_name(format!("{}.coa-new", target.file_name().unwrap().to_string_lossy()));
                fs::copy(&staged, &tmp)?;
                fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
                if let Err(e) = fsx::durable_replace(&tmp, &target) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e.into());
                }
                if !fsx::sha256_file(&target)?.eq_ignore_ascii_case(&entry.sha256) {
                    return Err(Error::HashMismatch { path, expected: entry.sha256.clone(), actual: "different after writing".into() });
                }
            }
            Action::MergeConfig => {
                let mut conf = ConfFile::parse_bytes(&fs::read(&target)?)?;
                let incoming = ConfFile::parse_bytes(&fs::read(&staged)?)?;
                merge::apply(&mut conf, &incoming);
                fsx::atomic_write(&target, conf.to_text().as_bytes())?;
                txn.ops[i].new_sha256 = fsx::sha256_file(&target)?;
            }
            Action::Skip | Action::Conflict => {}
        }
        crate::update_fault::checkpoint(root, &format!("written-file:{i}"));
        txn.ops[i].done = true;
        save(meta_dir, txn)?;
        crate::update_fault::checkpoint(root, &format!("completed-file:{i}"));
        completed += 1;
    }
    Ok(())
}

pub(crate) fn apply_trial_files(root: &Path, meta: &InstallMeta, m: &Manifest, tree: &Path, resolutions: &BTreeMap<String, Resolution>) -> Result<()> {
    for item in plan(root, meta, m, resolutions, Some(tree))? {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &item.path)?)?;
        let source = fsx::safe_join(tree, &item.path)?;
        match item.action {
            Action::Create | Action::Replace => fsx::atomic_write(&target, &fs::read(source)?)?,
            Action::MergeConfig => {
                let mut conf = ConfFile::parse_bytes(&fs::read(&target)?)?;
                merge::apply(&mut conf, &ConfFile::parse_bytes(&fs::read(source)?)?);
                fsx::atomic_write(&target, conf.to_text().as_bytes())?;
            }
            Action::Conflict => return Err(Error::Invalid(format!("{} needs a decision in the isolated copy.", item.path))),
            Action::Skip => {}
        }
    }
    Ok(())
}

/// Put every touched file back the way it was. Files created by the update (no previous version) are removed only if
/// they still hold exactly what the update wrote.
fn restore_files(root: &Path, before: &Path, txn: &mut Txn) -> Result<()> {
    for op in txn.ops.iter().rev().filter(|o| o.started) {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &op.path)?)?;
        let _ = fs::remove_file(target.with_file_name(format!("{}.coa-new", target.file_name().unwrap().to_string_lossy())));
        if op.had_previous {
            let saved = fsx::safe_join(before, &op.path)?;
            if !saved.is_file() {
                return Err(Error::Invalid(format!("the saved copy of {} is missing", op.path)));
            }
            fsx::atomic_write(&target, &fs::read(saved)?)?;
        } else if sha_if_exists(&target).map(|h| h.eq_ignore_ascii_case(&op.new_sha256)).unwrap_or(false) {
            fs::remove_file(&target)?;
        }
    }
    Ok(())
}

/// Restore the database recovery point first, then restore binaries and configuration.
pub fn rollback(root: &Path, meta_dir: &Path, id: &str, env: &dyn Env) -> Result<Txn> {
    let _lock = operation_lock(meta_dir)?;
    let mut txn = load(meta_dir, id)?;
    if matches!(txn.state, State::Committed | State::RolledBack) {
        return Err(Error::Invalid("This update cannot be rolled back.".into()));
    }
    txn.state = State::Restoring;
    save(meta_dir, &txn)?;
    if let Err(e) = restore_transaction(root, meta_dir, &txn_dir(meta_dir, id)?.join("before"), &mut txn, env) {
        txn.message = Some(format!("Recovery requires intervention: {e}"));
        save(meta_dir, &txn)?;
        write_diagnostic(root, meta_dir, &txn)?;
        return Err(e);
    }
    txn.state = State::RolledBack;
    txn.message = Some("rolled back".into());
    save(meta_dir, &txn)?;
    let _ = fs::remove_dir_all(txn_dir(meta_dir, id)?.join("tree"));
    write_diagnostic(root, meta_dir, &txn)?;
    load(meta_dir, id)
}

pub(crate) fn operation_lock(meta_dir: &Path) -> Result<fs::File> {
    fs::create_dir_all(meta_dir)?;
    let lock = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(meta_dir.join("update.lock"))?;
    if !fs4::fs_std::FileExt::try_lock_exclusive(&lock)? {
        return Err(Error::Invalid("Another update, repair or recovery is in progress.".into()));
    }
    Ok(lock)
}

/// Repair uses the same durable recovery journal as an update. Its caller holds operation_lock.
pub(crate) fn begin_repair(root: &Path, meta_dir: &Path, version: &str, backup: &str, files: &[FileEntry]) -> Result<Txn> {
    let id = format!("{}-repair-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), uuid::Uuid::new_v4().simple());
    let before = txn_dir(meta_dir, &id)?.join("before");
    fsx::atomic_write(&before.join("manager-install.json"), &fs::read(meta_dir.join("install.json"))?)?;
    let mut ops = Vec::new();
    for file in files {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &file.path)?)?;
        let had_previous = target.is_file();
        if had_previous { fsx::atomic_write(&fsx::safe_join(&before, &file.path)?, &fs::read(&target)?)?; }
        ops.push(Op { path: file.path.clone(), action: if had_previous { Action::Replace } else { Action::Create }, reason: None, new_sha256: file.sha256.clone(), had_previous, started: true, done: false });
    }
    let txn = Txn { id, state: State::Applying, from_version: Some(version.into()), to_version: version.into(), recovery_point: Some(backup.into()), databases_started: false, ops, message: Some("Repair is in progress.".into()), failure_stage: None, manifest_sha256: None, was_running: false, server_stopped: false, recovery_verified: false, trial_completed: false, protected_files: Vec::new(), recovery_hashes: BTreeMap::new(), outcome: None };
    save(meta_dir, &txn)?;
    Ok(txn)
}

pub(crate) fn repair_migrating(meta_dir: &Path, txn: &mut Txn) -> Result<()> {
    txn.state = State::Applied;
    txn.databases_started = true;
    save(meta_dir, txn)
}

pub(crate) fn finish_repair(root: &Path, meta_dir: &Path, txn: &mut Txn, success: bool, env: &dyn Env) -> Result<()> {
    if !success { restore_transaction(root, meta_dir, &txn_dir(meta_dir, &txn.id)?.join("before"), txn, env)?; }
    txn.state = if success { State::Committed } else { State::RolledBack };
    save(meta_dir, txn)
}

fn prune(meta_dir: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(updates_dir(meta_dir)) else { return };
    let mut dirs: Vec<_> = rd.flatten().filter(|e| read_journal(&e.path().join("txn.json")).is_ok_and(|t| matches!(t.state, State::Committed | State::RolledBack) && !t.server_stopped)).map(|e| e.path()).collect();
    dirs.sort();
    while dirs.len() > keep {
        if let Ok(path) = fsx::ensure_within(meta_dir, &dirs.remove(0)) {
            let Ok(txn) = read_journal(&path.join("txn.json")) else { continue };
            let Ok(marker) = cleanup_marker(meta_dir, &txn.id) else { continue };
            if fsx::atomic_write_json(&marker, &CleanupMarker { id: txn.id, state: txn.state, server_stopped: txn.server_stopped }).is_err() { continue; }
            if fs::remove_dir_all(path).is_ok() { let _ = fs::remove_file(marker); }
        }
    }
}

/// The real environment: a repack-shaped installation driven through its launcher.
pub struct RepackEnv<'a> {
    pub root: &'a Path,
    pub meta_dir: &'a Path,
}

impl RepackEnv<'_> {
    fn verify_snapshot_contents(&self, point: &crate::backup::RecoveryPoint) -> Result<()> {
        if point.kind != crate::backup::Kind::Full || !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) {
            return Err(Error::Invalid("The update recovery point does not contain every required database and configuration.".into()));
        }
        let mut names = std::collections::BTreeSet::new();
        for component in &point.components {
            if !names.insert(&component.name) { return Err(Error::Invalid("The recovery point contains duplicate components.".into())); }
            if component.name == "configs" {
                let files = component.files.as_ref().ok_or_else(|| Error::Invalid("The recovery point has no configuration inventory.".into()))?;
                if files.iter().any(|file| !component.file_sha256.contains_key(file)) {
                    return Err(Error::Invalid("The recovery point lacks configuration checksums; automatic restoration requires inspection.".into()));
                }
            } else {
                if component.sha256.as_ref().is_none_or(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())) || component.tables.is_none() {
                    return Err(Error::Invalid(format!("Recovery database {} has incomplete integrity metadata.", component.name)));
                }
                if component.name.contains('-') || component.name == "playerbots" { crate::db::schema_of(&component.name)?; }
                else { point.realm.schema(&component.name)?; }
            }
        }
        if !crate::backup::verify(self.meta_dir, &point.id)?.ok { return Err(Error::Invalid("The recovery point failed verification. Available copies were preserved.".into())); }
        Ok(())
    }

    pub(crate) fn verify_recovery_point(&self, point: &crate::backup::RecoveryPoint) -> Result<()> {
        self.verify_snapshot_contents(point)?;
        crate::backup::with_database(self.root, |db| {
            let db = db.clone().for_realm(crate::realms::Mode::Coa);
            for component in point.components.iter().filter(|c| c.sha256.is_some()) {
                let schema = if component.name.contains('-') || component.name == "playerbots" { crate::db::schema_of(&component.name)? } else { point.realm.schema(&component.name)? };
                if db.extra_objects(schema)? != 0 {
                    return Err(Error::Invalid(format!("Database {schema} has routines, triggers or views that automatic recovery cannot restore. The update was not started.")));
                }
            }
            Ok(())
        })
    }

    fn migration_snapshot(&self) -> Result<String> {
        if let Some(id) = unfinished(self.meta_dir).and_then(|t| t.recovery_point) {
            if crate::backup::get(self.meta_dir, &id)?.kind == crate::backup::Kind::Full && crate::backup::verify(self.meta_dir, &id)?.ok {
                return Ok(id);
            }
            return Err(Error::Invalid("The update recovery point is incomplete or damaged.".into()));
        }
        Ok(crate::backup::create(self.root, self.meta_dir, crate::backup::Kind::Full, crate::backup::Trigger::BeforeMigration, None, &|_| {})?.id)
    }
}

impl RepackEnv<'_> {
    fn wait_for_running(&self) -> bool {
        use crate::process::{observe, ServiceState};
        let ports = crate::layout::read_ports(self.root);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut ready_since = None;
        loop {
            let o = observe(self.root, &ports);
            if o.mysql.state == ServiceState::Running && o.auth.state == ServiceState::Running && o.world.state == ServiceState::Running && o.secondary_world.as_ref().is_none_or(|s| s.state == ServiceState::Running) {
                let since = ready_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() >= std::time::Duration::from_secs(10) { return true; }
            } else { ready_since = None; }
            if std::time::Instant::now() > deadline { return false; }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    fn validate_isolated(&self) -> Result<()> {
        crate::backup::with_database(self.root, |db| {
            let mut modes = vec![crate::realms::Mode::Coa];
            if crate::realms::state(self.root)?.wildcard_created { modes.push(crate::realms::Mode::Wildcard); }
            for mode in modes {
                if let Some(p) = crate::schema_check::check(&db.clone().for_realm(mode), self.root)?.first() {
                    return Err(Error::Invalid(format!("Database validation failed on {}: {}.{}: {}", mode.name(), p.database, p.table, p.detail)));
                }
            }
            Ok(())
        })?;
        // The check start is the first start of the new build: give the module configs the settings the update added
        // (as a normal start does), or the server logs a "missing property" line for every one of them.
        match crate::registry::MetaDir::open(self.meta_dir) {
            Ok((_, meta)) if meta.kind == crate::registry::InstallKind::New => {
                crate::config::materialize_module_configs(self.root)?;
            }
            // An imported server gets the files it lacks (nothing it has is changed).
            _ => {
                crate::config::create_missing_module_configs(self.root)?;
            }
        }
        let started = match crate::driver::validate_update(self.root) {
            Ok(out) => out,
            Err(e) => {
                let _ = crate::driver::run(self.root, crate::driver::Verb::StopAll);
                return Err(e);
            }
        };
        let healthy = started.ok && self.wait_for_running();
        if healthy {
            let out = crate::driver::run(self.root, crate::driver::Verb::StopAll)?;
            if !out.ok { return Err(Error::Invalid("The validated server could not be stopped before commit.".into())); }
            return Ok(());
        }
        let cause = crate::driver::startup_failure(self.root, &started);
        // Leave the files unlocked so that a rollback can replace them.
        let _ = crate::driver::run(self.root, crate::driver::Verb::StopAll);
        Err(Error::Invalid(cause))
    }
}

impl Env for RepackEnv<'_> {
    fn was_running(&self) -> Result<bool> {
        let o = crate::process::observe(self.root, &crate::layout::read_ports(self.root));
        Ok(o.world.state == crate::process::ServiceState::Running || crate::multiworld::is_running(self.root))
    }

    fn stop(&self) -> Result<()> {
        let out = crate::driver::run(self.root, crate::driver::Verb::StopAll)?;
        if !out.ok { return Err(Error::Invalid("The server could not be stopped for update or recovery.".into())); }
        self.ensure_stopped()
    }

    fn check_capacity(&self, manifest: &Manifest) -> Result<()> {
        crate::update_trial::capacity(self.root, self.meta_dir, manifest)
    }

    fn reserve_recovery(&self, before: &Path) -> Result<()> {
        crate::update_trial::reserve(self.root, before)
    }

    fn trial(&self, manifest: &Manifest, tree: &Path, before: &Path, point: &str, resolutions: &BTreeMap<String, Resolution>) -> Result<()> {
        crate::update_trial::run(self.root, self.meta_dir, manifest, tree, before, point, resolutions)
    }

    fn restore_running_state(&self, running: bool) -> Result<()> {
        if running {
            let out = crate::driver::start_after_update(self.root)?;
            if !out.ok { return Err(Error::Invalid(format!("The restored server could not start: {}", crate::driver::startup_failure(self.root, &out)))); }
            if !self.wait_for_running() {
                let _ = crate::driver::run(self.root, crate::driver::Verb::StopAll);
                return Err(Error::Invalid("The original running state could not be restored: the server did not stay healthy. Recovery copies were preserved.".into()));
            }
        } else { self.stop()?; }
        Ok(())
    }

    fn verify_restored(&self) -> Result<()> {
        let transaction = unfinished(self.meta_dir);
        if let Some(id) = transaction.as_ref().and_then(|t| t.recovery_point.as_ref()) {
            let point = crate::backup::get(self.meta_dir, &id)?;
            crate::backup::with_database(self.root, |db| {
                let db = db.clone().for_realm(crate::realms::Mode::Coa);
                for component in point.components.iter().filter(|c| c.sha256.is_some()) {
                    let schema = if component.name.contains('-') || component.name == "playerbots" { crate::db::schema_of(&component.name)? } else { point.realm.schema(&component.name)? };
                    if component.tables.is_some_and(|expected| db.tables(schema).map(|t| t.len() != expected).unwrap_or(true)) {
                        return Err(Error::Invalid(format!("Restored database {schema} does not match the original snapshot.")));
                    }
                    // The same deterministic dump includes schema, rows and migration history.
                    // Table counts alone cannot prove that accounts and inventory survived.
                    let check = self.meta_dir.join("backups").join(&id).join(format!("verify-{}.sql.zst", component.name));
                    let (_, actual) = db.dump_to(schema, &check)?;
                    let _ = fs::remove_file(&check);
                    let expected = if transaction.as_ref().is_some_and(|t| t.databases_started) {
                        fs::read_to_string(self.meta_dir.join("backups").join(id).join(format!("restore-{}.sha256", component.name)))?
                    } else { component.sha256.clone().unwrap_or_default() };
                    if expected.trim() != actual {
                        return Err(Error::Invalid(format!("Restored database {schema} content differs from the recovery point. Copies were preserved.")));
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn ensure_stopped(&self) -> Result<()> {
        use crate::process::{observe, ServiceState};
        let o = observe(self.root, &crate::layout::read_ports(self.root));
        if o.world.state != ServiceState::Stopped || o.auth.state != ServiceState::Stopped || crate::multiworld::is_running(self.root) {
            return Err(Error::Invalid("Stop the server before updating it; files cannot be replaced while it is running.".into()));
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<String> {
        let label = Some("before update".to_string());
        let point = crate::backup::create(self.root, self.meta_dir, crate::backup::Kind::Full, crate::backup::Trigger::BeforeUpdate, label, &|_| {})?;
        self.verify_recovery_point(&point)?;
        Ok(point.id)
    }

    fn restore_snapshot(&self, id: &str) -> Result<()> {
        let point = crate::backup::get(self.meta_dir, id)?;
        self.verify_snapshot_contents(&point)?;
        for component in point.components.iter().filter(|c| c.sha256.is_some()) {
            crate::backup::restore_update_database(self.root, self.meta_dir, id, &component.name)?;
        }
        crate::backup::restore_update_configs(self.root, self.meta_dir, id)?;
        Ok(())
    }

    fn verify_snapshot(&self, id: &str) -> Result<()> {
        self.verify_snapshot_contents(&crate::backup::get(self.meta_dir, id)?)
    }

    fn migrate(&self, manifest: &Manifest, staged: &Path) -> Result<ApplyReport> {
        let root = self.root;
        crate::backup::with_database(root, |db| {
            let realms = crate::realms::state(root)?;
            let mut result = crate::migrations::apply_pending(db, &manifest.migrations, staged, &|| {
                self.migration_snapshot()
            })?;
            if realms.wildcard_created && result.failed.is_none() {
                let other = if realms.active == crate::realms::Mode::Coa { crate::realms::Mode::Wildcard } else { crate::realms::Mode::Coa };
                let other_db = db.clone().for_realm(other);
                let shared_snapshot = result.snapshot.clone();
                let migrations: Vec<_> = manifest.migrations.iter().filter(|m| m.db != "auth").cloned().collect();
                let extra = crate::migrations::apply_pending(&other_db, &migrations, staged, &|| {
                    if let Some(id) = &shared_snapshot { return Ok(id.clone()); }
                    self.migration_snapshot()
                })?;
                result.applied.extend(extra.applied);
                result.failed = extra.failed;
                if result.snapshot.is_none() { result.snapshot = extra.snapshot; }
                if result.failed.is_none() {
                    if let Some(p) = crate::schema_check::check(&other_db, root)?.first() {
                        return Err(Error::Invalid(format!("Database validation failed on {}: {}.{}: {}", other.name(), p.table, p.column, p.detail)));
                    }
                }
            }
            if result.failed.is_none() {
                let problems = crate::schema_check::check(db, root)?;
                if let Some(p) = problems.first() {
                    return Err(Error::Invalid(format!("Database validation failed: {}.{}.{}: {} ({} problems).", p.database, p.table, p.column, p.detail, problems.len())));
                }
            }
            Ok(result)
        })
    }

    fn validate(&self) -> Result<()> {
        crate::install::with_scratch_ports(self.root, || self.validate_isolated())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{build, BuildOptions};
    use crate::registry::{InstallKind, InstallMeta};
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use std::cell::{Cell, RefCell};

    struct Fake {
        stopped: Cell<bool>,
        healthy: Cell<bool>,
        snapshot_ok: Cell<bool>,
        calls: RefCell<Vec<&'static str>>,
        migrate_fail: Cell<bool>,
        restore_fail: Cell<bool>,
    }

    impl Fake {
        fn ok() -> Fake {
            Fake { stopped: Cell::new(true), healthy: Cell::new(true), snapshot_ok: Cell::new(true), calls: Default::default(), migrate_fail: Cell::new(false), restore_fail: Cell::new(false) }
        }
    }

    impl Env for Fake {
        fn ensure_stopped(&self) -> Result<()> {
            self.calls.borrow_mut().push("stop");
            if self.stopped.get() { Ok(()) } else { Err(Error::Invalid("The server is running.".into())) }
        }
        fn snapshot(&self) -> Result<String> {
            self.calls.borrow_mut().push("snapshot");
            if self.snapshot_ok.get() { Ok("rp-1".into()) } else { Err(Error::Invalid("disk full".into())) }
        }
        fn migrate(&self, _m: &Manifest, _d: &Path) -> Result<ApplyReport> {
            self.calls.borrow_mut().push("migrate");
            Ok(ApplyReport { applied: vec![], failed: self.migrate_fail.get().then(|| ("m1".to_string(), "syntax".to_string())), snapshot: None })
        }
        fn restore_snapshot(&self, _id: &str) -> Result<()> {
            self.calls.borrow_mut().push("restore-databases");
            if self.restore_fail.get() { Err(Error::Invalid("recovery disk unavailable".into())) } else { Ok(()) }
        }
        fn validate(&self) -> Result<()> {
            self.calls.borrow_mut().push("validate");
            if self.healthy.get() { Ok(()) } else { Err(Error::Invalid("worldserver exited".into())) }
        }
    }

    struct World {
        _d: tempfile::TempDir,
        root: PathBuf,
        meta: PathBuf,
        pkg: PathBuf,
        key: String,
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// An installation at version 1 (files recorded as pristine) and a signed update package.
    fn world(extra_update_files: &[(&str, &[u8])], migrations: bool) -> World {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        let meta_dir = d.path().join("srv.manager");
        let pkg = d.path().join("pkg");
        let src = d.path().join("upd");
        write(&root, "Core/worldserver.exe", b"world-v1");
        write(&root, "Core/authserver.exe", b"auth-v1");
        write(&root, "Settings/worldserver.conf.template", b"[worldserver]\nRate.XP.Kill = 3\n");
        write(&root, "user-notes.txt", b"mine");
        fs::create_dir_all(&meta_dir).unwrap();
        let mut meta = InstallMeta::new(InstallKind::New, &root);
        meta.core.version = Some("1.0.0".into());
        for f in ["Core/worldserver.exe", "Core/authserver.exe"] {
            meta.original_hashes.insert(f.into(), fsx::sha256_file(&root.join(f)).unwrap());
        }
        for sub in ["manifests", "backups", "updates", "staging"] {
            fs::create_dir_all(meta_dir.join(sub)).unwrap();
        }
        fsx::atomic_write_json(&meta_dir.join("install.json"), &meta).unwrap();

        write(&src, "Core/worldserver.exe", b"world-v2");
        write(&src, "Settings/worldserver.conf.template", b"[worldserver]\nRate.XP.Kill = 1\nNew.Setting = 5\n");
        write(&src, "Core/newfile.dll", b"dll");
        for (p, c) in extra_update_files {
            write(&src, p, c);
        }
        if migrations {
            write(&src, "_migrations/world/m1.sql", b"SELECT 1;");
        }
        let mut m = build(&src, &pkg, &BuildOptions { kind: Kind::Update, version: "2.0.0".into(), core_commit: None, built_at: "x".into(), part_size: 1 << 20, bots_commit: None, migrations: vec![] }, &|_| {}).unwrap();
        // Settings/* are merge-config by the packager; make the migration entry visible to the runner
        if migrations {
            m.migrations.push(crate::manifest::Migration { compatible_sha256: vec![], id: "m1".into(), db: "world".into(), sha256: fsx::sha256_bytes(b"SELECT 1;"), destructive: false });
            fs::write(pkg.join("manifest.json"), serde_json::to_vec_pretty(&m).unwrap()).unwrap();
        }
        let sk = SigningKey::generate(&mut rand_core::OsRng);
        let bytes = fs::read(pkg.join("manifest.json")).unwrap();
        fs::write(pkg.join("manifest.json.sig"), base64::engine::general_purpose::STANDARD.encode(sk.sign(&bytes).to_bytes())).unwrap();
        World { _d: d, root, meta: meta_dir, pkg, key: base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes()) }
    }

    fn run(w: &World, env: &Fake, res: BTreeMap<String, Resolution>, fail_after: Option<usize>) -> Result<Outcome> {
        apply(
            &Params { root: &w.root, meta_dir: &w.meta, source: Source::Dir(w.pkg.clone()), trusted_key: &w.key, cancel: Cancel::default(), resolutions: res, env, fail_after_ops: fail_after },
            &|_, _| {},
        )
    }

    fn read(w: &World, rel: &str) -> Vec<u8> {
        fs::read(w.root.join(rel)).unwrap()
    }

    #[test]
    fn imported_squid_repack_requires_a_decision_even_for_pristine_worldserver() {
        let w = world(&[], false);
        write(&w.root, "CoA-Bots/release.json", b"{}");
        let (_, mut meta) = MetaDir::open(&w.meta).unwrap();
        meta.kind = InstallKind::Imported;
        let manifest: Manifest = serde_json::from_slice(&fs::read(w.pkg.join("manifest.json")).unwrap()).unwrap();
        let action = |resolutions: BTreeMap<String, Resolution>| {
            plan(&w.root, &meta, &manifest, &resolutions, None).unwrap().into_iter().find(|item| item.path == "Core/worldserver.exe").unwrap().action
        };
        assert_eq!(action(BTreeMap::new()), Action::Conflict);
        assert_eq!(action(BTreeMap::from([("Core/worldserver.exe".into(), Resolution::Keep)])), Action::Skip);
        assert_eq!(action(BTreeMap::from([("Core/worldserver.exe".into(), Resolution::Replace)])), Action::Replace);
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    fn legacy_pending(w: &World, env: &Fake) -> Outcome {
        let was_healthy = env.healthy.replace(true);
        let mut out = run(w, env, BTreeMap::new(), None).unwrap();
        env.healthy.set(was_healthy);
        out.txn.state = State::NeedsDecision;
        let (m, _) = fetch_manifest(&Source::Dir(w.pkg.clone()), &w.key).unwrap();
        package::extract(&w.pkg, &m, &txn_dir(&w.meta, &out.txn.id).unwrap().join("tree"), &|_, _| {}).unwrap();
        save(&w.meta, &out.txn).unwrap();
        out
    }

    #[test]
    fn happy_path_replaces_creates_merges_and_records_ownership() {
        let w = world(&[], false);
        let env = Fake::ok();
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
        assert_eq!(out.txn.state, State::Committed);
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v2");
        assert_eq!(read(&w, "Core/authserver.exe"), b"auth-v1", "files not in the update are untouched");
        assert_eq!(read(&w, "Core/newfile.dll"), b"dll");
        assert_eq!(read(&w, "user-notes.txt"), b"mine");
        let tpl = String::from_utf8(read(&w, "Settings/worldserver.conf.template")).unwrap();
        assert!(tpl.contains("Rate.XP.Kill = 3"), "the user's value survives the merge: {tpl}");
        assert!(tpl.contains("New.Setting = 5"), "new keys are added");
        let (_, meta) = MetaDir::open(&w.meta).unwrap();
        assert_eq!(meta.core.version.as_deref(), Some("2.0.0"));
        assert_eq!(meta.original_hashes["Core/worldserver.exe"], fsx::sha256_bytes(b"world-v2"));
        assert_eq!(*env.calls.borrow(), ["stop", "snapshot", "validate"]);
        assert!(!w.root.join("Core/worldserver.exe.coa-new").exists());
    }

    #[test]
    fn a_file_modified_by_the_user_is_a_conflict_until_resolved() {
        let w = world(&[], false);
        write(&w.root, "Core/worldserver.exe", b"my-custom-build");
        let env = Fake::ok();
        let e = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(e.to_string().contains("need your decision"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"my-custom-build", "nothing changed");
        assert!(env.calls.borrow().is_empty(), "the server was never touched or stopped");

        let mut keep = BTreeMap::new();
        keep.insert("Core/worldserver.exe".to_string(), Resolution::Keep);
        run(&w, &env, keep, None).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"my-custom-build");
        assert_eq!(read(&w, "Core/newfile.dll"), b"dll", "the rest of the update still applied");

        let w2 = world(&[], false);
        write(&w2.root, "Core/worldserver.exe", b"my-custom-build");
        let mut rep = BTreeMap::new();
        rep.insert("Core/worldserver.exe".to_string(), Resolution::Replace);
        run(&w2, &Fake::ok(), rep, None).unwrap();
        assert_eq!(read(&w2, "Core/worldserver.exe"), b"world-v2");
        let t = unfinished(&w2.meta);
        assert!(t.is_none());
        let saved = fs::read_dir(w2.meta.join("updates")).unwrap().flatten().next().unwrap().path().join("before/Core/worldserver.exe");
        assert_eq!(fs::read(saved).unwrap(), b"my-custom-build", "the replaced custom build is kept for rollback");
    }

    #[test]
    fn failure_in_the_middle_of_applying_restores_every_file_byte_for_byte() {
        for fail_after in [0usize, 1, 2] {
            let w = world(&[], false);
            let snap: Vec<(String, Vec<u8>)> = ["Core/worldserver.exe", "Core/authserver.exe", "Settings/worldserver.conf.template", "user-notes.txt"].iter().map(|p| (p.to_string(), read(&w, p))).collect();
            let e = run(&w, &Fake::ok(), BTreeMap::new(), Some(fail_after)).unwrap_err();
            assert!(e.to_string().contains("simulated"));
            for (p, b) in &snap {
                assert_eq!(&read(&w, p), b, "{p} after failure at op {fail_after}");
            }
            assert!(!w.root.join("Core/newfile.dll").exists(), "files created by the update are removed again");
            let (_, meta) = MetaDir::open(&w.meta).unwrap();
            assert_eq!(meta.core.version.as_deref(), Some("1.0.0"), "version not bumped");
        }
    }

    #[test]
    fn unhealthy_new_build_is_automatically_restored() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let error = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(error.to_string().contains("did not start correctly"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(unfinished(&w.meta).is_none());
        let t = latest(&w.meta).unwrap().unwrap();
        assert_eq!(t.state, State::RolledBack);
        assert!(t.recovery_verified);
        assert!(txn_dir(&w.meta, &t.id).unwrap().join("diagnostic.json").is_file());
    }

    #[test]
    fn retrying_startup_commits_without_reapplying_sql_or_restoring_databases() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = legacy_pending(&w, &env);
        let calls = env.calls.borrow().len();
        env.healthy.set(true);
        let done = retry_validation(&w.root, &w.meta, &out.txn.id, &Source::Dir(w.pkg.clone()), &w.key, &env).unwrap();
        assert_eq!(done.state, State::Committed);
        assert_eq!(done.recovery_point, out.txn.recovery_point);
        assert_eq!(&env.calls.borrow()[calls..], &["stop", "validate"]);
        assert!(unfinished(&w.meta).is_none());
        assert_eq!(MetaDir::open(&w.meta).unwrap().1.core.version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn legacy_retry_uses_the_exact_signed_release_and_preserves_failure() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = legacy_pending(&w, &env);
        let dir = txn_dir(&w.meta, &out.txn.id).unwrap();
        fs::remove_file(dir.join("manifest.json")).unwrap();
        fs::remove_file(dir.join("manifest.json.sig")).unwrap();
        let pending = retry_validation(&w.root, &w.meta, &out.txn.id, &Source::Dir(w.pkg.clone()), &w.key, &env).unwrap();
        assert_eq!(pending.state, State::NeedsDecision);
        assert_eq!(pending.recovery_point, out.txn.recovery_point);
        assert!(pending.message.unwrap().contains("worldserver exited"));
        rollback(&w.root, &w.meta, &out.txn.id, &env).unwrap();
        assert!(retry_validation(&w.root, &w.meta, &out.txn.id, &Source::Dir(w.pkg.clone()), &w.key, &env).is_err());
    }

    #[test]
    fn retry_rejects_modified_binaries_and_unsigned_manifests() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = legacy_pending(&w, &env);
        write(&w.root, "Core/worldserver.exe", b"unexpected binary");
        assert!(retry_validation(&w.root, &w.meta, &out.txn.id, &Source::Dir(w.pkg.clone()), &w.key, &env).is_err());
        write(&w.root, "Core/worldserver.exe", b"world-v2");
        fs::write(txn_dir(&w.meta, &out.txn.id).unwrap().join("manifest.json.sig"), "invalid").unwrap();
        assert!(retry_validation(&w.root, &w.meta, &out.txn.id, &Source::Dir(w.pkg.clone()), &w.key, &env).is_err());
        assert_eq!(unfinished(&w.meta).unwrap().state, State::NeedsDecision);
    }

    #[test]
    fn crash_recovery_uses_the_journal() {
        let w = world(&[], false);
        // simulate a crash after two operations: run to failure, then reset the journal to a non-terminal state
        run(&w, &Fake::ok(), BTreeMap::new(), Some(2)).unwrap_err();
        let id = fs::read_dir(w.meta.join("updates")).unwrap().flatten().next().unwrap().file_name().to_string_lossy().into_owned();
        // pretend the process died mid-apply: files are in the half-applied state again
        write(&w.root, "Core/worldserver.exe", b"world-v2");
        let mut t = load(&w.meta, &id).unwrap();
        t.state = State::Applying;
        for op in t.ops.iter_mut() {
            op.started = op.path == "Core/worldserver.exe";
            op.done = op.started;
        }
        save(&w.meta, &t).unwrap();
        assert_eq!(unfinished(&w.meta).unwrap().id, id);
        rollback(&w.root, &w.meta, &id, &Fake::ok()).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn syntactically_valid_corrupt_journal_blocks_startup() {
        let w = world(&[], false);
        let out = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap();
        let path = txn_dir(&w.meta, &out.txn.id).unwrap().join("txn.json");
        let mut txn = load(&w.meta, &out.txn.id).unwrap();
        txn.id = "another-transaction".into();
        fsx::atomic_write_json(&path, &txn).unwrap();
        assert!(latest(&w.meta).is_err());
        assert!(ensure_recovered(&w.meta).is_err());
        txn.id = out.txn.id.clone();
        txn.ops[0].path = "../outside.exe".into();
        fsx::atomic_write_json(&path, &txn).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
        txn = out.txn.clone();
        txn.state = State::Prepared;
        fsx::atomic_write_json(&path, &txn).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
        txn = out.txn.clone();
        txn.ops[0].done = false;
        fsx::atomic_write_json(&path, &txn).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
        txn = out.txn.clone();
        txn.recovery_point = Some("../another-backup".into());
        fsx::atomic_write_json(&path, &txn).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
    }

    #[test]
    fn damaged_snapshot_blocks_recovery_before_stopping_or_changing_files() {
        struct Damaged;
        impl Env for Damaged {
            fn ensure_stopped(&self) -> Result<()> { panic!("recovery must verify first") }
            fn snapshot(&self) -> Result<String> { unreachable!() }
            fn restore_snapshot(&self, _: &str) -> Result<()> { panic!("damaged snapshot must not be imported") }
            fn migrate(&self, _: &Manifest, _: &Path) -> Result<ApplyReport> { unreachable!() }
            fn validate(&self) -> Result<()> { unreachable!() }
            fn verify_snapshot(&self, _: &str) -> Result<()> { Err(Error::Invalid("damaged database archive".into())) }
        }
        for databases_started in [false, true] {
            let w = world(&[], false);
            let mut txn = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap().txn;
            txn.databases_started = databases_started;
            txn.recovery_point = Some("rp-1".into());
            let before = txn_dir(&w.meta, &txn.id).unwrap().join("before");
            let current = read(&w, "Core/worldserver.exe");
            assert!(restore_transaction(&w.root, &w.meta, &before, &mut txn, &Damaged).is_err());
            assert_eq!(read(&w, "Core/worldserver.exe"), current);
        }
    }

    #[test]
    fn missing_journal_is_not_confused_with_completed_cleanup() {
        let w = world(&[], false);
        let out = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap();
        fs::remove_file(txn_dir(&w.meta, &out.txn.id).unwrap().join("txn.json")).unwrap();
        assert!(latest(&w.meta).is_err());
        assert!(ensure_recovered(&w.meta).is_err());
        let marker = cleanup_marker(&w.meta, &out.txn.id).unwrap();
        fsx::atomic_write_json(&marker, &CleanupMarker { id: out.txn.id.clone(), state: State::Committed, server_stopped: false }).unwrap();
        assert!(latest(&w.meta).unwrap().is_none());
        assert!(ensure_recovered(&w.meta).is_ok());
        fsx::atomic_write_json(&marker, &CleanupMarker { id: out.txn.id, state: State::Applying, server_stopped: false }).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
    }

    #[test]
    fn recovery_restores_configs_after_verification_and_stop_regenerate_them() {
        struct Regenerates<'a>(&'a Path, bool);
        impl Env for Regenerates<'_> {
            fn ensure_stopped(&self) -> Result<()> { Ok(()) }
            fn was_running(&self) -> Result<bool> { Ok(self.1) }
            fn snapshot(&self) -> Result<String> { Ok("rp-1".into()) }
            fn restore_snapshot(&self, _: &str) -> Result<()> { Ok(()) }
            fn migrate(&self, _: &Manifest, _: &Path) -> Result<ApplyReport> { unreachable!() }
            fn validate(&self) -> Result<()> { Err(Error::Invalid("startup failed".into())) }
            fn verify_restored(&self) -> Result<()> {
                fsx::atomic_write(&self.0.join("Core/configs/modules/generated.conf"), b"new verification config")?;
                fsx::atomic_write(&self.0.join("Settings/realm-profiles/coa.json"), b"new realm profile")?;
                fsx::atomic_write(&self.0.join("Settings/realm-profiles/wildcard.json"), b"newly generated profile")?;
                fsx::atomic_write(&self.0.join("mysql/my.ini"), b"generated mysql configuration")?;
                fsx::atomic_write(&self.0.join("Core/configs/worldserver.conf"), b"generated by verification")
            }
            fn restore_running_state(&self, running: bool) -> Result<()> {
                assert_eq!(running, self.1);
                fsx::atomic_write(&self.0.join("Settings/realm-profiles/coa.json"), b"profile regenerated on restart")?;
                fsx::atomic_write(&self.0.join("Core/configs/worldserver.conf"), b"generated by stop")
            }
        }
        for was_running in [false, true] {
        let w = world(&[], false);
        write(&w.root, "Core/configs/worldserver.conf", b"original user config\r\n");
        write(&w.root, "Settings/realm-profiles/coa.json", b"original realm profile");
        write(&w.root, "mysql/my.ini", b"original mysql configuration");
        let env = Regenerates(&w.root, was_running);
        assert!(apply(&Params { root: &w.root, meta_dir: &w.meta, source: Source::Dir(w.pkg.clone()), trusted_key: &w.key,
            cancel: Cancel::default(), resolutions: BTreeMap::new(), env: &env, fail_after_ops: None }, &|_, _| {}).is_err());
        assert_eq!(read(&w, "Core/configs/worldserver.conf"), b"original user config\r\n");
        assert!(!w.root.join("Core/configs/modules/generated.conf").exists());
        assert!(!w.root.join("Settings/realm-profiles/wildcard.json").exists());
        assert_eq!(read(&w, "Settings/realm-profiles/coa.json"), b"original realm profile");
        assert_eq!(read(&w, "mysql/my.ini"), b"original mysql configuration");
        let recovered = latest(&w.meta).unwrap().unwrap();
        assert!(recovered.recovery_verified);
        assert_eq!(recovered.failure_stage, Some(State::Validating));
        assert_eq!(recovered.was_running, was_running);
        }
    }

    #[test]
    fn a_running_server_or_failed_backup_blocks_the_update_before_any_change() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.stopped.set(false);
        assert!(run(&w, &env, BTreeMap::new(), None).is_err());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        let w = world(&[], false);
        let env = Fake::ok();
        env.snapshot_ok.set(false);
        let e = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(e.to_string().contains("safety backup failed"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn a_failed_migration_undoes_the_files_and_reports_it() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.migrate_fail.set(true);
        let e = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(e.to_string().contains("Database update m1 failed"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(env.calls.borrow().contains(&"restore-databases"));
        assert!(!env.calls.borrow().contains(&"validate"), "the new build is never started after a failed migration");
        let ok = Fake::ok();
        let out = run(&w, &ok, BTreeMap::new(), None).unwrap();
        assert_eq!(out.txn.state, State::Committed);
        assert_eq!(*ok.calls.borrow(), ["stop", "snapshot", "migrate", "validate"]);
    }

    #[test]
    fn failed_database_recovery_keeps_new_files_and_blocks_retry_until_recovered() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.migrate_fail.set(true);
        env.restore_fail.set(true);
        let error = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(error.to_string().contains("Recovery failed"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v2");
        let pending = unfinished(&w.meta).unwrap();
        assert_eq!(pending.state, State::Failed);
        assert!(pending.databases_started);
        assert!(txn_dir(&w.meta, &pending.id).unwrap().join("tree").exists());
        assert!(run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap_err().to_string().contains("unfinished"));
        rollback(&w.root, &w.meta, &pending.id, &Fake::ok()).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(unfinished(&w.meta).is_none());
    }

    #[test]
    fn interrupted_migration_is_restored_before_files_and_never_replayed() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = legacy_pending(&w, &env);
        let recovery = Fake::ok();
        rollback(&w.root, &w.meta, &out.txn.id, &recovery).unwrap();
        assert_eq!(*recovery.calls.borrow(), ["stop", "restore-databases"]);
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn concurrent_updates_are_rejected_without_changes() {
        let w = world(&[], false);
        let _lock = operation_lock(&w.meta).unwrap();
        assert!(run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap_err().to_string().contains("in progress"));
        assert!(crate::driver::run(&w.root, crate::driver::Verb::StartAll).unwrap_err().to_string().contains("in progress"));
        assert!(crate::modules::set_enabled(&w.root, &w.meta, "companions", true).unwrap_err().to_string().contains("in progress"));
        assert!(crate::realms::select(&w.root, crate::realms::Mode::Wildcard).unwrap_err().to_string().contains("in progress"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn interrupted_repair_restores_files_databases_and_ownership_metadata() {
        let w = world(&[], false);
        let files = vec![
            FileEntry { path: "Core/worldserver.exe".into(), sha256: fsx::sha256_bytes(b"repaired"), size: 8, owner: crate::manifest::Owner::Core, policy: ReplacePolicy::Replace },
            FileEntry { path: "Core/repair-created.dll".into(), sha256: fsx::sha256_bytes(b"created"), size: 7, owner: crate::manifest::Owner::Core, policy: ReplacePolicy::Replace },
        ];
        let mut txn = begin_repair(&w.root, &w.meta, "1.0.0", "full-backup", &files).unwrap();
        write(&w.root, "Core/worldserver.exe", b"repaired");
        write(&w.root, "Core/repair-created.dll", b"created");
        repair_migrating(&w.meta, &mut txn).unwrap();
        assert_eq!(unfinished(&w.meta).unwrap().id, txn.id);
        let env = Fake::ok();
        rollback(&w.root, &w.meta, &txn.id, &env).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(!w.root.join("Core/repair-created.dll").exists());
        assert_eq!(*env.calls.borrow(), ["stop", "restore-databases"]);
        assert!(unfinished(&w.meta).is_none());
    }

    #[test]
    fn unreadable_update_journal_blocks_changes_and_startup() {
        let w = world(&[], false);
        write(&w.meta, "updates/broken/txn.json", b"{truncated");
        assert!(run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap_err().to_string().contains("cannot be read"));
        assert!(crate::driver::run(&w.root, crate::driver::Verb::StartAll).unwrap_err().to_string().contains("cannot be read"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn crash_during_commit_restores_the_original_installation_metadata() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = legacy_pending(&w, &env);
        let (_, mut meta) = MetaDir::open(&w.meta).unwrap();
        meta.core.version = Some("2.0.0".into());
        meta.core.commit = Some("a".repeat(40));
        fsx::atomic_write_json(&w.meta.join("install.json"), &meta).unwrap();
        rollback(&w.root, &w.meta, &out.txn.id, &Fake::ok()).unwrap();
        let (_, restored) = MetaDir::open(&w.meta).unwrap();
        assert_eq!(restored.core.version.as_deref(), Some("1.0.0"));
        assert_eq!(restored.core.commit, None);
    }

    #[test]
    fn damaged_original_file_blocks_recovery_without_announcing_success() {
        let w = world(&[], false);
        let out = legacy_pending(&w, &Fake::ok());
        write(&txn_dir(&w.meta, &out.txn.id).unwrap().join("before"), "Core/worldserver.exe", b"damaged backup");
        let error = rollback(&w.root, &w.meta, &out.txn.id, &Fake::ok()).unwrap_err();
        assert!(error.to_string().contains("damaged"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v2");
        let journal = load(&w.meta, &out.txn.id).unwrap();
        assert_eq!(journal.state, State::Restoring);
        assert!(!journal.recovery_verified);
        assert!(ensure_recovered(&w.meta).is_err());
    }

    #[test]
    fn bad_signature_and_wrong_package_kind_change_nothing() {
        let w = world(&[], false);
        let other = SigningKey::generate(&mut rand_core::OsRng);
        let wrong_key = base64::engine::general_purpose::STANDARD.encode(other.verifying_key().to_bytes());
        let env = Fake::ok();
        let r = apply(&Params { root: &w.root, meta_dir: &w.meta, source: Source::Dir(w.pkg.clone()), trusted_key: &wrong_key, cancel: Cancel::default(), resolutions: BTreeMap::new(), env: &env, fail_after_ops: None }, &|_, _| {});
        assert!(r.is_err());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(env.calls.borrow().is_empty());
        let (_, meta) = MetaDir::open(&w.meta).unwrap();
        let p = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap();
        assert_eq!(p.to_version, "2.0.0");
        assert!(p.items.iter().any(|i| i.path == "Core/newfile.dll" && i.action == Action::Create));
    }

    #[test]
    fn execution_reports_rejection_restoration_and_required_intervention() {
        let attempt = |w: &World, env: &Fake| execute(&Params { root: &w.root, meta_dir: &w.meta, source: Source::Dir(w.pkg.clone()), trusted_key: &w.key, cancel: Cancel::default(), resolutions: BTreeMap::new(), env, fail_after_ops: None }, &|_, _| {}).unwrap();
        let w = world(&[], false);
        fs::write(w.pkg.join("manifest.json.sig"), "invalid signature").unwrap();
        let rejected = attempt(&w, &Fake::ok());
        assert!(matches!(rejected.disposition, Disposition::Rejected));
        assert!(rejected.txn.is_none());
        assert!(w.meta.join("logs/update-result.json").is_file());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        let w = world(&[], true);
        let env = Fake::ok();
        env.migrate_fail.set(true);
        let restored = attempt(&w, &env);
        assert!(matches!(restored.disposition, Disposition::Restored));
        assert!(restored.txn.unwrap().recovery_verified);
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        let w = world(&[], true);
        env.restore_fail.set(true);
        let blocked = attempt(&w, &env);
        assert!(matches!(blocked.disposition, Disposition::RecoveryRequired));
        assert!(!blocked.txn.unwrap().recovery_verified);
        assert!(ensure_recovered(&w.meta).is_err());
    }
}
