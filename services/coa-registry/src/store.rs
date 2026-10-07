//! What the Registry keeps, and the rules for changing it. The rules are pure functions shared by the in-memory store (tests) and PostgreSQL,
//! so both behave identically; the stores only load and save.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use coa_registry_proto::caps::{AdvertisedCapabilities, Ruleset};
use coa_registry_proto::{HeartbeatRequest, RealmId, RegisterRequest};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("the realm is not known")]
    Unknown,
    #[error("another key owns this realm id")]
    KeyMismatch,
    #[error("the request is not newer than the last one applied")]
    Stale,
    #[error("the realm is not published")]
    NotPublished,
    #[error("{0}")]
    Backend(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, PartialEq)]
pub struct RealmRow {
    pub realm_id: RealmId,
    pub public_key: [u8; 32],
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    pub published: bool,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub ruleset: Ruleset,
    pub manager_version: String,
    pub capabilities: AdvertisedCapabilities,
    pub capabilities_hash: String,
    pub metadata_revision: u64,
    pub last_request_ts: i64,
    pub player_count: Option<u32>,
    pub player_capacity: Option<u32>,
}

/// The light part of a row a heartbeat decides on (the capabilities JSON stays in the database unless it changes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeartbeatView {
    pub published: bool,
    pub last_request_ts: i64,
    pub capabilities_hash: String,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub manager_version: String,
    pub metadata_revision: u64,
}

