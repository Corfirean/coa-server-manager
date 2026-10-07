//! The publishing lifecycle of a Host Manager, independent of Player Mode and of the portable engine.
//!
//! ```text
//! publish enabled -> ensure the realm's identity (UUIDv7 + Ed25519 key) -> register -> heartbeat every ~30 s
//! the realm's capabilities changed -> the next heartbeat carries them
//! Manager restart -> the same RealmId and key -> register again as itself -> heartbeat
//! publish disabled -> unpublish (retried in the background until it is delivered or refused)
//! ```
//!
//! Nothing here touches the realm's database writes, the worldserver or any portable store: the loop only *reads* what a realm advertises, and a
//! Registry that is down, slow or hostile changes nothing but this module's own status. Transient failures are retried with a capped exponential
//! backoff and jitter; a refusal that repeating cannot fix (bad signature, wrong key, unsupported protocol, invalid metadata) stops the realm's
//! publication until the owner acts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use coa_registry_proto::sign::encode_public_key;
use coa_registry_proto::*;
use ed25519_dalek::SigningKey;
use serde::Serialize;

use super::advert::{AdvertSource, LocalAdvert};
use super::client::{ClientError, RegistryClient};
use super::keys::KeyStore;
use super::settings::{self, PublishConfig, RegistrySettings};
use crate::{Error, Result};

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

#[derive(Clone, Debug)]
pub struct Timing {
    pub heartbeat: Duration,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    /// How long to wait before looking again at a realm that is not running or not known yet (no network involved).
    pub idle_poll: Duration,
    /// Clock-skew refusals are retried this often at most; a clock that stays wrong is the owner's to fix.
    pub max_clock_failures: u32,
    pub max_unpublish_attempts: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self { heartbeat: Duration::from_secs(HEARTBEAT_INTERVAL_SECS), backoff_base: Duration::from_secs(5), backoff_cap: Duration::from_secs(300), idle_poll: Duration::from_secs(5), max_clock_failures: 20, max_unpublish_attempts: 12 }
    }
}

/// `base * 2^(attempt-1)` capped, then spread by +-25% with `unit` in `[0, 1)`.
pub fn backoff(attempt: u32, base: Duration, cap: Duration, unit: f64) -> Duration {
    let exp = attempt.saturating_sub(1).min(20);
    let raw = base.as_secs_f64() * f64::from(1u32 << exp);
    let capped = raw.min(cap.as_secs_f64());
    Duration::from_secs_f64(capped * (0.75 + 0.5 * unit.clamp(0.0, 0.999_999)))
}

fn unit() -> f64 {
    let b = uuid::Uuid::new_v4();
    let n = u64::from_le_bytes(b.as_bytes()[..8].try_into().expect("8 bytes"));
    (n >> 11) as f64 / (1u64 << 53) as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    /// Waiting for the first attempt.
    Starting,
    /// The realm is not known, not running or has no capabilities yet: nothing is sent.
    WaitingForRealm,
    RealmStopped,
    Online,
    /// The Registry could not be reached (or asked to slow down); trying again.
    Retrying,
    /// The Registry refused the request and repeating cannot change that.
    Rejected,
    Unpublishing,
    Disabled,
}

#[derive(Clone, Debug, Serialize)]
pub struct RealmPublishStatus {
    pub local_id: String,
    pub realm_id: Option<String>,
    pub enabled: bool,
    pub state: PublishState,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub region: Option<String>,
    pub existing_only: bool,
    pub route: Option<String>,
    pub metadata_revision: Option<u64>,
    pub last_ok_unix: Option<i64>,
    pub last_error: Option<String>,
    pub retry_in_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RegistryStatus {
    pub url: Option<String>,
    pub realms: Vec<RealmPublishStatus>,
}

enum Phase {
    Register,
    Beat,
}

struct Publisher {
    cfg: PublishConfig,
    realm: Option<RealmId>,
    key: Option<SigningKey>,
    phase: Phase,
    state: PublishState,
    /// `None`: nothing to do until the owner acts (a permanent refusal).
    due: Option<Instant>,
    failures: u32,
    clock_failures: u32,
    unpublish_attempts: u32,
    acked_caps: Option<String>,
    acked_listing: Option<String>,
    revision: Option<u64>,
    last_ok: Option<i64>,
    last_error: Option<String>,
    last_ts: i64,
}

impl Publisher {
    fn new(cfg: PublishConfig, now: Instant) -> Self {
        let enabled = cfg.enabled;
        Self {
            cfg,
            realm: None,
            key: None,
            phase: Phase::Register,
            state: if enabled { PublishState::Starting } else { PublishState::Disabled },
            due: enabled.then_some(now),
            failures: 0,
            clock_failures: 0,
            unpublish_attempts: 0,
            acked_caps: None,
            acked_listing: None,
            revision: None,
            last_ok: None,
            last_error: None,
            last_ts: 0,
        }
    }

