//! Signed requests. One canonical input, one algorithm (Ed25519); see `docs/REGISTRY_PROTOCOL.md` for the format and test vectors.
//!
//! ```text
//! coa-registry-sig-v1
//! <protocol version>        decimal
//! <HTTP method>             upper case
//! <request path>            exactly as sent, no query
//! <realm id>                lower-case hyphenated UUID
//! <unix timestamp>          decimal seconds
//! <hex SHA-256 of the body> lower case; the empty body for a GET
//! ```
//!
//! The lines are joined with `\n` (no trailing newline) and the UTF-8 bytes are signed. The signature travels base64url (no padding).

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{invalid, Result, REGISTRY_PROTOCOL_VERSION};

pub const SIGNATURE_DOMAIN: &str = "coa-registry-sig-v1";

pub const HEADER_VERSION: &str = "x-coa-registry-version";
pub const HEADER_REALM: &str = "x-coa-realm";
pub const HEADER_TIMESTAMP: &str = "x-coa-timestamp";
pub const HEADER_SIGNATURE: &str = "x-coa-signature";

/// A realm's persistent identity: a UUIDv7, in its one canonical spelling.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RealmId(Uuid);

impl RealmId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Only the canonical form (lower case, hyphenated) of a version-7 UUID is accepted, so one id has one spelling in the signed input.
    pub fn parse(text: &str) -> Result<Self> {
        let Ok(uuid) = Uuid::parse_str(text) else { return invalid("the realm id is not a UUID") };
        if uuid.get_version_num() != 7 || uuid.get_variant() != uuid::Variant::RFC4122 {
            return invalid("the realm id is not a version-7 UUID");
        }
        if uuid.hyphenated().to_string() != text {
            return invalid("the realm id is not in canonical form");
        }
        Ok(Self(uuid))
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    pub fn from_uuid(uuid: Uuid) -> Result<Self> {
        Self::parse(&uuid.hyphenated().to_string())
    }
}

impl Default for RealmId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RealmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl std::fmt::Debug for RealmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RealmId({self})")
    }
}

impl serde::Serialize for RealmId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for RealmId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// The exact bytes that are signed.
pub fn signature_input(method: &str, path: &str, realm: &RealmId, timestamp: i64, body: &[u8]) -> Vec<u8> {
    format!("{SIGNATURE_DOMAIN}\n{REGISTRY_PROTOCOL_VERSION}\n{}\n{path}\n{realm}\n{timestamp}\n{}", method.to_ascii_uppercase(), hex::encode(Sha256::digest(body))).into_bytes()
}

/// The four headers of a signed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedHeaders {
    pub version: u32,
    pub realm: RealmId,
    pub timestamp: i64,
    pub signature: [u8; 64],
}

