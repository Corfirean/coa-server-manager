//! The player's operations on a remote realm, over a verified [`PlayerChannel`]: get or reuse an account, link an existing one, list and claim characters.
//! The player's realm credentials are kept in the protected store and nowhere else; the control database remembers only account names.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use coa_control_proto::app::{self, AppError, CharacterEntry, Request, Response};
use coa_control_proto::coord;
use coa_registry_proto::{RealmDetail, RealmId};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::identity::PlayerIdentity;
use super::player::PlayerChannel;
use super::secrets::SecretStore;
use super::store::{AccountKind, ControlStore};
use super::transport::{self, LinkError};
use crate::{Error, Result};

/// Where and as whom to reach a realm.
#[derive(Clone, Debug)]
pub struct RealmTarget {
    pub realm_id: RealmId,
    pub key: VerifyingKey,
    /// The Coordinator's player endpoint (no query).
    pub coordinator: String,
}

impl RealmTarget {
    /// From the Registry's public record of the realm; the key published there is what the Host must prove it holds.
    pub fn from_detail(registry_url: &str, detail: &RealmDetail) -> Result<RealmTarget> {
        let key = coord::decode_public_key(&detail.public_key).map_err(|e| Error::Invalid(e.to_string()))?;
        let coordinator = transport::coordinator_url(registry_url, "/coord/v1/player").ok_or_else(|| Error::Invalid("the Registry address is not usable for a connection".into()))?;
        Ok(RealmTarget { realm_id: detail.realm_id, key, coordinator })
    }
}

#[derive(Debug)]
pub enum ControlFail {
    Link(LinkError),
    App { code: AppError, message: String },
    Local(String),
}

impl ControlFail {
    pub fn code(&self) -> String {
        match self {
            ControlFail::Link(e) => e.code().to_string(),
            ControlFail::App { code, .. } => serde_json::to_value(code).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "error".into()),
            ControlFail::Local(_) => "local".into(),
        }
    }
}

impl std::fmt::Display for ControlFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlFail::Link(e) => write!(f, "{e}"),
            ControlFail::App { message, .. } => write!(f, "{message}"),
            ControlFail::Local(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ControlFail {}

impl From<LinkError> for ControlFail {
    fn from(e: LinkError) -> Self {
        ControlFail::Link(e)
    }
}

impl From<Error> for ControlFail {
    fn from(e: Error) -> Self {
        ControlFail::Local(e.to_string())
    }
}

type R<T> = std::result::Result<T, ControlFail>;

/// What the player types into the game's login window. The password is shown or copied on request and never logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({}, ****)", self.username)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccountOutcome {
    pub username: String,
    pub created: bool,
    pub password_reset: bool,
}

#[derive(Debug, Clone)]
pub struct Claimed {
    pub character_id: Uuid,
    pub payload: Vec<u8>,
    pub sha256: [u8; 32],
    pub collections: BTreeMap<String, Vec<u32>>,
}

pub struct PlayerControl {
    pub identity: PlayerIdentity,
    secrets: Arc<dyn SecretStore>,
    store: Mutex<ControlStore>,
}

fn secret_name(realm: &RealmId) -> String {
    format!("realm-{}", realm.to_string().replace('-', ""))
}

fn clock() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn expect<T>(response: Response, ok: impl FnOnce(Response) -> std::result::Result<T, Response>) -> R<T> {
    match ok(response) {
        Ok(v) => Ok(v),
        Err(Response::Error { code, message }) => Err(ControlFail::App { code, message }),
        Err(_) => Err(ControlFail::Link(LinkError::Protocol("the answer does not match the question".into()))),
    }
}

impl PlayerControl {
    pub fn open(dir: &Path, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        let identity = PlayerIdentity::load_or_create(secrets.as_ref())?;
        Ok(Self { identity, secrets, store: Mutex::new(ControlStore::open(dir)?) })
    }

