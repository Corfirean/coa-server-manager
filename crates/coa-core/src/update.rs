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
    #[serde(default)]
    pub recovery_hashes: BTreeMap<String, String>,
}

fn legacy_databases_started() -> bool { true }

/// Everything the transaction needs from the outside world; tests provide a fake.
pub trait Env {
    fn activate(&self) -> Result<()> { Ok(()) }
    fn recover_validation(&self) -> Result<()> { Ok(()) }
    fn automatic_rollback(&self) -> bool { false }
    fn validate_recovery(&self) -> Result<()> { self.validate() }
    fn ensure_stopped(&self) -> Result<()>;
    fn preflight(&self, _manifest: &Manifest) -> Result<()> { Ok(()) }
    fn verify_snapshot(&self, _id: &str) -> Result<()> { Ok(()) }
    fn rehearse(&self, _manifest: &Manifest, _tree: &Path, _items: &[PlanItem], _point: &str) -> Result<()> { Ok(()) }
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
    read_journal(&txn_dir(meta, id)?.join("txn.json"))
}

fn read_journal(path: &Path) -> Result<Txn> {
    let txn: Txn = fsx::read_json(path).map_err(|e| Error::Invalid(format!("Update journal {} cannot be read: {e}. Available recovery copies were preserved.", path.display())))?;
    if path.parent().and_then(Path::file_name).and_then(|id| id.to_str()) != Some(txn.id.as_str()) { return Err(Error::Invalid("Update journal identity is inconsistent.".into())); }
    txn_dir(path.parent().unwrap(), &txn.id)?;
    if let Some(id) = &txn.recovery_point {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || id.contains("..") { return Err(Error::Invalid("Update journal contains an invalid recovery point identity.".into())); }
    }
    let valid_hash = |hash: &str| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    let mut seen = std::collections::BTreeSet::new();
    for op in &txn.ops {
        fsx::safe_join(path.parent().unwrap(), &op.path)?;
        if !seen.insert(op.path.replace('\\', "/").to_ascii_lowercase()) || !valid_hash(&op.new_sha256)
            || (op.done && !op.started && !matches!(op.action, Action::Skip | Action::Conflict))
            || (txn.state == State::Prepared && op.started)
            || (txn.state == State::Committed && !op.done) {
            return Err(Error::Invalid("Update journal contains inconsistent file operations.".into()));
        }
    }
    if txn.recovery_hashes.values().any(|hash| !valid_hash(hash)) { return Err(Error::Invalid("Update journal contains invalid recovery hashes.".into())); }
    for key in txn.recovery_hashes.keys().filter(|key| !matches!(key.as_str(), "@install" | "@point")) { fsx::safe_join(path.parent().unwrap(), key)?; }
    Ok(txn)
}

#[derive(Serialize, Deserialize)]
struct CleanupMarker { id: String, state: State }

fn cleanup_marker(meta: &Path, id: &str) -> Result<PathBuf> {
    txn_dir(meta, id)?;
    Ok(meta.join("update-cleanup").join(format!("{id}.json")))
}

fn journals(meta: &Path) -> Result<Vec<Txn>> {
    let entries = match fs::read_dir(updates_dir(meta)) { Ok(entries) => entries, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]), Err(e) => return Err(e.into()) };
    let mut all = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_symlink() { return Err(Error::Invalid("Linked update storage cannot be recovered safely.".into())); }
        if !entry.file_type()?.is_dir() { continue; }
        let dir = fsx::ensure_within(meta, &entry.path())?;
        let path = dir.join("txn.json");
        if !path.is_file() {
            let id = entry.file_name().to_string_lossy().into_owned();
            let marker = cleanup_marker(meta, &id)?;
            if fsx::read_json::<CleanupMarker>(&marker).is_ok_and(|m| m.id == id && matches!(m.state, State::Committed | State::RolledBack)) { continue; }
            return Err(Error::Invalid(format!("Update {id} has no journal. Preserve its recovery copies and inspect it before starting or changing the server.")));
        }
        all.push(read_journal(&path)?);
    }
    all.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(all)
}

pub fn pending_checked(meta: &Path) -> Result<Option<Txn>> {
    Ok(journals(meta)?.into_iter().find(|t| !matches!(t.state, State::Committed | State::RolledBack | State::Prepared) && (t.state != State::Failed || t.databases_started || t.ops.iter().any(|o| o.started))))
}

/// A transaction that started but never reached a final state (crash, power loss). The UI offers rollback.
pub fn unfinished(meta: &Path) -> Option<Txn> {
    pending_checked(meta).ok().flatten()
}

