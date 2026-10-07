//! The end-to-end channel between a Player Manager and a Host Manager, over whatever carries frames between them (the Coordinator, later perhaps a direct socket).
//!
//! **Handshake: `Noise_XX_25519_ChaChaPoly_BLAKE2s`** (the Noise Protocol Framework, implemented by the `snow` crate; no primitive is invented here). Both sides use fresh
//! random X25519 static keys for each channel, so the Noise layer gives confidentiality, integrity and forward secrecy but no identity. Identity is added the standard way,
//! by **channel binding**: after the handshake each side signs the handshake hash `h` (unique to this channel) with its long-lived Ed25519 key.
//!
//! ```text
//! Player -> Host   msg1            (e)
//! Host   -> Player msg2            (e, ee, s, es)
//! Player -> Host   msg3            (s, se)                          channel is open; h = the handshake hash
//! Host   -> Player HostProof       Ed25519_realm_key("coa-ctl-host-v1\0" || h || realm_id)       the Player checks it against the key the Registry publishes
//! Player -> Host   PlayerProof     Ed25519_player_key("coa-ctl-player-v1\0" || h || realm_id || player_id)   sent only after the Host's proof was good
//! ```
//!
//! A Coordinator that relays the frames can neither read nor alter them (Noise authenticates every message), and cannot impersonate either side: a signature over `h`
//! for one channel is useless on another.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use coa_registry_proto::RealmId;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use snow::{Builder, HandshakeState, TransportState};
use uuid::Uuid;

use crate::{ControlError, Result};

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// Plaintext bytes per transport frame (a frame is this plus one flag byte plus the 16-byte tag).
pub const CHUNK: usize = 16 * 1024;
/// The largest application message (reassembled).
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_HANDSHAKE_BYTES: usize = 1024;

fn channel_err(e: snow::Error) -> ControlError {
    ControlError::Channel(e.to_string())
}

fn builder() -> Result<Builder<'static>> {
    Ok(Builder::new(NOISE_PARAMS.parse().map_err(channel_err)?))
}

/// The Player's side of the handshake.
pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    /// The state and the first message to send.
    pub fn start() -> Result<(Self, Vec<u8>)> {
        let b = builder()?;
        let key = b.generate_keypair().map_err(channel_err)?;
        let mut hs = b.local_private_key(&key.private).build_initiator().map_err(channel_err)?;
        let mut buf = vec![0u8; MAX_HANDSHAKE_BYTES];
        let n = hs.write_message(&[], &mut buf).map_err(channel_err)?;
        buf.truncate(n);
        Ok((Self { hs }, buf))
    }

    /// Read the Host's answer; returns the third message to send and the open channel.
    pub fn finish(mut self, msg2: &[u8]) -> Result<(Vec<u8>, Channel)> {
        if msg2.len() > MAX_HANDSHAKE_BYTES {
            return Err(ControlError::Limit("a handshake message is too long".into()));
        }
        let mut buf = vec![0u8; MAX_HANDSHAKE_BYTES];
        self.hs.read_message(msg2, &mut buf).map_err(channel_err)?;
        let n = self.hs.write_message(&[], &mut buf).map_err(channel_err)?;
        buf.truncate(n);
        if !self.hs.is_handshake_finished() {
            return Err(ControlError::Channel("the handshake did not finish".into()));
        }
        Ok((buf, Channel::from(self.hs)?))
    }
}

/// The Host's side of the handshake.
pub struct Responder {
    hs: HandshakeState,
}

impl Responder {
    /// Read the first message; returns the state and the second message to send.
    pub fn start(msg1: &[u8]) -> Result<(Self, Vec<u8>)> {
        if msg1.len() > MAX_HANDSHAKE_BYTES {
            return Err(ControlError::Limit("a handshake message is too long".into()));
        }
        let b = builder()?;
        let key = b.generate_keypair().map_err(channel_err)?;
        let mut hs = b.local_private_key(&key.private).build_responder().map_err(channel_err)?;
        let mut buf = vec![0u8; MAX_HANDSHAKE_BYTES];
        hs.read_message(msg1, &mut buf).map_err(channel_err)?;
        let n = hs.write_message(&[], &mut buf).map_err(channel_err)?;
        buf.truncate(n);
        Ok((Self { hs }, buf))
    }

    pub fn finish(mut self, msg3: &[u8]) -> Result<Channel> {
        if msg3.len() > MAX_HANDSHAKE_BYTES {
            return Err(ControlError::Limit("a handshake message is too long".into()));
        }
        let mut buf = vec![0u8; MAX_HANDSHAKE_BYTES];
        self.hs.read_message(msg3, &mut buf).map_err(channel_err)?;
        if !self.hs.is_handshake_finished() {
            return Err(ControlError::Channel("the handshake did not finish".into()));
        }
        Channel::from(self.hs)
    }
}

