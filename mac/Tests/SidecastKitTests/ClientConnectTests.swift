import Foundation
import XCTest
@testable import SidecastKit

/// Client side of PROTOCOL §1 / §1.1: address classes, connect plans, the computer list.
final class ClientConnectTests: XCTestCase {
    private let paired = PairedComputer(
        serverId: "id", serverName: "Studio", fingerprint: Data([0]), token: Data([0xAA]),
        lastHost: "192.168.1.20", lastPort: 47800,
        addresses: [
            ServerAddress(host: "mac.tail1234.ts.net", kind: .tailscaleDNS),
            ServerAddress(host: "100.101.102.103", kind: .tailscale),
            ServerAddress(host: "192.168.1.20", kind: .lan),   // same as lastHost
            ServerAddress(host: "10.0.0.5", kind: .lan),
            ServerAddress(host: "fd7a:115c:a1e0::5", kind: .tailscale),
            ServerAddress(host: "203.0.113.9", rawKind: "public"),   // unknown kind: ignored
        ],
        serverPort: 47801)

    private func lan(_ host: String, _ port: Int) -> ConnectCandidate {
        ConnectCandidate(host: host, port: port, route: .lan, connectTimeout: ConnectPlanner.lanConnectTimeout)
    }

    private func tailscale(_ host: String, _ port: Int) -> ConnectCandidate {
        ConnectCandidate(host: host, port: port, route: .tailscale, connectTimeout: ConnectPlanner.remoteConnectTimeout)
    }

    // MARK: - AddressClass

    func testAddressClasses() {
        XCTAssertEqual(AddressClass.kind(ofHost: "127.0.0.1"), .loopback)
        XCTAssertEqual(AddressClass.kind(ofHost: "::1"), .loopback)
        XCTAssertEqual(AddressClass.kind(ofHost: "192.168.1.20"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "10.1.2.3"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "172.31.255.1"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "172.32.0.1"), .other)
        XCTAssertEqual(AddressClass.kind(ofHost: "169.254.10.1"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "100.64.0.1"), .tailscale)
        XCTAssertEqual(AddressClass.kind(ofHost: "100.127.255.254"), .tailscale)
        XCTAssertEqual(AddressClass.kind(ofHost: "100.128.0.1"), .other)
        XCTAssertEqual(AddressClass.kind(ofHost: "fd7a:115c:a1e0::5"), .tailscale)
        XCTAssertEqual(AddressClass.kind(ofHost: "fd00::1"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "[fe80::1%en0]"), .privateLAN)
        XCTAssertEqual(AddressClass.kind(ofHost: "2001:db8::1"), .other)
        XCTAssertEqual(AddressClass.kind(ofHost: "8.8.8.8"), .other)
        // Names are never resolved.
        XCTAssertNil(AddressClass.kind(ofHost: "studio.local"))
        XCTAssertNil(AddressClass.kind(ofHost: "1.2.3"))
        XCTAssertNil(AddressClass.kind(ofHost: "1.2.3.256"))
        XCTAssertNil(AddressClass.kind(ofHost: "1.2.3.4.5"))
        XCTAssertNil(AddressClass.kind(ofHost: "cafe"))
        XCTAssertTrue(AddressClass.isPrivateLAN("192.168.0.9"))
        XCTAssertFalse(AddressClass.isPrivateLAN("127.0.0.1"))
        XCTAssertFalse(AddressClass.isPrivateLAN("100.70.0.1"))
    }

    // MARK: - ConnectPlanner (ported from the Android ConnectPlannerTest)

    func testBonjourServiceIsTheOnlyCandidateWhenDiscovered() {
        XCTAssertEqual(ConnectPlanner.plan(host: "Studio", port: 47800, bonjourService: "Studio", paired: paired),
                       [ConnectCandidate(host: "Studio", port: 47800, route: .lan,
                                         connectTimeout: ConnectPlanner.lanConnectTimeout, isBonjourService: true)])
    }

