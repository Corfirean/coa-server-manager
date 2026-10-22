//! What a local realm advertises, and how the Host learns it. Every value comes from the real thing, and what cannot be known is `None`, never a guess:
//!
//! | advertised | source |
//! |---|---|
//! | level cap, ruleset | the running core's capabilities (`probe_capabilities`: `MaxPlayerLevel` over RA, the ruleset of the realm's database) |
//! | rates | the realm's effective worldserver configuration (`allsettings`: the active file, else the documented default), when the server folder is known |
//! | modules | the Manager's module catalog against the server's `Core/configs/modules` (installed and enabled), when the server folder is known |
//! | players / bots | the realm's database: online characters of other accounts / of the bot accounts (the bot subsystem's account prefix) |
//! | capacity | the worldserver's `PlayerLimit` when it is above 0 |

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_registry_proto::{
    AccountProvisioning, AdvertisedCapabilities, ModuleEntry, Population, Rates,
};

use crate::portable::capabilities::RealmCapabilities;
use crate::portable::extension::ExtensionRegistry;
use crate::portable::realm::profile::probe_capabilities;
use crate::portable::service::access::{load_descriptors, RealmAccess};
use crate::{Error, Result};

/// The public form of a realm's capabilities: the profile as it is (same fields, same hash), bounded.
pub fn advertise(caps: &RealmCapabilities) -> Result<AdvertisedCapabilities> {
    let value = serde_json::to_value(caps)?;
    let advert: AdvertisedCapabilities = serde_json::from_value(value)
        .map_err(|e| Error::Invalid(format!("the capabilities cannot be advertised: {e}")))?;
    advert
        .validate()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(advert)
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalAdvert {
    /// The worldserver answers (database and console): only a running realm sends heartbeats.
    pub running: bool,
    /// What the realm advertises; the last known one while it is stopped, `None` before it was ever seen running.
    pub capabilities: Option<AdvertisedCapabilities>,
    pub population: Population,
    pub rates: Rates,
    pub modules: Vec<ModuleEntry>,
    pub account_provisioning: AccountProvisioning,
    pub manager_version: String,
}

/// Where the publishing loop gets a realm's current advertisement. Implementations must be quick or cache; they are called from the loop's thread.
pub trait AdvertSource: Send + Sync + 'static {
    fn current(&self, local_id: &str) -> Option<LocalAdvert>;
}

/// The configuration keys behind the advertised rates (the same ones the Manager's presets set).
fn rate(items: &[crate::allsettings::Item], key: &str) -> Option<f64> {
    let v: f64 = items
        .iter()
        .find(|i| i.key == key)?
        .value
        .trim()
        .trim_matches('"')
        .parse()
        .ok()?;
    (v.is_finite() && (0.0..=coa_registry_proto::MAX_RATE).contains(&v)).then_some(v)
}

/// The realm's effective rates; every one the configuration does not give is `None`.
pub fn rates_of(root: &Path) -> Rates {
    let Ok(items) = crate::allsettings::list(root) else {
        return Rates::default();
    };
    Rates {
        xp_kill: rate(&items, "Rate.XP.Kill"),
        xp_quest: rate(&items, "Rate.XP.Quest"),
        xp_explore: rate(&items, "Rate.XP.Explore"),
        loot: rate(&items, "Rate.Drop.Item.Normal"),
        money: rate(&items, "Rate.Drop.Money"),
        reputation: rate(&items, "Rate.Reputation.Gain"),
        honor: rate(&items, "Rate.Honor"),
    }
}

/// `PlayerLimit` when the realm has one.
pub fn capacity_of(root: &Path) -> Option<u32> {
    let items = crate::allsettings::list(root).ok()?;
    let n: u32 = items
        .iter()
        .find(|i| i.key == "PlayerLimit")?
        .value
        .trim()
        .parse()
        .ok()?;
    (1..=coa_registry_proto::MAX_PLAYER_NUMBER)
        .contains(&n)
        .then_some(n)
}

/// A module version fit for the Registry: its first token when that is a plain version, else nothing.
fn plain_version(v: Option<String>) -> Option<String> {
    let first = v?.split_whitespace().next()?.to_string();
    coa_registry_proto::validate_version(&first)
        .ok()
        .map(|_| first)
}

