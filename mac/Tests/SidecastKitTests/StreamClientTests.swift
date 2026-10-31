import Foundation
import XCTest
@testable import SidecastKit

// MARK: - Fakes

final class FakeTransport: ClientTransport {
    var onFrame: ((Frame) -> Void)?
    var onClose: ((Error?) -> Void)?
    var pendingSendBytes = 0
    private(set) var sent: [ClientMessage] = []
    private(set) var aborted = false

    func send(_ data: Data) {
        let parser = FrameParser()
        parser.append(data)
        for frame in try! parser.drain() { sent.append(try! ClientMessage.decode(frame)!) }
    }

    func abort() { aborted = true }

    func deliver(_ message: ServerMessage) {
        let parser = FrameParser()
        parser.append(message.encoded())
        for frame in try! parser.drain() { onFrame?(frame) }
    }

    func deliver(type: UInt8, json: String) { onFrame?(Frame(type: type, payload: Data(json.utf8))) }

    func clearSent() { sent.removeAll() }
}

final class FakeConnector: ClientConnector {
    private(set) var completion: ((Result<ClientConnection, ConnectFailure>) -> Void)?
    private(set) var cancelled = false

    func connect(_ completion: @escaping (Result<ClientConnection, ConnectFailure>) -> Void) { self.completion = completion }
    func cancel() { cancelled = true }

    func succeed(_ transport: FakeTransport, fingerprint: Data, route: Route = .lan,
                 reached: StreamClient.Endpoint? = nil) {
        completion?(.success(ClientConnection(transport: transport, peerFingerprint: fingerprint, reached: reached,
                                              route: route)))
    }

    func fail(_ failure: ConnectFailure) { completion?(.failure(failure)) }
}

/// Deterministic "random" bytes.
struct FixedRandom: RandomSource {
    func bytes(_ count: Int) -> Data { Data((0..<count).map { UInt8($0) }) }
}

final class ClientEvents: StreamClientDelegate {
    var phases: [StreamClient.Phase] = []
    var prompts: [StreamClient.PairingPrompt] = []
    var identity: StreamClient.ServerIdentity?
    var configs: [StreamConfig] = []
    var frames: [UInt32] = []
    var cursors: [CursorPosition] = []
    var micControls: [MicControl] = []
    var routes: [Route] = []
    var ended: [StreamClient.EndReason] = []
    /// Sent messages at the moment each video frame was delivered.
    var sentAtFrame: [Int] = []
    weak var transport: FakeTransport?

    func streamClient(_ client: StreamClient, didEnter phase: StreamClient.Phase) { phases.append(phase) }
    func streamClient(_ client: StreamClient, needsPairingCode prompt: StreamClient.PairingPrompt) { prompts.append(prompt) }
    func streamClient(_ client: StreamClient, didAuthenticate server: StreamClient.ServerIdentity) { identity = server }
    func streamClient(_ client: StreamClient, didReceive config: StreamConfig) { configs.append(config) }
    func streamClient(_ client: StreamClient, didReceive frame: VideoFrame) {
        frames.append(frame.seq)
        sentAtFrame.append(transport?.sent.count ?? -1)
    }
    func streamClient(_ client: StreamClient, didReceive cursor: CursorPosition) { cursors.append(cursor) }
    func streamClient(_ client: StreamClient, didReceive micControl: MicControl) { micControls.append(micControl) }
    func streamClient(_ client: StreamClient, didConnectVia route: Route) { routes.append(route) }
    func streamClient(_ client: StreamClient, didEndWith reason: StreamClient.EndReason) { ended.append(reason) }
}

/// The client state machine (PROTOCOL §5–§7) against a scripted server.
final class StreamClientTests: XCTestCase {
    private let fingerprint = Data(repeating: 0x42, count: 32)
    private let token = Data(repeating: 0xAA, count: 32)
    private let hello = Hello(deviceId: "device-1", deviceName: "iPad",
                              screen: ScreenInfo(width: 2732, height: 2048, densityDpi: 320, refreshRate: 120),
                              codecs: [CodecInfo(name: "hevc", maxWidth: 4096, maxHeight: 2304)], features: ["mic"])
    private let serverHello = ServerHello(serverId: "srv", serverName: "Studio", port: 47801,
                                          addresses: [ServerAddress(host: "100.64.0.9", kind: .tailscale)])

