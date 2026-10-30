import XCTest
@testable import SidecastKit

/// Replays a fixed byte sequence (then zeros).
final class ScriptedRandom: RandomSource {
    private var bytes: [UInt8]
    init(_ bytes: [UInt8]) { self.bytes = bytes }
    func bytes(_ count: Int) -> Data {
        var out = Data()
        for _ in 0..<count { out.append(bytes.isEmpty ? 0 : bytes.removeFirst()) }
        return out
    }
}

final class PairingTests: XCTestCase {
    private let fingerprint = Data(repeating: 0x11, count: 32)
    private let nonce = Data(repeating: 0x22, count: 16)

    private func proof(_ code: String) -> Data {
        PairingProofCalculator.proof(code: code, certFingerprint: fingerprint, nonce: nonce)
    }

    func testCodeFormatAndDisplay() {
        let code = PairingCode.generate(using: ScriptedRandom([0, 0, 0x30, 0x39]))   // 12345
        XCTAssertEqual(code.digits, "012345")
        XCTAssertEqual(code.displayString, "012 345")
        XCTAssertNil(PairingCode("12345"))
        XCTAssertNil(PairingCode("12345a"))
        XCTAssertNil(PairingCode("１２３４５６"))   // full-width digits are not ASCII
    }

    func testCodeGenerationRejectsBiasedRange() {
        // 0xFFFFFFFF ≥ 4_294_000_000 is rejected; the next draw (999_999) is used.
        let code = PairingCode.generate(using: ScriptedRandom([0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x0F, 0x42, 0x3F]))
        XCTAssertEqual(code.digits, "999999")
        // 4_293_999_999 (just below the limit) is accepted → 999_999.
        let edge = PairingCode.generate(using: ScriptedRandom([0xFF, 0xF1, 0x3D, 0x7F]))
        XCTAssertEqual(edge.digits, "999999")
    }

    func testSystemCodesAreSixDigitsAndVary() {
        let codes = Set((0..<50).map { _ in PairingCode.generate().digits })
        XCTAssertTrue(codes.allSatisfy { $0.count == 6 && $0.allSatisfy(\.isNumber) })
        XCTAssertGreaterThan(codes.count, 40)
    }

    func testCorrectProofSucceeds() {
        var s = PairingSession(code: PairingCode("482913")!, startedAt: 100)
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("482913"), certFingerprint: fingerprint, at: 110), .success)
        XCTAssertTrue(s.isFinished)
        // Replays after success are refused.
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("482913"), certFingerprint: fingerprint, at: 111), .expired)
    }

    func testProofOverDifferentCertificateFails() {
        var s = PairingSession(code: PairingCode("482913")!, startedAt: 0)
        let mitm = PairingProofCalculator.proof(code: "482913", certFingerprint: Data(repeating: 0x99, count: 32), nonce: nonce)
        XCTAssertEqual(s.verify(nonce: nonce, proof: mitm, certFingerprint: fingerprint, at: 1), .badCode(attemptsLeft: 4))
    }

    func testWrongCodesCountDownThenLockOut() {
        var s = PairingSession(code: PairingCode("000001")!, startedAt: 0)
        XCTAssertEqual(s.secondsRemaining(at: 0), 120)
        for left in stride(from: 4, through: 1, by: -1) {
            XCTAssertEqual(s.verify(nonce: nonce, proof: proof("999999"), certFingerprint: fingerprint, at: 1),
                           .badCode(attemptsLeft: left))
        }
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("999999"), certFingerprint: fingerprint, at: 2), .tooManyAttempts)
        XCTAssertEqual(s.attemptsLeft, 0)
        // Even the right code is refused once locked out.
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("000001"), certFingerprint: fingerprint, at: 3), .expired)
    }

    func testRetryAfterWrongCodeCanSucceed() {
        var s = PairingSession(code: PairingCode("135790")!, startedAt: 0)
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("135791"), certFingerprint: fingerprint, at: 1), .badCode(attemptsLeft: 4))
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("135790"), certFingerprint: fingerprint, at: 2), .success)
    }

    func testExpiry() {
        var s = PairingSession(code: PairingCode("135790")!, startedAt: 10)
        XCTAssertEqual(s.secondsRemaining(at: 129.5), 1)
        XCTAssertFalse(s.isExpired(at: 129.9))
        XCTAssertEqual(s.verify(nonce: nonce, proof: proof("135790"), certFingerprint: fingerprint, at: 130), .expired)
        XCTAssertEqual(s.secondsRemaining(at: 500), 0)
    }

    func testMalformedProofLengthsCostAnAttempt() {
        var s = PairingSession(code: PairingCode("135790")!, startedAt: 0)
        XCTAssertEqual(s.verify(nonce: Data(count: 8), proof: proof("135790"), certFingerprint: fingerprint, at: 1),
                       .badCode(attemptsLeft: 4))
        XCTAssertEqual(s.verify(nonce: nonce, proof: Data(count: 31), certFingerprint: fingerprint, at: 1),
                       .badCode(attemptsLeft: 3))
    }

    func testConstantTimeEquals() {
        XCTAssertTrue(constantTimeEquals(Data([1, 2, 3]), Data([1, 2, 3])))
        XCTAssertFalse(constantTimeEquals(Data([1, 2, 3]), Data([1, 2, 4])))
        XCTAssertFalse(constantTimeEquals(Data([1, 2]), Data([1, 2, 3])))
        XCTAssertTrue(constantTimeEquals(Data(), Data()))
    }

    func testTokensAreRandom32Bytes() {
        let a = DeviceToken.generate(), b = DeviceToken.generate()
        XCTAssertEqual(a.count, 32)
        XCTAssertNotEqual(a, b)
    }
}
