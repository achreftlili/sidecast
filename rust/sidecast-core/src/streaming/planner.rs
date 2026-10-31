use std::fmt;

use crate::protocol::{CodecInfo, ScreenInfo, StreamConfig, StreamLimits};

/// Video codecs of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    Hevc,
    H264,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 2] = [VideoCodec::Hevc, VideoCodec::H264];

    pub fn as_str(self) -> &'static str {
        match self {
            VideoCodec::Hevc => "hevc",
            VideoCodec::H264 => "h264",
        }
    }

    /// Case-insensitive, as HELLO codec names are compared.
    pub fn parse(raw: &str) -> Option<VideoCodec> {
        match raw.to_ascii_lowercase().as_str() {
            "hevc" => Some(VideoCodec::Hevc),
            "h264" => Some(VideoCodec::H264),
            _ => None,
        }
    }
}

impl fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the host's encoder can produce: a codec and its largest landscape frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderCapability {
    pub codec: VideoCodec,
    pub max_width: i64,
    pub max_height: i64,
}

impl EncoderCapability {
    pub const fn new(codec: VideoCodec, max_width: i64, max_height: i64) -> Self {
        EncoderCapability { codec, max_width, max_height }
    }

    /// VideoToolbox on Apple Silicon (the Mac app's encoder ceilings).
    pub const APPLE_SILICON: [EncoderCapability; 2] =
        [EncoderCapability::new(VideoCodec::Hevc, 8192, 4320), EncoderCapability::new(VideoCodec::H264, 4096, 2304)];

    /// H.264 level 5.1/5.2 frame-size ceiling (OpenH264 and most hardware encoders).
    pub const H264_LEVEL_5: EncoderCapability = EncoderCapability::new(VideoCodec::H264, 4096, 2304);
}

/// User settings that shape the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSettings {
    /// Render the virtual display at 2× (Retina-style / 200 % scaling) instead of 1×.
    pub hi_dpi: bool,
    /// Fraction of the phone's pixels to use: 1.0, 0.75 or 0.5.
    pub resolution_scale: f64,
    pub fps: i64,
    pub max_bitrate_mbps: i64,
    /// "Phone controls the computer": gates TOUCH and keyboard input (TEXT / KEY).
    pub touch_enabled: bool,
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings { hi_dpi: true, resolution_scale: 1.0, fps: 60, max_bitrate_mbps: 50, touch_enabled: true }
    }
}

impl StreamSettings {
    pub const ALLOWED_SCALES: [f64; 3] = [1.0, 0.75, 0.5];
    pub const ALLOWED_FPS: [i64; 3] = [30, 60, 120];
    pub const BITRATE_RANGE_MBPS: std::ops::RangeInclusive<i64> = 10..=100;
}

/// Bitrate ceiling and floor for one stream (PROTOCOL §5, §6): the user's
/// maximum capped by the client's limit; the 4 Mbps floor drops to the
/// ceiling when the limit is lower than that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitrateBounds {
    pub floor: i64,
    pub ceiling: i64,
}

impl BitrateBounds {
    pub const DEFAULT_FLOOR: i64 = 4_000_000;
    pub const DEFAULT_START: i64 = 20_000_000;

    pub fn new(setting_mbps: i64, limits: Option<&StreamLimits>) -> Self {
        let mut ceiling = setting_mbps.max(1) * 1_000_000;
        if let Some(cap) = limits.and_then(StreamLimits::max_bitrate) {
            ceiling = ceiling.min(cap.max(100_000));
        }
        BitrateBounds { floor: Self::DEFAULT_FLOOR.min(ceiling), ceiling }
    }

    /// Where the adaptive controller starts: the usual 20 Mbps, clamped into the bounds.
    pub fn start(&self) -> i64 {
        Self::DEFAULT_START.max(self.floor).min(self.ceiling)
    }
}

/// The virtual display to create, in points (pixels = points × 2 when HiDPI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualDisplayMode {
    pub width: i64,
    pub height: i64,
    pub hi_dpi: bool,
    pub refresh_rate: i64,
}

impl VirtualDisplayMode {
    pub fn pixel_width(&self) -> i64 {
        if self.hi_dpi { self.width * 2 } else { self.width }
    }

    pub fn pixel_height(&self) -> i64 {
        if self.hi_dpi { self.height * 2 } else { self.height }
    }
}

/// Everything negotiated for one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPlan {
    pub codec: VideoCodec,
    pub encode_width: i64,
    pub encode_height: i64,
    pub fps: i64,
    pub display: VirtualDisplayMode,
}

impl StreamPlan {
    pub fn stream_config(&self) -> StreamConfig {
        StreamConfig::new(self.codec.as_str(), self.encode_width, self.encode_height, self.fps)
    }

    /// Display points per phone dp (1 dp = 1/160 inch): the display spans the
    /// phone's width. Scales the gesture engine's distance thresholds.
    pub fn points_per_dp(&self, screen: &ScreenInfo) -> f64 {
        let width_px = screen.width.max(screen.height) as f64;
        let density = screen.density_dpi.max(1) as f64 / 160.0;
        let width_dp = width_px / density;
        if width_dp > 0.0 { self.display.width as f64 / width_dp } else { 1.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPlanError {
    /// No codec both the client can decode and the host can encode.
    NoCommonCodec,
    InvalidScreen,
}

impl fmt::Display for StreamPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamPlanError::NoCommonCodec => f.write_str("no codec both sides support"),
            StreamPlanError::InvalidScreen => f.write_str("invalid screen size"),
        }
    }
}

impl std::error::Error for StreamPlanError {}

/// Chooses the virtual display mode, codec and encode size for a client.
#[derive(Debug)]
pub struct StreamPlanner;

