//! Notifications for the host's user interface (tray app, CLI log).

use std::time::Duration;

use sidecast_core::protocol::InputMode;
use sidecast_core::security::PairingCode;
use sidecast_core::streaming::StreamPlan;

use crate::backend::SessionId;

/// Per-second stream statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamStats {
    pub fps: f64,
    pub mbps: f64,
    /// Adaptive bitrate target, bits per second.
    pub target_bitrate: i64,
    pub dropped: u32,
    pub keyframes: u32,
    /// Seconds.
    pub smoothed_rtt: Option<f64>,
    pub min_rtt: Option<f64>,
    pub in_flight: usize,
}

impl StreamStats {
    pub fn log_line(&self) -> String {
        let ms = |t: Option<f64>| t.map_or("-".to_owned(), |t| format!("{:.1}ms", t * 1000.0));
        format!(
            "stream: fps={:.0} Mbps={:.2} target={:.1}Mbps dropped={} keyframes={} srtt={} minrtt={} inflight={}",
            self.fps,
            self.mbps,
            self.target_bitrate as f64 / 1e6,
            self.dropped,
            self.keyframes,
            ms(self.smoothed_rtt),
            ms(self.min_rtt),
            self.in_flight
        )
    }
}

/// What happened on the server.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Listening {
        port: u16,
    },
    /// Show `code` to the user ("Enter this code on <device_name>").
    PairingStarted {
        session: SessionId,
        device_name: String,
        code: PairingCode,
        expires_in: Duration,
    },
    PairingEnded {
        session: SessionId,
        paired: bool,
    },
    StreamingStarted {
        device_name: String,
        plan: StreamPlan,
        data_saver: bool,
    },
    StreamingStopped,
    InputModeChanged(InputMode),
    Stats(StreamStats),
    PairedDevicesChanged,
}
