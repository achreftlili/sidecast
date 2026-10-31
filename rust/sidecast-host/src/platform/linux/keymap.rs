//! Keyboard mapping for the Linux remote-desktop APIs (Mutter's
//! RemoteDesktop and the xdg-desktop-portal's): KEY → evdev key codes, which
//! do not depend on the keyboard layout; TEXT → X keysyms; and the GNOME
//! shortcuts behind 3/4-finger swipes.
//!
//! Text: Mutter types a keysym only when the active layout has a key for it
//! (it drops the others), so ASCII goes as keysyms and everything else, by
//! default, through the Ctrl+Shift+U Unicode entry of IBus and GTK, which
//! Ubuntu's GNOME session has everywhere (DECISIONS Q38).

use sidecast_core::input::{Key, SemanticAction};
use sidecast_core::protocol::KeyModifiers;

/// Linux evdev codes (`linux/input-event-codes.h`), what both APIs take for
/// keycodes and pointer buttons.
pub mod evdev {
    pub const KEY_ESC: u32 = 1;
    pub const KEY_BACKSPACE: u32 = 14;
    pub const KEY_TAB: u32 = 15;
    pub const KEY_ENTER: u32 = 28;
    pub const KEY_LEFTCTRL: u32 = 29;
    /// The key above Tab (GNOME's `Above_Tab`, whatever the layout prints on it).
    pub const KEY_GRAVE: u32 = 41;
    pub const KEY_LEFTSHIFT: u32 = 42;
    pub const KEY_LEFTALT: u32 = 56;
    pub const KEY_HOME: u32 = 102;
    pub const KEY_UP: u32 = 103;
    pub const KEY_PAGEUP: u32 = 104;
    pub const KEY_LEFT: u32 = 105;
    pub const KEY_RIGHT: u32 = 106;
    pub const KEY_END: u32 = 107;
    pub const KEY_DOWN: u32 = 108;
    pub const KEY_PAGEDOWN: u32 = 109;
    pub const KEY_DELETE: u32 = 111;
    /// Super (the Windows key).
    pub const KEY_LEFTMETA: u32 = 125;

    pub const BTN_LEFT: i32 = 0x110;
    pub const BTN_RIGHT: i32 = 0x111;
}

/// X keysyms used for text (`xkbcommon-keysyms.h`).
pub mod keysym {
    pub const SPACE: u32 = 0x20;
    pub const LOWER_U: u32 = 0x75;
    /// Unicode keysyms: `0x0100_0000 + code point` for characters outside Latin-1.
    pub const UNICODE_OFFSET: u32 = 0x0100_0000;
}

/// The evdev code of a special key.
pub fn evdev_key(key: Key) -> u32 {
    use evdev::*;
    match key {
        Key::Enter => KEY_ENTER,
        Key::Backspace => KEY_BACKSPACE,
        Key::Delete => KEY_DELETE,
        Key::Tab => KEY_TAB,
        Key::Escape => KEY_ESC,
        Key::Left => KEY_LEFT,
        Key::Right => KEY_RIGHT,
        Key::Up => KEY_UP,
        Key::Down => KEY_DOWN,
        Key::Home => KEY_HOME,
        Key::End => KEY_END,
        Key::PageUp => KEY_PAGEUP,
        Key::PageDown => KEY_PAGEDOWN,
    }
}

/// The modifier keys to hold, in press order (Ctrl, Shift, Alt, Super).
pub fn modifier_keys(modifiers: KeyModifiers) -> Vec<u32> {
    [
        (KeyModifiers::CONTROL, evdev::KEY_LEFTCTRL),
        (KeyModifiers::SHIFT, evdev::KEY_LEFTSHIFT),
        (KeyModifiers::ALT, evdev::KEY_LEFTALT),
        (KeyModifiers::META, evdev::KEY_LEFTMETA),
    ]
    .into_iter()
    .filter(|(bit, _)| modifiers.contains(*bit))
    .map(|(_, code)| code)
    .collect()
}

/// A shortcut: `modifiers` held (in order) around one press of `key`; all evdev codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<u32>,
    pub key: u32,
}

impl Chord {
    fn new(modifiers: &[u32], key: u32) -> Self {
        Chord { modifiers: modifiers.to_vec(), key }
    }
}

/// GNOME's default shortcut for a 3/4-finger swipe (Settings → Keyboard →
/// Keyboard Shortcuts → Navigation):
/// - desktops: Super+Page Down / Super+Page Up (`switch-to-workspace-right/left`);
/// - overview: a tap on Super (the Activities overview, mutter's `overlay-key`);
/// - app windows: Alt+Above_Tab (`switch-group`, the next window of the focused
///   app; GNOME has no App Exposé).
pub fn semantic_chord(action: SemanticAction) -> Chord {
    use evdev::*;
    match action {
        SemanticAction::DesktopNext => Chord::new(&[KEY_LEFTMETA], KEY_PAGEDOWN),
        SemanticAction::DesktopPrevious => Chord::new(&[KEY_LEFTMETA], KEY_PAGEUP),
        SemanticAction::Overview => Chord::new(&[], KEY_LEFTMETA),
        SemanticAction::AppWindows => Chord::new(&[KEY_LEFTALT], KEY_GRAVE),
    }
}

