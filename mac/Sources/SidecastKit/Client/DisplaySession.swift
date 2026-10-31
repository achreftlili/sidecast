import Foundation

/// Owns the reconnect loop around single-use `StreamClient`s and turns their
/// events into one `State` for the display screen. Runs on the same serial
/// queue as its clients (every method must be called there); callbacks are
/// delivered on that queue too.
public final class DisplaySession: StreamClientDelegate {
    public enum State: Equatable, Sendable {
        case idle
        case connecting(name: String)
        /// PAIR_START sent; waiting for the computer to show a code.
        case pairing(name: String)
        case awaitingCode(name: String, prompt: StreamClient.PairingPrompt)
        case verifyingCode(name: String)
        case streaming(name: String)
        case reconnecting(name: String, cause: RetryCause, attempt: Int)
        case failed(name: String, error: SessionError)
        /// Closed because the app went to the background; `resume()` reconnects.
        case paused
        case stopped
    }

    /// Builds the client for one attempt: pin the certificate of `expectedServerId` if paired,
    /// count into `counters`, and announce `inputMode()` after authenticating.
    public typealias ClientFactory = (_ expectedServerId: String?, _ counters: StreamCounters,
                                      _ inputMode: @escaping () -> InputMode) -> StreamClient

    public private(set) var state: State = .idle {
        didSet { if state != oldValue { onState?(state) } }
    }
    public var onState: ((State) -> Void)?
    public var onStreamConfig: ((StreamConfig) -> Void)?
    /// Video goes straight through (already acknowledged).
    public var onVideoFrame: ((VideoFrame) -> Void)?
    /// The computer's pointer (§7.3), or nil when unknown (new connection, mode change).
    public var onCursor: ((CursorPosition?) -> Void)?
    /// The path being tried or used, nil between attempts (§1.1).
    public var onRoute: ((Route?) -> Void)?
    /// MIC_CONTROL of the current stream (§7.2).
    public var onMicControl: ((MicControl) -> Void)?
    /// The connection the microphone was serving ended; a new stream starts with the mic off.
    public var onMicSessionEnded: (() -> Void)?

    /// Receive-side counters, kept across reconnects.
    public let counters = StreamCounters()
    /// The user's input mode (§7.3), announced after every authentication.
    public private(set) var inputMode: InputMode
    public private(set) var name: String
    /// The computer this session is (or will be) paired with.
    public private(set) var serverId: String?
    /// Raw `platform` of the computer once it said (SERVER_HELLO), else what the caller knew.
    public private(set) var platform: String?

    private let pairings: PairingRepository
    private let scheduler: TaskScheduler
    private let makeClient: ClientFactory
    private var client: StreamClient?
    private var micOwner: StreamClient?
    private var identityChangedServerId: String?
    private var failures = 0
    private var everStreamed = false
    private var retryCause: RetryCause?
    private var pendingRetry: ScheduledTask?
    private var stopped = false

    public init(name: String, serverId: String?, platform: String? = nil, pairings: PairingRepository,
                scheduler: TaskScheduler, inputMode: InputMode = .touch, makeClient: @escaping ClientFactory) {
        self.name = name
        self.serverId = serverId
        self.platform = platform
        self.pairings = pairings
        self.scheduler = scheduler
        self.inputMode = inputMode
        self.makeClient = makeClient
    }

    public var isStreaming: Bool { client?.isStreaming == true }

    public func start() {
        if state == .idle { connect() }
    }

    /// Terminal: closes everything; the session cannot be restarted.
    public func stop() {
        guard !stopped else { return }
        stopped = true
        cancelRetry()
        let c = client
        client = nil
        c?.close()
        state = .stopped
    }

    /// The user pressed Retry / Reconnect on an error.
    public func retryNow() {
        guard !stopped else { return }
        cancelRetry()
        let c = client
        client = nil
        c?.close()
        failures = 0
        retryCause = nil
        connect()
    }

    /// "Pair again" after an identity change: forget the stale pin and token, then reconnect.
    public func forgetAndRepair() {
        guard !stopped else { return }
        if let id = identityChangedServerId ?? serverId { pairings.remove(serverId: id) }
        identityChangedServerId = nil
        serverId = nil
        failures = 0
        retryCause = nil
        cancelRetry()
        let c = client
        client = nil
        c?.close()
        connect()
    }

    /// The app went to the background: drop the connection, keep the session.
    public func pause() {
        if stopped { return }
        if case .failed = state { return }
        cancelRetry()
        let c = client
        client = nil
        c?.close()
        state = .paused
    }

    public func resume() {
        guard !stopped, state == .paused else { return }
        failures = 0
        retryCause = nil
        connect()
    }

