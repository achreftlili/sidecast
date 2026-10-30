import Foundation
import Network
import SidecastKit

/// What a session needs from the server. Called on the server queue.
protocol SessionHost: AnyObject {
    var serverHello: ServerHello { get }
    var certificateFingerprint: Data { get }
    var pairedDevices: PairedDeviceStore { get }
    func nextPairingCode() -> PairingCode
    func sessionWillStartPairing(_ session: ClientSession, code: PairingCode, expiresIn: TimeInterval)
    func sessionDidEndPairing(_ session: ClientSession, paired: Bool)
    func sessionDidAuthenticate(_ session: ClientSession)
    func sessionDidClose(_ session: ClientSession)
    /// AUDIO / MIC_STATE from a streaming session whose mic was requested.
    func session(_ session: ClientSession, didReceiveAudio packet: AudioPacket)
    func session(_ session: ClientSession, didReceiveMicState state: MicState)
}

/// One phone connection: the PROTOCOL §5 state machine
/// (HELLO → SERVER_HELLO → AUTH | PAIR_START/PAIR_PROOF → streaming).
/// All methods run on the server queue.
final class ClientSession {
    enum Phase: Equatable {
        case awaitingHello
        case awaitingAuth
        case pairing
        case authenticated
        case closed
    }

    let id = UUID()
    let channel: MessageChannel
    private(set) var phase: Phase = .awaitingHello
    private(set) var hello: Hello?
    private(set) var streamer: StreamController?
    private weak var host: SessionHost?
    private let queue: DispatchQueue
    private let clock: TimeSource
    private var pairing: PairingSession?
    private var pairingTimer: DispatchWorkItem?
    private var authTimer: DispatchWorkItem?
    /// The last MIC_CONTROL sent was active:true (a new session starts with the mic off).
    private(set) var micRequested = false
    /// Owned by the server: this session was announced to the mic bridge.
    var micStarted = false

    var deviceName: String { hello?.deviceName ?? "unknown device" }
    var deviceId: String? { hello?.deviceId }
    var peer: String { "\(channel.connection.endpoint)" }

    init(connection: NWConnection, host: SessionHost, queue: DispatchQueue, clock: TimeSource = MonotonicTimeSource()) {
        self.channel = MessageChannel(connection: connection, queue: queue)
        self.host = host
        self.queue = queue
        self.clock = clock
    }

    func start() {
        channel.onFrame = { [weak self] frame in self?.handle(frame) }
        channel.onClose = { [weak self] error in self?.didClose(error) }
        channel.start()
        let timeout = DispatchWorkItem { [weak self] in
            guard let self, self.phase != .authenticated, self.phase != .closed else { return }
            Log.info("session \(self.peer): not authenticated within \(Int(SidecastProtocol.authTimeout)) s — closing")
            self.close()
        }
        authTimer = timeout
        queue.asyncAfter(deadline: .now() + SidecastProtocol.authTimeout, execute: timeout)
    }

    /// Sends BYE `reason`, then closes.
    func bye(_ reason: ByeReason) {
        guard phase != .closed else { return }
        Log.info("session \(peer) (\(deviceName)): BYE \(reason.rawValue)")
        tearDown()
        channel.close(after: ServerMessage.bye(Bye(reason: reason)).encoded())
    }

    /// Closes without a BYE (dead peer, protocol violation).
    func close() {
        guard phase != .closed else { return }
        tearDown()
        channel.close()
    }

    /// Starts streaming (called by the server once this session owns the stream).
    func startStreaming(_ streamer: StreamController) {
        guard phase == .authenticated else { return }
        self.streamer = streamer
        streamer.onFailure = { [weak self] reason in
            guard let self else { return }
            if let reason { self.bye(reason) } else { self.close() }
        }
        streamer.start()
    }

