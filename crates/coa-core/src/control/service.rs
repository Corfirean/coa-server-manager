//! What a Host Manager does when a Player asks (Phase 12): provision or link a game account, list the player's characters at the character-select screen,
//! and export one as a portable character. All the realm-specific work is behind [`RealmBackend`]; this module decides, records and answers.
//!
//! Security rules kept here:
//! * the PlayerId a request is for comes from the authenticated channel, never from the request;
//! * a PlayerId is tied to the public key it first used on this realm; another key for the same id is refused;
//! * an account belongs to one PlayerId, a character to one claiming PlayerId;
//! * a password lives only in the request that carries it, in the call to the backend and in the realm's own login database (as a verifier); it is not logged,
//!   not stored here, and failed [`Request::Link`] attempts are rate-limited;
//! * the character handles in a listing are random per connection, the realm's own numbers never leave the Host.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_control_proto::app::{self, AppError, CharacterEntry, Request, Response};
use coa_registry_proto::RealmId;
use uuid::Uuid;

use super::store::{AccountKind, ClaimState, ControlStore, HostTransfer, TransferState};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealmInfo {
    /// Create accounts for joining players (the default). When false the realm only links accounts that exist.
    pub automatic: bool,
    /// An address a player's game client can reach right now, when the owner has stated one. Not a relay and not discovery.
    pub route: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRow {
    pub id: u32,
    /// Upper case, as the realm stores it.
    pub username: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presence {
    pub account_online: bool,
    pub online_characters: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharInfo {
    pub guid: u32,
    pub name: String,
    pub class: u32,
    pub race: u32,
    pub level: u32,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimExport {
    pub character_id: Uuid,
    pub payload: Vec<u8>,
    pub sha256: [u8; 32],
    pub collections: BTreeMap<String, Vec<u32>>,
}

#[derive(Clone, Debug)]
pub struct RemoteImportParams {
    pub character_id: Uuid,
    pub revision: u64,
    pub payload: Vec<u8>,
    pub content_hash: [u8; 32],
    pub collections: BTreeMap<String, Vec<u32>>,
    pub account_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteImportOutcome {
    pub local_guid: u32,
    pub session_id: Uuid,
    pub projected_level: Option<u32>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightVerdict {
    Compatible,
    Degraded,
    Incompatible,
}

/// The realm itself: its login database, its console and the portable engine.
pub trait RealmBackend: Send + Sync {
    fn info(&self, local_id: &str) -> Result<RealmInfo>;
    fn account_by_name(&self, local_id: &str, username: &str) -> Result<Option<AccountRow>>;
    fn account_by_id(&self, local_id: &str, id: u32) -> Result<Option<AccountRow>>;
    fn create_account(&self, local_id: &str, username: &str, password: &str) -> Result<()>;
    fn set_password(&self, local_id: &str, username: &str, password: &str) -> Result<()>;
    /// The account when the realm's own login check accepts this name and password; checked against the realm's stored verifier, nothing is stored.
    fn check_login(&self, local_id: &str, username: &str, password: &str) -> Result<Option<AccountRow>>;
    /// The character is already registered with this Manager's portable engine (made portable here before).
    fn is_portable(&self, local_id: &str, guid: u32) -> Result<bool>;
    fn presence(&self, local_id: &str, account_id: u32) -> Result<Presence>;
    fn characters(&self, local_id: &str, account_id: u32) -> Result<Vec<CharInfo>>;
    /// Read the character (offline, eligible, belonging to the account) and register it with the Host's portable store; the same character again returns the same id.
    fn export(&self, local_id: &str, account_id: u32, guid: u32) -> Result<ClaimExport>;
    /// Compatibility preflight check before receiving payload chunks.
    fn check_transfer(&self, local_id: &str, character_id: Uuid, revision: u64) -> Result<PreflightVerdict>;
    /// Import or update a transferred character into the realm and arm a session.
    fn import_remote(&self, local_id: &str, params: RemoteImportParams) -> Result<RemoteImportOutcome>;
}

/// State of one channel.
#[derive(Default)]
pub struct Session {
    tokens: HashMap<u32, u32>,
    next: u32,
    requests: u32,
    transfer_chunks: u32,
    pub client_ip: Option<String>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_client_ip(client_ip: Option<String>) -> Self {
        Self {
            client_ip,
            ..Self::default()
        }
    }
}

const MAX_REQUESTS_PER_SESSION: u32 = 60;
const MAX_TRANSFER_CHUNKS_PER_SESSION: u32 = 128;
const LINK_FAILURES: usize = 5;
const LINK_WINDOW: Duration = Duration::from_secs(600);

pub trait RelayProvider: Send + Sync {
    fn allocate(&self, player_id: &Uuid, client_ip: Option<&str>) -> crate::error::Result<crate::control::relay_link::RelayAllocationInfo>;
}


pub struct HostService {
    local_id: String,
    realm_id: RealmId,
    store: Arc<Mutex<ControlStore>>,
    backend: Arc<dyn RealmBackend>,
    relay: Mutex<Option<Arc<dyn RelayProvider>>>,
    /// Serialises everything that changes accounts and claims on this realm.
    write: Mutex<()>,
    link_failures: Mutex<HashMap<Option<Uuid>, Vec<Instant>>>,
}

fn err(code: AppError, message: &str) -> Response {
    Response::Error { code, message: message.to_string() }
}

fn unavailable(what: &str, e: &Error) -> Response {
    tracing::warn!(error = %e, "{what}");
    err(AppError::Unavailable, "The server could not do that right now.")
}

impl HostService {
    pub fn new(local_id: &str, realm_id: RealmId, store: Arc<Mutex<ControlStore>>, backend: Arc<dyn RealmBackend>) -> Self {
        Self { local_id: local_id.to_string(), realm_id, store, backend, relay: Mutex::new(None), write: Mutex::new(()), link_failures: Mutex::new(HashMap::new()) }
    }

    pub fn set_relay(&self, provider: Arc<dyn RelayProvider>) {
        *self.relay.lock().unwrap() = Some(provider);
    }


    pub fn realm_id(&self) -> RealmId {
        self.realm_id
    }

    /// The player's public key is the one recorded for this player id when there is a record; `None` when the player is new here.
    pub fn key_matches(&self, player: &Uuid, public_key: &str) -> bool {
        match self.store.lock().ok().and_then(|s| s.host_player(&self.local_id, player).ok().flatten()) {
            Some(p) => p.public_key == public_key,
            None => true,
        }
    }

    pub fn handle(&self, session: &mut Session, player: &Uuid, public_key: &str, request: Request) -> Response {
        if matches!(request, Request::TransferChunk { .. }) {
            session.transfer_chunks += 1;
            if session.transfer_chunks > MAX_TRANSFER_CHUNKS_PER_SESSION {
                return err(AppError::RateLimited, "Too many transfer chunks on this connection.");
            }
        } else {
            session.requests += 1;
            if session.requests > MAX_REQUESTS_PER_SESSION {
                return err(AppError::RateLimited, "Too many requests on this connection.");
            }
        }
        if !self.key_matches(player, public_key) {
            return err(AppError::Invalid, "This player id belongs to another key on this realm.");
        }
        match request {
            Request::Hello { protocol, .. } => {
                if protocol != coa_control_proto::CONTROL_PROTOCOL_VERSION {
                    return err(AppError::UnsupportedVersion, "This protocol version is not supported.");
                }
                match self.backend.info(&self.local_id) {
                    Ok(info) => Response::Welcome { realm_id: self.realm_id, automatic: info.automatic, existing_only: !info.automatic, route: info.route },
                    Err(e) => unavailable("realm info", &e),
                }
            }
            Request::Provision { desired, password, have_credentials } => self.provision(player, public_key, desired, &password, have_credentials),
            Request::Link { login, password } => self.link(player, public_key, &login, &password),
            Request::ListCharacters => self.list(session, player),
            Request::Claim { token } => self.claim(session, player, token),
            Request::ClaimAck { character_id } => self.ack(player, character_id),
            Request::Route => match self.backend.info(&self.local_id) {
                Ok(info) => Response::RouteInfo { address: info.route },
                Err(e) => unavailable("route", &e),
            },
            Request::TransferOffer { transfer_id, character_id, canonical_revision, content_hash, total_size, collections } => {
                self.transfer_offer(player, transfer_id, character_id, canonical_revision, content_hash, total_size, collections)
            }
            Request::TransferChunk { transfer_id, offset, data } => self.transfer_chunk(player, transfer_id, offset, data),
            Request::TransferStatus { transfer_id } => self.transfer_status(player, transfer_id),
            Request::TransferCommit { transfer_id } => self.transfer_commit(player, transfer_id),
            Request::TransferAck { transfer_id, character_id } => self.transfer_ack(player, transfer_id, character_id),
            Request::AllocateRelay { player_id } => self.allocate_relay(session, player, &player_id),

        }
    }

    fn allocate_relay(&self, session: &Session, player: &Uuid, requested_player: &Uuid) -> Response {
        if player != requested_player {
            return err(AppError::Invalid, "Cannot request relay for another player identity.");
        }
        if self.mapping(player).is_none() {
            return err(AppError::NoAccount, "Player has no account on this realm.");
        }
        let guard = self.relay.lock().unwrap();
        if let Some(r) = guard.as_ref() {
            match r.allocate(player, session.client_ip.as_deref()) {
                Ok(alloc) => Response::RelayAllocated {
                    relay_host: alloc.relay_host,
                    auth_port: alloc.auth_port,
                    world_port: alloc.world_port,
                    token: alloc.token,
                    expires_at: alloc.expires_at,
                },
                Err(e) => err(AppError::RelayUnavailable, &e.to_string()),
            }

        } else {
            err(AppError::RelayUnavailable, "Relay is not configured on this host.")
        }
    }

    fn mapping(&self, player: &Uuid) -> Option<super::store::HostPlayer> {

        self.store.lock().ok().and_then(|s| s.host_player(&self.local_id, player).ok().flatten())
    }

    fn provision(&self, player: &Uuid, public_key: &str, desired: Option<String>, password: &str, have_credentials: bool) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        let info = match self.backend.info(&self.local_id) {
            Ok(i) => i,
            Err(e) => return unavailable("realm info", &e),
        };
        if let Some(m) = self.mapping(player) {
            // the realm may have lost the account (an administrator deleted it): the mapping then points at nothing
            match self.backend.account_by_id(&self.local_id, m.account_id) {
                Ok(Some(row)) if row.username.eq_ignore_ascii_case(&m.username) => {
                    if have_credentials {
                        return Response::Provisioned { username: m.username, created: false, reset: false };
                    }
                    return match self.backend.set_password(&self.local_id, &m.username, password) {
                        Ok(()) => Response::Provisioned { username: m.username, created: false, reset: true },
                        Err(e) => unavailable("password reset", &e),
                    };
                }
                Ok(_) => {
                    if let Ok(s) = self.store.lock() {
                        let _ = s.host_forget_player(&self.local_id, player);
                    }
                }
                Err(e) => return unavailable("account lookup", &e),
            }
        }
        if !info.automatic {
            return err(AppError::ProvisioningOff, "This realm does not create accounts for new players. Link the account you already have on it.");
        }
        let base = app::sanitize_username(desired.as_deref().unwrap_or(""));
        let mut candidates = vec![base.clone()];
        candidates.extend((0..10).map(|n| app::alternative_username(&base, player, n)));
        for name in candidates {
            match self.backend.account_by_name(&self.local_id, &name) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(e) => return unavailable("account lookup", &e),
            }
            if let Err(e) = self.backend.create_account(&self.local_id, &name, password) {
                // a name taken between the check and the creation looks like any other failure here; look again before giving up
                if matches!(self.backend.account_by_name(&self.local_id, &name), Ok(Some(_))) {
                    continue;
                }
                return unavailable("account creation", &e);
            }
            let row = match self.backend.account_by_name(&self.local_id, &name) {
                Ok(Some(r)) => r,
                Ok(None) => return err(AppError::Unavailable, "The account was not created."),
                Err(e) => return unavailable("account lookup", &e),
            };
            let bound = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_bind_player(&self.local_id, player, public_key, row.id, &row.username, AccountKind::Generated));
            if let Err(e) = bound {
                return unavailable("recording the account", &e);
            }
            return Response::Provisioned { username: row.username, created: true, reset: false };
        }
        err(AppError::AccountTaken, "No free account name was found; try a different name.")
    }

    fn too_many_failures(&self, player: &Uuid) -> bool {
        let Ok(mut map) = self.link_failures.lock() else { return true };
        let now = Instant::now();
        for key in [Some(*player), None] {
            let list = map.entry(key).or_default();
            list.retain(|t| now.duration_since(*t) < LINK_WINDOW);
        }
        map.get(&Some(*player)).is_some_and(|l| l.len() >= LINK_FAILURES) || map.get(&None).is_some_and(|l| l.len() >= LINK_FAILURES * 6)
    }

    fn note_failure(&self, player: &Uuid) {
        if let Ok(mut map) = self.link_failures.lock() {
            let now = Instant::now();
            map.entry(Some(*player)).or_default().push(now);
            map.entry(None).or_default().push(now);
        }
    }

    fn link(&self, player: &Uuid, public_key: &str, login: &str, password: &str) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        if self.too_many_failures(player) {
            return err(AppError::RateLimited, "Too many failed attempts; wait a few minutes.");
        }
        let row = match self.backend.check_login(&self.local_id, &login.to_ascii_uppercase(), password) {
            Ok(Some(r)) => r,
            Ok(None) => {
                self.note_failure(player);
                return err(AppError::WrongCredentials, "The realm does not accept that account name and password.");
            }
            Err(e) => return unavailable("login check", &e),
        };
        if let Some(m) = self.mapping(player) {
            return if m.account_id == row.id { Response::Linked { username: row.username } } else { err(AppError::AccountTaken, "This player already has another account on this realm.") };
        }
        let owner = self.store.lock().ok().and_then(|s| s.host_player_of_account(&self.local_id, row.id).ok().flatten());
        if owner.is_some() {
            return err(AppError::AccountTaken, "That account already belongs to another player.");
        }
        let bound = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_bind_player(&self.local_id, player, public_key, row.id, &row.username, AccountKind::Linked));
        match bound {
            Ok(()) => Response::Linked { username: row.username },
            Err(e) => unavailable("recording the account", &e),
        }
    }

    fn list(&self, session: &mut Session, player: &Uuid) -> Response {
        let Some(m) = self.mapping(player) else { return err(AppError::NoAccount, "This player has no account on this realm yet.") };
        let chars = match self.backend.characters(&self.local_id, m.account_id) {
            Ok(c) => c,
            Err(e) => return unavailable("character list", &e),
        };
        let claims = self.store.lock().ok().and_then(|s| s.host_claims_of(&self.local_id, player).ok()).unwrap_or_default();
        session.tokens.clear();
        let mut out = Vec::new();
        for c in chars.into_iter().take(app::MAX_CHARACTERS) {
            session.next += 1;
            let token = session.next;
            session.tokens.insert(token, c.guid);
            let yours = claims.iter().any(|k| k.local_guid == c.guid && k.state == ClaimState::Acknowledged);
            out.push(CharacterEntry { token, name: c.name, class: c.class, race: c.race, level: c.level, eligible: c.eligible, reasons: c.reasons, yours });
        }
        Response::Characters { characters: out }
    }

    fn claim(&self, session: &mut Session, player: &Uuid, token: u32) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        let Some(m) = self.mapping(player) else { return err(AppError::NoAccount, "This player has no account on this realm yet.") };
        let Some(&guid) = session.tokens.get(&token) else { return err(AppError::Invalid, "List the characters first.") };
        match self.backend.presence(&self.local_id, m.account_id) {
            Ok(p) if p.account_online && p.online_characters == 0 => {}
            Ok(_) => return err(AppError::NotAtCharacterSelect, "Log in to this realm and stay at the character selection screen, then try again."),
            Err(e) => return unavailable("presence", &e),
        }
        let chars = match self.backend.characters(&self.local_id, m.account_id) {
            Ok(c) => c,
            Err(e) => return unavailable("character list", &e),
        };
        let Some(c) = chars.into_iter().find(|c| c.guid == guid) else { return err(AppError::NotYours, "That character is not on your account.") };
        if !c.eligible {
            return err(AppError::NotEligible, &format!("That character cannot be made portable yet ({}).", c.reasons.join(", ")));
        }
        let held = self.store.lock().ok().and_then(|s| s.host_claim(&self.local_id, guid).ok().flatten());
        if held.as_ref().is_some_and(|k| k.player_id != *player) {
            return err(AppError::AlreadyClaimed, "That character was already claimed by another player.");
        }
        if held.is_none() && self.backend.is_portable(&self.local_id, guid).unwrap_or(true) {
            return err(AppError::AlreadyClaimed, "That character is already portable on this realm; ask the realm's owner.");
        }
        let exported = match self.backend.export(&self.local_id, m.account_id, guid) {
            Ok(e) => e,
            Err(e) => return unavailable("export", &e),
        };
        if let Some(k) = &held {
            if k.character_id != exported.character_id {
                return err(AppError::Unavailable, "The character's record does not match; ask the realm's owner.");
            }
        }
        let recorded = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_claim_set(&self.local_id, guid, &exported.character_id, player));
        if let Err(e) = recorded {
            return unavailable("recording the claim", &e);
        }
        use base64::Engine;
        Response::Claimed { character_id: exported.character_id, sha256: hex::encode(exported.sha256), payload: base64::engine::general_purpose::STANDARD.encode(&exported.payload), collections: exported.collections }
    }

    fn ack(&self, player: &Uuid, character: Uuid) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        match self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_claim_acknowledge(&self.local_id, &character, player)) {
            Ok(true) => Response::Done,
            Ok(false) => err(AppError::Invalid, "There is no such claim."),
            Err(e) => unavailable("recording the acknowledgement", &e),
        }
    }

    fn transfer_path(&self, transfer_id: &Uuid) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("coa-transfers").join(&self.local_id);
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{transfer_id}.part"))
    }

    fn transfer_offer(&self, player: &Uuid, transfer_id: Uuid, character_id: Uuid, canonical_revision: u64, content_hash: String, total_size: usize, collections: BTreeMap<String, Vec<u32>>) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        let Some(m) = self.mapping(player) else { return err(AppError::NoAccount, "This player has no account on this realm yet.") };

        // 1. Ownership check: another player cannot overwrite a character claimed here
        if let Ok(Some(claim)) = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_claim_of_character(&self.local_id, &character_id)) {
            if claim.player_id != *player {
                return err(AppError::AlreadyClaimed, "That character was already claimed by another player.");
            }
        }

        // 2. Active session conflict: if account is online with active characters
        match self.backend.presence(&self.local_id, m.account_id) {
            Ok(p) if p.account_online && p.online_characters > 0 => {
                return err(AppError::Conflict, "The character is currently playing in a live session.");
            }
            Err(e) => return unavailable("presence check", &e),
            _ => {}
        }

        // 3. Compatibility check before accepting payload
        match self.backend.check_transfer(&self.local_id, character_id, canonical_revision) {
            Ok(PreflightVerdict::Incompatible) => return err(AppError::Incompatible, "This character is incompatible with this realm."),
            Err(e) => return unavailable("compatibility check", &e),
            _ => {}
        }

        // 4. Idempotency / already committed check
        if let Ok(Some(existing)) = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_transfer_latest(&self.local_id, &character_id)) {
            if existing.state != TransferState::Receiving && existing.revision == canonical_revision && existing.content_hash == content_hash {
                if let (Some(guid), Some(sid)) = (existing.local_guid, existing.session_id) {
                    return Response::TransferCommitted {
                        transfer_id,
                        character_id,
                        local_guid: guid,
                        session_id: sid,
                        projected_level: existing.projected_level,
                        notes: existing.notes,
                    };
                }
            }
        }

        // 5. Resume check for this specific transfer_id
        if let Ok(Some(cur)) = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_transfer(&self.local_id, &transfer_id)) {
            if cur.player_id == *player && cur.state == TransferState::Receiving && cur.content_hash == content_hash {
                let path = self.transfer_path(&transfer_id);
                let on_disk = std::fs::metadata(&path).map(|meta| meta.len() as usize).unwrap_or(0);
                return Response::TransferReady { transfer_id, received_offset: on_disk.min(cur.received_size) };
            }
        }

        // 6. Record new transfer
        let transfer = HostTransfer {
            realm_local_id: self.local_id.clone(),
            transfer_id,
            character_id,
            player_id: *player,
            revision: canonical_revision,
            content_hash,
            total_size,
            received_size: 0,
            state: TransferState::Receiving,
            local_guid: None,
            session_id: None,
            projected_level: None,
            notes: vec![],
            collections,
        };
        if let Err(e) = self.store.lock().map_err(|_| Error::Invalid("lock".into())).and_then(|s| s.host_transfer_save(&transfer)) {
            return unavailable("saving transfer", &e);
        }
        let path = self.transfer_path(&transfer_id);
        let _ = std::fs::remove_file(&path);

        Response::TransferReady { transfer_id, received_offset: 0 }
    }

    fn transfer_chunk(&self, player: &Uuid, transfer_id: Uuid, offset: usize, data: String) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return err(AppError::Unavailable, "Busy."),
        };
        let t = match store.host_transfer(&self.local_id, &transfer_id) {
            Ok(Some(t)) => t,
            Ok(None) => return err(AppError::Invalid, "Unknown transfer id."),
            Err(e) => return unavailable("reading transfer", &e),
        };
        drop(store);

        if t.player_id != *player {
            return err(AppError::NotYours, "Transfer does not belong to this player.");
        }
        if t.state != TransferState::Receiving {
            return err(AppError::Invalid, "Transfer is not receiving chunks.");
        }

        use base64::Engine;
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&data) {
            Ok(b) => b,
            Err(_) => return err(AppError::Invalid, "Invalid base64 in chunk data."),
        };

        if offset != t.received_size {
            return Response::TransferChunkAck { transfer_id, received_offset: t.received_size };
        }

        use std::io::Write;
        let path = self.transfer_path(&transfer_id);
        let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => return unavailable("opening transfer file", &Error::Invalid(e.to_string())),
        };
        if let Err(e) = file.write_all(&bytes) {
            return unavailable("writing chunk to disk", &Error::Invalid(e.to_string()));
        }

        let new_received = t.received_size + bytes.len();
        if let Ok(s) = self.store.lock() {
            let _ = s.host_transfer_update_progress(&transfer_id, new_received);
        }

        Response::TransferChunkAck { transfer_id, received_offset: new_received }
    }

    fn transfer_status(&self, player: &Uuid, transfer_id: Uuid) -> Response {
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return err(AppError::Unavailable, "Busy."),
        };
        let t = match store.host_transfer(&self.local_id, &transfer_id) {
            Ok(Some(t)) => t,
            Ok(None) => return err(AppError::Invalid, "Unknown transfer id."),
            Err(e) => return unavailable("reading transfer", &e),
        };
        if t.player_id != *player {
            return err(AppError::NotYours, "Transfer does not belong to this player.");
        }
        match t.state {
            TransferState::Receiving => Response::TransferReady { transfer_id, received_offset: t.received_size },
            TransferState::Committed | TransferState::Acknowledged => {
                Response::TransferCommitted {
                    transfer_id,
                    character_id: t.character_id,
                    local_guid: t.local_guid.unwrap_or_default(),
                    session_id: t.session_id.unwrap_or_default(),
                    projected_level: t.projected_level,
                    notes: t.notes,
                }
            }
        }
    }

    fn transfer_commit(&self, player: &Uuid, transfer_id: Uuid) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return err(AppError::Unavailable, "Busy."),
        };
        let t = match store.host_transfer(&self.local_id, &transfer_id) {
            Ok(Some(t)) => t,
            Ok(None) => return err(AppError::Invalid, "Unknown transfer id."),
            Err(e) => return unavailable("reading transfer", &e),
        };
        drop(store);

        if t.player_id != *player {
            return err(AppError::NotYours, "Transfer does not belong to this player.");
        }
        if t.state != TransferState::Receiving {
            if let (Some(guid), Some(sid)) = (t.local_guid, t.session_id) {
                return Response::TransferCommitted {
                    transfer_id,
                    character_id: t.character_id,
                    local_guid: guid,
                    session_id: sid,
                    projected_level: t.projected_level,
                    notes: t.notes,
                };
            }
        }

        if t.received_size != t.total_size {
            return err(AppError::Invalid, "Transfer incomplete.");
        }

        let path = self.transfer_path(&transfer_id);
        let payload = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return unavailable("reading transfer file", &Error::Invalid(e.to_string())),
        };

        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(&payload));
        if hash != t.content_hash {
            let _ = std::fs::remove_file(&path);
            return err(AppError::Invalid, "Content hash mismatch.");
        }

        let mut hash_bytes = [0u8; 32];
        if hex::decode_to_slice(&t.content_hash, &mut hash_bytes).is_err() {
            return err(AppError::Invalid, "Invalid content hash format.");
        }

        let Some(m) = self.mapping(player) else { return err(AppError::NoAccount, "Player has no account on this realm.") };

        let params = RemoteImportParams {
            character_id: t.character_id,
            revision: t.revision,
            payload,
            content_hash: hash_bytes,
            collections: t.collections,
            account_id: m.account_id,
        };

        let outcome = match self.backend.import_remote(&self.local_id, params) {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return unavailable("importing character", &e);
            }
        };

        let _ = std::fs::remove_file(&path);

        if let Ok(s) = self.store.lock() {
            let _ = s.host_transfer_commit(&transfer_id, outcome.local_guid, &outcome.session_id, outcome.projected_level, &outcome.notes);
            let _ = s.host_claim_set(&self.local_id, outcome.local_guid, &t.character_id, player);
            let _ = s.host_claim_acknowledge(&self.local_id, &t.character_id, player);
        }

        Response::TransferCommitted {
            transfer_id,
            character_id: t.character_id,
            local_guid: outcome.local_guid,
            session_id: outcome.session_id,
            projected_level: outcome.projected_level,
            notes: outcome.notes,
        }
    }

    fn transfer_ack(&self, player: &Uuid, transfer_id: Uuid, _character_id: Uuid) -> Response {
        let Ok(_guard) = self.write.lock() else { return err(AppError::Unavailable, "Busy.") };
        if let Ok(s) = self.store.lock() {
            if let Ok(Some(t)) = s.host_transfer(&self.local_id, &transfer_id) {
                if t.player_id == *player {
                    let _ = s.host_transfer_ack(&transfer_id);
                }
            }
        }
        Response::Done
    }
}

