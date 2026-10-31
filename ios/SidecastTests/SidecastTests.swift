import SidecastKit
import XCTest
@testable import Sidecast

/// iOS-side logic. The protocol, session, touch, keyboard and microphone logic
/// lives in SidecastKit and is tested there (`cd mac && swift test`).
final class OverlayPresenterTests: XCTestCase {
    private func present(_ state: DisplaySession.State, decoder: DecoderStatus = .idle, route: Route? = nil,
                         platform: HostPlatform? = .macos) -> Overlay? {
        OverlayPresenter.present(state, decoder: decoder, route: route, fallbackName: "10.0.0.2", platform: platform)
    }

    func testStreamingShowsTheVideoOnceThereIsAPicture() {
        XCTAssertEqual(present(.streaming(name: "Studio"))?.title, "Starting the stream…")
        XCTAssertEqual(present(.streaming(name: "Studio"), decoder: .running(hasPicture: false))?.progress, true)
        XCTAssertNil(present(.streaming(name: "Studio"), decoder: .running(hasPicture: true)))
        let unsupported = present(.streaming(name: "Studio"),
                                  decoder: .unsupported(config: StreamConfig(codec: "hevc", width: 4000, height: 3000, fps: 60),
                                                        detail: "VideoToolbox error -12906"))
        XCTAssertEqual(unsupported?.primary?.kind, .retry)
        XCTAssertEqual(unsupported?.detail, "The video decoder rejected HEVC 4000×3000 (VideoToolbox error -12906).")
    }

    func testConnectingSaysWhichRoute() {
        XCTAssertEqual(present(.idle)?.title, "Connecting to 10.0.0.2…")
        XCTAssertEqual(present(.connecting(name: "Studio"), route: .tailscale)?.detail, "Connecting via Tailscale…")
        let lost = present(.reconnecting(name: "Studio", cause: .connectionLost, attempt: 3), route: .lan)
        XCTAssertEqual(lost?.title, "Connection lost — reconnecting…")
        XCTAssertEqual(lost?.detail, "Connecting via home network…\nAttempt 3")
        XCTAssertEqual(lost?.secondary?.kind, .close)
    }

    func testErrorsOfferTheRightWayOut() {
        XCTAssertEqual(present(.failed(name: "Studio", error: .identityChanged))?.primary?.kind, .pairAgain)
        XCTAssertEqual(present(.failed(name: "Studio", error: .replaced))?.primary?.label, "Reconnect")
        XCTAssertNil(present(.failed(name: "Studio", error: .protocolMismatch))?.primary)
        // Screen Recording steps only for a Mac; others get generic advice.
        XCTAssertEqual(present(.failed(name: "Studio", error: .permissionMissing))?.title, "Screen Recording permission needed")
        XCTAssertEqual(present(.failed(name: "Tower", error: .permissionMissing), platform: .windows)?.title,
                       "Screen capture permission needed")
        XCTAssertEqual(present(.failed(name: "Box", error: .permissionMissing), platform: nil)?.title,
                       "Screen capture permission needed")
    }

    func testNoMacWordingInTheOverlay() {
        let states: [DisplaySession.State] = [
            .idle, .pairing(name: "S"), .awaitingCode(name: "S", prompt: .init(attemptsLeft: 5, expiresAt: 0, wrongCode: false)),
            .reconnecting(name: "S", cause: .unreachable, attempt: 1), .failed(name: "S", error: .displayFailed),
            .failed(name: "S", error: .rejected("x")), .failed(name: "S", error: .pairingExpired),
        ]
        for state in states {
            let o = present(state)
            XCTAssertFalse((o?.title ?? "").contains("Mac"), "\(state)")
            XCTAssertFalse((o?.detail ?? "").contains("Mac"), "\(state)")
        }
    }
}

final class StoreTests: XCTestCase {
    func testPairingsSurviveInTheKeychain() throws {
        let service = "sidecast.tests.\(UUID().uuidString)"
        let pairing = PairedComputer(serverId: "srv", serverName: "Studio", fingerprint: Data(repeating: 1, count: 32),
                                     token: Data(repeating: 2, count: 32), lastHost: "10.0.0.2", lastPort: 47800,
                                     addresses: [ServerAddress(host: "100.64.0.2", kind: .tailscale)], serverPort: 47800,
                                     platform: "linux")
        KeychainPairingStore(service: service).save(pairing)
        let reloaded = KeychainPairingStore(service: service)
        defer { reloaded.removeAll() }
        guard let found = reloaded.pairing(serverId: "srv") else {
            throw XCTSkip("no Keychain in this unsigned test host")
        }
        XCTAssertEqual(found, pairing)
        reloaded.remove(serverId: "srv")
        XCTAssertNil(KeychainPairingStore(service: service).pairing(serverId: "srv"))
    }

    func testSettingsDefaults() {
        let defaults = UserDefaults(suiteName: "sidecast.tests.\(UUID().uuidString)")!
        let settings = AppSettings(defaults: defaults)
        XCTAssertTrue(settings.micEnabled)
        XCTAssertTrue(settings.dataSaver)
        XCTAssertEqual(settings.inputMode, .touch)
        settings.inputMode = .trackpad
        XCTAssertEqual(AppSettings(defaults: defaults).inputMode, .trackpad)
        defaults.set("bogus", forKey: "inputMode")
        XCTAssertEqual(settings.inputMode, .touch)
        // A launch argument (`-micEnabled NO`) arrives as a string.
        defaults.set("NO", forKey: "micEnabled")
        XCTAssertFalse(settings.micEnabled)
        settings.micEnabled = true
        XCTAssertTrue(settings.micEnabled)
    }

    func testCodecsAndScreen() {
        let codecs = DecoderCapabilities.advertised()
        XCTAssertEqual(codecs.last?.name, "h264")
        XCTAssertTrue(codecs.allSatisfy { $0.maxWidth == 4096 && $0.maxHeight == 2304 })
        XCTAssertEqual(DecoderCapabilities.advertised(override: ["hevc"]).map(\.name), ["hevc"])
        let screen = ScreenInfo.landscape(of: UIScreen.main)
        XCTAssertGreaterThanOrEqual(screen.width, screen.height)
        XCTAssertGreaterThanOrEqual(screen.refreshRate, 60)
    }
}
