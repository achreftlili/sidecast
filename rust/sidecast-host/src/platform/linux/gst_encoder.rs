//! Hardware H.264 through GStreamer (the `gstreamer` cargo feature): VA-API
//! (`vah264enc`, `vah264lpenc`, the older `vaapih264enc`) or NVENC
//! (`nvh264enc`), whichever works on this computer, before OpenH264. The
//! GStreamer libraries are LGPL and linked dynamically; no GPL element (x264)
//! is ever used.
//!
//! Pipeline: `appsrc ! videoconvert ! <encoder> ! h264parse config-interval=-1
//! ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink`: Annex-B,
//! one access unit per buffer, SPS/PPS before every IDR. Low latency: no
//! B-frames, CBR, a keyframe at least every 2 s; one frame in, one out.

use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use sidecast_core::streaming::{EncoderCapability, VideoCodec};
use sidecast_server::backend::{
    BackendError, CapturedFrame, EncodedFrame, EncoderConfig, EncoderFactory, PixelData, VideoEncoder,
};

use super::pixels;

/// Tried in order; the first one that encodes a test frame wins.
const HARDWARE_ENCODERS: [&str; 4] = ["nvh264enc", "vah264enc", "vah264lpenc", "vaapih264enc"];

/// Creates encoders with one GStreamer element.
#[derive(Debug, Clone)]
pub struct GstEncoderFactory {
    element: String,
}

impl GstEncoderFactory {
    /// The first hardware encoder that works here, or `None` (OpenH264 then).
    pub fn detect() -> Option<GstEncoderFactory> {
        if let Err(error) = gst::init() {
            log::info!("encoder: GStreamer unavailable ({error})");
            return None;
        }
        HARDWARE_ENCODERS.into_iter().find_map(|element| Self::try_element(element).ok())
    }

    /// `element` if it encodes a small test frame.
    pub fn try_element(element: &str) -> Result<GstEncoderFactory, BackendError> {
        gst::init().map_err(|e| BackendError::encoder_failed(e.to_string()))?;
        if gst::ElementFactory::find(element).is_none() {
            return Err(BackendError::encoder_failed(format!("no GStreamer element {element}")));
        }
        let config = EncoderConfig {
            codec: VideoCodec::H264,
            width: 320,
            height: 192,
            fps: 30,
            bitrate: 1_000_000,
            keyframe_interval: 2.0,
        };
        let mut encoder = GstEncoder::new(element, &config)?;
        let frame = CapturedFrame {
            pts_micros: 0,
            width: 320,
            height: 192,
            pixels: PixelData::I420 {
                y: vec![80; 320 * 192],
                u: vec![128; 160 * 96],
                v: vec![128; 160 * 96],
                y_stride: 320,
                uv_stride: 160,
            },
        };
        match encoder.encode(&frame, true) {
            Ok(Some(encoded)) if encoded.is_keyframe => {
                log::info!("encoder: hardware H.264 through GStreamer {element}");
                Ok(GstEncoderFactory { element: element.to_owned() })
            }
            Ok(_) => Err(BackendError::encoder_failed(format!("{element} produced no keyframe"))),
            Err(error) => {
                log::debug!("encoder: {element} does not work here: {error}");
                Err(error)
            }
        }
    }
}

impl EncoderFactory for GstEncoderFactory {
    fn name(&self) -> &str {
        &self.element
    }

    fn capabilities(&self) -> Vec<EncoderCapability> {
        vec![EncoderCapability::new(VideoCodec::H264, 4096, 2304)]
    }

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, BackendError> {
        if config.codec != VideoCodec::H264 {
            return Err(BackendError::encoder_failed(format!("{} encodes H.264 only", self.element)));
        }
        Ok(Box::new(GstEncoder::new(&self.element, config)?))
    }
}

fn failed(what: &str, error: impl std::fmt::Display) -> BackendError {
    BackendError::encoder_failed(format!("GStreamer {what}: {error}"))
}

/// Sets a property from its string form when the element has it (names differ between encoders).
fn set(element: &gst::Element, name: &str, value: &str) {
    if element.find_property(name).is_some() {
        element.set_property_from_str(name, value);
    }
}

/// Bitrate in the element's unit (kbit/s, except OpenH264's bit/s).
fn bitrate_value(element: &str, bits_per_second: u32) -> String {
    if element == "openh264enc" { bits_per_second.to_string() } else { (bits_per_second / 1000).max(1).to_string() }
}