#[cfg(any(test, feature = "testkit"))]
pub mod fake {
    use super::*;

    #[derive(Default)]
    pub struct FakeRealm {
        pub automatic: Mutex<bool>,
        pub route: Mutex<Option<String>>,
        pub accounts: Mutex<Vec<(u32, String, String)>>,
        pub chars: Mutex<Vec<(u32, CharInfo)>>,
        pub online: Mutex<HashMap<u32, Presence>>,
        pub exports: Mutex<HashMap<u32, Uuid>>,
        pub seen_passwords: Mutex<Vec<String>>,
        pub imported: Mutex<HashMap<Uuid, (u32, Uuid)>>,
        pub incompatible: Mutex<std::collections::HashSet<Uuid>>,
    }

    impl FakeRealm {
        pub fn new(automatic: bool) -> Arc<Self> {
            let r = Self::default();
            *r.automatic.lock().unwrap() = automatic;
            Arc::new(r)
        }
        pub fn add_account(&self, name: &str, password: &str) -> u32 {
            let mut a = self.accounts.lock().unwrap();
            let id = a.len() as u32 + 1;
            a.push((id, name.to_ascii_uppercase(), password.to_ascii_uppercase()));
            id
        }
        pub fn add_char(&self, account: u32, guid: u32, name: &str, eligible: bool) {
            self.chars.lock().unwrap().push((account, CharInfo { guid, name: name.into(), class: 1, race: 1, level: 60, eligible, reasons: if eligible { vec![] } else { vec!["online".into()] } }));
        }
        pub fn at_select(&self, account: u32, online: bool, chars_online: u32) {
            self.online.lock().unwrap().insert(account, Presence { account_online: online, online_characters: chars_online });
        }
    }

