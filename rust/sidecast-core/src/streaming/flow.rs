use std::collections::HashMap;

use super::BitrateBounds;
use crate::protocol::MAX_FRAMES_IN_FLIGHT;

/// Tracks frames that are being encoded or have been sent but not acked
/// (PROTOCOL §6). A captured frame may only be encoded when a slot is free,
/// so a backed-up link drops frames *before* encoding and the decoder never
/// misses a reference. Times are seconds on the caller's monotonic clock.
#[derive(Debug, Clone)]
pub struct InFlightTracker {
    capacity: usize,
    /// Frames handed to the encoder whose output has not been sent yet.
    reserved: usize,
    /// seq → send time, for frames sent and not yet acknowledged.
    sent: HashMap<u32, f64>,
    next_seq: u32,
}

impl Default for InFlightTracker {
    fn default() -> Self {
        Self::new(MAX_FRAMES_IN_FLIGHT)
    }
}

impl InFlightTracker {
    pub fn new(capacity: usize) -> Self {
        InFlightTracker { capacity, reserved: 0, sent: HashMap::new(), next_seq: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn reserved(&self) -> usize {
        self.reserved
    }

    pub fn in_flight(&self) -> usize {
        self.reserved + self.sent.len()
    }

    pub fn sent_unacked(&self) -> usize {
        self.sent.len()
    }

    /// Claims a slot for a frame about to be encoded. `false` means: drop it.
    pub fn try_reserve(&mut self) -> bool {
        if self.in_flight() >= self.capacity {
            return false;
        }
        self.reserved += 1;
        true
    }

    /// The encoder produced no output for a reserved frame.
    pub fn cancel_reservation(&mut self) {
        self.reserved = self.reserved.saturating_sub(1);
    }

    /// Turns a reservation into a sent frame and returns its seq.
    pub fn mark_sent(&mut self, time: f64) -> u32 {
        self.reserved = self.reserved.saturating_sub(1);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.sent.insert(seq, time);
        seq
    }

    /// Records FRAME_ACK(seq). Acks are cumulative for robustness: every frame
    /// up to `seq` leaves the window. Returns the RTT sample for `seq` itself
    /// (ack arrival − send time), or `None` for an unknown/duplicate seq.
    pub fn ack(&mut self, seq: u32, time: f64) -> Option<f64> {
        let sample = self.sent.get(&seq).map(|sent| (time - sent).max(0.0));
        // Wrap-aware "older than or equal": the distance back from seq is < 2^31.
        self.sent.retain(|key, _| seq.wrapping_sub(*key) >= 0x8000_0000);
        sample
    }

    /// Forgets all sent frames (e.g. when the stream restarts).
    pub fn reset(&mut self) {
        self.reserved = 0;
        self.sent.clear();
    }
}

/// Tuning of [`AdaptiveBitrateController`]; bitrates in bits per second.
#[derive(Debug, Clone, PartialEq)]
pub struct AbrConfig {
    pub start_bitrate: i64,
    pub min_bitrate: i64,
    pub max_bitrate: i64,
    /// Seconds above the minimum RTT that count as congestion.
    pub rtt_margin: f64,
    pub decrease_factor: f64,
    pub clean_windows_before_increase: u32,
    /// Evaluation window in seconds.
    pub window: f64,
    /// Drops before this many acks (encoder pipeline filling up) are ignored.
    pub warmup_acks: u32,
}

impl Default for AbrConfig {
    fn default() -> Self {
        AbrConfig {
            start_bitrate: 20_000_000,
            min_bitrate: 4_000_000,
            max_bitrate: 50_000_000,
            rtt_margin: 0.040,
            decrease_factor: 0.75,
            clean_windows_before_increase: 2,
            window: 1.0,
            warmup_acks: 30,
        }
    }
}

impl AbrConfig {
    /// Bounds from the user setting and the client's limit; starts at 20 Mbps clamped into them.
    pub fn from_bounds(bounds: BitrateBounds, window: f64) -> Self {
        AbrConfig {
            start_bitrate: bounds.start(),
            min_bitrate: bounds.floor,
            max_bitrate: bounds.ceiling,
            window,
            ..AbrConfig::default()
        }
    }

    fn sanitized(mut self) -> Self {
        self.min_bitrate = self.min_bitrate.max(100_000);
        self.max_bitrate = self.max_bitrate.max(self.min_bitrate);
        self
    }
}

/// Outcome of one evaluated window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbrDecision {
    Hold,
    Decrease { reason: String },
    Increase,
}

/// AIMD bitrate control evaluated once per window (PROTOCOL §6):
/// - a dropped frame, or smoothed RTT > min RTT + 40 ms → bitrate × 0.75 (≥ min)
/// - after 2 consecutive clean windows → + max(1 Mbps, 5 % of max) (≤ max)
///
/// A window without any ack is neither clean nor congested (no evidence) and
/// leaves both the bitrate and the clean-window streak unchanged. Drops during
/// warm-up (before `warmup_acks` acks, while the encoder pipeline fills at
/// stream start) say nothing about the link and are not counted.
#[derive(Debug, Clone)]
pub struct AdaptiveBitrateController {
    config: AbrConfig,
    bitrate: i64,
    smoothed_rtt: Option<f64>,
    min_rtt: Option<f64>,
    clean_streak: u32,
    window_start: f64,
    dropped_in_window: u32,
    samples_in_window: u32,
    total_samples: u32,
}

impl AdaptiveBitrateController {
    pub fn new(config: AbrConfig, now: f64) -> Self {
        let config = config.sanitized();
        let bitrate = config.start_bitrate.max(config.min_bitrate).min(config.max_bitrate);
        AdaptiveBitrateController {
            config,
            bitrate,
            smoothed_rtt: None,
            min_rtt: None,
            clean_streak: 0,
            window_start: now,
            dropped_in_window: 0,
            samples_in_window: 0,
            total_samples: 0,
        }
    }