    fn reset_progress(&mut self, now: Instant) {
        self.phase = Phase::Register;
        self.failures = 0;
        self.clock_failures = 0;
        self.acked_caps = None;
        self.acked_listing = None;
        self.last_error = None;
        self.state = PublishState::Starting;
        self.due = Some(now);
    }
}

pub struct RegistryHost {
    dir: PathBuf,
    settings: RegistrySettings,
    keys: Arc<dyn KeyStore>,
    source: Arc<dyn AdvertSource>,
    clock: Clock,
    timing: Timing,
    client: Option<RegistryClient>,
    publishers: BTreeMap<String, Publisher>,
}

impl RegistryHost {
    pub fn open(dir: &Path, keys: Arc<dyn KeyStore>, source: Arc<dyn AdvertSource>, clock: Clock, timing: Timing, now: Instant) -> Result<Self> {
        let settings = settings::load(dir)?;
        let client = settings.url.as_deref().map(RegistryClient::new).transpose()?;
        let mut host = Self { dir: dir.to_path_buf(), settings, keys, source, clock, timing, client, publishers: BTreeMap::new() };
        for (local, cfg) in host.settings.realms.clone() {
            let mut p = Publisher::new(cfg.clone(), now);
            if cfg.enabled {
                host.attach_identity(&mut p);
            }
            host.publishers.insert(local, p);
        }
        Ok(host)
    }

    pub fn url(&self) -> Option<&str> {
        self.settings.url.as_deref()
    }

    /// The identity of an enabled realm: its id and key must both exist; a missing key is not replaced behind the owner's back.
    fn attach_identity(&self, p: &mut Publisher) {
        let Some(id) = p.cfg.realm_id else {
            p.state = PublishState::Rejected;
            p.due = None;
            p.last_error = Some("the realm has no identity yet".into());
            return;
        };
        p.realm = Some(id);
        match self.keys.load(&id) {
            Ok(Some(key)) => p.key = Some(key),
            Ok(None) => {
                p.state = PublishState::Rejected;
                p.due = None;
                p.last_error = Some("the realm's key is missing on this machine; stop publishing and publish again to create a new identity".into());
            }
            Err(e) => {
                p.state = PublishState::Rejected;
                p.due = None;
                p.last_error = Some(format!("the realm's key cannot be read: {e}"));
            }
        }
    }

    fn save(&self) -> Result<()> {
        settings::save(&self.dir, &self.settings)
    }

    pub fn set_url(&mut self, url: Option<&str>, now: Instant) -> Result<()> {
        let url = match url.map(str::trim).filter(|u| !u.is_empty()) {
            Some(u) => Some(settings::validate_url(u)?),
            None => None,
        };
        if url == self.settings.url {
            return Ok(());
        }
        self.client = url.as_deref().map(RegistryClient::new).transpose()?;
        self.settings.url = url;
        self.save()?;
        for p in self.publishers.values_mut().filter(|p| p.cfg.enabled && p.key.is_some()) {
            p.reset_progress(now);
        }
        Ok(())
    }

    /// Start (or restart) publishing a local realm. The first time creates its identity; later times keep it.
    pub fn publish(&mut self, local_id: &str, display_name: &str, description: &str, language: &str, region: Option<&str>, now: Instant) -> Result<()> {
        if self.client.is_none() {
            return Err(Error::Invalid("set the Registry address first".into()));
        }
        let mut cfg = self.settings.realms.get(local_id).cloned().unwrap_or_default();
        cfg.display_name = display_name.trim().to_string();
        cfg.description = description.trim().to_string();
        cfg.language = language.trim().to_string();
        cfg.region = region.map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
        cfg.enabled = true;
        cfg.validate()?;
        // an identity whose key is gone from this machine cannot sign for itself any more: publishing again makes a new identity, never a guess at the old key
        if let Some(id) = cfg.realm_id {
            if matches!(self.keys.load(&id), Ok(None)) {
                cfg.realm_id = None;
            }
        }
        let created = if cfg.realm_id.is_none() {
            let id = RealmId::new();
            self.keys.create(&id)?;
            cfg.realm_id = Some(id);
            true
        } else {
            false
        };
        self.settings.realms.insert(local_id.to_string(), cfg.clone());
        if let Err(e) = self.save() {
            if created {
                if let Some(id) = cfg.realm_id {
                    let _ = self.keys.remove(&id);
                }
            }
            self.settings.realms.remove(local_id);
            return Err(e);
        }
        let mut p = Publisher::new(cfg, now);
        self.attach_identity(&mut p);
        if let Some(old) = self.publishers.get(local_id) {
            p.last_ok = old.last_ok;
            p.last_ts = old.last_ts;
        }
        self.publishers.insert(local_id.to_string(), p);
        Ok(())
    }