    impl RealmBackend for FakeRealm {
        fn info(&self, _: &str) -> Result<RealmInfo> {
            Ok(RealmInfo { automatic: *self.automatic.lock().unwrap(), route: self.route.lock().unwrap().clone() })
        }
        fn account_by_name(&self, _: &str, username: &str) -> Result<Option<AccountRow>> {
            Ok(self.accounts.lock().unwrap().iter().find(|a| a.1 == username.to_ascii_uppercase()).map(|a| AccountRow { id: a.0, username: a.1.clone() }))
        }
        fn account_by_id(&self, _: &str, id: u32) -> Result<Option<AccountRow>> {
            Ok(self.accounts.lock().unwrap().iter().find(|a| a.0 == id).map(|a| AccountRow { id: a.0, username: a.1.clone() }))
        }
        fn create_account(&self, _: &str, username: &str, password: &str) -> Result<()> {
            self.seen_passwords.lock().unwrap().push(password.to_string());
            if self.account_by_name("", username)?.is_some() {
                return Err(Error::Invalid("taken".into()));
            }
            self.add_account(username, password);
            Ok(())
        }
        fn set_password(&self, _: &str, username: &str, password: &str) -> Result<()> {
            self.seen_passwords.lock().unwrap().push(password.to_string());
            for a in self.accounts.lock().unwrap().iter_mut() {
                if a.1 == username.to_ascii_uppercase() {
                    a.2 = password.to_ascii_uppercase();
                }
            }
            Ok(())
        }
        fn check_login(&self, _: &str, username: &str, password: &str) -> Result<Option<AccountRow>> {
            Ok(self.accounts.lock().unwrap().iter().find(|a| a.1 == username.to_ascii_uppercase() && a.2 == password.to_ascii_uppercase()).map(|a| AccountRow { id: a.0, username: a.1.clone() }))
        }
        fn is_portable(&self, _: &str, guid: u32) -> Result<bool> {
            Ok(self.exports.lock().unwrap().contains_key(&guid))
        }
        fn presence(&self, _: &str, account_id: u32) -> Result<Presence> {
            Ok(self.online.lock().unwrap().get(&account_id).copied().unwrap_or(Presence { account_online: false, online_characters: 0 }))
        }
        fn characters(&self, _: &str, account_id: u32) -> Result<Vec<CharInfo>> {
            Ok(self.chars.lock().unwrap().iter().filter(|c| c.0 == account_id).map(|c| c.1.clone()).collect())
        }
        fn export(&self, _: &str, _: u32, guid: u32) -> Result<ClaimExport> {
            let id = *self.exports.lock().unwrap().entry(guid).or_insert_with(Uuid::now_v7);
            Ok(ClaimExport { character_id: id, payload: format!("payload-{guid}").into_bytes(), sha256: [7; 32], collections: BTreeMap::from([("coa:appearance".to_string(), vec![1, 2, 3])]) })
        }
        fn check_transfer(&self, _: &str, character_id: Uuid, _: u64) -> Result<PreflightVerdict> {
            if self.incompatible.lock().unwrap().contains(&character_id) {
                Ok(PreflightVerdict::Incompatible)
            } else {
                Ok(PreflightVerdict::Compatible)
            }
        }
        fn import_remote(&self, _: &str, params: RemoteImportParams) -> Result<RemoteImportOutcome> {
            if self.incompatible.lock().unwrap().contains(&params.character_id) {
                return Err(Error::Invalid("Character is incompatible with this realm.".into()));
            }
            let mut map = self.imported.lock().unwrap();
            let len = map.len() as u32 + 100;
            let (guid, sid) = *map.entry(params.character_id).or_insert((len, Uuid::now_v7()));
            Ok(RemoteImportOutcome { local_guid: guid, session_id: sid, projected_level: None, notes: vec![] })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRealm;
    use super::*;

    fn service(realm: &Arc<FakeRealm>) -> HostService {
        HostService::new("srv-1", RealmId::new(), Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap())), realm.clone())
    }

