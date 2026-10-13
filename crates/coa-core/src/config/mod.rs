//! Safe configuration editing: schema-driven, line-preserving, validated, snapshotted and atomically written.

pub mod merge;
pub mod parser;
pub mod schema;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, FieldError, Result};
use crate::fsx;
use parser::ConfFile;
use schema::{values_equal, Category, Preset, PresetFile, Restart, Schema, Setting};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Bots,
    Server,
}

impl Scope {
    pub fn schema(self) -> &'static Schema {
        static BOTS: OnceLock<Schema> = OnceLock::new();
        static SERVER: OnceLock<Schema> = OnceLock::new();
        match self {
            Scope::Bots => BOTS.get_or_init(|| Schema::parse(include_str!("../../../../schemas/bots.json")).expect("bots schema")),
            Scope::Server => SERVER.get_or_init(|| Schema::parse(include_str!("../../../../schemas/server.json")).expect("server schema")),
        }
    }

    pub fn presets(self) -> &'static [Preset] {
        static BOTS: OnceLock<PresetFile> = OnceLock::new();
        static SERVER: OnceLock<PresetFile> = OnceLock::new();
        let f = match self {
            Scope::Bots => BOTS.get_or_init(|| serde_json::from_str(include_str!("../../../../schemas/presets/bots.json")).expect("bots presets")),
            Scope::Server => SERVER.get_or_init(|| serde_json::from_str(include_str!("../../../../schemas/presets/server.json")).expect("server presets")),
        };
        &f.presets
    }

    fn name(self) -> &'static str {
        match self {
            Scope::Bots => "bots",
            Scope::Server => "server",
        }
    }
}

/// Files that carry a scope's settings. `read` is what the next server start will use.
#[derive(Debug, Clone)]
pub struct Targets {
    pub read: PathBuf,
    /// Every file a change must be written to (always includes `read`).
    pub writes: Vec<PathBuf>,
    /// The generated file the launcher rewrites from a template at each start, when applicable.
    pub generated: Option<PathBuf>,
}

pub fn targets(root: &Path, scope: Scope) -> Result<Targets> {
    match scope {
        Scope::Bots => {
            let conf = root.join("Core/configs/modules/mod_coa_playerbots.conf");
            if !conf.is_file() {
                // The server ships the documented defaults as `.dist` and uses them until an active file exists: make it.
                let dist = root.join("Core/configs/modules/mod_coa_playerbots.conf.dist");
                if !dist.is_file() {
                    return Err(Error::Invalid("CoA Companions (bots) are not installed on this server.".into()));
                }
                fsx::atomic_write(&conf, &fs::read(&dist)?)?;
            }
            Ok(Targets { read: conf.clone(), writes: vec![conf], generated: None })
        }
        Scope::Server => {
            let conf = root.join("Core/configs/worldserver.conf");
            let template = root.join("Settings/worldserver.conf.template");
            match (template.is_file(), conf.is_file()) {
                (true, true) => Ok(Targets { read: template.clone(), writes: vec![template, conf.clone()], generated: Some(conf) }),
                (true, false) => Ok(Targets { read: template.clone(), writes: vec![template], generated: None }),
                (false, true) => Ok(Targets { read: conf.clone(), writes: vec![conf], generated: None }),
                (false, false) => Err(Error::Invalid("worldserver.conf was not found.".into())),
            }
        }
    }
}

fn load_conf(path: &Path) -> Result<ConfFile> {
    ConfFile::parse_bytes(&fs::read(path)?)
}

fn has_placeholder(raw: &str) -> bool {
    let t = raw.trim().trim_matches('"');
    t.starts_with('@') && t.ends_with('@') && t.len() > 2
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingView {
    #[serde(flatten)]
    pub meta: Setting,
    pub value: Value,
    pub is_default: bool,
    /// The key exists in the configuration file (otherwise `value` is the built-in default).
    pub present: bool,
    /// The file holds a value that is not valid for this setting.
    pub problem: Option<String>,
    /// Manual edit of the generated file that the launcher will overwrite at next start.
    pub drift: bool,
}

#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub scope: Scope,
    pub categories: Vec<Category>,
    pub settings: Vec<SettingView>,
    /// Keys in the file the schema does not know (kept untouched).
    pub unknown_keys: usize,
    pub drift_keys: Vec<String>,
    pub files: Vec<String>,
}

/// Keys whose value in the generated conf differs from the template (i.e. hand edits the launcher will reset).
/// Modules read `Core/configs/modules/<name>.conf`, but packages ship only the documented `<name>.conf.dist` (the active
/// files are the server owner's). Create every missing active file from its `.dist`, so a fresh server runs with the
/// documented defaults (for example `CoA.Enable = 1`, without which the Ascension client is dropped after login).
/// An existing file only receives keys its `.dist` gained since (added with their documentation, values never changed),
/// so a server updated to a build with a new setting gets that setting's documented default. Files the launcher writes
/// itself are skipped. Returns what was created (`name.conf`) or extended (`name.conf (+n keys)`).
pub fn materialize_module_configs(root: &Path) -> Result<Vec<String>> {
    materialize(root, true)
}

