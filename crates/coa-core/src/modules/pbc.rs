//! Owns the connection file created by the Manager. Handwritten connection files stay with their owners.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::parser::{ConfFile, unquote};
use crate::realms::{self, Mode};
use crate::{Error, Result, fsx};

pub mod models;

const MODULE: &str = "mod-pbc";
const PATH_SETTING: &str = "PBC.DefaultConnection";
pub(crate) const ASSET_DIR: &str = "Extras/PlayerbotCharacters";

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

fn managed_connection(root: &Path) -> Result<(PathBuf, String)> {
    let file = match realms::state(root)?.active {
        Mode::Coa => "manager-connection.jsonc",
        Mode::Wildcard => "manager-connection-wildcard.jsonc",
        Mode::Reborn => "manager-connection-reborn.jsonc",
    };
    let relative = format!("{ASSET_DIR}/{file}");
    let path = fsx::ensure_within(root, &root.join(&relative))?;
    Ok((path, format!("../{relative}")))
}

/// Secondary worlds use their own Core directory. Point their packaged paths back to the shared assets.
pub(crate) fn rebase_packaged_paths(config: &str, root: &Path) -> Result<String> {
    let mut conf = ConfFile::parse_bytes(config.as_bytes())?;
    let assets = root.join(ASSET_DIR);
    for key in [
        PATH_SETTING, "PBC.UtilityConnection", "PBC.CondensationConnection",
        "PBC.RelationshipUpdateConnection", "PBC.PromptsPath", "PBC.CharacterCardsPath",
        "PBC.HttpServerFrontendPath",
    ] {
        let Some(relative) = conf.get(key).map(unquote)
            .and_then(|path| path.strip_prefix("../Extras/PlayerbotCharacters/"))
        else { continue };
        let Ok(path) = fsx::safe_join(&assets, relative) else { continue };
        let path = path.to_string_lossy().replace('\\', "/");
        conf.set(key, &format!("\"{path}\""), &[]);
    }
    Ok(conf.to_text())
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
    let (path, setting) = managed_connection(root)?;
    let active = unquote(&module_setting(root)?) == setting;
    let connection = read_connection(&path)?
        .as_deref()
        .map(parse)
        .transpose()?
        .map(view);
    Ok(Setup { active, connection })
}

fn connection_url(base_url: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(base_url.trim().trim_end_matches('/'))
        .map_err(|_| Error::Invalid("Enter a valid model API base URL.".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none()
        || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
    {
        return Err(Error::Invalid("Use an HTTP or HTTPS base URL without credentials, a query, or a fragment.".into()));
    }
    Ok(url)
}

fn connection_key(
    api_type: ApiType,
    base_url: &str,
    submitted_key: Option<&str>,
    remove_key: bool,
    previous: Option<Connection>,
) -> Result<String> {
    let submitted_key = submitted_key.map(str::trim).filter(|value| !value.is_empty());
    if submitted_key.is_some_and(|value| value.chars().any(char::is_control)) {
        return Err(Error::Invalid("Enter the API key on a single line.".into()));
    }

    let key = if remove_key {
        String::new()
    } else if let Some(key) = submitted_key {
        key.to_owned()
    } else {
        previous
            .filter(|old| old.api_type == api_type && old.base_url == base_url)
            .map(|old| old.api_key)
            .unwrap_or_default()
    };
    Ok(key)
}

pub fn save(root: &Path, meta: &Path, update: ConnectionUpdate) -> Result<Setup> {
    let (path, setting) = managed_connection(root)?;
    let active = unquote(&module_setting(root)?) == setting;
    let url = connection_url(&update.base_url)?;
    let base_url = url.as_str().trim_end_matches('/');
    let model = update.model.trim();
    if model.is_empty() || model.chars().any(char::is_control) {
        return Err(Error::Invalid("Enter a model name on a single line.".into()));
    }
    let previous = read_connection(&path)?;
    let key = connection_key(
        update.api_type, base_url, update.api_key.as_deref(), update.remove_key,
        previous.as_deref().map(parse).transpose()?,
    )?;
    let connection = Connection {
        api_type: update.api_type,
        base_url: base_url.into(),
        model: model.into(),
        api_key: key,
        request_timeout_sec: if matches!(update.api_type, ApiType::Ollama) { 60 } else { 30 },
    };
    fsx::atomic_write_private(&path, &serde_json::to_vec_pretty(&connection)?)?;
    if !active {
        let changes = BTreeMap::from([(PATH_SETTING.into(), setting)]);
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

    #[test]
    fn secondary_world_uses_the_shared_pbc_assets_and_keeps_custom_paths() {
        let temp = tempfile::tempdir().unwrap();
        let config = "[worldserver]\nPBC.DefaultConnection = \"../Extras/PlayerbotCharacters/manager-connection.jsonc\"\nPBC.PromptsPath = \"../Extras/PlayerbotCharacters/prompts\"\nPBC.UtilityConnection = \"custom.jsonc\"\n";
        let result = rebase_packaged_paths(config, temp.path()).unwrap();
        let result = ConfFile::parse_bytes(result.as_bytes()).unwrap();
        let expected = temp.path().join(ASSET_DIR).to_string_lossy().replace('\\', "/");
        assert_eq!(result.get(PATH_SETTING).map(unquote), Some(format!("{expected}/manager-connection.jsonc").as_str()));
        assert_eq!(result.get("PBC.PromptsPath").map(unquote), Some(format!("{expected}/prompts").as_str()));
        assert_eq!(result.get("PBC.UtilityConnection").map(unquote), Some("custom.jsonc"));
    }

    #[test]
    fn each_realm_saves_its_own_model_connection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let config = root.join("Core/configs/modules/playerbots_characters.conf");
        let meta = root.join("metadata");
        fsx::atomic_write(&config, b"[worldserver]\nPBC.Enable = 0\nPBC.DefaultConnection = \"\"\n").unwrap();
        let (coa_path, _) = managed_connection(root).unwrap();
        let endpoint = "https://example.test/v1";
        save(root, &meta, update(ApiType::Openai, endpoint, Some("coa-key"))).unwrap();
        let coa_config = fs::read(&config).unwrap();
        let coa_connection = fs::read(&coa_path).unwrap();

        let state = root.join("Settings/realm-profile.json");
        let mut realm_state = realms::RealmState {
            active: Mode::Wildcard,
            wildcard_created: true,
            ..Default::default()
        };
        fsx::atomic_write_json(&state, &realm_state).unwrap();
        let (wildcard_path, _) = managed_connection(root).unwrap();
        assert_ne!(coa_path, wildcard_path);
        assert!(load(root).unwrap().connection.is_none());
        let mut wildcard = update(ApiType::Openai, endpoint, None);
        wildcard.model = "wildcard-model".into();
        let saved = save(root, &meta, wildcard).unwrap().connection.unwrap();
        assert!(!saved.has_key);
        assert_eq!(saved.model, "wildcard-model");
        assert_eq!(fs::read(&coa_path).unwrap(), coa_connection);

        realm_state.active = Mode::Coa;
        fsx::atomic_write_json(&state, &realm_state).unwrap();
        fsx::atomic_write(&config, &coa_config).unwrap();
        let restored = load(root).unwrap();
        assert!(restored.active && restored.connection.unwrap().has_key);
    }
}
