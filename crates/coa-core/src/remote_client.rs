//! A player's client connection, independent of any local server installation.
use crate::{client, fsx, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub host: String,
    pub client_path: Option<String>,
    /// The listed server `host` belongs to, when the player joined it through the server list: its account is
    /// filled into the game's login box at every Play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
}

pub fn load(dir: &Path) -> Result<Profile> {
    match std::fs::read(dir.join("connection.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profile::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, profile: &Profile) -> Result<()> {
    if !profile.host.is_empty() && !client::host_ok(&profile.host) {
        return Err(Error::Invalid(
            "Enter a server IP address or hostname, without a URL or port.".into(),
        ));
    }
    if let Some(path) = &profile.client_path {
        let client = PathBuf::from(path);
        if client::detect(&client, None).is_none() {
            return Err(Error::Invalid("This folder does not look like a game client (it needs Data and the game executable).".into()));
        }
        if client::is_running(&client) {
            return Err(Error::Invalid(
                "Close the game before changing its connection.".into(),
            ));
        }
        if !profile.host.is_empty() {
            client::set_realmlist(&client, dir, &profile.host)?;
        }
    }
    fsx::atomic_write_json(&dir.join("connection.json"), profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_from_before_the_realm_field_still_loads() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("connection.json"), br#"{"host":"1.2.3.4","client_path":null}"#).unwrap();
        let p = load(d.path()).unwrap();
        assert_eq!((p.host.as_str(), p.realm), ("1.2.3.4", None));
    }
    #[test]
    fn connection_needs_no_server_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            host: "192.168.1.10".into(),
            client_path: None,
            realm: None,
        };
        save(dir.path(), &profile).unwrap();
        assert_eq!(load(dir.path()).unwrap().host, profile.host);
        assert!(!dir.path().join("install.json").exists());
    }
    #[test]
    fn linking_client_sets_host_without_touching_other_game_settings() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(game.join("Data/enUS")).unwrap();
        std::fs::write(game.join("Wow.exe"), b"fixture").unwrap();
        std::fs::write(
            game.join("Data/enUS/realmlist.wtf"),
            "set realmlist 127.0.0.1\n",
        )
        .unwrap();
        std::fs::create_dir_all(game.join("WTF")).unwrap();
        std::fs::write(game.join("WTF/Config.wtf"), "SET realmName \"My realm\"\n").unwrap();
        let profile = Profile {
            host: "192.168.1.10".into(),
            client_path: Some(game.to_string_lossy().into()),
            realm: None,
        };
        save(dir.path(), &profile).unwrap();
        assert!(
            std::fs::read_to_string(game.join("Data/enUS/realmlist.wtf"))
                .unwrap()
                .contains(&profile.host)
        );
        assert_eq!(
            std::fs::read_to_string(game.join("WTF/Config.wtf")).unwrap(),
            "SET realmName \"My realm\"\n"
        );
        assert!(save(
            dir.path(),
            &Profile {
                host: "bad\nset realmlist other".into(),
                ..profile
            }
        )
        .is_err());
        assert_eq!(load(dir.path()).unwrap().host, "192.168.1.10");
    }
}
