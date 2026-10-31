//! One client's stream (PROTOCOL §6): capture frames → flow control (≤ 4 in
//! flight, drop *before* encoding) → encoder thread → VIDEO_FRAME, plus the
//! AIMD bitrate, the 5 s liveness check, CURSOR throttling and the input task.
//! Mirrors the Mac app's `StreamController.swift`.
//!
//! The controller lives in its session's task; capture threads only touch
//! [`Shared`] through a [`FrameSink`], and the encoder runs on its own thread.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sidecast_core::input::{CursorDecision, CursorThrottle};
use sidecast_core::protocol::{
    ByeReason, CursorPosition, InputMode, KEYFRAME_INTERVAL, KeyEvent, LIVENESS_TIMEOUT, ServerMessage, TouchEvent,
    VideoFrame,
};
use sidecast_core::streaming::{
    AbrConfig, AdaptiveBitrateController, BitrateBounds, InFlightTracker, StreamPlan, StreamPlanner, StreamSettings,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::backend::{
    BackendError, Backends, CaptureSession, CapturedFrame, ClientInfo, EncodedFrame, EncoderConfig, VideoEncoder,
};
use crate::clock::now_secs;
use crate::events::StreamStats;
use crate::input::{self, InputCommand};
use crate::outbox::Outbox;

/// Flow-control state shared with capture threads and the encoder thread.
struct Shared {
    tracker: InFlightTracker,
    abr: AdaptiveBitrateController,
    encoder: Option<mpsc::Sender<EncoderJob>>,
    /// Generation of the installed encoder; frames of other generations are dropped.
    generation: u64,
    keyframe_requested: bool,
    dropped_in_window: u32,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum EncoderJob {
    Encode { frame: CapturedFrame, requested_keyframe: bool, generation: u64 },
    SetBitrate(u32),
}

/// Where a capture backend delivers frames. Cheap to clone; callable from any thread.
#[derive(Clone)]
pub struct FrameSink {
    shared: Arc<Mutex<Shared>>,
    generation: u64,
}

impl std::fmt::Debug for FrameSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameSink").field("generation", &self.generation).finish()
    }
}

impl FrameSink {
    /// Claims a flow-control slot for the next frame. `None` means drop this
    /// frame (4 frames already in flight, or the encoder is not ready yet), so
    /// a capture backend can skip copying or converting it.
    pub fn try_reserve(&self) -> Option<FrameSlot> {
        let mut s = lock(&self.shared);
        if s.generation != self.generation {
            return None;
        }
        let encoder = s.encoder.clone()?;
        if !s.tracker.try_reserve() {
            s.abr.record_drop(1);
            s.dropped_in_window += 1;
            return None;
        }
        let requested_keyframe = std::mem::take(&mut s.keyframe_requested);
        Some(FrameSlot { sink: self.clone(), encoder, requested_keyframe, armed: true })
    }

    /// Reserves and submits in one step; `false` when the frame was dropped.
    pub fn submit(&self, frame: CapturedFrame) -> bool {
        self.try_reserve().is_some_and(|slot| slot.submit(frame))
    }
}

/// A reserved flow-control slot. Dropping it unused releases the slot.
pub struct FrameSlot {
    sink: FrameSink,
    encoder: mpsc::Sender<EncoderJob>,
    requested_keyframe: bool,
    armed: bool,
}

impl std::fmt::Debug for FrameSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameSlot").field("generation", &self.sink.generation).finish()
    }
}

impl FrameSlot {
    /// The encoder will make this frame a keyframe.
    pub fn is_keyframe_requested(&self) -> bool {
        self.requested_keyframe
    }

    pub fn submit(mut self, frame: CapturedFrame) -> bool {
        let job =
            EncoderJob::Encode { frame, requested_keyframe: self.requested_keyframe, generation: self.sink.generation };
        if self.encoder.send(job).is_err() {
            return false; // the encoder is gone; Drop releases the slot
        }
        self.armed = false;
        true
    }
}

impl Drop for FrameSlot {
    fn drop(&mut self) {
        if self.armed {
            let mut s = lock(&self.sink.shared);
            s.tracker.cancel_reservation();
            if self.requested_keyframe {
                s.keyframe_requested = true;
            }
        }
    }
}

