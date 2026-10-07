//! What the Registry keeps, and the rules for changing it. The rules are pure functions shared by the in-memory store (tests) and PostgreSQL,
//! so both behave identically; the stores only load and save.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use coa_registry_proto::caps::{AdvertisedCapabilities, Ruleset};
use coa_registry_proto::{HeartbeatRequest, Listing, Population, RealmId, RegisterRequest};

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

/// The version of advertisement a row holds: 1 is a Phase-10 record (identity, key and capabilities kept, nothing structured), 2 is a full v2 listing.
pub const ADVERT_VERSION: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct RealmRow {
    pub realm_id: RealmId,
    pub public_key: [u8; 32],
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    pub published: bool,
    pub listing: Listing,
    pub listing_hash: String,
    /// Derived from the capabilities.
    pub ruleset: Ruleset,
    pub level_cap: Option<u32>,
    pub capabilities: AdvertisedCapabilities,
    pub capabilities_hash: String,
    pub population: Population,
    pub metadata_revision: u64,
    pub last_request_ts: i64,
    pub advert_version: u8,
}

/// The light part of a row a heartbeat decides on (the capabilities JSON stays in the database unless it changes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeartbeatView {
    pub published: bool,
    pub last_request_ts: i64,
    pub listing_hash: String,
    pub capabilities_hash: String,
    pub metadata_revision: u64,
    pub advert_version: u8,
}

impl From<&RealmRow> for HeartbeatView {
    fn from(r: &RealmRow) -> Self {
        Self { published: r.published, last_request_ts: r.last_request_ts, listing_hash: r.listing_hash.clone(), capabilities_hash: r.capabilities_hash.clone(), metadata_revision: r.metadata_revision, advert_version: r.advert_version }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeartbeatPlan {
    /// Set only when the listing is applied: the only case in which its columns are written.
    pub listing: Option<Listing>,
    pub listing_hash: String,
    /// Set only when the hash changed: the only case in which the large JSON is written.
    pub capabilities: Option<AdvertisedCapabilities>,
    pub capabilities_hash: String,
    pub metadata_revision: u64,
    pub metadata_changed: bool,
    pub resend_listing: bool,
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
        listing: None,
        listing_hash: view.listing_hash.clone(),
        capabilities: None,
        capabilities_hash: view.capabilities_hash.clone(),
        metadata_revision: view.metadata_revision,
        metadata_changed: false,
        resend_listing: false,
        resend_capabilities: false,
    };
    // a record from before protocol 2 has no structured listing: it takes the first one it is given, whatever the hashes say
    let legacy = view.advert_version < ADVERT_VERSION;
    match &hb.listing {
        Some(l) if legacy || hb.listing_hash != view.listing_hash => {
            plan.listing = Some(l.clone());
            plan.listing_hash = hb.listing_hash.clone();
            plan.metadata_changed = true;
        }
        None if legacy || hb.listing_hash != view.listing_hash => plan.resend_listing = true,
        _ => {}
    }
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

/// The new row for a registration: a first one, or the same key announcing itself again (a restart, a changed name, a republication, a record upgraded from protocol 1).
pub fn plan_register(existing: Option<&RealmRow>, req: &RegisterRequest, key: [u8; 32], now: i64, ts: i64) -> StoreResult<(RealmRow, bool)> {
    let mut row = RealmRow {
        realm_id: req.realm_id,
        public_key: key,
        created_at: now,
        updated_at: now,
        last_seen_at: now,
        published: true,
        listing: req.listing.clone(),
        listing_hash: req.listing_hash.clone(),
        ruleset: req.capabilities.content.ruleset,
        level_cap: req.capabilities.level_cap(),
        capabilities: req.capabilities.clone(),
        capabilities_hash: req.capabilities_hash.clone(),
        population: req.population,
        metadata_revision: 1,
        last_request_ts: ts,
        advert_version: ADVERT_VERSION,
    };
    let Some(old) = existing else { return Ok((row, true)) };
    if old.public_key != key {
        return Err(StoreError::KeyMismatch);
    }
    if ts <= old.last_request_ts {
        return Err(StoreError::Stale);
    }
    row.created_at = old.created_at;
    let changed = !old.published || old.advert_version < ADVERT_VERSION || old.listing_hash != row.listing_hash || old.capabilities_hash != row.capabilities_hash;
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
    pub listing_hash: String,
    pub capabilities_hash: String,
    pub resend_listing: bool,
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

    /// Put a row in as it is (tests: a record left by an earlier protocol).
    #[doc(hidden)]
    pub fn put_for_test(&self, row: RealmRow) {
        self.rows.lock().unwrap().insert(row.realm_id, row);
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
        if let Some(l) = plan.listing.clone() {
            row.listing = l;
            row.advert_version = ADVERT_VERSION;
        }
        row.listing_hash = plan.listing_hash.clone();
        if let Some(c) = plan.capabilities.clone() {
            row.ruleset = c.content.ruleset;
            row.level_cap = c.level_cap();
            row.capabilities = c;
        }
        row.capabilities_hash = plan.capabilities_hash.clone();
        if plan.metadata_changed {
            row.updated_at = now;
        }
        row.metadata_revision = plan.metadata_revision;
        row.last_seen_at = now;
        row.last_request_ts = ts;
        row.population = hb.population;
        Ok(HeartbeatOutcome { metadata_revision: plan.metadata_revision, listing_hash: plan.listing_hash, capabilities_hash: plan.capabilities_hash, resend_listing: plan.resend_listing, resend_capabilities: plan.resend_capabilities })
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
