import CoreGraphics
import XCTest
@testable import SidecastKit

/// Two-finger tap (right click) and 3/4-finger swipes (Spaces, Mission Control, App Exposé).
final class MultiFingerGestureTests: XCTestCase {
    private var scheduler: ManualScheduler!
    private var engine: GestureEngine!
    private var actions: [MouseAction] = []
    private let size = CGSize(width: 1024, height: 512)

    override func setUp() {
        scheduler = ManualScheduler()
        actions = []
        engine = GestureEngine(scheduler: scheduler) { [unowned self] in self.actions.append($0) }
        engine.configure(displaySize: size)   // 1 pt per dp: slop 15, swipe threshold 60
    }

    // MARK: - Helpers (display points)

    private typealias Finger = (id: UInt8, x: Double, y: Double)

    private func touch(_ t: UInt32, _ action: TouchAction, _ fingers: [Finger], pid: UInt8? = nil) {
        let pointers = fingers.map { TouchPointer(id: $0.id, x: Float($0.x / 1024), y: Float($0.y / 512)) }
        engine.handle(TouchEvent(eventTimeMs: t, action: action, actionPointerId: pid ?? fingers[0].id, pointers: pointers))
    }

    /// Android order: DOWN for the first finger, POINTER_DOWN (all pointers so far) for each next one, 10 ms apart.
    @discardableResult
    private func fingersDown(_ t: UInt32, _ fingers: [Finger]) -> UInt32 {
        touch(t, .down, [fingers[0]])
        for i in 1..<fingers.count {
            touch(t + UInt32(i * 10), .pointerDown, Array(fingers[0...i]), pid: fingers[i].id)
        }
        return t + UInt32((fingers.count - 1) * 10)
    }

    /// POINTER_UP from the last finger back to the second, then UP for the first, 10 ms apart.
    @discardableResult
    private func fingersUp(_ t: UInt32, _ fingers: [Finger]) -> UInt32 {
        var time = t
        for i in stride(from: fingers.count - 1, through: 1, by: -1) {
            touch(time, .pointerUp, Array(fingers[0...i]), pid: fingers[i].id)
            time += 10
        }
        touch(time, .up, [fingers[0]])
        return time
    }

    private func shifted(_ fingers: [Finger], _ dx: Double, _ dy: Double) -> [Finger] {
        fingers.map { ($0.id, $0.x + dx, $0.y + dy) }
    }

    /// Moves all fingers by (dx, dy) in `steps` MOVEs, 8 ms apart. Returns (end time, fingers).
    @discardableResult
    private func moveAll(_ t: UInt32, _ fingers: [Finger], dx: Double, dy: Double, steps: Int = 20) -> (UInt32, [Finger]) {
        var time = t
        for i in 1...steps {
            time = t + UInt32(i * 8)
            touch(time, .move, shifted(fingers, dx * Double(i) / Double(steps), dy * Double(i) / Double(steps)))
        }
        return (time, shifted(fingers, dx, dy))
    }

    private func fingers(_ n: Int) -> [Finger] {
        (0..<n).map { i -> Finger in
            let x: Double = 400 + Double(i) * 40
            let y: Double = i % 2 == 0 ? 250 : 260
            return (UInt8(i), x, y)
        }
    }

    /// A whole n-finger swipe; returns the navigations it produced.
    private func swipe(_ n: Int, dx: Double, dy: Double) -> [NavigationAction] {
        let start = fingers(n)
        let t = fingersDown(0, start)
        let (end, moved) = moveAll(t + 8, start, dx: dx, dy: dy)
        fingersUp(end + 10, moved)
        return navigations
    }

    private var navigations: [NavigationAction] {
        actions.compactMap { if case .navigate(let n) = $0 { return n } else { return nil } }
    }

    private var rightClicks: [CGPoint] {
        actions.compactMap { if case .rightClick(let p) = $0 { return p } else { return nil } }
    }

    private var hasClick: Bool { actions.contains { if case .click = $0 { return true } else { return false } } }

    private var scrollPhases: [ScrollPhase] {
        actions.compactMap { if case let .scroll(_, _, _, phase) = $0 { return phase } else { return nil } }
    }