    fn pw() -> String {
        app::generate_password()
    }

    fn provision(s: &HostService, p: &Uuid, desired: Option<&str>, password: &str, have: bool) -> Response {
        s.handle(&mut Session::default(), p, "KEY", Request::Provision { desired: desired.map(str::to_string), password: password.to_string(), have_credentials: have })
    }

    #[test]
    fn a_new_player_gets_an_account_and_the_same_one_again() {
        let realm = FakeRealm::new(true);
        let s = service(&realm);
        let p = Uuid::now_v7();
        let password = pw();
        let Response::Provisioned { username, created, reset } = provision(&s, &p, Some("Dmitry"), &password, false) else { panic!() };
        assert_eq!((username.as_str(), created, reset), ("DMITRY", true, false));
        assert!(realm.check_login("", "DMITRY", &password).unwrap().is_some(), "the password the player sent is the realm's");
        // coming back with credentials changes nothing
        assert_eq!(provision(&s, &p, Some("Dmitry"), &pw(), true), Response::Provisioned { username: "DMITRY".into(), created: false, reset: false });
        assert_eq!(realm.accounts.lock().unwrap().len(), 1);
        // coming back without them (reinstalled Manager) gets a new password for the same account
        let fresh = pw();
        assert_eq!(provision(&s, &p, None, &fresh, false), Response::Provisioned { username: "DMITRY".into(), created: false, reset: true });
        assert!(realm.check_login("", "DMITRY", &fresh).unwrap().is_some());
    }

