//! The orchestration of portable play: what the player's one click does, and what the Manager keeps doing while the game runs.
//!
//! This is a thin layer over the engines of Phases 1-8 (export, import, update, projection, sessions, reconciliation): it chooses and orders
//! them, keeps their stores, and turns their state into words. It decides nothing about a character's content.
//!
//! ```text
//!   Play(character, realm)
//!     realm reachable? (database, console)           -> Offline
//!     capabilities of the realm's core (live)         -> remembered, compared with the last ones
//!     compatibility of the character with them        -> Incompatible stops here, nothing was written
//!     the character is on the realm?
//!        no                                           -> offer, import online (projected when above the cap), bind
//!        yes, session waiting or running              -> nothing to do
//!        yes, realm copy behind the canonical one     -> a divergence is a conflict, else the realm must be stopped and updated
//!        yes, up to date                              -> arm a session on the copy
//!   tick() every second:
//!     the Host of every realm with a live session: baseline at the game's login, checkpoints, final sync at logout,
//!     messages to the Owner store and its answers back, the account collections, the realm's progression
//! ```
//!
//! Everything is persisted by the stores of the engines; what this layer keeps in memory (the throttling of the Host, the last look at a
//! realm) is rebuilt after a restart, and the first tick after it checkpoints at once.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::ra::Ra;

use super::super::capabilities::RealmCapabilities;
use super::super::compat::{self, CompatibilityReport, Inputs, Operation, Outcome, Topic};
use super::super::error::PortableError;
use super::super::extension::ExtensionRegistry;
use super::super::ids::{CharacterId, SessionId};
use super::super::projection::{Decision, Oracle, ProgressionPin, SuppliedDecision};
use super::super::realm::knowledge::RealmKnowledge;
use super::super::realm::online::import_character_online_on;
use super::super::realm::profile::{probe_capabilities, with_remembered_progression};
use super::super::realm::project::decide_with_core;
use super::super::realm::{self, ImportOptions};
use super::super::session::bridge::{RealmBridge, RowState, SessionRow};
use super::super::session::live::LiveBridge;
use super::super::session::protocol::OwnerAck;
use super::super::session::{HostConfig, HostEvent, HostMemory, HostService, OwnerService};
use super::super::store::{AckEffect, HostState, Store};
use super::access::{self, Descriptor, Kind, RealmAccess};
use super::view::*;

/// What the service cannot do itself: starting and stopping a server this Manager installed. The application provides it.
pub trait ServerControl: Send {
    /// Stop everything of the installation (world, authentication, database).
    fn stop_all(&self, install_id: &str) -> crate::Result<()>;
    /// Start the database only.
    fn start_database(&self, install_id: &str) -> crate::Result<()>;
    /// Start everything of the installation.
    fn start_all(&self, install_id: &str) -> crate::Result<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceError {
    pub code: String,
    pub message: String,
    pub notes: Vec<Note>,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}

type Res<T> = std::result::Result<T, ServiceError>;

fn fail(code: &str, message: impl Into<String>) -> ServiceError {
    ServiceError {
        code: code.into(),
        message: message.into(),
        notes: vec![],
    }
}

impl From<PortableError> for ServiceError {
    fn from(e: PortableError) -> Self {
        let code = match &e {
            PortableError::Incompatible { .. } => "incompatible",
            PortableError::UpdateConflicts(_) => "conflict",
            PortableError::ProjectionNeedsRunningCore { .. } => "update_required",
            PortableError::ProgressionChanged(_) => "update_required",
            PortableError::AlreadyOnRealm { .. } => "already_on_realm",
            PortableError::SessionOpen => "session_open",
            PortableError::RealmRead(_) => "realm_offline",
            _ => "other",
        };
        fail(code, e.to_string())
    }
}

impl From<crate::Error> for ServiceError {
    fn from(e: crate::Error) -> Self {
        fail("other", e.to_string())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    accounts: BTreeMap<String, String>,
}

/// What was last seen of a realm. Refreshed by the tick, never by asking for the state.
#[derive(Default)]
struct Obs {
    db_ok: bool,
    ra_ok: bool,
    seen: Option<Instant>,
    caps: Option<RealmCapabilities>,
    caps_at: Option<Instant>,
    rows: HashMap<u32, SessionRow>,
    memory: HostMemory,
    profile_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct Saved {
    revision: u64,
    at: String,
}

/// A realm character exported for a remote player (see [`PortableService::export_for_claim`]).
#[derive(Debug, Clone)]
pub struct ClaimBundle {
    pub character_id: CharacterId,
    pub payload: Vec<u8>,
    pub content_hash: [u8; 32],
    pub collections: BTreeMap<String, Vec<u32>>,
}

pub struct PortableService {
    dir: PathBuf,
    owner: Store,
    host: Store,
    owner_profile: crate::portable::ProfileId,
    host_profile: crate::portable::ProfileId,
    installs: Vec<(String, PathBuf)>,
    realms: Vec<RealmAccess>,
    broken_descriptors: Vec<(PathBuf, String)>,
    obs: HashMap<String, Obs>,
    settings: Settings,
    errors: VecDeque<ErrorEntry>,
    op: Option<OperationView>,
    conflicts: HashSet<(CharacterId, String)>,
    reproject: HashSet<(CharacterId, String)>,
    saved: HashMap<(CharacterId, String), Saved>,
    verdicts: HashMap<(CharacterId, String), Verdict>,
    control: Option<Box<dyn ServerControl>>,
    host_config: HostConfig,
    started: Instant,
    last_tick: Option<Instant>,
    published: Option<Arc<Mutex<PortableState>>>,
    registry: Arc<ExtensionRegistry>,
}

const OBS_EVERY: Duration = Duration::from_secs(5);
const CAPS_EVERY: Duration = Duration::from_secs(60);
const MAX_ERRORS: usize = 30;

fn now_text() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn class_name(class: &str) -> String {
    let id: u32 = class
        .rsplit(':')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    match id {
        12 => "Barbarian",
        13 => "Witch Doctor",
        14 => "Felsworn",
        15 => "Witch Hunter",
        16 => "Stormbringer",
        17 => "Knight of Xoroth",
        18 => "Guardian",
        19 => "Templar",
        20 => "Blood Mage",
        21 => "Ranger",
        22 => "Chronomancer",
        23 => "Necromancer",
        24 => "Pyromancer",
        25 => "Cultist",
        26 => "Starcaller",
        27 => "Sun Cleric",
        28 => "Tinker",
        29 => "Venomancer",
        30 => "Reaper",
        31 => "Primalist",
        32 => "Runemaster",
        _ => return format!("Class {id}"),
    }
    .to_string()
}

fn topic_code(t: &Topic) -> (&'static str, Option<(&'static str, String)>) {
    match t {
        Topic::Ruleset => ("ruleset", None),
        Topic::CharacterFormat => ("format", None),
        Topic::OnlineImport => ("online_import", None),
        Topic::RuntimeSessions => ("sessions", None),
        Topic::ClientData => ("client_data", None),
        Topic::Wardrobe => ("wardrobe", None),
        Topic::Collection(k) => ("collection", Some(("kind", k.clone()))),
        Topic::Extension(ns) => ("extension", Some(("namespace", ns.clone()))),
        Topic::Progression => ("progression", None),
    }
}

/// The words of a compatibility report: what is not simply compatible, as codes the interface translates.
pub fn notes_of(report: &CompatibilityReport) -> Vec<Note> {
    let mut out = Vec::new();
    for outcome in &report.outcomes {
        let mut params = BTreeMap::new();
        let (topic, code, detail) = match outcome {
            Outcome::Compatible(_) => continue,
            Outcome::Projected { topic, from, to } => {
                params.insert("from".to_string(), from.to_string());
                params.insert("to".to_string(), to.to_string());
                (topic, "projection".to_string(), outcome.to_string())
            }
            Outcome::Held {
                topic,
                held,
                applicable,
                reason,
            } => {
                params.insert("held".into(), held.to_string());
                params.insert("applicable".into(), applicable.to_string());
                (
                    topic,
                    format!("held_{}", topic_code(topic).0),
                    reason.clone(),
                )
            }
            Outcome::Unsupported { topic, reason } => (
                topic,
                format!("unsupported_{}", topic_code(topic).0),
                reason.clone(),
            ),
            Outcome::Blocking { topic, reason } => (
                topic,
                format!("blocked_{}", topic_code(topic).0),
                reason.clone(),
            ),
        };
        if let Some((k, v)) = topic_code(topic).1 {
            params.insert(k.to_string(), v);
        }
        out.push(Note {
            code,
            params,
            detail,
        });
    }
    out
}

fn verdict_of(v: compat::Verdict) -> Verdict {
    match v {
        compat::Verdict::Compatible => Verdict::Compatible,
        compat::Verdict::Degraded => Verdict::Degraded,
        compat::Verdict::Incompatible => Verdict::Incompatible,
    }
}

fn projection_of(report: &CompatibilityReport) -> Option<(u32, u32)> {
    report.outcomes.iter().find_map(|o| match o {
        Outcome::Projected { from, to, .. } => Some((*from, *to)),
        _ => None,
    })
}

fn valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.@".contains(&b))
}

impl PortableService {
    /// Open the stores in `dir` (`owner/`, `host/`) and read the realms: the descriptors in `dir/realms` and the installations given.
    pub fn open(
        dir: &Path,
        installs: Vec<(String, PathBuf)>,
    ) -> Result<PortableService, ServiceError> {
        std::fs::create_dir_all(dir).map_err(|e| fail("storage", e.to_string()))?;
        let mut owner = Store::open(&dir.join("owner"))?;
        let mut host = Store::open(&dir.join("host"))?;
        let owner_profile = owner.default_profile()?;
        let host_profile = host.default_profile()?;
        let settings = std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut s = PortableService {
            dir: dir.to_path_buf(),
            owner,
            host,
            owner_profile,
            host_profile,
            installs,
            realms: vec![],
            broken_descriptors: vec![],
            obs: HashMap::new(),
            settings,
            errors: VecDeque::new(),
            op: None,
            conflicts: HashSet::new(),
            reproject: HashSet::new(),
            saved: HashMap::new(),
            verdicts: HashMap::new(),
            control: None,
            host_config: HostConfig {
                checkpoint_interval_secs: 30,
                collection_interval_secs: 300,
            },
            started: Instant::now(),
            last_tick: None,
            published: None,
            registry: Arc::new(ExtensionRegistry::new()),
        };
        s.reload_realms();
        Ok(s)
    }

