//! H.264 encoding with Media Foundation: the GPU's encoder MFT (NVIDIA,
//! AMD, Intel; asynchronous) first, then Microsoft's software H.264 encoder
//! MFT (synchronous, licensed with Windows), then OpenH264 from the server
//! crate as the last fallback.
//!
//! Settings for a live screen: low-latency mode, constant bitrate (changed
//! live by the adaptive controller), no B-frames, a keyframe at least every
//! 2 s (the server also forces one on request), Main profile like the Mac
//! (some low-end phone decoders reject High's 8×8 transform), BT.709 limited
//! range. Input is NV12 in system memory; output goes through
//! `bitstream::normalize` (4-byte start codes, SPS/PPS on every keyframe).
//!
//! A new encoder must encode a test frame before it is used: a driver's MFT
//! that misbehaves (no output, a hang, invalid data) fails over to the next
//! encoder instead of breaking the stream.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use sidecast_core::streaming::{EncoderCapability, VideoCodec};
use sidecast_server::backend::{
    BackendError, CapturedFrame, EncodedFrame, EncoderConfig, EncoderFactory, PixelData, VideoEncoder,
};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQualityVsSpeed, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame,
    CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate, IMFAttributes, IMFMediaEventGenerator, IMFMediaType, IMFSample,
    IMFShutdown, IMFTransform, METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT,
    MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_TRANSFORM_ASYNC,
    MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
    MFNominalRange_16_235, MFSTARTUP_LITE, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_FRIENDLY_NAME_Attribute,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    MFVideoPrimaries_BT709, MFVideoTransFunc_709, MFVideoTransferMatrix_BT709, eAVEncCommonRateControlMode_CBR,
    eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface, PWSTR};

use super::bitstream;
use super::convert::packed_nv12;
use super::util::com_thread;

/// Media Foundation's encoders, hardware first.
pub fn factories() -> Vec<Arc<dyn EncoderFactory>> {
    vec![Arc::new(MediaFoundationFactory::new(Kind::Hardware)), Arc::new(MediaFoundationFactory::new(Kind::Software))]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The GPU vendors' encoder MFTs (asynchronous).
    Hardware,
    /// Synchronous software MFTs (Microsoft's H.264 encoder).
    Software,
}

/// Creates H.264 encoders from the MFTs of one kind.
#[derive(Debug)]
pub struct MediaFoundationFactory {
    kind: Kind,
    /// Friendly names of the MFTs found, enumerated once.
    found: OnceLock<Vec<String>>,
}

impl MediaFoundationFactory {
    pub fn new(kind: Kind) -> Self {
        MediaFoundationFactory { kind, found: OnceLock::new() }
    }

    /// The encoder MFTs of this kind on this computer.
    pub fn available(&self) -> &[String] {
        self.found.get_or_init(|| match enumerate(self.kind) {
            Ok(list) => list.iter().map(friendly_name).collect(),
            Err(error) => {
                log::info!("encoder: Media Foundation {:?} encoders unavailable: {error}", self.kind);
                Vec::new()
            }
        })
    }
}

impl EncoderFactory for MediaFoundationFactory {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Hardware => "media-foundation-hardware",
            Kind::Software => "media-foundation-software",
        }
    }

    fn capabilities(&self) -> Vec<EncoderCapability> {
        if self.available().is_empty() { Vec::new() } else { vec![EncoderCapability::H264_LEVEL_5] }
    }

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, BackendError> {
        if config.codec != VideoCodec::H264 {
            return Err(BackendError::encoder_failed(format!(
                "Media Foundation encodes H.264 here, not {}",
                config.codec
            )));
        }
        let mut errors = Vec::new();
        for activate in enumerate(self.kind).map_err(BackendError::encoder_failed)? {
            let name = friendly_name(&activate);
            match MfEncoder::open(&activate, &name, config).and_then(|mut e| e.self_test().map(|()| e)) {
                Ok(encoder) => {
                    log::info!("encoder: {name} ({})", if encoder.events.is_some() { "hardware" } else { "software" });
                    return Ok(Box::new(encoder));
                }
                Err(error) => {
                    log::warn!("encoder: {name} cannot be used: {error}");
                    errors.push(format!("{name}: {error}"));
                }
            }
        }
        Err(BackendError::encoder_failed(if errors.is_empty() {
            "no Media Foundation H.264 encoder".to_owned()
        } else {
            errors.join("; ")
        }))
    }
}

