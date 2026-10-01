//! Download and update of the game client from the community launcher's public manifest.
//!
//! The manifest lists every file with its size and SHA-256; each file is fetched by hash and verified before it is
//! moved into place (`download::fetch_with`, resumable). The Manager keeps a small state file inside the client
//! folder (`.coa-manager/client-state.json`) so that later checks are cheap and so that files the player changed
//! are recognised: they are never overwritten without being asked, and a replaced file is moved aside, not deleted.
//! WTF, Cache, other addons, logs and `realmlist.wtf` are not part of what the Manager synchronises.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::download::{self, Cancel, HttpTransport, Job, Transport};
use crate::error::{Error, Result};
use crate::fsx;

pub const MANIFEST_URL: &str = "https://launcher-api.coa-development.org/downloads/client/latest.json";
pub const OBJECTS_URL: &str = "https://launcher-api.coa-development.org/downloads/client/objects";
const STATE_DIR: &str = ".coa-manager";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub version: String,
    pub published_at: Option<String>,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Deserialize)]
struct RawManifest {
    schema: u32,
    version: String,
    #[serde(rename = "publishedAt")]
    published_at: Option<String>,
    files: Vec<ManifestFile>,
}

/// Things the Manager never writes, even if a manifest lists them: logs, the realmlist, the player's own settings,
/// caches and the companion addon (those have their own safe paths).
fn is_excluded(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.ends_with(".log")
        || p.rsplit('/').next() == Some("realmlist.wtf")
        || p.starts_with("wtf/")
        || p.starts_with("cache/")
        || p.starts_with("errors/")
        || p.starts_with("logs/")
        || p.starts_with(".coa-manager/")
        || p.starts_with("interface/addons/coabotui/")
}

/// Files players customise: graphics wrappers and their configuration. If one exists and differs from the published
/// file it is left exactly as it is, whatever the "keep my files" choice; it is only created when missing.
fn is_player_owned(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(name.as_str(), "d3d8.dll" | "d3d9.dll" | "d3d10core.dll" | "d3d11.dll" | "dxgi.dll" | "dxvk.conf")
        || matches!(name.rsplit('.').next(), Some("ini" | "conf" | "cfg" | "wtf"))
}

pub fn parse_manifest(text: &str) -> Result<Manifest> {
    let raw: RawManifest = serde_json::from_str(text).map_err(|e| Error::Invalid(format!("The client list could not be read: {e}")))?;
    if raw.schema != 1 {
        return Err(Error::Invalid(format!("The client list uses a newer format ({}); update the Manager.", raw.schema)));
    }
    if raw.version.trim().is_empty() || raw.version.len() > 64 {
        return Err(Error::Invalid("The client list has no version.".into()));
    }
    let root = Path::new("client-root");
    let mut files = Vec::with_capacity(raw.files.len());
    for f in raw.files {
        fsx::safe_join(root, &f.path)?;
        let hash_ok = f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit());
        if !hash_ok || f.size > MAX_FILE_BYTES {
            return Err(Error::Invalid(format!("The client list has a bad entry for {}.", f.path)));
        }
        if is_excluded(&f.path) {
            continue;
        }
        files.push(ManifestFile { path: f.path.replace('\\', "/"), size: f.size, sha256: f.sha256.to_ascii_lowercase() });
    }
    if files.is_empty() {
        return Err(Error::Invalid("The client list is empty.".into()));
    }
    Ok(Manifest { version: raw.version, published_at: raw.published_at, files })
}

pub fn fetch_manifest(transport: &dyn Transport, url: &str) -> Result<Manifest> {
    let mut reply = transport.get(url, 0).map_err(|e| Error::Invalid(format!("Could not reach the client download server: {e}")))?;
    if reply.status != 200 {
        return Err(Error::Invalid(format!("The client download server answered {}.", reply.status)));
    }
    let mut text = String::new();
    reply.body.by_ref().take(MAX_MANIFEST_BYTES).read_to_string(&mut text).map_err(|e| Error::Invalid(format!("Could not read the client list: {e}")))?;
    parse_manifest(&text)
}

pub fn fetch_latest() -> Result<Manifest> {
    download::check_url(MANIFEST_URL)?;
    fetch_manifest(&HttpTransport::with_total_timeout(30)?, MANIFEST_URL)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    pub mtime: u64,
    pub sha256: String,
    /// The player chose to keep this file although the manifest has a different one (this is the manifest hash
    /// they were asked about; a newer manifest asks again).
    #[serde(default)]
    pub kept_against: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// The manifest version this folder was brought to; None while a download is unfinished.
    pub version: Option<String>,
    pub files: BTreeMap<String, Entry>,
}

