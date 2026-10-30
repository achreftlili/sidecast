import Foundation

/// TEXT payload (PROTOCOL §7.1): UTF-8 text typed on the phone, 1…4096 bytes.
public enum TextMessage {
    public static let maxBytes = 4096

    /// UTF-8 encoding of `text` (no validation: encoders may build bad frames for testing).
    public static func encodedPayload(_ text: String) -> Data { Data(text.utf8) }

    /// Decodes a TEXT payload. Empty, oversized or invalid UTF-8 payloads are malformed.
    public static func decode(_ payload: Data) throws -> String {
        guard !payload.isEmpty else { throw ProtocolError.malformed(.text, "empty") }
        guard payload.count <= maxBytes else {
            throw ProtocolError.malformed(.text, "\(payload.count) bytes exceeds \(maxBytes)")
        }
        // `String(decoding:)` repairs bad sequences with U+FFFD; a lossless
        // round trip proves the bytes were valid UTF-8.
        let text = String(decoding: payload, as: UTF8.self)
        guard text.utf8.elementsEqual(payload) else { throw ProtocolError.malformed(.text, "invalid UTF-8") }
        return text
    }
}

/// The `key` byte of KEY (PROTOCOL §7.1).
public enum ProtocolKey: UInt8, CaseIterable, Sendable {
    case returnKey = 1
    case backspace = 2
    case forwardDelete = 3
    case tab = 4
    case escape = 5
    case left = 6
    case right = 7
    case up = 8
    case down = 9
    case home = 10
    case end = 11
    case pageUp = 12
    case pageDown = 13
}

/// The `modifiers` bitmask of KEY (PROTOCOL §7.1). Unknown bits are ignored.
public struct KeyModifiers: OptionSet, Equatable, Sendable {
    public let rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }

    public static let shift = KeyModifiers(rawValue: 1)
    public static let control = KeyModifiers(rawValue: 2)
    public static let option = KeyModifiers(rawValue: 4)
    public static let command = KeyModifiers(rawValue: 8)

    public static let all: KeyModifiers = [.shift, .control, .option, .command]
}

/// KEY payload: `key u8, modifiers u8`. Raw bytes are kept so unknown values
/// survive decoding; consumers ignore what they do not know.
public struct KeyEvent: Equatable, Sendable {
    public static let payloadLength = 2

    public var key: UInt8
    public var modifiers: UInt8

    public init(key: UInt8, modifiers: UInt8 = 0) {
        self.key = key
        self.modifiers = modifiers
    }

    public init(_ key: ProtocolKey, modifiers: KeyModifiers = []) {
        self.init(key: key.rawValue, modifiers: modifiers.rawValue)
    }

    /// The known key, or nil (the event is then ignored).
    public var protocolKey: ProtocolKey? { ProtocolKey(rawValue: key) }

    /// The known modifier bits; unknown bits are dropped.
    public var keyModifiers: KeyModifiers { KeyModifiers(rawValue: modifiers).intersection(.all) }

    public func encodedPayload() -> Data { Data([key, modifiers]) }

    /// Decodes a KEY payload. Shorter than 2 bytes is truncated; trailing bytes
    /// are ignored (forward compatibility, like TOUCH).
    public static func decode(_ payload: Data) throws -> KeyEvent {
        var r = ByteReader(payload)
        guard let key = r.u8(), let modifiers = r.u8() else { throw ProtocolError.truncated(.key) }
        return KeyEvent(key: key, modifiers: modifiers)
    }
}
