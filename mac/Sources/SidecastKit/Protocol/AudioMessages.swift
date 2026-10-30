import Foundation

/// AUDIO payload (PROTOCOL §7.2): `seq u32 BE`, then PCM signed 16-bit
/// little-endian, mono, 48 kHz — 1…4800 samples (480 = 10 ms recommended).
public struct AudioPacket: Equatable, Sendable {
    public static let sampleRate = 48_000
    public static let channels = 1
    public static let maxSamples = 4_800
    public static let recommendedSamples = 480
    static let headerLength = 4

    public var seq: UInt32
    public var samples: [Int16]

    public init(seq: UInt32, samples: [Int16]) {
        self.seq = seq
        self.samples = samples
    }

    public func encodedPayload() -> Data {
        var out = Data(capacity: Self.headerLength + samples.count * 2)
        withUnsafeBytes(of: seq.bigEndian) { out.append(contentsOf: $0) }
        for s in samples {
            withUnsafeBytes(of: s.littleEndian) { out.append(contentsOf: $0) }
        }
        return out
    }

    /// Odd sample bytes, 0 samples or more than 4800 are malformed.
    public static func decode(_ payload: Data) throws -> AudioPacket {
        guard payload.count >= headerLength else { throw ProtocolError.truncated(.audio) }
        let sampleBytes = payload.count - headerLength
        guard sampleBytes % 2 == 0 else { throw ProtocolError.malformed(.audio, "odd sample byte count \(sampleBytes)") }
        let count = sampleBytes / 2
        guard (1...maxSamples).contains(count) else {
            throw ProtocolError.malformed(.audio, "\(count) samples outside 1…\(maxSamples)")
        }
        return payload.withUnsafeBytes { raw -> AudioPacket in
            let b = raw.bindMemory(to: UInt8.self)
            let seq = UInt32(b[0]) << 24 | UInt32(b[1]) << 16 | UInt32(b[2]) << 8 | UInt32(b[3])
            let samples = [Int16](unsafeUninitializedCapacity: count) { out, initialized in
                for i in 0..<count {
                    let lo = UInt16(b[headerLength + 2 * i]), hi = UInt16(b[headerLength + 2 * i + 1])
                    out[i] = Int16(bitPattern: lo | hi << 8)
                }
                initialized = count
            }
            return AudioPacket(seq: seq, samples: samples)
        }
    }
}