    pub fn config(&self) -> &AbrConfig {
        &self.config
    }

    /// Current target in bits per second.
    pub fn bitrate(&self) -> i64 {
        self.bitrate
    }

    pub fn smoothed_rtt(&self) -> Option<f64> {
        self.smoothed_rtt
    }

    pub fn min_rtt(&self) -> Option<f64> {
        self.min_rtt
    }

    pub fn clean_streak(&self) -> u32 {
        self.clean_streak
    }

    /// The step used for additive increase.
    pub fn increase_step(&self) -> i64 {
        (self.config.max_bitrate / 20).max(1_000_000)
    }

    pub fn record_drop(&mut self, count: u32) {
        if self.total_samples < self.config.warmup_acks {
            return; // warm-up
        }
        self.dropped_in_window += count;
    }

    /// Feeds one RTT sample in seconds (EWMA with α = 1/8, like TCP's SRTT).
    pub fn record_rtt(&mut self, rtt: f64) {
        self.samples_in_window += 1;
        self.total_samples = self.total_samples.saturating_add(1);
        self.min_rtt = Some(self.min_rtt.map_or(rtt, |m| m.min(rtt)));
        self.smoothed_rtt = Some(match self.smoothed_rtt {
            Some(s) => s + (rtt - s) / 8.0,
            None => rtt,
        });
    }

    /// Changes the ceiling (user setting). The bitrate is clamped immediately.
    pub fn set_max_bitrate(&mut self, max_bitrate: i64) {
        self.config.max_bitrate = max_bitrate;
        self.config = self.config.clone().sanitized();
        self.clamp_bitrate();
    }

    /// Changes floor and ceiling together (setting × client limit). The bitrate is clamped immediately.
    pub fn set_bounds(&mut self, bounds: BitrateBounds) {
        self.config.min_bitrate = bounds.floor;
        self.config.max_bitrate = bounds.ceiling;
        self.config = self.config.clone().sanitized();
        self.clamp_bitrate();
    }

    fn clamp_bitrate(&mut self) {
        self.bitrate = self.bitrate.max(self.config.min_bitrate).min(self.config.max_bitrate);
    }

    /// Call periodically; returns the decision once a window has elapsed, else `None`.
    pub fn tick(&mut self, now: f64) -> Option<AbrDecision> {
        if now - self.window_start < self.config.window {
            return None;
        }
        self.window_start = now;
        let dropped = std::mem::take(&mut self.dropped_in_window);
        let samples = std::mem::take(&mut self.samples_in_window);

        let rtt_congested = match (self.smoothed_rtt, self.min_rtt) {
            (Some(s), Some(m)) => s > m + self.config.rtt_margin,
            _ => false,
        };
        if dropped > 0 || rtt_congested {
            self.clean_streak = 0;
            let reduced = (self.bitrate as f64 * self.config.decrease_factor) as i64;
            self.bitrate = reduced.max(self.config.min_bitrate);
            let reason = if dropped > 0 { format!("{dropped} dropped") } else { "rtt".to_owned() };
            return Some(AbrDecision::Decrease { reason });
        }
        if samples == 0 {
            return Some(AbrDecision::Hold);
        }
        self.clean_streak += 1;
        if self.clean_streak >= self.config.clean_windows_before_increase {
            self.clean_streak = 0;
            let next = (self.bitrate + self.increase_step()).min(self.config.max_bitrate);
            if next != self.bitrate {
                self.bitrate = next;
                return Some(AbrDecision::Increase);
            }
        }
        Some(AbrDecision::Hold)
    }
}
