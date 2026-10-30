import CoreGraphics
import CoreVideo
import Foundation
import SidecastKit

/// Test-mode frame source: a synthetic moving pattern (scrolling colour bars,
/// a bouncing box, a frame-counter bar code and a noise band so the encoder
/// really spends its bitrate), rendered into 420v pixel buffers at `fps`.
/// No display, capture or input injection: touches go through the real
/// gesture engine and the resulting actions are logged.
final class TestPatternBackend: StreamBackend {
    var onRuntimeFailure: ((Error) -> Void)?
    var onCursor: ((CursorPosition, Bool) -> Void)?
    private var displaySize = CGSize.zero
    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.testpattern", qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private var renderer: TestPatternRenderer?
    private var touches = 0
    private var texts = 0
    private var keys = 0
    private lazy var engine: GestureEngine = {
        let engine = GestureEngine(scheduler: DispatchQueueScheduler(queue: queue)) { [weak self] action in
            self?.logGesture(action)
        }
        engine.trace = { Log.info("test pattern: \($0)") }
        return engine
    }()
    private var scrollSteps = 0
    private var dragSteps = 0
    /// Trackpad cursor moves, summarised (count, first and last position reached).
    private var moveSteps = 0
    private var moveFrom: CGPoint?
    private var moveTo: CGPoint?

    func start(plan: StreamPlan, client: ClientInfo, onFrame: @escaping FrameHandler,
               completion: @escaping (Result<Void, Error>) -> Void) {
        queue.async {
            self.stopLocked()
            guard let renderer = TestPatternRenderer(width: plan.encodeWidth, height: plan.encodeHeight) else {
                return completion(.failure(StreamStartError.captureFailed("cannot create pixel buffer pool")))
            }
            self.renderer = renderer
            self.displaySize = CGSize(width: plan.display.width, height: plan.display.height)
            self.engine.configure(displaySize: self.displaySize,
                                  pointsPerDp: DisplayCaptureBackend.pointsPerDp(plan: plan, screen: client.screen))
            let t = DispatchSource.makeTimerSource(flags: .strict, queue: self.queue)
            let interval = 1.0 / Double(plan.fps)
            t.schedule(deadline: .now() + interval, repeating: interval, leeway: .milliseconds(1))
            t.setEventHandler { [weak self] in
                guard let buffer = self?.renderer?.nextFrame() else { return }
                onFrame(buffer, MonotonicTimeSource.nowMicros)
            }
            self.timer = t
            t.resume()
            Log.info("test pattern: \(plan.encodeWidth)x\(plan.encodeHeight) @ \(plan.fps) fps for \(client.deviceName)")
            completion(.success(()))
        }
    }

    func stop() {
        queue.async {
            self.engine.reset()
            self.engine.configure(displaySize: .zero)
            self.stopLocked()
        }
    }

    func requestRefresh() {}   // frames flow continuously

    func handleTouch(_ event: TouchEvent) {
        queue.async {
            self.touches += 1
            let pts = event.pointers.map { String(format: "%d(%.3f,%.3f)", $0.id, $0.x, $0.y) }.joined(separator: " ")
            let line = "test pattern: TOUCH #\(self.touches) action=\(event.action) pointers=\(pts)"
            if event.touchAction == .move { Log.debug(line) } else { Log.info(line) }   // MOVEs only with --verbose
            self.engine.handle(event)
            if event.touchAction != .move { self.flushMoves() }
        }
    }

    func setInputMode(_ mode: InputMode) {
        queue.async {
            Log.info("test pattern: input mode \(mode.rawValue) — gestures now use the \(mode.rawValue) engine")
            self.engine.setMode(mode)
            self.flushMoves()
            if mode == .trackpad { self.reportEngineCursor() }
        }
    }

    func refreshCursor() {
        queue.async {
            if self.engine.mode == .trackpad { self.reportEngineCursor() }
        }
    }

    /// No real pointer in test mode: the engine's cursor, or off-display before the first touch.
    private func reportEngineCursor() {
        let position = engine.hasCursor ? CursorPosition(point: engine.cursor, in: displaySize) : .offDisplay
        onCursor?(position, true)
    }

    /// Logs a run of cursor moves as one line.
    private func flushMoves() {
        guard moveSteps > 0, let from = moveFrom, let to = moveTo else { return }
        Log.info(String(format: "test pattern: gesture → cursor moved in %d steps (%.0f, %.0f) → (%.0f, %.0f), Δ=(%+.0f, %+.0f)",
                        moveSteps, from.x, from.y, to.x, to.y, to.x - from.x, to.y - from.y) + " — not injected")
        moveSteps = 0
        moveFrom = nil
        moveTo = nil
    }

