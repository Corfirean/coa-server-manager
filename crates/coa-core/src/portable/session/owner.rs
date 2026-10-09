//! The Owner's entry points: the only place a message from a realm enters the canonical store.

use super::super::collection::IdSet;
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

    // ---- account collections (Phase 6) --------------------------------------------------------------------------------------

    /// `CollectionObserved` as bytes from a transport: the realm account's set is **unioned** into the profile's collection
    /// (nothing is ever removed because a realm lacks it). The answer carries the canonical set only when it has ids the realm
    /// did not show. A message that cannot be parsed is an error and changes nothing; one that is parsed but not acceptable is
    /// answered with `Rejected`.
    pub fn handle_collection(&mut self, bytes: &[u8]) -> Result<CollectionAck> {
        let msg: CollectionObserved = collection_from_json(bytes)?;
        let profile = self.store.default_profile()?;
        let rejected = |store: &Store, reason: &str| -> Result<CollectionAck> {
            let info = store.collection_info(profile, &msg.kind).ok().flatten();
            Ok(CollectionAck {
                protocol_version: PROTOCOL_VERSION,
                kind: msg.kind.clone(),
                outcome: CollectionOutcome::Rejected(reason.to_string()),
                collection_revision: info.as_ref().map_or(0, |i| i.revision),
                collection_hash: info.map_or_else(String::new, |i| hex::encode(i.hash)),
                canonical: None,
            })
        };
        if !carried_kind(&msg.kind) {
            return rejected(self.store, "this collection kind is not carried");
        }
        let observed = match msg.set.open(&msg.kind) {
            Ok(set) => set,
            Err(e) => return rejected(self.store, &e.to_string()),
        };
        let merge = self.store.merge_collection(profile, &msg.kind, &observed)?;
        let (info, canonical) = self
            .store
            .collection(profile, &msg.kind)?
            .map(|(i, s)| (Some(i), s))
            .unwrap_or((None, IdSet::new()));
        let canonical_state = match (
            &info,
            observed.count_new(&canonical) == 0 && canonical.count_new(&observed) == 0,
        ) {
            (Some(info), false) => {
                Some(CollectionState::new(&msg.kind, info.revision, &canonical)?)
            }
            _ => None,
        };
        Ok(CollectionAck {
            protocol_version: PROTOCOL_VERSION,
            kind: msg.kind,
            outcome: if merge.changed {
                CollectionOutcome::Applied
            } else {
                CollectionOutcome::Unchanged
            },
            collection_revision: info.as_ref().map_or(0, |i| i.revision),
            collection_hash: info.map_or_else(String::new, |i| hex::encode(i.hash)),
            canonical: canonical_state,
        })
    }

    /// The canonical collections as messages (for a realm that has none of them yet, or a new one).
    pub fn collection_states(&mut self) -> Result<Vec<CollectionState>> {
        let profile = self.store.default_profile()?;
        let mut out = Vec::new();
        for kind in COLLECTION_KINDS {
            if let Some((info, set)) = self.store.collection(profile, kind)? {
                out.push(CollectionState::new(kind, info.revision, &set)?);
            }
        }
        Ok(out)
    }
}
