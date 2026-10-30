import CoreVideo
import Foundation
import SidecastKit

/// Per-second stream statistics (menu bar and test-mode log).
struct StreamStats {
    var fps: Double
    var mbps: Double
    var targetBitrate: Int
    var dropped: Int
    var keyframes: Int
    var smoothedRTT: TimeInterval?
    var minRTT: TimeInterval?
    var inFlight: Int

    var logLine: String {
        func ms(_ t: TimeInterval?) -> String { t.map { String(format: "%.1fms", $0 * 1000) } ?? "-" }
        return String(format: "stream: fps=%.0f Mbps=%.2f target=%.1fMbps dropped=%d keyframes=%d srtt=%@ minrtt=%@ inflight=%d",
                      fps, mbps, Double(targetBitrate) / 1e6, dropped, keyframes, ms(smoothedRTT), ms(minRTT), inFlight)
    }
}

/// Minimal mutex-protected box.
final class Locked<State>: @unchecked Sendable {
    private let lock = NSLock()
    private var state: State
    init(_ state: State) { self.state = state }

    func withLock<R>(_ body: (inout State) throws -> R) rethrows -> R {
        lock.lock()
        defer { lock.unlock() }
        return try body(&state)
    }
}

/// Runs one client's stream: backend frames → flow control (≤ 4 in flight,
/// drop before encode) → encoder → VIDEO_FRAME, plus AIMD bitrate and the
/// 5 s liveness check. Control methods run on `queue` (the server queue).
final class StreamController {
    private struct Shared {   // touched from capture, VideoToolbox and server threads
        var tracker = InFlightTracker()
        var abr: AdaptiveBitrateController
        var encoder: VideoEncoder?
        var generation = 0
        var keyframeRequested = true
        var droppedInWindow = 0
    }

    let client: ClientInfo
    private(set) var plan: StreamPlan
    private var settings: StreamSettings
    private let backend: StreamBackend
    private let queue: DispatchQueue
    private let send: (Data) -> Void
    private let clock: TimeSource
    private let shared: Locked<Shared>
    private let encodeQueue = DispatchQueue(label: "com.achreftlili.sidecast.encode", qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private var lastActivity: TimeInterval = 0
    private var running = false
    private var framesInWindow = 0, bytesInWindow = 0, keyframesInWindow = 0

    var onStats: ((StreamStats) -> Void)?
    /// The stream failed (reason for the BYE) or the client went silent (nil).
    var onFailure: ((ByeReason?) -> Void)?
    /// A STREAM_CONFIG was sent for `plan`.
    var onConfigured: ((StreamPlan) -> Void)?
    /// The client switched touch ↔ trackpad.
    var onInputMode: ((InputMode) -> Void)?
    /// Per session; a new stream starts in touch mode.
    private(set) var inputMode: InputMode = .touch
    // CURSOR (trackpad mode only), ≤ 60/s, final position always sent
    private var cursorThrottle = CursorThrottle()
    private var cursorFlush: DispatchWorkItem?
    private var cursorsInWindow = 0
    private var lastCursor: CursorPosition?

    init(client: ClientInfo, plan: StreamPlan, settings: StreamSettings, backend: StreamBackend,
         queue: DispatchQueue, clock: TimeSource = MonotonicTimeSource(), send: @escaping (Data) -> Void) {
        self.client = client
        self.plan = plan
        self.settings = settings
        self.backend = backend
        self.queue = queue
        self.clock = clock
        self.send = send
        // The 1 s timer drives the windows; 0.9 s keeps timer jitter from skipping one.
        let bounds = BitrateBounds(settingMbps: settings.maxBitrateMbps, limits: client.limits)
        let config = AdaptiveBitrateController.Config(bounds: bounds, window: 0.9)
        shared = Locked(Shared(abr: AdaptiveBitrateController(config: config, now: clock.now())))
        if let limits = client.limits {
            Log.info("stream: data saver for \(client.deviceName) (network \(client.network ?? "?")): limits "
                     + "\(limits.maxBitrateKbps.map { "≤ \($0) kbps" } ?? "no bitrate cap"), "
                     + "\(limits.maxFps.map { "≤ \($0) fps" } ?? "no fps cap") → bitrate "
                     + String(format: "%.1f–%.1f Mbps (start %.1f), %d fps", Double(bounds.floor) / 1e6,
                              Double(bounds.ceiling) / 1e6, Double(bounds.start) / 1e6, plan.fps))
        }
    }

    func start() {
        guard !running else { return }
        running = true
        backend.onCursor = { [weak self] position, force in
            self?.queue.async { self?.handleCursor(position, force: force) }
        }
        lastActivity = clock.now()
        startTimer()
        startBackend()
    }

    func stop() {
        guard running else { return }
        running = false
        cursorFlush?.cancel()
        cursorFlush = nil
        timer?.cancel()
        timer = nil
        let encoder = detachEncoder()
        backend.stop()
        encodeQueue.async { encoder?.invalidate() }
    }

    /// Any message from the client counts as a sign of life.
    func noteActivity() { lastActivity = clock.now() }

    func handleAck(_ seq: UInt32) {
        let now = clock.now()
        shared.withLock { s in
            if let rtt = s.tracker.ack(seq, at: now) { s.abr.recordRTT(rtt) }
        }
    }

    func requestKeyframe() {
        shared.withLock { $0.keyframeRequested = true }
        backend.requestRefresh()
    }

    func handleTouch(_ event: TouchEvent) {
        guard settings.touchEnabled else { return }
        backend.handleTouch(event)
    }

    /// TEXT / KEY share the touch setting ("Phone controls the Mac").
    func handleText(_ text: String) {
        guard settings.touchEnabled else { return }
        backend.handleText(text)
    }

    func handleKey(_ event: KeyEvent) {
        guard settings.touchEnabled else { return }
        backend.handleKey(event)
    }

    func setInputMode(_ mode: InputMode) {
        guard mode != inputMode else { return }
        inputMode = mode
        cursorFlush?.cancel()
        cursorFlush = nil
        cursorThrottle.reset()
        Log.info("stream: input mode \(mode.rawValue) for \(client.deviceName)")
        backend.setInputMode(mode)   // in trackpad mode the backend reports the pointer at once
        onInputMode?(mode)
    }

    // MARK: - CURSOR

    /// Pointer position from the backend. Sent only in trackpad mode, throttled.
    private func handleCursor(_ position: CursorPosition, force: Bool) {
        guard running, inputMode == .trackpad else { return }
        switch cursorThrottle.offer(position, at: clock.now(), force: force) {
        case .send(let p):
            sendCursor(p)
        case .deferred(let until):
            scheduleCursorFlush(at: until)
        case .unchanged:
            break
        }
    }

    private func scheduleCursorFlush(at time: TimeInterval) {
        guard cursorFlush == nil else { return }
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.cursorFlush = nil
            guard self.running, self.inputMode == .trackpad else { return }
            if let p = self.cursorThrottle.flush(at: self.clock.now()) {
                self.sendCursor(p)
            } else if self.cursorThrottle.hasPending {
                self.scheduleCursorFlush(at: self.clock.now() + 0.002)
            }
        }
        cursorFlush = work
        queue.asyncAfter(deadline: .now() + max(0, time - clock.now()), execute: work)
    }

