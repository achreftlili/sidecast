//! Which of the host's IPv4 addresses to advertise in SERVER_HELLO, and as
//! what (PROTOCOL §1.1). Ported from the Mac app's `NetworkAddresses.swift`,
//! with interface-name rules extended to Linux and Windows.

use std::collections::HashSet;

use crate::protocol::{AddressKind, ServerAddress};

/// A parsed dotted-quad IPv4 address.
pub fn octets(ip: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut o = [0u8; 4];
    for (slot, part) in o.iter_mut().zip(parts) {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(o)
}

/// 100.64.0.0/10 (100.64.0.0 – 100.127.255.255): Tailscale's range (RFC 6598 shared space).
pub fn is_tailscale_range(ip: &str) -> bool {
    octets(ip).is_some_and(|o| o[0] == 100 && (o[1] & 0xC0) == 64)
}

/// RFC 1918: 10/8, 172.16/12, 192.168/16.
pub fn is_private(ip: &str) -> bool {
    octets(ip).is_some_and(|o| o[0] == 10 || (o[0] == 172 && (o[1] & 0xF0) == 16) || (o[0] == 192 && o[1] == 168))
}

/// 169.254/16.
pub fn is_link_local(ip: &str) -> bool {
    octets(ip).is_some_and(|o| o[0] == 169 && o[1] == 254)
}

/// Tunnel interfaces: Tailscale's own (`tailscale0` on Linux, "Tailscale" on
/// Windows), macOS `utun*`, and generic VPN tunnels.
pub fn is_tunnel_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["utun", "tailscale", "tun", "wg", "ppp", "ipsec", "zt"].iter().any(|p| n.starts_with(p))
}

/// Interfaces a phone on the same network cannot reach: loopback, container
/// and VM bridges, AirDrop/Wi-Fi Direct links.
pub fn is_virtual_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // `lo`, `lo0`: but not Windows' "Local Area Connection".
    let loopback =
        n.strip_prefix("lo").is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit())) || n.starts_with("loopback");
    loopback
        || [
            "docker",
            "br-",
            "veth",
            "virbr",
            "vboxnet",
            "vmnet",
            "vethernet",
            "vmware",
            "virtualbox",
            "hyper-v",
            "awdl",
            "llw",
            "anpi",
            "ap1",
            "gif",
            "stf",
            "lxc",
            "lxd",
            "cni",
            "flannel",
            "podman",
        ]
        .iter()
        .any(|p| n.starts_with(p))
}

/// The kind to advertise an up interface's IPv4 address as, or `None` to skip it:
/// - 100.64.0.0/10 on a tunnel (`utun*`, `tailscale0`, "Tailscale") → `tailscale`;
/// - anything usable on a physical interface (Wi-Fi, Ethernet, Internet
///   sharing bridges) → `lan` (normally RFC 1918; a CGNAT 100.64/10 or public
///   address there is still the LAN path);
/// - loopback, link-local, multicast, other tunnels (corporate VPNs) and
///   container/VM bridges → skipped.
pub fn classify(interface: &str, ip: &str) -> Option<AddressKind> {
    let o = octets(ip)?;
    if o[0] == 0 || o[0] == 127 || o[0] >= 224 || is_link_local(ip) {
        return None;
    }
    if is_tunnel_interface(interface) {
        return is_tailscale_range(ip).then_some(AddressKind::Tailscale);
    }
    if is_virtual_interface(interface) {
        return None;
    }
    Some(AddressKind::Lan)
}

/// LAN first, then Tailscale, then MagicDNS names; duplicates removed, order otherwise kept.
pub fn ordered(addresses: &[ServerAddress]) -> Vec<ServerAddress> {
    let rank = |a: &ServerAddress| match a.address_kind() {
        Some(AddressKind::Lan) => 0,
        Some(AddressKind::Tailscale) => 1,
        Some(AddressKind::TailscaleDns) => 2,
        None => 3,
    };
    let mut seen = HashSet::new();
    let mut unique: Vec<ServerAddress> =
        addresses.iter().filter(|a| seen.insert(a.host.to_ascii_lowercase())).cloned().collect();
    unique.sort_by_key(rank); // stable: keeps the original order within a kind
    unique
}

