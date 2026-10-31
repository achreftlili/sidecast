import XCTest
@testable import SidecastKit

/// SERVER_HELLO `platform` and Bonjour TXT `os` (PROTOCOL §1, §4).
final class HostPlatformTests: XCTestCase {
    private func decode(_ json: String) throws -> ServerHello {
        guard case .serverHello(let hello)? = try ServerMessage.decode(Frame(.serverHello, payload: Data(json.utf8))) else {
            throw NSError(domain: "test", code: 1)
        }
        return hello
    }

    func testThisHostAlwaysSendsMacos() throws {
        let wire = ServerMessage.serverHello(ServerHello(serverId: "S", serverName: "Studio")).encoded().dropFirst(5)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: wire) as? [String: Any])
        XCTAssertEqual(json["platform"] as? String, "macos")
    }

    func testDecodesEveryKnownPlatform() throws {
        for platform in HostPlatform.allCases {
            let hello = try decode(#"{"protocol":1,"serverId":"S","serverName":"n","platform":"\#(platform.rawValue)"}"#)
            XCTAssertEqual(hello.hostPlatform, platform)
            XCTAssertEqual(hello, ServerHello(serverId: "S", serverName: "n", platform: platform))
        }
    }

    func testAbsentMeansAnOlderMacHost() throws {
        let hello = try decode(#"{"protocol":1,"serverId":"S","serverName":"n"}"#)
        XCTAssertEqual(hello.platform, "macos")
        XCTAssertEqual(hello.hostPlatform, .macos)
    }

    func testUnknownIsKeptButHasNoKnownPlatform() throws {
        let hello = try decode(#"{"protocol":1,"serverId":"S","serverName":"n","platform":"haiku"}"#)
        XCTAssertEqual(hello.platform, "haiku")
        XCTAssertNil(hello.hostPlatform, "shown as a generic computer")
        // Round-trips unchanged.
        let parser = FrameParser()
        parser.append(ServerMessage.serverHello(hello).encoded())
        XCTAssertEqual(try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame())), .serverHello(hello))
    }

    func testMalformedPlatformIsIgnoredNotFatal() throws {
        XCTAssertEqual(try decode(#"{"protocol":1,"serverId":"S","serverName":"n","platform":42}"#).hostPlatform, .macos)
        XCTAssertEqual(try decode(#"{"protocol":1,"serverId":"S","serverName":"n","platform":null}"#).hostPlatform, .macos)
        // The required fields stay required.
        XCTAssertThrowsError(try decode(#"{"protocol":1,"serverName":"n","platform":"linux"}"#))
    }

    func testBonjourTXTRecord() {
        XCTAssertEqual(BonjourTXT.record(serverId: "S"), ["v": "1", "id": "S", "os": "macos"])
        XCTAssertEqual(BonjourTXT.record(serverId: "S", platform: .linux)["os"], "linux")
    }
}
