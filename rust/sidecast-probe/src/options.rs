//! Command-line options, compatible with the Swift `sidecast-probe` where they overlap.

use std::path::PathBuf;

use sidecast_core::protocol::{CodecInfo, DEFAULT_PORT, HostPlatform, InputMode, KeyEvent, StreamLimits};
use sidecast_core::streaming::VideoCodec;

use crate::gestures::ProbeGesture;

/// TEXT / KEY to send once frames flow, in command-line order.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardInput {
    Text(String),
    Key(KeyEvent),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub host: String,
    pub port: u16,
    pub seconds: f64,
    /// Pairing codes to try in order.
    pub codes: Vec<String>,
    /// Read the pairing code from this file once the host shows one (hosts
    /// with random codes, e.g. a real Linux host in an end-to-end test).
    pub code_file: Option<PathBuf>,
    pub state_path: Option<PathBuf>,
    /// Write the last decoded picture's luma here as a PGM image.
    pub save_frame: Option<PathBuf>,
    pub codec: Option<VideoCodec>,
    pub width: i64,
    pub height: i64,
    pub refresh_rate: f64,
    pub keyframe_request: bool,
    /// Send the default input script (a tap, TEXT, two KEYs) once frames flow.
    pub input: bool,
    pub keyboard: Vec<KeyboardInput>,
    pub gestures: Vec<ProbeGesture>,
    pub mic: bool,
    pub input_mode: Option<InputMode>,
    pub network: Option<String>,
    pub limits: Option<StreamLimits>,
    pub stall_after: Option<f64>,
    /// Succeed only if authentication fails with this reason (and the matching BYE).
    pub expect_auth_failure: Option<String>,
    /// Fail unless SERVER_HELLO announces this platform.
    pub expect_platform: Option<HostPlatform>,
    /// Fail unless the stream uses this codec.
    pub expect_codec: Option<VideoCodec>,
    /// Decode H.264 with OpenH264 (when built with the feature).
    pub decode: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            host: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            seconds: 5.0,
            codes: Vec::new(),
            code_file: None,
            state_path: None,
            save_frame: None,
            codec: None,
            width: 2400,
            height: 1080,
            refresh_rate: 60.0,
            keyframe_request: false,
            input: true,
            keyboard: Vec::new(),
            gestures: Vec::new(),
            mic: false,
            input_mode: None,
            network: None,
            limits: None,
            stall_after: None,
            expect_auth_failure: None,
            expect_platform: None,
            expect_codec: None,
            decode: true,
        }
    }
}

pub const USAGE: &str = "\
Usage: sidecast-probe --host H --port P [--seconds N] [--code 123456[,654321…]] [--state FILE]
                      [--code-file FILE] [--codec hevc|h264] [--width W --height H] [--refresh HZ]
                      [--keyframe-request] [--no-input] [--type TEXT]… [--key N[:MODS]]…
                      [--gesture NAME]… [--input-mode touch|trackpad] [--mic]
                      [--network metered|unmetered] [--limits KBPS:FPS] [--stall-after S]
                      [--expect-auth-failure REASON] [--expect-platform macos|windows|linux]
                      [--expect-codec hevc|h264] [--no-decode] [--save-frame FILE.pgm]

A scripted fake phone: pairs (--code) or authenticates (--state), negotiates a
stream, acks and checks every frame (Annex-B structure; H.264 is also decoded
when built with OpenH264), sends input, prints statistics, and exits 1 on any
protocol violation. --code-file waits (up to 30 s) for the code a host shows to
appear in FILE, for hosts with random codes; --save-frame writes the last
decoded picture's luma as a PGM image.