    pub fn set_control(&mut self, control: Box<dyn ServerControl>) {
        self.control = Some(control);
    }

    pub fn set_host_config(&mut self, config: HostConfig) {
        self.host_config = config;
    }

    /// Where the snapshot of the state is published whenever something changes.
    pub fn set_publisher(&mut self, p: Arc<Mutex<PortableState>>) {
        self.published = Some(p);
        self.publish();
    }

    pub fn set_installs(&mut self, installs: Vec<(String, PathBuf)>) {
        if installs != self.installs {
            self.installs = installs;
            self.reload_realms();
        }
    }

    pub fn reload_realms(&mut self) {
        let (descriptors, broken) = access::load_descriptors(&self.dir.join("realms"));
        self.broken_descriptors = broken;
        let mut realms: Vec<RealmAccess> = self
            .installs
            .iter()
            .map(|(id, root)| RealmAccess::from_install(id, root))
            .collect();
        for d in descriptors {
            match RealmAccess::from_descriptor(d) {
                Ok(a) if !realms.iter().any(|r| r.id == a.id) => realms.push(a),
                Ok(a) => self.note_error(Some(&a.id), "two realms have the same id".to_string()),
                Err(e) => self.note_error(None, e.to_string()),
            }
        }
        let known: HashSet<String> = realms.iter().map(|r| r.id.clone()).collect();
        self.obs.retain(|k, _| known.contains(k));
        self.realms = realms;
        self.publish();
    }

    pub fn add_prepared_realm(&mut self, file: &Path) -> Res<RealmView> {
        let d = access::add_descriptor(&self.dir.join("realms"), file)?;
        self.reload_realms();
        self.refresh_obs(&d.id, true);
        self.realm_view(&d.id)
    }

    pub fn remove_prepared_realm(&mut self, id: &str) -> Res<()> {
        let access = self.access(id)?;
        if access.kind != Kind::Prepared {
            return Err(fail(
                "not_prepared",
                "Only a realm that was added from a file can be removed here.",
            ));
        }
        if self
            .host
            .host_live_sessions(id)
            .map_err(ServiceError::from)?
            .iter()
            .any(|s| s.state != HostState::Closed)
        {
            return Err(fail(
                "session_open",
                "A character is in play on this realm.",
            ));
        }
        access::remove_descriptor(&self.dir.join("realms"), id)?;
        self.reload_realms();
        Ok(())
    }

    fn access(&self, id: &str) -> Res<RealmAccess> {
        self.realms
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| fail("realm_missing", "That realm is not known to this Manager."))
    }

    fn cid(&self, id: &str) -> Res<CharacterId> {
        let cid: CharacterId = id
            .parse()
            .map_err(|_| fail("character_missing", "That character is not known."))?;
        self.owner
            .character(cid)
            .map(|_| cid)
            .map_err(|_| fail("character_missing", "That character is not known."))
    }

    fn note_error(&mut self, realm: Option<&str>, message: String) {
        if self.errors.len() >= MAX_ERRORS {
            self.errors.pop_front();
        }
        // a flood of the same message is one entry
        if self
            .errors
            .back()
            .is_some_and(|e| e.message == message && e.realm.as_deref() == realm)
        {
            return;
        }
        self.errors.push_back(ErrorEntry {
            at: now_text(),
            realm: realm.map(str::to_string),
            message,
        });
    }

    fn save_settings(&self) {
        let _ = crate::fsx::atomic_write_json(&self.dir.join("settings.json"), &self.settings);
    }

    // ---- looking at a realm -----------------------------------------------------------------------------------------------------

    fn refresh_obs(&mut self, id: &str, force: bool) {
        let Ok(access) = self.access(id) else { return };
        let due = self
            .obs
            .get(id)
            .and_then(|o| o.seen)
            .is_none_or(|t| t.elapsed() >= OBS_EVERY);
        if !due && !force {
            return;
        }
        let db_ok = access.db_reachable();
        let ra_ok = db_ok && access.ra().is_ok();
        let o = self.obs.entry(id.to_string()).or_default();
        o.db_ok = db_ok;
        o.ra_ok = ra_ok;
        o.seen = Some(Instant::now());
        if !ra_ok {
            o.caps_at = None;
        }
    }

    /// The realm's capabilities as its core reports them now (at most once a minute), remembered by the Host's store.
    fn capabilities(
        &mut self,
        access: &RealmAccess,
        db: &Db,
        ra: &mut Ra,
        force: bool,
    ) -> Res<RealmCapabilities> {
        if !force {
            if let Some(o) = self.obs.get(&access.id) {
                if let (Some(c), Some(at)) = (&o.caps, o.caps_at) {
                    if at.elapsed() < CAPS_EVERY {
                        return Ok(c.clone());
                    }
                }
            }
        }
        let data = access
            .data_dir
            .is_dir()
            .then_some(access.data_dir.as_path());
        let caps = probe_capabilities(db, data, Some(ra), &self.registry)
            .map_err(|e| fail("realm_unreadable", e.to_string()))?;
        let o = self.obs.entry(access.id.clone()).or_default();
        let memory = std::mem::take(&mut o.memory);
        let mut host =
            HostService::resume(&mut self.host, &access.id, self.host_config.clone(), memory);
        let observed = host.observe_profile(&caps, "live");
        let memory = host.into_memory();
        let o = self.obs.entry(access.id.clone()).or_default();
        o.memory = memory;
        o.caps = Some(caps.clone());
        o.caps_at = Some(Instant::now());
        o.profile_at = Some(Instant::now());
        observed.map_err(ServiceError::from)?;
        Ok(caps)
    }