/// Low-latency settings, by element family.
fn configure(encoder: &gst::Element, element: &str, config: &EncoderConfig) {
    let gop = ((config.keyframe_interval * config.fps.max(1) as f64).round() as u32).max(1).to_string();
    set(encoder, "bitrate", &bitrate_value(element, config.bitrate));
    match element {
        "nvh264enc" => {
            set(encoder, "rc-mode", "cbr");
            set(encoder, "bframes", "0");
            set(encoder, "b-frames", "0");
            set(encoder, "gop-size", &gop);
            set(encoder, "zerolatency", "true");
            set(encoder, "preset", "low-latency-hq");
        }
        "vaapih264enc" => {
            set(encoder, "rate-control", "cbr");
            set(encoder, "max-bframes", "0");
            set(encoder, "keyframe-period", &gop);
        }
        "openh264enc" => {
            set(encoder, "rate-control", "bitrate");
            set(encoder, "gop-size", &gop);
            set(encoder, "complexity", "low");
        }
        _ => {
            // vah264enc / vah264lpenc
            set(encoder, "rate-control", "cbr");
            set(encoder, "b-frames", "0");
            set(encoder, "key-int-max", &gop);
            set(encoder, "target-usage", "7");
        }
    }
}

pub struct GstEncoder {
    element: String,
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    encoder: gst::Element,
    config: EncoderConfig,
    format: Option<&'static str>,
    frames: u64,
}

impl std::fmt::Debug for GstEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GstEncoder").field("element", &self.element).finish_non_exhaustive()
    }
}

impl GstEncoder {
    pub fn new(element: &str, config: &EncoderConfig) -> Result<Self, BackendError> {
        gst::init().map_err(|e| failed("init", e))?;
        let pipeline = gst::Pipeline::new();
        let appsrc = gst_app::AppSrc::builder()
            .is_live(true)
            .format(gst::Format::Time)
            .stream_type(gst_app::AppStreamType::Stream)
            .build();
        let make = |name: &str| gst::ElementFactory::make(name).build().map_err(|e| failed(name, e));
        let convert = make("videoconvert")?;
        let encoder = make(element)?;
        configure(&encoder, element, config);
        let parse = make("h264parse")?;
        set(&parse, "config-interval", "-1");
        let caps =
            gst::Caps::builder("video/x-h264").field("stream-format", "byte-stream").field("alignment", "au").build();
        let appsink = gst_app::AppSink::builder().caps(&caps).sync(false).build();
        pipeline
            .add_many([appsrc.upcast_ref(), &convert, &encoder, &parse, appsink.upcast_ref()])
            .map_err(|e| failed("pipeline", e))?;
        gst::Element::link_many([appsrc.upcast_ref(), &convert, &encoder, &parse, appsink.upcast_ref()])
            .map_err(|e| failed("link", e))?;
        Ok(GstEncoder {
            element: element.to_owned(),
            pipeline,
            appsrc,
            appsink,
            encoder,
            config: config.clone(),
            format: None,
            frames: 0,
        })
    }

    /// (Re)starts the pipeline for frames in `format`.
    fn ensure_format(&mut self, format: &'static str) -> Result<(), BackendError> {
        if self.format == Some(format) {
            return Ok(());
        }
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", format)
            .field("width", self.config.width as i32)
            .field("height", self.config.height as i32)
            .field("framerate", gst::Fraction::new(self.config.fps.max(1) as i32, 1))
            .build();
        let _ = self.pipeline.set_state(gst::State::Null);
        self.appsrc.set_caps(Some(&caps));
        self.pipeline.set_state(gst::State::Playing).map_err(|e| failed("start", e))?;
        self.format = Some(format);
        Ok(())
    }
}

