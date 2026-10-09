//! The application messages inside the end-to-end channel. One request, one response, in order. Nothing here is ever visible to the Coordinator.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{invalid, ControlError, Result};

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MIN_USERNAME: usize = 3;
pub const MAX_USERNAME: usize = 16;
pub const MIN_PASSWORD: usize = 8;
pub const MAX_PASSWORD: usize = 16;
pub const MAX_CHARACTERS: usize = 200;

/// A game account name: 3 to 16 ASCII letters, digits or `_`, not beginning or ending with `_`.
pub fn valid_username(s: &str) -> bool {
    (MIN_USERNAME..=MAX_USERNAME).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !s.starts_with('_') && !s.ends_with('_')
}

/// A game account password: 8 to 16 ASCII letters and digits (the game compares passwords case-insensitively, so letters carry no extra strength; digits and length do).
pub fn valid_password(s: &str) -> bool {
    (MIN_PASSWORD..=MAX_PASSWORD).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A password for a realm, 16 characters of `A-Z0-9` from the operating system's randomness (about 82 bits), unique per realm.
pub fn generate_password() -> String {
    const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut out = String::with_capacity(16);
    while out.len() < 16 {
        for b in Uuid::new_v4().as_bytes() {
            // rejection sampling: 252 is the largest multiple of 36 below 256, so every symbol is equally likely
            if *b < 252 && out.len() < 16 {
                out.push(ALPHABET[(*b % 36) as usize] as char);
            }
        }
    }
    out
}

/// The account name wanted, made fit for the game: letters and digits only, upper case, at most 16 characters.
pub fn sanitize_username(wanted: &str) -> String {
    let base: String = wanted.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    let base: String = base.chars().take(MAX_USERNAME).collect();
    if base.len() >= MIN_USERNAME { base } else { format!("{base}PLAYER").chars().take(MAX_USERNAME).collect() }
}

/// The stable alternative of an occupied name: `DMITRY` becomes `DMITRY_7K4M`. The suffix is derived from the player's id, so the same player gets the same alternative.
pub fn alternative_username(base: &str, player: &Uuid, attempt: u32) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes = player.as_bytes();
    let mix = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]).wrapping_add(attempt.wrapping_mul(0x9E37_79B1));
    let suffix: String = (0..4).map(|i| ALPHABET[((mix >> (i * 5)) & 31) as usize] as char).collect();
    let keep = MAX_USERNAME - 5;
    let head: String = base.chars().take(keep).collect();
    format!("{head}_{suffix}")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello { protocol: u32, client: String },
    /// Make the player's account on this realm, or reuse it. `password` is the one the player's Manager keeps for this realm; it travels only inside the channel.
    Provision { desired: Option<String>, password: String, have_credentials: bool },
    ListCharacters,
    Claim { token: u32 },
    ClaimAck { character_id: Uuid },
    /// Link an account the realm already has (one time).
    Link { login: String, password: String },
    Route,
    /// Phase 12.1: Start or resume a remote portable character transfer.
    TransferOffer {
        transfer_id: Uuid,
        character_id: Uuid,
        canonical_revision: u64,
        content_hash: String,
        total_size: usize,
        collections: BTreeMap<String, Vec<u32>>,
    },
    /// A chunk of the encoded portable character snapshot payload.
    TransferChunk {
        transfer_id: Uuid,
        offset: usize,
        data: String,
    },
    /// Query status or resume progress of a transfer.
    TransferStatus {
        transfer_id: Uuid,
    },
    /// Commit the transferred payload into the realm and arm a session.
    TransferCommit {
        transfer_id: Uuid,
    },
    /// Acknowledge successful transfer and session arming.
    TransferAck {
        transfer_id: Uuid,
        character_id: Uuid,
    },
    /// Phase 13: Request a short-lived relay allocation for a relayed JOIN.
    AllocateRelay {
        player_id: Uuid,
    },
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppError {
    UnsupportedVersion,
    Invalid,
    ProvisioningOff,
    NoAccount,
    NotAtCharacterSelect,
    NotYours,
    AlreadyClaimed,
    NotEligible,
    WrongCredentials,
    AccountTaken,
    RateLimited,
    Unavailable,
    Incompatible,
    Conflict,
    RelayUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterEntry {
    /// Opaque to the player: the Host's handle for this character, valid for this realm.
    pub token: u32,
    pub name: String,
    pub class: u32,
    pub race: u32,
    pub level: u32,
    pub eligible: bool,
    pub reasons: Vec<String>,
    /// Already portable and claimed by this player.
    pub yours: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Welcome { realm_id: coa_registry_proto::RealmId, automatic: bool, existing_only: bool, route: Option<String> },
    Provisioned { username: String, created: bool, reset: bool },
    Characters { characters: Vec<CharacterEntry> },
    /// The character as the Host exported it: `payload` is the base64 of the encoded portable snapshot, `sha256` the hash of its canonical JSON, `collections` the account's
    /// appearance and vanity ids the Host holds for the account (kind -> ids).
    Claimed { character_id: Uuid, sha256: String, payload: String, collections: BTreeMap<String, Vec<u32>> },
    Linked { username: String },
    Done,
    RouteInfo { address: Option<String> },
    /// The Host is ready to receive transfer chunks (or indicates an offset to resume from).
    TransferReady { transfer_id: Uuid, received_offset: usize },
    /// Acknowledgement of a transfer chunk.
    TransferChunkAck { transfer_id: Uuid, received_offset: usize },
    /// Successful commit and session arm.
    TransferCommitted {
        transfer_id: Uuid,
        character_id: Uuid,
        local_guid: u32,
        session_id: Uuid,
        projected_level: Option<u32>,
        notes: Vec<String>,
    },
    /// Phase 13: Relay allocation details returned to the player.
    RelayAllocated {
        relay_host: String,
        auth_port: u16,
        world_port: u16,
        token: String,
        expires_at: i64,
    },
    Error { code: AppError, message: String },
}


pub fn encode_request(r: &Request) -> Vec<u8> {
    serde_json::to_vec(r).expect("a request serialises")
}

pub fn decode_request(bytes: &[u8]) -> Result<Request> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ControlError::Limit("a request is too long".into()));
    }
    let r: Request = serde_json::from_slice(bytes).map_err(|_| ControlError::Invalid("a request is not valid".into()))?;
    match &r {
        Request::Provision { desired, password, .. } => {
            if !valid_password(password) || desired.as_deref().is_some_and(|d| d.len() > 64 || d.chars().any(char::is_control)) {
                return invalid("a provisioning request has an unusable password or name");
            }
        }
        Request::Link { login, password } => {
            if !valid_username(login) || !valid_password_for_link(password) {
                return invalid("a link request has an unusable login or password");
            }
        }
        Request::Hello { client, .. } if client.len() > 64 => return invalid("the client name is too long"),
        Request::TransferOffer { content_hash, total_size, .. } => {
            if content_hash.len() != 64 || !content_hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return invalid("transfer content hash must be 64 hex characters");
            }
            if *total_size > crate::noise::MAX_MESSAGE_BYTES {
                return Err(ControlError::Limit("transfer total size is too large".into()));
            }
        }
        Request::TransferChunk { data, .. } => {
            if data.len() > MAX_REQUEST_BYTES {
                return Err(ControlError::Limit("transfer chunk is too large".into()));
            }
        }
        _ => {}
    }
    Ok(r)
}

