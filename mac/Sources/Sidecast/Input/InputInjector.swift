import CoreGraphics
import Foundation
import SidecastKit

/// Posts gesture-engine actions as CGEvents at the HID level, mapped onto a
/// display's global bounds (ported from SideScreen's event injection).
/// Tracks held buttons so they can always be released (SideScreen #71).
///
/// Clicks are human-like (`ClickTimeline`): move → 12 ms → down → 40 ms → up,
/// with click state and pressure set. The waits block the caller's serial
/// input queue on purpose, so no later touch is injected before the button-up.
final class InputInjector {
    /// Delays of human-like clicks (one place: `GestureThresholds.clickTiming`).
    var clickTiming = GestureThresholds().clickTiming
    private let source = CGEventSource(stateID: .hidSystemState)
    private var leftDown = false
    private var rightDown = false
    private var lastLocation = CGPoint.zero
    private var scrollRemainder = (dx: 0.0, dy: 0.0)

    /// Global bounds (points) of the target display; re-read for every action.
    var boundsProvider: () -> CGRect = { .zero }

    func perform(_ action: MouseAction) {
        let bounds = boundsProvider()
        guard bounds.width > 0, bounds.height > 0 else { return }
        func global(_ p: CGPoint) -> CGPoint {
            let g = CGPoint(x: bounds.minX + p.x, y: bounds.minY + p.y)
            return CGPoint(x: min(max(g.x, bounds.minX), bounds.maxX - 1), y: min(max(g.y, bounds.minY), bounds.maxY - 1))
        }
        if let steps = ClickTimeline.steps(for: action, timing: clickTiming) {
            ClickTimeline.play(steps, sleep: { usleep(useconds_t($0) * 1000) }, post: { spec in
                self.post(spec, at: global(spec.point))
            })
            return
        }
        switch action {
        case .move, .buttonDown, .drag, .buttonUp, .click, .rightClick:
            break   // timelines above
        case .scroll(let dx, let dy, let p, let phase):
            scroll(dx: dx, dy: dy, at: global(p), phase: phase)
        case .zoom(let delta, let p):
            // ⌘-scroll zooms in most Mac apps (SideScreen behaviour).
            guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1,
                                      wheel1: Int32(clamping: delta), wheel2: 0, wheel3: 0) else { return }
            event.location = global(p)
            event.flags = .maskCommand
            event.post(tap: .cghidEventTap)
        case .navigate(let navigation):
            // Mission Control shortcuts (Control + arrow), via the KEY path.
            press(MacKeyMapping.keyStroke(for: navigation))
        }
    }

    /// Types `text` as Unicode, independent of the keyboard layout: one
    /// keyDown/keyUp pair (virtual key 0 + Unicode string) per chunk of
    /// ≤ 20 UTF-16 units, split between grapheme clusters. Modifier flags are
    /// cleared so a key held on the Mac does not turn the text into shortcuts.
    func typeText(_ text: String) {
        for chunk in UnicodeTyping.chunks(UnicodeTyping.removingControlCharacters(text)) {
            let units = Array(chunk.utf16)
            for keyDown in [true, false] {
                guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: keyDown) else { continue }
                event.flags = []
                units.withUnsafeBufferPointer { buffer in
                    event.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress)
                }
                event.post(tap: .cghidEventTap)
            }
        }
    }

    /// Presses and releases one key with exactly `stroke.flags` held.
    func press(_ stroke: KeyStroke) {
        for keyDown in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: stroke.virtualKey,
                                      keyDown: keyDown) else { continue }
            event.flags = stroke.flags
            event.post(tap: .cghidEventTap)
        }
    }

    /// Releases any button still held (disconnect, cancel, accessibility revoked).
    func releaseAll() {
        if leftDown { release(.left, at: lastLocation) }
        if rightDown { release(.right, at: lastLocation) }
        scrollRemainder = (0, 0)
    }

    // MARK: - Private

    private func release(_ button: MouseButton, at point: CGPoint) {
        post(MouseEventSpec(button == .left ? .leftUp : .rightUp, point, clickState: 1, pressure: 0), at: point)
    }

    private func mouse(_ type: CGEventType, at point: CGPoint, button: CGMouseButton, clickState: Int64? = nil,
                       pressure: Double? = nil) {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
                                  mouseButton: button) else { return }
        if let clickState { event.setIntegerValueField(.mouseEventClickState, value: clickState) }
        if let pressure { event.setDoubleValueField(.mouseEventPressure, value: pressure) }
        event.post(tap: .cghidEventTap)
        lastLocation = point
    }

    /// One timeline event at a global point; keeps the held-button bookkeeping.
    private func post(_ spec: MouseEventSpec, at point: CGPoint) {
        switch spec.kind {
        case .moved:
            mouse(.mouseMoved, at: point, button: .left)
        case .leftDown:
            mouse(.leftMouseDown, at: point, button: .left, clickState: spec.clickState, pressure: spec.pressure)
            leftDown = true
        case .leftDragged:
            mouse(.leftMouseDragged, at: point, button: .left, clickState: spec.clickState, pressure: spec.pressure)
        case .leftUp:
            mouse(.leftMouseUp, at: point, button: .left, clickState: spec.clickState, pressure: spec.pressure)
            leftDown = false
        case .rightDown:
            mouse(.rightMouseDown, at: point, button: .right, clickState: spec.clickState, pressure: spec.pressure)
            rightDown = true
        case .rightUp:
            mouse(.rightMouseUp, at: point, button: .right, clickState: spec.clickState, pressure: spec.pressure)
            rightDown = false
        }
    }

    /// Pixel scroll with gesture/momentum phases; sub-pixel remainders carry over.
    private func scroll(dx: Double, dy: Double, at point: CGPoint, phase: ScrollPhase) {
        let totalX = dx + scrollRemainder.dx, totalY = dy + scrollRemainder.dy
        let ix = Int32(clamping: Int(totalX.rounded(.towardZero)))
        let iy = Int32(clamping: Int(totalY.rounded(.towardZero)))
        scrollRemainder = (totalX - Double(ix), totalY - Double(iy))
        let isEdge = [.began, .ended, .momentumBegan, .momentumEnded].contains(phase)
        guard ix != 0 || iy != 0 || isEdge,
              let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2,
                                  wheel1: iy, wheel2: ix, wheel3: 0) else { return }
        event.location = point
        // kCGScrollPhase*: began 1, changed 2, ended 4; kCGMomentumScrollPhase*: begin 1, continue 2, end 3.
        let (scrollPhase, momentumPhase): (Int64, Int64) = {
            switch phase {
            case .began: return (1, 0)
            case .changed: return (2, 0)
            case .ended: return (4, 0)
            case .momentumBegan: return (0, 1)
            case .momentum: return (0, 2)
            case .momentumEnded: return (0, 3)
            }
        }()
        event.setIntegerValueField(.scrollWheelEventScrollPhase, value: scrollPhase)
        event.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentumPhase)
        if phase == .ended || phase == .momentumEnded { scrollRemainder = (0, 0) }
        event.post(tap: .cghidEventTap)
    }
}