/// An open channel: messages go out as one or more encrypted frames and come back reassembled.
pub struct Channel {
    transport: TransportState,
    hash: Vec<u8>,
    partial: Vec<u8>,
}

impl Channel {
    fn from(hs: HandshakeState) -> Result<Self> {
        let hash = hs.get_handshake_hash().to_vec();
        Ok(Self { transport: hs.into_transport_mode().map_err(channel_err)?, hash, partial: Vec::new() })
    }

    /// The handshake hash: unique to this channel, the same on both ends.
    pub fn handshake_hash(&self) -> &[u8] {
        &self.hash
    }

    /// Encrypt one application message into frames (in order).
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(ControlError::Limit("a message is too long".into()));
        }
        let chunks: Vec<&[u8]> = if message.is_empty() { vec![&[][..]] } else { message.chunks(CHUNK).collect() };
        let mut frames = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let mut plain = Vec::with_capacity(chunk.len() + 1);
            plain.push(u8::from(i + 1 < chunks.len()));
            plain.extend_from_slice(chunk);
            let mut out = vec![0u8; plain.len() + 16];
            let n = self.transport.write_message(&plain, &mut out).map_err(channel_err)?;
            out.truncate(n);
            frames.push(out);
        }
        Ok(frames)
    }

    /// Decrypt one frame. `Some(message)` when it completed a message; `None` when more frames follow. A frame that does not authenticate ends the channel.
    pub fn open_frame(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        if frame.len() < 17 || frame.len() > CHUNK + 17 {
            return Err(ControlError::Limit("a frame is too short or too long".into()));
        }
        let mut plain = vec![0u8; frame.len()];
        let n = self.transport.read_message(frame, &mut plain).map_err(channel_err)?;
        plain.truncate(n);
        let (&more, chunk) = plain.split_first().ok_or_else(|| ControlError::Channel("an empty frame".into()))?;
        if self.partial.len() + chunk.len() > MAX_MESSAGE_BYTES {
            self.partial.clear();
            return Err(ControlError::Limit("a message is too long".into()));
        }
        self.partial.extend_from_slice(chunk);
        match more {
            0 => Ok(Some(std::mem::take(&mut self.partial))),
            1 => Ok(None),
            _ => Err(ControlError::Channel("a frame has an unknown flag".into())),
        }
    }
}

// ---- identity bound to the channel --------------------------------------------------------------------------------------------

fn host_input(h: &[u8], realm: &RealmId) -> Vec<u8> {
    let mut v = b"coa-ctl-host-v1\0".to_vec();
    v.extend_from_slice(h);
    v.extend_from_slice(realm.to_string().as_bytes());
    v
}