    func testUnpairedComputerIsTriedAtThePickedAddressOnly() {
        XCTAssertEqual(ConnectPlanner.plan(host: "192.168.1.9", port: 47800, bonjourService: nil, paired: nil),
                       [lan("192.168.1.9", 47800)])
        XCTAssertEqual(ConnectPlanner.plan(host: "100.70.0.1", port: 47800, bonjourService: nil, paired: nil),
                       [tailscale("100.70.0.1", 47800)])
        XCTAssertEqual(ConnectPlanner.plan(host: "mac.example.com", port: 5000, bonjourService: nil, paired: nil),
                       [ConnectCandidate(host: "mac.example.com", port: 5000, route: .internet,
                                         connectTimeout: ConnectPlanner.remoteConnectTimeout)])
        XCTAssertEqual(ConnectPlanner.plan(host: "127.0.0.1", port: 47897, bonjourService: nil, paired: nil),
                       [lan("127.0.0.1", 47897)])
    }

    func testPairedComputerTriesLastHostThenLanThenTailscaleThenTailscaleDNS() {
        XCTAssertEqual(ConnectPlanner.plan(host: "192.168.1.20", port: 47800, bonjourService: nil, paired: paired), [
            lan("192.168.1.20", 47800),   // lastHost at lastPort
            lan("192.168.1.20", 47801),   // advertised lan address at the advertised port
            lan("10.0.0.5", 47801),
            tailscale("100.101.102.103", 47801),
            tailscale("fd7a:115c:a1e0::5", 47801),
            tailscale("mac.tail1234.ts.net", 47801),
        ])
    }

    func testDuplicatesAreRemovedCaseInsensitively() {
        var computer = paired
        computer.lastHost = "Mac.Tail1234.ts.net"
        computer.serverPort = 0   // older host: addresses use lastPort
        computer.addresses = [ServerAddress(host: "mac.tail1234.ts.net", kind: .tailscaleDNS),
                              ServerAddress(host: "100.101.102.103", kind: .tailscale)]
        XCTAssertEqual(ConnectPlanner.plan(host: "Mac.Tail1234.ts.net", port: 47800, bonjourService: nil, paired: computer),
                       [tailscale("Mac.Tail1234.ts.net", 47800), tailscale("100.101.102.103", 47800)])
    }

    func testPairingWithoutAddressesStillUsesLastHostAndThePickedHost() {
        var old = paired
        old.addresses = []
        old.serverPort = 0
        old.lastHost = ""
        XCTAssertEqual(ConnectPlanner.plan(host: "192.168.1.30", port: 47800, bonjourService: nil, paired: old),
                       [lan("192.168.1.30", 47800)])
        var moved = paired
        moved.addresses = []
        moved.serverPort = 0
        XCTAssertEqual(ConnectPlanner.plan(host: "192.168.1.31", port: 47800, bonjourService: nil, paired: moved),
                       [lan("192.168.1.20", 47800), lan("192.168.1.31", 47800)])
    }

    func testRoutesOfNames() {
        XCTAssertEqual(ConnectPlanner.route(host: "studio.local", kind: nil), .lan)
        XCTAssertEqual(ConnectPlanner.route(host: "studio.local.", kind: nil), .lan)
        XCTAssertEqual(ConnectPlanner.route(host: "studio.tail1234.ts.net", kind: nil), .tailscale)
        XCTAssertEqual(ConnectPlanner.route(host: "studio.lan", kind: .lan), .lan)
        XCTAssertEqual(ConnectPlanner.route(host: "studio", kind: .tailscaleDNS), .tailscale)
        XCTAssertEqual(ConnectPlanner.route(host: "mac.example.com", kind: nil), .internet)
        // A numeric address is classified by itself, whatever the advertised kind says.
        XCTAssertEqual(ConnectPlanner.route(host: "203.0.113.9", kind: .lan), .internet)
        XCTAssertEqual(ConnectPlanner.plan(host: "studio.local", port: 47800, bonjourService: nil, paired: nil)
            .first?.connectTimeout, ConnectPlanner.lanConnectTimeout)
    }

    // MARK: - FallbackConnector

    private final class FakeAttempt: ConnectAttempt {
        var cancelled = false
        func cancel() { cancelled = true }
    }

    func testFallbackTriesInOrderUntilOneConnects() {
        let candidates = [lan("10.0.0.1", 1), lan("10.0.0.2", 2), tailscale("100.64.0.3", 3)]
        var tried: [String] = []
        var finishers: [(Result<String, ConnectFailure>) -> Void] = []
        let connector = FallbackConnector<String>(candidates: candidates, onAttempt: { tried.append($0.host) }) { c, done in
            finishers.append(done)
            return FakeAttempt()
        }
        var result: Result<(String, ConnectCandidate), ConnectFailure>?
        connector.start { result = $0 }
        XCTAssertEqual(tried, ["10.0.0.1"])
        finishers[0](.failure(.failed("refused")))
        XCTAssertEqual(tried, ["10.0.0.1", "10.0.0.2"])
        finishers[1](.failure(.identityChanged))
        finishers[1](.success("late duplicate"))   // ignored: that attempt already finished
        XCTAssertNil(result)
        finishers[2](.success("conn"))
        guard case .success(let (connection, candidate))? = result else { return XCTFail("\(String(describing: result))") }
        XCTAssertEqual(connection, "conn")
        XCTAssertEqual(candidate.host, "100.64.0.3")
    }

