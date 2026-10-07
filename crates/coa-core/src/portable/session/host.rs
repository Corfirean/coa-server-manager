//! The Host's session driver: arms sessions, takes the automatic baseline, runs checkpoints and queues the messages the
//! Owner must receive. It never decides what the canonical character becomes and never opens the Owner's database.

use std::collections::{HashMap, HashSet};

use super::super::collection::IdSet;
use super::super::capabilities::{Progression, RealmCapabilities};
use super::super::error::{PortableError, Result};
use super::super::realm::collections::Applied;
use super::super::ids::{CharacterId, PortableItemId, PortablePetId, ProfileId, SessionId};
use super::super::store::{AckEffect, CollectionOutboxMessage, HostSession, HostState, OutboxMessage, ProfileChange, StaleMapping, Store};
use super::bridge::{CheckpointReply, RealmBridge, RealmRead, RowState};
use super::protocol::*;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Seconds between checkpoints of a running session.
    pub checkpoint_interval_secs: u64,
    /// Seconds between looks at the account collections of a running session (a look is one cheap query per kind; the
    /// collection is read and sent only when its fingerprint changed). A session start and a final checkpoint always look.
    pub collection_interval_secs: u64,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { checkpoint_interval_secs: 60, collection_interval_secs: 300 }
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
    /// The realm account's collection of this kind changed: the whole compact set was queued for the Owner.
    CollectionQueued { kind: String, count: usize },
    /// The realm's progression (level cap or rules) is not the one this session started under: the session ends with a final checkpoint taken
    /// **under the old pin**, and the working copy must be projected again before the next session is armed.
    ProfileMoved { session: SessionId },
    /// The realm's progression moved but the character is online under the old one (or could not be read): the session ends at its logout.
    ProfileWaiting { session: SessionId, why: String },
}

pub struct HostService<'a> {
    store: &'a mut Store,
    server_id: String,
    config: HostConfig,
    /// When the last checkpoint of each session was requested (not persisted: after a restart the first tick checkpoints).
    last_checkpoint: HashMap<SessionId, u64>,
    /// When the account collections of each session were last looked at.
    last_collection: HashMap<SessionId, u64>,
    /// The progression the realm reports now (from the last observed profile).
    progression: Option<Progression>,
}

impl<'a> HostService<'a> {
    pub fn new(store: &'a mut Store, server_id: &str, config: HostConfig) -> Self {
        Self { store, server_id: server_id.to_string(), config, last_checkpoint: HashMap::new(), last_collection: HashMap::new(), progression: None }
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
            if self.moved(&s)? {
                if !s.reproject {
                    match s.state {
                        HostState::Open => self.end_for_profile(bridge, &s, guid, &mut events)?,
                        HostState::Armed => {
                            self.store.host_mark_reproject(s.session_id)?;
                            events.push(HostEvent::ProfileMoved { session: s.session_id });
                        }
                        _ => {}
                    }
                }
                continue;
            }
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
        let forced: HashSet<SessionId> = events
            .iter()
            .filter_map(|e| match e {
                HostEvent::BaselineTaken { session } | HostEvent::CheckpointQueued { session, final_checkpoint: true, .. } => Some(*session),
                _ => None,
            })
            .collect();
        self.sync_collections(bridge, now_secs, &forced, &mut events)?;
        Ok(events)
    }

    /// The realm's progression is not the one this session runs under.
    fn moved(&self, s: &HostSession) -> Result<bool> {
        let Some(current) = &self.progression else { return Ok(false) };
        let pin = match &s.pin {
            Some(pin) => Some(pin.clone()),
            None => self.store.character_pin(s.character_id, &s.server_id)?,
        };
        Ok(pin.is_some_and(|p| p.progression_signature != current.progression_signature || p.max_player_level != current.max_player_level || p.policy_version != current.projection_policy_version))
    }

