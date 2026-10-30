import Foundation

/// Android `MotionEvent` action codes carried in TOUCH (PROTOCOL §7).
public enum TouchAction: UInt8, Sendable {
    case down = 0
    case up = 1
    case move = 2
    case cancel = 3
    case pointerDown = 5
    case pointerUp = 6
}

public struct TouchPointer: Equatable, Sendable {
    public var id: UInt8
    /// Normalized to the video frame: (0,0) top-left … (1,1) bottom-right.
    public var x: Float
    public var y: Float

    public init(id: UInt8, x: Float, y: Float) {
        self.id = id
        self.x = x
        self.y = y
    }
}

/// TOUCH payload.
public struct TouchEvent: Equatable, Sendable {
    public static let maxPointers = 10

    /// Client uptime in ms. Wraps; only differences are meaningful.
    public var eventTimeMs: UInt32
    /// Raw action byte. Use `touchAction` for the known values.
    public var action: UInt8
    public var actionPointerId: UInt8
    public var pointers: [TouchPointer]

    public init(eventTimeMs: UInt32, action: TouchAction, actionPointerId: UInt8, pointers: [TouchPointer]) {
        self.eventTimeMs = eventTimeMs
        self.action = action.rawValue
        self.actionPointerId = actionPointerId
        self.pointers = pointers
    }

    public init(eventTimeMs: UInt32, rawAction: UInt8, actionPointerId: UInt8, pointers: [TouchPointer]) {
        self.eventTimeMs = eventTimeMs
        self.action = rawAction
        self.actionPointerId = actionPointerId
        self.pointers = pointers
    }

    public var touchAction: TouchAction? { TouchAction(rawValue: action) }

    public func encodedPayload() -> Data {
        var w = ByteWriter(capacity: 7 + pointers.count * 9)
        w.u32(eventTimeMs)
        w.u8(action)
        w.u8(actionPointerId)
        w.u8(UInt8(pointers.count))
        for p in pointers {
            w.u8(p.id)
            w.f32(p.x)
            w.f32(p.y)
        }
        return w.data
    }

    /// Decodes a TOUCH payload. Trailing bytes are ignored (forward compatibility).
    public static func decode(_ payload: Data) throws -> TouchEvent {
        var r = ByteReader(payload)
        guard let time = r.u32(), let action = r.u8(), let pid = r.u8(), let count = r.u8() else {
            throw ProtocolError.truncated(.touch)
        }
        guard count >= 1, Int(count) <= maxPointers else {
            throw ProtocolError.malformed(.touch, "pointer count \(count) outside 1…\(maxPointers)")
        }
        var pointers: [TouchPointer] = []
        pointers.reserveCapacity(Int(count))
        for _ in 0..<count {
            guard let id = r.u8(), let x = r.f32(), let y = r.f32() else {
                throw ProtocolError.truncated(.touch)
            }
            pointers.append(TouchPointer(id: id, x: x, y: y))
        }
        return TouchEvent(eventTimeMs: time, rawAction: action, actionPointerId: pid, pointers: pointers)
    }
}

/// VIDEO_FRAME payload: `seq u32, flags u8, ptsMicros u64, Annex-B data`.
public struct VideoFrame: Equatable, Sendable {
    public static let headerLength = 13
    public static let keyframeFlag: UInt8 = 0x01

    public var seq: UInt32
    public var flags: UInt8
    public var ptsMicros: UInt64
    public var data: Data

    public init(seq: UInt32, isKeyframe: Bool, ptsMicros: UInt64, data: Data) {
        self.seq = seq
        self.flags = isKeyframe ? Self.keyframeFlag : 0
        self.ptsMicros = ptsMicros
        self.data = data
    }

    public init(seq: UInt32, flags: UInt8, ptsMicros: UInt64, data: Data) {
        self.seq = seq
        self.flags = flags
        self.ptsMicros = ptsMicros
        self.data = data
    }

    public var isKeyframe: Bool { flags & Self.keyframeFlag != 0 }

    /// Complete wire frame (header + payload) built in one allocation.
    public func encodedFrame() -> Data {
        let payloadLength = Self.headerLength + data.count
        var out = Data(capacity: SidecastProtocol.headerLength + payloadLength)
        out.append(MessageType.videoFrame.rawValue)
        withUnsafeBytes(of: UInt32(payloadLength).bigEndian) { out.append(contentsOf: $0) }
        withUnsafeBytes(of: seq.bigEndian) { out.append(contentsOf: $0) }
        out.append(flags)
        withUnsafeBytes(of: ptsMicros.bigEndian) { out.append(contentsOf: $0) }
        out.append(data)
        return out
    }

    public static func decode(_ payload: Data) throws -> VideoFrame {
        guard payload.count >= headerLength else { throw ProtocolError.truncated(.videoFrame) }
        var r = ByteReader(payload.prefix(headerLength))
        let seq = r.u32()!, flags = r.u8()!, pts = r.u64()!
        let data = payload.subdata(in: (payload.startIndex + headerLength)..<payload.endIndex)
        return VideoFrame(seq: seq, flags: flags, ptsMicros: pts, data: data)
    }
}
