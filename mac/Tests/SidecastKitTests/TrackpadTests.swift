import CoreGraphics
import XCTest
@testable import SidecastKit

/// Trackpad input mode (PROTOCOL §7.3): relative cursor, taps at the cursor.
final class TrackpadTests: XCTestCase {
    private var scheduler: ManualScheduler!
    private var engine: GestureEngine!
    private var actions: [MouseAction] = []
    private let size = CGSize(width: 1024, height: 512)
    private let centre = CGPoint(x: 512, y: 256)

    override func setUp() {
        scheduler = ManualScheduler()
        actions = []
        engine = GestureEngine(scheduler: scheduler) { [unowned self] in self.actions.append($0) }
        engine.configure(displaySize: size)   // 1 pt per dp
        engine.setMode(.trackpad)
    }

    // MARK: - Helpers (finger positions in display points)

    private typealias Finger = (id: UInt8, x: Double, y: Double)

    private func touch(_ t: UInt32, _ action: TouchAction, _ fingers: [Finger], pid: UInt8? = nil) {
        let pointers = fingers.map { TouchPointer(id: $0.id, x: Float($0.x / 1024), y: Float($0.y / 512)) }
        engine.handle(TouchEvent(eventTimeMs: t, action: action, actionPointerId: pid ?? fingers[0].id, pointers: pointers))
    }

