//! Management of client-side patches and assets for Custom Races.
//!
//! Handles detecting, enabling/disabling (via `.disabled` extension renaming),
//! and downloading/extracting client patches (Esteria and additional races)
//! into the player's WoW client directory.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download::Cancel;
use crate::error::{Error, Result};
use crate::fsx;

pub const CLIENT_VERSION: &str = "1.7";
pub const MANIFEST_FILENAME: &str = "custom_races_patch_manifest.txt";
pub const ARCHIVES: &[(&str, &str)] = &[
    (
        "CoA-Custom-1.6-main-20261012-4dd3a4ec.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.6/CoA-Custom-1.6-main-20261012-4dd3a4ec.zip",
    ),
    (
        "CoA-Custom-1.6-client-part1.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.6/CoA-Custom-1.6-client-part1.zip",
    ),
    (
        "CoA-Custom-1.6-client-part2.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.6/CoA-Custom-1.6-client-part2.zip",
    ),
    (
        "CoA-Custom-1.7-client.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.7/CoA-Custom-1.7-client.zip",
    ),
];

pub const RACE_MPQS: &[&str] = &[
    "patch-T.MPQ",
    "patch-ZE1.MPQ",
    "patch-ZE2.MPQ",
    "patch-ZE3.MPQ",
    "patch-ZE5.MPQ",
    "patch-ZE6.MPQ",
    "patch-ZE7.MPQ",
    "patch-ZE8.MPQ",
    "patch-ZE9.MPQ",
    "patch-ZEA.MPQ",
    "patch-ZG.MPQ",
    "patch-ZH.MPQ",
    "patch-ZHA.MPQ",
    "patch-ZHM.MPQ",
    "patch-ZM.MPQ",
    "patch-ZVU.MPQ",
    "patch-ZES.MPQ",
    "patch-ZJ.MPQ",
    "patch-ZZE.MPQ",
    "patch-ZZR.MPQ",
    "patch-ZZS.MPQ",
    "patch-ZZT.MPQ",
    "patch-ZZU.MPQ",
    "patch-ZZV.MPQ",
    "patch-ZZW.MPQ",
];

pub const EXCLUDED_BOT_RACES_RECOMMENDED: &str =
    "9,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128";

pub const EXCLUDED_BOT_RACES_EXPERIMENTAL: &str =
    "65,72,77,80,81";

/// Finds all active playerbots.conf files on this server (root, coa-bots folder, and secondary realm).
pub fn playerbots_conf_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let candidates = [
        root.join("Core/configs/modules/playerbots.conf"),
        root.join("coa-bots/Core/configs/modules/playerbots.conf"),
        root.join("coa-bots/core/configs/modules/playerbots.conf"),
        crate::multiworld::root(root).join("Core/configs/modules/playerbots.conf"),
    ];
    for path in candidates {
        if path.is_file() {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        for dist in [
            root.join("Core/configs/modules/playerbots.conf.dist"),
            root.join("coa-bots/Core/configs/modules/playerbots.conf.dist"),
        ] {
            if dist.is_file() {
                let target = dist.with_extension("");
                if fs::copy(&dist, &target).is_ok() {
                    paths.push(target);
                    break;
                }
            }
        }
    }
    paths
}

/// Checks whether playerbots are configured to allow custom races.
/// Returns true if AiPlayerbot.ExcludedBotRaces exists and does not exclude general custom races.
pub fn is_bot_custom_races_allowed(root: &Path) -> Option<bool> {
    for path in playerbots_conf_paths(root) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(conf) = crate::config::parser::ConfFile::parse_bytes(&bytes) {
                if let Some(val) = conf.get("AiPlayerbot.ExcludedBotRaces") {
                    let clean = val.trim().trim_matches(['"', '\'']);
                    let excluded: std::collections::BTreeSet<u32> = clean.split(',').filter_map(|race| race.trim().parse().ok()).collect();
                    return Some((1..=128).filter(|race| ![1, 2, 3, 4, 5, 6, 7, 8, 10, 11].contains(race)).any(|race| !excluded.contains(&race)));
                }
            }
        }
    }
    None
}