impl VideoEncoder for GstEncoder {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>, BackendError> {
        if frame.width != self.config.width || frame.height != self.config.height {
            return Err(BackendError::encoder_failed(format!(
                "frame is {}x{}, encoder {}x{}",
                frame.width, frame.height, self.config.width, self.config.height
            )));
        }
        let (format, bytes) = pixels::pack_for_gstreamer(frame)
            .ok_or_else(|| BackendError::encoder_failed("frame planes do not match its size"))?;
        self.ensure_format(format)?;
        let first = self.frames == 0;
        if force_keyframe {
            // What gst_video_event_new_upstream_force_key_unit builds; the encoder
            // base class turns it into an IDR for the next frame.
            let request = gst::Structure::builder("GstForceKeyUnit")
                .field("running-time", u64::MAX)
                .field("all-headers", true)
                .field("count", 0u32)
                .build();
            if let Some(pad) = self.encoder.static_pad("src") {
                pad.send_event(gst::event::CustomUpstream::new(request));
            }
        }
        let frame_time = 1_000_000_000 / self.config.fps.max(1) as u64;
        let mut buffer = gst::Buffer::from_mut_slice(bytes);
        if let Some(buffer) = buffer.get_mut() {
            buffer.set_pts(gst::ClockTime::from_nseconds(self.frames * frame_time));
            buffer.set_duration(gst::ClockTime::from_nseconds(frame_time));
        }
        self.frames += 1;
        self.appsrc.push_buffer(buffer).map_err(|e| failed("push", e))?;
        // Hardware encoders take a moment for their first frame.
        let wait = if first { Duration::from_secs(2) } else { Duration::from_millis(500) };
        let Some(sample) = self.appsink.try_pull_sample(gst::ClockTime::from_nseconds(wait.as_nanos() as u64)) else {
            return Ok(None);
        };
        let Some(buffer) = sample.buffer() else { return Ok(None) };
        let map = buffer.map_readable().map_err(|e| failed("map", e))?;
        Ok(Some(EncodedFrame {
            data: map.as_slice().to_vec(),
            is_keyframe: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
        }))
    }

    fn set_bitrate(&mut self, bits_per_second: u32) {
        if bits_per_second == 0 || bits_per_second == self.config.bitrate {
            return;
        }
        self.config.bitrate = bits_per_second;
        set(&self.encoder, "bitrate", &bitrate_value(&self.element, bits_per_second));
    }
}

impl Drop for GstEncoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::annexb::inspect_access_unit;

    fn frame(shade: u8) -> CapturedFrame {
        CapturedFrame {
            pts_micros: 0,
            width: 320,
            height: 192,
            pixels: PixelData::I420 {
                y: vec![shade; 320 * 192],
                u: vec![128; 160 * 96],
                v: vec![128; 160 * 96],
                y_stride: 320,
                uv_stride: 160,
            },
        }
    }

    #[test]
    fn bitrate_units_per_element() {
        assert_eq!(bitrate_value("vah264enc", 20_000_000), "20000");
        assert_eq!(bitrate_value("nvh264enc", 500), "1");
        assert_eq!(bitrate_value("openh264enc", 20_000_000), "20000000");
    }

    /// The wrapper around a real GStreamer encoder. CI machines have no GPU, so
    /// this uses GStreamer's OpenH264 element (BSD library, LGPL plugin; tests
    /// only) and is skipped where the plugin is not installed.
    #[test]
    fn encodes_annexb_access_units_with_forced_keyframes() {
        let factory = match GstEncoderFactory::try_element("openh264enc") {
            Ok(factory) => factory,
            // CI installs the plugin and sets this, so the test cannot silently skip there.
            Err(error) if std::env::var_os("SIDECAST_REQUIRE_GST_OPENH264").is_some() => panic!("{error}"),
            Err(error) => {
                eprintln!("skipped: {error}");
                return;
            }
        };
        let config = EncoderConfig {
            codec: VideoCodec::H264,
            width: 320,
            height: 192,
            fps: 30,
            bitrate: 2_000_000,
            keyframe_interval: 2.0,
        };
        assert!(factory.create(&EncoderConfig { codec: VideoCodec::Hevc, ..config.clone() }).is_err());
        let mut encoder = factory.create(&config).unwrap();
        let first = encoder.encode(&frame(40), true).unwrap().expect("one access unit per frame");
        assert!(first.is_keyframe);
        let info = inspect_access_unit(VideoCodec::H264, &first.data, true).unwrap();
        assert!(info.has_parameter_sets && info.has_random_access_slice, "SPS/PPS + IDR in Annex-B");
        encoder.set_bitrate(1_000_000);
        let mut delta_seen = false;
        for shade in 41..46 {
            if let Some(next) = encoder.encode(&frame(shade), false).unwrap() {
                inspect_access_unit(VideoCodec::H264, &next.data, next.is_keyframe).unwrap();
                delta_seen |= !next.is_keyframe;
            }
        }
        assert!(delta_seen, "frames between keyframes are deltas");
        let forced = encoder.encode(&frame(120), true).unwrap().unwrap();
        assert!(forced.is_keyframe, "a forced keyframe is an IDR");
        inspect_access_unit(VideoCodec::H264, &forced.data, true).unwrap();
        // Wrong sizes are errors, not panics.
        let mut small = frame(0);
        small.width = 160;
        assert!(encoder.encode(&small, false).is_err());
    }
}
