import Foundation

/// How the client reaches the computer (PROTOCOL §1.1), as shown to the user.
public enum Route: String, Equatable, Sendable {
    case lan
    case tailscale
    case internet
}

/// Classifies destination addresses (PROTOCOL §1.1). Pure: parses numeric
/// literals only and never resolves names. Pinning a connection to the local
/// network (never over cellular) is allowed only for `.privateLAN`.
public enum AddressClass {
    public enum Kind: Equatable, Sendable {
        case loopback
        case privateLAN
        case tailscale
        case other
    }

    private struct Block {
        let network: [UInt8]
        let prefix: Int

        func contains(_ address: [UInt8]) -> Bool {
            guard address.count == network.count, prefix >= 0, prefix <= address.count * 8 else { return false }
            let fullBytes = prefix / 8
            for i in 0..<fullBytes where address[i] != network[i] { return false }
            let rest = prefix % 8
            guard rest > 0 else { return true }
            let mask = UInt8(truncatingIfNeeded: 0xFF << (8 - rest))
            return address[fullBytes] & mask == network[fullBytes] & mask
        }
    }

    private static let privateV4 = [
        Block(network: [10, 0, 0, 0], prefix: 8),
        Block(network: [172, 16, 0, 0], prefix: 12),
        Block(network: [192, 168, 0, 0], prefix: 16),
        Block(network: [169, 254, 0, 0], prefix: 16),   // link-local
    ]
    /// CGNAT shared space; Tailscale assigns its IPv4 addresses from it.
    private static let tailscaleV4 = Block(network: [100, 64, 0, 0], prefix: 10)
    private static let loopbackV4 = Block(network: [127, 0, 0, 0], prefix: 8)
    /// Tailscale's IPv6 range; it lies inside the ULA block, so it is checked first.
    private static let tailscaleV6 = Block(network: [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0] + [UInt8](repeating: 0, count: 10),
                                           prefix: 48)
    private static let privateV6 = [
        Block(network: [0xfc] + [UInt8](repeating: 0, count: 15), prefix: 7),        // unique local
        Block(network: [0xfe, 0x80] + [UInt8](repeating: 0, count: 14), prefix: 10), // link-local
    ]
    private static let loopbackV6 = [UInt8](repeating: 0, count: 15) + [1]

    /// The class of a numeric address (4 or 16 bytes, network order).
    public static func kind(of address: [UInt8]) -> Kind {
        if address.count == 4 {
            if loopbackV4.contains(address) { return .loopback }
            if privateV4.contains(where: { $0.contains(address) }) { return .privateLAN }
            if tailscaleV4.contains(address) { return .tailscale }
            return .other
        }
        if address.count == 16 {
            if address == loopbackV6 { return .loopback }
            if tailscaleV6.contains(address) { return .tailscale }
            if privateV6.contains(where: { $0.contains(address) }) { return .privateLAN }
        }
        return .other
    }

    /// The class of `host` when it is a numeric literal, nil for a host name.
    public static func kind(ofHost host: String) -> Kind? {
        literal(host).map { kind(of: $0) }
    }

    /// RFC 1918 / link-local / ULA literal: the only destinations a connection
    /// may be kept off cellular for.
    public static func isPrivateLAN(_ host: String) -> Bool {
        kind(ofHost: host) == .privateLAN
    }

    public static func route(of kind: Kind) -> Route {
        switch kind {
        case .loopback, .privateLAN: return .lan
        case .tailscale: return .tailscale
        case .other: return .internet
        }
    }

    /// `host` parsed as a numeric IPv4 (4 bytes) or IPv6 (16 bytes) literal;
    /// brackets and an IPv6 `%scope` are ignored. Nil for a host name.
    public static func literal(_ host: String) -> [UInt8]? {
        var h = host.trimmingCharacters(in: .whitespaces)
        if h.hasPrefix("[") { h.removeFirst() }
        if h.hasSuffix("]") { h.removeLast() }
        if let percent = h.firstIndex(of: "%") { h = String(h[..<percent]) }
        if !h.contains(":") {
            // Strict dotted quad: inet_pton would accept no more, but say so explicitly.
            let parts = h.split(separator: ".", omittingEmptySubsequences: false)
            guard parts.count == 4 else { return nil }
            var bytes: [UInt8] = []
            for part in parts {
                guard !part.isEmpty, part.count <= 3, part.allSatisfy({ $0.isASCII && $0.isNumber }),
                      let v = Int(part), v <= 255 else { return nil }
                bytes.append(UInt8(v))
            }
            return bytes
        }
        guard h.allSatisfy({ $0.isHexDigit || $0 == ":" || $0 == "." }) else { return nil }
        var address = in6_addr()
        guard inet_pton(AF_INET6, h, &address) == 1 else { return nil }
        return withUnsafeBytes(of: &address) { Array($0) }
    }
}
