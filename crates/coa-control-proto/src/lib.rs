//! The Player <-> Host control protocol (Phase 12), control plane only.
//!
//! ```text
//! Player Manager --(WebSocket)--> Coordinator <--(WebSocket, opened by the Host)-- Host Manager
//!          \________ Noise_XX end-to-end channel, authenticated by Ed25519 signatures over its handshake hash ________/
//! ```
//!
//! * [`coord`]: what the Coordinator sees and decides: who may connect (a Host proves its RealmId key, a Player proves a PlayerId key), how frames are routed, the limits.
//! * [`noise`]: the end-to-end channel the Coordinator cannot read: the Noise handshake, the framing of long messages, and the binding of identities to it.
//! * [`app`]: the application messages inside the channel (account provisioning, listing and claiming characters, linking an existing account, the route).
//!
//! The Coordinator never receives plaintext account passwords, character data or database/RA credentials: everything of that kind exists only inside the channel.

pub mod app;
pub mod coord;
pub mod noise;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ControlError {
    #[error("{0}")]
    Invalid(String),
    #[error("the channel failed: {0}")]
    Channel(String),
    #[error("the peer could not prove who it is: {0}")]
    Auth(String),
    #[error("a limit was exceeded: {0}")]
    Limit(String),
}

pub type Result<T> = std::result::Result<T, ControlError>;

pub(crate) fn invalid<T>(m: impl Into<String>) -> Result<T> {
    Err(ControlError::Invalid(m.into()))
}
