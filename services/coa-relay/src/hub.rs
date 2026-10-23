//! Central hub state and coordination for coa-relay (Phase 13).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};


use coa_control_proto::relay::{RelayTarget, TunnelMsg};
use coa_registry_proto::RealmId;
use ed25519_dalek::VerifyingKey;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::limits::*;
use crate::ports::PortPool;

pub trait KeyLookup: Send + Sync + 'static {
    fn host_key(&self, realm: &RealmId) -> impl Future<Output = Option<VerifyingKey>> + Send;
}

#[derive(Default)]
pub struct MemoryKeys(Mutex<HashMap<RealmId, [u8; 32]>>);

impl MemoryKeys {
    pub fn set(&self, realm: RealmId, key: &VerifyingKey) {
        self.0.lock().unwrap().insert(realm, key.to_bytes());
    }
}

impl KeyLookup for MemoryKeys {
    async fn host_key(&self, realm: &RealmId) -> Option<VerifyingKey> {
        self.0.lock().unwrap().get(realm).and_then(|b| VerifyingKey::from_bytes(b).ok())
    }
}

impl<T: KeyLookup> KeyLookup for Arc<T> {
    fn host_key(&self, realm: &RealmId) -> impl Future<Output = Option<VerifyingKey>> + Send {
        (**self).host_key(realm)
    }
}


pub struct PgKeys {
    pool: deadpool_postgres::Pool,
}

impl PgKeys {
    pub fn new(pool: deadpool_postgres::Pool) -> Self {
        Self { pool }
    }
}

impl KeyLookup for PgKeys {
    async fn host_key(&self, realm: &RealmId) -> Option<VerifyingKey> {
        let c = self.pool.get().await.ok()?;
        let row = c
            .query_opt(
                "SELECT public_key FROM realms WHERE realm_id = $1::text::uuid AND published AND advert_version = 2",
                &[&realm.to_string()],
            )
            .await
            .ok()??;
        let bytes: Vec<u8> = row.get(0);
        VerifyingKey::from_bytes(&bytes.try_into().ok()?).ok()
    }
}

#[derive(Clone, Debug)]
pub struct Allocation {
    pub token: String,
    pub realm_id: RealmId,
    pub player_id: Uuid,
    pub expected_client_ip: Option<String>,
    pub auth_port: u16,
    pub world_port: u16,
    pub created_at: i64,
    pub expires_at: i64,
    pub consumed_auth: bool,
    pub consumed_world: bool,
}

pub struct HostHandle {
    pub realm_id: RealmId,
    pub tx: mpsc::Sender<TunnelMsg>,
}

pub struct Hub<K: KeyLookup> {
    pub relay_host: String,
    pub pool: Arc<PortPool>,
    pub keys: K,
    pub hosts: Arc<Mutex<HashMap<RealmId, HostHandle>>>,
    pub allocations: Arc<Mutex<HashMap<String, Allocation>>>,
    pub port_allocations: Arc<Mutex<HashMap<u16, (String, RelayTarget)>>>,
    pub stream_txs: Arc<Mutex<HashMap<u32, mpsc::Sender<TunnelMsg>>>>,
    pub next_stream_id: AtomicU32,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl<K: KeyLookup> Hub<K> {
    pub fn new(
        relay_host: String,
        min_port: u16,
        max_port: u16,
        keys: K,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            relay_host,
            pool: Arc::new(PortPool::new(min_port, max_port)),
            keys,
            hosts: Arc::new(Mutex::new(HashMap::new())),
            allocations: Arc::new(Mutex::new(HashMap::new())),
            port_allocations: Arc::new(Mutex::new(HashMap::new())),
            stream_txs: Arc::new(Mutex::new(HashMap::new())),
            next_stream_id: AtomicU32::new(1),
            clock,
        }
    }


    pub fn register_host(&self, realm_id: RealmId, tx: mpsc::Sender<TunnelMsg>) {
        let mut hosts = self.hosts.lock().unwrap();
        hosts.insert(realm_id, HostHandle { realm_id, tx });
        tracing::info!(%realm_id, "host tunnel registered");
    }

    pub fn unregister_host(&self, realm_id: &RealmId) {
        let mut hosts = self.hosts.lock().unwrap();
        hosts.remove(realm_id);
        let mut allocs = self.allocations.lock().unwrap();
        let mut port_map = self.port_allocations.lock().unwrap();
        allocs.retain(|_, a| {
            if &a.realm_id == realm_id {
                if port_map.remove(&a.auth_port).is_some() {
                    self.pool.release(a.auth_port);
                }
                if port_map.remove(&a.world_port).is_some() {
                    self.pool.release(a.world_port);
                }
                false
            } else {
                true
            }
        });
        tracing::info!(%realm_id, "host tunnel unregistered, allocations and ports reclaimed");
    }

