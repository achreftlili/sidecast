import Foundation
import Network
import Security
import SidecastKit

/// Server → UI notifications, delivered on the main queue.
enum ServerEvent {
    case listening(port: UInt16)
    case listenerFailed(String)
    case pairingStarted(session: UUID, deviceName: String, code: PairingCode, expiresAt: Date)
    case pairingEnded(session: UUID, paired: Bool)
    case streamingStarted(deviceName: String, plan: StreamPlan, dataSaver: Bool)
    case streamingStopped
    case inputModeChanged(InputMode)
    case stats(StreamStats)
    case pairedDevicesChanged
}

/// TLS listener + Bonjour advertisement + session management. Only one
/// phone streams at a time; only an *authenticated* newcomer replaces it.
final class SidecastServer: SessionHost {
    struct Configuration {
        var port: UInt16
        var advertiseBonjour: Bool
        var serverId: String
        var serverName: String
        /// Test mode only: every pairing uses this code.
        var fixedPairingCode: PairingCode?
        /// SERVER_HELLO `addresses` (called on the server queue for every HELLO).
        var addresses: () -> [ServerAddress] = { [] }
    }

    static let maxSessions = 16

    let queue = DispatchQueue(label: "com.achreftlili.sidecast.server", qos: .userInitiated)
    let identity: TLSIdentity
    let pairedDevices: PairedDeviceStore
    var onEvent: ((ServerEvent) -> Void)?

    private var configuration: Configuration
    private var settings: StreamSettings
    private let makeBackend: () -> StreamBackend
    private let micBridge: MicBridge?
    /// Returns a BYE reason if streaming cannot start (e.g. Screen Recording missing).
    private let streamingBlocker: () -> ByeReason?
    private var listener: NWListener?
    private var sessions: [UUID: ClientSession] = [:]
    private weak var activeSession: ClientSession?
    private weak var pairingSession: ClientSession?

    init(configuration: Configuration, identity: TLSIdentity, pairedDevices: PairedDeviceStore,
         settings: StreamSettings, makeBackend: @escaping () -> StreamBackend,
         micBridge: MicBridge? = nil,
         streamingBlocker: @escaping () -> ByeReason? = { nil }) {
        self.configuration = configuration
        self.identity = identity
        self.pairedDevices = pairedDevices
        self.settings = settings
        self.makeBackend = makeBackend
        self.micBridge = micBridge
        self.streamingBlocker = streamingBlocker
    }

    // MARK: - Control (any thread)

    func start() {
        queue.async { self.startListener() }
    }

    /// Changes the listening port (existing connections are kept).
    func setPort(_ port: UInt16) {
        queue.async {
            guard port != self.configuration.port else { return }
            self.configuration.port = port
            self.listener?.cancel()
            self.listener = nil
            self.startListener()
        }
    }

    func update(settings: StreamSettings) {
        queue.async {
            self.settings = settings
            self.activeSession?.streamer?.update(settings: settings)
        }
    }

    /// Forgets a paired phone and disconnects it if it is connected.
    func forgetDevice(id: String) {
        queue.async {
            try? self.pairedDevices.remove(deviceId: id)
            for session in self.sessions.values where session.deviceId == id && session.phase == .authenticated {
                session.bye(.unknownDevice)
            }
            self.emit(.pairedDevicesChanged)
        }
    }

    /// Sends BYE `server_shutdown` to everyone and stops listening.
    func shutdown(completion: (() -> Void)? = nil) {
        queue.async {
            self.listener?.cancel()
            self.listener = nil
            for session in self.sessions.values { session.bye(.serverShutdown) }
            // Give the BYEs a moment to leave.
            self.queue.asyncAfter(deadline: .now() + 0.3) { completion?() }
        }
    }

    // MARK: - Listener

    private func startListener() {
        let tls = NWProtocolTLS.Options()
        let sec = tls.securityProtocolOptions
        if let localIdentity = sec_identity_create(identity.identity) {
            sec_protocol_options_set_local_identity(sec, localIdentity)
        }
        sec_protocol_options_set_min_tls_protocol_version(sec, .TLSv12)
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        let params = NWParameters(tls: tls, tcp: tcp)
        params.allowLocalEndpointReuse = true

        let port = configuration.port
        do {
            let listener = try NWListener(using: params, on: NWEndpoint.Port(rawValue: port)!)
            if configuration.advertiseBonjour {
                let txt = NWTXTRecord(["v": String(SidecastProtocol.version), "id": configuration.serverId])
                listener.service = NWListener.Service(name: configuration.serverName,
                                                      type: SidecastProtocol.bonjourServiceType,
                                                      domain: nil, txtRecord: txt)
            }
            listener.newConnectionHandler = { [weak self] connection in self?.accept(connection) }
            listener.stateUpdateHandler = { [weak self, weak listener] state in
                guard let self, let listener, listener === self.listener else { return }
                switch state {
                case .ready:
                    Log.info("server: listening on port \(port) (TLS\(self.configuration.advertiseBonjour ? ", Bonjour" : ""))")
                    self.emit(.listening(port: port))
                case .failed(let error):
                    Log.error("server: listener failed: \(error)")
                    self.emit(.listenerFailed("\(error)"))
                    listener.cancel()
                    self.listener = nil
                default:
                    break
                }
            }
            self.listener = listener
            listener.start(queue: queue)
        } catch {
            Log.error("server: cannot listen on port \(port): \(error)")
            emit(.listenerFailed("\(error)"))
        }
    }