    /// Sends MIC_CONTROL (only to an authenticated, streaming client that advertised `mic`).
    func sendMicControl(_ active: Bool) {
        guard phase == .authenticated, streamer != nil, hello?.supportsMic == true, active != micRequested else { return }
        micRequested = active
        send(.micControl(active ? .start : .stop))
        Log.info("session \(peer) (\(deviceName)): MIC_CONTROL active=\(active)")
    }

    /// Ends a pairing that another phone's pairing superseded (or that timed out).
    func expirePairing() {
        guard phase == .pairing else { return }
        send(.authResult(AuthResult(ok: false, reason: .pairingExpired)))
        finishPairing(paired: false)
        bye(.pairingExpired)
    }

    // MARK: - Incoming

    private func handle(_ frame: Frame) {
        streamer?.noteActivity()
        let message: ClientMessage?
        do {
            message = try ClientMessage.decode(frame)
        } catch HelloError.versionMismatch(let version) {
            Log.info("session \(peer): protocol \(version) not supported")
            return bye(.protocolMismatch)
        } catch {
            if let type = frame.messageType, [.text, .key, .audio, .micState, .inputMode].contains(type) {
                // PROTOCOL §7.1 / §7.2: malformed keyboard and mic payloads are ignored, never fatal.
                Log.debug("session \(peer): ignoring malformed \(frame.messageType!): \(error)")
                return
            }
            if frame.messageType == .touch, phase == .authenticated {
                Log.debug("session \(peer): ignoring malformed TOUCH: \(error)")
                return
            }
            Log.error("session \(peer): protocol error: \(error)")
            return bye(.protocolMismatch)
        }
        guard let message else { return }   // unknown type: skipped

        switch (phase, message) {
        case (.awaitingHello, .hello(let h)):
            hello = h
            phase = .awaitingAuth
            Log.info("session \(peer): HELLO from \(h.deviceName) screen \(h.screen.width)x\(h.screen.height) "
                     + "codecs \(h.codecs.map { "\($0.name) \($0.maxWidth)x\($0.maxHeight)" })")
            if let host { send(.serverHello(host.serverHello)) }
        case (.awaitingHello, .inputMode):
            Log.debug("session \(peer): ignoring INPUT_MODE before HELLO")
        case (.awaitingHello, _):
            Log.error("session \(peer): \(message.type) before HELLO")
            bye(.protocolMismatch)
        case (_, .ping(let t)):
            send(.pong(t))
        case (.awaitingAuth, .auth(let request)), (.pairing, .auth(let request)):
            handleAuth(request)
        case (.awaitingAuth, .pairStart), (.pairing, .pairStart):
            startPairing()
        case (.pairing, .pairProof(let proof)):
            handleProof(proof)
        case (.authenticated, .frameAck(let seq)):
            streamer?.handleAck(seq)
        case (.authenticated, .keyframeRequest):
            streamer?.requestKeyframe()
        case (.authenticated, .touch(let event)):
            streamer?.handleTouch(event)
        case (.authenticated, .text(let text)):
            streamer?.handleText(text)
        case (.authenticated, .key(let event)):
            streamer?.handleKey(event)
        case (.authenticated, .audio(let packet)):
            // Without a pending active:true it is ignored (PROTOCOL §7.2); not logged, it would be 100/s.
            if micRequested { host?.session(self, didReceiveAudio: packet) }
        case (.authenticated, .micState(let state)):
            if micRequested {
                host?.session(self, didReceiveMicState: state)
            } else {
                Log.debug("session \(peer): ignoring MIC_STATE (mic not requested)")
            }
        case (.authenticated, .inputMode(let message)):
            if let mode = message.inputMode {
                streamer?.setInputMode(mode)
            } else {
                Log.debug("session \(peer): ignoring unknown input mode \(message.mode.debugDescription)")
            }
        case (_, .audio):
            break   // before authentication: ignored, not logged
        default:
            Log.debug("session \(peer): ignoring \(message.type) in phase \(phase)")
        }
    }

