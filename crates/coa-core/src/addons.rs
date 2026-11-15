//! A catalog of game addons the Manager can install into a client's `Interface/AddOns`.
//!
//! Every addon is pinned to a commit of its GitHub repository; the catalog lists each file it installs with its
//! git blob SHA-1 and size, which are checked after download (GitHub's archives are not byte-stable, their
//! contents are). The Manager records what it installed in `Interface/AddOns/.coa-addons.json` and only ever
//! removes or replaces those folders; a folder of the same name that someone else put there is moved to
//! `Interface/AddOns/.coa-addons-backup` first.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::download::{Cancel, Transport};
use crate::error::{Error, Result};
use crate::fsx;

static CATALOG: &str = include_str!("../../../addons/catalog.json");
const STATE: &str = ".coa-addons.json";
const BACKUP: &str = ".coa-addons-backup";
/// No addon archive is anywhere near this; a larger answer is not what the catalog describes.
const MAX_ARCHIVE: u64 = 1 << 30;

#[derive(Debug, Clone, Deserialize)]
struct AddonFile {
    path: String,
    from: String,
    sha1: String,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    id: String,
    name: String,
    description: BTreeMap<String, String>,
    author: String,
    license: String,
    #[serde(default)]
    needs: Option<String>,
    repo: String,
    commit: String,
    archive: String,
    folders: Vec<String>,
    files: Vec<AddonFile>,
}

#[derive(Deserialize)]
struct Catalog {
    addons: Vec<Entry>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Installed {
    commit: String,
    folders: Vec<String>,
}

/// One addon of the catalog as the Manager shows it for a client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonView {
    pub id: String,
    pub name: String,
    pub description: BTreeMap<String, String>,
    pub author: String,
    pub license: String,
    /// A server module the addon is made for (`playerbots`), shown as a hint.
    pub needs: Option<String>,
    pub repo: String,
    pub size: u64,
    pub installed: bool,
    pub update_available: bool,
    /// Folders of this addon that are in the client but were not installed by the Manager.
    pub foreign_folders: Vec<String>,
}

fn catalog() -> Result<Vec<Entry>> {
    let catalog: Catalog = serde_json::from_str(CATALOG)?;
    for entry in &catalog.addons {
        for folder in &entry.folders {
            if folder.is_empty() || folder.contains(['/', '\\']) || folder.starts_with('.') {
                return Err(Error::Invalid(format!("Invalid addon folder in the catalog: {folder}")));
            }
        }
    }
    Ok(catalog.addons)
}

fn addons_dir(client: &Path) -> PathBuf {
    client.join("Interface").join("AddOns")
}

fn read_state(client: &Path) -> BTreeMap<String, Installed> {
    fsx::read_json(&addons_dir(client).join(STATE)).unwrap_or_default()
}

fn write_state(client: &Path, state: &BTreeMap<String, Installed>) -> Result<()> {
    fsx::atomic_write_json(&addons_dir(client).join(STATE), state)
}

/// The catalog with what is installed in `client`.
pub fn list(client: &Path) -> Result<Vec<AddonView>> {
    let state = read_state(client);
    let dir = addons_dir(client);
    let owned: Vec<&String> = state.values().flat_map(|i| i.folders.iter()).collect();
    Ok(catalog()?
        .into_iter()
        .map(|entry| {
            let installed = state.get(&entry.id);
            AddonView {
                foreign_folders: entry
                    .folders
                    .iter()
                    .filter(|f| dir.join(f).exists() && !owned.contains(f))
                    .cloned()
                    .collect(),
                size: entry.files.iter().map(|f| f.size).sum(),
                installed: installed.is_some(),
                update_available: installed.is_some_and(|i| i.commit != entry.commit),
                id: entry.id,
                name: entry.name,
                description: entry.description,
                author: entry.author,
                license: entry.license,
                needs: entry.needs,
                repo: entry.repo,
            }
        })
        .collect())
}

