//! `Settings/repack.json`: ports and the console login of one repack (`manage.py settings()`).

use std::path::Path;

use crate::error::{Error, Result};

pub const PORT_KEYS: [&str; 4] = ["mysqlPort", "authPort", "worldPort", "raPort"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mysql_port: u16,
    pub auth_port: u16,
    pub world_port: u16,
    pub ra_port: u16,
    pub ra_username: String,
    pub ra_password: String,
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
                _ => return Err(Error::Invalid("Invalid port in Settings/repack.json".into())),
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
}
