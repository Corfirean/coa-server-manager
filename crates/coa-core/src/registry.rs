//! Installation registry and per-installation metadata directory (`<server>.manager`).
//!
//! Registering or importing an installation writes only inside the metadata directory and the registry file.
//! The server folder itself is never written to.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;

pub const META_SCHEMA: u32 = 1;
pub const LAYOUT_REPACK_V1: &str = "repack-v1";
pub const LAYOUT_DOCKER_V1: &str = "docker-v1";
const META_SUBDIRS: [&str; 6] = ["manifests", "backups", "migrations", "logs", "cache", "staging"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    New,
    Imported,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: Option<String>,
    pub commit: Option<String>,
}

/// Non-secret database facts only. Credentials never live in install.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbInfo {
    pub port: Option<u16>,
    pub schemas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMeta {
    pub schema: u32,
    pub id: String,
    pub kind: InstallKind,
    pub server_path: String,
    pub layout: String,
    pub created_at: String,
    pub manager_version: String,
    #[serde(default)]
    pub core: VersionInfo,
    #[serde(default)]
    pub bots: VersionInfo,
    #[serde(default)]
    pub database: DbInfo,
    #[serde(default)]
    pub client_path: Option<String>,
    /// Files that existed before the Manager touched the installation (path -> sha256, when known).
    #[serde(default)]
    pub original_hashes: BTreeMap<String, String>,
    /// Files the Manager itself created or later modified.
    #[serde(default)]
    pub managed_files: Vec<String>,
}

impl InstallMeta {
    pub fn new(kind: InstallKind, server_path: &Path) -> Self {
        Self {
            schema: META_SCHEMA,
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            server_path: server_path.to_string_lossy().into_owned(),
            layout: LAYOUT_REPACK_V1.into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            manager_version: crate::MANAGER_VERSION.into(),
            core: VersionInfo::default(),
            bots: VersionInfo::default(),
            database: DbInfo::default(),
            client_path: None,
            original_hashes: BTreeMap::new(),
            managed_files: Vec::new(),
        }
    }
}

/// `D:\CoA-Repack` -> `D:\CoA-Repack.manager`.
pub fn metadata_dir_for(server: &Path) -> Result<PathBuf> {
    let name = server
        .file_name()
        .ok_or_else(|| Error::Invalid("a drive root cannot be a server folder".into()))?
        .to_string_lossy()
        .into_owned();
    Ok(server.with_file_name(format!("{name}.manager")))
}

pub struct MetaDir {
    pub root: PathBuf,
}

