import XCTest
@testable import SidecastKit

final class MessageTests: XCTestCase {
    private let sampleHello = """
    {
      "protocol": 1,
      "deviceId": "8c1f0000-1111-2222-3333-444455556666",
      "deviceName": "Pixel 8",
      "screen": { "width": 2400, "height": 1080, "densityDpi": 420, "refreshRate": 120.0 },
      "codecs": [
        { "name": "hevc", "maxWidth": 3840, "maxHeight": 2160 },
        { "name": "h264", "maxWidth": 1920, "maxHeight": 1080 }
      ],
      "futureField": {"anything": [1, 2, 3]}
    }
    """

    func testHelloDecodesAndIgnoresUnknownKeys() throws {
        let msg = try ClientMessage.decode(Frame(.hello, payload: Data(sampleHello.utf8)))
        guard case .hello(let hello)? = msg else { return XCTFail("expected HELLO") }
        XCTAssertEqual(hello.deviceName, "Pixel 8")
        XCTAssertEqual(hello.screen, ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 120))
        XCTAssertEqual(hello.codecs.map(\.name), ["hevc", "h264"])
    }

    /// Android's org.json writer escapes "/" as "\/" and writes whole floats as ints.
    func testHelloFromAndroidJSONWriter() throws {
        let android = #"{"protocol":1,"deviceId":"a\/b-1","deviceName":"Galaxy S24 \/ work","#
            + #""screen":{"width":2340,"height":1080,"densityDpi":450,"refreshRate":120},"#
            + #""codecs":[{"name":"hevc","maxWidth":3840,"maxHeight":2160}]}"#
        guard case .hello(let hello)? = try ClientMessage.decode(Frame(.hello, payload: Data(android.utf8))) else {
            return XCTFail("expected HELLO")
        }
        XCTAssertEqual(hello.deviceId, "a/b-1")
        XCTAssertEqual(hello.deviceName, "Galaxy S24 / work")
        XCTAssertEqual(hello.screen.refreshRate, 120)
        XCTAssertEqual(hello.codecs, [CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160)])
    }

    func testHelloWithoutOptionalKeysUsesDefaults() throws {
        let minimal = #"{"protocol":1,"deviceId":"d","screen":{"width":1920,"height":1080}}"#
        guard case .hello(let hello)? = try ClientMessage.decode(Frame(.hello, payload: Data(minimal.utf8))) else {
            return XCTFail("expected HELLO")
        }
        XCTAssertEqual(hello.deviceName, "Android phone")
        XCTAssertEqual(hello.screen, ScreenInfo(width: 1920, height: 1080, densityDpi: 160, refreshRate: 60))
        XCTAssertEqual(hello.codecs, [])
        // deviceId and screen stay mandatory.
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.hello, payload: Data(#"{"protocol":1,"deviceId":"d"}"#.utf8))))
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.hello, payload: Data(#"{"protocol":1,"screen":{"width":1,"height":1}}"#.utf8))))
    }

    func testPairProofAndAuthWithEscapedSlashes() throws {
        let proof = #"{"nonce":"AAECAwQFBgcICQoLDA0ODw==","proof":"40sw5wf5dNH6Tj5a\/cUjw\/yaOatMfUP0KvJ9ywQkSv4="}"#
        guard case .pairProof(let p)? = try ClientMessage.decode(Frame(.pairProof, payload: Data(proof.utf8))) else {
            return XCTFail("expected PAIR_PROOF")
        }
        XCTAssertEqual(p.proof.data.hexString, "e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe")
    }

    func testHelloVersionMismatchIsDetectedEvenForOtherShapes() {
        let v2 = Data(#"{"protocol": 2, "somethingNew": true}"#.utf8)
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.hello, payload: v2))) { error in
            XCTAssertEqual(error as? HelloError, .versionMismatch(2))
        }
    }

    func testMalformedJSONThrows() {
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.auth, payload: Data("{".utf8))))
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.auth, payload: Data(#"{"deviceId":"x"}"#.utf8))))
    }

    func testAuthTokenBase64ToleratesLineBreaks() throws {
        let token = Data(repeating: 0xAB, count: 32)
        let wrapped = token.base64EncodedString() + "\n"
        let json = #"{"deviceId":"d1","token":"\#(wrapped.replacingOccurrences(of: "\n", with: "\\n"))"}"#
        let msg = try ClientMessage.decode(Frame(.auth, payload: Data(json.utf8)))
        XCTAssertEqual(msg, .auth(AuthRequest(deviceId: "d1", token: token)))
    }

    func testServerJSONShapes() throws {
        func json(_ m: ServerMessage) -> [String: Any] {
            let wire = m.encoded()
            return try! JSONSerialization.jsonObject(with: wire.dropFirst(5)) as! [String: Any]
        }
        let hello = json(.serverHello(ServerHello(serverId: "S", serverName: "Mac")))
        XCTAssertEqual(hello["protocol"] as? Int, 1)
        XCTAssertEqual(hello["serverId"] as? String, "S")
        XCTAssertEqual(hello["serverName"] as? String, "Mac")

        let fail = json(.authResult(AuthResult(ok: false, reason: .badCode, attemptsLeft: 4)))
        XCTAssertEqual(fail["ok"] as? Bool, false)
        XCTAssertEqual(fail["reason"] as? String, "bad_code")
        XCTAssertEqual(fail["attemptsLeft"] as? Int, 4)
        XCTAssertNil(fail["token"])

        let ok = json(.authResult(AuthResult(ok: true, token: Data(repeating: 0xFF, count: 32))))
        XCTAssertEqual(ok.keys.sorted(), ["ok", "token"])
        XCTAssertEqual(ok["token"] as? String, Data(repeating: 0xFF, count: 32).base64EncodedString())

        let cfg = json(.streamConfig(StreamConfig(codec: "hevc", width: 2400, height: 1080, fps: 60)))
        XCTAssertEqual(cfg["codec"] as? String, "hevc")
        XCTAssertEqual(cfg["width"] as? Int, 2400)

        XCTAssertEqual(json(.bye(Bye(reason: .permissionMissing)))["reason"] as? String, "permission_missing")
        let challenge = json(.pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)))
        XCTAssertEqual(challenge["expiresInSec"] as? Int, 120)
        XCTAssertEqual(challenge["attemptsLeft"] as? Int, 5)
    }

    func testRoundTripAllServerMessages() throws {
        let all: [ServerMessage] = [
            .serverHello(ServerHello(serverId: "id", serverName: "n")),
            .pairChallenge(PairChallenge(expiresInSec: 120, attemptsLeft: 5)),
            .authResult(AuthResult(ok: true, token: Data([1, 2, 3]))),
            .streamConfig(StreamConfig(codec: "h264", width: 1920, height: 864, fps: 60)),
            .videoFrame(VideoFrame(seq: 0, isKeyframe: false, ptsMicros: 0, data: Data())),
            .pong(.max),
            .bye(Bye(reason: .replaced)),
        ]
        for m in all {
            let parser = FrameParser()
            parser.append(m.encoded())
            XCTAssertEqual(try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame())), m)
        }
    }

    func testTouchMultiPointerRoundTripAndFloatByteOrder() throws {
        let event = TouchEvent(eventTimeMs: .max, action: .pointerDown, actionPointerId: 1,
                               pointers: [TouchPointer(id: 0, x: -0.01, y: 1.02), TouchPointer(id: 1, x: 0.75, y: 0.5)])
        let payload = event.encodedPayload()
        XCTAssertEqual(payload.count, 7 + 2 * 9)
        // 0.75 = 0x3F400000 big-endian
        XCTAssertEqual(Array(payload[17..<21]), [0x3F, 0x40, 0x00, 0x00])
        XCTAssertEqual(try TouchEvent.decode(payload), event)
    }

    func testTouchPointerCountOutOfRangeIsRejected() {
        var w = ByteWriter()
        w.u32(1); w.u8(0); w.u8(0); w.u8(0)
        XCTAssertThrowsError(try TouchEvent.decode(w.data))
    }
}