Gestures: tap2 swipe3-left swipe3-right swipe3-up swipe3-down swipe4-left tap3
scroll2 trackpad-move trackpad-tap trackpad-doubletap-drag";

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut o = Options::default();
        let mut it = args.into_iter();
        fn value(flag: &str, it: &mut dyn Iterator<Item = String>) -> Result<String, String> {
            it.next().ok_or_else(|| format!("{flag} needs a value"))
        }
        fn number<T: std::str::FromStr>(flag: &str, it: &mut dyn Iterator<Item = String>) -> Result<T, String> {
            value(flag, it)?.parse().map_err(|_| format!("invalid {flag}"))
        }
        let codec = |raw: String| VideoCodec::parse(&raw).ok_or_else(|| "codec must be hevc or h264".to_owned());
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--host" => o.host = value(&arg, &mut it)?,
                "--port" => o.port = number::<u16>(&arg, &mut it).and_then(nonzero)?,
                "--seconds" => o.seconds = number::<f64>(&arg, &mut it).and_then(positive)?,
                "--code" => o.codes = value(&arg, &mut it)?.split(',').map(str::to_owned).collect(),
                "--code-file" => o.code_file = Some(PathBuf::from(value(&arg, &mut it)?)),
                "--save-frame" => o.save_frame = Some(PathBuf::from(value(&arg, &mut it)?)),
                "--state" => o.state_path = Some(PathBuf::from(value(&arg, &mut it)?)),
                "--codec" => o.codec = Some(codec(value(&arg, &mut it)?)?),
                "--width" => o.width = number(&arg, &mut it)?,
                "--height" => o.height = number(&arg, &mut it)?,
                "--refresh" => o.refresh_rate = number(&arg, &mut it)?,
                "--keyframe-request" => o.keyframe_request = true,
                "--no-input" => o.input = false,
                "--type" => {
                    let text = value(&arg, &mut it)?;
                    if text.is_empty() {
                        return Err("--type needs non-empty text".into());
                    }
                    o.keyboard.push(KeyboardInput::Text(text));
                }
                "--key" => o.keyboard.push(KeyboardInput::Key(parse_key(&value(&arg, &mut it)?)?)),
                "--gesture" => {
                    let name = value(&arg, &mut it)?;
                    o.gestures.push(ProbeGesture::parse(&name).ok_or_else(|| format!("unknown --gesture {name}"))?);
                }
                "--input-mode" => {
                    let raw = value(&arg, &mut it)?;
                    o.input_mode =
                        Some(InputMode::parse(&raw).ok_or("--input-mode must be touch or trackpad".to_owned())?);
                }
                "--mic" => o.mic = true,
                "--network" => {
                    let network = value(&arg, &mut it)?;
                    if !matches!(network.as_str(), "metered" | "unmetered") {
                        return Err("--network must be metered or unmetered".into());
                    }
                    o.network = Some(network);
                }
                "--limits" => {
                    let spec = value(&arg, &mut it)?;
                    let parsed =
                        spec.split_once(':').and_then(|(k, f)| Some((k.parse::<i64>().ok()?, f.parse::<i64>().ok()?)));
                    match parsed {
                        Some((kbps, fps)) if kbps > 0 && fps > 0 => {
                            o.limits = Some(StreamLimits::new(Some(kbps), Some(fps)))
                        }
                        _ => return Err("--limits must be KBPS:FPS, e.g. 4000:30".into()),
                    }
                }
                "--stall-after" => o.stall_after = Some(number::<f64>(&arg, &mut it).and_then(positive)?),
                "--expect-auth-failure" => o.expect_auth_failure = Some(value(&arg, &mut it)?),
                "--expect-platform" => {
                    let raw = value(&arg, &mut it)?;
                    o.expect_platform = Some(HostPlatform::parse(&raw).ok_or(format!("unknown platform {raw}"))?);
                }
                "--expect-codec" => o.expect_codec = Some(codec(value(&arg, &mut it)?)?),
                "--no-decode" => o.decode = false,
                "--help" | "-h" => return Err(USAGE.into()),
                other => return Err(format!("unknown argument {other}\n{USAGE}")),
            }
        }
        Ok(o)
    }

    /// The decoders this fake phone claims (order = preference), like the Swift probe.
    pub fn codecs(&self) -> Vec<CodecInfo> {
        let hevc = CodecInfo::new("hevc", 3840, 2160);
        let h264 = CodecInfo::new("h264", 1920, 1080);
        match self.codec {
            Some(VideoCodec::Hevc) => vec![hevc],
            Some(VideoCodec::H264) => vec![h264],
            None => vec![hevc, h264],
        }
    }
}

fn nonzero(port: u16) -> Result<u16, String> {
    if port > 0 { Ok(port) } else { Err("invalid --port".into()) }
}

fn positive(v: f64) -> Result<f64, String> {
    if v > 0.0 && v.is_finite() { Ok(v) } else { Err("value must be positive".into()) }
}

/// `N` or `N:MODS`, both decimal bytes (unknown values allowed, to exercise the host).
fn parse_key(spec: &str) -> Result<KeyEvent, String> {
    let (key, mods) = spec.split_once(':').unwrap_or((spec, "0"));
    match (key.parse::<u8>(), mods.parse::<u8>()) {
        (Ok(key), Ok(mods)) => Ok(KeyEvent::raw(key, mods)),
        _ => Err(format!("--key must be N or N:MODS (0…255 each), got {spec}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_the_swift_probe_flags() {
        let o = parse(&[
            "--host",
            "10.0.0.2",
            "--port",
            "47895",
            "--code",
            "111111,123456",
            "--seconds",
            "3",
            "--codec",
            "h264",
            "--key",
            "6:1",
            "--type",
            "é",
            "--gesture",
            "tap2",
            "--limits",
            "4000:30",
            "--expect-platform",
            "linux",
        ])
        .unwrap();
        assert_eq!((o.host.as_str(), o.port, o.seconds), ("10.0.0.2", 47895, 3.0));
        assert_eq!(o.codes, ["111111", "123456"]);
        assert_eq!(o.codecs(), [CodecInfo::new("h264", 1920, 1080)]);
        assert_eq!(o.keyboard, [KeyboardInput::Key(KeyEvent::raw(6, 1)), KeyboardInput::Text("é".into())]);
        assert_eq!(o.limits, Some(StreamLimits::DATA_SAVER));
        assert_eq!(o.expect_platform, Some(HostPlatform::Linux));
    }

    #[test]
    fn rejects_bad_values() {
        for bad in [
            &["--port", "0"][..],
            &["--seconds", "-1"],
            &["--codec", "av1"],
            &["--key", "300"],
            &["--limits", "4000"],
            &["--x"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