pub fn ensure_recovered(meta: &Path) -> Result<()> {
    if crate::update_isolation::pending(meta) {
        return Err(Error::Invalid("Interrupted startup validation must be recovered before starting the server.".into()));
    }
    if let Some(t) = pending_checked(meta)? {
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
    // An update is decided by version. The same version has the same files, so a file the owner edited (or removed)
    // is not an update: Repair lists such files and restores them on request.
    let same_version = meta.core.version.as_deref().is_some_and(|installed| installed == manifest.version);
    for f in manifest.files.iter().filter(|f| !f.path.starts_with(STAGED_PREFIX)) {
        let target = fsx::ensure_within(root, &fsx::safe_join(root, &f.path)?)?;
        if same_version {
            out.push(PlanItem { path: f.path.clone(), action: Action::Skip, reason: Some("same version; Repair restores changed files".into()) });
            continue;
        }
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
            (_, Some(_)) if f.path == "Scripts/manage.py" && staged.is_some_and(|tree| {
                match (fs::read_to_string(tree.join(&f.path)), fs::read(&target)) {
                    (Ok(signed), Ok(actual)) => fsx::sha256_bytes(signed.as_bytes()) == f.sha256 && crate::driver::launcher_matches(&signed, &actual),
                    _ => false,
                }
            }) => (Action::Skip, Some("already up to date with Manager integration".into())),
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
    pub pending_migrations: usize,
    pub download_bytes: u64,
    /// False when the pending SQL was not counted (database stopped during a background check, or an error).
    pub database_checked: bool,
    /// Why the database could not be inspected. The check still answers about the files; applying the update starts
    /// the database itself and reports its own failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_check_error: Option<String>,
}

/// Whether looking for an update may start the database to count pending SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseAccess {
    /// An explicit check: start the database if it is stopped (and stop it again afterwards).
    Start,
    /// The background check that repeats every few minutes must not start and stop MySQL on a stopped server.
    OnlyIfRunning,
}

/// The update packages are those of the Windows repack (executables, a bundled MySQL, a launcher): applying one to a Docker
/// server would put Windows files in a Linux server folder. Updating a Docker server is separate work.
fn refuse_for_docker(root: &Path) -> Result<()> {
    if crate::docker::is_docker(root) {
        return Err(Error::Invalid("Updates are not available for Docker servers yet.".into()));
    }
    Ok(())
}

/// Describe the update; a managed launcher may need a one-time authenticated payload check.
pub fn preview(root: &Path, meta: &InstallMeta, source: &Source, trusted_key: &str, resolutions: &BTreeMap<String, Resolution>) -> Result<Preview> {
    preview_with(root, meta, source, trusted_key, resolutions, DatabaseAccess::Start)
}

pub fn preview_with(root: &Path, meta: &InstallMeta, source: &Source, trusted_key: &str, resolutions: &BTreeMap<String, Resolution>, access: DatabaseAccess) -> Result<Preview> {
    refuse_for_docker(root)?;
    let meta_dir = crate::registry::metadata_dir_for(root)?;
    let _lock = operation_lock(&meta_dir)?;
    ensure_recovered(&meta_dir)?;
    let (m, _) = fetch_manifest(source, trusted_key)?;
    check_manifest(&m)?;
    reject_downgrade(meta, &m)?;
    let mut items = plan(root, meta, &m, resolutions, None)?;
    if items.iter().any(|item| item.path == "Scripts/manage.py" && item.action != Action::Skip)
        && launcher_already_integrated(root, &meta_dir, meta, &m, source)? {
        if let Some(item) = items.iter_mut().find(|item| item.path == "Scripts/manage.py") {
            item.action = Action::Skip;
            item.reason = Some("already up to date with Manager integration".into());
        }
    }
    // Counting pending SQL needs the database. If it cannot be started the files can still be compared, so report
    // that instead of failing the whole check.
    let (pending_migrations, database_checked, database_check_error) = match (RepackEnv { root, meta_dir: &meta_dir }).pending_migrations_with(&m, access) {
        Ok(Some(count)) => (count, true, None),
        Ok(None) => (m.migrations.len(), false, None),
        Err(error) => {
            tracing::warn!(%error, "update check: the database could not be inspected");
            (m.migrations.len(), false, Some(error.to_string()))
        }
    };
    Ok(Preview {
        from_version: meta.core.version.clone(),
        to_version: m.version.clone(),
        conflicts: items.iter().filter(|i| i.action == Action::Conflict).map(|i| i.path.clone()).collect(),
        items,
        migrations: m.migrations.len(),
        pending_migrations,
        download_bytes: m.archive.as_ref().map(|a| a.parts.iter().map(|p| p.size).sum()).unwrap_or(0),
        database_checked,
        database_check_error,
    })
}

fn launcher_already_integrated(root: &Path, meta_dir: &Path, meta: &InstallMeta, manifest: &Manifest, source: &Source) -> Result<bool> {
    let Some(file) = manifest.files.iter().find(|file| file.path == "Scripts/manage.py") else { return Ok(false); };
    let actual = sha_if_exists(&root.join(&file.path));
    if actual.as_ref() != meta.original_hashes.get(&file.path) || actual.is_none() { return Ok(false); }
    let cache = meta_dir.join("manifests").join(format!("launcher-{}.py", file.sha256));
    let signed = match fs::read(&cache) {
        Ok(bytes) if fsx::sha256_bytes(&bytes) == file.sha256 => bytes,
        _ => {
            let temp = tempfile::tempdir()?;
            let parts = fetch_parts(source, manifest, &temp.path().join("download"), &Cancel::default(), &|_, _| {})?;
            package::extract(&parts, manifest, &temp.path().join("tree"), &|_, _| {})?;
            let bytes = fs::read(temp.path().join("tree").join(&file.path))?;
            if fsx::sha256_bytes(&bytes) != file.sha256 { return Err(Error::Invalid("The signed launcher failed verification.".into())); }
            // This derived cache is optional; failure to save it does not change the server.
            let _ = fsx::atomic_write(&cache, &bytes);
            bytes
        }
    };
    Ok(std::str::from_utf8(&signed).ok().is_some_and(|signed| {
        fs::read(root.join(&file.path)).ok().is_some_and(|actual| crate::driver::launcher_matches(signed, &actual))
    }))
}

fn reject_downgrade(meta: &InstallMeta, manifest: &Manifest) -> Result<()> {
    if let Some(c) = &manifest.compatibility {
        let platform = if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" };
        if c.platform != platform {
            return Err(Error::Invalid(format!("This package targets {} rather than {platform}.", c.platform)));
        }
        if meta.core.version.as_ref().is_none_or(|version| !c.source_versions.contains(version)) {
            return Err(Error::Invalid("This server version is outside the release's tested compatibility matrix.".into()));
        }
    }
    if let Some(installed) = meta.core.version.as_deref() {
        if let (Some(have), Some(candidate)) = (crate::manifest::parse_version(installed), crate::manifest::parse_version(&manifest.version)) {
            if candidate < have {
                return Err(Error::Invalid(format!("Installed server {installed} is newer than package {}. Use its recovery point to undo an update; older packages cannot be applied.", manifest.version)));
            }
        }
    }
    Ok(())
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

pub fn validate_candidate(meta: &InstallMeta, manifest: &Manifest) -> Result<()> {
    check_manifest(manifest)?;
    reject_downgrade(meta, manifest)
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
    refuse_for_docker(p.root)?;
    let (root, meta_dir) = (p.root, p.meta_dir);
    let _lock = operation_lock(meta_dir)?;
    ensure_recovered(meta_dir)?;
    let (md, mut meta) = MetaDir::open(meta_dir)?;
    let _ = md;

    step(report, "Checking the update", 2);
    let (m, manifest_bytes) = fetch_manifest(&p.source, p.trusted_key)?;
    tracing::info!(root = %root.display(), from_version = ?meta.core.version, to_version = %m.version, migrations = m.migrations.len(), "server update selected");
    check_manifest(&m)?;
    reject_downgrade(&meta, &m)?;
    p.env.preflight(&m)?;
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

    let mut txn = Txn { id: id.clone(), state: State::Prepared, from_version: meta.core.version.clone(), to_version: m.version.clone(), recovery_point: None, databases_started: false, ops: Vec::new(), message: None, recovery_hashes: BTreeMap::new() };
    txn.recovery_hashes.insert("@install".into(), fsx::sha256_file(&before.join("manager-install.json"))?);
    save(meta_dir, &txn)?;

    // Nothing has touched the installation yet; failures up to here just discard the staging area.
    let staged = (|| -> Result<PathBuf> {
        step(report, "Downloading the update", 5);
        let parts = fetch_parts(&p.source, &m, &meta_dir.join("staging").join("download"), &p.cancel, &|f, _| step(report, "Downloading the update", 5 + (f * 35.0) as u8))?;
        step(report, "Verifying the update", 42);
        package::extract(&parts, &m, &tree, &|_, _| {})?;
        crate::migrations::verify_files(&m.migrations, &tree.join("_migrations"))?;
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
    let point_meta = crate::backup::point_json(meta_dir, txn.recovery_point.as_deref().unwrap())?;
    if point_meta.is_file() { txn.recovery_hashes.insert("@point".into(), fsx::sha256_file(&point_meta)?); }
    save(meta_dir, &txn)?;
    step(report, "Testing the update on a private copy of your server", 54);
    if let Err(error) = p.env.rehearse(&m, &tree, &items, txn.recovery_point.as_deref().unwrap()) {
        txn.state = State::Failed;
        txn.message = Some(format!("The private update rehearsal failed: {error}. The installed server was not updated."));
        save(meta_dir, &txn)?;
        return Err(Error::Invalid(txn.message.unwrap()));
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
    txn.state = State::Applied;
    save(meta_dir, &txn)?;

    // MySQL DDL can commit before a later statement fails. Restore the full recovery point on failure.
    let mut migrated = None;
    if !m.migrations.is_empty() || m.files.iter().any(|file| file.path == crate::schema_check::CONTRACT) {
        step(report, "Updating the database", 75);
        txn.databases_started = true;
        save(meta_dir, &txn)?;
        match p.env.migrate(&m, &tree.join("_migrations")) {
            Ok(r) if r.failed.is_none() => migrated = Some(r),
            Ok(r) => {
                let (mid, why) = r.failed.clone().unwrap();
                step(report, "Undoing the update", 76);
                return fail_after_apply(p, &mut txn, &before, &tree, format!("Database update {mid} failed: {why}"), Some(r));
            }
            Err(e) => {
                step(report, "Undoing the update", 76);
                return fail_after_apply(p, &mut txn, &before, &tree, e.to_string(), None);
            }
        }
    }

    step(report, "Starting the updated server", 85);
    // Core startup can apply SQL and write character data even when the package has no explicit migrations.
    txn.databases_started = true;
    save(meta_dir, &txn)?;
    if let Err(e) = p.env.validate() {
        if p.env.automatic_rollback() {
            return fail_after_apply(p, &mut txn, &before, &tree,
                format!("The updated server did not start correctly: {e}"), migrated);
        }
        txn.state = State::NeedsDecision;
        txn.message = Some(format!("The updated server did not start correctly: {e}"));
        save(meta_dir, &txn)?;
        return Ok(Outcome { txn, migrations: migrated });
    }

    finish(root, meta_dir, &mut meta, &m, &tree, &mut txn)?;
    p.env.activate()?;
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
    env.activate()?;
    Ok(txn)
}

fn fail_after_apply(p: &Params, txn: &mut Txn, before: &Path, tree: &Path, why: String, report: Option<ApplyReport>) -> Result<Outcome> {
    tracing::error!(transaction = %txn.id, databases_started = txn.databases_started, "server update failed; restoring recovery point");
    let restored = restore_transaction(p.root, p.meta_dir, before, txn, p.env).and_then(|()| {
        if p.env.automatic_rollback() { p.env.validate_recovery() } else { Ok(()) }
    });
    if restored.is_err() { let _ = p.env.ensure_stopped(); }
    tracing::info!(transaction = %txn.id, recovered = restored.is_ok(), "server update recovery finished");
    txn.state = if restored.is_ok() { State::RolledBack } else { State::Failed };
    txn.message = Some(match &restored { Ok(()) => why.clone(), Err(e) => format!("{why} Recovery failed: {e}") });
    save(p.meta_dir, txn)?;
    if restored.is_ok() {
        let _ = fs::remove_dir_all(tree);
        if p.env.automatic_rollback() { p.env.activate()?; }
    }
    let _ = report;
    Err(Error::Invalid(match restored {
        Ok(()) if txn.databases_started => format!("{why} The server files and databases were restored to the recovery point."),
        Ok(()) => format!("{why} The server files were restored."),
        Err(e) => format!("{why} Recovery failed: {e}. Resolve the unfinished update before starting the server."),
    }))
}

fn restore_transaction(root: &Path, meta: &Path, before: &Path, txn: &mut Txn, env: &dyn Env) -> Result<()> {
    for (rel, expected) in &txn.recovery_hashes {
        let saved = match rel.as_str() {
            "@install" => before.join("manager-install.json"),
            "@point" => crate::backup::point_json(meta, txn.recovery_point.as_deref().ok_or_else(|| Error::Invalid("The recovery point identity is missing.".into()))?)?,
            _ => fsx::safe_join(before, rel)?,
        };
        if fsx::sha256_file(&saved)? != *expected { return Err(Error::Invalid(format!("Recovery copy {rel} is damaged; no restoration was started."))); }
    }
    if let Some(id) = &txn.recovery_point { env.verify_snapshot(id)?; }
    for op in txn.ops.iter().filter(|op| op.started && op.had_previous) {
        if !fsx::safe_join(before, &op.path)?.is_file() { return Err(Error::Invalid(format!("The recovery copy of {} is missing; no restoration was started.", op.path))); }
    }
    if !before.join("manager-install.json").is_file() { return Err(Error::Invalid("The saved installation metadata is missing; no restoration was started.".into())); }
    env.ensure_stopped()?;
    env.recover_validation()?;
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
            txn.recovery_hashes.insert(path.clone(), fsx::sha256_file(&keep)?);
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
    refuse_for_docker(root)?;
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
    let mut recovery_hashes = BTreeMap::new();
    recovery_hashes.insert("@install".into(), fsx::sha256_file(&before.join("manager-install.json"))?);
    for op in ops.iter().filter(|op| op.had_previous) { recovery_hashes.insert(op.path.clone(), fsx::sha256_file(&fsx::safe_join(&before, &op.path)?)?); }
    let txn = Txn { id, state: State::Applying, from_version: Some(version.into()), to_version: version.into(), recovery_point: Some(backup.into()), databases_started: false, ops, message: Some("Repair is in progress.".into()), recovery_hashes };
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
    let mut dirs: Vec<_> = rd.flatten().filter(|e| read_journal(&e.path().join("txn.json")).is_ok_and(|t| matches!(t.state, State::Committed | State::RolledBack))).map(|e| e.path()).collect();
    dirs.sort();
    while dirs.len() > keep {
        let dir = dirs.remove(0);
        if let Ok(txn) = read_journal(&dir.join("txn.json")) {
            if let Ok(marker) = cleanup_marker(meta_dir, &txn.id) {
                if fsx::atomic_write_json(&marker, &CleanupMarker { id: txn.id, state: txn.state }).is_ok() && fs::remove_dir_all(&dir).is_ok() { let _ = fs::remove_file(marker); }
            }
        }
    }
}

/// The real environment: a repack-shaped installation driven through its launcher.
pub struct RepackEnv<'a> {
    pub root: &'a Path,
    pub meta_dir: &'a Path,
}

impl RepackEnv<'_> {
    pub fn pending_migrations(&self, manifest: &Manifest) -> Result<usize> {
        Ok(self.pending_migrations_with(manifest, DatabaseAccess::Start)?.unwrap_or(0))
    }

    /// `None` when the database is not running and `access` forbids starting it.
    pub fn pending_migrations_with(&self, manifest: &Manifest, access: DatabaseAccess) -> Result<Option<usize>> {
        if manifest.migrations.is_empty() { return Ok(Some(0)); }
        let count = |db: &crate::db::Db| -> Result<usize> {
            let realms = crate::realms::state(self.root)?;
            let mut modes = vec![realms.active];
            if realms.wildcard_created { modes.push(if realms.active == crate::realms::Mode::Coa { crate::realms::Mode::Wildcard } else { crate::realms::Mode::Coa }); }
            let mut count = 0;
            for (index, mode) in modes.into_iter().enumerate() {
                let list: Vec<_> = manifest.migrations.iter().filter(|m| index == 0 || m.db != "auth").cloned().collect();
                count += crate::migrations::pending_count(&db.clone().for_realm(mode), &list)?;
            }
            Ok(count)
        };
        match access {
            DatabaseAccess::Start => crate::backup::with_database(self.root, count).map(Some),
            DatabaseAccess::OnlyIfRunning => crate::backup::with_running_database(self.root, count),
        }
    }
    pub(crate) fn verify_recovery_point(&self, point: &crate::backup::RecoveryPoint) -> Result<()> {
        if point.kind != crate::backup::Kind::Full || !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) {
            return Err(Error::Invalid("The update recovery point does not contain every required database and configuration.".into()));
        }
        if !crate::backup::verify(self.meta_dir, &point.id)?.ok { return Err(Error::Invalid("The recovery point failed verification.".into())); }
        if let Some(snapshot) = &point.mysql_snapshot {
            if fsx::sha256_file(&self.root.join("mysql/bin/mysqld.exe"))? != snapshot.server_sha256 {
                return Err(Error::Invalid("The MySQL executable differs from the recovery point.".into()));
            }
            return Ok(());
        }
        crate::backup::with_database(self.root, |db| {
            let db = db.clone().for_realm(crate::realms::Mode::Coa);
            let mut objects = Vec::new();
            for component in point.components.iter().filter(|c| c.sha256.is_some()) {
                let schema = if component.name.contains('-') || component.name == "playerbots" { crate::db::schema_of(&component.name)? } else { point.realm.schema(&component.name)? };
                objects.extend(db.recovery_objects(schema)?);
            }
            if !objects.is_empty() {
                let path = self.meta_dir.join("diagnostics").join(format!("database-recovery-{}.json", uuid::Uuid::new_v4()));
                let report = serde_json::json!({ "schema": 1, "checkedAt": chrono::Utc::now().to_rfc3339(), "recoveryPoint": point.id, "objects": objects });
                if let Err(error) = fsx::atomic_write_json(&path, &report) {
                    tracing::warn!(%error, "Could not save database recovery compatibility report");
                }
                let sample = objects.iter().take(3).map(|object| format!("{}.{} ({})", object["database"].as_str().unwrap_or("?"), object["name"].as_str().unwrap_or("?"), object["kind"].as_str().unwrap_or("?"))).collect::<Vec<_>>().join(", ");
                return Err(Error::Invalid(format!("Automatic recovery does not support {} database objects: {sample}. Export diagnostics for the complete list. The update was not started.", objects.len())));
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
    fn recover_validation(&self) -> Result<()> {
        crate::update_isolation::restore(self.root, self.meta_dir)
    }

    fn activate(&self) -> Result<()> {
        ensure_recovered(self.meta_dir)?;
        let outcome = crate::driver::validate_update(self.root)?;
        if outcome.ok { Ok(()) } else {
            Err(Error::Invalid(format!("The update transaction finished, but the server could not start: {}", crate::driver::startup_failure(self.root, &outcome))))
        }
    }

    fn automatic_rollback(&self) -> bool { true }
    fn rehearse(&self, manifest: &Manifest, tree: &Path, items: &[PlanItem], id: &str) -> Result<()> {
        if items.iter().any(|item| item.action != Action::Skip && item.path.to_ascii_lowercase().starts_with("mysql/")) {
            return Err(Error::Invalid("A server update cannot replace MySQL while using a cold recovery copy; a separate database engine upgrade is required.".into()));
        }
        let point = crate::backup::get(self.meta_dir, id)?;
        let Some(snapshot) = &point.mysql_snapshot else {
            // Docker uses its separate experimental release channel until its clone runner is qualified.
            if crate::docker::is_docker(self.root) { return Ok(()); }
            return Err(Error::Invalid("This installation has no complete MySQL recovery copy for an update rehearsal.".into()));
        };
        crate::mysql_snapshot::validate_connections(self.root, self.root)?;
        crate::mysql_snapshot::stop(self.root)?;
        let point_dir = crate::backup::point_json(self.meta_dir, id)?.parent().unwrap().to_path_buf();
        let fixture = crate::mysql_snapshot::rehearsal_copy(self.root, self.meta_dir, &point_dir, snapshot)?;
        let metadata = crate::registry::metadata_dir_for(&fixture)?;
        let env = RepackEnv { root: &fixture, meta_dir: &metadata };
        let result = crate::install::with_scratch_ports(&fixture, || {
            let mut txn = Txn { id: "rehearsal".into(), state: State::Applying, from_version: None,
                to_version: manifest.version.clone(), recovery_point: None, databases_started: false,
                ops: items.iter().filter(|item| item.action != Action::Skip).map(|item| Op {
                    path: item.path.clone(), action: item.action, reason: item.reason.clone(),
                    new_sha256: manifest.files.iter().find(|file| file.path == item.path).unwrap().sha256.clone(),
                    had_previous: fsx::safe_join(&fixture, &item.path).is_ok_and(|path| path.is_file()), started:false, done:false,
                }).collect(), message:None, recovery_hashes:BTreeMap::new() };
            let before = txn_dir(&metadata, &txn.id)?.join("before");
            fs::create_dir_all(&before)?;
            apply_ops(&fixture, tree, &before, &manifest.files, &mut txn, &metadata, None)?;
            crate::mysql_snapshot::validate_connections(&fixture, self.root)?;
            crate::mysql_snapshot::isolate_connections(&fixture)?;
            let identities = crate::backup::with_database(&fixture, |db| {
                let identities = database_identities(db)?;
                let realms = crate::realms::state(&fixture)?;
                let mut modes = vec![realms.active];
                if realms.wildcard_created { modes.push(if realms.active == crate::realms::Mode::Coa { crate::realms::Mode::Wildcard } else { crate::realms::Mode::Coa }); }
                for (index, mode) in modes.into_iter().enumerate() {
                    let db = db.clone().for_realm(mode);
                    crate::schema_check::repair_missing_defaults(&db, &fixture, manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT).map(|file| file.sha256.as_str()))?;
                    let list: Vec<_> = manifest.migrations.iter().filter(|migration| index == 0 || migration.db != "auth").cloned().collect();
                    let report = crate::migrations::apply_pending(&db, &list, &tree.join("_migrations"), &|| Ok("private rehearsal".into()))?;
                    if let Some((migration, error)) = report.failed { return Err(Error::Invalid(format!("Rehearsal migration {migration} failed: {error}"))); }
                    crate::schema_check::repair_missing_defaults(&db, &fixture, manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT).map(|file| file.sha256.as_str()))?;
                }
                verify_database_identities(db, &identities)?;
                Ok(identities)
            })?;
            env.validate()?;
            crate::backup::with_database(&fixture, |db| verify_database_identities(db, &identities))?;
            env.ensure_stopped()
        });
        let stopped = crate::mysql_snapshot::stop(&fixture);
        let live_state = if result.is_ok() { crate::mysql_snapshot::stop(self.root).and_then(|()| crate::mysql_snapshot::unchanged(self.root, snapshot)) } else { Ok(()) };
        let logs: BTreeMap<_, _> = ["Core/Logs/auth-console.log", "Core/Logs/world-console.log", "Core/Logs/supervisor.log", "mysql/logs/mysql-error.log"].into_iter()
            .map(|path| (path, crate::diag::redact(&crate::health::tail(&fixture.join(path), 32768)))).collect();
        let mut schema_reports = Vec::<serde_json::Value>::new();
        if let Ok(entries) = fs::read_dir(metadata.join("diagnostics")) {
            for entry in entries.flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("schema-validation-")) {
                let path = fsx::ensure_within(&metadata, &entry.path())?;
                if fs::metadata(&path)?.len() <= 4 * 1024 * 1024 { schema_reports.push(fsx::read_json(&path)?); }
            }
        }
        let report = serde_json::json!({"schema":1,"checkedAt":chrono::Utc::now().to_rfc3339(),"toVersion":manifest.version,"recoveryPoint":id,
            "result":if result.is_ok() && stopped.is_ok() && live_state.is_ok() {"passed"} else {"failed"}, "error":result.as_ref().err().map(ToString::to_string), "stopError":stopped.as_ref().err().map(ToString::to_string), "liveStateError":live_state.as_ref().err().map(ToString::to_string), "logs":logs,"schemaReports":schema_reports,"fixture":fixture});
        let report_path = self.meta_dir.join("diagnostics").join(format!("database-rehearsal-{}.json", uuid::Uuid::new_v4()));
        fsx::atomic_write_json(&report_path, &report)?;
        stopped?;
        let parent = fixture.parent().unwrap();
        let owned = fsx::ensure_within(&self.meta_dir.join("rehearsals"), parent)?;
        fs::remove_dir_all(owned)?;
        live_state?;
        result
    }
    fn preflight(&self, manifest: &Manifest) -> Result<()> {
        crate::mysql_snapshot::stop_previous_rehearsals(self.meta_dir)?;
        if let Some(compatibility) = &manifest.compatibility {
            let (_, meta) = MetaDir::open(self.meta_dir)?;
            if let Some(source) = meta.core.version.as_ref().and_then(|v| compatibility.source_databases.get(v)) {
                if let Some(expected) = &source.schema_sha256 {
                    let contract = self.root.join("Scripts/database-schema.json");
                    if fsx::sha256_file(&contract)? != *expected {
                        return Err(Error::Invalid("The installed database schema contract differs from the qualified upgrade source; repair the installation first.".into()));
                    }
                }
                crate::backup::with_database(self.root, |db| {
                    let mut modes = vec![crate::realms::Mode::Coa];
                    if crate::realms::state(self.root)?.wildcard_created { modes.push(crate::realms::Mode::Wildcard); }
                    for mode in modes {
                        if let Some(problem) = crate::schema_check::check_with_report(&db.clone().for_realm(mode), self.root)?.first() {
                            if crate::docker::is_docker(self.root) {
                                return Err(Error::Invalid(format!("The installed {} database does not match its upgrade source: {}.{}: {}", mode.name(), problem.database, problem.table, problem.detail)));
                            }
                            tracing::warn!(realm = mode.name(), table = %problem.table, "Installed schema differs; the private candidate rehearsal must qualify this database");
                        }
                    }
                    Ok(())
                })?;
            }
        }
        self.pending_migrations(manifest).map(|_| ())
    }

    fn verify_snapshot(&self, id: &str) -> Result<()> {
        let point = crate::backup::get(self.meta_dir, id)?;
        if point.kind != crate::backup::Kind::Full || !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) {
            return Err(Error::Invalid("The recovery point is incomplete; no restoration was started.".into()));
        }
        if !crate::backup::verify(self.meta_dir, id)?.ok { return Err(Error::Invalid("The recovery point is damaged; available copies were preserved.".into())); }
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
        if point.kind != crate::backup::Kind::Full { return Err(Error::Invalid("Database rollback requires a full recovery point.".into())); }
        if !["characters", "auth", "world", "configs"].iter().all(|name| point.components.iter().any(|c| c.name == *name)) || !crate::backup::verify(self.meta_dir, id)?.ok {
            return Err(Error::Invalid("The full database recovery point is incomplete or damaged.".into()));
        }
        if let Some(snapshot) = &point.mysql_snapshot {
            crate::mysql_snapshot::stop(self.root)?;
            let point_dir = crate::backup::point_json(self.meta_dir, id)?.parent().unwrap().to_path_buf();
            crate::mysql_snapshot::swap(self.root, self.meta_dir, &point_dir, snapshot, false)?;
            crate::backup::restore_configs(self.root, self.meta_dir, id)?;
            return Ok(());
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
            if let Some(contract) = manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT) {
                self.migration_snapshot()?;
                crate::schema_check::repair_missing_defaults(db, root, Some(&contract.sha256))?;
            }
            let mut result = crate::migrations::apply_pending(db, &manifest.migrations, staged, &|| {
                self.migration_snapshot()
            })?;
            if realms.wildcard_created && result.failed.is_none() {
                let other = if realms.active == crate::realms::Mode::Coa { crate::realms::Mode::Wildcard } else { crate::realms::Mode::Coa };
                let other_db = db.clone().for_realm(other);
                crate::schema_check::repair_missing_defaults(&other_db, root, manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT).map(|file| file.sha256.as_str()))?;
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
                    crate::schema_check::repair_missing_defaults(&other_db, root, manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT).map(|file| file.sha256.as_str()))?;
                    if let Some(p) = crate::schema_check::check_with_report(&other_db, root)?.first() {
                        return Err(Error::Invalid(format!("Database validation failed on {}: {}.{}: {}", other.name(), p.table, p.column, p.detail)));
                    }
                }
            }
            if result.failed.is_none() {
                crate::schema_check::repair_missing_defaults(db, root, manifest.files.iter().find(|file| file.path == crate::schema_check::CONTRACT).map(|file| file.sha256.as_str()))?;
                let problems = crate::schema_check::check_with_report(db, root)?;
                if let Some(p) = problems.first() {
                    return Err(Error::Invalid(format!("Database validation failed: {}.{}.{}: {} ({} problems).", p.database, p.table, p.column, p.detail, problems.len())));
                }
            }
            Ok(result)
        })
    }

    fn validate(&self) -> Result<()> {
        self.validate_server(true)
    }

    fn validate_recovery(&self) -> Result<()> {
        self.validate_server(false)
    }
}