    private func down(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .down, [(0, x, y)]) }
    private func move(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .move, [(0, x, y)]) }
    private func up(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .up, [(0, x, y)]) }

    /// One finger from (x, y) by (dx, dy) in `steps` MOVEs `every` ms apart; returns the end time.
    @discardableResult
    private func slide(from t: UInt32, _ x: Double, _ y: Double, dx: Double, dy: Double = 0,
                       steps: Int, every: UInt32) -> UInt32 {
        var time = t
        for i in 1...steps {
            time = t + UInt32(i) * every
            move(time, x + dx * Double(i) / Double(steps), y + dy * Double(i) / Double(steps))
        }
        return time
    }

    private func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x, y: y) }

    private var clicks: [MouseAction] {
        actions.filter { if case .click = $0 { return true } else { return false } }
    }

    private var rightClicks: [CGPoint] {
        actions.compactMap { if case .rightClick(let p) = $0 { return p } else { return nil } }
    }

    // MARK: - Pointing

    func testTouchModeIsTheDefault() {
        let e = GestureEngine(scheduler: scheduler) { _ in }
        XCTAssertEqual(e.mode, .touch)
    }

    func testOneFingerMovesTheCursorRelatively() {
        down(0, 100, 100)
        XCTAssertEqual(actions, [], "touch-down does not jump the cursor to the finger")
        let end = slide(from: 0, 100, 100, dx: 100, steps: 50, every: 50)   // 40 dp/s: slow, gain 0.6
        up(end + 50, 200, 100)
        XCTAssertEqual(engine.cursor.x, 512 + 60, accuracy: 1e-6)
        XCTAssertEqual(engine.cursor.y, 256, accuracy: 1e-6)
        XCTAssertEqual(actions.last, .move(to: engine.cursor))
        XCTAssertTrue(actions.allSatisfy { if case .move = $0 { return true } else { return false } })
    }

    func testCursorIsClampedToTheDisplay() {
        for i in 0..<3 {   // three full-width swipes to the right
            let t = UInt32(i) * 2000
            down(t, 0, 256)
            up(slide(from: t, 0, 256, dx: 1023, steps: 100, every: 10) + 10, 1023, 256)
        }
        XCTAssertEqual(engine.cursor.x, 1023)
        down(10_000, 500, 511)
        up(slide(from: 10_000, 500, 511, dx: 0, dy: -511, steps: 100, every: 10) + 10, 500, 0)
        XCTAssertEqual(engine.cursor.y, 0)
        down(20_000, 1023, 0)
        for i in 0..<2 {
            let t = UInt32(20_000 + i * 2000)
            if i > 0 { down(t, 1023, 0) }
            up(slide(from: t, 1023, 0, dx: -1023, dy: 511, steps: 100, every: 10) + 10, 0, 511)
        }
        XCTAssertEqual(engine.cursor, p(0, 511))
    }

    func testAccelerationIsMonotonicAndBounded() {
        let th = GestureThresholds()
        XCTAssertEqual(GestureEngine.trackpadGain(speedDpPerSecond: 0, thresholds: th), 0.6, accuracy: 1e-9)
        XCTAssertEqual(GestureEngine.trackpadGain(speedDpPerSecond: 150, thresholds: th), 0.6, accuracy: 1e-9)
        XCTAssertEqual(GestureEngine.trackpadGain(speedDpPerSecond: 1500, thresholds: th), 1.5, accuracy: 1e-9)
        XCTAssertEqual(GestureEngine.trackpadGain(speedDpPerSecond: 50_000, thresholds: th), 1.5, accuracy: 1e-9)
        var previous = 0.0
        for v in stride(from: 0.0, through: 3000, by: 5) {
            let g = GestureEngine.trackpadGain(speedDpPerSecond: v, thresholds: th)
            XCTAssertGreaterThanOrEqual(g, previous, "gain must never drop as speed rises (v=\(v))")
            previous = g
        }
    }

    func testFastSwipesTravelFurtherThanSlowOnes() {
        down(0, 200, 100)
        up(slide(from: 0, 200, 100, dx: 300, steps: 100, every: 50) + 50, 500, 100)   // 60 dp/s
        let slow = engine.cursor.x - 512
        XCTAssertEqual(slow, 180, accuracy: 1e-6, "a slow swipe gets the base gain")
        engine.setMode(.touch)
        engine.setMode(.trackpad)
        let before = engine.cursor.x
        down(20_000, 500, 300)                                                          // stays on the display
        up(slide(from: 20_000, 500, 300, dx: -300, steps: 10, every: 8) + 8, 200, 300)   // 3750 dp/s
        let fast = before - engine.cursor.x
        XCTAssertGreaterThan(fast, slow * 2)
        XCTAssertLessThanOrEqual(fast, 300 * 1.5 + 1e-6)
    }

    func testStartsFromTheSystemCursorWhenItIsOnTheDisplay() {
        var system: CGPoint? = p(100, 50)
        engine.cursorProvider = { system }
        down(0, 500, 300)
        up(slide(from: 0, 500, 300, dx: 50, steps: 25, every: 50) + 50, 550, 300)
        XCTAssertEqual(engine.cursor.x, 130, accuracy: 1e-6)
        // The user moved the real mouse meanwhile: the next touch starts from there.
        system = p(300, 300)
        down(5000, 500, 300); up(5060, 500, 300)
        XCTAssertEqual(clicks.last, .click(at: p(300, 300), count: 1))
        // Off this display (or unknown): keep our own cursor.
        system = p(-10, 5)
        down(10_000, 500, 300); up(10_060, 500, 300)
        XCTAssertEqual(clicks.last, .click(at: p(300, 300), count: 1))
    }

    func testCursorStartsAtTheDisplayCentre() {
        engine.cursorProvider = { CGPoint(x: 5000, y: 5000) }   // not on this display
        down(0, 900, 400); up(70, 900, 400)
        XCTAssertEqual(actions, [.click(at: centre, count: 1)])
    }

    // MARK: - Taps

    func testTapClicksAtTheCursorNotTheFinger() {
        down(0, 900, 400)
        up(80, 900, 400)
        XCTAssertEqual(actions, [.click(at: centre, count: 1)])
    }

    func testJitterDuringATapDoesNotMoveTheClick() {
        down(0, 900, 400)
        move(40, 904, 402)                          // within the slop: the cursor nudges…
        XCTAssertNotEqual(engine.cursor, centre)
        up(80, 904, 402)                            // …but the tap clicks where it started
        XCTAssertEqual(actions.last, .click(at: centre, count: 1))
        XCTAssertEqual(engine.cursor, centre)
    }

    func testSlowTapAndLongPressDoNothing() {
        down(0, 500, 300); up(300, 500, 300)        // > 250 ms
        down(1000, 500, 300); up(1900, 500, 300)    // a long press
        XCTAssertEqual(clicks, [])
        XCTAssertEqual(rightClicks, [], "no long-press right click in trackpad mode")
    }

    func testMovingBeyondTheSlopNeverClicks() {
        down(0, 500, 300)
        move(40, 540, 300)
        up(80, 540, 300)
        XCTAssertEqual(clicks, [])
    }

    func testDoubleTapIsADoubleClick() {
        down(0, 500, 300); up(60, 500, 300)
        down(200, 700, 100); up(260, 700, 100)      // anywhere on the phone: it clicks at the cursor
        down(400, 500, 300); up(450, 500, 300)      // a third tap starts over
        XCTAssertEqual(clicks, [.click(at: centre, count: 1), .click(at: centre, count: 2), .click(at: centre, count: 1)])
    }

    func testSecondTapTooLateIsAnotherSingleClick() {
        down(0, 500, 300); up(60, 500, 300)
        down(500, 500, 300); up(560, 500, 300)      // 440 ms after the first release
        XCTAssertEqual(clicks, [.click(at: centre, count: 1), .click(at: centre, count: 1)])
    }

    func testDoubleTapAndHoldDrags() {
        down(0, 500, 300); up(60, 500, 300)
        down(200, 500, 300)
        move(250, 505, 300)                         // within the slop: undecided, cursor still
        XCTAssertEqual(engine.cursor, centre)
        let end = slide(from: 300, 505, 300, dx: 35, steps: 20, every: 50)   // slow, beyond the slop
        XCTAssertTrue(engine.isButtonHeld)
        up(end + 20, 540, 300)
        XCTAssertFalse(engine.isButtonHeld)
        XCTAssertEqual(actions.first, .click(at: centre, count: 1))
        XCTAssertEqual(actions[1], .buttonDown(.left, at: centre))
        let drags = actions.compactMap { if case .drag(let p) = $0 { return p } else { return nil } }
        XCTAssertFalse(drags.isEmpty)
        XCTAssertEqual(drags.last!.x, 512 + 40 * 0.6, accuracy: 1e-6, "relative drag, whole travel applied")
        XCTAssertEqual(actions.last, .buttonUp(.left, at: engine.cursor))
        XCTAssertEqual(clicks.count, 1, "no double click when it became a drag")
    }

    // MARK: - Two or more fingers

    func testTwoFingerTapRightClicksAtTheCursor() {
        touch(0, .down, [(0, 800, 400)])
        touch(15, .pointerDown, [(0, 800, 400), (1, 860, 400)], pid: 1)
        touch(90, .pointerUp, [(0, 800, 400), (1, 860, 400)], pid: 1)
        touch(110, .up, [(0, 800, 400)])
        XCTAssertEqual(actions, [.rightClick(at: centre)])
    }

    func testTwoFingerScrollHappensAtTheCursor() {
        touch(0, .down, [(0, 300, 200)])
        touch(10, .pointerDown, [(0, 300, 200), (1, 400, 200)], pid: 1)
        for i in 1...6 {   // a flick downwards
            let y = 200 + Double(i) * 20
            touch(UInt32(10 + i * 10), .move, [(0, 300, y), (1, 400, y)])
        }
        touch(75, .pointerUp, [(0, 300, 320), (1, 400, 320)], pid: 1)
        touch(80, .up, [(0, 300, 320)])
        scheduler.advance(by: 3)
        let scrolls = actions.compactMap { a -> (CGPoint, ScrollPhase)? in
            if case let .scroll(_, _, at, phase) = a { return (at, phase) } else { return nil }
        }
        XCTAssertEqual(scrolls.first?.1, .began)
        XCTAssertTrue(scrolls.contains { $0.1 == .momentumBegan }, "momentum is reused")
        XCTAssertTrue(scrolls.allSatisfy { $0.0 == centre }, "scrolls at the cursor, not at the fingers")
        XCTAssertEqual(rightClicks, [])
        XCTAssertEqual(engine.cursor, centre, "scrolling does not move the cursor")
    }

    func testPinchZoomsAtTheCursor() {
        touch(0, .down, [(0, 450, 250)])
        touch(10, .pointerDown, [(0, 450, 250), (1, 550, 250)], pid: 1)
        for i in 1...6 {
            let spread = Double(i) * 15
            touch(UInt32(10 + i * 20), .move, [(0, 450 - spread, 250), (1, 550 + spread, 250)])
        }
        let zooms = actions.compactMap { if case let .zoom(d, at) = $0 { return (d, at) } else { return nil } }
        XCTAssertFalse(zooms.isEmpty)
        XCTAssertTrue(zooms.allSatisfy { $0.0 > 0 && $0.1 == centre })
    }

    func testThreeFingerSwipeNavigatesAsInTouchMode() {
        touch(0, .down, [(0, 400, 250)])
        touch(10, .pointerDown, [(0, 400, 250), (1, 440, 260)], pid: 1)
        touch(20, .pointerDown, [(0, 400, 250), (1, 440, 260), (2, 480, 250)], pid: 2)
        for i in 1...20 {
            let dx = -Double(i) * 10
            touch(UInt32(20 + i * 8), .move, [(0, 400 + dx, 250), (1, 440 + dx, 260), (2, 480 + dx, 250)])
        }
        touch(200, .pointerUp, [(0, 200, 250), (1, 240, 260), (2, 280, 250)], pid: 2)
        touch(210, .pointerUp, [(0, 200, 250), (1, 240, 260)], pid: 1)
        touch(220, .up, [(0, 200, 250)])
        XCTAssertEqual(actions, [.navigate(.spaceRight)])
    }

    // MARK: - Mode switches

    func testModeSwitchMidTrackpadDragReleasesTheButton() {
        down(0, 500, 300); up(60, 500, 300)
        down(200, 500, 300)
        move(260, 540, 300)
        XCTAssertTrue(engine.isButtonHeld)
        engine.setMode(.touch)
        XCTAssertFalse(engine.isButtonHeld)
        XCTAssertEqual(actions.last, .buttonUp(.left, at: engine.cursor))
        // The rest of that touch is ignored in the new mode (no stray click on lift).
        let count = actions.count
        up(300, 540, 300)
        XCTAssertEqual(actions.count, count)
    }

    func testModeSwitchMidTouchDragReleasesTheButton() {
        engine.setMode(.touch)
        down(0, 200, 200)
        move(600, 260, 200)                         // long-press drag (touch mode)
        XCTAssertTrue(engine.isButtonHeld)
        engine.setMode(.trackpad)
        XCTAssertFalse(engine.isButtonHeld)
        XCTAssertEqual(actions.last, .buttonUp(.left, at: p(260, 200)))
    }

    func testModeSwitchMidScrollEndsTheScrollWithoutMomentum() {
        engine.setMode(.touch)
        touch(0, .down, [(0, 400, 200)])
        touch(10, .pointerDown, [(0, 400, 200), (1, 500, 200)], pid: 1)
        touch(40, .move, [(0, 400, 240), (1, 500, 240)])
        engine.setMode(.trackpad)
        guard case let .scroll(_, _, _, phase)? = actions.last else { return XCTFail("expected a scroll") }
        XCTAssertEqual(phase, .ended)
        scheduler.advance(by: 2)
        XCTAssertEqual(scheduler.pendingCount, 0)
    }

    func testSettingTheSameModeChangesNothing() {
        down(0, 500, 300); up(60, 500, 300)
        down(200, 500, 300)
        move(260, 540, 300)
        let count = actions.count
        engine.setMode(.trackpad)
        XCTAssertEqual(actions.count, count)
        XCTAssertTrue(engine.isButtonHeld)
    }

    func testResetReleasesATrackpadDrag() {
        down(0, 500, 300); up(60, 500, 300)
        down(200, 500, 300)
        move(260, 540, 300)
        engine.reset()
        XCTAssertFalse(engine.isButtonHeld)
        XCTAssertEqual(actions.last, .buttonUp(.left, at: engine.cursor))
    }

    // MARK: - INPUT_MODE (0x18)

    func testInputModeMessage() throws {
        func decode(_ json: String) throws -> ClientMessage? {
            try ClientMessage.decode(Frame(.inputMode, payload: Data(json.utf8)))
        }
        XCTAssertEqual(try decode(#"{"mode":"trackpad"}"#), .inputMode(InputModeMessage(.trackpad)))
        XCTAssertEqual(try decode(#"{"mode":"touch","later":1}"#), .inputMode(InputModeMessage(.touch)))
        guard case .inputMode(let unknown)? = try decode(#"{"mode":"stylus"}"#) else { return XCTFail() }
        XCTAssertNil(unknown.inputMode, "unknown modes decode but are ignored")
        for bad in ["{", #"{"mode":5}"#, "{}", "[]"] {
            XCTAssertThrowsError(try decode(bad), bad) { error in
                guard case .malformed(.inputMode, _)? = error as? ProtocolError else { return XCTFail("\(error)") }
            }
        }
        let wire = ClientMessage.inputMode(InputModeMessage(.trackpad)).encoded()
        XCTAssertEqual(wire.first, 0x18)
        XCTAssertEqual(String(decoding: wire.dropFirst(5), as: UTF8.self), #"{"mode":"trackpad"}"#)
    }
}
