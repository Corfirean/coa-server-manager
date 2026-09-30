//! Router port forwarding through UPnP (IGD), for players whose router supports it. Discovery and reading the router's
//! outside address change nothing; adding a mapping only ever touches the login and world ports, with a Manager-owned
//! description so the mappings can be recognised and removed again.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

pub const DESCRIPTION_PREFIX: &str = "CoA Server Manager";

#[derive(Debug, Clone)]
pub struct Gateway {
    pub control_url: String,
    pub service: String,
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)? + i;
    Some(s[i..j].trim())
}

/// `LOCATION:` header of an SSDP response (case-insensitive).
pub fn parse_location(response: &str) -> Option<String> {
    response.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("location").then(|| v.trim().to_string())
    })
}

fn host_of(url: &str) -> Option<Ipv4Addr> {
    let rest = url.strip_prefix("http://")?;
    rest.split(['/', ':']).next()?.parse().ok()
}

/// Only devices on the local network are ever contacted (an SSDP answer must not be able to point us elsewhere).
pub fn is_lan_url(url: &str) -> bool {
    host_of(url).map(crate::net::is_private).unwrap_or(false)
}

/// From a device description document: the WAN connection service and its absolute control URL.
pub fn parse_description(xml: &str, location: &str) -> Option<Gateway> {
    let mut rest = xml;
    while let Some(i) = rest.find("<service>") {
        let block = between(&rest[i..], "<service>", "</service>")?;
        let stype = between(block, "<serviceType>", "</serviceType>")?;
        if stype.contains("WANIPConnection") || stype.contains("WANPPPConnection") {
            let control = between(block, "<controlURL>", "</controlURL>")?;
            let origin = {
                let after = location.strip_prefix("http://")?;
                format!("http://{}", after.split('/').next()?)
            };
            let base = between(xml, "<URLBase>", "</URLBase>").map(|b| b.trim_end_matches('/').to_string()).unwrap_or(origin);
            let url = if control.starts_with("http://") { control.to_string() } else { format!("{base}/{}", control.trim_start_matches('/')) };
            return Some(Gateway { control_url: url, service: stype.to_string() });
        }
        rest = &rest[i + "<service>".len()..];
    }
    None
}

fn envelope(service: &str, action: &str, args: &[(&str, String)]) -> String {
    let body: String = args.iter().map(|(k, v)| format!("<{k}>{}</{k}>", v.replace('&', "&amp;").replace('<', "&lt;"))).collect();
    format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{body}</u:{action}></s:Body></s:Envelope>")
}

fn soap(gw: &Gateway, action: &str, args: &[(&str, String)]) -> Result<String> {
    if !is_lan_url(&gw.control_url) {
        return Err(Error::Invalid("The router address is not on your network; refusing to contact it.".into()));
    }
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(6)).build().map_err(|e| Error::Invalid(e.to_string()))?;
    let resp = client
        .post(&gw.control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction", format!("\"{}#{action}\"", gw.service))
        .body(envelope(&gw.service, action, args))
        .send()
        .map_err(|_| Error::Invalid("The router did not answer.".into()))?;
    let ok = resp.status().is_success();
    let text = resp.text().unwrap_or_default();
    if ok {
        Ok(text)
    } else {
        Err(Error::Invalid(between(&text, "<errorDescription>", "</errorDescription>").unwrap_or("the router refused the request").to_string()))
    }
}

/// Look for a UPnP router for a couple of seconds.
pub fn discover() -> Option<Gateway> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(600))).ok()?;
    let msg = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
    let target: SocketAddr = "239.255.255.250:1900".parse().ok()?;
    let _ = sock.send_to(msg.as_bytes(), target);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 2048];
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(4)).build().ok()?;
    while Instant::now() < deadline {
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let Some(loc) = parse_location(&String::from_utf8_lossy(&buf[..n])) else { continue };
        if !is_lan_url(&loc) {
            continue;
        }
        if let Some(gw) = client.get(&loc).send().ok().and_then(|r| r.text().ok()).and_then(|x| parse_description(&x, &loc)) {
            return Some(gw);
        }
    }
    None
}

/// The address the router has on the internet side.
pub fn external_ip(gw: &Gateway) -> Result<Ipv4Addr> {
    let text = soap(gw, "GetExternalIPAddress", &[])?;
    between(&text, "<NewExternalIPAddress>", "</NewExternalIPAddress>")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Invalid("The router did not report an outside address.".into()))
}

pub fn add_mapping(gw: &Gateway, port: u16, lan: Ipv4Addr, what: &str) -> Result<()> {
    soap(
        gw,
        "AddPortMapping",
        &[
            ("NewRemoteHost", String::new()),
            ("NewExternalPort", port.to_string()),
            ("NewProtocol", "TCP".into()),
            ("NewInternalPort", port.to_string()),
            ("NewInternalClient", lan.to_string()),
            ("NewEnabled", "1".into()),
            ("NewPortMappingDescription", format!("{DESCRIPTION_PREFIX} - {what}")),
            ("NewLeaseDuration", "0".into()),
        ],
    )
    .map(|_| ())
}

pub fn remove_mapping(gw: &Gateway, port: u16) -> Result<()> {
    soap(gw, "DeletePortMapping", &[("NewRemoteHost", String::new()), ("NewExternalPort", port.to_string()), ("NewProtocol", "TCP".into())]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "<root><device><deviceList><device><deviceList><device><serviceList>\
        <service><serviceType>urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1</serviceType><controlURL>/ctl/common</controlURL></service>\
        <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/ctl/IPConn</controlURL></service>\
        </serviceList></device></deviceList></device></deviceList></device></root>";

    #[test]
    fn parses_ssdp_location_and_description() {
        let resp = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\nST: x\r\n\r\n";
        let loc = parse_location(resp).unwrap();
        assert_eq!(loc, "http://192.168.1.1:5000/rootDesc.xml");
        let gw = parse_description(DESC, &loc).unwrap();
        assert_eq!(gw.control_url, "http://192.168.1.1:5000/ctl/IPConn");
        assert!(gw.service.ends_with("WANIPConnection:1"));
        assert!(parse_description("<root></root>", &loc).is_none());
    }

    #[test]
    fn only_lan_devices_are_contacted() {
        assert!(is_lan_url("http://192.168.1.1:5000/x") && is_lan_url("http://10.0.0.1/x"));
        assert!(!is_lan_url("http://8.8.8.8/x") && !is_lan_url("https://192.168.1.1/x") && !is_lan_url("http://example.com/x"));
        let evil = Gateway { control_url: "http://203.0.113.5/ctl".into(), service: "urn:x".into() };
        assert!(external_ip(&evil).is_err(), "a public address is refused before any request");
        assert!(add_mapping(&evil, 3724, "192.168.1.5".parse().unwrap(), "Auth").is_err());
    }

    #[test]
    fn envelopes_carry_only_the_requested_port_and_escape_values() {
        let e = envelope("urn:s", "AddPortMapping", &[("NewExternalPort", "3724".into()), ("NewPortMappingDescription", "a<b&c".into())]);
        assert!(e.contains("<NewExternalPort>3724</NewExternalPort>") && e.contains("a&lt;b&amp;c"));
        assert!(e.contains("<u:AddPortMapping xmlns:u=\"urn:s\">"));
    }
}
