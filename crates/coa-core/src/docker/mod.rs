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
pub mod fixture;
pub mod install;
mod lifecycle;
pub mod logs;
pub mod sharing;

pub use cli::{Call, Docker, Output, SystemDocker};
pub use lifecycle::{check_docker, observe, observe_with, run, run_with};
pub use sharing::{bind_is_open, exposure, set_open};
pub(crate) use lifecycle::destroy;

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::{self, Classification, ScanReport};

/// Present in a Docker installation, absent from a repack.
pub const MARKER: &str = "Settings/docker.json";
pub(crate) const MYSQL_IMAGE: &str = "mysql:8.4";
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
    /// Where the game data (dbc, maps, vmaps, mmaps) is, when it is not the `Data` folder of the server. Used in place and
    /// mounted read-only, so one copy can serve several servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// A MySQL data directory the database container uses in place of its volume. Only for the disposable fixture that
    /// validates a release (the data directory of the signed base package); an installation never sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mysql_data: Option<String>,
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

    /// The folder mounted as the game data.
    pub fn data_path(&self, root: &Path) -> std::path::PathBuf {
        self.data_dir.as_deref().map(std::path::PathBuf::from).unwrap_or_else(|| root.join("Data"))
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
        for d in [&self.data_dir, &self.mysql_data].into_iter().flatten() {
            // The folder goes into a `--volume host:container` option, where a colon would be read as a separator.
            if !Path::new(d).is_absolute() || d.contains(':') {
                return Err(Error::Invalid("A Docker folder must be a full path without a colon.".into()));
            }
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

/// Create the world and auth configuration files from their `.dist` templates when they do not exist. A package only
/// carries the templates, and on Windows the launcher writes the active files at every start; here nothing else would.
/// An existing file is never touched, so the person's settings survive updates. Returns the files created.
pub fn ensure_main_configs(root: &Path) -> Result<Vec<String>> {
    let mut created = Vec::new();
    for name in ["worldserver", "authserver"] {
        let (conf, dist) = (root.join(format!("Core/configs/{name}.conf")), root.join(format!("Core/configs/{name}.conf.dist")));
        if !conf.exists() && dist.is_file() {
            std::fs::copy(&dist, &conf)?;
            created.push(format!("{name}.conf"));
        }
    }
    Ok(created)
}

/// Settings the Docker backend gives the containers as environment variables. The core reads `AC_<KEY>` before the
/// configuration file, so a value written to the file for one of these keys is silently ignored. They are what makes the
/// containers work (where things are inside them, listening on every interface, the console, no built-in database
/// updater, the databases); the owner has no use for them and the screens do not offer them.
pub(crate) const FORCED_KEYS: [&str; 12] = [
    "BindIP",
    "WorldServerPort",
    "RealmServerPort",
    "LogsDir",
    "DataDir",
    "Updates.EnableDatabases",
    "Ra.Enable",
    "Ra.IP",
    "Ra.Port",
    "LoginDatabaseInfo",
    "WorldDatabaseInfo",
    "CharacterDatabaseInfo",
];

/// Is this setting fixed by the Docker backend (see `FORCED_KEYS`)?
pub fn managed_setting(key: &str) -> bool {
    FORCED_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// The environment variable the core reads for a setting: `AC_` and the key in upper snake case, a `_` between a lower-case
/// letter and a capital, and at letter / digit boundaries (the core's `IniKeyToEnvVarKey`).
#[cfg(test)]
pub(crate) fn env_name(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let mut out = String::from("AC_");
    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, ' ' | '.' | '-') {
            out.push('_');
            continue;
        }
        out.push(c.to_ascii_uppercase());
        if let Some(&next) = chars.get(i + 1) {
            let split = (!c.is_ascii_uppercase() && next.is_ascii_uppercase()) || (!c.is_ascii_digit() && next.is_ascii_digit()) || (c.is_ascii_digit() && !next.is_ascii_digit() && !matches!(next, ' ' | '.' | '-'));
            if split {
                out.push('_');
            }
        }
    }
    out
}

/// The folder mounted as the game data of this Docker server.
pub fn game_data_dir(root: &Path) -> Result<std::path::PathBuf> {
    Ok(Config::load(root)?.data_path(root))
}

/// Point a Docker server at another game data folder (used in place, read-only). The containers are recreated at the next
/// start, so the servers must be stopped: a running one keeps the folder it has.
pub fn set_game_data(root: &Path, path: &Path) -> Result<()> {
    let problems = install::data_problems(path);
    if !problems.is_empty() {
        return Err(Error::Invalid(problems.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join(" ")));
    }
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    let busy = |s: &crate::process::ServiceStatus| matches!(s.state, crate::process::ServiceState::Running | crate::process::ServiceState::Starting | crate::process::ServiceState::Stopping);
    if busy(&observed.world) || busy(&observed.auth) {
        return Err(Error::Invalid("Stop the server before changing the game data folder.".into()));
    }
    let mut cfg = Config::load(root)?;
    cfg.data_dir = Some(path.to_string_lossy().into_owned());
    cfg.validate()?;
    fsx::atomic_write_json(&root.join(MARKER), &cfg)
}

/// A reason a value cannot work on a Docker server, for settings that name a path: the server runs in a container that sees only
/// its own `Core` folder (as `/srv/core`, its working directory), so a path elsewhere on this computer does not exist for it.
pub fn setting_problem(key: &str, raw: &str) -> Option<&'static str> {
    let raw = raw.trim().trim_matches('"');
    let outside = Path::new(raw).is_absolute() && !raw.starts_with("/srv/core/");
    (key.eq_ignore_ascii_case("CoaBots.TalentBuildsPath") && !raw.is_empty() && outside)
        .then_some("must be a path inside the server's Core folder (for example reference/ascensionsidekick-level-builds.json), because the server runs in a container that cannot see other folders")
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
    let data = cfg.as_ref().map(|c| c.data_path(root)).unwrap_or_else(|_| root.join("Data"));
    let has_data = data.join("dbc").is_dir() && data.join("maps").is_dir();
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn the_game_data_folder_can_be_changed_when_the_servers_are_stopped() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        std::fs::create_dir_all(root.join("Settings")).unwrap();
        std::fs::write(root.join(MARKER), r#"{"project":"t1","dataDir":"/old/data"}"#).unwrap();
        let good = d.path().join("data");
        std::fs::create_dir_all(good.join("dbc")).unwrap();
        std::fs::create_dir_all(good.join("maps")).unwrap();
        assert_eq!(game_data_dir(&root).unwrap(), std::path::PathBuf::from("/old/data"));
        set_game_data(&root, &good).unwrap();
        assert_eq!(game_data_dir(&root).unwrap(), good);
        assert_eq!(Config::load(&root).unwrap().project, "t1", "the other settings are kept");
        // A folder that does not look like game data is refused and nothing changes.
        let empty = d.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(set_game_data(&root, &empty).is_err());
        assert_eq!(game_data_dir(&root).unwrap(), good);
        assert!(set_game_data(&root, std::path::Path::new("relative/dir")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_path_outside_the_core_folder_is_refused_for_a_docker_server() {
        let key = "CoaBots.TalentBuildsPath";
        assert!(setting_problem(key, "reference/builds.json").is_none());
        assert!(setting_problem(key, "").is_none());
        assert!(setting_problem(key, "\"/srv/core/reference/builds.json\"").is_none());
        assert!(setting_problem(key, "/home/ana/builds.json").is_some());
        assert!(setting_problem("Rate.XP.Kill", "/home/ana/x").is_none(), "only settings that name a path");
    }

    #[test]
    fn setting_names_become_the_variables_the_core_reads() {
        for (key, var) in [
            ("BindIP", "AC_BIND_IP"),
            ("Ra.Enable", "AC_RA_ENABLE"),
            ("Ra.IP", "AC_RA_IP"),
            ("Updates.EnableDatabases", "AC_UPDATES_ENABLE_DATABASES"),
            ("WorldServerPort", "AC_WORLD_SERVER_PORT"),
            ("LoginDatabaseInfo", "AC_LOGIN_DATABASE_INFO"),
            ("DataDir", "AC_DATA_DIR"),
            ("Dynamic.XP.Reminder.Interval", "AC_DYNAMIC_XP_REMINDER_INTERVAL"),
            ("EtherealBazaar.Enable", "AC_ETHEREAL_BAZAAR_ENABLE"),
        ] {
            assert_eq!(env_name(key), var, "{key}");
        }
    }

    #[test]
    fn the_settings_called_managed_are_exactly_the_variables_the_containers_get() {
        // If a variable is added to the containers without being listed here (or the other way round), a screen would
        // offer a setting that does nothing, or hide one that works.
        let given: std::collections::BTreeSet<String> = lifecycle::forced_variables().into_iter().collect();
        let listed: std::collections::BTreeSet<String> = FORCED_KEYS.iter().map(|k| env_name(k)).collect();
        assert_eq!(given, listed);
        assert!(managed_setting("ra.enable") && managed_setting("Updates.EnableDatabases") && !managed_setting("PlayerLimit"));
    }

    #[test]
    fn the_main_configs_are_created_from_their_templates_and_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("Core/configs");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("worldserver.conf.dist"), "Setting = default\n").unwrap();
        fs::write(cfg.join("authserver.conf.dist"), "Auth = default\n").unwrap();

        assert_eq!(ensure_main_configs(d.path()).unwrap(), ["worldserver.conf", "authserver.conf"]);
        assert_eq!(fs::read_to_string(cfg.join("worldserver.conf")).unwrap(), "Setting = default\n");

        // The person's own settings survive the next start.
        fs::write(cfg.join("worldserver.conf"), "Setting = mine\n").unwrap();
        assert!(ensure_main_configs(d.path()).unwrap().is_empty());
        assert_eq!(fs::read_to_string(cfg.join("worldserver.conf")).unwrap(), "Setting = mine\n");
        // Without a template there is nothing to create and nothing fails.
        fs::remove_file(cfg.join("authserver.conf")).unwrap();
        fs::remove_file(cfg.join("authserver.conf.dist")).unwrap();
        assert!(ensure_main_configs(d.path()).unwrap().is_empty());
    }
}
