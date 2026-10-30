import Foundation

/// Appends big-endian integers and floats to a byte buffer.
public struct ByteWriter {
    public private(set) var data: Data

    public init(capacity: Int = 64) {
        data = Data()
        data.reserveCapacity(capacity)
    }

    public mutating func u8(_ v: UInt8) { data.append(v) }

    public mutating func u32(_ v: UInt32) {
        withUnsafeBytes(of: v.bigEndian) { data.append(contentsOf: $0) }
    }

    public mutating func u64(_ v: UInt64) {
        withUnsafeBytes(of: v.bigEndian) { data.append(contentsOf: $0) }
    }

    /// IEEE-754 binary32, big-endian (Java `DataOutputStream.writeFloat` order).
    public mutating func f32(_ v: Float) { u32(v.bitPattern) }

    public mutating func bytes<D: DataProtocol>(_ d: D) { data.append(contentsOf: d) }
}

/// Reads big-endian integers and floats from a byte buffer.
public struct ByteReader {
    private let bytes: [UInt8]
    public private(set) var offset = 0

    public init<D: DataProtocol>(_ data: D) { bytes = Array(data) }

    public var remaining: Int { bytes.count - offset }

    public mutating func u8() -> UInt8? {
        guard remaining >= 1 else { return nil }
        defer { offset += 1 }
        return bytes[offset]
    }

    public mutating func u32() -> UInt32? {
        guard remaining >= 4 else { return nil }
        var v: UInt32 = 0
        for i in 0..<4 { v = (v << 8) | UInt32(bytes[offset + i]) }
        offset += 4
        return v
    }

    public mutating func u64() -> UInt64? {
        guard remaining >= 8 else { return nil }
        var v: UInt64 = 0
        for i in 0..<8 { v = (v << 8) | UInt64(bytes[offset + i]) }
        offset += 8
        return v
    }

    public mutating func f32() -> Float? { u32().map { Float(bitPattern: $0) } }

    public mutating func rest() -> Data {
        defer { offset = bytes.count }
        return Data(bytes[offset...])
    }
}
