import Foundation

/// One address to try for a computer, with the route it implies and its connect timeout.
public struct ConnectCandidate: Equatable, Sendable, CustomStringConvertible {
    /// Host name or numeric address; for a Bonjour service, its instance name.
    public var host: String
    public var port: Int
    /// `host` names a `_sidecast._tcp` Bonjour service that the system resolves.
    public var isBonjourService: Bool
    public var route: Route
    public var connectTimeout: TimeInterval

    public init(host: String, port: Int, route: Route, connectTimeout: TimeInterval, isBonjourService: Bool = false) {
        self.host = host
        self.port = port
        self.route = route
        self.connectTimeout = connectTimeout
        self.isBonjourService = isBonjourService
    }

    public var description: String {
        isBonjourService ? "bonjour:\(host) (\(route))" : "\(host):\(port) (\(route), \(connectTimeout) s)"
    }
}

/// Which addresses to try, in order, to reach a computer (PROTOCOL §1.1). Pure.
public enum ConnectPlanner {
    public static let lanConnectTimeout: TimeInterval = 2.5
    public static let remoteConnectTimeout: TimeInterval = 6

    private static let storedKindOrder: [AddressKind] = [.lan, .tailscale, .tailscaleDNS]

    /// - Parameters:
    ///   - host: the address the user picked (list row or "Add by IP address")
    ///   - port: its port
    ///   - bonjourService: the computer's Bonjour instance name when it is discovered right now
    ///   - paired: the stored pairing; nil when not paired (or not known yet)
    /// - Returns: the Bonjour service alone when discovered; the picked address alone for an
    ///   unpaired computer; otherwise lastHost, the picked address, then the stored lan,
    ///   tailscale and tailscale-dns addresses — duplicates removed, unknown kinds ignored.
    public static func plan(host: String, port: Int, bonjourService: String?,
                            paired: PairedComputer?) -> [ConnectCandidate] {
        if let service = bonjourService {
            return [ConnectCandidate(host: service, port: port, route: .lan, connectTimeout: lanConnectTimeout,
                                     isBonjourService: true)]
        }
        guard let paired else { return [candidate(host, port, kind: nil)] }
        let lastPort = isPort(paired.lastPort) ? paired.lastPort : port
        let addressPort = isPort(paired.serverPort) ? paired.serverPort : lastPort
        var ordered: [ConnectCandidate] = []
        if !paired.lastHost.trimmingCharacters(in: .whitespaces).isEmpty {
            ordered.append(candidate(paired.lastHost, lastPort, kind: nil))
        }
        if !host.trimmingCharacters(in: .whitespaces).isEmpty { ordered.append(candidate(host, port, kind: nil)) }
        for kind in storedKindOrder {
            for address in paired.addresses where address.addressKind == kind {
                ordered.append(candidate(address.host, addressPort, kind: kind))
            }
        }
        var seen = Set<String>()
        return ordered.filter { seen.insert("\($0.host.lowercased()):\($0.port)").inserted }
    }

    /// Route of `host`: numeric addresses by class, names by their advertised kind or suffix.
    public static func route(host: String, kind: AddressKind?) -> Route {
        if let literalKind = AddressClass.kind(ofHost: host) { return AddressClass.route(of: literalKind) }
        let lower = host.lowercased()
        switch kind {
        case .lan: return .lan
        case .tailscale, .tailscaleDNS: return .tailscale
        case nil: break
        }
        if lower.hasSuffix(".local") || lower.hasSuffix(".local.") { return .lan }
        if lower.hasSuffix(".ts.net") || lower.hasSuffix(".ts.net.") { return .tailscale }
        return .internet
    }

    private static func candidate(_ host: String, _ port: Int, kind: AddressKind?) -> ConnectCandidate {
        let h = host.trimmingCharacters(in: .whitespaces)
        let route = route(host: h, kind: kind)
        return ConnectCandidate(host: h, port: port, route: route,
                                connectTimeout: route == .lan ? lanConnectTimeout : remoteConnectTimeout)
    }

    private static func isPort(_ p: Int) -> Bool { (1...65535).contains(p) }
}

/// Why connecting failed.
public enum ConnectFailure: Error, Equatable, Sendable {
    /// The computer presented a certificate other than the pinned one (§2).
    case identityChanged
    case failed(String)
}

/// A pending connection attempt.
public protocol ConnectAttempt: AnyObject {
    func cancel()
}

/// Tries candidates in order; the first that connects wins. Each attempt does
/// its own certificate pinning, so the token never goes to an unpinned server
/// whichever address answers. A pin mismatch at one address does not stop the
/// others, but is what gets reported if none connects.
///
/// Not thread-safe: use it, and complete attempts, on one serial queue.
public final class FallbackConnector<Connection> {
    public typealias Attempt = (ConnectCandidate, @escaping (Result<Connection, ConnectFailure>) -> Void) -> ConnectAttempt

    private let candidates: [ConnectCandidate]
    private let attempt: Attempt
    private let onAttempt: (ConnectCandidate) -> Void
    private var current: ConnectAttempt?
    private var index = 0
    private var identityChanged = false
    private var lastError: String?
    private var completion: ((Result<(Connection, ConnectCandidate), ConnectFailure>) -> Void)?
    private var finished = false

    public init(candidates: [ConnectCandidate], onAttempt: @escaping (ConnectCandidate) -> Void = { _ in },
                attempt: @escaping Attempt) {
        self.candidates = candidates
        self.attempt = attempt
        self.onAttempt = onAttempt
    }

    /// Starts trying; `completion` runs exactly once unless `cancel()` comes first.
    public func start(_ completion: @escaping (Result<(Connection, ConnectCandidate), ConnectFailure>) -> Void) {
        precondition(self.completion == nil, "FallbackConnector is single-use")
        self.completion = completion
        next()
    }

    /// Stops the current attempt; no completion is delivered afterwards.
    public func cancel() {
        guard !finished else { return }
        finished = true
        completion = nil
        current?.cancel()
        current = nil
    }

    private func next() {
        guard !finished else { return }
        guard index < candidates.count else {
            finish(.failure(identityChanged ? .identityChanged : .failed(lastError ?? "no address to connect to")))
            return
        }
        let candidate = candidates[index]
        index += 1
        onAttempt(candidate)
        var done = false
        current = attempt(candidate) { [weak self] result in
            guard let self, !done, !self.finished else { return }
            done = true
            self.current = nil
            switch result {
            case .success(let connection):
                self.finish(.success((connection, candidate)))
            case .failure(.identityChanged):
                self.identityChanged = true
                self.next()
            case .failure(.failed(let why)):
                self.lastError = why
                self.next()
            }
        }
        if done { current = nil }   // the attempt completed synchronously
    }

    private func finish(_ result: Result<(Connection, ConnectCandidate), ConnectFailure>) {
        guard !finished else { return }
        finished = true
        let handler = completion
        completion = nil
        handler?(result)
    }
}
