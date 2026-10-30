import Foundation
import Security

/// Source of cryptographically secure random bytes (injectable for tests).
public protocol RandomSource {
    func bytes(_ count: Int) -> Data
}

/// `SecRandomCopyBytes`-backed random source.
public struct SystemRandom: RandomSource {
    public init() {}

    public func bytes(_ count: Int) -> Data {
        var data = Data(count: count)
        let status = data.withUnsafeMutableBytes { buf in
            SecRandomCopyBytes(kSecRandomDefault, count, buf.baseAddress!)
        }
        // A failure here means the system CSPRNG is unavailable; nothing secure can continue.
        precondition(status == errSecSuccess, "SecRandomCopyBytes failed: \(status)")
        return data
    }
}

/// Compares two byte strings in time independent of where they differ.
public func constantTimeEquals(_ a: Data, _ b: Data) -> Bool {
    guard a.count == b.count else { return false }
    var diff: UInt8 = 0
    for (x, y) in zip(a, b) { diff |= x ^ y }
    return diff == 0
}

extension Data {
    /// Lowercase hexadecimal representation.
    public var hexString: String { map { String(format: "%02x", $0) }.joined() }

    /// Parses lowercase or uppercase hex; nil if malformed.
    public init?(hexString: String) {
        let chars = Array(hexString.utf8)
        guard chars.count % 2 == 0 else { return nil }
        var out = Data(capacity: chars.count / 2)
        func nibble(_ c: UInt8) -> UInt8? {
            switch c {
            case 48...57: return c - 48
            case 97...102: return c - 87
            case 65...70: return c - 55
            default: return nil
            }
        }
        var i = 0
        while i < chars.count {
            guard let hi = nibble(chars[i]), let lo = nibble(chars[i + 1]) else { return nil }
            out.append(hi << 4 | lo)
            i += 2
        }
        self = out
    }
}
