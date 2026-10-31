import Foundation
import Network
import SidecastKit

/// A scripted fake phone: TLS → HELLO → pairing or AUTH → stream for N
/// seconds, acking and decoding every frame, then prints stats.
/// Everything runs on `queue`.
final class ProbeClient {
    private enum Phase { case connecting, hello, authenticating, streaming, finished }

    private let options: ProbeOptions
    private let queue = DispatchQueue(label: "sidecast-probe")
    private let finish: (Int32) -> Void
    private var channel: MessageChannel!
    private var state: ProbeState
    private var phase = Phase.connecting
    private var fingerprint: Data?
    private var serverHello: ServerHello?
    private var decoder: ProbeDecoder?
    private var config: StreamConfig?
    private var stats = ProbeStats()
    private var failures: [String] = []
    private var timers: [DispatchSourceTimer] = []
    private var expectedSeq: UInt32?
    private var expectKeyframe = false
    private var keyframeRequestedAt: TimeInterval?
    private var stalledAt: TimeInterval?
    private var codeIndex = 0
    private var keyboardSent = false
    // Microphone (--mic)
    private var micTimer: DispatchSourceTimer?
    private var micStartedAt: TimeInterval = 0
    private var micSeq: UInt32 = 0
    private var micSampleIndex = 0
    private var micControls: [Bool] = []
    private var micPacketsSent = 0
    // CURSOR (trackpad mode)
    private var cursorCount = 0
    private var cursorPerSecond: [Int: Int] = [:]
    private var lastCursor: CursorPosition?
    /// Reason of a failed AUTH_RESULT whose BYE we are waiting for.
    private var authFailure: String?
    private let clock = MonotonicTimeSource()

    init(options: ProbeOptions, finish: @escaping (Int32) -> Void) {
        self.options = options
        self.finish = finish
        self.state = ProbeState.load(path: options.statePath)
    }

    func start() {
        queue.async { self.connect() }
    }

    // MARK: - Connection

    private func connect() {
        let tls = NWProtocolTLS.Options()
        let sec = tls.securityProtocolOptions
        sec_protocol_options_set_min_tls_protocol_version(sec, .TLSv12)
        // Trust on first pairing: accept any certificate here; the fingerprint
        // is checked against the pinned one before any secret is sent.
        sec_protocol_options_set_verify_block(sec, { _, _, complete in complete(true) }, queue)
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        let connection = NWConnection(host: NWEndpoint.Host(options.host),
                                      port: NWEndpoint.Port(rawValue: options.port)!,
                                      using: NWParameters(tls: tls, tcp: tcp))
        channel = MessageChannel(connection: connection, queue: queue)
        channel.onReady = { [weak self] in self?.connected() }
        channel.onFrame = { [weak self] frame in self?.handle(frame) }
        channel.onClose = { [weak self] error in self?.closed(error) }
        channel.start()
        after(10) { [weak self] in
            guard let self, self.phase == .connecting else { return }
            self.fail("timed out connecting to \(self.options.host):\(self.options.port)")
        }
    }

    private func connected() {
        fingerprint = channel.peerLeafFingerprint()
        var tlsVersion = "?"
        if let meta = channel.connection.metadata(definition: NWProtocolTLS.definition) as? NWProtocolTLS.Metadata {
            let v = sec_protocol_metadata_get_negotiated_tls_protocol_version(meta.securityProtocolMetadata)
            tlsVersion = v == .TLSv13 ? "1.3" : v == .TLSv12 ? "1.2" : "\(v.rawValue)"
        }
        print("probe: TLS \(tlsVersion) up, server certificate fingerprint \(fingerprint?.hexString ?? "none")")
        guard fingerprint != nil else { return fail("no server certificate") }
        phase = .hello
        let hello = Hello(deviceId: state.deviceId, deviceName: "sidecast-probe",
                          screen: ScreenInfo(width: options.width, height: options.height, densityDpi: 420,
                                             refreshRate: options.refreshRate),
                          codecs: options.codecs, features: options.mic ? [ClientFeature.mic] : [],
                          network: options.network, limits: options.limits)
        send(.hello(hello))
    }

