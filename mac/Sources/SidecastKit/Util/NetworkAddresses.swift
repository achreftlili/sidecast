import Foundation

/// Which of this Mac's IPv4 addresses to advertise in SERVER_HELLO (PROTOCOL §1.1),
/// and as what. Pure logic so it can be unit-tested.
public enum NetworkAddresses {
    /// A parsed dotted-quad IPv4 address, or nil.
    public static func octets(_ ip: String) -> (UInt8, UInt8, UInt8, UInt8)? {
        let parts = ip.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        var o: [UInt8] = []
        for part in parts {
            guard !part.isEmpty, part.count <= 3, part.allSatisfy(\.isASCII), let v = UInt8(part) else { return nil }
            o.append(v)
        }
        return (o[0], o[1], o[2], o[3])
    }

    /// 100.64.0.0/10 (100.64.0.0 – 100.127.255.255): Tailscale's range (RFC 6598 shared space).
    public static func isTailscaleRange(_ ip: String) -> Bool {
        guard let o = octets(ip) else { return false }
        return o.0 == 100 && (o.1 & 0xC0) == 64
    }

    /// RFC 1918: 10/8, 172.16/12, 192.168/16.
    public static func isPrivate(_ ip: String) -> Bool {
        guard let o = octets(ip) else { return false }
        return o.0 == 10 || (o.0 == 172 && (o.1 & 0xF0) == 16) || (o.0 == 192 && o.1 == 168)
    }

    /// 169.254/16.
    public static func isLinkLocal(_ ip: String) -> Bool {
        guard let o = octets(ip) else { return false }
        return o.0 == 169 && o.1 == 254
    }

    /// Wi-Fi, Ethernet, Thunderbolt bridge and Internet Sharing interfaces.
    public static func isLANInterface(_ name: String) -> Bool {
        name.hasPrefix("en") || name.hasPrefix("bridge")
    }

    /// The kind to advertise an up interface's IPv4 address as, or nil to skip it:
    /// - 100.64.0.0/10 on a tunnel (`utun*`, …) → `tailscale`;
    /// - anything usable on Wi-Fi/Ethernet/bridge → `lan` (normally RFC 1918; a
    ///   CGNAT 100.64/10 or public address on such an interface is still the LAN path);
    /// - loopback, link-local, unspecified and anything else → skipped.
    public static func classify(interface: String, ipv4 ip: String) -> AddressKind? {
        guard let o = octets(ip), o.0 != 0, o.0 != 127, o.0 < 224, !isLinkLocal(ip) else { return nil }
        if isLANInterface(interface) { return .lan }
        return isTailscaleRange(ip) ? .tailscale : nil
    }

    /// LAN first, then Tailscale, then MagicDNS names; duplicates removed, order otherwise kept.
    public static func ordered(_ addresses: [ServerAddress]) -> [ServerAddress] {
        let rank: [AddressKind: Int] = [.lan: 0, .tailscale: 1, .tailscaleDNS: 2]
        var seen = Set<String>()
        let unique = addresses.filter { seen.insert($0.host.lowercased()).inserted }
        return unique.enumerated()
            .sorted { a, b in
                let ra = a.element.addressKind.flatMap { rank[$0] } ?? 3
                let rb = b.element.addressKind.flatMap { rank[$0] } ?? 3
                return ra != rb ? ra < rb : a.offset < b.offset
            }
            .map(\.element)
    }

    /// `Self.DNSName` from `tailscale status --json`, without the trailing dot.
    public static func magicDNSName(fromStatusJSON data: Data) -> String? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let me = json["Self"] as? [String: Any],
              var name = me["DNSName"] as? String else { return nil }
        while name.hasSuffix(".") { name.removeLast() }
        name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? nil : name
    }
}
