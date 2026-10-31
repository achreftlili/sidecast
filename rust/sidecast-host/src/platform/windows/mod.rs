//! Windows backends (ROADMAP P2, DECISIONS Q37):
//!
//! - display: a virtual monitor from the signed Virtual Display Driver
//!   (`screen.rs`, `vdd.rs`), or an existing monitor; DXGI Desktop
//!   Duplication with the pointer drawn in, converted to NV12 (`capture.rs`);
//! - encoders: Media Foundation's hardware H.264 MFT, then its software one
//!   (`encoder.rs`); OpenH264 from the server crate comes last;
//! - input: `SendInput` (`input.rs`);
//! - keep awake: `SetThreadExecutionState` (`power.rs`);
//! - microphone: a virtual audio cable such as VB-CABLE, when installed (`mic.rs`).
//!
//! The pure parts (key and shortcut mapping, coordinates, pixel conversion,
//! encoder output, the driver's settings file, cable names) are compiled and
//! unit-tested on every OS; the rest only on Windows.

// The pure modules are only used by the Windows ones; elsewhere only their tests run.
#![cfg_attr(not(windows), allow(dead_code))]

pub mod bitstream;
pub mod cable;
pub mod convert;
pub mod keymap;
pub mod mapping;
pub mod vdd_config;

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod display;
#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
pub mod input;
#[cfg(windows)]
pub mod mic;
#[cfg(windows)]
pub mod power;
#[cfg(windows)]
pub mod screen;
#[cfg(windows)]
pub mod util;
#[cfg(windows)]
pub mod vdd;

#[cfg(windows)]
pub use self::platform::{backends, diagnose};

#[cfg(windows)]
mod platform {
    use std::sync::Arc;

    use sidecast_server::backend::{BackendError, Backends, MicSink};

    use super::vdd_config::RELEASES_URL;
    use super::{capture, display, encoder, input, mic, power, screen, util, vdd};
    use crate::platform::PlatformOptions;

    /// The Windows backends.
    pub fn backends(options: &PlatformOptions) -> Result<Backends, BackendError> {
        util::init_process();
        let display = Arc::new(screen::WindowsDisplay::new(options.monitor.clone()));
        let mut backends = Backends::new(display, encoder::factories());
        backends.input = Some(Arc::new(input::SendInputBackend));
        backends.power = Some(Arc::new(power::ExecutionState));
        backends.mic = mic::VirtualCableMic::find().map(|m| Arc::new(m) as Arc<dyn MicSink>);
        if backends.mic.is_none() {
            log::info!(
                "mic: no virtual audio cable (e.g. VB-CABLE) installed, so the phone's microphone is not offered"
            );
        }
        let status = vdd::Status::probe();
        log::info!("display: Virtual Display Driver {}", status.describe());
        if !status.usable() {
            log::info!(
                "display: without it a phone mirrors or extends onto an existing monitor; get it from {RELEASES_URL}"
            );
        }
        Ok(backends)
    }

    /// What this computer offers the host (`sidecast-host --diagnose`).
    pub fn diagnose() -> String {
        util::init_process();
        let mut out = String::new();
        out.push_str("Monitors (GDI):\n");
        for m in display::monitors() {
            out.push_str(&format!(
                "  {}{}{}\n",
                m.describe(),
                if m.is_virtual_display() { ", Virtual Display Driver" } else { "" },
                if m.attached {
                    format!(" at ({}, {}) {} Hz", m.rect.left, m.rect.top, m.refresh)
                } else {
                    String::new()
                }
            ));
        }
        out.push_str("Outputs (DXGI Desktop Duplication):\n");
        for output in capture::outputs() {
            out.push_str(&format!("  {output}\n"));
        }
        out.push_str(&format!("Virtual Display Driver: {}\n", vdd::Status::probe().describe()));
        if let Some((path, _, settings)) = vdd::read_settings() {
            let modes: Vec<String> = settings.modes.iter().map(ToString::to_string).collect();
            out.push_str(&format!(
                "  {}: {} monitor(s), resolutions {}, global refresh rates {:?}\n",
                path.display(),
                settings.monitor_count.unwrap_or(1),
                modes.join(", "),
                settings.global_refresh_rates
            ));
        }
        out.push_str("Media Foundation H.264 encoders:\n");
        for line in encoder::describe() {
            out.push_str(&format!("  {line}\n"));
        }
        let playback = mic::playback_names();
        out.push_str(&format!(
            "Audio playback devices: {}\n",
            if playback.is_empty() { "none".to_owned() } else { playback.join(", ") }
        ));
        out.push_str(&format!(
            "Phone microphone: {}\n",
            if mic::VirtualCableMic::find().is_some() {
                "via the virtual audio cable"
            } else {
                "not offered (no virtual audio cable)"
            }
        ));
        out
    }
}
