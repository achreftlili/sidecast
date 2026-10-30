import CoreGraphics
import XCTest
@testable import SidecastKit

/// PROTOCOL §7.1 keyboard input: TEXT / KEY decoding, macOS key mapping and
/// Unicode-event chunking.
final class KeyboardTests: XCTestCase {
    // MARK: - TEXT decoding

    func testTextRoundTripsMultilingualText() throws {
        for text in ["a", "héllo 👋", "日本語のテキスト", "Привет", "👨‍👩‍👧‍👦", "e\u{301}"] {
            XCTAssertEqual(try ClientMessage.decode(ClientMessage.text(text).frame()), .text(text))
        }
    }

    func testTextAtExactly4096BytesIsAccepted() throws {
        let text = String(repeating: "é", count: 2048)   // 2 bytes each
        XCTAssertEqual(text.utf8.count, 4096)
        XCTAssertEqual(try ClientMessage.decode(Frame(.text, payload: Data(text.utf8))), .text(text))
    }

    func testEmptyTextIsMalformed() {
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.text, payload: Data()))) { error in
            guard case .malformed(.text, _)? = error as? ProtocolError else { return XCTFail("\(error)") }
        }
    }

    func testTextOver4096BytesIsMalformed() {
        let payload = Data(repeating: 0x61, count: 4097)
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.text, payload: payload))) { error in
            guard case .malformed(.text, _)? = error as? ProtocolError else { return XCTFail("\(error)") }
        }
    }

    func testInvalidUTF8IsMalformed() {
        let cases: [[UInt8]] = [
            [0xC3],                    // truncated 2-byte sequence
            [0xFF],                    // never valid
            [0x61, 0x80, 0x62],        // stray continuation byte
            [0xC0, 0xAF],              // overlong "/"
            [0xED, 0xA0, 0x80],        // UTF-16 surrogate encoded in UTF-8
            [0xF4, 0x90, 0x80, 0x80],  // above U+10FFFF
        ]
        for bytes in cases {
            XCTAssertThrowsError(try ClientMessage.decode(Frame(.text, payload: Data(bytes))), "\(bytes)") { error in
                guard case .malformed(.text, _)? = error as? ProtocolError else { return XCTFail("\(error)") }
            }
        }
    }

    // MARK: - KEY decoding

    func testOneByteKeyIsTruncated() {
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.key, payload: Data([0x01])))) { error in
            XCTAssertEqual(error as? ProtocolError, .truncated(.key))
        }
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.key, payload: Data())))
    }

    func testKeyKeepsUnknownValuesAndIgnoresTrailingBytes() throws {
        let msg = try ClientMessage.decode(Frame(.key, payload: Data([0xC8, 0xFF, 0x00])))
        XCTAssertEqual(msg, .key(KeyEvent(key: 200, modifiers: 0xFF)))
        guard case .key(let event)? = msg else { return XCTFail() }
        XCTAssertNil(event.protocolKey)
        XCTAssertEqual(event.keyModifiers, .all)   // unknown bits 0xF0 dropped
    }

    func testKeyRoundTripsEveryKnownKey() throws {
        for key in ProtocolKey.allCases {
            let event = KeyEvent(key, modifiers: [.command, .option])
            XCTAssertEqual(try ClientMessage.decode(ClientMessage.key(event).frame()), .key(event))
        }
    }

    // MARK: - macOS mapping

    func testVirtualKeyCodes() {
        let expected: [ProtocolKey: CGKeyCode] = [
            .returnKey: 0x24, .backspace: 0x33, .forwardDelete: 0x75, .tab: 0x30, .escape: 0x35,
            .left: 0x7B, .right: 0x7C, .up: 0x7E, .down: 0x7D,
            .home: 0x73, .end: 0x77, .pageUp: 0x74, .pageDown: 0x79,
        ]
        XCTAssertEqual(expected.count, ProtocolKey.allCases.count)
        for (key, code) in expected {
            XCTAssertEqual(MacKeyMapping.virtualKey(for: key), code, "\(key)")
            XCTAssertEqual(MacKeyMapping.virtualKey(forProtocolKey: key.rawValue), code, "\(key)")
        }
        // Protocol numbering 1…13 (PROTOCOL §7.1 table).
        XCTAssertEqual(ProtocolKey.allCases.map(\.rawValue), Array(1...13))
    }

    func testUnknownKeysMapToNil() {
        for raw: UInt8 in [0, 14, 99, 255] {
            XCTAssertNil(MacKeyMapping.virtualKey(forProtocolKey: raw), "\(raw)")
            XCTAssertNil(MacKeyMapping.keyStroke(for: KeyEvent(key: raw, modifiers: 1)), "\(raw)")
        }
    }

    func testModifierFlags() {
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 0), [])
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 1), .maskShift)
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 2), .maskControl)
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 4), .maskAlternate)
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 8), .maskCommand)
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 15),
                       [.maskShift, .maskControl, .maskAlternate, .maskCommand])
        // Unknown bits are ignored.
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 0xF0), [])
        XCTAssertEqual(MacKeyMapping.eventFlags(forRawModifiers: 0x19), [.maskShift, .maskCommand])
    }

    func testKeyStrokes() {
        XCTAssertEqual(MacKeyMapping.keyStroke(for: KeyEvent(.returnKey)), KeyStroke(virtualKey: 0x24, flags: []))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: KeyEvent(.backspace, modifiers: .option)),
                       KeyStroke(virtualKey: 0x33, flags: .maskAlternate))
        // Arrows carry the fn + numeric-pad flags a hardware keyboard sets.
        XCTAssertEqual(MacKeyMapping.keyStroke(for: KeyEvent(.left, modifiers: .shift)),
                       KeyStroke(virtualKey: 0x7B, flags: [.maskShift, .maskSecondaryFn, .maskNumericPad]))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: KeyEvent(.end, modifiers: .command)),
                       KeyStroke(virtualKey: 0x77, flags: [.maskCommand, .maskSecondaryFn]))
        XCTAssertEqual(MacKeyMapping.keyStroke(for: KeyEvent(key: 4, modifiers: 0xF2)),   // Tab, Ctrl + junk bits
                       KeyStroke(virtualKey: 0x30, flags: .maskControl))
    }

    // MARK: - Chunking

    /// Every chunk fits, nothing is lost, and no grapheme is split.
    private func assertWellChunked(_ text: String, _ chunks: [String], limit: Int = 20,
                                   file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(chunks.joined(), text, file: file, line: line)
        XCTAssertEqual(chunks.map(\.count).reduce(0, +), text.count, "a grapheme was split", file: file, line: line)
        for chunk in chunks {
            XCTAssertFalse(chunk.isEmpty, file: file, line: line)
            XCTAssertLessThanOrEqual(chunk.utf16.count, limit, file: file, line: line)
        }
    }

    func testFiftyASCIICharactersMakeThreeChunks() {
        let text = String(repeating: "abcdefghij", count: 5)
        let chunks = UnicodeTyping.chunks(text)
        XCTAssertEqual(chunks.map(\.utf16.count), [20, 20, 10])
        assertWellChunked(text, chunks)
    }

    func testShortAndEmptyText() {
        XCTAssertEqual(UnicodeTyping.chunks("héllo 👋"), ["héllo 👋"])
        XCTAssertEqual(UnicodeTyping.chunks(""), [])
        let twenty = String(repeating: "x", count: 20)
        XCTAssertEqual(UnicodeTyping.chunks(twenty), [twenty])
    }

    func testSurrogatePairIsNeverSplit() {
        let text = String(repeating: "a", count: 19) + "👋" + "b"   // 👋 = 2 UTF-16 units at 19…20
        let chunks = UnicodeTyping.chunks(text)
        XCTAssertEqual(chunks, [String(repeating: "a", count: 19), "👋b"])
        assertWellChunked(text, chunks)

        let emoji = String(repeating: "😀", count: 25)   // 50 units
        let emojiChunks = UnicodeTyping.chunks(emoji)
        XCTAssertEqual(emojiChunks.map(\.count), [10, 10, 5])
        assertWellChunked(emoji, emojiChunks)
    }

    func testFamilyZWJSequenceIsNeverSplit() {
        let family = "👨‍👩‍👧‍👦"
        XCTAssertEqual(family.count, 1)
        XCTAssertEqual(family.utf16.count, 11)
        let text = String(repeating: "a", count: 15) + family + family + "z"
        let chunks = UnicodeTyping.chunks(text)
        XCTAssertEqual(chunks, [String(repeating: "a", count: 15), family, family + "z"])
        assertWellChunked(text, chunks)
    }

    func testDecomposedAccentIsNeverSplit() {
        let e = "e\u{301}"   // e + COMBINING ACUTE ACCENT, one grapheme of 2 units
        XCTAssertEqual(e.count, 1)
        let text = String(repeating: "x", count: 19) + e + String(repeating: e, count: 12)
        let chunks = UnicodeTyping.chunks(text)
        XCTAssertEqual(chunks.first, String(repeating: "x", count: 19))
        XCTAssertEqual(chunks[1].unicodeScalars.first, "e")
        assertWellChunked(text, chunks)
        for chunk in chunks { XCTAssertNotEqual(chunk.unicodeScalars.first, "\u{301}") }
    }

    func testMixedScriptsAndFlags() {
        let text = "Grüße 🇫🇷🇩🇪 日本語 🏴󠁧󠁢󠁳󠁣󠁴󠁿 👍🏽 مرحبا "
        assertWellChunked(text, UnicodeTyping.chunks(text))
        assertWellChunked(text, UnicodeTyping.chunks(text, maxUTF16: 14), limit: 14)
    }

    func testOversizedGraphemeIsSplitBetweenScalarsOnly() {
        // One grapheme of 31 units: "a" + 30 combining marks.
        let zalgo = "a" + String(repeating: "\u{301}", count: 30)
        XCTAssertEqual(zalgo.count, 1)
        let chunks = UnicodeTyping.chunks("b" + zalgo + "c")
        XCTAssertEqual(chunks.map(\.utf16.count), [1, 20, 11, 1])
        XCTAssertEqual(chunks.joined(), "b" + zalgo + "c")

        // A tag sequence of 13 non-BMP scalars (26 units): never cut inside a pair.
        let tags = "\u{1F3F4}" + String(repeating: "\u{E0061}", count: 11) + "\u{E007F}"
        XCTAssertEqual(tags.count, 1)
        let tagChunks = UnicodeTyping.chunks(tags)
        XCTAssertEqual(tagChunks.map(\.utf16.count), [20, 6])
        XCTAssertEqual(tagChunks.joined(), tags)

        // Odd limit: two 2-unit scalars fit in 5, a third does not.
        XCTAssertEqual(UnicodeTyping.chunks("👋👋👋", maxUTF16: 5), ["👋👋", "👋"])
    }

    func testControlCharactersAreDropped() {
        XCTAssertEqual(UnicodeTyping.removingControlCharacters("a\tb\nc\r\nd\u{7F}e\u{1B}"), "abcde")
        XCTAssertEqual(UnicodeTyping.removingControlCharacters("héllo 👋 👨‍👩‍👧‍👦"), "héllo 👋 👨‍👩‍👧‍👦")
    }
}
