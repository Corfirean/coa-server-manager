use super::*;
use crate::control::service::{RemoteSessionMessage, RemoteSessionState};
use crate::portable::session::protocol::{
    self, CollectionAck, CollectionObserved, OwnerAck, PortableCheckpoint, PortableSessionStarted,
};
use coa_control_proto::app::SessionMessageKind;

pub fn owner_realm(realm: &str) -> String {
    format!("remote-{realm}")
}

impl PortableService {
    /// Open the Owner's durable session before its snapshot leaves this computer.
    pub fn prepare_remote_transfer(
        &mut self,
        character: &str,
        realm: &str,
    ) -> Res<(ClaimBundle, uuid::Uuid, u64)> {
        let cid = self.cid(character)?;
        let offer = OwnerService::new(&mut self.owner).offer(cid, &owner_realm(realm))?;
        let bundle = self.export_for_remote_transfer(character)?;
        self.publish();
        Ok((bundle, offer.session_id.as_uuid(), offer.canonical_revision))
    }

    pub fn remote_session_closed(&self, session: uuid::Uuid) -> Res<bool> {
        let sid = SessionId::from_uuid(session)?;
        Ok(self
            .owner
            .owner_session_info(sid)?
            .is_some_and(|row| row.0 == "closed"))
    }

    pub fn remote_session_idle(&self, session: uuid::Uuid) -> Res<bool> {
        let sid = SessionId::from_uuid(session)?;
        Ok(self
            .owner
            .owner_session_info(sid)?
            .is_some_and(|row| row.0 == "offered" || row.0 == "superseded"))
    }

    pub fn next_remote_session(
        &self,
        character: uuid::Uuid,
        realm: &str,
    ) -> Res<Option<uuid::Uuid>> {
        Ok(self
            .owner
            .owner_next_session(CharacterId::from_uuid(character)?, &owner_realm(realm))?
            .map(|sid| sid.as_uuid()))
    }

    /// The channel authenticates the public realm. Its private local id is never an Owner-side namespace.
    pub fn apply_remote_session_message(
        &mut self,
        character: uuid::Uuid,
        session: uuid::Uuid,
        realm: &str,
        kind: SessionMessageKind,
        bytes: &[u8],
    ) -> Res<Vec<u8>> {
        let cid = CharacterId::from_uuid(character)?;
        let sid = SessionId::from_uuid(session)?;
        let scope = owner_realm(realm);
        if self.owner.owner_session_realm(sid)?.as_deref() != Some(scope.as_str()) {
            return Err(fail(
                "not_yours",
                "This remote session was not opened by this Manager.",
            ));
        }
        let ruleset = self.owner.character(cid)?.ruleset;
        let answer = match kind {
            SessionMessageKind::Started => {
                let mut message: PortableSessionStarted = protocol::from_json(bytes)?;
                if message.character_id != cid
                    || message.session_id != sid
                    || message.ruleset != ruleset
                {
                    return Err(fail(
                        "not_yours",
                        "The baseline belongs to another character or session.",
                    ));
                }
                message.server_id = scope;
                let ack = self.owner.owner_session_started(&message)?;
                protocol::to_json(&ack)?
            }
            SessionMessageKind::Checkpoint => {
                let mut message: PortableCheckpoint = protocol::from_json(bytes)?;
                if message.character_id != cid
                    || message.session_id != sid
                    || message.ruleset != ruleset
                {
                    return Err(fail(
                        "not_yours",
                        "The checkpoint belongs to another character or session.",
                    ));
                }
                message.server_id = scope;
                let ack = self.owner.owner_checkpoint(&message)?;
                protocol::to_json(&ack)?
            }
            SessionMessageKind::Collection => {
                let mut message: CollectionObserved = protocol::collection_from_json(bytes)?;
                if message.ruleset != ruleset {
                    return Err(fail(
                        "incompatible",
                        "The collection belongs to another ruleset.",
                    ));
                }
                message.server_id = scope;
                let ack = OwnerService::new(&mut self.owner)
                    .handle_collection(&protocol::collection_to_json(&message)?)?;
                protocol::collection_to_json(&ack)?
            }
        };
        self.publish();
        Ok(answer)
    }

    fn checked_remote_session(
        &self,
        realm: &str,
        character: uuid::Uuid,
        session: uuid::Uuid,
        account: u32,
    ) -> Res<(RealmAccess, crate::portable::store::HostSession)> {
        let access = self.access(realm)?;
        let cid = CharacterId::from_uuid(character)?;
        let sid = SessionId::from_uuid(session)?;
        let row = self
            .host
            .host_session(sid)?
            .ok_or_else(|| fail("not_yours", "Unknown portable session."))?;
        if row.character_id != cid || row.server_id != realm {
            return Err(fail(
                "not_yours",
                "The portable session belongs to another realm or character.",
            ));
        }
        let guid = row
            .local_guid
            .ok_or_else(|| fail("not_yours", "The portable session has no realm character."))?;
        if realm::collections::account_of(&access.db()?, guid)? != Some(account) {
            return Err(fail(
                "not_yours",
                "The portable session belongs to another account.",
            ));
        }
        Ok((access, row))
    }

