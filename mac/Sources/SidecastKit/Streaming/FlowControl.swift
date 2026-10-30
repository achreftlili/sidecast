import Foundation

/// Tracks frames that are being encoded or have been sent but not acked
/// (PROTOCOL §6). A captured frame may only be encoded when a slot is free,
/// so a backed-up link drops frames *before* encoding and the decoder never
/// misses a reference. Not thread-safe; guard it with the caller's lock/queue.
public struct InFlightTracker: Sendable {
    public let capacity: Int
    /// Frames handed to the encoder whose output has not been sent yet.
    public private(set) var reserved = 0
    /// seq → send time, for frames sent and not yet acknowledged.
    private var sent: [UInt32: TimeInterval] = [:]
    private var nextSeq: UInt32 = 0

    public init(capacity: Int = SidecastProtocol.maxFramesInFlight) {
        self.capacity = capacity
    }

    public var inFlight: Int { reserved + sent.count }
    public var sentUnacked: Int { sent.count }

    /// Claims a slot for a frame about to be encoded. False means: drop it.
    public mutating func tryReserve() -> Bool {
        guard inFlight < capacity else { return false }
        reserved += 1
        return true
    }

    /// The encoder produced no output for a reserved frame.
    public mutating func cancelReservation() {
        reserved = max(0, reserved - 1)
    }

    /// Turns a reservation into a sent frame and returns its seq.
    public mutating func markSent(at time: TimeInterval) -> UInt32 {
        reserved = max(0, reserved - 1)
        let seq = nextSeq
        nextSeq &+= 1
        sent[seq] = time
        return seq
    }

    /// Records FRAME_ACK(seq). Acks are cumulative for robustness: every
    /// frame up to `seq` leaves the window. Returns the RTT sample for `seq`
    /// itself (ack arrival − send time), or nil for an unknown/duplicate seq.
    public mutating func ack(_ seq: UInt32, at time: TimeInterval) -> TimeInterval? {
        let sample = sent[seq].map { max(0, time - $0) }
        // Wrap-aware "older than or equal": the distance back from seq is < 2^31.
        sent = sent.filter { key, _ in seq &- key >= 0x8000_0000 }
        return sample
    }

    /// Forgets all sent frames (e.g. when the connection resets its stream).
    public mutating func reset() {
        reserved = 0
        sent.removeAll()
    }
}

/// AIMD bitrate control evaluated once per second (PROTOCOL §6):
/// - a dropped frame, or smoothed RTT > min RTT + 40 ms → bitrate × 0.75 (≥ min)
/// - after 2 consecutive clean windows → + max(1 Mbps, 5 % of max) (≤ max)
///
/// A window without any ack is neither clean nor congested (no evidence) and
/// leaves both the bitrate and the clean-window streak unchanged. Drops during
/// warm-up (before `warmupAcks` acks, i.e. while the encoder pipeline fills at
/// stream start) say nothing about the link and are not counted.
public struct AdaptiveBitrateController: Sendable {
    public struct Config: Equatable, Sendable {
        public var startBitrate: Int
        public var minBitrate: Int
        public var maxBitrate: Int
        public var rttMargin: TimeInterval
        public var decreaseFactor: Double
        public var cleanWindowsBeforeIncrease: Int
        public var window: TimeInterval
        public var warmupAcks: Int

        /// Bounds from the user setting and the client's limit; starts at 20 Mbps clamped into them.
        public init(bounds: BitrateBounds, window: TimeInterval = 1.0) {
            self.init(startBitrate: bounds.start, minBitrate: bounds.floor, maxBitrate: bounds.ceiling, window: window)
        }

