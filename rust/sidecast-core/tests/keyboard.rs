//! Port of `KeyboardTests.swift` (PROTOCOL §7.1): TEXT / KEY decoding, the
//! platform-neutral key mapping (replacing the macOS key-code tests) and
//! Unicode chunking.

use sidecast_core::input::{Key, KeyStroke, typing};
use sidecast_core::protocol::{ClientMessage, Frame, KeyEvent, KeyModifiers, MessageType, ProtocolError, ProtocolKey};
use unicode_segmentation::UnicodeSegmentation;

fn client(msg_type: MessageType, payload: &[u8]) -> Result<Option<ClientMessage>, ProtocolError> {
    ClientMessage::decode(&Frame::new(msg_type, payload.to_vec()))
}

fn assert_text_malformed(payload: &[u8]) {
    match client(MessageType::Text, payload) {
        Err(ProtocolError::Malformed(MessageType::Text, _)) => {}
        other => panic!("{payload:?}: expected malformed TEXT, got {other:?}"),
    }
}

fn graphemes(text: &str) -> usize {
    text.graphemes(true).count()
}

fn utf16(text: &str) -> usize {
    text.encode_utf16().count()
}

// MARK: - TEXT decoding

#[test]
fn text_round_trips_multilingual_text() {
    for text in ["a", "héllo 👋", "日本語のテキスト", "Привет", "👨‍👩‍👧‍👦", "e\u{301}"]
    {
        let message = ClientMessage::Text(text.to_owned());
        assert_eq!(ClientMessage::decode(&message.frame()), Ok(Some(message)));
    }
}

#[test]
fn text_at_exactly_4096_bytes_is_accepted() {
    let text = "é".repeat(2048); // 2 bytes each
    assert_eq!(text.len(), 4096);
    assert_eq!(client(MessageType::Text, text.as_bytes()), Ok(Some(ClientMessage::Text(text))));
}

#[test]
fn empty_text_is_malformed() {
    assert_text_malformed(&[]);
}

#[test]
fn text_over_4096_bytes_is_malformed() {
    assert_text_malformed(&[0x61; 4097]);
}

#[test]
fn invalid_utf8_is_malformed() {
    let cases: [&[u8]; 6] = [
        &[0xC3],                   // truncated 2-byte sequence
        &[0xFF],                   // never valid
        &[0x61, 0x80, 0x62],       // stray continuation byte
        &[0xC0, 0xAF],             // overlong "/"
        &[0xED, 0xA0, 0x80],       // UTF-16 surrogate encoded in UTF-8
        &[0xF4, 0x90, 0x80, 0x80], // above U+10FFFF
    ];
    for bytes in cases {
        assert_text_malformed(bytes);
    }
}

// MARK: - KEY decoding

#[test]
fn one_byte_key_is_truncated() {
    assert_eq!(client(MessageType::Key, &[0x01]), Err(ProtocolError::Truncated(MessageType::Key)));
    assert!(client(MessageType::Key, &[]).is_err());
}

#[test]
fn key_keeps_unknown_values_and_ignores_trailing_bytes() {
    let msg = client(MessageType::Key, &[0xC8, 0xFF, 0x00]).unwrap();
    assert_eq!(msg, Some(ClientMessage::Key(KeyEvent::raw(200, 0xFF))));
    let Some(ClientMessage::Key(event)) = msg else { unreachable!() };
    assert_eq!(event.protocol_key(), None);
    assert_eq!(event.key_modifiers(), KeyModifiers::ALL); // unknown bits 0xF0 dropped
}

#[test]
fn key_round_trips_every_known_key() {
    for key in ProtocolKey::ALL {
        let message = ClientMessage::Key(KeyEvent::new(key, KeyModifiers::META | KeyModifiers::ALT));
        assert_eq!(ClientMessage::decode(&message.frame()), Ok(Some(message)));
    }
}

// MARK: - Neutral key mapping (the Mac maps these to kVK_* codes)

#[test]
fn protocol_key_codes_map_to_neutral_keys_in_table_order() {
    // Protocol numbering 1…13 (PROTOCOL §7.1 table).
    assert_eq!(ProtocolKey::ALL.map(|k| k as u8), core::array::from_fn::<u8, 13, _>(|i| i as u8 + 1));
    let mapped: Vec<Key> = (1..=13).map(|raw| Key::from_wire(raw).unwrap()).collect();
    assert_eq!(mapped, Key::ALL);
    assert_eq!(Key::from_protocol(ProtocolKey::Return), Key::Enter);
    assert_eq!(Key::from_protocol(ProtocolKey::ForwardDelete), Key::Delete);
}