    private var scheduler: ManualScheduler!
    private var connector: FakeConnector!
    private var transport: FakeTransport!
    private var pairings: InMemoryPairingRepository!
    private var events: ClientEvents!
    private var client: StreamClient!
    private var mode = InputMode.trackpad

    override func setUp() {
        scheduler = ManualScheduler(start: 100)
        connector = FakeConnector()
        transport = FakeTransport()
        pairings = InMemoryPairingRepository()
        events = ClientEvents()
        events.transport = transport
    }

    private func makeClient(counters: StreamCounters = StreamCounters()) {
        client = StreamClient(connector: connector, hello: hello, pairings: pairings,
                              endpoint: .init(host: "127.0.0.1", port: 47897), scheduler: scheduler, clock: scheduler,
                              random: FixedRandom(), counters: counters, inputMode: { [unowned self] in self.mode })
        client.delegate = events
    }

    private func storedPairing(fingerprint: Data? = nil) -> PairedComputer {
        PairedComputer(serverId: "srv", serverName: "Old name", fingerprint: fingerprint ?? self.fingerprint, token: token,
                       lastHost: "192.168.1.2", lastPort: 47800)
    }

    /// Connects and answers HELLO; returns what the client sent.
    private func connectAndGreet() {
        makeClient()
        client.start()
        connector.succeed(transport, fingerprint: fingerprint, reached: .init(host: "10.0.0.7", port: 47897))
        XCTAssertEqual(transport.sent.first, .hello(hello))
        transport.deliver(.serverHello(serverHello))
    }

    private func streamAfterAuth() {
        pairings.save(storedPairing())
        connectAndGreet()
        transport.deliver(.authResult(AuthResult(ok: true)))
    }

    // MARK: - Pairing and authentication

