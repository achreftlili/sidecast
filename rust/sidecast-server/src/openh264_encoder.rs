//! Software H.264 with Cisco's OpenH264 (BSD-2-Clause, compiled from source
//! by the `openh264` crate): the fallback encoder every host has, and the
//! encoder of test-pattern mode. Constrained Baseline, which every phone
//! decoder handles; BT.709 limited range signalled in the VUI like the Mac's
//! encoder. Patent note: see THIRD_PARTY_NOTICES.md.

use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig as OpenH264Config, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::YUVSlices;
use sidecast_core::streaming::{EncoderCapability, VideoCodec};

use crate::backend::{
    BackendError, CapturedFrame, EncodedFrame, EncoderConfig, EncoderFactory, PixelData, VideoEncoder,
};

/// Creates [`OpenH264Encoder`]s.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenH264Factory;

impl EncoderFactory for OpenH264Factory {
    fn name(&self) -> &str {
        "openh264"
    }

    /// OpenH264 encodes up to level 5.2 / 3840×2160.
    fn capabilities(&self) -> Vec<EncoderCapability> {
        vec![EncoderCapability::new(VideoCodec::H264, 3840, 2160)]
    }

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, BackendError> {
        if config.codec != VideoCodec::H264 {
            return Err(BackendError::encoder_failed(format!("OpenH264 cannot encode {}", config.codec)));
        }
        Ok(Box::new(OpenH264Encoder::new(config)?))
    }
}

/// One OpenH264 encoder instance.
pub struct OpenH264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
    bitrate: u32,
    /// I420 scratch for NV12 / BGRA input.
    scratch: Vec<u8>,
}

impl std::fmt::Debug for OpenH264Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenH264Encoder").field("width", &self.width).field("height", &self.height).finish()
    }
}