    private func closed(_ error: Error?) {
        guard phase != .finished else { return }
        if let authFailure { return fail("connection closed without BYE after AUTH_RESULT \(authFailure)") }
        if let stalledAt, phase == .streaming {
            // Liveness check: the server must drop a silent client after ~5 s.
            let silent = clock.now() - stalledAt
            print(String(format: "probe: server closed the connection %.1f s after we went silent", silent))
            phase = .finished
            timers.forEach { $0.cancel() }
            decoder?.finish()
            if silent < 4.5 || silent > 7.5 { failures.append(String(format: "liveness timeout was %.1f s, expected ~5 s", silent)) }
            return report()
        }
        fail("connection closed unexpectedly\(error.map { ": \($0)" } ?? "")")
    }

    // MARK: - Messages

    private func handle(_ frame: Frame) {
        let message: ServerMessage?
        do {
            message = try ServerMessage.decode(frame)
        } catch {
            return fail("protocol error from server: \(error)")
        }
        guard let message else { return print("probe: skipping unknown message type 0x\(String(frame.type, radix: 16))") }
        switch message {
        case .serverHello(let hello): handleServerHello(hello)
        case .pairChallenge(let challenge): handleChallenge(challenge)
        case .authResult(let result): handleAuthResult(result)
        case .streamConfig(let cfg): handleStreamConfig(cfg)
        case .videoFrame(let video): handleVideo(video, wireBytes: frame.payload.count + 5)
        case .pong(let t): stats.recordRTT(Double(MonotonicTimeSource.nowMicros &- t) / 1e6)
        case .micControl(let control): handleMicControl(control)
        case .cursor(let position): handleCursor(position)
        case .bye(let bye):
            if phase == .finished { return }
            if let authFailure { return concludeAuthFailure(authFailure, bye: bye.reason) }
            fail("server sent BYE \(bye.reason)")
        }
    }

    private func handleServerHello(_ hello: ServerHello) {
        guard phase == .hello else { return fail("unexpected SERVER_HELLO") }
        guard hello.protocol == SidecastProtocol.version else { return fail("server protocol \(hello.protocol)") }
        serverHello = hello
        print("probe: SERVER_HELLO \(hello.serverName) serverId \(hello.serverId) port \(hello.port.map(String.init) ?? "-")"
              + " platform \(hello.platform)\(hello.hostPlatform == nil ? " (unknown)" : "")")
        for address in hello.addresses ?? [] {
            print("probe:   address \(address.host) (\(address.kind))")
        }
        if hello.addresses == nil { print("probe:   no addresses advertised") }
        phase = .authenticating
        if let pairing = state.servers[hello.serverId] {
            guard pairing.fingerprint == fingerprint?.hexString else {
                return fail("This computer's identity changed — pair again (pinned \(pairing.fingerprint))")
            }
            guard let token = Data(base64Encoded: pairing.token) else { return fail("corrupt token in state") }
            print("probe: pinned fingerprint matches; sending AUTH")
            send(.auth(AuthRequest(deviceId: state.deviceId, token: token)))
        } else if !options.codes.isEmpty {
            print("probe: not paired yet; sending PAIR_START")
            send(.pairStart)
        } else {
            fail("not paired with this computer and no --code given")
        }
    }

    private func handleChallenge(_ challenge: PairChallenge) {
        guard !options.codes.isEmpty, fingerprint != nil else { return fail("unexpected PAIR_CHALLENGE") }
        print("probe: PAIR_CHALLENGE expiresInSec=\(challenge.expiresInSec) attemptsLeft=\(challenge.attemptsLeft)")
        sendProof()
    }

