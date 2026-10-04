//! "Play with Friends": which address friends use, opening the login/world servers to the network (and only those),
//! keeping the realm's advertised address right after every start, and the small package a friend needs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::parser::ConfFile;
use crate::config::{take_snapshot, Scope};
use crate::db::{Account, Db};
use crate::error::{Error, Result};
use crate::fsx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Only this computer.
    Local,
    /// Friends on the same home network.
    Lan,
    /// Friends over the internet through this connection (needs open ports).
    Direct,
    /// Friends join a private network (Tailscale); no router setup.
    Private,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub mode: Mode,
    /// The address friends type into their client for this mode.
    pub host: Option<String>,
    /// Independent of the effective host of the current mode. None means automatic LAN detection.
    #[serde(default)]
    pub lan_address_override: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: Mode::Local, host: None, lan_address_override: None }
    }
}

impl Settings {
    /// Only LAN application changes the LAN preference. Other modes keep it for the next LAN session.
    pub fn select_mode(&mut self, mode: Mode, host: String, lan_override: Option<String>) {
        if mode == Mode::Lan {
            self.lan_address_override = lan_override;
        }
        self.mode = mode;
        self.host = Some(host);
    }
}

fn file(meta: &Path) -> PathBuf {
    meta.join("friends.json")
}

pub fn load(meta: &Path) -> Settings {
    fsx::read_json(&file(meta)).unwrap_or_default()
}

pub fn save(meta: &Path, s: &Settings) -> Result<()> {
    fsx::atomic_write_json(&file(meta), s)
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')) && !h.starts_with('-')
}

/// The files whose `BindIP` decides who can reach the login and world servers.
fn bind_targets(root: &Path) -> Vec<PathBuf> {
    ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf", "Settings/authserver.conf.template", "Core/configs/authserver.conf"]
        .iter()
        .map(|r| root.join(r))
        .filter(|p| p.is_file())
        .collect()
}

/// Whether the configuration currently lets other computers reach the login and world servers.
pub fn bind_is_open(root: &Path) -> bool {
    if crate::docker::is_docker(root) {
        return crate::docker::bind_is_open(root);
    }
    let files = bind_targets(root);
    !files.is_empty()
        && files
            .iter()
            .all(|p| fs::read(p).ok().and_then(|b| ConfFile::parse_bytes(&b).ok()).and_then(|c| c.get("BindIP").map(|v| v.trim() == "\"0.0.0.0\"")).unwrap_or(false))
}

/// Open (`0.0.0.0`) or close (`127.0.0.1`) the login and world servers to other computers. The database and the server
/// console are never touched. Returns true if anything changed (a restart is then needed). The old files are snapshotted.
pub fn set_open(root: &Path, meta: &Path, open: bool) -> Result<bool> {
    if crate::docker::is_docker(root) {
        return crate::docker::set_open(root, meta, open);
    }
    let want = if open { "\"0.0.0.0\"" } else { "\"127.0.0.1\"" };
    let mut originals = Vec::new();
    let mut edits = Vec::new();
    for p in bind_targets(root) {
        let bytes = fs::read(&p)?;
        let mut conf = ConfFile::parse_bytes(&bytes)?;
        if conf.get("BindIP").map(str::trim) == Some(want) {
            continue;
        }
        conf.set("BindIP", want, &["Added by CoA Server Manager"]);
        originals.push((p.clone(), bytes));
        edits.push((p, conf.to_text()));
    }
    // Friends connect from other addresses, and the server only applies the CoA client protocol (extension packets,
    // header mode) to loopback unless remote clients are allowed - so sharing must switch that on as well.
    let coa_conf = root.join("Core/configs/modules/coa.conf");
    if let Ok(bytes) = fs::read(&coa_conf) {
        let mut conf = ConfFile::parse_bytes(&bytes)?;
        // Opening turns it on; closing leaves it alone (it only matters while the servers listen beyond this computer).
        if open && conf.get("CoA.AllowRemoteClients").map(str::trim) != Some("1") {
            conf.set("CoA.AllowRemoteClients", "1", &["Set by CoA Server Manager together with the friends mode"]);
            originals.push((coa_conf.clone(), bytes));
            edits.push((coa_conf, conf.to_text()));
        }
    }
    if edits.is_empty() {
        return Ok(false);
    }
    take_snapshot(meta, Scope::Server, if open { "before opening the servers to friends" } else { "before closing the servers to friends" }, &originals)?;
    for (p, text) in edits {
        fsx::atomic_write(&p, text.as_bytes())?;
    }
    Ok(true)
}

/// Make the realm list advertise `host` (the launcher resets it to 127.0.0.1 at every start, so this runs after each start).
pub fn apply_realm_address(root: &Path, host: &str) -> Result<()> {
    if !host_ok(host) {
        return Err(Error::Invalid("That address is not valid.".into()));
    }
    let db = Db::from_repack(root, Account::Admin)?;
    let realm = crate::realms::state(root)?.active.realm_id();
    let where_realms = if crate::realms::state(root)?.simultaneous { "id IN (1,2)".into() } else { format!("id={realm}") };
    db.query(&format!("UPDATE acore_auth.realmlist SET address='{host}', localAddress='{host}' WHERE {where_realms};"))?;
    Ok(())
}

