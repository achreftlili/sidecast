import XCTest

/// Drives the app against a running host, end to end: add by address, pair,
/// stream, touch, type, trackpad mode. Skipped unless a host is given, e.g.
///
///   dist/Sidecast.app/Contents/MacOS/Sidecast --test-pattern --data-dir /tmp/sc \
///       --port 47897 --test-pairing-code 123456 --no-bonjour
///   TEST_RUNNER_SIDECAST_TEST_HOST=127.0.0.1:47897 xcodebuild test -project ios/Sidecast.xcodeproj \
///       -scheme Sidecast -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
///       -only-testing:SidecastUITests
///
/// The host's log shows what arrived (TOUCH, gestures, TEXT, KEY, input mode).
/// `SIDECAST_TEST_SCREENSHOTS=<dir>` also saves the screenshots there.
final class EndToEndUITests: XCTestCase {
    private var env: [String: String] { ProcessInfo.processInfo.environment }

    override func setUp() {
        continueAfterFailure = false
    }

    func testPairStreamTouchTypeAndTrackpad() throws {
        guard let host = env["SIDECAST_TEST_HOST"], !host.isEmpty else {
            throw XCTSkip("set TEST_RUNNER_SIDECAST_TEST_HOST=<host:port> of a test-pattern host")
        }
        let app = XCUIApplication()
        app.launchArguments += ["-inputMode", "touch"]
        // The simulator's microphone is the computer's: only used when asked for.
        if env["SIDECAST_TEST_MIC"] != "1" { app.launchArguments += ["-micEnabled", "NO"] }
        if let codecs = env["SIDECAST_TEST_CODECS"] { app.launchArguments += ["-SidecastCodecs", codecs] }
        // Held in landscape, like a second screen.
        XCUIDevice.shared.orientation = .landscapeLeft
        defer { XCUIDevice.shared.orientation = .portrait }
        app.launch()

        app.buttons["addByIP"].tap()
        let add = app.alerts.firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 5))
        add.textFields["hostField"].typeText(host)
        add.buttons["Connect"].tap()

        // Pairing happens once per host: the next runs authenticate with the stored token.
        let code = app.alerts.textFields["pairingCode"]
        let keyboardButton = app.buttons["keyboard"]
        if code.waitForExistence(timeout: 15) {
            save(app, "1-pairing")
            code.typeText(env["SIDECAST_TEST_CODE"] ?? "123456")
        }
        XCTAssertTrue(keyboardButton.waitForExistence(timeout: 30), "the stream did not start")
        sleep(2)
        save(app, "2-streaming")
        hold()

        // Touch mode: a tap (click), a drag (scroll), a pinch (two pointers: zoom). No
        // two-finger tap: XCUITest places twoFingerTap() wrongly in a landscape-only app,
        // and its pinch is slower than the host's 0.3 s two-finger tap.
        let window = app.windows.firstMatch
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.4)).tap()
        sleep(1)
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.7))
            .press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3)))
        sleep(1)
        window.pinch(withScale: 3, velocity: 4)
        sleep(1)

        // Keyboard: text, then Return.
        keyboardButton.tap()
        sleep(2)
        save(app, "3-keyboard")
        hold()
        app.typeText("Hello é👍")
        app.typeText("\n")
        sleep(1)
        keyboardButton.tap()

        // Trackpad mode: the computer's pointer appears; move it and click.
        app.buttons["inputMode"].tap()
        sleep(1)
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5))
            .press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.6)))
        sleep(1)
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        sleep(1)
        save(app, "4-trackpad")   // the pointer is a drawn view: checked on this screenshot
        hold()
        app.buttons["inputMode"].tap()   // back to touch for the next run
        sleep(1)

        // iOS suspends apps in the background: the stream lets go, then reconnects on return.
        XCUIDevice.shared.press(.home)
        sleep(3)
        app.activate()
        XCTAssertTrue(keyboardButton.waitForExistence(timeout: 30), "the stream did not come back after the background")
        save(app, "5-resumed")

        app.buttons["disconnect"].tap()
        app.alerts.buttons["Disconnect"].tap()
        XCTAssertTrue(app.buttons["addByIP"].waitForExistence(timeout: 5))
        save(app, "6-list")
    }

    /// `SIDECAST_TEST_HOLD=<seconds>`: pause at each checkpoint (to look at the simulator or the host).
    private func hold() {
        if let seconds = env["SIDECAST_TEST_HOLD"].flatMap(UInt32.init) { sleep(seconds) }
    }

    private func save(_ app: XCUIApplication, _ name: String) {
        let shot = app.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let dir = env["SIDECAST_TEST_SCREENSHOTS"] {
            try? shot.pngRepresentation.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).png"))
        }
    }
}
