//! What the Host remembers about publishing: the Registry's address and, per local realm, whether it is published and under which public identity.
//! No key and no credential is stored here (keys live behind [`KeyStore`](super::keys::KeyStore)).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coa_registry_proto::{validate_description, validate_display_name, validate_language, RealmId};
use serde::{Deserialize, Serialize};

use crate::{fsx, Error, Result};

pub const SETTINGS_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishConfig {
    pub realm_id: Option<RealmId>,
    pub enabled: bool,
    pub display_name: String,
    pub description: String,
    pub language: String,
}

impl PublishConfig {
    pub fn validate(&self) -> Result<()> {
        validate_display_name(&self.display_name).map_err(|e| Error::Invalid(e.to_string()))?;
        validate_description(&self.description).map_err(|e| Error::Invalid(e.to_string()))?;
        validate_language(&self.language).map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySettings {
    pub schema: u32,
    /// The Registry's base address, e.g. `http://host` or `https://registry.example` (no path).
    pub url: Option<String>,
    /// Keyed by the local realm id (`srv-<install id>` or a prepared realm's id).
    pub realms: BTreeMap<String, PublishConfig>,
}

impl Default for RegistrySettings {
    fn default() -> Self {
        Self { schema: SETTINGS_SCHEMA, url: None, realms: BTreeMap::new() }
    }
}

/// A Registry address: http or https, a host, an optional port, nothing else.
pub fn validate_url(url: &str) -> Result<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = trimmed.strip_prefix("http://").or_else(|| trimmed.strip_prefix("https://")).ok_or_else(|| Error::Invalid("the Registry address must start with http:// or https://".into()))?;
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err(Error::Invalid("the Registry address is a host and an optional port, nothing else".into()));
    }
    let host = rest.rsplit_once(':').map(|(h, p)| if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() { h } else { rest }).unwrap_or(rest);
    if !crate::client::host_ok(host) || host.contains(':') {
        return Err(Error::Invalid("the Registry host is not a host name or an IPv4 address".into()));
    }
    Ok(trimmed.to_string())
}

pub fn file(dir: &Path) -> PathBuf {
    dir.join("registry.json")
}

pub fn load(dir: &Path) -> Result<RegistrySettings> {
    match std::fs::read(file(dir)) {
        Ok(bytes) => {
            let s: RegistrySettings = serde_json::from_slice(&bytes)?;
            if s.schema != SETTINGS_SCHEMA {
                return Err(Error::Invalid(format!("registry settings schema {} is not supported", s.schema)));
            }
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RegistrySettings::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, settings: &RegistrySettings) -> Result<()> {
    fsx::atomic_write(&file(dir), &serde_json::to_vec_pretty(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_a_host_and_a_port_only() {
        assert_eq!(validate_url("http://127.0.0.1:8080/").unwrap(), "http://127.0.0.1:8080");
        assert_eq!(validate_url(" https://registry.example ").unwrap(), "https://registry.example");
        for bad in ["", "ftp://x", "registry.example", "http://", "http://x/registry", "http://user:pw@x", "http://x?y=1", "http://x y", "http://[::1]:80", "http://x:abc"] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_round_trip_and_refuse_what_they_do_not_know() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), RegistrySettings::default());
        let mut s = RegistrySettings { url: Some("http://x".into()), ..Default::default() };
        s.realms.insert("srv-1".into(), PublishConfig { realm_id: Some(RealmId::new()), enabled: true, display_name: "A".into(), description: String::new(), language: "en".into() });
        save(dir.path(), &s).unwrap();
        assert_eq!(load(dir.path()).unwrap(), s);
        let text = std::fs::read_to_string(file(dir.path())).unwrap();
        assert!(!text.contains("key") && !text.contains("password"), "{text}");
        std::fs::write(file(dir.path()), r#"{"schema":1,"url":null,"realms":{},"private_key":"x"}"#).unwrap();
        assert!(load(dir.path()).is_err());
    }
}
