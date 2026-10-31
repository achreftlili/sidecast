import Foundation

/// Byte transport of an established connection: a `MessageChannel` in the apps,
/// a fake in tests. Used on the client's queue only.
public protocol ClientTransport: AnyObject {
    /// A complete frame arrived.
    var onFrame: ((Frame) -> Void)? { get set }
    /// The connection ended; nil error = orderly close by the peer.
    var onClose: ((Error?) -> Void)? { get set }
    /// Bytes handed to `send` whose transmission has not completed yet.
    var pendingSendBytes: Int { get }
    func send(_ data: Data)
    /// Immediate close.
    func abort()
}

extension MessageChannel: ClientTransport {
    public func send(_ data: Data) { send(data, completion: nil) }
    public func abort() { cancel() }
}

/// An established, TLS-protected connection and the identity it proved.
public struct ClientConnection {
    public var transport: ClientTransport
    /// SHA-256 of the server's leaf certificate DER (PROTOCOL §2).
    public var peerFingerprint: Data
    /// The address that answered (remembered as lastHost); nil when unknown.
    public var reached: StreamClient.Endpoint?
    /// How it was reached; nil when unknown.
    public var route: Route?

    public init(transport: ClientTransport, peerFingerprint: Data, reached: StreamClient.Endpoint? = nil,
                route: Route? = nil) {
        self.transport = transport
        self.peerFingerprint = peerFingerprint
        self.reached = reached
        self.route = route
    }
}

/// Opens one connection, pinning the certificate when paired. Completes on the client's queue.
public protocol ClientConnector: AnyObject {
    func connect(_ completion: @escaping (Result<ClientConnection, ConnectFailure>) -> Void)
    func cancel()
}

/// Events of one `StreamClient`, delivered on its queue, in order.
public protocol StreamClientDelegate: AnyObject {
    func streamClient(_ client: StreamClient, didEnter phase: StreamClient.Phase)
    func streamClient(_ client: StreamClient, needsPairingCode prompt: StreamClient.PairingPrompt)
    func streamClient(_ client: StreamClient, didAuthenticate server: StreamClient.ServerIdentity)
    func streamClient(_ client: StreamClient, didReceive config: StreamConfig)
    /// Already acknowledged (FRAME_ACK is sent on receipt, before decoding, §6).
    func streamClient(_ client: StreamClient, didReceive frame: VideoFrame)
    func streamClient(_ client: StreamClient, didReceive cursor: CursorPosition)
    func streamClient(_ client: StreamClient, didReceive micControl: MicControl)
    func streamClient(_ client: StreamClient, didConnectVia route: Route)
    /// Exactly once per client.
    func streamClient(_ client: StreamClient, didEndWith reason: StreamClient.EndReason)
}

extension StreamClientDelegate {
    public func streamClient(_ client: StreamClient, didEnter phase: StreamClient.Phase) {}
    public func streamClient(_ client: StreamClient, needsPairingCode prompt: StreamClient.PairingPrompt) {}
    public func streamClient(_ client: StreamClient, didAuthenticate server: StreamClient.ServerIdentity) {}
    public func streamClient(_ client: StreamClient, didReceive config: StreamConfig) {}
    public func streamClient(_ client: StreamClient, didReceive frame: VideoFrame) {}
    public func streamClient(_ client: StreamClient, didReceive cursor: CursorPosition) {}
    public func streamClient(_ client: StreamClient, didReceive micControl: MicControl) {}
    public func streamClient(_ client: StreamClient, didConnectVia route: Route) {}
}

/// Receive-side counters shared across reconnects; read by the stats overlay from any thread.
public final class StreamCounters: @unchecked Sendable {
    private let lock = NSLock()
    private var _videoFrames = 0
    private var _videoBytes = 0
    private var _rttMicros: Int64 = -1
    private var _audioDropped = 0

    public init() {}

    public var videoFrames: Int { lock.withLock { _videoFrames } }
    public var videoBytes: Int { lock.withLock { _videoBytes } }
    /// Latest PING round trip, or -1 before the first PONG.
    public var rttMicros: Int64 { lock.withLock { _rttMicros } }
    /// AUDIO packets dropped because the network fell behind.
    public var audioDropped: Int { lock.withLock { _audioDropped } }