    private func sendCursor(_ p: CursorPosition) {
        send(ServerMessage.cursor(p).encoded())
        cursorsInWindow += 1
        lastCursor = p
    }

    /// Applies new settings; restarts the pipeline (new STREAM_CONFIG) if the plan changes.
    func update(settings: StreamSettings) {
        if self.settings.touchEnabled, !settings.touchEnabled {
            // End any gesture in progress so no button stays held.
            backend.handleTouch(TouchEvent(eventTimeMs: 0, action: .cancel, actionPointerId: 0,
                                           pointers: [TouchPointer(id: 0, x: 0, y: 0)]))
        }
        self.settings = settings
        let bounds = BitrateBounds(settingMbps: settings.maxBitrateMbps, limits: client.limits)
        let bitrate = shared.withLock { s -> Int in
            s.abr.setBounds(bounds)
            return s.abr.bitrate
        }
        applyBitrate(bitrate)
        if let newPlan = try? StreamPlanner.plan(screen: client.screen, codecs: client.codecs, settings: settings,
                                                 limits: client.limits) {
            reconfigure(to: newPlan)
        }
    }

    /// Restarts with a different plan (display mode / codec / size / fps).
    private func reconfigure(to newPlan: StreamPlan) {
        guard running, newPlan != plan else { return }
        Log.info("stream: reconfiguring to \(newPlan.codec.rawValue) \(newPlan.encodeWidth)x\(newPlan.encodeHeight)@\(newPlan.fps)")
        let encoder = detachEncoder()
        backend.stop()
        encodeQueue.async { encoder?.invalidate() }
        plan = newPlan
        startBackend()
    }

    // MARK: - Pipeline

    private func startBackend() {
        let plan = self.plan
        backend.onRuntimeFailure = { [weak self] error in
            self?.queue.async {
                guard let self, self.running, plan == self.plan else { return }
                Log.error("stream: \(error)")
                self.onFailure?(.displayFailed)
            }
        }
        backend.start(plan: plan, client: client, onFrame: { [weak self] buffer, pts in
            self?.submit(buffer, ptsMicros: pts)
        }, completion: { [weak self] result in
            self?.queue.async { self?.backendStarted(result, plan: plan) }
        })
    }