    private func handleAuth(_ request: AuthRequest) {
        guard let host else { return }
        if phase == .pairing { finishPairing(paired: false) }
        guard let device = host.pairedDevices.authenticate(deviceId: request.deviceId, token: request.token.data) else {
            Log.info("session \(peer): AUTH failed for device \(request.deviceId)")
            send(.authResult(AuthResult(ok: false, reason: .unknownDevice)))
            return bye(.unknownDevice)
        }
        host.pairedDevices.markSeen(deviceId: device.deviceId, name: hello?.deviceName)
        Log.info("session \(peer): authenticated \(device.deviceName)")
        send(.authResult(AuthResult(ok: true)))
        didAuthenticate()
    }

    private func startPairing() {
        guard let host else { return }
        pairingTimer?.cancel()
        let code = host.nextPairingCode()
        host.sessionWillStartPairing(self, code: code, expiresIn: SidecastProtocol.pairingLifetime)
        pairing = PairingSession(code: code, startedAt: clock.now())
        phase = .pairing
        let expiry = DispatchWorkItem { [weak self] in self?.expirePairing() }
        pairingTimer = expiry
        queue.asyncAfter(deadline: .now() + SidecastProtocol.pairingLifetime, execute: expiry)
        Log.info("session \(peer): pairing with \(deviceName), code \(code.displayString)")
        send(.pairChallenge(PairChallenge(expiresInSec: Int(SidecastProtocol.pairingLifetime),
                                          attemptsLeft: SidecastProtocol.pairingAttempts)))
    }

    private func handleProof(_ proof: PairProof) {
        guard let host, var session = pairing, let hello else { return }
        let outcome = session.verify(nonce: proof.nonce.data, proof: proof.proof.data,
                                     certFingerprint: host.certificateFingerprint, at: clock.now())
        pairing = session
        switch outcome {
        case .success:
            let token = DeviceToken.generate()
            do {
                try host.pairedDevices.add(deviceId: hello.deviceId, deviceName: hello.deviceName, token: token)
            } catch {
                Log.error("session \(peer): could not save pairing: \(error)")
                finishPairing(paired: false)
                return bye(.serverShutdown)
            }
            Log.info("session \(peer): paired \(hello.deviceName)")
            finishPairing(paired: true)
            send(.authResult(AuthResult(ok: true, token: token)))
            didAuthenticate()
        case .badCode(let left):
            Log.info("session \(peer): wrong pairing code, \(left) attempts left")
            send(.authResult(AuthResult(ok: false, reason: .badCode, attemptsLeft: left)))
        case .tooManyAttempts:
            send(.authResult(AuthResult(ok: false, reason: .tooManyAttempts, attemptsLeft: 0)))
            finishPairing(paired: false)
            bye(.tooManyAttempts)
        case .expired:
            send(.authResult(AuthResult(ok: false, reason: .pairingExpired)))
            finishPairing(paired: false)
            bye(.pairingExpired)
        }
    }

    private func didAuthenticate() {
        phase = .authenticated
        authTimer?.cancel()
        authTimer = nil
        host?.sessionDidAuthenticate(self)
    }

    private func finishPairing(paired: Bool) {
        pairingTimer?.cancel()
        pairingTimer = nil
        guard pairing != nil else { return }
        pairing = nil
        if phase == .pairing { phase = .awaitingAuth }
        host?.sessionDidEndPairing(self, paired: paired)
    }

    // MARK: - Lifecycle

    private func send(_ message: ServerMessage) {
        channel.send(message.encoded())
    }

    private func tearDown() {
        micRequested = false
        if pairing != nil { finishPairing(paired: false) }
        authTimer?.cancel()
        authTimer = nil
        streamer?.stop()
        streamer = nil
        phase = .closed
    }

    private func didClose(_ error: Error?) {
        if let error { Log.debug("session \(peer): connection ended: \(error)") }
        Log.info("session \(peer) (\(deviceName)): closed")
        tearDown()
        host?.sessionDidClose(self)
        host = nil
    }
}
