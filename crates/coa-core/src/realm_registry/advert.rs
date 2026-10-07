//! What a local realm advertises, and how the Host learns it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_registry_proto::AdvertisedCapabilities;

use crate::portable::capabilities::RealmCapabilities;
use crate::portable::extension::ExtensionRegistry;
use crate::portable::realm::profile::probe_capabilities;
use crate::portable::service::access::{load_descriptors, RealmAccess};
use crate::{Error, Result};

/// The public form of a realm's capabilities: the profile as it is (same fields, same hash), bounded.
pub fn advertise(caps: &RealmCapabilities) -> Result<AdvertisedCapabilities> {
    let value = serde_json::to_value(caps)?;
    let advert: AdvertisedCapabilities = serde_json::from_value(value).map_err(|e| Error::Invalid(format!("the capabilities cannot be advertised: {e}")))?;
    advert.validate().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(advert)
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalAdvert {
    /// The worldserver answers (database and console): only a running realm sends heartbeats.
    pub running: bool,
    /// What the realm advertises; the last known one while it is stopped, `None` before it was ever seen running.
    pub capabilities: Option<AdvertisedCapabilities>,
    pub player_count: Option<u32>,
    pub player_capacity: Option<u32>,
    pub manager_version: String,
}

/// Where the publishing loop gets a realm's current advertisement. Implementations must be quick or cache; they are called from the loop's thread.
pub trait AdvertSource: Send + Sync + 'static {
    fn current(&self, local_id: &str) -> Option<LocalAdvert>;
}

struct Cached {
    at: Instant,
    advert: LocalAdvert,
    caps_at: Option<Instant>,
}

struct Inner {
    descriptors: PathBuf,
    installs: Box<dyn Fn() -> Vec<(String, PathBuf)> + Send + Sync>,
    registry: Arc<ExtensionRegistry>,
    cache: Mutex<HashMap<String, Cached>>,
    inflight: Mutex<HashSet<String>>,
    fresh: Duration,
    caps_every: Duration,
}

/// Probes the realms this Manager knows (installed servers and prepared descriptors) with the same code the portable engine uses.
///
/// Probing talks to the realm's database and console and can take many seconds, so it never runs on the caller's thread: `current` answers from
/// the last probe at once (`None` before the first one has finished) and starts a refresh in the background when that probe is older than a few seconds.
pub struct LocalRealmsSource {
    inner: Arc<Inner>,
}

impl LocalRealmsSource {
    pub fn new(descriptors: impl Into<PathBuf>, installs: impl Fn() -> Vec<(String, PathBuf)> + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                descriptors: descriptors.into(),
                installs: Box::new(installs),
                registry: Arc::new(ExtensionRegistry::new()),
                cache: Mutex::new(HashMap::new()),
                inflight: Mutex::new(HashSet::new()),
                fresh: Duration::from_secs(10),
                caps_every: Duration::from_secs(60),
            }),
        }
    }
}

impl Inner {
    fn access(&self, local_id: &str) -> Option<RealmAccess> {
        if let Some((id, root)) = (self.installs)().into_iter().find(|(id, _)| format!("srv-{id}") == local_id || id == local_id) {
            return Some(RealmAccess::from_install(&id, &root));
        }
        let (descriptors, _) = load_descriptors(&self.descriptors);
        descriptors.into_iter().find(|d| d.id == local_id).and_then(|d| RealmAccess::from_descriptor(d).ok())
    }