impl RepackEnv<'_> {
    fn validate_server(&self, check_schema: bool) -> Result<()> {
        let stopped = crate::driver::run(self.root, crate::driver::Verb::StopAll)?;
        if !stopped.ok { return Err(Error::Invalid("Could not stop the server before isolated validation.".into())); }
        self.ensure_stopped()?;
        self.recover_validation()?;
        let result = (|| -> Result<()> {
            crate::update_isolation::begin(self.root, self.meta_dir)?;
            use crate::process::{observe, ServiceState};
            if check_schema { crate::backup::with_database(self.root, |db| {
                let mut modes = vec![crate::realms::Mode::Coa];
                if crate::realms::state(self.root)?.wildcard_created { modes.push(crate::realms::Mode::Wildcard); }
                for mode in modes {
                    if let Some(p) = crate::schema_check::check_with_report(&db.clone().for_realm(mode), self.root)?.first() {
                        return Err(Error::Invalid(format!("Database validation failed on {}: {}.{}: {}", mode.name(), p.database, p.table, p.detail)));
                    }
                }
                Ok(())
            })?; }
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
        })();
        let stopped = crate::driver::run(self.root, crate::driver::Verb::StopAll)?;
        if !stopped.ok {
            return Err(Error::Invalid("Isolated validation could not stop the server; recovery configurations were preserved.".into()));
        }
        self.ensure_stopped()?;
        self.recover_validation()?;
        result
    }

}