/// What the stream's helpers report back to the session task.
pub(crate) enum StreamEvent {
    Started { generation: u64, result: Result<StartedStream, BackendError> },
    Encoded { generation: u64, frame: Option<EncodedFrame>, pts_micros: u64, requested_keyframe: bool },
    EncoderFailed { generation: u64, error: BackendError },
    Cursor { position: CursorPosition, force: bool },
}

pub(crate) struct StartedStream {
    capture: Box<dyn CaptureSession>,
    encoder: Box<dyn VideoEncoder>,
    slot: OwnedMutexGuard<()>,
}

/// What the session should do after `on_started`.
pub(crate) enum StartOutcome {
    /// Nothing (a superseded start).
    Ignored,
    /// STREAM_CONFIG was sent; `first` for the session's first one.
    Configured { first: bool },
}

/// The session must close: `Some(reason)` with a BYE, `None` silently (dead client).
pub(crate) struct StreamFailure(pub Option<ByeReason>);

pub(crate) struct StreamController {
    client: ClientInfo,
    plan: StreamPlan,
    settings: StreamSettings,
    backends: Backends,
    out: Outbox,
    events: UnboundedSender<StreamEvent>,
    shared: Arc<Mutex<Shared>>,
    stream_slot: Arc<AsyncMutex<()>>,
    log_stats: bool,

    running: bool,
    generation: u64,
    capture: Option<Box<dyn CaptureSession>>,
    slot: Option<OwnedMutexGuard<()>>,
    configured: bool,
    power: Option<Box<dyn Send>>,
    keep_awake: bool,
    input: Option<UnboundedSender<InputCommand>>,
    input_mode: InputMode,

    last_activity: f64,
    frames_in_window: u32,
    bytes_in_window: usize,
    keyframes_in_window: u32,
    cursor: CursorThrottle,
    cursor_flush_at: Option<f64>,
    cursors_in_window: u32,
    last_cursor: Option<CursorPosition>,
}

