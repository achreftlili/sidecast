import Foundation

/// A computer advertising `_sidecast._tcp` on the local network.
public struct DiscoveredComputer: Equatable, Sendable {
    /// Bonjour instance name: the computer's name.
    public var serviceName: String
    /// TXT `id`; nil if the computer did not publish one.
    public var serverId: String?
    /// TXT `v`.
    public var protocolVersion: String?
    /// Raw TXT `os`; nil for older hosts (Macs).
    public var platform: String?

    public init(serviceName: String, serverId: String?, protocolVersion: String?, platform: String? = nil) {
        self.serviceName = serviceName
        self.serverId = serverId
        self.protocolVersion = protocolVersion
        self.platform = platform
    }
}

/// One row of the computer list: discovered and/or paired, merged by serverId.
public struct ComputerListItem: Equatable, Sendable {
    public var key: String
    public var serverId: String?
    public var name: String
    /// Bonjour instance name while the computer is discovered.
    public var serviceName: String?
    /// Last known address (paired computers), nil when only discovered.
    public var host: String?
    public var port: Int
    public var online: Bool
    public var paired: Bool
    public var protocolVersion: String?
    /// Not on this network, but paired with a Tailscale address: reachable remotely (§1.1).
    public var remote: Bool
    /// The computer's OS; nil for one this version does not know (a generic computer).
    public var platform: HostPlatform?

    /// Same protocol version (or unknown).
    public var compatible: Bool {
        protocolVersion == nil || protocolVersion == String(SidecastProtocol.version)
    }
}

public enum ComputerList {
    /// Online computers first, then alphabetical. A computer advertised twice keeps its first entry.
    public static func merge(discovered: [DiscoveredComputer], paired: [PairedComputer]) -> [ComputerListItem] {
        let pairedById = Dictionary(paired.map { ($0.serverId, $0) }, uniquingKeysWith: { a, _ in a })
        var items: [String: ComputerListItem] = [:]
        var order: [String] = []
        for computer in discovered {
            let key = computer.serverId ?? "bonjour:\(computer.serviceName)"
            guard items[key] == nil else { continue }
            let pairing = computer.serverId.flatMap { pairedById[$0] }
            items[key] = ComputerListItem(
                key: key, serverId: computer.serverId, name: computer.serviceName,
                serviceName: computer.serviceName,
                host: pairing.flatMap { $0.lastHost.isEmpty ? nil : $0.lastHost },
                port: pairing.map(\.lastPort) ?? Int(SidecastProtocol.defaultPort),
                online: true, paired: pairing != nil, protocolVersion: computer.protocolVersion,
                remote: false,
                // TXT os is what the computer says now; absent = an older Mac host (§1).
                platform: HostPlatform(rawValue: computer.platform ?? HostPlatform.macos.rawValue))
            order.append(key)
        }
        for pairing in paired where items[pairing.serverId] == nil {
            let host = pairing.lastHost.isEmpty ? pairing.addresses.first?.host : pairing.lastHost
            items[pairing.serverId] = ComputerListItem(
                key: pairing.serverId, serverId: pairing.serverId,
                name: pairing.serverName.isEmpty ? pairing.lastHost : pairing.serverName,
                serviceName: nil, host: host,
                port: (1...65535).contains(pairing.lastPort) ? pairing.lastPort : Int(SidecastProtocol.defaultPort),
                online: false, paired: true, protocolVersion: nil, remote: pairing.hasTailscale,
                platform: pairing.hostPlatform)
            order.append(pairing.serverId)
        }
        return order.compactMap { items[$0] }.enumerated().sorted { a, b in
            if a.element.online != b.element.online { return a.element.online }
            let byName = a.element.name.localizedCaseInsensitiveCompare(b.element.name)
            return byName == .orderedSame ? a.offset < b.offset : byName == .orderedAscending
        }.map(\.element)
    }
}

/// Parses the "Add by IP address" fields. Pure.
public enum HostInput {
    public struct Parsed: Equatable, Sendable {
        public var host: String
        public var port: Int
    }

    /// Accepts `192.168.1.20`, `studio.local`, `192.168.1.20:47801`,
    /// `[fe80::1%en0]:47800` or a bare IPv6 literal. A port inside `hostText`
    /// wins over `portText`; a blank port means the default. Nil for an
    /// empty/invalid host or an out-of-range port.
    public static func parse(host hostText: String, port portText: String) -> Parsed? {
        var host = hostText.trimmingCharacters(in: .whitespacesAndNewlines)
        let portField = portText.trimmingCharacters(in: .whitespacesAndNewlines)
        var port: Int?
        if !portField.isEmpty {
            guard let p = Int(portField) else { return nil }
            port = p
        }
        if host.hasPrefix("[") {
            guard let close = host.firstIndex(of: "]") else { return nil }
            let rest = host[host.index(after: close)...]
            if rest.hasPrefix(":") {
                guard let p = Int(rest.dropFirst()) else { return nil }
                port = p
            } else if !rest.isEmpty {
                return nil
            }
            host = String(host[host.index(after: host.startIndex)..<close])
        } else if host.filter({ $0 == ":" }).count == 1, let colon = host.firstIndex(of: ":") {
            guard let p = Int(host[host.index(after: colon)...]) else { return nil }
            port = p
            host = String(host[..<colon])
        }
        guard !host.isEmpty, !host.contains(where: { $0.isWhitespace || $0 == "/" }) else { return nil }
        let p = port ?? Int(SidecastProtocol.defaultPort)
        guard (1...65535).contains(p) else { return nil }
        return Parsed(host: host, port: p)
    }
}

/// HELLO `network` / `limits` (PROTOCOL §5) for the data saver. Pure.
public enum DataSaver {
    /// `hello` for a connection made now: `network` always tells whether the
    /// active network is `metered` (cellular, or Low Data Mode); `limits` ask
    /// for the data-saver caps only on a metered network with the setting `enabled`.
    public static func apply(to hello: Hello, metered: Bool, enabled: Bool) -> Hello {
        var h = hello
        h.network = (metered ? ClientNetwork.metered : ClientNetwork.unmetered).rawValue
        h.limits = metered && enabled ? StreamLimits.dataSaver : nil
        return h
    }
}