    /// How joining players reach this realm: whether it creates accounts for them, and an address a game client can use right now. The realm has to have been published
    /// before (the choices live with its publishing settings); they take effect at the next heartbeat and on the control link at once.
    pub fn set_access(&mut self, local_id: &str, existing_only: bool, route: Option<&str>) -> Result<()> {
        let route = route.map(str::trim).filter(|r| !r.is_empty()).map(settings::validate_route).transpose()?;
        let Some(cfg) = self.settings.realms.get_mut(local_id) else { return Err(Error::Invalid("Publish this realm first.".into())) };
        let before = cfg.clone();
        cfg.existing_only = existing_only;
        cfg.route = route;
        let cfg = cfg.clone();
        if let Err(e) = self.save() {
            self.settings.realms.insert(local_id.to_string(), before);
            return Err(e);
        }
        if let Some(p) = self.publishers.get_mut(local_id) {
            p.cfg = cfg;
        }
        Ok(())
    }

    /// Stop publishing: the realm is unpublished at the Registry (retried in the background); its identity is kept for a later republication.
    pub fn unpublish(&mut self, local_id: &str, now: Instant) -> Result<()> {
        let Some(cfg) = self.settings.realms.get_mut(local_id) else { return Ok(()) };
        cfg.enabled = false;
        let cfg = cfg.clone();
        self.save()?;
        let Some(p) = self.publishers.get_mut(local_id) else { return Ok(()) };
        let was_registered = p.realm.is_some() && p.key.is_some() && (p.revision.is_some() || matches!(p.phase, Phase::Beat));
        p.cfg = cfg;
        p.unpublish_attempts = 0;
        p.failures = 0;
        if was_registered && self.client.is_some() {
            p.state = PublishState::Unpublishing;
            p.due = Some(now);
        } else {
            p.state = PublishState::Disabled;
            p.due = None;
        }
        Ok(())
    }

    /// Try again now (after a refusal the owner has dealt with).
    pub fn retry(&mut self, local_id: &str, now: Instant) {
        if let Some(p) = self.publishers.get_mut(local_id) {
            if p.cfg.enabled && p.key.is_some() {
                p.reset_progress(now);
            }
        }
    }

    pub fn status(&self, now: Instant) -> RegistryStatus {
        let realms = self
            .publishers
            .iter()
            .map(|(local, p)| RealmPublishStatus {
                local_id: local.clone(),
                realm_id: p.cfg.realm_id.map(|r| r.to_string()),
                enabled: p.cfg.enabled,
                state: p.state,
                display_name: p.cfg.display_name.clone(),
                description: p.cfg.description.clone(),
                language: p.cfg.language.clone(),
                region: p.cfg.region.clone(),
                existing_only: p.cfg.existing_only,
                route: p.cfg.route.clone(),
                metadata_revision: p.revision,
                last_ok_unix: p.last_ok,
                last_error: p.last_error.clone(),
                retry_in_secs: p.due.map(|d| d.saturating_duration_since(now).as_secs()),
            })
            .collect();
        RegistryStatus { url: self.settings.url.clone(), realms }
    }

    /// The public key a published realm is known by (never the private one).
    pub fn public_key(&self, local_id: &str) -> Option<String> {
        self.publishers.get(local_id).and_then(|p| p.key.as_ref()).map(|k| encode_public_key(&k.verifying_key()))
    }

    /// What the Registry itself says about a published realm (the authenticated read; useful for support and for tests).
    pub fn registry_record(&mut self, local_id: &str) -> std::result::Result<RealmDetail, ClientError> {
        let Some(mut p) = self.publishers.remove(local_id) else { return Err(ClientError::Protocol("the realm is not known here".into())) };
        let out = match (self.client.as_ref(), p.realm, p.key.clone()) {
            (Some(client), Some(realm), Some(key)) => {
                let ts = self.next_ts(&mut p);
                client.self_info(&key, &realm, ts)
            }
            _ => Err(ClientError::Protocol("the realm has no identity or the Registry address is not set".into())),
        };
        self.publishers.insert(local_id.to_string(), p);
        out
    }

