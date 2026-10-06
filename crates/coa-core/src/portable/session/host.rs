//! The Host's session driver: arms sessions, takes the automatic baseline, runs checkpoints and queues the messages the
//! Owner must receive. It never decides what the canonical character becomes and never opens the Owner's database.

use std::collections::HashMap;

use super::super::error::{PortableError, Result};
use super::super::ids::{CharacterId, PortableItemId, PortablePetId, ProfileId, SessionId};
use super::super::store::{AckEffect, HostSession, HostState, OutboxMessage, Store};
use super::bridge::{CheckpointReply, RealmBridge, RealmRead, RowState};
use super::protocol::*;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Seconds between checkpoints of a running session.
    pub checkpoint_interval_secs: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { checkpoint_interval_secs: 60 }
    }
}

/// What one tick did, for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// `B0` was read and `PortableSessionStarted` queued; the player is released.
    BaselineTaken { session: SessionId },
    /// The realm held the player but the baseline could not be taken yet.
    BaselineRetry { session: SessionId, why: String },
    /// The realm released the session without the Host (nothing can be done: the baseline is lost).
    BaselineMissed { session: SessionId },
    CheckpointRequested { session: SessionId, sequence: u64 },
    CheckpointBusy { session: SessionId, sequence: u64 },
    CheckpointQueued { session: SessionId, sequence: u64, final_checkpoint: bool },
    /// The realm shows another session than the Host armed: the Host's session is closed.
    SessionReplaced { session: SessionId },
    Refused { session: SessionId, why: String },
}

pub struct HostService<'a> {
    store: &'a mut Store,
    server_id: String,
    config: HostConfig,
    /// When the last checkpoint of each session was requested (not persisted: after a restart the first tick checkpoints).
    last_checkpoint: HashMap<SessionId, u64>,
}

impl<'a> HostService<'a> {
    pub fn new(store: &'a mut Store, server_id: &str, config: HostConfig) -> Self {
        Self { store, server_id: server_id.to_string(), config, last_checkpoint: HashMap::new() }
    }

    pub fn store(&self) -> &Store {
        self.store
    }

    /// Hand the Host an offer: it keeps a copy of the character and prepares the session. The caller then imports the
    /// character into the realm (offline importer or the core's import service) with this session id armed, and calls
    /// [`HostService::bind`] with the realm's local guid.
    pub fn accept_offer(&mut self, profile: ProfileId, offer: &SessionOffer) -> Result<HostSession> {
        if offer.protocol_version != PROTOCOL_VERSION {
            return Err(PortableError::UnsupportedFormat { found: offer.protocol_version, supported: PROTOCOL_VERSION });
        }
        if offer.server_id != self.server_id {
            return Err(PortableError::Invalid("the offer is for another realm".into()));
        }
        let model = offer.snapshot.open()?;
        if model.character_id != offer.character_id {
            return Err(PortableError::WrongCharacter { expected: offer.character_id, found: model.character_id });
        }
        self.store.host_install_copy(profile, &model, offer.canonical_revision, "owner")?;
        self.store.host_prepare_session(offer, &model)
    }

    /// The character exists on the realm with this guid.
    pub fn bind(&mut self, session: SessionId, local_guid: u32) -> Result<()> {
        self.store.host_bind_session(session, local_guid)
    }

    /// One pass over every live session of this realm. Safe to call as often as wanted; every step is idempotent and resumable.
    pub fn tick(&mut self, bridge: &mut dyn RealmBridge, now_secs: u64) -> Result<Vec<HostEvent>> {
        let mut events = Vec::new();
        for s in self.store.host_live_sessions(&self.server_id)? {
            let Some(guid) = s.local_guid else { continue };
            let Some(row) = bridge.session_row(guid)? else { continue };
            if row.session_id != s.session_id {
                if s.state == HostState::Open {
                    events.push(HostEvent::SessionReplaced { session: s.session_id });
                }
                continue;
            }
            match (s.state, row.state) {
                (HostState::Armed, RowState::BaselineReady) => self.take_baseline(bridge, &s, guid, &mut events)?,
                (HostState::Armed, RowState::Active | RowState::Ended) => events.push(HostEvent::BaselineMissed { session: s.session_id }),
                (HostState::Open, RowState::Active) => self.run_checkpoint(bridge, &s, guid, row.checkpoint_seq, false, now_secs, &mut events)?,
                (HostState::Open, RowState::Ended) => self.run_checkpoint(bridge, &s, guid, row.checkpoint_seq, true, now_secs, &mut events)?,
                _ => {}
            }
        }
        Ok(events)
    }

    fn priors(&self, s: &HostSession) -> Result<(HashMap<u32, (PortableItemId, String)>, HashMap<u32, (PortablePetId, String)>)> {
        Ok((self.store.active_item_lookup(s.character_id, &s.server_id)?, self.store.active_pet_lookup(s.character_id, &s.server_id)?))
    }

