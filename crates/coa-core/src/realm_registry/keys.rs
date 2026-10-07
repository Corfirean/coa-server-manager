//! A realm's Ed25519 identity key: one per realm, generated on first publication, never leaving this machine.
//!
//! Storage is behind [`KeyStore`]. The implementation here keeps one file per realm in the Manager's own data folder, readable by the current user
//! only (mode 0600 on Unix; on Windows the inherited permissions are removed and the current user is granted access with `icacls`). That is
//! weaker than an OS-protected secret store (DPAPI / Credential Manager), which this project does not use yet: anybody who can read the user's
//! files can read the key. The key is never written to the realm descriptor, a log, the diagnostics or the Registry.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;

use crate::{Error, Result};

pub trait KeyStore: Send + Sync {
    /// The key of this realm, if one was created.
    fn load(&self, realm: &RealmId) -> Result<Option<SigningKey>>;
    /// A new random key for a realm that has none. Refuses to replace an existing one.
    fn create(&self, realm: &RealmId) -> Result<SigningKey>;
    fn remove(&self, realm: &RealmId) -> Result<()>;
}

pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn file(&self, realm: &RealmId) -> PathBuf {
        self.dir.join(format!("{realm}.key"))
    }
}

fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

#[cfg(unix)]
fn restrict(file: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn restrict(file: &Path) -> Result<()> {
    let user = std::env::var("USERNAME").map_err(|_| Error::Invalid("the current user is unknown".into()))?;
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let who = if domain.is_empty() { user } else { format!("{domain}\\{user}") };
    let out = std::process::Command::new("icacls").arg(file).arg("/inheritance:r").arg("/grant:r").arg(format!("{who}:(F)")).output()?;
    if !out.status.success() {
        return Err(Error::Invalid("the key file's permissions could not be restricted".into()));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict(_file: &Path) -> Result<()> {
    Ok(())
}

impl KeyStore for FileKeyStore {
    fn load(&self, realm: &RealmId) -> Result<Option<SigningKey>> {
        let text = match fs::read_to_string(self.file(realm)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let bytes = B64.decode(text.trim()).map_err(|_| Error::Invalid("a realm key file is not valid".into()))?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| Error::Invalid("a realm key file is not valid".into()))?;
        Ok(Some(SigningKey::from_bytes(&seed)))
    }

    fn create(&self, realm: &RealmId) -> Result<SigningKey> {
        fs::create_dir_all(&self.dir)?;
        let path = self.file(realm);
        if path.exists() {
            return Err(Error::Invalid("this realm already has a key".into()));
        }
        let seed = random_seed();
        let tmp = self.dir.join(format!(".{realm}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let write = || -> Result<()> {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            restrict(&tmp)?;
            f.write_all(B64.encode(seed).as_bytes())?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &path)?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(SigningKey::from_bytes(&seed))
    }

    fn remove(&self, realm: &RealmId) -> Result<()> {
        match fs::remove_file(self.file(realm)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileKeyStore::new(dir.path().join("keys"));
        let realm = RealmId::new();
        assert!(store.load(&realm).unwrap().is_none());
        let key = store.create(&realm).unwrap();
        assert_eq!(store.load(&realm).unwrap().unwrap().to_bytes(), key.to_bytes());
        assert!(store.create(&realm).is_err(), "a key is never replaced");
        let other = RealmId::new();
        assert_ne!(store.create(&other).unwrap().to_bytes(), key.to_bytes(), "one key per realm");
        store.remove(&realm).unwrap();
        assert!(store.load(&realm).unwrap().is_none());
        store.remove(&realm).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("keys")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_damaged_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileKeyStore::new(dir.path());
        let realm = RealmId::new();
        fs::write(dir.path().join(format!("{realm}.key")), "garbage").unwrap();
        assert!(store.load(&realm).is_err());
        assert!(store.create(&realm).is_err(), "the broken file is not silently replaced");
    }

    #[cfg(windows)]
    #[test]
    fn the_key_file_is_not_inherited_from_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileKeyStore::new(dir.path());
        let realm = RealmId::new();
        store.create(&realm).unwrap();
        let out = std::process::Command::new("icacls").arg(dir.path().join(format!("{realm}.key"))).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(!text.contains("BUILTIN\\Users") && !text.contains("Everyone") && !text.contains("Authenticated Users"), "{text}");
    }
}
