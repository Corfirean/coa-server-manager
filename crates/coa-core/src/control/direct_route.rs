//! Direct Route Discovery and State Machine (Phase 14):
//! State machine:
//! Probe interfaces
//!   -> discover public address
//!   -> check existing reachability
//!   -> try automatic mapping (UPnP IGD, NAT-PMP, PCP) if needed
//!   -> start DirectIngress proxy (Auth rewrite + World byte tunnel)
//!   -> verify externally via Coordinator probe helper
//!   -> DIRECT_READY
//! otherwise
//! RELAY_ONLY / DIRECT_UNAVAILABLE
//!
//! Important: STUN is discovery only (never hole punching). External verification prevents
//! false positives from broken router hairpinning. Raw public IP is never stored in public Registry.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::direct_ingress::DirectIngress;
use super::nat_pmp;
use super::service::HostService;
use crate::net;
use crate::upnp;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RouteStatus {
    Probing,
    DirectReady {
        public_ip: String,
        auth_port: u16,
        world_port: u16,
        method: String,
    },
    RelayOnly {
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub enum ActiveMapping {
    None,
    Upnp {
        gateway: upnp::Gateway,
        auth_port: u16,
        world_port: u16,
    },
    NatPmp {
        gateway: Ipv4Addr,
        auth_internal: u16,
        auth_external: u16,
        world_internal: u16,
        world_external: u16,
        lifetime_secs: u32,
        renew_at: std::time::Instant,
    },
}

impl ActiveMapping {
    pub fn cleanup(&mut self) {
        match self {
            ActiveMapping::None => {}
            ActiveMapping::Upnp { gateway, auth_port, world_port } => {
                let _ = upnp::remove_mapping(gateway, *auth_port);
                let _ = upnp::remove_mapping(gateway, *world_port);
            }
            ActiveMapping::NatPmp { gateway, auth_internal, world_internal, .. } => {
                let _ = nat_pmp::delete_mapping(*gateway, *auth_internal);
                let _ = nat_pmp::delete_mapping(*gateway, *world_internal);
            }
        }
        *self = ActiveMapping::None;
    }
}

pub struct DirectRouteManager {
    status: Arc<RwLock<RouteStatus>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DirectRouteManager {
    pub fn start(
        registry_base_url: Option<String>,
        service: Arc<HostService>,
    ) -> Self {
        let status = Arc::new(RwLock::new(RouteStatus::Probing));
        let stop = Arc::new(AtomicBool::new(false));

        let st_clone = status.clone();
        let stop_clone = stop.clone();

        let handle = std::thread::Builder::new()
            .name("direct-route-manager".into())
            .spawn(move || {
                run_route_manager(registry_base_url, service, st_clone, stop_clone);
            })
            .ok();

        Self {
            status,
            stop,
            handle,
        }
    }

    pub fn status(&self) -> RouteStatus {
        self.status.read().map(|s| s.clone()).unwrap_or(RouteStatus::Probing)
    }

    pub fn is_direct_ready(&self) -> bool {
        matches!(self.status(), RouteStatus::DirectReady { .. })
    }

    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DirectRouteManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn set_status(status: &Arc<RwLock<RouteStatus>>, new_status: RouteStatus) {
    if let Ok(mut g) = status.write() {
        *g = new_status;
    }
}

fn probe_endpoints(base_url: Option<&str>, ports: &[u16]) -> bool {
    if let Some(base) = base_url {
        let probe_url = format!("{}/coord/v1/probe", base.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .ok();
        if let Some(c) = client {
            let probe_body = coa_control_proto::coord::ProbePayload {
                ports: ports.to_vec(),
            };
            let body_str = serde_json::to_string(&probe_body).unwrap_or_default();
            if let Ok(resp) = c.post(&probe_url)
                .header("Content-Type", "application/json")
                .body(body_str)
                .send()
            {
                if let Ok(text) = resp.text() {
                    if let Ok(p_resp) = serde_json::from_str::<coa_control_proto::coord::ProbeResponse>(&text) {
                        return p_resp.all_reachable;
                    }
                }
            }
        }
        false
    } else {
        std::env::var("COA_TEST_FORCE_DIRECT_VERIFIED").is_ok()
    }
}

fn run_route_manager(
    registry_base_url: Option<String>,
    service: Arc<HostService>,
    status: Arc<RwLock<RouteStatus>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        attempt_route_discovery(&registry_base_url, &service, &status, &stop);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        for _ in 0..20 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

fn attempt_route_discovery(
    registry_base_url: &Option<String>,
    service: &Arc<HostService>,
    status: &Arc<RwLock<RouteStatus>>,
    stop: &Arc<AtomicBool>,
) {
    // 1. Discover local LAN address
    let Some(lan_ip) = net::lan_ip() else {
        set_status(status, RouteStatus::RelayOnly {
            reason: "No usable local network address found.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    // 2. Discover default gateway using OS routing table API
    let default_gw = net::default_gateway();

    // 3. Discover UPnP Gateway and/or NAT-PMP Gateway
    let upnp_gw = upnp::discover();
    let mut discovered_public_ip: Option<Ipv4Addr> = None;
    let mut method = "direct_public";

    if let Some(ref gw) = upnp_gw {
        if let Ok(ext) = upnp::external_ip(gw) {
            discovered_public_ip = Some(ext);
            method = "upnp";
        }
    }

    if discovered_public_ip.is_none() {
        if let Some(gw) = default_gw {
            if let Some(ext) = nat_pmp::query_external_ip(gw) {
                discovered_public_ip = Some(ext);
                method = "nat_pmp";
            }
        }
    }

    if discovered_public_ip.is_none() && !net::is_private(lan_ip) && !net::is_cgnat_range(lan_ip) {
        discovered_public_ip = Some(lan_ip);
        method = "public_ip";
    }

    let mut probe_ip: Option<Ipv4Addr> = None;
    if let Some(ref base) = registry_base_url {
        let probe_url = format!("{}/coord/v1/probe?ports=3724", base.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .ok();
        if let Some(c) = client {
            if let Ok(resp) = c.get(&probe_url).send() {
                if let Ok(text) = resp.text() {
                    if let Ok(p_resp) = serde_json::from_str::<coa_control_proto::coord::ProbeResponse>(&text) {
                        probe_ip = p_resp.client_ip.parse::<Ipv4Addr>().ok();
                    }
                }
            }
        }
    }

    let public_ip = discovered_public_ip.or(probe_ip);

    let Some(public_ip) = public_ip else {
        set_status(status, RouteStatus::RelayOnly {
            reason: "Could not discover public IPv4 address.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    // 4. CGNAT check
    if net::is_cgnat_range(public_ip) {
        set_status(status, RouteStatus::RelayOnly {
            reason: format!("Carrier-grade NAT detected ({public_ip}); direct routing unavailable."),
        });
        service.set_direct_route(None);
        return;
    };

    // 5. Start local direct ingress proxy
    let local_auth = std::env::var("COA_OVERRIDE_LOCAL_AUTH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(coa_control_proto::relay::LOCAL_AUTH_PORT);
    let local_world = std::env::var("COA_OVERRIDE_LOCAL_WORLD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(coa_control_proto::relay::LOCAL_WORLD_PORT);

    let desired_auth_port: u16 = std::env::var("COA_DIRECT_AUTH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3724);
    let desired_world_port: u16 = std::env::var("COA_DIRECT_WORLD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8085);

    let ingress = match DirectIngress::start(
        desired_auth_port,
        desired_world_port,
        public_ip.to_string(),
        desired_world_port,
        local_auth,
        local_world,
    ) {
        Ok(ing) => Ok(ing),
        Err(_) => {
            DirectIngress::start(
                0,
                0,
                public_ip.to_string(),
                desired_world_port,
                local_auth,
                local_world,
            )
        }
    };

    let Ok(mut ingress) = ingress else {
        set_status(status, RouteStatus::RelayOnly {
            reason: "Cannot bind direct ingress listeners.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    let bound_auth_port = ingress.auth_port();
    let bound_world_port = ingress.world_port();

    // 6. Transactional Port Mapping (if behind NAT)
    let is_behind_nat = net::is_private(lan_ip);
    let mut mapped_auth_port = bound_auth_port;
    let mut mapped_world_port = bound_world_port;
    let mut active_mapping = ActiveMapping::None;

    if is_behind_nat {
        // Try UPnP IGD transactionally
        if let Some(ref gw) = upnp_gw {
            if upnp::add_mapping(gw, bound_auth_port, lan_ip, "Auth Direct").is_ok() {
                if upnp::add_mapping(gw, bound_world_port, lan_ip, "World Direct").is_ok() {
                    mapped_auth_port = bound_auth_port;
                    mapped_world_port = bound_world_port;
                    active_mapping = ActiveMapping::Upnp {
                        gateway: gw.clone(),
                        auth_port: bound_auth_port,
                        world_port: bound_world_port,
                    };
                    method = "upnp";
                } else {
                    // Transactional rollback: remove auth mapping immediately before fallback!
                    let _ = upnp::remove_mapping(gw, bound_auth_port);
                }
            }
        }

        // Try NAT-PMP transactionally if UPnP failed
        if matches!(active_mapping, ActiveMapping::None) {
            if let Some(gw) = default_gw {
                let lifetime = 3600;
                if let Some(a_ext) = nat_pmp::request_mapping(gw, bound_auth_port, bound_auth_port, lifetime) {
                    if let Some(w_ext) = nat_pmp::request_mapping(gw, bound_world_port, bound_world_port, lifetime) {
                        mapped_auth_port = a_ext;
                        mapped_world_port = w_ext;
                        let renew_at = std::time::Instant::now() + Duration::from_secs(lifetime as u64 / 2);
                        active_mapping = ActiveMapping::NatPmp {
                            gateway: gw,
                            auth_internal: bound_auth_port,
                            auth_external: a_ext,
                            world_internal: bound_world_port,
                            world_external: w_ext,
                            lifetime_secs: lifetime,
                            renew_at,
                        };
                        method = "nat_pmp";
                    } else {
                        // Transactional rollback: remove auth mapping immediately before fallback!
                        let _ = nat_pmp::delete_mapping(gw, bound_auth_port);
                    }
                }
            }
        }

        if matches!(active_mapping, ActiveMapping::None) {
            set_status(status, RouteStatus::RelayOnly {
                reason: "Router port mapping (UPnP/NAT-PMP) failed or unsupported.".into(),
            });
            service.set_direct_route(None);
            ingress.stop();
            return;
        }
    }

    // Update DirectIngress with the final verified mapped external world endpoint
    ingress.update_external_world_endpoint(&public_ip.to_string(), mapped_world_port);

    // 7. External Reachability Verification Probe
    let verified = probe_endpoints(registry_base_url.as_deref(), &[mapped_auth_port, mapped_world_port]);

    if !verified {
        set_status(status, RouteStatus::RelayOnly {
            reason: format!("External port reachability check failed on {public_ip}:{mapped_auth_port},{mapped_world_port}."),
        });
        service.set_direct_route(None);
        active_mapping.cleanup();
        ingress.stop();
        return;
    }

    // 8. DIRECT_READY: update status and register direct route with HostService
    let direct_endpoint = format!("{public_ip}:{mapped_auth_port}");
    set_status(status, RouteStatus::DirectReady {
        public_ip: public_ip.to_string(),
        auth_port: mapped_auth_port,
        world_port: mapped_world_port,
        method: method.to_string(),
    });
    service.set_direct_route(Some(direct_endpoint));

    // 9. Route Health, Renewal, and Network Change Monitoring Loop
    let mut last_health_check = std::time::Instant::now();
    let last_lan_ip = lan_ip;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));

        // Network change detection
        let current_lan_ip = net::lan_ip();
        if current_lan_ip != Some(last_lan_ip) {
            set_status(status, RouteStatus::RelayOnly {
                reason: "Local network interface address changed.".into(),
            });
            service.set_direct_route(None);
            active_mapping.cleanup();
            ingress.stop();
            return;
        }

        // NAT-PMP mapping renewal before expiry
        let mut need_probe = false;
        if let ActiveMapping::NatPmp {
            gateway,
            auth_internal,
            auth_external,
            world_internal,
            world_external,
            lifetime_secs,
            ref mut renew_at,
        } = active_mapping {
            if std::time::Instant::now() >= *renew_at {
                let ra = nat_pmp::request_mapping(gateway, auth_internal, auth_external, lifetime_secs);
                let rw = nat_pmp::request_mapping(gateway, world_internal, world_external, lifetime_secs);
                if ra == Some(auth_external) && rw == Some(world_external) {
                    *renew_at = std::time::Instant::now() + Duration::from_secs(lifetime_secs as u64 / 2);
                    need_probe = true;
                } else {
                    set_status(status, RouteStatus::RelayOnly {
                        reason: "NAT-PMP port mapping renewal failed.".into(),
                    });
                    service.set_direct_route(None);
                    active_mapping.cleanup();
                    ingress.stop();
                    return;
                }
            }
        }

        // Periodic external reachability check (every 30s) or immediately after renewal
        if need_probe || last_health_check.elapsed() >= Duration::from_secs(30) {
            last_health_check = std::time::Instant::now();
            if registry_base_url.is_some() {
                let still_reachable = probe_endpoints(registry_base_url.as_deref(), &[mapped_auth_port, mapped_world_port]);
                if !still_reachable {
                    set_status(status, RouteStatus::RelayOnly {
                        reason: format!("Periodic external reachability probe failed on {public_ip}:{mapped_auth_port},{mapped_world_port}."),
                    });
                    service.set_direct_route(None);
                    active_mapping.cleanup();
                    ingress.stop();
                    return;
                }
            }
        }
    }

    // Graceful shutdown
    service.set_direct_route(None);
    active_mapping.cleanup();
    ingress.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_mapping_cleanup() {
        let mut mapping = ActiveMapping::NatPmp {
            gateway: Ipv4Addr::new(127, 0, 0, 1),
            auth_internal: 3724,
            auth_external: 43724,
            world_internal: 8085,
            world_external: 48085,
            lifetime_secs: 3600,
            renew_at: std::time::Instant::now() + Duration::from_secs(1800),
        };
        assert!(matches!(mapping, ActiveMapping::NatPmp { .. }));
        mapping.cleanup();
        assert!(matches!(mapping, ActiveMapping::None));
    }

    #[test]
    fn test_route_manager_health_invalidation() {
        let status = Arc::new(RwLock::new(RouteStatus::DirectReady {
            public_ip: "198.51.100.1".into(),
            auth_port: 3724,
            world_port: 8085,
            method: "nat_pmp".into(),
        }));
        set_status(&status, RouteStatus::RelayOnly {
            reason: "Renewal failed".into(),
        });
        let cur = status.read().unwrap().clone();
        assert!(matches!(cur, RouteStatus::RelayOnly { .. }));
    }
}

