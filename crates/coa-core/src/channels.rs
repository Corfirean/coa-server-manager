//! Signed channel pointers select one immutable package for an entire transaction.
use serde::{Deserialize, Serialize};
use crate::{Error, Result};
use crate::pkgsource::{fetch_small, Source};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pointer {
    pub schema: u32,
    pub channel: String,
    pub version: String,
    pub release_tag: String,
    pub snapshot: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
}

pub fn verify(bytes: &[u8], channel: &str, key: &str) -> Result<Pointer> {
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    crate::signing::verify(envelope.payload.as_bytes(), &envelope.signature, key)?;
    let pointer: Pointer = serde_json::from_str(&envelope.payload).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    if pointer.schema != 1 || pointer.channel != channel
        || pointer.release_tag != format!("server-{}", pointer.version)
        || pointer.version.is_empty()
        || !pointer.version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        || pointer.snapshot.len() != 64
        || !pointer.snapshot.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidManifest("Invalid channel pointer".into()));
    }
    Ok(pointer)
}

pub fn resolve(channel: &str) -> Result<Source> {
    if channel != "stable" && channel != "edge" {
        return Err(Error::InvalidManifest("Unknown release channel".into()));
    }
    let source = Source::Url("https://raw.githubusercontent.com/Corfirean/coa-server-build/channels".into());
    let bytes = fetch_small(&source, &format!("{channel}.json"))?;
    let pointer = verify(&bytes, channel, crate::signing::EMBEDDED_PUBLIC_KEY)?;
    Ok(Source::Url(format!("https://github.com/Corfirean/coa-server-build/releases/download/{}", pointer.release_tag)))
}

pub fn sign(pointer: &Pointer, seed: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = STANDARD.decode(seed.trim()).map_err(|_| Error::Invalid("Invalid signing key".into()))?
        .try_into().map_err(|_| Error::Invalid("Invalid signing key length".into()))?;
    let payload = serde_json::to_string(pointer).map_err(|e| Error::Invalid(e.to_string()))?;
    let signature = STANDARD.encode(SigningKey::from_bytes(&seed).sign(payload.as_bytes()).to_bytes());
    let bytes = serde_json::to_vec(&Envelope { payload, signature }).map_err(|e| Error::Invalid(e.to_string()))?;
    verify(&bytes, &pointer.channel, crate::signing::EMBEDDED_PUBLIC_KEY)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(pointer: &Pointer) -> (Vec<u8>, String) {
        let sk = SigningKey::from_bytes(&[17; 32]);
        let payload = serde_json::to_string(pointer).unwrap();
        let signature = STANDARD.encode(sk.sign(payload.as_bytes()).to_bytes());
        (serde_json::to_vec(&Envelope { payload, signature }).unwrap(), STANDARD.encode(sk.verifying_key().to_bytes()))
    }

    #[test]
    fn verifies_identity_signature_and_immutable_tag() {
        let mut p = Pointer { schema: 1, channel: "stable".into(), version: "0.261010.40".into(), release_tag: "server-0.261010.40".into(), snapshot: "a".repeat(64) };
        let (bytes, key) = signed(&p);
        assert_eq!(verify(&bytes, "stable", &key).unwrap().version, p.version);
        assert!(verify(&bytes, "edge", &key).is_err());
        let mut changed: Envelope = serde_json::from_slice(&bytes).unwrap();
        changed.payload = changed.payload.replace("40", "41");
        assert!(verify(&serde_json::to_vec(&changed).unwrap(), "stable", &key).is_err());
        p.release_tag = "stable".into();
        let (bytes, key) = signed(&p);
        assert!(verify(&bytes, "stable", &key).is_err());
    }
}
