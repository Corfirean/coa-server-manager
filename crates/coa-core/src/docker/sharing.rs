//! Sharing a Docker server with friends. Who can reach the game is decided by the address the container ports are
//! published on (`bindAddress` in `Settings/docker.json`): `127.0.0.1` is this computer only, anything else is the network.
//! The `BindIP` of the configuration files is forced to `0.0.0.0` inside the containers and changes nothing, so the
//! repack's way of opening the servers (editing `BindIP`) is replaced here. The database is never published and the server
//! console is only ever published on the loopback address.

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use super::{Config, MARKER};
use crate::config::parser::ConfFile;
use crate::config::{take_snapshot, Scope};
use crate::error::Result;
use crate::fsx;
use crate::layout::Ports;
use crate::net::Exposure;
use crate::process::{ServiceState, ServiceStatus};

fn is_open(cfg: &Config) -> bool {
    cfg.bind_address.parse::<IpAddr>().map(|ip| !ip.is_loopback()).unwrap_or(false)
}

/// Whether the login and world servers are published beyond this computer.
pub fn bind_is_open(root: &Path) -> bool {
    Config::load(root).map(|c| is_open(&c)).unwrap_or(false)
}

/// Publish the login and world servers beyond this computer (`0.0.0.0`) or only on it (`127.0.0.1`). An address the owner chose
/// by hand (for example one network card) is kept when opening. Returns true if anything changed, which needs a restart of
/// the containers. The old files are snapshotted.
pub fn set_open(root: &Path, meta: &Path, open: bool) -> Result<bool> {
    let mut cfg = Config::load(root)?;
    let mut originals: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut changed = false;
    if is_open(&cfg) != open {
        originals.push((root.join(MARKER), fs::read(root.join(MARKER))?));
        cfg.bind_address = if open { "0.0.0.0" } else { "127.0.0.1" }.into();
        changed = true;
    }
    // Friends connect from other addresses, and the server only applies the CoA client protocol to loopback unless remote clients
    // are allowed; opening switches that on (closing leaves it, it only matters while the servers listen beyond this computer).
    let coa_conf = root.join("Core/configs/modules/coa.conf");
    let mut coa_edit = None;
    if open {
        if let Ok(bytes) = fs::read(&coa_conf) {
            let mut conf = ConfFile::parse_bytes(&bytes)?;
            if conf.get("CoA.AllowRemoteClients").map(str::trim) != Some("1") {
                conf.set("CoA.AllowRemoteClients", "1", &["Set by CoA Server Manager together with the friends mode"]);
                originals.push((coa_conf.clone(), bytes));
                coa_edit = Some(conf.to_text());
            }
        }
    }
    if !changed && coa_edit.is_none() {
        return Ok(false);
    }
    take_snapshot(meta, Scope::Server, if open { "before opening the servers to friends" } else { "before closing the servers to friends" }, &originals)?;
    if changed {
        fsx::atomic_write_json(&root.join(MARKER), &cfg)?;
    }
    if let Some(text) = coa_edit {
        fsx::atomic_write(&coa_conf, text.as_bytes())?;
    }
    Ok(changed)
}

/// What each service can be reached from, computed from how the containers publish their ports: the database is never published,
/// the console only on the loopback address, the game servers on `bindAddress`.
pub fn exposure(root: &Path, ports: &Ports) -> Vec<Exposure> {
    let open = bind_is_open(root);
    let observed = crate::process::observe(root, ports);
    let up = |s: &ServiceStatus| matches!(s.state, ServiceState::Running | ServiceState::Starting);
    vec![
        Exposure { port: ports.mysql, what: "database", listening: up(&observed.mysql), reachable_from_network: false },
        Exposure { port: ports.ra, what: "server console", listening: up(&observed.world), reachable_from_network: false },
        Exposure { port: ports.auth, what: "login server", listening: up(&observed.auth), reachable_from_network: open && up(&observed.auth) },
        Exposure { port: ports.world, what: "game world", listening: up(&observed.world), reachable_from_network: open && up(&observed.world) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(bind: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        let meta = d.path().join("srv.manager");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::create_dir_all(root.join("Core/configs/modules")).unwrap();
        fs::create_dir_all(&meta).unwrap();
        let extra = bind.map(|b| format!(r#","bindAddress":"{b}""#)).unwrap_or_default();
        fs::write(root.join(MARKER), format!(r#"{{"project":"t1"{extra}}}"#)).unwrap();
        fs::write(root.join("Core/configs/modules/coa.conf"), "CoA.Enable = 1\n").unwrap();
        (d, root, meta)
    }

    #[test]
    fn opening_and_closing_changes_the_published_address_and_allows_remote_clients() {
        let (_d, root, meta) = server(None);
        assert!(!bind_is_open(&root));
        assert!(set_open(&root, &meta, true).unwrap(), "a restart is needed");
        assert!(bind_is_open(&root));
        assert_eq!(Config::load(&root).unwrap().bind_address, "0.0.0.0");
        assert!(fs::read_to_string(root.join("Core/configs/modules/coa.conf")).unwrap().contains("CoA.AllowRemoteClients = 1"));
        assert!(!set_open(&root, &meta, true).unwrap(), "nothing left to change");
        assert!(set_open(&root, &meta, false).unwrap());
        assert!(!bind_is_open(&root));
        assert_eq!(Config::load(&root).unwrap().bind_address, "127.0.0.1");
    }

    #[test]
    fn an_address_chosen_by_hand_is_kept_when_opening_and_other_settings_survive() {
        let (_d, root, meta) = server(Some("192.168.1.20"));
        assert!(bind_is_open(&root), "any address but the loopback is the network");
        set_open(&root, &meta, true).unwrap();
        let cfg = Config::load(&root).unwrap();
        assert_eq!(cfg.bind_address, "192.168.1.20");
        assert_eq!(cfg.project, "t1");
    }

    #[test]
    fn the_old_settings_are_snapshotted_before_a_change() {
        let (_d, root, meta) = server(None);
        set_open(&root, &meta, true).unwrap();
        assert_eq!(crate::config::list_snapshots(&meta).len(), 1);
    }

    #[test]
    fn the_database_and_the_console_are_never_reported_reachable() {
        let (_d, root, _meta) = server(Some("0.0.0.0"));
        let rows = exposure(&root, &Ports::default());
        let by = |what: &str| rows.iter().find(|e| e.what == what).unwrap();
        assert!(!by("database").reachable_from_network && !by("server console").reachable_from_network);
        // Nothing runs in this test (docker is not asked to start anything), so nothing is reachable yet.
        assert!(rows.iter().all(|e| !e.reachable_from_network));
    }
}
