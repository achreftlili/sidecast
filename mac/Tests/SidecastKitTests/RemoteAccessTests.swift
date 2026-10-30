import XCTest
@testable import SidecastKit

/// PROTOCOL §1.1 addresses and §5 HELLO `network` / `limits` (data saver).
final class RemoteAccessTests: XCTestCase {
    // MARK: - Address classification

    func testTailscaleRangeBoundaries() {
        XCTAssertFalse(NetworkAddresses.isTailscaleRange("100.63.255.255"))
        XCTAssertTrue(NetworkAddresses.isTailscaleRange("100.64.0.0"))
        XCTAssertTrue(NetworkAddresses.isTailscaleRange("100.101.102.103"))
        XCTAssertTrue(NetworkAddresses.isTailscaleRange("100.127.255.255"))
        XCTAssertFalse(NetworkAddresses.isTailscaleRange("100.128.0.0"))
        XCTAssertFalse(NetworkAddresses.isTailscaleRange("101.64.0.1"))
    }

    func testRFC1918() {
        for ip in ["10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255", "192.168.0.1", "192.168.255.254"] {
            XCTAssertTrue(NetworkAddresses.isPrivate(ip), ip)
        }
        for ip in ["11.0.0.1", "172.15.255.255", "172.32.0.0", "192.169.0.1", "192.167.1.1", "100.64.0.1", "8.8.8.8"] {
            XCTAssertFalse(NetworkAddresses.isPrivate(ip), ip)
        }
    }

    func testClassification() {
        XCTAssertEqual(NetworkAddresses.classify(interface: "en0", ipv4: "192.168.1.20"), .lan)
        XCTAssertEqual(NetworkAddresses.classify(interface: "en7", ipv4: "10.0.0.5"), .lan)
        XCTAssertEqual(NetworkAddresses.classify(interface: "bridge100", ipv4: "192.168.2.1"), .lan)
        XCTAssertEqual(NetworkAddresses.classify(interface: "utun4", ipv4: "100.101.102.103"), .tailscale)
        XCTAssertEqual(NetworkAddresses.classify(interface: "utun4", ipv4: "100.64.0.0"), .tailscale)
        XCTAssertEqual(NetworkAddresses.classify(interface: "utun4", ipv4: "100.127.255.255"), .tailscale)
        // A CGNAT-range address on Wi-Fi is the LAN path, not Tailscale.
        XCTAssertEqual(NetworkAddresses.classify(interface: "en0", ipv4: "100.70.1.2"), .lan)
        // Other tunnels (corporate VPN), link-local, loopback and junk are not advertised.
        XCTAssertNil(NetworkAddresses.classify(interface: "utun2", ipv4: "10.8.0.6"))
        XCTAssertNil(NetworkAddresses.classify(interface: "utun2", ipv4: "100.128.0.1"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "169.254.10.20"))
        XCTAssertNil(NetworkAddresses.classify(interface: "lo0", ipv4: "127.0.0.1"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "0.0.0.0"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "224.0.0.251"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "1.2.3"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "256.1.1.1"))
        XCTAssertNil(NetworkAddresses.classify(interface: "en0", ipv4: "a.b.c.d"))
    }

    func testOrderingIsLANThenTailscaleThenDNSWithoutDuplicates() {
        let input = [
            ServerAddress(host: "mac.tail1234.ts.net", kind: .tailscaleDNS),
            ServerAddress(host: "100.101.102.103", kind: .tailscale),
            ServerAddress(host: "192.168.1.20", kind: .lan),
            ServerAddress(host: "10.0.0.5", kind: .lan),
            ServerAddress(host: "192.168.1.20", kind: .lan),
        ]
        XCTAssertEqual(NetworkAddresses.ordered(input).map(\.host),
                       ["192.168.1.20", "10.0.0.5", "100.101.102.103", "mac.tail1234.ts.net"])
    }

