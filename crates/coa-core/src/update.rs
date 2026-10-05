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
    Applying,
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
}

fn legacy_databases_started() -> bool { true }

/// Everything the transaction needs from the outside world; tests provide a fake.
pub trait Env {
    fn ensure_stopped(&self) -> Result<()>;
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
    Ok(updates_dir(meta).join(id))
}

fn save(meta: &Path, t: &Txn) -> Result<()> {
    fsx::atomic_write_json(&txn_dir(meta, &t.id)?.join("txn.json"), t)
}

pub fn load(meta: &Path, id: &str) -> Result<Txn> {
    fsx::read_json(&txn_dir(meta, id)?.join("txn.json"))
}

/// A transaction that started but never reached a final state (crash, power loss). The UI offers rollback.
pub fn unfinished(meta: &Path) -> Option<Txn> {
    let rd = fs::read_dir(updates_dir(meta)).ok()?;
    let mut all: Vec<Txn> = rd.flatten().filter_map(|e| fsx::read_json::<Txn>(&e.path().join("txn.json")).ok()).collect();
    all.sort_by(|a, b| b.id.cmp(&a.id));
    all.into_iter().find(|t| !matches!(t.state, State::Committed | State::RolledBack | State::Prepared) && (t.state != State::Failed || t.databases_started || t.ops.iter().any(|o| o.started)))
}

