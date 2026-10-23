//! AzerothCore's SRP6 account verifier (salt + verifier), so the Manager can create the service account it
//! needs for the server console without ever knowing a shared default password.

use num_bigint::BigUint;
use sha1::{Digest, Sha1};

const N_HEX: &str = "894B645E89E1535BBDAD5B8B290650530801B18EBFBF5E8FAB3C82872A3E9BB7";

fn n() -> BigUint {
    BigUint::parse_bytes(N_HEX.as_bytes(), 16).expect("constant")
}

fn le32(v: &BigUint) -> [u8; 32] {
    let mut b = v.to_bytes_le();
    b.resize(32, 0);
    b.try_into().expect("32 bytes")
}

/// Verifier for `username`/`password` under `salt` (both names are case-folded, as the server does).
pub fn verifier(username: &str, password: &str, salt: &[u8; 32]) -> [u8; 32] {
    let h1 =
        Sha1::digest(format!("{}:{}", username.to_uppercase(), password.to_uppercase()).as_bytes());
    let mut h2 = Sha1::new();
    h2.update(salt);
    h2.update(h1);
    let x = BigUint::from_bytes_le(&h2.finalize());
    le32(&BigUint::from(7u32).modpow(&x, &n()))
}

/// A fresh random salt.
pub fn new_salt() -> [u8; 32] {
    let mut s = [0u8; 32];
    for chunk in s.chunks_mut(16) {
        chunk.copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(s: &str) -> [u8; 32] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    #[test]
    fn matches_a_verifier_created_by_a_real_worldserver() {
        // Account created through the server's own `account create` command (RA) on a disposable fixture.
        let salt = hex32("54A0DE1E331BB7D2E3964389DB5A443115ED3619BED86B501157DA93C4FC2CF6");
        let expected = hex32("7C39E59C220E8D62CE4CAAB110D4BE8C2381B62E95825B0DC1A4640E57233A46");
        assert_eq!(verifier("ManagerTest1", "hunter22", &salt), expected);
        assert_eq!(
            verifier("MANAGERTEST1", "HUNTER22", &salt),
            expected,
            "case-insensitive like the server"
        );
        assert_ne!(verifier("ManagerTest1", "hunter23", &salt), expected);
    }

    #[test]
    fn salts_are_random() {
        assert_ne!(new_salt(), new_salt());
    }
}
