//! Detached Ed25519 signatures over the exact bytes of `manifest.json` (`manifest.json.sig`, base64).
//! No canonicalisation step exists to get wrong: what is downloaded is what is verified.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::error::{Error, Result};

/// Public key baked into the application (private half lives only with the release maintainer / CI secret).
pub const EMBEDDED_PUBLIC_KEY: &str = include_str!("../../../keys/manifest-signing.pub");

pub fn verify(manifest_bytes: &[u8], sig_b64: &str, public_key_b64: &str) -> Result<()> {
    let bad = |m: &str| Error::InvalidManifest(format!("signature: {m}"));
    let key: [u8; 32] = STANDARD
        .decode(public_key_b64.trim())
        .map_err(|_| bad("public key is not base64"))?
        .try_into()
        .map_err(|_| bad("public key has wrong length"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| bad("public key is invalid"))?;
    let sig: [u8; 64] = STANDARD
        .decode(sig_b64.trim())
        .map_err(|_| bad("not base64"))?
        .try_into()
        .map_err(|_| bad("wrong length"))?;
    key.verify(manifest_bytes, &Signature::from_bytes(&sig))
        .map_err(|_| bad("does not match the manifest"))
}

pub fn verify_embedded(manifest_bytes: &[u8], sig_b64: &str) -> Result<()> {
    verify(manifest_bytes, sig_b64, EMBEDDED_PUBLIC_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn pair() -> (SigningKey, String) {
        let sk = SigningKey::generate(&mut rand_core::OsRng);
        let pk = STANDARD.encode(sk.verifying_key().to_bytes());
        (sk, pk)
    }

    #[test]
    fn accepts_valid_and_rejects_tampered_or_foreign() {
        let (sk, pk) = pair();
        let msg = br#"{"schema":1}"#;
        let sig = STANDARD.encode(sk.sign(msg).to_bytes());
        verify(msg, &sig, &pk).unwrap();
        assert!(
            verify(br#"{"schema":2}"#, &sig, &pk).is_err(),
            "tampered manifest"
        );
        let (_, other_pk) = pair();
        assert!(verify(msg, &sig, &other_pk).is_err(), "different key");
        assert!(verify(msg, "not-base64!!", &pk).is_err());
        assert!(verify(msg, &sig, "AAAA").is_err());
    }

    #[test]
    fn embedded_key_is_a_valid_public_key() {
        let raw = STANDARD.decode(EMBEDDED_PUBLIC_KEY.trim()).unwrap();
        assert_eq!(raw.len(), 32);
        VerifyingKey::from_bytes(&raw.try_into().unwrap()).unwrap();
    }
}