    func recordVideo(bytes: Int) { lock.withLock { _videoFrames += 1; _videoBytes += bytes } }
    func recordRTT(_ micros: Int64) { lock.withLock { _rttMicros = micros } }
    func recordAudioDrop() { lock.withLock { _audioDropped += 1 } }
}

/// One connection attempt to a computer, from connect to close (PROTOCOL §5–§7),
/// as a state machine without I/O of its own: the connector and transport do the
/// networking, the scheduler and clock the timing. Everything — including every
/// method here — runs on one serial queue, which the connector, transport and
/// scheduler share.
///
/// Exactly one `didEndWith` is delivered per client. A client is never reused:
/// reconnecting means creating a new one (see `DisplaySession`).
public final class StreamClient {
    public struct Endpoint: Equatable, Sendable {
        public var host: String
        public var port: Int

        public init(host: String, port: Int) {
            self.host = host
            self.port = port
        }
    }

    public struct Timing: Sendable {
        public var pingInterval: TimeInterval = 1
        /// §6: no PONG and no VIDEO_FRAME for this long → connection lost.
        public var livenessTimeout: TimeInterval = 3
        /// Max wait for a reply the computer owes us (SERVER_HELLO, PAIR_CHALLENGE, AUTH_RESULT).
        public var responseTimeout: TimeInterval = 10
        public var watchdogTick: TimeInterval = 0.1

        public init() {}
    }

    public enum Phase: Equatable, Sendable {
        case connecting
        case handshaking
        case pairing
        case verifyingCode
        case authenticating
        case streaming
    }

    /// The computer is showing a code; ask the user for it. `expiresAt` is on the client's clock.
    public struct PairingPrompt: Equatable, Sendable {
        public var attemptsLeft: Int
        public var expiresAt: TimeInterval
        public var wrongCode: Bool

        public init(attemptsLeft: Int, expiresAt: TimeInterval, wrongCode: Bool) {
            self.attemptsLeft = attemptsLeft
            self.expiresAt = expiresAt
            self.wrongCode = wrongCode
        }
    }

    public struct ServerIdentity: Equatable, Sendable {
        public var serverId: String
        public var serverName: String
        /// Raw SERVER_HELLO `platform` ("macos" when absent); see `hostPlatform`.
        public var platform: String
        public var newlyPaired: Bool

        /// nil for an OS this version does not know: a generic computer.
        public var hostPlatform: HostPlatform? { HostPlatform(rawValue: platform) }
    }

    public enum EndReason: Equatable, Sendable {
        /// `close()` was called.
        case closedByUser
        /// Could not connect / TLS failed / the computer stopped answering before streaming.
        case connectFailed(String)
        /// Liveness timeout or connection error after authentication.
        case connectionLost
        /// Pinned certificate mismatch; `serverId` is known when detected after SERVER_HELLO.
        case identityChanged(serverId: String?)
        /// BYE, or a terminal AUTH_RESULT failure, with its §4 reason string.
        case serverEnded(String)
        case protocolError(String)
    }

    private enum State {
        case idle, connecting, awaitServerHello, awaitChallenge, awaitCode, awaitPairResult, awaitAuthResult, streaming, ended
    }

    /// AUDIO is dropped rather than queued once this much is waiting to be sent
    /// (~100 ms of audio): a stalled network must not delay the microphone further.
    public static let audioBacklogLimitBytes = 10 * (SidecastProtocol.headerLength + 4 + 2 * AudioPacket.recommendedSamples)

    public weak var delegate: StreamClientDelegate?

    private let connector: ClientConnector
    private let hello: Hello
    private let pairings: PairingRepository
    private let endpoint: Endpoint
    private let scheduler: TaskScheduler
    private let clock: TimeSource
    private let random: RandomSource
    private let timing: Timing
    private let counters: StreamCounters
    private let inputMode: () -> InputMode
    private let log: (String) -> Void

    private var state = State.idle
    private var connection: ClientConnection?
    private var server: ServerHello?
    private var pairingExpiresAt: TimeInterval = 0
    private var responseDeadline = TimeInterval.infinity
    private var lastAlive: TimeInterval = 0
    private var lastPing = -TimeInterval.infinity
    private var watchdog: ScheduledTask?

