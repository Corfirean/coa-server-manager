use super::*;
use crate::control::store::PlayerRemoteSession;
use crate::portable::service::PortableRuntime;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

fn protocol_error(message: &str) -> ControlFail {
    ControlFail::Link(LinkError::Protocol(message.into()))
}

impl PlayerControl {
    /// Compatibility helper for transport tests. Interactive play uses the Owner's durable session id.
    pub fn transfer_character(
        &self,
        channel: &mut PlayerChannel,
        character: Uuid,
        ruleset: &str,
        revision: u64,
        payload: &[u8],
        hash: [u8; 32],
        collections: BTreeMap<String, Vec<u32>>,
    ) -> R<RemoteTransferOutcome> {
        self.transfer_with_id(
            channel,
            Uuid::now_v7(),
            false,
            character,
            ruleset,
            revision,
            payload,
            hash,
            collections,
        )
    }

    pub fn transfer_owned_character(
        &self,
        channel: &mut PlayerChannel,
        session: Uuid,
        character: Uuid,
        ruleset: &str,
        revision: u64,
        payload: &[u8],
        hash: [u8; 32],
        collections: BTreeMap<String, Vec<u32>>,
    ) -> R<RemoteTransferOutcome> {
        self.store()?
            .player_remote_session_set(&PlayerRemoteSession {
                character_id: character,
                realm_id: channel.realm_id.to_string(),
                session_id: session,
                public_key: coord::encode_public_key(&channel.realm_key),
                coordinator: channel.coordinator.clone(),
            })?;
        let result = self.transfer_with_id(
            channel,
            session,
            true,
            character,
            ruleset,
            revision,
            payload,
            hash,
            collections,
        );
        if let Ok(outcome) = &result {
            if outcome.session_id != session {
                return Err(protocol_error(
                    "The Host returned another portable session.",
                ));
            }
        }
        result
    }

    pub fn pending_remote_sessions(&self) -> R<Vec<PlayerRemoteSession>> {
        Ok(self.store()?.player_remote_sessions()?)
    }

    /// An idle, already-saved realm may be left; an uncertain or open session must finish first.
    pub fn leave_idle_remote_session(&self, character: Uuid, runtime: &PortableRuntime) -> R<()> {
        for pending in self
            .pending_remote_sessions()?
            .into_iter()
            .filter(|pending| pending.character_id == character)
        {
            let sid = pending.session_id;
            let idle = runtime
                .call(move |service| service.remote_session_idle(sid))
                .map_err(|error| ControlFail::Local(error.to_string()))?
                .map_err(|error| ControlFail::Local(error.message))?;
            if !idle {
                return Err(ControlFail::Local("Progress from the previous realm is still being saved. Keep both Managers running until it finishes.".into()));
            }
            self.store()?.player_remote_session_remove(character, sid)?;
        }
        Ok(())
    }

    /// Persist the Owner's acknowledgement before acknowledging it to the Host. A lost reply is retried without making another revision.
    pub fn sync_remote_session(
        &self,
        pending: &PlayerRemoteSession,
        runtime: &PortableRuntime,
    ) -> R<bool> {
        let mut channel = self.remote_session_channel(pending)?;
        self.sync_remote_session_on_channel(pending, runtime, &mut channel)
    }

    fn remote_session_channel(&self, pending: &PlayerRemoteSession) -> R<PlayerChannel> {
        let realm = RealmId::parse(&pending.realm_id)
            .map_err(|error| ControlFail::Local(error.to_string()))?;
        let key = coord::decode_public_key(&pending.public_key)
            .map_err(|error| protocol_error(&error.to_string()))?;
        let target = RealmTarget {
            realm_id: realm,
            key,
            coordinator: pending.coordinator.clone(),
        };
        self.connect(&target)
    }