fn database_identities(db: &crate::db::Db) -> Result<BTreeMap<String, std::collections::BTreeSet<String>>> {
    let db = db.clone().for_realm(crate::realms::Mode::Coa);
    let mut identities = BTreeMap::new();
    for (schema, table, columns) in [
        ("acore_characters", "characters", "guid,account,HEX(name)"),
        ("acore_characters_wildcard", "characters", "guid,account,HEX(name)"),
        ("acore_auth", "account", "id,HEX(username)"),
    ] {
        if !db.schema_exists(schema)? || !db.tables(schema)?.iter().any(|name| name == table) { continue; }
        let rows = db.query(&format!("SELECT {columns} FROM `{schema}`.`{table}`;"))?;
        identities.insert(schema.into(), rows.lines().map(str::to_string).collect());
    }
    Ok(identities)
}

fn verify_database_identities(db: &crate::db::Db, before: &BTreeMap<String, std::collections::BTreeSet<String>>) -> Result<()> {
    let after = database_identities(db)?;
    for (schema, expected) in before {
        if after.get(schema).is_none_or(|actual| !expected.is_subset(actual)) {
            return Err(Error::Invalid(format!("The private update changed or removed existing character or account identities in {schema}.")));
        }
    }
    Ok(())
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
        activation_expected: RefCell<Option<(PathBuf, String)>>,
        activation_failed: Cell<bool>,
        activated: Cell<bool>,
        automatic_rollback: Cell<bool>,
        recovery_healthy: Cell<bool>,
        stopped: Cell<bool>,
        healthy: Cell<bool>,
        snapshot_ok: Cell<bool>,
        calls: RefCell<Vec<&'static str>>,
        migrate_fail: Cell<bool>,
        preflight_fail: Cell<bool>,
        restore_fail: Cell<bool>,
        rehearsal_fail: Cell<bool>,
    }

    impl Fake {
        fn ok() -> Fake {
            Fake { activation_expected: RefCell::new(None), activation_failed: Cell::new(false), activated: Cell::new(false), automatic_rollback: Cell::new(false), recovery_healthy: Cell::new(true), stopped: Cell::new(true), healthy: Cell::new(true), snapshot_ok: Cell::new(true), calls: Default::default(), migrate_fail: Cell::new(false), preflight_fail: Cell::new(false), restore_fail: Cell::new(false), rehearsal_fail: Cell::new(false) }
        }
    }

    impl Env for Fake {
        fn rehearse(&self, _: &Manifest, _: &Path, _: &[PlanItem], _: &str) -> Result<()> {
            if self.rehearsal_fail.get() { Err(Error::Invalid("SQL failed in the private copy".into())) } else { Ok(()) }
        }
        fn activate(&self) -> Result<()> {
            if let Some((meta, version)) = self.activation_expected.borrow().as_ref() {
                ensure_recovered(meta)?;
                assert_eq!(MetaDir::open(meta)?.1.core.version.as_ref(), Some(version));
            }
            self.activated.set(true);
            if self.activation_failed.get() { Err(Error::Invalid("Public startup failed".into())) } else { Ok(()) }
        }
        fn automatic_rollback(&self) -> bool { self.automatic_rollback.get() }
        fn validate_recovery(&self) -> Result<()> {
            self.calls.borrow_mut().push("validate-recovery");
            if self.recovery_healthy.get() { Ok(()) } else { Err(Error::Invalid("Restored server is unhealthy".into())) }
        }
        fn preflight(&self, _: &Manifest) -> Result<()> {
            if self.preflight_fail.get() { Err(Error::Invalid("Conflicting migration history".into())) } else { Ok(()) }
        }
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
    fn a_docker_server_is_never_given_the_repack_files() {
        // Found by trying the installer: the update check of a Docker server read the Windows channel and offered its files.
        let w = world(&[], false);
        write(&w.root, "Settings/docker.json", br#"{"project":"t1"}"#);
        let before = read(&w, "Core/worldserver.exe");
        let meta = InstallMeta::new(InstallKind::New, &w.root);

        let err = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("Docker"), "{err}");
        let err = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap_err();
        assert!(err.to_string().contains("Docker"), "{err}");
        assert!(rollback(&w.root, &w.meta, "any", &Fake::ok()).is_err());
        assert_eq!(read(&w, "Core/worldserver.exe"), before, "nothing was touched");
        assert!(!w.root.join("Core/newfile.dll").exists());

        // The same package is applied to the same folder once it is not a Docker server.
        fs::remove_file(w.root.join("Settings/docker.json")).unwrap();
        assert_eq!(run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap().txn.state, State::Committed);
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
    fn public_startup_requires_a_completed_transaction_and_saved_version() {
        let w = world(&[], false);
        let env = Fake::ok();
        *env.activation_expected.borrow_mut() = Some((w.meta.clone(), "2.0.0".into()));
        run(&w, &env, BTreeMap::new(), None).unwrap();
        assert!(env.activated.get());
    }

    #[test]
    fn startup_failure_after_commit_does_not_restore_a_live_database() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.activation_failed.set(true);
        assert!(run(&w, &env, BTreeMap::new(), None).is_err());
        assert!(ensure_recovered(&w.meta).is_ok());
        assert_eq!(MetaDir::open(&w.meta).unwrap().1.core.version.as_deref(), Some("2.0.0"));
        assert!(!env.calls.borrow().contains(&"restore-databases"));
    }

    #[test]
    fn an_integrated_launcher_is_current_but_user_edits_and_new_package_bytes_are_not() {
        let source = b"import sys\nfrom pathlib import Path\nfrom squid_playerbots import configure\nROOT = Path(__file__).resolve().parents[1]\n";
        let w = world(&[("Scripts/manage.py", source)], false);
        run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap();
        let integrated = crate::driver::patch_launcher_imports(std::str::from_utf8(source).unwrap()).unwrap();
        write(&w.root, "Scripts/manage.py", integrated.as_bytes());
        let (_, mut meta) = MetaDir::open(&w.meta).unwrap();
        meta.original_hashes.insert("Scripts/manage.py".into(), fsx::sha256_bytes(integrated.as_bytes()));
        fsx::atomic_write_json(&w.meta.join("install.json"), &meta).unwrap();
        let p = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap();
        assert_eq!(p.items.iter().find(|item| item.path == "Scripts/manage.py").unwrap().action, Action::Skip);
        let manifest = fetch_manifest(&Source::Dir(w.pkg.clone()), &w.key).unwrap().0;
        let applied_plan = plan(&w.root, &meta, &manifest, &BTreeMap::new(), Some(&w._d.path().join("upd"))).unwrap();
        assert_eq!(applied_plan.iter().find(|item| item.path == "Scripts/manage.py").unwrap().action, Action::Skip);
        let mut changed = fetch_manifest(&Source::Dir(w.pkg.clone()), &w.key).unwrap().0;
        changed.files.iter_mut().find(|file| file.path == "Scripts/manage.py").unwrap().sha256 = fsx::sha256_bytes(b"new launcher");
        assert!(launcher_already_integrated(&w.root, &w.meta, &meta, &changed, &Source::Dir(w.pkg.clone())).is_err());
        write(&w.root, "Scripts/manage.py", format!("{integrated}\n# user edit\n").as_bytes());
        // Same version: the owner's edit is not an update (Repair lists and restores it).
        let p = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap();
        assert!(p.items.iter().all(|item| item.action == Action::Skip) && p.conflicts.is_empty());
        // An older installed build with the same edit does need a decision.
        meta.core.version = Some("1.0.0".into());
        let p = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap();
        assert_eq!(p.items.iter().find(|item| item.path == "Scripts/manage.py").unwrap().action, Action::Conflict);
        meta.original_hashes.insert("Scripts/manage.py".into(), fsx::sha256_file(&w.root.join("Scripts/manage.py")).unwrap());
        let p = preview(&w.root, &meta, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap();
        assert_eq!(p.items.iter().find(|item| item.path == "Scripts/manage.py").unwrap().action, Action::Replace,
            "Recorded user content must not be mistaken for a recognized Manager integration");
    }

    #[test]
    fn older_packages_are_rejected_before_preview_or_application() {
        let w = world(&[], false);
        let (_, mut installed) = MetaDir::open(&w.meta).unwrap();
        installed.core.version = Some("3.0.0".into());
        fsx::atomic_write_json(&w.meta.join("install.json"), &installed).unwrap();
        let env = Fake::ok();
        assert!(run(&w, &env, BTreeMap::new(), None).unwrap_err().to_string().contains("newer than package"));
        assert!(preview(&w.root, &installed, &Source::Dir(w.pkg.clone()), &w.key, &BTreeMap::new()).unwrap_err().to_string().contains("newer than package"));
        assert!(env.calls.borrow().is_empty());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(pending_checked(&w.meta).unwrap().is_none());
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
    fn automatic_health_failure_restores_databases_files_and_checks_recovery() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.automatic_rollback.set(true);
        env.healthy.set(false);
        assert!(run(&w, &env, BTreeMap::new(), None).unwrap_err().to_string().contains("restored"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(!w.root.join("Core/newfile.dll").exists());
        assert!(env.calls.borrow().contains(&"restore-databases"));
        assert!(env.calls.borrow().contains(&"validate-recovery"));
        assert!(unfinished(&w.meta).is_none());
    }

    #[test]
    fn incompatible_source_or_platform_is_rejected_without_touching_installation() {
        let w = world(&[], false);
        let (_, meta) = MetaDir::open(&w.meta).unwrap();
        let mut manifest = Manifest::parse(&fs::read(w.pkg.join("manifest.json")).unwrap()).unwrap();
        manifest.min_manager_version = "0.6.13".into();
        manifest.core.commit = Some("a".repeat(40));
        manifest.compatibility = Some(crate::manifest::Compatibility { schema: 1,
            platform: if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" }.into(),
            core_commit: "a".repeat(40), module_commits: [("squid".into(), "b".repeat(40))].into(),
            source_databases: Default::default(), client_patch_version: "1.5.1".into(), source_versions: vec!["1.0.0".into()] });
        manifest.validate().unwrap();
        validate_candidate(&meta, &manifest).unwrap();
        manifest.compatibility.as_mut().unwrap().source_versions = vec!["0.9.0".into()];
        assert!(validate_candidate(&meta, &manifest).unwrap_err().to_string().contains("compatibility matrix"));
        manifest.compatibility.as_mut().unwrap().source_versions = vec!["1.0.0".into()];
        manifest.compatibility.as_mut().unwrap().platform = if cfg!(windows) { "linux-x86_64" } else { "windows-x86_64" }.into();
        assert!(validate_candidate(&meta, &manifest).unwrap_err().to_string().contains("targets"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(unfinished(&w.meta).is_none());
    }

    #[test]
    fn changed_installed_schema_contract_blocks_before_database_access() {
        let w = world(&[], false);
        write(&w.root, "Scripts/database-schema.json", b"modified schema contract");
        let mut manifest = Manifest::parse(&fs::read(w.pkg.join("manifest.json")).unwrap()).unwrap();
        manifest.compatibility = Some(crate::manifest::Compatibility { schema: 1,
            platform: "windows-x86_64".into(), core_commit: "a".repeat(40),
            module_commits: [("squid".into(), "b".repeat(40))].into(), client_patch_version: "1.5.1".into(),
            source_versions: vec!["1.0.0".into()],
            source_databases: [("1.0.0".into(), crate::manifest::SourceDatabase {
                manifest_sha256: "a".repeat(64), schema_sha256: Some("b".repeat(64)) })].into() });
        let env = RepackEnv { root: &w.root, meta_dir: &w.meta };
        let error = env.preflight(&manifest).unwrap_err().to_string();
        assert!(error.contains("schema contract differs"), "{error}");
        assert!(!w.root.join("Runtime/python/python.exe").exists());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(unfinished(&w.meta).is_none());
    }

    #[test]
    fn unhealthy_restoration_preserves_recovery_and_blocks_startup() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.automatic_rollback.set(true);
        env.healthy.set(false);
        env.recovery_healthy.set(false);
        assert!(run(&w, &env, BTreeMap::new(), None).unwrap_err().to_string().contains("Recovery failed"));
        let pending = unfinished(&w.meta).unwrap();
        assert_eq!(pending.state, State::Failed);
        assert!(txn_dir(&w.meta, &pending.id).unwrap().join("before").exists());
        assert!(ensure_recovered(&w.meta).is_err());
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
            op.done &= op.started;
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
    fn sql_manifest_mismatch_is_rejected_before_snapshot_or_replacement() {
        let w = world(&[], true);
        let manifest_path = w.pkg.join("manifest.json");
        let mut manifest: Manifest = fsx::read_json(&manifest_path).unwrap();
        manifest.migrations[0].sha256 = "a".repeat(64);
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        // A publisher mistake: valid signed archive bytes disagree with the SQL metadata.
        let key = SigningKey::generate(&mut rand_core::OsRng);
        let trusted = base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes());
        fs::write(&manifest_path, &bytes).unwrap();
        fs::write(w.pkg.join("manifest.json.sig"), base64::engine::general_purpose::STANDARD.encode(key.sign(&bytes).to_bytes())).unwrap();
        let env = Fake::ok();
        let error = apply(&Params { root: &w.root, meta_dir: &w.meta, source: Source::Dir(w.pkg.clone()), trusted_key: &trusted,
            cancel: Default::default(), resolutions: Default::default(), env: &env, fail_after_ops: None }, &|_, _| {}).unwrap_err();
        assert!(matches!(error, Error::HashMismatch { .. }), "Expected SQL metadata mismatch, received {error}");
        assert!(env.calls.borrow().is_empty());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert!(pending_checked(&w.meta).unwrap().is_none());
    }

    #[test]
    fn preflight_rejection_precedes_snapshot_and_file_changes() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.preflight_fail.set(true);
        let original = fsx::sha256_file(&w.meta.join("install.json")).unwrap();
        assert!(run(&w, &env, BTreeMap::new(), None).unwrap_err().to_string().contains("Conflicting migration history"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert_eq!(fsx::sha256_file(&w.meta.join("install.json")).unwrap(), original);
        assert!(env.calls.borrow().is_empty());
        assert!(fs::read_dir(w.meta.join("updates")).unwrap().next().is_none());
    }

    #[test]
    fn failed_rehearsal_preserves_installed_files_version_and_recovery_point() {
        let w = world(&[], true);
        let env = Fake::ok();
        env.rehearsal_fail.set(true);
        let original = fsx::sha256_file(&w.meta.join("install.json")).unwrap();
        let error = run(&w, &env, BTreeMap::new(), None).unwrap_err();
        assert!(error.to_string().contains("installed server was not updated"));
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v1");
        assert_eq!(fsx::sha256_file(&w.meta.join("install.json")).unwrap(), original);
        assert_eq!(&*env.calls.borrow(), &["stop", "snapshot"]);
        assert!(pending_checked(&w.meta).unwrap().is_none());
        let journal = journals(&w.meta).unwrap().remove(0);
        assert_eq!(journal.recovery_point.as_deref(), Some("rp-1"));
        assert!(!journal.databases_started && journal.ops.is_empty());
    }

    #[test]
    fn missing_and_semantically_corrupt_journals_block_operations() {
        let w = world(&[], false);
        let txn = run(&w, &Fake::ok(), BTreeMap::new(), None).unwrap().txn;
        let path = txn_dir(&w.meta, &txn.id).unwrap().join("txn.json");
        let mut bad = txn.clone();
        bad.id = "another-update".into();
        fsx::atomic_write_json(&path, &bad).unwrap();
        assert!(pending_checked(&w.meta).is_err());
        bad = txn.clone();
        bad.ops[0].path = "../outside".into();
        fsx::atomic_write_json(&path, &bad).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
        bad = txn.clone();
        bad.ops[0].started = false;
        fsx::atomic_write_json(&path, &bad).unwrap();
        assert!(ensure_recovered(&w.meta).is_err());
        fs::remove_file(&path).unwrap();
        assert!(pending_checked(&w.meta).is_err());
        let marker = cleanup_marker(&w.meta, &txn.id).unwrap();
        fsx::atomic_write_json(&marker, &CleanupMarker { id: txn.id, state: State::Committed }).unwrap();
        assert!(pending_checked(&w.meta).unwrap().is_none());
    }

    #[test]
    fn damaged_saved_file_blocks_restore_before_any_environment_action() {
        let w = world(&[], false);
        let env = Fake::ok();
        env.healthy.set(false);
        let txn = run(&w, &env, BTreeMap::new(), None).unwrap().txn;
        let before = txn_dir(&w.meta, &txn.id).unwrap().join("before");
        write(&before, "Core/worldserver.exe", b"damaged original");
        env.calls.borrow_mut().clear();
        assert!(rollback(&w.root, &w.meta, &txn.id, &env).unwrap_err().to_string().contains("damaged"));
        assert!(env.calls.borrow().is_empty());
        assert_eq!(read(&w, "Core/worldserver.exe"), b"world-v2");
        assert_eq!(pending_checked(&w.meta).unwrap().unwrap().state, State::NeedsDecision);
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
