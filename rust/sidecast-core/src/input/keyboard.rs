//! KEY codes → platform-neutral keys, and TEXT preparation (PROTOCOL §7.1).

use crate::protocol::{KeyEvent, KeyModifiers, ProtocolKey};

/// A special key, independent of any OS key-code table. Hosts map it to their
/// own codes (macOS `kVK_*`, Windows `VK_*`, Linux evdev `KEY_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Enter,
    Backspace,
    /// Forward delete (⌦).
    Delete,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    pub const ALL: [Key; 13] = [
        Key::Enter,
        Key::Backspace,
        Key::Delete,
        Key::Tab,
        Key::Escape,
        Key::Left,
        Key::Right,
        Key::Up,
        Key::Down,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
    ];

    pub fn from_protocol(key: ProtocolKey) -> Key {
        match key {
            ProtocolKey::Return => Key::Enter,
            ProtocolKey::Backspace => Key::Backspace,
            ProtocolKey::ForwardDelete => Key::Delete,
            ProtocolKey::Tab => Key::Tab,
            ProtocolKey::Escape => Key::Escape,
            ProtocolKey::Left => Key::Left,
            ProtocolKey::Right => Key::Right,
            ProtocolKey::Up => Key::Up,
            ProtocolKey::Down => Key::Down,
            ProtocolKey::Home => Key::Home,
            ProtocolKey::End => Key::End,
            ProtocolKey::PageUp => Key::PageUp,
            ProtocolKey::PageDown => Key::PageDown,
        }
    }

    /// The key for a raw `key` byte; `None` for keys this version does not know (ignored).
    pub fn from_wire(raw: u8) -> Option<Key> {
        ProtocolKey::from_u8(raw).map(Key::from_protocol)
    }

    pub fn is_arrow(self) -> bool {
        matches!(self, Key::Left | Key::Right | Key::Up | Key::Down)
    }

    /// Keys a hardware keyboard reports as navigation ("function" on macOS,
    /// "extended" on Windows): the arrows, forward delete, Home, End, Page
    /// Up/Down. Mirroring that makes injected keys look like real ones.
    pub fn is_navigation(self) -> bool {
        self.is_arrow() || matches!(self, Key::Delete | Key::Home | Key::End | Key::PageUp | Key::PageDown)
    }
}

/// A key press to inject: a key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyStroke {
    pub key: Key,
    pub modifiers: KeyModifiers,
}

impl KeyStroke {
    /// The stroke for a KEY message, or `None` when the key is unknown.
    /// Unknown modifier bits are dropped.
    pub fn from_event(event: &KeyEvent) -> Option<KeyStroke> {
        Some(KeyStroke { key: Key::from_wire(event.key)?, modifiers: event.key_modifiers() })
    }
}

/// Preparing TEXT for injection.
pub mod typing {
    use unicode_segmentation::UnicodeSegmentation;

    /// macOS `CGEventKeyboardSetUnicodeString` carries at most 20 UTF-16 units per event.
    pub const MAX_UTF16_PER_EVENT: usize = 20;

    /// Chunks of ≤ `max_utf16` UTF-16 units, cut only between grapheme clusters
    /// (so surrogate pairs, combining sequences and ZWJ emoji stay whole). A
    /// single grapheme longer than the limit is cut between Unicode scalars as
    /// a last resort, still never inside a surrogate pair; the text system
    /// recombines the pieces as they are inserted one after another.
    pub fn chunks(text: &str, max_utf16: usize) -> Vec<String> {
        let limit = max_utf16.max(2); // a non-BMP scalar needs 2 units
        let mut out = Vec::new();
        let mut current = String::new();
        let mut current_units = 0;
        for grapheme in text.graphemes(true) {
            let units = grapheme.encode_utf16().count();
            if units > limit {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    current_units = 0;
                }
                let mut piece = String::new();
                let mut piece_units = 0;
                for scalar in grapheme.chars() {
                    let width = scalar.len_utf16();
                    if piece_units + width > limit {
                        out.push(std::mem::take(&mut piece));
                        piece_units = 0;
                    }
                    piece.push(scalar);
                    piece_units += width;
                }
                if !piece.is_empty() {
                    out.push(piece);
                }
                continue;
            }
            if current_units + units > limit {
                out.push(std::mem::take(&mut current));
                current_units = 0;
            }
            current.push_str(grapheme);
            current_units += units;
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }

    /// Drops graphemes containing control characters (general category Cc).
    /// The client sends those as KEY; typing them as Unicode would act
    /// unpredictably across apps.
    pub fn remove_control_characters(text: &str) -> String {
        text.graphemes(true).filter(|g| !g.chars().any(char::is_control)).collect()
    }
}