    pub fn remote_session_message(
        &mut self,
        realm: &str,
        character: uuid::Uuid,
        session: uuid::Uuid,
        account: u32,
    ) -> Res<RemoteSessionState> {
        let (_, row) = self.checked_remote_session(realm, character, session, account)?;
        let host = HostService::new(&mut self.host, realm, self.host_config.clone());
        let collection = host
            .collection_outbox()?
            .into_iter()
            .find(|message| message.account == account);
        let message = if let Some(message) = collection {
            Some(RemoteSessionMessage {
                kind: SessionMessageKind::Collection,
                bytes: message.bytes,
            })
        } else {
            host.outbox()?
                .into_iter()
                .find(|message| message.session_id == row.session_id)
                .map(|message| RemoteSessionMessage {
                    kind: if message.started {
                        SessionMessageKind::Started
                    } else {
                        SessionMessageKind::Checkpoint
                    },
                    bytes: message.bytes,
                })
        };
        Ok(RemoteSessionState {
            message,
            closed: row.state == crate::portable::store::HostState::Closed,
        })
    }

    pub fn apply_remote_session_ack(
        &mut self,
        realm: &str,
        character: uuid::Uuid,
        session: uuid::Uuid,
        account: u32,
        kind: SessionMessageKind,
        bytes: &[u8],
    ) -> Res<()> {
        let (access, row) = self.checked_remote_session(realm, character, session, account)?;
        let db = access.db()?;
        let knowledge = RealmKnowledge::from_data_dir(&access.data_dir)
            .ok()
            .map(Arc::new);
        let mut bridge = LiveBridge::new(
            &db,
            access
                .ra()
                .map_err(|error| fail("realm_offline", error.to_string()))?,
        )
        .with_knowledge(knowledge);
        let mut host = HostService::new(&mut self.host, realm, self.host_config.clone());
        match kind {
            SessionMessageKind::Collection => {
                let ack: CollectionAck = protocol::collection_from_json(bytes)?;
                host.receive_collection_ack(&mut bridge, account, &ack)?;
            }
            _ => {
                let ack: OwnerAck = protocol::from_json(bytes)?;
                if ack.session_id != row.session_id {
                    return Err(fail(
                        "not_yours",
                        "The acknowledgement belongs to another session.",
                    ));
                }
                host.receive_ack(&mut bridge, &ack)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::{
        fixtures::geared_level_eighty,
        session::protocol::{AckOutcome, PortableCheckpoint, PortableSessionStarted},
    };

    #[test]
    fn remote_progress_is_scoped_durable_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PortableService::open(dir.path(), vec![]).unwrap();
        let model = geared_level_eighty();
        let cid = model.character_id;
        service
            .owner
            .host_install_copy(service.owner_profile, &model, 7, "fixture")
            .unwrap();
        let (_, sid, revision) = service
            .prepare_remote_transfer(&cid.to_string(), "public-realm")
            .unwrap();
        assert_eq!(revision, 7);
        let session = SessionId::from_uuid(sid).unwrap();
        let started =
            PortableSessionStarted::new(session, cid, "private-host-id", revision, 1, &model, None)
                .unwrap();
        let bytes = protocol::to_json(&started).unwrap();
        assert!(service
            .apply_remote_session_message(
                cid.as_uuid(),
                sid,
                "other-realm",
                SessionMessageKind::Started,
                &bytes
            )
            .is_err());
        assert!(service
            .apply_remote_session_message(
                uuid::Uuid::now_v7(),
                sid,
                "public-realm",
                SessionMessageKind::Started,
                &bytes
            )
            .is_err());
        let ack: OwnerAck = protocol::from_json(
            &service
                .apply_remote_session_message(
                    cid.as_uuid(),
                    sid,
                    "public-realm",
                    SessionMessageKind::Started,
                    &bytes,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(ack.outcome, AckOutcome::Applied);
        assert_eq!(service.state().characters[0].status, PlayStatus::Syncing);
        assert!(service
            .prepare_remote_transfer(&cid.to_string(), "other-realm")
            .is_err());
        drop(service);
        let mut service = PortableService::open(dir.path(), vec![]).unwrap();
        let mut changed = model.clone();
        changed.progression.money += 123;
        let checkpoint = PortableCheckpoint::new(
            session,
            cid,
            "private-host-id",
            revision,
            1,
            true,
            &changed,
            None,
        )
        .unwrap();
        let bytes = protocol::to_json(&checkpoint).unwrap();
        let first: OwnerAck = protocol::from_json(
            &service
                .apply_remote_session_message(
                    cid.as_uuid(),
                    sid,
                    "public-realm",
                    SessionMessageKind::Checkpoint,
                    &bytes,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first.outcome, AckOutcome::Applied);
        let again: OwnerAck = protocol::from_json(
            &service
                .apply_remote_session_message(
                    cid.as_uuid(),
                    sid,
                    "public-realm",
                    SessionMessageKind::Checkpoint,
                    &bytes,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(again.outcome, AckOutcome::Duplicate);
        assert_eq!(first.canonical_revision, again.canonical_revision);
        assert!(service.remote_session_closed(sid).unwrap());
        drop(service);
        let service = PortableService::open(dir.path(), vec![]).unwrap();
        assert_eq!(
            service.owner.load_current(cid).unwrap().progression.money,
            changed.progression.money
        );
    }
}