/// The modules this server build contains, with whether each is on. Hidden ones (settings offered elsewhere) are not listed.
pub fn modules_of(root: &Path) -> Vec<ModuleEntry> {
    let mut out: Vec<ModuleEntry> = crate::modules::list(root)
        .into_iter()
        .filter(|m| {
            m.installed && !m.hidden && coa_registry_proto::validate_module_id(&m.id).is_ok()
        })
        .map(|m| ModuleEntry {
            id: m.id,
            version: plain_version(m.version),
            enabled: m.enabled,
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.truncate(coa_registry_proto::MAX_MODULES);
    out
}

/// The realm a local id names: an installed server (`srv-<install id>` or the bare install id) or a prepared descriptor.
pub fn find_access(
    descriptors: &Path,
    installs: &[(String, PathBuf)],
    local_id: &str,
) -> Option<RealmAccess> {
    if let Some((id, root)) = installs
        .iter()
        .find(|(id, _)| format!("srv-{id}") == local_id || id == local_id)
    {
        return Some(RealmAccess::from_install(id, root));
    }
    let (found, _) = load_descriptors(descriptors);
    found
        .into_iter()
        .find(|d| d.id == local_id)
        .and_then(|d| RealmAccess::from_descriptor(d).ok())
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
    /// Can this Manager create game accounts for joining players (the control service runs)?
    provisioning: Box<dyn Fn(&str) -> bool + Send + Sync>,
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
    pub fn new(
        descriptors: impl Into<PathBuf>,
        installs: impl Fn() -> Vec<(String, PathBuf)> + Send + Sync + 'static,
    ) -> Self {
        Self::with_provisioning(descriptors, installs, |_| false)
    }

    /// `provisioning` says, for a local realm id, whether this Manager is serving account creation for it: a realm only advertises automatic accounts when that is true.
    pub fn with_provisioning(
        descriptors: impl Into<PathBuf>,
        installs: impl Fn() -> Vec<(String, PathBuf)> + Send + Sync + 'static,
        provisioning: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                descriptors: descriptors.into(),
                installs: Box::new(installs),
                registry: Arc::new(ExtensionRegistry::new()),
                provisioning: Box::new(provisioning),
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
        find_access(&self.descriptors, &(self.installs)(), local_id)
    }

    fn probe(&self, local_id: &str) {
        let previous = self
            .cache
            .lock()
            .ok()
            .and_then(|c| c.get(local_id).map(|c| (c.advert.clone(), c.caps_at)));
        let Some(access) = self.access(local_id) else {
            return;
        };
        let automatic = (self.provisioning)(local_id);
        let mut advert = LocalAdvert {
            running: false,
            capabilities: previous.as_ref().and_then(|(a, _)| a.capabilities.clone()),
            population: Population::default(),
            rates: Rates::default(),
            modules: Vec::new(),
            account_provisioning: AccountProvisioning {
                automatic,
                existing_only: !automatic,
            },
            manager_version: crate::MANAGER_VERSION.to_string(),
        };
        // configuration-derived values do not need the realm to be running
        let mut prefix = "COABOTHOST".to_string();
        if let Some(root) = access
            .root
            .as_deref()
            .filter(|r| r.join("Core/configs").is_dir())
        {
            advert.rates = rates_of(root);
            advert.modules = modules_of(root);
            advert.population.capacity = capacity_of(root);
            prefix = crate::population::bot_account_prefix(root).to_uppercase();
        }
        let mut caps_at = previous.as_ref().and_then(|(_, t)| *t);
        if access.db_reachable() {
            if let (Ok(db), Ok(mut ra)) = (access.db(), access.ra()) {
                advert.running = true;
                if let Ok(p) = crate::population::query_with(&db, &prefix) {
                    advert.population.players = p.players_online;
                    advert.population.bots = p.bots_online;
                }
                if let Some(c) = advert.population.capacity {
                    advert.population.players = advert.population.players.min(c);
                }
                if caps_at.is_none_or(|t| t.elapsed() >= self.caps_every)
                    || advert.capabilities.is_none()
                {
                    let data = access
                        .data_dir
                        .is_dir()
                        .then_some(access.data_dir.as_path());
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
            cache.insert(
                local_id.to_string(),
                Cached {
                    at: Instant::now(),
                    advert,
                    caps_at,
                },
            );
        }
    }
}

impl AdvertSource for LocalRealmsSource {
    fn current(&self, local_id: &str) -> Option<LocalAdvert> {
        let (answer, stale) = match self.inner.cache.lock().ok()?.get(local_id) {
            Some(c) => (Some(c.advert.clone()), c.at.elapsed() >= self.inner.fresh),
            None => (None, true),
        };
        if stale
            && self
                .inner
                .inflight
                .lock()
                .ok()?
                .insert(local_id.to_string())
        {
            let inner = self.inner.clone();
            let id = local_id.to_string();
            let spawned = std::thread::Builder::new()
                .name("realm-probe".into())
                .spawn(move || {
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
            features: [
                Feature::RuntimeSessions,
                Feature::Wardrobe,
                Feature::Collections,
                Feature::LevelProjection,
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()]
                .into_iter()
                .collect(),
            extensions: vec![ExtensionSupport {
                namespace: "coa".into(),
                module_version: "1".into(),
                formats: FormatRange::single(1),
            }],
            client_catalog: BTreeMap::from([(
                "Appearances.dbc".to_string(),
                CatalogEntry {
                    sha256: "c".repeat(64),
                    records: 7,
                },
            )]),
        };
        let progression = Progression {
            max_player_level: 60,
            projection_protocol: 1,
            projection_policy_version: 1,
            progression_signature: "d".repeat(64),
            scaling_enabled: true,
        };
        let core = Some(CoreIdentity {
            commit: "0f3737574afd".into(),
            branch: "feat/x".into(),
            date: "2026-10-01".into(),
        });
        let caps = RealmCapabilities::build(core, content, Some(progression)).unwrap();
        let advert = advertise(&caps).unwrap();
        assert_eq!(
            advert.content_profile_hash, caps.content_profile_hash,
            "the Registry sees the hash the Manager computed"
        );
        assert_eq!(
            serde_json::to_value(&advert).unwrap(),
            serde_json::to_value(&caps).unwrap(),
            "the same JSON, field for field"
        );
        assert_eq!(advert.level_cap(), Some(60));
    }

    fn server(root: &Path, conf: &str, dist: &str) {
        let configs = root.join("Core/configs");
        std::fs::create_dir_all(configs.join("modules")).unwrap();
        std::fs::write(configs.join("worldserver.conf"), conf).unwrap();
        std::fs::write(configs.join("worldserver.conf.dist"), dist).unwrap();
    }

    const DIST: &str = "[worldserver]\n\n#\n#    Rate.XP.Kill\n#        Default:     1\n#\n\nRate.XP.Kill = 1\n\n#\n#    Rate.XP.Quest\n#        Default:     1\n#\n\nRate.XP.Quest = 1\n\n#\n#    Rate.Drop.Money\n#        Default:     1\n#\n\nRate.Drop.Money = 1\n\n#\n#    PlayerLimit\n#        Default:     0\n#\n\nPlayerLimit = 0\n";

    #[test]
    fn rates_are_the_effective_configuration_and_unknown_ones_are_null() {
        let dir = tempfile::tempdir().unwrap();
        server(
            dir.path(),
            "[worldserver]\nRate.XP.Kill = 2.5\nRate.Drop.Money = \"3\"\nPlayerLimit = 120\n",
            DIST,
        );
        let r = rates_of(dir.path());
        assert_eq!(
            (r.xp_kill, r.xp_quest, r.money),
            (Some(2.5), Some(1.0), Some(3.0)),
            "the file, else the documented default"
        );
        assert_eq!(
            (r.loot, r.reputation, r.honor, r.xp_explore),
            (None, None, None, None),
            "a rate the server's configuration does not document is not guessed"
        );
        assert_eq!(capacity_of(dir.path()), Some(120));
        std::fs::write(
            dir.path().join("Core/configs/worldserver.conf"),
            "[worldserver]\nRate.XP.Kill = banana\nPlayerLimit = 0\n",
        )
        .unwrap();
        let r = rates_of(dir.path());
        assert_eq!(r.xp_kill, None, "an unreadable value is unknown, not 1");
        assert_eq!(capacity_of(dir.path()), None, "PlayerLimit 0 is no limit");
        assert_eq!(rates_of(&dir.path().join("nothing")), Rates::default());
    }

    #[test]
    fn module_ids_of_the_catalog_are_all_valid_registry_ids_and_versions_are_sanitised() {
        for e in crate::modules::catalog() {
            assert!(
                coa_registry_proto::validate_module_id(&e.id).is_ok(),
                "{}",
                e.id
            );
        }
        assert_eq!(
            plain_version(Some("v1.4.2 · 0123abcd".into())),
            Some("v1.4.2".into())
        );
        assert_eq!(plain_version(Some("· abc".into())), None);
        assert_eq!(plain_version(Some("<b>".into())), None);
        assert_eq!(plain_version(None), None);
        let dir = tempfile::tempdir().unwrap();
        server(dir.path(), "[worldserver]\n", DIST);
        let m = modules_of(dir.path());
        assert!(
            m.is_empty(),
            "a server folder without module files lists no module: {m:?}"
        );
    }
}