    pub fn realm_id(&self, local_id: &str) -> Option<RealmId> {
        self.publishers.get(local_id).and_then(|p| p.realm)
    }

    fn next_ts(&self, p: &mut Publisher) -> i64 {
        let ts = (self.clock)().max(p.last_ts + 1);
        p.last_ts = ts;
        ts
    }

    /// One pass over the realms that are due. Network calls block (with their own timeouts); nothing else does.
    pub fn tick(&mut self, now: Instant) {
        let due: Vec<String> = self.publishers.iter().filter(|(_, p)| p.due.is_some_and(|d| d <= now)).map(|(k, _)| k.clone()).collect();
        for local in due {
            let Some(mut p) = self.publishers.remove(&local) else { continue };
            self.step(&local, &mut p, now);
            self.publishers.insert(local, p);
        }
    }

    fn step(&self, local: &str, p: &mut Publisher, now: Instant) {
        let (Some(client), Some(realm), Some(key)) = (self.client.as_ref(), p.realm, p.key.clone()) else {
            p.due = None;
            return;
        };
        if p.state == PublishState::Unpublishing {
            return self.step_unpublish(client, &realm, &key, p, now);
        }
        let Some(advert) = self.source.current(local) else {
            p.state = PublishState::WaitingForRealm;
            p.due = Some(now + self.timing.idle_poll);
            return;
        };
        let result = match p.phase {
            Phase::Register => self.register(client, &realm, &key, p, &advert, now),
            Phase::Beat => self.beat(client, &realm, &key, p, &advert, now),
        };
        if let Err(e) = result {
            self.on_error(p, e, now);
        }
    }

    /// What a player reads about this realm: the owner's words, and everything else as the realm itself reports it.
    fn listing_of(&self, p: &Publisher, advert: &LocalAdvert) -> Listing {
        Listing {
            display_name: p.cfg.display_name.clone(),
            description: p.cfg.description.clone(),
            language: p.cfg.language.clone(),
            region: p.cfg.region.clone(),
            rates: advert.rates.clone(),
            modules: advert.modules.clone(),
            account_provisioning: advert.account_provisioning,
            manager_version: advert.manager_version.clone(),
        }
    }

    fn register(&self, client: &RegistryClient, realm: &RealmId, key: &SigningKey, p: &mut Publisher, advert: &LocalAdvert, now: Instant) -> std::result::Result<(), ClientError> {
        let Some(caps) = advert.capabilities.clone().filter(|_| advert.running) else {
            p.state = if advert.running { PublishState::WaitingForRealm } else { PublishState::RealmStopped };
            p.due = Some(now + self.timing.idle_poll);
            return Ok(());
        };
        let listing = self.listing_of(p, advert);
        let req = RegisterRequest {
            protocol_version: REGISTRY_PROTOCOL_VERSION,
            realm_id: *realm,
            public_key: encode_public_key(&key.verifying_key()),
            listing_hash: listing.hash(),
            listing,
            capabilities_hash: caps.advert_hash(),
            capabilities: caps,
            population: advert.population,
        };
        let ts = self.next_ts(p);
        let resp = client.register(key, &req, ts)?;
        p.phase = Phase::Beat;
        p.acked_caps = Some(resp.capabilities_hash);
        p.acked_listing = Some(resp.listing_hash);
        self.ok(p, resp.metadata_revision, now);
        Ok(())
    }

    fn beat(&self, client: &RegistryClient, realm: &RealmId, key: &SigningKey, p: &mut Publisher, advert: &LocalAdvert, now: Instant) -> std::result::Result<(), ClientError> {
        let Some(caps) = advert.capabilities.clone().filter(|_| advert.running) else {
            p.state = PublishState::RealmStopped;
            p.due = Some(now + self.timing.idle_poll);
            return Ok(());
        };
        let listing = self.listing_of(p, advert);
        let (listing_hash, caps_hash) = (listing.hash(), caps.advert_hash());
        let req = HeartbeatRequest {
            protocol_version: REGISTRY_PROTOCOL_VERSION,
            listing: (p.acked_listing.as_deref() != Some(listing_hash.as_str())).then_some(listing),
            listing_hash,
            capabilities: (p.acked_caps.as_deref() != Some(caps_hash.as_str())).then_some(caps),
            capabilities_hash: caps_hash,
            population: advert.population,
        };
        let ts = self.next_ts(p);
        let resp = client.heartbeat(key, realm, &req, ts)?;
        // what the Registry holds now decides what is sent next; a part it asked for is sent again at once
        p.acked_listing = if resp.resend_listing { None } else { Some(resp.listing_hash) };
        p.acked_caps = if resp.resend_capabilities { None } else { Some(resp.capabilities_hash) };
        self.ok(p, resp.metadata_revision, now);
        if resp.resend_listing || resp.resend_capabilities {
            p.due = Some(now + Duration::from_secs(1));
        }
        Ok(())
    }

