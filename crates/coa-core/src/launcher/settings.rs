//! `Settings/repack.json`: ports and the console login of one repack (`manage.py settings()`).

use std::path::Path;

use crate::error::{Error, Result};

pub const PORT_KEYS: [&str; 4] = ["mysqlPort", "authPort", "worldPort", "raPort"];

/// The realm a folder runs: its row in `acore_auth.realmlist` and its two game databases.
///
/// Without `Settings/realm-profile.json` this is what the repack launcher always did (realm 1 "AzerothCore" on
/// `acore_world`/`acore_characters`). With it, the active profile decides, as the Manager's realm patch of
/// `manage.py` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realm {
    pub id: u32,
    pub name: &'static str,
    pub world_db: &'static str,
    pub characters_db: &'static str,
}

impl Realm {
    pub fn load(root: &Path) -> Result<Realm> {
        let path = root.join("Settings/realm-profile.json");
        if !path.exists() {
            return Ok(Realm { id: 1, name: "AzerothCore", world_db: "acore_world", characters_db: "acore_characters" });
        }
        let v: serde_json::Value = crate::fsx::read_json(&path)?;
        Ok(match v.get("active").and_then(|a| a.as_str()).unwrap_or("coa") {
            "wildcard" => Realm { id: 2, name: "Wildcard", world_db: "acore_world_wildcard", characters_db: "acore_characters_wildcard" },
            "reborn" => Realm { id: 3, name: "Warcraft Reborn", world_db: "acore_world_reborn", characters_db: "acore_characters_reborn" },
            _ => Realm { id: 1, name: "Conquest of Azeroth", world_db: "acore_world", characters_db: "acore_characters" },
        })
    }
}

/// An extra world of a server (`<server>/.realms/<name>`): it shares the main folder's database and never
/// starts or stops it.
pub fn is_extra_world(root: &Path) -> bool {
    root.parent().and_then(|p| p.file_name()).is_some_and(|n| n == ".realms")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mysql_port: u16,
    pub auth_port: u16,
    pub world_port: u16,
    pub ra_port: u16,
    pub ra_username: String,
    pub ra_password: String,
    pub realm: Realm,
    /// The database belongs to the main folder of this server (an extra world): it is only checked, never
    /// started.
    pub shared_database: bool,
    /// Environment variables the started services must not see (for example a port override of the caller).
    pub env_remove: Vec<String>,
    /// The whole document, for keys this code does not know (kept when it is written back).
    pub raw: serde_json::Map<String, serde_json::Value>,
}

impl Settings {
    /// Read `Settings/repack.json`, with every port moved by `offset` (a second world of the same server runs on
    /// shifted ports). The same checks as `manage.py`: integer ports in 1025..65535 after the offset, all
    /// different.
    pub fn load(root: &Path, offset: i32) -> Result<Settings> {
        let bytes = std::fs::read(root.join("Settings/repack.json"))?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let raw: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(text).map_err(|e| Error::Invalid(format!("Settings/repack.json: {e}")))?;
        let mut ports = [0u16; 4];
        for (i, key) in PORT_KEYS.iter().enumerate() {
            // Python's `type(value) is not int` refuses booleans and floats alike.
            let value = raw.get(*key).and_then(|v| v.as_i64()).filter(|_| raw[*key].is_i64() || raw[*key].is_u64());
            let shifted = value.map(|v| v + i64::from(offset));
            match shifted {
                Some(p) if p > 1024 && p < 65536 => ports[i] = p as u16,
                _ => return Err(Error::Service(crate::error::ErrorCode::InvalidConfigValue, "Invalid port in Settings/repack.json".into())),
            }
        }
        let mut unique = ports.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != 4 {
            return Err(Error::Invalid("Each service needs a different port.".into()));
        }
        let text_of = |k: &str| raw.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        Ok(Settings {
            mysql_port: ports[0],
            auth_port: ports[1],
            world_port: ports[2],
            ra_port: ports[3],
            ra_username: text_of("raUsername"),
            ra_password: text_of("raPassword"),
            realm: Realm::load(root)?,
            shared_database: is_extra_world(root),
            env_remove: Vec::new(),
            raw,
        })
    }

    /// `{"mysqlPort": .., "authPort": .., "worldPort": .., "raPort": ..}` as `manage.py` records it in
    /// `.state/configuration.json` (the keys of repack.json ending in "Port", in the file's order).
    pub fn port_map(&self) -> serde_json::Map<String, serde_json::Value> {
        let shifted = [self.mysql_port, self.auth_port, self.world_port, self.ra_port];
        let mut out = serde_json::Map::new();
        for key in self.raw.keys().filter(|k| k.ends_with("Port")) {
            let value = PORT_KEYS
                .iter()
                .position(|p| p == key)
                .map(|i| serde_json::Value::from(shifted[i]))
                .unwrap_or_else(|| self.raw[key].clone());
            out.insert(key.clone(), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir.join("Settings")).unwrap();
        std::fs::write(dir.join("Settings/repack.json"), text).unwrap();
    }

    #[test]
    fn ports_are_checked_like_the_python_launcher() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), r#"{"mysqlPort":3307,"authPort":3724,"worldPort":8085,"raPort":3443,"raUsername":"U","raPassword":"P"}"#);
        let s = Settings::load(d.path(), 0).unwrap();
        assert_eq!((s.mysql_port, s.auth_port, s.world_port, s.ra_port), (3307, 3724, 8085, 3443));
        assert_eq!(s.ra_username, "U");
        let shifted = Settings::load(d.path(), 10).unwrap();
        assert_eq!(shifted.world_port, 8095);
        assert_eq!(shifted.port_map()["worldPort"], serde_json::json!(8095));

        for bad in [
            r#"{"mysqlPort":1024,"authPort":3724,"worldPort":8085,"raPort":3443}"#,
            r#"{"mysqlPort":"3307","authPort":3724,"worldPort":8085,"raPort":3443}"#,
            r#"{"mysqlPort":3307.0,"authPort":3724,"worldPort":8085,"raPort":3443}"#,
            r#"{"mysqlPort":true,"authPort":3724,"worldPort":8085,"raPort":3443}"#,
            r#"{"mysqlPort":3307,"authPort":3307,"worldPort":8085,"raPort":3443}"#,
            r#"{"authPort":3724,"worldPort":8085,"raPort":3443}"#,
        ] {
            write(d.path(), bad);
            assert!(Settings::load(d.path(), 0).is_err(), "{bad}");
        }
        write(d.path(), "\u{feff}{\"mysqlPort\":3307,\"authPort\":3724,\"worldPort\":8085,\"raPort\":65535}");
        assert!(Settings::load(d.path(), 1).is_err(), "65536 after the offset");
        assert!(Settings::load(d.path(), 0).is_ok(), "a byte order mark is accepted");
    }

    #[test]
    fn the_realm_follows_the_active_profile() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Realm::load(d.path()).unwrap().name, "AzerothCore");
        std::fs::create_dir_all(d.path().join("Settings")).unwrap();
        for (active, id, db) in [("coa", 1, "acore_world"), ("wildcard", 2, "acore_world_wildcard"), ("reborn", 3, "acore_world_reborn")] {
            std::fs::write(d.path().join("Settings/realm-profile.json"), format!("{{\"active\":\"{active}\"}}")).unwrap();
            let r = Realm::load(d.path()).unwrap();
            assert_eq!((r.id, r.world_db), (id, db));
        }
        assert!(is_extra_world(&d.path().join(".realms/secondary")));
        assert!(!is_extra_world(d.path()));
    }
}