    private func accept(_ connection: NWConnection) {
        guard sessions.count < Self.maxSessions else {
            Log.info("server: too many connections, rejecting \(connection.endpoint)")
            connection.cancel()
            return
        }
        let session = ClientSession(connection: connection, host: self, queue: queue)
        sessions[session.id] = session
        Log.info("server: connection from \(connection.endpoint)")
        session.start()
    }

    private func emit(_ event: ServerEvent) {
        guard let onEvent else { return }
        DispatchQueue.main.async { onEvent(event) }
    }

    // MARK: - SessionHost

    var serverHello: ServerHello {
        let addresses = NetworkAddresses.ordered(configuration.addresses())
        return ServerHello(serverId: configuration.serverId, serverName: configuration.serverName,
                           port: Int(configuration.port), addresses: addresses.isEmpty ? nil : addresses)
    }

    var certificateFingerprint: Data { identity.fingerprint }

    func nextPairingCode() -> PairingCode {
        configuration.fixedPairingCode ?? PairingCode.generate()
    }

    func sessionWillStartPairing(_ session: ClientSession, code: PairingCode, expiresIn: TimeInterval) {
        // One pairing window at a time: a newer request supersedes the older one.
        if let other = pairingSession, other !== session { other.expirePairing() }
        pairingSession = session
        emit(.pairingStarted(session: session.id, deviceName: session.deviceName, code: code,
                             expiresAt: Date().addingTimeInterval(expiresIn)))
    }

    func sessionDidEndPairing(_ session: ClientSession, paired: Bool) {
        if pairingSession === session { pairingSession = nil }
        emit(.pairingEnded(session: session.id, paired: paired))
        if paired { emit(.pairedDevicesChanged) }
    }

    func sessionDidAuthenticate(_ session: ClientSession) {
        if let reason = streamingBlocker() {
            return session.bye(reason)
        }
        guard let hello = session.hello else { return session.bye(.protocolMismatch) }
        let plan: StreamPlan
        do {
            plan = try StreamPlanner.plan(screen: hello.screen, codecs: hello.codecs, settings: settings,
                                          limits: hello.limits)
        } catch {
            Log.error("server: no usable stream for \(hello.deviceName): \(error)")
            return session.bye(.protocolMismatch)
        }
        if let previous = activeSession, previous !== session {
            Log.info("server: \(hello.deviceName) takes over from \(previous.deviceName)")
            endMic(for: previous)
            previous.bye(.replaced)
        }
        activeSession = session
        let client = ClientInfo(deviceId: hello.deviceId, deviceName: hello.deviceName,
                                screen: hello.screen, codecs: hello.codecs, network: hello.network, limits: hello.limits)
        let streamer = StreamController(client: client, plan: plan, settings: settings, backend: makeBackend(),
                                        queue: queue) { [weak session] data in session?.channel.send(data) }
        streamer.onStats = { [weak self] stats in self?.emit(.stats(stats)) }
        streamer.onInputMode = { [weak self] mode in self?.emit(.inputModeChanged(mode)) }
        streamer.onConfigured = { [weak self, weak session] plan in
            self?.emit(.streamingStarted(deviceName: hello.deviceName, plan: plan, dataSaver: client.dataSaver))
            if let self, let session { self.startMic(for: session) }
        }
        session.startStreaming(streamer)
    }

    func sessionDidClose(_ session: ClientSession) {
        endMic(for: session)
        sessions.removeValue(forKey: session.id)
        if pairingSession === session { pairingSession = nil }
        if activeSession === session {
            activeSession = nil
            emit(.streamingStopped)
        }
    }

    func session(_ session: ClientSession, didReceiveAudio packet: AudioPacket) {
        guard session.micStarted, activeSession === session else { return }
        micBridge?.handleAudio(packet, from: session.id)
    }

    func session(_ session: ClientSession, didReceiveMicState state: MicState) {
        guard session.micStarted, activeSession === session else { return }
        micBridge?.handleMicState(state, from: session.id)
    }

    // MARK: - Microphone (PROTOCOL §7.2)

    /// Announces a session to the mic bridge once, when its stream is first configured.
    private func startMic(for session: ClientSession) {
        guard let micBridge, !session.micStarted, activeSession === session, let hello = session.hello else { return }
        session.micStarted = true
        micBridge.sessionStarted(id: session.id, deviceName: hello.deviceName, supportsMic: hello.supportsMic) {
            [weak self, weak session] active in
            self?.queue.async { session?.sendMicControl(active) }
        }
    }

    private func endMic(for session: ClientSession) {
        guard session.micStarted else { return }
        session.micStarted = false
        micBridge?.sessionEnded(id: session.id)
    }
}
