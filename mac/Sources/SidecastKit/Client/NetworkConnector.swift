import Foundation
import Network
import Security

/// Client-side TLS (PROTOCOL §2): TCP with TCP_NODELAY, TLS 1.2 minimum, no
/// hostname or chain validation (the certificate is self-signed); trust on
/// first pairing, then the pinned fingerprint.
public enum ClientTLS {
    /// Without a `pin` any certificate is accepted (its fingerprint is checked
    /// against the pairing before any secret is sent). With one, the leaf's
    /// SHA-256 must equal it, otherwise the handshake fails and `onPinMismatch`
    /// runs on `queue` first.
    public static func parameters(pin: Data?, queue: DispatchQueue,
                                  onPinMismatch: @escaping () -> Void = {}) -> NWParameters {
        let tls = NWProtocolTLS.Options()
        let sec = tls.securityProtocolOptions
        sec_protocol_options_set_min_tls_protocol_version(sec, .TLSv12)
        sec_protocol_options_set_verify_block(sec, { _, trust, complete in
            guard let pin else { return complete(true) }
            let matches = leafFingerprint(of: trust).map { constantTimeEquals($0, pin) } ?? false
            if !matches { onPinMismatch() }
            complete(matches)
        }, queue)
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        return NWParameters(tls: tls, tcp: tcp)
    }

    static func leafFingerprint(of trust: sec_trust_t) -> Data? {
        let secTrust = sec_trust_copy_ref(trust).takeRetainedValue()
        guard let chain = SecTrustCopyCertificateChain(secTrust) as? [SecCertificate], let leaf = chain.first else {
            return nil
        }
        return CertificateFingerprint.sha256(der: SecCertificateCopyData(leaf) as Data)
    }
}

/// Connects to the first reachable candidate (PROTOCOL §1.1) over pinned TLS
/// and hands back a running `MessageChannel`. Used on `queue`, which the
/// channel then keeps using.
public final class NetworkConnector: ClientConnector {
    private let candidates: [ConnectCandidate]
    private let pin: Data?
    private let queue: DispatchQueue
    private let onAttempt: (ConnectCandidate) -> Void
    private let log: (String) -> Void
    private var fallback: FallbackConnector<ClientConnection>?

    /// - Parameter pin: the pinned fingerprint of the computer this is expected to be, if paired.
    public init(candidates: [ConnectCandidate], pin: Data?, queue: DispatchQueue,
                onAttempt: @escaping (ConnectCandidate) -> Void = { _ in }, log: @escaping (String) -> Void = { _ in }) {
        self.candidates = candidates
        self.pin = pin
        self.queue = queue
        self.onAttempt = onAttempt
        self.log = log
    }

    public func connect(_ completion: @escaping (Result<ClientConnection, ConnectFailure>) -> Void) {
        let pin = pin, queue = queue, log = log
        let fallback = FallbackConnector<ClientConnection>(candidates: candidates, onAttempt: { [onAttempt] c in
            log("trying \(c)")
            onAttempt(c)
        }) { candidate, done in
            TLSAttempt(candidate: candidate, pin: pin, queue: queue, log: log, done: done)
        }
        self.fallback = fallback
        fallback.start { result in completion(result.map(\.0)) }
    }

    public func cancel() { fallback?.cancel() }
}

/// One TLS connection attempt with a connect timeout.
private final class TLSAttempt: ConnectAttempt {
    private let candidate: ConnectCandidate
    private var channel: MessageChannel?
    private var done: ((Result<ClientConnection, ConnectFailure>) -> Void)?
    private var timeout: DispatchWorkItem?
    private var pinMismatch = false

    init(candidate: ConnectCandidate, pin: Data?, queue: DispatchQueue, log: @escaping (String) -> Void,
         done: @escaping (Result<ClientConnection, ConnectFailure>) -> Void) {
        self.candidate = candidate
        self.done = done
        let endpoint: NWEndpoint
        if candidate.isBonjourService {
            endpoint = .service(name: candidate.host, type: SidecastProtocol.bonjourServiceType, domain: "local.",
                                interface: nil)
        } else if let port = NWEndpoint.Port(rawValue: UInt16(clamping: candidate.port)), candidate.port > 0 {
            endpoint = .hostPort(host: NWEndpoint.Host(candidate.host), port: port)
        } else {
            queue.async { [weak self] in self?.finish(.failure(.failed("invalid port \(candidate.port)"))) }
            return
        }
        let parameters = ClientTLS.parameters(pin: pin, queue: queue) { [weak self] in
            log("\(candidate.host): certificate does not match the pinned fingerprint")
            self?.pinMismatch = true
        }
        // Keep a home-network destination off cellular (an RFC 1918 address is
        // meaningless there); Tailscale and public destinations use whatever is active.
        if candidate.route == .lan, candidate.isBonjourService || AddressClass.isPrivateLAN(candidate.host) {
            parameters.prohibitedInterfaceTypes = [.cellular]
        }
        let channel = MessageChannel(connection: NWConnection(to: endpoint, using: parameters), queue: queue)
        self.channel = channel
        channel.onReady = { [weak self, weak channel] in
            guard let self, let channel else { return }
            guard let fingerprint = channel.peerLeafFingerprint() else {
                channel.cancel()
                return self.finish(.failure(.failed("no server certificate")))
            }
            channel.onClose = nil
            self.finish(.success(ClientConnection(transport: channel, peerFingerprint: fingerprint,
                                                  reached: self.reachedEndpoint(channel), route: candidate.route)))
        }
        channel.onClose = { [weak self] error in
            guard let self else { return }
            if self.pinMismatch { return self.finish(.failure(.identityChanged)) }
            self.finish(.failure(.failed(error.map { "\($0)" } ?? "connection closed")))
        }
        let timeout = DispatchWorkItem { [weak self] in
            guard let self, self.done != nil else { return }
            self.channel?.onClose = nil
            self.channel?.cancel()
            self.finish(.failure(.failed("timed out after \(candidate.connectTimeout) s")))
        }
        self.timeout = timeout
        queue.asyncAfter(deadline: .now() + candidate.connectTimeout, execute: timeout)
        channel.start()
    }

    func cancel() {
        timeout?.cancel()
        done = nil
        channel?.onClose = nil
        channel?.cancel()
        channel = nil
    }

    private func finish(_ result: Result<ClientConnection, ConnectFailure>) {
        timeout?.cancel()
        guard let done else { return }
        self.done = nil
        if case .failure = result { channel = nil }
        done(result)
    }

    /// The address that answered: the name or address tried, or what a Bonjour service resolved to.
    private func reachedEndpoint(_ channel: MessageChannel) -> StreamClient.Endpoint? {
        guard candidate.isBonjourService else { return StreamClient.Endpoint(host: candidate.host, port: candidate.port) }
        guard case .hostPort(let host, let port)? = channel.connection.currentPath?.remoteEndpoint else { return nil }
        let name: String
        switch host {
        case .name(let n, _): name = n
        case .ipv4(let a): name = "\(a)"
        case .ipv6(let a): name = "\(a)"
        @unknown default: return nil
        }
        return StreamClient.Endpoint(host: name, port: Int(port.rawValue))
    }
}
