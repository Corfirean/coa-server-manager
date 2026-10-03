//! The game client is a separate object: detected read-only, with targeted updates to
//! `realmlist.wtf` (backed up first) and the `Interface/AddOns/CoABotUI` folder (backed up when it differs).
//! The selected realm in WTF/Config.wtf is synchronized while the client is closed.
//! Cache, other addons and the executable are never modified.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::fsx;

pub const ADDON_NAME: &str = "CoABotUI";
const EXECUTABLES: [&str; 3] = ["Ascension.exe", "Wow.exe", "WoW.exe"];

pub fn is_running(client: &Path) -> bool {
    EXECUTABLES.iter().any(|exe| {
        let path = client.join(exe);
        let path = dunce::canonicalize(&path).unwrap_or(path);
        !crate::process::find_by_exe(&path).is_empty()
    })
}

fn realm_config(old: &[u8], realm: &str) -> Vec<u8> {
    let eol: &[u8] = if old.windows(2).any(|w| w == b"\r\n") { b"\r\n" } else { b"\n" };
    let mut out = Vec::new();
    let mut replaced = false;
    for line in old.split_inclusive(|b| *b == b'\n') {
        let body = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
        let mut words = body.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
        let matches = words.next().is_some_and(|w| w.eq_ignore_ascii_case(b"SET"))
            && words.next().is_some_and(|w| w.eq_ignore_ascii_case(b"realmName"));
        if matches {
            if line.starts_with(b"\xef\xbb\xbf") { out.extend_from_slice(b"\xef\xbb\xbf"); }
            out.extend_from_slice(format!("SET realmName \"{realm}\"").as_bytes());
            if line.ends_with(b"\r\n") { out.extend_from_slice(b"\r\n"); }
            else if line.ends_with(b"\n") { out.push(b'\n'); }
            replaced = true;
        } else {
            out.extend_from_slice(line);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with(b"\n") { out.extend_from_slice(eol); }
        out.extend_from_slice(format!("SET realmName \"{realm}\"").as_bytes());
        out.extend_from_slice(eol);
    }
    out
}

pub fn sync_realm(client: &Path, meta: &Path, mode: crate::realms::Mode) -> Result<bool> {
    sync_realm_when_closed(client, meta, mode, is_running(client))
}

fn sync_realm_when_closed(client: &Path, meta: &Path, mode: crate::realms::Mode, running: bool) -> Result<bool> {
    if running { return Ok(false); }
    let file = client.join("WTF/Config.wtf");
    let old = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let realm = match mode {
        crate::realms::Mode::Coa => "Conquest of Azeroth",
        crate::realms::Mode::Wildcard => "Wildcard",
    };
    let new = realm_config(&old, realm);
    if new == old { return Ok(true); }
    if file.exists() {
        let saved = backup_dir(meta).join(format!("config-realm-{}.wtf", uuid::Uuid::new_v4()));
        fsx::atomic_write(&saved, &old)?;
    }
    fsx::atomic_write(&file, &new)?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize)]
pub struct Realmlist {
    pub path: String,
    pub host: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddonState {
    pub installed: bool,
    pub version: Option<String>,
    /// Some(true) when the installed files equal the package's; None when there is no package to compare with.
    pub up_to_date: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub path: String,
    pub executable: String,
    pub realmlists: Vec<Realmlist>,
    pub addon: AddonState,
    pub other_addons: usize,
}

fn find_exe(dir: &Path) -> Option<&'static str> {
    EXECUTABLES.iter().copied().find(|e| dir.join(e).is_file())
}

/// `Data/<locale>/realmlist.wtf` files (a client may have several locales).
pub fn realmlist_files(client: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(client.join("Data"))
        .map(|rd| rd.flatten().map(|e| e.path().join("realmlist.wtf")).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn parse_host(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.get(..3).filter(|p| p.eq_ignore_ascii_case("set")).and_then(|_| l[3..].trim_start().get(..9).filter(|w| w.eq_ignore_ascii_case("realmlist")).map(|_| l[3..].trim_start()[9..].trim()))?;
        Some(rest.trim_matches('"').to_string())
    })
}