    func testMagicDNSNameFromStatusJSON() {
        let json = #"{"Version":"1.76.1","Self":{"DNSName":"achrefs-mbp.tail1234.ts.net.","TailscaleIPs":["100.101.102.103"]}}"#
        XCTAssertEqual(NetworkAddresses.magicDNSName(fromStatusJSON: Data(json.utf8)), "achrefs-mbp.tail1234.ts.net")
        XCTAssertNil(NetworkAddresses.magicDNSName(fromStatusJSON: Data(#"{"Self":{"DNSName":""}}"#.utf8)))
        XCTAssertNil(NetworkAddresses.magicDNSName(fromStatusJSON: Data(#"{"BackendState":"Stopped"}"#.utf8)))
        XCTAssertNil(NetworkAddresses.magicDNSName(fromStatusJSON: Data("not json".utf8)))
    }

    // MARK: - SERVER_HELLO

    func testServerHelloCarriesPortAndAddresses() throws {
        let hello = ServerHello(serverId: "S", serverName: "Mac", port: 47800, addresses: [
            ServerAddress(host: "192.168.1.20", kind: .lan),
            ServerAddress(host: "100.101.102.103", kind: .tailscale),
            ServerAddress(host: "mac.tail1234.ts.net", kind: .tailscaleDNS),
        ])
        let json = String(decoding: ServerMessage.serverHello(hello).encoded().dropFirst(5), as: UTF8.self)
        XCTAssertEqual(json, #"{"addresses":[{"host":"192.168.1.20","kind":"lan"},{"host":"100.101.102.103","kind":"tailscale"},"#
                       + #"{"host":"mac.tail1234.ts.net","kind":"tailscale-dns"}],"port":47800,"protocol":1,"serverId":"S","serverName":"Mac"}"#)
        let parser = FrameParser()
        parser.append(ServerMessage.serverHello(hello).encoded())
        XCTAssertEqual(try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame())), .serverHello(hello))
        // Without them the old shape is unchanged.
        let old = String(decoding: ServerMessage.serverHello(ServerHello(serverId: "S", serverName: "Mac")).encoded().dropFirst(5),
                         as: UTF8.self)
        XCTAssertEqual(old, #"{"protocol":1,"serverId":"S","serverName":"Mac"}"#)
    }

    // MARK: - HELLO network / limits

    private func hello(_ extra: String) throws -> Hello {
        let json = #"{"protocol":1,"deviceId":"d","screen":{"width":2400,"height":1080}"# + extra + "}"
        guard case .hello(let h)? = try ClientMessage.decode(Frame(.hello, payload: Data(json.utf8))) else {
            throw NSError(domain: "test", code: 1)
        }
        return h
    }

    func testNetworkAndLimitsDecode() throws {
        let h = try hello(#","network":"metered","limits":{"maxBitrateKbps":4000,"maxFps":30}"#)
        XCTAssertEqual(h.clientNetwork, .metered)
        XCTAssertEqual(h.limits, StreamLimits.dataSaver)
        XCTAssertEqual(h.limits?.maxBitrate, 4_000_000)
        let none = try hello("")
        XCTAssertNil(none.network)
        XCTAssertNil(none.limits)
        XCTAssertEqual(try hello(#","network":"unmetered""#).clientNetwork, .unmetered)
    }

    func testMalformedNetworkAndLimitsAreIgnoredNotFatal() throws {
        XCTAssertNil(try hello(#","limits":"fast""#).limits)
        XCTAssertNil(try hello(#","limits":[1,2]"#).limits)
        XCTAssertNil(try hello(#","limits":{"maxBitrateKbps":0,"maxFps":-5}"#).limits, "non-positive = no limit")
        XCTAssertNil(try hello(#","limits":{}"#).limits)
        XCTAssertEqual(try hello(#","limits":{"maxBitrateKbps":"4000","maxFps":24}"#).limits, StreamLimits(maxFps: 24))
        XCTAssertEqual(try hello(#","limits":{"maxBitrateKbps":3999.7}"#).limits, StreamLimits(maxBitrateKbps: 3999))
        XCTAssertNil(try hello(#","network":42"#).network)
        let future = try hello(#","network":"satellite""#)
        XCTAssertEqual(future.network, "satellite")
        XCTAssertNil(future.clientNetwork)
    }

    func testHelloEncodesNetworkAndLimitsOnlyWhenSet() throws {
        let screen = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 120)
        let plain = Hello(deviceId: "d", deviceName: "n", screen: screen, codecs: [])
        XCTAssertFalse(String(decoding: ClientMessage.hello(plain).frame().payload, as: UTF8.self).contains("limits"))
        let saver = Hello(deviceId: "d", deviceName: "n", screen: screen, codecs: [],
                          network: "metered", limits: .dataSaver)
        let json = String(decoding: ClientMessage.hello(saver).frame().payload, as: UTF8.self)
        XCTAssertTrue(json.contains(#""limits":{"maxBitrateKbps":4000,"maxFps":30}"#), json)
        XCTAssertTrue(json.contains(#""network":"metered""#), json)
        XCTAssertEqual(try ClientMessage.decode(ClientMessage.hello(saver).frame()), .hello(saver))
    }

    // MARK: - Applying limits

    private let screen = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 120)
    private let codecs = [CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160)]

    func testFpsIsMinOfSettingPanelAndLimit() throws {
        var settings = StreamSettings()
        settings.fps = 60
        let limited = try StreamPlanner.plan(screen: screen, codecs: codecs, settings: settings, limits: .dataSaver)
        XCTAssertEqual(limited.fps, 30)
        XCTAssertEqual(limited.display.refreshRate, 60, "the virtual display keeps its normal refresh rate")
        XCTAssertEqual(try StreamPlanner.plan(screen: screen, codecs: codecs, settings: settings).fps, 60)
        XCTAssertEqual(try StreamPlanner.plan(screen: screen, codecs: codecs, settings: settings,
                                              limits: StreamLimits(maxFps: 90)).fps, 60, "a looser limit changes nothing")
        settings.fps = 120
        let slowPanel = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 60)
        XCTAssertEqual(try StreamPlanner.plan(screen: slowPanel, codecs: codecs, settings: settings,
                                              limits: StreamLimits(maxFps: 90)).fps, 60, "the panel still caps")
        XCTAssertEqual(try StreamPlanner.plan(screen: screen, codecs: codecs, settings: settings,
                                              limits: StreamLimits(maxFps: 1)).fps, StreamPlanner.minLimitedFps)
        XCTAssertEqual(try StreamPlanner.plan(screen: screen, codecs: codecs, settings: settings,
                                              limits: StreamLimits(maxBitrateKbps: 4000)).fps, 120, "bitrate-only limit")
    }

    func testBitrateBounds() {
        let none = BitrateBounds(settingMbps: 50, limits: nil)
        XCTAssertEqual([none.floor, none.ceiling, none.start], [4_000_000, 50_000_000, 20_000_000])
        let saver = BitrateBounds(settingMbps: 50, limits: .dataSaver)
        XCTAssertEqual([saver.floor, saver.ceiling, saver.start], [4_000_000, 4_000_000, 4_000_000])
        let tight = BitrateBounds(settingMbps: 50, limits: StreamLimits(maxBitrateKbps: 2_500))
        XCTAssertEqual([tight.floor, tight.ceiling, tight.start], [2_500_000, 2_500_000, 2_500_000], "floor = limit below 4 Mbps")
        let loose = BitrateBounds(settingMbps: 50, limits: StreamLimits(maxBitrateKbps: 10_000))
        XCTAssertEqual([loose.floor, loose.ceiling, loose.start], [4_000_000, 10_000_000, 10_000_000], "start clamped to the max")
        let settingWins = BitrateBounds(settingMbps: 10, limits: StreamLimits(maxBitrateKbps: 20_000))
        XCTAssertEqual(settingWins.ceiling, 10_000_000)
        XCTAssertEqual(BitrateBounds(settingMbps: 50, limits: StreamLimits(maxFps: 30)).ceiling, 50_000_000)
    }

    func testAdaptiveControllerStaysInsideTheLimit() {
        let bounds = BitrateBounds(settingMbps: 50, limits: StreamLimits(maxBitrateKbps: 3_000))
        var abr = AdaptiveBitrateController(config: .init(bounds: bounds), now: 0)
        XCTAssertEqual(abr.bitrate, 3_000_000)
        // Clean windows never push it above the limit…
        var now = 0.0
        for _ in 0..<10 {
            for _ in 0..<40 { abr.recordRTT(0.01) }
            now += 1
            abr.tick(now: now)
        }
        XCTAssertEqual(abr.bitrate, 3_000_000)
        // …and congestion never drops it below the (lowered) floor.
        for _ in 0..<5 {
            abr.recordDrop()
            now += 1
            abr.tick(now: now)
        }
        XCTAssertEqual(abr.bitrate, 3_000_000)
        // Settings changes keep the limit applied.
        abr.setBounds(BitrateBounds(settingMbps: 100, limits: StreamLimits(maxBitrateKbps: 3_000)))
        XCTAssertEqual(abr.config.maxBitrate, 3_000_000)
        abr.setBounds(BitrateBounds(settingMbps: 100, limits: nil))
        XCTAssertEqual(abr.config.maxBitrate, 100_000_000)
        XCTAssertEqual(abr.config.minBitrate, 4_000_000)
    }
}
