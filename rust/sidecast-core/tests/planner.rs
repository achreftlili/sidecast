//! Port of `StreamPlannerTests.swift` and the fps-limit test of `RemoteAccessTests.swift`.
//! `EncoderCapability::APPLE_SILICON` reproduces the Mac's encoder ceilings, so
//! the expectations are the Swift ones; the last tests cover hosts with fewer codecs.

use sidecast_core::protocol::{CodecInfo, ScreenInfo, StreamConfig, StreamLimits};
use sidecast_core::streaming::{
    EncoderCapability, StreamPlan, StreamPlanError, StreamPlanner, StreamSettings, VideoCodec, VirtualDisplayMode,
};

fn pixel8() -> ScreenInfo {
    ScreenInfo::new(2400, 1080, 420, 120.0)
}

fn both_codecs() -> Vec<CodecInfo> {
    vec![CodecInfo::new("hevc", 3840, 2160), CodecInfo::new("h264", 1920, 1080)]
}

fn plan(screen: &ScreenInfo, codecs: &[CodecInfo], settings: &StreamSettings) -> Result<StreamPlan, StreamPlanError> {
    StreamPlanner::plan(screen, codecs, settings, None, &EncoderCapability::APPLE_SILICON)
}

fn limited(settings: &StreamSettings, screen: &ScreenInfo, limits: Option<StreamLimits>) -> StreamPlan {
    let codecs = [CodecInfo::new("hevc", 3840, 2160)];
    StreamPlanner::plan(screen, &codecs, settings, limits.as_ref(), &EncoderCapability::APPLE_SILICON).unwrap()
}

fn settings(hi_dpi: bool, resolution_scale: f64, fps: i64) -> StreamSettings {
    StreamSettings { hi_dpi, resolution_scale, fps, ..StreamSettings::default() }
}

fn size(p: &StreamPlan) -> [i64; 2] {
    [p.encode_width, p.encode_height]
}

#[test]
fn default_hidpi_uses_full_phone_pixels_at_two_x() {
    let p = plan(&pixel8(), &both_codecs(), &StreamSettings::default()).unwrap();
    assert_eq!(p.codec, VideoCodec::Hevc);
    assert_eq!(p.display, VirtualDisplayMode { width: 1200, height: 540, hi_dpi: true, refresh_rate: 60 });
    assert_eq!(p.display.pixel_width(), 2400);
    assert_eq!(p.encode_width, 2400);
    assert_eq!(p.encode_height, 1080);
    assert_eq!(p.fps, 60);
    assert_eq!(p.stream_config(), StreamConfig::new("hevc", 2400, 1080, 60));
}

#[test]
fn non_hidpi_and_scale() {
    let p = plan(&pixel8(), &both_codecs(), &settings(false, 0.75, 60)).unwrap();
    assert_eq!(p.display, VirtualDisplayMode { width: 1800, height: 810, hi_dpi: false, refresh_rate: 60 });
    assert_eq!(size(&p), [1800, 810]);

    let p = plan(&pixel8(), &both_codecs(), &settings(true, 0.5, 60)).unwrap();
    assert_eq!(p.display, VirtualDisplayMode { width: 600, height: 270, hi_dpi: true, refresh_rate: 60 });
    assert_eq!(size(&p), [1200, 540]);
}

#[test]
fn odd_sizes_become_even() {
    let odd = ScreenInfo::new(2341, 1081, 400, 60.0);
    let p = plan(&odd, &both_codecs(), &settings(false, 1.0, 60)).unwrap();
    assert_eq!(p.encode_width % 2, 0);
    assert_eq!(p.encode_height % 2, 0);
    assert_eq!([p.display.width, p.display.height], [2340, 1080]);
}

#[test]
fn falls_back_to_h264_when_hevc_cannot_decode_the_size() {
    let codecs = [CodecInfo::new("hevc", 1280, 720), CodecInfo::new("h264", 2560, 1440)];
    let p = plan(&pixel8(), &codecs, &StreamSettings::default()).unwrap();
    assert_eq!(p.codec, VideoCodec::H264);
    assert_eq!(size(&p), [2400, 1080]);
}

#[test]
fn clamps_to_largest_decoder_when_nothing_fits() {
    let codecs = [CodecInfo::new("h264", 1920, 1080)];
    let p = plan(&pixel8(), &codecs, &StreamSettings::default()).unwrap();
    assert_eq!(p.codec, VideoCodec::H264);
    assert_eq!(p.encode_width, 1920);
    assert_eq!(p.encode_height, 864); // 1920 × 1080/2400, aspect preserved
    // The virtual display keeps the phone's full mode; capture downsamples.
    assert_eq!(p.display.pixel_width(), 2400);
}

#[test]
fn portrait_limits_are_treated_as_landscape() {
    let codecs = [CodecInfo::new("hevc", 1080, 1920)];
    let p = plan(&pixel8(), &codecs, &StreamSettings::default()).unwrap();
    assert_eq!(size(&p), [1920, 864]);
}

