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

pub const MANIFEST_FILENAME: &str = "custom_races_patch_manifest.txt";
pub const ARCHIVES: &[(&str, &str)] = &[
    (
        "CoA-Custom-1.4-main-20261007-b46a130e.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.4/CoA-Custom-1.4-main-20261007-b46a130e.zip",
    ),
    (
        "CoA-Custom-1.4-client-part1.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.4/CoA-Custom-1.4-client-part1.zip",
    ),
    (
        "CoA-Custom-1.4-client-part2.zip",
        "https://github.com/ilusixn/azerothcore-wotlk-coa/releases/download/coa-custom-1.4/CoA-Custom-1.4-client-part2.zip",
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
];

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
    pub installed_mpqs: usize,
    pub total_mpqs: usize,
    pub has_root_files: bool,
    pub client_path: Option<String>,
}

/// Inspect the client folder and determine the state of custom races patches.
pub fn client_patch_status(client_path: &Path) -> ClientPatchStatus {
    let data_dir = client_path.join("Data");
    let mut enabled_count = 0;
    let mut disabled_count = 0;

    for mpq in RACE_MPQS {
        if data_dir.join(format!("{mpq}.disabled")).is_file() {
            disabled_count += 1;
        } else if data_dir.join(mpq).is_file() {
            enabled_count += 1;
        }
    }

    let has_root = client_path.join("EsteriaAppearance.dll").is_file()
        || client_path.join("dinput8.dll").is_file()
        || client_path.join("dinput8.dll.disabled").is_file();

    let total = RACE_MPQS.len();
    let state = if enabled_count == total && client_path.join("dinput8.dll").is_file() {
        ClientPatchState::Enabled
    } else if disabled_count == total && client_path.join("dinput8.dll.disabled").is_file() {
        ClientPatchState::Disabled
    } else if (enabled_count + disabled_count) > 0 {
        ClientPatchState::PartiallyInstalled
    } else {
        ClientPatchState::NotInstalled
    };

    ClientPatchStatus {
        state,
        installed_mpqs: enabled_count + disabled_count,
        total_mpqs: total,
        has_root_files: has_root,
        client_path: Some(client_path.to_string_lossy().into_owned()),
    }
}

/// Enable or disable client patches by renaming MPQs and `dinput8.dll` with `.disabled`.
pub fn set_client_patch_enabled(client_path: &Path, enabled: bool) -> Result<()> {
    if !client_path.is_dir() {
        return Err(Error::Invalid("Game client directory not found.".into()));
    }

    if crate::client::is_running(client_path) {
        return Err(Error::Invalid("Close the game before changing custom races patches.".into()));
    }
    if !client_path.join(MANIFEST_FILENAME).is_file()
        && !RACE_MPQS.iter().skip(1).any(|name| client_path.join("Data").join(name).is_file()
            || client_path.join("Data").join(format!("{name}.disabled")).is_file()) {
        return Ok(());
    }
    let mut paths: Vec<PathBuf> = RACE_MPQS.iter().map(|name| client_path.join("Data").join(name)).collect();
    paths.push(client_path.join("dinput8.dll"));
    paths.push(client_path.join("Ascension.exe"));
    for active in paths {
        let disabled = active.with_extension(format!("{}.disabled", active.extension().unwrap_or_default().to_string_lossy()));
        let original = active.with_extension(format!("{}.original", active.extension().unwrap_or_default().to_string_lossy()));
        if enabled && disabled.is_file() {
            if active.is_file() {
                if !original.is_file() {
                    return Err(Error::Invalid(format!("Cannot replace existing {} without an original backup.", active.display())));
                }
                fs::remove_file(&active)?;
            }
            fs::rename(&disabled, &active)?;
        } else if !enabled && active.is_file() && !disabled.is_file() {
            if active.file_name().is_some_and(|name| name == "Ascension.exe") && !original.is_file() {
                continue;
            }
            fs::rename(&active, &disabled)?;
            if original.is_file() {
                fs::copy(&original, &active)?;
            }
        }
    }

    tracing::info!(enabled, "custom races client patches toggled");
    Ok(())
}