    /// Sends PAIR_PROOF for the current code, after `--pair-delay-ms` of silence (a user typing).
    private func sendProof() {
        let code = options.codes[codeIndex]
        if options.pairDelayMs > 0 {
            print("probe: typing the code for \(options.pairDelayMs) ms (connection silent, no PINGs)")
        }
        after(Double(options.pairDelayMs) / 1000) { [weak self] in
            guard let self, self.phase == .authenticating, let fingerprint = self.fingerprint else { return }
            let nonce = SystemRandom().bytes(PairingProofCalculator.nonceLength)
            let proof = PairingProofCalculator.proof(code: code, certFingerprint: fingerprint, nonce: nonce)
            print("probe: PAIR_PROOF with code \(code)")
            self.send(.pairProof(PairProof(nonce: nonce, proof: proof)))
        }
    }

    private func handleAuthResult(_ result: AuthResult) {
        guard phase == .authenticating, authFailure == nil, let serverHello else { return fail("unexpected AUTH_RESULT") }
        guard result.ok else {
            let reason = result.reason ?? "?"
            print("probe: AUTH_RESULT ok=false reason=\(reason) attemptsLeft=\(result.attemptsLeft.map(String.init) ?? "-")")
            switch reason {
            case ByeReason.badCode.rawValue:
                guard let left = result.attemptsLeft, left >= 1 else { return fail("bad_code must carry attemptsLeft >= 1") }
                if codeIndex + 1 < options.codes.count {
                    codeIndex += 1
                    return sendProof()   // the server keeps the pairing open: retry
                }
                return concludeAuthFailure(reason, bye: nil)
            case ByeReason.tooManyAttempts.rawValue:
                guard result.attemptsLeft == 0 else { return fail("too_many_attempts must carry attemptsLeft 0") }
            case ByeReason.unknownDevice.rawValue:
                state.servers.removeValue(forKey: serverHello.serverId)
                try? state.save(path: options.statePath)
                print("probe: server forgot us; deleted the stored pairing")
            default:
                break
            }
            authFailure = reason   // a BYE must follow
            after(3) { [weak self] in
                guard let self, self.phase != .finished, self.authFailure != nil else { return }
                self.fail("no BYE within 3 s after AUTH_RESULT \(reason)")
            }
            return
        }
        if let expected = options.expectAuthFailure {
            return fail("authentication succeeded, expected failure \(expected)")
        }
        if let token = result.token?.data {
            state.servers[serverHello.serverId] = ProbeState.Pairing(
                fingerprint: fingerprint!.hexString, token: token.base64EncodedString(),
                serverName: serverHello.serverName, host: options.host, port: options.port)
            do { try state.save(path: options.statePath) } catch { return fail("cannot save state: \(error)") }
            print("probe: PAIRED (token received\(options.statePath == nil ? "" : ", saved to state"))")
        } else {
            print("probe: AUTH ok")
        }
        startStreaming()
    }

    /// Ends a run whose authentication failed: PASS only if `--expect-auth-failure` named this reason.
    private func concludeAuthFailure(_ reason: String, bye: String?) {
        if let bye { print("probe: BYE \(bye)") }
        if reason == ByeReason.tooManyAttempts.rawValue, bye != reason {
            failures.append("expected BYE too_many_attempts, got \(bye ?? "none")")
        }
        phase = .finished
        timers.forEach { $0.cancel() }
        channel.cancel()
        if options.expectAuthFailure != reason {
            failures.append("authentication failed: \(reason)")
        } else {
            print("probe: authentication failed with \(reason), as expected")
        }
        report()
    }