/// An existing account's password may be anything the game accepts (6 to 16 printable characters without spaces or quotes).
pub fn valid_password_for_link(s: &str) -> bool {
    (6..=16).contains(&s.len()) && s.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\'' && c != '\\')
}

pub fn encode_response(r: &Response) -> Vec<u8> {
    serde_json::to_vec(r).expect("a response serialises")
}

pub fn decode_response(bytes: &[u8]) -> Result<Response> {
    serde_json::from_slice(bytes).map_err(|_| ControlError::Invalid("a response is not valid".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_random_unique_and_fit_the_game() {
        let a = generate_password();
        let b = generate_password();
        assert_ne!(a, b);
        assert!(valid_password(&a) && a.len() == 16 && a.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()), "{a}");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            assert!(seen.insert(generate_password()), "no repeats");
        }
        let symbols: std::collections::HashSet<char> = (0..200).flat_map(|_| generate_password().chars().collect::<Vec<_>>()).collect();
        assert_eq!(symbols.len(), 36, "every symbol occurs");
    }

    #[test]
    fn names_are_made_fit_and_the_alternative_is_stable() {
        assert_eq!(sanitize_username("Dmitry"), "DMITRY");
        assert_eq!(sanitize_username("d m-i_t.r y"), "DMITRY");
        assert_eq!(sanitize_username("ab"), "ABPLAYER");
        assert_eq!(sanitize_username("a very long player name indeed"), "AVERYLONGPLAYERN");
        assert_eq!(sanitize_username("<script>"), "SCRIPT");
        assert!(valid_username(&sanitize_username("")) && valid_username(&sanitize_username("日本語")));
        let p = Uuid::now_v7();
        let alt = alternative_username("DMITRY", &p, 0);
        assert!(valid_username(&alt) && alt.starts_with("DMITRY_") && alt.len() == 11, "{alt}");
        assert_eq!(alt, alternative_username("DMITRY", &p, 0), "stable for the same player");
        assert_ne!(alt, alternative_username("DMITRY", &p, 1), "another attempt, another suffix");
        assert_ne!(alt, alternative_username("DMITRY", &Uuid::now_v7(), 0));
        assert_eq!(alternative_username("ABCDEFGHIJKLMNOP", &p, 0).len(), 16, "a long name is shortened to make room");
    }

    #[test]
    fn requests_are_strict() {
        let ok = encode_request(&Request::Provision { desired: Some("DMITRY".into()), password: generate_password(), have_credentials: false });
        assert!(decode_request(&ok).is_ok());
        for bad in [
            br#"{"op":"provision","desired":null,"password":"short","have_credentials":false}"#.as_slice(),
            br#"{"op":"provision","desired":null,"password":"has space 123456","have_credentials":false}"#,
            br#"{"op":"claim","token":1,"extra":true}"#,
            br#"{"op":"link","login":"a b","password":"secret1"}"#,
            br#"{"op":"unknown"}"#,
            b"not json",
        ] {
            assert!(decode_request(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(decode_request(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
        assert!(decode_request(br#"{"op":"list_characters"}"#).is_ok());
        assert!(valid_password_for_link("pa55-w0rd") && !valid_password_for_link("has\"quote") && !valid_password_for_link("short"));
    }

    #[test]
    fn transfer_messages_roundtrip_and_validate() {
        let tid = Uuid::new_v4();
        let cid = Uuid::new_v4();
        let hash = "a".repeat(64);
        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 5,
            content_hash: hash.clone(),
            total_size: 1024,
            collections: BTreeMap::new(),
        };
        let encoded = encode_request(&offer);
        let decoded = decode_request(&encoded).unwrap();
        assert_eq!(offer, decoded);

        let bad_hash = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 5,
            content_hash: "too_short".into(),
            total_size: 1024,
            collections: BTreeMap::new(),
        };
        assert!(decode_request(&encode_request(&bad_hash)).is_err());

        let chunk = Request::TransferChunk {
            transfer_id: tid,
            offset: 0,
            data: "aGVsbG8=".into(),
        };
        assert_eq!(chunk, decode_request(&encode_request(&chunk)).unwrap());

        let resp = Response::TransferCommitted {
            transfer_id: tid,
            character_id: cid,
            local_guid: 42,
            session_id: Uuid::new_v4(),
            projected_level: Some(60),
            notes: vec!["Projected".into()],
        };
        let resp_dec = decode_response(&encode_response(&resp)).unwrap();
        assert_eq!(resp, resp_dec);
    }
}
