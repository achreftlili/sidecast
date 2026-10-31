import Foundation

/// What a client remembers about a computer it paired with (PROTOCOL §5
/// "Pairing", step 4), keyed by `serverId`.
public struct PairedComputer: Codable, Equatable, Sendable, CustomStringConvertible {
    public var serverId: String
    public var serverName: String
    /// SHA-256 of the computer's leaf certificate, lowercase hex (the pin, §2).
    public var fingerprintHex: String
    /// The device token issued at pairing (secret).
    public var token: Base64Data
    /// Where the computer was last reached.
    public var lastHost: String
    public var lastPort: Int
    /// Where the computer says it can be reached (SERVER_HELLO, §1.1); empty for old servers.
    public var addresses: [ServerAddress]
    /// The listening port from SERVER_HELLO; 0 when it never said.
    public var serverPort: Int
    /// Raw SERVER_HELLO `platform`; nil for pairings made before it existed (Macs).
    public var platform: String?

    public init(serverId: String, serverName: String, fingerprint: Data, token: Data,
                lastHost: String, lastPort: Int, addresses: [ServerAddress] = [], serverPort: Int = 0,
                platform: String? = nil) {
        self.serverId = serverId
        self.serverName = serverName
        self.fingerprintHex = fingerprint.hexString
        self.token = Base64Data(token)
        self.lastHost = lastHost
        self.lastPort = lastPort
        self.addresses = addresses
        self.serverPort = serverPort
        self.platform = platform
    }

    public var fingerprint: Data? { Data(hexString: fingerprintHex) }

    /// The computer's OS; nil for one this version does not know (a generic computer).
    public var hostPlatform: HostPlatform? { HostPlatform(rawValue: platform ?? HostPlatform.macos.rawValue) }

    /// Reachable over Tailscale when away from home.
    public var hasTailscale: Bool {
        addresses.contains { $0.addressKind == .tailscale || $0.addressKind == .tailscaleDNS }
    }

    /// Never prints the token.
    public var description: String {
        "PairedComputer(serverId: \(serverId), serverName: \(serverName), fingerprint: \(fingerprintHex), "
            + "lastHost: \(lastHost), lastPort: \(lastPort), addresses: \(addresses.map(\.host)), serverPort: \(serverPort))"
    }

    private enum CodingKeys: String, CodingKey {
        case serverId, serverName, fingerprintHex, token, lastHost, lastPort, addresses, serverPort, platform
    }

    /// Tolerant of fields added later: only the identity and the secrets are required.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        serverId = try c.decode(String.self, forKey: .serverId)
        serverName = try c.decodeIfPresent(String.self, forKey: .serverName) ?? ""
        fingerprintHex = try c.decode(String.self, forKey: .fingerprintHex)
        token = try c.decode(Base64Data.self, forKey: .token)
        lastHost = try c.decodeIfPresent(String.self, forKey: .lastHost) ?? ""
        lastPort = try c.decodeIfPresent(Int.self, forKey: .lastPort) ?? Int(SidecastProtocol.defaultPort)
        addresses = (try? c.decodeIfPresent([ServerAddress].self, forKey: .addresses)) ?? []
        serverPort = try c.decodeIfPresent(Int.self, forKey: .serverPort) ?? 0
        platform = try? c.decodeIfPresent(String.self, forKey: .platform)
    }
}

/// Persistence of pairings, keyed by serverId. Implementations must be thread-safe.
public protocol PairingRepository: AnyObject {
    func pairing(serverId: String) -> PairedComputer?
    func all() -> [PairedComputer]
    func save(_ pairing: PairedComputer)
    func remove(serverId: String)
}

extension PairingRepository {
    /// Remembers where this computer was last reached and, when SERVER_HELLO said,
    /// its addresses, port and platform (nil = keep what is stored). No-op when not paired.
    public func updateEndpoint(serverId: String, serverName: String, host: String, port: Int,
                               addresses: [ServerAddress]? = nil, serverPort: Int? = nil, platform: String? = nil) {
        guard let current = pairing(serverId: serverId) else { return }
        var updated = current
        if !serverName.trimmingCharacters(in: .whitespaces).isEmpty { updated.serverName = serverName }
        updated.lastHost = host
        updated.lastPort = port
        if let addresses { updated.addresses = addresses }
        if let serverPort { updated.serverPort = serverPort }
        if let platform { updated.platform = platform }
        if updated != current { save(updated) }
    }
}

/// Pairings kept in memory (tests, the probe).
public final class InMemoryPairingRepository: PairingRepository, @unchecked Sendable {
    private let lock = NSLock()
    private var pairings: [String: PairedComputer] = [:]

    public init(_ initial: [PairedComputer] = []) {
        for p in initial { pairings[p.serverId] = p }
    }

    public func pairing(serverId: String) -> PairedComputer? { lock.withLock { pairings[serverId] } }

    public func all() -> [PairedComputer] {
        lock.withLock { pairings.values.sorted { $0.serverId < $1.serverId } }
    }

    public func save(_ pairing: PairedComputer) { lock.withLock { pairings[pairing.serverId] = pairing } }

    public func remove(serverId: String) { _ = lock.withLock { pairings.removeValue(forKey: serverId) } }
}