    pub fn host_tx(&self, realm_id: &RealmId) -> Option<mpsc::Sender<TunnelMsg>> {
        self.hosts.lock().unwrap().get(realm_id).map(|h| h.tx.clone())
    }

    pub fn reap_expired(&self) {
        let now = (self.clock)();
        let mut allocs = self.allocations.lock().unwrap();
        let mut port_map = self.port_allocations.lock().unwrap();
        allocs.retain(|_, a| {
            if a.expires_at <= now {
                if port_map.remove(&a.auth_port).is_some() {
                    self.pool.release(a.auth_port);
                }
                if port_map.remove(&a.world_port).is_some() {
                    self.pool.release(a.world_port);
                }
                false
            } else {
                true
            }
        });
    }

    pub fn release_port(&self, port: u16) {
        let mut port_map = self.port_allocations.lock().unwrap();
        if let Some((token, target)) = port_map.remove(&port) {
            self.pool.release(port);
            let mut allocs = self.allocations.lock().unwrap();
            if let Some(alloc) = allocs.get_mut(&token) {
                match target {
                    RelayTarget::Auth => alloc.consumed_auth = true,
                    RelayTarget::World => alloc.consumed_world = true,
                }
                let has_ports = port_map.values().any(|(t, _)| t == &token);
                if !has_ports {
                    allocs.remove(&token);
                }
            }
        }
    }

    pub fn allocate(
        &self,
        realm_id: RealmId,
        player_id: Uuid,
        expected_client_ip: Option<String>,
    ) -> Result<Allocation, String> {
        let now = (self.clock)();

        // Clean up expired allocations
        self.reap_expired();

        // Check if player already has an active, unexpired allocation on this realm
        {
            let mut allocs = self.allocations.lock().unwrap();
            if let Some(existing) = allocs.values_mut().find(|a| a.realm_id == realm_id && a.player_id == player_id && a.expires_at > now) {
                if existing.expected_client_ip.is_none() && expected_client_ip.is_some() {
                    existing.expected_client_ip = expected_client_ip;
                }
                return Ok(existing.clone());
            }

            let realm_count = allocs.values().filter(|a| a.realm_id == realm_id).count();
            if realm_count >= MAX_ALLOCATIONS_PER_REALM {
                return Err("too many allocations for realm".into());
            }
        }

        // Acquire ports
        let (auth_port, world_port) = self.pool.acquire_pair().ok_or("relay ports exhausted")?;
        let token = Uuid::new_v4().to_string();
        let alloc = Allocation {
            token: token.clone(),
            realm_id,
            player_id,
            expected_client_ip,
            auth_port,
            world_port,
            created_at: now,
            expires_at: now + ALLOCATION_TIMEOUT.as_secs() as i64,
            consumed_auth: false,
            consumed_world: false,
        };

        {
            let mut allocs = self.allocations.lock().unwrap();
            let mut port_map = self.port_allocations.lock().unwrap();
            allocs.insert(token.clone(), alloc.clone());
            port_map.insert(auth_port, (token.clone(), RelayTarget::Auth));
            port_map.insert(world_port, (token.clone(), RelayTarget::World));
        }

        tracing::info!(%realm_id, %player_id, auth_port, world_port, "relay allocation created");
        Ok(alloc)
    }

    pub fn get_allocation_by_port(&self, port: u16) -> Option<(Allocation, RelayTarget)> {
        let port_map = self.port_allocations.lock().unwrap();
        let (token, target) = port_map.get(&port)?.clone();
        let allocs = self.allocations.lock().unwrap();
        let alloc = allocs.get(&token)?.clone();
        Some((alloc, target))
    }

    pub fn register_stream(&self, stream_id: u32, tx: mpsc::Sender<TunnelMsg>) {
        self.stream_txs.lock().unwrap().insert(stream_id, tx);
    }

    pub fn unregister_stream(&self, stream_id: u32) {
        self.stream_txs.lock().unwrap().remove(&stream_id);
    }

    pub fn stream_tx(&self, stream_id: u32) -> Option<mpsc::Sender<TunnelMsg>> {
        self.stream_txs.lock().unwrap().get(&stream_id).cloned()
    }
}
