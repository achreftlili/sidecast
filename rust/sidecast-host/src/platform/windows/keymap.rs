//! OS-neutral keys, modifiers and semantic actions → Windows virtual-key
//! chords for `SendInput`. Pure, so it is tested on every OS.

use sidecast_core::input::{Key, SemanticAction};
use sidecast_core::protocol::KeyModifiers;

/// The Windows virtual-key codes used here (`winuser.h`).
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const ESCAPE: u16 = 0x1B;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const DELETE: u16 = 0x2E;
    pub const D: u16 = 0x44;
    pub const LWIN: u16 = 0x5B;
    pub const LSHIFT: u16 = 0xA0;
    pub const LCONTROL: u16 = 0xA2;
    pub const LMENU: u16 = 0xA4;
}

/// A key as `SendInput` sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VirtualKey {
    pub code: u16,
    /// `KEYEVENTF_EXTENDEDKEY`: the key has an E0 scan-code prefix on a real
    /// keyboard (navigation cluster, Windows key). Apps that read scan codes
    /// tell e.g. the arrow keys from the numeric keypad by it.
    pub extended: bool,
}

impl VirtualKey {
    const fn plain(code: u16) -> Self {
        VirtualKey { code, extended: false }
    }

    const fn extended(code: u16) -> Self {
        VirtualKey { code, extended: true }
    }
}

/// The virtual key for a neutral key.
pub fn virtual_key(key: Key) -> VirtualKey {
    let code = match key {
        Key::Enter => vk::RETURN,
        Key::Backspace => vk::BACK,
        Key::Delete => vk::DELETE,
        Key::Tab => vk::TAB,
        Key::Escape => vk::ESCAPE,
        Key::Left => vk::LEFT,
        Key::Right => vk::RIGHT,
        Key::Up => vk::UP,
        Key::Down => vk::DOWN,
        Key::Home => vk::HOME,
        Key::End => vk::END,
        Key::PageUp => vk::PRIOR,
        Key::PageDown => vk::NEXT,
    };
    // `is_navigation` is exactly the set of keys a keyboard reports as extended.
    VirtualKey { code, extended: key.is_navigation() }
}

/// The modifier keys for `modifiers`, in the order they are pressed: Ctrl,
/// Alt, Shift, then Windows (META), as a person would. The left-hand keys,
/// so shortcuts that care about the side (Right Alt = AltGr) are not hit.
pub fn modifier_keys(modifiers: KeyModifiers) -> Vec<VirtualKey> {
    [
        (KeyModifiers::CONTROL, VirtualKey::plain(vk::LCONTROL)),
        (KeyModifiers::ALT, VirtualKey::plain(vk::LMENU)),
        (KeyModifiers::SHIFT, VirtualKey::plain(vk::LSHIFT)),
        (KeyModifiers::META, VirtualKey::extended(vk::LWIN)),
    ]
    .into_iter()
    .filter(|(modifier, _)| modifiers.contains(*modifier))
    .map(|(_, key)| key)
    .collect()
}

/// A key pressed with modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<VirtualKey>,
    pub key: VirtualKey,
}

impl Chord {
    pub fn new(key: Key, modifiers: KeyModifiers) -> Self {
        Chord { modifiers: modifier_keys(modifiers), key: virtual_key(key) }
    }

    /// The key transitions to send as one `SendInput` batch (so no other
    /// input interleaves): modifiers down, key down, key up, modifiers up in
    /// reverse order. `true` = down.
    pub fn transitions(&self) -> Vec<(VirtualKey, bool)> {
        let mut out = Vec::with_capacity(2 * self.modifiers.len() + 2);
        out.extend(self.modifiers.iter().map(|m| (*m, true)));
        out.push((self.key, true));
        out.push((self.key, false));
        out.extend(self.modifiers.iter().rev().map(|m| (*m, false)));
        out
    }
}

/// The Windows shortcut for a 3/4-finger swipe:
/// - `DesktopNext` / `DesktopPrevious`: Win+Ctrl+→ / ← (next / previous virtual desktop);
/// - `Overview`: Win+Tab (Task View);
/// - `AppWindows`: Win+D, show the desktop (again: restore the windows).
///   Windows has no App Exposé; Win+D is what a Windows precision touchpad
///   does on a three-finger swipe down, so the gesture keeps its meaning
///   for Windows users (DECISIONS Q37).
pub fn semantic_chord(action: SemanticAction) -> Chord {
    let win = VirtualKey::extended(vk::LWIN);
    let ctrl = VirtualKey::plain(vk::LCONTROL);
    match action {
        SemanticAction::DesktopNext => Chord { modifiers: vec![win, ctrl], key: VirtualKey::extended(vk::RIGHT) },
        SemanticAction::DesktopPrevious => Chord { modifiers: vec![win, ctrl], key: VirtualKey::extended(vk::LEFT) },
        SemanticAction::Overview => Chord { modifiers: vec![win], key: VirtualKey::plain(vk::TAB) },
        SemanticAction::AppWindows => Chord { modifiers: vec![win], key: VirtualKey::plain(vk::D) },
    }
}