fn verify_archive(path: &Path, filename: &str) -> Result<()> {
    let expected = match filename {
        "CoA-Custom-1.4-main-20261007-b46a130e.zip" => "eaf934b5283476aa7b5d6eca7200ceae016e866ddbdab726bd47f39739c9c802",
        "CoA-Custom-1.4-client-part1.zip" => "669f8396e82f396125e0be2070b2bad782f0104c695939a88924ec97f4fc4d21",
        "CoA-Custom-1.4-client-part2.zip" => "088b3ddcf292abccbc84453c0ccf4cd74ff6b20c848d5902dc83854553aecc8b",
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
    let data_dir = client_path.join("Data");
    fs::create_dir_all(&data_dir)?;

    // 1. Back up original Ascension.exe and patch-T.MPQ if they exist and haven't been backed up yet
    let exe = client_path.join("Ascension.exe");
    let exe_bak = client_path.join("Ascension.exe.original");
    if exe.is_file() && !exe_bak.is_file() {
        report("Backing up original Ascension.exe...", 0.02);
        fs::copy(&exe, &exe_bak)?;
    }

    let t_mpq = data_dir.join("patch-T.MPQ");
    let t_mpq_bak = data_dir.join("patch-T.MPQ.original");
    if t_mpq.is_file() && !t_mpq_bak.is_file() {
        report("Backing up original patch-T.MPQ...", 0.04);
        fs::copy(&t_mpq, &t_mpq_bak)?;
    }

    let dll = client_path.join("dinput8.dll");
    let dll_backup = client_path.join("dinput8.dll.original");
    if dll.is_file() && !dll_backup.is_file() {
        fs::copy(&dll, &dll_backup)?;
    }

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
                    installed_mpqs.push(base);
                } else if base.eq_ignore_ascii_case("dinput8.dll") {
                    let dest = client_path.join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    installed_roots.push(base);
                }
            } else if zname_norm.contains("files/client_root/") {
                let base = Path::new(&zname_norm)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if !base.is_empty() {
                    let dest = client_path.join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    installed_roots.push(base);
                }
            }
        }
    }

    // 4. Record manifest
    report("Finalizing manifest...", 0.98);
    let manifest_path = client_path.join(MANIFEST_FILENAME);
    installed_mpqs.sort();
    installed_mpqs.dedup();
    installed_roots.sort();
    installed_roots.dedup();

    let mut manifest_body = String::from("# Custom Race MPQ files\n");
    for m in &installed_mpqs {
        manifest_body.push_str(&format!("MPQ:{m}\n"));
    }
    manifest_body.push_str("# Custom Race root files\n");
    for r in &installed_roots {
        manifest_body.push_str(&format!("ROOT:{r}\n"));
    }

    fsx::atomic_write(&manifest_path, manifest_body.as_bytes())?;
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

    #[test]
    fn disabling_restores_original_client_files_and_enabling_restores_custom_files() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        for path in [data.join("patch-T.MPQ"), temp.path().join("Ascension.exe"), temp.path().join("dinput8.dll")] {
            fs::write(&path, b"custom").unwrap();
            let backup = path.with_extension(format!("{}.original", path.extension().unwrap().to_string_lossy()));
            fs::write(&backup, b"original").unwrap();
        }
        fs::write(temp.path().join(MANIFEST_FILENAME), "MPQ:patch-T.MPQ\n").unwrap();
        set_client_patch_enabled(temp.path(), false).unwrap();
        assert_eq!(fs::read(data.join("patch-T.MPQ")).unwrap(), b"original");
        assert_eq!(fs::read(temp.path().join("Ascension.exe")).unwrap(), b"original");
        set_client_patch_enabled(temp.path(), true).unwrap();
        assert_eq!(fs::read(data.join("patch-T.MPQ")).unwrap(), b"custom");
        assert_eq!(fs::read(temp.path().join("Ascension.exe")).unwrap(), b"custom");
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

        // 2. Create MPQs and root files
        for mpq in RACE_MPQS {
            fs::write(data_dir.join(mpq), b"test mpq content").unwrap();
        }
        fs::write(client_path.join("dinput8.dll"), b"test dll").unwrap();
        fs::write(client_path.join("EsteriaAppearance.dll"), b"test dll").unwrap();

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
}
