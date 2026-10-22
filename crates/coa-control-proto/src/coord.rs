//! The Coordinator's own protocol. Text frames are JSON control messages; binary frames carry one end-to-end frame for one connection, prefixed with the
//! connection number. The Coordinator routes by connection number and never looks inside.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use coa_registry_proto::RealmId;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{invalid, ControlError, Result};

/// The largest end-to-end frame the Coordinator forwards (a Noise message with its tag fits in 64 KiB; chunks are much smaller).
pub const MAX_FRAME_BYTES: usize = 20 * 1024;
/// Bytes of the connection number in front of a binary frame.
pub const CONN_PREFIX: usize = 4;
pub const MAX_CONTROL_TEXT_BYTES: usize = 2048;
pub const HELLO_TIMEOUT_SECS: u64 = 10;
/// A Hello's timestamp must be this close to the Coordinator's clock.
pub const MAX_HELLO_SKEW_SECS: i64 = 120;

/// Hard limits of the Coordinator (the defaults of its configuration).
pub mod limits {
    /// Simultaneous Player connections routed to one Host.
    pub const PER_HOST_CONNECTIONS: usize = 16;
    /// Simultaneous connections from one address.
    pub const PER_IP_CONNECTIONS: usize = 8;
    /// New Player connections per minute per realm, and per address.
    pub const PER_REALM_PER_MINUTE: u32 = 60;
    pub const PER_IP_PER_MINUTE: u32 = 30;
    /// Bytes through one connection, in total.
    pub const BYTES_PER_CONNECTION: usize = 16 * 1024 * 1024;
    /// Seconds a connection may live, and may be silent.
    pub const CONNECTION_LIFETIME_SECS: u64 = 600;
    pub const IDLE_SECS: u64 = 60;
    /// Seconds the Host has to answer a new connection (the first frame back).
    pub const HOST_ANSWER_SECS: u64 = 10;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    /// Coordinator -> anyone who connected: prove who you are with this.
    Challenge {
        protocol: u32,
        nonce: String,
    },
    /// Host -> Coordinator.
    HostHello {
        realm_id: RealmId,
        ts: i64,
        sig: String,
    },
    /// Player -> Coordinator (the realm it wants is in the URL and is signed).
    PlayerHello {
        player_id: Uuid,
        public_key: String,
        realm_id: RealmId,
        ts: i64,
        sig: String,
    },
    /// Coordinator -> Host: the Host is registered and these are the limits.
    HostReady {
        max_connections: u32,
        max_frame: u32,
    },
    /// Coordinator -> Host: a Player wants to talk; its frames will carry this number.
    Open {
        conn: u32,
        #[serde(default)]
        client_ip: Option<String>,
    },
    /// Either side of a connection ends it.
    Close {
        conn: u32,
        reason: Option<String>,
    },
    /// Coordinator -> Player: the Host accepted the connection; binary frames flow now.
    PlayerReady {
        conn: u32,
        max_frame: u32,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadHello,
    UnknownRealm,
    HostOffline,
    HostBusy,
    RateLimited,
    TooLarge,
    Timeout,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbePayload {
    pub ports: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResponse {
    pub client_ip: String,
    pub results: std::collections::HashMap<u16, bool>,
    pub all_reachable: bool,
}

pub fn challenge_nonce() -> String {
    let mut n = [0u8; 32];
    n[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    n[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    B64.encode(n)
}

fn host_input(realm: &RealmId, nonce: &str, ts: i64) -> Vec<u8> {
    format!(
        "coa-coord-host-v1\n{}\n{realm}\n{nonce}\n{ts}",
        crate::CONTROL_PROTOCOL_VERSION
    )
    .into_bytes()
}

fn player_input(player: &Uuid, public_key: &str, realm: &RealmId, nonce: &str, ts: i64) -> Vec<u8> {
    format!(
        "coa-coord-player-v1\n{}\n{}\n{public_key}\n{realm}\n{nonce}\n{ts}",
        crate::CONTROL_PROTOCOL_VERSION,
        player.hyphenated()
    )
    .into_bytes()
}

pub fn sign_host_hello(key: &SigningKey, realm: &RealmId, nonce: &str, ts: i64) -> Frame {
    Frame::HostHello {
        realm_id: *realm,
        ts,
        sig: B64.encode(key.sign(&host_input(realm, nonce, ts)).to_bytes()),
    }
}

pub fn sign_player_hello(
    key: &SigningKey,
    player: &Uuid,
    realm: &RealmId,
    nonce: &str,
    ts: i64,
) -> Frame {
    let public_key = B64.encode(key.verifying_key().as_bytes());
    let sig = B64.encode(
        key.sign(&player_input(player, &public_key, realm, nonce, ts))
            .to_bytes(),
    );
    Frame::PlayerHello {
        player_id: *player,
        public_key,
        realm_id: *realm,
        ts,
        sig,
    }
}

fn decode_sig(sig: &str) -> Result<Signature> {
    if sig.len() != 86 {
        return invalid("the signature has the wrong length");
    }
    let bytes: [u8; 64] = B64
        .decode(sig)
        .map_err(|_| ControlError::Invalid("the signature is not base64url".into()))?
        .try_into()
        .map_err(|_| ControlError::Invalid("the signature is not 64 bytes".into()))?;
    Ok(Signature::from_bytes(&bytes))
}

/// A Host's hello checked against the key the Registry holds for the realm.
pub fn verify_host_hello(
    frame: &Frame,
    realm_key: &VerifyingKey,
    expect_realm: Option<&RealmId>,
    nonce: &str,
    now: i64,
) -> Result<RealmId> {
    let Frame::HostHello { realm_id, ts, sig } = frame else {
        return Err(ControlError::Auth("not a host hello".into()));
    };
    if expect_realm.is_some_and(|r| r != realm_id) {
        return Err(ControlError::Auth("another realm".into()));
    }
    if (ts - now).abs() > MAX_HELLO_SKEW_SECS {
        return Err(ControlError::Auth(
            "the timestamp is outside the allowed skew".into(),
        ));
    }
    realm_key
        .verify_strict(&host_input(realm_id, nonce, *ts), &decode_sig(sig)?)
        .map_err(|_| ControlError::Auth("the signature does not match".into()))?;
    Ok(*realm_id)
}

/// A Player's hello checked against the key it presents (proof of possession; the Host decides whether it is a known player).
pub fn verify_player_hello(
    frame: &Frame,
    nonce: &str,
    now: i64,
) -> Result<(Uuid, RealmId, VerifyingKey)> {
    let Frame::PlayerHello {
        player_id,
        public_key,
        realm_id,
        ts,
        sig,
    } = frame
    else {
        return Err(ControlError::Auth("not a player hello".into()));
    };
    if (ts - now).abs() > MAX_HELLO_SKEW_SECS {
        return Err(ControlError::Auth(
            "the timestamp is outside the allowed skew".into(),
        ));
    }
    let key = decode_public_key(public_key)?;
    key.verify_strict(
        &player_input(player_id, public_key, realm_id, nonce, *ts),
        &decode_sig(sig)?,
    )
    .map_err(|_| ControlError::Auth("the signature does not match".into()))?;
    Ok((*player_id, *realm_id, key))
}

pub fn decode_public_key(text: &str) -> Result<VerifyingKey> {
    coa_registry_proto::sign::decode_public_key(text)
        .map_err(|e| ControlError::Invalid(e.to_string()))
}

pub fn encode_public_key(key: &VerifyingKey) -> String {
    coa_registry_proto::sign::encode_public_key(key)
}

/// `[connection number][end-to-end frame]`
pub fn binary(conn: u32, frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CONN_PREFIX + frame.len());
    out.extend_from_slice(&conn.to_be_bytes());
    out.extend_from_slice(frame);
    out
}

pub fn split_binary(data: &[u8]) -> Result<(u32, &[u8])> {
    if data.len() < CONN_PREFIX || data.len() > CONN_PREFIX + MAX_FRAME_BYTES {
        return Err(ControlError::Limit(
            "a frame is too short or too long".into(),
        ));
    }
    Ok((
        u32::from_be_bytes(data[..CONN_PREFIX].try_into().expect("4 bytes")),
        &data[CONN_PREFIX..],
    ))
}

pub fn parse_text(text: &str) -> Result<Frame> {
    if text.len() > MAX_CONTROL_TEXT_BYTES {
        return Err(ControlError::Limit("a control message is too long".into()));
    }
    serde_json::from_str(text)
        .map_err(|_| ControlError::Invalid("a control message is not valid".into()))
}

pub fn to_text(frame: &Frame) -> String {
    serde_json::to_string(frame).expect("a frame serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    #[test]
    fn a_host_proves_its_realm_key_and_only_that() {
        let realm = RealmId::new();
        let nonce = challenge_nonce();
        let hello = sign_host_hello(&key(1), &realm, &nonce, 1000);
        assert_eq!(
            verify_host_hello(&hello, &key(1).verifying_key(), Some(&realm), &nonce, 1010).unwrap(),
            realm
        );
        assert!(
            verify_host_hello(&hello, &key(2).verifying_key(), Some(&realm), &nonce, 1010).is_err(),
            "another key"
        );
        assert!(
            verify_host_hello(
                &hello,
                &key(1).verifying_key(),
                Some(&RealmId::new()),
                &nonce,
                1010
            )
            .is_err(),
            "another realm"
        );
        assert!(
            verify_host_hello(
                &hello,
                &key(1).verifying_key(),
                None,
                &challenge_nonce(),
                1010
            )
            .is_err(),
            "another challenge: no replay"
        );
        assert!(
            verify_host_hello(&hello, &key(1).verifying_key(), None, &nonce, 5000).is_err(),
            "too old"
        );
    }

    #[test]
    fn a_player_proves_possession_of_its_key() {
        let (player, realm, nonce) = (Uuid::now_v7(), RealmId::new(), challenge_nonce());
        let hello = sign_player_hello(&key(3), &player, &realm, &nonce, 50);
        let (p, r, k) = verify_player_hello(&hello, &nonce, 55).unwrap();
        assert_eq!(
            (p, r, k.to_bytes()),
            (player, realm, key(3).verifying_key().to_bytes())
        );
        let Frame::PlayerHello {
            player_id,
            public_key,
            realm_id,
            ts,
            sig,
        } = hello.clone()
        else {
            unreachable!()
        };
        let other_key = encode_public_key(&key(4).verifying_key());
        assert!(
            verify_player_hello(
                &Frame::PlayerHello {
                    player_id,
                    public_key: other_key,
                    realm_id,
                    ts,
                    sig: sig.clone()
                },
                &nonce,
                55
            )
            .is_err(),
            "a key that did not sign"
        );
        assert!(
            verify_player_hello(
                &Frame::PlayerHello {
                    player_id: Uuid::now_v7(),
                    public_key,
                    realm_id,
                    ts,
                    sig
                },
                &nonce,
                55
            )
            .is_err(),
            "another player id"
        );
    }

    #[test]
    fn frames_are_strict_and_bounded() {
        assert!(parse_text(r#"{"t":"close","conn":1,"reason":null,"extra":1}"#).is_err());
        assert!(parse_text(&"x".repeat(5000)).is_err());
        assert!(parse_text(r#"{"t":"open","conn":7}"#).is_ok());
        let b = binary(9, b"abc");
        assert_eq!(split_binary(&b).unwrap(), (9, b"abc".as_slice()));
        assert!(split_binary(&[0, 0]).is_err());
        assert!(split_binary(&vec![0u8; CONN_PREFIX + MAX_FRAME_BYTES + 1]).is_err());
    }
}
