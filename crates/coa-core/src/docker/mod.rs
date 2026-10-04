//! Docker backend. The three services (MySQL, authserver, worldserver) run in containers that the Manager creates
//! and controls through the `docker` command line; the server folder keeps the same shape as a repack where it
//! matters (`Core/configs`, `Core/Logs`, `Settings/*.json`, `Data/`) so configuration, logs and the RA console work
//! unchanged. This is the only runtime available on Linux. The Windows repack path is not touched: an
//! installation is a Docker one only when `Settings/docker.json` exists.
//!
//! ```text
//! <server>/
//!   Core/worldserver, authserver   mounted at /srv/core (working directory of both containers)
//!   Core/configs/ Core/Logs/       read and written by the Manager as usual
//!   Data/                          mounted read-only at /srv/data
//!   Settings/docker.json           marks the installation and names its containers
//!   Settings/repack.json           ports and RA login (same file as a repack)
//!   Settings/database.json         database passwords (same file as a repack)
//! ```

mod cli;
mod lifecycle;

pub use cli::{Call, Docker, Output, SystemDocker};
pub use lifecycle::{observe, observe_with, run, run_with};

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::{self, Classification, ScanReport};

/// Present in a Docker installation, absent from a repack.
pub const MARKER: &str = "Settings/docker.json";
const MYSQL_IMAGE: &str = "mysql:8.4";
const RUNTIME_DOCKERFILE: &str = include_str!("Dockerfile.runtime");

pub fn is_docker(root: &Path) -> bool {
    root.join(MARKER).is_file()
}

fn loopback() -> String {
    "127.0.0.1".into()
}

fn mysql_image() -> String {
    MYSQL_IMAGE.into()
}

/// `Settings/docker.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Short unique name of this installation; part of every container, network and volume name.
    pub project: String,
    /// Host address the game ports are published on: `127.0.0.1` (this computer only) or `0.0.0.0` / a LAN address.
    #[serde(default = "loopback")]
    pub bind_address: String,
    #[serde(default = "mysql_image")]
    pub mysql_image: String,
}

pub(crate) struct Names {
    pub network: String,
    pub volume: String,
    pub db: String,
    pub world: String,
    pub auth: String,
}

impl Config {
    pub fn load(root: &Path) -> Result<Config> {
        let cfg: Config = fsx::read_json(&root.join(MARKER)).map_err(|_| Error::Invalid("The Docker settings of this server could not be read.".into()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let name_ok = !self.project.is_empty()
            && self.project.len() <= 32
            && self.project.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && self.project.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !name_ok {
            return Err(Error::Invalid("The Docker project name must be 1-32 characters: lower-case letters, digits and dashes.".into()));
        }
        if self.bind_address.parse::<IpAddr>().is_err() {
            return Err(Error::Invalid(format!("{} is not an IP address.", self.bind_address)));
        }
        let image_ok = !self.mysql_image.is_empty() && self.mysql_image.chars().all(|c| c.is_ascii_alphanumeric() || "._/:@-".contains(c));
        if !image_ok {
            return Err(Error::Invalid("The database image name is not valid.".into()));
        }
        Ok(())
    }

    /// Name of the database container (the one `docker exec` talks to).
    pub(crate) fn database_container(&self) -> String {
        self.names().db
    }

    pub(crate) fn names(&self) -> Names {
        let p = &self.project;
        Names { network: format!("coa-{p}"), volume: format!("coa-{p}-db"), db: format!("coa-{p}-db"), world: format!("coa-{p}-world"), auth: format!("coa-{p}-auth") }
    }
}

/// Name of the runtime image: it follows the content of its Dockerfile, so changing the libraries builds a new image.
pub(crate) fn runtime_image() -> String {
    format!("coa-runtime:{}", &fsx::sha256_bytes(RUNTIME_DOCKERFILE.as_bytes())[..12])
}

/// Read-only description of a Docker installation, shaped like the scan of a repack so the screens that list what a
/// server holds work for both.
pub(crate) fn scan(root: &Path) -> Result<ScanReport> {
    let cfg = Config::load(root);
    let world = layout::hash_exe(&root.join("Core/worldserver"), None, "worldserver");
    let auth = layout::hash_exe(&root.join("Core/authserver"), None, "authserver");
    let modules_dir = root.join("Core/configs/modules");
    let mut module_configs: Vec<String> = std::fs::read_dir(&modules_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".conf")).collect())
        .unwrap_or_default();
    module_configs.sort();

    let bot_active = modules_dir.join("mod_coa_playerbots.conf");
    let bot_conf = if bot_active.is_file() { bot_active } else { modules_dir.join("mod_coa_playerbots.conf.dist") };
    let bot_config_keys = layout::count_bot_keys(&bot_conf);
    let has_data = layout::exists(root, "Data/dbc") && layout::exists(root, "Data/maps");
    let has_confs = layout::exists(root, "Core/configs/worldserver.conf") && layout::exists(root, "Core/configs/authserver.conf");

    let items = vec![
        layout::item("worldserver", "World server", world.is_some(), None),
        layout::item("authserver", "Auth server", auth.is_some(), None),
        layout::item("worldserver_conf", "World server configuration", layout::exists(root, "Core/configs/worldserver.conf"), None),
        layout::item("authserver_conf", "Auth server configuration", layout::exists(root, "Core/configs/authserver.conf"), None),
        layout::item("modules_conf", "Module configuration", modules_dir.is_dir(), Some(format!("{} files", module_configs.len()))),
        layout::item("game_data", "Game data (maps, DBC)", has_data, None),
        layout::item("database_runtime", "Database runtime", cfg.is_ok(), cfg.as_ref().ok().map(|c| c.mysql_image.clone())),
        layout::item("launcher", "Docker settings", cfg.is_ok(), cfg.as_ref().ok().map(|c| c.project.clone())),
        layout::item("companions", "CoA Companions (bots)", bot_conf.is_file(), (bot_config_keys > 0).then(|| format!("{bot_config_keys} settings"))),
    ];
    let mut notes = Vec::new();
    if let Err(e) = &cfg {
        notes.push(e.to_string());
    }
    let healthy = cfg.is_ok() && world.is_some() && auth.is_some() && has_data && has_confs;
    Ok(ScanReport {
        path: root.to_string_lossy().into_owned(),
        classification: if healthy { Classification::Healthy } else { Classification::Partial },
        items,
        worldserver: world,
        authserver: auth,
        release: None,
        banner_revision: layout::banner_revision_in_log(&root.join("Core/Logs/Server.log")),
        module_configs,
        bot_config_keys,
        ports: layout::read_ports(root),
        database_schemas: Vec::new(),
        client: ["Client", "client"].iter().find_map(|d| layout::detect_client(&root.join(d))),
        notes,
        suggested_path: None,
        hint: None,
        modifies_files: false,
    })
}