fn startup() -> Result<(), String> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            com_thread();
            // SAFETY: no pointers; Media Foundation stays started for the process.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|e| {
                format!("Media Foundation is not available ({e}; Windows N editions need the Media Feature Pack)")
            })
        })
        .clone()
}

/// The H.264 encoder MFTs of one kind, best first (the registry's merit order).
fn enumerate(kind: Kind) -> Result<Vec<IMFActivate>, String> {
    startup()?;
    com_thread();
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let flags = match kind {
        Kind::Hardware => MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        Kind::Software => MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: valid in/out pointers; on success `list` is a CoTaskMem array of `count` activates.
    unsafe { MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count) }
        .map_err(|e| format!("cannot list encoders: {e}"))?;
    if list.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: the array holds `count` initialized entries; we take ownership of
    // each (moving it out leaves `None`) and free the array once.
    let activates = unsafe {
        let entries = std::slice::from_raw_parts_mut(list, count as usize);
        let taken = entries.iter_mut().filter_map(Option::take).collect();
        CoTaskMemFree(Some(list.cast()));
        taken
    };
    Ok(activates)
}

fn friendly_name(activate: &IMFActivate) -> String {
    let mut text = PWSTR::null();
    let mut length = 0u32;
    // SAFETY: on success `text` is a CoTaskMem string we free once.
    unsafe {
        if activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut length).is_ok() {
            let name = text.to_string().unwrap_or_default();
            CoTaskMemFree(Some(text.0.cast()));
            return name;
        }
    }
    "unnamed H.264 encoder".to_owned()
}

/// One Media Foundation H.264 encoder.
struct MfEncoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    /// Asynchronous (hardware) MFTs report through events.
    events: Option<IMFMediaEventGenerator>,
    name: String,
    width: usize,
    height: usize,
    frame_duration: i64,
    bitrate: u32,
    /// The MFT allocates its output samples.
    provides_samples: bool,
    output_size: u32,
    sequence_header: Vec<u8>,
    /// Asynchronous MFTs: input requests not answered yet.
    need_input: u32,
    /// Outputs not returned yet, oldest first. Every output must reach the
    /// phone (a lost P-frame corrupts the picture until the next keyframe),
    /// so a late one is returned by a later call instead of being dropped.
    outputs: VecDeque<Vec<u8>>,
    /// Sample time of the next input (100 ns units): steady, whatever the capture timestamps do.
    next_time: i64,
    input: Vec<u8>,
}

