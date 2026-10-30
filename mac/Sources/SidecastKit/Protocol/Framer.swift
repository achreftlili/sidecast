import Foundation

/// One framed message: `type: u8, length: u32 BE, payload`.
public struct Frame: Equatable, Sendable {
    public let type: UInt8
    public let payload: Data

    public init(type: UInt8, payload: Data = Data()) {
        self.type = type
        self.payload = payload
    }

    public init(_ type: MessageType, payload: Data = Data()) {
        self.init(type: type.rawValue, payload: payload)
    }

    /// The known message type, or nil for a type this version does not define.
    public var messageType: MessageType? { MessageType(rawValue: type) }

    /// Wire encoding of this frame (header + payload).
    public func encoded() -> Data {
        var out = Data(capacity: SidecastProtocol.headerLength + payload.count)
        out.append(type)
        withUnsafeBytes(of: UInt32(payload.count).bigEndian) { out.append(contentsOf: $0) }
        out.append(payload)
        return out
    }
}

/// Incremental frame parser: feed it bytes as they arrive (in any chunking),
/// then pull complete frames. A declared length above 16 MiB is a protocol
/// error; the connection should be closed when `nextFrame()` throws.
public final class FrameParser {
    private var buffer = Data()
    private var readOffset = 0
    private let maxPayload: Int

    public init(maxPayload: Int = SidecastProtocol.maxPayloadLength) {
        self.maxPayload = maxPayload
    }

    /// Bytes received but not yet returned as frames.
    public var bufferedByteCount: Int { buffer.count - readOffset }

    public func append(_ data: Data) {
        compactIfNeeded()
        buffer.append(data)
    }

    /// Returns the next complete frame, or nil when more bytes are needed.
    public func nextFrame() throws -> Frame? {
        guard bufferedByteCount >= SidecastProtocol.headerLength else { return nil }
        let start = buffer.startIndex + readOffset
        let type = buffer[start]
        var length = 0
        for i in 1...4 { length = (length << 8) | Int(buffer[start + i]) }
        guard length <= maxPayload else { throw ProtocolError.payloadTooLarge(length) }
        let total = SidecastProtocol.headerLength + length
        guard bufferedByteCount >= total else { return nil }
        let payload = buffer.subdata(in: (start + SidecastProtocol.headerLength)..<(start + total))
        readOffset += total
        return Frame(type: type, payload: payload)
    }

    /// Drains every complete frame currently buffered.
    public func drain() throws -> [Frame] {
        var frames: [Frame] = []
        while let frame = try nextFrame() { frames.append(frame) }
        return frames
    }

    /// Drops consumed bytes once they dominate the buffer (amortised O(1) per byte).
    private func compactIfNeeded() {
        guard readOffset > 0 else { return }
        if readOffset == buffer.count {
            buffer = Data()
            readOffset = 0
        } else if readOffset >= 64 * 1024 && readOffset * 2 >= buffer.count {
            buffer = buffer.subdata(in: (buffer.startIndex + readOffset)..<buffer.endIndex)
            readOffset = 0
        }
    }
}