    func testFallbackReportsIdentityChangeWhenNothingConnects() {
        let connector = FallbackConnector<String>(candidates: [lan("10.0.0.1", 1), lan("10.0.0.2", 2)]) { c, done in
            done(c.host == "10.0.0.1" ? .failure(.identityChanged) : .failure(.failed("timeout")))
            return FakeAttempt()
        }
        var result: Result<(String, ConnectCandidate), ConnectFailure>?
        connector.start { result = $0 }
        guard case .failure(.identityChanged)? = result else { return XCTFail("\(String(describing: result))") }

        let none = FallbackConnector<String>(candidates: []) { _, _ in FakeAttempt() }
        none.start { result = $0 }
        guard case .failure(.failed)? = result else { return XCTFail("\(String(describing: result))") }
    }

    func testFallbackCancelStopsTheCurrentAttemptSilently() {
        let attempt = FakeAttempt()
        var done: ((Result<String, ConnectFailure>) -> Void)?
        let connector = FallbackConnector<String>(candidates: [lan("10.0.0.1", 1), lan("10.0.0.2", 2)]) { _, d in
            done = d
            return attempt
        }
        var called = false
        connector.start { _ in called = true }
        connector.cancel()
        XCTAssertTrue(attempt.cancelled)
        done?(.failure(.failed("cancelled")))
        XCTAssertFalse(called)
    }

    // MARK: - Host input, platform, data saver

    func testHostInput() {
        XCTAssertEqual(HostInput.parse(host: "192.168.1.20", port: ""), .init(host: "192.168.1.20", port: 47800))
        XCTAssertEqual(HostInput.parse(host: " 127.0.0.1:47897 ", port: ""), .init(host: "127.0.0.1", port: 47897))
        XCTAssertEqual(HostInput.parse(host: "studio.local", port: "47801"), .init(host: "studio.local", port: 47801))
        XCTAssertEqual(HostInput.parse(host: "192.168.1.20:5000", port: "47801"), .init(host: "192.168.1.20", port: 5000))
        XCTAssertEqual(HostInput.parse(host: "[fe80::1%en0]:47800", port: ""), .init(host: "fe80::1%en0", port: 47800))
        XCTAssertEqual(HostInput.parse(host: "fd7a:115c:a1e0::5", port: ""), .init(host: "fd7a:115c:a1e0::5", port: 47800))
        XCTAssertNil(HostInput.parse(host: "", port: ""))
        XCTAssertNil(HostInput.parse(host: "a b", port: ""))
        XCTAssertNil(HostInput.parse(host: "host/path", port: ""))
        XCTAssertNil(HostInput.parse(host: "host", port: "0"))
        XCTAssertNil(HostInput.parse(host: "host", port: "65536"))
        XCTAssertNil(HostInput.parse(host: "host:x", port: ""))
        XCTAssertNil(HostInput.parse(host: "[::1", port: ""))
    }

    func testPlatformOfPairingsAndListRows() {
        var computer = paired
        XCTAssertEqual(computer.hostPlatform, .macos)   // pairings made before `platform` existed
        computer.platform = "linux"
        XCTAssertEqual(computer.hostPlatform, .linux)
        computer.platform = "haiku"
        XCTAssertNil(computer.hostPlatform, "a generic computer")
        let rows = ComputerList.merge(discovered: [DiscoveredComputer(serviceName: "Old Mac", serverId: "1", protocolVersion: "1"),
                                                   DiscoveredComputer(serviceName: "Beos", serverId: "2", protocolVersion: "1",
                                                                      platform: "haiku")],
                                      paired: [])
        XCTAssertEqual(rows.map(\.platform), [nil, .macos])
    }

