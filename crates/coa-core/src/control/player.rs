//! The Player Manager's side: connect to a realm through the Coordinator, open the end-to-end channel, check that the other end is the realm's Host (the
//! realm key the Registry publishes), prove the player's own key and exchange requests. One [`PlayerChannel`] is one short conversation.

use std::time::Duration;

use coa_control_proto::app::{self, Request, Response};
use coa_control_proto::coord::{self, Frame};
use coa_control_proto::noise::{self, Channel, HostProof, Initiator};
use coa_registry_proto::RealmId;
use ed25519_dalek::VerifyingKey;

use super::identity::PlayerIdentity;
use super::transport::{self, LinkError, Ws};

pub struct PlayerChannel {
    ws: Ws,
    channel: Channel,
    conn: u32,
}

const STEP: Duration = Duration::from_secs(20);
const ANSWER: Duration = Duration::from_secs(60);

impl PlayerChannel {
    /// `coordinator` is the Coordinator's player endpoint without the query, e.g. `wss://registry.example/coord/v1/player`.
    pub fn connect(
        coordinator: &str,
        realm: &RealmId,
        realm_key: &VerifyingKey,
        identity: &PlayerIdentity,
        clock: &dyn Fn() -> i64,
    ) -> Result<PlayerChannel, LinkError> {
        let mut ws = transport::connect(
            &format!("{coordinator}?realm={realm}"),
            Duration::from_millis(500),
        )?;
        let nonce = match transport::next_frame(&mut ws, STEP)? {
            Frame::Challenge { nonce, .. } => nonce,
            _ => return Err(LinkError::Protocol("a challenge was expected".into())),
        };
        transport::send_text(
            &mut ws,
            &coord::sign_player_hello(&identity.key, &identity.player_id, realm, &nonce, clock()),
        )?;
        let conn = match transport::next_frame(&mut ws, STEP)? {
            Frame::PlayerReady { conn, .. } => conn,
            _ => {
                return Err(LinkError::Protocol(
                    "the Coordinator did not accept the hello".into(),
                ))
            }
        };
        let (initiator, msg1) = Initiator::start().map_err(LinkError::from)?;
        transport::send_binary(&mut ws, msg1)?;
        let msg2 = next_binary(&mut ws, STEP)?;
        let (msg3, mut channel) = initiator.finish(&msg2).map_err(LinkError::from)?;
        transport::send_binary(&mut ws, msg3)?;
        // the Host proves it holds the realm's key on this very channel before anything private is said
        let proof_bytes = next_message(&mut ws, &mut channel, STEP)?;
        let proof: HostProof = serde_json::from_slice(&proof_bytes)
            .map_err(|_| LinkError::Auth("the host's proof is not valid".into()))?;
        noise::verify_host_proof(&proof, realm_key, realm, &channel).map_err(LinkError::from)?;
        let mine = noise::player_proof(&identity.key, realm, &identity.player_id, &channel);
        for f in channel
            .seal(&serde_json::to_vec(&mine).map_err(|e| LinkError::Protocol(e.to_string()))?)
            .map_err(LinkError::from)?
        {
            transport::send_binary(&mut ws, f)?;
        }
        Ok(PlayerChannel { ws, channel, conn })
    }

    pub fn connection(&self) -> u32 {
        self.conn
    }

    pub fn request(&mut self, request: &Request) -> Result<Response, LinkError> {
        for f in self
            .channel
            .seal(&app::encode_request(request))
            .map_err(LinkError::from)?
        {
            transport::send_binary(&mut self.ws, f)?;
        }
        let bytes = next_message(&mut self.ws, &mut self.channel, ANSWER)?;
        app::decode_response(&bytes).map_err(LinkError::from)
    }

    pub fn close(mut self) {
        let _ = self.ws.close(None);
    }
}

fn next_binary(ws: &mut Ws, total: Duration) -> Result<Vec<u8>, LinkError> {
    let end = std::time::Instant::now() + total;
    loop {
        if std::time::Instant::now() >= end {
            return Err(LinkError::Timeout);
        }
        match transport::poll(ws)? {
            None => continue,
            Some(tungstenite::Message::Binary(b)) => return Ok(b.to_vec()),
            Some(tungstenite::Message::Text(t)) => {
                // an error frame from the Coordinator (the Host went away, a limit was hit)
                return match coord::parse_text(t.as_str()).map_err(LinkError::from)? {
                    Frame::Error { code, message } => {
                        Err(transport::from_error_frame(code, &message))
                    }
                    Frame::Close { .. } => Err(LinkError::HostOffline),
                    _ => Err(LinkError::Protocol("an unexpected control frame".into())),
                };
            }
            Some(tungstenite::Message::Close(_)) => {
                return Err(LinkError::Io("the connection was closed".into()))
            }
            Some(_) => continue,
        }
    }
}

/// The next complete encrypted message (several frames when it is long).
fn next_message(ws: &mut Ws, channel: &mut Channel, total: Duration) -> Result<Vec<u8>, LinkError> {
    let end = std::time::Instant::now() + total;
    loop {
        let left = end
            .checked_duration_since(std::time::Instant::now())
            .ok_or(LinkError::Timeout)?;
        let frame = next_binary(ws, left)?;
        if let Some(m) = channel.open_frame(&frame).map_err(LinkError::from)? {
            return Ok(m);
        }
    }
}