#[test]
fn unknown_keys_map_to_none() {
    for raw in [0, 14, 99, 255] {
        assert_eq!(Key::from_wire(raw), None, "{raw}");
        assert_eq!(KeyStroke::from_event(&KeyEvent::raw(raw, 1)), None, "{raw}");
    }
}

#[test]
fn modifier_bits() {
    assert_eq!(KeyModifiers::from_bits(0), KeyModifiers::NONE);
    assert_eq!(KeyModifiers::from_bits(1), KeyModifiers::SHIFT);
    assert_eq!(KeyModifiers::from_bits(2), KeyModifiers::CONTROL);
    assert_eq!(KeyModifiers::from_bits(4), KeyModifiers::ALT);
    assert_eq!(KeyModifiers::from_bits(8), KeyModifiers::META);
    assert_eq!(
        KeyModifiers::from_bits(15),
        KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::META
    );
    // Unknown bits are ignored.
    assert_eq!(KeyModifiers::from_bits(0xF0), KeyModifiers::NONE);
    assert_eq!(KeyModifiers::from_bits(0x19), KeyModifiers::SHIFT | KeyModifiers::META);
    assert!(KeyModifiers::ALL.contains(KeyModifiers::CONTROL));
    assert!(KeyModifiers::NONE.is_empty());
}

#[test]
fn key_strokes() {
    let stroke = |key, modifiers| KeyStroke { key, modifiers };
    assert_eq!(
        KeyStroke::from_event(&KeyEvent::new(ProtocolKey::Return, KeyModifiers::NONE)),
        Some(stroke(Key::Enter, KeyModifiers::NONE))
    );
    assert_eq!(
        KeyStroke::from_event(&KeyEvent::new(ProtocolKey::Backspace, KeyModifiers::ALT)),
        Some(stroke(Key::Backspace, KeyModifiers::ALT))
    );
    assert_eq!(
        KeyStroke::from_event(&KeyEvent::new(ProtocolKey::Left, KeyModifiers::SHIFT)),
        Some(stroke(Key::Left, KeyModifiers::SHIFT))
    );
    assert_eq!(
        KeyStroke::from_event(&KeyEvent::new(ProtocolKey::End, KeyModifiers::META)),
        Some(stroke(Key::End, KeyModifiers::META))
    );
    // Tab, Ctrl + junk bits.
    assert_eq!(KeyStroke::from_event(&KeyEvent::raw(4, 0xF2)), Some(stroke(Key::Tab, KeyModifiers::CONTROL)));
}

/// Same sets as the Mac's `intrinsicFlags`: arrows are fn + numeric pad,
/// forward delete / Home / End / Page Up / Page Down are fn only.
#[test]
fn navigation_keys_match_hardware_flags() {
    let arrows = [Key::Left, Key::Right, Key::Up, Key::Down];
    let navigation = [Key::Delete, Key::Home, Key::End, Key::PageUp, Key::PageDown];
    for key in Key::ALL {
        assert_eq!(key.is_arrow(), arrows.contains(&key), "{key:?}");
        assert_eq!(key.is_navigation(), arrows.contains(&key) || navigation.contains(&key), "{key:?}");
    }
    for key in [Key::Enter, Key::Backspace, Key::Tab, Key::Escape] {
        assert!(!key.is_navigation(), "{key:?}");
    }
}

// MARK: - Chunking

/// Every chunk fits, nothing is lost, and no grapheme is split.
fn assert_well_chunked(text: &str, chunks: &[String], limit: usize) {
    assert_eq!(chunks.concat(), text);
    assert_eq!(chunks.iter().map(|c| graphemes(c)).sum::<usize>(), graphemes(text), "a grapheme was split");
    for chunk in chunks {
        assert!(!chunk.is_empty());
        assert!(utf16(chunk) <= limit, "{chunk:?} has {} UTF-16 units", utf16(chunk));
    }
}

fn chunks(text: &str) -> Vec<String> {
    typing::chunks(text, typing::MAX_UTF16_PER_EVENT)
}