    private func handleStreamConfig(_ cfg: StreamConfig) {
        guard phase == .streaming else { return fail("STREAM_CONFIG before authentication") }
        guard let codec = VideoCodec(rawValue: cfg.codec) else { return fail("unknown codec \(cfg.codec)") }
        if cfg.width % 2 != 0 || cfg.height % 2 != 0 { failures.append("odd stream size \(cfg.width)x\(cfg.height)") }
        print("probe: STREAM_CONFIG \(cfg.codec) \(cfg.width)x\(cfg.height)@\(cfg.fps)")
        if let maxFps = options.limits?.maxFps, cfg.fps > maxFps {
            failures.append("STREAM_CONFIG fps \(cfg.fps) exceeds the requested limit \(maxFps)")
        }
        decoder?.finish()
        decoder = ProbeDecoder(codec: codec, width: cfg.width, height: cfg.height)
        config = cfg
        expectKeyframe = true
        stats.configs += 1
        // Like the phone: ask for a keyframe after every decoder (re)configuration.
        keyframeRequestedAt = clock.now()
        send(.keyframeRequest)
    }

    private func handleVideo(_ video: VideoFrame, wireBytes: Int) {
        guard let decoder else { return fail("VIDEO_FRAME before STREAM_CONFIG") }
        let seq = video.seq
        if options.slowAckMs > 0 {
            after(Double(options.slowAckMs) / 1000) { [weak self] in self?.send(.frameAck(seq: seq)) }
        } else {
            send(.frameAck(seq: seq))
        }
        if let expected = expectedSeq, seq != expected { stats.seqGaps += 1 }
        if expectedSeq == nil, seq != 0 { failures.append("first seq is \(seq), expected 0") }
        expectedSeq = seq &+ 1
        if video.flags & ~VideoFrame.keyframeFlag != 0 { failures.append("reserved flag bits set: \(video.flags)") }
        if expectKeyframe, !video.isKeyframe { failures.append("frame \(seq) after STREAM_CONFIG is not a keyframe") }
        expectKeyframe = false
        let now = clock.now()
        if video.isKeyframe, let asked = keyframeRequestedAt {
            print(String(format: "probe: keyframe arrived %.0f ms after KEYFRAME_REQUEST", (now - asked) * 1000))
            keyframeRequestedAt = nil
        }
        if !keyboardSent {
            keyboardSent = true
            sendKeyboard()
            playGestures()
        }
        let hadParameterSets = decoder.decode(video.data)
        if video.isKeyframe, !hadParameterSets { failures.append("keyframe \(seq) without parameter sets") }
        stats.recordFrame(bytes: wireBytes, keyframe: video.isKeyframe, at: now)
    }

    // MARK: - Cursor

    private func handleCursor(_ position: CursorPosition) {
        guard options.inputMode == .trackpad else {
            return failures.append("CURSOR received although the probe is not in trackpad mode")
        }
        cursorCount += 1
        let second = Int(clock.now() - stats.startTime)
        cursorPerSecond[second, default: 0] += 1
        lastCursor = position
        let shown = position.isOnDisplay ? "" : " (pointer not on this display)"
        print(String(format: "probe: CURSOR #%d x=%.4f y=%.4f", cursorCount, position.x, position.y) + shown)
    }

    // MARK: - Microphone

    private func handleMicControl(_ control: MicControl) {
        print("probe: MIC_CONTROL active=\(control.active)"
              + (control.sampleRate.map { " sampleRate=\($0)" } ?? "") + (control.channels.map { " channels=\($0)" } ?? ""))
        guard options.mic else { return failures.append("MIC_CONTROL sent to a client that did not advertise mic") }
        guard phase == .streaming else { return failures.append("MIC_CONTROL before streaming") }
        micControls.append(control.active)
        if control.active {
            if let rate = control.sampleRate, rate != AudioPacket.sampleRate { failures.append("MIC_CONTROL sampleRate \(rate)") }
            if let channels = control.channels, channels != AudioPacket.channels { failures.append("MIC_CONTROL channels \(channels)") }
            startMic()
        } else {
            stopMic(reason: .stopped)
        }
    }