pub fn ensure_recovered(meta: &Path) -> Result<()> {
    match fs::read_dir(updates_dir(meta)) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let journal = entry.path().join("txn.json");
                if journal.exists() {
                    fsx::read_json::<Txn>(&journal).map_err(|e| Error::Invalid(format!("Update journal {} cannot be read: {e}. Recover it before changing or starting the server.", journal.display())))?;
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
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
                let manager_launcher = f.path == "Scripts/manage.py" && recorded.is_some_and(|hash| {
                    fs::read(&target).is_ok_and(|bytes| crate::driver::launcher_matches_recorded(hash, &bytes))
                });
                if f.path == "Core/worldserver.exe" && meta.kind == crate::registry::InstallKind::Imported && crate::squid::imported_repack(root) {
                    match resolutions.get(&f.path) {
                        Some(Resolution::Replace) => (Action::Replace, Some("Replacing this binary stops the original SquidBots launcher/updater from accepting its hash.".into())),
                        Some(Resolution::Keep) => (Action::Skip, Some("kept the original SquidBots binary".into())),
                        None => (Action::Conflict, Some("This SquidBots repack verifies its worldserver hash. Replacing it stops Start_All_Bots/coa_update from working. Choose explicitly whether to replace this binary.".into())),
                    }
                } else if recorded.map(|r| r.eq_ignore_ascii_case(c)).unwrap_or(false) || manager_launcher {
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

fn step(report: &dyn Fn(&str, u8), name: &str, pct: u8) {
    tracing::info!(step = name, percent = pct, "server update progress");
    report(name, pct);
}

/// Run the whole update. On failure before the new build is validated the files are rolled back automatically;
/// a build that is applied but unhealthy is left in `NeedsDecision` so the owner chooses.
pub fn apply(p: &Params, report: &dyn Fn(&str, u8)) -> Result<Outcome> {
    let (root, meta_dir) = (p.root, p.meta_dir);
    let _lock = operation_lock(meta_dir)?;
    ensure_recovered(meta_dir)?;
    let (md, mut meta) = MetaDir::open(meta_dir)?;
    let _ = md;

    step(report, "Checking the update", 2);
    let (m, manifest_bytes) = fetch_manifest(&p.source, p.trusted_key)?;
    tracing::info!(root = %root.display(), from_version = ?meta.core.version, to_version = %m.version, migrations = m.migrations.len(), "server update selected");
    check_manifest(&m)?;
    let archive = m.archive.clone().ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
    let id = format!("{}-{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), m.version.replace('.', "_"), uuid::Uuid::new_v4().simple());
    let tdir = txn_dir(meta_dir, &id)?;
    let (tree, before) = (tdir.join("tree"), tdir.join("before"));
    fs::create_dir_all(&before)?;
    let signature = crate::pkgsource::fetch_small(&p.source, "manifest.json.sig")?;
    crate::signing::verify(&manifest_bytes, &String::from_utf8_lossy(&signature), p.trusted_key)?;
    fsx::atomic_write(&tdir.join("manifest.json"), &manifest_bytes)?;
    fsx::atomic_write(&tdir.join("manifest.json.sig"), &signature)?;
    fsx::atomic_write(&before.join("manager-install.json"), &fs::read(meta_dir.join("install.json"))?)?;

    let mut txn = Txn { id: id.clone(), state: State::Prepared, from_version: meta.core.version.clone(), to_version: m.version.clone(), recovery_point: None, databases_started: false, ops: Vec::new(), message: None };
    save(meta_dir, &txn)?;

    // Nothing has touched the installation yet; failures up to here just discard the staging area.
    let staged = (|| -> Result<PathBuf> {
        step(report, "Downloading the update", 5);
        let parts = fetch_parts(&p.source, &m, &meta_dir.join("staging").join("download"), &p.cancel, &|f, _| step(report, "Downloading the update", 5 + (f * 35.0) as u8))?;
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

    step(report, "Making sure the server is stopped", 48);
    p.env.ensure_stopped()?;
    step(report, "Saving a recovery point", 52);
    txn.recovery_point = Some(p.env.snapshot().map_err(|e| Error::Invalid(format!("The update was not applied because the safety backup failed: {e}")))?);

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
    txn.state = State::Applied;
    save(meta_dir, &txn)?;

    // MySQL DDL can commit before a later statement fails. Restore the full recovery point on failure.
    let mut migrated = None;
    if !m.migrations.is_empty() {
        step(report, "Updating the database", 75);
        txn.databases_started = true;
        save(meta_dir, &txn)?;
        match p.env.migrate(&m, &tree.join("_migrations")) {
            Ok(r) if r.failed.is_none() => migrated = Some(r),
            Ok(r) => {
                let (mid, why) = r.failed.clone().unwrap();
                return fail_after_apply(p, &mut txn, &before, &tree, format!("Database update {mid} failed: {why}"), Some(r));
            }
            Err(e) => return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), None),
        }
    }

    step(report, "Starting the updated server", 85);
    if let Err(e) = p.env.validate() {
        txn.state = State::NeedsDecision;
        txn.message = Some(format!("The updated server did not start correctly: {e}"));
        save(meta_dir, &txn)?;
        return Ok(Outcome { txn, migrations: migrated });
    }

    finish(root, meta_dir, &mut meta, &m, &tree, &mut txn)?;
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
    fsx::atomic_write_json(&meta_dir.join("install.json"), meta)?;
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
    let restored = restore_transaction(p.root, p.meta_dir, before, txn, p.env);
    tracing::info!(transaction = %txn.id, recovered = restored.is_ok(), "server update recovery finished");
    txn.state = if restored.is_ok() { State::RolledBack } else { State::Failed };
    txn.message = Some(match &restored { Ok(()) => why.clone(), Err(e) => format!("{why} Recovery failed: {e}") });
    save(p.meta_dir, txn)?;
    if restored.is_ok() { let _ = fs::remove_dir_all(tree); }
    let _ = report;
    Err(Error::Invalid(match restored {
        Ok(()) if txn.databases_started => format!("{why} The server files and databases were restored to the recovery point."),
        Ok(()) => format!("{why} The server files were restored."),
        Err(e) => format!("{why} Recovery failed: {e}. Resolve the unfinished update before starting the server."),
    }))
}

fn restore_transaction(root: &Path, meta: &Path, before: &Path, txn: &mut Txn, env: &dyn Env) -> Result<()> {
    env.ensure_stopped()?;
    if txn.databases_started {
        let id = txn.recovery_point.as_deref().ok_or_else(|| Error::Invalid("The database recovery point is missing.".into()))?;
        env.restore_snapshot(id)?;
    }
    restore_files(root, before, txn)?;
    let saved_meta = before.join("manager-install.json");
    if saved_meta.exists() {
        fsx::atomic_write(&meta.join("install.json"), &fs::read(saved_meta)?)?;
    }
    Ok(())
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
            fsx::atomic_write(&keep, &fs::read(&target)?)?;
        }
        // Journal only after the old bytes are safely saved; a crash during backup must not
        // turn an untouched file into a rollback operation with a missing saved copy.
        txn.ops[i].started = true;
        save(meta_dir, txn)?;
        fs::create_dir_all(target.parent().unwrap())?;
        match action {
            Action::Create | Action::Replace => {
                // Same-volume temp file + rename: the target is either the old or the new file, never half of one.
                let tmp = target.with_file_name(format!("{}.coa-new", target.file_name().unwrap().to_string_lossy()));
                fs::copy(&staged, &tmp)?;
                if let Err(e) = fs::rename(&tmp, &target) {
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
        txn.ops[i].done = true;
        save(meta_dir, txn)?;
        completed += 1;
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
    restore_transaction(root, meta_dir, &txn_dir(meta_dir, id)?.join("before"), &mut txn, env)?;
    txn.state = State::RolledBack;
    txn.message = Some("rolled back".into());
    save(meta_dir, &txn)?;
    let _ = fs::remove_dir_all(txn_dir(meta_dir, id)?.join("tree"));
    Ok(txn)
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
    let txn = Txn { id, state: State::Applying, from_version: Some(version.into()), to_version: version.into(), recovery_point: Some(backup.into()), databases_started: false, ops, message: Some("Repair is in progress.".into()) };
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
    let mut dirs: Vec<_> = rd.flatten().filter(|e| fsx::read_json::<Txn>(&e.path().join("txn.json")).is_ok_and(|t| matches!(t.state, State::Committed | State::RolledBack))).map(|e| e.path()).collect();
    dirs.sort();
    while dirs.len() > keep {
        let _ = fs::remove_dir_all(dirs.remove(0));
    }
}

/// The real environment: a repack-shaped installation driven through its launcher.
pub struct RepackEnv<'a> {
    pub root: &'a Path,
    pub meta_dir: &'a Path,
}

impl RepackEnv<'_> {
    pub(crate) fn verify_recovery_point(&self, point: &crate::backup::RecoveryPoint) -> Result<()> {
        if point.kind != crate::backup::Kind::Full || !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) {
            return Err(Error::Invalid("The update recovery point does not contain every required database and configuration.".into()));
        }
        if !crate::backup::verify(self.meta_dir, &point.id)?.ok { return Err(Error::Invalid("The recovery point failed verification.".into())); }
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

impl Env for RepackEnv<'_> {
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
        if point.kind != crate::backup::Kind::Full { return Err(Error::Invalid("Database rollback requires a full recovery point.".into())); }
        if !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) || !crate::backup::verify(self.meta_dir, id)?.ok {
            return Err(Error::Invalid("The full database recovery point is incomplete or damaged.".into()));
        }
        for component in point.components.iter().filter(|c| c.sha256.is_some()) {
            crate::backup::restore_database(self.root, self.meta_dir, id, &component.name)?;
        }
        crate::backup::restore_configs(self.root, self.meta_dir, id)?;
        Ok(())
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
        use crate::process::{observe, ServiceState};
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
                let _ = crate::config::materialize_module_configs(self.root);
            }
            // An imported server gets the files it lacks (nothing it has is changed).
            _ => {
                let _ = crate::config::create_missing_module_configs(self.root);
            }
        }
        let started = match crate::driver::validate_update(self.root) {
            Ok(out) => out,
            Err(e) => {
                let _ = crate::driver::run(self.root, crate::driver::Verb::StopAll);
                return Err(e);
            }
        };
        let ports = crate::layout::read_ports(self.root);
        let healthy = started.ok && {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            let mut ready_since = None;
            loop {
                let o = observe(self.root, &ports);
                if o.mysql.state == ServiceState::Running && o.auth.state == ServiceState::Running && o.world.state == ServiceState::Running && o.secondary_world.as_ref().is_none_or(|s| s.state == ServiceState::Running) {
                    let since = ready_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= std::time::Duration::from_secs(10) { break true; }
                } else {
                    ready_since = None;
                }
                if std::time::Instant::now() > deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        };
        if healthy {
            return Ok(());
        }
        let cause = crate::driver::startup_failure(self.root, &started);
        // Leave the files unlocked so that a rollback can replace them.
        let _ = crate::driver::run(self.root, crate::driver::Verb::StopAll);
        Err(Error::Invalid(cause))
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
            m.migrations.push(crate::manifest::Migration { id: "m1".into(), db: "world".into(), sha256: fsx::sha256_bytes(b"SELECT 1;"), destructive: false });
            fs::write(pkg.join("manifest.json"), serde_json::to_vec_pretty(&m).unwrap()).unwrap();
        }
        let sk = SigningKey::generate(&mut rand_core::OsRng);
        let bytes = fs::read(pkg.join("manifest.json")).unwrap();
        fs::write(pkg.join("manifest.json.sig"), base64::engine::general_purpose::STANDARD.encode(sk.sign(&bytes).to_bytes())).unwrap();
        World { _d: d, root, meta: meta_dir, pkg, key: base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes()) }
    }

    const LEGACY_LAUNCHER: &str = "import sys\nROOT = Path(__file__).resolve().parents[1]\nfrom squid_playerbots import validate_bots\n(\"WorldDatabaseInfo\", \"acore_world\")\n(\"CharacterDatabaseInfo\", \"acore_characters\")\nSET name='AzerothCore',address=\nWHERE id=1;\nmysql(\"UPDATE acore_auth.realmlist SET flag=0\nif __name__ == \"__main__\":\n";

    fn legacy_launcher_world(import_patch: bool) -> World {
        let w = world(&[("Scripts/manage.py", b"new release launcher")], false);
        let mut source = crate::realms::patch_launcher(LEGACY_LAUNCHER).unwrap();
        if import_patch { source = crate::driver::patch_launcher_imports(&source).unwrap(); }
        write(&w.root, "Scripts/manage.py", source.as_bytes());
        let (_, mut meta) = MetaDir::open(&w.meta).unwrap();
        // Older Managers retained the signed original hash after adding realm profiles.
        meta.original_hashes.insert("Scripts/manage.py".into(), fsx::sha256_bytes(LEGACY_LAUNCHER.as_bytes()));
        fsx::atomic_write_json(&w.meta.join("install.json"), &meta).unwrap();
        w
    }

    #[test]
    fn legacy_manager_realm_launcher_is_replaced_during_update() {
        let w = legacy_launcher_world(false);
        let outcome = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap();
        assert_eq!(outcome.txn.state, State::Committed);
        assert_eq!(read(&w, "Scripts/manage.py"), b"new release launcher");
    }

    #[test]
    fn legacy_realm_and_import_repairs_are_replaced_during_update() {
        let w = legacy_launcher_world(true);
        let outcome = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap();
        assert_eq!(outcome.txn.state, State::Committed);
        assert_eq!(read(&w, "Scripts/manage.py"), b"new release launcher");
    }

    #[test]
    fn genuine_launcher_edits_still_require_a_decision_and_can_be_kept() {
        let w = legacy_launcher_world(true);
        let mut custom = read(&w, "Scripts/manage.py");
        custom.extend_from_slice(b"\n# owner's custom integration\n");
        write(&w.root, "Scripts/manage.py", &custom);
        let (_, meta) = MetaDir::open(&w.meta).unwrap();
        let manifest = Manifest::parse(&fs::read(w.pkg.join("manifest.json")).unwrap()).unwrap();
        let items = plan(&w.root, &meta, &manifest, &BTreeMap::new(), None).unwrap();
        assert_eq!(items.iter().find(|item| item.path == "Scripts/manage.py").unwrap().action, Action::Conflict);
        let resolutions = BTreeMap::from([("Scripts/manage.py".into(), Resolution::Keep)]);
        let outcome = run(&w, &Fake::ok(), resolutions, None).unwrap();
        assert_eq!(outcome.txn.state, State::Committed);
        assert_eq!(read(&w, "Scripts/manage.py"), custom);
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
    fn unhealthy_new_build_is_kept_until_the_owner_rolls_back() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
        assert_eq!(out.txn.state, State::NeedsDecision);
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v2", "not undone automatically");
        assert_eq!(unfinished(&w.meta).unwrap().id, out.txn.id);
        let (_, meta) = MetaDir::open(&w.meta).unwrap();
        assert_eq!(meta.core.version.as_deref(), Some("1.0.0"), "not committed");

        rollback(&w.root, &w.meta, &out.txn.id, &env).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(!w.root.join("Core/newfile.dll").exists());
        assert!(unfinished(&w.meta).is_none());
        assert!(rollback(&w.root, &w.meta, &out.txn.id, &env).is_err(), "already rolled back");
    }

    #[test]
    fn retrying_startup_commits_without_reapplying_sql_or_restoring_databases() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.healthy.set(false);
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
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
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
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
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
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
        }
        save(&w.meta, &t).unwrap();
        assert_eq!(unfinished(&w.meta).unwrap().id, id);
        rollback(&w.root, &w.meta, &id, &Fake::ok()).unwrap();
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
    }

    #[test]
    fn a_running_server_or_failed_backup_blocks_the_update_before_any_change() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.stopped.set(false);
        assert!(run(&w, &env, BTreeMap::new(), None).is_err());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
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
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
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
        let out = run(&w, &env, BTreeMap::new(), None).unwrap();
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
}