    fn sync_remote_session_on_channel(
        &self,
        pending: &PlayerRemoteSession,
        runtime: &PortableRuntime,
        channel: &mut PlayerChannel,
    ) -> R<bool> {
        let character_id = pending.character_id;
        let session_id = pending.session_id;
        for _ in 0..16 {
            let (kind, total_size, content_hash) = match channel.request(&Request::SessionPoll {
                character_id,
                session_id,
            })? {
                Response::SessionIdle { closed } => {
                    if closed {
                        let locally_closed = runtime
                            .call(move |service| service.remote_session_closed(session_id))
                            .map_err(|error| ControlFail::Local(error.to_string()))?
                            .map_err(|error| ControlFail::Local(error.message))?;
                        if !locally_closed {
                            return Err(protocol_error(
                                "The Host closed a session before the Owner saved it.",
                            ));
                        }
                        let realm = pending.realm_id.clone();
                        let next = runtime
                            .call(move |service| service.next_remote_session(character_id, &realm))
                            .map_err(|error| ControlFail::Local(error.to_string()))?
                            .map_err(|error| ControlFail::Local(error.message))?;
                        if let Some(next) = next {
                            self.store()?.player_remote_session_advance(
                                character_id,
                                session_id,
                                next,
                            )?;
                        } else {
                            self.store()?
                                .player_remote_session_remove(character_id, session_id)?;
                        }
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Response::SessionPending {
                    kind,
                    total_size,
                    content_hash,
                } if total_size > 0
                    && total_size <= app::MAX_SESSION_BYTES
                    && content_hash.len() == 64 =>
                {
                    (kind, total_size, content_hash)
                }
                Response::Error { code, message } => {
                    return Err(ControlFail::App { code, message })
                }
                _ => return Err(protocol_error("Invalid portable session metadata.")),
            };
            let mut bytes = Vec::with_capacity(total_size);
            while bytes.len() < total_size {
                match channel.request(&Request::SessionRead {
                    character_id,
                    session_id,
                    message_hash: content_hash.clone(),
                    offset: bytes.len(),
                })? {
                    Response::SessionData { offset, data } if offset == bytes.len() => {
                        let chunk = B64
                            .decode(data)
                            .map_err(|_| protocol_error("Invalid portable session chunk."))?;
                        if chunk.is_empty() || chunk.len() > total_size - bytes.len() {
                            return Err(protocol_error("Invalid portable session chunk bounds."));
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Response::Error { code, message } => {
                        return Err(ControlFail::App { code, message })
                    }
                    _ => return Err(protocol_error("Invalid portable session chunk offset.")),
                }
            }
            if hex::encode(Sha256::digest(&bytes)) != content_hash {
                return Err(protocol_error("The portable session message hash changed."));
            }
            let remote_realm = pending.realm_id.clone();
            let ack = runtime
                .call(move |service| {
                    service.apply_remote_session_message(
                        character_id,
                        session_id,
                        &remote_realm,
                        kind,
                        &bytes,
                    )
                })
                .map_err(|error| ControlFail::Local(error.to_string()))?
                .map_err(|error| ControlFail::Local(error.message))?;
            let mut offset = 0;
            while offset < ack.len() {
                let end = ack.len().min(offset + 24 * 1024);
                match channel.request(&Request::SessionAck {
                    character_id,
                    session_id,
                    message_hash: content_hash.clone(),
                    offset,
                    total_size: ack.len(),
                    data: B64.encode(&ack[offset..end]),
                })? {
                    Response::SessionAcked {
                        received_size,
                        complete,
                    } if received_size > offset
                        && received_size <= ack.len()
                        && (!complete || received_size == ack.len()) =>
                    {
                        offset = received_size
                    }
                    Response::Error { code, message } => {
                        return Err(ControlFail::App { code, message })
                    }
                    _ => return Err(protocol_error("Invalid portable acknowledgement offset.")),
                }
            }
        }
        Ok(false)
    }

    /// Independent of the visible page and the game process. Durable sessions resume after a Manager restart.
    pub fn start_remote_session_sync(player: &Arc<Self>, runtime: &Arc<PortableRuntime>) {
        let player = Arc::downgrade(player);
        let runtime = Arc::downgrade(runtime);
        let _ = std::thread::Builder::new().name("portable-remote-sync".into()).spawn(move || {
            let mut errors = std::collections::HashMap::<Uuid, String>::new();
            let mut channels = std::collections::HashMap::<String, PlayerChannel>::new();
            loop {
                {
                    let (Some(player), Some(runtime)) = (player.upgrade(), runtime.upgrade()) else { return };
                    if let Ok(sessions) = player.pending_remote_sessions() {
                        let wanted: std::collections::HashSet<_> = sessions.iter().map(|session| format!("{}|{}|{}", session.realm_id, session.public_key, session.coordinator)).collect();
                        channels.retain(|key, _| wanted.contains(key));
                        for session in sessions {
                            let key = format!("{}|{}|{}", session.realm_id, session.public_key, session.coordinator);
                            let result = (|| {
                                if !channels.contains_key(&key) {
                                    if channels.len() >= 8 { channels.clear(); }
                                    channels.insert(key.clone(), player.remote_session_channel(&session)?);
                                }
                                player.sync_remote_session_on_channel(&session, &runtime, channels.get_mut(&key).expect("inserted"))
                            })();
                            match result {
                                Ok(_) => { errors.remove(&session.character_id); },
                                Err(error) => {
                                    channels.remove(&key);
                                    let message = error.to_string();
                                    if errors.get(&session.character_id) != Some(&message) {
                                        tracing::warn!(character = %session.character_id, error = %message, "remote portable progress is pending; retrying");
                                        errors.insert(session.character_id, message);
                                    }
                                }
                            }
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
    }
}
