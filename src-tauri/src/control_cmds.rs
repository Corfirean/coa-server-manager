//! The control plane in the interface (Phase 12): joining a realm (an account made or reused, the game started against the realm's address), linking an account that
//! already exists, claiming a legacy character, and the owner's choices for their own realm. The work is in `coa_core::control`; this file only connects it to the app.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use coa_core::control::backend::LocalBackend;
use coa_core::control::host_manager::{HostControl, HostControlRealm};
use coa_core::control::join::{ControlFail, Credentials, PlayerControl, RealmTarget};
use coa_core::control::secrets;
use coa_core::control::store::ControlStore;
use coa_core::portable::service::{self as portable_service, PortableRuntime, ServiceError};
use coa_core::realm_registry::{FileKeyStore, KeyStore};
use coa_core::registry::Registry;
use coa_registry_proto::RealmId;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::{data_dir, remote_dir, AppState, REMOTE_CLIENT_ID};

fn fail(code: &str, message: impl Into<String>) -> ServiceError {
    ServiceError { code: code.into(), message: message.into(), notes: vec![] }
}

fn from_control(e: ControlFail) -> ServiceError {
    fail(&e.code(), e.to_string())
}

fn registry_url() -> Result<String, ServiceError> {
    let settings = coa_core::realm_registry::settings::load(&data_dir().join("registry")).map_err(|e| fail("registry", e.to_string()))?;
    Ok(std::env::var("COA_REGISTRY_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| settings.effective_url().to_string()))
}

fn player(state: &State<'_, AppState>) -> Result<Arc<PlayerControl>, ServiceError> {
    state.control.clone().ok_or_else(|| fail("stopped", "The server connection service could not start; see the Manager's log."))
}

fn realm_id(text: &str) -> Result<RealmId, ServiceError> {
    RealmId::parse(text).map_err(|e| fail("invalid", e.to_string()))
}

/// The folder of the control plane's own state: `control/player` and `control/host` (databases) and `secrets` (protected).
fn control_dir() -> PathBuf {
    data_dir().join("control")
}

/// Start both halves: the Player's operations and the Host's links. Either may be missing without stopping the Manager.
pub(crate) fn start(dir: &std::path::Path, portable: Option<Arc<PortableRuntime>>) -> (Option<Arc<PlayerControl>>, Option<Arc<HostControl>>) {
    let base = dir.join("control");
    let secrets: Arc<dyn secrets::SecretStore> = Arc::from(secrets::default_store(&dir.join("secrets")));
    let player = match PlayerControl::open(&base.join("player"), secrets) {
        Ok(p) => Some(Arc::new(p)),
        Err(e) => {
            tracing::error!(error = %e, "the player control service did not start");
            None
        }
    };
    let host = portable.and_then(|portable| {
        let store = match ControlStore::open(&base.join("host")) {
            Ok(s) => Arc::new(Mutex::new(s)),
            Err(e) => {
                tracing::error!(error = %e, "the host control service did not start");
                return None;
            }
        };
        let installs_reg = Registry::at(dir.join("installs.json"));
        let backend = LocalBackend::new(portable, dir.join("registry"), dir.join("portable").join("realms"), Arc::new(move || installs_reg.list().unwrap_or_default()));
        let keys: Arc<dyn KeyStore> = Arc::new(FileKeyStore::new(dir.join("registry").join("keys")));
        let url = std::env::var("COA_REGISTRY_URL").ok().filter(|u| !u.is_empty());
        Some(Arc::new(HostControl::start(dir.join("registry"), keys, store, Arc::new(backend), url)))
    });
    (player, host)
}

#[derive(Serialize)]
pub(crate) struct ControlStatus {
    secret_store: String,
    player_id: Option<String>,
    hosting: Vec<HostControlRealm>,
    preferred_username: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    #[serde(default)]
    preferred_username: String,
}

fn prefs_file() -> PathBuf {
    control_dir().join("preferences.json")
}

fn load_prefs() -> Preferences {
    std::fs::read(prefs_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

#[tauri::command]
pub(crate) fn control_status(state: State<'_, AppState>) -> ControlStatus {
    ControlStatus {
        secret_store: state.control.as_ref().map(|c| c.secret_store_kind().to_string()).unwrap_or_default(),
        player_id: state.control.as_ref().map(|c| c.identity.player_id.to_string()),
        hosting: state.host_control.as_ref().map(|h| h.status()).unwrap_or_default(),
        preferred_username: load_prefs().preferred_username,
    }
}

#[tauri::command]
pub(crate) fn control_set_preferred_username(name: String) -> Result<String, ServiceError> {
    let trimmed = name.trim();
    let clean = if trimmed.is_empty() { String::new() } else { coa_control_proto::app::sanitize_username(trimmed) };
    std::fs::create_dir_all(control_dir()).map_err(|e| fail("other", e.to_string()))?;
    let bytes = serde_json::to_vec(&Preferences { preferred_username: clean.clone() }).map_err(|e| fail("other", e.to_string()))?;
    coa_core::fsx::atomic_write(&prefs_file(), &bytes).map_err(|e| fail("other", e.to_string()))?;
    Ok(clean)
}

/// How joining players reach a realm this Manager publishes: whether it creates their accounts, and the address a game client can use right now.
#[tauri::command]
pub(crate) async fn registry_set_access(state: State<'_, AppState>, local_id: String, existing_only: bool, route: Option<String>) -> Result<(), ServiceError> {
    let rt = state.realm_registry.clone().ok_or_else(|| fail("stopped", "the publishing service did not start"))?;
    tauri::async_runtime::spawn_blocking(move || rt.set_access(&local_id, existing_only, route.as_deref()).map_err(|e| fail("invalid", e.to_string())))
        .await
        .map_err(|e| fail("other", e.to_string()))?
}

#[derive(Serialize)]
pub(crate) struct JoinOutcome {
    /// `ready` (the game can start), `launched`, `needs_link`, `needs_relay`, `needs_client`, `incompatible`, `needs_transfer`.
    status: String,
    username: Option<String>,
    account_created: bool,
    password_reset: bool,
    route: Option<String>,
    notes: Vec<portable_service::Note>,
}

impl JoinOutcome {
    fn stop(status: &str, username: Option<String>) -> Self {
        Self { status: status.into(), username, account_created: false, password_reset: false, route: None, notes: vec![] }
    }
}

fn target_of(url: &str, rid: &RealmId) -> Result<(RealmTarget, coa_registry_proto::RealmDetail), ServiceError> {
    let client = coa_core::realm_registry::BrowseClient::new(url).map_err(|e| fail("registry", e.to_string()))?;
    let detail = client.detail(rid).map_err(|e| fail("registry", e.to_string()))?;
    let target = RealmTarget::from_detail(url, &detail).map_err(|e| fail("registry", e.to_string()))?;
    Ok((target, detail))
}

/// Join a server: an account is made on it (or the one the player has is reused), the character is checked against the server, and when the server can be reached
/// the game starts against it. Everything that is not possible yet is reported as a status, not as a failure.
#[tauri::command]
pub(crate) async fn join_realm(state: State<'_, AppState>, realm: String, character: Option<String>, launch: bool) -> Result<JoinOutcome, ServiceError> {
    let pc = player(&state)?;
    let rid = realm_id(&realm)?;
    let prefer = Some(load_prefs().preferred_username).filter(|n| !n.is_empty());
    let character_for_check = character.clone();
    let portable = state.portable.clone();
    let mut outcome = tauri::async_runtime::spawn_blocking(move || -> Result<JoinOutcome, ServiceError> {
        let url = registry_url()?;
        let (target, detail) = target_of(&url, &rid)?;
        let mut ch = pc.connect(&target).map_err(from_control)?;
        let (_automatic, existing_only, route) = pc.welcome(&mut ch).map_err(from_control)?;
        if pc.credentials(&rid).map_err(|e| fail("other", e.to_string()))?.is_none() && existing_only {
            return Ok(JoinOutcome::stop("needs_link", None));
        }
        let account = pc.ensure_account(&mut ch, &rid, prefer.as_deref()).map_err(from_control)?;
        let mut out = JoinOutcome { status: "ready".into(), username: Some(account.username), account_created: account.created, password_reset: account.password_reset, route: route.clone(), notes: vec![] };
        if let Some(character) = character_for_check {
            let portable = portable.ok_or_else(|| fail("stopped", "The portable play service could not start."))?;
            let caps = serde_json::to_value(&detail.capabilities).map_err(|e| fail("other", e.to_string()))?;
            let id = character.clone();
            let pre = portable.call(move |s| s.preflight_advert(&id, &caps))?.map_err(|e| e)?;
            out.notes = pre.notes.clone();
            if pre.step == portable_service::Step::Blocked {
                ch.close();
                out.status = "incompatible".into();
                return Ok(out);
            }
            let here = pc.claimed_here(&rid).map_err(from_control)?;
            if !here.iter().any(|(id, _)| id.to_string() == character) {
                // Phase 12.1: Transfer canonical character to remote Host over the encrypted control channel
                let char_id_str = character.clone();
                let bundle = portable.call(move |s| s.export_for_remote_transfer(&char_id_str))?.map_err(|e| e)?;
                let cuuid = bundle.character_id.as_uuid();
                let _outcome = pc.transfer_character(&mut ch, cuuid, 1, &bundle.payload, bundle.content_hash, bundle.collections).map_err(from_control)?;
                let _ = pc.acknowledge(&mut ch, cuuid, &rid, &character);
            }
        }
        if route.is_none() {
            match pc.allocate_relay(&mut ch) {
                Ok(relay_alloc) => {
                    let r = format!("{}:{}", relay_alloc.relay_host, relay_alloc.auth_port);
                    out.route = Some(r);
                    out.status = "ready".into();
                }
                Err(_) => {
                    out.status = "needs_relay".into();
                }
            }
        }
        ch.close();
        Ok(out)

    })
    .await
    .map_err(|e| fail("other", e.to_string()))??;

    if outcome.status == "ready" {
        let host = outcome.route.clone().unwrap_or_default();
        let client_ready = {
            let profile = coa_core::remote_client::load(&remote_dir()).map_err(|e| fail("other", e.to_string()))?;
            profile.client_path.as_deref().is_some_and(|p| coa_core::client::detect(std::path::Path::new(p), None).is_some())
        };
        if !client_ready {
            outcome.status = "needs_client".into();
        } else if launch {
            let dir = remote_dir();
            let mut profile = coa_core::remote_client::load(&dir).map_err(|e| fail("other", e.to_string()))?;
            profile.host = host;
            coa_core::remote_client::save(&dir, &profile).map_err(|e| fail("launch", e.to_string()))?;
            super::play(state, REMOTE_CLIENT_ID.to_string()).await.map_err(|e| fail("launch", e.technical))?;
            outcome.status = "launched".into();
        }
    }
    Ok(outcome)
}

/// Link an account the server already has (one time). The name and password go to the server's Manager inside the encrypted channel and are checked against the server's own
/// login records; they are then kept in the protected store.
#[tauri::command]
pub(crate) async fn join_link(state: State<'_, AppState>, realm: String, login: String, password: String) -> Result<String, ServiceError> {
    let pc = player(&state)?;
    let rid = realm_id(&realm)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (target, _) = target_of(&registry_url()?, &rid)?;
        let mut ch = pc.connect(&target).map_err(from_control)?;
        let name = pc.link_existing(&mut ch, &rid, login.trim(), &password).map_err(from_control)?;
        ch.close();
        Ok(name)
    })
    .await
    .map_err(|e| fail("other", e.to_string()))?
}

#[derive(Serialize)]
pub(crate) struct AccountView {
    username: String,
    kind: String,
    has_password: bool,
}

#[tauri::command]
pub(crate) fn join_account(state: State<'_, AppState>, realm: String) -> Result<Option<AccountView>, ServiceError> {
    let pc = player(&state)?;
    let rid = realm_id(&realm)?;
    let known = pc.known_account(&rid).map_err(from_control)?;
    let has = pc.credentials(&rid).map_err(|e| fail("other", e.to_string()))?.is_some();
    Ok(known.map(|(username, kind)| AccountView { username, kind: kind.as_str().into(), has_password: has }))
}

/// The login the game asks for. Only on the player's explicit request ("Show" / "Copy"); never logged.
#[tauri::command]
pub(crate) fn join_credentials(state: State<'_, AppState>, realm: String) -> Result<Option<Credentials>, ServiceError> {
    let pc = player(&state)?;
    pc.credentials(&realm_id(&realm)?).map_err(|e| fail("other", e.to_string()))
}

#[tauri::command]
pub(crate) fn join_forget(state: State<'_, AppState>, realm: String) -> Result<(), ServiceError> {
    player(&state)?.forget(&realm_id(&realm)?).map_err(from_control)
}

#[derive(Serialize)]
pub(crate) struct RemoteCharacter {
    name: String,
    class: u32,
    race: u32,
    level: u32,
    eligible: bool,
    reasons: Vec<String>,
    yours: bool,
}

/// "Characters on this server": the characters of the player's account on a realm, read by that realm's Manager.
#[tauri::command]
pub(crate) async fn join_characters(state: State<'_, AppState>, realm: String) -> Result<Vec<RemoteCharacter>, ServiceError> {
    let pc = player(&state)?;
    let rid = realm_id(&realm)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (target, _) = target_of(&registry_url()?, &rid)?;
        let mut ch = pc.connect(&target).map_err(from_control)?;
        let list = pc.list_characters(&mut ch).map_err(from_control)?;
        ch.close();
        Ok(list.into_iter().map(|c| RemoteCharacter { name: c.name, class: c.class, race: c.race, level: c.level, eligible: c.eligible, reasons: c.reasons, yours: c.yours }).collect())
    })
    .await
    .map_err(|e| fail("other", e.to_string()))?
}

