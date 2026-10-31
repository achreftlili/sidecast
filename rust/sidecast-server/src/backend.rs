//! The traits platform code implements. A host is a [`Backends`] value: a
//! display/capture backend, one or more encoders, and optional input,
//! microphone and power backends. The server owns the protocol, flow control
//! and timing; backends only produce frames and inject input.
//!
//! Threading: every method may block (create a virtual display, open an
//! encoder). The server calls them from blocking threads or from its input
//! task, never while holding a lock. Frames are submitted from any thread.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use sidecast_core::input::{GestureAction, InputEvent};
use sidecast_core::protocol::{AudioPacket, ByeReason, CodecInfo, MicState, ScreenInfo, StreamLimits};
use sidecast_core::streaming::{EncoderCapability, StreamPlan, VideoCodec};
use sidecast_core::{Point, Size};

pub use crate::stream::{FrameSink, FrameSlot};

/// What went wrong in platform code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendErrorKind {
    /// This platform has no implementation yet.
    NotSupported,
    /// The user must grant something first (screen capture, input injection).
    PermissionMissing,
    /// The virtual display or capture could not start.
    DisplayFailed,
    /// No encoder could be created or it failed.
    EncoderFailed,
    Other,
}

impl BackendError {
    pub fn new(kind: BackendErrorKind, message: impl Into<String>) -> Self {
        BackendError { kind, message: message.into() }
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::new(BackendErrorKind::NotSupported, message)
    }

    pub fn permission_missing(message: impl Into<String>) -> Self {
        Self::new(BackendErrorKind::PermissionMissing, message)
    }

    pub fn display_failed(message: impl Into<String>) -> Self {
        Self::new(BackendErrorKind::DisplayFailed, message)
    }

    pub fn encoder_failed(message: impl Into<String>) -> Self {
        Self::new(BackendErrorKind::EncoderFailed, message)
    }

    /// The BYE a phone gets when this stops its stream.
    pub fn bye_reason(&self) -> ByeReason {
        match self.kind {
            BackendErrorKind::PermissionMissing => ByeReason::PermissionMissing,
            _ => ByeReason::DisplayFailed,
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            BackendErrorKind::NotSupported => "not supported",
            BackendErrorKind::PermissionMissing => "permission missing",
            BackendErrorKind::DisplayFailed => "display failed",
            BackendErrorKind::EncoderFailed => "encoder failed",
            BackendErrorKind::Other => "error",
        };
        write!(f, "{kind}: {}", self.message)
    }
}

impl std::error::Error for BackendError {}

/// The phone a stream is for (from its HELLO).
#[derive(Debug, Clone, PartialEq)]
pub struct ClientInfo {
    pub device_id: String,
    pub device_name: String,
    pub screen: ScreenInfo,
    pub codecs: Vec<CodecInfo>,
    /// HELLO `network` / `limits` (data saver, PROTOCOL §5).
    pub network: Option<String>,
    pub limits: Option<StreamLimits>,
    pub supports_mic: bool,
}

impl ClientInfo {
    pub fn data_saver(&self) -> bool {
        self.limits.is_some()
    }
}

/// Where the streamed display sits, for input injection.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayGeometry {
    /// Size in points: the coordinate space of [`InputEvent`] positions
    /// (top-left origin). Usually `plan.display.width × height`.
    pub size: Size,
    /// Top-left of the display in the desktop's global coordinates, for
    /// injection APIs that take absolute desktop positions.
    pub origin: Point,
    /// Platform identifier of the display (Windows device name, Wayland
    /// output, X11 RandR output…), opaque to the server.
    pub id: Option<String>,
}

impl DisplayGeometry {
    pub fn new(size: Size) -> Self {
        DisplayGeometry { size, origin: Point::ZERO, id: None }
    }
}

