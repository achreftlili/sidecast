//! The input task of a stream: TOUCH → gesture engine → human-like click
//! timelines → the platform's [`InputSink`], plus TEXT and KEY, all on one
//! task so touches and keys stay in order (a click's 40 ms hold delays what
//! follows, as on the Mac). In trackpad mode it reports the pointer for CURSOR.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sidecast_core::input::{GestureAction, GestureEngine, InputEvent, KeyStroke, click_timeline, typing};
use sidecast_core::protocol::{CursorPosition, InputMode, KeyEvent, TouchAction, TouchEvent};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;

use crate::backend::{DisplayGeometry, InputSink};
use crate::stream::StreamEvent;

pub(crate) enum InputCommand {
    Touch(TouchEvent),
    Text(String),
    Key(KeyEvent),
    Mode(InputMode),
    /// Report the pointer again (forced), e.g. after a new STREAM_CONFIG.
    RefreshCursor,
    /// End any gesture and release held buttons (input turned off).
    Reset,
    Stop,
}

type SharedSink = Arc<Mutex<Box<dyn InputSink>>>;

fn lock(sink: &SharedSink) -> MutexGuard<'_, Box<dyn InputSink>> {
    sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn spawn(
    sink: Box<dyn InputSink>,
    geometry: DisplayGeometry,
    points_per_dp: f64,
    mode: InputMode,
    events: UnboundedSender<StreamEvent>,
) -> UnboundedSender<InputCommand> {
    let (tx, rx) = unbounded_channel();
    tokio::spawn(InputTask::new(sink, geometry, points_per_dp, mode, events).run(rx));
    tx
}

struct InputTask {
    sink: SharedSink,
    engine: GestureEngine,
    geometry: DisplayGeometry,
    mode: InputMode,
    events: UnboundedSender<StreamEvent>,
    last_reported: Option<CursorPosition>,
    timer: Option<(u64, Instant)>,
}

impl InputTask {
    fn new(
        sink: Box<dyn InputSink>,
        geometry: DisplayGeometry,
        points_per_dp: f64,
        mode: InputMode,
        events: UnboundedSender<StreamEvent>,
    ) -> Self {
        let sink: SharedSink = Arc::new(Mutex::new(sink));
        let mut engine = GestureEngine::default();
        engine.configure(geometry.size, points_per_dp);
        let provider = sink.clone();
        // Trackpad mode starts from the real pointer when it is on our display.
        engine.set_cursor_provider(move || lock(&provider).pointer_position());
        let _ = engine.set_mode(mode);
        InputTask { sink, engine, geometry, mode, events, last_reported: None, timer: None }
    }

    async fn run(mut self, mut commands: UnboundedReceiver<InputCommand>) {
        let mut sampler = tokio::time::interval(Duration::from_millis(100));
        sampler.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        if self.mode == InputMode::Trackpad {
            self.report_system_pointer(true);
        }
        loop {
            let timer = self.timer;
            tokio::select! {
                command = commands.recv() => match command {
                    None | Some(InputCommand::Stop) => break,
                    Some(command) => self.handle(command).await,
                },
                _ = tokio::time::sleep_until(timer.map_or_else(Instant::now, |t| t.1)), if timer.is_some() => {
                    if let Some((generation, _)) = timer {
                        let actions = self.engine.fire_timer(generation);
                        self.apply_all(actions).await;
                    }
                }
                // The user may move the pointer with the computer's own mouse.
                _ = sampler.tick(), if self.mode == InputMode::Trackpad => self.report_system_pointer(false),
            }
            self.sync_timer();
        }
        let actions = self.engine.reset();
        self.apply_all(actions).await;
        lock(&self.sink).release_all();
    }

    /// Arms the momentum timer the engine asked for (a new generation replaces the old one).
    fn sync_timer(&mut self) {
        self.timer = match (self.engine.pending_timer(), self.timer) {
            (None, _) => None,
            (Some(t), Some(armed)) if armed.0 == t.generation => Some(armed),
            (Some(t), _) => Some((t.generation, Instant::now() + Duration::from_secs_f64(t.delay))),
        };
    }

    async fn handle(&mut self, command: InputCommand) {
        match command {
            InputCommand::Touch(event) => {
                if event.touch_action() != Some(TouchAction::Move) {
                    log::debug!(
                        "input: touch action={} t={} pointers={}",
                        event.action,
                        event.event_time_ms,
                        event.pointers.len()
                    );
                }
                let actions = self.engine.handle(&event);
                self.apply_all(actions).await;
            }
            InputCommand::Text(text) => {
                let text = typing::remove_control_characters(&text);
                if !text.is_empty() {
                    lock(&self.sink).inject(&InputEvent::Text(text));
                }
            }
            InputCommand::Key(event) => match KeyStroke::from_event(&event) {
                Some(stroke) => {
                    lock(&self.sink).inject(&InputEvent::Key { key: stroke.key, modifiers: stroke.modifiers })
                }
                None => log::debug!("input: ignoring unknown KEY {}", event.key),
            },
            InputCommand::Mode(mode) => {
                let actions = self.engine.set_mode(mode);
                self.apply_all(actions).await;
                self.mode = mode;
                self.last_reported = None;
                if mode == InputMode::Trackpad {
                    self.report_system_pointer(true);
                }
            }
            InputCommand::RefreshCursor => {
                if self.mode == InputMode::Trackpad {
                    self.report_system_pointer(true);
                }
            }
            InputCommand::Reset => {
                let actions = self.engine.reset();
                self.apply_all(actions).await;
                lock(&self.sink).release_all();
            }
            InputCommand::Stop => {}
        }
    }

    async fn apply_all(&mut self, actions: Vec<GestureAction>) {
        for action in actions {
            self.apply(action).await;
        }
    }

    async fn apply(&mut self, action: GestureAction) {
        lock(&self.sink).observe_gesture(&action);
        if self.mode == InputMode::Trackpad
            && let Some(p) = action.pointer_location()
        {
            // Trackpad mode: the phone draws the pointer (CURSOR).
            self.report(CursorPosition::from_point(p, self.geometry.size), false);
        }
        if let Some(steps) = click_timeline(&action, self.engine.thresholds.click_timing) {
            for step in steps {
                if step.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(step.delay_ms.into())).await;
                }
                lock(&self.sink).inject(&InputEvent::Pointer(step.event));
            }
            return;
        }
        let event = match action {
            GestureAction::Scroll { dx, dy, at, phase } => InputEvent::Scroll { dx, dy, at, phase },
            GestureAction::Zoom { delta, at } => InputEvent::Zoom { delta, at },
            GestureAction::Semantic(semantic) => InputEvent::Semantic(semantic),
            _ => return,
        };
        lock(&self.sink).inject(&event);
    }

    fn report_system_pointer(&mut self, force: bool) {
        let position = lock(&self.sink).pointer_position();
        let normalized =
            position.map_or(CursorPosition::OFF_DISPLAY, |p| CursorPosition::from_point(p, self.geometry.size));
        self.report(normalized, force);
    }

    fn report(&mut self, position: CursorPosition, force: bool) {
        if !force && Some(position) == self.last_reported {
            return;
        }
        self.last_reported = Some(position);
        let _ = self.events.send(StreamEvent::Cursor { position, force });
    }
}