    /// Gesture-engine output (on `queue`). Scroll/drag steps are counted, the rest logged.
    private func logGesture(_ action: MouseAction) {
        if engine.mode == .trackpad, let p = action.pointerLocation {
            onCursor?(CursorPosition(point: p, in: displaySize), false)
        }
        if let steps = ClickTimeline.steps(for: action), steps.count > 1 {
            Log.info("test pattern: click timeline: \(ClickTimeline.describe(steps)) — not injected")
        }
        if case .move(let p) = action, engine.mode == .trackpad {
            if moveFrom == nil { moveFrom = p }   // first position reached
            moveTo = p
            moveSteps += 1
            return
        }
        flushMoves()
        switch action {
        case .scroll(_, _, _, let phase) where phase == .changed || phase == .momentum:
            scrollSteps += 1
        case .scroll(_, _, _, let phase):
            if phase == .began || phase == .momentumBegan { scrollSteps = 0 }
            let steps = phase == .ended || phase == .momentumEnded ? " after \(scrollSteps) steps" : ""
            Log.info("test pattern: gesture → scroll \(phase)\(steps) — not injected")
        case .drag:
            dragSteps += 1
        case .navigate(let navigation):
            let stroke = MacKeyMapping.keyStroke(for: navigation)
            let arrow = ["←", "→", "↑", "↓"][[NavigationAction.spaceLeft, .spaceRight, .missionControl, .appExpose]
                .firstIndex(of: navigation) ?? 0]
            Log.info("test pattern: gesture → navigate \(navigation) = Control+\(arrow) virtualKey=0x"
                     + String(format: "%02x flags=0x%06llx", stroke.virtualKey, stroke.flags.rawValue) + " — not injected")
        default:
            if case .buttonUp = action, dragSteps > 0 {
                Log.info("test pattern: gesture → \(dragSteps) drag steps")
                dragSteps = 0
            }
            Log.info("test pattern: gesture → \(Self.describe(action)) — not injected")
        }
    }

    private static func describe(_ action: MouseAction) -> String {
        func at(_ p: CGPoint) -> String { String(format: "(%.0f, %.0f)", p.x, p.y) }
        switch action {
        case .move(let p): return "move to \(at(p))"
        case .buttonDown(let b, let p): return "\(b) button down at \(at(p))"
        case .buttonUp(let b, let p): return "\(b) button up at \(at(p))"
        case .click(let p, let count): return "click ×\(count) at \(at(p))"
        case .rightClick(let p): return "right click at \(at(p))"
        case .zoom(let delta, let p): return "zoom \(delta) at \(at(p))"
        case .drag(let p): return "drag to \(at(p))"
        case .scroll(let dx, let dy, let p, let phase): return String(format: "scroll %.1f,%.1f ", dx, dy) + "\(phase) at \(at(p))"
        case .navigate(let n): return "navigate \(n)"
        }
    }

    /// Logs what would be typed (test mode never injects).
    func handleText(_ text: String) {
        queue.async {
            self.texts += 1
            let chunks = UnicodeTyping.chunks(UnicodeTyping.removingControlCharacters(text))
            Log.info("test pattern: TEXT #\(self.texts) \(text.debugDescription) (\(text.utf8.count) bytes, "
                     + "\(text.utf16.count) UTF-16 units, \(chunks.count) key event pair(s)) — not injected")
        }
    }

    func handleKey(_ event: KeyEvent) {
        queue.async {
            self.keys += 1
            let raw = "key=\(event.key) modifiers=0x" + String(format: "%02x", event.modifiers)
            guard let key = event.protocolKey, let stroke = MacKeyMapping.keyStroke(for: event) else {
                return Log.info("test pattern: KEY #\(self.keys) \(raw) unknown key — ignored")
            }
            Log.info("test pattern: KEY #\(self.keys) \(raw) (\(key)) → virtualKey=0x"
                     + String(format: "%02x flags=0x%06llx", stroke.virtualKey, stroke.flags.rawValue) + " — not injected")
        }
    }

    private func stopLocked() {
        timer?.cancel()
        timer = nil
        renderer = nil
    }
}

