import CryptoKit
import XCTest
@testable import SidecastKit

/// PROTOCOL.md §8 test vectors, reproduced exactly.
final class ProtocolVectorTests: XCTestCase {
    private func hex(_ s: String) -> Data {
        Data(hexString: s.replacingOccurrences(of: " ", with: ""))!
    }

    // §8.1
    func testPairingProofVector() {
        let fingerprint = Data(SHA256.hash(data: Data("sidecast-test-cert".utf8)))
        XCTAssertEqual(fingerprint.hexString, "004ac6c710efca069afd7657c7262b316b18e1595f710ea8a5d2289d04b9d196")
        let nonce = hex("000102030405060708090a0b0c0d0e0f")
        XCTAssertEqual(nonce.base64EncodedString(), "AAECAwQFBgcICQoLDA0ODw==")

        let proof = PairingProofCalculator.proof(code: "123456", certFingerprint: fingerprint, nonce: nonce)
        XCTAssertEqual(proof.hexString, "e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe")
        XCTAssertEqual(proof.base64EncodedString(), "40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=")

        // The server-side state machine accepts exactly this proof.
        var session = PairingSession(code: PairingCode("123456")!, startedAt: 0)
        XCTAssertEqual(session.verify(nonce: nonce, proof: proof, certFingerprint: fingerprint, at: 1), .success)
    }

    func testPairProofJSONCarriesVectorBase64() throws {
        let msg = ClientMessage.pairProof(PairProof(nonce: hex("000102030405060708090a0b0c0d0e0f"),
                                                    proof: Data(base64Encoded: "40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=")!))
        let json = String(decoding: msg.frame().payload, as: UTF8.self)
        XCTAssertTrue(json.contains("\"nonce\":\"AAECAwQFBgcICQoLDA0ODw==\""), json)
        XCTAssertTrue(json.contains("\"proof\":\"40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=\""), json)
    }

    // §8.2
    func testTokenHashVector() {
        let token = Data(repeating: 0xAA, count: 32)
        XCTAssertEqual(DeviceToken.hash(token).hexString,
                       "e0e77a507412b120f6ede61f62295b1a7b2ff19d3dcc8f7253e51663470c888e")
    }

    // §8.3 FRAME_ACK seq = 258
    func testFrameAckVector() throws {
        let wire = hex("11 00 00 00 04 00 00 01 02")
        XCTAssertEqual(ClientMessage.frameAck(seq: 258).encoded(), wire)
        XCTAssertEqual(try decodeClient(wire), .frameAck(seq: 258))
    }

    // §8.3 TOUCH
    func testTouchVector() throws {
        let wire = hex("10 00 00 00 10  00 00 03 e8  02  00  01  00  3f 00 00 00  3e 80 00 00")
        let event = TouchEvent(eventTimeMs: 1000, action: .move, actionPointerId: 0,
                               pointers: [TouchPointer(id: 0, x: 0.5, y: 0.25)])
        XCTAssertEqual(ClientMessage.touch(event).encoded(), wire)
        XCTAssertEqual(try decodeClient(wire), .touch(event))
    }

    // §8.3 VIDEO_FRAME
    func testVideoFrameVector() throws {
        let wire = hex("85 00 00 00 12  00 00 00 07  01  00 00 00 00 00 0f 42 40  00 00 00 01 40")
        let frame = VideoFrame(seq: 7, isKeyframe: true, ptsMicros: 1_000_000, data: hex("00 00 00 01 40"))
        XCTAssertEqual(ServerMessage.videoFrame(frame).encoded(), wire)
        XCTAssertEqual(try decodeServer(wire), .videoFrame(frame))
    }

    // §8.3 TEXT "é"
    func testTextVector() throws {
        let wire = hex("14 00 00 00 02 c3 a9")
        XCTAssertEqual(ClientMessage.text("é").encoded(), wire)
        XCTAssertEqual(try decodeClient(wire), .text("é"))
    }

    // §8.3 KEY Return (1), no modifiers; then KEY Left (6) with Shift
    func testKeyVectors() throws {
        let returnWire = hex("15 00 00 00 02 01 00")
        XCTAssertEqual(ClientMessage.key(KeyEvent(.returnKey)).encoded(), returnWire)
        XCTAssertEqual(try decodeClient(returnWire), .key(KeyEvent(key: 1, modifiers: 0)))

        let shiftLeftWire = hex("15 00 00 00 02 06 01")
        XCTAssertEqual(ClientMessage.key(KeyEvent(.left, modifiers: .shift)).encoded(), shiftLeftWire)
        XCTAssertEqual(try decodeClient(shiftLeftWire), .key(KeyEvent(key: 6, modifiers: 1)))

        // Both back to back in one stream, as the vector lists them.
        let parser = FrameParser()
        parser.append(returnWire + shiftLeftWire)
        let frames = try parser.drain()
        XCTAssertEqual(try frames.map { try ClientMessage.decode($0) },
                       [.key(KeyEvent(.returnKey)), .key(KeyEvent(.left, modifiers: .shift))])
    }

    // §8.3 AUDIO seq = 1, two samples [1, -2]
    func testAudioVector() throws {
        let wire = hex("16 00 00 00 08 00 00 00 01 01 00 fe ff")
        let packet = AudioPacket(seq: 1, samples: [1, -2])
        XCTAssertEqual(ClientMessage.audio(packet).encoded(), wire)
        XCTAssertEqual(try decodeClient(wire), .audio(packet))
    }

    // §8.3 PING t = 1
    func testPingVector() throws {
        let wire = hex("13 00 00 00 08 00 00 00 00 00 00 00 01")
        XCTAssertEqual(ClientMessage.ping(1).encoded(), wire)
        XCTAssertEqual(try decodeClient(wire), .ping(1))
        // PONG echoes the same 8 bytes.
        XCTAssertEqual(ServerMessage.pong(1).encoded().dropFirst(5), wire.dropFirst(5))
    }

    // §8.4 SERVER_HELLO platform
    func testServerHelloPlatformVectors() throws {
        func platform(_ json: String) throws -> HostPlatform? {
            guard case .serverHello(let hello)? = try ServerMessage.decode(Frame(.serverHello, payload: Data(json.utf8))) else {
                throw NSError(domain: "test", code: 1)
            }
            return hello.hostPlatform
        }
        XCTAssertEqual(try platform(#"{"protocol":1,"serverId":"S","serverName":"Studio","platform":"macos"}"#), .macos)
        XCTAssertEqual(try platform(#"{"protocol":1,"serverId":"S","serverName":"Studio","platform":"windows"}"#), .windows)
        XCTAssertEqual(try platform(#"{"protocol":1,"serverId":"S","serverName":"Studio","platform":"linux"}"#), .linux)
        XCTAssertEqual(try platform(#"{"protocol":1,"serverId":"S","serverName":"Studio"}"#), .macos)
        XCTAssertNil(try platform(#"{"protocol":1,"serverId":"S","serverName":"Studio","platform":"beos"}"#))
    }

    private func decodeClient(_ wire: Data) throws -> ClientMessage? {
        let parser = FrameParser()
        parser.append(wire)
        let frame = try XCTUnwrap(try parser.nextFrame())
        XCTAssertEqual(parser.bufferedByteCount, 0)
        return try ClientMessage.decode(frame)
    }

    private func decodeServer(_ wire: Data) throws -> ServerMessage? {
        let parser = FrameParser()
        parser.append(wire)
        return try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame()))
    }
}