fn toc_version(toc: &Path) -> Option<String> {
    fs::read_to_string(toc).ok()?.lines().find_map(|l| l.strip_prefix("## Version:").map(|v| v.trim().to_string()))
}

fn tree_hashes(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let (Ok(rel), Ok(h)) = (p.strip_prefix(root), fsx::sha256_file(&p)) {
                out.insert(rel.to_string_lossy().replace('\\', "/"), h);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Where a server package ships the addon, relative to the server folder.
pub fn addon_source(server_root: &Path) -> Option<PathBuf> {
    let p = server_root.join("Extras").join(ADDON_NAME);
    p.join(format!("{ADDON_NAME}.toc")).is_file().then_some(p)
}

pub fn addon_state(client: &Path, source: Option<&Path>) -> AddonState {
    let dir = client.join("Interface/AddOns").join(ADDON_NAME);
    let toc = dir.join(format!("{ADDON_NAME}.toc"));
    let installed = toc.is_file();
    AddonState {
        installed,
        version: installed.then(|| toc_version(&toc)).flatten(),
        up_to_date: source.filter(|_| installed).map(|s| {
            let (want, have) = (tree_hashes(s), tree_hashes(&dir));
            want.iter().all(|(k, v)| have.get(k) == Some(v))
        }),
    }
}

/// Read-only look at a folder; None if it is not a game client.
pub fn detect(path: &Path, addon_package: Option<&Path>) -> Option<ClientInfo> {
    if !path.join("Data").is_dir() {
        return None;
    }
    let exe = find_exe(path)?;
    let realmlists = realmlist_files(path)
        .into_iter()
        .map(|p| Realmlist { host: fs::read_to_string(&p).ok().and_then(|t| parse_host(&t)), path: p.to_string_lossy().into_owned() })
        .collect();
    let other = fs::read_dir(path.join("Interface/AddOns")).map(|rd| rd.flatten().filter(|e| e.path().is_dir() && e.file_name() != ADDON_NAME).count()).unwrap_or(0);
    Some(ClientInfo { path: path.to_string_lossy().into_owned(), executable: exe.into(), realmlists, addon: addon_state(path, addon_package), other_addons: other })
}

fn backup_dir(meta: &Path) -> PathBuf {
    meta.join("backups").join("client")
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) && !h.starts_with('-')
}

/// Locales a client has game data for (`Data/<locale>/locale-<locale>.MPQ`), e.g. `enUS`.
fn installed_locales(client: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(client.join("Data"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.len() == 4 && n[..2].bytes().all(|b| b.is_ascii_lowercase()) && n[2..].bytes().all(|b| b.is_ascii_uppercase()))
                .filter(|n| client.join("Data").join(n).join(format!("locale-{n}.MPQ")).is_file())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Point every locale's realmlist at `host`. Each changed file is copied to the backup folder first.
/// A freshly downloaded client has no realmlist yet; one is created for each installed locale.
pub fn set_realmlist(client: &Path, meta: &Path, host: &str) -> Result<Vec<String>> {
    if !host_ok(host) {
        return Err(Error::Invalid("That address is not valid.".into()));
    }
    let mut changed = Vec::new();
    if realmlist_files(client).is_empty() {
        for loc in installed_locales(client) {
            let file = client.join("Data").join(&loc).join("realmlist.wtf");
            fsx::atomic_write(&file, format!("set realmlist {host}
").as_bytes())?;
            changed.push(file.to_string_lossy().into_owned());
        }
        if !changed.is_empty() {
            return Ok(changed);
        }
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    for file in realmlist_files(client) {
        let old = fs::read(&file)?;
        let text = String::from_utf8(old.clone()).map_err(|_| Error::Invalid("realmlist.wtf is not text".into()))?;
        if parse_host(&text).as_deref() == Some(host) {
            continue;
        }
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut replaced = false;
        let mut lines: Vec<String> = text
            .lines()
            .map(|l| {
                if !replaced && parse_host(l).is_some() {
                    replaced = true;
                    format!("set realmlist {host}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        if !replaced {
            lines.push(format!("set realmlist {host}"));
        }
        let new = lines.join(eol) + eol;
        let locale = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let saved = backup_dir(meta).join(format!("realmlist-{locale}-{stamp}.wtf"));
        fsx::atomic_write(&saved, &old)?;
        fsx::atomic_write(&file, new.as_bytes())?;
        changed.push(file.to_string_lossy().into_owned());
    }
    Ok(changed)
}

/// Replace the text of every locale's realmlist with `content` (a named realmlist chosen by the player). Each changed file
/// is saved to the backup folder first; files that already hold the same lines are left alone. A client without one gets a
/// file for each installed locale. Returns the files that changed.
pub fn write_realmlist(client: &Path, meta: &Path, content: &str) -> Result<Vec<String>> {
    let compact = |t: &str| t.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    let mut files = realmlist_files(client);
    if files.is_empty() {
        files = installed_locales(client).into_iter().map(|loc| client.join("Data").join(loc).join("realmlist.wtf")).collect();
    }
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut changed = Vec::new();
    for file in files {
        let old = fs::read(&file).ok();
        if let Some(old) = &old {
            if compact(&String::from_utf8_lossy(old)) == compact(content) {
                continue;
            }
            let locale = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            fsx::atomic_write(&backup_dir(meta).join(format!("realmlist-{locale}-{stamp}.wtf")), old)?;
        }
        fsx::atomic_write(&file, content.as_bytes())?;
        changed.push(file.to_string_lossy().into_owned());
    }
    Ok(changed)
}

/// Copy the addon into the client. If a different version is there, that one folder is backed up first;
/// files in it that the package does not contain are left alone.
pub fn install_addon(client: &Path, meta: &Path, source: &Path) -> Result<()> {
    if !source.join(format!("{ADDON_NAME}.toc")).is_file() {
        return Err(Error::Invalid("The addon package is incomplete.".into()));
    }
    let dest = fsx::ensure_within(client, &client.join("Interface/AddOns").join(ADDON_NAME))?;
    let want = tree_hashes(source);
    if dest.is_dir() {
        let have = tree_hashes(&dest);
        if want.iter().all(|(k, v)| have.get(k) == Some(v)) {
            return Ok(());
        }
        let keep = backup_dir(meta).join(format!("{ADDON_NAME}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
        for (rel, _) in &have {
            let to = fsx::safe_join(&keep, rel)?;
            fs::create_dir_all(to.parent().unwrap())?;
            fs::copy(fsx::safe_join(&dest, rel)?, to)?;
        }
    }
    for rel in want.keys() {
        let target = fsx::safe_join(&dest, rel)?;
        fsx::atomic_write(&target, &fs::read(fsx::safe_join(source, rel)?)?)?;
    }
    Ok(())
}

/// Start the game client. The caller is responsible for making sure the server is ready first.
pub fn launch(client: &Path) -> Result<u32> {
    let exe = find_exe(client).ok_or_else(|| Error::Invalid("The game client was not found.".into()))?;
    let child = Command::new(client.join(exe)).current_dir(client).spawn()?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_selection_preserves_other_settings_bytes_and_line_endings() {
        let old = b"\xef\xbb\xbfSET realmName \"Wildcard\"\r\nSET accountName \"\xff\"\r\nset REALMNAME \"old\"\r\nSET realmList \"127.0.0.1\"";
        let expected = b"\xef\xbb\xbfSET realmName \"Conquest of Azeroth\"\r\nSET accountName \"\xff\"\r\nSET realmName \"Conquest of Azeroth\"\r\nSET realmList \"127.0.0.1\"";
        assert_eq!(realm_config(old, "Conquest of Azeroth"), expected);
        assert_eq!(realm_config(b"SET sound 1", "Wildcard"), b"SET sound 1\nSET realmName \"Wildcard\"\n");
    }

    #[test]
    fn realm_sync_backs_up_config_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let meta = d.path().join("meta");
        let original = b"SET sound 1\r\nSET realmName \"Wildcard\"\r\n";
        fs::write(c.join("WTF/Config.wtf"), original).unwrap();
        assert!(sync_realm(&c, &meta, crate::realms::Mode::Coa).unwrap());
        let backups: Vec<_> = fs::read_dir(backup_dir(&meta)).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path()).unwrap(), original);
        assert!(sync_realm(&c, &meta, crate::realms::Mode::Coa).unwrap());
        assert_eq!(fs::read_dir(backup_dir(&meta)).unwrap().count(), 1);
        assert!(sync_realm(&c, &meta, crate::realms::Mode::Wildcard).unwrap());
        assert_eq!(fs::read(c.join("WTF/Config.wtf")).unwrap(), original);
    }

    #[test]
    fn running_client_defers_realm_update_until_closed() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let meta = d.path().join("meta");
        let original = b"SET realmName \"Wildcard\"\n";
        fs::write(c.join("WTF/Config.wtf"), original).unwrap();
        assert!(!sync_realm_when_closed(&c, &meta, crate::realms::Mode::Coa, true).unwrap());
        assert_eq!(fs::read(c.join("WTF/Config.wtf")).unwrap(), original);
        assert!(!backup_dir(&meta).exists());
        assert!(sync_realm_when_closed(&c, &meta, crate::realms::Mode::Coa, false).unwrap());
        assert_eq!(fs::read(c.join("WTF/Config.wtf")).unwrap(), b"SET realmName \"Conquest of Azeroth\"\n");
    }

    fn fake_client(d: &Path) -> PathBuf {
        let c = d.join("wow");
        fs::create_dir_all(c.join("Data/enUS")).unwrap();
        fs::create_dir_all(c.join("Data/ruRU")).unwrap();
        fs::create_dir_all(c.join("Interface/AddOns/ElvUI")).unwrap();
        fs::create_dir_all(c.join("WTF")).unwrap();
        fs::write(c.join("Ascension.exe"), b"exe").unwrap();
        fs::write(c.join("Data/enUS/realmlist.wtf"), "set realmlist old.example.com\r\nset patchlist x\r\n").unwrap();
        fs::write(c.join("Data/ruRU/realmlist.wtf"), "set realmlist 127.0.0.1\n").unwrap();
        fs::write(c.join("WTF/Config.wtf"), "SET x 1").unwrap();
        fs::write(c.join("Interface/AddOns/ElvUI/ElvUI.toc"), "## Title: Elv").unwrap();
        c
    }

    fn package(d: &Path, version: &str) -> PathBuf {
        let p = d.join("pkg").join(ADDON_NAME);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("CoABotUI.toc"), format!("## Version: {version}\nCoABotUI.lua\n")).unwrap();
        fs::write(p.join("CoABotUI.lua"), format!("-- v{version}")).unwrap();
        p
    }

    fn snapshot(root: &Path) -> BTreeMap<String, String> {
        tree_hashes(root)
    }

    #[test]
    fn detects_only_real_clients_and_reads_realmlists() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let info = detect(&c, None).unwrap();
        assert_eq!(info.executable, "Ascension.exe");
        assert_eq!(info.realmlists.len(), 2);
        assert_eq!(info.realmlists[0].host.as_deref(), Some("old.example.com"));
        assert_eq!(info.other_addons, 1);
        assert!(!info.addon.installed);
        assert!(detect(d.path(), None).is_none(), "a plain folder is not a client");
        fs::create_dir_all(d.path().join("srv/Data")).unwrap();
        assert!(detect(&d.path().join("srv"), None).is_none(), "Data without an executable is not a client");
    }

    #[test]
    fn realmlist_change_is_backed_up_minimal_and_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let meta = d.path().join("meta");
        let before = snapshot(&c);
        let changed = set_realmlist(&c, &meta, "192.168.0.5").unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(fs::read_to_string(c.join("Data/enUS/realmlist.wtf")).unwrap(), "set realmlist 192.168.0.5\r\nset patchlist x\r\n", "other lines and line endings preserved");
        assert_eq!(fs::read_to_string(c.join("Data/ruRU/realmlist.wtf")).unwrap(), "set realmlist 192.168.0.5\n");
        let saved: Vec<_> = fs::read_dir(meta.join("backups/client")).unwrap().flatten().collect();
        assert_eq!(saved.len(), 2, "each changed file was backed up");
        assert_eq!(fs::read_to_string(saved.iter().find(|e| e.file_name().to_string_lossy().contains("enUS")).unwrap().path()).unwrap(), "set realmlist old.example.com\r\nset patchlist x\r\n");
        // nothing else in the client changed
        let after = snapshot(&c);
        let diff: Vec<_> = after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        assert_eq!(diff, ["Data/ruRU/realmlist.wtf", "Data/enUS/realmlist.wtf"].iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());
        assert!(set_realmlist(&c, &meta, "192.168.0.5").unwrap().is_empty(), "already set: no write, no new backup");
        assert!(set_realmlist(&c, &meta, "bad host; rm -rf").is_err());
    }

    #[test]
    fn a_fresh_client_gets_a_realmlist_for_each_installed_locale() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("fresh");
        fs::create_dir_all(c.join("Data/enUS")).unwrap();
        fs::create_dir_all(c.join("Data/Content")).unwrap();
        fs::write(c.join("Data/enUS/locale-enUS.MPQ"), b"x").unwrap();
        fs::write(c.join("Ascension.exe"), b"exe").unwrap();
        let changed = set_realmlist(&c, &d.path().join("meta"), "127.0.0.1").unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(fs::read_to_string(c.join("Data/enUS/realmlist.wtf")).unwrap(), "set realmlist 127.0.0.1
");
        assert!(!c.join("Data/Content/realmlist.wtf").exists());
        assert_eq!(detect(&c, None).unwrap().realmlists[0].host.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn addon_install_touches_only_its_own_folder_and_backs_up_a_different_version() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let meta = d.path().join("meta");
        let v1 = package(d.path(), "1");
        install_addon(&c, &meta, &v1).unwrap();
        assert_eq!(addon_state(&c, Some(&v1)).up_to_date, Some(true));
        assert!(!meta.join("backups/client").exists() || fs::read_dir(meta.join("backups/client")).unwrap().next().is_none(), "first install has nothing to back up");

        let before = snapshot(&c);
        install_addon(&c, &meta, &v1).unwrap();
        assert_eq!(snapshot(&c), before, "same version: no change at all");

        // user tweaks their copy, then a newer package arrives
        fs::write(c.join("Interface/AddOns/CoABotUI/notes.txt"), "mine").unwrap();
        let v2 = package(d.path(), "2");
        let outside_before: BTreeMap<_, _> = snapshot(&c).into_iter().filter(|(k, _)| !k.starts_with("Interface/AddOns/CoABotUI")).collect();
        install_addon(&c, &meta, &v2).unwrap();
        assert_eq!(addon_state(&c, Some(&v2)).version.as_deref(), Some("2"));
        assert_eq!(fs::read_to_string(c.join("Interface/AddOns/CoABotUI/notes.txt")).unwrap(), "mine", "the user's extra file survives");
        let outside_after: BTreeMap<_, _> = snapshot(&c).into_iter().filter(|(k, _)| !k.starts_with("Interface/AddOns/CoABotUI")).collect();
        assert_eq!(outside_before, outside_after, "other addons, WTF, Data untouched");
        let kept = fs::read_dir(meta.join("backups/client")).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with(ADDON_NAME)).unwrap().path();
        assert_eq!(fs::read_to_string(kept.join("CoABotUI.lua")).unwrap(), "-- v1", "the previous version and the user's copy are backed up");
        assert!(kept.join("notes.txt").is_file());
    }

    #[test]
    fn incomplete_addon_package_and_missing_client_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_client(d.path());
        let empty = d.path().join("emptypkg");
        fs::create_dir_all(&empty).unwrap();
        assert!(install_addon(&c, &d.path().join("meta"), &empty).is_err());
        assert!(launch(d.path()).is_err());
    }
}