    /// 10 ms packets of a 440 Hz sine, seq from 0, paced by the clock (catches up after timer jitter).
    private func startMic() {
        micTimer?.cancel()
        micSeq = 0
        micStartedAt = clock.now()
        send(.micState(MicState(capturing: true)))
        print("probe: MIC_STATE capturing=true; sending a 440 Hz tone every 10 ms")
        let t = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        t.schedule(deadline: .now(), repeating: .milliseconds(10), leeway: .milliseconds(1))
        t.setEventHandler { [weak self] in self?.sendDueAudio() }
        t.resume()
        micTimer = t
    }

    private func stopMic(reason: MicStateReason) {
        guard let timer = micTimer else { return }
        timer.cancel()
        micTimer = nil
        send(.micState(MicState(capturing: false, reason: reason)))
        print("probe: MIC_STATE capturing=false reason=\(reason.rawValue)")
    }

    private func sendDueAudio() {
        let frames = AudioPacket.recommendedSamples
        let due = UInt32((clock.now() - micStartedAt) / 0.010) + 1
        while micSeq < due {
            var samples = [Int16](repeating: 0, count: frames)
            for i in 0..<frames {
                let phase = 2 * Double.pi * 440 * Double(micSampleIndex + i) / Double(AudioPacket.sampleRate)
                samples[i] = Int16(sin(phase) * 16_000)
            }
            micSampleIndex += frames
            send(.audio(AudioPacket(seq: micSeq, samples: samples)))
            micSeq += 1
            micPacketsSent += 1
        }
    }

    // MARK: - Streaming

    private func startStreaming() {
        phase = .streaming
        if let mode = options.inputMode {
            print("probe: sending INPUT_MODE \(mode.rawValue)")
            send(.inputMode(InputModeMessage(mode)))
        }
        stats.start(at: clock.now())
        repeating(1) { [weak self] in self?.send(.ping(MonotonicTimeSource.nowMicros)) }
        repeating(1) { [weak self] in
            guard let self else { return }
            print(self.stats.secondLine(decoder: self.decoder))
        }
        if options.touch {
            var t: UInt32 = 0
            repeating(1) { [weak self] in
                t &+= 1000
                let p = [TouchPointer(id: 0, x: 0.5, y: 0.5)]
                self?.send(.touch(TouchEvent(eventTimeMs: t, action: .down, actionPointerId: 0, pointers: p)))
                self?.send(.touch(TouchEvent(eventTimeMs: t &+ 60, action: .up, actionPointerId: 0, pointers: p)))
            }
        }
        if options.keyframeRequest {
            after(options.seconds / 2) { [weak self] in
                guard let self else { return }
                print("probe: sending KEYFRAME_REQUEST")
                self.keyframeRequestedAt = self.clock.now()
                self.send(.keyframeRequest)
            }
        }
        if let stall = options.stallAfter {
            after(stall) { [weak self] in
                guard let self else { return }
                print("probe: going silent (no more acks or pings)")
                self.stalledAt = self.clock.now()
            }
            after(stall + 10) { [weak self] in self?.fail("server did not drop the silent client within 10 s") }
            return
        }
        after(options.seconds) { [weak self] in self?.complete() }
    }

    /// Sends `--type` / `--key` messages, in order, once frames flow.
    private func sendKeyboard() {
        for message in options.keyboard {
            switch message {
            case .text(let text):
                let note = text.utf8.count > TextMessage.maxBytes ? " (over the 4096-byte limit)" : ""
                print("probe: sending TEXT \(text.debugDescription) (\(text.utf8.count) bytes)\(note)")
            case .key(let event):
                let name = event.protocolKey.map { "\($0)" } ?? "unknown"
                print("probe: sending KEY key=\(event.key) (\(name)) modifiers=0x" + String(format: "%02x", event.modifiers))
            default:
                continue
            }
            send(message)
        }
    }

