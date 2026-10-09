//! Secrets the Manager keeps for the player: the Ed25519 key of the player's identity and the game account passwords per realm. They are never written to a
//! JSON file, to SQLite, to a log or to a diagnostics report.
//!
//! * **Windows**: every secret is one file that holds a DPAPI blob (`CryptProtectData`, current-user scope, an application-specific entropy value). Only the
//!   same Windows user on the same machine can decrypt it; a copy of the file on another machine or account is useless. (The files live in the Manager's data
//!   folder. Windows Credential Manager would also have done; DPAPI is what it uses underneath, without its size and naming limits.)
//! * **Other systems**: a file with mode 0600 in the Manager's data folder. **That is not encryption**: anybody who can read the user's files can read it. A
//!   desktop keyring (Secret Service / Keychain) is the proper store there and is not implemented yet; [`SecretStore::kind`] says which one is in use so that
//!   the interface can tell the player.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::{Error, Result};

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, name: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
    /// `dpapi`, `file` (mode 0600, not encrypted) or `memory`.
    fn kind(&self) -> &'static str;
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 96
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || name.starts_with('.')
    {
        return Err(Error::Invalid("a secret has an invalid name".into()));
    }
    Ok(())
}

/// The store this system should use, in `dir`.
pub fn default_store(dir: &Path) -> Box<dyn SecretStore> {
    #[cfg(windows)]
    {
        Box::new(DpapiStore::new(dir))
    }
    #[cfg(not(windows))]
    {
        Box::new(FileStore::new(dir))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid("a secret has no folder".into()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        restrict(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn restrict(file: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn restrict(file: &Path) -> Result<()> {
    let user = std::env::var("USERNAME")
        .map_err(|_| Error::Invalid("the current user is unknown".into()))?;
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let who = if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    };
    let out = std::process::Command::new("icacls")
        .arg(file)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{who}:(F)"))
        .output()?;
    if !out.status.success() {
        return Err(Error::Invalid(
            "a secret file's permissions could not be restricted".into(),
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn restrict(_file: &Path) -> Result<()> {
    Ok(())
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// A file with mode 0600 per secret. Not encrypted.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.secret"))
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        check_name(name)?;
        read_file(&self.path(name))
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<()> {
        check_name(name)?;
        write_private(&self.path(name), value)
    }
    fn delete(&self, name: &str) -> Result<()> {
        check_name(name)?;
        remove_file(&self.path(name))
    }
    fn kind(&self) -> &'static str {
        "file"
    }
}

/// Process memory only (tests).
#[derive(Default)]
pub struct MemoryStore(Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        check_name(name)?;
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::Invalid("a lock was poisoned".into()))?
            .get(name)
            .cloned())
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<()> {
        check_name(name)?;
        self.0
            .lock()
            .map_err(|_| Error::Invalid("a lock was poisoned".into()))?
            .insert(name.to_string(), value.to_vec());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<()> {
        check_name(name)?;
        self.0
            .lock()
            .map_err(|_| Error::Invalid("a lock was poisoned".into()))?
            .remove(name);
        Ok(())
    }
    fn kind(&self) -> &'static str {
        "memory"
    }
}

#[cfg(windows)]
pub use dpapi::DpapiStore;

#[cfg(windows)]
mod dpapi {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Mixed into every blob: a different program running as the same user does not decrypt the Manager's secrets by accident.
    const ENTROPY: &[u8] = b"coa-server-manager/secret-store/v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        v
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(Error::Invalid("Windows could not protect a secret".into()));
        }
        Ok(take(out))
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = blob(sealed);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(Error::Invalid("a secret could not be unlocked (it was made by another Windows user or on another computer)".into()));
        }
        Ok(take(out))
    }

    pub struct DpapiStore {
        dir: PathBuf,
    }

    impl DpapiStore {
        pub fn new(dir: impl Into<PathBuf>) -> Self {
            Self { dir: dir.into() }
        }
        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(format!("{name}.dpapi"))
        }
    }

    impl SecretStore for DpapiStore {
        fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
            check_name(name)?;
            read_file(&self.path(name))?
                .map(|b| unprotect(&b))
                .transpose()
        }
        fn put(&self, name: &str, value: &[u8]) -> Result<()> {
            check_name(name)?;
            write_private(&self.path(name), &protect(value)?)
        }
        fn delete(&self, name: &str) -> Result<()> {
            check_name(name)?;
            remove_file(&self.path(name))
        }
        fn kind(&self) -> &'static str {
            "dpapi"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(store: &dyn SecretStore) {
        assert_eq!(store.get("realm-abc").unwrap(), None);
        store.put("realm-abc", b"hunter2-secret").unwrap();
        assert_eq!(
            store.get("realm-abc").unwrap().as_deref(),
            Some(b"hunter2-secret".as_slice())
        );
        store.put("realm-abc", b"another").unwrap();
        assert_eq!(
            store.get("realm-abc").unwrap().as_deref(),
            Some(b"another".as_slice()),
            "replaced"
        );
        store.delete("realm-abc").unwrap();
        store.delete("realm-abc").unwrap();
        assert_eq!(store.get("realm-abc").unwrap(), None);
        for bad in ["", "../x", "a/b", "a b", ".hidden", &"x".repeat(97)] {
            assert!(
                store.put(bad, b"x").is_err() && store.get(bad).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_memory_and_file_stores_keep_and_forget() {
        round_trip(&MemoryStore::default());
        let dir = tempfile::tempdir().unwrap();
        round_trip(&FileStore::new(dir.path().join("secrets")));
        assert_eq!(FileStore::new(dir.path()).kind(), "file");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_secrets_are_not_stored_in_the_clear_and_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = DpapiStore::new(dir.path().join("secrets"));
        round_trip(&store);
        store
            .put("player-key", b"VERY-SECRET-PLAYER-KEY-0123456789")
            .unwrap();
        let raw = fs::read(dir.path().join("secrets").join("player-key.dpapi")).unwrap();
        assert!(
            !raw.windows(11).any(|w| w == b"VERY-SECRET"),
            "the file holds no plaintext"
        );
        assert!(raw.len() > 40);
        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        fs::write(
            dir.path().join("secrets").join("player-key.dpapi"),
            tampered,
        )
        .unwrap();
        assert!(
            store.get("player-key").is_err(),
            "a damaged blob is an error, not garbage"
        );
        assert_eq!(store.kind(), "dpapi");
    }
}