    #[test]
    fn an_occupied_name_gets_a_stable_alternative() {
        let realm = FakeRealm::new(true);
        realm.add_account("Dmitry", "whatever1");
        let s = service(&realm);
        let p = Uuid::now_v7();
        let Response::Provisioned { username, created, .. } = provision(&s, &p, Some("DMITRY"), &pw(), false) else { panic!() };
        assert!(created && username.starts_with("DMITRY_") && username.len() == 11 && app::valid_username(&username), "{username}");
        assert_eq!(username, app::alternative_username("DMITRY", &p, 0));
        let q = Uuid::now_v7();
        let Response::Provisioned { username: other, .. } = provision(&s, &q, Some("DMITRY"), &pw(), false) else { panic!() };
        assert_ne!(other, username, "another player, another account");
    }

    #[test]
    fn existing_only_realms_create_nothing() {
        let realm = FakeRealm::new(false);
        let s = service(&realm);
        let r = provision(&s, &Uuid::now_v7(), Some("X"), &pw(), false);
        assert!(matches!(r, Response::Error { code: AppError::ProvisioningOff, .. }), "{r:?}");
        assert!(realm.accounts.lock().unwrap().is_empty());
        let Response::Welcome { automatic, existing_only, .. } = s.handle(&mut Session::default(), &Uuid::now_v7(), "K", Request::Hello { protocol: 1, client: "t".into() }) else { panic!() };
        assert!(!automatic && existing_only);
    }

