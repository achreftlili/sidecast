import Foundation

/// HAPTIC payload (PROTOCOL §7.4): one byte, the kind of tick the phone gives.
public enum HapticKind: UInt8, Sendable {
    /// The computer pressed the left button because the finger rested ½ s.
    case hold = 1

    /// nil for an unknown kind or a malformed payload: the client ignores it (never fatal).
    public static func decode(_ payload: Data) -> HapticKind? {
        guard payload.count == 1, let kind = payload.first else { return nil }
        return HapticKind(rawValue: kind)
    }
}