    private func backendStarted(_ result: Result<Void, Error>, plan: StreamPlan) {
        guard running, plan == self.plan else { return }   // superseded meanwhile
        switch result {
        case .failure(let error):
            Log.error("stream: could not start: \(error)")
            onFailure?(.displayFailed)
        case .success:
            do {
                let bitrate = shared.withLock { $0.abr.bitrate }
                let encoder = try VideoEncoder(codec: plan.codec, width: plan.encodeWidth, height: plan.encodeHeight,
                                               fps: plan.fps, bitrate: bitrate)
                shared.withLock { s in
                    s.encoder = encoder
                    s.generation += 1
                    s.keyframeRequested = true
                }
                // Sent before any frame of this encoder: their delivery is queued behind us.
                send(ServerMessage.streamConfig(plan.streamConfig).encoded())
                Log.info("stream: STREAM_CONFIG \(plan.codec.rawValue) \(plan.encodeWidth)x\(plan.encodeHeight)@\(plan.fps) "
                         + "(display \(plan.display.width)x\(plan.display.height) pt\(plan.display.hiDPI ? " HiDPI" : ""))")
                onConfigured?(plan)
                if inputMode == .trackpad { backend.refreshCursor() }   // the new stream gets the pointer too
            } catch {
                Log.error("stream: \(error)")
                onFailure?(.displayFailed)
            }
        }
    }

    /// Called on capture threads. Drops the frame before encoding when 4 are in flight.
    private func submit(_ buffer: CVPixelBuffer, ptsMicros: UInt64) {
        let job = shared.withLock { s -> (encoder: VideoEncoder, force: Bool, generation: Int)? in
            guard let encoder = s.encoder else { return nil }
            guard s.tracker.tryReserve() else {
                s.abr.recordDrop()
                s.droppedInWindow += 1
                return nil
            }
            let force = s.keyframeRequested
            s.keyframeRequested = false
            return (encoder, force, s.generation)
        }
        guard let job else { return }
        encodeQueue.async { [weak self] in
            job.encoder.encode(buffer, ptsMicros: ptsMicros, forceKeyframe: job.force) { frame in
                self?.queue.async { self?.deliver(frame, generation: job.generation, forced: job.force) }
            }
        }
    }

    private func deliver(_ frame: EncodedFrame?, generation: Int, forced: Bool) {
        let now = clock.now()
        let isCurrent = running
        let seq = shared.withLock { s -> UInt32? in
            guard let frame, !frame.data.isEmpty, isCurrent, generation == s.generation else {
                s.tracker.cancelReservation()
                if forced { s.keyframeRequested = true }   // the forced keyframe never made it out
                return nil
            }
            return s.tracker.markSent(at: now)
        }
        guard let seq, let frame else { return }
        send(VideoFrame(seq: seq, isKeyframe: frame.isKeyframe, ptsMicros: frame.ptsMicros, data: frame.data).encodedFrame())
        framesInWindow += 1
        bytesInWindow += frame.data.count
        if frame.isKeyframe { keyframesInWindow += 1 }
    }

    private func detachEncoder() -> VideoEncoder? {
        shared.withLock { s in
            let encoder = s.encoder
            s.encoder = nil
            s.generation += 1
            return encoder
        }
    }

    private func applyBitrate(_ bitrate: Int) {
        guard let encoder = shared.withLock({ $0.encoder }) else { return }
        encodeQueue.async { encoder.setBitrate(bitrate) }
    }

    // MARK: - Once per second

    private func startTimer() {
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(20))
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        timer = t
    }

    private func tick() {
        let now = clock.now()
        if now - lastActivity > SidecastProtocol.livenessTimeout {
            Log.info("stream: nothing received from \(client.deviceName) for \(Int(SidecastProtocol.livenessTimeout)) s — closing")
            onFailure?(nil)
            return
        }
        let (changed, stats) = shared.withLock { s -> (Int?, StreamStats) in
            let before = s.abr.bitrate
            _ = s.abr.tick(now: now)
            let stats = StreamStats(fps: Double(framesInWindow), mbps: Double(bytesInWindow) * 8 / 1e6,
                                    targetBitrate: s.abr.bitrate, dropped: s.droppedInWindow,
                                    keyframes: keyframesInWindow, smoothedRTT: s.abr.smoothedRTT,
                                    minRTT: s.abr.minRTT, inFlight: s.tracker.inFlight)
            s.droppedInWindow = 0
            return (s.abr.bitrate != before ? s.abr.bitrate : nil, stats)
        }
        framesInWindow = 0
        bytesInWindow = 0
        keyframesInWindow = 0
        if Log.echoToStdout, cursorsInWindow > 0, let c = lastCursor {
            Log.info(String(format: "stream: CURSOR sent %d in the last second, last (%.4f, %.4f)", cursorsInWindow, c.x, c.y))
        }
        cursorsInWindow = 0
        if let changed { applyBitrate(changed) }
        if Log.echoToStdout { Log.info(stats.logLine) }
        onStats?(stats)
    }
}