impl StreamController {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: ClientInfo,
        plan: StreamPlan,
        settings: StreamSettings,
        backends: Backends,
        out: Outbox,
        events: UnboundedSender<StreamEvent>,
        stream_slot: Arc<AsyncMutex<()>>,
        keep_awake: bool,
        log_stats: bool,
    ) -> Self {
        let bounds = BitrateBounds::new(settings.max_bitrate_mbps, client.limits.as_ref());
        // The 1 s timer drives the windows; 0.9 s keeps timer jitter from skipping one.
        let abr = AdaptiveBitrateController::new(AbrConfig::from_bounds(bounds, 0.9), now_secs());
        if let Some(limits) = &client.limits {
            log::info!(
                "stream: data saver for {} (network {}): limits {}, {} → bitrate {:.1}–{:.1} Mbps (start {:.1}), {} fps",
                client.device_name,
                client.network.as_deref().unwrap_or("?"),
                limits.max_bitrate_kbps.map_or("no bitrate cap".to_owned(), |k| format!("≤ {k} kbps")),
                limits.max_fps.map_or("no fps cap".to_owned(), |f| format!("≤ {f} fps")),
                bounds.floor as f64 / 1e6,
                bounds.ceiling as f64 / 1e6,
                bounds.start() as f64 / 1e6,
                plan.fps
            );
        }
        let shared = Shared {
            tracker: InFlightTracker::default(),
            abr,
            encoder: None,
            generation: 0,
            keyframe_requested: true,
            dropped_in_window: 0,
        };
        StreamController {
            client,
            plan,
            settings,
            backends,
            out,
            events,
            shared: Arc::new(Mutex::new(shared)),
            stream_slot,
            log_stats,
            running: false,
            generation: 0,
            capture: None,
            slot: None,
            configured: false,
            power: None,
            keep_awake,
            input: None,
            input_mode: InputMode::Touch,
            last_activity: now_secs(),
            frames_in_window: 0,
            bytes_in_window: 0,
            keyframes_in_window: 0,
            cursor: CursorThrottle::default(),
            cursor_flush_at: None,
            cursors_in_window: 0,
            last_cursor: None,
        }
    }

    pub fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    pub fn start(&mut self) {
        if self.running {
            return;
        }
        self.running = true;
        self.last_activity = now_secs();
        self.begin();
    }

    /// Starts capture + encoder for the current plan on blocking threads; the
    /// result comes back as [`StreamEvent::Started`]. Only one stream holds the
    /// display at a time: a takeover waits for the previous one to let go.
    fn begin(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let held = self.slot.take();
        let slot = self.stream_slot.clone();
        let display = self.backends.display.clone();
        let factories = self.backends.encoders.clone();
        let (plan, client) = (self.plan, self.client.clone());
        let bitrate = lock(&self.shared).abr.bitrate();
        let frames = FrameSink { shared: self.shared.clone(), generation };
        let events = self.events.clone();
        tokio::spawn(async move {
            let guard = match held {
                Some(guard) => Some(guard),
                None => tokio::time::timeout(Duration::from_secs(10), slot.lock_owned()).await.ok(),
            };
            let result = match guard {
                None => Err(BackendError::display_failed("the previous stream did not stop")),
                Some(guard) => tokio::task::spawn_blocking(move || {
                    let config = EncoderConfig {
                        codec: plan.codec,
                        width: plan.encode_width as u32,
                        height: plan.encode_height as u32,
                        fps: plan.fps as u32,
                        bitrate: bitrate.clamp(0, u32::MAX as i64) as u32,
                        keyframe_interval: KEYFRAME_INTERVAL,
                    };
                    let encoder = create_encoder(&factories, &config)?;
                    let capture = display.start(&plan, &client, frames)?;
                    Ok(StartedStream { capture, encoder, slot: guard })
                })
                .await
                .unwrap_or_else(|e| Err(BackendError::display_failed(format!("capture start panicked: {e}")))),
            };
            // A session that ended meanwhile: tear the display down properly.
            if let Err(unsent) = events.send(StreamEvent::Started { generation, result })
                && let StreamEvent::Started { result: Ok(started), .. } = unsent.0
            {
                release_capture(started.capture, Some(started.slot));
            }
        });
    }

    /// Installs a started capture + encoder and sends STREAM_CONFIG.
    pub fn on_started(
        &mut self,
        generation: u64,
        result: Result<StartedStream, BackendError>,
    ) -> Result<StartOutcome, StreamFailure> {
        if !self.running || generation != self.generation {
            if let Ok(started) = result {
                release_capture(started.capture, Some(started.slot));
            }
            return Ok(StartOutcome::Ignored); // superseded meanwhile
        }
        let started = match result {
            Ok(started) => started,
            Err(error) => {
                log::error!("stream: could not start: {error}");
                return Err(StreamFailure(Some(error.bye_reason())));
            }
        };
        let geometry = started.capture.geometry();
        self.capture = Some(started.capture);
        self.slot = Some(started.slot);
        let (tx, rx) = mpsc::channel();
        {
            let mut s = lock(&self.shared);
            s.encoder = Some(tx);
            s.generation = generation;
            s.keyframe_requested = true;
        }
        spawn_encoder_thread(started.encoder, rx, self.shared.clone(), generation, self.events.clone());

        // Sent before any frame of this encoder: their delivery is queued behind us.
        self.out.send(ServerMessage::StreamConfig(self.plan.stream_config()).encode());
        let d = self.plan.display;
        log::info!(
            "stream: STREAM_CONFIG {} {}x{}@{} (display {}x{} pt{})",
            self.plan.codec,
            self.plan.encode_width,
            self.plan.encode_height,
            self.plan.fps,
            d.width,
            d.height,
            if d.hi_dpi { " HiDPI" } else { "" }
        );

        if let Some(input) = &self.backends.input {
            match input.create(&geometry) {
                Ok(sink) => {
                    let points_per_dp = self.plan.points_per_dp(&self.client.screen);
                    self.input =
                        Some(input::spawn(sink, geometry, points_per_dp, self.input_mode, self.events.clone()));
                }
                Err(error) => log::warn!("input: unavailable for this stream: {error}"),
            }
        }
        if self.input_mode == InputMode::Trackpad {
            self.send_input(InputCommand::RefreshCursor); // the new stream gets the pointer too
        }
        let first = !self.configured;
        self.configured = true;
        if first
            && self.keep_awake
            && let Some(power) = &self.backends.power
        {
            match power.keep_awake("streaming to a phone") {
                Ok(guard) => self.power = Some(guard),
                Err(error) => log::warn!("power: cannot keep the computer awake: {error}"),
            }
        }
        Ok(StartOutcome::Configured { first })
    }

    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        self.cursor_flush_at = None;
        self.detach_encoder();
        if let Some(input) = self.input.take() {
            let _ = input.send(InputCommand::Stop);
        }
        if let Some(capture) = self.capture.take() {
            release_capture(capture, self.slot.take());
        }
        self.slot = None;
        self.power = None;
    }

    fn detach_encoder(&mut self) {
        let mut s = lock(&self.shared);
        s.encoder = None; // the encoder thread drains its queue and exits
        s.generation = 0; // streams count from 1: frames still in flight are void
    }

    /// Any message from the client counts as a sign of life.
    pub fn note_activity(&mut self) {
        self.last_activity = now_secs();
    }

    pub fn handle_ack(&mut self, seq: u32) {
        let now = now_secs();
        let mut s = lock(&self.shared);
        if let Some(rtt) = s.tracker.ack(seq, now) {
            s.abr.record_rtt(rtt);
        }
    }

    pub fn request_keyframe(&mut self) {
        lock(&self.shared).keyframe_requested = true;
        if let Some(capture) = &mut self.capture {
            capture.request_refresh();
        }
    }

    fn send_input(&self, command: InputCommand) {
        if let Some(input) = &self.input {
            let _ = input.send(command);
        }
    }

    /// TOUCH, TEXT and KEY share the "phone controls the computer" setting.
    pub fn handle_touch(&mut self, event: TouchEvent) {
        if self.settings.touch_enabled {
            self.send_input(InputCommand::Touch(event));
        }
    }

    pub fn handle_text(&mut self, text: String) {
        if self.settings.touch_enabled {
            self.send_input(InputCommand::Text(text));
        }
    }

    pub fn handle_key(&mut self, event: KeyEvent) {
        if self.settings.touch_enabled {
            self.send_input(InputCommand::Key(event));
        }
    }

    /// `true` when the mode changed.
    pub fn set_input_mode(&mut self, mode: InputMode) -> bool {
        if mode == self.input_mode {
            return false;
        }
        self.input_mode = mode;
        self.cursor_flush_at = None;
        self.cursor.reset();
        log::info!("stream: input mode {} for {}", mode.as_str(), self.client.device_name);
        // In trackpad mode the input task reports the pointer at once.
        self.send_input(InputCommand::Mode(mode));
        true
    }

    // MARK: - Frames

    pub fn on_encoded(&mut self, generation: u64, frame: Option<EncodedFrame>, pts_micros: u64, requested: bool) {
        let now = now_secs();
        let running = self.running;
        let seq = {
            let mut s = lock(&self.shared);
            match &frame {
                Some(f) if running && generation == s.generation && !f.data.is_empty() => {
                    Some(s.tracker.mark_sent(now))
                }
                _ => {
                    s.tracker.cancel_reservation();
                    if requested {
                        s.keyframe_requested = true; // the forced keyframe never made it out
                    }
                    None
                }
            }
        };
        let (Some(seq), Some(frame)) = (seq, frame) else { return };
        self.out.send(VideoFrame::encode_frame_parts(seq, frame.is_keyframe, pts_micros, &frame.data));
        self.frames_in_window += 1;
        self.bytes_in_window += frame.data.len();
        if frame.is_keyframe {
            self.keyframes_in_window += 1;
        }
    }

    pub fn is_current_encoder(&self, generation: u64) -> bool {
        self.running && lock(&self.shared).generation == generation
    }

    // MARK: - CURSOR (trackpad mode only, ≤ 60/s, final position always sent)

    pub fn on_cursor(&mut self, position: CursorPosition, force: bool) {
        if !self.running || self.input_mode != InputMode::Trackpad {
            return;
        }
        match self.cursor.offer(position, now_secs(), force) {
            CursorDecision::Send(p) => self.send_cursor(p),
            CursorDecision::Deferred { until } => {
                self.cursor_flush_at.get_or_insert(until);
            }
            CursorDecision::Unchanged => {}
        }
    }

    fn send_cursor(&mut self, position: CursorPosition) {
        self.out.send(ServerMessage::Cursor(position).encode());
        self.cursors_in_window += 1;
        self.last_cursor = Some(position);
    }

    /// When the session should call [`Self::on_deadline`].
    pub fn next_deadline(&self) -> Option<Instant> {
        self.cursor_flush_at.map(crate::clock::instant_at)
    }

    pub fn on_deadline(&mut self) {
        let Some(at) = self.cursor_flush_at else { return };
        let now = now_secs();
        if now < at {
            return;
        }
        self.cursor_flush_at = None;
        if !self.running || self.input_mode != InputMode::Trackpad {
            return;
        }
        if let Some(p) = self.cursor.flush(now) {
            self.send_cursor(p);
        } else if self.cursor.has_pending() {
            self.cursor_flush_at = Some(now + 0.002);
        }
    }

    // MARK: - Settings

    /// Applies new settings; restarts the pipeline (new STREAM_CONFIG) if the plan changes.
    pub fn update_settings(
        &mut self,
        settings: StreamSettings,
        encoders: &[sidecast_core::streaming::EncoderCapability],
    ) {
        if self.settings.touch_enabled && !settings.touch_enabled {
            self.send_input(InputCommand::Reset); // no button may stay held
        }
        self.settings = settings.clone();
        let bounds = BitrateBounds::new(settings.max_bitrate_mbps, self.client.limits.as_ref());
        let (bitrate, encoder) = {
            let mut s = lock(&self.shared);
            s.abr.set_bounds(bounds);
            (s.abr.bitrate(), s.encoder.clone())
        };
        if let Some(encoder) = encoder {
            let _ = encoder.send(EncoderJob::SetBitrate(bitrate as u32));
        }
        let planned = StreamPlanner::plan(
            &self.client.screen,
            &self.client.codecs,
            &settings,
            self.client.limits.as_ref(),
            encoders,
        );
        if let Ok(plan) = planned
            && self.running
            && plan != self.plan
        {
            log::info!(
                "stream: reconfiguring to {} {}x{}@{}",
                plan.codec,
                plan.encode_width,
                plan.encode_height,
                plan.fps
            );
            self.detach_encoder();
            if let Some(input) = self.input.take() {
                let _ = input.send(InputCommand::Stop);
            }
            if let Some(capture) = self.capture.take() {
                // Keep the stream slot: this stream still owns the display.
                release_capture(capture, None);
            }
            self.plan = plan;
            self.begin();
        }
    }

    // MARK: - Once per second

    /// Liveness, bitrate window and statistics.
    pub fn tick(&mut self) -> Result<Option<StreamStats>, StreamFailure> {
        let now = now_secs();
        if now - self.last_activity > LIVENESS_TIMEOUT {
            log::info!(
                "stream: nothing received from {} for {} s — closing",
                self.client.device_name,
                LIVENESS_TIMEOUT as u32
            );
            return Err(StreamFailure(None));
        }
        if !self.configured {
            return Ok(None);
        }
        let (changed, stats, encoder) = {
            let mut s = lock(&self.shared);
            let before = s.abr.bitrate();
            s.abr.tick(now);
            let stats = StreamStats {
                fps: self.frames_in_window as f64,
                mbps: self.bytes_in_window as f64 * 8.0 / 1e6,
                target_bitrate: s.abr.bitrate(),
                dropped: std::mem::take(&mut s.dropped_in_window),
                keyframes: self.keyframes_in_window,
                smoothed_rtt: s.abr.smoothed_rtt(),
                min_rtt: s.abr.min_rtt(),
                in_flight: s.tracker.in_flight(),
            };
            let changed = (s.abr.bitrate() != before).then(|| s.abr.bitrate());
            (changed, stats, s.encoder.clone())
        };
        self.frames_in_window = 0;
        self.bytes_in_window = 0;
        self.keyframes_in_window = 0;
        if self.log_stats
            && self.cursors_in_window > 0
            && let Some(c) = self.last_cursor
        {
            log::info!(
                "stream: CURSOR sent {} in the last second, last ({:.4}, {:.4})",
                self.cursors_in_window,
                c.x,
                c.y
            );
        }
        self.cursors_in_window = 0;
        if let (Some(bitrate), Some(encoder)) = (changed, encoder) {
            let _ = encoder.send(EncoderJob::SetBitrate(bitrate.clamp(0, u32::MAX as i64) as u32));
        }
        if self.log_stats {
            log::info!("{}", stats.log_line());
        }
        Ok(Some(stats))
    }
}

