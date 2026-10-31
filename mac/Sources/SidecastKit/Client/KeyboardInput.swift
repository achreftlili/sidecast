import Foundation

/// What the device's keyboard does on the computer (PROTOCOL §7.1): sent as TEXT / KEY.
public enum KeyboardEvent: Equatable, Sendable {
    /// Text to type; never contains control characters (those are keys).
    case text(String)
    case key(KeyEvent)

    public static func press(_ key: ProtocolKey, _ modifiers: KeyModifiers = []) -> KeyboardEvent {
        .key(KeyEvent(key, modifiers: modifiers))
    }
}

/// Keyboard input through a text field that mirrors what was typed on the
/// computer since the context was last reset. The system keyboard edits the
/// field freely (autocorrection, predictions, dictation revising its words,
/// marked text of input methods); every change is turned into keystrokes: one
/// Backspace per character after the common prefix, then the new text, so the
/// computer always holds what the field shows. Pure.
public enum TextFieldDiff {
    /// Keystrokes that turn `old` (already typed on the computer, just before
    /// its cursor) into `new`. Characters are grapheme clusters, so an emoji is
    /// one Backspace. "\n", "\r", "\r\n" become Return and "\t" Tab; other
    /// control characters are neither typed nor deleted. Text is sent NFC.
    public static func events(from old: String, to new: String) -> [KeyboardEvent] {
        let a = Array(old), b = Array(new)
        var same = 0
        while same < a.count, same < b.count, a[same] == b[same] { same += 1 }
        var events: [KeyboardEvent] = []
        for c in a[same...] where output(of: c) != nil {
            events.append(.press(.backspace))
        }
        var run = ""
        for c in b[same...] {
            switch output(of: c) {
            case .text(let t)?:
                run += t
            case .key(let k)?:
                if !run.isEmpty { events.append(.text(run)) }
                run = ""
                events.append(.key(k))
            case nil:
                continue
            }
        }
        if !run.isEmpty { events.append(.text(run)) }
        return events
    }

    /// What one character becomes on the computer, nil for a dropped control character.
    static func output(of c: Character) -> KeyboardEvent? {
        switch c {
        case "\n", "\r", "\r\n": return .press(.returnKey)
        case "\t": return .press(.tab)
        default:
            if c.unicodeScalars.contains(where: { $0.properties.generalCategory == .control }) { return nil }
            return .text(String(c).precomposedStringWithCanonicalMapping)
        }
    }
}

/// A hardware keyboard attached to the device (UIKit `UIKey`) → KEY / TEXT.
/// Takes the raw values (`UIKeyboardHIDUsage`, `UIKeyModifierFlags`) so it is
/// unit tested without UIKit. Pure.
public enum HardwareKeyboard {
    /// USB HID keyboard usage → protocol key; nil when the protocol has no such key.
    public static func protocolKey(hidUsage: Int) -> ProtocolKey? {
        switch hidUsage {
        case 0x28, 0x58: return .returnKey     // Return, keypad Enter
        case 0x2A: return .backspace           // Delete (Backspace)
        case 0x4C: return .forwardDelete
        case 0x2B: return .tab
        case 0x29: return .escape
        case 0x50: return .left
        case 0x4F: return .right
        case 0x52: return .up
        case 0x51: return .down
        case 0x4A: return .home
        case 0x4D: return .end
        case 0x4B: return .pageUp
        case 0x4E: return .pageDown
        default: return nil
        }
    }

    // UIKeyModifierFlags bits.
    static let shiftFlag = 1 << 17
    static let controlFlag = 1 << 18
    static let alternateFlag = 1 << 19
    static let commandFlag = 1 << 20

    /// UIKeyModifierFlags → KEY modifiers (Command = Command, Option = Option).
    public static func modifiers(flags: Int) -> KeyModifiers {
        var m: KeyModifiers = []
        if flags & shiftFlag != 0 { m.insert(.shift) }
        if flags & controlFlag != 0 { m.insert(.control) }
        if flags & alternateFlag != 0 { m.insert(.option) }
        if flags & commandFlag != 0 { m.insert(.command) }
        return m
    }

    /// A key press. `characters`: what the key types (`UIKey.characters`). Nil for
    /// keys Sidecast leaves alone: modifier keys alone, Control/Command shortcuts
    /// on printable keys (the protocol has no letter keys), and keys that type nothing.
    public static func translate(hidUsage: Int, modifierFlags: Int, characters: String) -> KeyboardEvent? {
        if let key = protocolKey(hidUsage: hidUsage) { return .press(key, modifiers(flags: modifierFlags)) }
        if modifierFlags & (controlFlag | commandFlag) != 0 { return nil }
        // UIKit names special keys ("UIKeyInputF1") and maps function keys to the
        // private-use block U+F700–U+F8FF: neither is text.
        guard !characters.isEmpty, !characters.hasPrefix("UIKeyInput"),
              !characters.unicodeScalars.contains(where: {
                  $0.properties.generalCategory == .control || (0xF700...0xF8FF).contains($0.value)
              }) else { return nil }
        return .text(characters.precomposedStringWithCanonicalMapping)
    }
}