    private func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x, y: y) }

    // MARK: - Two-finger tap

    func testTwoFingerTapRightClicksWhereTheFirstFingerLanded() {
        touch(0, .down, [(0, 300, 200)])
        touch(20, .pointerDown, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(40, .move, [(0, 302, 201), (1, 361, 206)])          // jitter within the slop
        touch(110, .pointerUp, [(0, 302, 201), (1, 361, 206)], pid: 1)
        touch(130, .up, [(0, 302, 201)])
        XCTAssertEqual(actions, [.move(to: p(300, 200)), .rightClick(at: p(300, 200))])
    }

    func testTwoFingerTapWhenTheFirstFingerLiftsFirst() {
        touch(0, .down, [(0, 300, 200)])
        touch(15, .pointerDown, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(100, .pointerUp, [(0, 300, 200), (1, 360, 205)], pid: 0)
        touch(120, .up, [(1, 360, 205)])
        XCTAssertEqual(rightClicks, [p(300, 200)])
        XCTAssertFalse(hasClick)
    }

    func testSlowTwoFingerTapDoesNothing() {
        touch(0, .down, [(0, 300, 200)])
        touch(20, .pointerDown, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(250, .pointerUp, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(350, .up, [(0, 300, 200)])                          // 350 ms > 300 ms
        XCTAssertEqual(rightClicks, [])
        actions = []
        touch(1000, .down, [(0, 300, 200)])
        touch(1020, .pointerDown, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(1350, .pointerUp, [(0, 300, 200), (1, 360, 205)], pid: 1)   // first lift already too late
        touch(1360, .up, [(0, 300, 200)])
        XCTAssertEqual(rightClicks, [])
        XCTAssertFalse(hasClick)
    }

    func testTwoFingerScrollIsNotARightClick() {
        touch(0, .down, [(0, 400, 200)])
        touch(10, .pointerDown, [(0, 400, 200), (1, 500, 200)], pid: 1)
        touch(60, .move, [(0, 400, 230), (1, 500, 230)])
        touch(100, .move, [(0, 400, 250), (1, 500, 250)])
        touch(150, .pointerUp, [(0, 400, 250), (1, 500, 250)], pid: 1)
        touch(160, .up, [(0, 400, 250)])
        XCTAssertEqual(rightClicks, [])
        XCTAssertEqual(scrollPhases.prefix(3), [.began, .changed, .ended])
    }

    func testPinchIsNotARightClick() {
        touch(0, .down, [(0, 450, 250)])
        touch(10, .pointerDown, [(0, 450, 250), (1, 550, 250)], pid: 1)
        touch(40, .move, [(0, 420, 250), (1, 580, 250)])           // spread 60 > 20 dp → pinch
        touch(80, .move, [(0, 400, 250), (1, 600, 250)])
        touch(120, .pointerUp, [(0, 400, 250), (1, 600, 250)], pid: 1)
        touch(130, .up, [(0, 400, 250)])
        XCTAssertEqual(rightClicks, [])
        XCTAssertTrue(actions.contains { if case .zoom = $0 { return true } else { return false } })
    }

    func testRemainingFingerWanderingCancelsTheTap() {
        touch(0, .down, [(0, 300, 200)])
        touch(20, .pointerDown, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(80, .pointerUp, [(0, 300, 200), (1, 360, 205)], pid: 1)
        touch(100, .move, [(0, 340, 200)])                        // 40 pt > slop
        touch(120, .up, [(0, 340, 200)])
        XCTAssertEqual(actions, [.move(to: p(300, 200))])
    }

    func testSecondFingerAfterAOneFingerScrollIsNoTap() {
        touch(0, .down, [(0, 300, 200)])
        touch(30, .move, [(0, 300, 240)])                         // one-finger scroll
        touch(60, .pointerDown, [(0, 300, 240), (1, 360, 240)], pid: 1)
        touch(90, .pointerUp, [(0, 300, 240), (1, 360, 240)], pid: 1)
        touch(110, .up, [(0, 300, 240)])
        XCTAssertEqual(rightClicks, [])
        XCTAssertEqual(scrollPhases, [.began, .ended])
    }

    func testTwoFingerTapDoesNotTurnTheNextTapIntoADoubleClick() {
        touch(0, .down, [(0, 300, 200)]); touch(60, .up, [(0, 300, 200)])          // single click
        touch(150, .down, [(0, 300, 200)])
        touch(160, .pointerDown, [(0, 300, 200), (1, 360, 200)], pid: 1)
        touch(220, .pointerUp, [(0, 300, 200), (1, 360, 200)], pid: 1)
        touch(230, .up, [(0, 300, 200)])                                         // right click
        touch(300, .down, [(0, 300, 200)]); touch(350, .up, [(0, 300, 200)])      // single again
        XCTAssertEqual(actions.filter { if case .click = $0 { return true } else { return false } },
                       [.click(at: p(300, 200), count: 1), .click(at: p(300, 200), count: 1)])
        XCTAssertEqual(rightClicks, [p(300, 200)])
    }

    // MARK: - 3/4-finger swipes

    func testThreeFingerSwipeDirectionsFollowTheTrackpad() {
        XCTAssertEqual(swipe(3, dx: -200, dy: 0), [.spaceRight], "fingers left → Space to the right")
        actions = []
        XCTAssertEqual(swipe(3, dx: 200, dy: 0), [.spaceLeft], "fingers right → Space to the left")
        actions = []
        XCTAssertEqual(swipe(3, dx: 0, dy: -150), [.missionControl])
        actions = []
        XCTAssertEqual(swipe(3, dx: 0, dy: 150), [.appExpose])
    }

    func testSwipeEmitsNothingButTheNavigation() {
        _ = swipe(3, dx: -200, dy: 0)
        XCTAssertEqual(actions, [.move(to: p(400, 250)), .navigate(.spaceRight)])
    }

    func testFourFingerSwipe() {
        XCTAssertEqual(swipe(4, dx: -200, dy: 0), [.spaceRight])
        actions = []
        XCTAssertEqual(swipe(4, dx: 0, dy: -120), [.missionControl])
        XCTAssertFalse(hasClick)
    }

    func testOnlyOneNavigationPerGestureEvenForLongOrBackAndForthSwipes() {
        let start = fingers(3)
        let t = fingersDown(0, start)
        var (time, now) = moveAll(t + 8, start, dx: -600, dy: 0, steps: 60)   // 10× the threshold
        (time, now) = moveAll(time, now, dx: 700, dy: 0, steps: 60)           // and back the other way
        (time, now) = moveAll(time, now, dx: 0, dy: -300, steps: 30)
        fingersUp(time + 10, now)
        XCTAssertEqual(navigations, [.spaceRight])
    }

    func testDiagonalSwipeUsesTheDominantAxis() {
        XCTAssertEqual(swipe(3, dx: -80, dy: -50), [.spaceRight])
        actions = []
        XCTAssertEqual(swipe(3, dx: 40, dy: 90), [.appExpose])
        actions = []
        XCTAssertEqual(swipe(3, dx: 50, dy: -70), [.missionControl])
    }

    func testSmallThreeFingerJitterDoesNothing() {
        let start = fingers(3)
        let t = fingersDown(0, start)
        var (time, now) = moveAll(t + 8, start, dx: -25, dy: 15, steps: 5)
        (time, now) = moveAll(time, now, dx: 45, dy: -30, steps: 5)
        fingersUp(time + 10, now)
        XCTAssertEqual(navigations, [])
        XCTAssertEqual(actions, [.move(to: p(400, 250))])
    }

    func testThreeFingerTapDoesNothing() {
        let start = fingers(3)
        let t = fingersDown(0, start)
        touch(t + 20, .move, shifted(start, 1, 1))
        fingersUp(t + 60, shifted(start, 1, 1))
        XCTAssertEqual(actions, [.move(to: p(400, 250))], "no click, no right click, no navigation")
        // The next one-finger tap is a plain single click.
        touch(500, .down, [(0, 100, 100)]); touch(550, .up, [(0, 100, 100)])
        XCTAssertEqual(actions.last, .click(at: p(100, 100), count: 1))
    }

    func testThirdFingerMidTwoFingerScrollEndsTheScrollWithoutMomentumOrClick() {
        touch(0, .down, [(0, 400, 200)])
        touch(10, .pointerDown, [(0, 400, 200), (1, 500, 200)], pid: 1)
        for i in 1...6 {   // a fast two-finger flick that would normally coast
            let y = 200 + Double(i) * 20
            touch(UInt32(10 + i * 10), .move, [(0, 400, y), (1, 500, y)])
        }
        XCTAssertEqual(scrollPhases.first, .began)
        touch(75, .pointerDown, [(0, 400, 320), (1, 500, 320), (2, 450, 330)], pid: 2)
        XCTAssertEqual(scrollPhases.last, .ended)
        scheduler.advance(by: 2)
        XCTAssertFalse(scrollPhases.contains(.momentumBegan), "no momentum after a third finger")
        XCTAssertEqual(scheduler.pendingCount, 0)
        let scrollCount = scrollPhases.count
        touch(90, .move, [(0, 400, 330), (1, 500, 330), (2, 450, 340)])
        touch(100, .pointerUp, [(0, 400, 330), (1, 500, 330), (2, 450, 340)], pid: 2)
        touch(110, .move, [(0, 400, 360), (1, 500, 360)])          // two fingers left: still no scroll
        touch(120, .pointerUp, [(0, 400, 360), (1, 500, 360)], pid: 1)
        touch(130, .up, [(0, 400, 360)])
        XCTAssertEqual(scrollPhases.count, scrollCount)
        XCTAssertFalse(hasClick)
        XCTAssertEqual(rightClicks, [])
    }

    func testThirdFingerReleasesAHeldButton() {
        touch(0, .down, [(0, 200, 200)])
        touch(600, .move, [(0, 260, 200)])                        // long-press drag
        XCTAssertTrue(engine.isButtonHeld)
        touch(650, .pointerDown, [(0, 260, 200), (1, 300, 200), (2, 340, 200)], pid: 2)
        XCTAssertFalse(engine.isButtonHeld)
        XCTAssertEqual(actions.last, .buttonUp(.left, at: p(260, 200)))
    }

    func testFingerSetChangesNeverCountAsTravel() {
        let three = fingers(3)                                    // x 400…480
        let t = fingersDown(0, three)
        let four = three + [(3, 950, 250)]                        // centroid jumps ~+120 pt
        touch(t + 10, .pointerDown, four, pid: 3)
        touch(t + 20, .move, shifted(four, 2, 0))
        touch(t + 30, .pointerUp, shifted(four, 2, 0), pid: 3)    // and back
        touch(t + 40, .move, shifted(three, 4, 0))
        touch(t + 50, .pointerUp, shifted(three, 4, 0), pid: 2)   // down to two fingers: no jump either
        touch(t + 60, .move, Array(shifted(three, 6, 0).prefix(2)))
        touch(t + 70, .pointerUp, Array(shifted(three, 6, 0).prefix(2)), pid: 1)
        touch(t + 80, .up, [shifted(three, 6, 0)[0]])
        XCTAssertEqual(navigations, [])
        XCTAssertEqual(actions.count, 1)
    }

    func testMovesWithADifferentFingerSetRebaseline() {
        let three = fingers(3)
        let t = fingersDown(0, three)
        // A MOVE whose pointers differ from the tracked set (no POINTER_* in between) must not jump.
        touch(t + 10, .move, [(0, 900, 250), (1, 940, 250), (5, 980, 250)])
        touch(t + 20, .move, [(0, 905, 250), (1, 945, 250), (5, 985, 250)])
        XCTAssertEqual(navigations, [])
    }

    func testSwipeThresholdIsInPhoneDp() {
        engine.configure(displaySize: size, pointsPerDp: 2)       // 60 dp = 120 pt
        XCTAssertEqual(swipe(3, dx: -100, dy: 0), [], "100 pt = 50 dp")
        actions = []
        XCTAssertEqual(swipe(3, dx: -140, dy: 0), [.spaceRight], "140 pt = 70 dp")
    }

    func testOneAndTwoFingerGesturesStillWorkAfterASwipe() {
        _ = swipe(3, dx: -200, dy: 0)
        actions = []
        touch(1000, .down, [(0, 100, 100)]); touch(1050, .up, [(0, 100, 100)])
        XCTAssertEqual(actions, [.move(to: p(100, 100)), .click(at: p(100, 100), count: 1)])
        actions = []
        touch(2000, .down, [(0, 400, 200)])
        touch(2010, .pointerDown, [(0, 400, 200), (1, 500, 200)], pid: 1)
        touch(2060, .move, [(0, 400, 230), (1, 500, 230)])
        touch(2400, .pointerUp, [(0, 400, 230), (1, 500, 230)], pid: 1)
        touch(2420, .up, [(0, 400, 230)])
        XCTAssertEqual(scrollPhases, [.began, .ended])
    }

    func testCancelAndResetDuringASwipeEmitNothing() {
        let t = fingersDown(0, fingers(3))
        touch(t + 10, .cancel, fingers(3))
        engine.reset()
        XCTAssertEqual(actions, [.move(to: p(400, 250))])
    }

    // MARK: - Shortcuts

    func testNavigationShortcutsAreControlArrowsFlaggedLikeHardware() {
        let flags: CGEventFlags = [.maskControl, .maskSecondaryFn, .maskNumericPad]
        XCTAssertEqual(MacKeyMapping.keyStroke(for: .spaceLeft), KeyStroke(virtualKey: 0x7B, flags: flags))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: .spaceRight), KeyStroke(virtualKey: 0x7C, flags: flags))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: .missionControl), KeyStroke(virtualKey: 0x7E, flags: flags))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: .appExpose), KeyStroke(virtualKey: 0x7D, flags: flags))
    }
}
