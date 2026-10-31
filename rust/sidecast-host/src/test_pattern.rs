//! Test-pattern mode, the equivalent of the Mac app's `--test-pattern`: the
//! real server, TLS, pairing, flow control and encoder, fed with a synthetic
//! moving pattern. No display, capture, input injection, audio device or
//! power change: touches go through the real gesture engine and what would
//! be injected is logged, as is microphone audio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sidecast_core::audio::{AudioSequenceTracker, SequenceEvent};
use sidecast_core::input::{
    ClickTiming, GestureAction, InputEvent, PointerEventKind, ScrollPhase, click_timeline, describe_timeline, typing,
};
use sidecast_core::protocol::{AudioPacket, MicState};
use sidecast_core::streaming::StreamPlan;
use sidecast_core::{Point, Size};
use sidecast_server::backend::{
    BackendError, CaptureSession, CapturedFrame, ClientInfo, DisplayBackend, DisplayGeometry, FrameSink, InputBackend,
    InputSink, MicController, MicSink, PixelData, SessionId,
};
use sidecast_server::clock::monotonic_micros;

/// Frames from [`PatternRenderer`] at the plan's frame rate.
#[derive(Debug, Default)]
pub struct TestPatternDisplay;

impl DisplayBackend for TestPatternDisplay {
    fn name(&self) -> &str {
        "test-pattern"
    }

    fn start(
        &self,
        plan: &StreamPlan,
        client: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError> {
        let (width, height, fps) = (plan.encode_width as usize, plan.encode_height as usize, plan.fps.max(1) as u32);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("sidecast-test-pattern".into())
            .spawn(move || {
                let mut renderer = PatternRenderer::new(width, height);
                let interval = Duration::from_secs_f64(1.0 / fps as f64);
                let mut next = Instant::now() + interval;
                while !flag.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    }
                    // After a stall, pace from now instead of bursting to catch up.
                    next = (next + interval).max(Instant::now());
                    // Render only when flow control has room (drops happen before encoding).
                    if let Some(slot) = frames.try_reserve() {
                        slot.submit(renderer.next_frame());
                    } else {
                        renderer.skip();
                    }
                }
            })
            .map_err(|e| BackendError::display_failed(format!("cannot start the pattern thread: {e}")))?;
        log::info!("test pattern: {width}x{height} @ {fps} fps for {}", client.device_name);
        let geometry = DisplayGeometry::new(Size::new(plan.display.width as f64, plan.display.height as f64));
        Ok(Box::new(PatternCapture { stop, thread: Some(thread), geometry }))
    }
}

struct PatternCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    geometry: DisplayGeometry,
}

impl CaptureSession for PatternCapture {
    fn geometry(&self) -> DisplayGeometry {
        self.geometry.clone()
    }
}

impl Drop for PatternCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Scrolling colour bars, a bouncing box, a frame-counter bar code and a
/// noise band (so the encoder really spends bits), as I420 BT.709 video range.
pub struct PatternRenderer {
    width: usize,
    height: usize,
    frame: usize,
    rng: u64,
    /// One row of bars, twice as wide as the frame, so scrolling is a copy at an offset.
    bars_y: Vec<u8>,
    bars_u: Vec<u8>,
    bars_v: Vec<u8>,
}

impl PatternRenderer {
    /// BT.709 video-range Y, Cb, Cr of eight colour bars.
    const BARS: [(u8, u8, u8); 8] = [
        (235, 128, 128),
        (219, 16, 138),
        (188, 154, 16),
        (173, 42, 26),
        (78, 214, 230),
        (63, 102, 240),
        (32, 240, 118),
        (16, 128, 128),
    ];

    pub fn new(width: usize, height: usize) -> Self {
        let bar_width = (width / 8).max(2) & !1;
        let row = |pick: fn((u8, u8, u8)) -> u8, step: usize| -> Vec<u8> {
            (0..width * 2 / step + 2).map(|x| pick(Self::BARS[(x * step / bar_width) % 8])).collect()
        };
        PatternRenderer {
            width,
            height,
            frame: 0,
            rng: 0x9E37_79B9_7F4A_7C15,
            bars_y: row(|b| b.0, 1),
            bars_u: row(|b| b.1, 2),
            bars_v: row(|b| b.2, 2),
        }
    }

    /// Advances the animation without drawing (a dropped frame).
    pub fn skip(&mut self) {
        self.frame += 1;
    }

