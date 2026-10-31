import Foundation
import XCTest
@testable import SidecastKit

/// Client input (PROTOCOL §7): touch sequencing and mapping, keyboard, pointer overlay.
final class ClientInputTests: XCTestCase {
    private func t(_ key: String, _ x: Double, _ y: Double) -> TrackedTouch<String> { TrackedTouch(key: key, x: x, y: y) }

    private func ids(_ e: PointerEvent) -> [Int] { e.pointers.map(\.id) }

    // MARK: - TouchSequencer

    func testFingersLandAndLiftOneEventEachLikeAndroid() {
        let s = TouchSequencer<String>()
        // Two fingers in the same UIKit callback: DOWN then POINTER_DOWN.
        let down = s.began([t("a", 10, 10), t("b", 20, 20)])
        XCTAssertEqual(down.map(\.action), [.down, .pointerDown])
        XCTAssertEqual(down.map(\.actionIndex), [0, 1])
        XCTAssertEqual(down.map(ids), [[0], [0, 1]])

        let move = s.moved([t("b", 25, 26)])
        XCTAssertEqual(move, [PointerEvent(action: .move, actionIndex: 0,
                                           pointers: [PointerSample(id: 0, x: 10, y: 10), PointerSample(id: 1, x: 25, y: 26)])])
        // The first finger lifts: POINTER_UP still lists it, at its last position.
        let up0 = s.ended([t("a", 11, 12)])
        XCTAssertEqual(up0, [PointerEvent(action: .pointerUp, actionIndex: 0,
                                          pointers: [PointerSample(id: 0, x: 11, y: 12), PointerSample(id: 1, x: 25, y: 26)])])
        // A new finger takes the lowest free id (0), ahead of id 1 in index order.
        let again = s.began([t("c", 30, 30)])
        XCTAssertEqual(again.map(\.action), [.pointerDown])
        XCTAssertEqual(again.map(\.actionIndex), [0])
        XCTAssertEqual(again.map(ids), [[0, 1]])
        let last = s.ended([t("c", 30, 30), t("b", 25, 26)])
        XCTAssertEqual(last.map(\.action), [.pointerUp, .up])
        XCTAssertEqual(last.map(ids), [[0, 1], [1]])
        XCTAssertEqual(s.activeCount, 0)
        XCTAssertEqual(s.moved([t("b", 1, 1)]), [])   // unknown touches are ignored
    }

    func testCancelEndsTheWholeGestureAndGhostsAreForgotten() {
        let s = TouchSequencer<String>()
        _ = s.began([t("a", 1, 1), t("b", 2, 2)])
        XCTAssertEqual(s.cancelled().map(\.action), [.cancel])
        XCTAssertEqual(s.cancelled(), [])
        // A finger whose end was never reported is dropped when UIKit no longer lists it.
        _ = s.began([t("a", 1, 1)])
        let next = s.began([t("z", 5, 5)], stillDown: ["z"])
        XCTAssertEqual(next.map(\.action), [.down])
        XCTAssertEqual(next.map(ids), [[0]])
    }

    // MARK: - PointerFilter (ported from the Android PointerFilterTest)

    private func ev(_ action: TouchAction, _ index: Int, _ pointers: [(Int, Double)]) -> PointerEvent {
        PointerEvent(action: action, actionIndex: index, pointers: pointers.map { PointerSample(id: $0.0, x: $0.1, y: 50) })
    }

    func testEdgeThumbIsHeldBackAndTheGestureStaysWellFormed() {
        // Extra pointers within 10 of the left edge are a gripping thumb.
        let f = PointerFilter { x, _, first in !first && x < 10 }
        XCTAssertEqual(f.filter(ev(.down, 0, [(0, 50)])), ev(.down, 0, [(0, 50)]))
        XCTAssertNil(f.filter(ev(.pointerDown, 1, [(0, 50), (1, 5)])))
        XCTAssertEqual(f.heldBack, [1])
        XCTAssertEqual(f.filter(ev(.move, 0, [(0, 60), (1, 5)])), ev(.move, 0, [(0, 60)]))
        XCTAssertNil(f.filter(ev(.pointerUp, 1, [(0, 60), (1, 5)])))
        XCTAssertEqual(f.filter(ev(.up, 0, [(0, 60)])), ev(.up, 0, [(0, 60)]))
    }

