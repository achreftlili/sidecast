import CoreGraphics
import Foundation

/// Delays of human-like clicks (PROTOCOL §7.3, DECISIONS Q32): Chromium /
/// Electron apps (VS Code) ignore zero-duration synthetic clicks.
public struct ClickTiming: Equatable, Sendable {
    /// Pointer move → button down.
    public var settleMs: Int
    /// Button down → button up.
    public var holdMs: Int

    public init(settleMs: Int = 12, holdMs: Int = 40) {
        self.settleMs = max(0, settleMs)
        self.holdMs = max(0, holdMs)
    }
}

/// One mouse CGEvent to post.
public struct MouseEventSpec: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case moved, leftDown, leftUp, leftDragged, rightDown, rightUp
    }

    public var kind: Kind
    /// Display-local point.
    public var point: CGPoint
    /// `.mouseEventClickState` (nil for plain moves).
    public var clickState: Int64?
    /// `.mouseEventPressure`: 1 while a button is down, 0 on release (nil for plain moves).
    public var pressure: Double?

    public init(_ kind: Kind, _ point: CGPoint, clickState: Int64? = nil, pressure: Double? = nil) {
        self.kind = kind
        self.point = point
        self.clickState = clickState
        self.pressure = pressure
    }
}

/// An event preceded by a wait.
public struct TimelineStep: Equatable, Sendable {
    public var delayMs: Int
    public var event: MouseEventSpec

    public init(_ delayMs: Int, _ event: MouseEventSpec) {
        self.delayMs = delayMs
        self.event = event
    }
}

/// Turns pointer/button actions into timed event sequences:
/// - click: move → settle → down(clickState n) → hold → up(clickState n)
/// - right click: move → settle → right down → hold → right up
/// - drag start: move → settle → down; drag steps and releases are immediate
/// Scroll, zoom and navigation are not mouse-button timelines (nil).
public enum ClickTimeline {
    public static func steps(for action: MouseAction, timing: ClickTiming = ClickTiming()) -> [TimelineStep]? {
        switch action {
        case .move(let p):
            return [TimelineStep(0, MouseEventSpec(.moved, p))]
        case .click(let p, let count):
            let n = Int64(max(1, count))
            return [
                TimelineStep(0, MouseEventSpec(.moved, p)),
                TimelineStep(timing.settleMs, MouseEventSpec(.leftDown, p, clickState: n, pressure: 1)),
                TimelineStep(timing.holdMs, MouseEventSpec(.leftUp, p, clickState: n, pressure: 0)),
            ]
        case .rightClick(let p):
            return [
                TimelineStep(0, MouseEventSpec(.moved, p)),
                TimelineStep(timing.settleMs, MouseEventSpec(.rightDown, p, clickState: 1, pressure: 1)),
                TimelineStep(timing.holdMs, MouseEventSpec(.rightUp, p, clickState: 1, pressure: 0)),
            ]
        case .buttonDown(let button, let p):
            return [
                TimelineStep(0, MouseEventSpec(.moved, p)),
                TimelineStep(timing.settleMs, MouseEventSpec(button == .left ? .leftDown : .rightDown, p,
                                                             clickState: 1, pressure: 1)),
            ]
        case .drag(let p):
            return [TimelineStep(0, MouseEventSpec(.leftDragged, p, clickState: 1, pressure: 1))]
        case .buttonUp(let button, let p):
            return [TimelineStep(0, MouseEventSpec(button == .left ? .leftUp : .rightUp, p, clickState: 1, pressure: 0))]
        case .scroll, .zoom, .navigate:
            return nil
        }
    }

    /// Plays steps strictly in order: `sleep(ms)` before each delayed event, then `post`.
    /// The caller runs it on its serial input queue, so later events wait for it.
    public static func play(_ steps: [TimelineStep], sleep: (Int) -> Void, post: (MouseEventSpec) -> Void) {
        for step in steps {
            if step.delayMs > 0 { sleep(step.delayMs) }
            post(step.event)
        }
    }

    /// "moved (10, 20) → +12 ms leftDown cs1 → +40 ms leftUp cs1" (logs).
    public static func describe(_ steps: [TimelineStep]) -> String {
        steps.enumerated().map { i, step in
            let e = step.event
            let base = String(format: "%@ (%.0f, %.0f)", "\(e.kind)", e.point.x, e.point.y)
                + (e.clickState.map { " cs\($0)" } ?? "")
            return i == 0 && step.delayMs == 0 ? base : "+\(step.delayMs) ms \(base)"
        }.joined(separator: " → ")
    }
}