#[test]
fn fifty_ascii_characters_make_three_chunks() {
    let text = "abcdefghij".repeat(5);
    let c = chunks(&text);
    assert_eq!(c.iter().map(|c| utf16(c)).collect::<Vec<_>>(), [20, 20, 10]);
    assert_well_chunked(&text, &c, 20);
}

#[test]
fn short_and_empty_text() {
    assert_eq!(chunks("héllo 👋"), ["héllo 👋"]);
    assert!(chunks("").is_empty());
    let twenty = "x".repeat(20);
    assert_eq!(chunks(&twenty), std::slice::from_ref(&twenty));
}

#[test]
fn surrogate_pair_is_never_split() {
    let text = "a".repeat(19) + "👋" + "b"; // 👋 = 2 UTF-16 units at 19…20
    let c = chunks(&text);
    assert_eq!(c, ["a".repeat(19), "👋b".to_owned()]);
    assert_well_chunked(&text, &c, 20);

    let emoji = "😀".repeat(25); // 50 units
    let emoji_chunks = chunks(&emoji);
    assert_eq!(emoji_chunks.iter().map(|c| graphemes(c)).collect::<Vec<_>>(), [10, 10, 5]);
    assert_well_chunked(&emoji, &emoji_chunks, 20);
}

#[test]
fn family_zwj_sequence_is_never_split() {
    let family = "👨‍👩‍👧‍👦";
    assert_eq!(graphemes(family), 1);
    assert_eq!(utf16(family), 11);
    let text = "a".repeat(15) + family + family + "z";
    let c = chunks(&text);
    assert_eq!(c, ["a".repeat(15), family.to_owned(), format!("{family}z")]);
    assert_well_chunked(&text, &c, 20);
}

#[test]
fn decomposed_accent_is_never_split() {
    let e = "e\u{301}"; // e + COMBINING ACUTE ACCENT, one grapheme of 2 units
    assert_eq!(graphemes(e), 1);
    let text = "x".repeat(19) + e + &e.repeat(12);
    let c = chunks(&text);
    assert_eq!(c[0], "x".repeat(19));
    assert_eq!(c[1].chars().next(), Some('e'));
    assert_well_chunked(&text, &c, 20);
    for chunk in &c {
        assert_ne!(chunk.chars().next(), Some('\u{301}'));
    }
}

#[test]
fn mixed_scripts_and_flags() {
    let text = "Grüße 🇫🇷🇩🇪 日本語 🏴󠁧󠁢󠁳󠁣󠁴󠁿 👍🏽 مرحبا ";
    assert_well_chunked(text, &chunks(text), 20);
    assert_well_chunked(text, &typing::chunks(text, 14), 14);
}

#[test]
fn oversized_grapheme_is_split_between_scalars_only() {
    // One grapheme of 31 units: "a" + 30 combining marks.
    let zalgo = "a".to_owned() + &"\u{301}".repeat(30);
    assert_eq!(graphemes(&zalgo), 1);
    let text = format!("b{zalgo}c");
    let c = chunks(&text);
    assert_eq!(c.iter().map(|c| utf16(c)).collect::<Vec<_>>(), [1, 20, 11, 1]);
    assert_eq!(c.concat(), text);

    // A tag sequence of 13 non-BMP scalars (26 units): never cut inside a pair.
    let tags = "\u{1F3F4}".to_owned() + &"\u{E0061}".repeat(11) + "\u{E007F}";
    assert_eq!(graphemes(&tags), 1);
    let tag_chunks = chunks(&tags);
    assert_eq!(tag_chunks.iter().map(|c| utf16(c)).collect::<Vec<_>>(), [20, 6]);
    assert_eq!(tag_chunks.concat(), tags);

    // Odd limit: two 2-unit scalars fit in 5, a third does not.
    assert_eq!(typing::chunks("👋👋👋", 5), ["👋👋", "👋"]);
}

#[test]
fn control_characters_are_dropped() {
    assert_eq!(typing::remove_control_characters("a\tb\nc\r\nd\u{7F}e\u{1B}"), "abcde");
    assert_eq!(typing::remove_control_characters("héllo 👋 👨‍👩‍👧‍👦"), "héllo 👋 👨‍👩‍👧‍👦");
}
