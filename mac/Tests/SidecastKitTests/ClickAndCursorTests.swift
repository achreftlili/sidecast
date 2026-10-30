import CoreGraphics
import XCTest
@testable import SidecastKit

/// Human-like click timelines and the CURSOR stream (PROTOCOL §7.3, DECISIONS Q32).
final class ClickAndCursorTests: XCTestCase {
    private let p = CGPoint(x: 100, y: 50)

    // MARK: - Click timelines

    func testClickIsMoveSettleDownHoldUp() throws {
        let steps = try XCTUnwrap(ClickTimeline.steps(for: .click(at: p, count: 1)))
        XCTAssertEqual(steps, [
            TimelineStep(0, MouseEventSpec(.moved, p)),
            TimelineStep(12, MouseEventSpec(.leftDown, p, clickState: 1, pressure: 1)),
            TimelineStep(40, MouseEventSpec(.leftUp, p, clickState: 1, pressure: 0)),
        ])
    }

    func testDoubleClickCarriesClickStateTwo() throws {
        let steps = try XCTUnwrap(ClickTimeline.steps(for: .click(at: p, count: 2)))
        XCTAssertEqual(steps.map(\.event.clickState), [nil, 2, 2])
        XCTAssertEqual(steps.map(\.delayMs), [0, 12, 40])
    }

    func testRightClickTimeline() throws {
        let steps = try XCTUnwrap(ClickTimeline.steps(for: .rightClick(at: p)))
        XCTAssertEqual(steps.map(\.event.kind), [.moved, .rightDown, .rightUp])
        XCTAssertEqual(steps.map(\.delayMs), [0, 12, 40])
        XCTAssertEqual(steps.map(\.event.pressure), [nil, 1, 0])
    }

    func testDragStartMovesThenPressesAndDragStepsAreImmediate() throws {
        XCTAssertEqual(try XCTUnwrap(ClickTimeline.steps(for: .buttonDown(.left, at: p))).map(\.event.kind), [.moved, .leftDown])
        XCTAssertEqual(ClickTimeline.steps(for: .drag(to: p)), [TimelineStep(0, MouseEventSpec(.leftDragged, p, clickState: 1, pressure: 1))])
        XCTAssertEqual(ClickTimeline.steps(for: .buttonUp(.left, at: p)), [TimelineStep(0, MouseEventSpec(.leftUp, p, clickState: 1, pressure: 0))])
        XCTAssertEqual(ClickTimeline.steps(for: .move(to: p)), [TimelineStep(0, MouseEventSpec(.moved, p))])
        XCTAssertNil(ClickTimeline.steps(for: .scroll(dx: 1, dy: 1, at: p, phase: .changed)))
        XCTAssertNil(ClickTimeline.steps(for: .zoom(delta: 1, at: p)))
        XCTAssertNil(ClickTimeline.steps(for: .navigate(.missionControl)))
    }

    func testTimingComesFromTheThresholds() throws {
        var thresholds = GestureThresholds()
        XCTAssertEqual(thresholds.clickTiming, ClickTiming(settleMs: 12, holdMs: 40))
        thresholds.clickTiming = ClickTiming(settleMs: 5, holdMs: 60)
        let steps = try XCTUnwrap(ClickTimeline.steps(for: .click(at: p, count: 1), timing: thresholds.clickTiming))
        XCTAssertEqual(steps.map(\.delayMs), [0, 5, 60])
    }

    /// A fake poster with a fake clock: events of later actions never come before an earlier click's button-up.
    func testPlayingKeepsStrictOrderAcrossActions() {
        var clock = 0
        var log: [(Int, MouseEventSpec.Kind)] = []
        let actions: [MouseAction] = [.click(at: p, count: 1), .move(to: CGPoint(x: 300, y: 300)),
                                      .rightClick(at: p), .buttonDown(.left, at: p), .drag(to: CGPoint(x: 110, y: 50)),
                                      .buttonUp(.left, at: CGPoint(x: 110, y: 50))]
        for action in actions {
            ClickTimeline.play(ClickTimeline.steps(for: action)!, sleep: { clock += $0 }, post: { log.append((clock, $0.kind)) })
        }
        XCTAssertEqual(log.map(\.1), [.moved, .leftDown, .leftUp, .moved, .moved, .rightDown, .rightUp,
                                      .moved, .leftDown, .leftDragged, .leftUp])
        XCTAssertEqual(log.map(\.0), [0, 12, 52, 52, 52, 64, 104, 104, 116, 116, 116])
        XCTAssertEqual(log.map(\.0), log.map(\.0).sorted(), "time never goes backwards")
    }

    func testDescribe() throws {
        let steps = try XCTUnwrap(ClickTimeline.steps(for: .click(at: p, count: 2)))
        XCTAssertEqual(ClickTimeline.describe(steps), "moved (100, 50) → +12 ms leftDown (100, 50) cs2 → +40 ms leftUp (100, 50) cs2")
    }