/// Sets AiPlayerbot.ExcludedBotRaces in all discovered playerbots.conf files.
pub fn sync_bot_races_config(root: &Path, allow_custom_races: bool) -> Result<()> {
    let excluded = if allow_custom_races {
        EXCLUDED_BOT_RACES_EXPERIMENTAL
    } else {
        EXCLUDED_BOT_RACES_RECOMMENDED
    };
    for path in playerbots_conf_paths(root) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(mut conf) = crate::config::parser::ConfFile::parse_bytes(&bytes) {
                conf.set(
                    "AiPlayerbot.ExcludedBotRaces",
                    &format!("\"{excluded}\""),
                    &[
                        "CoA Custom: races new random bots never get",
                        "Off (recommended): excludes all added races",
                        "On (experimental): allows stable added races",
                    ],
                );
                fsx::atomic_write(&path, conf.to_text().as_bytes())?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientPatchState {
    NotInstalled,
    Enabled,
    Disabled,
    PartiallyInstalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientPatchStatus {
    pub state: ClientPatchState,
    pub version: Option<String>,
    pub installed_mpqs: usize,
    pub total_mpqs: usize,
    pub has_root_files: bool,
    pub client_path: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PatchFile {
    path: String,
    sha256: String,
}

fn patch_files(client: &Path) -> Result<Vec<PatchFile>> {
    let manifest = fs::read_to_string(client.join(MANIFEST_FILENAME)).unwrap_or_default();
    let hashes = if manifest.lines().any(|line| line == "# CoA Custom 1.5") { include_str!("custom_races_hashes_1_5.json") } else if manifest.lines().any(|line| line == "# CoA Custom 1.6") { include_str!("custom_races_hashes_1_6.json") } else { include_str!("custom_races_hashes.json") };
    let bundled: std::collections::BTreeMap<String, String> = serde_json::from_str(hashes)?;
    let mut files = std::collections::BTreeMap::new();
    for line in manifest.lines() {
        let path = if let Some(name) = line.strip_prefix("MPQ:") {
            if name.contains(['/', '\\']) { return Err(Error::Invalid("Invalid custom races patch manifest.".into())); }
            Some(format!("Data/{name}"))
        } else if let Some(name) = line.strip_prefix("ROOT:") {
            if name.contains(['/', '\\']) { return Err(Error::Invalid("Invalid custom races patch manifest.".into())); }
            Some(name.to_string())
        } else { None };
        if let Some(path) = path {
            fsx::safe_join(client, &path)?;
            files.insert(path.clone(), bundled.get(&path).cloned().unwrap_or_default());
        }
    }
    if files.is_empty() { files = bundled; }
    for line in manifest.lines().filter_map(|line| line.strip_prefix("SHA256:")) {
        let (hash, path) = line.split_once(':').ok_or_else(|| Error::Invalid("Invalid custom races checksum.".into()))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || !files.contains_key(path) {
            return Err(Error::Invalid("Invalid custom races checksum.".into()));
        }
        files.insert(path.into(), hash.into());
    }
    Ok(files.into_iter().map(|(path, sha256)| PatchFile { path, sha256 }).collect())
}

fn suffix(path: &Path, extension: &str) -> PathBuf {
    path.with_file_name(format!("{}.{extension}", path.file_name().unwrap_or_default().to_string_lossy()))
}

fn matches_patch(path: &Path, file: &PatchFile) -> bool {
    path.is_file() && fsx::sha256_file(path).is_ok_and(|hash| hash == file.sha256)
}

/// Only complete, verified payloads are reported as enabled or disabled.
pub fn client_patch_status(client_path: &Path) -> ClientPatchStatus {
    let data = client_path.join("Data");
    let installed_mpqs = RACE_MPQS.iter().filter(|name| data.join(name).is_file() || suffix(&data.join(name), "disabled").is_file()).count();
    let has_root_files = client_path.join("EsteriaAppearance.dll").is_file() || client_path.join("dinput8.dll").is_file() || client_path.join("dinput8.dll.disabled").is_file();
    let manifest = fs::read_to_string(client_path.join(MANIFEST_FILENAME)).unwrap_or_default();
    let current = manifest.lines().any(|line| line == format!("# CoA Custom {CLIENT_VERSION}"));
    let state = if !manifest.is_empty() && current && !client_path.join(".coa-races-transaction").exists() {
        match patch_files(client_path) {
            Ok(files) if files.iter().all(|f| matches_patch(&client_path.join(&f.path), f) && !suffix(&client_path.join(&f.path), "disabled").exists()) => ClientPatchState::Enabled,
            Ok(files) if files.iter().all(|f| matches_patch(&suffix(&client_path.join(&f.path), "disabled"), f)) => ClientPatchState::Disabled,
            _ => ClientPatchState::PartiallyInstalled,
        }
    } else if !manifest.is_empty() || patch_files(client_path).is_ok_and(|files| files.iter().filter(|file| file.path.starts_with("Data/") && file.path != "Data/patch-T.MPQ").any(|file| matches_patch(&client_path.join(&file.path), file) || matches_patch(&suffix(&client_path.join(&file.path), "disabled"), file))) {
        ClientPatchState::PartiallyInstalled
    } else { ClientPatchState::NotInstalled };
    let version = manifest.lines().find_map(|line| line.strip_prefix("# CoA Custom ").map(str::to_string));
    ClientPatchStatus { state, version, installed_mpqs, total_mpqs: RACE_MPQS.len(), has_root_files,
        client_path: Some(client_path.to_string_lossy().into_owned()) }
}

#[derive(Serialize, Deserialize)]
struct SavedFile { path: String, existed: bool }

struct PatchTransaction<'a> { root: &'a Path, entries: Vec<SavedFile>, finished: bool }
impl<'a> PatchTransaction<'a> {
    fn new(root: &'a Path) -> Result<Self> {
        recover_patch_transaction(root)?;
        fs::create_dir(root.join(".coa-races-transaction"))?;
        Ok(Self { root, entries: Vec::new(), finished: false })
    }
    fn save(&mut self, path: &Path) -> Result<()> {
        let relative = path.strip_prefix(self.root).map_err(|_| Error::Invalid("Patch path escaped the client directory.".into()))?.to_string_lossy().replace('\\', "/");
        fsx::safe_join(self.root, &relative)?;
        if self.entries.iter().any(|entry| entry.path == relative) { return Ok(()); }
        let index = self.entries.len();
        self.entries.push(SavedFile { path: relative, existed: path.is_file() });
        fsx::atomic_write_json(&self.root.join(".coa-races-transaction/journal.json"), &self.entries)?;
        if path.is_file() { fs::rename(path, self.root.join(".coa-races-transaction").join(index.to_string()))?; }
        Ok(())
    }
    fn copy(&mut self, source: &Path, target: &Path) -> Result<()> {
        self.save(target)?;
        fs::create_dir_all(target.parent().ok_or_else(|| Error::Invalid("Patch file has no parent directory.".into()))?)?;
        fs::copy(source, target)?;
        Ok(())
    }
    fn move_file(&mut self, source: &Path, target: &Path) -> Result<()> {
        self.save(source)?;
        let index = self.entries.iter().position(|entry| self.root.join(&entry.path) == source).unwrap();
        // Copy from the journal so rollback retains the previous bytes.
        let saved = self.root.join(".coa-races-transaction").join(index.to_string());
        self.copy(&saved, target)
    }
    fn finish(mut self) -> Result<()> {
        fsx::atomic_write(&self.root.join(".coa-races-transaction/committed"), b"committed")?;
        self.finished = true;
        fs::remove_dir_all(self.root.join(".coa-races-transaction"))?;
        Ok(())
    }
}
impl Drop for PatchTransaction<'_> {
    fn drop(&mut self) { if !self.finished { let _ = recover_patch_transaction(self.root); } }
}

fn recover_patch_transaction(root: &Path) -> Result<()> {
    let directory = root.join(".coa-races-transaction");
    if !directory.is_dir() { return Ok(()); }
    if !directory.join("committed").exists() && directory.join("journal.json").exists() {
        let entries: Vec<SavedFile> = fsx::read_json(&directory.join("journal.json"))?;
        for (index, entry) in entries.iter().enumerate().rev() {
            let path = fsx::safe_join(root, &entry.path)?;
            let saved = directory.join(index.to_string());
            if saved.is_file() {
                if path.is_file() { fs::remove_file(&path)?; }
                fs::rename(saved, path)?;
            } else if !entry.existed && path.is_file() { fs::remove_file(path)?; }
        }
    }
    fs::remove_dir_all(directory)?;
    Ok(())
}

fn patch_lock(client: &Path) -> Result<File> {
    use fs4::fs_std::FileExt;
    let lock = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(client.join(".coa-races.lock"))?;
    if !lock.try_lock_exclusive()? { return Err(Error::Invalid("Another custom races operation is using this client.".into())); }
    Ok(lock)
}

fn preserve_original(tx: &mut PatchTransaction<'_>, active: &Path) -> Result<()> {
    let original = suffix(active, "original");
    if original.is_file() && fsx::sha256_file(active)? == fsx::sha256_file(&original)? { return Ok(()); }
    if original.is_file() {
        let previous = suffix(active, &format!("original.previous-{}", uuid::Uuid::new_v4().simple()));
        tx.copy(&original, &previous)?;
    }
    tx.copy(active, &original)
}

/// Preserve launcher replacements, and switch every file owned by the package.
pub fn set_client_patch_enabled(client: &Path, enabled: bool) -> Result<()> {
    if !client.is_dir() { return Err(Error::Invalid("Game client directory not found.".into())); }
    if crate::client::is_running(client) { return Err(Error::Invalid("Close the game before changing custom races patches.".into())); }
    let _lock = patch_lock(client)?;
    recover_patch_transaction(client)?;
    if !client.join(MANIFEST_FILENAME).is_file() { return Ok(()); }
    let files = patch_files(client)?;
    if enabled {
        for file in &files {
            let active = client.join(&file.path);
            if !matches_patch(&active, file) && !matches_patch(&suffix(&active, "disabled"), file) {
                return Err(Error::Invalid(format!("Custom races file {} was replaced or is missing. Repair the client patch before enabling it.", file.path)));
            }
        }
    }
    let mut tx = PatchTransaction::new(client)?;
    for file in files {
        let active = client.join(&file.path);
        let disabled = suffix(&active, "disabled");
        let original = suffix(&active, "original");
        if enabled {
            if active.is_file() && !matches_patch(&active, &file) { preserve_original(&mut tx, &active)?; }
            if matches_patch(&disabled, &file) { tx.move_file(&disabled, &active)?; }
        } else if matches_patch(&active, &file) {
            if disabled.is_file() && !matches_patch(&disabled, &file) {
                tx.move_file(&disabled, &suffix(&disabled, &format!("replaced-{}", uuid::Uuid::new_v4().simple())))?;
            }
            tx.move_file(&active, &disabled)?;
            if original.is_file() { tx.copy(&original, &active)?; }
        } else if active.is_file() {
            preserve_original(&mut tx, &active)?;
        }
    }
    tx.finish()
}

pub fn uninstall_client_patches(client: &Path) -> Result<()> {
    if crate::client::is_running(client) { return Err(Error::Invalid("Close the game before removing custom races patches.".into())); }
    let _lock = patch_lock(client)?;
    recover_patch_transaction(client)?;
    if !client.join(MANIFEST_FILENAME).is_file() { return Ok(()); }
    let mut tx = PatchTransaction::new(client)?;
    for file in patch_files(client)? {
        let active = client.join(&file.path);
        if matches_patch(&active, &file) {
            tx.save(&active)?;
            let original = suffix(&active, "original");
            if original.is_file() { tx.copy(&original, &active)?; }
        }
        let disabled = suffix(&active, "disabled");
        if active.is_file() && !matches_patch(&active, &file) && !suffix(&active, "original").is_file() {
            tx.move_file(&active, &suffix(&active, &format!("replaced-{}", uuid::Uuid::new_v4().simple())))?;
        }
        if disabled.is_file() {
            if !matches_patch(&disabled, &file) { tx.move_file(&disabled, &suffix(&disabled, &format!("replaced-{}", uuid::Uuid::new_v4().simple())))?; }
            else { tx.save(&disabled)?; }
        }
    }
    tx.save(&client.join(MANIFEST_FILENAME))?;
    tx.finish()
}

fn verify_archive(path: &Path, filename: &str) -> Result<()> {
    let expected = match filename {
        "CoA-Custom-1.6-main-20261012-4dd3a4ec.zip" => "dfe802ea70047c129ac2e0aaa557bab3110e3cc6926d36403b8c880a0e310ee4",
        "CoA-Custom-1.6-client-part1.zip" => "1dbf1901ad5be61eba66973609df90712ffc78ef9e9b2c8549a5f3e36332bf12",
        "CoA-Custom-1.6-client-part2.zip" => "689f7f946b4180fe977dabedc7cfe642e3e7f27431600c64c4dd5b7960b1bc66",
        "CoA-Custom-1.7-client.zip" => "ad389ef486f44f269445f80807b25b214c39fc5d21272b5708bfeac8b1efefa0",
        _ => return Err(Error::Invalid("Unknown custom races archive.".into())),
    };
    if fsx::sha256_file(path)? != expected {
        return Err(Error::Invalid(format!("Custom races archive {filename} failed verification. Remove it and try again.")));
    }
    Ok(())
}

/// Locate an archive locally in Downloads or cache, or download it from GitHub.
fn resolve_or_download_archive(
    filename: &str,
    url: &str,
    client: &reqwest::blocking::Client,
    cancel: &Cancel,
    report: &dyn Fn(&str, f32),
    base_percent: f32,
    percent_slice: f32,
) -> Result<PathBuf> {
    // 1. Check user Downloads folder
    if let Some(userprofile) = std::env::var_os("USERPROFILE") {
        let downloads = PathBuf::from(userprofile).join("Downloads");
        let candidate = downloads.join(filename);
        if candidate.is_file() {
            if let Ok(meta) = candidate.metadata() {
                if meta.len() > 1000 {
                    tracing::info!(path = %candidate.display(), "using existing archive in Downloads");
                    verify_archive(&candidate, filename)?;
                    return Ok(candidate);
                }
            }
        }
    }

    // 2. Check temp / cache directory
    let cache_dir = std::env::temp_dir().join("coa-client-races");
    fs::create_dir_all(&cache_dir)?;
    let target = cache_dir.join(filename);
    if target.is_file() {
        if let Ok(meta) = target.metadata() {
            if meta.len() > 1000 {
                verify_archive(&target, filename)?;
                return Ok(target);
            }
        }
    }

    // 3. Download from GitHub Releases
    report(&format!("Downloading {filename}..."), base_percent);
    let part_path = cache_dir.join(format!("{filename}.part"));

    let resp = client
        .get(url)
        .send()
        .map_err(|e| Error::NetworkUnreachable(format!("Could not reach download server: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::NetworkUnreachable(format!(
            "Download failed with status {} for {url}",
            resp.status()
        )));
    }

    let total_bytes = resp.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut src = resp;
    let mut dst = File::create(&part_path)?;

    let mut buf = [0u8; 64 * 1024];
    let mut last_report = std::time::Instant::now();

    loop {
        if cancel.is_set() {
            let _ = fs::remove_file(&part_path);
            return Err(Error::Invalid("Operation cancelled.".into()));
        }

        let n = src
            .read(&mut buf)
            .map_err(|e| Error::NetworkUnreachable(format!("Download interrupted: {e}")))?;
        if n == 0 {
            break;
        }

        dst.write_all(&buf[..n])?;
        downloaded += n as u64;

        if last_report.elapsed() > std::time::Duration::from_millis(200) {
            let ratio = if total_bytes > 0 {
                (downloaded as f32 / total_bytes as f32).clamp(0.0, 1.0)
            } else {
                0.5
            };
            report(
                &format!(
                    "Downloading {filename} ({:.1} MB)...",
                    downloaded as f64 / 1_048_576.0
                ),
                base_percent + ratio * percent_slice,
            );
            last_report = std::time::Instant::now();
        }
    }

    dst.flush()?;
    drop(dst);

    verify_archive(&part_path, filename)?;
    fs::rename(&part_path, &target)?;
    Ok(target)
}

/// Download (if needed) and extract custom races client assets into the target WoW client.
pub fn install_client_patches(
    client_path: &Path,
    cancel: &Cancel,
    report: &dyn Fn(&str, f32),
) -> Result<()> {
    if !client_path.is_dir() {
        return Err(Error::Invalid("Game client directory does not exist.".into()));
    }

    if crate::client::is_running(client_path) {
        return Err(Error::Invalid("Close the game before installing custom races patches.".into()));
    }
    let _lock = patch_lock(client_path)?;
    recover_patch_transaction(client_path)?;
    let staging = tempfile::Builder::new().prefix(".coa-races-stage-").tempdir_in(client_path)?;
    let data_dir = staging.path().join("Data");
    fs::create_dir_all(&data_dir)?;

    // 2. Resolve/download archives (0.05 .. 0.50)
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!("CoA-Server-Manager/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| Error::Invalid(format!("Cannot prepare downloader: {e}")))?;

    let mut resolved_archives = Vec::new();
    let n_archives = ARCHIVES.len() as f32;
    for (i, (fname, url)) in ARCHIVES.iter().enumerate() {
        let base_pct = 0.05 + (i as f32 / n_archives) * 0.45;
        let slice = 0.45 / n_archives;
        let archive_path =
            resolve_or_download_archive(fname, url, &http, cancel, report, base_pct, slice)?;
        resolved_archives.push(archive_path);
    }

    // 3. Extract archives (0.50 .. 0.95)
    let mut installed_mpqs = Vec::new();
    let mut installed_roots = Vec::new();

    for (i, arch_path) in resolved_archives.iter().enumerate() {
        if cancel.is_set() {
            return Err(Error::Invalid("Operation cancelled.".into()));
        }

        let arch_name = arch_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        report(
            &format!("Unpacking {arch_name}..."),
            0.50 + (i as f32 / n_archives) * 0.45,
        );

        let file = File::open(arch_path)?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| Error::Invalid(format!("Failed to open zip {arch_name}: {e}")))?;

        let num_files = zip.len();
        for file_idx in 0..num_files {
            if cancel.is_set() {
                return Err(Error::Invalid("Operation cancelled.".into()));
            }

            let mut zfile = zip
                .by_index(file_idx)
                .map_err(|e| Error::Invalid(format!("Zip entry error in {arch_name}: {e}")))?;

            if zfile.is_dir() {
                continue;
            }

            let zname = zfile.name().to_string();
            let zname_norm = zname.replace('\\', "/");

            if zname_norm.contains("files/client/") {
                let base = Path::new(&zname_norm)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if base.to_ascii_lowercase().ends_with(".mpq") {
                    let dest = data_dir.join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    let disabled = data_dir.join(format!("{base}.disabled"));
                    if disabled.is_file() { fs::remove_file(disabled)?; }
                    installed_mpqs.push(base);
                } else if base.eq_ignore_ascii_case("dinput8.dll") {
                    let dest = staging.path().join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    let disabled = staging.path().join(format!("{base}.disabled"));
                    if disabled.is_file() { fs::remove_file(disabled)?; }
                    installed_roots.push(base);
                }
            } else if zname_norm.contains("files/client_root/") {
                let base = Path::new(&zname_norm)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if !base.is_empty() {
                    let dest = staging.path().join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    let disabled = staging.path().join(format!("{base}.disabled"));
                    if disabled.is_file() { fs::remove_file(disabled)?; }
                    installed_roots.push(base);
                }
            }
        }
    }

    if !RACE_MPQS.iter().all(|name| installed_mpqs.iter().any(|installed| installed == name)) {
        return Err(Error::Invalid("Custom races archives are missing required client patches.".into()));
    }

    // 4. Record manifest
    report("Finalizing manifest...", 0.98);
    let manifest_path = client_path.join(MANIFEST_FILENAME);
    installed_mpqs.sort();
    installed_mpqs.dedup();
    installed_roots.sort();
    installed_roots.dedup();

    let mut manifest_body = format!("# CoA Custom {CLIENT_VERSION}\n# Custom Race MPQ files\n");
    for m in &installed_mpqs {
        manifest_body.push_str(&format!("MPQ:{m}\n"));
    }
    manifest_body.push_str("# Custom Race root files\n");
    for r in &installed_roots {
        manifest_body.push_str(&format!("ROOT:{r}\n"));
    }

    let bundled: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes.json"))?;
    let mut payload = Vec::new();
    for path in installed_mpqs.iter().map(|name| format!("Data/{name}")).chain(installed_roots.iter().cloned()) {
        let staged = fsx::safe_join(staging.path(), &path)?;
        let hash = fsx::sha256_file(&staged)?;
        if bundled.get(&path) != Some(&hash) { return Err(Error::Invalid(format!("Custom races file {path} failed verification."))); }
        manifest_body.push_str(&format!("SHA256:{hash}:{path}\n"));
        payload.push(PatchFile { path, sha256: hash });
    }
    if cancel.is_set() { return Err(Error::Invalid("Operation cancelled.".into())); }
    if payload.len() != bundled.len() { return Err(Error::Invalid("Custom races archives are missing required runtime files.".into())); }
    let previous = patch_files(client_path)?;
    let mut tx = PatchTransaction::new(client_path)?;
    for file in payload {
        let dest = fsx::safe_join(client_path, &file.path)?;
        if dest.is_file() && !previous.iter().any(|old| old.path == file.path && matches_patch(&dest, old)) && !matches_patch(&dest, &file) {
            preserve_original(&mut tx, &dest)?;
        }
        tx.copy(&staging.path().join(&file.path), &dest)?;
        let disabled = suffix(&dest, "disabled");
        if disabled.is_file() {
            if !previous.iter().any(|old| old.path == file.path && matches_patch(&disabled, old)) && !matches_patch(&disabled, &file) {
                tx.move_file(&disabled, &suffix(&disabled, &format!("replaced-{}", uuid::Uuid::new_v4().simple())))?;
            } else { tx.save(&disabled)?; }
        }
    }
    tx.save(&manifest_path)?;
    fsx::atomic_write(&manifest_path, manifest_body.as_bytes())?;
    tx.finish()?;
    report("Custom races client patches installed successfully!", 1.0);
    tracing::info!(
        mpqs = installed_mpqs.len(),
        roots = installed_roots.len(),
        "custom races client patch installation complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_files(root: &Path, names: &[&str]) {
        let mut manifest = format!("# CoA Custom {CLIENT_VERSION}\n");
        for name in names {
            let (kind, base) = name.strip_prefix("Data/").map_or(("ROOT", *name), |base| ("MPQ", base));
            manifest.push_str(&format!("{kind}:{base}\nSHA256:{}:{name}\n", fsx::sha256_file(&root.join(name)).unwrap()));
        }
        fs::write(root.join(MANIFEST_FILENAME), manifest).unwrap();
    }

    #[test]
    #[ignore = "Requires the downloaded release archives"]
    fn bundled_hashes_match_verified_release_archives() {
        use sha2::{Digest, Sha256};
        let folder = PathBuf::from(std::env::var_os("COA_RACES_ARCHIVE_DIR").expect("COA_RACES_ARCHIVE_DIR"));
        let expected: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let mut actual = std::collections::BTreeMap::new();
        for (name, _) in ARCHIVES {
            let path = folder.join(name);
            verify_archive(&path, name).unwrap();
            let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).unwrap();
                let name = entry.name().replace('\\', "/");
                if entry.is_dir() { continue; }
                let relative = if let Some(name) = name.split("files/client_root/").nth(1) { name.to_string() }
                    else if let Some(name) = name.split("files/client/").nth(1) { if name == "dinput8.dll" { name.to_string() } else { format!("Data/{name}") } }
                    else { continue };
                let mut hash = Sha256::new();
                let mut buffer = vec![0; 1024 * 1024];
                loop { let read = entry.read(&mut buffer).unwrap(); if read == 0 { break; } hash.update(&buffer[..read]); }
                actual.insert(relative, hex::encode(hash.finalize()));
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn legacy_installation_uses_its_own_hashes_when_preserving_originals() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.5\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes_1_5.json")).unwrap();
        let current: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.iter().find(|file| file.path == "Ascension.exe").unwrap().sha256, legacy["Ascension.exe"]);
        assert_eq!(files.len(), legacy.len());
        assert!(current.len() > legacy.len());
        assert!(!files.iter().any(|file| file.path == "Data/patch-ZZS.MPQ"));
    }

    #[test]
    fn previous_release_uses_its_own_hashes_for_upgrade_and_removal() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.6\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes_1_6.json")).unwrap();
        let current: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), legacy.len());
        assert_eq!(files.iter().find(|file| file.path == "Data/patch-T.MPQ").unwrap().sha256, legacy["Data/patch-T.MPQ"]);
        assert_ne!(legacy["Data/patch-T.MPQ"], current["Data/patch-T.MPQ"]);
        assert!(!files.iter().any(|file| file.path == "Data/patch-ZZW.MPQ"));
        assert!(legacy.keys().all(|key| current.contains_key(key)));
    }

    #[test]
    fn another_patch_operation_cannot_enter_the_same_client() {
        let temp = tempfile::tempdir().unwrap();
        let first = patch_lock(temp.path()).unwrap();
        assert!(patch_lock(temp.path()).is_err());
        drop(first);
        patch_lock(temp.path()).unwrap();
    }

    #[test]
    fn switching_preserves_launcher_updates_for_every_owned_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        let names = ["Data/patch-T.MPQ", "Data/patch-ZE1.MPQ", "Ascension.exe", "dinput8.dll", "EsteriaAppearance.dll"];
        for name in names { fs::write(root.join(name), b"custom").unwrap(); fs::write(suffix(&root.join(name), "original"), b"old official").unwrap(); }
        record_files(root, &names);
        set_client_patch_enabled(root, false).unwrap();
        for name in names { assert_eq!(fs::read(root.join(name)).unwrap(), b"old official"); fs::write(root.join(name), b"new official").unwrap(); }
        set_client_patch_enabled(root, true).unwrap();
        for name in names { assert_eq!(fs::read(root.join(name)).unwrap(), b"custom"); assert_eq!(fs::read(suffix(&root.join(name), "original")).unwrap(), b"new official"); }
        // An official launcher can also replace a currently enabled file.
        fs::write(root.join("Ascension.exe"), b"newest official").unwrap();
        assert_eq!(client_patch_status(root).state, ClientPatchState::PartiallyInstalled);
        set_client_patch_enabled(root, false).unwrap();
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), b"newest official");
        assert!(set_client_patch_enabled(root, true).unwrap_err().to_string().contains("Repair"));
        uninstall_client_patches(root).unwrap();
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), b"newest official");
        assert_eq!(client_patch_status(root).state, ClientPatchState::NotInstalled);
    }

    #[test]
    fn failed_and_interrupted_transactions_restore_previous_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let file = root.join("Ascension.exe");
        fs::write(&file, b"previous").unwrap();
        {
            let mut tx = PatchTransaction::new(root).unwrap();
            tx.save(&file).unwrap();
            fs::write(&file, b"replacement").unwrap();
            let staged = root.join("staged.dll");
            fs::write(&staged, b"new").unwrap();
            tx.copy(&staged, &root.join("Data/new.dll")).unwrap();
            assert_eq!(fs::read(root.join("Data/new.dll")).unwrap(), b"new");
        }
        assert_eq!(fs::read(&file).unwrap(), b"previous");
        assert!(!root.join("Data/new.dll").exists());
        let mut tx = PatchTransaction::new(root).unwrap();
        tx.save(&file).unwrap();
        fs::write(&file, b"interrupted").unwrap();
        std::mem::forget(tx);
        recover_patch_transaction(root).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"previous");
    }

    #[test]
    fn existing_backups_and_foreign_disabled_files_are_retained() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let file = root.join("dinput8.dll");
        fs::write(&file, b"custom").unwrap();
        record_files(root, &["dinput8.dll"]);
        fs::write(suffix(&file, "disabled"), b"third party").unwrap();
        fs::write(suffix(&file, "original"), b"original").unwrap();
        set_client_patch_enabled(root, false).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert!(fs::read_dir(root).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().contains("replaced-")));
        fs::write(&file, b"updated original").unwrap();
        set_client_patch_enabled(root, true).unwrap();
        assert!(fs::read_dir(root).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().contains("original.previous-")));
    }

    #[test]
    fn uninstall_moves_corrupted_custom_only_assets_out_of_the_active_client() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        let file = root.join("Data/patch-ZE1.MPQ");
        fs::write(&file, b"custom").unwrap();
        record_files(root, &["Data/patch-ZE1.MPQ"]);
        fs::write(&file, b"corrupted custom").unwrap();
        uninstall_client_patches(root).unwrap();
        assert!(!file.exists());
        assert!(fs::read_dir(root.join("Data")).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().contains("replaced-")));
        assert_eq!(client_patch_status(root).state, ClientPatchState::NotInstalled);
    }

    #[test]
    fn unknown_or_corrupt_bytes_are_not_reported_as_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        for name in RACE_MPQS { fs::write(root.join("Data").join(name), b"corrupt").unwrap(); }
        fs::write(root.join("dinput8.dll"), b"corrupt").unwrap();
        fs::write(root.join(MANIFEST_FILENAME), format!("# CoA Custom {CLIENT_VERSION}\n")).unwrap();
        assert_eq!(client_patch_status(root).state, ClientPatchState::PartiallyInstalled);
    }

    #[test]
    fn an_old_complete_installation_requires_an_update() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Data")).unwrap();
        for name in RACE_MPQS { fs::write(temp.path().join("Data").join(name), b"old patch").unwrap(); }
        fs::write(temp.path().join("dinput8.dll"), b"old runtime").unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.4\n").unwrap();
        assert_eq!(client_patch_status(temp.path()).state, ClientPatchState::PartiallyInstalled);
    }

    #[test]
    fn an_unpatched_client_is_left_untouched() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Data")).unwrap();
        fs::write(temp.path().join("Data/patch-T.MPQ"), b"original").unwrap();
        set_client_patch_enabled(temp.path(), false).unwrap();
        assert_eq!(fs::read(temp.path().join("Data/patch-T.MPQ")).unwrap(), b"original");
    }

    #[test]
    fn corrupt_downloads_are_rejected() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), b"corrupt archive").unwrap();
        assert!(verify_archive(temp.path(), ARCHIVES[0].0).is_err());
    }

    #[test]
    fn test_patch_status_and_toggle() {
        let temp = tempfile::tempdir().unwrap();
        let client_path = temp.path();
        let data_dir = client_path.join("Data");
        fs::create_dir_all(&data_dir).unwrap();

        // 1. Initial state: Not installed
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::NotInstalled);

        fs::write(client_path.join(MANIFEST_FILENAME), format!("# CoA Custom {CLIENT_VERSION}\n")).unwrap();
        // 2. Create MPQs and root files
        for mpq in RACE_MPQS {
            fs::write(data_dir.join(mpq), b"test mpq content").unwrap();
        }
        fs::write(client_path.join("dinput8.dll"), b"test dll").unwrap();
        fs::write(client_path.join("EsteriaAppearance.dll"), b"test dll").unwrap();

        let names: Vec<String> = RACE_MPQS.iter().map(|name| format!("Data/{name}")).chain(["dinput8.dll".into(), "EsteriaAppearance.dll".into()]).collect();
        record_files(client_path, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Enabled);
        assert_eq!(st.installed_mpqs, RACE_MPQS.len());

        // 3. Disable patches
        set_client_patch_enabled(client_path, false).unwrap();
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Disabled);
        assert!(data_dir.join("patch-T.MPQ.disabled").is_file());
        assert!(!data_dir.join("patch-T.MPQ").is_file());
        assert!(client_path.join("dinput8.dll.disabled").is_file());
        assert!(!client_path.join("dinput8.dll").is_file());

        // 4. Re-enable patches
        set_client_patch_enabled(client_path, true).unwrap();
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Enabled);
        assert!(data_dir.join("patch-T.MPQ").is_file());
        assert!(!data_dir.join("patch-T.MPQ.disabled").is_file());
        assert!(client_path.join("dinput8.dll").is_file());
        assert!(!client_path.join("dinput8.dll.disabled").is_file());
    }
    #[test]
    fn disabling_bot_custom_races_excludes_every_nonstandard_race() {
        let temp = tempfile::tempdir().unwrap();
        let conf = temp.path().join("Core/configs/modules/playerbots.conf");
        fs::create_dir_all(conf.parent().unwrap()).unwrap();
        fs::write(&conf, "AiPlayerbot.ExcludedBotRaces = \"\"\n").unwrap();
        sync_bot_races_config(temp.path(), false).unwrap();
        assert_eq!(is_bot_custom_races_allowed(temp.path()), Some(false));
        let parsed = crate::config::parser::ConfFile::parse_bytes(&fs::read(conf).unwrap()).unwrap();
        let value = parsed.get("AiPlayerbot.ExcludedBotRaces").unwrap().trim_matches('"');
        let excluded: Vec<u32> = value.split(',').map(|race| race.parse().unwrap()).collect();
        for race in 1..=128 {
            assert_eq!(excluded.contains(&race), ![1, 2, 3, 4, 5, 6, 7, 8, 10, 11].contains(&race), "race {race}");
        }
    }


}