/// Renders the pattern. Not thread-safe; used on one queue.
final class TestPatternRenderer {
    let width: Int
    let height: Int
    private let pool: CVPixelBufferPool
    private var frame = 0
    private var rng: UInt64 = 0x9E37_79B9_7F4A_7C15
    // One row of bars, twice as wide as the frame, so scrolling is a memcpy at an offset.
    private let barsY: [UInt8]
    private let barsUV: [UInt8]

    /// BT.709 video-range Y, Cb, Cr of eight colour bars.
    private static let bars: [(UInt8, UInt8, UInt8)] = [
        (235, 128, 128), (219, 16, 138), (188, 154, 16), (173, 42, 26),
        (78, 214, 230), (63, 102, 240), (32, 240, 118), (16, 128, 128),
    ]

    init?(width: Int, height: Int) {
        self.width = width
        self.height = height
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferWidthKey: width,
            kCVPixelBufferHeightKey: height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as [CFString: Any],
        ]
        var pool: CVPixelBufferPool?
        guard CVPixelBufferPoolCreate(nil, nil, attrs as CFDictionary, &pool) == kCVReturnSuccess, let pool else {
            return nil
        }
        self.pool = pool
        let barWidth = max(2, (width / 8) & ~1)
        var y = [UInt8](repeating: 0, count: width * 2 + 2)
        var uv = [UInt8](repeating: 0, count: width * 2 + 2)
        for x in stride(from: 0, to: y.count - 1, by: 2) {
            let bar = Self.bars[(x / barWidth) % Self.bars.count]
            y[x] = bar.0; y[x + 1] = bar.0
            uv[x] = bar.1; uv[x + 1] = bar.2
        }
        barsY = y
        barsUV = uv
    }

    func nextFrame() -> CVPixelBuffer? {
        var out: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &out) == kCVReturnSuccess, let buffer = out else { return nil }
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let yBase = CVPixelBufferGetBaseAddressOfPlane(buffer, 0)?.assumingMemoryBound(to: UInt8.self),
              let uvBase = CVPixelBufferGetBaseAddressOfPlane(buffer, 1)?.assumingMemoryBound(to: UInt8.self)
        else { return nil }
        let yStride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0)
        let uvStride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1)

        // Scrolling bars.
        let offset = (frame * 6) % width & ~1
        barsY.withUnsafeBufferPointer { src in
            for row in 0..<height { memcpy(yBase + row * yStride, src.baseAddress! + offset, width) }
        }
        barsUV.withUnsafeBufferPointer { src in
            for row in 0..<(height / 2) { memcpy(uvBase + row * uvStride, src.baseAddress! + offset, width) }
        }
        drawBox(yBase, yStride, uvBase, uvStride)
        drawCounter(yBase, yStride)
        drawNoise(yBase, yStride)
        frame += 1
        return buffer
    }

    /// A white box bouncing around the frame.
    private func drawBox(_ y: UnsafeMutablePointer<UInt8>, _ ys: Int, _ uv: UnsafeMutablePointer<UInt8>, _ uvs: Int) {
        let size = max(8, min(width, height) / 6) & ~1
        let spanX = max(1, width - size), spanY = max(1, height - size)
        func bounce(_ t: Int, _ span: Int) -> Int { let p = t % (2 * span); return p < span ? p : 2 * span - p }
        let x0 = bounce(frame * 7, spanX) & ~1, y0 = bounce(frame * 5, spanY) & ~1
        for row in y0..<(y0 + size) { memset(y + row * ys + x0, 235, size) }
        for row in (y0 / 2)..<((y0 + size) / 2) { memset(uv + row * uvs + x0, 128, size) }
    }

    /// The frame number as a 24-bit bar code along the top edge.
    private func drawCounter(_ y: UnsafeMutablePointer<UInt8>, _ ys: Int) {
        let cell = max(4, width / 48)
        let rows = min(height, cell)
        for bit in 0..<24 where bit * cell + cell <= width {
            let value: Int32 = (frame >> (23 - bit)) & 1 == 1 ? 235 : 16
            for row in 0..<rows { memset(y + row * ys + bit * cell, value, cell) }
        }
    }

    /// Per-frame random luma in the bottom eighth: incompressible, so the
    /// encoder's output tracks its target bitrate.
    private func drawNoise(_ y: UnsafeMutablePointer<UInt8>, _ ys: Int) {
        let band = max(2, height / 8)
        for row in (height - band)..<height {
            let line = UnsafeMutableRawPointer(y + row * ys)
            var x = 0
            while x + 8 <= width {
                rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17
                line.storeBytes(of: rng, toByteOffset: x, as: UInt64.self)
                x += 8
            }
        }
    }
}
