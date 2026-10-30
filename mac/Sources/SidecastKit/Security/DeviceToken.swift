import CryptoKit
import Foundation

/// Device tokens issued at pairing. Only `SHA-256(token)` is ever stored.
public enum DeviceToken {
    public static let length = 32

    public static func generate(using random: RandomSource = SystemRandom()) -> Data {
        random.bytes(length)
    }

    public static func hash(_ token: Data) -> Data {
        Data(SHA256.hash(data: token))
    }
}

/// Certificate fingerprints: SHA-256 of the leaf certificate's DER encoding.
public enum CertificateFingerprint {
    public static func sha256(der: Data) -> Data {
        Data(SHA256.hash(data: der))
    }
}