impl SignedHeaders {
    pub fn pairs(&self) -> [(&'static str, String); 4] {
        [
            (HEADER_VERSION, self.version.to_string()),
            (HEADER_REALM, self.realm.to_string()),
            (HEADER_TIMESTAMP, self.timestamp.to_string()),
            (HEADER_SIGNATURE, B64.encode(self.signature)),
        ]
    }

    /// Read the headers from whatever the transport offers. A missing or malformed header is an error; the protocol version is checked by the caller.
    pub fn parse<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Result<Self> {
        let need = |name: &str| get(name).ok_or_else(|| crate::ProtoError::Invalid(format!("the {name} header is missing")));
        let version: u32 = need(HEADER_VERSION)?.parse().map_err(|_| crate::ProtoError::Invalid("the protocol version header is not a number".into()))?;
        let realm = RealmId::parse(need(HEADER_REALM)?)?;
        let timestamp: i64 = need(HEADER_TIMESTAMP)?.parse().map_err(|_| crate::ProtoError::Invalid("the timestamp header is not a number".into()))?;
        let sig = need(HEADER_SIGNATURE)?;
        if sig.len() != 86 {
            return invalid("the signature header has the wrong length");
        }
        let bytes = B64.decode(sig).map_err(|_| crate::ProtoError::Invalid("the signature header is not base64url".into()))?;
        let signature: [u8; 64] = bytes.try_into().map_err(|_| crate::ProtoError::Invalid("the signature is not 64 bytes".into()))?;
        Ok(Self { version, realm, timestamp, signature })
    }
}

/// Sign a request. The caller chooses the timestamp (it must grow with every request of one realm).
pub fn sign_request(key: &SigningKey, method: &str, path: &str, realm: &RealmId, timestamp: i64, body: &[u8]) -> SignedHeaders {
    let signature = key.sign(&signature_input(method, path, realm, timestamp, body)).to_bytes();
    SignedHeaders { version: REGISTRY_PROTOCOL_VERSION, realm: *realm, timestamp, signature }
}

/// Verify a request against a public key. Strict: a signature that merely validates under a weak key or a non-canonical encoding is refused.
pub fn verify_request(public: &VerifyingKey, headers: &SignedHeaders, method: &str, path: &str, body: &[u8]) -> bool {
    let input = signature_input(method, path, &headers.realm, headers.timestamp, body);
    public.verify_strict(&input, &Signature::from_bytes(&headers.signature)).is_ok()
}

pub fn encode_public_key(key: &VerifyingKey) -> String {
    B64.encode(key.as_bytes())
}

pub fn decode_public_key(text: &str) -> Result<VerifyingKey> {
    if text.len() != 43 {
        return invalid("the public key has the wrong length");
    }
    let bytes = B64.decode(text).map_err(|_| crate::ProtoError::Invalid("the public key is not base64url".into()))?;
    let array: [u8; 32] = bytes.try_into().map_err(|_| crate::ProtoError::Invalid("the public key is not 32 bytes".into()))?;
    let key = VerifyingKey::from_bytes(&array).map_err(|_| crate::ProtoError::Invalid("the public key is not a valid Ed25519 key".into()))?;
    if key.is_weak() {
        return invalid("the public key is a weak key");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REALM: &str = "018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn realm_ids_have_one_spelling() {
        assert!(RealmId::parse(REALM).is_ok());
        assert!(RealmId::parse(&REALM.to_uppercase()).is_err(), "upper case is another spelling");
        assert!(RealmId::parse(&REALM.replace('-', "")).is_err(), "so is the simple form");
        assert!(RealmId::parse("018f2d9e-5c3a-4b21-8c4d-0e5f6a7b8c9d").is_err(), "version 4 is not an identity");
        assert!(RealmId::parse("018f2d9e-5c3a-7b21-0c4d-0e5f6a7b8c9d").is_err(), "nor is a foreign variant");
        assert!(RealmId::parse("../../etc").is_err());
        assert!(RealmId::parse("").is_err());
        assert_eq!(RealmId::new().to_string().len(), 36);
    }

    /// The deterministic vector documented in `docs/REGISTRY_PROTOCOL.md`: Ed25519 signatures are deterministic, so this never changes.
    #[test]
    fn the_documented_test_vector() {
        let realm = RealmId::parse(REALM).unwrap();
        let key = key();
        assert_eq!(encode_public_key(&key.verifying_key()), VECTOR_PUBLIC_KEY);
        let body = br#"{"protocol_version":1}"#;
        let input = signature_input("post", "/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat", &realm, 1_790_000_000, body);
        assert_eq!(String::from_utf8(input).unwrap(), VECTOR_INPUT);
        let headers = sign_request(&key, "POST", "/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat", &realm, 1_790_000_000, body);
        assert_eq!(B64.encode(headers.signature), VECTOR_SIGNATURE);
        assert!(verify_request(&key.verifying_key(), &headers, "POST", "/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat", body));
    }

    /// The second documented vector: a GET has the empty body, whose SHA-256 is `e3b0c442...`.
    #[test]
    fn the_documented_vector_of_a_read() {
        let realm = RealmId::parse(REALM).unwrap();
        let headers = sign_request(&key(), "GET", "/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d", &realm, 1_790_000_030, b"");
        assert_eq!(B64.encode(headers.signature), "38nHMS6yvl8yrTbLmG1w5LiLVkvTaiRdQPrgT2GSvtR8jW_W0z50IRxD_kInBYP5Yl1Qwwm-r6BVDdTR5ts-BQ");
    }

    const VECTOR_PUBLIC_KEY: &str = "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw";
    const VECTOR_INPUT: &str = "coa-registry-sig-v1
1
POST
/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat
018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d
1790000000
00aa4c2c857995eb8e19cb0fade07e4b49aac774e37a99c37a0c9f549204d9de";
    const VECTOR_SIGNATURE: &str = "1ys2C7RVjhEtTl0bVrdowpuPELwOnhuiYZKp88dAWdcA3cezP9QuI903OJi2N2mFA9kifblD_fvdT2B_8F5_Dw";

    #[test]
    fn every_part_of_the_input_is_covered() {
        let realm = RealmId::parse(REALM).unwrap();
        let other = RealmId::parse("018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9e").unwrap();
        let key = key();
        let path = "/registry/v1/realms/x";
        let h = sign_request(&key, "POST", path, &realm, 100, b"body");
        let public = key.verifying_key();
        assert!(verify_request(&public, &h, "POST", path, b"body"));
        assert!(!verify_request(&public, &h, "POST", path, b"bodY"), "the body");
        assert!(!verify_request(&public, &h, "POST", "/registry/v1/realms/y", b"body"), "the path");
        assert!(!verify_request(&public, &h, "GET", path, b"body"), "the method");
        assert!(!verify_request(&public, &SignedHeaders { realm: other, ..h.clone() }, "POST", path, b"body"), "the realm");
        assert!(!verify_request(&public, &SignedHeaders { timestamp: 101, ..h.clone() }, "POST", path, b"body"), "the timestamp");
        assert!(!verify_request(&SigningKey::from_bytes(&[8u8; 32]).verifying_key(), &h, "POST", path, b"body"), "the key");
    }

    #[test]
    fn headers_round_trip_and_hostile_ones_are_refused() {
        let realm = RealmId::parse(REALM).unwrap();
        let h = sign_request(&key(), "POST", "/p", &realm, 5, b"");
        let pairs = h.pairs();
        let get = |name: &str| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str());
        assert_eq!(SignedHeaders::parse(get).unwrap(), h);
        assert!(SignedHeaders::parse(|n| if n == HEADER_SIGNATURE { None } else { get(n) }).is_err());
        assert!(SignedHeaders::parse(|n| if n == HEADER_SIGNATURE { Some("AAAA") } else { get(n) }).is_err());
        assert!(SignedHeaders::parse(|n| if n == HEADER_TIMESTAMP { Some("12x") } else { get(n) }).is_err());
        assert!(SignedHeaders::parse(|n| if n == HEADER_REALM { Some("nope") } else { get(n) }).is_err());
    }

    #[test]
    fn public_keys_are_checked() {
        assert!(decode_public_key(&encode_public_key(&key().verifying_key())).is_ok());
        assert!(decode_public_key("short").is_err());
        assert!(decode_public_key(&B64.encode([0u8; 32])).is_err(), "the identity point is a weak key");
        assert!(decode_public_key(&"A".repeat(43)).is_err());
    }
}
