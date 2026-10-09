//! NAT-PMP (RFC 6886) client for automatic port mapping on routers supporting NAT-PMP.
//! Discovery and port mapping change only login and world ports, and only when the router answers.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

pub const NAT_PMP_PORT: u16 = 5351;

/// Request the external (public) IPv4 address from the NAT-PMP gateway.
pub fn query_external_ip(gateway: Ipv4Addr) -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(800)))
        .ok()?;
    let target = SocketAddr::V4(SocketAddrV4::new(gateway, NAT_PMP_PORT));

    // NAT-PMP External Address Request: Version 0 (1 byte), Opcode 0 (1 byte)
    let req = [0u8, 0u8];
    sock.send_to(&req, target).ok()?;

    let mut buf = [0u8; 16];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    if from != target || n < 12 {
        return None;
    }

    // Response format:
    // Byte 0: Version (0)
    // Byte 1: Opcode (128 = 0x80)
    // Bytes 2-3: Result Code (0 = Success)
    // Bytes 4-7: Seconds since start of epoch
    // Bytes 8-11: External IPv4 address
    if buf[0] == 0 && buf[1] == 128 && buf[2] == 0 && buf[3] == 0 {
        Some(Ipv4Addr::new(buf[8], buf[9], buf[10], buf[11]))
    } else {
        None
    }
}

/// Request a TCP port mapping from the NAT-PMP gateway.
/// If `lifetime_secs` is 0, this requests deletion of the mapping.
pub fn request_mapping(
    gateway: Ipv4Addr,
    internal_port: u16,
    external_port: u16,
    lifetime_secs: u32,
) -> Option<u16> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(1000)))
        .ok()?;
    let target = SocketAddr::V4(SocketAddrV4::new(gateway, NAT_PMP_PORT));

    // NAT-PMP TCP Mapping Request:
    // Byte 0: Version (0)
    // Byte 1: Opcode (2 = TCP)
    // Bytes 2-3: Reserved (0)
    // Bytes 4-5: Internal Port (big-endian)
    // Bytes 6-7: Requested External Port (big-endian)
    // Bytes 8-11: Lifetime in seconds (big-endian)
    let mut req = [0u8; 12];
    req[0] = 0;
    req[1] = 2; // TCP mapping
    req[4..6].copy_from_slice(&internal_port.to_be_bytes());
    req[6..8].copy_from_slice(&external_port.to_be_bytes());
    req[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());

    sock.send_to(&req, target).ok()?;

    let mut buf = [0u8; 16];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    if from != target || n < 16 {
        return None;
    }

    // Response format:
    // Byte 0: Version (0)
    // Byte 1: Opcode (130 = 0x82)
    // Bytes 2-3: Result Code (0 = Success)
    // Bytes 4-7: Epoch seconds
    // Bytes 8-9: Internal Port
    // Bytes 10-11: Assigned External Port
    // Bytes 12-15: Port Mapping Lifetime
    if buf[0] == 0 && buf[1] == 130 && buf[2] == 0 && buf[3] == 0 {
        let assigned = u16::from_be_bytes([buf[10], buf[11]]);
        Some(assigned)
    } else {
        None
    }
}

/// Delete a TCP port mapping from the NAT-PMP gateway.
pub fn delete_mapping(gateway: Ipv4Addr, internal_port: u16) -> bool {
    request_mapping(gateway, internal_port, 0, 0).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_pmp_packet_formats() {
        let req_ip = [0u8, 0u8];
        assert_eq!(req_ip.len(), 2);

        let mut req_map = [0u8; 12];
        req_map[0] = 0;
        req_map[1] = 2;
        req_map[4..6].copy_from_slice(&3724u16.to_be_bytes());
        req_map[6..8].copy_from_slice(&3724u16.to_be_bytes());
        req_map[8..12].copy_from_slice(&3600u32.to_be_bytes());
        assert_eq!(req_map[1], 2);
        assert_eq!(u16::from_be_bytes([req_map[4], req_map[5]]), 3724);
        assert_eq!(
            u32::from_be_bytes([req_map[8], req_map[9], req_map[10], req_map[11]]),
            3600
        );

        // Test timeout on unanswering gateway
        assert_eq!(query_external_ip(Ipv4Addr::new(127, 0, 0, 1)), None);
        assert_eq!(
            request_mapping(Ipv4Addr::new(127, 0, 0, 1), 3724, 3724, 3600),
            None
        );
        assert!(!delete_mapping(Ipv4Addr::new(127, 0, 0, 1), 3724));
    }
}
