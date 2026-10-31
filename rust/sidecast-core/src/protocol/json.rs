//! JSON payloads (PROTOCOL §4–§7). Receivers ignore unknown keys; optional
//! fields that are malformed are dropped rather than rejecting the message,
//! like the Swift and Kotlin codecs. Keys are written in sorted order so the
//! output is byte-identical to the Mac app's.

use serde::{Deserialize, Deserializer, Serialize};

use super::{ByeReason, MessageType, ProtocolError, feature};

/// Binary data in JSON: standard Base64 with padding (RFC 4648 §4). Decoding
/// skips characters outside the alphabet (Android's `Base64.DEFAULT` wraps
/// lines) and tolerates missing padding.
pub mod b64 {
    use base64::Engine;
    use base64::alphabet;
    use base64::engine::general_purpose::STANDARD;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );

    pub fn encode(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let cleaned: String =
            text.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
        LENIENT.decode(cleaned.trim_end_matches('=')).ok()
    }

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        decode(&text).ok_or_else(|| D::Error::custom("invalid base64"))
    }

    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer, de::Error};

        pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            match bytes {
                Some(b) => s.serialize_str(&super::encode(b)),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            match Option::<String>::deserialize(d)? {
                Some(text) => super::decode(&text).map(Some).ok_or_else(|| D::Error::custom("invalid base64")),
                None => Ok(None),
            }
        }
    }
}

pub(crate) fn encode_json<T: Serialize>(value: &T) -> Vec<u8> {
    // These plain structs cannot fail to serialize.
    serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec())
}

pub(crate) fn decode_json<'a, T: Deserialize<'a>>(
    payload: &'a [u8],
    msg_type: MessageType,
) -> Result<T, ProtocolError> {
    serde_json::from_slice(payload).map_err(|e| ProtocolError::Malformed(msg_type, e.to_string()))
}

/// A string field that is ignored (None) when it has another JSON type.
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Some(s),
        _ => None,
    })
}

fn default_device_name() -> String {
    "Android phone".to_owned()
}
fn default_density() -> i64 {
    160
}
fn default_refresh_rate() -> f64 {
    60.0
}
fn default_max_width() -> i64 {
    1920
}
fn default_max_height() -> i64 {
    1080
}

// MARK: - Client → Server

/// HELLO `screen`: physical landscape pixels of the area the stream fills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenInfo {
    /// Only the size is essential; density and refresh rate fall back to defaults.
    #[serde(default = "default_density")]
    pub density_dpi: i64,
    pub height: i64,
    /// Accepts `120` as well as `120.0`.
    #[serde(default = "default_refresh_rate")]
    pub refresh_rate: f64,
    pub width: i64,
}

impl ScreenInfo {
    pub fn new(width: i64, height: i64, density_dpi: i64, refresh_rate: f64) -> Self {
        ScreenInfo { density_dpi, height, refresh_rate, width }
    }
}

/// A decoder the client can use, with the largest landscape size it handles at ≥ 60 fps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecInfo {
    /// A codec without a reported limit is assumed to handle 1080p.
    #[serde(default = "default_max_height")]
    pub max_height: i64,
    #[serde(default = "default_max_width")]
    pub max_width: i64,
    pub name: String,
}

impl CodecInfo {
    pub fn new(name: &str, max_width: i64, max_height: i64) -> Self {
        CodecInfo { max_height, max_width, name: name.to_owned() }
    }
}

/// HELLO `network` values (PROTOCOL §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientNetwork {
    Metered,
    Unmetered,
}

impl ClientNetwork {
    pub fn parse(raw: &str) -> Option<ClientNetwork> {
        match raw {
            "metered" => Some(ClientNetwork::Metered),
            "unmetered" => Some(ClientNetwork::Unmetered),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ClientNetwork::Metered => "metered",
            ClientNetwork::Unmetered => "unmetered",
        }
    }
}

/// HELLO `limits` (data saver, PROTOCOL §5). Missing or non-positive fields mean "no limit".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bitrate_kbps: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<i64>,
}

