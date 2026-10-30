import CoreGraphics
import Foundation

/// A key press to inject: macOS virtual key code + the flags held with it.
public struct KeyStroke: Equatable, Sendable {
    public var virtualKey: CGKeyCode
    public var flags: CGEventFlags

    public init(virtualKey: CGKeyCode, flags: CGEventFlags) {
        self.virtualKey = virtualKey
        self.flags = flags
    }
}

/// PROTOCOL §7.1 KEY → macOS virtual key codes (Carbon `kVK_*`) and CGEventFlags.
public enum MacKeyMapping {
    public static func virtualKey(for key: ProtocolKey) -> CGKeyCode {
        switch key {
        case .returnKey: return 0x24      // kVK_Return
        case .backspace: return 0x33      // kVK_Delete
        case .forwardDelete: return 0x75  // kVK_ForwardDelete
        case .tab: return 0x30            // kVK_Tab
        case .escape: return 0x35         // kVK_Escape
        case .left: return 0x7B           // kVK_LeftArrow
        case .right: return 0x7C          // kVK_RightArrow
        case .up: return 0x7E             // kVK_UpArrow
        case .down: return 0x7D           // kVK_DownArrow
        case .home: return 0x73           // kVK_Home
        case .end: return 0x77            // kVK_End
        case .pageUp: return 0x74         // kVK_PageUp
        case .pageDown: return 0x79       // kVK_PageDown
        }
    }

    /// Virtual key for a raw `key` byte; nil for keys this version does not know (ignored).
    public static func virtualKey(forProtocolKey raw: UInt8) -> CGKeyCode? {
        ProtocolKey(rawValue: raw).map(virtualKey(for:))
    }

    /// Modifier bitmask → CGEventFlags. Unknown bits are ignored.
    public static func eventFlags(for modifiers: KeyModifiers) -> CGEventFlags {
        var flags: CGEventFlags = []
        if modifiers.contains(.shift) { flags.insert(.maskShift) }
        if modifiers.contains(.control) { flags.insert(.maskControl) }
        if modifiers.contains(.option) { flags.insert(.maskAlternate) }
        if modifiers.contains(.command) { flags.insert(.maskCommand) }
        return flags
    }

    public static func eventFlags(forRawModifiers raw: UInt8) -> CGEventFlags {
        eventFlags(for: KeyModifiers(rawValue: raw).intersection(.all))
    }

    /// Flags a hardware keyboard always sets for this key: navigation keys are
    /// "function" keys, arrows also count as numeric pad (NSEvent `.function` /
    /// `.numericPad`). Mirroring them makes injected keys look like real ones.
    public static func intrinsicFlags(for key: ProtocolKey) -> CGEventFlags {
        switch key {
        case .left, .right, .up, .down: return [.maskSecondaryFn, .maskNumericPad]
        case .forwardDelete, .home, .end, .pageUp, .pageDown: return .maskSecondaryFn
        case .returnKey, .backspace, .tab, .escape: return []
        }
    }

    /// Control + arrow for a swipe navigation, flagged like a hardware arrow key
    /// (Control + fn + numeric pad), which the Mission Control shortcuts need.
    public static func keyStroke(for navigation: NavigationAction) -> KeyStroke {
        // Every navigation maps to a known arrow key.
        keyStroke(for: navigation.keyEvent) ?? KeyStroke(virtualKey: 0, flags: [])
    }

    /// The stroke to inject for a KEY message, or nil when the key is unknown.
    public static func keyStroke(for event: KeyEvent) -> KeyStroke? {
        guard let key = event.protocolKey else { return nil }
        return KeyStroke(virtualKey: virtualKey(for: key),
                         flags: eventFlags(for: event.keyModifiers).union(intrinsicFlags(for: key)))
    }
}

/// Splits TEXT into pieces small enough for one Unicode keyboard event each.
public enum UnicodeTyping {
    /// `CGEventKeyboardSetUnicodeString` carries at most 20 UTF-16 units per event.
    public static let maxUTF16PerEvent = 20

    /// Chunks of ≤ `maxUTF16` UTF-16 units, cut only between grapheme clusters
    /// (so surrogate pairs, combining sequences and ZWJ emoji stay whole).
    /// A single grapheme longer than the limit is cut between Unicode scalars
    /// as a last resort — still never inside a surrogate pair; the text system
    /// recombines the pieces as they are inserted one after another.
    public static func chunks(_ text: String, maxUTF16: Int = maxUTF16PerEvent) -> [String] {
        let limit = max(2, maxUTF16)   // a non-BMP scalar needs 2 units
        var out: [String] = []
        var current = ""
        var currentUnits = 0
        func flush() {
            guard !current.isEmpty else { return }
            out.append(current)
            current = ""
            currentUnits = 0
        }
        for character in text {
            let units = character.utf16.count
            if units > limit {
                flush()
                var piece = String.UnicodeScalarView()
                var pieceUnits = 0
                for scalar in character.unicodeScalars {
                    let width = UTF16.width(scalar)
                    if pieceUnits + width > limit {
                        out.append(String(piece))
                        piece = String.UnicodeScalarView()
                        pieceUnits = 0
                    }
                    piece.append(scalar)
                    pieceUnits += width
                }
                if !piece.isEmpty { out.append(String(piece)) }
                continue
            }
            if currentUnits + units > limit { flush() }
            current.append(character)
            currentUnits += units
        }
        flush()
        return out
    }

    /// Drops control characters (general category Cc). The client sends those
    /// as KEY; typing them as Unicode would act unpredictably across apps.
    public static func removingControlCharacters(_ text: String) -> String {
        String(text.filter { character in
            !character.unicodeScalars.contains { $0.properties.generalCategory == .control }
        })
    }
}
