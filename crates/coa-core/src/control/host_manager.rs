//! Keeps one [`HostLink`] per published realm: it follows the publishing settings (`registry.json`), so a realm that is published has its control link and a realm that is
//! not does not. The link needs the realm's key, which exists once the realm has been published at least once.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use coa_registry_proto::RealmId;
use serde::Serialize;

use super::host_link::{Clock, HostLink, LinkStatus};
use super::service::{HostService, RealmBackend};
use super::store::ControlStore;
use super::transport::coordinator_url;
use crate::realm_registry::host::system_clock;
use crate::realm_registry::keys::KeyStore;
use crate::realm_registry::settings;

#[derive(Clone, Debug, Serialize)]
pub struct HostControlRealm {
    pub local_id: String,
    pub realm_id: String,
    pub link: LinkStatus,
}

pub struct HostControl {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<BTreeMap<String, HostControlRealm>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

struct Running {
    realm_id: RealmId,
    url: String,
    link: HostLink,
    #[allow(dead_code)]
    relay_link: Option<Arc<crate::control::relay_link::RelayLink>>,
    #[allow(dead_code)]
    direct_route: Option<crate::control::direct_route::DirectRouteManager>,
}


impl HostControl {
    /// `url_override` replaces the Registry address from the settings (`COA_REGISTRY_URL` in tests and deployments).
    pub fn start(settings_dir: PathBuf, keys: Arc<dyn KeyStore>, store: Arc<Mutex<ControlStore>>, backend: Arc<dyn RealmBackend>, url_override: Option<String>) -> HostControl {
        Self::start_with(settings_dir, keys, store, backend, url_override, system_clock(), Duration::from_secs(2))
    }

    pub fn start_with(settings_dir: PathBuf, keys: Arc<dyn KeyStore>, store: Arc<Mutex<ControlStore>>, backend: Arc<dyn RealmBackend>, url_override: Option<String>, clock: Clock, every: Duration) -> HostControl {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(BTreeMap::new()));
        let (s, st) = (stop.clone(), status.clone());
        let join = std::thread::Builder::new()
            .name("control-host-manager".into())
            .spawn(move || {
                let mut running: BTreeMap<String, Running> = BTreeMap::new();
                while !s.load(Ordering::SeqCst) {
                    let wanted = wanted(&settings_dir, keys.as_ref(), url_override.as_deref());
                    running.retain(|id, r| wanted.get(id).is_some_and(|(realm, url, _, _)| *realm == r.realm_id && *url == r.url));
                    for (local_id, (realm_id, url, base, key)) in wanted {
                        if running.contains_key(&local_id) {
                            continue;
                        }
                        let service = Arc::new(HostService::new(&local_id, realm_id, store.clone(), backend.clone()));
                        let relay_link = crate::control::transport::relay_url(&base, "/relay/v1/host")
                            .map(|r_url| {
                                let rl = crate::control::relay_link::RelayLink::start(r_url, realm_id, key.clone());
                                service.set_relay(rl.clone());
                                rl
                            });
                        let direct_route = Some(crate::control::direct_route::DirectRouteManager::start(Some(base.clone()), service.clone()));
                        let link = HostLink::start(url.clone(), realm_id, key, service, clock.clone());
                        running.insert(local_id, Running { realm_id, url, link, relay_link, direct_route });
                    }
                    if let Ok(mut st) = st.lock() {
                        *st = running.iter().map(|(id, r)| (id.clone(), HostControlRealm { local_id: id.clone(), realm_id: r.realm_id.to_string(), link: r.link.status() })).collect();
                    }
                    let end = std::time::Instant::now() + every;
                    while std::time::Instant::now() < end && !s.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                running.clear();
            })
            .ok();
        HostControl { stop, status, join: Mutex::new(join) }
    }


    pub fn status(&self) -> Vec<HostControlRealm> {
        self.status.lock().map(|s| s.values().cloned().collect()).unwrap_or_default()
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.lock().ok().and_then(|mut g| g.take()) {
            let _ = j.join();
        }
    }
}

impl Drop for HostControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type Wanted = BTreeMap<String, (RealmId, String, String, ed25519_dalek::SigningKey)>;

fn wanted(dir: &std::path::Path, keys: &dyn KeyStore, url_override: Option<&str>) -> Wanted {
    let mut out = Wanted::new();
    let Ok(s) = settings::load(dir) else { return out };
    let base = url_override.map(str::to_string).unwrap_or_else(|| s.effective_url().to_string());
    let Some(url) = coordinator_url(&base, "/coord/v1/host") else { return out };
    for (local_id, cfg) in s.realms {
        let (true, Some(realm)) = (cfg.enabled, cfg.realm_id) else { continue };
        if let Ok(Some(key)) = keys.load(&realm) {
            out.insert(local_id, (realm, url.clone(), base.clone(), key));
        }
    }
    out
}