impl StreamLimits {
    /// The phone's default data-saver caps: 4000 kbps, 30 fps.
    pub const DATA_SAVER: StreamLimits = StreamLimits { max_bitrate_kbps: Some(4_000), max_fps: Some(30) };

    pub fn new(max_bitrate_kbps: Option<i64>, max_fps: Option<i64>) -> Self {
        StreamLimits { max_bitrate_kbps, max_fps }
    }

    /// Bitrate cap in bits per second, if any.
    pub fn max_bitrate(&self) -> Option<i64> {
        self.max_bitrate_kbps.map(|k| k.saturating_mul(1_000))
    }

    /// No usable cap at all.
    pub fn is_empty(&self) -> bool {
        self.max_bitrate_kbps.is_none() && self.max_fps.is_none()
    }

    /// Tolerant reading: a field of the wrong type, or ≤ 0, is ignored (floats
    /// are truncated); anything but an object is no limits at all.
    fn from_value(value: &serde_json::Value) -> Option<StreamLimits> {
        let object = value.as_object()?;
        let positive = |key: &str| -> Option<i64> {
            let v = object.get(key)?;
            if let Some(i) = v.as_i64() {
                return (i > 0).then_some(i);
            }
            if v.is_u64() {
                return v.as_u64().map(|u| u.min(1_000_000_000) as i64);
            }
            let f = v.as_f64()?;
            (f.is_finite() && f >= 1.0).then(|| f.min(1e9) as i64)
        };
        let limits = StreamLimits { max_bitrate_kbps: positive("maxBitrateKbps"), max_fps: positive("maxFps") };
        (!limits.is_empty()).then_some(limits)
    }
}

fn lenient_limits<'de, D: Deserializer<'de>>(d: D) -> Result<Option<StreamLimits>, D::Error> {
    Ok(StreamLimits::from_value(&serde_json::Value::deserialize(d)?))
}

/// HELLO (client → server, PROTOCOL §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(default)]
    pub codecs: Vec<CodecInfo>,
    pub device_id: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    /// Optional capabilities (see [`feature`]); omitted when empty, as older clients do.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Data-saver caps; `None` when absent, malformed or empty.
    #[serde(default, deserialize_with = "lenient_limits", skip_serializing_if = "Option::is_none")]
    pub limits: Option<StreamLimits>,
    /// Raw `network`; see [`Hello::client_network`].
    #[serde(default, deserialize_with = "lenient_string", skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub protocol: i64,
    pub screen: ScreenInfo,
}

impl Hello {
    pub fn new(device_id: &str, device_name: &str, screen: ScreenInfo, codecs: Vec<CodecInfo>) -> Self {
        Hello {
            codecs,
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            features: Vec::new(),
            limits: None,
            network: None,
            protocol: super::VERSION,
            screen,
        }
    }

    pub fn client_network(&self) -> Option<ClientNetwork> {
        self.network.as_deref().and_then(ClientNetwork::parse)
    }

    /// The client can stream its microphone (PROTOCOL §7.2).
    pub fn supports_mic(&self) -> bool {
        self.features.iter().any(|f| f == feature::MIC)
    }

    /// Checks the version first, so a future HELLO shape is reported as a
    /// version mismatch rather than a malformed message.
    pub(crate) fn decode(payload: &[u8]) -> Result<Hello, ProtocolError> {
        #[derive(Deserialize)]
        struct Version {
            protocol: i64,
        }
        let version: Version = decode_json(payload, MessageType::Hello)?;
        if version.protocol != super::VERSION {
            return Err(ProtocolError::VersionMismatch(version.protocol));
        }
        decode_json(payload, MessageType::Hello)
    }
}

/// AUTH (client → server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    pub device_id: String,
    #[serde(with = "b64")]
    pub token: Vec<u8>,
}

/// PAIR_PROOF (client → server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairProof {
    #[serde(with = "b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "b64")]
    pub proof: Vec<u8>,
}