    func testAHeldFirstPointerPromotesTheNextOneToDown() {
        let f = PointerFilter { x, _, _ in x > 900 }   // a control at the right
        XCTAssertNil(f.filter(ev(.down, 0, [(0, 950)])))
        XCTAssertEqual(f.filter(ev(.pointerDown, 1, [(0, 950), (1, 100)])), ev(.down, 0, [(1, 100)]))
        XCTAssertNil(f.filter(ev(.pointerUp, 0, [(0, 950), (1, 100)])))
        XCTAssertEqual(f.filter(ev(.up, 0, [(1, 120)])), ev(.up, 0, [(1, 120)]))
        XCTAssertNil(f.filter(ev(.cancel, 0, [(0, 950)])))
    }

    // MARK: - Letterbox and TOUCH

    func testLetterboxFitsAndMapsBothWays() {
        // 16:9 video in a wider container: pillarbox.
        let r = Letterbox.fit(containerWidth: 2000, containerHeight: 900, videoWidth: 1600, videoHeight: 900)
        XCTAssertEqual(r, VideoRect(x: 200, y: 0, width: 1600, height: 900))
        XCTAssertEqual(r.normalizeX(200), 0)
        XCTAssertEqual(r.normalizeX(1800), 1)
        XCTAssertEqual(r.normalizeX(100), -0.0625)   // in the bar: beyond 0…1, the server clamps
        XCTAssertEqual(r.toContainerX(0.5), 1000)
        // Taller container: letterbox, snapped to pixels at 3x.
        let l = Letterbox.fit(containerWidth: 874, containerHeight: 402, videoWidth: 2622, videoHeight: 1000, scale: 3)
        XCTAssertEqual(l.width, 874)
        XCTAssertEqual(l.height * 3, (l.height * 3).rounded())
        XCTAssertEqual(l.y, ((402 - l.height) / 2 * 3).rounded(.down) / 3)
        XCTAssertEqual(Letterbox.fit(containerWidth: 100, containerHeight: 50, videoWidth: 0, videoHeight: 0),
                       VideoRect(x: 0, y: 0, width: 100, height: 50))
        XCTAssertEqual(Letterbox.fit(containerWidth: 0, containerHeight: 50, videoWidth: 10, videoHeight: 10), .zero)
    }

    func testTouchTranslationMatchesTheProtocolVector() {
        // PROTOCOL §8.3: eventTimeMs 1000, MOVE, pointer 0 at (0.5, 0.25).
        let rect = VideoRect(x: 100, y: 0, width: 800, height: 400)
        let event = PointerEvent(action: .move, actionIndex: 0, pointers: [PointerSample(id: 0, x: 500, y: 100)])
        let touch = TouchTranslator.translate(event, eventTimeMs: 1000, rect: rect)
        XCTAssertEqual(ClientMessage.touch(touch!).encoded().hexString, "1000000010000003e8020001003f0000003e800000")
    }

    func testTouchTranslationEdgeCases() {
        let rect = VideoRect(x: 0, y: 0, width: 100, height: 100)
        let many = (0..<12).map { PointerSample(id: $0, x: Double($0), y: 0) }
        let move = TouchTranslator.translate(PointerEvent(action: .move, actionIndex: 0, pointers: many), eventTimeMs: 5, rect: rect)
        XCTAssertEqual(move?.pointers.count, 10)
        XCTAssertNil(TouchTranslator.translate(PointerEvent(action: .pointerDown, actionIndex: 11, pointers: many),
                                               eventTimeMs: 5, rect: rect))
        let up = TouchTranslator.translate(PointerEvent(action: .pointerUp, actionIndex: 1, pointers: Array(many.prefix(2))),
                                           eventTimeMs: 5, rect: rect)
        XCTAssertEqual(up?.actionPointerId, 1)
        XCTAssertEqual(up?.touchAction, .pointerUp)
        XCTAssertNil(TouchTranslator.translate(PointerEvent(action: .move, actionIndex: 0, pointers: []), eventTimeMs: 0, rect: rect))
        XCTAssertEqual(TouchTranslator.eventTimeMs(uptime: 4_294_967.296 + 1), 1000)   // wraps
    }