/// Pixels of one captured frame. Software encoders take `I420`; the others
/// let capture hand its native format to a matching hardware encoder without
/// a copy (e.g. a D3D11 texture or a DMA-BUF in `Native`).
pub enum PixelData {
    /// Planar Y, U, V (4:2:0), BT.709 limited range.
    I420 { y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, y_stride: usize, uv_stride: usize },
    /// Y plane + interleaved UV plane (4:2:0).
    Nv12 { y: Vec<u8>, uv: Vec<u8>, y_stride: usize, uv_stride: usize },
    /// 8-bit B, G, R, A per pixel.
    Bgra { data: Vec<u8>, stride: usize },
    /// A platform buffer only the platform's own encoder understands.
    Native(Box<dyn Any + Send>),
}

impl fmt::Debug for PixelData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PixelData::I420 { .. } => "I420",
            PixelData::Nv12 { .. } => "NV12",
            PixelData::Bgra { .. } => "BGRA",
            PixelData::Native(_) => "Native",
        })
    }
}

/// One frame from the capture backend.
#[derive(Debug)]
pub struct CapturedFrame {
    /// Capture time, monotonic microseconds (see [`crate::clock::monotonic_micros`]).
    pub pts_micros: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: PixelData,
}

/// Produces the frames of the streamed display.
pub trait DisplayBackend: Send + Sync {
    /// For logs, e.g. "windows-idd" or "test-pattern".
    fn name(&self) -> &str;

    /// Checked before every stream; an error ends the session with its BYE
    /// reason (e.g. `permission_missing` until screen capture is allowed).
    fn preflight(&self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Creates the virtual display for `plan.display` (or selects the chosen
    /// monitor) and starts delivering frames of `plan.encode_width ×
    /// plan.encode_height` at about `plan.fps` to `frames`, from any thread.
    /// Called on a blocking thread. Dropping the session stops capture and
    /// removes the virtual display.
    fn start(
        &self,
        plan: &StreamPlan,
        client: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError>;
}

/// A running capture, owned by the stream.
pub trait CaptureSession: Send {
    /// Where input for this display goes.
    fn geometry(&self) -> DisplayGeometry;

    /// Re-deliver the latest frame now, so a keyframe request on a static
    /// screen is answered promptly. Frames that flow continuously can ignore it.
    fn request_refresh(&mut self) {}
}

/// Settings for one encoder instance.
#[derive(Debug, Clone, PartialEq)]
pub struct EncoderConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Target bits per second (changes later through [`VideoEncoder::set_bitrate`]).
    pub bitrate: u32,
    /// Emit a keyframe at least this often, in seconds (the server also forces one).
    pub keyframe_interval: f64,
}

/// One encoded access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Annex-B (00 00 00 01 start codes) for exactly one access unit;
    /// keyframes carry their parameter sets (H.264: SPS, PPS; HEVC: VPS, SPS, PPS).
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

/// A low-latency encoder: no B-frames / reordering, one access unit out per
/// frame in. Runs on a dedicated thread.
pub trait VideoEncoder: Send {
    /// `Ok(None)` when the encoder skipped the frame (rate control).
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>, BackendError>;

    fn set_bitrate(&mut self, bits_per_second: u32);
}

/// Creates encoders of some kind (hardware vendor API, Media Foundation,
/// VA-API, OpenH264…). The server asks factories in order.
pub trait EncoderFactory: Send + Sync {
    fn name(&self) -> &str;

    /// Codecs and sizes this factory can encode; they shape the stream plan.
    fn capabilities(&self) -> Vec<EncoderCapability>;

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, BackendError>;
}

/// Injects input into the streamed display (and the focused app for keys and text).
pub trait InputBackend: Send + Sync {
    /// A sink for a stream on `geometry`. Called when a stream (re)starts.
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError>;
}

/// The per-stream input injector. The server calls it from one task, in
/// order, with click timing already applied (PROTOCOL §7.3).
pub trait InputSink: Send {
    fn inject(&mut self, event: &InputEvent);

    /// The gesture behind the next events (diagnostics; optional).
    fn observe_gesture(&mut self, _action: &GestureAction) {}

    /// The system pointer in display points, or `None` when it is not on the
    /// streamed display. Feeds trackpad mode and CURSOR (PROTOCOL §7.3).
    fn pointer_position(&mut self) -> Option<Point> {
        None
    }

    /// Releases every button and key this sink holds (disconnect safety).
    fn release_all(&mut self);
}

/// Identifies one connection for the microphone backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Starts and stops the phone's microphone for one streaming session (MIC_CONTROL).
#[derive(Clone)]
pub struct MicController {
    pub(crate) send: Arc<dyn Fn(bool) + Send + Sync>,
}

impl MicController {
    /// Asks the phone to start (`true`) or stop capturing. Ignored when the
    /// phone did not advertise `mic` or the session ended.
    pub fn set_active(&self, active: bool) {
        (self.send)(active)
    }
}

impl fmt::Debug for MicController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MicController")
    }
}