/// The git blob id of `bytes` (what the catalog records for each file).
fn git_blob_sha1(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn download(transport: &dyn Transport, url: &str, dest: &Path, cancel: &Cancel) -> Result<()> {
    crate::download::check_url(url)?;
    let reply = transport.get(url, 0).map_err(Error::Invalid)?;
    if reply.status != 200 {
        return Err(Error::Invalid(format!("The addon download answered HTTP {}.", reply.status)));
    }
    let mut body = reply.body.take(MAX_ARCHIVE + 1);
    let mut file = fs::File::create(dest)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        if cancel.is_set() {
            return Err(Error::Invalid("Operation cancelled.".into()));
        }
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_ARCHIVE {
            return Err(Error::Invalid("The addon download is larger than any catalog addon.".into()));
        }
        file.write_all(&buf[..n])?;
    }
    file.sync_all()?;
    Ok(())
}

/// Unpack the catalog's files of `entry` from `archive` into `staging`, checking every one.
fn unpack(entry: &Entry, archive: &Path, staging: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)
        .map_err(|e| Error::Invalid(format!("The addon download is not a zip archive: {e}")))?;
    // GitHub archives hold everything under one top folder (`<repo>-<commit>/`).
    let mut by_path = BTreeMap::new();
    for index in 0..zip.len() {
        let name = zip
            .by_index(index)
            .map_err(|e| Error::Invalid(e.to_string()))?
            .name()
            .replace('\\', "/");
        if let Some((_, inner)) = name.split_once('/') {
            by_path.insert(inner.to_string(), index);
        }
    }
    for file in &entry.files {
        let index = *by_path
            .get(&file.from)
            .ok_or_else(|| Error::Invalid(format!("The addon download lacks {}.", file.from)))?;
        let mut zipped = zip.by_index(index).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut bytes = Vec::with_capacity(file.size as usize);
        zipped.by_ref().take(file.size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != file.size || git_blob_sha1(&bytes) != file.sha1 {
            return Err(Error::Invalid(format!("{} failed verification.", file.from)));
        }
        let target = fsx::safe_join(staging, &file.path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, bytes)?;
    }
    Ok(())
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Install or update `id` in `client` from `transport`.
pub fn install_with(transport: &dyn Transport, client: &Path, id: &str, cancel: &Cancel) -> Result<Vec<AddonView>> {
    if crate::client::is_running(client) {
        return Err(Error::Invalid("Close the game before installing addons.".into()));
    }
    let entry = catalog()?
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| Error::Invalid(format!("Unknown addon {id}.")))?;
    install_entry(transport, client, &entry, cancel)?;
    list(client)
}

fn install_entry(transport: &dyn Transport, client: &Path, entry: &Entry, cancel: &Cancel) -> Result<()> {
    let dir = addons_dir(client);
    fs::create_dir_all(&dir)?;
    // Staged next to AddOns' own folders, so the final moves stay on one volume even for a linked AddOns.
    let staging = tempfile::Builder::new().prefix(".coa-addon-").tempdir_in(&dir)?;
    let archive = staging.path().join("archive.zip");
    download(transport, &entry.archive, &archive, cancel)?;
    let unpacked = staging.path().join("files");
    unpack(entry, &archive, &unpacked)?;
    if cancel.is_set() {
        return Err(Error::Invalid("Operation cancelled.".into()));
    }
    let mut state = read_state(client);
    let ours: Vec<String> = state.values().flat_map(|i| i.folders.clone()).collect();
    // Move what is in the way out first, so a failed move leaves the client as it was.
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let undo = |moved: &[(PathBuf, PathBuf)]| {
        for (from, to) in moved.iter().rev() {
            let _ = fs::rename(to, from);
        }
    };
    for folder in &entry.folders {
        let target = dir.join(folder);
        if !target.exists() {
            continue;
        }
        let aside = if ours.contains(folder) {
            staging.path().join("old").join(folder)
        } else {
            dir.join(BACKUP).join(format!("{folder}-{}", stamp()))
        };
        if let Some(parent) = aside.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(e) = fs::rename(&target, &aside) {
            undo(&moved);
            return Err(Error::Invalid(format!("{folder} is in use and could not be replaced: {e}")));
        }
        moved.push((target, aside));
    }
    let mut placed: Vec<PathBuf> = Vec::new();
    for folder in &entry.folders {
        let target = dir.join(folder);
        if let Err(e) = fs::rename(unpacked.join(folder), &target) {
            for p in &placed {
                let _ = fs::remove_dir_all(p);
            }
            undo(&moved);
            return Err(Error::Invalid(format!("{folder} could not be installed: {e}")));
        }
        placed.push(target);
    }
    state.insert(
        entry.id.clone(),
        Installed {
            commit: entry.commit.clone(),
            folders: entry.folders.clone(),
        },
    );
    write_state(client, &state)?;
    tracing::info!(addon = %entry.id, commit = %entry.commit, "addon installed");
    Ok(())
}