/// How TOUCH is interpreted (PROTOCOL §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InputMode {
    /// The phone is a touchscreen: positions are absolute (default).
    #[default]
    Touch,
    /// The phone is a laptop trackpad: finger movement moves the cursor relatively.
    Trackpad,
}

impl InputMode {
    pub fn parse(raw: &str) -> Option<InputMode> {
        match raw {
            "touch" => Some(InputMode::Touch),
            "trackpad" => Some(InputMode::Trackpad),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            InputMode::Touch => "touch",
            InputMode::Trackpad => "trackpad",
        }
    }
}

/// INPUT_MODE (client → server): `{"mode": "touch" | "trackpad"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputModeMessage {
    /// Raw mode; unknown values decode fine and are ignored.
    pub mode: String,
}

impl InputModeMessage {
    pub fn new(mode: InputMode) -> Self {
        InputModeMessage { mode: mode.as_str().to_owned() }
    }

    pub fn input_mode(&self) -> Option<InputMode> {
        InputMode::parse(&self.mode)
    }
}

/// MIC_STATE `reason` values (PROTOCOL §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicStateReason {
    Stopped,
    Disabled,
    PermissionDenied,
    Background,
    Unavailable,
}

impl MicStateReason {
    pub const ALL: [MicStateReason; 5] = [
        MicStateReason::Stopped,
        MicStateReason::Disabled,
        MicStateReason::PermissionDenied,
        MicStateReason::Background,
        MicStateReason::Unavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MicStateReason::Stopped => "stopped",
            MicStateReason::Disabled => "disabled",
            MicStateReason::PermissionDenied => "permission_denied",
            MicStateReason::Background => "background",
            MicStateReason::Unavailable => "unavailable",
        }
    }

    pub fn parse(raw: &str) -> Option<MicStateReason> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }
}

/// MIC_STATE (client → server): whether the phone's microphone is capturing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicState {
    pub capturing: bool,
    /// Raw reason when not capturing; see [`MicState::known_reason`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MicState {
    pub fn capturing() -> Self {
        MicState { capturing: true, reason: None }
    }

    pub fn stopped(reason: MicStateReason) -> Self {
        MicState { capturing: false, reason: Some(reason.as_str().to_owned()) }
    }

    pub fn known_reason(&self) -> Option<MicStateReason> {
        self.reason.as_deref().and_then(MicStateReason::parse)
    }
}

// MARK: - Server → Client

/// The host's operating system: SERVER_HELLO `platform` and DNS-SD TXT `os`
/// (PROTOCOL §1, §4, DECISIONS Q34). Display and OS-specific advice only: the
/// protocol is the same for every OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostPlatform {
    Macos,
    Windows,
    Linux,
}

impl HostPlatform {
    pub const ALL: [HostPlatform; 3] = [HostPlatform::Macos, HostPlatform::Windows, HostPlatform::Linux];

    /// The OS this binary runs on (`None` elsewhere, e.g. the BSDs).
    pub fn current() -> Option<HostPlatform> {
        if cfg!(target_os = "macos") {
            Some(HostPlatform::Macos)
        } else if cfg!(target_os = "windows") {
            Some(HostPlatform::Windows)
        } else if cfg!(target_os = "linux") {
            Some(HostPlatform::Linux)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HostPlatform::Macos => "macos",
            HostPlatform::Windows => "windows",
            HostPlatform::Linux => "linux",
        }
    }

    pub fn parse(raw: &str) -> Option<HostPlatform> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw)
    }
}

/// Mac hosts before `platform` existed omit it, so absent, `null` or
/// malformed all mean macOS (PROTOCOL §4, §8.4).
fn platform_or_macos<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(lenient_string(d)?.unwrap_or_else(default_platform))
}

fn default_platform() -> String {
    HostPlatform::Macos.as_str().to_owned()
}

/// SERVER_HELLO `addresses[].kind` (PROTOCOL §1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressKind {
    Lan,
    Tailscale,
    TailscaleDns,
}