impl From<&RealmRow> for HeartbeatView {
    fn from(r: &RealmRow) -> Self {
        Self {
            published: r.published,
            last_request_ts: r.last_request_ts,
            capabilities_hash: r.capabilities_hash.clone(),
            display_name: r.display_name.clone(),
            description: r.description.clone(),
            language: r.language.clone(),
            manager_version: r.manager_version.clone(),
            metadata_revision: r.metadata_revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeartbeatPlan {
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub manager_version: String,
    /// Set only when the hash changed: the only case in which the large JSON is written.
    pub capabilities: Option<AdvertisedCapabilities>,
    pub capabilities_hash: String,
    pub metadata_revision: u64,
    pub metadata_changed: bool,
    pub resend_capabilities: bool,
}

pub fn plan_heartbeat(view: &HeartbeatView, hb: &HeartbeatRequest, ts: i64) -> StoreResult<HeartbeatPlan> {
    if ts <= view.last_request_ts {
        return Err(StoreError::Stale);
    }
    if !view.published {
        return Err(StoreError::NotPublished);
    }
    let mut plan = HeartbeatPlan {
        display_name: hb.display_name.clone().unwrap_or_else(|| view.display_name.clone()),
        description: hb.description.clone().unwrap_or_else(|| view.description.clone()),
        language: hb.language.clone().unwrap_or_else(|| view.language.clone()),
        manager_version: hb.manager_version.clone().unwrap_or_else(|| view.manager_version.clone()),
        capabilities: None,
        capabilities_hash: view.capabilities_hash.clone(),
        metadata_revision: view.metadata_revision,
        metadata_changed: false,
        resend_capabilities: false,
    };
    plan.metadata_changed = plan.display_name != view.display_name || plan.description != view.description || plan.language != view.language || plan.manager_version != view.manager_version;
    match &hb.capabilities {
        Some(c) if hb.capabilities_hash != view.capabilities_hash => {
            plan.capabilities = Some(c.clone());
            plan.capabilities_hash = hb.capabilities_hash.clone();
            plan.metadata_changed = true;
        }
        None if hb.capabilities_hash != view.capabilities_hash => plan.resend_capabilities = true,
        _ => {}
    }
    if plan.metadata_changed {
        plan.metadata_revision += 1;
    }
    Ok(plan)
}

/// The new row for a registration: a first one, or the same key announcing itself again (a restart, a changed name, a republication).
pub fn plan_register(existing: Option<&RealmRow>, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> StoreResult<(RealmRow, bool)> {
    let mut row = RealmRow {
        realm_id: req.realm_id,
        public_key: key,
        created_at: now,
        updated_at: now,
        last_seen_at: now,
        published: true,
        display_name: req.display_name.clone(),
        description: req.description.clone(),
        language: req.language.clone(),
        ruleset: req.ruleset,
        manager_version: req.manager_version.clone(),
        capabilities: req.capabilities.clone(),
        capabilities_hash: req.capabilities_hash.clone(),
        metadata_revision: 1,
        last_request_ts: ts,
        player_count: req.player_count,
        player_capacity: req.player_capacity,
    };
    let Some(old) = existing else { return Ok((row, true)) };
    if old.public_key != key {
        return Err(StoreError::KeyMismatch);
    }
    if ts <= old.last_request_ts {
        return Err(StoreError::Stale);
    }
    row.created_at = old.created_at;
    let changed = !old.published
        || old.display_name != row.display_name
        || old.description != row.description
        || old.language != row.language
        || old.ruleset != row.ruleset
        || old.manager_version != row.manager_version
        || old.capabilities_hash != row.capabilities_hash;
    row.metadata_revision = old.metadata_revision + u64::from(changed);
    row.updated_at = if changed { now } else { old.updated_at };
    Ok((row, false))
}

pub trait Store: Send + Sync + 'static {
    fn ping(&self) -> impl Future<Output = bool> + Send;
    /// The key a realm id is bound to, if it is registered.
    fn key_of(&self, id: &RealmId) -> impl Future<Output = StoreResult<Option<[u8; 32]>>> + Send;
    fn get(&self, id: &RealmId) -> impl Future<Output = StoreResult<Option<RealmRow>>> + Send;
    /// `Ok((row, created))`
    fn register(&self, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> impl Future<Output = StoreResult<(RealmRow, bool)>> + Send;
    fn heartbeat(&self, id: &RealmId, hb: &HeartbeatRequest, now: i64, ts: i64) -> impl Future<Output = StoreResult<HeartbeatOutcome>> + Send;
    /// The metadata revision after the change.
    fn unpublish(&self, id: &RealmId, now: i64, ts: i64) -> impl Future<Output = StoreResult<u64>> + Send;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeartbeatOutcome {
    pub metadata_revision: u64,
    pub capabilities_hash: String,
    pub resend_capabilities: bool,
}

#[derive(Default)]
pub struct MemoryStore {
    rows: Mutex<HashMap<RealmId, RealmRow>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[doc(hidden)]
    pub fn get_for_test(&self, id: &RealmId) -> Option<RealmRow> {
        self.rows.lock().unwrap().get(id).cloned()
    }

    /// Forget everything (tests: a Registry that lost its data).
    #[doc(hidden)]
    pub fn forget_all(&self) {
        self.rows.lock().unwrap().clear();
    }
}

/// A store shared between a test and the server it runs.
impl<T: Store> Store for std::sync::Arc<T> {
    fn ping(&self) -> impl Future<Output = bool> + Send {
        (**self).ping()
    }
    fn key_of(&self, id: &RealmId) -> impl Future<Output = StoreResult<Option<[u8; 32]>>> + Send {
        (**self).key_of(id)
    }
    fn get(&self, id: &RealmId) -> impl Future<Output = StoreResult<Option<RealmRow>>> + Send {
        (**self).get(id)
    }
    fn register(&self, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> impl Future<Output = StoreResult<(RealmRow, bool)>> + Send {
        (**self).register(req, key, now, ts)
    }
    fn heartbeat(&self, id: &RealmId, hb: &HeartbeatRequest, now: i64, ts: i64) -> impl Future<Output = StoreResult<HeartbeatOutcome>> + Send {
        (**self).heartbeat(id, hb, now, ts)
    }
    fn unpublish(&self, id: &RealmId, now: i64, ts: i64) -> impl Future<Output = StoreResult<u64>> + Send {
        (**self).unpublish(id, now, ts)
    }
}

impl Store for MemoryStore {
    async fn ping(&self) -> bool {
        true
    }

    async fn key_of(&self, id: &RealmId) -> StoreResult<Option<[u8; 32]>> {
        Ok(self.rows.lock().unwrap().get(id).map(|r| r.public_key))
    }

    async fn get(&self, id: &RealmId) -> StoreResult<Option<RealmRow>> {
        Ok(self.rows.lock().unwrap().get(id).cloned())
    }

    async fn register(&self, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> StoreResult<(RealmRow, bool)> {
        let mut rows = self.rows.lock().unwrap();
        let (row, created) = plan_register(rows.get(&req.realm_id), req, key, now, ts)?;
        rows.insert(req.realm_id, row.clone());
        Ok((row, created))
    }

    async fn heartbeat(&self, id: &RealmId, hb: &HeartbeatRequest, now: i64, ts: i64) -> StoreResult<HeartbeatOutcome> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.get_mut(id).ok_or(StoreError::Unknown)?;
        let plan = plan_heartbeat(&HeartbeatView::from(&*row), hb, ts)?;
        row.display_name = plan.display_name;
        row.description = plan.description;
        row.language = plan.language;
        row.manager_version = plan.manager_version;
        if let Some(c) = plan.capabilities {
            row.ruleset = c.content.ruleset;
            row.capabilities = c;
        }
        row.capabilities_hash = plan.capabilities_hash.clone();
        if plan.metadata_changed {
            row.updated_at = now;
        }
        row.metadata_revision = plan.metadata_revision;
        row.last_seen_at = now;
        row.last_request_ts = ts;
        row.player_count = hb.player_count;
        row.player_capacity = hb.player_capacity;
        Ok(HeartbeatOutcome { metadata_revision: plan.metadata_revision, capabilities_hash: plan.capabilities_hash, resend_capabilities: plan.resend_capabilities })
    }

    async fn unpublish(&self, id: &RealmId, now: i64, ts: i64) -> StoreResult<u64> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.get_mut(id).ok_or(StoreError::Unknown)?;
        if ts <= row.last_request_ts {
            return Err(StoreError::Stale);
        }
        if row.published {
            row.published = false;
            row.metadata_revision += 1;
            row.updated_at = now;
        }
        row.last_request_ts = ts;
        Ok(row.metadata_revision)
    }
}