    pub fn next_frame(&mut self) -> CapturedFrame {
        let (w, h) = (self.width, self.height);
        let (cw, ch) = (w / 2, h / 2);
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        let offset = ((self.frame * 6) % w) & !1;
        for row in y.chunks_exact_mut(w) {
            row.copy_from_slice(&self.bars_y[offset..offset + w]);
        }
        for (row_u, row_v) in u.chunks_exact_mut(cw).zip(v.chunks_exact_mut(cw)) {
            row_u.copy_from_slice(&self.bars_u[offset / 2..offset / 2 + cw]);
            row_v.copy_from_slice(&self.bars_v[offset / 2..offset / 2 + cw]);
        }
        self.draw_box(&mut y, &mut u, &mut v);
        self.draw_counter(&mut y);
        self.draw_noise(&mut y);
        self.frame += 1;
        CapturedFrame {
            pts_micros: monotonic_micros(),
            width: w as u32,
            height: h as u32,
            pixels: PixelData::I420 { y, u, v, y_stride: w, uv_stride: cw },
        }
    }

    /// A white box bouncing around the frame.
    fn draw_box(&self, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
        let (w, h) = (self.width, self.height);
        let size = (w.min(h) / 6).max(8) & !1;
        let span_x = w.saturating_sub(size).max(1);
        let span_y = h.saturating_sub(size).max(1);
        let bounce = |t: usize, span: usize| {
            let p = t % (2 * span);
            if p < span { p } else { 2 * span - p }
        };
        let x0 = bounce(self.frame * 7, span_x) & !1;
        let y0 = bounce(self.frame * 5, span_y) & !1;
        for row in y0..(y0 + size).min(h) {
            y[row * w + x0..row * w + (x0 + size).min(w)].fill(235);
        }
        for row in y0 / 2..((y0 + size) / 2).min(h / 2) {
            let start = row * (w / 2) + x0 / 2;
            let end = row * (w / 2) + ((x0 + size) / 2).min(w / 2);
            u[start..end].fill(128);
            v[start..end].fill(128);
        }
    }

    /// The frame number as a 24-bit bar code along the top edge.
    fn draw_counter(&self, y: &mut [u8]) {
        let w = self.width;
        let cell = (w / 48).max(4);
        for bit in 0..24 {
            if bit * cell + cell > w {
                break;
            }
            let value = if (self.frame >> (23 - bit)) & 1 == 1 { 235 } else { 16 };
            for row in 0..cell.min(self.height) {
                y[row * w + bit * cell..row * w + bit * cell + cell].fill(value);
            }
        }
    }

