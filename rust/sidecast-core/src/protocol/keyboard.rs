//! TEXT and KEY payloads (PROTOCOL §7.1).

use super::{MessageType, ProtocolError};

/// TEXT payload: UTF-8 text typed on the phone, 1…4096 bytes.
pub mod text {
    use super::{MessageType, ProtocolError};

    pub const MAX_BYTES: usize = 4096;

    /// Empty, oversized or invalid UTF-8 payloads are malformed.
    pub fn decode(payload: &[u8]) -> Result<String, ProtocolError> {
        if payload.is_empty() {
            return Err(ProtocolError::Malformed(MessageType::Text, "empty".into()));
        }
        if payload.len() > MAX_BYTES {
            return Err(ProtocolError::Malformed(
                MessageType::Text,
                format!("{} bytes exceeds {MAX_BYTES}", payload.len()),
            ));
        }
        // `from_utf8` rejects overlong forms, surrogates and values above U+10FFFF.
        std::str::from_utf8(payload)
            .map(str::to_owned)
            .map_err(|_| ProtocolError::Malformed(MessageType::Text, "invalid UTF-8".into()))
    }
}

/// The `key` byte of KEY (PROTOCOL §7.1 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProtocolKey {
    Return = 1,
    Backspace = 2,
    ForwardDelete = 3,
    Tab = 4,
    Escape = 5,
    Left = 6,
    Right = 7,
    Up = 8,
    Down = 9,
    Home = 10,
    End = 11,
    PageUp = 12,
    PageDown = 13,
}

impl ProtocolKey {
    pub const ALL: [ProtocolKey; 13] = [
        ProtocolKey::Return,
        ProtocolKey::Backspace,
        ProtocolKey::ForwardDelete,
        ProtocolKey::Tab,
        ProtocolKey::Escape,
        ProtocolKey::Left,
        ProtocolKey::Right,
        ProtocolKey::Up,
        ProtocolKey::Down,
        ProtocolKey::Home,
        ProtocolKey::End,
        ProtocolKey::PageUp,
        ProtocolKey::PageDown,
    ];

    pub fn from_u8(raw: u8) -> Option<ProtocolKey> {
        Self::ALL.into_iter().find(|k| *k as u8 == raw)
    }
}

/// The `modifiers` bitmask of KEY. Unknown bits are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyModifiers(u8);

impl KeyModifiers {
    pub const NONE: KeyModifiers = KeyModifiers(0);
    pub const SHIFT: KeyModifiers = KeyModifiers(1);
    pub const CONTROL: KeyModifiers = KeyModifiers(2);
    /// Option on a Mac, Alt elsewhere.
    pub const ALT: KeyModifiers = KeyModifiers(4);
    /// Command on a Mac, the Windows / Super key elsewhere.
    pub const META: KeyModifiers = KeyModifiers(8);
    pub const ALL: KeyModifiers = KeyModifiers(15);

    /// Known bits only; unknown ones are dropped.
    pub const fn from_bits(raw: u8) -> Self {
        KeyModifiers(raw & Self::ALL.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: KeyModifiers) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: KeyModifiers) -> KeyModifiers {
        KeyModifiers(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for KeyModifiers {
    type Output = KeyModifiers;
    fn bitor(self, rhs: KeyModifiers) -> KeyModifiers {
        self.union(rhs)
    }
}

/// KEY payload: `key u8, modifiers u8`. Raw bytes are kept so unknown values
/// survive decoding; consumers ignore what they do not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub key: u8,
    pub modifiers: u8,
}

impl KeyEvent {
    pub fn new(key: ProtocolKey, modifiers: KeyModifiers) -> Self {
        KeyEvent { key: key as u8, modifiers: modifiers.bits() }
    }

    pub fn raw(key: u8, modifiers: u8) -> Self {
        KeyEvent { key, modifiers }
    }

    /// The known key, or `None` (the event is then ignored).
    pub fn protocol_key(&self) -> Option<ProtocolKey> {
        ProtocolKey::from_u8(self.key)
    }

    /// The known modifier bits.
    pub fn key_modifiers(&self) -> KeyModifiers {
        KeyModifiers::from_bits(self.modifiers)
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        vec![self.key, self.modifiers]
    }

    /// Shorter than 2 bytes is truncated; trailing bytes are ignored.
    pub fn decode(payload: &[u8]) -> Result<KeyEvent, ProtocolError> {
        match payload {
            [key, modifiers, ..] => Ok(KeyEvent { key: *key, modifiers: *modifiers }),
            _ => Err(ProtocolError::Truncated(MessageType::Key)),
        }
    }
}
