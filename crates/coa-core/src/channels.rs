//! Signed channel pointers select one immutable package for an entire transaction.
use crate::pkgsource::{fetch_small, Source};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

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
    verify_with(bytes, channel, key)
}

/// A pointer must carry its own channel key's signature, never the other one.
fn verify_with(bytes: &[u8], channel: &str, key: &str) -> Result<Pointer> {
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    crate::signing::verify_exact(envelope.payload.as_bytes(), &envelope.signature, key)?;
    let pointer: Pointer = serde_json::from_str(&envelope.payload)
        .map_err(|e| Error::InvalidManifest(e.to_string()))?;
    if pointer.schema != 1
        || pointer.channel != channel
        || pointer.release_tag != release_tag(channel, &pointer.version)
        || pointer.version.is_empty()
        || !pointer
            .version
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.')
        || pointer.snapshot.len() != 64
        || !pointer.snapshot.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::InvalidManifest("Invalid channel pointer".into()));
    }
    Ok(pointer)
}

/// The release a channel's packages are published under: `server-<v>`, or `preview-server-<v>` for the
/// Modules Preview (pre-releases that never become the repository's latest release).
pub fn release_tag(channel: &str, version: &str) -> String {
    if channel == "preview" {
        format!("preview-server-{version}")
    } else {
        format!("server-{version}")
    }
}

/// Where a channel's pointer lives and which key signs it. The preview channel has its own branch and
/// key, so publishing it can never move a stable or edge pointer.
fn channel_home(channel: &str) -> Result<(&'static str, &'static str)> {
    match channel {
        "stable" | "edge" => Ok(("channels", crate::signing::EMBEDDED_PUBLIC_KEY)),
        "preview" => Ok(("preview-channels", crate::signing::PREVIEW_PUBLIC_KEY)),
        _ => Err(Error::InvalidManifest("Unknown release channel".into())),
    }
}

pub fn resolve(channel: &str) -> Result<Source> {
    let (branch, key) = channel_home(channel)?;
    let source = Source::Url(format!("https://raw.githubusercontent.com/Corfirean/coa-server-build/{branch}"));
    let bytes = fetch_small(&source, &format!("{channel}.json"))?;
    let pointer = verify_with(&bytes, channel, key)?;
    Ok(Source::Url(format!(
        "https://github.com/Corfirean/coa-server-build/releases/download/{}",
        pointer.release_tag
    )))
}

pub fn sign(pointer: &Pointer, seed: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = STANDARD
        .decode(seed.trim())
        .map_err(|_| Error::Invalid("Invalid signing key".into()))?
        .try_into()
        .map_err(|_| Error::Invalid("Invalid signing key length".into()))?;
    let payload = serde_json::to_string(pointer).map_err(|e| Error::Invalid(e.to_string()))?;
    let signature = STANDARD.encode(
        SigningKey::from_bytes(&seed)
            .sign(payload.as_bytes())
            .to_bytes(),
    );
    let bytes = serde_json::to_vec(&Envelope { payload, signature })
        .map_err(|e| Error::Invalid(e.to_string()))?;
    verify(
        &bytes,
        &pointer.channel,
        channel_home(&pointer.channel)?.1,
    )?;
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
        (
            serde_json::to_vec(&Envelope { payload, signature }).unwrap(),
            STANDARD.encode(sk.verifying_key().to_bytes()),
        )
    }

    #[test]
    fn verifies_identity_signature_and_immutable_tag() {
        let mut p = Pointer {
            schema: 1,
            channel: "stable".into(),
            version: "0.261010.40".into(),
            release_tag: "server-0.261010.40".into(),
            snapshot: "a".repeat(64),
        };
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

    #[test]
    fn preview_pointers_name_preview_releases_and_live_apart_from_stable() {
        let p = Pointer {
            schema: 1,
            channel: "preview".into(),
            version: "0.261105.1".into(),
            release_tag: "preview-server-0.261105.1".into(),
            snapshot: "b".repeat(64),
        };
        let (bytes, key) = signed(&p);
        assert_eq!(verify(&bytes, "preview", &key).unwrap().release_tag, "preview-server-0.261105.1");
        // A preview pointer is not a stable one, and a stable-style tag is not a preview release.
        assert!(verify(&bytes, "stable", &key).is_err());
        let stable_tag = Pointer { release_tag: "server-0.261105.1".into(), ..p };
        let (bytes, key) = signed(&stable_tag);
        assert!(verify(&bytes, "preview", &key).is_err());
        assert_eq!(channel_home("preview").unwrap().0, "preview-channels");
        assert_eq!(channel_home("stable").unwrap().0, "channels");
        assert!(channel_home("nightly").is_err());
    }

    #[test]
    fn each_channel_pointer_needs_its_own_key() {
        // A pointer signed by some other key (here: not the embedded stable key) fails under the stable key
        // even though the app also trusts the preview key for packages.
        let p = Pointer {
            schema: 1,
            channel: "stable".into(),
            version: "0.261105.2".into(),
            release_tag: "server-0.261105.2".into(),
            snapshot: "c".repeat(64),
        };
        let (bytes, _) = signed(&p);
        assert!(verify_with(&bytes, "stable", crate::signing::EMBEDDED_PUBLIC_KEY).is_err());
        assert!(verify_with(&bytes, "preview", crate::signing::PREVIEW_PUBLIC_KEY).is_err());
    }
}
