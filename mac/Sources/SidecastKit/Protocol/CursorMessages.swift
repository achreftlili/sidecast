import CoreGraphics
import Foundation

/// CURSOR payload (PROTOCOL §7.3): the Mac's pointer, normalized to the
/// streamed display; (-1, -1) = not on this display. 8 bytes, f32 BE.
public struct CursorPosition: Equatable, Sendable {
    public var x: Float
    public var y: Float

    public init(x: Float, y: Float) {
        self.x = x
        self.y = y
    }

    /// The pointer is on another screen.
    public static let offDisplay = CursorPosition(x: -1, y: -1)

    /// The client shows its pointer only for 0…1 on both axes.
    public var isOnDisplay: Bool { (0...1).contains(x) && (0...1).contains(y) }

    /// A display-local point (points) normalized to `size`, clamped to 0…1.
    public init(point: CGPoint, in size: CGSize) {
        guard size.width > 0, size.height > 0, point.x.isFinite, point.y.isFinite else {
            self = .offDisplay
            return
        }
        x = Float(min(max(point.x / size.width, 0), 1))
        y = Float(min(max(point.y / size.height, 0), 1))
    }

    public func encodedPayload() -> Data {
        var w = ByteWriter(capacity: 8)
        w.f32(x)
        w.f32(y)
        return w.data
    }

    /// Trailing bytes are ignored (forward compatibility).
    public static func decode(_ payload: Data) throws -> CursorPosition {
        var r = ByteReader(payload)
        guard let x = r.f32(), let y = r.f32() else { throw ProtocolError.truncated(.cursor) }
        return CursorPosition(x: x, y: y)
    }
}

/// Rate limit for CURSOR (≤ 60 per second) that never loses the final
/// position: positions offered too soon are held and sent by `flush`.
public struct CursorThrottle: Sendable {
    public enum Decision: Equatable, Sendable {
        /// Send this now.
        case send(CursorPosition)
        /// Held; call `flush(at:)` at this time.
        case deferred(until: TimeInterval)
        /// Same as what the client already has.
        case unchanged
    }

    public let minInterval: TimeInterval
    public private(set) var lastSent: CursorPosition?
    private var lastSentAt: TimeInterval?
    private var pending: CursorPosition?

    public init(maxPerSecond: Double = 60) {
        minInterval = 1 / max(1, maxPerSecond)
    }

    /// A new position. `force` (mode switch, new STREAM_CONFIG) sends even if unchanged, but still respects the rate.
    public mutating func offer(_ position: CursorPosition, at now: TimeInterval, force: Bool = false) -> Decision {
        if !force, pending == nil, position == lastSent { return .unchanged }
        if let at = lastSentAt, now - at < minInterval {
            pending = position
            return .deferred(until: at + minInterval)
        }
        return .send(markSent(position, at: now))
    }

    /// Sends the held position, if any (and if the interval has passed).
    public mutating func flush(at now: TimeInterval) -> CursorPosition? {
        guard let position = pending else { return nil }
        if let at = lastSentAt, now - at < minInterval - 1e-9 { return nil }
        return markSent(position, at: now)
    }

    public var hasPending: Bool { pending != nil }

    /// Forget everything (new mode, new session).
    public mutating func reset() {
        lastSent = nil
        lastSentAt = nil
        pending = nil
    }

    private mutating func markSent(_ position: CursorPosition, at now: TimeInterval) -> CursorPosition {
        pending = nil
        lastSent = position
        lastSentAt = now
        return position
    }
}