pub fn install(client: &Path, id: &str, cancel: &Cancel) -> Result<Vec<AddonView>> {
    install_with(&crate::download::HttpTransport::with_total_timeout(1800)?, client, id, cancel)
}

/// Remove an addon the Manager installed (only its recorded folders).
pub fn remove(client: &Path, id: &str) -> Result<Vec<AddonView>> {
    if crate::client::is_running(client) {
        return Err(Error::Invalid("Close the game before removing addons.".into()));
    }
    let mut state = read_state(client);
    let Some(installed) = state.remove(id) else {
        return list(client);
    };
    let dir = addons_dir(client);
    for folder in &installed.folders {
        if folder.is_empty() || folder.contains(['/', '\\']) || folder.starts_with('.') {
            continue;
        }
        let target = dir.join(folder);
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
    }
    write_state(client, &state)?;
    list(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Reply;

    struct Serve(Vec<u8>);
    impl Transport for Serve {
        fn get(&self, _: &str, _: u64) -> std::result::Result<Reply, String> {
            Ok(Reply { status: 200, content_range: None, body: Box::new(std::io::Cursor::new(self.0.clone())) })
        }
    }

    /// A GitHub-like archive of the catalog entry `id` with the given file bodies (path in repo -> bytes).
    fn archive(entry: &Entry, body: &dyn Fn(&AddonFile) -> Vec<u8>) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for file in &entry.files {
            zip.start_file(format!("repo-{}/{}", entry.commit, file.from), zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&body(file)).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    /// A small catalog entry whose files hash to what the archive will hold.
    fn small_entry() -> Entry {
        let bytes = b"## Interface: 30300\n".to_vec();
        Entry {
            id: "t".into(),
            name: "T".into(),
            description: BTreeMap::new(),
            author: "a".into(),
            license: "x".into(),
            needs: None,
            repo: "r".into(),
            commit: "c1".into(),
            archive: "https://example.invalid/a.zip".into(),
            folders: vec!["TestAddon".into()],
            files: vec![AddonFile {
                path: "TestAddon/TestAddon.toc".into(),
                from: "TestAddon/TestAddon.toc".into(),
                sha1: git_blob_sha1(&bytes),
                size: bytes.len() as u64,
            }],
        }
    }

    #[test]
    fn the_catalog_is_valid_and_pins_every_addon() {
        let entries = catalog().unwrap();
        assert!(entries.len() >= 4);
        for e in &entries {
            assert_eq!(e.commit.len(), 40, "{}", e.id);
            assert!(e.archive.starts_with("https://codeload.github.com/") && e.archive.ends_with(&e.commit));
            assert!(e.description.contains_key("en") && e.description.contains_key("ru"));
            for folder in &e.folders {
                assert!(e.files.iter().any(|f| f.path.eq_ignore_ascii_case(&format!("{folder}/{folder}.toc"))
                    || f.path.starts_with(&format!("{folder}/")) && f.path.ends_with(".toc")), "{folder} has no toc");
            }
            for f in &e.files {
                fsx::safe_join(Path::new("C:/x"), &f.path).unwrap();
                assert!(e.folders.iter().any(|folder| f.path.starts_with(&format!("{folder}/"))));
            }
        }
    }

    #[test]
    #[ignore = "Downloads the catalog's small addons from GitHub"]
    fn the_real_catalog_installs_from_github() {
        let client = tempfile::tempdir().unwrap();
        for id in ["incahoots", "coacombat", "squidbots-lite"] {
            let views = install(client.path(), id, &Cancel::default()).unwrap();
            let view = views.iter().find(|v| v.id == id).unwrap();
            assert!(view.installed && !view.update_available, "{id}");
        }
        assert!(addons_dir(client.path()).join("IncahootsCoA/IncahootsCoA.toc").is_file());
        assert!(addons_dir(client.path()).join("SquidBotsLite/SquidBotsLite.toc").is_file());
    }

    #[test]
    fn git_blob_ids_match_git() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(git_blob_sha1(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    fn unpack_and_place(entry: &Entry, client: &Path, bytes: Vec<u8>) -> Result<()> {
        let dir = addons_dir(client);
        fs::create_dir_all(&dir)?;
        let staging = tempfile::tempdir_in(&dir)?;
        let archive = staging.path().join("a.zip");
        download(&Serve(bytes), &entry.archive, &archive, &Cancel::default())?;
        unpack(entry, &archive, &staging.path().join("files"))
    }

    #[test]
    fn a_tampered_or_incomplete_download_is_refused() {
        let client = tempfile::tempdir().unwrap();
        let entry = small_entry();
        let good = archive(&entry, &|_| b"## Interface: 30300\n".to_vec());
        unpack_and_place(&entry, client.path(), good).unwrap();
        let bad = archive(&entry, &|_| b"## Interface: 30300 evil\n".to_vec());
        assert!(unpack_and_place(&entry, client.path(), bad).unwrap_err().to_string().contains("verification"));
        let empty = {
            let mut out = std::io::Cursor::new(Vec::new());
            zip::ZipWriter::new(&mut out).finish().unwrap();
            out.into_inner()
        };
        assert!(unpack_and_place(&entry, client.path(), empty).unwrap_err().to_string().contains("lacks"));
    }

    #[test]
    fn install_update_and_remove_keep_other_folders() {
        let client = tempfile::tempdir().unwrap();
        let mut entry = small_entry();
        let dir = addons_dir(client.path());
        fs::create_dir_all(dir.join("TestAddon")).unwrap();
        fs::write(dir.join("TestAddon/mine.lua"), b"player's own").unwrap();
        fs::create_dir_all(dir.join("Other")).unwrap();
        let serve = Serve(archive(&entry, &|_| b"## Interface: 30300
".to_vec()));
        install_entry(&serve, client.path(), &entry, &Cancel::default()).unwrap();
        assert!(dir.join("TestAddon/TestAddon.toc").is_file());
        assert!(!dir.join("TestAddon/mine.lua").exists(), "the player's folder was set aside");
        let backups: Vec<_> = fs::read_dir(dir.join(BACKUP)).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].path().join("mine.lua").is_file());
        assert_eq!(read_state(client.path())["t"].commit, "c1");
        // An update replaces the Manager's own folder without another backup.
        entry.commit = "c2".into();
        install_entry(&serve, client.path(), &entry, &Cancel::default()).unwrap();
        assert_eq!(read_state(client.path())["t"].commit, "c2");
        assert_eq!(fs::read_dir(dir.join(BACKUP)).unwrap().count(), 1);
        // A failed download changes nothing.
        let broken = Serve(archive(&entry, &|_| b"tampered".to_vec()));
        assert!(install_entry(&broken, client.path(), &entry, &Cancel::default()).is_err());
        assert!(dir.join("TestAddon/TestAddon.toc").is_file());
        remove(client.path(), "t").unwrap();
        assert!(!dir.join("TestAddon").exists());
        assert!(dir.join("Other").is_dir());
        assert!(!read_state(client.path()).contains_key("t"));
        // No leftover staging folders.
        assert!(fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".coa-addon-")));
    }
}