    fn ok(&self, p: &mut Publisher, revision: u64, now: Instant) {
        p.state = PublishState::Online;
        p.failures = 0;
        p.clock_failures = 0;
        p.last_error = None;
        p.revision = Some(revision);
        p.last_ok = Some((self.clock)());
        p.due = Some(now + self.timing.heartbeat.mul_f64(0.9 + 0.2 * unit()));
    }

    fn on_error(&self, p: &mut Publisher, e: ClientError, now: Instant) {
        p.last_error = Some(e.to_string());
        match &e {
            ClientError::Rejected { code: ErrorCode::UnknownRealm | ErrorCode::NotPublished, .. } => {
                p.phase = Phase::Register;
                p.acked_caps = None;
                p.acked_listing = None;
                p.failures += 1;
                p.state = PublishState::Retrying;
                p.due = Some(now + if p.failures <= 1 { Duration::from_secs(1) } else { backoff(p.failures, self.timing.backoff_base, self.timing.backoff_cap, unit()) });
            }
            ClientError::Rejected { code: ErrorCode::BadTimestamp | ErrorCode::TimestampNotMonotonic, .. } => {
                p.clock_failures += 1;
                p.failures += 1;
                if p.clock_failures > self.timing.max_clock_failures {
                    p.state = PublishState::Rejected;
                    p.due = None;
                } else {
                    p.state = PublishState::Retrying;
                    p.due = Some(now + backoff(p.failures, self.timing.backoff_base, self.timing.backoff_cap, unit()));
                }
            }
            e if e.is_permanent() => {
                p.state = PublishState::Rejected;
                p.due = None;
            }
            _ => {
                p.failures += 1;
                p.state = PublishState::Retrying;
                p.due = Some(now + backoff(p.failures, self.timing.backoff_base, self.timing.backoff_cap, unit()));
            }
        }
    }

    fn step_unpublish(&self, client: &RegistryClient, realm: &RealmId, key: &SigningKey, p: &mut Publisher, now: Instant) {
        let ts = self.next_ts(p);
        match client.unpublish(key, realm, ts) {
            Ok(resp) => {
                p.revision = Some(resp.metadata_revision);
                p.last_error = None;
                p.state = PublishState::Disabled;
                p.due = None;
            }
            Err(ClientError::Rejected { code: ErrorCode::UnknownRealm, .. }) => {
                p.state = PublishState::Disabled;
                p.due = None;
            }
            Err(e) => {
                p.last_error = Some(e.to_string());
                p.unpublish_attempts += 1;
                if e.is_permanent() || p.unpublish_attempts >= self.timing.max_unpublish_attempts {
                    p.state = PublishState::Disabled;
                    p.due = None;
                } else {
                    p.due = Some(now + backoff(p.unpublish_attempts, self.timing.backoff_base, self.timing.backoff_cap, unit()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_grows_is_capped_and_is_spread() {
        let (base, cap) = (Duration::from_secs(5), Duration::from_secs(300));
        let mid = |n| backoff(n, base, cap, 0.5).as_secs_f64();
        assert!((mid(1) - 5.0).abs() < 0.01 && (mid(2) - 10.0).abs() < 0.01 && (mid(3) - 20.0).abs() < 0.01);
        assert!((mid(10) - 300.0).abs() < 0.01 && (mid(500) - 300.0).abs() < 0.01, "capped");
        assert!(backoff(4, base, cap, 0.0) < backoff(4, base, cap, 0.999), "jitter spreads");
        assert!(backoff(30, base, cap, 0.999) <= Duration::from_secs_f64(300.0 * 1.25));
        assert!(backoff(1, base, cap, 0.0).as_secs_f64() >= 3.7);
        for _ in 0..1000 {
            let u = unit();
            assert!((0.0..1.0).contains(&u));
        }
    }
}