/// The host side of the phone microphone (PROTOCOL §7.2): a virtual input
/// device fed with the phone's audio (see `sidecast_core::audio::AudioJitterBuffer`).
pub trait MicSink: Send + Sync {
    /// A session started streaming; the mic starts off. Call
    /// `control.set_active(true)` while some app records from the virtual mic.
    fn session_started(&self, session: SessionId, device_name: &str, supports_mic: bool, control: MicController);

    /// The session stopped streaming (closed or replaced).
    fn session_ended(&self, session: SessionId);

    /// AUDIO while the last MIC_CONTROL sent to that session was `active:true`.
    fn audio(&self, session: SessionId, packet: &AudioPacket);

    /// MIC_STATE while the last MIC_CONTROL sent to that session was `active:true`.
    fn mic_state(&self, session: SessionId, state: &MicState);
}

/// Keeps the computer awake while a phone streams.
pub trait PowerGuard: Send + Sync {
    /// Prevents idle sleep until the returned value is dropped.
    fn keep_awake(&self, reason: &str) -> Result<Box<dyn Send>, BackendError>;
}

/// Everything platform-specific a host provides.
#[derive(Clone)]
pub struct Backends {
    pub display: Arc<dyn DisplayBackend>,
    /// Tried in order for the negotiated codec; their capabilities decide the codec.
    pub encoders: Vec<Arc<dyn EncoderFactory>>,
    /// `None`: TOUCH, TEXT and KEY are ignored.
    pub input: Option<Arc<dyn InputBackend>>,
    /// `None`: no virtual microphone, MIC_CONTROL is never sent.
    pub mic: Option<Arc<dyn MicSink>>,
    pub power: Option<Arc<dyn PowerGuard>>,
}

impl Backends {
    pub fn new(display: Arc<dyn DisplayBackend>, encoders: Vec<Arc<dyn EncoderFactory>>) -> Self {
        Backends { display, encoders, input: None, mic: None, power: None }
    }

    /// Every codec/size the encoders can produce, best first.
    pub fn encoder_capabilities(&self) -> Vec<EncoderCapability> {
        let mut all: Vec<EncoderCapability> = Vec::new();
        for factory in &self.encoders {
            for cap in factory.capabilities() {
                match all.iter_mut().find(|c| c.codec == cap.codec) {
                    Some(existing) => {
                        existing.max_width = existing.max_width.max(cap.max_width);
                        existing.max_height = existing.max_height.max(cap.max_height);
                    }
                    None => all.push(cap),
                }
            }
        }
        all
    }
}

impl fmt::Debug for Backends {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Backends")
            .field("display", &self.display.name())
            .field("encoders", &self.encoders.iter().map(|e| e.name().to_owned()).collect::<Vec<_>>())
            .field("input", &self.input.is_some())
            .field("mic", &self.mic.is_some())
            .field("power", &self.power.is_some())
            .finish()
    }
}
