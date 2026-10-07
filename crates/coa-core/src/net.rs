//! Network facts for "Play with Friends": addresses, carrier-grade NAT detection, whether internal services are exposed,
//! and Tailscale (private network) detection. Nothing here changes any setting.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{Error, Result};

/// This computer's address on the local network (the interface the default route uses). No packet is sent.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET-1: only selects the outgoing interface
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanAddress {
    pub interface: String,
    pub address: String,
    pub is_default: bool,
}

/// Suitable unicast IPv4 addresses; shared carrier/Tailscale space belongs to Private mode.
pub fn is_lan_address(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local()
        && !ip.is_multicast() && !ip.is_broadcast() && ip.octets()[0] != 0
        && ip.octets()[0] < 240 && !is_cgnat_range(ip)
}

pub fn validate_lan_address(address: &str) -> Result<Ipv4Addr> {
    address.parse::<Ipv4Addr>().ok().filter(|ip| is_lan_address(*ip))
        .ok_or_else(|| Error::Invalid("Invalid IPv4 address for local network.".into()))
}

/// An absent/disconnected manual address is intentional: never fall back to another adapter.
pub fn resolve_lan_host(manual: Option<&str>, automatic: Option<Ipv4Addr>) -> Result<String> {
    match manual {
        Some(address) => validate_lan_address(address).map(|ip| ip.to_string()),
        None => automatic.filter(|ip| is_lan_address(*ip)).map(|ip| ip.to_string())
            .ok_or_else(|| Error::Invalid("This computer has no usable local network address.".into())),
    }
}

fn lan_candidates(rows: impl IntoIterator<Item = (String, Ipv4Addr)>, automatic: Option<Ipv4Addr>) -> Vec<LanAddress> {
    let mut rows: Vec<_> = rows.into_iter().filter(|(_, ip)| is_lan_address(*ip)).collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter(|(_, ip)| seen.insert(*ip)).map(|(interface, ip)| LanAddress {
        interface, address: ip.to_string(), is_default: Some(ip) == automatic,
    }).collect()
}

/// Native OS enumeration (GetAdaptersAddresses on Windows, getifaddrs on Unix).
pub fn lan_addresses(automatic: Option<Ipv4Addr>) -> Result<Vec<LanAddress>> {
    let interfaces = if_addrs::get_if_addrs()?;
    Ok(lan_candidates(interfaces.into_iter().filter(|i| i.is_oper_up()).filter_map(|i| {
        match i.ip() { IpAddr::V4(ip) => Some((i.name, ip)), _ => None }
    }), automatic))
}

pub fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_link_local()
}

/// 100.64.0.0/10: shared address space used by carriers for CGNAT (and by Tailscale for its own network).
pub fn is_cgnat_range(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// The router's outside address is a normal public address that matches the one the internet sees.
    DirectPossible,
    /// The router's outside address is private/shared: the provider shares one public address between customers.
    Cgnat,
    /// Not enough information (no router answer).
    Unknown,
}

/// `router_external`: the address the router reports for its internet side (UPnP); `public`: what an outside service sees.
pub fn classify(router_external: Option<Ipv4Addr>, public: Option<Ipv4Addr>) -> Reachability {
    match (router_external, public) {
        (Some(r), _) if is_private(r) || is_cgnat_range(r) => Reachability::Cgnat,
        (Some(r), Some(p)) if r != p => Reachability::Cgnat,
        (Some(_), Some(_)) => Reachability::DirectPossible,
        (Some(r), None) => {
            let _ = r;
            Reachability::Unknown
        }
        _ => Reachability::Unknown,
    }
}

/// Ask an outside service which public address this connection comes from. Only called when the user asks.
pub fn public_ip() -> Result<Ipv4Addr> {
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(8)).build().map_err(|e| Error::Invalid(e.to_string()))?;
    let text = client.get("https://api.ipify.org").send().and_then(|r| r.text()).map_err(|_| Error::Invalid("Could not reach the internet to find your public address.".into()))?;
    text.trim().parse().map_err(|_| Error::Invalid("The address service gave an unexpected answer.".into()))
}

#[derive(Debug, Clone, Serialize)]
pub struct Exposure {
    pub port: u16,
    pub what: &'static str,
    /// The service accepts connections from other computers.
    pub reachable_from_network: bool,
    pub listening: bool,
}

/// For each internal service port: does it listen on more than the loopback interface? MySQL and the server console must not.
pub fn exposure(ports: &crate::layout::Ports) -> Vec<Exposure> {
    let all = crate::process::listeners_detailed();
    let check = |port: u16, what: &'static str| {
        let rows: Vec<_> = all.iter().filter(|l| l.port == port).collect();
        Exposure { port, what, listening: !rows.is_empty(), reachable_from_network: rows.iter().any(|l| !l.loopback_only) }
    };
    vec![check(ports.mysql, "database"), check(ports.ra, "server console"), check(ports.auth, "login server"), check(ports.world, "game world")]
}

#[derive(Debug, Clone, Serialize)]
pub struct Tailscale {
    pub installed: bool,
    /// This computer's private address (100.x.y.z) when connected.
    pub ip: Option<String>,
    pub connected: bool,
}

#[cfg(windows)]
fn tailscale_exe() -> Option<PathBuf> {
    ["C:/Program Files/Tailscale/tailscale.exe", "C:/Program Files (x86)/Tailscale/tailscale.exe"].iter().map(PathBuf::from).find(|p| p.is_file())
}

#[cfg(not(windows))]
fn tailscale_exe() -> Option<PathBuf> {
    ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/usr/sbin/tailscale"].iter().map(PathBuf::from).find(|p| p.is_file())
}