impl AddressKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AddressKind::Lan => "lan",
            AddressKind::Tailscale => "tailscale",
            AddressKind::TailscaleDns => "tailscale-dns",
        }
    }

    pub fn parse(raw: &str) -> Option<AddressKind> {
        match raw {
            "lan" => Some(AddressKind::Lan),
            "tailscale" => Some(AddressKind::Tailscale),
            "tailscale-dns" => Some(AddressKind::TailscaleDns),
            _ => None,
        }
    }
}

/// One address the host can be reached at (PROTOCOL §1.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServerAddress {
    pub host: String,
    /// Raw kind; see [`ServerAddress::address_kind`].
    pub kind: String,
}

impl ServerAddress {
    pub fn new(host: &str, kind: AddressKind) -> Self {
        ServerAddress { host: host.to_owned(), kind: kind.as_str().to_owned() }
    }

    pub fn address_kind(&self) -> Option<AddressKind> {
        AddressKind::parse(&self.kind)
    }
}

/// SERVER_HELLO (server → client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHello {
    /// Reachable addresses: LAN first, then Tailscale, then MagicDNS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<ServerAddress>>,
    /// Raw host OS, always sent; see [`ServerHello::host_platform`]. Kept raw
    /// so an unknown future value round-trips.
    #[serde(default = "default_platform", deserialize_with = "platform_or_macos")]
    pub platform: String,
    /// The listening port (same for every address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    pub protocol: i64,
    pub server_id: String,
    pub server_name: String,
}

impl ServerHello {
    pub fn new(server_id: &str, server_name: &str, platform: HostPlatform) -> Self {
        ServerHello {
            addresses: None,
            platform: platform.as_str().to_owned(),
            port: None,
            protocol: super::VERSION,
            server_id: server_id.to_owned(),
            server_name: server_name.to_owned(),
        }
    }

    /// `None` for an OS this version does not know: show a generic computer.
    pub fn host_platform(&self) -> Option<HostPlatform> {
        HostPlatform::parse(&self.platform)
    }
}

/// PAIR_CHALLENGE (server → client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairChallenge {
    pub attempts_left: i64,
    pub expires_in_sec: i64,
}

/// AUTH_RESULT (server → client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts_left: Option<i64>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, with = "b64::option", skip_serializing_if = "Option::is_none")]
    pub token: Option<Vec<u8>>,
}

impl AuthResult {
    pub fn ok(token: Option<Vec<u8>>) -> Self {
        AuthResult { attempts_left: None, ok: true, reason: None, token }
    }

    pub fn failed(reason: ByeReason, attempts_left: Option<i64>) -> Self {
        AuthResult { attempts_left, ok: false, reason: Some(reason.as_str().to_owned()), token: None }
    }
}

/// STREAM_CONFIG (server → client, PROTOCOL §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub codec: String,
    pub fps: i64,
    pub height: i64,
    pub width: i64,
}

impl StreamConfig {
    pub fn new(codec: &str, width: i64, height: i64, fps: i64) -> Self {
        StreamConfig { codec: codec.to_owned(), fps, height, width }
    }
}

/// MIC_CONTROL (server → client): start or stop the phone's microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicControl {
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i64>,
}

impl MicControl {
    /// `{"active": true, "sampleRate": 48000, "channels": 1}`
    pub const START: MicControl = MicControl {
        active: true,
        channels: Some(super::AudioPacket::CHANNELS as i64),
        sample_rate: Some(super::AudioPacket::SAMPLE_RATE as i64),
    };
    /// `{"active": false}`
    pub const STOP: MicControl = MicControl { active: false, channels: None, sample_rate: None };
}

/// BYE (server → client), then the server closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bye {
    pub reason: String,
}

impl Bye {
    pub fn new(reason: ByeReason) -> Self {
        Bye { reason: reason.as_str().to_owned() }
    }

    pub fn known_reason(&self) -> Option<ByeReason> {
        ByeReason::parse(&self.reason)
    }
}
