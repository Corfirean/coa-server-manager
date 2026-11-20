//! Owns the connection file created by the Manager. Handwritten connection files stay with their owners.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::{Error, Result, fsx};

const MODULE: &str = "mod-pbc";
const PATH_SETTING: &str = "PBC.DefaultConnection";
const CONNECTION_FILE: &str = "Extras/PlayerbotCharacters/manager-connection.jsonc";
const CONNECTION_SETTING: &str = "../Extras/PlayerbotCharacters/manager-connection.jsonc";

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiType {
    Openai,
    Ollama,
    Anthropic,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionUpdate {
    pub api_type: ApiType,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub remove_key: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    api_type: ApiType,
    base_url: String,
    model: String,
    api_key: String,
    request_timeout_sec: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub api_type: ApiType,
    pub base_url: String,
    pub model: String,
    pub has_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub active: bool,
    pub connection: Option<ConnectionView>,
}

fn connection_path(root: &Path) -> Result<PathBuf> {
    fsx::ensure_within(root, &root.join(CONNECTION_FILE))
}

fn read_connection(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn parse(bytes: &[u8]) -> Result<Connection> {
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid("The Manager model connection file is invalid.".into()))
}

fn view(connection: Connection) -> ConnectionView {
    ConnectionView {
        api_type: connection.api_type,
        base_url: connection.base_url,
        model: connection.model,
        has_key: !connection.api_key.is_empty(),
    }
}

fn module_setting(root: &Path) -> Result<String> {
    super::settings(root, MODULE)?
        .into_iter()
        .find(|setting| setting.key == PATH_SETTING)
        .map(|setting| setting.value)
        .ok_or_else(|| Error::Invalid("This server package does not provide a model connection setting.".into()))
}

pub fn load(root: &Path) -> Result<Setup> {
    let active = module_setting(root)?.trim_matches('"') == CONNECTION_SETTING;
    let connection = read_connection(&connection_path(root)?)?
        .as_deref()
        .map(parse)
        .transpose()?
        .map(view);
    Ok(Setup { active, connection })
}

pub fn save(root: &Path, meta: &Path, update: ConnectionUpdate) -> Result<Setup> {
    let active = module_setting(root)?.trim_matches('"') == CONNECTION_SETTING;
    let base_url = update.base_url.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(base_url).map_err(|_| Error::Invalid("Enter a valid model API base URL.".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none()
        || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
    {
        return Err(Error::Invalid("Use an HTTP or HTTPS base URL without credentials, a query, or a fragment.".into()));
    }
    let base_url = url.as_str().trim_end_matches('/');
    let model = update.model.trim();
    if model.is_empty() || model.chars().any(char::is_control) {
        return Err(Error::Invalid("Enter a model name on a single line.".into()));
    }
    let submitted_key = update.api_key.as_deref().map(str::trim).filter(|value| !value.is_empty());
    if submitted_key.is_some_and(|value| value.chars().any(char::is_control)) {
        return Err(Error::Invalid("Enter the API key on a single line.".into()));
    }

    let path = connection_path(root)?;
    let previous = read_connection(&path)?;
    let previous_connection = previous.as_deref().map(parse).transpose()?;
    let key = if update.remove_key {
        String::new()
    } else if let Some(key) = submitted_key {
        key.to_owned()
    } else {
        previous_connection
            .filter(|old| old.api_type == update.api_type && old.base_url == base_url)
            .map(|old| old.api_key)
            .unwrap_or_default()
    };
    let connection = Connection {
        api_type: update.api_type,
        base_url: base_url.into(),
        model: model.into(),
        api_key: key,
        request_timeout_sec: if matches!(update.api_type, ApiType::Ollama) { 60 } else { 30 },
    };
    fsx::atomic_write_private(&path, &serde_json::to_vec_pretty(&connection)?)?;
    if !active {
        let changes = BTreeMap::from([(PATH_SETTING.into(), CONNECTION_SETTING.into())]);
        if let Err(error) = super::save_settings(root, meta, MODULE, &changes) {
            if let Some(bytes) = previous {
                fsx::atomic_write_private(&path, &bytes)?;
            } else {
                fs::remove_file(&path)?;
            }
            return Err(error);
        }
    }
    Ok(Setup { active: true, connection: Some(view(connection)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(api_type: ApiType, base_url: &str, api_key: Option<&str>) -> ConnectionUpdate {
        ConnectionUpdate { api_type, base_url: base_url.into(), model: "example-model".into(),
            api_key: api_key.map(str::to_owned), remove_key: false }
    }

    #[test]
    fn model_setup_preserves_manual_files_and_only_reuses_keys_for_the_same_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let meta = root.join("metadata");
        let configs = root.join("Core/configs/modules");
        fs::create_dir_all(&meta).unwrap();
        fs::create_dir_all(&configs).unwrap();
        fs::write(configs.join("playerbots_characters.conf.dist"),
            "[worldserver]\nPBC.Enable = 0\nPBC.DefaultConnection = ../my-custom.jsonc\n").unwrap();
        fs::write(root.join("my-custom.jsonc"), b"// manual connection").unwrap();
        assert!(!load(root).unwrap().active);

        let base_url = "https://api.example.com/v1";
        let saved = save(root, &meta, update(ApiType::Openai, base_url, Some("fixture-key"))).unwrap();
        assert!(saved.active && saved.connection.as_ref().unwrap().has_key);
        assert!(!serde_json::to_string(&saved).unwrap().contains("fixture-key"));
        assert!(save(root, &meta, update(ApiType::Openai, base_url, None)).unwrap().connection.unwrap().has_key);
        let mut clear = update(ApiType::Openai, base_url, None);
        clear.remove_key = true;
        assert!(!save(root, &meta, clear).unwrap().connection.unwrap().has_key);
        save(root, &meta, update(ApiType::Openai, base_url, Some("replacement-key"))).unwrap();
        let changed = save(root, &meta, update(ApiType::Ollama, "http://127.0.0.1:11434", None)).unwrap();
        assert!(!changed.connection.unwrap().has_key);
        assert!(save(root, &meta, update(ApiType::Openai, "https://user:secret@example.com", None)).is_err());
        assert_eq!(fs::read(root.join("my-custom.jsonc")).unwrap(), b"// manual connection");
        let config = fs::read_to_string(configs.join("playerbots_characters.conf")).unwrap();
        assert!(config.contains("PBC.Enable = 0"));
        assert!(!config.contains("replacement-key"));
    }
}
