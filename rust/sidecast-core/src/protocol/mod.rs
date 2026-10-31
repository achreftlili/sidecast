//! The Sidecast wire protocol, version 1 (`docs/PROTOCOL.md`).

mod binary;
mod frame;
mod json;
mod keyboard;
mod messages;

pub use binary::{AudioPacket, CursorPosition, TouchAction, TouchEvent, TouchPointer, VideoFrame};
pub use frame::{Frame, FrameParser};
/// Base64 as carried in JSON (encode, lenient decode, serde helpers).
pub use json::b64 as json_b64;
pub use json::{
    AddressKind, AuthRequest, AuthResult, Bye, ClientNetwork, CodecInfo, Hello, HostPlatform, InputMode,
    InputModeMessage, MicControl, MicState, MicStateReason, PairChallenge, PairProof, ScreenInfo, ServerAddress,
    ServerHello, StreamConfig, StreamLimits,
};
pub use keyboard::{KeyEvent, KeyModifiers, ProtocolKey, text};
pub use messages::{ClientMessage, ServerMessage};

use std::fmt;

/// Protocol version spoken by this implementation.
pub const VERSION: i64 = 1;
/// Default TCP port (PROTOCOL §1).
pub const DEFAULT_PORT: u16 = 47800;
/// DNS-SD service type, without the domain.
pub const SERVICE_TYPE: &str = "_sidecast._tcp";

/// The DNS-SD TXT record (PROTOCOL §1).
pub mod txt {
    use super::HostPlatform;

    pub const VERSION_KEY: &str = "v";
    pub const SERVER_ID_KEY: &str = "id";
    pub const OS_KEY: &str = "os";

    /// What a host advertises: `v=1`, `id=<serverId>`, `os=<platform>`.
    pub fn record(server_id: &str, platform: HostPlatform) -> [(&'static str, String); 3] {
        [
            (VERSION_KEY, super::VERSION.to_string()),
            (SERVER_ID_KEY, server_id.to_owned()),
            (OS_KEY, platform.as_str().to_owned()),
        ]
    }

    /// The OS a discovered host advertises: absent = an older Mac host; `None` = unknown (generic computer).
    pub fn platform(os: Option<&str>) -> Option<HostPlatform> {
        os.map_or(Some(HostPlatform::Macos), HostPlatform::parse)
    }
}
/// Largest payload a frame may carry (16 MiB). Larger is a protocol error.
pub const MAX_PAYLOAD_LENGTH: usize = 16 * 1024 * 1024;
/// Frame header: type (u8) + length (u32).
pub const HEADER_LENGTH: usize = 5;

/// Pairing code lifetime in seconds (PROTOCOL §5).
pub const PAIRING_LIFETIME: f64 = 120.0;
/// Wrong codes allowed per pairing window.
pub const PAIRING_ATTEMPTS: u32 = 5;
/// Any connection not authenticated within this many seconds is closed.
pub const AUTH_TIMEOUT: f64 = 150.0;
/// An authenticated client that sent nothing for this many seconds is closed.
pub const LIVENESS_TIMEOUT: f64 = 5.0;
/// Frames sent (or being encoded) and not yet acknowledged (PROTOCOL §6).
pub const MAX_FRAMES_IN_FLIGHT: usize = 4;
/// The encoder emits a keyframe at least this often, in seconds.
pub const KEYFRAME_INTERVAL: f64 = 2.0;

/// Message type codes (PROTOCOL §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageType {
    // Client → Server
    Hello = 0x01,
    Auth = 0x02,
    PairStart = 0x03,
    PairProof = 0x04,
    Touch = 0x10,
    FrameAck = 0x11,
    KeyframeRequest = 0x12,
    Ping = 0x13,
    Text = 0x14,
    Key = 0x15,
    Audio = 0x16,
    MicState = 0x17,
    InputMode = 0x18,
    // Server → Client
    ServerHello = 0x81,
    PairChallenge = 0x82,
    AuthResult = 0x83,
    StreamConfig = 0x84,
    VideoFrame = 0x85,
    Pong = 0x86,
    Bye = 0x87,
    MicControl = 0x88,
    Cursor = 0x89,
}