    /// The profile of a realm that is not running: what its core last said, with its progression.
    fn remembered(&self, access: &RealmAccess, db: &Db) -> Option<RealmCapabilities> {
        let stored = self.host.realm_profile(&access.id).ok().flatten()?;
        let data = access
            .data_dir
            .is_dir()
            .then_some(access.data_dir.as_path());
        let schema = realm::probe(db).ok()?;
        let offline = realm::profile::assemble(db, &schema, data, None, &self.registry).ok()?;
        with_remembered_progression(offline, Some(&stored)).ok()
    }

    fn options(&self, access: &RealmAccess, caps: &RealmCapabilities) -> ImportOptions {
        let knowledge = access
            .data_dir
            .is_dir()
            .then(|| RealmKnowledge::from_data_dir(&access.data_dir).ok())
            .flatten()
            .map(Arc::new);
        ImportOptions {
            game_server_users: access.game_server_users.clone(),
            knowledge,
            capabilities: Some(Arc::new(caps.clone())),
            extensions: Some(self.registry.clone()),
            ..ImportOptions::default()
        }
    }

    fn account_id(&self, db: &Db, name: &str) -> Res<u32> {
        if !valid_account_name(name) {
            return Err(fail("account_missing", "That is not a game account name."));
        }
        let upper = realm::sqlenc::Val::text(name.to_ascii_uppercase()).sql();
        let out = db
            .query(&format!(
                "SELECT id FROM acore_auth.account WHERE username = {upper}"
            ))
            .map_err(|e| fail("realm_unreadable", e.to_string()))?;
        out.lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
            .ok_or_else(|| {
                fail(
                    "account_missing",
                    format!("The realm has no game account named {name}."),
                )
            })
    }

    // ---- views --------------------------------------------------------------------------------------------------------------------

    pub fn realm_view(&self, id: &str) -> Res<RealmView> {
        let a = self.access(id)?;
        let o = self.obs.get(id);
        let caps = o.and_then(|o| o.caps.as_ref());
        let stored = self.host.realm_profile(id).ok().flatten();
        let cap = caps
            .or(stored.as_ref())
            .and_then(|c| c.progression.as_ref())
            .map(|p| p.max_player_level);
        let portable = match caps.or(stored.as_ref()) {
            Some(c)
                if c.content
                    .supports(super::super::capabilities::Feature::RuntimeSessions) =>
            {
                "ready"
            }
            Some(_) if o.is_some_and(|o| o.ra_ok) => "setup",
            _ => "unknown",
        };
        Ok(RealmView {
            id: a.id.clone(),
            name: a.name.clone(),
            kind: a.kind,
            address: a.address.clone(),
            online: o.is_some_and(|o| o.ra_ok),
            database_ok: o.is_some_and(|o| o.db_ok),
            level_cap: cap,
            portable: portable.to_string(),
            game_account: self.settings.accounts.get(id).cloned(),
            installed_id: a.install_id.clone(),
        })
    }

