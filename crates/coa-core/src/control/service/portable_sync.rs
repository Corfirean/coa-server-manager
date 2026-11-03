//! Authenticated, bounded transport for the existing Owner/Host session messages.
use super::*;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};
use std::io::Write;

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl HostService {
    fn session_account(&self, player: &Uuid, character: Uuid) -> Option<u32> {
        let mapping = self.mapping(player)?;
        let transfer = self
            .store
            .lock()
            .ok()?
            .host_transfer_latest(&self.local_id, &character)
            .ok()??;
        (transfer.player_id == *player && transfer.state != TransferState::Receiving)
            .then_some(mapping.account_id)
    }

    pub(super) fn session_poll(&self, player: &Uuid, character: Uuid, session: Uuid) -> Response {
        let Some(account) = self.session_account(player, character) else {
            return err(
                AppError::NotYours,
                "This portable session does not belong to this player.",
            );
        };
        match self
            .backend
            .session_message(&self.local_id, character, session, account)
        {
            Ok(RemoteSessionState {
                message: Some(message),
                ..
            }) => Response::SessionPending {
                kind: message.kind,
                total_size: message.bytes.len(),
                content_hash: digest(&message.bytes),
            },
            Ok(state) => Response::SessionIdle {
                closed: state.closed,
            },
            Err(error) => unavailable("reading portable session", &error),
        }
    }

    pub(super) fn session_read(
        &self,
        player: &Uuid,
        character: Uuid,
        session: Uuid,
        expected: &str,
        offset: usize,
    ) -> Response {
        let Some(account) = self.session_account(player, character) else {
            return err(
                AppError::NotYours,
                "This portable session does not belong to this player.",
            );
        };
        let message =
            match self
                .backend
                .session_message(&self.local_id, character, session, account)
            {
                Ok(state) => match state.message {
                    Some(message) => message,
                    None => {
                        return err(
                            AppError::Conflict,
                            "The portable session message changed; retry.",
                        )
                    }
                },
                Err(error) => return unavailable("reading portable session", &error),
            };
        if digest(&message.bytes) != expected || offset > message.bytes.len() {
            return err(
                AppError::Conflict,
                "The portable session message changed; retry.",
            );
        }
        let end = message.bytes.len().min(offset.saturating_add(24 * 1024));
        Response::SessionData {
            offset,
            data: B64.encode(&message.bytes[offset..end]),
        }
    }

    pub(super) fn session_ack_chunk(
        &self,
        player: &Uuid,
        character: Uuid,
        session: Uuid,
        expected: &str,
        offset: usize,
        total_size: usize,
        data: &str,
    ) -> Response {
        let Ok(_guard) = self.write.lock() else {
            return err(AppError::Unavailable, "Busy.");
        };
        let Some(account) = self.session_account(player, character) else {
            return err(
                AppError::NotYours,
                "This portable session does not belong to this player.",
            );
        };
        if expected.len() != 64
            || !expected.bytes().all(|b| b.is_ascii_hexdigit())
            || total_size == 0
            || total_size > app::MAX_SESSION_BYTES
            || offset > total_size
            || data.len() > 48 * 1024
        {
            return err(AppError::Invalid, "Invalid acknowledgement bounds.");
        }
        let bytes = match B64.decode(data) {
            Ok(bytes) if bytes.len() <= total_size - offset => bytes,
            _ => return err(AppError::Invalid, "Invalid acknowledgement chunk."),
        };
        let directory = std::env::temp_dir()
            .join("coa-session-acks")
            .join(digest(self.local_id.as_bytes()));
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return unavailable("preparing session acknowledgement", &Error::from(error));
        }
        let path = directory.join(format!("{character}-{session}.part"));
        let state = match self
            .backend
            .session_message(&self.local_id, character, session, account)
        {
            Ok(state) => state,
            Err(error) => return unavailable("reading portable session", &error),
        };
        let Some(message) = state.message else {
            let _ = std::fs::remove_file(&path);
            return Response::SessionAcked {
                received_size: total_size,
                complete: true,
            };
        };
        if digest(&message.bytes) != expected {
            return err(
                AppError::Conflict,
                "The portable session message changed; retry.",
            );
        }
        // Bind one partial file to the precise message and acknowledgement length.
        let mut header = expected.as_bytes().to_vec();
        header.extend_from_slice(&(total_size as u64).to_le_bytes());
        let write = (|| -> Result<usize> {
            if offset == 0 {
                let mut file = std::fs::File::create(&path)?;
                file.write_all(&header)?;
            }
            let current = std::fs::read(&path)?;
            if !current.starts_with(&header) {
                return Err(Error::Invalid(
                    "The acknowledgement changed; restart it.".into(),
                ));
            }
            let received = current.len() - header.len();
            if received != offset {
                return Ok(received);
            }
            let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(received + bytes.len())
        })();
        let received = match write {
            Ok(received) => received,
            Err(error) => return unavailable("saving portable acknowledgement", &error),
        };
        if received != total_size {
            return Response::SessionAcked {
                received_size: received,
                complete: false,
            };
        }
        let complete = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return unavailable("reading portable acknowledgement", &Error::from(error))
            }
        };
        match self.backend.session_ack(
            &self.local_id,
            character,
            session,
            account,
            message.kind,
            &complete[header.len()..],
        ) {
            Ok(()) => {
                let _ = std::fs::remove_file(&path);
                Response::SessionAcked {
                    received_size: total_size,
                    complete: true,
                }
            }
            Err(error) => unavailable("applying portable acknowledgement", &error),
        }
    }
}