impl StreamPlanner {
    /// Clients report decoder limits measured at this frame rate (PROTOCOL §5).
    const LIMIT_REFERENCE_FPS: i64 = 60;
    /// Lowest frame rate a client limit can ask for.
    pub const MIN_LIMITED_FPS: i64 = 5;

    /// `limits` (HELLO data saver) caps the stream's frame rate: fps = min(setting,
    /// phone refresh, limit). The virtual display keeps the uncapped refresh rate.
    /// Codecs the host has no encoder for (`encoders`) are skipped.
    pub fn plan(
        screen: &ScreenInfo,
        codecs: &[CodecInfo],
        settings: &StreamSettings,
        limits: Option<&StreamLimits>,
        encoders: &[EncoderCapability],
    ) -> Result<StreamPlan, StreamPlanError> {
        // Landscape physical pixels (the client promises width ≥ height; be tolerant).
        let phone_w = screen.width.max(screen.height);
        let phone_h = screen.width.min(screen.height);
        if phone_w < 64 || phone_h < 64 {
            return Err(StreamPlanError::InvalidScreen);
        }

        let scale = if StreamSettings::ALLOWED_SCALES.contains(&settings.resolution_scale) {
            settings.resolution_scale
        } else {
            1.0
        };
        let display_fps = Self::effective_fps(settings.fps, screen.refresh_rate);
        let fps = Self::limited_fps(display_fps, limits);

        // Virtual display: the scaled pixel size, as 2× points when HiDPI.
        let target_w = phone_w as f64 * scale;
        let target_h = phone_h as f64 * scale;
        let display = if settings.hi_dpi {
            VirtualDisplayMode {
                width: ((target_w / 2.0).round() as i64).max(1),
                height: ((target_h / 2.0).round() as i64).max(1),
                hi_dpi: true,
                refresh_rate: display_fps,
            }
        } else {
            VirtualDisplayMode {
                width: even_floor(target_w.round() as i64),
                height: even_floor(target_h.round() as i64),
                hi_dpi: false,
                refresh_rate: display_fps,
            }
        };
        let (want_w, want_h) = (display.pixel_width(), display.pixel_height());

        // Codec: the client's most preferred codec that fits the wanted size;
        // if none fits, the one with the largest budget (and we downscale).
        let candidates: Vec<(VideoCodec, (i64, i64))> = codecs
            .iter()
            .filter_map(|info| {
                let codec = VideoCodec::parse(&info.name)?;
                if info.max_width <= 0 || info.max_height <= 0 {
                    return None;
                }
                let encoder = encoders.iter().find(|e| e.codec == codec)?;
                let (lw, lh) = Self::scale_limit((info.max_width, info.max_height), fps);
                let bounds = (lw.max(lh).min(encoder.max_width), lw.min(lh).min(encoder.max_height));
                Some((codec, bounds))
            })
            .collect();
        let chosen = candidates
            .iter()
            .find(|(_, b)| fits(want_w, want_h, *b))
            // `rev` so ties go to the client's earlier preference, as in the Swift planner.
            .or_else(|| candidates.iter().rev().max_by_key(|(_, b)| b.0 * b.1))
            .ok_or(StreamPlanError::NoCommonCodec)?;
        let (w, h) = clamp(want_w, want_h, chosen.1);
        Ok(StreamPlan { codec: chosen.0, encode_width: w, encode_height: h, fps, display })
    }

    /// The fps setting, capped by the phone's refresh rate (never below 30).
    pub fn effective_fps(setting: i64, client_refresh: f64) -> i64 {
        let wanted = if StreamSettings::ALLOWED_FPS.contains(&setting) { setting } else { 60 };
        if !(client_refresh.is_finite() && client_refresh > 0.0) {
            return wanted;
        }
        let panel = (client_refresh.round() as i64).max(30);
        wanted.min(panel)
    }

    /// `fps` capped by the client's `maxFps` (never below [`Self::MIN_LIMITED_FPS`]).
    pub fn limited_fps(fps: i64, limits: Option<&StreamLimits>) -> i64 {
        match limits.and_then(|l| l.max_fps) {
            Some(cap) => fps.min(cap.max(Self::MIN_LIMITED_FPS)),
            None => fps,
        }
    }

    /// A decoder limit is measured at 60 fps. Faster streams spend the same
    /// decode budget on more frames, so shrink the area by 60/fps (ported
    /// from SideScreen's CodecLimits.scaleLimit).
    pub fn scale_limit(limit: (i64, i64), fps: i64) -> (i64, i64) {
        if fps <= Self::LIMIT_REFERENCE_FPS {
            return limit;
        }
        let k = (Self::LIMIT_REFERENCE_FPS as f64 / fps as f64).sqrt();
        (even_floor((limit.0 as f64 * k) as i64), even_floor((limit.1 as f64 * k) as i64))
    }
}

/// Whether a landscape size fits a landscape box.
fn fits(w: i64, h: i64, bounds: (i64, i64)) -> bool {
    w <= bounds.0 && h <= bounds.1
}

/// Scales (w, h) down to fit `bounds`, keeping the aspect ratio, with even dimensions.
fn clamp(width: i64, height: i64, bounds: (i64, i64)) -> (i64, i64) {
    if fits(width, height, bounds) {
        return (even_floor(width), even_floor(height));
    }
    let s = (bounds.0 as f64 / width as f64).min(bounds.1 as f64 / height as f64);
    // The epsilon keeps e.g. 2400 × (1920/2400) from truncating to 1919.
    let w = even_floor(bounds.0.min((width as f64 * s + 1e-6) as i64));
    let h = even_floor(bounds.1.min((height as f64 * s + 1e-6) as i64));
    (w, h)
}

fn even_floor(v: i64) -> i64 {
    (v & !1).max(2)
}
