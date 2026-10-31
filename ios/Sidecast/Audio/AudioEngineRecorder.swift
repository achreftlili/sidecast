import AVFoundation
import SidecastKit

/// The device microphone as 48 kHz mono PCM16 (PROTOCOL §7.2), through
/// AVAudioEngine with voice processing (echo cancellation and noise suppression,
/// like Android's voice-communication source). Samples arrive about every 10 ms:
/// an AVAudioSinkNode sees each I/O cycle, where an input tap would batch 100 ms
/// or more. Callbacks are delivered on `queue` (the session queue).
final class AudioEngineRecorder: MicRecorder {
    private let queue: DispatchQueue
    private let engine = AVAudioEngine()
    private let convertQueue = DispatchQueue(label: "sidecast.mic.convert", qos: .userInteractive)
    private var sink: AVAudioSinkNode?
    private var observers: [NSObjectProtocol] = []
    private var running = false

    init(queue: DispatchQueue) {
        self.queue = queue
    }

    func start(onSamples: @escaping ([Int16]) -> Void, onFailure: @escaping () -> Void) -> Bool {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .mixWithOthers])
            try session.setPreferredSampleRate(Double(AudioPacket.sampleRate))
            try session.setPreferredIOBufferDuration(0.01)
            try session.setActive(true)
        } catch {
            Log.mic.error("audio session: \(String(describing: error), privacy: .public)")
            return false
        }
        let input = engine.inputNode
        do {
            try input.setVoiceProcessingEnabled(true)
        } catch {
            Log.mic.info("voice processing unavailable: \(String(describing: error), privacy: .public)")
        }
        let hardware = input.outputFormat(forBus: 0)
        guard hardware.sampleRate > 0, hardware.channelCount > 0,
              let mono = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: hardware.sampleRate, channels: 1,
                                       interleaved: false),
              let target = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(AudioPacket.sampleRate),
                                         channels: 1, interleaved: true),
              let converter = AVAudioConverter(from: mono, to: target) else {
            Log.mic.error("no usable input format (\(hardware))")
            return false
        }
        let queue = queue
        let convertQueue = convertQueue
        // Runs on the real-time I/O thread: copy the first channel and leave.
        let sink = AVAudioSinkNode { _, frameCount, bufferList in
            let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: bufferList))
            guard let first = buffers.first, let data = first.mData, frameCount > 0 else { return noErr }
            let count = min(Int(frameCount), Int(first.mDataByteSize) / MemoryLayout<Float>.size)
            let samples = Array(UnsafeBufferPointer(start: data.assumingMemoryBound(to: Float.self), count: count))
            convertQueue.async {
                let pcm = Self.convert(samples, from: mono, with: converter)
                if !pcm.isEmpty { queue.async { onSamples(pcm) } }
            }
            return noErr
        }
        engine.attach(sink)
        engine.connect(input, to: sink, format: nil)
        self.sink = sink
        // A route change or an interruption (a phone call) stops the engine: report it.
        let center = NotificationCenter.default
        let failed: (Notification) -> Void = { [weak self] _ in
            queue.async {
                guard let self, self.running else { return }
                onFailure()
            }
        }
        observers = [
            center.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil, using: failed),
            center.addObserver(forName: AVAudioSession.interruptionNotification, object: session, queue: nil, using: failed),
        ]
        engine.prepare()
        do {
            try engine.start()
        } catch {
            Log.mic.error("engine start: \(String(describing: error), privacy: .public)")
            stop()
            return false
        }
        running = true
        Log.mic.info("capturing at \(hardware.sampleRate) Hz → 48 kHz mono")
        return true
    }

    func stop() {
        running = false
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        engine.stop()
        if let sink {
            engine.disconnectNodeInput(sink)
            engine.detach(sink)
        }
        sink = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Float32 mono at the hardware rate → PCM16 mono at 48 kHz.
    private static func convert(_ samples: [Float], from format: AVAudioFormat, with converter: AVAudioConverter) -> [Int16] {
        guard let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
              let channel = input.floatChannelData?[0] else { return [] }
        samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: samples.count) }
        input.frameLength = AVAudioFrameCount(samples.count)
        let ratio = converter.outputFormat.sampleRate / format.sampleRate
        let capacity = AVAudioFrameCount((Double(samples.count) * ratio).rounded(.up)) + 32
        guard let output = AVAudioPCMBuffer(pcmFormat: converter.outputFormat, frameCapacity: capacity) else { return [] }
        var consumed = false
        var error: NSError?
        converter.convert(to: output, error: &error) { _, status in
            if consumed {
                status.pointee = .noDataNow
                return nil
            }
            consumed = true
            status.pointee = .haveData
            return input
        }
        guard error == nil, let pcm = output.int16ChannelData?[0] else { return [] }
        return Array(UnsafeBufferPointer(start: pcm, count: Int(output.frameLength)))
    }
}

/// The microphone permission (AVAudioApplication); answers come back on `queue`.
final class SystemMicPermission: MicPermission {
    private let queue: DispatchQueue

    init(queue: DispatchQueue) {
        self.queue = queue
    }

    var isGranted: Bool { AVAudioApplication.shared.recordPermission == .granted }
    var isDenied: Bool { AVAudioApplication.shared.recordPermission == .denied }

    func request(_ completion: @escaping (Bool) -> Void) {
        let queue = queue
        AVAudioApplication.requestRecordPermission { granted in queue.async { completion(granted) } }
    }
}
