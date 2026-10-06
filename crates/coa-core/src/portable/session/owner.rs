//! The Owner's entry points: the only place a message from a realm enters the canonical store.

use super::super::error::Result;
use super::super::ids::{CharacterId, SessionId};
use super::super::store::Store;
use super::protocol::*;

pub struct OwnerService<'a> {
    store: &'a mut Store,
}

impl<'a> OwnerService<'a> {
    pub fn new(store: &'a mut Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        self.store
    }

    /// Offer the character's current canonical revision to a realm and open the session that follows it.
    pub fn offer(&mut self, character: CharacterId, server_id: &str) -> Result<SessionOffer> {
        self.store.owner_offer_session(character, server_id)
    }

    /// `PortableSessionStarted` as bytes from a transport. A message that cannot be parsed is an error and changes nothing.
    pub fn handle_started(&mut self, bytes: &[u8]) -> Result<OwnerAck> {
        let msg: PortableSessionStarted = from_json(bytes)?;
        self.store.owner_session_started(&msg)
    }

    /// `PortableCheckpoint` as bytes from a transport.
    pub fn handle_checkpoint(&mut self, bytes: &[u8]) -> Result<OwnerAck> {
        let msg: PortableCheckpoint = from_json(bytes)?;
        self.store.owner_checkpoint(&msg)
    }

    pub fn session(&self, session: SessionId) -> Result<Option<(String, u64, u64)>> {
        self.store.owner_session_info(session)
    }
}
