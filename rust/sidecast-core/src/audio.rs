//! Phone microphone audio on the host side (PROTOCOL §7.2).

use crate::protocol::AudioPacket;

/// What an AUDIO sequence number means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceEvent {
    First,
    InOrder,
    /// `missing` packets were skipped before this one.
    Gap {
        missing: u32,
    },
    Restart,
}

/// Classifies AUDIO sequence numbers. TCP never reorders or duplicates, so a
/// number below the expected one means the phone restarted its numbering (a
/// new capture after MIC_CONTROL active:true).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioSequenceTracker {
    expected: Option<u32>,
}

impl AudioSequenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expected(&self) -> Option<u32> {
        self.expected
    }

    pub fn observe(&mut self, seq: u32) -> SequenceEvent {
        let previous = self.expected.replace(seq.wrapping_add(1));
        let Some(expected) = previous else { return SequenceEvent::First };
        if seq == expected {
            return SequenceEvent::InOrder;
        }
        let diff = seq.wrapping_sub(expected) as i32; // wrap-aware
        if diff > 0 { SequenceEvent::Gap { missing: diff as u32 } } else { SequenceEvent::Restart }
    }

    pub fn reset(&mut self) {
        self.expected = None;
    }
}

/// Sizes of an [`AudioJitterBuffer`] in frames (samples, mono).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitterConfig {
    pub target_frames: usize,
    pub max_frames: usize,
}

impl Default for JitterConfig {
    /// 40 ms target, 150 ms ceiling at 48 kHz.
    fn default() -> Self {
        Self::new(1_920, 7_200)
    }
}

impl JitterConfig {
    pub fn new(target_frames: usize, max_frames: usize) -> Self {
        JitterConfig { target_frames, max_frames: max_frames.max(target_frames) }
    }
}

/// Counters, reset by [`AudioJitterBuffer::take_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub packets: usize,
    pub received_frames: usize,
    /// Packets the phone skipped (sequence gaps).
    pub missing_packets: usize,
    /// Silence inserted for those gaps.
    pub gap_fill_frames: usize,
    pub restarts: usize,
    /// Oldest audio dropped because too much was queued.
    pub dropped_frames: usize,
    pub underruns: usize,
    pub played_frames: usize,
    /// Silence rendered while buffering or after an underrun.
    pub silent_frames: usize,
}

/// Jitter buffer between AUDIO packets (network, bursty) and the audio
/// device's render callback (steady):
/// - plays only once `target_frames` (~40 ms) are queued, and re-buffers after
///   an underrun (the shortfall is silence);
/// - more than `max_frames` (~150 ms) queued: the oldest audio is dropped back
///   down to the target, so latency recovers after a burst;
/// - sequence gaps are filled with silence (one packet's length per missing
///   packet, at most `max_frames`).
///
/// Not thread-safe; callers serialise access. `read` never allocates.
#[derive(Debug, Clone)]
pub struct AudioJitterBuffer {
    config: JitterConfig,
    stats: JitterStats,
    buffering: bool,
    ring: Vec<f32>,
    head: usize,
    count: usize,
    sequence: AudioSequenceTracker,
    last_packet_frames: usize,
}

impl Default for AudioJitterBuffer {
    fn default() -> Self {
        Self::new(JitterConfig::default())
    }
}

impl AudioJitterBuffer {
    pub fn new(config: JitterConfig) -> Self {
        // Room for the ceiling plus the largest packet and a full gap fill.
        let capacity = config.max_frames * 2 + AudioPacket::MAX_SAMPLES + 1;
        AudioJitterBuffer {
            config,
            stats: JitterStats::default(),
            buffering: true,
            ring: vec![0.0; capacity],
            head: 0,
            count: 0,
            sequence: AudioSequenceTracker::new(),
            last_packet_frames: AudioPacket::RECOMMENDED_SAMPLES,
        }
    }

    pub fn config(&self) -> JitterConfig {
        self.config
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    pub fn is_buffering(&self) -> bool {
        self.buffering
    }

    pub fn queued_frames(&self) -> usize {
        self.count
    }

    /// Empties the buffer for a new capture (after MIC_CONTROL active:true).
    pub fn reset(&mut self) {
        self.head = 0;
        self.count = 0;
        self.buffering = true;
        self.sequence.reset();
        self.last_packet_frames = AudioPacket::RECOMMENDED_SAMPLES;
    }

    /// Returns and clears the counters.
    pub fn take_stats(&mut self) -> JitterStats {
        std::mem::take(&mut self.stats)
    }

    pub fn push(&mut self, packet: &AudioPacket) {
        self.push_samples(packet.seq, &packet.samples);
    }

    pub fn push_samples(&mut self, seq: u32, samples: &[i16]) {
        self.stats.packets += 1;
        self.stats.received_frames += samples.len();
        match self.sequence.observe(seq) {
            SequenceEvent::First | SequenceEvent::InOrder => {}
            SequenceEvent::Gap { missing } => {
                self.stats.missing_packets += missing as usize;
                let fill = (missing as usize).saturating_mul(self.last_packet_frames).min(self.config.max_frames);
                self.append_silence(fill);
                self.stats.gap_fill_frames += fill;
            }
            SequenceEvent::Restart => self.stats.restarts += 1,
        }
        if !samples.is_empty() {
            self.last_packet_frames = samples.len();
        }
        self.append(samples);
        if self.count > self.config.max_frames {
            self.drop_oldest(self.count - self.config.target_frames);
        }
    }

    /// Fills `out` with the next frames; silence while buffering or on underrun.
    /// Returns how many frames were real audio.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let wanted = out.len();
        if wanted == 0 {
            return 0;
        }
        if self.buffering {
            if self.count >= self.config.target_frames.max(1) {
                self.buffering = false;
            } else {
                out.fill(0.0);
                self.stats.silent_frames += wanted;
                return 0;
            }
        }
        let take = self.count.min(wanted);
        let capacity = self.ring.len();
        let first = take.min(capacity - self.head);
        out[..first].copy_from_slice(&self.ring[self.head..self.head + first]);
        if take > first {
            out[first..take].copy_from_slice(&self.ring[..take - first]);
        }
        self.head = (self.head + take) % capacity;
        self.count -= take;
        self.stats.played_frames += take;
        if take < wanted {
            out[take..].fill(0.0);
            self.stats.silent_frames += wanted - take;
            self.stats.underruns += 1;
            self.buffering = true;
        }
        take
    }

    // MARK: - Ring

    fn make_room(&mut self, frames: usize) {
        let overflow = (self.count + frames).saturating_sub(self.ring.len());
        if overflow > 0 {
            self.drop_oldest(overflow);
        }
    }

    fn drop_oldest(&mut self, frames: usize) {
        let n = frames.min(self.count);
        self.head = (self.head + n) % self.ring.len();
        self.count -= n;
        self.stats.dropped_frames += n;
    }

    fn append(&mut self, samples: &[i16]) {
        let n = samples.len().min(self.ring.len());
        self.make_room(n);
        let capacity = self.ring.len();
        let mut tail = (self.head + self.count) % capacity;
        for &s in &samples[samples.len() - n..] {
            self.ring[tail] = s as f32 / 32_768.0;
            tail = (tail + 1) % capacity;
        }
        self.count += n;
    }

    fn append_silence(&mut self, frames: usize) {
        let n = frames.min(self.ring.len());
        self.make_room(n);
        let capacity = self.ring.len();
        let mut tail = (self.head + self.count) % capacity;
        for _ in 0..n {
            self.ring[tail] = 0.0;
            tail = (tail + 1) % capacity;
        }
        self.count += n;
    }
}