    func testThreeFingerTap() {
        let d = ThreeFingerTapDetector(slop: 10)
        func p(_ n: Int, dx: Double = 0) -> [PointerSample] { (0..<n).map { PointerSample(id: $0, x: Double($0 * 100) + dx, y: 0) } }
        XCTAssertFalse(d.onEvent(PointerEvent(action: .down, actionIndex: 0, pointers: p(1)), time: 0))
        XCTAssertFalse(d.onEvent(PointerEvent(action: .pointerDown, actionIndex: 1, pointers: p(2)), time: 0.01))
        XCTAssertFalse(d.onEvent(PointerEvent(action: .pointerDown, actionIndex: 2, pointers: p(3)), time: 0.02))
        XCTAssertFalse(d.onEvent(PointerEvent(action: .move, actionIndex: 0, pointers: p(3, dx: 5)), time: 0.05))
        XCTAssertFalse(d.onEvent(PointerEvent(action: .pointerUp, actionIndex: 2, pointers: p(3)), time: 0.1))
        XCTAssertTrue(d.onEvent(PointerEvent(action: .up, actionIndex: 0, pointers: p(1)), time: 0.15))
        // Moved too far, or two fingers only: no tap.
        _ = d.onEvent(PointerEvent(action: .down, actionIndex: 0, pointers: p(3)), time: 1)
        _ = d.onEvent(PointerEvent(action: .move, actionIndex: 0, pointers: p(3, dx: 50)), time: 1.1)
        XCTAssertFalse(d.onEvent(PointerEvent(action: .up, actionIndex: 0, pointers: p(1)), time: 1.2))
        _ = d.onEvent(PointerEvent(action: .down, actionIndex: 0, pointers: p(2)), time: 2)
        XCTAssertFalse(d.onEvent(PointerEvent(action: .up, actionIndex: 0, pointers: p(1)), time: 2.1))
        // Too slow.
        _ = d.onEvent(PointerEvent(action: .down, actionIndex: 0, pointers: p(3)), time: 3)
        XCTAssertFalse(d.onEvent(PointerEvent(action: .up, actionIndex: 0, pointers: p(1)), time: 3.5))
    }

    func testPointerPlacement() {
        let rect = VideoRect(x: 100, y: 0, width: 800, height: 400)
        let c = CursorPosition(x: 0.5, y: 0.25)
        XCTAssertEqual(PointerPlacement.position(cursor: c, mode: .trackpad, playing: true, rect: rect), CGPoint(x: 500, y: 100))
        XCTAssertNil(PointerPlacement.position(cursor: c, mode: .touch, playing: true, rect: rect))
        XCTAssertNil(PointerPlacement.position(cursor: c, mode: .trackpad, playing: false, rect: rect))
        XCTAssertNil(PointerPlacement.position(cursor: .offDisplay, mode: .trackpad, playing: true, rect: rect))
        XCTAssertNil(PointerPlacement.position(cursor: nil, mode: .trackpad, playing: true, rect: rect))
        XCTAssertNil(PointerPlacement.position(cursor: c, mode: .trackpad, playing: true, rect: .zero))
    }

    // MARK: - Keyboard

    func testTypingAppendsText() {
        XCTAssertEqual(TextFieldDiff.events(from: "", to: "H"), [.text("H")])
        XCTAssertEqual(TextFieldDiff.events(from: "Hel", to: "Hello"), [.text("lo")])
        XCTAssertEqual(TextFieldDiff.events(from: "Hello", to: "Hello"), [])
    }

