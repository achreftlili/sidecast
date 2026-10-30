import Foundation
import SidecastKit

/// Synthetic multi-touch gestures for `--gesture NAME`, sequenced like Android:
/// DOWN for the first finger, POINTER_DOWN (all pointers so far) for each next
/// one, MOVE every 8 ms with every pointer (actionPointerId = first pointer),
/// then POINTER_UP (lifting pointer still listed) down to the last UP.
enum ProbeGesture: String, CaseIterable {
    case tap2
    case swipe3Left = "swipe3-left"
    case swipe3Right = "swipe3-right"
    case swipe3Up = "swipe3-up"
    case swipe3Down = "swipe3-down"
    case swipe4Left = "swipe4-left"
    case tap3
    case scroll2
    // Meant for --input-mode trackpad
    case trackpadMove = "trackpad-move"
    case trackpadTap = "trackpad-tap"
    case trackpadDoubletapDrag = "trackpad-doubletap-drag"

    /// What the Mac's gesture engine should make of it.
    var expected: String {
        switch self {
        case .tap2: return "right click"
        case .swipe3Left, .swipe4Left: return "navigate spaceRight (Control+→)"
        case .swipe3Right: return "navigate spaceLeft (Control+←)"
        case .swipe3Up: return "navigate missionControl (Control+↑)"
        case .swipe3Down: return "navigate appExpose (Control+↓)"
        case .tap3: return "nothing"
        case .scroll2: return "scroll began…ended (+ momentum)"
        case .trackpadMove: return "trackpad: cursor moves right ≈ 0.6 × 20 % of the display"
        case .trackpadTap: return "trackpad: click at the cursor (not at the finger)"
        case .trackpadDoubletapDrag: return "trackpad: click, then button down + drag + button up at the cursor"
        }
    }

    struct Step {
        var ms: Int
        var action: TouchAction
        var actionPointerId: UInt8
        var pointers: [TouchPointer]
    }

    var steps: [Step] {
        var s = Script()
        switch self {
        case .tap2:
            s.down([(0.45, 0.5), (0.55, 0.5)])
            s.wait(30)
            s.move(dx: 0.002, dy: 0.001, duration: 16)          // finger jitter, within the slop
            s.wait(40)
            s.up()
        case .swipe3Left: s.swipe(fingers: 3, from: (0.55, 0.5), dx: -0.25, dy: 0)
        case .swipe3Right: s.swipe(fingers: 3, from: (0.3, 0.5), dx: 0.25, dy: 0)
        case .swipe3Up: s.swipe(fingers: 3, from: (0.45, 0.65), dx: 0, dy: -0.3)
        case .swipe3Down: s.swipe(fingers: 3, from: (0.45, 0.35), dx: 0, dy: 0.3)
        case .swipe4Left: s.swipe(fingers: 4, from: (0.5, 0.5), dx: -0.25, dy: 0)
        case .tap3:
            s.down([(0.4, 0.5), (0.47, 0.52), (0.54, 0.5)])
            s.wait(20)
            s.move(dx: 0.002, dy: -0.002, duration: 24)
            s.wait(20)
            s.up()
        case .scroll2:
            s.down([(0.45, 0.35), (0.55, 0.35)])
            s.wait(20)
            s.move(dx: 0, dy: 0.25, duration: 160)              // a flick: momentum should follow
            s.up(spacing: 8)
        case .trackpadMove:
            s.down([(0.3, 0.5)])
            s.wait(16)
            s.move(dx: 0.2, dy: 0, duration: 1_000)             // slow: about 180 dp/s on a phone
            s.up()
        case .trackpadTap:
            s.down([(0.8, 0.2)])                                // far from the cursor on purpose
            s.wait(70)
            s.up()
        case .trackpadDoubletapDrag:
            s.down([(0.5, 0.6)])
            s.wait(60)
            s.up()
            s.wait(120)                                         // second touch within 350 ms
            s.down([(0.5, 0.6)])
            s.wait(60)
            s.move(dx: -0.1, dy: 0.05, duration: 300)
            s.up()
        }
        return s.steps
    }

    /// Builds a timeline; positions are normalized (clamped to 0…1).
    private struct Script {
        var steps: [Step] = []
        var t = 0
        var fingers: [TouchPointer] = []

        mutating func down(_ positions: [(Float, Float)], spacing: Int = 12) {
            for (i, p) in positions.enumerated() {
                if i > 0 { t += spacing }
                fingers.append(TouchPointer(id: UInt8(i), x: p.0, y: p.1))
                steps.append(Step(ms: t, action: i == 0 ? .down : .pointerDown, actionPointerId: UInt8(i), pointers: fingers))
            }
        }

        mutating func wait(_ ms: Int) { t += ms }

        mutating func move(dx: Float, dy: Float, duration: Int, every: Int = 8) {
            let count = max(1, duration / every)
            let start = fingers
            for k in 1...count {
                t += every
                let f = Float(k) / Float(count)
                fingers = start.map { TouchPointer(id: $0.id, x: clamp($0.x + dx * f), y: clamp($0.y + dy * f)) }
                steps.append(Step(ms: t, action: .move, actionPointerId: fingers[0].id, pointers: fingers))
            }
        }

        mutating func up(spacing: Int = 12) {
            while fingers.count > 1 {
                t += spacing
                let lifting = fingers[fingers.count - 1]
                steps.append(Step(ms: t, action: .pointerUp, actionPointerId: lifting.id, pointers: fingers))
                fingers.removeLast()
            }
            t += spacing
            steps.append(Step(ms: t, action: .up, actionPointerId: fingers[0].id, pointers: fingers))
            fingers = []
        }

        mutating func swipe(fingers n: Int, from origin: (Float, Float), dx: Float, dy: Float) {
            down((0..<n).map { (origin.0 + Float($0) * 0.06, origin.1 + ($0 % 2 == 0 ? 0 : 0.02)) })
            wait(16)
            move(dx: dx, dy: dy, duration: 200)
            up()
        }

        private func clamp(_ v: Float) -> Float { min(max(v, 0), 1) }
    }
}