impl OpenH264Encoder {
    pub fn new(config: &EncoderConfig) -> Result<Self, BackendError> {
        let fps = config.fps.max(1);
        let settings = OpenH264Config::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(config.bitrate))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .profile(Profile::Baseline)
            .complexity(Complexity::Low)
            // OpenH264's bitrate control only works with frame skipping on; a
            // skipped frame releases its flow-control slot like a dropped one.
            .skip_frames(true)
            // Not supported for screen content (OpenH264 turns them off with a warning).
            .adaptive_quantization(false)
            .background_detection(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                (config.keyframe_interval * fps as f64).round().max(1.0) as u32,
            ))
            .vui(VuiConfig::bt709());
        let api = openh264::OpenH264API::from_source();
        let encoder = Encoder::with_api_config(api, settings)
            .map_err(|e| BackendError::encoder_failed(format!("OpenH264: {e}")))?;
        Ok(OpenH264Encoder {
            encoder,
            width: config.width as usize,
            height: config.height as usize,
            bitrate: config.bitrate,
            scratch: Vec::new(),
        })
    }

    fn check_planes(&self, y: usize, u: usize, y_stride: usize, uv_stride: usize) -> Result<(), BackendError> {
        let (w, h) = (self.width, self.height);
        let ok = y_stride >= w && uv_stride >= w / 2 && y == h * y_stride && u == (h / 2) * uv_stride;
        ok.then_some(()).ok_or_else(|| {
            BackendError::encoder_failed(format!("frame planes do not match {w}x{h} (strides {y_stride}/{uv_stride})"))
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>, BackendError> {
        if frame.width as usize != self.width || frame.height as usize != self.height {
            return Err(BackendError::encoder_failed(format!(
                "frame is {}x{}, encoder {}x{}",
                frame.width, frame.height, self.width, self.height
            )));
        }
        let (w, h) = (self.width, self.height);
        let (y_len, uv_len) = (w * h, (w / 2) * (h / 2));
        match &frame.pixels {
            PixelData::I420 { .. } => {}
            PixelData::Nv12 { y, uv, y_stride, uv_stride } => {
                self.scratch.resize(y_len + 2 * uv_len, 0);
                nv12_to_i420(y, uv, *y_stride, *uv_stride, w, h, &mut self.scratch);
            }
            PixelData::Bgra { data, stride } => {
                self.scratch.resize(y_len + 2 * uv_len, 0);
                bgra_to_i420(data, *stride, w, h, &mut self.scratch);
            }
            PixelData::Native(_) => {
                return Err(BackendError::encoder_failed("OpenH264 needs CPU pixels, not a native surface"));
            }
        }
        let source = match &frame.pixels {
            PixelData::I420 { y, u, v, y_stride, uv_stride } => {
                self.check_planes(y.len(), u.len(), *y_stride, *uv_stride)?;
                if v.len() != u.len() {
                    return Err(BackendError::encoder_failed("U and V planes differ in size"));
                }
                YUVSlices::new((y, u, v), (w, h), (*y_stride, *uv_stride, *uv_stride))
            }
            _ => {
                let (y, uv) = self.scratch.split_at(y_len);
                let (u, v) = uv.split_at(uv_len);
                YUVSlices::new((y, u, v), (w, h), (w, w / 2, w / 2))
            }
        };
        if force_keyframe {
            self.encoder.force_intra_frame();
        }
        let bitstream = self.encoder.encode(&source).map_err(|e| BackendError::encoder_failed(e.to_string()))?;
        let frame_type = bitstream.frame_type();
        if matches!(frame_type, FrameType::Skip | FrameType::Invalid) {
            return Ok(None);
        }
        let data = bitstream.to_vec();
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(EncodedFrame { data, is_keyframe: matches!(frame_type, FrameType::IDR | FrameType::I) }))
    }

    fn set_bitrate(&mut self, bits_per_second: u32) {
        if bits_per_second == 0 || bits_per_second == self.bitrate {
            return;
        }
        let raising = bits_per_second > self.bitrate;
        self.bitrate = bits_per_second;
        // The ceiling allows short bursts (keyframes) at 1.5× the target, like the Mac's DataRateLimits.
        let target = bits_per_second.min(i32::MAX as u32) as i32;
        let ceiling = (bits_per_second as u64 * 3 / 2).min(i32::MAX as u64) as i32;
        // OpenH264 rejects a target above the current maximum: raise the maximum first, lower it last.
        let options = if raising {
            [(openh264_sys2::ENCODER_OPTION_MAX_BITRATE, ceiling), (openh264_sys2::ENCODER_OPTION_BITRATE, target)]
        } else {
            [(openh264_sys2::ENCODER_OPTION_BITRATE, target), (openh264_sys2::ENCODER_OPTION_MAX_BITRATE, ceiling)]
        };
        for (option, value) in options {
            let mut info = openh264_sys2::SBitrateInfo { iLayer: openh264_sys2::SPATIAL_LAYER_ALL, iBitrate: value };
            // SAFETY: the encoder is initialised (the crate initialises it on the
            // first encode; before that OpenH264 returns an error code we ignore),
            // and `info` outlives the call. Bitrate options do not change anything
            // the crate relies on (it only tracks dimensions).
            #[allow(unsafe_code)]
            let status = unsafe { self.encoder.raw_api().set_option(option, (&raw mut info).cast()) };
            if status != 0 {
                log::debug!("openh264: bitrate option {option} = {value} not applied ({status})");
            }
        }
    }
}

/// NV12 → I420 (tightly packed into `out`).
fn nv12_to_i420(y: &[u8], uv: &[u8], y_stride: usize, uv_stride: usize, w: usize, h: usize, out: &mut [u8]) {
    let (out_y, out_uv) = out.split_at_mut(w * h);
    let (out_u, out_v) = out_uv.split_at_mut((w / 2) * (h / 2));
    for row in 0..h {
        out_y[row * w..row * w + w].copy_from_slice(&y[row * y_stride..row * y_stride + w]);
    }
    for row in 0..h / 2 {
        let line = &uv[row * uv_stride..];
        for col in 0..w / 2 {
            out_u[row * (w / 2) + col] = line[2 * col];
            out_v[row * (w / 2) + col] = line[2 * col + 1];
        }
    }
}

/// BGRA → I420, BT.709 limited range (what the VUI signals), chroma averaged over 2×2.
fn bgra_to_i420(bgra: &[u8], stride: usize, w: usize, h: usize, out: &mut [u8]) {
    let (out_y, out_uv) = out.split_at_mut(w * h);
    let (out_u, out_v) = out_uv.split_at_mut((w / 2) * (h / 2));
    let luma = |b: i32, g: i32, r: i32| ((47 * r + 157 * g + 16 * b + 128) >> 8) + 16;
    for row in 0..h {
        let line = &bgra[row * stride..];
        for col in 0..w {
            let p = &line[4 * col..4 * col + 3];
            out_y[row * w + col] = luma(p[0] as i32, p[1] as i32, p[2] as i32).clamp(16, 235) as u8;
        }
    }
    for row in 0..h / 2 {
        for col in 0..w / 2 {
            let (mut b, mut g, mut r) = (0i32, 0i32, 0i32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = &bgra[(2 * row + dy) * stride + 4 * (2 * col + dx)..];
                b += p[0] as i32;
                g += p[1] as i32;
                r += p[2] as i32;
            }
            let (b, g, r) = (b / 4, g / 4, r / 4);
            let u = ((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
            out_u[row * (w / 2) + col] = u.clamp(16, 240) as u8;
            out_v[row * (w / 2) + col] = v.clamp(16, 240) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::annexb::inspect_access_unit;

    fn gray_frame(w: u32, h: u32, shade: u8) -> CapturedFrame {
        let (w, h) = (w as usize, h as usize);
        CapturedFrame {
            pts_micros: 0,
            width: w as u32,
            height: h as u32,
            pixels: PixelData::I420 {
                y: vec![shade; w * h],
                u: vec![128; w * h / 4],
                v: vec![128; w * h / 4],
                y_stride: w,
                uv_stride: w / 2,
            },
        }
    }

    #[test]
    fn encodes_annexb_with_parameter_sets_on_keyframes() {
        let config = EncoderConfig {
            codec: VideoCodec::H264,
            width: 320,
            height: 192,
            fps: 30,
            bitrate: 2_000_000,
            keyframe_interval: 2.0,
        };
        let mut encoder = OpenH264Factory.create(&config).unwrap();
        let first = encoder.encode(&gray_frame(320, 192, 40), true).unwrap().unwrap();
        assert!(first.is_keyframe);
        let info = inspect_access_unit(VideoCodec::H264, &first.data, true).unwrap();
        assert!(info.has_parameter_sets && info.has_random_access_slice);
        encoder.set_bitrate(1_000_000);
        let second = encoder.encode(&gray_frame(320, 192, 41), false).unwrap().unwrap(); // no scene change
        assert!(!second.is_keyframe);
        inspect_access_unit(VideoCodec::H264, &second.data, false).unwrap();
        encoder.set_bitrate(4_000_000);
        let forced = encoder.encode(&gray_frame(320, 192, 120), true).unwrap().unwrap();
        assert!(forced.is_keyframe);
        inspect_access_unit(VideoCodec::H264, &forced.data, true).unwrap();
    }

    #[test]
    fn converts_bgra_and_nv12() {
        let config = EncoderConfig {
            codec: VideoCodec::H264,
            width: 64,
            height: 64,
            fps: 30,
            bitrate: 500_000,
            keyframe_interval: 2.0,
        };
        let mut encoder = OpenH264Factory.create(&config).unwrap();
        let bgra = CapturedFrame {
            pts_micros: 0,
            width: 64,
            height: 64,
            pixels: PixelData::Bgra { data: vec![200; 64 * 4 * 64], stride: 64 * 4 },
        };
        assert!(encoder.encode(&bgra, true).unwrap().unwrap().is_keyframe);
        let nv12 = CapturedFrame {
            pts_micros: 0,
            width: 64,
            height: 64,
            pixels: PixelData::Nv12 { y: vec![16; 64 * 64], uv: vec![128; 64 * 32], y_stride: 64, uv_stride: 64 },
        };
        assert!(encoder.encode(&nv12, false).unwrap().is_some());
        // Wrong sizes are errors, not panics.
        assert!(encoder.encode(&gray_frame(32, 32, 0), false).is_err());
        assert!(OpenH264Factory.create(&EncoderConfig { codec: VideoCodec::Hevc, ..config }).is_err());
    }

    #[test]
    fn bgra_white_and_black_are_video_range() {
        let mut out = vec![0u8; 2 * 2 + 2];
        bgra_to_i420(&[255; 16], 8, 2, 2, &mut out);
        assert_eq!(&out[..4], &[235; 4]);
        assert_eq!(&out[4..], &[128, 128]);
        bgra_to_i420(&[0; 16], 8, 2, 2, &mut out);
        assert_eq!(&out[..4], &[16; 4]);
    }
}