    func testAutocorrectionAndDictationRevisionsBecomeBackspacesThenText() {
        XCTAssertEqual(TextFieldDiff.events(from: "Hello wrld", to: "Hello world "),
                       [.press(.backspace), .press(.backspace), .press(.backspace), .text("orld ")])
        XCTAssertEqual(TextFieldDiff.events(from: "abc", to: "ab"), [.press(.backspace)])
        XCTAssertEqual(TextFieldDiff.events(from: "abc", to: ""), [.press(.backspace), .press(.backspace), .press(.backspace)])
    }

    func testGraphemeClustersAreOneBackspaceAndTextIsNFC() {
        XCTAssertEqual(TextFieldDiff.events(from: "a👨‍👩‍👧", to: "a"), [.press(.backspace)])
        XCTAssertEqual(TextFieldDiff.events(from: "a🇫🇷", to: "a"), [.press(.backspace)])
        XCTAssertEqual(TextFieldDiff.events(from: "", to: "e\u{301}"), [.text("\u{e9}")])
        // Canonically equal: nothing to retype.
        XCTAssertEqual(TextFieldDiff.events(from: "\u{e9}", to: "e\u{301}"), [])
    }

    func testControlCharactersBecomeKeysOrAreDropped() {
        XCTAssertEqual(TextFieldDiff.events(from: "", to: "a\nb\tc\u{7}d\r\n"),
                       [.text("a"), .press(.returnKey), .text("b"), .press(.tab), .text("cd"), .press(.returnKey)])
        XCTAssertEqual(TextFieldDiff.events(from: "a\u{7}", to: "a"), [])
    }

    func testHardwareKeys() {
        XCTAssertEqual(HardwareKeyboard.translate(hidUsage: 0x50, modifierFlags: 1 << 17, characters: "UIKeyInputLeftArrow"),
                       .press(.left, .shift))
        XCTAssertEqual(HardwareKeyboard.translate(hidUsage: 0x28, modifierFlags: 1 << 20, characters: "\r"),
                       .press(.returnKey, .command))
        XCTAssertEqual(HardwareKeyboard.translate(hidUsage: 0x29, modifierFlags: 0, characters: "UIKeyInputEscape"), .press(.escape))
        XCTAssertEqual(HardwareKeyboard.translate(hidUsage: 0x04, modifierFlags: 0, characters: "a"), .text("a"))
        XCTAssertEqual(HardwareKeyboard.translate(hidUsage: 0x04, modifierFlags: 1 << 19, characters: "å"), .text("å"))
        XCTAssertNil(HardwareKeyboard.translate(hidUsage: 0x04, modifierFlags: 1 << 20, characters: "a"))   // Cmd+A
        XCTAssertNil(HardwareKeyboard.translate(hidUsage: 0x04, modifierFlags: 1 << 18, characters: "a"))   // Ctrl+A
        XCTAssertNil(HardwareKeyboard.translate(hidUsage: 0xE1, modifierFlags: 1 << 17, characters: ""))     // Shift alone
        XCTAssertNil(HardwareKeyboard.translate(hidUsage: 0x3A, modifierFlags: 0, characters: "UIKeyInputF1"))
        XCTAssertNil(HardwareKeyboard.translate(hidUsage: 0x3A, modifierFlags: 0, characters: "\u{F704}"))
        XCTAssertEqual(HardwareKeyboard.modifiers(flags: (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 16)), .all)
    }

    func testTextIsSplitIntoProtocolSizedChunksAtCharacterBoundaries() {
        XCTAssertEqual(TextMessage.split(""), [])
        XCTAssertEqual(TextMessage.split("hi"), ["hi"])
        let emoji = String(repeating: "👍", count: 1_025)   // 4100 bytes
        let chunks = TextMessage.split(emoji)
        XCTAssertEqual(chunks.map { $0.utf8.count }, [4096, 4])
        XCTAssertEqual(chunks.joined(), emoji)
        XCTAssertEqual(TextMessage.split("abcdef", maxBytes: 4), ["abcd", "ef"])
        // A single cluster above the limit is cut between scalars.
        let long = "e" + String(repeating: "\u{301}", count: 5)   // 11 bytes, one cluster
        XCTAssertEqual(TextMessage.split(long, maxBytes: 4).map { $0.unicodeScalars.count }, [2, 2, 2])
    }
}