/// Only the missing active files, created from their `.dist`; a file that exists is not touched at all. This is what a
/// server the Manager imported (not installed) gets: without its file a module logs a "missing property" line for every
/// setting it reads, and some modules read them on every world tick.
pub fn create_missing_module_configs(root: &Path) -> Result<Vec<String>> {
    materialize(root, false)
}

fn materialize(root: &Path, extend_existing: bool) -> Result<Vec<String>> {
    let dir = root.join("Core").join("configs").join("modules");
    let mut created = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(created) };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".conf.dist") else { continue };
        if stem == "coa_bugreport" || stem == "mod_ascension_compat" {
            continue; // written by the launcher at every start
        }
        let active = dir.join(format!("{stem}.conf"));
        if !active.exists() {
            fs::copy(e.path(), &active)?;
            if stem == "coa" {
                // This product serves the CoA client only, and the servers listen on this computer unless the owner
                // shares them; without this key the server drops CoA clients that connect from another address
                // (friends over a private network) right after login.
                if let Ok(bytes) = fs::read(&active) {
                    if let Ok(mut conf) = parser::ConfFile::parse_bytes(&bytes) {
                        conf.set("CoA.AllowRemoteClients", "1", &["Set by CoA Server Manager: this server is built for CoA clients"]);
                        fsx::atomic_write(&active, conf.to_text().as_bytes())?;
                    }
                }
            }
            created.push(format!("{stem}.conf"));
        } else if !extend_existing {
            continue;
        } else if let (Ok(have), Ok(dist)) = (fs::read(&active), fs::read(e.path())) {
            if let (Ok(mut conf), Ok(dist)) = (parser::ConfFile::parse_bytes(&have), parser::ConfFile::parse_bytes(&dist)) {
                let plan = merge::apply(&mut conf, &dist);
                if !plan.added.is_empty() {
                    fsx::atomic_write(&active, conf.to_text().as_bytes())?;
                    created.push(format!("{stem}.conf (+{} keys)", plan.added.len()));
                }
            }
        }
    }
    created.sort();
    Ok(created)
}

pub fn drift(root: &Path) -> Result<Vec<String>> {
    let t = targets(root, Scope::Server)?;
    let Some(generated) = &t.generated else { return Ok(Vec::new()) };
    let (template, conf) = (load_conf(&t.read)?, load_conf(generated)?);
    let mut keys = BTreeSet::new();
    // Compare what the server actually uses: the last occurrence of each key (a file may repeat a key).
    let names: BTreeSet<&str> = conf.entries().map(|(k, _)| k).collect();
    for k in names {
        let v = conf.get(k).unwrap_or_default();
        match template.get(k) {
            Some(tv) if has_placeholder(tv) => {}
            Some(tv) if tv.trim() != v.trim() => {
                keys.insert(k.to_string());
            }
            None => {
                keys.insert(k.to_string());
            }
            _ => {}
        }
    }
    Ok(keys.into_iter().collect())
}

pub fn load(root: &Path, scope: Scope) -> Result<SettingsView> {
    // A Docker server that was added but never started has no active world configuration yet; create it from its template.
    if scope == Scope::Server && crate::docker::is_docker(root) {
        let _ = crate::docker::ensure_main_configs(root);
    }
    let schema = scope.schema();
    let t = targets(root, scope)?;
    let conf = load_conf(&t.read)?;
    let drift_keys = if scope == Scope::Server { drift(root).unwrap_or_default() } else { Vec::new() };
    let known: BTreeSet<&str> = schema.settings.iter().map(|s| s.key.as_str()).collect();
    let unknown_keys = conf.entries().filter(|(k, _)| !known.contains(k)).count();

    // On a Docker server some settings are fixed by the backend (environment beats the file): not offered.
    let docker = scope == Scope::Server && crate::docker::is_docker(root);
    let settings = schema
        .settings
        .iter()
        .filter(|s| !(docker && crate::docker::managed_setting(&s.key)))
        .map(|s| {
            let raw = conf.get(&s.key);
            let (value, problem) = match raw {
                Some(r) => match s.from_raw(r) {
                    Ok(v) => (v, None),
                    Err(e) => (s.default.clone(), Some(e)),
                },
                None => (s.default.clone(), None),
            };
            SettingView {
                is_default: values_equal(&value, &s.default),
                present: raw.is_some(),
                problem,
                drift: drift_keys.iter().any(|k| *k == s.key),
                value,
                meta: s.clone(),
            }
        })
        .collect();

    Ok(SettingsView {
        scope,
        categories: schema.categories.clone(),
        settings,
        unknown_keys,
        drift_keys,
        files: t.writes.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    })
}

