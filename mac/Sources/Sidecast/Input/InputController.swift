import ApplicationServices
import CoreGraphics
import Foundation
import SidecastKit

/// Touch → gesture engine → CGEvents for the virtual display, plus keyboard
/// input (TEXT / KEY) for the focused app, on one serial queue so touches and
/// keys stay in order. Input is ignored while Accessibility is not granted and
/// while no display is configured (not streaming).
final class InputController {
    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.input", qos: .userInteractive)
    private let injector = InputInjector()
    private lazy var engine: GestureEngine = {
        let engine = GestureEngine(scheduler: DispatchQueueScheduler(queue: queue)) { [weak self] action in
            guard let self else { return }
            self.injector.perform(action)
            // Trackpad mode: the phone draws the pointer (CURSOR).
            if self.mode == .trackpad, let p = action.pointerLocation { self.report(CursorPosition(point: p, in: self.displaySize)) }
        }
        engine.trace = { Log.info("input: \($0)") }
        // Trackpad mode starts from the real cursor when it is on our display.
        engine.cursorProvider = { [weak self] in
            guard let self, self.displayID != kCGNullDirectDisplay, let location = CGEvent(source: nil)?.location else {
                return nil
            }
            let bounds = CGDisplayBounds(self.displayID)
            guard bounds.contains(location) else { return nil }
            return CGPoint(x: location.x - bounds.minX, y: location.y - bounds.minY)
        }
        return engine
    }()
    private var displayID: CGDirectDisplayID = kCGNullDirectDisplay
    private var displaySize = CGSize.zero
    private var mode: InputMode = .touch
    private var cursorSampler: DispatchSourceTimer?
    private var lastReported: CursorPosition?

    /// Trackpad mode: pointer positions for CURSOR (input queue). The Bool forces a send.
    var onCursor: ((CursorPosition, Bool) -> Void)?
    private var trusted = false
    private var trustCheckedAt: TimeInterval = -1
    private var reportedMissing = false
    private let clock = MonotonicTimeSource()

    /// Called (main queue) when input is dropped because Accessibility is missing.
    var onAccessibilityMissing: (() -> Void)?

    /// Targets `displayID`, whose size in points is `size`; `pointsPerDp` scales gesture slop.
    func configure(displayID: CGDirectDisplayID, size: CGSize, pointsPerDp: Double) {
        queue.async {
            self.displayID = displayID
            self.displaySize = size
            self.injector.boundsProvider = { CGDisplayBounds(displayID) }
            self.engine.configure(displaySize: size, pointsPerDp: pointsPerDp)
            if self.mode == .trackpad {
                self.startCursorSampling()
                self.reportSystemCursor(force: true)
            }
        }
    }

    func handle(_ event: TouchEvent) {
        queue.async {
            guard self.displayID != kCGNullDirectDisplay else { return }
            guard self.isTrusted() else {
                self.engine.reset()
                self.injector.releaseAll()
                return
            }
            if event.touchAction != .move {
                // Diagnostics: finger events (not MOVEs) and gesture decisions.
                Log.info("input: touch \(event.touchAction.map { "\($0)" } ?? "\(event.action)") t=\(event.eventTimeMs) pointers=\(event.pointers.count)")
            }
            self.engine.handle(event)
        }
    }

    /// Touch ↔ trackpad (PROTOCOL §7.3): ends the current gesture (buttons released, scroll ended).
    func setInputMode(_ mode: InputMode) {
        queue.async {
            Log.info("input: mode \(mode.rawValue)")
            self.engine.setMode(mode)
            self.mode = mode
            self.lastReported = nil
            if mode == .trackpad {
                self.startCursorSampling()
                self.reportSystemCursor(force: true)
            } else {
                self.stopCursorSampling()
            }
        }
    }

    /// Reports the pointer again (forced), e.g. after a new STREAM_CONFIG.
    func refreshCursor() {
        queue.async {
            guard self.mode == .trackpad else { return }
            self.reportSystemCursor(force: true)
        }
    }

    // MARK: - Pointer for CURSOR (trackpad mode)

    private func report(_ position: CursorPosition, force: Bool = false) {
        guard force || position != lastReported else { return }
        lastReported = position
        onCursor?(position, force)
    }

    /// The system pointer on our display (normalized), or off-display. The user may move
    /// it with the Mac's own trackpad or mouse, so it is sampled every 100 ms.
    private func reportSystemCursor(force: Bool = false) {
        guard displayID != kCGNullDirectDisplay, let location = CGEvent(source: nil)?.location else {
            return report(.offDisplay, force: force)
        }
        let bounds = CGDisplayBounds(displayID)
        guard bounds.contains(location) else { return report(.offDisplay, force: force) }
        report(CursorPosition(point: CGPoint(x: location.x - bounds.minX, y: location.y - bounds.minY),
                              in: bounds.size), force: force)
    }

    private func startCursorSampling() {
        guard cursorSampler == nil, displayID != kCGNullDirectDisplay else { return }
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 0.1, repeating: 0.1, leeway: .milliseconds(10))
        t.setEventHandler { [weak self] in self?.reportSystemCursor() }
        t.resume()
        cursorSampler = t
    }

    private func stopCursorSampling() {
        cursorSampler?.cancel()
        cursorSampler = nil
    }

    /// Types TEXT into whatever is focused on the Mac.
    func handleText(_ text: String) {
        queue.async {
            guard self.displayID != kCGNullDirectDisplay, self.isTrusted() else { return }
            self.injector.typeText(text)
        }
    }

    /// Presses a KEY (with its modifiers) in whatever is focused on the Mac.
    func handleKey(_ event: KeyEvent) {
        guard let stroke = MacKeyMapping.keyStroke(for: event) else {
            return Log.debug("input: ignoring unknown KEY \(event.key)")
        }
        queue.async {
            guard self.displayID != kCGNullDirectDisplay, self.isTrusted() else { return }
            self.injector.press(stroke)
        }
    }

    /// Ends any gesture and releases held buttons (disconnect safety).
    func reset() {
        queue.async {
            self.engine.reset()
            self.injector.releaseAll()
            self.stopCursorSampling()
            self.displayID = kCGNullDirectDisplay
            self.engine.configure(displaySize: .zero)
        }
    }

    /// `AXIsProcessTrusted` at most every 2 s.
    private func isTrusted() -> Bool {
        let now = clock.now()
        if trustCheckedAt < 0 || now - trustCheckedAt > 2 {
            trusted = AXIsProcessTrusted()
            trustCheckedAt = now
            if trusted {
                reportedMissing = false
            } else if !reportedMissing {
                reportedMissing = true
                Log.info("input: Accessibility not granted — touch and keyboard input ignored")
                if let cb = onAccessibilityMissing { DispatchQueue.main.async(execute: cb) }
            }
        }
        return trusted
    }
}