    func testPairingFlowSavesThePairingAndStartsStreaming() throws {
        connectAndGreet()
        XCTAssertEqual(transport.sent.last, .pairStart)
        transport.deliver(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
        XCTAssertEqual(events.prompts, [.init(attemptsLeft: 5, expiresAt: 220, wrongCode: false)])
        XCTAssertFalse(client.submitPairingCode("12345"))   // not 6 digits
        XCTAssertTrue(client.submitPairingCode("123456"))
        XCTAssertFalse(client.submitPairingCode("123456"))   // not asked for any more
        let nonce = FixedRandom().bytes(16)
        XCTAssertEqual(transport.sent.last, .pairProof(PairProof(
            nonce: nonce, proof: PairingProofCalculator.proof(code: "123456", certFingerprint: fingerprint, nonce: nonce))))

        transport.clearSent()
        transport.deliver(.authResult(AuthResult(ok: true, token: token)))
        let saved = try XCTUnwrap(pairings.pairing(serverId: "srv"))
        XCTAssertEqual(saved.fingerprint, fingerprint)
        XCTAssertEqual(saved.token.data, token)
        XCTAssertEqual(saved.lastHost, "10.0.0.7")   // the address that answered
        XCTAssertEqual(saved.serverPort, 47801)
        XCTAssertEqual(saved.addresses, serverHello.addresses)
        XCTAssertEqual(events.identity, .init(serverId: "srv", serverName: "Studio", platform: "macos", newlyPaired: true))
        // INPUT_MODE comes first, before any touch, then the first PING.
        XCTAssertEqual(transport.sent.first, .inputMode(InputModeMessage(.trackpad)))
        guard case .ping? = transport.sent.dropFirst().first else { return XCTFail("\(transport.sent)") }
        XCTAssertEqual(events.phases, [.connecting, .handshaking, .pairing, .verifyingCode, .streaming])
        XCTAssertTrue(client.isStreaming)
    }

    func testWrongCodeAsksAgainAndTheLastOneEndsThePairing() {
        connectAndGreet()
        transport.deliver(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
        XCTAssertTrue(client.submitPairingCode("000000"))
        transport.deliver(.authResult(AuthResult(ok: false, reason: .badCode, attemptsLeft: 4)))
        XCTAssertEqual(events.prompts.last, .init(attemptsLeft: 4, expiresAt: 220, wrongCode: true))
        XCTAssertTrue(client.submitPairingCode("111111"))
        transport.deliver(.authResult(AuthResult(ok: false, reason: .tooManyAttempts, attemptsLeft: 0)))
        XCTAssertEqual(events.ended, [.serverEnded("too_many_attempts")])
        XCTAssertNil(pairings.pairing(serverId: "srv"))
        XCTAssertTrue(transport.aborted)
    }

    func testStoredTokenAuthenticatesAndUpdatesTheEndpoint() {
        streamAfterAuth()
        XCTAssertTrue(transport.sent.contains(.auth(AuthRequest(deviceId: "device-1", token: token))))
        XCTAssertFalse(transport.sent.contains(.pairStart))
        let updated = pairings.pairing(serverId: "srv")
        XCTAssertEqual(updated?.serverName, "Studio")
        XCTAssertEqual(updated?.lastHost, "10.0.0.7")
        XCTAssertEqual(updated?.lastPort, 47897)
        XCTAssertEqual(updated?.serverPort, 47801)
        XCTAssertEqual(events.identity?.newlyPaired, false)
    }

    func testAPinMismatchNeverSendsTheToken() {
        pairings.save(storedPairing(fingerprint: Data(repeating: 0x01, count: 32)))
        connectAndGreet()
        XCTAssertEqual(events.ended, [.identityChanged(serverId: "srv")])
        XCTAssertFalse(transport.sent.contains { if case .auth = $0 { return true } else { return false } })
    }

    func testUnknownDeviceForgetsThePairing() {
        pairings.save(storedPairing())
        connectAndGreet()
        transport.deliver(.authResult(AuthResult(ok: false, reason: .unknownDevice)))
        XCTAssertEqual(events.ended, [.serverEnded("unknown_device")])
        XCTAssertNil(pairings.pairing(serverId: "srv"))
    }

    func testPairingWindowClosedBeforeAnyProofIsTerminal() {
        connectAndGreet()
        transport.deliver(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
        transport.deliver(.authResult(AuthResult(ok: false, reason: .pairingExpired)))
        XCTAssertEqual(events.ended, [.serverEnded("pairing_expired")])
    }

    func testProtocolMismatchAndPlatform() {
        makeClient()
        client.start()
        connector.succeed(transport, fingerprint: fingerprint)
        transport.deliver(type: MessageType.serverHello.rawValue, json: #"{"protocol":2,"serverId":"s","serverName":"n"}"#)
        XCTAssertEqual(events.ended, [.serverEnded("protocol_mismatch")])

        setUp()
        pairings.save(storedPairing())
        makeClient()
        client.start()
        connector.succeed(transport, fingerprint: fingerprint)
        transport.deliver(type: MessageType.serverHello.rawValue,
                          json: #"{"protocol":1,"serverId":"srv","serverName":"Tower","platform":"windows","extra":1}"#)
        transport.deliver(.authResult(AuthResult(ok: true)))
        XCTAssertEqual(events.identity?.hostPlatform, .windows)
        XCTAssertEqual(pairings.pairing(serverId: "srv")?.platform, "windows")
    }

    func testConnectFailures() {
        makeClient()
        client.start()
        connector.fail(.identityChanged)
        XCTAssertEqual(events.ended, [.identityChanged(serverId: nil)])

        setUp()
        makeClient()
        client.start()
        connector.fail(.failed("refused"))
        XCTAssertEqual(events.ended, [.connectFailed("refused")])
    }

    func testCloseWhileConnectingAbortsALateConnection() {
        makeClient()
        client.start()
        client.close()
        XCTAssertEqual(events.ended, [.closedByUser])
        XCTAssertTrue(connector.cancelled)
        connector.succeed(transport, fingerprint: fingerprint)
        XCTAssertTrue(transport.aborted)
        XCTAssertTrue(transport.sent.isEmpty)
        client.close()
        XCTAssertEqual(events.ended.count, 1)
    }

    // MARK: - Streaming

    func testVideoFramesAreAckedBeforeDecoding() {
        streamAfterAuth()
        let config = StreamConfig(codec: "hevc", width: 2732, height: 2048, fps: 60)
        transport.deliver(.streamConfig(config))
        XCTAssertEqual(events.configs, [config])
        transport.clearSent()
        transport.deliver(.videoFrame(VideoFrame(seq: 0, isKeyframe: true, ptsMicros: 1, data: Data([0, 0, 0, 1, 0x40]))))
        transport.deliver(.videoFrame(VideoFrame(seq: 1, isKeyframe: false, ptsMicros: 2, data: Data([0, 0, 0, 1, 0x02]))))
        XCTAssertEqual(transport.sent, [.frameAck(seq: 0), .frameAck(seq: 1)])
        XCTAssertEqual(events.frames, [0, 1])
        XCTAssertEqual(events.sentAtFrame, [1, 2])   // each ack was out before the frame was handed on
    }

    func testVideoBeforeAuthenticationIsAProtocolError() {
        connectAndGreet()
        transport.deliver(.videoFrame(VideoFrame(seq: 0, isKeyframe: true, ptsMicros: 1, data: Data([1]))))
        guard case .protocolError? = events.ended.first else { return XCTFail("\(events.ended)") }
    }

    func testPingEverySecondAndLivenessTimeout() {
        streamAfterAuth()
        func pings() -> Int { transport.sent.filter { if case .ping = $0 { return true } else { return false } }.count }
        XCTAssertEqual(pings(), 1)
        scheduler.advance(by: 1.05)
        XCTAssertEqual(pings(), 2)
        // A PONG keeps the connection alive and measures the round trip.
        guard case .ping(let t)? = transport.sent.last else { return XCTFail() }
        scheduler.advance(by: 0.02)
        transport.deliver(.pong(t))
        XCTAssertEqual(client.isStreaming, true)
        scheduler.advance(by: 2.9)
        XCTAssertTrue(events.ended.isEmpty)
        scheduler.advance(by: 0.3)
        XCTAssertEqual(events.ended, [.connectionLost])
        XCTAssertTrue(transport.aborted)
    }

    func testRoundTripIsMeasured() {
        let counters = StreamCounters()
        pairings.save(storedPairing())
        makeClient(counters: counters)
        client.start()
        connector.succeed(transport, fingerprint: fingerprint)
        transport.deliver(.serverHello(serverHello))
        transport.deliver(.authResult(AuthResult(ok: true)))
        guard case .ping(let t)? = transport.sent.last else { return XCTFail("\(transport.sent)") }
        scheduler.advance(by: 0.025)
        transport.deliver(.pong(t))
        XCTAssertEqual(Double(counters.rttMicros), 25_000, accuracy: 1)
    }

    func testTheComputerMustAnswerWithinTheResponseTimeout() {
        makeClient()
        client.start()
        connector.succeed(transport, fingerprint: fingerprint)
        scheduler.advance(by: 9.9)
        XCTAssertTrue(events.ended.isEmpty)
        scheduler.advance(by: 0.3)
        XCTAssertEqual(events.ended, [.connectFailed("the computer did not answer")])
    }

    func testNoTimeoutWhileTheUserTypesTheCode() {
        connectAndGreet()
        transport.deliver(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
        scheduler.advance(by: 60)
        XCTAssertTrue(events.ended.isEmpty)
    }

    func testByeEndsAndTransportLossIsConnectionLostOnlyWhileStreaming() {
        streamAfterAuth()
        transport.deliver(.bye(Bye(reason: .replaced)))
        XCTAssertEqual(events.ended, [.serverEnded("replaced")])

        setUp()
        streamAfterAuth()
        transport.onClose?(nil)
        XCTAssertEqual(events.ended, [.connectionLost])

        setUp()
        connectAndGreet()
        transport.onClose?(nil)
        XCTAssertEqual(events.ended, [.connectFailed("the computer closed the connection")])
    }

    func testCursorAndMicControlOnlyWhileStreaming() {
        connectAndGreet()
        transport.deliver(.micControl(.start))
        XCTAssertTrue(events.micControls.isEmpty)
        setUp()
        streamAfterAuth()
        transport.deliver(.micControl(.start))
        transport.deliver(.cursor(CursorPosition(x: 0.5, y: 0.25)))
        XCTAssertEqual(events.micControls, [.start])
        XCTAssertEqual(events.cursors, [CursorPosition(x: 0.5, y: 0.25)])
    }

    func testUnknownMessageTypesAreSkipped() {
        streamAfterAuth()
        transport.onFrame?(Frame(type: 0xEE, payload: Data([1, 2, 3])))
        XCTAssertTrue(events.ended.isEmpty)
        transport.onFrame?(Frame(type: MessageType.authResult.rawValue, payload: Data("{".utf8)))
        guard case .protocolError? = events.ended.first else { return XCTFail("\(events.ended)") }
    }

    func testInputDroppedUnlessStreaming() {
        connectAndGreet()
        transport.clearSent()
        client.sendTouch(TouchEvent(eventTimeMs: 1, action: .down, actionPointerId: 0, pointers: [TouchPointer(id: 0, x: 0, y: 0)]))
        client.sendText("a")
        client.sendKey(KeyEvent(.returnKey))
        client.sendInputMode(.touch)
        client.sendMicState(MicState(capturing: true))
        client.sendAudio(AudioPacket(seq: 0, samples: [1]))
        client.requestKeyframe()
        XCTAssertTrue(transport.sent.isEmpty)
    }

    func testKeyboardTouchAndKeyframeWhileStreaming() {
        streamAfterAuth()
        transport.clearSent()
        let touch = TouchEvent(eventTimeMs: 1, action: .down, actionPointerId: 0, pointers: [TouchPointer(id: 0, x: 0.5, y: 0.5)])
        client.sendTouch(touch)
        client.sendText(String(repeating: "é", count: 2_049))   // 4098 bytes: two TEXT messages
        client.sendKey(KeyEvent(.left, modifiers: .shift))
        client.requestKeyframe()
        client.sendInputMode(.touch)
        XCTAssertEqual(transport.sent, [
            .touch(touch), .text(String(repeating: "é", count: 2_048)), .text("é"),
            .key(KeyEvent(.left, modifiers: .shift)), .keyframeRequest, .inputMode(InputModeMessage(.touch)),
        ])
    }

    func testAudioIsDroppedWhenTheNetworkFallsBehind() {
        let counters = StreamCounters()
        pairings.save(storedPairing())
        makeClient(counters: counters)
        client.start()
        connector.succeed(transport, fingerprint: fingerprint)
        transport.deliver(.serverHello(serverHello))
        transport.deliver(.authResult(AuthResult(ok: true)))
        transport.clearSent()
        let packet = AudioPacket(seq: 0, samples: [Int16](repeating: 1, count: 480))
        client.sendAudio(packet)
        transport.pendingSendBytes = StreamClient.audioBacklogLimitBytes + 1
        client.sendAudio(packet)
        XCTAssertEqual(transport.sent, [.audio(packet)])
        XCTAssertEqual(counters.audioDropped, 1)
    }
}

/// The reconnect loop around StreamClients (PROTOCOL §6 "Liveness").
final class DisplaySessionTests: XCTestCase {
    private var scheduler: ManualScheduler!
    private var pairings: InMemoryPairingRepository!
    private var connectors: [FakeConnector] = []
    private var transports: [FakeTransport] = []
    private var expected: [String?] = []
    private var session: DisplaySession!
    private var states: [DisplaySession.State] = []
    private let fingerprint = Data(repeating: 7, count: 32)

    override func setUp() {
        scheduler = ManualScheduler()
        pairings = InMemoryPairingRepository()
        connectors = []
        transports = []
        expected = []
        states = []
        session = DisplaySession(name: "127.0.0.1", serverId: nil, pairings: pairings, scheduler: scheduler) {
            [unowned self] serverId, counters, inputMode in
            self.expected.append(serverId)
            let connector = FakeConnector()
            self.connectors.append(connector)
            self.transports.append(FakeTransport())
            return StreamClient(connector: connector,
                                hello: Hello(deviceId: "d", deviceName: "iPhone",
                                             screen: ScreenInfo(width: 2622, height: 1206, densityDpi: 480, refreshRate: 120),
                                             codecs: []),
                                pairings: self.pairings, endpoint: .init(host: "127.0.0.1", port: 47897),
                                scheduler: self.scheduler, clock: self.scheduler, random: FixedRandom(), counters: counters,
                                inputMode: inputMode)
        }
        session.onState = { [unowned self] in self.states.append($0) }
    }

    private func pairAndStream(_ i: Int) {
        connectors[i].succeed(transports[i], fingerprint: fingerprint)
        transports[i].deliver(.serverHello(ServerHello(serverId: "srv", serverName: "Studio")))
        if pairings.pairing(serverId: "srv") == nil {
            transports[i].deliver(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
            XCTAssertTrue(session.submitCode("123456"))
            transports[i].deliver(.authResult(AuthResult(ok: true, token: Data([1, 2, 3]))))
        } else {
            transports[i].deliver(.authResult(AuthResult(ok: true)))
        }
    }

    func testPairsStreamsAndReconnectsWithBackoffAfterTheLinkDies() {
        session.start()
        pairAndStream(0)
        XCTAssertEqual(session.state, .streaming(name: "Studio"))
        XCTAssertEqual(states.map { "\($0)" }.count, 5)   // connecting, pairing, awaitingCode, verifying, streaming
        transports[0].onClose?(nil)
        XCTAssertEqual(session.state, .reconnecting(name: "Studio", cause: .connectionLost, attempt: 1))
        XCTAssertEqual(connectors.count, 1)
        scheduler.advance(by: 1)
        XCTAssertEqual(connectors.count, 2)
        XCTAssertEqual(expected, [nil, "srv"])   // the second attempt pins the paired computer
        connectors[1].fail(.failed("unreachable"))
        XCTAssertEqual(session.state, .reconnecting(name: "Studio", cause: .connectionLost, attempt: 2))
        scheduler.advance(by: 1.9)
        XCTAssertEqual(connectors.count, 2)
        scheduler.advance(by: 0.1)
        XCTAssertEqual(connectors.count, 3)
        pairAndStream(2)
        XCTAssertEqual(session.state, .streaming(name: "Studio"))
    }

    func testTerminalErrorsWaitForTheUser() {
        session.start()
        pairAndStream(0)
        transports[0].deliver(.bye(Bye(reason: .replaced)))
        XCTAssertEqual(session.state, .failed(name: "Studio", error: .replaced))
        scheduler.advance(by: 30)
        XCTAssertEqual(connectors.count, 1)
        session.retryNow()
        XCTAssertEqual(connectors.count, 2)
        XCTAssertEqual(session.state, .connecting(name: "Studio"))
    }

    func testUnknownDevicePairsAgainImmediately() {
        pairings.save(PairedComputer(serverId: "srv", serverName: "Studio", fingerprint: fingerprint, token: Data([9]),
                                     lastHost: "127.0.0.1", lastPort: 47897))
        session.start()
        connectors[0].succeed(transports[0], fingerprint: fingerprint)
        transports[0].deliver(.serverHello(ServerHello(serverId: "srv", serverName: "Studio")))
        transports[0].deliver(.authResult(AuthResult(ok: false, reason: .unknownDevice)))
        XCTAssertNil(pairings.pairing(serverId: "srv"))
        XCTAssertEqual(connectors.count, 2)
        XCTAssertEqual(session.state, .reconnecting(name: "127.0.0.1", cause: .repair, attempt: 0))
    }

    func testIdentityChangeThenPairAgainForgetsThePin() {
        pairings.save(PairedComputer(serverId: "srv", serverName: "Studio", fingerprint: Data(repeating: 1, count: 32),
                                     token: Data([9]), lastHost: "127.0.0.1", lastPort: 47897))
        session.start()
        connectors[0].succeed(transports[0], fingerprint: fingerprint)
        transports[0].deliver(.serverHello(ServerHello(serverId: "srv", serverName: "Studio")))
        XCTAssertEqual(session.state, .failed(name: "127.0.0.1", error: .identityChanged))
        session.forgetAndRepair()
        XCTAssertNil(pairings.pairing(serverId: "srv"))
        pairAndStream(1)
        XCTAssertEqual(session.state, .streaming(name: "Studio"))
    }

    func testPauseAndResume() {
        session.start()
        pairAndStream(0)
        session.pause()
        XCTAssertEqual(session.state, .paused)
        XCTAssertTrue(transports[0].aborted)
        session.resume()
        XCTAssertEqual(connectors.count, 2)
        pairAndStream(1)
        XCTAssertEqual(session.state, .streaming(name: "Studio"))
        session.stop()
        XCTAssertEqual(session.state, .stopped)
        session.resume()
        XCTAssertEqual(connectors.count, 2)
    }

    func testMicTrafficGoesToTheStreamThatAskedAndEndsWithIt() {
        var controls: [MicControl] = []
        var ended = 0
        session.onMicControl = { controls.append($0) }
        session.onMicSessionEnded = { ended += 1 }
        session.start()
        pairAndStream(0)
        transports[0].deliver(.micControl(.start))
        XCTAssertEqual(controls, [.start])
        transports[0].clearSent()
        session.sendMicState(MicState(capturing: true))
        XCTAssertEqual(transports[0].sent, [.micState(MicState(capturing: true))])
        transports[0].onClose?(nil)
        XCTAssertEqual(ended, 1)
        session.sendMicState(MicState(capturing: false, reason: .stopped))
        XCTAssertEqual(transports[0].sent.count, 1)
    }

    func testInputModeIsAnnouncedOnEveryConnection() {
        session.setInputMode(.trackpad)
        session.start()
        pairAndStream(0)
        XCTAssertTrue(transports[0].sent.contains(.inputMode(InputModeMessage(.trackpad))))
        transports[0].clearSent()
        session.setInputMode(.touch)
        XCTAssertEqual(transports[0].sent, [.inputMode(InputModeMessage(.touch))])
    }

    func testBackoffSchedule() {
        XCTAssertEqual((1...6).map(ReconnectBackoff.delay(failures:)), [1, 2, 4, 5, 5, 5])
    }

    func testPolicy() {
        XCTAssertEqual(SessionPolicy.next(after: .closedByUser, everStreamed: true), .stop)
        XCTAssertEqual(SessionPolicy.next(after: .connectFailed("x"), everStreamed: false), .retry(.unreachable, immediate: false))
        XCTAssertEqual(SessionPolicy.next(after: .connectFailed("x"), everStreamed: true), .retry(.connectionLost, immediate: false))
        XCTAssertEqual(SessionPolicy.next(after: .serverEnded("server_shutdown"), everStreamed: true),
                       .retry(.serverStopped, immediate: false))
        XCTAssertEqual(SessionPolicy.next(after: .serverEnded("permission_missing"), everStreamed: true),
                       .fail(.permissionMissing))
        XCTAssertEqual(SessionPolicy.next(after: .serverEnded("weird"), everStreamed: true), .fail(.rejected("weird")))
    }
}
