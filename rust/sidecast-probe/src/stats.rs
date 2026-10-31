//! Counters and per-second series for the probe's report (as the Swift probe prints them).

use std::time::Instant;

#[derive(Debug, Default)]
pub struct Stats {
    pub configs: u32,
    pub seq_gaps: u32,
    pub frames: u64,
    pub keyframes: u64,
    pub bytes: u64,
    pub decoded: u64,
    pub decode_errors: u64,
    pub size_mismatches: u64,
    pub structure_errors: u64,
    pub rtts: Vec<f64>,
    mbps_per_second: Vec<f64>,
    fps_per_second: Vec<u64>,
    started: Option<Instant>,
    last_frame: Option<Instant>,
    last_keyframe: Option<Instant>,
    pub max_keyframe_gap: f64,
    window_frames: u64,
    window_bytes: u64,
    window_keyframes: u64,
    window_decoded_start: u64,
}

impl Stats {
    pub fn start(&mut self, at: Instant) {
        self.started = Some(at);
    }

    pub fn record_frame(&mut self, bytes: usize, keyframe: bool, at: Instant) {
        self.frames += 1;
        self.bytes += bytes as u64;
        self.window_frames += 1;
        self.window_bytes += bytes as u64;
        if keyframe {
            self.keyframes += 1;
            self.window_keyframes += 1;
            if let Some(last) = self.last_keyframe {
                self.max_keyframe_gap = self.max_keyframe_gap.max((at - last).as_secs_f64());
            }
            self.last_keyframe = Some(at);
        }
        self.last_frame = Some(at);
    }

    /// Forget the keyframe history (a new STREAM_CONFIG restarts the cadence).
    pub fn restart_keyframe_cadence(&mut self) {
        self.last_keyframe = None;
    }

    /// One line per second; resets the window.
    pub fn second_line(&mut self) -> String {
        let mbps = self.window_bytes as f64 * 8.0 / 1e6;
        self.mbps_per_second.push(mbps);
        self.fps_per_second.push(self.window_frames);
        let line = format!(
            "probe: t={:2}s frames={:3} keyframes={} Mbps={mbps:6.2} decoded={} decodeErrors={} rtt={}",
            self.mbps_per_second.len(),
            self.window_frames,
            self.window_keyframes,
            self.decoded - self.window_decoded_start,
            self.decode_errors,
            self.rtts.last().map_or("-".to_owned(), |r| format!("{:.1}ms", r * 1000.0))
        );
        self.window_decoded_start = self.decoded;
        self.window_frames = 0;
        self.window_bytes = 0;
        self.window_keyframes = 0;
        line
    }

    pub fn summary(&self) -> Vec<String> {
        let duration = match (self.started, self.last_frame) {
            (Some(s), Some(l)) => (l - s).as_secs_f64().max(0.001),
            _ => 0.001,
        };
        let avg_rtt = if self.rtts.is_empty() { 0.0 } else { self.rtts.iter().sum::<f64>() / self.rtts.len() as f64 };
        let max_rtt = self.rtts.iter().copied().fold(0.0, f64::max);
        let series: Vec<String> = self.mbps_per_second.iter().map(|m| format!("{m:.1}")).collect();
        let fps: Vec<String> = self.fps_per_second.iter().map(u64::to_string).collect();
        vec![
            format!(
                "probe: SUMMARY frames={} decoded={} decodeErrors={} sizeMismatches={} structureErrors={} keyframes={} configs={} seqGaps={}",
                self.frames,
                self.decoded,
                self.decode_errors,
                self.size_mismatches,
                self.structure_errors,
                self.keyframes,
                self.configs,
                self.seq_gaps
            ),
            format!(
                "probe: SUMMARY avgFps={:.1} avgMbps={:.2} maxKeyframeGap={:.2}s rtt avg={:.1}ms max={:.1}ms ({} pongs)",
                self.frames as f64 / duration,
                self.bytes as f64 * 8.0 / 1e6 / duration,
                self.max_keyframe_gap,
                avg_rtt * 1000.0,
                max_rtt * 1000.0,
                self.rtts.len()
            ),
            format!("probe: SUMMARY Mbps per second: {}", series.join(" ")),
            format!("probe: SUMMARY fps per second: {}", fps.join(" ")),
        ]
    }

    /// What makes a streaming run fail.
    pub fn problems(&self, decoding: bool) -> Vec<String> {
        let mut p = Vec::new();
        if self.configs == 0 {
            p.push("no STREAM_CONFIG received".to_owned());
        }
        if self.frames == 0 {
            p.push("no video frames received".to_owned());
        }
        if decoding && self.decoded == 0 {
            p.push("no frame decoded".to_owned());
        }
        if self.decode_errors > 0 {
            p.push(format!("{} decode errors", self.decode_errors));
        }
        if self.size_mismatches > 0 {
            p.push(format!("{} decoded frames had the wrong size", self.size_mismatches));
        }
        if self.structure_errors > 0 {
            p.push(format!("{} access units with an invalid Annex-B structure", self.structure_errors));
        }
        if self.seq_gaps > 0 {
            p.push(format!("{} sequence gaps", self.seq_gaps));
        }
        if self.rtts.is_empty() {
            p.push("no PONG received".to_owned());
        }
        if self.max_keyframe_gap > 2.5 {
            p.push(format!("keyframe gap {:.2}s exceeds 2 s", self.max_keyframe_gap));
        }
        p
    }
}
