//! How the Manager reaches a realm for portable play: its database, its console, its job directory and its client data.
//!
//! There are two kinds of realm:
//!
//! * **installed**: a server this Manager installed or imported; everything is derived from the installation (its `Settings`, ports and
//!   folders);
//! * **prepared**: a realm the Manager was given access to by a descriptor file (`<data>/portable/realms/<id>.json`), for a player who has
//!   no server of their own. Phase 9 reaches a realm on the same machine (or one whose database and console are reachable on its loopback
//!   through a shared folder); the secure remote path is the Registry/Relay phase.
//!
//! A descriptor holds credentials. It is read, never echoed: [`RealmAccess`] has no `Debug` of its secrets, and nothing here is part of a
//! diagnostics report.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::{Account, Db};
use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::read_ports;
use crate::ra::Ra;
use crate::realms::Mode;

pub const DESCRIPTOR_SCHEMA: u32 = 1;

#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

impl Serialize for Redacted {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Redacted {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d).map(Redacted)
    }
}

/// What a prepared realm's file says. Paths use forward slashes or escaped backslashes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub schema: u32,
    /// Stable id of the realm in the portable stores (`valid_server_id`).
    pub id: String,
    pub name: String,
    /// What the game client connects to (`set realmlist`).
    pub address: String,
    /// The realm's name in the client's realm list, when it is not the default.
    #[serde(default)]
    pub realm_name: Option<String>,
    /// The folder of the database's client tools (`mysql.exe`).
    pub mysql_bin: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: Redacted,
    pub ra_port: u16,
    pub ra_user: String,
    pub ra_password: Redacted,
    /// The core's `PortableImport.JobDir`.
    pub job_dir: String,
    /// The realm's `Data` folder (its client tables are hashed and compared with what its core loaded).
    pub data_dir: String,
    /// The database users of the game servers: a session of one of them means the realm runs.
    #[serde(default = "default_game_users")]
    pub game_server_users: Vec<String>,
}

fn default_game_users() -> Vec<String> {
    vec!["acore".to_string()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Installed,
    Prepared,
}

/// Everything needed to work with one realm.
#[derive(Debug, Clone)]
pub struct RealmAccess {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub address: String,
    pub realm_name: Option<String>,
    /// The installation id (installed realms): what the rest of the Manager calls this server.
    pub install_id: Option<String>,
    pub root: Option<PathBuf>,
    pub job_dir: PathBuf,
    pub data_dir: PathBuf,
    pub game_server_users: Vec<String>,
    source: Source,
}

#[derive(Debug, Clone)]
enum Source {
    Install { root: PathBuf },
    Descriptor(Box<Descriptor>),
}

pub fn valid_descriptor_id(id: &str) -> bool {
    super::super::store::valid_server_id(id)
}

fn leak(s: &str) -> &'static str {
    // a database user name: a handful per process, kept for its lifetime like the `Db` that needs a `'static` one
    Box::leak(s.to_string().into_boxed_str())
}

impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Result<Descriptor> {
        if bytes.len() > 64 * 1024 {
            return Err(Error::Invalid("A realm file is limited to 64 KiB.".into()));
        }
        let text = String::from_utf8_lossy(bytes);
        let d: Descriptor = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| Error::Invalid(format!("This is not a realm file: {e}")))?;
        d.validate()?;
        Ok(d)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| Error::Invalid(format!("The realm file is not valid: {what}."));
        if self.schema != DESCRIPTOR_SCHEMA {
            return Err(bad("it is a newer or unknown version"));
        }
        if !valid_descriptor_id(&self.id) {
            return Err(bad("the id may only use letters, digits and - _ . :"));
        }
        if self.name.trim().is_empty() || self.name.len() > 80 {
            return Err(bad("the name is empty or too long"));
        }
        if !crate::client::host_ok(&self.address) {
            return Err(bad("the address is not a host name or an IP address"));
        }
        if self.db_port == 0 || self.ra_port == 0 {
            return Err(bad("a port is 0"));
        }
        if !self.db_user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') || self.db_user.is_empty() {
            return Err(bad("the database user is not a plain name"));
        }
        if self.game_server_users.iter().any(|u| u.is_empty() || !u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
            return Err(bad("a game server user is not a plain name"));
        }
        if !Path::new(&self.mysql_bin).join(if cfg!(windows) { "mysql.exe" } else { "mysql" }).is_file() {
            return Err(bad("the database tools are not in the given folder"));
        }
        Ok(())
    }
}

