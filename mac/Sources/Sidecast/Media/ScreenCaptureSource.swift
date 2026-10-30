import AppKit
import CoreMedia
@preconcurrency import ScreenCaptureKit
import SidecastKit

/// ScreenCaptureKit capture of one display (ported from SideScreen's
/// ScreenCapture, without its CGDisplayStream fallback): 420v video range,
/// cursor shown, `1/fps` minimum frame interval, a watchdog for idle screens
/// and stalls, and a restart after wake. Control methods run on the main thread.
final class ScreenCaptureSource: NSObject, SCStreamOutput, SCStreamDelegate {
    private struct FrameState {
        var lastBuffer: CVPixelBuffer?
        var lastFrameTime: TimeInterval = 0
        var framesSeen = 0
    }

    var onFailure: ((Error) -> Void)?

    private let displayID: CGDirectDisplayID
    private let width: Int
    private let height: Int
    private let fps: Int
    private let onFrame: StreamBackend.FrameHandler
    private let sampleQueue = DispatchQueue(label: "com.achreftlili.sidecast.capture", qos: .userInteractive)
    private let frames = Locked(FrameState())
    private let clock = MonotonicTimeSource()
    private var stream: SCStream?
    private var running = false
    private var generation = 0
    private var startedAt: TimeInterval = 0
    private var restarts = 0
    private var watchdog: Timer?
    private var wakeObservers: [NSObjectProtocol] = []

    private static let maxRestarts = 3

    init(displayID: CGDirectDisplayID, width: Int, height: Int, fps: Int,
         onFrame: @escaping StreamBackend.FrameHandler) {
        self.displayID = displayID
        self.width = width
        self.height = height
        self.fps = fps
        self.onFrame = onFrame
    }

    func start(completion: @escaping (Result<Void, Error>) -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        running = true
        let gen = generation
        Task { @MainActor in
            do {
                try await self.startStream(generation: gen)
                self.startWatchdog()
                self.observeWake()
                completion(.success(()))
            } catch {
                completion(.failure(StreamStartError.captureFailed("\(error)")))
            }
        }
    }

    func stop() {
        dispatchPrecondition(condition: .onQueue(.main))
        running = false
        generation += 1
        watchdog?.invalidate()
        watchdog = nil
        let center = NSWorkspace.shared.notificationCenter
        wakeObservers.forEach { center.removeObserver($0) }
        wakeObservers.removeAll()
        if let stream {
            self.stream = nil
            Task { try? await stream.stopCapture() }
        }
        frames.withLock { $0 = FrameState() }
    }

    /// Re-sends the last frame when the screen is static (keyframe requests).
    func refresh() {
        let now = clock.now()
        let buffer = frames.withLock { s -> CVPixelBuffer? in
            now - s.lastFrameTime > 0.1 ? s.lastBuffer : nil
        }
        if let buffer { onFrame(buffer, MonotonicTimeSource.nowMicros) }
    }

    // MARK: - Stream setup

    @MainActor
    private func startStream(generation gen: Int) async throws {
        let display = try await findDisplay()
        guard running, gen == generation else { return }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = width
        config.height = height
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        // Video range: some Android decoders ignore the full-range flag (SideScreen #55).
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorMatrix = CGDisplayStream.yCbCrMatrix_ITU_R_709_2
        config.showsCursor = true
        config.queueDepth = 6
        config.capturesAudio = false
        config.scalesToFit = false
        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: sampleQueue)
        try await stream.startCapture()
        guard running, gen == generation else {
            try? await stream.stopCapture()
            return
        }
        self.stream = stream
        startedAt = clock.now()
        Log.info("capture: started \(width)x\(height) @ \(fps) fps on display \(displayID)")
    }

    /// The virtual display shows up in SCShareableContent shortly after creation; poll for it.
    @MainActor
    private func findDisplay() async throws -> SCDisplay {
        for attempt in 1...10 {
            if let content = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false),
               let display = content.displays.first(where: { $0.displayID == displayID }) {
                return display
            }
            Log.debug("capture: display \(displayID) not visible yet (attempt \(attempt))")
            try await Task.sleep(nanoseconds: 500_000_000)
        }
        throw StreamStartError.captureFailed("display \(displayID) never appeared in ScreenCaptureKit")
    }

    // MARK: - SCStreamOutput / SCStreamDelegate

    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sample.isValid,
              let info = (CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]])?.first,
              let raw = info[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let buffer = CMSampleBufferGetImageBuffer(sample) else { return }   // idle: screen unchanged
        let pts = CMSampleBufferGetPresentationTimeStamp(sample)
        let micros = pts.isValid ? UInt64(max(0, pts.seconds) * 1_000_000) : MonotonicTimeSource.nowMicros
        let now = clock.now()
        frames.withLock { s in
            s.lastBuffer = buffer
            s.lastFrameTime = now
            s.framesSeen += 1
        }
        onFrame(buffer, micros)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        Log.error("capture: stream stopped: \(error)")
        DispatchQueue.main.async { self.scheduleRestart(after: 1, reason: "stream error") }
    }

    // MARK: - Watchdog and wake

    private func startWatchdog() {
        watchdog?.invalidate()
        watchdog = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.checkFlow() }
    }

    private func checkFlow() {
        guard running, stream != nil else { return }
        let now = clock.now()
        let (seen, last, buffer) = frames.withLock { ($0.framesSeen, $0.lastFrameTime, $0.lastBuffer) }
        if seen == 0 {
            // Never got a frame: the stream is wedged, not idle.
            if now - startedAt > 5 { scheduleRestart(after: 0, reason: "no frames after start") }
        } else if now - last > 2, let buffer {
            // Static screen: macOS sends nothing. Re-send the last frame so the
            // client keeps getting (cheap) frames and periodic keyframes.
            frames.withLock { $0.lastFrameTime = now }
            onFrame(buffer, MonotonicTimeSource.nowMicros)
        }
    }

    private func observeWake() {
        guard wakeObservers.isEmpty else { return }
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.screensDidWakeNotification, NSWorkspace.didWakeNotification] {
            wakeObservers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                // Display sleep tears SCStream down (-3815); give WindowServer time to settle.
                self?.scheduleRestart(after: 2, reason: "wake", countsTowardLimit: false)
            })
        }
    }

    private var restartPending = false

    private func scheduleRestart(after delay: TimeInterval, reason: String, countsTowardLimit: Bool = true) {
        guard running, !restartPending else { return }
        if countsTowardLimit {
            if frames.withLock({ $0.framesSeen }) > 0 { restarts = 0 }   // it worked for a while
            restarts += 1
            guard restarts <= Self.maxRestarts else {
                Log.error("capture: giving up after \(Self.maxRestarts) restarts")
                onFailure?(StreamStartError.captureFailed("capture keeps failing (\(reason))"))
                return
            }
        }
        restartPending = true
        generation += 1
        let gen = generation
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
            Task { @MainActor in
                self.restartPending = false
                guard self.running, gen == self.generation else { return }
                Log.info("capture: restarting (\(reason))")
                if let old = self.stream {
                    self.stream = nil
                    try? await old.stopCapture()
                }
                self.frames.withLock { $0.framesSeen = 0 }
                do {
                    try await self.startStream(generation: gen)
                } catch {
                    Log.error("capture: restart failed: \(error)")
                    self.scheduleRestart(after: 1, reason: "restart failed")
                }
            }
        }
    }
}