    /// End a session whose realm changed its progression: the realm's state is read as it is (the realm's core refuses the character at
    /// login under the new progression, so nobody has played it) and sent as the **final checkpoint under the old pin**. The Owner merges it
    /// as it merges any other; the next session is not armed until the working copy is projected again.
    fn end_for_profile(&mut self, bridge: &mut dyn RealmBridge, s: &HostSession, guid: u32, events: &mut Vec<HostEvent>) -> Result<()> {
        let (items, pets) = self.priors(s)?;
        let read = match bridge.read(guid, &items, &pets) {
            Ok(read) => read,
            Err(e) => {
                events.push(HostEvent::ProfileWaiting { session: s.session_id, why: e.to_string() });
                return Ok(());
            }
        };
        if read.online {
            events.push(HostEvent::ProfileWaiting { session: s.session_id, why: "the character is online".into() });
            return Ok(());
        }
        if read.session.as_ref().is_none_or(|r| r.session_id != s.session_id) {
            return Ok(());
        }
        let sequence = self.store.host_begin_checkpoint(s.session_id)?;
        let msg = PortableCheckpoint::new(s.session_id, s.character_id, &s.server_id, s.base_revision, sequence, true, &read.exported.model, s.pin.clone())?;
        self.store.host_queue_checkpoint(s.session_id, &msg, &read.exported.observations, &read.exported.pet_observations)?;
        self.store.host_mark_reproject(s.session_id)?;
        events.push(HostEvent::CheckpointQueued { session: s.session_id, sequence, final_checkpoint: true });
        events.push(HostEvent::ProfileMoved { session: s.session_id });
        Ok(())
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
        let progression = self.session_progression(s)?;
        let msg = PortableSessionStarted::new(s.session_id, s.character_id, &s.server_id, s.base_revision, row.generation, &exported.model, progression)?;
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
        let msg = PortableCheckpoint::new(s.session_id, s.character_id, &s.server_id, s.base_revision, sequence, final_checkpoint, &exported.model, s.pin.clone())?;
        self.store.host_queue_checkpoint(s.session_id, &msg, &exported.observations, &exported.pet_observations)?;
        self.last_checkpoint.insert(s.session_id, now_secs);
        events.push(HostEvent::CheckpointQueued { session: s.session_id, sequence, final_checkpoint });
        Ok(())
    }