    fn copy_status(
        &self,
        character: CharacterId,
        record_revision: u64,
        realm_id: &str,
    ) -> (PlayStatus, bool, Option<u32>, u64, String) {
        let mapping = self
            .host
            .server_mappings(character)
            .ok()
            .and_then(|m| m.into_iter().find(|m| m.server_id == realm_id));
        let Some(m) = mapping else {
            return (PlayStatus::Ready, false, None, 0, String::new());
        };
        let projected = self
            .host
            .projection_context(character, realm_id)
            .ok()
            .flatten()
            .map(|c| c.projected_level);
        let behind = m.last_revision < record_revision;
        let obs = self.obs.get(realm_id);
        let live = self
            .host
            .host_live_sessions(realm_id)
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.character_id == character);
        let stale = self.reproject.contains(&(character, realm_id.to_string()));
        let status = if self
            .op
            .as_ref()
            .is_some_and(|o| o.character_id == character.to_string() && o.realm_id == realm_id)
        {
            self.op
                .as_ref()
                .map(|o| o.status)
                .unwrap_or(PlayStatus::Preparing)
        } else if self.conflicts.contains(&(character, realm_id.to_string())) {
            PlayStatus::Conflict
        } else if !obs.is_some_and(|o| o.db_ok) {
            PlayStatus::Offline
        } else if let Some(s) = &live {
            let row = s.local_guid.and_then(|g| obs.and_then(|o| o.rows.get(&g)));
            let pending = self
                .host
                .host_outbox_pending(realm_id)
                .map(|p| p.iter().any(|m| m.session_id == s.session_id))
                .unwrap_or(false);
            match (s.state, row.map(|r| r.state)) {
                _ if stale || s.reproject => PlayStatus::UpdateRequired,
                (HostState::Armed, _) => PlayStatus::WaitingLogin,
                (_, Some(RowState::Ended)) => PlayStatus::Saving,
                (_, Some(RowState::Active | RowState::BaselineReady)) if pending => {
                    PlayStatus::Syncing
                }
                (_, Some(RowState::Active | RowState::BaselineReady)) => PlayStatus::Playing,
                _ if !obs.is_some_and(|o| o.ra_ok) => PlayStatus::Offline,
                _ => PlayStatus::Playing,
            }
        } else if behind || stale {
            PlayStatus::UpdateRequired
        } else if projected.is_some()
            || self.verdicts.get(&(character, realm_id.to_string())) == Some(&Verdict::Degraded)
        {
            PlayStatus::CompatWarning
        } else {
            PlayStatus::Ready
        };
        (status, behind, projected, m.last_revision, m.updated_at)
    }

    pub fn state(&self) -> PortableState {
        let mut characters = Vec::new();
        let list = self
            .owner
            .list_characters(self.owner_profile)
            .unwrap_or_default();
        for c in list.into_iter().filter(|c| !c.archived) {
            let mut copies = Vec::new();
            for m in self
                .host
                .server_mappings(c.character_id)
                .unwrap_or_default()
            {
                let Some(realm) = self.realms.iter().find(|r| r.id == m.server_id) else {
                    continue;
                };
                let (status, behind, projected_level, rev, at) =
                    self.copy_status(c.character_id, c.revision, &m.server_id);
                let degraded = projected_level.is_some()
                    || self.verdicts.get(&(c.character_id, m.server_id.clone()))
                        == Some(&Verdict::Degraded);
                copies.push(CopyView {
                    realm_id: m.server_id.clone(),
                    realm_name: realm.name.clone(),
                    status,
                    synced_revision: rev,
                    behind,
                    projected_level,
                    degraded,
                    updated_at: at,
                });
            }
            let active = copies
                .iter()
                .find(|x| {
                    matches!(
                        x.status,
                        PlayStatus::Playing
                            | PlayStatus::Saving
                            | PlayStatus::WaitingLogin
                            | PlayStatus::Syncing
                    )
                })
                .cloned();
            let status = active
                .as_ref()
                .map(|a| a.status)
                .or_else(|| {
                    copies
                        .iter()
                        .map(|c| c.status)
                        .find(|s| matches!(s, PlayStatus::Conflict | PlayStatus::UpdateRequired))
                })
                .unwrap_or(PlayStatus::Ready);
            characters.push(CharacterView {
                id: c.character_id.to_string(),
                name: c.name,
                class_name: class_name(&c.class),
                race: c.race,
                level: c.level,
                revision: c.revision,
                updated_at: c.updated_at,
                active_realm: active.map(|a| a.realm_name),
                status,
                copies,
            });
        }
        let realms = self
            .realms
            .iter()
            .filter_map(|r| self.realm_view(&r.id).ok())
            .collect();
        let sessions_open = self
            .realms
            .iter()
            .map(|r| {
                self.host
                    .host_live_sessions(&r.id)
                    .map(|v| v.len() as u32)
                    .unwrap_or(0)
            })
            .sum();
        PortableState {
            characters,
            realms,
            operation: self.op.clone(),
            runtime: RuntimeView {
                running: self.last_tick.is_some(),
                last_tick_secs: self.last_tick.map(|t| t.elapsed().as_secs()),
                sessions_open,
                errors: self.errors.iter().cloned().collect(),
            },
        }
    }

    fn publish(&self) {
        if let Some(p) = &self.published {
            if let Ok(mut guard) = p.lock() {
                *guard = self.state();
            }
        }
    }

    fn set_op(&mut self, character: CharacterId, realm: &str, status: PlayStatus) {
        self.op = Some(OperationView {
            character_id: character.to_string(),
            realm_id: realm.to_string(),
            status,
        });
        self.publish();
    }

    fn clear_op(&mut self) {
        self.op = None;
        self.publish();
    }

    // ---- compatibility -----------------------------------------------------------------------------------------------------------

    fn report(
        &self,
        character: &super::super::model::PortableCharacter,
        caps: &RealmCapabilities,
        opts: &ImportOptions,
        ops: &[Operation],
        live: bool,
    ) -> CompatibilityReport {
        let mut merged: Option<CompatibilityReport> = None;
        for op in ops {
            let r = compat::evaluate(&Inputs {
                operation: *op,
                model: character,
                capabilities: caps,
                knowledge: opts.knowledge.as_deref(),
                collections: &[],
                extensions: opts.extensions.as_deref(),
                projection_decider: live || opts.projection.is_some(),
            });
            merged = Some(match merged {
                None => r,
                Some(mut m) => {
                    for o in r.outcomes {
                        if !m.outcomes.iter().any(|e| e.topic() == o.topic()) {
                            m.outcomes.push(o);
                        }
                    }
                    m
                }
            });
        }
        merged.expect("an operation")
    }

    /// The same compatibility check for a realm that is only known by what it advertises (a row of the public list): the Phase 7/8 rules evaluated against the
    /// advertised capabilities, with nothing written anywhere. What cannot be known from an advertisement (the realm's client tables) is not assumed.
    pub fn preflight_advert(
        &mut self,
        character: &str,
        capabilities: &serde_json::Value,
    ) -> Res<PreflightView> {
        let cid = self.cid(character)?;
        let caps = RealmCapabilities::from_json(capabilities.to_string().as_bytes())
            .map_err(|e| fail("realm_unreadable", e.to_string()))?;
        let canonical = self.owner.load_current(cid)?;
        let opts = ImportOptions {
            capabilities: Some(Arc::new(caps.clone())),
            extensions: Some(self.registry.clone()),
            ..ImportOptions::default()
        };
        let report = self.report(
            &canonical,
            &caps,
            &opts,
            &[Operation::OnlineImport, Operation::RuntimeSession],
            true,
        );
        let verdict = verdict_of(report.verdict());
        let notes = notes_of(&report);
        let projection = projection_of(&report);
        let step = if verdict == Verdict::Incompatible {
            Step::Blocked
        } else {
            Step::Prepare
        };
        Ok(PreflightView {
            verdict,
            step,
            projection,
            notes,
            needs_account: false,
        })
    }

    /// What pressing Play would do, found by looking only: nothing is written to a realm or to a store.
    pub fn preflight(&mut self, character: &str, realm_id: &str) -> Res<PreflightView> {
        let cid = self.cid(character)?;
        let access = self.access(realm_id)?;
        self.refresh_obs(realm_id, true);
        let obs_ok = self
            .obs
            .get(realm_id)
            .map(|o| (o.db_ok, o.ra_ok))
            .unwrap_or((false, false));
        if !obs_ok.0 {
            return Ok(PreflightView {
                verdict: Verdict::Compatible,
                step: Step::Offline,
                projection: None,
                notes: vec![Note {
                    code: "realm_offline".into(),
                    params: BTreeMap::new(),
                    detail: "the realm's database cannot be reached".into(),
                }],
                needs_account: false,
            });
        }
        let db = access.db()?;
        let canonical = self.owner.load_current(cid)?;
        let record = self.owner.character(cid)?;
        let mapping = self
            .host
            .server_mappings(cid)?
            .into_iter()
            .find(|m| m.server_id == realm_id);
        let needs_account = mapping.is_none() && !self.settings.accounts.contains_key(realm_id);
        let live_session = self
            .host
            .host_live_sessions(realm_id)?
            .into_iter()
            .any(|s| s.character_id == cid);

        let (caps, running) = if obs_ok.1 {
            let mut ra = access.ra()?;
            (self.capabilities(&access, &db, &mut ra, false)?, true)
        } else {
            match self.remembered(&access, &db) {
                Some(c) => (c, false),
                None => {
                    return Ok(PreflightView {
                        verdict: Verdict::Compatible,
                        step: Step::Offline,
                        projection: None,
                        notes: vec![Note {
                            code: "realm_unknown".into(),
                            params: BTreeMap::new(),
                            detail: "the realm has never been seen running".into(),
                        }],
                        needs_account,
                    })
                }
            }
        };
        let opts = self.options(&access, &caps);
        let ops: &[Operation] = if mapping.is_none() {
            &[Operation::OnlineImport, Operation::RuntimeSession]
        } else {
            &[Operation::Update, Operation::RuntimeSession]
        };
        let report = self.report(&canonical, &caps, &opts, ops, running);
        let verdict = verdict_of(report.verdict());
        self.verdicts.insert((cid, realm_id.to_string()), verdict);
        let mut notes = notes_of(&report);
        let projection = projection_of(&report);
        let step = if verdict == Verdict::Incompatible {
            Step::Blocked
        } else if !running {
            Step::Offline
        } else if mapping.is_none() {
            Step::Prepare
        } else if live_session && !self.reproject.contains(&(cid, realm_id.to_string())) {
            Step::Resume
        } else {
            let behind = mapping
                .as_ref()
                .is_some_and(|m| m.last_revision < record.revision);
            let pin_stale = self.reproject.contains(&(cid, realm_id.to_string()));
            if !behind && !pin_stale && !self.pin_moved(cid, realm_id, &caps) {
                Step::Arm
            } else {
                match realm::preview_update(&db, &self.host, cid, realm_id, &opts) {
                    Ok(p) if !p.conflicts.is_empty() => {
                        self.conflicts.insert((cid, realm_id.to_string()));
                        notes.push(Note {
                            code: "conflict".into(),
                            params: BTreeMap::new(),
                            detail: p.conflicts.join("; "),
                        });
                        Step::Resolve
                    }
                    Ok(_) => Step::Restart,
                    Err(PortableError::RealmRead(_)) => Step::Restart,
                    Err(e) => {
                        notes.push(Note {
                            code: "unreadable".into(),
                            params: BTreeMap::new(),
                            detail: e.to_string(),
                        });
                        Step::Restart
                    }
                }
            }
        };
        self.publish();
        Ok(PreflightView {
            verdict,
            step,
            projection,
            notes,
            needs_account,
        })
    }

    fn pin_moved(&self, character: CharacterId, realm_id: &str, caps: &RealmCapabilities) -> bool {
        let Some(p) = &caps.progression else {
            return false;
        };
        match self.host.character_pin(character, realm_id).ok().flatten() {
            Some(pin) => {
                pin.progression_signature != p.progression_signature
                    || pin.max_player_level != p.max_player_level
                    || pin.policy_version != p.projection_policy_version
            }
            None => true,
        }
    }

    // ---- Play --------------------------------------------------------------------------------------------------------------------

    /// Prepare everything for a session of this character on this realm. Afterwards the game's login is the only step left.
    pub fn play(
        &mut self,
        character: &str,
        realm_id: &str,
        account: Option<&str>,
    ) -> Res<PlayView> {
        let cid = self.cid(character)?;
        let access = self.access(realm_id)?;
        let account = account
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string);
        if account.as_deref().is_some_and(|n| !valid_account_name(n)) {
            return Err(fail("account_missing", "That is not a game account name."));
        }
        if self.op.is_some() {
            return Err(fail("busy", "Another character is being prepared."));
        }
        self.set_op(cid, realm_id, PlayStatus::Preparing);
        let result = self.play_inner(cid, &access, account.as_deref());
        if result.is_ok() {
            if let Some(name) = &account {
                self.settings
                    .accounts
                    .insert(realm_id.to_string(), name.clone());
                self.save_settings();
            }
        }
        self.clear_op();
        if let Err(e) = &result {
            self.note_error(Some(realm_id), e.to_string());
        }
        result
    }

    fn play_inner(
        &mut self,
        cid: CharacterId,
        access: &RealmAccess,
        account: Option<&str>,
    ) -> Res<PlayView> {
        self.refresh_obs(&access.id, true);
        let o = self
            .obs
            .get(&access.id)
            .map(|o| (o.db_ok, o.ra_ok))
            .unwrap_or((false, false));
        if !o.0 {
            return Err(fail(
                "realm_offline",
                "The realm's database cannot be reached.",
            ));
        }
        if !o.1 {
            return Err(fail("realm_offline", "The realm is not running."));
        }
        let pre = self.preflight(&cid.to_string(), &access.id)?;
        match pre.step {
            Step::Blocked => {
                return Err(ServiceError {
                    code: "incompatible".into(),
                    message: "This realm cannot take this character; nothing was changed.".into(),
                    notes: pre.notes,
                })
            }
            Step::Offline => return Err(fail("realm_offline", "The realm is not running.")),
            Step::Resolve => {
                return Err(ServiceError {
                    code: "conflict".into(),
                    message: "The realm's copy and the saved character both changed.".into(),
                    notes: pre.notes,
                })
            }
            Step::Restart => self.restart_update(cid, access)?,
            Step::Prepare => self.prepare(cid, access, account)?,
            Step::Arm => self.arm(cid, access)?,
            Step::Resume => {}
        }
        self.conflicts.remove(&(cid, access.id.clone()));
        let status = if pre.verdict == Verdict::Degraded {
            PlayStatus::CompatWarning
        } else {
            PlayStatus::WaitingLogin
        };
        Ok(PlayView {
            status,
            projection: pre.projection,
            notes: pre.notes,
            realm_address: access.address.clone(),
            realm_id: access.id.clone(),
        })
    }

    /// The character is not on the realm: offer it, put it there (projected when it is above the cap) and bind the session.
    fn prepare(
        &mut self,
        cid: CharacterId,
        access: &RealmAccess,
        account: Option<&str>,
    ) -> Res<()> {
        let account = account
            .map(str::to_string)
            .or_else(|| self.settings.accounts.get(&access.id).cloned())
            .ok_or_else(|| {
                fail(
                    "needs_account",
                    "Tell the Manager which game account on this realm plays this character.",
                )
            })?;
        let db = access.db()?;
        let mut ra = access
            .ra()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let caps = self.capabilities(access, &db, &mut ra, true)?;
        let account_id = self.account_id(&db, &account)?;
        let opts = self.options(access, &caps);
        let offer = OwnerService::new(&mut self.owner).offer(cid, &access.id)?;
        HostService::new(&mut self.host, &access.id, self.host_config.clone())
            .accept_offer(self.host_profile, &offer)?;
        let imported = import_character_online_on(
            Some(&db),
            &mut ra,
            &mut self.host,
            cid,
            &access.id,
            account_id,
            &opts,
            &access.job_dir,
            Some(offer.session_id),
        )?;
        HostService::new(&mut self.host, &access.id, self.host_config.clone())
            .bind(offer.session_id, imported.local_guid)?;
        self.reproject.remove(&(cid, access.id.clone()));
        Ok(())
    }

    /// The character is on the realm and up to date: arm a session on the copy that is there.
    fn arm(&mut self, cid: CharacterId, access: &RealmAccess) -> Res<()> {
        let db = access.db()?;
        let ra = access
            .ra()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let mapping = self
            .host
            .server_mappings(cid)?
            .into_iter()
            .find(|m| m.server_id == access.id)
            .ok_or_else(|| fail("other", "The character is not on this realm."))?;
        let offer = OwnerService::new(&mut self.owner).offer(cid, &access.id)?;
        HostService::new(&mut self.host, &access.id, self.host_config.clone())
            .accept_offer(self.host_profile, &offer)?;
        let mut bridge = LiveBridge::new(&db, ra);
        bridge.arm(
            mapping.local_guid,
            offer.session_id,
            cid,
            offer.canonical_revision,
            1,
        )?;
        HostService::new(&mut self.host, &access.id, self.host_config.clone())
            .bind(offer.session_id, mapping.local_guid)?;
        Ok(())
    }

    /// The realm's copy is behind the canonical character (or the realm's progression moved): the realm is stopped, the copy is updated in place
    /// with the next session armed in the same transaction, and the realm is started again. Possible only when this Manager runs the server.
    fn restart_update(&mut self, cid: CharacterId, access: &RealmAccess) -> Res<()> {
        let (Some(control), Some(install)) = (self.control.as_ref(), access.install_id.clone())
        else {
            return Err(fail("update_required", "This realm has to be stopped and started again to receive your latest progress; the realm's owner has to do it."));
        };
        let _ = control;
        let db = access.db()?;
        let mut ra = access
            .ra()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let caps = self.capabilities(access, &db, &mut ra, true)?;
        let mut opts = self.options(access, &caps);
        let canonical = self.owner.load_current(cid)?;
        // the decision of the running core is the only one there is: ask it now, then stop
        if let Decision::Projected(hold) = decide_with_core(&mut ra, &access.job_dir, &canonical)? {
            opts.projection = Some(Oracle(Arc::new(SuppliedDecision(hold))));
        }
        drop(ra);
        let control = self.control.as_ref().expect("checked above");
        control.stop_all(&install)?;
        control.start_database(&install)?;
        let outcome = (|| -> Res<SessionId> {
            let offer = OwnerService::new(&mut self.owner).offer(cid, &access.id)?;
            HostService::new(&mut self.host, &access.id, self.host_config.clone())
                .accept_offer(self.host_profile, &offer)?;
            realm::update_realm_character_in_session(
                &db,
                &mut self.host,
                cid,
                &access.id,
                &opts,
                Some(offer.session_id),
            )?;
            let guid = self
                .host
                .server_mappings(cid)?
                .into_iter()
                .find(|m| m.server_id == access.id)
                .map(|m| m.local_guid)
                .ok_or_else(|| fail("other", "The character is not on this realm."))?;
            HostService::new(&mut self.host, &access.id, self.host_config.clone())
                .bind(offer.session_id, guid)?;
            Ok(offer.session_id)
        })();
        let started = self
            .control
            .as_ref()
            .expect("checked above")
            .start_all(&install);
        outcome?;
        started?;
        self.reproject.remove(&(cid, access.id.clone()));
        self.obs.entry(access.id.clone()).or_default().caps_at = None;
        Ok(())
    }

    // ---- the tick -----------------------------------------------------------------------------------------------------------------

    /// One pass over every realm: look at it, and run the Host of those with a live session. Safe to call as often as wanted.
    pub fn tick(&mut self) -> Vec<(String, HostEvent)> {
        self.last_tick = Some(Instant::now());
        let mut out = Vec::new();
        let realms = self.realms.clone();
        for access in realms {
            self.refresh_obs(&access.id, false);
            let live = match self.host.host_live_sessions(&access.id) {
                Ok(l) => l,
                Err(e) => {
                    self.note_error(Some(&access.id), e.to_string());
                    continue;
                }
            };
            if live.is_empty() {
                continue;
            }
            match self.tick_realm(&access) {
                Ok(events) => out.extend(events.into_iter().map(|e| (access.id.clone(), e))),
                Err(e) => self.note_error(Some(&access.id), e.to_string()),
            }
        }
        self.publish();
        out
    }

    fn tick_realm(&mut self, access: &RealmAccess) -> Res<Vec<HostEvent>> {
        let now = self.started.elapsed().as_secs();
        let db = access.db()?;
        // the realm's progression, looked at on its own console (the bridge keeps another)
        let probe_due = self
            .obs
            .get(&access.id)
            .and_then(|o| o.profile_at)
            .is_none_or(|t| t.elapsed() >= CAPS_EVERY);
        if probe_due {
            if let Ok(mut ra) = access.ra() {
                if let Err(e) = self.capabilities(access, &db, &mut ra, true) {
                    self.note_error(Some(&access.id), e.to_string());
                }
            }
        }
        let knowledge = access
            .data_dir
            .is_dir()
            .then(|| RealmKnowledge::from_data_dir(&access.data_dir).ok())
            .flatten()
            .map(Arc::new);
        let ra = access.ra().ok();
        let mut bridge = match ra {
            Some(ra) => LiveBridge::new(&db, ra),
            None => LiveBridge::without_console(&db),
        }
        .with_knowledge(knowledge);
        let memory = self
            .obs
            .get_mut(&access.id)
            .map(|o| std::mem::take(&mut o.memory))
            .unwrap_or_default();
        let mut host =
            HostService::resume(&mut self.host, &access.id, self.host_config.clone(), memory);
        let events = host.tick(&mut bridge, now);
        let mut finished: Vec<(SessionId, u64)> = Vec::new();
        let mut delivery: std::result::Result<(), PortableError> = Ok(());
        if events.is_ok() {
            delivery = (|| {
                for m in host.outbox()? {
                    let mut owner = OwnerService::new(&mut self.owner);
                    let ack: OwnerAck = if m.started {
                        owner.handle_started(&m.bytes)?
                    } else {
                        owner.handle_checkpoint(&m.bytes)?
                    };
                    if let AckEffect::Finished { next_revision, .. } =
                        host.receive_ack(&mut bridge, &ack)?
                    {
                        finished.push((m.session_id, next_revision));
                    }
                }
                for m in host.collection_outbox()? {
                    let ack = OwnerService::new(&mut self.owner).handle_collection(&m.bytes)?;
                    host.receive_collection_ack(&mut bridge, m.account, &ack)?;
                }
                Ok(())
            })();
        }
        let memory = host.into_memory();
        // the rows of the live sessions, for the words of the interface
        let live = self.host.host_live_sessions(&access.id).unwrap_or_default();
        let mut rows = HashMap::new();
        for s in &live {
            if let Some(guid) = s.local_guid {
                if let Ok(Some(row)) = bridge.session_row(guid) {
                    rows.insert(guid, row);
                }
            }
        }
        let o = self.obs.entry(access.id.clone()).or_default();
        o.memory = memory;
        o.rows = rows;
        let events = events?;
        delivery?;
        for (session, revision) in finished {
            if let Ok(Some(s)) = self.host.host_session(session) {
                self.saved.insert(
                    (s.character_id, access.id.clone()),
                    Saved {
                        revision,
                        at: now_text(),
                    },
                );
            }
        }
        for e in &events {
            if let HostEvent::ProfileMoved { session } = e {
                if let Ok(Some(s)) = self.host.host_session(*session) {
                    self.reproject.insert((s.character_id, access.id.clone()));
                }
            }
        }
        Ok(events)
    }

    // ---- other actions -----------------------------------------------------------------------------------------------------------

    pub fn local_characters(&mut self, realm_id: &str) -> Res<Vec<LocalCharacterView>> {
        let access = self.access(realm_id)?;
        let db = access
            .db()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let list =
            realm::inspect_characters(&db).map_err(|e| fail("realm_offline", e.to_string()))?;
        let mut out = Vec::new();
        for c in list {
            if self.host.find_by_local(realm_id, c.local_guid)?.is_some() {
                continue;
            }
            out.push(LocalCharacterView {
                token: c.local_guid,
                name: c.name.clone(),
                class_name: class_name(&format!("coa:class:{}", c.class)),
                level: c.level,
                account: c.username.clone().unwrap_or_default(),
                eligible: c.eligible(),
                reasons: c.blockers.iter().map(|b| b.code().to_string()).collect(),
            });
        }
        Ok(out)
    }

    /// Make a character that exists on a realm portable: it is read (offline characters only) and registered, here and as the canonical one.
    pub fn make_portable(&mut self, realm_id: &str, token: u32) -> Res<CharacterView> {
        let access = self.access(realm_id)?;
        let db = access
            .db()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let made = realm::make_portable(&db, &mut self.host, self.host_profile, realm_id, token)?;
        let model = self.host.load_current(made.character_id)?;
        if let Err(e) = self
            .owner
            .create_character_with_id(self.owner_profile, model, realm_id)
        {
            let _ = self.host.detach_realm_copy(made.character_id, realm_id);
            return Err(e.into());
        }
        if let Ok(mut ra) = access.ra() {
            if let Ok(caps) = self.capabilities(&access, &db, &mut ra, true) {
                if let Some(p) = &caps.progression {
                    let _ = self.host.set_mapping_pin(
                        made.character_id,
                        realm_id,
                        Some(&ProgressionPin::native(p, &caps.content_profile_hash)),
                    );
                }
            }
        }
        self.publish();
        self.state()
            .characters
            .into_iter()
            .find(|c| c.id == made.character_id.to_string())
            .ok_or_else(|| fail("other", "The character was not registered."))
    }

    // ---- remote claims (Phase 12) --------------------------------------------------------------------------------------------------

    /// How to reach a realm this Manager knows (its database and console).
    pub fn realm_access(&self, id: &str) -> Res<RealmAccess> {
        self.access(id)
    }

    /// Is this realm character already registered with this Manager's host store (made portable here before)?
    pub fn is_portable_here(&self, realm_id: &str, local_guid: u32) -> Res<bool> {
        Ok(self.host.find_by_local(realm_id, local_guid)?.is_some())
    }

    /// Read an offline character of a realm this Manager hosts and register it, **for a remote player who claims it**: the character is registered with the host
    /// store only (the player's own Manager keeps the canonical copy), and what leaves is the encoded snapshot and the account's collections. A character that is
    /// already registered is read from the store again (a retried claim returns the same character).
    pub fn export_for_claim(&mut self, realm_id: &str, local_guid: u32) -> Res<ClaimBundle> {
        let access = self.access(realm_id)?;
        let db = access
            .db()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let character_id = match self.host.find_by_local(realm_id, local_guid)? {
            Some(c) => c,
            None => {
                realm::make_portable(&db, &mut self.host, self.host_profile, realm_id, local_guid)?
                    .character_id
            }
        };
        let model = self.host.load_current(character_id)?;
        let encoded = super::super::snapshot::encode(&model)?;
        let mut collections = BTreeMap::new();
        if realm::ruleset_of(&db) == super::super::model::Ruleset::Coa {
            if let Some(account) = realm::collections::account_of(&db, local_guid)? {
                for kind in ["coa:appearance", "coa:vanity"] {
                    if let Ok(set) = realm::collections::read_set(&db, account, kind) {
                        collections.insert(kind.to_string(), set.ids().to_vec());
                    }
                }
            }
        }
        self.publish();
        Ok(ClaimBundle {
            character_id,
            payload: encoded.payload,
            content_hash: encoded.content_hash,
            collections,
        })
    }

    /// Store a character claimed on a remote realm as this player's canonical one (revision 1), with the account collections that came with it. The payload is verified
    /// against the hash and the character id before anything is written; a character that is already here is left alone.
    pub fn adopt_claim(
        &mut self,
        realm_id: &str,
        character_id: CharacterId,
        payload: &[u8],
        content_hash: &[u8; 32],
        collections: &BTreeMap<String, Vec<u32>>,
    ) -> Res<CharacterView> {
        let model = super::super::snapshot::decode(payload, Some(content_hash))?;
        if model.character_id != character_id {
            return Err(fail(
                "other",
                "The character data does not belong to the character that was claimed.",
            ));
        }
        if self.owner.character(character_id).is_err() {
            self.owner
                .create_character_with_id(self.owner_profile, model, realm_id)?;
        }
        for (kind, ids) in collections {
            if !matches!(kind.as_str(), "coa:appearance" | "coa:vanity") {
                continue;
            }
            let set = super::super::collection::IdSet::from_ids(ids.iter().copied())?;
            self.owner
                .merge_collection(self.owner_profile, kind, &set)?;
        }
        self.publish();
        self.state()
            .characters
            .into_iter()
            .find(|c| c.id == character_id.to_string())
            .ok_or_else(|| fail("other", "The character was not registered."))
    }

    /// Export a canonical character and collections for transfer to a remote realm (Phase 12.1).
    pub fn export_for_remote_transfer(&self, character: &str) -> Res<ClaimBundle> {
        let cid = self.cid(character)?;
        let model = self.owner.load_current(cid)?;
        let encoded = super::super::snapshot::encode(&model)?;
        let mut collections = BTreeMap::new();
        for kind in ["coa:appearance", "coa:vanity"] {
            if let Ok(Some((_, set))) = self.owner.collection(self.owner_profile, kind) {
                collections.insert(kind.to_string(), set.ids().to_vec());
            }
        }
        Ok(ClaimBundle {
            character_id: cid,
            payload: encoded.payload,
            content_hash: encoded.content_hash,
            collections,
        })
    }

    /// Preflight check for remote transfer before receiving payload chunks (Phase 12.1).
    pub fn check_remote_transfer(
        &mut self,
        realm_id: &str,
        character_id: CharacterId,
        _revision: u64,
    ) -> Res<crate::control::service::PreflightVerdict> {
        let access = self.access(realm_id)?;
        if access.worker_threads().is_some_and(|w| w != 1) {
            return Ok(crate::control::service::PreflightVerdict::Incompatible);
        }
        self.refresh_obs(realm_id, false);
        let obs = self
            .obs
            .get(realm_id)
            .map(|o| (o.db_ok, o.ra_ok))
            .unwrap_or((false, false));
        if !obs.0 {
            return Err(fail(
                "realm_offline",
                "The realm's database cannot be reached.",
            ));
        }
        if let Ok(model) = self.host.load_current(character_id) {
            let db = access.db()?;
            let caps = match self.remembered(&access, &db) {
                Some(c) => c,
                None => return Ok(crate::control::service::PreflightVerdict::Compatible),
            };
            let opts = self.options(&access, &caps);
            let ops = [Operation::OnlineImport, Operation::RuntimeSession];
            let report = self.report(&model, &caps, &opts, &ops, obs.1);
            match verdict_of(report.verdict()) {
                Verdict::Incompatible => {
                    return Ok(crate::control::service::PreflightVerdict::Incompatible)
                }
                Verdict::Degraded => {
                    return Ok(crate::control::service::PreflightVerdict::Degraded)
                }
                Verdict::Compatible => {
                    return Ok(crate::control::service::PreflightVerdict::Compatible)
                }
            }
        }
        Ok(crate::control::service::PreflightVerdict::Compatible)
    }

    /// Import or update a transferred character into the remote realm and arm a session (Phase 12.1).
    pub fn import_remote_character(
        &mut self,
        realm_id: &str,
        params: crate::control::service::RemoteImportParams,
    ) -> Res<crate::control::service::RemoteImportOutcome> {
        let cid = CharacterId::from_uuid(params.character_id)?;
        let model = super::super::snapshot::decode(&params.payload, Some(&params.content_hash))?;
        if model.character_id != cid {
            return Err(fail(
                "other",
                "The character data does not belong to the character that was transferred.",
            ));
        }

        let access = self.access(realm_id)?;
        if let Some(threads) = access.worker_threads() {
            if threads != 1 {
                return Err(fail(
                    "incompatible_worker_threads",
                    format!("CharacterDatabase.WorkerThreads is set to {threads}. Portable sessions strictly require CharacterDatabase.WorkerThreads = 1 in worldserver.conf to prevent database race conditions. Please set CharacterDatabase.WorkerThreads = 1 in Core/configs/worldserver.conf and restart worldserver.")
                ));
            }
        }
        let db = access.db()?;
        let mut ra = access
            .ra()
            .map_err(|e| fail("realm_offline", e.to_string()))?;
        let caps = self.capabilities(&access, &db, &mut ra, true)?;
        let opts = self.options(&access, &caps);

        // Preflight compatibility check before ANY mutation
        let ops = [Operation::OnlineImport, Operation::RuntimeSession];
        let report = self.report(&model, &caps, &opts, &ops, true);
        if verdict_of(report.verdict()) == Verdict::Incompatible {
            return Err(fail(
                "incompatible",
                "Character is incompatible with this realm; nothing was changed.",
            ));
        }

        // Install or advance copy in host store
        self.host
            .host_install_copy(self.host_profile, &model, params.revision, "remote")?;

        // Monotonic union of collections
        for (kind, ids) in &params.collections {
            if matches!(kind.as_str(), "coa:appearance" | "coa:vanity") {
                if let Ok(set) = super::super::collection::IdSet::from_ids(ids.iter().copied()) {
                    let _ = self.host.merge_collection(self.host_profile, kind, &set);
                }
            }
        }

        let existing_mapping = self
            .host
            .server_mappings(cid)?
            .into_iter()
            .find(|m| m.server_id == realm_id);

        let session_id = SessionId::new();
        let envelope = super::super::session::protocol::Envelope::from_encoded(
            &super::super::snapshot::encode(&model)?,
        );
        let offer = super::super::session::protocol::SessionOffer {
            protocol_version: super::super::session::protocol::PROTOCOL_VERSION,
            session_id,
            character_id: cid,
            server_id: realm_id.to_string(),
            canonical_revision: params.revision,
            snapshot: envelope,
        };
        HostService::new(&mut self.host, realm_id, self.host_config.clone())
            .accept_offer(self.host_profile, &offer)?;

        let local_guid = if let Some(m) = existing_mapping {
            // Already exists on realm: arm session on existing copy
            let mut bridge = LiveBridge::new(&db, ra);
            bridge.arm(m.local_guid, offer.session_id, cid, params.revision, 1)?;
            HostService::new(&mut self.host, realm_id, self.host_config.clone())
                .bind(offer.session_id, m.local_guid)?;
            m.local_guid
        } else {
            // Import online into running realm
            let imported = import_character_online_on(
                Some(&db),
                &mut ra,
                &mut self.host,
                cid,
                realm_id,
                params.account_id,
                &opts,
                &access.job_dir,
                Some(offer.session_id),
            )?;
            HostService::new(&mut self.host, realm_id, self.host_config.clone())
                .bind(offer.session_id, imported.local_guid)?;
            imported.local_guid
        };

        self.reproject.remove(&(cid, realm_id.to_string()));
        self.publish();

        let projected_level = caps
            .progression
            .as_ref()
            .filter(|p| u32::from(model.progression.level) > p.max_player_level)
            .map(|p| p.max_player_level);
        let notes = notes_of(&report).into_iter().map(|n| n.detail).collect();

        Ok(crate::control::service::RemoteImportOutcome {
            local_guid,
            session_id: session_id.as_uuid(),
            projected_level,
            notes,
        })
    }

    /// The two ways out of a divergence the Manager does not own.
    pub fn resolve(&mut self, character: &str, realm_id: &str, action: Resolve) -> Res<()> {
        let cid = self.cid(character)?;
        let access = self.access(realm_id)?;
        match action {
            Resolve::Detach => {
                self.host.detach_realm_copy(cid, realm_id)?;
            }
            Resolve::UseCanonical => {
                let Some(install) = access.install_id.clone().filter(|_| self.control.is_some())
                else {
                    return Err(fail("update_required", "The realm has to be stopped to overwrite its copy; the realm's owner has to do it."));
                };
                let db = access.db()?;
                let mut ra = access
                    .ra()
                    .map_err(|e| fail("realm_offline", e.to_string()))?;
                let caps = self.capabilities(&access, &db, &mut ra, true)?;
                let mut opts = self.options(&access, &caps);
                let canonical = self.owner.load_current(cid)?;
                if let Decision::Projected(hold) =
                    decide_with_core(&mut ra, &access.job_dir, &canonical)?
                {
                    opts.projection = Some(Oracle(Arc::new(SuppliedDecision(hold))));
                }
                drop(ra);
                let control = self.control.as_ref().expect("checked above");
                control.stop_all(&install)?;
                control.start_database(&install)?;
                let outcome = (|| -> Res<()> {
                    let offer = OwnerService::new(&mut self.owner).offer(cid, realm_id)?;
                    HostService::new(&mut self.host, realm_id, self.host_config.clone())
                        .accept_offer(self.host_profile, &offer)?;
                    realm::update_realm_character_to_canonical(
                        &db,
                        &mut self.host,
                        cid,
                        realm_id,
                        &opts,
                        Some(offer.session_id),
                    )?;
                    let guid = self
                        .host
                        .server_mappings(cid)?
                        .into_iter()
                        .find(|m| m.server_id == realm_id)
                        .map(|m| m.local_guid)
                        .ok_or_else(|| fail("other", "The character is not on this realm."))?;
                    HostService::new(&mut self.host, realm_id, self.host_config.clone())
                        .bind(offer.session_id, guid)?;
                    Ok(())
                })();
                let started = self
                    .control
                    .as_ref()
                    .expect("checked above")
                    .start_all(&install);
                outcome?;
                started?;
            }
        }
        self.conflicts.remove(&(cid, realm_id.to_string()));
        self.reproject.remove(&(cid, realm_id.to_string()));
        self.publish();
        Ok(())
    }

    pub fn history(&self, character: &str) -> Res<Vec<HistoryEntry>> {
        let cid = self.cid(character)?;
        let names: HashMap<&str, &str> = self
            .realms
            .iter()
            .map(|r| (r.id.as_str(), r.name.as_str()))
            .collect();
        let mut out: Vec<HistoryEntry> = self
            .owner
            .list_revisions(cid)?
            .into_iter()
            .map(|r| HistoryEntry {
                revision: r.revision,
                at: r.created_at,
                source_realm: names
                    .get(r.source_server_id.as_str())
                    .map(|n| n.to_string())
                    .unwrap_or(r.source_server_id),
                kind: HistoryKind::of(r.note.as_deref()),
            })
            .collect();
        out.sort_by(|a, b| b.revision.cmp(&a.revision));
        Ok(out)
    }

    /// A report a bug report can carry: ids and hashes, never a credential and never a character.
    pub fn diagnostics(&self) -> DiagnosticsReport {
        let realms = self
            .realms
            .iter()
            .map(|r| {
                let stored = self.host.realm_profile(&r.id).ok().flatten();
                let caps = self
                    .obs
                    .get(&r.id)
                    .and_then(|o| o.caps.as_ref())
                    .or(stored.as_ref());
                RealmDiagnostics {
                    server_id: r.id.clone(),
                    name: r.name.clone(),
                    kind: r.kind,
                    online: self.obs.get(&r.id).is_some_and(|o| o.ra_ok),
                    capability_profile_hash: caps.map(|c| c.content_profile_hash.clone()),
                    core_commit: caps.and_then(|c| c.core.as_ref()).map(|c| c.commit.clone()),
                    level_cap: caps
                        .and_then(|c| c.progression.as_ref())
                        .map(|p| p.max_player_level),
                    progression_signature: caps
                        .and_then(|c| c.progression.as_ref())
                        .map(|p| p.progression_signature.clone()),
                    session_protocol: caps.map(|c| c.content.session_protocol),
                }
            })
            .collect();
        let mut characters = Vec::new();
        for c in self
            .owner
            .list_characters(self.owner_profile)
            .unwrap_or_default()
        {
            let mut copies = Vec::new();
            for m in self
                .host
                .server_mappings(c.character_id)
                .unwrap_or_default()
            {
                let live = self
                    .host
                    .host_live_sessions(&m.server_id)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| s.character_id == c.character_id);
                let ctx = self
                    .host
                    .projection_context(c.character_id, &m.server_id)
                    .ok()
                    .flatten();
                copies.push(CopyDiagnostics {
                    server_id: m.server_id.clone(),
                    synced_revision: m.last_revision,
                    session_id: live.as_ref().map(|s| s.session_id.to_string()),
                    session_state: live.as_ref().map(|s| format!("{:?}", s.state)),
                    last_checkpoint: live.as_ref().map(|s| s.acked_sequence),
                    progression_pin: live.as_ref().and_then(|s| s.pin.clone()).or_else(|| {
                        self.host
                            .character_pin(c.character_id, &m.server_id)
                            .ok()
                            .flatten()
                    }),
                    projected: ctx.is_some(),
                    compatibility: self
                        .verdicts
                        .get(&(c.character_id, m.server_id.clone()))
                        .copied(),
                });
            }
            characters.push(CharacterDiagnostics {
                character_id: c.character_id.to_string(),
                canonical_revision: c.revision,
                copies,
            });
        }
        DiagnosticsReport {
            generated_at: now_text(),
            manager_version: env!("CARGO_PKG_VERSION").to_string(),
            realms,
            characters,
            errors: self.errors.iter().cloned().collect(),
        }
    }

    /// What the interface needs to start the game for a realm: the installation id (when this Manager runs the server) and the address.
    pub fn realm_launch_info(&self, realm_id: &str) -> Res<(Option<String>, String)> {
        let a = self.access(realm_id)?;
        Ok((a.install_id.clone(), a.address.clone()))
    }

    pub fn saved(&self, character: &str, realm_id: &str) -> Option<(u64, String)> {
        let cid: CharacterId = character.parse().ok()?;
        self.saved
            .get(&(cid, realm_id.to_string()))
            .map(|s| (s.revision, s.at.clone()))
    }

    pub fn owner(&self) -> &Store {
        &self.owner
    }

    #[cfg(test)]
    pub(crate) fn owner_mut(&mut self) -> &mut Store {
        &mut self.owner
    }

    #[cfg(test)]
    pub(crate) fn owner_profile(&self) -> crate::portable::ProfileId {
        self.owner_profile
    }

    pub fn host(&self) -> &Store {
        &self.host
    }

    pub fn descriptor_errors(&self) -> &[(PathBuf, String)] {
        &self.broken_descriptors
    }
}

#[allow(dead_code)]
fn _descriptor_is_used(_: &Descriptor) {}
