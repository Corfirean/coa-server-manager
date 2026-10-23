//! A realm's Ed25519 identity key: one per realm, generated on first publication, never leaving this machine.
//!
//! Storage is behind [`KeyStore`]. The implementation here uses [`ProtectedKeyStore`], which integrates with the Manager's
//! OS-protected [`SecretStore`](crate::control::secrets::SecretStore):
//! * **Windows**: DPAPI `CryptProtectData` (current-user scope, domain-separated application entropy). No plaintext private key at rest.
//! * **Other systems**: Restricted file permissions (mode 0600).
//!
//! Migration from legacy unencrypted `{realm}.key` files is automatic and transactional:
//! 1. The legacy file is read and its Ed25519 seed is validated.
//! 2. The seed is encrypted and stored into `SecretStore`.
//! 3. The encrypted key is verified by reading it back and confirming it matches.
//! 4. Only after verification succeeds, the plaintext legacy file is removed.
//!
//! If a key is corrupted or cannot be decrypted, the store **fails closed** with an actionable error.
//! It **never** silently regenerates a new realm identity behind the user's back.

use std::fs;
use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;

use crate::control::secrets::{default_store, SecretStore};
use crate::{Error, Result};

pub trait KeyStore: Send + Sync {
    /// The key of this realm, if one was created.
    fn load(&self, realm: &RealmId) -> Result<Option<SigningKey>>;
    /// A new random key for a realm that has none. Refuses to replace an existing one.
    fn create(&self, realm: &RealmId) -> Result<SigningKey>;
    fn remove(&self, realm: &RealmId) -> Result<()>;
}

/// Key store backed by the platform's protected [`SecretStore`], with automatic transactional migration from legacy files.
pub struct ProtectedKeyStore {
    dir: PathBuf,
    secrets: Box<dyn SecretStore>,
}

impl ProtectedKeyStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let secrets = default_store(&dir);
        Self { dir, secrets }
    }

    pub fn with_store(dir: impl Into<PathBuf>, secrets: Box<dyn SecretStore>) -> Self {
        Self {
            dir: dir.into(),
            secrets,
        }
    }

    fn legacy_file(&self, realm: &RealmId) -> PathBuf {
        self.dir.join(format!("{realm}.key"))
    }

    fn secret_name(realm: &RealmId) -> String {
        format!("realm-key-{realm}")
    }
}

pub type FileKeyStore = ProtectedKeyStore;

fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

impl KeyStore for ProtectedKeyStore {
    fn load(&self, realm: &RealmId) -> Result<Option<SigningKey>> {
        let name = Self::secret_name(realm);

        // 1. Check protected SecretStore first
        if let Some(bytes) = self.secrets.get(&name)? {
            let seed: [u8; 32] = bytes.try_into().map_err(|_| {
                Error::Invalid("a realm signing key in protected storage is corrupted".into())
            })?;
            let key = SigningKey::from_bytes(&seed);

            // Clean up any remaining legacy plaintext file left over from an interrupted migration
            let legacy = self.legacy_file(realm);
            if legacy.exists() {
                let _ = fs::remove_file(&legacy);
            }
            return Ok(Some(key));
        }

        // 2. Secret not yet in SecretStore; check for legacy plaintext file
        let legacy = self.legacy_file(realm);
        if !legacy.exists() {
            return Ok(None);
        }

        // Transactional migration from legacy plaintext file:
        // Step A: Read once
        let text = match fs::read_to_string(&legacy) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // Step B: Validate seed & key
        let bytes = B64
            .decode(text.trim())
            .map_err(|_| Error::Invalid("legacy realm key file is corrupt".into()))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Invalid("legacy realm key file has invalid length".into()))?;
        let key = SigningKey::from_bytes(&seed);

        // Step C: Encrypt into SecretStore atomically
        self.secrets.put(&name, &seed)?;

        // Step D: Verify it can be decrypted and matches exactly
        let verified = self.secrets.get(&name)?.ok_or_else(|| {
            Error::Invalid("failed to verify newly encrypted realm key in secret store".into())
        })?;
        if verified != seed {
            return Err(Error::Invalid(
                "verification mismatch for encrypted realm key".into(),
            ));
        }

        // Step E: Only then remove old plaintext key file
        let _ = fs::remove_file(&legacy);