/// Pairs that must satisfy `min <= max` after a save.
const ORDERED_PAIRS: [(&str, &str); 6] = [
    ("CoaBots.Profile.IntentMinMs", "CoaBots.Profile.IntentMaxMs"),
    ("CoaBots.WorldBrain.PlannerMinMs", "CoaBots.WorldBrain.PlannerMaxMs"),
    ("CoaBots.WorldBrain.ScanMinMs", "CoaBots.WorldBrain.ScanMaxMs"),
    ("CoaBots.WorldBrain.MountDistanceMin", "CoaBots.WorldBrain.MountDistanceMax"),
    ("CoaBots.WorldBrain.PartyMinMinutes", "CoaBots.WorldBrain.PartyMaxMinutes"),
    ("CoaBots.WorldBrain.QuestSuspendMs", "CoaBots.WorldBrain.QuestSuspendMaxMs"),
];

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub key: String,
    pub title: String,
    pub from: Option<Value>,
    pub to: Value,
    pub restart: Restart,
    pub dangerous: bool,
}

#[derive(Debug, Serialize)]
pub struct SaveReport {
    pub changed: Vec<Change>,
    /// Strongest restart any changed setting needs, if any changed.
    pub restart: Option<Restart>,
    pub snapshot: Option<String>,
}

fn effective(view: &SettingsView, changes: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    changes
        .get(key)
        .and_then(Value::as_f64)
        .or_else(|| view.settings.iter().find(|s| s.meta.key == key).and_then(|s| s.value.as_f64()))
}

