use crate::protocol::CursorPosition;

/// What [`CursorThrottle::offer`] decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorDecision {
    /// Send this now.
    Send(CursorPosition),
    /// Held; call [`CursorThrottle::flush`] at this time.
    Deferred { until: f64 },
    /// Same as what the client already has.
    Unchanged,
}

/// Rate limit for CURSOR (≤ 60 per second, PROTOCOL §7.3) that never loses
/// the final position: positions offered too soon are held and sent by `flush`.
#[derive(Debug, Clone)]
pub struct CursorThrottle {
    min_interval: f64,
    last_sent: Option<CursorPosition>,
    last_sent_at: Option<f64>,
    pending: Option<CursorPosition>,
}

impl Default for CursorThrottle {
    fn default() -> Self {
        Self::new(60.0)
    }
}

impl CursorThrottle {
    pub fn new(max_per_second: f64) -> Self {
        CursorThrottle {
            min_interval: 1.0 / max_per_second.max(1.0),
            last_sent: None,
            last_sent_at: None,
            pending: None,
        }
    }

    pub fn min_interval(&self) -> f64 {
        self.min_interval
    }

    pub fn last_sent(&self) -> Option<CursorPosition> {
        self.last_sent
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// A new position. `force` (mode switch, new STREAM_CONFIG) sends even if
    /// unchanged, but still respects the rate.
    pub fn offer(&mut self, position: CursorPosition, now: f64, force: bool) -> CursorDecision {
        if !force && self.pending.is_none() && Some(position) == self.last_sent {
            return CursorDecision::Unchanged;
        }
        if let Some(at) = self.last_sent_at
            && now - at < self.min_interval
        {
            self.pending = Some(position);
            return CursorDecision::Deferred { until: at + self.min_interval };
        }
        CursorDecision::Send(self.mark_sent(position, now))
    }

    /// The held position, if any (and if the interval has passed).
    pub fn flush(&mut self, now: f64) -> Option<CursorPosition> {
        let position = self.pending?;
        if let Some(at) = self.last_sent_at
            && now - at < self.min_interval - 1e-9
        {
            return None;
        }
        Some(self.mark_sent(position, now))
    }

    /// Forget everything (new mode, new session).
    pub fn reset(&mut self) {
        self.last_sent = None;
        self.last_sent_at = None;
        self.pending = None;
    }

    fn mark_sent(&mut self, position: CursorPosition, now: f64) -> CursorPosition {
        self.pending = None;
        self.last_sent = Some(position);
        self.last_sent_at = Some(now);
        position
    }
}