    /// - Parameter inputMode: the mode (§7.3) to announce right after authentication; read once per connection.
    public init(connector: ClientConnector, hello: Hello, pairings: PairingRepository, endpoint: Endpoint,
                scheduler: TaskScheduler, clock: TimeSource = MonotonicTimeSource(),
                random: RandomSource = SystemRandom(), timing: Timing = Timing(),
                counters: StreamCounters = StreamCounters(),
                inputMode: @escaping () -> InputMode = { .touch }, log: @escaping (String) -> Void = { _ in }) {
        self.connector = connector
        self.hello = hello
        self.pairings = pairings
        self.endpoint = endpoint
        self.scheduler = scheduler
        self.clock = clock
        self.random = random
        self.timing = timing
        self.counters = counters
        self.inputMode = inputMode
        self.log = log
    }

    public var isStreaming: Bool { state == .streaming }
    public var isEnded: Bool { state == .ended }

    public func start() {
        precondition(state == .idle, "StreamClient is single-use")
        state = .connecting
        delegate?.streamClient(self, didEnter: .connecting)
        connector.connect { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let connection): self.connected(connection)
            case .failure(.identityChanged): self.end(.identityChanged(serverId: nil))
            case .failure(.failed(let why)): self.end(.connectFailed(why))
            }
        }
    }

    /// User-initiated close. Safe any time.
    public func close() { end(.closedByUser) }

    /// Sends PAIR_PROOF for `code`. False (nothing sent) when it is not 6 digits or no code is asked for.
    @discardableResult
    public func submitPairingCode(_ code: String) -> Bool {
        guard PairingCode(code) != nil, state == .awaitCode, let connection else { return false }
        state = .awaitPairResult
        let nonce = random.bytes(PairingProofCalculator.nonceLength)
        let proof = PairingProofCalculator.proof(code: code, certFingerprint: connection.peerFingerprint, nonce: nonce)
        armResponseDeadline()
        delegate?.streamClient(self, didEnter: .verifyingCode)
        send(.pairProof(PairProof(nonce: nonce, proof: proof)))
        return true
    }

    public func sendTouch(_ touch: TouchEvent) {
        if state == .streaming { send(.touch(touch)) }
    }

    /// Types `text` (§7.1), split into as many TEXT messages as its UTF-8 size needs.
    /// It must not contain control characters: send those as KEY. Dropped unless streaming.
    public func sendText(_ text: String) {
        guard state == .streaming else { return }
        for chunk in TextMessage.split(text) { send(.text(chunk)) }
    }

    public func sendKey(_ key: KeyEvent) {
        if state == .streaming { send(.key(key)) }
    }

    public func requestKeyframe() {
        if state == .streaming { send(.keyframeRequest) }
    }

    /// INPUT_MODE (§7.3) after the user toggled. Dropped unless streaming.
    public func sendInputMode(_ mode: InputMode) {
        if state == .streaming { send(.inputMode(InputModeMessage(mode))) }
    }

    /// MIC_STATE (§7.2). Dropped unless streaming.
    public func sendMicState(_ micState: MicState) {
        if state == .streaming { send(.micState(micState)) }
    }

    /// AUDIO (§7.2). Dropped unless streaming, or when the network is too far behind.
    public func sendAudio(_ packet: AudioPacket) {
        guard state == .streaming, let transport = connection?.transport else { return }
        guard transport.pendingSendBytes <= Self.audioBacklogLimitBytes else {
            counters.recordAudioDrop()
            return
        }
        transport.send(ClientMessage.audio(packet).encoded())
    }

    // MARK: - Connection

    private func connected(_ connection: ClientConnection) {
        guard state != .ended else {
            connection.transport.abort()   // close() raced with connect()
            return
        }
        self.connection = connection
        connection.transport.onFrame = { [weak self] frame in self?.handle(frame) }
        connection.transport.onClose = { [weak self] error in self?.transportClosed(error) }
        if let route = connection.route { delegate?.streamClient(self, didConnectVia: route) }
        state = .awaitServerHello
        armResponseDeadline()
        startWatchdog()
        delegate?.streamClient(self, didEnter: .handshaking)
        send(.hello(hello))
    }

    private func transportClosed(_ error: Error?) {
        guard state != .ended else { return }
        if state == .streaming { return end(.connectionLost) }
        end(.connectFailed(error.map { "\($0)" } ?? "the computer closed the connection"))
    }

    // MARK: - Messages

    private func handle(_ frame: Frame) {
        guard state != .ended else { return }
        let message: ServerMessage?
        do {
            message = try ServerMessage.decode(frame)
        } catch {
            return end(.protocolError("\(error)"))
        }
        guard let message else { return }   // unknown types are skipped (§3)
        switch message {
        case .serverHello(let hello): serverHello(hello)
        case .pairChallenge(let challenge): pairChallenge(challenge)
        case .authResult(let result): authResult(result)
        case .streamConfig(let config):
            guard requireStreaming("STREAM_CONFIG") else { return }
            log("stream config \(config.codec) \(config.width)x\(config.height)@\(config.fps)")
            delegate?.streamClient(self, didReceive: config)
        case .videoFrame(let video):
            guard requireStreaming("VIDEO_FRAME") else { return }
            send(.frameAck(seq: video.seq))   // ack on receipt, before decoding (§6)
            lastAlive = clock.now()
            counters.recordVideo(bytes: frame.payload.count + SidecastProtocol.headerLength)
            delegate?.streamClient(self, didReceive: video)
        case .pong(let t):
            lastAlive = clock.now()
            let rtt = Int64(nowMicros) - Int64(bitPattern: t)
            if (0...60_000_000).contains(rtt) { counters.recordRTT(rtt) }
        case .bye(let bye):
            end(.serverEnded(bye.reason))
        case .cursor(let position):
            if state == .streaming { delegate?.streamClient(self, didReceive: position) }
        case .micControl(let control):
            // Only meaningful for a streaming client; anything earlier is ignored (§7.2).
            if state == .streaming { delegate?.streamClient(self, didReceive: control) } else { log("ignoring early MIC_CONTROL") }
        }
    }

    private func serverHello(_ msg: ServerHello) {
        guard state == .awaitServerHello else { return end(.protocolError("unexpected SERVER_HELLO")) }
        guard msg.protocol == SidecastProtocol.version else {
            return end(.serverEnded(ByeReason.protocolMismatch.rawValue))
        }
        server = msg
        guard let connection else { return }
        if let paired = pairings.pairing(serverId: msg.serverId) {
            // Pin check by serverId as well: covers connections made by address, where the
            // TLS layer could not know which pin to apply. The token never goes to an impostor.
            guard let pin = paired.fingerprint, constantTimeEquals(pin, connection.peerFingerprint) else {
                return end(.identityChanged(serverId: msg.serverId))
            }
            state = .awaitAuthResult
            armResponseDeadline()
            delegate?.streamClient(self, didEnter: .authenticating)
            send(.auth(AuthRequest(deviceId: hello.deviceId, token: paired.token.data)))
        } else {
            state = .awaitChallenge
            armResponseDeadline()
            delegate?.streamClient(self, didEnter: .pairing)
            send(.pairStart)
        }
    }

    private func pairChallenge(_ msg: PairChallenge) {
        guard state == .awaitChallenge || state == .awaitCode else {
            return end(.protocolError("unexpected PAIR_CHALLENGE"))
        }
        pairingExpiresAt = clock.now() + TimeInterval(msg.expiresInSec)
        state = .awaitCode
        responseDeadline = .infinity   // the user is typing; the computer enforces expiry
        delegate?.streamClient(self, needsPairingCode: PairingPrompt(attemptsLeft: msg.attemptsLeft,
                                                                     expiresAt: pairingExpiresAt, wrongCode: false))
    }

    private func authResult(_ msg: AuthResult) {
        guard let hello = server, let connection else { return end(.protocolError("AUTH_RESULT before SERVER_HELLO")) }
        let reached = connection.reached ?? endpoint
        switch state {
        case .awaitAuthResult:
            guard msg.ok else {
                let reason = msg.reason ?? ByeReason.unknownDevice.rawValue
                // §5 AUTH: forget the stale pairing; the next attempt pairs again.
                if reason == ByeReason.unknownDevice.rawValue { pairings.remove(serverId: hello.serverId) }
                return end(.serverEnded(reason))
            }
            // Remember the address that answered and where the computer says it can be reached.
            pairings.updateEndpoint(serverId: hello.serverId, serverName: hello.serverName,
                                    host: reached.host, port: reached.port, addresses: hello.addresses,
                                    serverPort: hello.port, platform: hello.platform)
            becomeStreaming(ServerIdentity(serverId: hello.serverId, serverName: hello.serverName,
                                           platform: hello.platform, newlyPaired: false))
        case .awaitPairResult:
            if msg.ok {
                guard let token = msg.token?.data, !token.isEmpty else {
                    return end(.protocolError("AUTH_RESULT ok without a token after pairing"))
                }
                pairings.save(PairedComputer(serverId: hello.serverId, serverName: hello.serverName,
                                             fingerprint: connection.peerFingerprint, token: token,
                                             lastHost: reached.host, lastPort: reached.port,
                                             addresses: hello.addresses ?? [], serverPort: hello.port ?? 0,
                                             platform: hello.platform))
                becomeStreaming(ServerIdentity(serverId: hello.serverId, serverName: hello.serverName,
                                               platform: hello.platform, newlyPaired: true))
                return
            }
            let attemptsLeft = msg.attemptsLeft ?? 0
            if msg.reason == ByeReason.badCode.rawValue, attemptsLeft > 0 {
                state = .awaitCode
                responseDeadline = .infinity
                delegate?.streamClient(self, needsPairingCode: PairingPrompt(attemptsLeft: attemptsLeft,
                                                                             expiresAt: pairingExpiresAt, wrongCode: true))
            } else if msg.reason == ByeReason.badCode.rawValue {
                end(.serverEnded(ByeReason.tooManyAttempts.rawValue))
            } else {
                end(.serverEnded(msg.reason ?? ByeReason.badCode.rawValue))
            }
        case .awaitChallenge, .awaitCode:
            // The computer may end a pairing window before any PAIR_PROOF: it expired, or
            // another device started pairing. A failure here is terminal, not a protocol error.
            if msg.ok { return end(.protocolError("AUTH_RESULT ok before PAIR_PROOF")) }
            end(.serverEnded(msg.reason ?? ByeReason.pairingExpired.rawValue))
        default:
            end(.protocolError("unexpected AUTH_RESULT"))
        }
    }

    private func becomeStreaming(_ identity: ServerIdentity) {
        let now = clock.now()
        lastAlive = now
        responseDeadline = .infinity
        // Sent before anything else of this connection, so it precedes every TOUCH.
        send(.inputMode(InputModeMessage(inputMode())))
        state = .streaming
        log("authenticated with \(identity.serverName) (\(identity.serverId)), newlyPaired=\(identity.newlyPaired)")
        delegate?.streamClient(self, didAuthenticate: identity)
        guard state == .streaming else { return }   // the delegate may have closed us
        delegate?.streamClient(self, didEnter: .streaming)
        sendPing(now)
    }

    private func requireStreaming(_ what: String) -> Bool {
        guard state == .streaming else {
            end(.protocolError("\(what) before authentication"))
            return false
        }
        return true
    }

    // MARK: - Liveness

    private func startWatchdog() {
        watchdog = scheduler.schedule(after: timing.watchdogTick) { [weak self] in
            guard let self, self.state != .ended else { return }
            self.tick()
            if self.state != .ended { self.startWatchdog() }
        }
    }

    private func tick() {
        let now = clock.now()
        switch state {
        case .streaming:
            if now - lastAlive > timing.livenessTimeout {
                log("no PONG or VIDEO_FRAME for \(timing.livenessTimeout) s")
                end(.connectionLost)
            } else if now - lastPing >= timing.pingInterval - 1e-6 {   // ticks accumulate rounding
                sendPing(now)
            }
        case .awaitServerHello, .awaitChallenge, .awaitPairResult, .awaitAuthResult:
            if now > responseDeadline { end(.connectFailed("the computer did not answer")) }
        default:
            break
        }
    }

    private func sendPing(_ now: TimeInterval) {
        lastPing = now
        send(.ping(nowMicros))
    }

    /// The client's monotonic clock in microseconds (PING payload, opaque to the server).
    private var nowMicros: UInt64 { UInt64(max(0, clock.now() * 1_000_000)) }

    private func armResponseDeadline() {
        responseDeadline = clock.now() + timing.responseTimeout
    }

    // MARK: - Teardown

    private func send(_ message: ClientMessage) {
        guard state != .ended else { return }
        connection?.transport.send(message.encoded())
    }

    private func end(_ reason: EndReason) {
        guard state != .ended else { return }
        state = .ended
        watchdog?.cancel()
        watchdog = nil
        log("ended: \(reason)")
        connector.cancel()
        if let transport = connection?.transport {
            transport.onFrame = nil
            transport.onClose = nil
            transport.abort()
        }
        delegate?.streamClient(self, didEndWith: reason)
    }
}