/// Before every server start: when the owner shares the server with friends, make sure the login and world servers
/// listen on all addresses. The launcher's own tools (for example switching detailed logging off) restore the config
/// templates from older copies and would silently put `BindIP` back to 127.0.0.1 while the realm list still advertises
/// the shared address - the game then shows the realm and drops back to the login screen without any message.
pub fn ensure_bind(root: &Path, meta: &Path) -> Result<bool> {
    match load(meta).mode {
        Mode::Local => Ok(false),
        _ => set_open(root, meta, true),
    }
}

/// After every server start: put the chosen address back into the realm list (the launcher resets it to 127.0.0.1).
pub fn reapply(root: &Path, meta: &Path) -> Result<()> {
    let s = load(meta);
    match (s.mode, s.host) {
        (Mode::Local, _) | (_, None) => Ok(()),
        (_, Some(host)) => apply_realm_address(root, &host),
    }
}

/// Build the friend's package: instructions, a ready realmlist file and (optionally) the companion addon.
/// Only whitelisted content goes in - never credentials, configuration or database files.
pub fn make_friend_package(root: &Path, host: &str, include_addon: bool, out_zip: &Path) -> Result<()> {
    if !host_ok(host) {
        return Err(Error::Invalid("That address is not valid.".into()));
    }
    let file = fs::File::create(out_zip)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, opts).map_err(|e| Error::Invalid(e.to_string()))?;
        zip.write_all(bytes)?;
        Ok(())
    };
    add("HOW-TO-CONNECT.txt", crate::net::instructions(host).replace('\n', "\r\n").as_bytes())?;
    add("realmlist.wtf", format!("set realmlist {host}\r\n").as_bytes())?;
    if include_addon {
        if let Some(src) = crate::client::addon_source(root) {
            let mut stack = vec![src.clone()];
            while let Some(dir) = stack.pop() {
                for e in fs::read_dir(&dir)? {
                    let p = e?.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let rel = p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                        add(&format!("Interface/AddOns/CoABotUI/{rel}"), &fs::read(&p)?)?;
                    }
                }
            }
        }
    }
    zip.finish().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        let meta = d.path().join("srv.manager");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::create_dir_all(root.join("Core/configs")).unwrap();
        fs::create_dir_all(&meta).unwrap();
        for (f, body) in [
            ("Settings/worldserver.conf.template", "[worldserver]\r\nBindIP = \"127.0.0.1\"\r\nRa.IP = \"127.0.0.1\"\r\n"),
            ("Core/configs/worldserver.conf", "[worldserver]\r\nBindIP = \"127.0.0.1\"\r\nRa.IP = \"127.0.0.1\"\r\n"),
            ("Settings/authserver.conf.template", "[authserver]\nBindIP = \"127.0.0.1\"\n"),
            ("Core/configs/authserver.conf", "[authserver]\nBindIP = \"127.0.0.1\"\n"),
        ] {
            fs::write(root.join(f), body).unwrap();
        }
        (d, root, meta)
    }

    #[test]
    fn opening_changes_only_bind_ip_of_login_and_world_and_never_the_console() {
        let (_d, root, meta) = setup();
        assert!(set_open(&root, &meta, true).unwrap());
        let w = fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap();
        assert_eq!(w, "[worldserver]\r\nBindIP = \"0.0.0.0\"\r\nRa.IP = \"127.0.0.1\"\r\n", "console address untouched, line endings kept");
        assert_eq!(fs::read_to_string(root.join("Settings/authserver.conf.template")).unwrap(), "[authserver]\nBindIP = \"0.0.0.0\"\n");
        assert!(!set_open(&root, &meta, true).unwrap(), "idempotent");
        assert!(crate::config::list_snapshots(&meta).iter().any(|s| s.reason.contains("opening")), "a snapshot was taken first");
        assert!(set_open(&root, &meta, false).unwrap());
        assert!(fs::read_to_string(root.join("Core/configs/worldserver.conf")).unwrap().contains("BindIP = \"127.0.0.1\""));
    }

    #[test]
    fn sharing_also_allows_remote_coa_clients_and_closing_leaves_it() {
        let (_d, root, meta) = setup();
        let coa = root.join("Core/configs/modules/coa.conf");
        fs::create_dir_all(coa.parent().unwrap()).unwrap();
        fs::write(&coa, "CoA.Enable = 1
CoA.AllowRemoteClients = 0
").unwrap();
        assert!(set_open(&root, &meta, true).unwrap());
        assert!(fs::read_to_string(&coa).unwrap().contains("CoA.AllowRemoteClients = 1"));
        assert!(fs::read_to_string(&coa).unwrap().contains("CoA.Enable = 1"));
        assert!(set_open(&root, &meta, false).unwrap());
        assert!(fs::read_to_string(&coa).unwrap().contains("CoA.AllowRemoteClients = 1"), "closing only changes BindIP");
    }

    #[test]
    fn a_shared_server_gets_its_bind_address_back_before_start_and_a_local_one_is_left_alone() {
        let (_d, root, meta) = setup();
        assert!(!ensure_bind(&root, &meta).unwrap(), "local mode: nothing to do");
        save(&meta, &Settings { mode: Mode::Private, host: Some("100.64.1.2".into()), ..Settings::default() }).unwrap();
        assert!(ensure_bind(&root, &meta).unwrap(), "templates were reset to 127.0.0.1 -> reopened");
        assert!(bind_is_open(&root));
        assert!(!ensure_bind(&root, &meta).unwrap(), "already open: idempotent");
    }

    #[test]
    fn settings_round_trip_and_default_to_local() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path()).mode, Mode::Local);
        save(d.path(), &Settings { mode: Mode::Private, host: Some("100.64.1.2".into()), ..Settings::default() }).unwrap();
        let s = load(d.path());
        assert_eq!((s.mode, s.host.as_deref()), (Mode::Private, Some("100.64.1.2")));
    }

    #[test]
    fn old_settings_load_without_migrating_effective_host_to_override() {
        let s: Settings = serde_json::from_str(r#"{"mode":"lan","host":"192.168.1.50"}"#).unwrap();
        assert_eq!(s.mode, Mode::Lan);
        assert_eq!(s.host.as_deref(), Some("192.168.1.50"));
        assert_eq!(s.lan_address_override, None);
    }

    #[test]
    fn lan_preference_survives_save_load_and_every_other_mode_and_can_be_cleared() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.select_mode(Mode::Lan, "192.168.1.50".into(), Some("192.168.1.50".into()));
        for (mode, host) in [(Mode::Private, "100.101.20.5"), (Mode::Direct, "203.0.113.9"), (Mode::Local, "127.0.0.1")] {
            s.select_mode(mode, host.into(), None);
            save(d.path(), &s).unwrap();
            s = load(d.path());
            assert_eq!(s.lan_address_override.as_deref(), Some("192.168.1.50"));
            assert_eq!(s.host.as_deref(), Some(host));
            let lan = crate::net::resolve_lan_host(s.lan_address_override.as_deref(), Some("192.168.0.169".parse().unwrap())).unwrap();
            s.select_mode(Mode::Lan, lan, s.lan_address_override.clone());
            assert_eq!(s.host.as_deref(), Some("192.168.1.50"));
        }
        s.select_mode(Mode::Lan, "192.168.0.169".into(), None);
        save(d.path(), &s).unwrap();
        assert_eq!(load(d.path()).lan_address_override, None);
    }

    #[test]
    fn friend_package_contains_only_whitelisted_files() {
        let (d, root, _m) = setup();
        fs::write(root.join("Settings/database.json"), "{\"rootPassword\":\"SECRET\"}").unwrap();
        fs::write(root.join("Settings/repack.json"), "{\"raPassword\":\"SECRET\"}").unwrap();
        let addon = root.join("Extras/CoABotUI");
        fs::create_dir_all(addon.join("Libs")).unwrap();
        fs::write(addon.join("CoABotUI.toc"), "## Version: 1").unwrap();
        fs::write(addon.join("Libs/x.lua"), "-- lib").unwrap();
        let out = d.path().join("friend.zip");
        make_friend_package(&root, "203.0.113.7", true, &out).unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&out).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        assert!(names.contains(&"HOW-TO-CONNECT.txt".to_string()) && names.contains(&"realmlist.wtf".to_string()));
        assert!(names.contains(&"Interface/AddOns/CoABotUI/Libs/x.lua".to_string()));
        assert!(!names.iter().any(|n| n.contains("database") || n.contains("repack") || n.contains("conf")), "{names:?}");
        let mut realm = String::new();
        std::io::Read::read_to_string(&mut z.by_name("realmlist.wtf").unwrap(), &mut realm).unwrap();
        assert_eq!(realm, "set realmlist 203.0.113.7\r\n");
        for i in 0..z.len() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(&mut z.by_index(i).unwrap(), &mut body);
            assert!(!body.contains("SECRET"));
        }
        assert!(make_friend_package(&root, "bad host; x", false, &d.path().join("x.zip")).is_err());
    }

    #[test]
    fn manual_lan_host_survives_startup_bind_restoration_and_drives_friend_package() {
        let (d, root, meta) = setup();
        let mut s = Settings::default();
        s.select_mode(Mode::Lan, "192.168.1.50".into(), Some("192.168.1.50".into()));
        save(&meta, &s).unwrap();
        assert!(ensure_bind(&root, &meta).unwrap());
        assert!(bind_is_open(&root));
        let restored = load(&meta);
        assert_eq!(restored.host.as_deref(), Some("192.168.1.50"));
        assert_eq!(restored.lan_address_override.as_deref(), Some("192.168.1.50"));
        let package = d.path().join("manual-lan.zip");
        make_friend_package(&root, restored.host.as_deref().unwrap(), false, &package).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(package).unwrap()).unwrap();
        let mut realm = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("realmlist.wtf").unwrap(), &mut realm).unwrap();
        assert_eq!(realm, "set realmlist 192.168.1.50\r\n");
    }
}