    /// What the session runs under: the projection of the character on this realm, else the realm's progression it was synchronised under.
    fn session_progression(&self, s: &HostSession) -> Result<Option<SessionProgression>> {
        if let Some(ctx) = self.store.projection_context(s.character_id, &s.server_id)? {
            let progression = SessionProgression { pin: ctx.pin(), projection: Some(ctx) };
            progression.validate()?;
            return Ok(Some(progression));
        }
        Ok(self.store.character_pin(s.character_id, &s.server_id)?.map(|pin| SessionProgression { pin, projection: None }))
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
            if !old.reproject {
                self.rearm(bridge, old.character_id, *next_session, *next_revision)?;
            }
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
            if s.generation > 1 && !s.reproject {
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

    // ---- account collections (Phase 6) --------------------------------------------------------------------------------------

    fn sync_collections(&mut self, bridge: &mut dyn RealmBridge, now_secs: u64, forced: &HashSet<SessionId>, events: &mut Vec<HostEvent>) -> Result<()> {
        for s in self.store.host_live_sessions(&self.server_id)? {
            let (Some(guid), HostState::Open) = (s.local_guid, s.state) else { continue };
            let due = forced.contains(&s.session_id) || self.last_collection.get(&s.session_id).is_none_or(|t| now_secs >= *t + self.config.collection_interval_secs);
            if !due {
                continue;
            }
            self.last_collection.insert(s.session_id, now_secs);
            let Some(account) = bridge.account_of(guid)? else { continue };
            for kind in COLLECTION_KINDS {
                self.observe_collection(bridge, account, kind, now_secs, events)?;
            }
        }
        Ok(())
    }

    /// One look at one account collection: nothing is read or sent unless its fingerprint changed, and nothing is sent when the
    /// Owner already acknowledged exactly the set the realm shows.
    fn observe_collection(&mut self, bridge: &mut dyn RealmBridge, account: u32, kind: &str, now_secs: u64, events: &mut Vec<HostEvent>) -> Result<()> {
        let fingerprint = bridge.collection_fingerprint(account, kind)?;
        let row = self.store.host_collection(&self.server_id, account, kind)?;
        if row.as_ref().is_some_and(|r| r.fingerprint == fingerprint) {
            return Ok(());
        }
        let set = bridge.read_collection(account, kind)?;
        let hash = set.hash(kind);
        // the first look always reports (even an empty set): that is how the Owner learns of the realm and answers with what it holds
        let already = row.as_ref().and_then(|r| r.acked_hash) == Some(hash);
        if already {
            self.store.host_collection_observe(&self.server_id, account, kind, &fingerprint, &hash, None, now_secs)?;
            return Ok(());
        }
        let bytes = collection_to_json(&CollectionObserved::new(&self.server_id, kind, &set)?)?;
        self.store.host_collection_observe(&self.server_id, account, kind, &fingerprint, &hash, Some(&bytes), now_secs)?;
        events.push(HostEvent::CollectionQueued { kind: kind.to_string(), count: set.len() });
        Ok(())
    }

    /// Collection messages waiting for the Owner.
    pub fn collection_outbox(&self) -> Result<Vec<CollectionOutboxMessage>> {
        self.store.host_collection_outbox(&self.server_id)
    }

    /// The Owner's answer to a `CollectionObserved` of `account`. When the canonical set has ids the realm lacked, the ones its
    /// client data knows are written to the account now.
    pub fn receive_collection_ack(&mut self, bridge: &mut dyn RealmBridge, account: u32, ack: &CollectionAck) -> Result<Option<Applied>> {
        if !carried_kind(&ack.kind) {
            return Err(PortableError::Invalid("the acknowledgement is for a kind that is not carried".into()));
        }
        if let CollectionOutcome::Rejected(_) = &ack.outcome {
            self.store.host_collection_drop_pending(&self.server_id, account, &ack.kind)?;
            return Ok(None);
        }
        let Some(row) = self.store.host_collection(&self.server_id, account, &ack.kind)? else { return Ok(None) };
        let Some(observed) = row.observed_hash else { return Ok(None) };
        // an Owner that holds nothing of this kind yet answers with an empty hash
        let hash: Option<[u8; 32]> = if ack.collection_hash.is_empty() {
            None
        } else {
            Some(hex::decode(&ack.collection_hash).ok().and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()).ok_or_else(|| PortableError::Invalid("the acknowledgement has a bad collection hash".into()))?)
        };
        match &ack.canonical {
            Some(state) => self.apply_canonical(bridge, account, state),
            None => {
                self.store.host_collection_acknowledge(&self.server_id, account, &ack.kind, None, None, &observed, ack.collection_revision, hash.as_ref())?;
                Ok(None)
            }
        }
    }

    /// The canonical collection of one kind (from an acknowledgement, or sent on its own to a realm that has none of it).
    pub fn receive_collection_state(&mut self, bridge: &mut dyn RealmBridge, account: u32, state: &CollectionState) -> Result<Option<Applied>> {
        self.apply_canonical(bridge, account, state)
    }

    fn apply_canonical(&mut self, bridge: &mut dyn RealmBridge, account: u32, state: &CollectionState) -> Result<Option<Applied>> {
        if !carried_kind(&state.kind) {
            return Err(PortableError::Invalid("the collection kind is not carried".into()));
        }
        let canonical: IdSet = state.set.open(&state.kind)?;
        let applied = bridge.apply_collection(account, &state.kind, &canonical)?;
        // what the realm shows now is a subset of the canonical set, so it adds nothing the Owner does not hold
        let fingerprint = bridge.collection_fingerprint(account, &state.kind)?;
        let now_hash = bridge.read_collection(account, &state.kind)?.hash(&state.kind);
        let canonical_hash = state.set.hash_bytes()?;
        self.store.host_collection_acknowledge(&self.server_id, account, &state.kind, Some(&fingerprint), Some(&now_hash), &now_hash, state.collection_revision, Some(&canonical_hash))?;
        Ok(Some(applied))
    }

    // ---- the realm's content profile (Phase 7) ------------------------------------------------------------------------------

    /// The realm's content profile as it is now. When it differs from the one remembered, what the Host sent and received of the account
    /// collections is forgotten so that the next look reports every account again (the realm may now know ids it did not, and the Owner
    /// answers with what it holds); the characters whose profile is stale are returned for a re-evaluation (whatever their revision).
    pub fn observe_profile(&mut self, caps: &RealmCapabilities, source: &str) -> Result<(ProfileChange, Vec<StaleMapping>)> {
        let change = self.store.set_realm_profile(&self.server_id, caps, source)?;
        self.progression = caps.progression.clone();
        if change.changed() {
            self.store.host_collection_reset(&self.server_id)?;
            self.last_collection.clear();
        }
        let stale = self.store.mappings_with_other_profile(&self.server_id, &caps.content_profile_hash)?;
        Ok((change, stale))
    }
}
