//! Game Relay protocol (Phase 13):
//! Host-to-Relay multiplexed tunnel protocol, allocation messaging,
//! challenge-response authentication, and WoW 3.3.5 REALM_LIST address rewrite helper.

use coa_registry_proto::RealmId;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use uuid::Uuid;

pub const RELAY_PROTOCOL_VERSION: u32 = 1;

/// Default local targets that the Host opens for game traffic.
pub const LOCAL_AUTH_PORT: u16 = 3724;
pub const LOCAL_WORLD_PORT: u16 = 8085;

/// Predefined targets that the Relay can request over the Host tunnel.
/// Arbitrary destinations are strictly forbidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayTarget {
    Auth,
    World,
}

impl RelayTarget {
    pub fn local_port(&self) -> u16 {
        match self {
            RelayTarget::Auth => std::env::var("COA_OVERRIDE_LOCAL_AUTH_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(LOCAL_AUTH_PORT),
            RelayTarget::World => std::env::var("COA_OVERRIDE_LOCAL_WORLD_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(LOCAL_WORLD_PORT),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayChallenge {
    pub nonce: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostHello {
    pub realm_id: RealmId,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayWelcome {
    pub ok: bool,
    pub message: String,
}

/// Messages multiplexed across the Host <-> Relay tunnel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum TunnelMsg {
    /// Host asks Relay to allocate a game session for a player.
    Allocate {
        request_id: u64,
        player_id: Uuid,
        #[serde(default)]
        expected_client_ip: Option<String>,
    },
    /// Relay responds to Host with allocation details.
    AllocateOk {
        request_id: u64,
        token: String,
        relay_host: String,
        auth_port: u16,
        world_port: u16,
        expires_at: i64,
    },
    AllocateErr {
        request_id: u64,
        error: String,
    },

    /// Relay asks Host to open a local connection to `target` (Auth or World) for an incoming player connection.
    Connect {
        stream_id: u32,
        target: RelayTarget,
        token: String,
    },
    /// Host confirms local connection established.
    ConnectOk {
        stream_id: u32,
    },
    /// Connect failed.
    ConnectErr {
        stream_id: u32,
        error: String,
    },

    /// Stream data chunk (base64 encoded).
    Data {
        stream_id: u32,
        chunk: String,
    },
    /// Stream clean half-close / EOF.
    Close {
        stream_id: u32,
    },
    /// Stream abrupt abort / error.
    Reset {
        stream_id: u32,
    },
    /// Keepalive ping.
    Ping,
    /// Keepalive pong.
    Pong,
}

fn relay_input(realm: &RealmId, nonce: &str) -> Vec<u8> {
    format!(
        "coa-relay-host-v1\n{}\n{realm}\n{nonce}",
        RELAY_PROTOCOL_VERSION
    )
    .into_bytes()
}

/// Create a signature over (realm_id, nonce) using the realm's signing key.
pub fn sign_relay_challenge(key: &SigningKey, realm_id: &RealmId, nonce: &str) -> String {
    let sig: Signature = key.sign(&relay_input(realm_id, nonce));
    hex::encode(sig.to_bytes())
}

/// Verify a signature over (realm_id, nonce) using the realm's verifying key.
pub fn verify_relay_challenge(
    key: &VerifyingKey,
    realm_id: &RealmId,
    nonce: &str,
    sig_hex: &str,
) -> bool {
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&sig_bytes) else {
        return false;
    };
    key.verify_strict(&relay_input(realm_id, nonce), &sig)
        .is_ok()
}

/// Rewrites the realm address in a WoW 3.3.5 REALM_LIST response packet (Opcode 0x10).
///
/// In WoW 3.3.5 client protocol:
/// - Byte 0: 0x10 (REALM_LIST)
/// - Bytes 1..2: uint16 LE payload length (total size after byte 2)
/// - Bytes 3..6: uint32 LE unused (0)
/// - Bytes 7..8: uint16 LE realm count
/// - For each realm:
///   - uint8 type (1 byte)
///   - uint8 lock (1 byte)
///   - uint8 flag (1 byte)
///   - name: null-terminated C string
///   - address: null-terminated C string ("host:port\0")
///   - population: 4 bytes float
///   - characters: 1 byte uint8
///   - timezone: 1 byte uint8
///   - realm_id: 1 byte uint8
///   - if flag & 0x04 (SPECIFYBUILD): 5 bytes build info
/// - Footer: 2 bytes (0x10, 0x00)
///
/// Replaces the address of the first realm (or matches) with `new_host_port` and adjusts
/// the uint16 length at bytes 1..2 by the byte length delta.
pub fn rewrite_realm_list_address(
    pkt: &[u8],
    new_host_port: &str,
) -> Result<Option<Vec<u8>>, String> {
    if pkt.is_empty() || pkt[0] != 0x10 {
        return Ok(None);
    }
    if pkt.len() < 9 {
        return Err("packet too short for REALM_LIST header".into());
    }
    let body_len = u16::from_le_bytes([pkt[1], pkt[2]]) as usize;
    if pkt.len() < 3 + body_len {
        return Err("truncated REALM_LIST packet".into());
    }
    let realm_count = u16::from_le_bytes([pkt[7], pkt[8]]) as usize;
    if realm_count == 0 {
        return Ok(Some(pkt.to_vec()));
    }

    let mut pos = 9;
    if pos + 3 > pkt.len() {
        return Err("malformed realm header".into());
    }
    pos += 3; // type, lock, flag

    let name_end = pkt[pos..]
        .iter()
        .position(|&b| b == 0)
        .ok_or("unterminated realm name")?
        + pos;
    pos = name_end + 1;

    let addr_start = pos;
    let addr_end = pkt[pos..]
        .iter()
        .position(|&b| b == 0)
        .ok_or("unterminated realm address")?
        + pos;
    let old_addr = &pkt[addr_start..addr_end];

    if new_host_port.len() > 255 {
        return Err("new host:port exceeds maximum address length".into());
    }
    let diff = new_host_port.len() as isize - old_addr.len() as isize;
    let new_body_len_isize = body_len as isize + diff;
    if !(0..=u16::MAX as isize).contains(&new_body_len_isize) {
        return Err("adjusted body length out of bounds".into());
    }
    let new_body_len = new_body_len_isize as u16;

    let mut out = Vec::with_capacity(pkt.len() + new_host_port.len() + 1);
    out.extend_from_slice(&pkt[..addr_start]);
    out.extend_from_slice(new_host_port.as_bytes());
    out.extend_from_slice(&pkt[addr_end..]);
    out[1..3].copy_from_slice(&new_body_len.to_le_bytes());

    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_signature_roundtrip() {
        let seed = [42u8; 32];
        let key = SigningKey::from_bytes(&seed);

        let realm = RealmId::new();
        let nonce = "test-challenge-nonce-123";

        let sig = sign_relay_challenge(&key, &realm, nonce);
        assert!(verify_relay_challenge(
            &key.verifying_key(),
            &realm,
            nonce,
            &sig
        ));

        // Wrong nonce
        assert!(!verify_relay_challenge(
            &key.verifying_key(),
            &realm,
            "other-nonce",
            &sig
        ));
        // Wrong realm
        assert!(!verify_relay_challenge(
            &key.verifying_key(),
            &RealmId::new(),
            nonce,
            &sig
        ));
    }

    #[test]
    fn rewrite_realm_list_address_works() {
        // Build a mock REALM_LIST packet for WoW 3.3.5
        let mut pkt = Vec::new();
        pkt.push(0x10); // Opcode
        pkt.extend_from_slice(&[0, 0]); // Placeholder for body_len
        pkt.extend_from_slice(&[0, 0, 0, 0]); // Unused uint32
        pkt.extend_from_slice(&1u16.to_le_bytes()); // Realm count = 1

        // Realm 0
        pkt.push(1); // Type
        pkt.push(0); // Lock
        pkt.push(0); // Flags
        pkt.extend_from_slice(b"CoA Test Realm\0"); // Name
        pkt.extend_from_slice(b"127.0.0.1:8085\0"); // Old Address
        pkt.extend_from_slice(&0.5f32.to_le_bytes()); // Population
        pkt.push(1); // Characters
        pkt.push(1); // Timezone
        pkt.push(1); // Realm ID

        // Footer
        pkt.push(0x10);
        pkt.push(0x00);

        let body_len = (pkt.len() - 3) as u16;
        pkt[1..3].copy_from_slice(&body_len.to_le_bytes());

        // Rewrite
        let new_target = "coa-manager.duckdns.org:40005";
        let res = rewrite_realm_list_address(&pkt, new_target)
            .unwrap()
            .expect("rewritten");

        assert_eq!(res[0], 0x10);
        let new_len = u16::from_le_bytes([res[1], res[2]]) as usize;
        assert_eq!(res.len(), 3 + new_len);

        // Address in res must be new_target
        let old_addr = b"127.0.0.1:8085";
        let new_addr = b"coa-manager.duckdns.org:40005";
        assert!(!res.windows(old_addr.len()).any(|w| w == old_addr));
        assert!(res.windows(new_addr.len()).any(|w| w == new_addr));
    }

    #[test]
    fn non_realm_list_ignored() {
        let pkt = vec![0x00, 0x01, 0x02];
        assert_eq!(rewrite_realm_list_address(&pkt, "test:8085"), Ok(None));
    }
}