    /// Random luma in a bottom band: incompressible, so the encoder's output
    /// tracks its target bitrate. Kept thin: OpenH264 runs on the CPU.
    fn draw_noise(&mut self, y: &mut [u8]) {
        let band = (self.height / 48).max(2);
        let start = (self.height - band) * self.width;
        for chunk in y[start..].chunks_mut(8) {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let bytes = self.rng.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// Logs what a real input backend would inject.
#[derive(Debug, Default)]
pub struct LoggingInput;

impl InputBackend for LoggingInput {
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError> {
        Ok(Box::new(LoggingSink { size: geometry.size, ..LoggingSink::default() }))
    }
}

#[derive(Default)]
struct LoggingSink {
    size: Size,
    /// Where the pointer would be: test mode has no real pointer, so trackpad
    /// mode and CURSOR follow what was "injected".
    pointer: Option<Point>,
    texts: usize,
    keys: usize,
    scroll_steps: usize,
    drag_steps: usize,
    moves: usize,
    move_from: Option<Point>,
    move_to: Option<Point>,
}

fn at(p: Point) -> String {
    format!("({:.0}, {:.0})", p.x, p.y)
}

impl LoggingSink {
    /// Logs a run of pointer moves as one line.
    fn flush_moves(&mut self) {
        if let (Some(from), Some(to)) = (self.move_from.take(), self.move_to.take()) {
            log::info!(
                "test pattern: gesture → cursor moved in {} steps {} → {}, Δ=({:+.0}, {:+.0}) — not injected",
                self.moves,
                at(from),
                at(to),
                to.x - from.x,
                to.y - from.y
            );
        }
        self.moves = 0;
    }
}

impl InputSink for LoggingSink {
    fn observe_gesture(&mut self, action: &GestureAction) {
        if let Some(steps) = click_timeline(action, ClickTiming::default())
            && steps.len() > 1
        {
            log::info!("test pattern: click timeline: {} — not injected", describe_timeline(&steps));
        }
        if let GestureAction::Move { to } = action {
            self.move_from.get_or_insert(*to);
            self.move_to = Some(*to);
            self.moves += 1;
            return;
        }
        self.flush_moves();
        match *action {
            GestureAction::Scroll { phase: ScrollPhase::Changed | ScrollPhase::Momentum, .. } => self.scroll_steps += 1,
            GestureAction::Scroll { phase, .. } => {
                if matches!(phase, ScrollPhase::Began | ScrollPhase::MomentumBegan) {
                    self.scroll_steps = 0;
                }
                let steps = if matches!(phase, ScrollPhase::Ended | ScrollPhase::MomentumEnded) {
                    format!(" after {} steps", self.scroll_steps)
                } else {
                    String::new()
                };
                log::info!("test pattern: gesture → scroll {phase:?}{steps} — not injected");
            }
            GestureAction::Drag { .. } => self.drag_steps += 1,
            GestureAction::Semantic(semantic) => {
                log::info!("test pattern: gesture → semantic {} ({semantic:?}) — not injected", semantic.name())
            }
            other => {
                if matches!(other, GestureAction::ButtonUp { .. }) && self.drag_steps > 0 {
                    log::info!("test pattern: gesture → {} drag steps", self.drag_steps);
                    self.drag_steps = 0;
                }
                let described = match other {
                    GestureAction::ButtonDown { button, at: p } => format!("{button:?} button down at {}", at(p)),
                    GestureAction::ButtonUp { button, at: p } => format!("{button:?} button up at {}", at(p)),
                    GestureAction::Click { at: p, count } => format!("click ×{count} at {}", at(p)),
                    GestureAction::RightClick { at: p } => format!("right click at {}", at(p)),
                    GestureAction::Zoom { delta, at: p } => format!("zoom {delta} at {}", at(p)),
                    _ => format!("{other:?}"),
                };
                log::info!("test pattern: gesture → {described} — not injected");
            }
        }
    }

    fn inject(&mut self, event: &InputEvent) {
        match event {
            InputEvent::Pointer(pointer) => {
                self.pointer = Some(pointer.at);
                if pointer.kind != PointerEventKind::Moved {
                    log::debug!("test pattern: pointer {:?} at {}", pointer.kind, at(pointer.at));
                }
            }
            InputEvent::Text(text) => {
                self.texts += 1;
                let chunks = typing::chunks(text, typing::MAX_UTF16_PER_EVENT);
                log::info!(
                    "test pattern: TEXT #{} {text:?} ({} bytes, {} UTF-16 units, {} key event pair(s)) — not injected",
                    self.texts,
                    text.len(),
                    text.encode_utf16().count(),
                    chunks.len()
                );
            }
            InputEvent::Key { key, modifiers } => {
                self.keys += 1;
                log::info!(
                    "test pattern: KEY #{} {key:?} modifiers=0x{:02x}{} — not injected",
                    self.keys,
                    modifiers.bits(),
                    if key.is_navigation() { " (navigation key)" } else { "" }
                );
            }
            InputEvent::Scroll { .. } | InputEvent::Zoom { .. } | InputEvent::Semantic(_) => {}
        }
    }

    fn pointer_position(&mut self) -> Option<Point> {
        self.pointer.filter(|p| p.x >= 0.0 && p.y >= 0.0 && p.x < self.size.width && p.y < self.size.height)
    }

    fn release_all(&mut self) {
        self.flush_moves();
        log::debug!("test pattern: release all buttons");
    }
}

/// Test-mode microphone (port of the Mac's `TestMicBridge`): logs MIC_STATE
/// and, once a second, AUDIO statistics (packets, gaps, level and the tone
/// frequency estimated from zero crossings). With `activate_on_start` it
/// asks a mic-capable phone for its microphone as soon as it streams.
pub struct LoggingMic {
    activate_on_start: bool,
    state: Arc<Mutex<MicWindow>>,
}

#[derive(Default)]
struct MicWindow {
    session: Option<SessionId>,
    tracker: AudioSequenceTracker,
    packets: usize,
    samples: usize,
    missing: usize,
    restarts: usize,
    peak: i32,
    zero_crossings: usize,
    last_sample: i16,
    total_packets: usize,
    total_samples: usize,
}

impl LoggingMic {
    pub fn new(activate_on_start: bool) -> Self {
        let state = Arc::new(Mutex::new(MicWindow::default()));
        let weak = Arc::downgrade(&state);
        let _ = std::thread::Builder::new().name("sidecast-test-mic".into()).spawn(move || {
            while let Some(state) = weak.upgrade() {
                Self::tick(&mut state.lock().unwrap_or_else(|p| p.into_inner()));
                drop(state);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        LoggingMic { activate_on_start, state }
    }

    fn tick(w: &mut MicWindow) {
        if w.session.is_none() || w.packets == 0 {
            return;
        }
        let seconds = w.samples as f64 / AudioPacket::SAMPLE_RATE as f64;
        let hz = if seconds > 0.0 { w.zero_crossings as f64 / 2.0 / seconds } else { 0.0 };
        log::info!(
            "test mic: AUDIO packets={} samples={} gaps={} restarts={} peak={} tone≈{hz:.0} Hz",
            w.packets,
            w.samples,
            w.missing,
            w.restarts,
            w.peak
        );
        w.total_packets += w.packets;
        w.total_samples += w.samples;
        (w.packets, w.samples, w.missing, w.restarts, w.peak, w.zero_crossings) = (0, 0, 0, 0, 0, 0);
    }
}

impl std::fmt::Debug for LoggingMic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggingMic").field("activate_on_start", &self.activate_on_start).finish()
    }
}

impl MicSink for LoggingMic {
    fn session_started(&self, session: SessionId, device_name: &str, supports_mic: bool, control: MicController) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) =
            MicWindow { session: Some(session), ..MicWindow::default() };
        log::info!(
            "test mic: {device_name} {} the microphone",
            if supports_mic { "supports" } else { "does not support" }
        );
        if !supports_mic {
            return;
        }
        if self.activate_on_start {
            log::info!("test mic: --test-mic-active → MIC_CONTROL active:true");
            control.set_active(true);
        } else {
            log::info!("test mic: no --test-mic-active, the microphone stays off");
        }
    }

    fn session_ended(&self, session: SessionId) {
        let mut w = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if w.session != Some(session) {
            return;
        }
        Self::tick(&mut w);
        log::info!("test mic: session ended — AUDIO total packets={} samples={}", w.total_packets, w.total_samples);
        w.session = None;
    }

    fn audio(&self, session: SessionId, packet: &AudioPacket) {
        let mut w = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if w.session != Some(session) {
            return;
        }
        match w.tracker.observe(packet.seq) {
            SequenceEvent::Gap { missing } => w.missing += missing as usize,
            SequenceEvent::Restart => w.restarts += 1,
            SequenceEvent::First | SequenceEvent::InOrder => {}
        }
        w.packets += 1;
        w.samples += packet.samples.len();
        let mut last = w.last_sample;
        for &s in &packet.samples {
            w.peak = w.peak.max((s as i32).abs());
            if (last < 0) != (s < 0) {
                w.zero_crossings += 1;
            }
            last = s;
        }
        w.last_sample = last;
    }

    fn mic_state(&self, session: SessionId, state: &MicState) {
        if self.state.lock().unwrap_or_else(|p| p.into_inner()).session != Some(session) {
            return;
        }
        log::info!(
            "test mic: MIC_STATE capturing={}{}",
            state.capturing,
            state.reason.as_deref().map(|r| format!(" reason={r}")).unwrap_or_default()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_makes_animated_i420_frames() {
        let mut renderer = PatternRenderer::new(1920, 864);
        let a = renderer.next_frame();
        let b = renderer.next_frame();
        let (PixelData::I420 { y: ya, u, v, y_stride, uv_stride }, PixelData::I420 { y: yb, .. }) =
            (&a.pixels, &b.pixels)
        else {
            panic!("expected I420");
        };
        assert_eq!((a.width, a.height, *y_stride, *uv_stride), (1920, 864, 1920, 960));
        assert_eq!((ya.len(), u.len(), v.len()), (1920 * 864, 960 * 432, 960 * 432));
        assert_ne!(ya, yb, "the pattern moves");
        assert!(b.pts_micros >= a.pts_micros);
        // Odd sizes the planner never produces still render.
        let _ = PatternRenderer::new(66, 64).next_frame();
    }
}