    #[test]
    fn a_deleted_account_is_made_again() {
        let realm = FakeRealm::new(true);
        let s = service(&realm);
        let p = Uuid::now_v7();
        provision(&s, &p, Some("Anna"), &pw(), false);
        realm.accounts.lock().unwrap().clear();
        let Response::Provisioned { created, username, .. } = provision(&s, &p, Some("Anna"), &pw(), true) else { panic!() };
        assert!(created && username == "ANNA");
    }

    #[test]
    fn linking_checks_the_realms_own_login_and_binds_one_player() {
        let realm = FakeRealm::new(true);
        let id = realm.add_account("Legacy", "Secret12");
        let s = service(&realm);
        let (p, q) = (Uuid::now_v7(), Uuid::now_v7());
        let link = |who: &Uuid, key: &str, login: &str, pw: &str| s.handle(&mut Session::default(), who, key, Request::Link { login: login.into(), password: pw.into() });
        assert!(matches!(link(&p, "KP", "LEGACY", "wrongpass"), Response::Error { code: AppError::WrongCredentials, .. }));
        assert_eq!(link(&p, "KP", "legacy", "secret12"), Response::Linked { username: "LEGACY".into() });
        assert_eq!(link(&p, "KP", "LEGACY", "Secret12"), Response::Linked { username: "LEGACY".into() }, "idempotent");
        assert!(matches!(link(&q, "KQ", "LEGACY", "Secret12"), Response::Error { code: AppError::AccountTaken, .. }), "an account belongs to one player");
        assert_eq!(s.store.lock().unwrap().host_player("srv-1", &p).unwrap().unwrap().account_id, id);
        assert!(!s.key_matches(&p, "OTHER") && s.key_matches(&p, "KP") && s.key_matches(&Uuid::now_v7(), "ANY"));
        let r = s.handle(&mut Session::default(), &p, "OTHER", Request::ListCharacters);
        assert!(matches!(r, Response::Error { code: AppError::Invalid, .. }), "another key for the same player id is refused");
    }

    #[test]
    fn guessing_passwords_is_rate_limited() {
        let realm = FakeRealm::new(true);
        realm.add_account("Legacy", "Secret12");
        let s = service(&realm);
        let p = Uuid::now_v7();
        for _ in 0..LINK_FAILURES {
            assert!(matches!(s.handle(&mut Session::default(), &p, "K", Request::Link { login: "LEGACY".into(), password: "guess123".into() }), Response::Error { code: AppError::WrongCredentials, .. }));
        }
        let r = s.handle(&mut Session::default(), &p, "K", Request::Link { login: "LEGACY".into(), password: "Secret12".into() });
        assert!(matches!(r, Response::Error { code: AppError::RateLimited, .. }), "even the right password waits: {r:?}");
    }

    fn claimed_setup() -> (Arc<FakeRealm>, HostService, Uuid, u32) {
        let realm = FakeRealm::new(true);
        let acc = realm.add_account("Thrall", "Secret12");
        realm.add_char(acc, 100, "Thrall", true);
        realm.add_char(acc, 101, "Busy", false);
        realm.add_char(999, 200, "Stranger", true);
        let s = service(&realm);
        let p = Uuid::now_v7();
        assert!(matches!(s.handle(&mut Session::default(), &p, "K", Request::Link { login: "THRALL".into(), password: "Secret12".into() }), Response::Linked { .. }));
        (realm, s, p, acc)
    }

    #[test]
    fn a_claim_needs_the_character_select_screen_ownership_and_eligibility() {
        let (realm, s, p, acc) = claimed_setup();
        let mut session = Session::default();
        let Response::Characters { characters } = s.handle(&mut session, &p, "K", Request::ListCharacters) else { panic!() };
        assert_eq!(characters.len(), 2, "only this account's characters");
        let good = characters.iter().find(|c| c.name == "Thrall").unwrap();
        let busy = characters.iter().find(|c| c.name == "Busy").unwrap();
        assert!(good.eligible && !busy.eligible && !good.yours);
        assert!(characters.iter().all(|c| c.token < 100), "the realm's own numbers are not handed out");
        // not logged in
        let r = s.handle(&mut session, &p, "K", Request::Claim { token: good.token });
        assert!(matches!(r, Response::Error { code: AppError::NotAtCharacterSelect, .. }), "{r:?}");
        // logged in with a character in the world
        realm.at_select(acc, true, 1);
        assert!(matches!(s.handle(&mut session, &p, "K", Request::Claim { token: good.token }), Response::Error { code: AppError::NotAtCharacterSelect, .. }));
        realm.at_select(acc, true, 0);
        assert!(matches!(s.handle(&mut session, &p, "K", Request::Claim { token: busy.token }), Response::Error { code: AppError::NotEligible, .. }));
        assert!(matches!(s.handle(&mut session, &p, "K", Request::Claim { token: 9999 }), Response::Error { code: AppError::Invalid, .. }), "a made-up handle");
        let Response::Claimed { character_id, sha256, payload, collections } = s.handle(&mut session, &p, "K", Request::Claim { token: good.token }) else { panic!() };
        assert_eq!(sha256, hex::encode([7u8; 32]));
        assert!(!payload.is_empty() && collections["coa:appearance"] == vec![1, 2, 3]);
        // asking again returns the same character
        let Response::Claimed { character_id: again, .. } = s.handle(&mut session, &p, "K", Request::Claim { token: good.token }) else { panic!() };
        assert_eq!(again, character_id);
        assert_eq!(s.handle(&mut session, &p, "K", Request::ClaimAck { character_id }), Response::Done);
        assert!(matches!(s.handle(&mut session, &p, "K", Request::ClaimAck { character_id: Uuid::now_v7() }), Response::Error { .. }));
        let Response::Characters { characters } = s.handle(&mut session, &p, "K", Request::ListCharacters) else { panic!() };
        assert!(characters.iter().find(|c| c.name == "Thrall").unwrap().yours);
    }