    fn take_baseline(&mut self, bridge: &mut dyn RealmBridge, s: &HostSession, guid: u32, events: &mut Vec<HostEvent>) -> Result<()> {
        let (items, pets) = self.priors(s)?;
        let RealmRead { exported, session, .. } = bridge.read(guid, &items, &pets)?;
        // the baseline counts only if the very snapshot it came from still shows the baseline marker of this session
        let Some(row) = session.filter(|r| r.session_id == s.session_id && r.state == RowState::BaselineReady) else {
            events.push(HostEvent::BaselineRetry { session: s.session_id, why: "the marker moved while the baseline was being read".into() });
            return Ok(());
        };
        let msg = PortableSessionStarted::new(s.session_id, s.character_id, &s.server_id, s.base_revision, row.generation, &exported.model)?;
        self.store.host_queue_started(s.session_id, &msg, &exported.model, &exported.observations, &exported.pet_observations)?;
        // the baseline is persisted (and queued): only now may the player move on
        bridge.release(s.session_id)?;
        events.push(HostEvent::BaselineTaken { session: s.session_id });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_checkpoint(&mut self, bridge: &mut dyn RealmBridge, s: &HostSession, guid: u32, marker_seq: u64, final_checkpoint: bool, now_secs: u64, events: &mut Vec<HostEvent>) -> Result<()> {
        // one checkpoint at a time: the last one has to be queued before another is reserved
        let due = s.pending_sequence.is_some() || final_checkpoint || self.last_checkpoint.get(&s.session_id).is_none_or(|t| now_secs >= *t + self.config.checkpoint_interval_secs);
        if !due {
            return Ok(());
        }
        let sequence = self.store.host_begin_checkpoint(s.session_id)?;
        if !final_checkpoint && marker_seq < sequence {
            match bridge.request_checkpoint(guid, s.session_id, sequence)? {
                CheckpointReply::Queued => {
                    self.last_checkpoint.insert(s.session_id, now_secs);
                    events.push(HostEvent::CheckpointRequested { session: s.session_id, sequence });
                }
                CheckpointReply::Busy => events.push(HostEvent::CheckpointBusy { session: s.session_id, sequence }),
                CheckpointReply::NotOnline => {}
                CheckpointReply::Refused(why) => events.push(HostEvent::Refused { session: s.session_id, why }),
            }
            // the marker is read from the database, never assumed from the reply
            return Ok(());
        }
        let (items, pets) = self.priors(s)?;
        let RealmRead { exported, session, .. } = bridge.read(guid, &items, &pets)?;
        let Some(row) = session.filter(|r| r.session_id == s.session_id) else { return Ok(()) };
        if !final_checkpoint && row.checkpoint_seq < sequence {
            return Ok(());
        }
        if final_checkpoint && row.state != RowState::Ended {
            return Ok(());
        }
        let msg = PortableCheckpoint::new(s.session_id, s.character_id, &s.server_id, s.base_revision, sequence, final_checkpoint, &exported.model)?;
        self.store.host_queue_checkpoint(s.session_id, &msg, &exported.observations, &exported.pet_observations)?;
        self.last_checkpoint.insert(s.session_id, now_secs);
        events.push(HostEvent::CheckpointQueued { session: s.session_id, sequence, final_checkpoint });
        Ok(())
    }

    /// Messages waiting for delivery, in order.
    pub fn outbox(&self) -> Result<Vec<OutboxMessage>> {
        self.store.host_outbox_pending(&self.server_id)
    }

    /// Apply an acknowledgement. When it ends a session, the realm's marker is re-armed for the session that follows.
    pub fn receive_ack(&mut self, bridge: &mut dyn RealmBridge, ack: &OwnerAck) -> Result<AckEffect> {
        let session = self.store.host_session(ack.session_id)?;
        let effect = self.store.host_receive_ack(ack)?;
        if let (AckEffect::Finished { next_session, next_revision }, Some(old)) = (&effect, session) {
            self.rearm(bridge, old.character_id, *next_session, *next_revision)?;
        }
        Ok(effect)
    }

    /// Re-arm the realm for every armed session whose marker is not on the realm yet (after a crash between the local commit
    /// of a final acknowledgement and the realm write).
    pub fn rearm_pending(&mut self, bridge: &mut dyn RealmBridge) -> Result<usize> {
        let mut n = 0;
        for s in self.store.host_live_sessions(&self.server_id)? {
            let (Some(guid), HostState::Armed) = (s.local_guid, s.state) else { continue };
            let current = bridge.session_row(guid)?;
            if current.as_ref().is_some_and(|r| r.session_id == s.session_id) {
                continue;
            }
            if s.generation > 1 {
                bridge.arm(guid, s.session_id, s.character_id, s.base_revision, s.generation)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn rearm(&mut self, bridge: &mut dyn RealmBridge, character: CharacterId, session: SessionId, revision: u64) -> Result<()> {
        let s = self.store.host_session(session)?.ok_or_else(|| PortableError::Invalid("the next session was not recorded".into()))?;
        let guid = s.local_guid.ok_or_else(|| PortableError::Invalid("the next session has no realm character".into()))?;
        bridge.arm(guid, session, character, revision, s.generation)
    }
}
