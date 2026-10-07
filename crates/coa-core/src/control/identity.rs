//! The player's identity: a random `PlayerId` and an Ed25519 key, created once and kept in the [`SecretStore`](super::secrets::SecretStore).
//! The id is public (a Host records it); the key never leaves the machine. A key that cannot be read is an error, never silently replaced by a new identity,
//! because the new identity would own none of the player's accounts and characters.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::secrets::SecretStore;
use crate::{Error, Result};

const NAME: &str = "player-identity";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    player_id: Uuid,
    seed: String,
}

#[derive(Clone)]
pub struct PlayerIdentity {
    pub player_id: Uuid,
    pub key: SigningKey,
}

impl std::fmt::Debug for PlayerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PlayerIdentity({})", self.player_id)
    }
}

impl PlayerIdentity {
    /// The stored identity, or a new one when the player has none yet.
    pub fn load_or_create(secrets: &dyn SecretStore) -> Result<PlayerIdentity> {
        if let Some(bytes) = secrets.get(NAME)? {
            let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("the player identity is damaged".into()))?;
            let seed: [u8; 32] = B64.decode(&stored.seed).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| Error::Invalid("the player identity is damaged".into()))?;
            return Ok(PlayerIdentity { player_id: stored.player_id, key: SigningKey::from_bytes(&seed) });
        }
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let id = PlayerIdentity { player_id: Uuid::now_v7(), key: SigningKey::from_bytes(&seed) };
        let stored = Stored { player_id: id.player_id, seed: B64.encode(seed) };
        secrets.put(NAME, &serde_json::to_vec(&stored)?)?;
        Ok(id)
    }

    pub fn public_key(&self) -> String {
        coa_control_proto::coord::encode_public_key(&self.key.verifying_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::secrets::MemoryStore;

    #[test]
    fn the_identity_is_created_once_and_survives() {
        let secrets = MemoryStore::default();
        let a = PlayerIdentity::load_or_create(&secrets).unwrap();
        let b = PlayerIdentity::load_or_create(&secrets).unwrap();
        assert_eq!((a.player_id, a.key.to_bytes()), (b.player_id, b.key.to_bytes()));
        assert_ne!(PlayerIdentity::load_or_create(&MemoryStore::default()).unwrap().player_id, a.player_id);
        assert!(!format!("{a:?}").contains(&B64.encode(a.key.to_bytes())), "the key is not printed");
    }

    #[test]
    fn a_damaged_identity_is_an_error_not_a_new_player() {
        let secrets = MemoryStore::default();
        secrets.put(NAME, b"garbage").unwrap();
        assert!(PlayerIdentity::load_or_create(&secrets).is_err());
        assert_eq!(secrets.get(NAME).unwrap().unwrap(), b"garbage", "nothing was overwritten");
    }
}