impl Drop for StreamController {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The first factory that supports the codec and opens.
fn create_encoder(
    factories: &[Arc<dyn crate::backend::EncoderFactory>],
    config: &EncoderConfig,
) -> Result<Box<dyn VideoEncoder>, BackendError> {
    let mut last_error = BackendError::encoder_failed(format!("no encoder for {}", config.codec));
    for factory in factories.iter().filter(|f| f.capabilities().iter().any(|c| c.codec == config.codec)) {
        match factory.create(config) {
            Ok(encoder) => {
                log::info!(
                    "stream: encoder {} {} {}x{}@{} {:.1} Mbps",
                    factory.name(),
                    config.codec,
                    config.width,
                    config.height,
                    config.fps,
                    config.bitrate as f64 / 1e6
                );
                return Ok(encoder);
            }
            Err(error) => {
                log::warn!("stream: encoder {} failed: {error}", factory.name());
                last_error = error;
            }
        }
    }
    Err(last_error)
}

/// Drops an event nobody will handle (the session ended), releasing a started
/// capture the same way a running one is released.
pub(crate) fn discard(event: StreamEvent) {
    if let StreamEvent::Started { result: Ok(started), .. } = event {
        release_capture(started.capture, Some(started.slot));
    }
}

/// Tears down a capture (and possibly the virtual display) off the async
/// runtime; the stream slot is released only afterwards, so the next stream
/// never overlaps the old display.
fn release_capture(capture: Box<dyn CaptureSession>, slot: Option<OwnedMutexGuard<()>>) {
    let teardown = move || {
        drop(capture);
        drop(slot);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(teardown);
        }
        Err(_) => teardown(),
    }
}