/// The keysym of a character: Latin-1 keysyms equal the code point, the rest
/// are Unicode keysyms.
pub fn char_keysym(c: char) -> u32 {
    match c as u32 {
        cp @ (0x20..=0x7e | 0xa0..=0xff) => cp,
        cp => keysym::UNICODE_OFFSET + cp,
    }
}

/// How TEXT reaches the focused app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextInput {
    /// ASCII as keysyms, other characters through Ctrl+Shift+U (IBus / GTK).
    #[default]
    UnicodeEntry,
    /// Every character as a keysym: right when the layout has keys for them
    /// (e.g. é on a French layout); Mutter drops the others.
    Keysyms,
}

impl TextInput {
    /// `SIDECAST_TEXT_INPUT=keysyms|unicode-entry`.
    pub fn parse(raw: &str) -> Option<TextInput> {
        match raw.trim() {
            "keysyms" | "keysym" => Some(TextInput::Keysyms),
            "unicode-entry" | "unicode" => Some(TextInput::UnicodeEntry),
            _ => None,
        }
    }
}

/// One character to type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStep {
    /// Press and release this keysym (Mutter adds Shift/AltGr as the layout needs).
    Keysym(u32),
    /// Ctrl+Shift+U, the code point in hex, then Space.
    UnicodeEntry(u32),
}

pub fn text_steps(text: &str, input: TextInput) -> Vec<TextStep> {
    text.chars()
        .map(|c| match input {
            TextInput::UnicodeEntry if !c.is_ascii() => TextStep::UnicodeEntry(c as u32),
            _ => TextStep::Keysym(char_keysym(c)),
        })
        .collect()
}

/// The keysyms typed after Ctrl+Shift+U for a code point: lowercase hex digits.
pub fn hex_keysyms(code_point: u32) -> Vec<u32> {
    format!("{code_point:x}").chars().map(|c| c as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_keys_map_to_evdev_codes() {
        assert_eq!(evdev_key(Key::Enter), 28);
        assert_eq!(evdev_key(Key::Backspace), 14);
        assert_eq!(evdev_key(Key::Delete), 111);
        assert_eq!(evdev_key(Key::Tab), 15);
        assert_eq!(evdev_key(Key::Escape), 1);
        assert_eq!(
            [Key::Left, Key::Right, Key::Up, Key::Down].map(evdev_key),
            [105, 106, 103, 108],
            "arrows are layout independent"
        );
        assert_eq!([Key::Home, Key::End, Key::PageUp, Key::PageDown].map(evdev_key), [102, 107, 104, 109]);
        // Every key has its own code.
        let mut codes: Vec<u32> = Key::ALL.iter().map(|k| evdev_key(*k)).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Key::ALL.len());
    }

    #[test]
    fn modifiers_press_in_a_fixed_order() {
        assert!(modifier_keys(KeyModifiers::NONE).is_empty());
        assert_eq!(modifier_keys(KeyModifiers::SHIFT), [42]);
        assert_eq!(modifier_keys(KeyModifiers::ALL), [29, 42, 56, 125]);
        assert_eq!(modifier_keys(KeyModifiers::META | KeyModifiers::CONTROL), [29, 125]);
    }

    #[test]
    fn swipes_use_the_gnome_default_shortcuts() {
        assert_eq!(semantic_chord(SemanticAction::DesktopNext), Chord::new(&[125], 109), "Super+Page Down");
        assert_eq!(semantic_chord(SemanticAction::DesktopPrevious), Chord::new(&[125], 104), "Super+Page Up");
        assert_eq!(semantic_chord(SemanticAction::Overview), Chord::new(&[], 125), "Super alone");
        assert_eq!(semantic_chord(SemanticAction::AppWindows), Chord::new(&[56], 41), "Alt+Above_Tab");
    }

    #[test]
    fn keysyms_for_latin1_and_unicode() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym(' '), keysym::SPACE);
        assert_eq!(char_keysym('~'), 0x7e);
        assert_eq!(char_keysym('é'), 0xe9, "Latin-1 keysym");
        assert_eq!(char_keysym('€'), 0x0100_20ac);
        assert_eq!(char_keysym('👋'), 0x0101_f44b);
        assert_eq!(char_keysym('\u{7f}'), 0x0100_007f, "DEL is not a Latin-1 keysym");
    }

    #[test]
    fn text_uses_unicode_entry_for_non_ascii_by_default() {
        assert_eq!(
            text_steps("hé👋", TextInput::default()),
            [TextStep::Keysym(0x68), TextStep::UnicodeEntry(0xe9), TextStep::UnicodeEntry(0x1f44b)]
        );
        assert_eq!(
            text_steps("hé👋", TextInput::Keysyms),
            [TextStep::Keysym(0x68), TextStep::Keysym(0xe9), TextStep::Keysym(0x0101_f44b)]
        );
        assert_eq!(hex_keysyms(0x1f44b), "1f44b".chars().map(|c| c as u32).collect::<Vec<_>>());
        assert_eq!(TextInput::parse("keysyms"), Some(TextInput::Keysyms));
        assert_eq!(TextInput::parse(" unicode-entry "), Some(TextInput::UnicodeEntry));
        assert_eq!(TextInput::parse("ibus"), None);
    }
}