    func testPointerLocation() {
        XCTAssertEqual(MouseAction.click(at: p, count: 1).pointerLocation, p)
        XCTAssertEqual(MouseAction.drag(to: p).pointerLocation, p)
        XCTAssertNil(MouseAction.scroll(dx: 0, dy: 0, at: p, phase: .ended).pointerLocation)
        XCTAssertNil(MouseAction.navigate(.appExpose).pointerLocation)
    }

    // MARK: - CURSOR codec

    // §8.3 CURSOR x = 0.5, y = 0.25
    func testCursorVector() throws {
        let wire = Data([0x89, 0x00, 0x00, 0x00, 0x08, 0x3f, 0x00, 0x00, 0x00, 0x3e, 0x80, 0x00, 0x00])
        XCTAssertEqual(ServerMessage.cursor(CursorPosition(x: 0.5, y: 0.25)).encoded(), wire)
        let parser = FrameParser()
        parser.append(wire)
        XCTAssertEqual(try ServerMessage.decode(try XCTUnwrap(try parser.nextFrame())), .cursor(CursorPosition(x: 0.5, y: 0.25)))
    }

    func testCursorOffDisplayAndTruncation() throws {
        let off = ServerMessage.cursor(.offDisplay).encoded()
        XCTAssertEqual(Array(off.dropFirst(5)), [0xbf, 0x80, 0, 0, 0xbf, 0x80, 0, 0])   // -1.0, -1.0
        XCTAssertFalse(CursorPosition.offDisplay.isOnDisplay)
        XCTAssertTrue(CursorPosition(x: 0, y: 1).isOnDisplay)
        XCTAssertThrowsError(try ServerMessage.decode(Frame(.cursor, payload: Data([0x3f, 0, 0, 0]))))
    }

    func testCursorNormalisation() {
        let size = CGSize(width: 1200, height: 540)
        XCTAssertEqual(CursorPosition(point: CGPoint(x: 600, y: 135), in: size), CursorPosition(x: 0.5, y: 0.25))
        XCTAssertEqual(CursorPosition(point: CGPoint(x: -20, y: 9000), in: size), CursorPosition(x: 0, y: 1), "clamped")
        XCTAssertEqual(CursorPosition(point: CGPoint(x: 1, y: 1), in: .zero), .offDisplay)
    }

    // MARK: - Throttle

    private func pos(_ x: Float) -> CursorPosition { CursorPosition(x: x, y: 0.5) }

    func testThrottleSendsAtMostSixtyPerSecondAndKeepsTheLastPosition() {
        var throttle = CursorThrottle()
        var sent: [(TimeInterval, CursorPosition)] = []
        var flushAt: TimeInterval?
        // 1000 positions in one second (a 1 kHz mover), flushing whenever asked to.
        for i in 0..<1000 {
            let now = Double(i) / 1000
            if let at = flushAt, now >= at {
                flushAt = nil
                if let p = throttle.flush(at: now) { sent.append((now, p)) }
            }
            switch throttle.offer(pos(Float(i) / 1000), at: now) {
            case .send(let p): sent.append((now, p))
            case .deferred(let until): flushAt = flushAt ?? until
            case .unchanged: break
            }
        }
        if let at = flushAt, let p = throttle.flush(at: at) { sent.append((at, p)) }
        XCTAssertLessThanOrEqual(sent.count, 61)
        XCTAssertGreaterThanOrEqual(sent.count, 55)
        for pair in zip(sent, sent.dropFirst()) {
            XCTAssertGreaterThanOrEqual(pair.1.0 - pair.0.0, 1.0 / 60 - 1e-9)
        }
        XCTAssertEqual(sent.last?.1, pos(0.999), "the final position is always delivered")
    }

    func testThrottleSkipsUnchangedUnlessForced() {
        var throttle = CursorThrottle()
        XCTAssertEqual(throttle.offer(pos(0.1), at: 0), .send(pos(0.1)))
        XCTAssertEqual(throttle.offer(pos(0.1), at: 1), .unchanged)
        XCTAssertEqual(throttle.offer(pos(0.1), at: 2, force: true), .send(pos(0.1)))
        XCTAssertEqual(throttle.offer(pos(0.2), at: 2.001), .deferred(until: 2 + 1.0 / 60))
        XCTAssertNil(throttle.flush(at: 2.005), "too early")
        XCTAssertEqual(throttle.flush(at: 2.02), pos(0.2))
        XCTAssertNil(throttle.flush(at: 3), "nothing pending")
        throttle.reset()
        XCTAssertEqual(throttle.offer(pos(0.2), at: 3), .send(pos(0.2)), "after reset the client is assumed to know nothing")
    }
}