    pub fn secret_store_kind(&self) -> &'static str {
        self.secrets.kind()
    }

    fn store(&self) -> R<std::sync::MutexGuard<'_, ControlStore>> {
        self.store.lock().map_err(|_| ControlFail::Local("the control database is busy".into()))
    }

    /// The credentials the realm has confirmed (a password still waiting for the realm's confirmation is not one).
    pub fn credentials(&self, realm: &RealmId) -> Result<Option<Credentials>> {
        Ok(self.raw_credentials(realm)?.filter(|c| !c.username.is_empty()))
    }

    fn raw_credentials(&self, realm: &RealmId) -> Result<Option<Credentials>> {
        let Some(bytes) = self.secrets.get(&secret_name(realm))? else { return Ok(None) };
        serde_json::from_slice(&bytes).map(Some).map_err(|_| Error::Invalid("the saved credentials for this server are damaged".into()))
    }

    fn save_credentials(&self, realm: &RealmId, c: &Credentials) -> Result<()> {
        self.secrets.put(&secret_name(realm), &serde_json::to_vec(c)?)
    }

    /// Forget the account on a realm (the saved password and the name). The account itself stays on the realm.
    pub fn forget(&self, realm: &RealmId) -> R<()> {
        self.secrets.delete(&secret_name(realm))?;
        self.store()?.player_realm_remove(&realm.to_string())?;
        Ok(())
    }

    pub fn known_account(&self, realm: &RealmId) -> R<Option<(String, AccountKind)>> {
        Ok(self.store()?.player_realm(&realm.to_string())?.map(|r| (r.username, r.kind)))
    }

    /// Open a verified channel to the realm's Host. The realm's key is pinned the first time: the Registry says which key a realm id has, and a Registry (or anything between) that later
    /// names another key for the same id is refused here, before anything is sent.
    pub fn connect(&self, target: &RealmTarget) -> R<PlayerChannel> {
        let id = target.realm_id.to_string();
        let key = coord::encode_public_key(&target.key);
        match self.store()?.realm_pin(&id)? {
            Some(pinned) if pinned != key => return Err(ControlFail::Link(LinkError::Auth("this realm's key is not the one it had when you first joined it".into()))),
            _ => {}
        }
        let channel = PlayerChannel::connect(&target.coordinator, &target.realm_id, &target.key, &self.identity, &clock)?;
        self.store()?.realm_pin_set(&id, &key)?;
        Ok(channel)
    }

    pub fn welcome(&self, ch: &mut PlayerChannel) -> R<(bool, bool, Option<String>)> {
        let r = ch.request(&Request::Hello { protocol: coa_control_proto::CONTROL_PROTOCOL_VERSION, client: "coa-manager".into() })?;
        expect(r, |r| match r {
            Response::Welcome { automatic, existing_only, route, .. } => Ok((automatic, existing_only, route)),
            other => Err(other),
        })
    }

    /// The account on this realm: reused when the player has one, created otherwise (on realms that create accounts). The generated password is saved *before* it is
    /// sent, so a crash between the Host creating the account and the player learning of it leaves a password that the next call sets again.
    pub fn ensure_account(&self, ch: &mut PlayerChannel, realm: &RealmId, preferred: Option<&str>) -> R<AccountOutcome> {
        let saved = self.raw_credentials(realm)?;
        // saved credentials with no account name are a password that was made but never confirmed by the realm: it is sent again, as a new one
        let have = saved.as_ref().is_some_and(|c| !c.username.is_empty());
        let password = saved.as_ref().map(|c| c.password.clone()).unwrap_or_else(app::generate_password);
        if saved.is_none() {
            self.save_credentials(realm, &Credentials { username: String::new(), password: password.clone() })?;
        }
        let r = ch.request(&Request::Provision { desired: preferred.map(str::to_string), password: password.clone(), have_credentials: have })?;
        let (username, created, reset) = expect(r, |r| match r {
            Response::Provisioned { username, created, reset } => Ok((username, created, reset)),
            other => Err(other),
        })
        .inspect_err(|e| {
            // a refusal means the pending password will never be used; a lost connection keeps it for the next try
            if !have && matches!(e, ControlFail::App { .. }) {
                let _ = self.secrets.delete(&secret_name(realm));
            }
        })?;
        self.save_credentials(realm, &Credentials { username: username.clone(), password })?;
        // an account the player linked stays a linked one when the realm confirms it again
        let kind = match self.store()?.player_realm(&realm.to_string())? {
            Some(known) if known.username == username => known.kind,
            _ => AccountKind::Generated,
        };
        self.store()?.player_realm_set(&realm.to_string(), &username, kind)?;
        Ok(AccountOutcome { username, created, password_reset: reset })
    }

    /// One-time link of an account the realm already has.
    pub fn link_existing(&self, ch: &mut PlayerChannel, realm: &RealmId, login: &str, password: &str) -> R<String> {
        let r = ch.request(&Request::Link { login: login.to_string(), password: password.to_string() })?;
        let username = expect(r, |r| match r {
            Response::Linked { username } => Ok(username),
            other => Err(other),
        })?;
        self.save_credentials(realm, &Credentials { username: username.clone(), password: password.to_string() })?;
        self.store()?.player_realm_set(&realm.to_string(), &username, AccountKind::Linked)?;
        Ok(username)
    }

    pub fn list_characters(&self, ch: &mut PlayerChannel) -> R<Vec<CharacterEntry>> {
        let r = ch.request(&Request::ListCharacters)?;
        expect(r, |r| match r {
            Response::Characters { characters } => Ok(characters),
            other => Err(other),
        })
    }

    pub fn claim(&self, ch: &mut PlayerChannel, token: u32) -> R<Claimed> {
        let r = ch.request(&Request::Claim { token })?;
        let (character_id, sha256, payload, collections) = expect(r, |r| match r {
            Response::Claimed { character_id, sha256, payload, collections } => Ok((character_id, sha256, payload, collections)),
            other => Err(other),
        })?;
        let payload = base64::engine::general_purpose::STANDARD.decode(payload).map_err(|_| ControlFail::Link(LinkError::Protocol("the character data is not valid".into())))?;
        let hash: [u8; 32] = hex::decode(&sha256).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| ControlFail::Link(LinkError::Protocol("the character hash is not valid".into())))?;
        Ok(Claimed { character_id, payload, sha256: hash, collections })
    }

    /// Tell the Host the character is safely stored here; only then is the claim final.
    pub fn acknowledge(&self, ch: &mut PlayerChannel, character_id: Uuid, realm: &RealmId, name: &str) -> R<()> {
        self.store()?.player_claim_add(&realm.to_string(), &character_id, name)?;
        let r = ch.request(&Request::ClaimAck { character_id })?;
        expect(r, |r| match r {
            Response::Done => Ok(()),
            other => Err(other),
        })
    }

    pub fn route(&self, ch: &mut PlayerChannel) -> R<Option<String>> {
        let r = ch.request(&Request::Route)?;
        expect(r, |r| match r {
            Response::RouteInfo { address } => Ok(address),
            other => Err(other),
        })
    }

    pub fn claimed_here(&self, realm: &RealmId) -> R<Vec<(Uuid, String)>> {
        Ok(self.store()?.player_claims(&realm.to_string())?)
    }
}