fn player_input(h: &[u8], realm: &RealmId, player: &Uuid) -> Vec<u8> {
    let mut v = b"coa-ctl-player-v1\0".to_vec();
    v.extend_from_slice(h);
    v.extend_from_slice(realm.to_string().as_bytes());
    v.extend_from_slice(player.hyphenated().to_string().as_bytes());
    v
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProof {
    pub realm_id: RealmId,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerProof {
    pub player_id: Uuid,
    pub public_key: String,
    pub sig: String,
}

fn decode_sig(sig: &str) -> Result<Signature> {
    let bytes: [u8; 64] = B64.decode(sig).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| ControlError::Auth("a signature is not 64 bytes of base64url".into()))?;
    Ok(Signature::from_bytes(&bytes))
}

pub fn host_proof(key: &SigningKey, realm: &RealmId, channel: &Channel) -> HostProof {
    HostProof { realm_id: *realm, sig: B64.encode(key.sign(&host_input(channel.handshake_hash(), realm)).to_bytes()) }
}

/// The Player checks that the peer holds the key the Registry publishes for the realm it asked for, on this very channel.
pub fn verify_host_proof(proof: &HostProof, realm_key: &VerifyingKey, expected_realm: &RealmId, channel: &Channel) -> Result<()> {
    if proof.realm_id != *expected_realm {
        return Err(ControlError::Auth("the peer answers for another realm".into()));
    }
    realm_key.verify_strict(&host_input(channel.handshake_hash(), expected_realm), &decode_sig(&proof.sig)?).map_err(|_| ControlError::Auth("the peer is not the realm's Host".into()))
}

pub fn player_proof(key: &SigningKey, realm: &RealmId, player: &Uuid, channel: &Channel) -> PlayerProof {
    PlayerProof { player_id: *player, public_key: crate::coord::encode_public_key(&key.verifying_key()), sig: B64.encode(key.sign(&player_input(channel.handshake_hash(), realm, player)).to_bytes()) }
}

/// The Host learns which player key signed this channel; whether that key is the one it knows for the player id is the Host's decision.
pub fn verify_player_proof(proof: &PlayerProof, realm: &RealmId, channel: &Channel) -> Result<VerifyingKey> {
    let key = crate::coord::decode_public_key(&proof.public_key)?;
    key.verify_strict(&player_input(channel.handshake_hash(), realm, &proof.player_id), &decode_sig(&proof.sig)?).map_err(|_| ControlError::Auth("the player's proof does not match this channel".into()))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Channel, Channel) {
        let (init, m1) = Initiator::start().unwrap();
        let (resp, m2) = Responder::start(&m1).unwrap();
        let (m3, player) = init.finish(&m2).unwrap();
        let host = resp.finish(&m3).unwrap();
        (player, host)
    }

    #[test]
    fn the_channel_carries_messages_of_any_size_in_both_directions() {
        let (mut player, mut host) = pair();
        assert_eq!(player.handshake_hash(), host.handshake_hash(), "both ends hold the same channel binding");
        for len in [0usize, 1, 100, CHUNK - 1, CHUNK, CHUNK + 1, 5 * CHUNK + 7, 600_000] {
            let msg: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let frames = player.seal(&msg).unwrap();
            assert!(frames.iter().all(|f| f.len() <= CHUNK + 17), "every frame fits the coordinator's limit");
            let mut got = None;
            for f in &frames {
                got = host.open_frame(f).unwrap();
            }
            assert_eq!(got.as_deref(), Some(msg.as_slice()), "{len}");
            let back = host.seal(&msg).unwrap();
            let mut got = None;
            for f in &back {
                got = player.open_frame(f).unwrap();
            }
            assert_eq!(got.as_deref(), Some(msg.as_slice()));
        }
    }

    #[test]
    fn what_a_relay_sees_is_not_the_message_and_what_it_changes_is_refused() {
        let (mut player, mut host) = pair();
        let secret = b"the password of this account is HUNTER2HUNTER2";
        let frames = player.seal(secret).unwrap();
        assert!(!frames[0].windows(7).any(|w| w == b"HUNTER2" || w == b"account"), "ciphertext carries no plaintext");
        let mut tampered = frames[0].clone();
        tampered[20] ^= 1;
        assert!(host.open_frame(&tampered).is_err(), "a changed frame does not authenticate");
        let (mut p2, mut h2) = pair();
        let f = p2.seal(b"x").unwrap();
        assert!(h2.open_frame(&f[0]).is_ok());
        assert!(h2.open_frame(&f[0]).is_err(), "a replayed frame is refused (the nonce moved on)");
        let (mut p3, mut h3) = pair();
        let (mut p4, _h4) = pair();
        let foreign = p4.seal(b"x").unwrap();
        assert!(h3.open_frame(&foreign[0]).is_err(), "a frame of another channel is refused");
        let _ = &mut p3;
    }

    #[test]
    fn a_message_that_is_too_long_or_a_frame_that_is_malformed_is_a_limit_error() {
        let (mut player, mut host) = pair();
        assert!(player.seal(&vec![0u8; MAX_MESSAGE_BYTES + 1]).is_err());
        assert!(host.open_frame(&[0u8; 5]).is_err());
        assert!(host.open_frame(&vec![0u8; CHUNK + 40]).is_err());
        assert!(Responder::start(&vec![0u8; 5000]).is_err());
    }

    #[test]
    fn identities_are_bound_to_one_channel() {
        let (player_ch, host_ch) = pair();
        let (other_player_ch, _other_host_ch) = pair();
        let realm = RealmId::new();
        let (realm_key, player_key) = (SigningKey::from_bytes(&[5; 32]), SigningKey::from_bytes(&[6; 32]));
        let player_id = Uuid::now_v7();

        let proof = host_proof(&realm_key, &realm, &host_ch);
        verify_host_proof(&proof, &realm_key.verifying_key(), &realm, &player_ch).unwrap();
        assert!(verify_host_proof(&proof, &SigningKey::from_bytes(&[9; 32]).verifying_key(), &realm, &player_ch).is_err(), "an impostor with another key");
        assert!(verify_host_proof(&proof, &realm_key.verifying_key(), &RealmId::new(), &player_ch).is_err(), "another realm");
        assert!(verify_host_proof(&proof, &realm_key.verifying_key(), &realm, &other_player_ch).is_err(), "a proof captured on another channel is useless (no relay can replay it)");

        let pp = player_proof(&player_key, &realm, &player_id, &player_ch);
        let key = verify_player_proof(&pp, &realm, &host_ch).unwrap();
        assert_eq!(key.to_bytes(), player_key.verifying_key().to_bytes());
        assert!(verify_player_proof(&pp, &RealmId::new(), &host_ch).is_err());
        assert!(verify_player_proof(&PlayerProof { player_id: Uuid::now_v7(), ..pp.clone() }, &realm, &host_ch).is_err(), "another player id under the same signature");
        let (_, third_host) = pair();
        assert!(verify_player_proof(&pp, &realm, &third_host).is_err(), "another channel");
    }
}