#[test]
fn client_preference_order_wins() {
    let codecs = [CodecInfo::new("h264", 3840, 2160), CodecInfo::new("hevc", 3840, 2160)];
    assert_eq!(plan(&pixel8(), &codecs, &StreamSettings::default()).unwrap().codec, VideoCodec::H264);
}

#[test]
fn unknown_codecs_are_ignored_and_no_common_codec_throws() {
    let codecs = [CodecInfo::new("av1", 3840, 2160)];
    assert_eq!(plan(&pixel8(), &codecs, &StreamSettings::default()), Err(StreamPlanError::NoCommonCodec));
    assert!(plan(&pixel8(), &[], &StreamSettings::default()).is_err());
}

#[test]
fn fps_is_capped_by_panel_refresh_and_high_fps_shrinks_decoder_budget() {
    let slow_panel = ScreenInfo::new(2400, 1080, 420, 60.0);
    let p = plan(&slow_panel, &both_codecs(), &settings(true, 1.0, 120)).unwrap();
    assert_eq!(p.fps, 60);

    // 120 fps on a 120 Hz panel: a 1920×1080@60 H.264 budget shrinks by √(60/120).
    let codecs = [CodecInfo::new("h264", 1920, 1080)];
    let p = plan(&pixel8(), &codecs, &settings(true, 1.0, 120)).unwrap();
    assert_eq!(p.fps, 120);
    assert!(p.encode_width <= 1358);
    assert!(p.encode_height <= 764);
    assert_eq!(p.display.refresh_rate, 120);

    let p = plan(&pixel8(), &both_codecs(), &settings(true, 1.0, 30)).unwrap();
    assert_eq!(p.fps, 30);
}

#[test]
fn portrait_screen_is_normalized_to_landscape() {
    let portrait = ScreenInfo::new(1080, 2400, 420, 60.0);
    let p = plan(&portrait, &both_codecs(), &StreamSettings::default()).unwrap();
    assert_eq!(size(&p), [2400, 1080]);
}

#[test]
fn invalid_values_fall_back_to_defaults() {
    let p = plan(&pixel8(), &both_codecs(), &settings(true, 0.3, 75)).unwrap();
    assert_eq!(p.fps, 60);
    assert_eq!(p.encode_width, 2400);
    assert_eq!(
        plan(&ScreenInfo::new(0, 0, 1, 60.0), &both_codecs(), &StreamSettings::default()),
        Err(StreamPlanError::InvalidScreen)
    );
}

// MARK: - Client limits (RemoteAccessTests)

#[test]
fn fps_is_min_of_setting_panel_and_limit() {
    let mut s = StreamSettings { fps: 60, ..StreamSettings::default() };
    let p = limited(&s, &pixel8(), Some(StreamLimits::DATA_SAVER));
    assert_eq!(p.fps, 30);
    assert_eq!(p.display.refresh_rate, 60, "the virtual display keeps its normal refresh rate");
    assert_eq!(limited(&s, &pixel8(), None).fps, 60);
    assert_eq!(
        limited(&s, &pixel8(), Some(StreamLimits::new(None, Some(90)))).fps,
        60,
        "a looser limit changes nothing"
    );
    s.fps = 120;
    let slow_panel = ScreenInfo::new(2400, 1080, 420, 60.0);
    assert_eq!(limited(&s, &slow_panel, Some(StreamLimits::new(None, Some(90)))).fps, 60, "the panel still caps");
    assert_eq!(limited(&s, &pixel8(), Some(StreamLimits::new(None, Some(1)))).fps, StreamPlanner::MIN_LIMITED_FPS);
    assert_eq!(limited(&s, &pixel8(), Some(StreamLimits::new(Some(4000), None))).fps, 120, "bitrate-only limit");
}

// MARK: - Hosts with fewer encoders

#[test]
fn h264_only_host_picks_h264_from_a_hevc_first_client() {
    let p = StreamPlanner::plan(
        &pixel8(),
        &both_codecs(),
        &StreamSettings::default(),
        None,
        &[EncoderCapability::H264_LEVEL_5],
    )
    .unwrap();
    assert_eq!(p.codec, VideoCodec::H264);
    assert_eq!(size(&p), [1920, 864]);
}

#[test]
fn hevc_only_client_has_no_common_codec_with_an_h264_only_host() {
    let codecs = [CodecInfo::new("hevc", 3840, 2160)];
    assert_eq!(
        StreamPlanner::plan(&pixel8(), &codecs, &StreamSettings::default(), None, &[EncoderCapability::H264_LEVEL_5]),
        Err(StreamPlanError::NoCommonCodec)
    );
}

#[test]
fn points_per_dp_spans_the_phone_width() {
    // 2400 px at 420 dpi = 914.3 dp across a 1200 pt display.
    let p = plan(&pixel8(), &both_codecs(), &StreamSettings::default()).unwrap();
    assert!((p.points_per_dp(&pixel8()) - 1200.0 / (2400.0 / 2.625)).abs() < 1e-9);
}