/// Encodes jobs in order; forces a keyframe at least every 2 s (PROTOCOL §6)
/// so every encoder gets that rule. Exits when the controller drops its sender,
/// releasing the slots of frames still queued.
fn spawn_encoder_thread(
    mut encoder: Box<dyn VideoEncoder>,
    jobs: mpsc::Receiver<EncoderJob>,
    shared: Arc<Mutex<Shared>>,
    generation: u64,
    events: UnboundedSender<StreamEvent>,
) {
    let spawned = std::thread::Builder::new().name("sidecast-encoder".into()).spawn(move || {
        let interval = Duration::from_secs_f64(KEYFRAME_INTERVAL);
        let mut last_keyframe: Option<Instant> = None;
        let mut failures = 0;
        while let Ok(job) = jobs.recv() {
            let (frame, requested_keyframe, job_generation) = match job {
                EncoderJob::SetBitrate(bps) => {
                    encoder.set_bitrate(bps);
                    continue;
                }
                EncoderJob::Encode { frame, requested_keyframe, generation } => (frame, requested_keyframe, generation),
            };
            if lock(&shared).generation != job_generation {
                lock(&shared).tracker.cancel_reservation(); // stream stopped or restarted meanwhile
                continue;
            }
            let due = last_keyframe.is_none_or(|t| t.elapsed() >= interval);
            let pts_micros = frame.pts_micros;
            let encoded = match encoder.encode(&frame, requested_keyframe || due) {
                Ok(encoded) => {
                    failures = 0;
                    encoded
                }
                Err(error) => {
                    failures += 1;
                    log::warn!("encoder: frame failed: {error}");
                    if failures >= 10 {
                        let _ = events.send(StreamEvent::EncoderFailed { generation, error });
                    }
                    None
                }
            };
            if encoded.as_ref().is_some_and(|e| e.is_keyframe) {
                last_keyframe = Some(Instant::now());
            }
            let event =
                StreamEvent::Encoded { generation: job_generation, frame: encoded, pts_micros, requested_keyframe };
            if events.send(event).is_err() {
                lock(&shared).tracker.cancel_reservation();
            }
        }
    });
    if let Err(error) = spawned {
        log::error!("encoder: cannot start its thread: {error}");
    }
}
