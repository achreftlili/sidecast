import Foundation

/// Classifies AUDIO sequence numbers (PROTOCOL §7.2). TCP never reorders or
/// duplicates, so a number below the expected one means the phone restarted
/// its numbering (a new capture after MIC_CONTROL active:true).
public struct AudioSequenceTracker: Equatable, Sendable {
    public enum Event: Equatable, Sendable {
        case first
        case inOrder
        /// `missing` packets were skipped before this one.
        case gap(missing: UInt32)
        case restart
    }

    public private(set) var expected: UInt32?

    public init() {}

    public mutating func observe(_ seq: UInt32) -> Event {
        defer { expected = seq &+ 1 }
        guard let expected else { return .first }
        if seq == expected { return .inOrder }
        let diff = Int32(bitPattern: seq &- expected)   // wrap-aware
        return diff > 0 ? .gap(missing: UInt32(diff)) : .restart
    }

    public mutating func reset() { expected = nil }
}

/// Jitter buffer between AUDIO packets (network, bursty) and the audio
/// device's render callback (steady):
/// - plays only once `targetFrames` (~40 ms) are queued, and re-buffers after
///   an underrun (the shortfall is silence);
/// - more than `maxFrames` (~150 ms) queued: the oldest audio is dropped back
///   down to the target, so latency recovers after a burst;
/// - sequence gaps are filled with silence (one packet's length per missing
///   packet, at most `maxFrames`).
/// Not thread-safe; callers serialise access (the app wraps it in a lock).
/// `read` never allocates.
public struct AudioJitterBuffer: Sendable {
    public struct Configuration: Equatable, Sendable {
        public var targetFrames: Int
        public var maxFrames: Int

        /// 40 ms target, 150 ms ceiling at 48 kHz.
        public init(targetFrames: Int = 1_920, maxFrames: Int = 7_200) {
            self.targetFrames = max(0, targetFrames)
            self.maxFrames = max(self.targetFrames, maxFrames)
        }
    }

    public struct Stats: Equatable, Sendable {
        public var packets = 0
        public var receivedFrames = 0
        /// Packets the phone skipped (sequence gaps).
        public var missingPackets = 0
        /// Silence inserted for those gaps.
        public var gapFillFrames = 0
        public var restarts = 0
        /// Oldest audio dropped because too much was queued.
        public var droppedFrames = 0
        public var underruns = 0
        public var playedFrames = 0
        /// Silence rendered while buffering or after an underrun.
        public var silentFrames = 0

        public init() {}
    }

    public let configuration: Configuration
    public private(set) var stats = Stats()
    public private(set) var isBuffering = true
    public var queuedFrames: Int { count }

    private var ring: [Float]
    private var head = 0
    private var count = 0
    private var sequence = AudioSequenceTracker()
    private var lastPacketFrames = AudioPacket.recommendedSamples

    public init(configuration: Configuration = Configuration()) {
        self.configuration = configuration
        // Room for the ceiling plus the largest packet and a full gap fill.
        ring = [Float](repeating: 0, count: configuration.maxFrames * 2 + AudioPacket.maxSamples + 1)
    }

    /// Empties the buffer for a new capture (after MIC_CONTROL active:true).
    public mutating func reset() {
        head = 0
        count = 0
        isBuffering = true
        sequence.reset()
        lastPacketFrames = AudioPacket.recommendedSamples
    }

    /// Returns and clears the counters.
    public mutating func takeStats() -> Stats {
        defer { stats = Stats() }
        return stats
    }

    public mutating func push(_ packet: AudioPacket) {
        packet.samples.withUnsafeBufferPointer { push(seq: packet.seq, samples: $0) }
    }

    public mutating func push(seq: UInt32, samples: UnsafeBufferPointer<Int16>) {
        stats.packets += 1
        stats.receivedFrames += samples.count
        switch sequence.observe(seq) {
        case .first, .inOrder:
            break
        case .gap(let missing):
            stats.missingPackets += Int(missing)
            let fill = min(Int(missing) * lastPacketFrames, configuration.maxFrames)
            appendSilence(fill)
            stats.gapFillFrames += fill
        case .restart:
            stats.restarts += 1
        }
        if !samples.isEmpty { lastPacketFrames = samples.count }
        append(samples)
        if count > configuration.maxFrames {
            drop(count - configuration.targetFrames)
        }
    }

    /// Fills `out` with the next frames; silence while buffering or on underrun.
    /// Returns how many frames were real audio.
    @discardableResult
    public mutating func read(into out: UnsafeMutableBufferPointer<Float>) -> Int {
        let wanted = out.count
        guard wanted > 0, let base = out.baseAddress else { return 0 }
        if isBuffering {
            if count >= max(configuration.targetFrames, 1) {
                isBuffering = false
            } else {
                base.update(repeating: 0, count: wanted)
                stats.silentFrames += wanted
                return 0
            }
        }
        let take = min(count, wanted)
        let capacity = ring.count
        ring.withUnsafeBufferPointer { src in
            let first = min(take, capacity - head)
            base.update(from: src.baseAddress! + head, count: first)
            if take > first { (base + first).update(from: src.baseAddress!, count: take - first) }
        }
        head = (head + take) % capacity
        count -= take
        stats.playedFrames += take
        if take < wanted {
            (base + take).update(repeating: 0, count: wanted - take)
            stats.silentFrames += wanted - take
            stats.underruns += 1
            isBuffering = true
        }
        return take
    }

    // MARK: - Ring

    private mutating func makeRoom(for frames: Int) {
        let overflow = count + frames - ring.count
        if overflow > 0 { drop(overflow) }
    }

    private mutating func drop(_ frames: Int) {
        let n = min(frames, count)
        head = (head + n) % ring.count
        count -= n
        stats.droppedFrames += n
    }

    private mutating func append(_ samples: UnsafeBufferPointer<Int16>) {
        let n = min(samples.count, ring.count)
        makeRoom(for: n)
        let capacity = ring.count
        var tail = (head + count) % capacity
        ring.withUnsafeMutableBufferPointer { dst in
            for i in (samples.count - n)..<samples.count {
                dst[tail] = Float(samples[i]) / 32_768
                tail += 1
                if tail == capacity { tail = 0 }
            }
        }
        count += n
    }

    private mutating func appendSilence(_ frames: Int) {
        let n = min(frames, ring.count)
        makeRoom(for: n)
        let capacity = ring.count
        var tail = (head + count) % capacity
        ring.withUnsafeMutableBufferPointer { dst in
            for _ in 0..<n {
                dst[tail] = 0
                tail += 1
                if tail == capacity { tail = 0 }
            }
        }
        count += n
    }
}
