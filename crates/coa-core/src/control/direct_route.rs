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
                run_route_discovery(registry_base_url, service, st_clone, stop_clone);
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

fn run_route_discovery(
    registry_base_url: Option<String>,
    service: Arc<HostService>,
    status: Arc<RwLock<RouteStatus>>,
    stop: Arc<AtomicBool>,
) {
    // 1. Discover local LAN address
    let Some(lan_ip) = net::lan_ip() else {
        set_status(&status, RouteStatus::RelayOnly {
            reason: "No usable local network address found.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    // 2. Discover UPnP Gateway and/or NAT-PMP Gateway
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
        let octets = lan_ip.octets();
        let gw_candidate = Ipv4Addr::new(octets[0], octets[1], octets[2], 1);
        if let Some(ext) = nat_pmp::query_external_ip(gw_candidate) {
            discovered_public_ip = Some(ext);
            method = "nat_pmp";
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
        set_status(&status, RouteStatus::RelayOnly {
            reason: "Could not discover public IPv4 address.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    // 3. CGNAT check: if router external IP is in CGNAT range, direct ingress is impossible
    if net::is_cgnat_range(public_ip) {
        set_status(&status, RouteStatus::RelayOnly {
            reason: format!("Carrier-grade NAT detected ({public_ip}); direct routing unavailable."),
        });
        service.set_direct_route(None);
        return;
    }

    // 4. Start local direct ingress proxy
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

    // Attempt to bind ingress
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
            // Ephemeral port fallback
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
        set_status(&status, RouteStatus::RelayOnly {
            reason: "Cannot bind direct ingress listeners.".into(),
        });
        service.set_direct_route(None);
        return;
    };

    let bound_auth_port = ingress.auth_port();
    let bound_world_port = ingress.world_port();

    // 5. Automatic Port Mapping (if behind NAT)
    let is_behind_nat = net::is_private(lan_ip);
    let mut mapped_auth_port = bound_auth_port;
    let mut mapped_world_port = bound_world_port;
    let mut mapping_active = false;

    if is_behind_nat {
        let mut mapped = false;
        // Try UPnP IGD
        if let Some(ref gw) = upnp_gw {
            let auth_map = upnp::add_mapping(gw, bound_auth_port, lan_ip, "Auth Direct");
            let world_map = upnp::add_mapping(gw, bound_world_port, lan_ip, "World Direct");
            if auth_map.is_ok() && world_map.is_ok() {
                mapped = true;
                mapping_active = true;
                method = "upnp";
            }
        }

        // Fallback to NAT-PMP if UPnP failed
        if !mapped {
            let octets = lan_ip.octets();
            let gw_candidate = Ipv4Addr::new(octets[0], octets[1], octets[2], 1);
            let ma = nat_pmp::request_mapping(gw_candidate, bound_auth_port, bound_auth_port, 3600);
            let mw = nat_pmp::request_mapping(gw_candidate, bound_world_port, bound_world_port, 3600);
            if let (Some(a), Some(w)) = (ma, mw) {
                mapped_auth_port = a;
                mapped_world_port = w;
                mapped = true;
                mapping_active = true;
                method = "nat_pmp";
            }
        }

        if !mapped {
            set_status(&status, RouteStatus::RelayOnly {
                reason: "Router port mapping (UPnP/NAT-PMP) failed or unsupported.".into(),
            });
            service.set_direct_route(None);
            ingress.stop();
            return;
        }
    }

    // 6. External Reachability Verification Probe
    let mut verified = false;
    if let Some(ref base) = registry_base_url {
        let probe_url = format!("{}/coord/v1/probe", base.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .ok();
        if let Some(c) = client {
            let probe_body = coa_control_proto::coord::ProbePayload {
                ports: vec![mapped_auth_port, mapped_world_port],
            };
            let body_str = serde_json::to_string(&probe_body).unwrap_or_default();
            if let Ok(resp) = c.post(&probe_url)
                .header("Content-Type", "application/json")
                .body(body_str)
                .send()
            {
                if let Ok(text) = resp.text() {
                    if let Ok(p_resp) = serde_json::from_str::<coa_control_proto::coord::ProbeResponse>(&text) {
                        if p_resp.all_reachable {
                            verified = true;
                        }
                    }
                }
            }
        }
    } else {
        if std::env::var("COA_TEST_FORCE_DIRECT_VERIFIED").is_ok() {
            verified = true;
        }
    }

    if !verified {
        set_status(&status, RouteStatus::RelayOnly {
            reason: format!("External port reachability check failed on {public_ip}:{mapped_auth_port},{mapped_world_port}."),
        });
        service.set_direct_route(None);

        if mapping_active {
            if let Some(ref gw) = upnp_gw {
                let _ = upnp::remove_mapping(gw, mapped_auth_port);
                let _ = upnp::remove_mapping(gw, mapped_world_port);
            }
        }
        ingress.stop();
        return;
    }

    // 7. DIRECT_READY: update status and register direct route with HostService
    let direct_endpoint = format!("{public_ip}:{mapped_auth_port}");
    set_status(&status, RouteStatus::DirectReady {
        public_ip: public_ip.to_string(),
        auth_port: mapped_auth_port,
        world_port: mapped_world_port,
        method: method.to_string(),
    });
    service.set_direct_route(Some(direct_endpoint));

    // Keep running until stop requested
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
    }

    // Clean up on exit
    service.set_direct_route(None);
    if mapping_active {
        if let Some(ref gw) = upnp_gw {
            let _ = upnp::remove_mapping(gw, mapped_auth_port);
            let _ = upnp::remove_mapping(gw, mapped_world_port);
        }
    }
    ingress.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_status_serialization() {
        let s = RouteStatus::DirectReady {
            public_ip: "198.51.100.1".into(),
            auth_port: 3724,
            world_port: 8085,
            method: "upnp".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"status\":\"direct_ready\""));
        assert!(json.contains("198.51.100.1"));

        let r = RouteStatus::RelayOnly {
            reason: "CGNAT detected".into(),
        };
        let r_json = serde_json::to_string(&r).unwrap();
        assert!(r_json.contains("\"status\":\"relay_only\""));
    }
}
