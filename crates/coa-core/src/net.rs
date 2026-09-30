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

fn tailscale_exe() -> Option<PathBuf> {
    ["C:/Program Files/Tailscale/tailscale.exe", "C:/Program Files (x86)/Tailscale/tailscale.exe"].iter().map(PathBuf::from).find(|p| p.is_file())
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
