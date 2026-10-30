import CryptoKit
import Foundation

/// The 6-digit code shown on the Mac during pairing.
public struct PairingCode: Equatable, Sendable, CustomStringConvertible {
    /// Exactly six ASCII digits, `000000`–`999999`.
    public let digits: String

    public init?(_ digits: String) {
        guard digits.count == 6, digits.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
        self.digits = digits
    }

    /// Uniformly random code (rejection sampling, no modulo bias).
    public static func generate(using random: RandomSource = SystemRandom()) -> PairingCode {
        let range: UInt64 = 1_000_000
        let space: UInt64 = 1 << 32
        let limit = space - space % range          // 4_294_000_000
        while true {
            let b = random.bytes(4)
            let value = b.reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
            if value < limit {
                return PairingCode(String(format: "%06llu", value % range))!
            }
        }
    }

    /// "123 456", as shown to the user.
    public var displayString: String {
        "\(digits.prefix(3)) \(digits.suffix(3))"
    }

    public var description: String { displayString }
}

/// `proof = HMAC-SHA256(key = ASCII(code), message = certFingerprint ‖ nonce)` (PROTOCOL §5).
public enum PairingProofCalculator {
    public static let nonceLength = 16
    public static let proofLength = 32

    public static func proof(code: String, certFingerprint: Data, nonce: Data) -> Data {
        let key = SymmetricKey(data: Data(code.utf8))
        var message = Data(certFingerprint)
        message.append(nonce)
        return Data(HMAC<SHA256>.authenticationCode(for: message, using: key))
    }
}

/// Result of checking one PAIR_PROOF.
public enum PairingOutcome: Equatable, Sendable {
    case success
    case badCode(attemptsLeft: Int)
    case tooManyAttempts
    case expired
}

/// One pairing attempt window: a code, a deadline and an attempt budget.
/// Pure state machine; the caller supplies the current time.
public struct PairingSession: Sendable {
    public let code: PairingCode
    public let startedAt: TimeInterval
    public let lifetime: TimeInterval
    public private(set) var attemptsLeft: Int
    public private(set) var isFinished = false

    public init(code: PairingCode,
                startedAt: TimeInterval,
                lifetime: TimeInterval = SidecastProtocol.pairingLifetime,
                attempts: Int = SidecastProtocol.pairingAttempts) {
        self.code = code
        self.startedAt = startedAt
        self.lifetime = lifetime
        self.attemptsLeft = attempts
    }

    public var expiresAt: TimeInterval { startedAt + lifetime }

    public func isExpired(at now: TimeInterval) -> Bool { now >= expiresAt }

    /// Whole seconds left, never negative.
    public func secondsRemaining(at now: TimeInterval) -> Int {
        max(0, Int((expiresAt - now).rounded(.up)))
    }

    /// Verifies a proof computed over `certFingerprint` (the server's own leaf).
    /// After `.success`, `.tooManyAttempts` or `.expired` the session is finished
    /// and every further call returns `.expired`.
    public mutating func verify(nonce: Data, proof: Data, certFingerprint: Data,
                                at now: TimeInterval) -> PairingOutcome {
        guard !isFinished else { return .expired }
        if isExpired(at: now) {
            isFinished = true
            return .expired
        }
        let wellFormed = nonce.count == PairingProofCalculator.nonceLength
            && proof.count == PairingProofCalculator.proofLength
        if wellFormed {
            let expected = PairingProofCalculator.proof(code: code.digits,
                                                        certFingerprint: certFingerprint,
                                                        nonce: nonce)
            if constantTimeEquals(expected, proof) {
                isFinished = true
                return .success
            }
        }
        attemptsLeft = max(0, attemptsLeft - 1)
        if attemptsLeft == 0 {
            isFinished = true
            return .tooManyAttempts
        }
        return .badCode(attemptsLeft: attemptsLeft)
    }
}