    private let hello = Hello(deviceId: "dev", deviceName: "iPad",
                              screen: ScreenInfo(width: 2732, height: 2048, densityDpi: 320, refreshRate: 120),
                              codecs: [CodecInfo(name: "hevc", maxWidth: 4096, maxHeight: 2304)], features: ["mic"])

    func testDataSaverAsksForTheCapsOnlyOnAMeteredNetworkWithTheSettingOn() throws {
        let metered = DataSaver.apply(to: hello, metered: true, enabled: true)
        XCTAssertEqual(metered.network, "metered")
        XCTAssertEqual(metered.limits, StreamLimits(maxBitrateKbps: 4000, maxFps: 30))
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: ClientMessage.hello(metered).frame().payload) as? [String: Any])
        XCTAssertEqual(json["network"] as? String, "metered")
        XCTAssertEqual((json["limits"] as? [String: Any])?["maxBitrateKbps"] as? Int, 4000)
        XCTAssertEqual((json["limits"] as? [String: Any])?["maxFps"] as? Int, 30)
        XCTAssertEqual(json["features"] as? [String], ["mic"])

        let unmetered = DataSaver.apply(to: hello, metered: false, enabled: true)
        XCTAssertEqual(unmetered.network, "unmetered")
        XCTAssertNil(unmetered.limits)
        // The setting off still tells the truth about the network.
        let off = DataSaver.apply(to: hello, metered: true, enabled: false)
        XCTAssertEqual(off.network, "metered")
        XCTAssertNil(off.limits)
    }

    // MARK: - Computer list

    func testComputerListMergesDiscoveredAndPaired() {
        var offline = paired
        offline.serverId = "b"
        offline.serverName = "Basement"
        offline.platform = "linux"
        var online = paired
        online.serverId = "a"
        online.serverName = "Studio"
        let items = ComputerList.merge(
            discovered: [
                DiscoveredComputer(serviceName: "Studio", serverId: "a", protocolVersion: "1"),
                DiscoveredComputer(serviceName: "Studio again", serverId: "a", protocolVersion: "1"),   // duplicate
                DiscoveredComputer(serviceName: "Guest PC", serverId: nil, protocolVersion: "2", platform: "windows"),
            ],
            paired: [offline, online])
        XCTAssertEqual(items.map(\.name), ["Guest PC", "Studio", "Basement"])
        XCTAssertEqual(items.map(\.online), [true, true, false])
        XCTAssertEqual(items.map(\.paired), [false, true, true])
        XCTAssertEqual(items[0].platform, .windows)
        XCTAssertFalse(items[0].compatible)
        XCTAssertEqual(items[1].serviceName, "Studio")
        XCTAssertEqual(items[1].host, "192.168.1.20")
        XCTAssertEqual(items[1].platform, .macos)
        XCTAssertTrue(items[1].compatible)
        XCTAssertNil(items[2].serviceName)
        XCTAssertEqual(items[2].host, "192.168.1.20")
        XCTAssertTrue(items[2].remote)
        XCTAssertEqual(items[2].platform, .linux)
    }

    func testPairedComputerCodingIsTolerantAndNeverPrintsTheToken() throws {
        let data = try JSONEncoder().encode(paired)
        XCTAssertEqual(try JSONDecoder().decode(PairedComputer.self, from: data), paired)
        let minimal = Data(#"{"serverId":"x","fingerprintHex":"00ff","token":"qg=="}"#.utf8)
        let decoded = try JSONDecoder().decode(PairedComputer.self, from: minimal)
        XCTAssertEqual(decoded.fingerprint, Data([0x00, 0xFF]))
        XCTAssertEqual(decoded.token.data, Data([0xAA]))
        XCTAssertEqual(decoded.lastPort, 47800)
        XCTAssertEqual(decoded.hostPlatform, .macos)
        XCTAssertFalse(paired.description.contains("qg=="))
    }

    func testUpdateEndpointKeepsWhatTheServerDidNotSay() {
        let store = InMemoryPairingRepository([paired])
        store.updateEndpoint(serverId: "id", serverName: "", host: "10.0.0.5", port: 47801)
        let updated = store.pairing(serverId: "id")
        XCTAssertEqual(updated?.serverName, "Studio")
        XCTAssertEqual(updated?.lastHost, "10.0.0.5")
        XCTAssertEqual(updated?.addresses, paired.addresses)
        store.updateEndpoint(serverId: "unknown", serverName: "x", host: "h", port: 1)
        XCTAssertNil(store.pairing(serverId: "unknown"))
    }
}
