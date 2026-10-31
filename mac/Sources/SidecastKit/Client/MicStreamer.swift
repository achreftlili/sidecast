import Foundation

/// One microphone capture, 48 kHz mono PCM16 (AVAudioEngine on iOS, a fake in tests).
public protocol MicRecorder: AnyObject {
    /// Starts capturing; false when the microphone is unavailable. `onSamples`
    /// gets chunks of any size, `onFailure` is called once if capture dies on its
    /// own; both must be delivered on the streamer's queue.
    func start(onSamples: @escaping ([Int16]) -> Void, onFailure: @escaping () -> Void) -> Bool
    /// Stops and frees everything. No callback afterwards.
    func stop()
}

/// The microphone permission.
public protocol MicPermission: AnyObject {
    var isGranted: Bool { get }
    /// True when the user already refused (asking again shows nothing).
    var isDenied: Bool { get }
    /// Asks the user; the answer comes back on the streamer's queue.
    func request(_ completion: @escaping (Bool) -> Void)
}

/// Where the microphone's traffic goes: the stream that asked for it.
public protocol MicSink: AnyObject {
    func sendMicState(_ state: MicState)
    func sendAudio(_ packet: AudioPacket)
}

/// Splits captured audio into AUDIO packets of exactly `size` samples. Pure.
public struct PCMChunker: Sendable {
    public let size: Int
    private var pending: [Int16] = []

    public init(size: Int = AudioPacket.recommendedSamples) {
        self.size = max(1, size)
    }

    /// Appends `samples`; returns every complete packet now available.
    public mutating func append(_ samples: [Int16]) -> [[Int16]] {
        pending.append(contentsOf: samples)
        guard pending.count >= size else { return [] }
        var out: [[Int16]] = []
        var start = 0
        while pending.count - start >= size {
            out.append(Array(pending[start..<(start + size)]))
            start += size
        }
        pending.removeFirst(start)
        return out
    }

    public mutating func reset() { pending.removeAll() }
}

/// Device microphone → "Sidecast Microphone" on the computer (PROTOCOL §7.2).
/// Captures only while the computer asks (MIC_CONTROL active:true), the user
/// setting allows it, the permission is granted and the display screen is in
/// the foreground; every refusal is reported with MIC_STATE. AUDIO carries
/// 10 ms (480 samples), seq from 0 after each active:true; MIC_STATE
/// capturing:true follows the first AUDIO.
///
/// Not thread-safe: every method, and the recorder and permission callbacks,
/// run on one serial queue.
public final class MicStreamer {
    private let sink: MicSink
    private let makeRecorder: () -> MicRecorder?
    private let permission: MicPermission
    private let isEnabled: () -> Bool
    private let log: (String) -> Void

    private var wanted = false
    private var foreground = false
    private var permissionPending = false
    private var permissionDenied = false
    private var failed = false
    private var released = false
    private var recorder: MicRecorder?
    /// Identifies the running capture, so late callbacks of a stopped one are ignored.
    private var generation = 0
    private var chunker = PCMChunker()
    private var seq: UInt32 = 0
    private var announced = false
    private var lastReported: MicState?

    /// True while audio is flowing (drives the red "Mic" indicator).
    public private(set) var isCapturing = false {
        didSet { if isCapturing != oldValue { onCapturingChange?(isCapturing) } }
    }
    public var onCapturingChange: ((Bool) -> Void)?

    public init(sink: MicSink, makeRecorder: @escaping () -> MicRecorder?, permission: MicPermission,
                isEnabled: @escaping () -> Bool, log: @escaping (String) -> Void = { _ in }) {
        self.sink = sink
        self.makeRecorder = makeRecorder
        self.permission = permission
        self.isEnabled = isEnabled
        self.log = log
    }

    /// MIC_CONTROL from the current stream.
    public func handle(_ control: MicControl) {
        guard !released else { return }
        if control.active {
            // A fresh request: seq restarts at 0 and every earlier refusal is re-evaluated.
            // A capture already running keeps going and re-announces itself with its next packet.
            wanted = true
            seq = 0
            announced = false
            permissionDenied = false
            failed = false
            lastReported = nil
            evaluate()
        } else {
            let was = wanted
            wanted = false
            permissionPending = false
            stopCapture()
            if was { report(MicState(capturing: false, reason: .stopped)) }
        }
    }

    /// The stream the microphone served ended; a new stream starts with the mic off.
    public func sessionEnded() {
        wanted = false
        permissionPending = false
        stopCapture()
        lastReported = nil
    }

    /// The display screen is visible: resume if the computer still wants the mic.
    public func enterForeground() {
        foreground = true
        failed = false   // the microphone may be free again
        if wanted { lastReported = nil }
        evaluate()
    }

    /// The display screen is no longer visible: never record in the background.
    public func enterBackground() {
        foreground = false
        guard wanted else { return }
        stopCapture()
        report(MicState(capturing: false, reason: .background))
    }

    /// The user changed the setting: re-evaluate a pending request.
    public func settingChanged() {
        if wanted { lastReported = nil }
        evaluate()
    }

    /// Terminal.
    public func release() {
        released = true
        wanted = false
        stopCapture()
    }

    // MARK: - Private

    private func evaluate() {
        guard wanted, !released else { return }
        if !isEnabled() {
            stopCapture()
            report(MicState(capturing: false, reason: .disabled))
        } else if !foreground {
            report(MicState(capturing: false, reason: .background))
        } else if recorder != nil {
            return   // capturing
        } else if failed {
            report(MicState(capturing: false, reason: .unavailable))
        } else if permission.isGranted {
            startCapture()
        } else if permissionDenied || permission.isDenied {
            report(MicState(capturing: false, reason: .permissionDenied))
        } else if !permissionPending {
            permissionPending = true
            permission.request { [weak self] granted in
                guard let self else { return }
                self.permissionPending = false
                if !granted { self.permissionDenied = true }
                self.evaluate()
            }
        }
    }

    private func startCapture() {
        guard let recorder = makeRecorder() else { return fail() }
        generation += 1
        let current = generation
        chunker.reset()
        self.recorder = recorder
        let started = recorder.start(onSamples: { [weak self] samples in
            guard let self, self.generation == current, self.recorder != nil else { return }
            self.deliver(samples)
        }, onFailure: { [weak self] in
            guard let self, self.generation == current, self.recorder != nil else { return }
            self.log("mic capture failed")
            self.fail()
        })
        if started {
            log("mic capture started")
        } else if self.recorder != nil {
            fail()
        }
    }

    private func stopCapture() {
        guard let r = recorder else { return }
        recorder = nil
        generation += 1
        r.stop()
        chunker.reset()
        announced = false
        isCapturing = false
        log("mic capture stopped")
    }

    private func deliver(_ samples: [Int16]) {
        for packet in chunker.append(samples) {
            sink.sendAudio(AudioPacket(seq: seq, samples: packet))
            seq &+= 1
            if !announced {
                announced = true
                isCapturing = true
                report(MicState(capturing: true))
            }
        }
    }

    private func fail() {
        if let r = recorder {
            recorder = nil
            generation += 1
            r.stop()
        }
        chunker.reset()
        announced = false
        isCapturing = false
        failed = true
        report(MicState(capturing: false, reason: .unavailable))
    }

    private func report(_ state: MicState) {
        guard state != lastReported else { return }
        lastReported = state
        sink.sendMicState(state)
    }
}