impl RealmAccess {
    pub fn from_descriptor(d: Descriptor) -> Result<RealmAccess> {
        d.validate()?;
        Ok(RealmAccess {
            id: d.id.clone(),
            name: d.name.clone(),
            kind: Kind::Prepared,
            address: d.address.clone(),
            realm_name: d.realm_name.clone(),
            install_id: None,
            root: None,
            job_dir: PathBuf::from(&d.job_dir),
            data_dir: PathBuf::from(&d.data_dir),
            game_server_users: d.game_server_users.clone(),
            source: Source::Descriptor(Box::new(d)),
        })
    }

    /// A server this Manager knows. The realm id is derived from the installation id so that it never collides with a descriptor's.
    pub fn from_install(install_id: &str, root: &Path) -> RealmAccess {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Server".into());
        RealmAccess {
            id: format!("srv-{install_id}"),
            name,
            kind: Kind::Installed,
            address: "127.0.0.1".into(),
            realm_name: None,
            install_id: Some(install_id.to_string()),
            root: Some(root.to_path_buf()),
            job_dir: root.join("Core").join("PortableImport"),
            data_dir: root.join("Data"),
            game_server_users: default_game_users(),
            source: Source::Install { root: root.to_path_buf() },
        }
    }

    /// The database with full rights (imports and updates of a stopped realm, reading characters).
    pub fn db(&self) -> Result<Db> {
        match &self.source {
            Source::Install { root } => Db::from_repack(root, Account::Admin),
            Source::Descriptor(d) => Ok(Db::with_tools(PathBuf::from(&d.mysql_bin), d.db_port, leak(&d.db_user), d.db_password.expose(), Mode::Coa)),
        }
    }

    pub fn ra(&self) -> Result<Ra> {
        match &self.source {
            Source::Install { root } => Ra::connect(root),
            Source::Descriptor(d) => Ra::connect_to(d.ra_port, &d.ra_user, d.ra_password.expose()),
        }
    }

    pub fn db_reachable(&self) -> bool {
        self.db().is_ok_and(|db| db.ping())
    }

    pub fn ports(&self) -> (Option<u16>, u16) {
        match &self.source {
            Source::Install { root } => (None, read_ports(root).ra),
            Source::Descriptor(d) => (Some(d.db_port), d.ra_port),
        }
    }
}

/// The descriptors in a folder (unreadable or invalid ones are reported, not fatal).
pub fn load_descriptors(dir: &Path) -> (Vec<Descriptor>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else { return (ok, bad) };
    let mut files: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    for file in files {
        match std::fs::read(&file).map_err(Error::from).and_then(|b| Descriptor::parse(&b)) {
            Ok(d) => ok.push(d),
            Err(e) => bad.push((file, e.to_string())),
        }
    }
    (ok, bad)
}

/// Remember a descriptor the player gave (copied into the Manager's own folder, so the original may be moved).
pub fn add_descriptor(dir: &Path, source: &Path) -> Result<Descriptor> {
    let bytes = std::fs::read(source).map_err(|e| Error::Invalid(format!("The realm file could not be read: {e}")))?;
    let d = Descriptor::parse(&bytes)?;
    std::fs::create_dir_all(dir)?;
    fsx::atomic_write_json(&dir.join(format!("{}.json", d.id)), &d)?;
    Ok(d)
}

