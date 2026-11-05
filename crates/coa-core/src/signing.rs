//! Detached Ed25519 signatures over the exact bytes of `manifest.json` (`manifest.json.sig`, base64).
//! No canonicalisation step exists to get wrong: what is downloaded is what is verified.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::error::{Error, Result};

/// Public key baked into the application (private half lives only with the release maintainer / CI secret).
pub const EMBEDDED_PUBLIC_KEY: &str = include_str!("../../../keys/manifest-signing.pub");

/// The Modules Preview's own key: preview packages and the preview channel are signed with it, so a
/// stable Manager (which knows only the key above) never accepts them. The preview Manager trusts both,
/// because it starts from stable servers and stable packages.
pub const PREVIEW_PUBLIC_KEY: &str = include_str!("../../../keys/preview-signing.pub");

/// Checks `sig_b64` against `public_key_b64`; the embedded key stands for both keys this app trusts.
pub fn verify(manifest_bytes: &[u8], sig_b64: &str, public_key_b64: &str) -> Result<()> {
    let first = verify_exact(manifest_bytes, sig_b64, public_key_b64);
    if first.is_err() && public_key_b64.trim() == EMBEDDED_PUBLIC_KEY.trim() {
        return verify_exact(manifest_bytes, sig_b64, PREVIEW_PUBLIC_KEY).map_err(|_| first.unwrap_err());
    }
    first
}

/// The public half (base64) of a base64 Ed25519 seed.
pub fn public_key_of(seed_b64: &str) -> Result<String> {
    let seed: [u8; 32] = STANDARD
        .decode(seed_b64.trim())
        .map_err(|_| Error::Invalid("signing key is not base64".into()))?
        .try_into()
        .map_err(|_| Error::Invalid("signing key has the wrong length".into()))?;
    Ok(STANDARD.encode(ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes()))
}

pub fn verify_exact(manifest_bytes: &[u8], sig_b64: &str, public_key_b64: &str) -> Result<()> {
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