// SAFETY: Media Foundation transforms and their interfaces are free-threaded
// COM objects in the process's multithreaded apartment (see
// `util::init_process`); the encoder is used by one thread at a time (it is
// created on a blocking thread and then moved to the server's encoder thread).
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    fn open(activate: &IMFActivate, name: &str, config: &EncoderConfig) -> Result<MfEncoder, String> {
        com_thread();
        fn fail(what: &'static str) -> impl Fn(windows::core::Error) -> String {
            move |e| format!("{what}: {e}")
        }
        // SAFETY: COM calls on live interfaces with valid arguments throughout.
        unsafe {
            let transform: IMFTransform = activate.ActivateObject().map_err(fail("cannot create it"))?;
            let attributes: Option<IMFAttributes> = transform.GetAttributes().ok();
            let is_async = attributes.as_ref().and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok()).unwrap_or(0) != 0;
            if let Some(attributes) = &attributes {
                if is_async {
                    attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(fail("cannot unlock it"))?;
                }
                let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let events = if is_async {
                Some(transform.cast::<IMFMediaEventGenerator>().map_err(fail("no event generator"))?)
            } else {
                None
            };
            let codec_api = transform.cast::<ICodecAPI>().ok();
            let fps = config.fps.max(1);
            let mut encoder = MfEncoder {
                transform,
                codec_api,
                events,
                name: name.to_owned(),
                width: config.width as usize,
                height: config.height as usize,
                frame_duration: 10_000_000 / fps as i64,
                bitrate: config.bitrate.max(100_000),
                provides_samples: false,
                output_size: 0,
                sequence_header: Vec::new(),
                need_input: 0,
                outputs: VecDeque::new(),
                next_time: 0,
                input: Vec::new(),
            };
            // Rate control before the media types: some encoders only read it then.
            encoder.set_codec_value(&CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR.0 as u32);
            encoder.set_codec_value(&CODECAPI_AVEncCommonMeanBitRate, encoder.bitrate);
            encoder.set_codec_bool(&CODECAPI_AVLowLatencyMode, true);
            encoder.set_codec_value(&CODECAPI_AVEncMPVDefaultBPictureCount, 0);
            let gop = (config.keyframe_interval * fps as f64).round().max(1.0) as u32;
            encoder.set_codec_value(&CODECAPI_AVEncMPVGOPSize, gop);
            if !is_async {
                // The software encoder must keep up on the CPU: favour speed (0 = fastest, 100 = best).
                encoder.set_codec_value(&CODECAPI_AVEncCommonQualityVsSpeed, 33);
            }

            let output = encoder.media_type(MFVideoFormat_H264, fps).map_err(fail("output type"))?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, encoder.bitrate).map_err(fail("output type"))?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32).map_err(fail("output type"))?;
            encoder.transform.SetOutputType(0, &output, 0).map_err(fail("output type rejected"))?;
            let input = encoder.media_type(MFVideoFormat_NV12, fps).map_err(fail("input type"))?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, encoder.width as u32).map_err(fail("input type"))?;
            encoder.transform.SetInputType(0, &input, 0).map_err(fail("NV12 input rejected"))?;

            encoder.read_output_info().map_err(fail("output stream"))?;
            encoder.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(fail("cannot start"))?;
            encoder.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(fail("cannot start"))?;
            Ok(encoder)
        }
    }

    /// A video media type with this encoder's size, frame rate and colours.
    fn media_type(&self, subtype: GUID, fps: u32) -> windows::core::Result<IMFMediaType> {
        let pack = |high: u64, low: u64| (high << 32) | low;
        // SAFETY: COM calls on a new media type with valid GUID pointers.
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(self.width as u64, self.height as u64))?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps as u64, 1))?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // BT.709 limited range, so every phone decoder picks the same matrix.
            t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
            t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            Ok(t)
        }
    }

    /// Output sample allocation and the sequence header of the current output type.
    fn read_output_info(&mut self) -> windows::core::Result<()> {
        // SAFETY: COM calls on the live transform.
        unsafe {
            let info = self.transform.GetOutputStreamInfo(0)?;
            let provides = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
            self.provides_samples = info.dwFlags & provides != 0;
            self.output_size = info.cbSize.max((self.width * self.height * 3 / 2) as u32);
            self.sequence_header = self
                .transform
                .GetOutputCurrentType(0)
                .ok()
                .and_then(|t| {
                    let size = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
                    let mut blob = vec![0u8; size as usize];
                    t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None).ok()?;
                    Some(blob)
                })
                .unwrap_or_default();
        }
        Ok(())
    }

    fn set_codec_value(&self, key: &GUID, value: u32) {
        self.set_codec(key, &VARIANT::from(value));
    }

    fn set_codec_bool(&self, key: &GUID, value: bool) {
        self.set_codec(key, &VARIANT::from(value));
    }

    fn set_codec(&self, key: &GUID, value: &VARIANT) {
        if let Some(api) = &self.codec_api {
            // SAFETY: `key` and `value` are valid for the call.
            if let Err(error) = unsafe { api.SetValue(key, value) } {
                log::debug!("encoder: {} ignores codec setting {key:?}: {error}", self.name);
            }
        }
    }

    /// Encodes a mid-grey keyframe; the encoder must produce a valid IDR.
    fn self_test(&mut self) -> Result<(), String> {
        let (w, h) = (self.width, self.height);
        let frame = CapturedFrame {
            pts_micros: 0,
            width: w as u32,
            height: h as u32,
            pixels: PixelData::Nv12 { y: vec![126; w * h], uv: vec![128; w * h / 2], y_stride: w, uv_stride: w },
        };
        // A buffering encoder may want a few frames before its first output.
        let mut result = Err("no output for the test frames".to_owned());
        for attempt in 0..4 {
            if let Some(unit) = self.encode_unit(&frame, attempt == 0)? {
                result = if unit.is_keyframe { Ok(()) } else { Err("the first frame is not an IDR keyframe".into()) };
                break;
            }
        }
        // The stream starts with a forced keyframe, so test output is never needed.
        self.outputs.clear();
        result
    }

    fn encode_unit(
        &mut self,
        frame: &CapturedFrame,
        force_keyframe: bool,
    ) -> Result<Option<bitstream::AccessUnit>, String> {
        if frame.width as usize != self.width || frame.height as usize != self.height {
            return Err(format!("frame is {}x{}, encoder {}x{}", frame.width, frame.height, self.width, self.height));
        }
        packed_nv12(&frame.pixels, self.width, self.height, &mut self.input)?;
        let sample = self.input_sample(self.next_time).map_err(|e| format!("input sample: {e}"))?;
        self.next_time += self.frame_duration;
        if force_keyframe {
            self.set_codec_value(&CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
        if self.events.is_some() {
            self.encode_async(&sample)?;
        } else {
            self.encode_sync(&sample)?;
        }
        while let Some(data) = self.outputs.pop_front() {
            if let Some(unit) = bitstream::normalize(&data, &self.sequence_header) {
                return Ok(Some(unit));
            }
        }
        Ok(None)
    }

    fn input_sample(&self, time: i64) -> windows::core::Result<IMFSample> {
        // SAFETY: the buffer is locked only while copying `input` into it,
        // and it is at least that long.
        unsafe {
            let buffer = MFCreateMemoryBuffer(self.input.len() as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(self.input.as_ptr(), data, self.input.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(self.input.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(self.frame_duration)?;
            Ok(sample)
        }
    }

    /// Synchronous MFT: input, then every output it has (queued).
    fn encode_sync(&mut self, sample: &IMFSample) -> Result<(), String> {
        // SAFETY: COM call on the live transform with a live sample.
        match unsafe { self.transform.ProcessInput(0, sample, 0) } {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                // It still holds output: take it, then retry once.
                self.drain_sync()?;
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, sample, 0) }.map_err(|e| format!("input refused: {e}"))?;
            }
            Err(error) => return Err(format!("input refused: {error}")),
        }
        self.drain_sync()
    }

    fn drain_sync(&mut self) -> Result<(), String> {
        while let Some(data) = self.pull_output()? {
            self.outputs.push_back(data);
        }
        Ok(())
    }

    /// Asynchronous MFT: wait for an input request (queueing any output on
    /// the way), give the frame, then wait for an output. An MFT that asks
    /// for more input before producing one is buffering: after a short grace
    /// period the call returns, and the output comes with a later call.
    fn encode_async(&mut self, sample: &IMFSample) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.need_input == 0 {
            match self.next_event(deadline)? {
                Event::NeedInput => self.need_input += 1,
                Event::HaveOutput => self.queue_async_output()?,
                Event::Other => {}
            }
        }
        // SAFETY: COM call on the live transform with a live sample.
        unsafe { self.transform.ProcessInput(0, sample, 0) }.map_err(|e| format!("input refused: {e}"))?;
        self.need_input -= 1;
        let queued = self.outputs.len();
        let mut asked_for_more_at: Option<Instant> = None;
        while self.outputs.len() == queued {
            // Low-latency encoders answer within a frame; a buffering one gets 30 ms.
            let wait_until = asked_for_more_at.map_or(deadline, |t| (t + Duration::from_millis(30)).min(deadline));
            match self.next_event(wait_until) {
                Ok(Event::HaveOutput) => self.queue_async_output()?,
                Ok(Event::NeedInput) => {
                    self.need_input += 1;
                    asked_for_more_at.get_or_insert_with(Instant::now);
                }
                Ok(Event::Other) => {}
                Err(_) if asked_for_more_at.is_some() => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn queue_async_output(&mut self) -> Result<(), String> {
        if let Some(data) = self.pull_output()? {
            self.outputs.push_back(data);
        }
        Ok(())
    }

    fn next_event(&mut self, deadline: Instant) -> Result<Event, String> {
        let events = self.events.as_ref().ok_or("not an asynchronous encoder")?;
        loop {
            // SAFETY: COM call on the live event generator; NO_WAIT never blocks.
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    // SAFETY: COM calls on the event we own.
                    let (kind, status) = unsafe { (event.GetType().unwrap_or(0), event.GetStatus()) };
                    if let Ok(status) = status
                        && status.is_err()
                    {
                        return Err(format!("encoder event {kind} failed: {}", windows::core::Error::from(status)));
                    }
                    return Ok(match kind {
                        k if k == METransformNeedInput.0 as u32 => Event::NeedInput,
                        k if k == METransformHaveOutput.0 as u32 => Event::HaveOutput,
                        _ => Event::Other,
                    });
                }
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Err("the encoder did not answer in time".into());
                    }
                    // Sub-millisecond: std's sleep uses high-resolution timers on Windows 10+.
                    std::thread::sleep(Duration::from_micros(250));
                }
                Err(error) => return Err(format!("encoder events: {error}")),
            }
        }
    }

    /// One output sample's bytes; `None` when the MFT needs more input.
    fn pull_output(&mut self) -> Result<Option<Vec<u8>>, String> {
        for _ in 0..4 {
            let sample = if self.provides_samples {
                None
            } else {
                // SAFETY: a new sample with one buffer of the size the MFT asked for.
                Some(unsafe {
                    let sample = MFCreateSample().map_err(|e| format!("output sample: {e}"))?;
                    let buffer = MFCreateMemoryBuffer(self.output_size).map_err(|e| format!("output buffer: {e}"))?;
                    sample.AddBuffer(&buffer).map_err(|e| format!("output sample: {e}"))?;
                    sample
                })
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            // SAFETY: one output buffer for stream 0; we take back ownership
            // of the sample and events right after the call.
            let result = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };
            // SAFETY: each ManuallyDrop field is taken exactly once.
            let (sample, _events) =
                unsafe { (ManuallyDrop::take(&mut buffers[0].pSample), ManuallyDrop::take(&mut buffers[0].pEvents)) };
            match result {
                Ok(()) => return sample.map(|s| sample_bytes(&s)).transpose().map_err(|e| format!("output: {e}")),
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // The encoder changed its output type (e.g. after the first
                    // frame): accept its new type and read the stream info again.
                    // SAFETY: COM calls on the live transform.
                    unsafe {
                        let t =
                            self.transform.GetOutputAvailableType(0, 0).map_err(|e| format!("stream change: {e}"))?;
                        self.transform.SetOutputType(0, &t, 0).map_err(|e| format!("stream change: {e}"))?;
                    }
                    self.read_output_info().map_err(|e| format!("stream change: {e}"))?;
                }
                Err(error) => return Err(format!("no output: {error}")),
            }
        }
        Err("the encoder keeps changing its output type".into())
    }
}