impl MessageType {
    pub const ALL: [MessageType; 22] = [
        MessageType::Hello,
        MessageType::Auth,
        MessageType::PairStart,
        MessageType::PairProof,
        MessageType::Touch,
        MessageType::FrameAck,
        MessageType::KeyframeRequest,
        MessageType::Ping,
        MessageType::Text,
        MessageType::Key,
        MessageType::Audio,
        MessageType::MicState,
        MessageType::InputMode,
        MessageType::ServerHello,
        MessageType::PairChallenge,
        MessageType::AuthResult,
        MessageType::StreamConfig,
        MessageType::VideoFrame,
        MessageType::Pong,
        MessageType::Bye,
        MessageType::MicControl,
        MessageType::Cursor,
    ];

    /// The known type for a wire byte, or `None` (such frames must be skipped).
    pub fn from_u8(value: u8) -> Option<MessageType> {
        Self::ALL.into_iter().find(|t| *t as u8 == value)
    }

    /// Upper-case protocol name, e.g. `FRAME_ACK`.
    pub fn name(self) -> &'static str {
        match self {
            MessageType::Hello => "HELLO",
            MessageType::Auth => "AUTH",
            MessageType::PairStart => "PAIR_START",
            MessageType::PairProof => "PAIR_PROOF",
            MessageType::Touch => "TOUCH",
            MessageType::FrameAck => "FRAME_ACK",
            MessageType::KeyframeRequest => "KEYFRAME_REQUEST",
            MessageType::Ping => "PING",
            MessageType::Text => "TEXT",
            MessageType::Key => "KEY",
            MessageType::Audio => "AUDIO",
            MessageType::MicState => "MIC_STATE",
            MessageType::InputMode => "INPUT_MODE",
            MessageType::ServerHello => "SERVER_HELLO",
            MessageType::PairChallenge => "PAIR_CHALLENGE",
            MessageType::AuthResult => "AUTH_RESULT",
            MessageType::StreamConfig => "STREAM_CONFIG",
            MessageType::VideoFrame => "VIDEO_FRAME",
            MessageType::Pong => "PONG",
            MessageType::Bye => "BYE",
            MessageType::MicControl => "MIC_CONTROL",
            MessageType::Cursor => "CURSOR",
        }
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// `reason` values of AUTH_RESULT and BYE (PROTOCOL §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByeReason {
    BadCode,
    TooManyAttempts,
    PairingExpired,
    UnknownDevice,
    ProtocolMismatch,
    /// Another phone took over.
    Replaced,
    ServerShutdown,
    DisplayFailed,
    /// A permission the host needs (e.g. screen recording) is missing.
    PermissionMissing,
}

impl ByeReason {
    pub const ALL: [ByeReason; 9] = [
        ByeReason::BadCode,
        ByeReason::TooManyAttempts,
        ByeReason::PairingExpired,
        ByeReason::UnknownDevice,
        ByeReason::ProtocolMismatch,
        ByeReason::Replaced,
        ByeReason::ServerShutdown,
        ByeReason::DisplayFailed,
        ByeReason::PermissionMissing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ByeReason::BadCode => "bad_code",
            ByeReason::TooManyAttempts => "too_many_attempts",
            ByeReason::PairingExpired => "pairing_expired",
            ByeReason::UnknownDevice => "unknown_device",
            ByeReason::ProtocolMismatch => "protocol_mismatch",
            ByeReason::Replaced => "replaced",
            ByeReason::ServerShutdown => "server_shutdown",
            ByeReason::DisplayFailed => "display_failed",
            ByeReason::PermissionMissing => "permission_missing",
        }
    }

    pub fn parse(raw: &str) -> Option<ByeReason> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }
}

impl fmt::Display for ByeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Capabilities a client lists in HELLO `features` (PROTOCOL §5).
pub mod feature {
    /// Phone microphone → the host's virtual microphone (PROTOCOL §7.2).
    pub const MIC: &str = "mic";
}

/// Errors raised while parsing or decoding wire data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// A frame declared a payload above 16 MiB: close the connection.
    PayloadTooLarge(usize),
    Truncated(MessageType),
    Malformed(MessageType, String),
    /// HELLO with a protocol version other than 1.
    VersionMismatch(i64),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::PayloadTooLarge(n) => {
                write!(f, "payload of {n} bytes exceeds the 16 MiB limit")
            }
            ProtocolError::Truncated(t) => write!(f, "{t} payload is truncated"),
            ProtocolError::Malformed(t, why) => write!(f, "{t} payload is malformed: {why}"),
            ProtocolError::VersionMismatch(v) => write!(f, "protocol version {v} is not supported"),
        }
    }
}

impl std::error::Error for ProtocolError {}
