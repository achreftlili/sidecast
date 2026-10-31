//! Command-line options, compatible with the Mac app's test-mode flags.

use std::fmt;
use std::path::PathBuf;

use sidecast_core::security::PairingCode;
use sidecast_core::streaming::StreamSettings;

#[derive(Debug, Default)]
pub struct Options {
    /// Headless test mode: synthetic frames, input and audio only logged.
    pub test_pattern: bool,
    pub data_dir: Option<PathBuf>,
    pub port: Option<u16>,
    /// Honoured only with `--test-pattern`: a fixed code must never reach a real install.
    pub test_pairing_code: Option<PairingCode>,
    pub mdns: bool,
    pub fps: Option<i64>,
    pub max_mbps: Option<i64>,
    pub name: Option<String>,
    /// Stream this existing monitor instead of a virtual display (overrides the saved setting).
    pub monitor: Option<String>,
    /// Print what this computer offers the host and exit.
    pub diagnose: bool,
    /// Test mode: send MIC_CONTROL active:true as soon as a mic-capable phone streams.
    pub test_mic_active: bool,
    /// No tray icon or notifications (Linux), only the console.
    pub no_tray: bool,
    pub verbose: bool,
    pub help: bool,
}

pub const USAGE: &str = "\
Usage: sidecast-host [--data-dir DIR] [--port N] [--name NAME] [--monitor NAME] [--no-mdns] [--no-tray] [--verbose]
       sidecast-host --diagnose
       sidecast-host --test-pattern --data-dir DIR [--port N] [--test-pairing-code 123456]
                     [--no-mdns] [--fps 30|60|120] [--max-mbps N] [--test-mic-active] [--verbose]

  --test-pattern        stream a synthetic animated pattern; touches, keys, text and
                        microphone audio are logged instead of injected
  --data-dir DIR        where the server id, TLS identity and pairings live
  --port N              TCP port (default 47800)
  --test-pairing-code C every pairing uses this code (test pattern only)
  --no-mdns             do not advertise _sidecast._tcp (alias: --no-bonjour)
  --name NAME           name shown on the phone (default: this computer's name)
  --monitor NAME        stream this existing monitor instead of a virtual display
                        (Windows: a device name such as \\\\.\\DISPLAY2; Linux on GNOME: a
                        connector such as DP-1; or `primary`)
  --diagnose            list what this computer offers the host (monitors, encoders,
                        virtual display, audio), then exit
  --no-tray             no tray icon or pairing notifications (Linux); the console still works
  --fps N, --max-mbps N stream settings for this run
  --test-mic-active     ask a mic-capable phone for its microphone at once (test pattern)
  --verbose             debug logging, including every TOUCH MOVE";

#[derive(Debug, PartialEq, Eq)]
pub struct OptionError(pub String);

impl fmt::Display for OptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Options {
    /// Parses the arguments after the program name.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, OptionError> {
        let mut o = Options { mdns: true, ..Options::default() };
        let mut pairing_code = None;
        let mut it = args.into_iter();
        let value = |flag: &str, it: &mut dyn Iterator<Item = String>| {
            it.next().ok_or_else(|| OptionError(format!("{flag} needs a value")))
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--test-pattern" => o.test_pattern = true,
                "--data-dir" => o.data_dir = Some(PathBuf::from(value(&arg, &mut it)?)),
                "--port" => {
                    o.port = Some(
                        value(&arg, &mut it)?
                            .parse()
                            .ok()
                            .filter(|p| *p > 0)
                            .ok_or_else(|| OptionError("invalid --port".into()))?,
                    )
                }
                "--test-pairing-code" => pairing_code = Some(value(&arg, &mut it)?),
                "--no-mdns" | "--no-bonjour" => o.mdns = false,
                "--fps" => {
                    let fps = value(&arg, &mut it)?.parse().ok().filter(|f| StreamSettings::ALLOWED_FPS.contains(f));
                    o.fps = Some(fps.ok_or_else(|| OptionError("--fps must be 30, 60 or 120".into()))?);
                }
                "--max-mbps" => {
                    let mbps =
                        value(&arg, &mut it)?.parse().ok().filter(|m| StreamSettings::BITRATE_RANGE_MBPS.contains(m));
                    o.max_mbps = Some(mbps.ok_or_else(|| OptionError("--max-mbps must be 10…100".into()))?);
                }
                "--name" => o.name = Some(value(&arg, &mut it)?),
                "--monitor" => {
                    let monitor = value(&arg, &mut it)?.trim().to_owned();
                    if monitor.is_empty() {
                        return Err(OptionError("--monitor needs a monitor name".into()));
                    }
                    o.monitor = Some(monitor);
                }
                "--diagnose" => o.diagnose = true,
                "--test-mic-active" => o.test_mic_active = true,
                "--no-tray" => o.no_tray = true,
                "--verbose" | "-v" => o.verbose = true,
                "--help" | "-h" => o.help = true,
                other => return Err(OptionError(format!("unknown argument {other}\n{USAGE}"))),
            }
        }
        if let Some(code) = pairing_code {
            let code =
                PairingCode::new(&code).ok_or_else(|| OptionError("--test-pairing-code must be 6 digits".into()))?;
            if !o.test_pattern {
                return Err(OptionError("--test-pairing-code is only allowed with --test-pattern".into()));
            }
            o.test_pairing_code = Some(code);
        }
        if o.test_pattern && o.data_dir.is_none() {
            return Err(OptionError("--test-pattern requires --data-dir".into()));
        }
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, OptionError> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn test_pattern_flags() {
        let o = parse(&[
            "--test-pattern",
            "--data-dir",
            "/tmp/x",
            "--port",
            "47895",
            "--test-pairing-code",
            "123456",
            "--no-bonjour",
            "--fps",
            "30",
            "--verbose",
        ])
        .unwrap();
        assert!(o.test_pattern && o.verbose && !o.mdns && !o.no_tray);
        assert!(parse(&["--no-tray"]).unwrap().no_tray);
        assert_eq!(o.port, Some(47895));
        assert_eq!(o.fps, Some(30));
        assert_eq!(o.test_pairing_code.unwrap().digits(), "123456");
    }

    #[test]
    fn host_flags() {
        let o = parse(&["--monitor", r"\\.\DISPLAY2", "--diagnose"]).unwrap();
        assert_eq!(o.monitor.as_deref(), Some(r"\\.\DISPLAY2"));
        assert!(o.diagnose && !o.test_pattern);
        assert!(parse(&["--monitor", " "]).is_err());
    }

    #[test]
    fn rejects_unsafe_or_invalid_combinations() {
        assert!(parse(&["--test-pattern"]).is_err(), "test pattern needs its own data dir");
        assert!(parse(&["--test-pairing-code", "123456"]).is_err(), "fixed codes only in test mode");
        assert!(parse(&["--test-pattern", "--data-dir", "d", "--test-pairing-code", "12345"]).is_err());
        assert!(parse(&["--port", "0"]).is_err());
        assert!(parse(&["--fps", "75"]).is_err());
        assert!(parse(&["--max-mbps", "500"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--port"]).is_err());
    }
}
