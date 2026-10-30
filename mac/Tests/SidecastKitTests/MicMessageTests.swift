import XCTest
@testable import SidecastKit

/// PROTOCOL §7.2 messages: AUDIO, MIC_STATE, MIC_CONTROL and HELLO `features`.
final class MicMessageTests: XCTestCase {
    private func decodeAudio(_ payload: [UInt8]) throws -> ClientMessage? {
        try ClientMessage.decode(Frame(.audio, payload: Data(payload)))
    }

    private func assertMalformed(_ payload: [UInt8], file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try decodeAudio(payload), file: file, line: line) { error in
            switch error as? ProtocolError {
            case .malformed(.audio, _)?, .truncated(.audio)?: break
            default: XCTFail("\(error)", file: file, line: line)
            }
        }
    }

    // MARK: - AUDIO

    func testAudioSamplesAreLittleEndianAndSeqBigEndian() throws {
        let packet = AudioPacket(seq: 0x0102_0304, samples: [.min, .max, 0, 0x1234])
        let payload = packet.encodedPayload()
        XCTAssertEqual(Array(payload), [0x01, 0x02, 0x03, 0x04, 0x00, 0x80, 0xFF, 0x7F, 0x00, 0x00, 0x34, 0x12])
        XCTAssertEqual(try AudioPacket.decode(payload), packet)
    }

    func testAudioAcceptsOneToMaxSamples() throws {
        for count in [1, 480, 4800] {
            let packet = AudioPacket(seq: 7, samples: [Int16](repeating: -3, count: count))
            XCTAssertEqual(try ClientMessage.decode(ClientMessage.audio(packet).frame()), .audio(packet), "\(count)")
        }
    }

    func testOddByteCountIsMalformed() {
        assertMalformed([0, 0, 0, 1, 0x01])
        assertMalformed([0, 0, 0, 1, 0x01, 0x00, 0xFE])
    }

    func testZeroSamplesIsMalformed() {
        assertMalformed([0, 0, 0, 1])
    }

    func testMoreThan4800SamplesIsMalformed() {
        assertMalformed([0, 0, 0, 1] + [UInt8](repeating: 0, count: 4801 * 2))
    }

    func testShorterThanTheSeqIsMalformed() {
        assertMalformed([])
        assertMalformed([0, 0, 1])
    }

    // MARK: - MIC_STATE

    func testMicStateDecoding() throws {
        func decode(_ json: String) throws -> ClientMessage? {
            try ClientMessage.decode(Frame(.micState, payload: Data(json.utf8)))
        }
        XCTAssertEqual(try decode(#"{"capturing":true}"#), .micState(MicState(capturing: true)))
        for reason in MicStateReason.allCases {
            guard case .micState(let state)? = try decode(#"{"capturing":false,"reason":"\#(reason.rawValue)"}"#) else {
                return XCTFail("expected MIC_STATE")
            }
            XCTAssertFalse(state.capturing)
            XCTAssertEqual(state.knownReason, reason)
        }
        // Unknown reasons and keys are kept / ignored, not errors.
        guard case .micState(let future)? = try decode(#"{"capturing":false,"reason":"thermal","x":1}"#) else {
            return XCTFail("expected MIC_STATE")
        }
        XCTAssertEqual(future.reason, "thermal")
        XCTAssertNil(future.knownReason)
        XCTAssertEqual(MicStateReason.permissionDenied.rawValue, "permission_denied")
        // Malformed JSON.
        XCTAssertThrowsError(try decode(#"{"reason":"stopped"}"#))
        XCTAssertThrowsError(try decode("{"))
    }

    func testMicStateRoundTrip() throws {
        let state = MicState(capturing: false, reason: .background)
        XCTAssertEqual(try ClientMessage.decode(ClientMessage.micState(state).frame()), .micState(state))
    }

    // MARK: - MIC_CONTROL

    func testMicControlJSONShapes() throws {
        func json(_ m: MicControl) -> String {
            String(decoding: ServerMessage.micControl(m).encoded().dropFirst(5), as: UTF8.self)
        }
        XCTAssertEqual(json(.start), #"{"active":true,"channels":1,"sampleRate":48000}"#)
        XCTAssertEqual(json(.stop), #"{"active":false}"#)
        XCTAssertEqual(ServerMessage.micControl(.start).encoded().first, 0x88)
    }

    func testMicControlRoundTripAndLenientDecoding() throws {
        for m in [MicControl.start, .stop] {
            let parser = FrameParser()
            parser.append(ServerMessage.micControl(m).encoded())
            XCTAssertEqual(try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame())), .micControl(m))
        }
        let minimal = try ServerMessage.decode(Frame(.micControl, payload: Data(#"{"active":true}"#.utf8)))
        XCTAssertEqual(minimal, .micControl(MicControl(active: true)))
    }

    // MARK: - HELLO features

    func testHelloFeatures() throws {
        let base = #"{"protocol":1,"deviceId":"d","screen":{"width":2400,"height":1080}"#
        guard case .hello(let none)? = try ClientMessage.decode(Frame(.hello, payload: Data((base + "}").utf8))),
              case .hello(let mic)? = try ClientMessage.decode(Frame(.hello, payload: Data((base + #","features":["mic","later"]}"#).utf8)))
        else { return XCTFail("expected HELLO") }
        XCTAssertEqual(none.features, [])
        XCTAssertFalse(none.supportsMic)
        XCTAssertEqual(mic.features, ["mic", "later"])
        XCTAssertTrue(mic.supportsMic)
    }

    func testHelloEncodesFeaturesOnlyWhenPresent() throws {
        let screen = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 60)
        let plain = Hello(deviceId: "d", deviceName: "n", screen: screen, codecs: [])
        let withMic = Hello(deviceId: "d", deviceName: "n", screen: screen, codecs: [], features: [ClientFeature.mic])
        let plainJSON = String(decoding: ClientMessage.hello(plain).frame().payload, as: UTF8.self)
        let micJSON = String(decoding: ClientMessage.hello(withMic).frame().payload, as: UTF8.self)
        XCTAssertFalse(plainJSON.contains("features"), plainJSON)
        XCTAssertTrue(micJSON.contains(#""features":["mic"]"#), micJSON)
        XCTAssertEqual(try ClientMessage.decode(ClientMessage.hello(withMic).frame()), .hello(withMic))
    }

    // MARK: - Admin commands

    func testInstallAndRemoveCommands() {
        let install = MicDriverCommands.installCommand(bundledDriver: "/Applications/Side cast's.app/Contents/Resources/SidecastMic.driver")
        XCTAssertEqual(install,
            "/bin/mkdir -p '/Library/Audio/Plug-Ins/HAL'"
            + " && /bin/rm -rf '/Library/Audio/Plug-Ins/HAL/SidecastMic.driver'"
            + " && /usr/bin/ditto '/Applications/Side cast'\\''s.app/Contents/Resources/SidecastMic.driver'"
            + " '/Library/Audio/Plug-Ins/HAL/SidecastMic.driver'"
            + " && /usr/sbin/chown -R root:wheel '/Library/Audio/Plug-Ins/HAL/SidecastMic.driver'"
            + " && (/bin/launchctl kickstart -k system/com.apple.audio.coreaudiod || /usr/bin/killall coreaudiod)")
        XCTAssertEqual(MicDriverCommands.removeCommand,
            "/bin/rm -rf '/Library/Audio/Plug-Ins/HAL/SidecastMic.driver'"
            + " && (/bin/launchctl kickstart -k system/com.apple.audio.coreaudiod || /usr/bin/killall coreaudiod)")
        let script = SudoersRule.appleScript(adminCommand: "/bin/echo \"hi\"", prompt: "Install “it”")
        XCTAssertEqual(script, #"do shell script "/bin/echo \"hi\"" with prompt "Install “it”" with administrator privileges"#)
        XCTAssertEqual(SudoersRule.appleScript(adminCommand: "x"), #"do shell script "x" with administrator privileges"#)
    }

    func testDriverStatus() {
        XCTAssertEqual(MicDriverCommands.status(installedVersion: nil, bundledVersion: "1.0.0"), .notInstalled)
        XCTAssertEqual(MicDriverCommands.status(installedVersion: nil, bundledVersion: nil), .notBundled)
        XCTAssertEqual(MicDriverCommands.status(installedVersion: "1.0.0", bundledVersion: "1.0.0"), .installed(version: "1.0.0"))
        XCTAssertEqual(MicDriverCommands.status(installedVersion: "1.0.0", bundledVersion: "1.0.1"),
                       .updateAvailable(installed: "1.0.0", bundled: "1.0.1"))
        XCTAssertEqual(MicDriverCommands.status(installedVersion: "1.9", bundledVersion: "1.10"),
                       .updateAvailable(installed: "1.9", bundled: "1.10"))
        // A newer installed driver (from a newer Sidecast) is not "downgrade available".
        XCTAssertEqual(MicDriverCommands.status(installedVersion: "2.0", bundledVersion: "1.0"), .installed(version: "2.0"))
        XCTAssertEqual(MicDriverCommands.status(installedVersion: "1.0", bundledVersion: nil), .installed(version: "1.0"))
    }
}