        Ok(Some(key))
    }

    fn create(&self, realm: &RealmId) -> Result<SigningKey> {
        let name = Self::secret_name(realm);
        let legacy = self.legacy_file(realm);

        // Fail closed: never overwrite an existing key in either store
        if legacy.exists() {
            return Err(Error::Invalid("this realm already has a key".into()));
        }
        if self.secrets.get(&name)?.is_some() {
            return Err(Error::Invalid("this realm already has a key".into()));
        }

        let seed = random_seed();
        self.secrets.put(&name, &seed)?;

        // Verify stored key immediately
        let verified = self.secrets.get(&name)?.ok_or_else(|| {
            Error::Invalid("failed to verify created realm key in secret store".into())
        })?;
        if verified != seed {
            let _ = self.secrets.delete(&name);
            return Err(Error::Invalid(
                "verification failed for newly created realm key".into(),
            ));
        }

        Ok(SigningKey::from_bytes(&seed))
    }

    fn remove(&self, realm: &RealmId) -> Result<()> {
        let name = Self::secret_name(realm);
        let _ = self.secrets.delete(&name);
        let legacy = self.legacy_file(realm);
        if legacy.exists() {
            let _ = fs::remove_file(legacy);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProtectedKeyStore::new(dir.path().join("keys"));
        let realm = RealmId::new();
        assert!(store.load(&realm).unwrap().is_none());
        let key = store.create(&realm).unwrap();
        assert_eq!(
            store.load(&realm).unwrap().unwrap().to_bytes(),
            key.to_bytes()
        );
        assert!(store.create(&realm).is_err(), "a key is never replaced");
        let other = RealmId::new();
        assert_ne!(
            store.create(&other).unwrap().to_bytes(),
            key.to_bytes(),
            "one key per realm"
        );
        store.remove(&realm).unwrap();
        assert!(store.load(&realm).unwrap().is_none());
        store.remove(&realm).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("keys"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn migration_from_legacy_plaintext_key_to_protected_store() {
        let dir = tempfile::tempdir().unwrap();
        let keys_dir = dir.path().join("keys");
        fs::create_dir_all(&keys_dir).unwrap();

        let realm = RealmId::new();
        let legacy_file = keys_dir.join(format!("{realm}.key"));
        let seed = [42u8; 32];
        let original_key = SigningKey::from_bytes(&seed);
        fs::write(&legacy_file, B64.encode(seed)).unwrap();
        assert!(legacy_file.exists());

        let store = ProtectedKeyStore::new(&keys_dir);

        // Load triggers migration
        let loaded = store.load(&realm).unwrap().expect("migrated key must load");
        assert_eq!(loaded.to_bytes(), original_key.to_bytes());

        // Legacy file must be cleanly removed after successful migration
        assert!(
            !legacy_file.exists(),
            "plaintext legacy key must be removed after migration"
        );

        // Subsequent load reads from protected store
        let reloaded = store
            .load(&realm)
            .unwrap()
            .expect("key must stay available in protected store");
        assert_eq!(reloaded.to_bytes(), original_key.to_bytes());
    }

    #[test]
    fn a_damaged_legacy_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProtectedKeyStore::new(dir.path());
        let realm = RealmId::new();
        fs::write(dir.path().join(format!("{realm}.key")), "garbage").unwrap();
        assert!(store.load(&realm).is_err());
        assert!(
            store.create(&realm).is_err(),
            "the broken file is not silently replaced"
        );
    }

    #[test]
    fn interrupted_migration_leaves_recoverable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let keys_dir = dir.path().join("keys");
        fs::create_dir_all(&keys_dir).unwrap();

        let realm = RealmId::new();
        let legacy_file = keys_dir.join(format!("{realm}.key"));
        let seed = [7u8; 32];

        // Simulate both legacy file and already written protected secret (e.g. crash right before legacy removal)
        let store = ProtectedKeyStore::new(&keys_dir);
        let key = store.create(&realm).unwrap();
        fs::write(&legacy_file, B64.encode(seed)).unwrap();

        // Load should succeed from protected store and clean up the orphan legacy file
        let loaded = store.load(&realm).unwrap().unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
        assert!(
            !legacy_file.exists(),
            "orphan legacy file cleaned up on next successful load"
        );
    }
}