/// Validate `changes` against the schema without writing anything.
pub fn validate(root: &Path, scope: Scope, changes: &BTreeMap<String, Value>) -> Result<BTreeMap<String, String>> {
    let schema = scope.schema();
    let mut errors = Vec::new();
    let mut raws = BTreeMap::new();
    let docker = scope == Scope::Server && crate::docker::is_docker(root);
    for (key, value) in changes {
        match schema.get(key) {
            None => errors.push(FieldError { key: key.clone(), message: "is not a setting the Manager can change".into() }),
            Some(_) if docker && crate::docker::managed_setting(key) => {
                errors.push(FieldError { key: key.clone(), message: "is set by the Docker backend; changing it in the file would have no effect".into() })
            }
            Some(s) => match s.to_raw(value) {
                Ok(raw) => {
                    if let Some(problem) = crate::docker::setting_problem(key, &raw).filter(|_| crate::docker::is_docker(root)) {
                        errors.push(FieldError { key: key.clone(), message: problem.into() });
                        continue;
                    }
                    raws.insert(key.clone(), raw);
                }
                Err(m) => errors.push(FieldError { key: key.clone(), message: m }),
            },
        }
    }
    if errors.is_empty() {
        let view = load(root, scope)?;
        for (lo, hi) in ORDERED_PAIRS {
            if !(changes.contains_key(lo) || changes.contains_key(hi)) {
                continue;
            }
            if let (Some(a), Some(b)) = (effective(&view, changes, lo), effective(&view, changes, hi)) {
                if a > b {
                    let key = if changes.contains_key(lo) { lo } else { hi };
                    errors.push(FieldError { key: key.into(), message: format!("the minimum ({lo}) cannot be larger than the maximum ({hi})") });
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(raws)
    } else {
        Err(Error::Validation(errors))
    }
}

/// Where snapshots of a scope's files are kept.
fn snapshot_root(meta_dir: &Path) -> PathBuf {
    meta_dir.join("backups").join("config")
}

const KEEP_SNAPSHOTS: usize = 40;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct SnapshotFile {
    original: String,
    stored: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub scope: Scope,
    pub reason: String,
    pub created_at: String,
    files: Vec<SnapshotFile>,
}

pub(crate) fn take_snapshot(meta_dir: &Path, scope: Scope, reason: &str, files: &[(PathBuf, Vec<u8>)]) -> Result<String> {
    let root = snapshot_root(meta_dir);
    fs::create_dir_all(&root)?;
    // The id is a timestamp to the millisecond. Two snapshots of the same scope can land in the same one (a restore
    // snapshots the current state just before reading the snapshot it restores), and reusing the folder would overwrite
    // the files about to be restored. Take a fresh folder, with a numeric suffix when the name is already used.
    let base = format!("{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S%3f"), scope.name());
    let (id, dir) = (1u32..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find_map(|id| {
            let dir = root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => Some(Ok((id, dir))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .expect("the suffix range is unbounded")?;
    let mut entries = Vec::new();
    for (i, (path, bytes)) in files.iter().enumerate() {
        let stored = format!("{i}_{}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        fsx::atomic_write(&dir.join(&stored), bytes)?;
        entries.push(SnapshotFile { original: path.to_string_lossy().into_owned(), stored });
    }
    let info = SnapshotInfo { id: id.clone(), scope, reason: reason.into(), created_at: chrono::Utc::now().to_rfc3339(), files: entries };
    fsx::atomic_write_json(&dir.join("snapshot.json"), &info)?;
    prune_snapshots(meta_dir);
    Ok(id)
}

fn prune_snapshots(meta_dir: &Path) {
    let Ok(rd) = fs::read_dir(snapshot_root(meta_dir)) else { return };
    let mut dirs: Vec<_> = rd.filter_map(|e| e.ok()).filter(|e| e.path().join("snapshot.json").is_file()).map(|e| e.path()).collect();
    dirs.sort();
    while dirs.len() > KEEP_SNAPSHOTS {
        let _ = fs::remove_dir_all(dirs.remove(0));
    }
}

pub fn list_snapshots(meta_dir: &Path) -> Vec<SnapshotInfo> {
    let mut out: Vec<SnapshotInfo> = fs::read_dir(snapshot_root(meta_dir))
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| fsx::read_json(&e.path().join("snapshot.json")).ok()).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// How many threads the world should use to update maps on this computer: half the logical processors, between 2 and 8.
/// Maps (continents, dungeons, battlegrounds) are updated in parallel, one thread per map, so more threads than the
/// number of maps in use bring nothing.
pub fn recommended_map_threads() -> u32 {
    let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    (cores / 2).clamp(2, 8)
}

const PERFORMANCE_MARKER: &str = "performance-defaults-v1.done";

/// Once per server: switch the world to updating maps on several threads (`MapUpdate.Threads`), which the stock
/// configuration leaves at 1. A value the owner already raised is respected, and because this runs a single time
/// (a marker file in the Manager's folder for the server) a later choice to go back to 1 is never overridden.
/// Returns the new thread count when something was written.
pub fn ensure_performance_defaults(root: &Path, meta_dir: &Path) -> Result<Option<u32>> {
    let marker = meta_dir.join(PERFORMANCE_MARKER);
    if marker.exists() {
        return Ok(None);
    }
    let view = load(root, Scope::Server)?;
    let current = view.settings.iter().find(|s| s.meta.key == "MapUpdate.Threads");
    let already = current.map(|s| s.present && s.value.as_i64().unwrap_or(1) > 1).unwrap_or(false);
    let threads = recommended_map_threads();
    let written = if already {
        None
    } else {
        let changes = BTreeMap::from([("MapUpdate.Threads".to_string(), Value::from(threads))]);
        save(root, meta_dir, Scope::Server, &changes)?;
        Some(threads)
    };
    let quieted = quiet_debug_loggers(root)?;
    fsx::atomic_write(&marker, b"MapUpdate.Threads and logging levels set by the Manager\n")?;
    tracing::info!(?written, ?quieted, "performance defaults applied");
    Ok(written)
}

/// Loggers some configurations leave at debug level: `network` writes a line for every packet sent and
/// `entities.player` one for every item check. With companions that is thousands of lines a second, each a disk write.
/// They are put back to info level (the next level up); anything else in the file is untouched.
fn quiet_debug_loggers(root: &Path) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for path in targets(root, Scope::Server)?.writes {
        let mut conf = ConfFile::parse_bytes(&fs::read(&path)?)?;
        let mut touched = false;
        for key in ["Logger.network", "Logger.entities.player"] {
            let Some(value) = conf.get(key).map(|v| v.trim().to_string()) else { continue };
            if let Some(rest) = value.strip_prefix("5,") {
                conf.set(key, &format!("4,{rest}"), &["Lowered from debug by CoA Server Manager: it wrote a line for every packet and item check"]);
                touched = true;
                if !changed.contains(&key.to_string()) {
                    changed.push(key.to_string());
                }
            }
        }
        if touched {
            fsx::atomic_write(&path, conf.to_text().as_bytes())?;
        }
    }
    Ok(changed)
}

/// Apply validated `changes`. Nothing is written unless every change is valid; previous contents are snapshotted first.
pub fn save(root: &Path, meta_dir: &Path, scope: Scope, changes: &BTreeMap<String, Value>) -> Result<SaveReport> {
    if scope == Scope::Bots { crate::realms::guard_module(root, "companions")?; }
    let raws = validate(root, scope, changes)?;
    let schema = scope.schema();
    let t = targets(root, scope)?;
    let current = load(root, scope)?;

    let mut originals: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut confs: Vec<ConfFile> = Vec::new();
    for p in &t.writes {
        let bytes = fs::read(p)?;
        confs.push(ConfFile::parse_bytes(&bytes)?);
        originals.push((p.clone(), bytes));
    }

    let mut changed = Vec::new();
    for (key, raw) in &raws {
        let setting = schema.get(key).expect("validated");
        let view = current.settings.iter().find(|s| s.meta.key == *key).expect("in schema");
        let same_everywhere = confs.iter().all(|c| c.get(key).map(str::trim) == Some(raw.as_str()));
        if same_everywhere {
            continue;
        }
        for c in &mut confs {
            c.set(key, raw, &["Added by CoA Server Manager"]);
        }
        changed.push(Change {
            key: key.clone(),
            title: setting.title.clone(),
            from: view.present.then(|| view.value.clone()),
            to: changes[key].clone(),
            restart: setting.restart_required,
            dangerous: setting.dangerous,
        });
    }
    if changed.is_empty() {
        return Ok(SaveReport { changed, restart: None, snapshot: None });
    }

    // Verify what we are about to write before touching the disk.
    let texts: Vec<String> = confs.iter().map(ConfFile::to_text).collect();
    for text in &texts {
        let reread = ConfFile::parse(text);
        for c in &changed {
            if reread.get(&c.key).map(str::trim) != Some(raws[&c.key].as_str()) {
                return Err(Error::Invalid(format!("internal check failed for {}", c.key)));
            }
        }
    }

    let reason = format!("before changing {} setting(s)", changed.len());
    let snapshot = take_snapshot(meta_dir, scope, &reason, &originals)?;

    let mut written = 0;
    for (i, (path, _)) in originals.iter().enumerate() {
        if let Err(e) = fsx::atomic_write(path, texts[i].as_bytes()) {
            for (p, bytes) in originals.iter().take(written) {
                let _ = fsx::atomic_write(p, bytes);
            }
            return Err(e);
        }
        written += 1;
    }
    let restart = changed.iter().map(|c| c.restart).max();
    tracing::info!(scope = scope.name(), changed = changed.len(), %snapshot, "configuration saved");
    Ok(SaveReport { changed, restart, snapshot: Some(snapshot) })
}

/// Put a snapshot's files back (after snapshotting the current state, so a restore is itself undoable).
pub fn restore_snapshot(meta_dir: &Path, id: &str) -> Result<()> {
    if id.contains(['/', '\\']) || id.contains("..") {
        return Err(Error::PathRejected(id.into()));
    }
    let dir = snapshot_root(meta_dir).join(id);
    let info: SnapshotInfo = fsx::read_json(&dir.join("snapshot.json"))?;
    let mut current = Vec::new();
    for f in &info.files {
        let p = PathBuf::from(&f.original);
        if let Ok(bytes) = fs::read(&p) {
            current.push((p, bytes));
        }
    }
    take_snapshot(meta_dir, info.scope, "before restoring a snapshot", &current)?;
    for f in &info.files {
        fsx::atomic_write(Path::new(&f.original), &fs::read(dir.join(&f.stored))?)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetChange {
    pub key: String,
    pub title: String,
    pub from: Value,
    pub to: Value,
    pub dangerous: bool,
}

#[derive(Debug, Serialize)]
pub struct PresetPreview {
    pub id: String,
    pub title: String,
    pub description: String,
    pub changes: Vec<PresetChange>,
}

fn diff(view: &SettingsView, wanted: impl Iterator<Item = (String, Value)>) -> Vec<PresetChange> {
    wanted
        .filter_map(|(key, to)| {
            let s = view.settings.iter().find(|s| s.meta.key == key)?;
            (!values_equal(&s.value, &to) || !s.present).then(|| PresetChange { title: s.meta.title.clone(), from: s.value.clone(), dangerous: s.meta.dangerous, key, to })
        })
        .filter(|c| !values_equal(&c.from, &c.to))
        .collect()
}

/// What applying a preset would change. Nothing is written; the UI shows "This preset will change N settings".
pub fn preview_preset(root: &Path, scope: Scope, preset_id: &str) -> Result<PresetPreview> {
    let preset = scope.presets().iter().find(|p| p.id == preset_id).ok_or_else(|| Error::Invalid(format!("unknown preset {preset_id}")))?;
    let view = load(root, scope)?;
    let changes = diff(&view, preset.values.iter().map(|(k, v)| (k.clone(), v.clone())));
    Ok(PresetPreview { id: preset.id.clone(), title: preset.title.clone(), description: preset.description.clone(), changes })
}

/// "Restore recommended defaults": every non-default setting back to its schema default.
pub fn preview_defaults(root: &Path, scope: Scope) -> Result<PresetPreview> {
    let view = load(root, scope)?;
    let changes = diff(&view, view.settings.iter().map(|s| (s.meta.key.clone(), s.meta.default.clone())));
    Ok(PresetPreview { id: "defaults".into(), title: "Recommended defaults".into(), description: "Every setting returns to its default value.".into(), changes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("srv");
        let meta = dir.path().join("srv.manager");
        crate::layout::testkit::fake_repack(&root);
        fs::create_dir_all(root.join("Settings")).unwrap();
        let world = "# Server\r\n[worldserver]\r\nRealmID = 1\r\nWorldServerPort = @WORLD_PORT@\r\n\r\n# XP\r\nRate.XP.Kill      = 1\r\nRate.Drop.Money = 1\r\nUnknown.Custom = \"keep me\"\r\nPlayerLimit = 0\r\n";
        fs::write(root.join("Settings/worldserver.conf.template"), world).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), world.replace("@WORLD_PORT@", "8085")).unwrap();
        fs::create_dir_all(&meta).unwrap();
        (dir, root, meta)
    }

    fn set(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn the_world_gets_several_map_threads_once_and_an_owner_choice_is_never_overridden() {
        let (_d, root, meta) = fixture();
        let wanted = recommended_map_threads();
        assert!((2..=8).contains(&wanted));
        for p in ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf"] {
            let mut t = fs::read_to_string(root.join(p)).unwrap();
            t.push_str("Logger.network=5,Server\r\nLogger.entities.player=5,Server\r\nLogger.module=4,Console Server\r\n");
            fs::write(root.join(p), t).unwrap();
        }
        assert_eq!(ensure_performance_defaults(&root, &meta).unwrap(), Some(wanted));
        for p in ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf"] {
            let t = fs::read_to_string(root.join(p)).unwrap();
            assert!(t.contains(&format!("MapUpdate.Threads = {wanted}")), "{p}: {t}");
            assert!(t.contains("Unknown.Custom = \"keep me\""), "other settings untouched");
            assert!(t.contains("Logger.network=4,Server") && t.contains("Logger.entities.player=4,Server"), "{p}: {t}");
            assert!(t.contains("Logger.module=4,Console Server"), "info level loggers are left alone");
        }
        // the owner goes back to one thread: it stays that way
        save(&root, &meta, Scope::Server, &set(&[("MapUpdate.Threads", json!(1))])).unwrap();
        assert_eq!(ensure_performance_defaults(&root, &meta).unwrap(), None);
        assert!(fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap().contains("MapUpdate.Threads = 1"));
    }

    #[test]
    fn an_already_raised_value_is_respected_on_the_first_run() {
        let (_d, root, meta) = fixture();
        save(&root, &meta, Scope::Server, &set(&[("MapUpdate.Threads", json!(6))])).unwrap();
        assert_eq!(ensure_performance_defaults(&root, &meta).unwrap(), None);
        assert!(fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap().contains("MapUpdate.Threads = 6"));
    }

    #[test]
    fn missing_module_configs_are_created_from_their_dist_and_existing_ones_are_kept() {
        let (_d, root, _m) = fixture();
        let m = root.join("Core/configs/modules");
        fs::create_dir_all(&m).unwrap();
        fs::write(m.join("coa.conf.dist"), "CoA.Enable = 1
").unwrap();
        fs::write(m.join("spellbook.conf.dist"), "Spellbook.Enable = 1
Spellbook.New = 5
").unwrap();
        fs::write(m.join("spellbook.conf"), "Spellbook.Enable = 0
").unwrap();
        fs::write(m.join("coa_bugreport.conf.dist"), "x = 1
").unwrap();
        assert_eq!(materialize_module_configs(&root).unwrap(), vec!["coa.conf".to_string(), "spellbook.conf (+1 keys)".to_string()]);
        let coa = fs::read_to_string(m.join("coa.conf")).unwrap();
        assert!(coa.contains("CoA.Enable = 1") && coa.contains("CoA.AllowRemoteClients = 1"), "{coa}");
        let sb = fs::read_to_string(m.join("spellbook.conf")).unwrap();
        assert!(sb.contains("Spellbook.Enable = 0"), "existing values are never changed: {sb}");
        assert!(sb.contains("Spellbook.New = 5"), "new documented key was added: {sb}");
        assert!(!m.join("coa_bugreport.conf").exists());
        assert!(materialize_module_configs(&root).unwrap().is_empty());
    }

    #[test]
    fn an_imported_server_only_gets_the_files_it_lacks() {
        let (_d, root, _m) = fixture();
        let m = root.join("Core/configs/modules");
        fs::create_dir_all(&m).unwrap();
        fs::write(m.join("dynamicxp.conf.dist"), "Dynamic.XP.Reminder.Interval = 40
").unwrap();
        fs::write(m.join("spellbook.conf.dist"), "Spellbook.Enable = 1
Spellbook.New = 5
").unwrap();
        fs::write(m.join("spellbook.conf"), "Spellbook.Enable = 0
").unwrap();
        assert_eq!(create_missing_module_configs(&root).unwrap(), vec!["dynamicxp.conf".to_string()]);
        assert!(fs::read_to_string(m.join("dynamicxp.conf")).unwrap().contains("Dynamic.XP.Reminder.Interval = 40"));
        assert_eq!(fs::read_to_string(m.join("spellbook.conf")).unwrap(), "Spellbook.Enable = 0
", "an existing file stays byte for byte");
    }

    #[test]
    fn schemas_and_presets_are_self_consistent() {
        for scope in [Scope::Bots, Scope::Server] {
            let schema = scope.schema();
            assert!(!schema.settings.is_empty());
            for p in scope.presets() {
                for (k, v) in &p.values {
                    let s = schema.get(k).unwrap_or_else(|| panic!("preset {} references unknown {k}", p.id));
                    s.to_raw(v).unwrap_or_else(|e| panic!("preset {} value for {k}: {e}", p.id));
                }
            }
        }
    }

    #[test]
    fn load_reads_values_defaults_and_counts_unknowns() {
        let (_d, root, _m) = fixture();
        let v = load(&root, Scope::Server).unwrap();
        let xp = v.settings.iter().find(|s| s.meta.key == "Rate.XP.Kill").unwrap();
        assert_eq!(xp.value, json!(1.0));
        assert!(xp.present && xp.is_default);
        let quest = v.settings.iter().find(|s| s.meta.key == "Rate.XP.Quest").unwrap();
        assert!(!quest.present, "missing key falls back to default");
        assert!(v.unknown_keys >= 3, "RealmID, WorldServerPort, Unknown.Custom are not curated");
        assert!(v.drift_keys.is_empty());
    }

    #[test]
    fn save_changes_only_that_key_in_template_and_generated_conf() {
        let (_d, root, meta) = fixture();
        let tpl_before = fs::read_to_string(root.join("Settings/worldserver.conf.template")).unwrap();
        let r = save(&root, &meta, Scope::Server, &set(&[("Rate.XP.Kill", json!(2.5))])).unwrap();
        assert_eq!(r.changed.len(), 1);
        assert_eq!(r.restart, Some(Restart::World));
        for f in ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf"] {
            let t = fs::read_to_string(root.join(f)).unwrap();
            assert!(t.contains("Rate.XP.Kill      = 2.5\r\n"), "{f}: {t:?}");
            assert!(t.contains("Unknown.Custom = \"keep me\"\r\n"));
        }
        let tpl_after = fs::read_to_string(root.join("Settings/worldserver.conf.template")).unwrap();
        assert_eq!(tpl_after.replace("= 2.5", "= 1"), tpl_before, "nothing but the one value changed");
        assert!(tpl_after.contains("@WORLD_PORT@"), "template placeholders survive");
    }

    #[test]
    fn invalid_input_writes_nothing() {
        let (_d, root, meta) = fixture();
        let before = fs::read(root.join("Core/configs/worldserver.conf")).unwrap();
        let e = save(&root, &meta, Scope::Server, &set(&[("Rate.XP.Kill", json!(3)), ("Rate.Drop.Money", json!(-4)), ("Nope.Key", json!(1))])).unwrap_err();
        match e {
            Error::Validation(f) => {
                assert_eq!(f.len(), 2);
                assert!(f.iter().any(|x| x.key == "Rate.Drop.Money"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(root.join("Core/configs/worldserver.conf")).unwrap(), before);
        assert!(list_snapshots(&meta).is_empty(), "no snapshot for a rejected save");
    }

    #[test]
    fn a_docker_server_is_not_offered_the_settings_its_backend_fixes() {
        let (_d, root, meta) = fixture();
        let keys = |root: &Path| load(root, Scope::Server).unwrap().settings.into_iter().map(|s| s.meta.key).collect::<Vec<_>>();
        for k in ["Ra.Enable", "Ra.IP", "Updates.EnableDatabases", "PlayerLimit"] {
            assert!(keys(&root).iter().any(|x| x == k), "a repack offers {k}");
        }
        fs::write(root.join("Settings/docker.json"), br#"{"project":"t1"}"#).unwrap();
        let docker = keys(&root);
        for k in ["Ra.Enable", "Ra.IP", "Updates.EnableDatabases"] {
            assert!(!docker.iter().any(|x| x == k), "a Docker server does not offer {k}: the containers' environment beats the file");
        }
        assert!(docker.iter().any(|x| x == "PlayerLimit"), "everything else is still offered");
        // Forcing a change through is refused and says why, instead of being saved and ignored.
        let err = save(&root, &meta, Scope::Server, &set(&[("Ra.Enable", json!(0))])).unwrap_err().to_string();
        assert!(err.contains("Docker backend"), "{err}");
        assert!(!fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap().contains("Ra.Enable = 0"));
        save(&root, &meta, Scope::Server, &set(&[("PlayerLimit", json!(77))])).unwrap();
    }

    #[test]
    fn missing_key_is_appended_and_noop_saves_do_nothing() {
        let (_d, root, meta) = fixture();
        let r = save(&root, &meta, Scope::Server, &set(&[("Rate.XP.Quest", json!(2))])).unwrap();
        assert_eq!(r.changed.len(), 1);
        assert!(r.changed[0].from.is_none());
        let t = fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap();
        assert!(t.ends_with("# Added by CoA Server Manager\r\nRate.XP.Quest = 2\r\n"), "{t:?}");
        let again = save(&root, &meta, Scope::Server, &set(&[("Rate.XP.Quest", json!(2))])).unwrap();
        assert!(again.changed.is_empty() && again.snapshot.is_none());
    }

    #[test]
    fn snapshot_restore_roundtrip_is_byte_exact_and_undoable() {
        let (_d, root, meta) = fixture();
        let conf = root.join("Core/configs/worldserver.conf");
        let original = fs::read(&conf).unwrap();
        save(&root, &meta, Scope::Server, &set(&[("PlayerLimit", json!(50))])).unwrap();
        assert_ne!(fs::read(&conf).unwrap(), original);
        let snap = list_snapshots(&meta).remove(0);
        restore_snapshot(&meta, &snap.id).unwrap();
        assert_eq!(fs::read(&conf).unwrap(), original);
        assert!(list_snapshots(&meta).len() >= 2, "restore took its own snapshot");
        assert!(restore_snapshot(&meta, "../evil").is_err());
    }

    #[test]
    fn snapshots_taken_in_the_same_millisecond_keep_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut ids = std::collections::HashSet::new();
        for i in 0..40 {
            let path = dir.path().join("worldserver.conf");
            let id = take_snapshot(dir.path(), Scope::Server, "test", &[(path, format!("version {i}").into_bytes())]).unwrap();
            assert!(ids.insert(id.clone()), "snapshot id {id} was handed out twice");
        }
        for info in list_snapshots(dir.path()) {
            assert_eq!(info.files.len(), 1);
        }
        assert_eq!(list_snapshots(dir.path()).len(), 40.min(KEEP_SNAPSHOTS));
    }

    #[test]
    fn a_repeated_key_with_the_same_effective_value_is_not_drift() {
        let (_d, root, _m) = fixture();
        for f in ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf"] {
            let p = root.join(f);
            let mut t = fs::read_to_string(&p).unwrap();
            t.push_str("Logger.x=6,Console Errors
Logger.x=6,Console Server
");
            fs::write(&p, t).unwrap();
        }
        assert!(drift(&root).unwrap().is_empty());
    }

    #[test]
    fn drift_detects_manual_edits_to_the_generated_conf_only() {
        let (_d, root, _m) = fixture();
        let conf = root.join("Core/configs/worldserver.conf");
        let t = fs::read_to_string(&conf).unwrap().replace("PlayerLimit = 0", "PlayerLimit = 99");
        fs::write(&conf, t).unwrap();
        assert_eq!(drift(&root).unwrap(), ["PlayerLimit"]);
        let v = load(&root, Scope::Server).unwrap();
        assert!(v.settings.iter().find(|s| s.meta.key == "PlayerLimit").unwrap().drift);
    }

    #[test]
    fn bots_ordering_rule_and_preset_preview() {
        let (_d, root, meta) = fixture();
        fs::write(root.join("Core/configs/modules/mod_coa_playerbots.conf"), "[worldserver]\nCoaBots.WorldBrain.PlannerMinMs = 2000\nCoaBots.WorldBrain.PlannerMaxMs = 8000\nCoaBots.Combat.VerboseLog = 1\n").unwrap();
        let e = save(&root, &meta, Scope::Bots, &set(&[("CoaBots.WorldBrain.PlannerMinMs", json!(9000))])).unwrap_err();
        assert!(matches!(e, Error::Validation(_)));
        let p = preview_preset(&root, Scope::Bots, "balanced").unwrap();
        assert!(p.changes.iter().any(|c| c.key == "CoaBots.Combat.VerboseLog" && c.to == json!(false)));
        let d = preview_defaults(&root, Scope::Bots).unwrap();
        assert!(d.changes.iter().all(|c| !values_equal(&c.from, &c.to)));
        // applying a preview through save() works end to end
        let map: BTreeMap<_, _> = p.changes.iter().map(|c| (c.key.clone(), c.to.clone())).collect();
        save(&root, &meta, Scope::Bots, &map).unwrap();
        assert!(preview_preset(&root, Scope::Bots, "balanced").unwrap().changes.is_empty(), "idempotent");
    }

    #[test]
    fn bots_scope_requires_the_module_config_or_its_default_file() {
        let (_d, root, _m) = fixture();
        let modules = root.join("Core/configs/modules");
        let conf = modules.join("mod_coa_playerbots.conf");
        let text = fs::read(&conf).unwrap();
        fs::remove_file(&conf).unwrap();
        assert!(load(&root, Scope::Bots).is_err(), "neither file: not installed");
        fs::write(modules.join("mod_coa_playerbots.conf.dist"), &text).unwrap();
        assert!(load(&root, Scope::Bots).is_ok(), "only the default file: it is made");
        assert!(conf.is_file());
    }
}
