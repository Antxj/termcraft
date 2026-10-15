//! Wake-on-LAN (Tools › "Wake On Lan"): the "magic packet" that wakes one
//! computer, given its MAC address, sent once by UDP to a broadcast (or
//! unicast) address.

use std::net::{Ipv4Addr, UdpSocket};

/// Default port of magic packets (the "discard" port).
pub const DEFAULT_PORT: u16 = 9;

/// A MAC address written `aa:bb:cc:dd:ee:ff`, `aa-bb-…` or `aabbccddeeff`.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let hex: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// Six 0xFF bytes, then the MAC address sixteen times.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFFu8; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        *chunk = mac;
    }
    packet
}

/// Send one magic packet for `mac` to `address:port`.
pub fn wake(mac: [u8; 6], address: Ipv4Addr, port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    socket.send_to(&magic_packet(mac), (address, port))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_addresses() {
        let mac = [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e];
        assert_eq!(parse_mac("00:1A:2B:3C:4D:5E"), Some(mac));
        assert_eq!(parse_mac("00-1a-2b-3c-4d-5e"), Some(mac));
        assert_eq!(parse_mac(" 001a2b3c4d5e "), Some(mac));
        assert_eq!(parse_mac("00:1a:2b:3c:4d"), None);
        assert_eq!(parse_mac("zz:1a:2b:3c:4d:5e"), None);
    }

    #[test]
    fn packet_layout() {
        let mac = [1, 2, 3, 4, 5, 6];
        let p = magic_packet(mac);
        assert_eq!(&p[..6], &[0xFF; 6]);
        assert!(p[6..].chunks(6).all(|c| c == mac));
    }

    #[test]
    fn sends_to_a_local_port() {
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mac = [0xAA; 6];
        wake(mac, Ipv4Addr::LOCALHOST, port).unwrap();
        let mut buf = [0u8; 200];
        listener
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let (n, _) = listener.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], &magic_packet(mac)[..]);
    }
}