pub fn parse_tailscale_ip(out: &str) -> Option<String> {
    out.lines().map(str::trim).find(|l| l.parse::<Ipv4Addr>().map(is_cgnat_range).unwrap_or(false)).map(str::to_string)
}

pub fn tailscale() -> Tailscale {
    let Some(exe) = tailscale_exe() else { return Tailscale { installed: false, ip: None, connected: false } };
    let mut c = Command::new(exe);
    c.args(["ip", "-4"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let ip = c.output().ok().filter(|o| o.status.success()).and_then(|o| parse_tailscale_ip(&String::from_utf8_lossy(&o.stdout)));
    Tailscale { installed: true, connected: ip.is_some(), ip }
}

/// Connection text a friend can use: the address to put in the game client.
pub fn instructions(host: &str) -> String {
    format!("1. Close the game.\n2. Open the file Data\\enUS\\realmlist.wtf in your game folder (any language folder under Data).\n3. Replace its content with:\n\n   set realmlist {host}\n\n4. Start the game and log in with the account your host created for you.\n")
}

pub fn locate_realmlist_dir(_root: &Path) -> &'static str {
    "Data\\enUS\\realmlist.wtf"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn lan_validation_and_resolution_never_replace_manual_addresses() {
        for good in ["192.168.1.20", "10.0.0.5", "172.16.2.4"] {
            assert!(validate_lan_address(good).is_ok());
        }
        for bad in ["abc", "999.1.1.1", "0.0.0.0", "0.1.2.3", "127.0.0.1", "127.9.1.2", "169.254.10.20", "100.101.20.5", "224.0.0.1", "255.255.255.255", "240.0.0.1", "192.168.1.1'; DROP TABLE realmlist"] {
            assert!(validate_lan_address(bad).is_err(), "{bad}");
        }
        let automatic = Some(ip("192.168.0.169"));
        assert_eq!(resolve_lan_host(None, automatic).unwrap(), "192.168.0.169");
        assert_eq!(resolve_lan_host(Some("192.168.1.50"), automatic).unwrap(), "192.168.1.50");
        assert_eq!(resolve_lan_host(Some("192.168.1.50"), None).unwrap(), "192.168.1.50");
        assert!(resolve_lan_host(Some("invalid"), automatic).is_err());
        assert!(resolve_lan_host(None, None).is_err());
        assert!(resolve_lan_host(None, Some(ip("169.254.1.2"))).is_err());
        assert!(resolve_lan_host(None, Some(ip("100.100.1.2"))).is_err());
    }

    #[test]
    fn candidates_are_filtered_deduplicated_deterministic_and_default_marked() {
        let rows = vec![
            ("Wi-Fi", "192.168.0.25"), ("Ethernet 2", "192.168.1.50"),
            ("Ethernet", "192.168.0.169"), ("Duplicate", "192.168.1.50"),
            ("Loopback", "127.0.0.2"), ("APIPA", "169.254.1.2"),
            ("Tailscale", "100.101.20.5"), ("Empty", "0.0.0.0"),
            ("Hyper-V Virtual", "10.0.0.5"),
        ];
        let make = |rows: Vec<(&str, &str)>| lan_candidates(rows.into_iter().map(|(name, address)| (name.into(), ip(address))), Some(ip("192.168.0.169")));
        let result = make(rows.clone());
        assert_eq!(result.len(), 4);
        assert_eq!(result.iter().filter(|a| a.is_default).count(), 1);
        assert!(result.iter().any(|a| a.interface == "Hyper-V Virtual"));
        assert!(result.iter().any(|a| a.is_default && a.address == "192.168.0.169"));
        assert_eq!(result, make(rows.into_iter().rev().collect()));
    }

    #[test]
    fn address_classes() {
        assert!(is_private(ip("192.168.1.5")) && is_private(ip("10.0.0.2")) && is_private(ip("172.16.4.4")) && is_private(ip("127.0.0.1")));
        assert!(!is_private(ip("8.8.8.8")));
        assert!(is_cgnat_range(ip("100.64.0.1")) && is_cgnat_range(ip("100.127.255.254")) && !is_cgnat_range(ip("100.128.0.1")) && !is_cgnat_range(ip("100.63.0.1")));
    }

    #[test]
    fn cgnat_detection_from_router_and_public_addresses() {
        assert_eq!(classify(Some(ip("203.0.113.9")), Some(ip("203.0.113.9"))), Reachability::DirectPossible);
        assert_eq!(classify(Some(ip("100.70.1.1")), Some(ip("203.0.113.9"))), Reachability::Cgnat);
        assert_eq!(classify(Some(ip("10.5.5.5")), None), Reachability::Cgnat);
        assert_eq!(classify(Some(ip("203.0.113.9")), Some(ip("198.51.100.4"))), Reachability::Cgnat, "router thinks one address, the internet sees another");
        assert_eq!(classify(None, Some(ip("203.0.113.9"))), Reachability::Unknown);
    }

    #[test]
    fn lan_ip_is_a_private_address_when_present() {
        if let Some(a) = lan_ip() {
            assert!(!a.is_loopback() && !a.is_unspecified());
        }
    }

    #[test]
    fn tailscale_output_parsing_and_instructions() {
        assert_eq!(parse_tailscale_ip("100.101.102.103\n"), Some("100.101.102.103".into()));
        assert_eq!(parse_tailscale_ip("192.168.1.2\n"), None);
        assert_eq!(parse_tailscale_ip(""), None);
        assert!(instructions("play.example.com").contains("set realmlist play.example.com"));
    }
}