impl MetaDir {
    pub fn install_json(&self) -> PathBuf {
        self.root.join("install.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.root.join("logs").join("manager.log")
    }

    /// Create the metadata directory for `server` (which must already exist) and write `install.json`.
    pub fn create(server: &Path, meta: &InstallMeta) -> Result<MetaDir> {
        if !server.is_dir() {
            return Err(Error::Invalid(format!("{} is not a folder", server.display())));
        }
        let server_abs = fsx::canonicalize_lenient(server)?;
        let root = metadata_dir_for(&server_abs)?;
        if fsx::starts_with_ci(&root, &server_abs) {
            return Err(Error::PathRejected("metadata folder would be inside the server folder".into()));
        }
        fs::create_dir_all(&root)?;
        for sub in META_SUBDIRS {
            fs::create_dir_all(root.join(sub))?;
        }
        let dir = MetaDir { root };
        fsx::atomic_write_json(&dir.install_json(), meta)?;
        Ok(dir)
    }

    pub fn open(root: &Path) -> Result<(MetaDir, InstallMeta)> {
        let meta: InstallMeta = fsx::read_json(&root.join("install.json"))?;
        Ok((MetaDir { root: root.to_path_buf() }, meta))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    installs: BTreeMap<String, String>,
}

/// `%LOCALAPPDATA%\CoAServerManager\installs.json`: id -> server path. Removing an entry never touches files.
pub struct Registry {
    file: PathBuf,
}

impl Registry {
    pub fn at(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    fn load(&self) -> Result<RegistryFile> {
        if !self.file.exists() {
            return Ok(RegistryFile::default());
        }
        fsx::read_json(&self.file)
    }

    pub fn list(&self) -> Result<Vec<(String, PathBuf)>> {
        Ok(self.load()?.installs.into_iter().map(|(id, p)| (id, PathBuf::from(p))).collect())
    }

    pub fn find_by_path(&self, server: &Path) -> Result<Option<String>> {
        let target = fsx::canonicalize_lenient(server)?;
        for (id, p) in self.list()? {
            if fsx::canonicalize_lenient(&p).map(|c| fsx::starts_with_ci(&c, &target) && fsx::starts_with_ci(&target, &c)).unwrap_or(false) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Register an installation. Rejects duplicates and nested/overlapping folders.
    pub fn register(&self, id: &str, server: &Path) -> Result<()> {
        let target = fsx::canonicalize_lenient(server)?;
        let mut file = self.load()?;
        for (other_id, other) in &file.installs {
            let other = fsx::canonicalize_lenient(Path::new(other)).unwrap_or_else(|_| PathBuf::from(other));
            if other_id == id {
                continue;
            }
            if fsx::starts_with_ci(&target, &other) || fsx::starts_with_ci(&other, &target) {
                return Err(Error::Invalid(format!(
                    "{} overlaps already registered installation {}",
                    target.display(),
                    other.display()
                )));
            }
        }
        file.installs.insert(id.to_string(), target.to_string_lossy().into_owned());
        fsx::atomic_write_json(&self.file, &file)
    }

    pub fn unregister(&self, id: &str) -> Result<()> {
        let mut file = self.load()?;
        if file.installs.remove(id).is_none() {
            return Err(Error::UnknownInstallation(id.into()));
        }
        fsx::atomic_write_json(&self.file, &file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_dir_is_a_sibling_and_server_folder_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("CoA-Repack");
        fs::create_dir_all(server.join("Core")).unwrap();
        fs::write(server.join("Core/worldserver.exe"), b"x").unwrap();
        let before = snapshot(&server);

        let meta = InstallMeta::new(InstallKind::Imported, &server);
        let md = MetaDir::create(&server, &meta).unwrap();
        assert_eq!(md.root.file_name().unwrap(), "CoA-Repack.manager");
        assert!(md.install_json().exists());
        for sub in META_SUBDIRS {
            assert!(md.root.join(sub).is_dir());
        }
        assert_eq!(before, snapshot(&server), "import must not modify the server folder");

        let (_, loaded) = MetaDir::open(&md.root).unwrap();
        assert_eq!(loaded.id, meta.id);
        assert_eq!(loaded.kind, InstallKind::Imported);
    }

    fn snapshot(root: &Path) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64)>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let md = e.metadata().unwrap();
                let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.push((rel, if md.is_dir() { 0 } else { md.len() }));
                if md.is_dir() {
                    walk(&e.path(), root, out);
                }
            }
        }
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn registry_rejects_duplicates_and_nesting_but_allows_reregister_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::at(dir.path().join("reg/installs.json"));
        let a = dir.path().join("a");
        let inner = a.join("inner");
        let b = dir.path().join("b");
        for p in [&a, &inner, &b] {
            fs::create_dir_all(p).unwrap();
        }
        reg.register("1", &a).unwrap();
        reg.register("1", &a).unwrap();
        assert!(reg.register("2", &a).is_err(), "same folder");
        assert!(reg.register("2", &inner).is_err(), "nested inside existing");
        assert!(reg.register("3", dir.path()).is_err(), "contains existing");
        reg.register("2", &b).unwrap();
        assert_eq!(reg.list().unwrap().len(), 2);
        assert_eq!(reg.find_by_path(&a).unwrap().as_deref(), Some("1"));
        reg.unregister("1").unwrap();
        assert!(a.exists(), "unregister never deletes files");
        assert!(matches!(reg.unregister("1"), Err(Error::UnknownInstallation(_))));
    }

    #[test]
    fn drive_root_and_missing_folder_are_rejected() {
        let meta = InstallMeta::new(InstallKind::New, Path::new("C:/nope"));
        assert!(MetaDir::create(Path::new("C:/definitely/missing/folder"), &meta).is_err());
        let root = if cfg!(windows) { "C:/" } else { "/" };
        assert!(metadata_dir_for(Path::new(root)).is_err());
    }
}