    fn probe(&self, local_id: &str) {
        let previous = self.cache.lock().ok().and_then(|c| c.get(local_id).map(|c| (c.advert.clone(), c.caps_at)));
        let Some(access) = self.access(local_id) else { return };
        let mut advert = LocalAdvert { running: false, capabilities: previous.as_ref().and_then(|(a, _)| a.capabilities.clone()), player_count: None, player_capacity: None, manager_version: crate::MANAGER_VERSION.to_string() };
        let mut caps_at = previous.as_ref().and_then(|(_, t)| *t);
        if access.db_reachable() {
            if let (Ok(db), Ok(mut ra)) = (access.db(), access.ra()) {
                advert.running = true;
                advert.player_count = ra.run("server info").ok().and_then(|text| connected_players(&text));
                if caps_at.is_none_or(|t| t.elapsed() >= self.caps_every) || advert.capabilities.is_none() {
                    let data = access.data_dir.is_dir().then_some(access.data_dir.as_path());
                    if let Ok(caps) = probe_capabilities(&db, data, Some(&mut ra), &self.registry) {
                        if let Ok(a) = advertise(&caps) {
                            advert.capabilities = Some(a);
                            caps_at = Some(Instant::now());
                        }
                    }
                }
            }
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(local_id.to_string(), Cached { at: Instant::now(), advert, caps_at });
        }
    }
}

/// `Connected players: 3. Characters in world: 3.`
pub fn connected_players(server_info: &str) -> Option<u32> {
    let rest = server_info.split("Connected players:").nth(1)?;
    rest.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

impl AdvertSource for LocalRealmsSource {
    fn current(&self, local_id: &str) -> Option<LocalAdvert> {
        let (answer, stale) = match self.inner.cache.lock().ok()?.get(local_id) {
            Some(c) => (Some(c.advert.clone()), c.at.elapsed() >= self.inner.fresh),
            None => (None, true),
        };
        if stale && self.inner.inflight.lock().ok()?.insert(local_id.to_string()) {
            let inner = self.inner.clone();
            let id = local_id.to_string();
            let spawned = std::thread::Builder::new().name("realm-probe".into()).spawn(move || {
                inner.probe(&id);
                if let Ok(mut f) = inner.inflight.lock() {
                    f.remove(&id);
                }
            });
            if spawned.is_err() {
                self.inner.inflight.lock().ok()?.remove(local_id);
            }
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_player_count_is_read_from_the_server_info_report() {
        assert_eq!(connected_players("Connected players: 3. Characters in world: 3."), Some(3));
        assert_eq!(connected_players("x\nConnected players: 0.\n"), Some(0));
        assert_eq!(connected_players("nothing"), None);
        assert_eq!(connected_players("Connected players: many"), None);
    }

    #[test]
    fn a_real_profile_is_advertised_without_changing_its_hash() {
        use crate::portable::capabilities::*;
        use crate::portable::model::Ruleset;
        use std::collections::{BTreeMap, BTreeSet};
        let content = ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: ContentProfile::manager_formats(),
            online_import_job_formats: vec![2],
            session_protocol: 2,
            collection_protocol: 1,
            features: [Feature::RuntimeSessions, Feature::Wardrobe, Feature::Collections, Feature::LevelProjection].into_iter().collect::<BTreeSet<_>>(),
            collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()].into_iter().collect(),
            extensions: vec![ExtensionSupport { namespace: "coa".into(), module_version: "1".into(), formats: FormatRange::single(1) }],
            client_catalog: BTreeMap::from([("Appearances.dbc".to_string(), CatalogEntry { sha256: "c".repeat(64), records: 7 })]),
        };
        let progression = Progression { max_player_level: 60, projection_protocol: 1, projection_policy_version: 1, progression_signature: "d".repeat(64), scaling_enabled: true };
        let core = Some(CoreIdentity { commit: "0f3737574afd".into(), branch: "feat/x".into(), date: "2026-10-01".into() });
        let caps = RealmCapabilities::build(core, content, Some(progression)).unwrap();
        let advert = advertise(&caps).unwrap();
        assert_eq!(advert.content_profile_hash, caps.content_profile_hash, "the Registry sees the hash the Manager computed");
        assert_eq!(serde_json::to_value(&advert).unwrap(), serde_json::to_value(&caps).unwrap(), "the same JSON, field for field");
    }
}