enum Event {
    NeedInput,
    HaveOutput,
    Other,
}

fn sample_bytes(sample: &IMFSample) -> windows::core::Result<Vec<u8>> {
    // SAFETY: the buffer is locked only while copying `length` bytes out of it.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        buffer.Lock(&mut data, None, Some(&mut length))?;
        let bytes = std::slice::from_raw_parts(data, length as usize).to_vec();
        buffer.Unlock()?;
        Ok(bytes)
    }
}

impl VideoEncoder for MfEncoder {
    fn encode(&mut self, frame: &CapturedFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>, BackendError> {
        let unit = self
            .encode_unit(frame, force_keyframe)
            .map_err(|e| BackendError::encoder_failed(format!("{}: {e}", self.name)))?;
        Ok(unit.map(|u| EncodedFrame { data: u.data, is_keyframe: u.is_keyframe }))
    }

    fn set_bitrate(&mut self, bits_per_second: u32) {
        if bits_per_second == 0 || bits_per_second == self.bitrate {
            return;
        }
        self.bitrate = bits_per_second;
        self.set_codec_value(&CODECAPI_AVEncCommonMeanBitRate, bits_per_second);
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // SAFETY: COM calls on the live transform; errors are irrelevant at teardown.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            // Asynchronous MFTs hold threads and event queues until shut down.
            if self.events.is_some()
                && let Ok(shutdown) = self.transform.cast::<IMFShutdown>()
            {
                let _ = shutdown.Shutdown();
            }
        }
    }
}