pub fn remove_descriptor(dir: &Path, id: &str) -> Result<bool> {
    if !valid_descriptor_id(id) {
        return Err(Error::Invalid("That is not a realm.".into()));
    }
    match std::fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(bin: &Path) -> Descriptor {
        Descriptor {
            schema: 1,
            id: "pt-guest".into(),
            name: "PT Guest".into(),
            address: "127.0.0.1".into(),
            realm_name: None,
            mysql_bin: bin.to_string_lossy().into(),
            db_port: 15307,
            db_user: "root".into(),
            db_password: Redacted::new("secret-db"),
            ra_port: 15443,
            ra_user: "ra".into(),
            ra_password: Redacted::new("secret-ra"),
            job_dir: "C:/x/jobs".into(),
            data_dir: "C:/x/Data".into(),
            game_server_users: vec!["acore".into()],
        }
    }

    fn tools() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(if cfg!(windows) { "mysql.exe" } else { "mysql" }), b"x").unwrap();
        dir
    }

    #[test]
    fn a_descriptor_round_trips_and_never_prints_its_secrets() {
        let tools = tools();
        let d = sample(tools.path());
        let json = serde_json::to_vec(&d).unwrap();
        assert_eq!(Descriptor::parse(&json).unwrap(), d);
        let shown = format!("{d:?} {:?}", RealmAccess::from_descriptor(d.clone()).unwrap());
        assert!(!shown.contains("secret-db") && !shown.contains("secret-ra"), "{shown}");
        assert!(Descriptor::parse(&[b' '; 70_000]).is_err());
    }

    #[test]
    fn a_descriptor_that_is_not_what_it_claims_is_refused() {
        let tools = tools();
        let ok = sample(tools.path());
        for edit in [
            |d: &mut Descriptor| d.schema = 2,
            |d: &mut Descriptor| d.id = "../evil".into(),
            |d: &mut Descriptor| d.name = " ".into(),
            |d: &mut Descriptor| d.address = "http://x/".into(),
            |d: &mut Descriptor| d.db_port = 0,
            |d: &mut Descriptor| d.db_user = "root'; --".into(),
            |d: &mut Descriptor| d.game_server_users = vec!["a b".into()],
            |d: &mut Descriptor| d.mysql_bin = "Z:/nowhere".into(),
        ] {
            let mut d = ok.clone();
            edit(&mut d);
            assert!(Descriptor::parse(&serde_json::to_vec(&d).unwrap()).is_err(), "{d:?}");
        }
        let mut with_extra: serde_json::Value = serde_json::to_value(&ok).unwrap();
        with_extra["extra"] = 1.into();
        assert!(Descriptor::parse(&serde_json::to_vec(&with_extra).unwrap()).is_err());
    }

    #[test]
    fn descriptors_are_kept_in_the_managers_own_folder() {
        let tools = tools();
        let d = sample(tools.path());
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("any-name.json");
        std::fs::write(&file, serde_json::to_vec(&d).unwrap()).unwrap();
        std::fs::write(src.path().join("broken.json"), b"{").unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(add_descriptor(dir.path(), &file).unwrap(), d);
        assert!(add_descriptor(dir.path(), &src.path().join("broken.json")).is_err());
        let (ok, bad) = load_descriptors(dir.path());
        assert_eq!((ok.len(), bad.len()), (1, 0));
        assert!(remove_descriptor(dir.path(), "pt-guest").unwrap() && !remove_descriptor(dir.path(), "pt-guest").unwrap());
        assert!(remove_descriptor(dir.path(), "../x").is_err());
    }

    #[test]
    fn an_installed_realm_is_derived_from_its_folder() {
        let a = RealmAccess::from_install("abc", Path::new("C:/games/My Server"));
        assert_eq!((a.id.as_str(), a.name.as_str(), a.kind), ("srv-abc", "My Server", Kind::Installed));
        assert!(a.job_dir.ends_with("PortableImport") && a.data_dir.ends_with("Data"));
        assert!(valid_descriptor_id(&a.id));
    }
}
