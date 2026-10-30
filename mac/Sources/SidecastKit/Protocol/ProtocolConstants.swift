import Foundation

/// Constants of the Sidecast wire protocol v1 (see docs/PROTOCOL.md).
public enum SidecastProtocol {
    public static let version = 1
    public static let defaultPort: UInt16 = 47800
    public static let bonjourServiceType = "_sidecast._tcp"
    /// Largest payload a frame may carry (16 MiB). Larger is a protocol error.
    public static let maxPayloadLength = 16 * 1024 * 1024
    /// Size of the frame header: type (u8) + length (u32).
    public static let headerLength = 5

    /// Pairing: code lifetime and attempts (PROTOCOL §5).
    public static let pairingLifetime: TimeInterval = 120
    public static let pairingAttempts = 5
    /// Any connection not authenticated within this time is closed.
    public static let authTimeout: TimeInterval = 150
    /// A streaming client that sent nothing for this long is closed.
    public static let livenessTimeout: TimeInterval = 5

    /// Flow control (PROTOCOL §6).
    public static let maxFramesInFlight = 4
    public static let keyframeInterval: TimeInterval = 2
}

/// Message type codes (PROTOCOL §4).
public enum MessageType: UInt8, CaseIterable, Sendable {
    // Client → Server
    case hello = 0x01
    case auth = 0x02
    case pairStart = 0x03
    case pairProof = 0x04
    case touch = 0x10
    case frameAck = 0x11
    case keyframeRequest = 0x12
    case ping = 0x13
    case text = 0x14
    case key = 0x15
    case audio = 0x16
    case micState = 0x17
    case inputMode = 0x18
    // Server → Client
    case serverHello = 0x81
    case pairChallenge = 0x82
    case authResult = 0x83
    case streamConfig = 0x84
    case videoFrame = 0x85
    case pong = 0x86
    case bye = 0x87
    case micControl = 0x88
    case cursor = 0x89
}

/// Capabilities a client lists in HELLO `features` (PROTOCOL §5).
public enum ClientFeature {
    /// Phone microphone → "Sidecast Microphone" (PROTOCOL §7.2).
    public static let mic = "mic"
}

/// `reason` values used in AUTH_RESULT and BYE.
public enum ByeReason: String, Codable, Sendable {
    case badCode = "bad_code"
    case tooManyAttempts = "too_many_attempts"
    case pairingExpired = "pairing_expired"
    case unknownDevice = "unknown_device"
    case protocolMismatch = "protocol_mismatch"
    case replaced
    case serverShutdown = "server_shutdown"
    case displayFailed = "display_failed"
    case permissionMissing = "permission_missing"
}

/// Errors raised while parsing or decoding wire data.
public enum ProtocolError: Error, Equatable, CustomStringConvertible {
    case payloadTooLarge(Int)
    case truncated(MessageType)
    case malformed(MessageType, String)

    public var description: String {
        switch self {
        case .payloadTooLarge(let n): return "payload of \(n) bytes exceeds the 16 MiB limit"
        case .truncated(let t): return "\(t) payload is truncated"
        case .malformed(let t, let why): return "\(t) payload is malformed: \(why)"
        }
    }
}