    /// Plays `--gesture` scripts in real time, 600 ms apart (beyond the double-tap window),
    /// paced by a strict 4 ms timer so MOVEs really leave ~8 ms apart.
    private func playGestures() {
        guard !options.gestures.isEmpty else { return }
        var schedule: [(ms: Int, run: () -> Void)] = []
        var offsetMs = 200
        let baseTime = UInt32(truncatingIfNeeded: MonotonicTimeSource.nowMicros / 1000)
        for gesture in options.gestures {
            let steps = gesture.steps
            let start = offsetMs
            schedule.append((start, {
                print("probe: gesture \(gesture.rawValue): \(steps.count) TOUCH events over \(steps.last?.ms ?? 0) ms "
                      + "(expect: \(gesture.expected))")
            }))
            for step in steps {
                let ms = start + step.ms
                let event = TouchEvent(eventTimeMs: baseTime &+ UInt32(ms), action: step.action,
                                       actionPointerId: step.actionPointerId, pointers: step.pointers)
                schedule.append((ms, { [weak self] in self?.send(.touch(event)) }))
            }
            offsetMs = start + (steps.last?.ms ?? 0) + 600
        }
        let startedAt = clock.now()
        var next = 0
        let t = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        t.schedule(deadline: .now(), repeating: .milliseconds(4), leeway: .milliseconds(1))
        t.setEventHandler { [weak self, weak t] in
            guard let self else { return }
            let elapsedMs = Int((self.clock.now() - startedAt) * 1000)
            while next < schedule.count, schedule[next].ms <= elapsedMs {
                schedule[next].run()
                next += 1
            }
            if next == schedule.count { t?.cancel() }
        }
        t.resume()
        timers.append(t)
    }

    private func complete() {
        guard phase == .streaming else { return }
        phase = .finished
        timers.forEach { $0.cancel() }
        channel.cancel()
        decoder?.finish()
        if let config { print("probe: stream \(config.codec) \(config.width)x\(config.height)@\(config.fps)") }
        if options.inputMode == .trackpad {
            let maxRate = cursorPerSecond.values.max() ?? 0
            print("probe: CURSOR received \(cursorCount), max \(maxRate) in one second, last "
                  + (lastCursor.map { String(format: "(%.4f, %.4f)", $0.x, $0.y) } ?? "-"))
            if maxRate > 62 { failures.append("CURSOR rate \(maxRate)/s exceeds 60/s") }
        }
        if options.mic {
            micTimer?.cancel()
            let controls = micControls.map { $0 ? "on" : "off" }.joined(separator: ",")
            print("probe: MIC controls received [\(controls)], AUDIO packets sent \(micPacketsSent) "
                  + "(\(micPacketsSent * AudioPacket.recommendedSamples) samples)")
        }
        failures += stats.problems(decoder: decoder, expectKeyframeResponse: keyframeRequestedAt != nil)
        for line in stats.summary(decoder: decoder) { print(line) }
        report()
    }

    private func fail(_ why: String) {
        guard phase != .finished else { return }
        phase = .finished
        timers.forEach { $0.cancel() }
        micTimer?.cancel()
        channel?.cancel()
        decoder?.finish()
        failures.append(why)
        report()
    }

    private func report() {
        if failures.isEmpty {
            print("probe: RESULT PASS")
            finish(0)
        } else {
            for f in failures { print("probe: FAILURE \(f)") }
            print("probe: RESULT FAIL")
            finish(1)
        }
    }

    // MARK: - Helpers

    private func send(_ message: ClientMessage) {
        guard stalledAt == nil else { return }
        channel.send(message.encoded())
    }

    private func after(_ seconds: Double, _ body: @escaping () -> Void) {
        queue.asyncAfter(deadline: .now() + seconds, execute: body)
    }

    private func repeating(_ interval: Double, _ body: @escaping () -> Void) {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + interval, repeating: interval)
        t.setEventHandler(handler: body)
        t.resume()
        timers.append(t)
    }
}