        public init(startBitrate: Int = 20_000_000, minBitrate: Int = 4_000_000,
                    maxBitrate: Int = 50_000_000, rttMargin: TimeInterval = 0.040,
                    decreaseFactor: Double = 0.75, cleanWindowsBeforeIncrease: Int = 2,
                    window: TimeInterval = 1.0, warmupAcks: Int = 30) {
            self.startBitrate = startBitrate
            self.minBitrate = minBitrate
            self.maxBitrate = maxBitrate
            self.rttMargin = rttMargin
            self.decreaseFactor = decreaseFactor
            self.cleanWindowsBeforeIncrease = cleanWindowsBeforeIncrease
            self.window = window
            self.warmupAcks = warmupAcks
        }
    }

    public enum Decision: Equatable, Sendable {
        case hold
        case decrease(reason: String)
        case increase
    }

    public private(set) var config: Config
    public private(set) var bitrate: Int
    public private(set) var smoothedRTT: TimeInterval?
    public private(set) var minRTT: TimeInterval?
    public private(set) var cleanStreak = 0
    private var windowStart: TimeInterval
    private var droppedInWindow = 0
    private var samplesInWindow = 0
    private var totalSamples = 0

    public init(config: Config = Config(), now: TimeInterval) {
        self.config = Self.sanitized(config)
        self.bitrate = min(max(self.config.startBitrate, self.config.minBitrate), self.config.maxBitrate)
        self.windowStart = now
    }

    /// The step used for additive increase.
    public var increaseStep: Int { max(1_000_000, config.maxBitrate / 20) }

    public mutating func recordDrop(count: Int = 1) {
        guard totalSamples >= config.warmupAcks else { return }   // warm-up
        droppedInWindow += count
    }

    /// Feeds one RTT sample (EWMA with α = 1/8, like TCP's SRTT).
    public mutating func recordRTT(_ rtt: TimeInterval) {
        samplesInWindow += 1
        totalSamples += 1
        minRTT = min(minRTT ?? rtt, rtt)
        if let s = smoothedRTT {
            smoothedRTT = s + (rtt - s) / 8
        } else {
            smoothedRTT = rtt
        }
    }

    /// Changes the ceiling (user setting). The bitrate is clamped immediately.
    public mutating func setMaxBitrate(_ maxBitrate: Int) {
        config.maxBitrate = maxBitrate
        config = Self.sanitized(config)
        bitrate = min(max(bitrate, config.minBitrate), config.maxBitrate)
    }

    /// Changes floor and ceiling together (setting × client limit). The bitrate is clamped immediately.
    public mutating func setBounds(_ bounds: BitrateBounds) {
        config.minBitrate = bounds.floor
        config.maxBitrate = bounds.ceiling
        config = Self.sanitized(config)
        bitrate = min(max(bitrate, config.minBitrate), config.maxBitrate)
    }

    /// Call periodically; evaluates every elapsed window. Returns the decision
    /// of the last evaluated window, or nil if no window ended yet.
    @discardableResult
    public mutating func tick(now: TimeInterval) -> Decision? {
        guard now - windowStart >= config.window else { return nil }
        windowStart = now
        let dropped = droppedInWindow
        let samples = samplesInWindow
        droppedInWindow = 0
        samplesInWindow = 0

        let rttCongested: Bool = {
            guard let s = smoothedRTT, let m = minRTT else { return false }
            return s > m + config.rttMargin
        }()
        if dropped > 0 || rttCongested {
            cleanStreak = 0
            bitrate = max(config.minBitrate, Int(Double(bitrate) * config.decreaseFactor))
            return .decrease(reason: dropped > 0 ? "\(dropped) dropped" : "rtt")
        }
        guard samples > 0 else { return .hold }
        cleanStreak += 1
        if cleanStreak >= config.cleanWindowsBeforeIncrease {
            cleanStreak = 0
            let next = min(config.maxBitrate, bitrate + increaseStep)
            if next != bitrate {
                bitrate = next
                return .increase
            }
        }
        return .hold
    }

    private static func sanitized(_ c: Config) -> Config {
        var c = c
        c.minBitrate = max(100_000, c.minBitrate)
        c.maxBitrate = max(c.minBitrate, c.maxBitrate)
        return c
    }
}