/// The encoders `--diagnose` reports.
pub fn describe() -> Vec<String> {
    [Kind::Hardware, Kind::Software]
        .into_iter()
        .map(|kind| match enumerate(kind) {
            Ok(list) if list.is_empty() => format!("{kind:?}: none"),
            Ok(list) => format!("{kind:?}: {}", list.iter().map(friendly_name).collect::<Vec<_>>().join(", ")),
            Err(error) => format!("{kind:?}: {error}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::annexb::inspect_access_unit;

    fn frame(w: usize, h: usize, shade: u8, pts: u64) -> CapturedFrame {
        CapturedFrame {
            pts_micros: pts,
            width: w as u32,
            height: h as u32,
            pixels: PixelData::Bgra { data: vec![shade; w * h * 4], stride: w * 4 },
        }
    }

    /// Encodes with every Media Foundation encoder this computer has (on CI:
    /// Microsoft's software encoder; hardware ones on a real PC). Skips when
    /// Media Foundation is missing (Windows N, Server without the feature).
    #[test]
    fn media_foundation_encoders_produce_valid_h264() {
        crate::platform::windows::util::init_process();
        for kind in [Kind::Hardware, Kind::Software] {
            let factory = MediaFoundationFactory::new(kind);
            if factory.available().is_empty() {
                eprintln!("skipped: no Media Foundation {kind:?} H.264 encoder here");
                continue;
            }
            assert_eq!(factory.capabilities(), [EncoderCapability::H264_LEVEL_5]);
            let config = EncoderConfig {
                codec: VideoCodec::H264,
                width: 640,
                height: 360,
                fps: 30,
                bitrate: 2_000_000,
                keyframe_interval: 2.0,
            };
            let mut encoder = factory.create(&config).expect("an available encoder opens and passes its self-test");
            let (mut outputs, mut keyframes) = (0, 0);
            for i in 0..30u64 {
                if i == 10 {
                    encoder.set_bitrate(1_000_000);
                }
                let force = i == 0 || i == 20;
                let encoded = encoder.encode(&frame(640, 360, (i * 8) as u8, i * 33_333), force).unwrap();
                if let Some(encoded) = encoded {
                    let info = inspect_access_unit(VideoCodec::H264, &encoded.data, encoded.is_keyframe).unwrap();
                    assert!(encoded.data.starts_with(&[0, 0, 0, 1]));
                    if encoded.is_keyframe {
                        assert!(info.has_parameter_sets && info.has_random_access_slice);
                        keyframes += 1;
                    }
                    if i == 0 || i == 20 {
                        assert!(encoded.is_keyframe, "{kind:?}: frame {i} was forced to be a keyframe");
                    }
                    outputs += 1;
                }
            }
            assert!(outputs >= 25, "{kind:?}: one access unit per frame (got {outputs} of 30)");
            assert!(keyframes >= 2, "{kind:?}: forced keyframes ({keyframes})");
            assert!(factory.create(&EncoderConfig { codec: VideoCodec::Hevc, ..config }).is_err());
        }
    }
}