fn state_path(client: &Path) -> PathBuf {
    client.join(STATE_DIR).join("client-state.json")
}

pub fn load_state(client: &Path) -> Option<State> {
    fsx::read_json(&state_path(client)).ok()
}

fn save_state(client: &Path, state: &State) -> Result<()> {
    fsx::atomic_write_json(&state_path(client), state)
}

#[derive(Debug, Clone, Serialize)]
pub struct Local {
    /// The Manager downloaded or adopted this folder (a state file exists).
    pub managed: bool,
    pub version: Option<String>,
}

pub fn local(client: &Path) -> Local {
    let s = load_state(client);
    Local { managed: s.is_some(), version: s.and_then(|s| s.version) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Not in the folder.
    Missing,
    /// Still as the Manager left it; the manifest has a newer file.
    Changed,
    /// Different from the manifest and not as the Manager left it (changed by the player or by something else).
    Modified,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub path: String,
    pub size: u64,
    pub kind: Kind,
    #[serde(skip)]
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub version: String,
    pub items: Vec<Item>,
    pub download_bytes: u64,
    pub total_files: usize,
    pub up_to_date_files: usize,
    pub kept_files: usize,
    #[serde(skip)]
    confirmed: BTreeMap<String, Entry>,
}

impl Plan {
    pub fn modified(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.kind == Kind::Modified).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// "scan" while comparing files, "download" while fetching, "finish" at the end.
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub file: Option<String>,
}

fn cancelled() -> Error {
    Error::Invalid("Download cancelled.".into())
}

fn mtime_secs(m: &fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn hash_file(path: &Path, cancel: &Cancel, on_bytes: &mut dyn FnMut(u64)) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.is_set() {
            return Err(cancelled());
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
        on_bytes(n as u64);
    }
}

/// Compare the folder with the manifest. Files are hashed only when their size or modification time no longer
/// matches what the state file recorded, so a routine check of a managed folder is quick.
pub fn plan(client: &Path, m: &Manifest, state: &State, cancel: &Cancel, on_step: &dyn Fn(Step)) -> Result<Plan> {
    let total = m.total_bytes();
    let mut done = 0u64;
    let mut items = Vec::new();
    let mut confirmed = BTreeMap::new();
    let (mut up_to_date, mut kept) = (0usize, 0usize);
    for f in &m.files {
        if cancel.is_set() {
            return Err(cancelled());
        }
        on_step(Step { phase: "scan", done, total, bytes_per_sec: 0, file: Some(f.path.clone()) });
        let path = fsx::safe_join(client, &f.path)?;
        let meta = match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            _ => {
                items.push(Item { path: f.path.clone(), size: f.size, kind: Kind::Missing, sha256: f.sha256.clone() });
                done += f.size;
                continue;
            }
        };
        let (size, mtime) = (meta.len(), mtime_secs(&meta));
        let entry = state.files.get(&f.path);
        let trusted = entry.filter(|e| e.size == size && e.mtime == mtime);
        let local: Option<String> = if let Some(e) = trusted {
            Some(e.sha256.clone())
        } else if size == f.size || entry.map(|e| e.size == size).unwrap_or(false) {
            let base = done;
            let mut seen = 0u64;
            let h = hash_file(&path, cancel, &mut |n| {
                seen += n;
                on_step(Step { phase: "scan", done: base + seen.min(f.size), total, bytes_per_sec: 0, file: Some(f.path.clone()) });
            })?;
            Some(h)
        } else {
            None
        };
        done += f.size;
        if is_player_owned(&f.path) && local.as_deref() != Some(f.sha256.as_str()) {
            let sha = match local {
                Some(h) => h,
                None => hash_file(&path, cancel, &mut |_| {})?,
            };
            kept += 1;
            confirmed.insert(f.path.clone(), Entry { size, mtime, sha256: sha, kept_against: Some(f.sha256.clone()) });
            continue;
        }
        if local.as_deref() == Some(f.sha256.as_str()) {
            up_to_date += 1;
            confirmed.insert(f.path.clone(), Entry { size, mtime, sha256: f.sha256.clone(), kept_against: None });
            continue;
        }
        let as_left = entry.filter(|e| local.as_deref() == Some(e.sha256.as_str()));
        match as_left {
            Some(e) if e.kept_against.as_deref() == Some(f.sha256.as_str()) => {
                kept += 1;
                confirmed.insert(f.path.clone(), Entry { size, mtime, sha256: e.sha256.clone(), kept_against: e.kept_against.clone() });
            }
            Some(e) if e.kept_against.is_none() => {
                items.push(Item { path: f.path.clone(), size: f.size, kind: Kind::Changed, sha256: f.sha256.clone() });
            }
            _ => items.push(Item { path: f.path.clone(), size: f.size, kind: Kind::Modified, sha256: f.sha256.clone() }),
        }
    }
    on_step(Step { phase: "scan", done: total, total, bytes_per_sec: 0, file: None });
    let download_bytes = items.iter().map(|i| i.size).sum();
    Ok(Plan { version: m.version.clone(), items, download_bytes, total_files: m.files.len(), up_to_date_files: up_to_date, kept_files: kept, confirmed })
}

