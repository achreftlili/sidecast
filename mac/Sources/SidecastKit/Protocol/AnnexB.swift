import Foundation

/// Conversion between VideoToolbox's length-prefixed (AVCC/HVCC) NAL units
/// and the Annex-B byte stream carried in VIDEO_FRAME.
public enum AnnexB {
    public static let startCode: [UInt8] = [0, 0, 0, 1]

    /// Builds one Annex-B access unit: `parameterSets` (VPS/SPS/PPS, keyframes
    /// only) followed by the NAL units of `lengthPrefixed`, each behind a
    /// 4-byte start code. Returns nil when the length-prefixed data is corrupt.
    public static func fromLengthPrefixed(_ lengthPrefixed: UnsafeRawBufferPointer,
                                          nalLengthSize: Int = 4,
                                          parameterSets: [Data] = []) -> Data? {
        guard (1...4).contains(nalLengthSize) else { return nil }
        var out = Data(capacity: lengthPrefixed.count + parameterSets.reduce(0) { $0 + $1.count + 4 } + 64)
        for ps in parameterSets {
            out.append(contentsOf: startCode)
            out.append(ps)
        }
        var offset = 0
        let total = lengthPrefixed.count
        while offset < total {
            guard total - offset >= nalLengthSize else { return nil }
            var length = 0
            for i in 0..<nalLengthSize { length = (length << 8) | Int(lengthPrefixed[offset + i]) }
            offset += nalLengthSize
            guard length > 0, total - offset >= length else { return nil }
            out.append(contentsOf: startCode)
            out.append(contentsOf: UnsafeRawBufferPointer(rebasing: lengthPrefixed[offset..<(offset + length)]))
            offset += length
        }
        return out
    }

    public static func fromLengthPrefixed(_ data: Data, nalLengthSize: Int = 4,
                                          parameterSets: [Data] = []) -> Data? {
        data.withUnsafeBytes { fromLengthPrefixed($0, nalLengthSize: nalLengthSize, parameterSets: parameterSets) }
    }

    /// Splits an Annex-B stream into NAL units (without start codes).
    /// Accepts both 3-byte and 4-byte start codes.
    public static func split(_ stream: Data) -> [Data] {
        let bytes = [UInt8](stream)
        var units: [Data] = []
        var i = 0
        var nalStart: Int? = nil
        let n = bytes.count
        while i + 2 < n {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                if let s = nalStart {
                    var end = i
                    if end > s, bytes[end - 1] == 0 { end -= 1 }   // 4-byte start code
                    if end > s { units.append(Data(bytes[s..<end])) }
                }
                i += 3
                nalStart = i
            } else {
                i += 1
            }
        }
        if let s = nalStart, s < n { units.append(Data(bytes[s..<n])) }
        return units
    }

    /// Re-encodes NAL units with 4-byte big-endian length prefixes.
    public static func lengthPrefixed(_ units: [Data]) -> Data {
        var out = Data(capacity: units.reduce(0) { $0 + $1.count + 4 })
        for u in units {
            withUnsafeBytes(of: UInt32(u.count).bigEndian) { out.append(contentsOf: $0) }
            out.append(u)
        }
        return out
    }

    /// HEVC NAL unit type (`(byte0 >> 1) & 0x3F`).
    public static func hevcNALType(_ unit: Data) -> UInt8? {
        unit.first.map { ($0 >> 1) & 0x3F }
    }

    /// H.264 NAL unit type (`byte0 & 0x1F`).
    public static func h264NALType(_ unit: Data) -> UInt8? {
        unit.first.map { $0 & 0x1F }
    }
}
