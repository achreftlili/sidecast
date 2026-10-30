import CoreGraphics
import XCTest
@testable import SidecastKit

final class GestureEngineTests: XCTestCase {
    private var scheduler: ManualScheduler!
    private var engine: GestureEngine!
    private var actions: [MouseAction] = []
    /// Power-of-two size: integer positions are exact as normalized Float32.
    private let size = CGSize(width: 1024, height: 512)

    override func setUp() {
        scheduler = ManualScheduler()
        actions = []
        engine = GestureEngine(scheduler: scheduler) { [unowned self] in self.actions.append($0) }
        engine.configure(displaySize: size)
    }

    // MARK: helpers (positions in display points)

    private func touch(_ t: UInt32, _ action: TouchAction, _ points: [(UInt8, Double, Double)], pid: UInt8? = nil) {
        let pointers = points.map { TouchPointer(id: $0.0, x: Float($0.1 / 1024), y: Float($0.2 / 512)) }
        engine.handle(TouchEvent(eventTimeMs: t, action: action, actionPointerId: pid ?? points[0].0, pointers: pointers))
    }

    private func down(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .down, [(0, x, y)]) }
    private func move(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .move, [(0, x, y)]) }
    private func up(_ t: UInt32, _ x: Double, _ y: Double) { touch(t, .up, [(0, x, y)]) }

    private func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x, y: y) }

    private var scrolls: [(Double, Double, ScrollPhase)] {
        actions.compactMap { if case let .scroll(dx, dy, _, phase) = $0 { return (dx, dy, phase) } else { return nil } }
    }

    // MARK: taps

    func testTapClicks() {
        down(1000, 100, 100)
        move(1050, 104, 102)                 // within slop
        up(1100, 104, 102)
        XCTAssertEqual(actions, [.move(to: p(100, 100)), .click(at: p(104, 102), count: 1)])
    }

    func testDoubleTap() {
        down(0, 100, 100); up(80, 100, 100)
        down(200, 105, 103); up(280, 105, 103)
        XCTAssertEqual(actions, [
            .move(to: p(100, 100)), .click(at: p(100, 100), count: 1),
            .move(to: p(105, 103)), .click(at: p(100, 100), count: 2),
        ])
        // A third quick tap is a fresh single click.
        down(400, 100, 100); up(450, 100, 100)
        XCTAssertEqual(actions.last, .click(at: p(100, 100), count: 1))
    }

    func testSlowSecondTapIsAnotherSingleClick() {
        down(0, 100, 100); up(80, 100, 100)
        down(900, 100, 100); up(980, 100, 100)
        XCTAssertEqual(actions.filter { if case .click(_, 1) = $0 { return true } else { return false } }.count, 2)
    }

    func testFarSecondTapIsAnotherSingleClick() {
        down(0, 100, 100); up(80, 100, 100)
        down(150, 300, 300); up(200, 300, 300)
        XCTAssertEqual(actions.last, .click(at: p(300, 300), count: 1))
    }

    func testTimestampsMayWrap() {
        down(UInt32.max - 50, 10, 10)
        up(30, 10, 10)                       // 81 ms later across the wrap
        XCTAssertEqual(actions.last, .click(at: p(10, 10), count: 1))
    }

    // MARK: long press

    func testLongPressRightClicks() {
        down(0, 200, 200)
        up(650, 201, 200)
        XCTAssertEqual(actions, [.move(to: p(200, 200)), .rightClick(at: p(201, 200))])
    }

    func testLongPressDrag() {
        down(0, 200, 200)
        move(300, 202, 200)                  // still within slop, too early
        move(600, 260, 200)                  // held 0.6 s, then moved → drag
        move(620, 300, 210)
        up(640, 300, 210)
        XCTAssertEqual(actions, [
            .move(to: p(200, 200)),
            .buttonDown(.left, at: p(200, 200)),
            .drag(to: p(260, 200)),
            .drag(to: p(300, 210)),
            .drag(to: p(300, 210)),
            .buttonUp(.left, at: p(300, 210)),
        ])
        XCTAssertFalse(engine.isButtonHeld)
    }

    func testResetReleasesHeldButton() {
        down(0, 200, 200)
        move(600, 260, 200)
        XCTAssertTrue(engine.isButtonHeld)
        engine.reset()
        XCTAssertEqual(actions.last, .buttonUp(.left, at: p(260, 200)))
        XCTAssertFalse(engine.isButtonHeld)
        // Idle reset emits nothing.
        let count = actions.count
        engine.reset()
        XCTAssertEqual(actions.count, count)
    }

    func testCancelReleasesButtonWithoutClicking() {
        down(0, 200, 200)
        move(600, 260, 200)
        touch(700, .cancel, [(0, 260, 200)])
        XCTAssertEqual(actions.last, .buttonUp(.left, at: p(260, 200)))
        down(800, 10, 10); touch(850, .cancel, [(0, 10, 10)])
        XCTAssertFalse(actions.contains { if case .click = $0 { return true } else { return false } })
    }

    func testDownWithoutUpReleasesPreviousDrag() {
        down(0, 200, 200)
        move(600, 260, 200)
        down(1000, 50, 50)                   // UP was lost
        XCTAssertEqual(actions.suffix(2), [.buttonUp(.left, at: p(260, 200)), .move(to: p(50, 50))])
    }

    // MARK: scrolling

    func testOneFingerScrollPhases() {
        down(0, 500, 300)
        move(100, 500, 280)                  // crosses the 15 pt slop → scroll began
        move(200, 500, 270)
        up(400, 500, 270)                    // lifted long after the last move: no momentum
        let s = scrolls
        XCTAssertEqual(s.map(\.2), [.began, .changed, .ended])
        XCTAssertEqual(s[0].1, -24, accuracy: 1e-9)          // -20 × 1.2
        XCTAssertEqual(s[1].1, -12, accuracy: 1e-9)
        scheduler.advance(by: 1)
        XCTAssertEqual(scrolls.count, 3)
        XCTAssertFalse(actions.contains { if case .click = $0 { return true } else { return false } })
    }

    func testFlickStartsMomentumThatDecaysAndEnds() {
        down(0, 500, 400)
        var y = 400.0
        for i in 1...6 {
            y -= 20
            move(UInt32(i * 10), 500, y)
        }
        up(65, 500, y)
        let before = scrolls.count
        XCTAssertEqual(scrolls.last?.2, .ended)
        scheduler.advance(by: 3)
        let momentum = Array(scrolls.dropFirst(before))
        XCTAssertEqual(momentum.first?.2, .momentumBegan)
        XCTAssertEqual(momentum.last?.2, .momentumEnded)
        let magnitudes = momentum.dropLast().map { abs($0.1) }
        XCTAssertGreaterThan(magnitudes.count, 5)
        XCTAssertEqual(magnitudes, magnitudes.sorted(by: >))   // decaying
        XCTAssertTrue(momentum.dropLast().allSatisfy { $0.1 < 0 })  // same direction as the flick
        XCTAssertEqual(scheduler.pendingCount, 0)
    }

    func testNewTouchStopsMomentum() {
        down(0, 500, 400)
        for i in 1...6 { move(UInt32(i * 10), 500, 400 - Double(i) * 20) }
        up(65, 500, 280)
        scheduler.advance(by: 0.05)
        down(200, 100, 100)
        XCTAssertEqual(scrolls.last?.2, .momentumEnded)
        let count = scrolls.count
        scheduler.advance(by: 2)
        XCTAssertEqual(scrolls.count, count)
    }

    // MARK: two fingers

    func testTwoFingerScroll() {
        touch(0, .down, [(0, 400, 200)])
        touch(10, .pointerDown, [(0, 400, 200), (1, 500, 200)], pid: 1)
        touch(60, .move, [(0, 400, 230), (1, 500, 230)])    // midpoint +30 → scroll
        touch(110, .move, [(0, 400, 250), (1, 500, 250)])
        touch(400, .pointerUp, [(0, 400, 250), (1, 500, 250)], pid: 1)
        touch(420, .move, [(0, 380, 260)])                  // leftover finger is ignored
        touch(450, .up, [(0, 380, 260)])
        XCTAssertEqual(scrolls.map(\.2), [.began, .changed, .ended])
        XCTAssertEqual(scrolls[1].1, 24, accuracy: 1e-9)
        XCTAssertFalse(actions.contains { if case .click = $0 { return true } else { return false } })
        XCTAssertFalse(actions.contains { if case .zoom = $0 { return true } else { return false } })
    }

    func testPinchZooms() {
        touch(0, .down, [(0, 450, 250)])
        touch(10, .pointerDown, [(0, 450, 250), (1, 550, 250)], pid: 1)
        for i in 1...10 {
            let spread = Double(i) * 10
            touch(UInt32(10 + i * 20), .move, [(0, 450 - spread, 250), (1, 550 + spread, 250)])
        }
        let zooms = actions.compactMap { if case let .zoom(d, _) = $0 { return d } else { return nil } }
        XCTAssertFalse(zooms.isEmpty)
        XCTAssertTrue(zooms.allSatisfy { $0 > 0 })
        XCTAssertEqual(zooms.reduce(0, +), 90)    // 0.5 × (300 − 120): growth after the 20 dp threshold

        actions = []
        for i in 1...5 {
            let spread = 100 - Double(i) * 15
            touch(UInt32(300 + i * 20), .move, [(0, 450 - spread, 250), (1, 550 + spread, 250)])
        }
        let zoomOut = actions.compactMap { if case let .zoom(d, _) = $0 { return d } else { return nil } }
        XCTAssertTrue(zoomOut.allSatisfy { $0 < 0 })
    }

    func testSecondFingerDuringDragReleasesButton() {
        down(0, 200, 200)
        move(600, 260, 200)
        touch(700, .pointerDown, [(0, 260, 200), (1, 300, 200)], pid: 1)
        XCTAssertEqual(actions.last, .buttonUp(.left, at: p(260, 200)))
        XCTAssertFalse(engine.isButtonHeld)
    }

    // MARK: geometry

    func testCoordinatesAreClampedOntoTheDisplay() {
        engine.handle(TouchEvent(eventTimeMs: 0, action: .down, actionPointerId: 0,
                                 pointers: [TouchPointer(id: 0, x: 1.3, y: -0.2)]))
        XCTAssertEqual(actions, [.move(to: p(1023, 0))])
        engine.handle(TouchEvent(eventTimeMs: 10, action: .up, actionPointerId: 0,
                                 pointers: [TouchPointer(id: 0, x: .nan, y: .infinity)]))
    }

    func testSlopScalesWithPointsPerDp() {
        engine.configure(displaySize: size, pointsPerDp: 2)
        down(0, 100, 100)
        move(50, 125, 100)                   // 25 pt = 12.5 dp < 15 dp → still a tap
        up(100, 125, 100)
        XCTAssertEqual(actions.last, .click(at: p(125, 100), count: 1))
    }

    func testIgnoredWithoutDisplaySize() {
        engine.configure(displaySize: .zero)
        down(0, 1, 1); up(10, 1, 1)
        XCTAssertTrue(actions.isEmpty)
    }
}