pub struct Apply<'a> {
    pub client: &'a Path,
    pub manifest: &'a Manifest,
    pub plan: &'a Plan,
    /// Leave files the player changed alone (they are remembered, and asked about again only when the manifest changes).
    pub keep_modified: bool,
    pub transport: &'a dyn Transport,
    pub objects_url: &'a str,
    pub cancel: &'a Cancel,
}

fn blocked(path: &str, e: std::io::Error) -> Error {
    Error::Invalid(format!("Could not replace {path}: {e}. Close the game and anything else using the client, then try again."))
}

/// Download what the plan lists. Progress is saved after every file, so an interrupted run resumes where it stopped.
pub fn apply(a: &Apply, on_step: &dyn Fn(Step)) -> Result<()> {
    let mut state = State { version: None, files: a.plan.confirmed.clone() };
    save_state(a.client, &state)?;

    let mut todo = Vec::new();
    for item in &a.plan.items {
        if item.kind == Kind::Modified && a.keep_modified {
            let path = fsx::safe_join(a.client, &item.path)?;
            let meta = fs::metadata(&path)?;
            let sha = hash_file(&path, a.cancel, &mut |_| {})?;
            state.files.insert(item.path.clone(), Entry { size: meta.len(), mtime: mtime_secs(&meta), sha256: sha, kept_against: Some(item.sha256.clone()) });
        } else {
            todo.push(item);
        }
    }
    let total: u64 = todo.iter().map(|i| i.size).sum();
    fsx::require_space(a.client, total)?;

    let staging = a.client.join(STATE_DIR).join("staging");
    let replaced = a.client.join(STATE_DIR).join("replaced").join(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let mut done = 0u64;
    let mut placed: HashMap<&str, PathBuf> = HashMap::new();
    for item in todo {
        if a.cancel.is_set() {
            save_state(a.client, &state)?;
            return Err(cancelled());
        }
        let target = fsx::safe_join(a.client, &item.path)?;
        let started = Instant::now();
        let staged = staging.join(&item.sha256);
        let copy_from = placed.get(item.sha256.as_str()).cloned();
        if copy_from.is_none() {
            let job = Job { url: format!("{}/{}", a.objects_url, item.sha256), dest: staged.clone(), sha256: item.sha256.clone(), size: item.size };
            let base = done;
            let result = download::fetch_with(a.transport, &job, a.cancel, &|p| {
                on_step(Step { phase: "download", done: base + p.downloaded, total, bytes_per_sec: p.bytes_per_sec, file: Some(item.path.clone()) });
            });
            if let Err(e) = result {
                let _ = save_state(a.client, &state);
                return Err(e);
            }
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if target.exists() {
            if item.kind == Kind::Modified {
                let keep = fsx::safe_join(&replaced, &item.path)?;
                if let Some(parent) = keep.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&target, &keep).map_err(|e| blocked(&item.path, e))?;
            } else {
                fs::remove_file(&target).map_err(|e| blocked(&item.path, e))?;
            }
        }
        match copy_from {
            Some(src) => {
                fs::copy(&src, &target).map_err(|e| blocked(&item.path, e))?;
            }
            None => fs::rename(&staged, &target).map_err(|e| blocked(&item.path, e))?,
        }
        placed.insert(&item.sha256, target.clone());
        let meta = fs::metadata(&target)?;
        state.files.insert(item.path.clone(), Entry { size: meta.len(), mtime: mtime_secs(&meta), sha256: item.sha256.clone(), kept_against: None });
        save_state(a.client, &state)?;
        done += item.size;
        let secs = started.elapsed().as_secs_f64().max(0.001);
        on_step(Step { phase: "download", done, total, bytes_per_sec: (item.size as f64 / secs) as u64, file: Some(item.path.clone()) });
    }
    state.version = Some(a.manifest.version.clone());
    save_state(a.client, &state)?;
    let _ = fs::remove_dir_all(&staging);
    on_step(Step { phase: "finish", done: total, total, bytes_per_sec: 0, file: None });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Reply;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Serves objects by hash; remembers what was requested and can fail after N objects.
    struct Server {
        objects: Mutex<HashMap<String, Vec<u8>>>,
        requested: Mutex<Vec<String>>,
        cancel_after: Option<(AtomicUsize, Cancel)>,
    }

    impl Server {
        fn new(files: &[(&str, &[u8])]) -> Self {
            let s = Server { objects: Default::default(), requested: Default::default(), cancel_after: None };
            for (_, body) in files {
                s.objects.lock().unwrap().insert(fsx::sha256_bytes(body), body.to_vec());
            }
            s
        }
    }

    impl Transport for Server {
        fn get(&self, url: &str, from: u64) -> std::result::Result<Reply, String> {
            let sha = url.rsplit('/').next().unwrap().to_string();
            self.requested.lock().unwrap().push(sha.clone());
            if let Some((left, cancel)) = &self.cancel_after {
                if left.fetch_sub(1, Ordering::SeqCst) == 1 {
                    cancel.cancel();
                }
            }
            let body = self.objects.lock().unwrap().get(&sha).cloned().ok_or("404")?;
            let (status, start) = if from > 0 { (206, from as usize) } else { (200, 0) };
            let cr = (from > 0).then(|| format!("bytes {from}-{}/{}", body.len() - 1, body.len()));
            Ok(Reply { status, content_range: cr, body: Box::new(Cursor::new(body[start..].to_vec())) })
        }
    }

    fn manifest(version: &str, files: &[(&str, &[u8])]) -> Manifest {
        Manifest {
            version: version.into(),
            published_at: None,
            files: files.iter().map(|(p, b)| ManifestFile { path: p.to_string(), size: b.len() as u64, sha256: fsx::sha256_bytes(b) }).collect(),
        }
    }

    const V1: [(&str, &[u8]); 4] = [("Wow.exe", b"exe-1"), ("Data/common.MPQ", b"common-mpq-data"), ("Data/patch-A.MPQ", b"patch-a-1"), ("Data/enUS/locale.MPQ", b"locale")];

    fn sync(dir: &Path, m: &Manifest, server: &Server, keep: bool) -> Result<Plan> {
        let state = load_state(dir).unwrap_or_default();
        let cancel = Cancel::default();
        let p = plan(dir, m, &state, &cancel, &|_| {})?;
        apply(&Apply { client: dir, manifest: m, plan: &p, keep_modified: keep, transport: server, objects_url: "https://x.invalid/objects", cancel: &cancel }, &|_| {})?;
        Ok(p)
    }

    #[test]
    fn manifest_rejects_traversal_and_bad_entries_and_skips_personal_files() {
        let ok = r#"{"schema":1,"version":"v1","files":[
            {"path":"Wow.exe","size":3,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"path":"Data/enUS/realmlist.wtf","size":3,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"path":"WTF/Config.wtf","size":3,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"path":"Interface/AddOns/CoABotUI/x.lua","size":3,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"path":"MemoryBridge.log","size":3,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["Wow.exe"]);
        for bad in [
            r#"{"schema":1,"version":"v","files":[{"path":"../evil.dll","size":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#,
            r#"{"schema":1,"version":"v","files":[{"path":"C:/evil.dll","size":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#,
            r#"{"schema":1,"version":"v","files":[{"path":"a.dll","size":1,"sha256":"short"}]}"#,
            r#"{"schema":2,"version":"v","files":[]}"#,
            r#"{"schema":1,"version":"v","files":[]}"#,
        ] {
            assert!(parse_manifest(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fresh_download_installs_every_file_verified_and_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let (m, s) = (manifest("v1", &V1), Server::new(&V1));
        let p = sync(d.path(), &m, &s, false).unwrap();
        assert_eq!(p.items.len(), 4);
        for (path, body) in V1 {
            assert_eq!(fs::read(d.path().join(path)).unwrap(), body);
        }
        let st = load_state(d.path()).unwrap();
        assert_eq!(st.version.as_deref(), Some("v1"));
        assert_eq!(st.files.len(), 4);
        assert!(!d.path().join(".coa-manager/staging").exists());
        assert!(local(d.path()).managed);
    }

    #[test]
    fn an_update_downloads_only_what_changed_and_a_second_check_is_empty() {
        let d = tempfile::tempdir().unwrap();
        sync(d.path(), &manifest("v1", &V1), &Server::new(&V1), false).unwrap();
        let mut v2 = V1.to_vec();
        v2[2] = ("Data/patch-A.MPQ", b"patch-a-2-longer");
        v2.push(("Data/new.MPQ", b"brand new"));
        let (m2, s2) = (manifest("v2", &v2), Server::new(&v2));
        let p = sync(d.path(), &m2, &s2, false).unwrap();
        let kinds: Vec<_> = p.items.iter().map(|i| (i.path.as_str(), i.kind)).collect();
        assert_eq!(kinds, [("Data/patch-A.MPQ", Kind::Changed), ("Data/new.MPQ", Kind::Missing)]);
        assert_eq!(s2.requested.lock().unwrap().len(), 2, "unchanged files were not downloaded again");
        assert_eq!(fs::read(d.path().join("Data/patch-A.MPQ")).unwrap(), b"patch-a-2-longer");
        assert!(!d.path().join(".coa-manager/replaced").exists(), "a file the Manager itself wrote needs no backup");
        let again = sync(d.path(), &m2, &s2, false).unwrap();
        assert!(again.items.is_empty() && again.up_to_date_files == 5);
        assert_eq!(load_state(d.path()).unwrap().version.as_deref(), Some("v2"));
    }

    #[test]
    fn a_file_the_player_changed_is_asked_about_kept_when_told_to_and_saved_when_replaced() {
        let d = tempfile::tempdir().unwrap();
        sync(d.path(), &manifest("v1", &V1), &Server::new(&V1), false).unwrap();
        fs::write(d.path().join("Wow.exe"), b"my patched exe").unwrap();
        let mut v2 = V1.to_vec();
        v2[0] = ("Wow.exe", b"exe-2");
        let (m2, s2) = (manifest("v2", &v2), Server::new(&v2));

        let p = sync(d.path(), &m2, &s2, true).unwrap();
        assert_eq!(p.modified().len(), 1);
        assert_eq!(fs::read(d.path().join("Wow.exe")).unwrap(), b"my patched exe", "kept");
        assert!(s2.requested.lock().unwrap().is_empty(), "nothing needed downloading");
        let quiet = sync(d.path(), &m2, &s2, true).unwrap();
        assert!(quiet.items.is_empty() && quiet.kept_files == 1, "the decision is remembered");

        let mut v3 = V1.to_vec();
        v3[0] = ("Wow.exe", b"exe-3");
        let (m3, s3) = (manifest("v3", &v3), Server::new(&v3));
        let again = sync(d.path(), &m3, &s3, false).unwrap();
        assert_eq!(again.modified().len(), 1, "a newer client asks again");
        assert_eq!(fs::read(d.path().join("Wow.exe")).unwrap(), b"exe-3");
        let saved = fs::read_dir(d.path().join(".coa-manager/replaced")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(fs::read(saved.join("Wow.exe")).unwrap(), b"my patched exe", "the player's version was moved aside, not lost");
    }

    #[test]
    fn renderer_files_configs_and_everything_not_in_the_manifest_survive_an_update_untouched() {
        let d = tempfile::tempdir().unwrap();
        let v1: Vec<(&str, &[u8])> = vec![("Wow.exe", b"exe-1"), ("d3d9.dll", b"official d3d9"), ("dxvk.conf", b"official conf"), ("Data/common.MPQ", b"common-1")];
        sync(d.path(), &manifest("v1", &v1), &Server::new(&v1), false).unwrap();
        // the player's own setup: custom renderer + configs, addons, WTF, realmlist, extra archives, backups
        let mine: Vec<(&str, &[u8])> = vec![
            ("d3d9.dll", b"my renderer"),
            ("dxvk.conf", b"my conf"),
            ("ModernWoWRenderer.ini", b"cfg"),
            ("GraphicsEffects.ini.bak", b"bak"),
            ("Data/enUS/realmlist.wtf", b"set realmlist 127.0.0.1
"),
            ("Data/patch-5.MPQ", b"extra archive"),
            ("Interface/AddOns/ElvUI/ElvUI.toc", b"## Title: Elv"),
            ("WTF/Account/x/SavedVariables.lua", b"vars"),
            ("Cache/x.wdb", b"cache"),
        ];
        for (p, b) in &mine {
            let path = d.path().join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b).unwrap();
        }
        let before: BTreeMap<String, String> = mine.iter().map(|(p, b)| (p.to_string(), fsx::sha256_bytes(b))).collect();
        let v2: Vec<(&str, &[u8])> = vec![("Wow.exe", b"exe-2"), ("d3d9.dll", b"official d3d9 v2"), ("dxvk.conf", b"official conf v2"), ("Data/common.MPQ", b"common-2")];
        let (m2, s2) = (manifest("v2", &v2), Server::new(&v2));
        for keep in [false, true] {
            sync(d.path(), &m2, &s2, keep).unwrap();
            for (p, h) in &before {
                assert_eq!(&fsx::sha256_file(&d.path().join(p)).unwrap(), h, "{p} must not change (keep_modified={keep})");
            }
            assert_eq!(fs::read(d.path().join("Wow.exe")).unwrap(), b"exe-2");
            assert_eq!(fs::read(d.path().join("Data/common.MPQ")).unwrap(), b"common-2");
        }
        assert!(!d.path().join(".coa-manager/replaced").exists(), "nothing of the player's was displaced");
    }

    #[test]
    fn an_existing_client_without_state_is_compared_by_content() {
        let d = tempfile::tempdir().unwrap();
        for (p, b) in V1 {
            let path = d.path().join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b).unwrap();
        }
        fs::write(d.path().join("Data/patch-A.MPQ"), b"patch-a-X").unwrap(); // same size, different content
        fs::remove_file(d.path().join("Data/enUS/locale.MPQ")).unwrap();
        fs::create_dir_all(d.path().join("WTF")).unwrap();
        fs::write(d.path().join("WTF/Config.wtf"), b"mine").unwrap();
        let m = manifest("v1", &V1);
        let p = plan(d.path(), &m, &State::default(), &Cancel::default(), &|_| {}).unwrap();
        let kinds: Vec<_> = p.items.iter().map(|i| (i.path.as_str(), i.kind)).collect();
        assert_eq!(kinds, [("Data/patch-A.MPQ", Kind::Modified), ("Data/enUS/locale.MPQ", Kind::Missing)]);
        assert_eq!(p.up_to_date_files, 2);
        assert_eq!(fs::read(d.path().join("WTF/Config.wtf")).unwrap(), b"mine");
    }

    #[test]
    fn cancel_keeps_finished_files_and_the_next_run_only_fetches_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let m = manifest("v1", &V1);
        let mut s = Server::new(&V1);
        let cancel = Cancel::default();
        s.cancel_after = Some((AtomicUsize::new(3), cancel.clone()));
        let state = State::default();
        let p = plan(d.path(), &m, &state, &cancel, &|_| {}).unwrap();
        let r = apply(&Apply { client: d.path(), manifest: &m, plan: &p, keep_modified: false, transport: &s, objects_url: "https://x.invalid/o", cancel: &cancel }, &|_| {});
        assert!(r.is_err());
        let st = load_state(d.path()).unwrap();
        assert!(st.version.is_none() && !st.files.is_empty() && st.files.len() < 4, "unfinished but remembered: {}", st.files.len());
        assert!(local(d.path()).managed && local(d.path()).version.is_none());

        let s2 = Server::new(&V1);
        sync(d.path(), &m, &s2, false).unwrap();
        assert_eq!(s2.requested.lock().unwrap().len(), 4 - st.files.len(), "only the missing files were requested");
        assert_eq!(load_state(d.path()).unwrap().version.as_deref(), Some("v1"));
    }

    #[test]
    fn a_wrong_object_never_reaches_the_client_folder() {
        let d = tempfile::tempdir().unwrap();
        let m = manifest("v1", &V1);
        let s = Server::new(&V1);
        // the server returns different bytes under the right hash
        let sha = fsx::sha256_bytes(b"exe-1");
        s.objects.lock().unwrap().insert(sha, b"EVIL!".to_vec());
        assert!(sync(d.path(), &m, &s, false).is_err());
        assert!(!d.path().join("Wow.exe").exists());
        assert!(load_state(d.path()).unwrap().version.is_none());
    }
}
