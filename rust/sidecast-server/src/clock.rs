//! One monotonic time base for the whole process, so capture timestamps,
//! flow-control send times and RTT samples are comparable.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Monotonic microseconds: the timebase of [`crate::backend::CapturedFrame::pts_micros`].
pub fn monotonic_micros() -> u64 {
    epoch().elapsed().as_micros() as u64
}

/// Monotonic seconds, for the pure state machines of `sidecast-core`.
pub(crate) fn now_secs() -> f64 {
    epoch().elapsed().as_secs_f64()
}

/// The `Instant` matching a [`now_secs`] value.
pub(crate) fn instant_at(secs: f64) -> Instant {
    epoch() + std::time::Duration::from_secs_f64(secs.max(0.0))
}

/// Wall-clock Unix seconds (pairing dates).
pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