/// `Self.DNSName` from `tailscale status --json`, without the trailing dot.
pub fn magic_dns_name(status_json: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(status_json).ok()?;
    let name = json.get("Self")?.get("DNSName")?.as_str()?;
    let name = name.trim_end_matches('.').trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_range_boundaries() {
        assert!(!is_tailscale_range("100.63.255.255"));
        assert!(is_tailscale_range("100.64.0.0"));
        assert!(is_tailscale_range("100.101.102.103"));
        assert!(is_tailscale_range("100.127.255.255"));
        assert!(!is_tailscale_range("100.128.0.0"));
        assert!(!is_tailscale_range("101.64.0.1"));
    }

    #[test]
    fn rfc1918() {
        for ip in ["10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.0.1", "192.168.255.254"] {
            assert!(is_private(ip), "{ip}");
        }
        for ip in ["11.0.0.1", "172.15.255.255", "172.32.0.0", "192.169.0.1", "192.167.1.1", "100.64.0.1", "8.8.8.8"] {
            assert!(!is_private(ip), "{ip}");
        }
    }

    #[test]
    fn classification_on_mac_interface_names() {
        use AddressKind::*;
        assert_eq!(classify("en0", "192.168.1.20"), Some(Lan));
        assert_eq!(classify("en7", "10.0.0.5"), Some(Lan));
        assert_eq!(classify("bridge100", "192.168.2.1"), Some(Lan));
        assert_eq!(classify("utun4", "100.101.102.103"), Some(Tailscale));
        assert_eq!(classify("utun4", "100.64.0.0"), Some(Tailscale));
        assert_eq!(classify("utun4", "100.127.255.255"), Some(Tailscale));
        // A CGNAT-range address on Wi-Fi is the LAN path, not Tailscale.
        assert_eq!(classify("en0", "100.70.1.2"), Some(Lan));
        // Other tunnels (corporate VPN), link-local, loopback and junk are not advertised.
        assert_eq!(classify("utun2", "10.8.0.6"), None);
        assert_eq!(classify("utun2", "100.128.0.1"), None);
        assert_eq!(classify("en0", "169.254.10.20"), None);
        assert_eq!(classify("lo0", "127.0.0.1"), None);
        assert_eq!(classify("en0", "0.0.0.0"), None);
        assert_eq!(classify("en0", "224.0.0.251"), None);
        assert_eq!(classify("en0", "1.2.3"), None);
        assert_eq!(classify("en0", "256.1.1.1"), None);
        assert_eq!(classify("en0", "a.b.c.d"), None);
    }

    #[test]
    fn classification_on_linux_and_windows_interface_names() {
        use AddressKind::*;
        assert_eq!(classify("wlp2s0", "192.168.1.30"), Some(Lan));
        assert_eq!(classify("eth0", "10.0.0.7"), Some(Lan));
        assert_eq!(classify("tailscale0", "100.90.1.2"), Some(Tailscale));
        assert_eq!(classify("docker0", "172.17.0.1"), None);
        assert_eq!(classify("br-1a2b3c", "172.18.0.1"), None);
        assert_eq!(classify("virbr0", "192.168.122.1"), None);
        assert_eq!(classify("wg0", "10.6.0.2"), None);
        assert_eq!(classify("Wi-Fi", "192.168.1.40"), Some(Lan));
        assert_eq!(classify("Ethernet 2", "192.168.0.12"), Some(Lan));
        assert_eq!(classify("Tailscale", "100.100.100.1"), Some(Tailscale));
        assert_eq!(classify("vEthernet (WSL)", "172.28.0.1"), None);
        assert_eq!(classify("Local Area Connection", "192.168.0.9"), Some(Lan));
        assert_eq!(classify("lo", "127.0.0.1"), None);
        assert_eq!(classify("Loopback Pseudo-Interface 1", "10.1.1.1"), None);
    }

    #[test]
    fn ordering_is_lan_then_tailscale_then_dns_without_duplicates() {
        let input = [
            ServerAddress::new("mac.tail1234.ts.net", AddressKind::TailscaleDns),
            ServerAddress::new("100.101.102.103", AddressKind::Tailscale),
            ServerAddress::new("192.168.1.20", AddressKind::Lan),
            ServerAddress::new("10.0.0.5", AddressKind::Lan),
            ServerAddress::new("192.168.1.20", AddressKind::Lan),
        ];
        let hosts: Vec<String> = ordered(&input).into_iter().map(|a| a.host).collect();
        assert_eq!(hosts, ["192.168.1.20", "10.0.0.5", "100.101.102.103", "mac.tail1234.ts.net"]);
    }

    #[test]
    fn magic_dns_name_from_status_json() {
        let json = br#"{"Version":"1.76.1","Self":{"DNSName":"achrefs-mbp.tail1234.ts.net.","TailscaleIPs":["100.101.102.103"]}}"#;
        assert_eq!(magic_dns_name(json).as_deref(), Some("achrefs-mbp.tail1234.ts.net"));
        assert_eq!(magic_dns_name(br#"{"Self":{"DNSName":""}}"#), None);
        assert_eq!(magic_dns_name(br#"{"BackendState":"Stopped"}"#), None);
        assert_eq!(magic_dns_name(b"not json"), None);
    }
}