    #[test]
    fn another_player_cannot_claim_a_claimed_character_or_one_of_another_account() {
        let (realm, s, p, acc) = claimed_setup();
        realm.at_select(acc, true, 0);
        let mut session = Session::default();
        let Response::Characters { characters } = s.handle(&mut session, &p, "K", Request::ListCharacters) else { panic!() };
        let token = characters.iter().find(|c| c.name == "Thrall").unwrap().token;
        assert!(matches!(s.handle(&mut session, &p, "K", Request::Claim { token }), Response::Claimed { .. }));
        // a second player linked to the same realm (a different account) has none of these characters
        let acc2 = realm.add_account("Other", "Secret34");
        realm.at_select(acc2, true, 0);
        let q = Uuid::now_v7();
        assert!(matches!(s.handle(&mut Session::default(), &q, "KQ", Request::Link { login: "OTHER".into(), password: "Secret34".into() }), Response::Linked { .. }));
        let mut qs = Session::default();
        let Response::Characters { characters } = s.handle(&mut qs, &q, "KQ", Request::ListCharacters) else { panic!() };
        assert!(characters.is_empty());
        assert!(matches!(s.handle(&mut qs, &q, "KQ", Request::Claim { token }), Response::Error { code: AppError::Invalid, .. }), "a handle from another connection means nothing");
        // even if the character were moved to q's account, the claim by p stands
        realm.chars.lock().unwrap().push((acc2, CharInfoLike::copy(100)));
        let Response::Characters { characters } = s.handle(&mut qs, &q, "KQ", Request::ListCharacters) else { panic!() };
        let moved = characters.iter().find(|c| c.name == "Thrall").unwrap().token;
        let r = s.handle(&mut qs, &q, "KQ", Request::Claim { token: moved });
        assert!(matches!(r, Response::Error { code: AppError::AlreadyClaimed, .. }), "{r:?}");
        // and an unlinked player has no account at all
        let r = s.handle(&mut Session::default(), &Uuid::now_v7(), "KZ", Request::ListCharacters);
        assert!(matches!(r, Response::Error { code: AppError::NoAccount, .. }));
    }

    struct CharInfoLike;
    impl CharInfoLike {
        fn copy(guid: u32) -> CharInfo {
            CharInfo { guid, name: "Thrall".into(), class: 1, race: 1, level: 60, eligible: true, reasons: vec![] }
        }
    }

    #[test]
    fn requests_per_connection_are_bounded_and_versions_checked() {
        let realm = FakeRealm::new(true);
        let s = service(&realm);
        let p = Uuid::now_v7();
        assert!(matches!(s.handle(&mut Session::default(), &p, "K", Request::Hello { protocol: 99, client: "x".into() }), Response::Error { code: AppError::UnsupportedVersion, .. }));
        let mut session = Session::default();
        let mut last = Response::Done;
        for _ in 0..=MAX_REQUESTS_PER_SESSION {
            last = s.handle(&mut session, &p, "K", Request::Route);
        }
        assert!(matches!(last, Response::Error { code: AppError::RateLimited, .. }));
        assert_eq!(s.handle(&mut Session::default(), &p, "K", Request::Route), Response::RouteInfo { address: None });
        *realm.route.lock().unwrap() = Some("203.0.113.5:3724".into());
        assert_eq!(s.handle(&mut Session::default(), &p, "K", Request::Route), Response::RouteInfo { address: Some("203.0.113.5:3724".into()) });
    }

    #[test]
    fn remote_transfer_lifecycle_empty_realm_import_and_ack() {
        let (realm, s, p, _acc) = claimed_setup();
        let mut session = Session::default();
        let cid = Uuid::now_v7();
        let tid = Uuid::now_v7();

        let payload = b"portable_character_snapshot_payload_test_data".to_vec();
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(&payload));