    /// The code's lifetime ran out on the device before the computer said so.
    public func expirePairing() {
        guard case .awaitingCode = state else { return }
        let c = client
        client = nil
        c?.close()
        state = .failed(name: name, error: .pairingExpired)
    }

    /// False when the code was not accepted for sending.
    @discardableResult
    public func submitCode(_ code: String) -> Bool { client?.submitPairingCode(code) ?? false }

    /// The user toggled touch / trackpad: remember it and tell the current stream.
    public func setInputMode(_ mode: InputMode) {
        inputMode = mode
        onCursor?(nil)   // the computer sends a fresh position after the switch
        client?.sendInputMode(mode)
    }

    /// A connection attempt over `route` starts (the connector reports each candidate it tries).
    public func reportRoute(_ route: Route) { onRoute?(route) }

    public func sendTouch(_ touch: TouchEvent) { client?.sendTouch(touch) }
    public func sendText(_ text: String) { client?.sendText(text) }
    public func sendKey(_ key: KeyEvent) { client?.sendKey(key) }
    public func requestKeyframe() { client?.requestKeyframe() }

    /// MIC_STATE / AUDIO go to the stream whose MIC_CONTROL the microphone is answering.
    public func sendMicState(_ micState: MicState) { micOwner?.sendMicState(micState) }
    public func sendAudio(_ packet: AudioPacket) { micOwner?.sendAudio(packet) }

    // MARK: - Internals

    private func connect() {
        let c = makeClient(serverId, counters) { [weak self] in self?.inputMode ?? .touch }
        c.delegate = self
        client = c
        if let cause = retryCause {
            state = .reconnecting(name: name, cause: cause, attempt: failures)
        } else {
            state = .connecting(name: name)
        }
        c.start()
    }

    private func handleEnded(_ reason: StreamClient.EndReason) {
        client = nil
        onRoute?(nil)
        onCursor?(nil)
        if stopped { return }
        if case .identityChanged(let id) = reason { identityChangedServerId = id ?? serverId }
        switch SessionPolicy.next(after: reason, everStreamed: everStreamed) {
        case .stop:
            state = .stopped
        case .fail(let error):
            retryCause = nil
            state = .failed(name: name, error: error)
        case .retry(let cause, let immediate):
            retryCause = cause
            if immediate {
                failures = 0
                connect()
                return
            }
            failures += 1
            state = .reconnecting(name: name, cause: cause, attempt: failures)
            pendingRetry = scheduler.schedule(after: ReconnectBackoff.delay(failures: failures)) { [weak self] in
                guard let self, !self.stopped, self.client == nil, case .reconnecting = self.state else { return }
                self.connect()
            }
        }
    }

    private func cancelRetry() {
        pendingRetry?.cancel()
        pendingRetry = nil
    }

    // MARK: - StreamClientDelegate

    public func streamClient(_ client: StreamClient, didEnter phase: StreamClient.Phase) {
        guard client === self.client else { return }
        switch phase {
        case .pairing: state = .pairing(name: name)
        case .verifyingCode: state = .verifyingCode(name: name)
        case .streaming: break   // didAuthenticate
        case .connecting, .handshaking, .authenticating:
            if case .reconnecting = state { return }
            state = .connecting(name: name)
        }
    }

    public func streamClient(_ client: StreamClient, needsPairingCode prompt: StreamClient.PairingPrompt) {
        guard client === self.client else { return }
        state = .awaitingCode(name: name, prompt: prompt)
    }

    public func streamClient(_ client: StreamClient, didAuthenticate server: StreamClient.ServerIdentity) {
        guard client === self.client else { return }
        failures = 0
        retryCause = nil
        everStreamed = true
        serverId = server.serverId
        platform = server.platform
        if !server.serverName.trimmingCharacters(in: .whitespaces).isEmpty { name = server.serverName }
        state = .streaming(name: name)
    }

    public func streamClient(_ client: StreamClient, didReceive config: StreamConfig) {
        if client === self.client { onStreamConfig?(config) }
    }

    public func streamClient(_ client: StreamClient, didReceive frame: VideoFrame) {
        if client === self.client { onVideoFrame?(frame) }
    }

    public func streamClient(_ client: StreamClient, didReceive cursor: CursorPosition) {
        if client === self.client { onCursor?(cursor) }
    }

    public func streamClient(_ client: StreamClient, didReceive micControl: MicControl) {
        guard client === self.client else { return }
        micOwner = client
        onMicControl?(micControl)
    }

    public func streamClient(_ client: StreamClient, didConnectVia route: Route) {
        if client === self.client { onRoute?(route) }
    }

    public func streamClient(_ client: StreamClient, didEndWith reason: StreamClient.EndReason) {
        if micOwner === client {
            micOwner = nil
            onMicSessionEnded?()
        }
        if client === self.client { handleEnded(reason) }
    }
}