/// "Make portable": the realm's Manager checks, over the encrypted channel, that the character belongs to the player's account on that realm, that the account is at the character
/// selection screen and that the character is offline and eligible; it exports the character and the player's Manager stores it as revision 1 of a portable character.
#[tauri::command]
pub(crate) async fn join_claim(state: State<'_, AppState>, realm: String, name: String) -> Result<portable_service::CharacterView, ServiceError> {
    let pc = player(&state)?;
    let rid = realm_id(&realm)?;
    let portable = state.portable.clone().ok_or_else(|| fail("stopped", "The portable play service could not start."))?;
    tauri::async_runtime::spawn_blocking(move || {
        let (target, _) = target_of(&registry_url()?, &rid)?;
        let mut ch = pc.connect(&target).map_err(from_control)?;
        let list = pc.list_characters(&mut ch).map_err(from_control)?;
        // the character's name is what the player chose; a name is unique on a realm
        let _ = list.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).ok_or_else(|| fail("not_yours", "That character is not on your account on this server."))?;
        let token = list.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).map(|c| c.token).unwrap_or(0);
        let claimed = pc.claim(&mut ch, token).map_err(from_control)?;
        let server_id = format!("rr-{rid}");
        let cid = coa_core::portable::ids::CharacterId::from_uuid(claimed.character_id).map_err(|e| fail("other", e.to_string()))?;
        let (payload, hash, collections) = (claimed.payload.clone(), claimed.sha256, claimed.collections.clone());
        let view = portable.call(move |s| s.adopt_claim(&server_id, cid, &payload, &hash, &collections))??;
        pc.acknowledge(&mut ch, claimed.character_id, &rid, &view.name).map_err(from_control)?;
        ch.close();
        Ok(view)
    })
    .await
    .map_err(|e| fail("other", e.to_string()))?
}