        // 1. Offer
        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash.clone(),
            total_size: payload.len(),
            collections: BTreeMap::from([("coa:appearance".into(), vec![1, 2, 3])]),
        };
        let r = s.handle(&mut session, &p, "K", offer);
        assert_eq!(r, Response::TransferReady { transfer_id: tid, received_offset: 0 });

        // 2. Chunk
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&payload);
        let chunk = Request::TransferChunk { transfer_id: tid, offset: 0, data };
        let r = s.handle(&mut session, &p, "K", chunk);
        assert_eq!(r, Response::TransferChunkAck { transfer_id: tid, received_offset: payload.len() });

        // 3. Commit
        let commit = Request::TransferCommit { transfer_id: tid };
        let r = s.handle(&mut session, &p, "K", commit);
        let Response::TransferCommitted { transfer_id, character_id, local_guid, session_id, .. } = r else { panic!("{r:?}") };
        assert_eq!(transfer_id, tid);
        assert_eq!(character_id, cid);
        assert!(local_guid >= 100);
        assert_ne!(session_id, Uuid::nil());

        // 4. Ack
        let ack = Request::TransferAck { transfer_id: tid, character_id: cid };
        assert_eq!(s.handle(&mut session, &p, "K", ack), Response::Done);

        // Verify imported in realm
        assert!(realm.imported.lock().unwrap().contains_key(&cid));
    }

    #[test]
    fn remote_transfer_incompatible_rejected_writes_nothing() {
        let (realm, s, p, _acc) = claimed_setup();
        let mut session = Session::default();
        let cid = Uuid::now_v7();
        let tid = Uuid::now_v7();

        // Mark incompatible in realm
        realm.incompatible.lock().unwrap().insert(cid);

        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 1,
            content_hash: "a".repeat(64),
            total_size: 100,
            collections: BTreeMap::new(),
        };
        let r = s.handle(&mut session, &p, "K", offer);
        assert!(matches!(r, Response::Error { code: AppError::Incompatible, .. }), "{r:?}");

        // Nothing written
        assert!(realm.imported.lock().unwrap().is_empty());
    }

    #[test]
    fn remote_transfer_interrupted_resumes_safely() {
        let (_realm, s, p, _acc) = claimed_setup();
        let mut session = Session::default();
        let cid = Uuid::now_v7();
        let tid = Uuid::now_v7();

        let chunk1 = b"first_chunk_of_payload_";
        let chunk2 = b"second_chunk_of_payload";
        let mut full = chunk1.to_vec();
        full.extend_from_slice(chunk2);

        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(&full));

        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash.clone(),
            total_size: full.len(),
            collections: BTreeMap::new(),
        };
        let r = s.handle(&mut session, &p, "K", offer);
        assert_eq!(r, Response::TransferReady { transfer_id: tid, received_offset: 0 });

        // Send chunk 1
        use base64::Engine;
        let c1_b64 = base64::engine::general_purpose::STANDARD.encode(chunk1);
        let r = s.handle(&mut session, &p, "K", Request::TransferChunk { transfer_id: tid, offset: 0, data: c1_b64 });
        assert_eq!(r, Response::TransferChunkAck { transfer_id: tid, received_offset: chunk1.len() });

        // Query status (resuming after interruption)
        let r = s.handle(&mut session, &p, "K", Request::TransferStatus { transfer_id: tid });
        assert_eq!(r, Response::TransferReady { transfer_id: tid, received_offset: chunk1.len() });

        // Send chunk 2
        let c2_b64 = base64::engine::general_purpose::STANDARD.encode(chunk2);
        let r = s.handle(&mut session, &p, "K", Request::TransferChunk { transfer_id: tid, offset: chunk1.len(), data: c2_b64 });
        assert_eq!(r, Response::TransferChunkAck { transfer_id: tid, received_offset: full.len() });

        // Commit succeeds
        let r = s.handle(&mut session, &p, "K", Request::TransferCommit { transfer_id: tid });
        assert!(matches!(r, Response::TransferCommitted { .. }));
    }

    #[test]
    fn remote_transfer_lost_reply_idempotency_does_not_duplicate() {
        let (realm, s, p, _acc) = claimed_setup();
        let mut session = Session::default();
        let cid = Uuid::now_v7();
        let tid = Uuid::now_v7();

        let payload = b"idempotency_test_payload".to_vec();
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(&payload));

        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash.clone(),
            total_size: payload.len(),
            collections: BTreeMap::new(),
        };
        s.handle(&mut session, &p, "K", offer);

        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&payload);
        s.handle(&mut session, &p, "K", Request::TransferChunk { transfer_id: tid, offset: 0, data });

        let Response::TransferCommitted { local_guid: guid1, session_id: sid1, .. } = s.handle(&mut session, &p, "K", Request::TransferCommit { transfer_id: tid }) else { panic!() };
        assert_eq!(realm.imported.lock().unwrap().len(), 1);

        // Client retries commit (lost reply):
        let Response::TransferCommitted { local_guid: guid2, session_id: sid2, .. } = s.handle(&mut session, &p, "K", Request::TransferCommit { transfer_id: tid }) else { panic!() };
        assert_eq!(guid1, guid2);
        assert_eq!(sid1, sid2);
        assert_eq!(realm.imported.lock().unwrap().len(), 1, "does not duplicate character");

        // Client starts a new transfer for the exact same committed revision: discovers committed state
        let tid2 = Uuid::now_v7();
        let offer2 = Request::TransferOffer {
            transfer_id: tid2,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash,
            total_size: payload.len(),
            collections: BTreeMap::new(),
        };
        let r = s.handle(&mut session, &p, "K", offer2);
        let Response::TransferCommitted { local_guid: guid3, session_id: sid3, .. } = r else { panic!("{r:?}") };
        assert_eq!(guid1, guid3);
        assert_eq!(sid1, sid3);
        assert_eq!(realm.imported.lock().unwrap().len(), 1, "does not duplicate character");
    }

    #[test]
    fn wrong_player_identity_cannot_overwrite_another_players_realm_copy() {
        let (_realm, s, p, _acc) = claimed_setup();
        let cid = Uuid::now_v7();
        let tid = Uuid::now_v7();

        let payload = b"player_1_character_data".to_vec();
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(&payload));

        let offer = Request::TransferOffer {
            transfer_id: tid,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash.clone(),
            total_size: payload.len(),
            collections: BTreeMap::new(),
        };
        assert!(matches!(s.handle(&mut Session::default(), &p, "K", offer), Response::TransferReady { .. }));
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&payload);
        s.handle(&mut Session::default(), &p, "K", Request::TransferChunk { transfer_id: tid, offset: 0, data });
        assert!(matches!(s.handle(&mut Session::default(), &p, "K", Request::TransferCommit { transfer_id: tid }), Response::TransferCommitted { .. }));
        assert_eq!(s.handle(&mut Session::default(), &p, "K", Request::TransferAck { transfer_id: tid, character_id: cid }), Response::Done);

        // Player 2 attempts to transfer with the same character_id
        let q = Uuid::now_v7();
        let srv = &s;
        // Provision account for player 2
        let pw = app::generate_password();
        assert!(matches!(srv.handle(&mut Session::default(), &q, "KQ", Request::Provision { desired: Some("PLAYER2".into()), password: pw, have_credentials: false }), Response::Provisioned { .. }));

        let tid_impostor = Uuid::now_v7();
        let impostor_offer = Request::TransferOffer {
            transfer_id: tid_impostor,
            character_id: cid,
            canonical_revision: 2,
            content_hash: "b".repeat(64),
            total_size: 100,
            collections: BTreeMap::new(),
        };
        let r = srv.handle(&mut Session::default(), &q, "KQ", impostor_offer);
        assert!(matches!(r, Response::Error { code: AppError::AlreadyClaimed, .. }), "{r:?}");
    }

    #[test]
    fn same_character_transferred_again_as_newer_revision_performs_update() {
        let (realm, s, p, _acc) = claimed_setup();
        let mut session = Session::default();
        let cid = Uuid::now_v7();
        let tid1 = Uuid::now_v7();

        let payload1 = b"character_revision_1_data".to_vec();
        use sha2::{Digest, Sha256};
        let hash1 = hex::encode(Sha256::digest(&payload1));

        // Rev 1
        s.handle(&mut session, &p, "K", Request::TransferOffer {
            transfer_id: tid1,
            character_id: cid,
            canonical_revision: 1,
            content_hash: hash1,
            total_size: payload1.len(),
            collections: BTreeMap::new(),
        });
        use base64::Engine;
        let d1 = base64::engine::general_purpose::STANDARD.encode(&payload1);
        s.handle(&mut session, &p, "K", Request::TransferChunk { transfer_id: tid1, offset: 0, data: d1 });
        let Response::TransferCommitted { local_guid: guid1, .. } = s.handle(&mut session, &p, "K", Request::TransferCommit { transfer_id: tid1 }) else { panic!() };
        s.handle(&mut session, &p, "K", Request::TransferAck { transfer_id: tid1, character_id: cid });
        assert_eq!(realm.imported.lock().unwrap().len(), 1);

        // Rev 2
        let tid2 = Uuid::now_v7();
        let payload2 = b"character_revision_2_newer_data".to_vec();
        let hash2 = hex::encode(Sha256::digest(&payload2));
        let r = s.handle(&mut session, &p, "K", Request::TransferOffer {
            transfer_id: tid2,
            character_id: cid,
            canonical_revision: 2,
            content_hash: hash2,
            total_size: payload2.len(),
            collections: BTreeMap::new(),
        });
        assert!(matches!(r, Response::TransferReady { .. }));
        let d2 = base64::engine::general_purpose::STANDARD.encode(&payload2);
        s.handle(&mut session, &p, "K", Request::TransferChunk { transfer_id: tid2, offset: 0, data: d2 });
        let Response::TransferCommitted { local_guid: guid2, .. } = s.handle(&mut session, &p, "K", Request::TransferCommit { transfer_id: tid2 }) else { panic!() };
        s.handle(&mut session, &p, "K", Request::TransferAck { transfer_id: tid2, character_id: cid });

        // Same local guid, updated, not duplicate
        assert_eq!(guid1, guid2);
        assert_eq!(realm.imported.lock().unwrap().len(), 1, "update, not duplicate import");
    }
}
