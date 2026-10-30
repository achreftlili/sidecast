import Foundation
import SidecastKit

/// Test-mode microphone: never touches Core Audio. Logs MIC_STATE and, once a
/// second, AUDIO statistics (packets, samples, sequence gaps, level, and the
/// tone frequency estimated from zero crossings). With `--test-mic-active` it
/// sends MIC_CONTROL active:true as soon as a mic-capable client streams.
final class TestMicBridge: MicBridge {
    private struct Window {
        var packets = 0
        var samples = 0
        var missing = 0
        var restarts = 0
        var peak: Int16 = 0
        var zeroCrossings = 0
    }

    private let activateOnStart: Bool
    private let queue = DispatchQueue(label: "com.achreftlili.sidecast.testmic")
    private var sessionId: UUID?
    private var timer: DispatchSourceTimer?
    private var tracker = AudioSequenceTracker()
    private var window = Window()
    private var total = Window()
    private var lastSample: Int16 = 0

    init(activateOnStart: Bool) {
        self.activateOnStart = activateOnStart
    }

    func sessionStarted(id: UUID, deviceName: String, supportsMic: Bool, control: @escaping (Bool) -> Void) {
        queue.async {
            self.sessionId = id
            self.tracker.reset()
            self.window = Window()
            self.total = Window()
            Log.info("test mic: \(deviceName) \(supportsMic ? "supports" : "does not support") the microphone")
            guard supportsMic else { return }
            guard self.activateOnStart else {
                return Log.info("test mic: no --test-mic-active, the microphone stays off")
            }
            self.startTimer()
            Log.info("test mic: --test-mic-active → MIC_CONTROL active:true")
            control(true)
        }
    }

    func sessionEnded(id: UUID) {
        queue.async {
            guard id == self.sessionId else { return }
            self.sessionId = nil
            self.timer?.cancel()
            self.timer = nil
            self.addWindowToTotal()
            let t = self.total
            Log.info("test mic: session ended — AUDIO total packets=\(t.packets) samples=\(t.samples) "
                     + "missing=\(t.missing) restarts=\(t.restarts)")
        }
    }

    func handleAudio(_ packet: AudioPacket, from id: UUID) {
        queue.async {
            guard id == self.sessionId else { return }
            var w = self.window
            switch self.tracker.observe(packet.seq) {
            case .first, .inOrder: break
            case .gap(let missing): w.missing += Int(missing)
            case .restart: w.restarts += 1
            }
            w.packets += 1
            w.samples += packet.samples.count
            var last = self.lastSample
            for s in packet.samples {
                let magnitude = s == .min ? Int16.max : abs(s)
                if magnitude > w.peak { w.peak = magnitude }
                if (last < 0) != (s < 0) { w.zeroCrossings += 1 }
                last = s
            }
            self.lastSample = last
            self.window = w
        }
    }

    func handleMicState(_ state: MicState, from id: UUID) {
        queue.async {
            guard id == self.sessionId else { return }
            Log.info("test mic: MIC_STATE capturing=\(state.capturing)\(state.reason.map { " reason=\($0)" } ?? "")")
        }
    }

    private func startTimer() {
        timer?.cancel()
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(20))
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        timer = t
    }

    @discardableResult
    private func addWindowToTotal() -> Window {
        let w = window
        window = Window()
        total.packets += w.packets
        total.samples += w.samples
        total.missing += w.missing
        total.restarts += w.restarts
        return w
    }

    private func tick() {
        let w = addWindowToTotal()
        let seconds = Double(w.samples) / Double(AudioPacket.sampleRate)
        let hz = seconds > 0 ? Double(w.zeroCrossings) / 2 / seconds : 0
        Log.info(String(format: "test mic: AUDIO packets=%d samples=%d gaps=%d restarts=%d peak=%d tone≈%.0f Hz",
                        w.packets, w.samples, w.missing, w.restarts, Int(w.peak), hz))
    }
}