/// UTF-16 code units of `text` as `KEYEVENTF_UNICODE` key transitions:
/// every unit down then up. A surrogate pair is two consecutive units, so
/// Windows delivers both halves as consecutive `WM_CHAR`s and the app
/// combines them; a whole pair always goes in the same batch.
pub fn unicode_transitions(text: &str) -> Vec<(u16, bool)> {
    text.encode_utf16().flat_map(|unit| [(unit, true), (unit, false)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_a_virtual_key_and_navigation_keys_are_extended() {
        let expected = [
            (Key::Enter, 0x0D, false),
            (Key::Backspace, 0x08, false),
            (Key::Delete, 0x2E, true),
            (Key::Tab, 0x09, false),
            (Key::Escape, 0x1B, false),
            (Key::Left, 0x25, true),
            (Key::Right, 0x27, true),
            (Key::Up, 0x26, true),
            (Key::Down, 0x28, true),
            (Key::Home, 0x24, true),
            (Key::End, 0x23, true),
            (Key::PageUp, 0x21, true),
            (Key::PageDown, 0x22, true),
        ];
        assert_eq!(expected.len(), Key::ALL.len());
        for (key, code, extended) in expected {
            assert_eq!(virtual_key(key), VirtualKey { code, extended }, "{key:?}");
        }
    }

    #[test]
    fn modifiers_press_in_a_fixed_order_and_release_in_reverse() {
        assert!(modifier_keys(KeyModifiers::NONE).is_empty());
        let all = modifier_keys(KeyModifiers::ALL);
        let codes: Vec<u16> = all.iter().map(|k| k.code).collect();
        assert_eq!(codes, [vk::LCONTROL, vk::LMENU, vk::LSHIFT, vk::LWIN]);
        assert!(all[3].extended, "the Windows key is an extended key");

        // Shift+Tab (KEY 4 with modifiers 1): Shift↓ Tab↓ Tab↑ Shift↑.
        let chord = Chord::new(Key::Tab, KeyModifiers::SHIFT);
        let shift = VirtualKey::plain(vk::LSHIFT);
        let tab = VirtualKey::plain(vk::TAB);
        assert_eq!(chord.transitions(), [(shift, true), (tab, true), (tab, false), (shift, false)]);

        // Ctrl+Alt+Delete-like chords release the last-pressed modifier first.
        let t = Chord::new(Key::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT).transitions();
        let order: Vec<(u16, bool)> = t.iter().map(|(k, down)| (k.code, *down)).collect();
        assert_eq!(
            order,
            [
                (vk::LCONTROL, true),
                (vk::LSHIFT, true),
                (vk::LEFT, true),
                (vk::LEFT, false),
                (vk::LSHIFT, false),
                (vk::LCONTROL, false)
            ]
        );
        assert!(t[2].0.extended);
    }

    #[test]
    fn semantic_actions_use_the_windows_shortcuts() {
        let codes = |action| {
            let chord = semantic_chord(action);
            (chord.modifiers.iter().map(|k| k.code).collect::<Vec<_>>(), chord.key.code)
        };
        assert_eq!(codes(SemanticAction::DesktopNext), (vec![vk::LWIN, vk::LCONTROL], vk::RIGHT));
        assert_eq!(codes(SemanticAction::DesktopPrevious), (vec![vk::LWIN, vk::LCONTROL], vk::LEFT));
        assert_eq!(codes(SemanticAction::Overview), (vec![vk::LWIN], vk::TAB));
        assert_eq!(codes(SemanticAction::AppWindows), (vec![vk::LWIN], vk::D));
        // The arrows of the desktop switch are extended, as on a keyboard.
        assert!(semantic_chord(SemanticAction::DesktopNext).key.extended);
        for action in SemanticAction::ALL {
            let t = semantic_chord(action).transitions();
            assert_eq!(t.first().map(|(k, d)| (k.code, *d)), Some((vk::LWIN, true)));
            assert_eq!(t.last().map(|(k, d)| (k.code, *d)), Some((vk::LWIN, false)), "{action:?} releases Win last");
        }
    }

    #[test]
    fn text_becomes_utf16_unit_pairs_with_surrogates_kept_together() {
        assert_eq!(unicode_transitions("é"), [(0xE9, true), (0xE9, false)]);
        // U+1F44B WAVING HAND SIGN = D83D DC4B.
        assert_eq!(unicode_transitions("👋"), [(0xD83D, true), (0xD83D, false), (0xDC4B, true), (0xDC4B, false)]);
        assert!(unicode_transitions("").is_empty());
    }
}
