//! TOUCH → pointer gestures, ported from the Mac app's `GestureEngine.swift`
//! (itself ported from SideScreen's gesture state machine):
//!
//! - tap → click; second tap nearby within 0.4 s → double click
//! - hold still for 0.5 s → the left button goes down where the finger landed
//!   and stays down until the finger lifts: lift in place = a (long) left
//!   click, move = left drag
//! - one-finger move → scroll (with momentum after a flick)
//! - two-finger move → scroll (with momentum); two-finger pinch → zoom
//! - two-finger tap (all fingers up within 0.3 s, no scroll/pinch) → right
//!   click where the first finger landed (where the cursor already is)
//! - 3 or 4 fingers: any 1/2-finger gesture ends (scroll ended without
//!   momentum, button released, no click); a centroid swipe past 60 dp emits
//!   exactly one semantic action for the whole gesture: left → next desktop,
//!   right → previous desktop, up → overview, down → app windows; a 3-finger
//!   tap does nothing (the phone uses it locally)
//!
//! Trackpad mode (PROTOCOL §7.3) keeps a cursor of its own and moves it by
//! finger deltas × an accelerated gain: tap = click at the cursor, double tap
//! = double click, tap then touch again and hold still for 0.5 s = left button
//! held at the cursor until the finger lifts (move = drag), double-tap-and-hold
//! then move = drag, two-finger tap = right click at the cursor, two-finger
//! move/pinch = scroll/zoom at the cursor, 3/4-finger swipes as in touch mode.
//! A single resting finger never presses the button there.
//!
//! Every gesture's outcome (click, scroll, drag, pointer move, or why nothing
//! happened) is logged at debug level.
//!
//! Durations use the client's `eventTimeMs` (immune to network jitter).
//! The long press and momentum need a timer (the button must go down while
//! the finger is still resting, before any further event arrives): after
//! every call the driver checks [`GestureEngine::pending_timer`] and calls
//! [`GestureEngine::fire_timer`] once its delay has passed. `reset` releases
//! a held button.

use super::{GestureAction, GestureThresholds, MouseButton, ScrollPhase, SemanticAction};
use crate::geometry::{Point, Size};
use crate::protocol::{InputMode, TouchAction, TouchEvent, TouchPointer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// One finger down, gesture undecided.
    Pending,
    /// One-finger scroll.
    Scrolling,
    /// Long press, finger still in place: left button held.
    Holding,
    /// Long press + move: left button held.
    Dragging,
    TwoFingerPending,
    TwoFingerScroll,
    Pinching,
    /// One of two tapping fingers lifted; right click if the other lifts in time.
    TwoFingerTapPending,
    /// 3+ fingers: swipe navigation only, until all fingers lift.
    MultiFinger,
    /// A multi-finger gesture ended; ignore until all fingers lift.
    WaitingForRelease,
    // Trackpad mode, one finger
    /// Down, moving the cursor; still a tap if it lifts quickly in place.
    TrackpadPending,
    /// Moved past the slop: only moves the cursor.
    TrackpadPointing,
    /// Second touch right after a tap: double tap, hold, or drag once it moves.
    TrackpadTapHold,
    /// Tap then hold, finger still in place: left button held at the cursor.
    TrackpadHolding,
    /// Left button held, cursor follows the finger.
    TrackpadDragging,
}

/// What the engine's single timer is armed for: a long press while one
/// finger rests, momentum after it lifts (a new touch stops momentum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timer {
    LongPress,
    Momentum,
}

/// A timer the engine wants: call [`GestureEngine::fire_timer`] with
/// `generation` after `delay` seconds, unless a later call replaced or
/// cancelled it (check `pending_timer` again after every call).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingTimer {
    pub generation: u64,
    pub delay: f64,
}

type CursorProvider = Box<dyn FnMut() -> Option<Point> + Send>;

/// The gesture state machine. Not thread-safe; drive it from one task.
pub struct GestureEngine {
    pub thresholds: GestureThresholds,
    display_size: Size,
    points_per_dp: f64,
    out: Vec<GestureAction>,

    state: State,
    primary_id: u8,
    start_point: Point,
    last_point: Point,
    start_time: u32,
    last_move_time: u32,
    scroll_began: bool,
    /// Scroll units per second.
    velocity: (f64, f64),
    last_tap: Option<(u32, Point)>,

    start_midpoint: Point,
    last_midpoint: Point,
    initial_pinch_distance: f64,
    last_pinch_distance: f64,
    zoom_remainder: f64,

    timer: Option<(u64, Timer)>,
    timer_generation: u64,
    /// The long-press timer pressed the button since the last `take_hold_started`.
    hold_started: bool,
    /// Per tick.
    momentum_step: (f64, f64),
    momentum_point: Point,
    momentum_started: bool,

    // Two-finger tap
    two_finger_tap_eligible: bool,
    secondary_click_point: Point,
    tap_remaining: Option<(u8, Point)>,

    // 3/4-finger swipe
    /// Display points.
    swipe_travel: (f64, f64),
    last_centroid: Point,
    swipe_ids: Vec<u8>,
    navigated: bool,

    // Trackpad mode
    mode: InputMode,
    cursor_provider: Option<CursorProvider>,
    cursor: Point,
    cursor_initialized: bool,
    cursor_at_down: Point,
    /// dp/s, smoothed.
    finger_speed: f64,
}

impl std::fmt::Debug for GestureEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GestureEngine")
            .field("mode", &self.mode)
            .field("state", &self.state)
            .field("display_size", &self.display_size)
            .finish_non_exhaustive()
    }
}

impl Default for GestureEngine {
    fn default() -> Self {
        Self::new(GestureThresholds::default())
    }
}

impl GestureEngine {
    pub fn new(thresholds: GestureThresholds) -> Self {
        GestureEngine {
            thresholds,
            display_size: Size::ZERO,
            points_per_dp: 1.0,
            out: Vec::new(),
            state: State::Idle,
            primary_id: 0,
            start_point: Point::ZERO,
            last_point: Point::ZERO,
            start_time: 0,
            last_move_time: 0,
            scroll_began: false,
            velocity: (0.0, 0.0),
            last_tap: None,
            start_midpoint: Point::ZERO,
            last_midpoint: Point::ZERO,
            initial_pinch_distance: 0.0,
            last_pinch_distance: 0.0,
            zoom_remainder: 0.0,
            timer: None,
            timer_generation: 0,
            hold_started: false,
            momentum_step: (0.0, 0.0),
            momentum_point: Point::ZERO,
            momentum_started: false,
            two_finger_tap_eligible: false,
            secondary_click_point: Point::ZERO,
            tap_remaining: None,
            swipe_travel: (0.0, 0.0),
            last_centroid: Point::ZERO,
            swipe_ids: Vec::new(),
            navigated: false,
            mode: InputMode::Touch,
            cursor_provider: None,
            cursor: Point::ZERO,
            cursor_initialized: false,
            cursor_at_down: Point::ZERO,
            finger_speed: 0.0,
        }
    }

    /// Size of the target display in points; `points_per_dp` scales the
    /// distance thresholds (display points per phone dp).
    pub fn configure(&mut self, display_size: Size, points_per_dp: f64) {
        self.display_size = display_size;
        self.points_per_dp = if points_per_dp > 0.0 && points_per_dp.is_finite() { points_per_dp } else { 1.0 };
    }

    pub fn display_size(&self) -> Size {
        self.display_size
    }

    pub fn points_per_dp(&self) -> f64 {
        self.points_per_dp
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    /// The system pointer, display-local, if it is on this display: trackpad
    /// mode starts from it at every touch-down.
    pub fn set_cursor_provider(&mut self, provider: impl FnMut() -> Option<Point> + Send + 'static) {
        self.cursor_provider = Some(Box::new(provider));
    }

    /// Trackpad cursor (display-local points); meaningful once [`Self::has_cursor`].
    pub fn cursor(&self) -> Point {
        self.cursor
    }

    /// The trackpad cursor has been placed (first trackpad touch).
    pub fn has_cursor(&self) -> bool {
        self.cursor_initialized
    }

    /// The engine holds the left button down (long press or drag in progress).
    pub fn is_button_held(&self) -> bool {
        matches!(self.state, State::Holding | State::Dragging | State::TrackpadHolding | State::TrackpadDragging)
    }

    /// True once after the long-press timer pressed the left button on a
    /// resting finger (touch hold or trackpad tap-then-hold): the phone can
    /// confirm it with a haptic tick (HAPTIC). Never set when the press is only
    /// inferred late from the phone's clock.
    pub fn take_hold_started(&mut self) -> bool {
        std::mem::take(&mut self.hold_started)
    }

    /// The timer to arm (long press or momentum), if any.
    pub fn pending_timer(&self) -> Option<PendingTimer> {
        self.timer.map(|(generation, timer)| PendingTimer {
            generation,
            delay: match timer {
                Timer::LongPress => self.thresholds.long_press_time,
                Timer::Momentum => self.thresholds.momentum_tick,
            },
        })
    }

    /// Runs the timer armed as `generation`; stale generations do nothing.
    pub fn fire_timer(&mut self, generation: u64) -> Vec<GestureAction> {
        if let Some((armed, timer)) = self.timer
            && armed == generation
        {
            self.timer = None;
            match timer {
                Timer::LongPress => self.long_press_elapsed(),
                Timer::Momentum => self.momentum_tick(),
            }
        }
        self.take_output()
    }

    fn arm(&mut self, timer: Timer) {
        self.timer_generation += 1;
        self.timer = Some((self.timer_generation, timer));
    }

    fn disarm(&mut self, timer: Timer) {
        if self.timer.is_some_and(|(_, armed)| armed == timer) {
            self.timer = None;
        }
    }

    /// Switches touch ↔ trackpad. A real change ends the current gesture
    /// cleanly (button released, scroll ended, momentum stopped) and forgets taps.
    pub fn set_mode(&mut self, mode: InputMode) -> Vec<GestureAction> {
        if mode != self.mode {
            self.reset_inner();
            self.mode = mode;
        }
        self.take_output()
    }

    /// Ends any gesture: releases a held button, stops momentum, forgets taps.
    /// Call on disconnect so no button stays stuck (SideScreen #71).
    pub fn reset(&mut self) -> Vec<GestureAction> {
        self.reset_inner();
        self.take_output()
    }

    pub fn handle(&mut self, event: &TouchEvent) -> Vec<GestureAction> {
        if !self.display_size.is_usable() || event.pointers.is_empty() {
            return Vec::new();
        }
        let Some(action) = event.touch_action() else { return Vec::new() };
        match action {
            TouchAction::Down => {
                if self.mode == InputMode::Trackpad {
                    self.trackpad_down(event)
                } else {
                    self.finger_down(event)
                }
            }
            TouchAction::Move => {
                if self.state == State::MultiFinger {
                    self.multi_finger_move(event)
                } else if self.state == State::TwoFingerTapPending {
                    self.two_finger_tap_move(event)
                } else if self.is_two_finger_state() && event.pointers.len() >= 2 {
                    self.two_finger_move(event)
                } else if self.is_trackpad_one_finger_state() {
                    self.trackpad_move(event)
                } else if self.is_one_finger_state() {
                    self.one_finger_move(event)
                }
            }
            TouchAction::Up => {
                if self.is_trackpad_one_finger_state() {
                    self.trackpad_up(event)
                } else {
                    self.last_finger_up(event)
                }
            }
            TouchAction::PointerDown => self.pointer_down(event),
            TouchAction::PointerUp => self.pointer_up(event),
            TouchAction::Cancel => {
                self.stop_momentum();
                self.end_current_gesture();
            }
        }
        self.take_output()
    }

    fn take_output(&mut self) -> Vec<GestureAction> {
        std::mem::take(&mut self.out)
    }

    fn emit(&mut self, action: GestureAction) {
        self.out.push(action);
    }

    fn reset_inner(&mut self) {
        self.stop_momentum();
        self.end_current_gesture();
        self.last_tap = None;
    }

    // MARK: - One finger

    fn is_one_finger_state(&self) -> bool {
        matches!(self.state, State::Pending | State::Scrolling | State::Holding | State::Dragging)
    }

    fn is_two_finger_state(&self) -> bool {
        matches!(self.state, State::TwoFingerPending | State::TwoFingerScroll | State::Pinching)
    }

    fn finger_down(&mut self, event: &TouchEvent) {
        self.stop_momentum();
        if self.state != State::Idle {
            self.end_current_gesture(); // a missed UP must not leave a button held
        }
        let pointer = action_pointer(event);
        let p = self.point(pointer);
        self.primary_id = pointer.id;
        self.start_point = p;
        self.last_point = p;
        self.start_time = event.event_time_ms;
        self.last_move_time = event.event_time_ms;
        self.velocity = (0.0, 0.0);
        self.state = State::Pending;
        self.emit(GestureAction::Move { to: p });
        self.arm(Timer::LongPress);
    }

    /// The finger rested in place for `long_press_time`: press the left
    /// button (where it landed, or at the trackpad cursor) and keep it down
    /// until the finger lifts or moves.
    fn long_press_elapsed(&mut self) {
        match self.state {
            State::Pending => {
                log::debug!("left button down: long press");
                self.state = State::Holding;
                self.emit(GestureAction::ButtonDown { button: MouseButton::Left, at: self.start_point });
            }
            State::TrackpadTapHold => {
                log::debug!("trackpad: left button down: tap then hold");
                self.state = State::TrackpadHolding;
                self.last_tap = None;
                self.emit(GestureAction::ButtonDown { button: MouseButton::Left, at: self.cursor });
            }
            _ => return,
        }
        self.hold_started = true;
    }

    fn one_finger_move(&mut self, event: &TouchEvent) {
        let p = self.point(self.primary_pointer(event));
        let t = event.event_time_ms;
        let dt = elapsed(self.last_move_time, t);
        if dt < self.thresholds.min_move_interval {
            return; // coalesce; the delta accumulates
        }
        match self.state {
            State::Pending => {
                if p.distance(self.start_point) > self.slop(self.thresholds.tap_max_distance) {
                    self.disarm(Timer::LongPress);
                    if elapsed(self.start_time, t) >= self.thresholds.long_press_time {
                        // Held long enough by the phone's clock, but the timer has not run yet.
                        log::debug!("drag: long press {} ms", ms(self.start_time, t));
                        self.state = State::Dragging;
                        self.emit(GestureAction::ButtonDown { button: MouseButton::Left, at: self.start_point });
                        self.emit(GestureAction::Drag { to: p });
                    } else {
                        log::debug!("scroll after {} ms", ms(self.start_time, t));
                        self.state = State::Scrolling;
                        self.scroll_began = false;
                        self.emit_scroll(p.x - self.last_point.x, p.y - self.last_point.y, p, dt);
                    }
                }
            }
            // Jitter within the slop leaves the press where it is.
            State::Holding => {
                if p.distance(self.start_point) > self.slop(self.thresholds.tap_max_distance) {
                    log::debug!("drag: after a long press");
                    self.state = State::Dragging;
                    self.emit(GestureAction::Drag { to: p });
                }
            }
            State::Scrolling => self.emit_scroll(p.x - self.last_point.x, p.y - self.last_point.y, p, dt),
            State::Dragging => self.emit(GestureAction::Drag { to: p }),
            _ => {}
        }
        self.last_point = p;
        self.last_move_time = t;
    }

    fn last_finger_up(&mut self, event: &TouchEvent) {
        let p = self.point(self.primary_pointer(event));
        let t = event.event_time_ms;
        self.disarm(Timer::LongPress);
        match self.state {
            State::Pending => {
                let away = p.distance(self.start_point);
                if away > self.slop(self.thresholds.tap_max_distance) {
                    // Lifted beyond the slop before any MOVE got that far.
                    log::debug!("no click: lifted {} dp away after {} ms", self.dp(away), ms(self.start_time, t));
                } else if elapsed(self.start_time, t) >= self.thresholds.long_press_time {
                    // Held long enough by the phone's clock, but the timer has not run yet:
                    // the left click the held button would have made.
                    log::debug!("click: long press {} ms", ms(self.start_time, t));
                    self.emit(GestureAction::Click { at: self.start_point, count: 1 });
                } else {
                    log::debug!("tap after {} ms", ms(self.start_time, t));
                    self.tap(p, t);
                }
            }
            State::Holding => {
                log::debug!("left button up: long press {} ms", ms(self.start_time, t));
                self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.start_point });
            }
            State::Scrolling => {
                self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at: p, phase: ScrollPhase::Ended });
                self.start_momentum_if_flicked(p, t);
            }
            State::Dragging => {
                self.emit(GestureAction::Drag { to: p });
                self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: p });
            }
            State::TwoFingerScroll => {
                let at = self.anchor(self.last_midpoint);
                self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at, phase: ScrollPhase::Ended });
            }
            State::TwoFingerTapPending => {
                if let Some((_, tap_point)) = self.tap_remaining
                    && elapsed(self.start_time, t) <= self.thresholds.two_finger_tap_max_duration
                    && p.distance(tap_point) <= self.slop(self.thresholds.tap_max_distance)
                {
                    log::debug!("right click: two-finger tap {} ms", ms(self.start_time, t));
                    if self.mode == InputMode::Trackpad {
                        self.cursor = self.secondary_click_point;
                    }
                    self.emit(GestureAction::RightClick { at: self.secondary_click_point });
                    self.last_tap = None;
                } else {
                    log::debug!("no right click: two-finger tap took {} ms", ms(self.start_time, t));
                }
            }
            // MultiFinger: the swipe (if any) already navigated; a tap does nothing.
            _ => {}
        }
        self.tap_remaining = None;
        self.state = State::Idle;
    }

    fn tap(&mut self, p: Point, time: u32) {
        if let Some((last_time, last_point)) = self.last_tap
            && elapsed(last_time, time) <= self.thresholds.double_tap_max_interval
            && p.distance(last_point) <= self.slop(self.thresholds.double_tap_max_distance)
        {
            self.emit(GestureAction::Click { at: last_point, count: 2 });
            self.last_tap = None; // a third tap starts over
        } else {
            self.emit(GestureAction::Click { at: p, count: 1 });
            self.last_tap = Some((time, p));
        }
    }

    // MARK: - Two fingers

    fn pointer_down(&mut self, event: &TouchEvent) {
        if self.state == State::MultiFinger {
            self.rebaseline_swipe(&event.pointers); // a 4th finger: no jump in the centroid
            return;
        }
        if event.pointers.len() >= 3 {
            self.begin_multi_finger(&event.pointers);
            return;
        }
        if event.pointers.len() < 2 {
            return;
        }
        if self.is_two_finger_state() {
            self.rebaseline(event.pointers[0], event.pointers[1], event.event_time_ms);
            return;
        }
        // Only an undecided one-finger touch can turn into a two-finger tap;
        // in trackpad mode it right-clicks at the cursor (where it was at touch-down).
        self.two_finger_tap_eligible = matches!(self.state, State::Pending | State::TrackpadPending);
        self.secondary_click_point =
            if self.mode == InputMode::Trackpad { self.cursor_at_down } else { self.last_point };
        self.stop_momentum();
        self.end_current_gesture();
        self.rebaseline(event.pointers[0], event.pointers[1], event.event_time_ms);
        self.start_midpoint = self.last_midpoint;
        self.initial_pinch_distance = self.last_pinch_distance;
        self.state = State::TwoFingerPending;
    }

    fn two_finger_move(&mut self, event: &TouchEvent) {
        let p1 = self.point(event.pointers[0]);
        let p2 = self.point(event.pointers[1]);
        let d = p1.distance(p2);
        let mid = p1.midpoint(p2);
        let t = event.event_time_ms;
        let dt = elapsed(self.last_move_time, t);
        if dt < self.thresholds.min_move_interval {
            return;
        }
        if self.state == State::TwoFingerPending {
            if (d - self.initial_pinch_distance).abs() > self.slop(self.thresholds.pinch_min_distance) {
                log::debug!("pinch");
                self.state = State::Pinching;
                self.zoom_remainder = 0.0;
            } else if mid.distance(self.start_midpoint) > self.slop(self.thresholds.tap_max_distance) {
                log::debug!("two-finger scroll");
                self.state = State::TwoFingerScroll;
                self.scroll_began = false;
                self.velocity = (0.0, 0.0);
            }
        }
        match self.state {
            State::Pinching => {
                self.zoom_remainder += (d - self.last_pinch_distance) * self.thresholds.zoom_sensitivity;
                let step = self.zoom_remainder.trunc(); // toward zero; the remainder carries over
                if step != 0.0 {
                    let at = self.anchor(mid);
                    self.emit(GestureAction::Zoom { delta: step as i32, at });
                    self.zoom_remainder -= step;
                }
            }
            State::TwoFingerScroll => {
                let at = self.anchor(mid);
                self.emit_scroll(mid.x - self.last_midpoint.x, mid.y - self.last_midpoint.y, at, dt);
            }
            _ => {}
        }
        self.last_pinch_distance = d;
        self.last_midpoint = mid;
        self.last_move_time = t;
    }

    fn pointer_up(&mut self, event: &TouchEvent) {
        let remaining: Vec<TouchPointer> =
            event.pointers.iter().copied().filter(|p| p.id != event.action_pointer_id).collect();
        if self.state == State::MultiFinger {
            if !remaining.is_empty() {
                self.rebaseline_swipe(&remaining);
            }
            return; // stays a multi-finger gesture until the last finger lifts
        }
        if self.is_two_finger_state() && remaining.len() >= 2 {
            self.rebaseline(remaining[0], remaining[1], event.event_time_ms);
            return;
        }
        match self.state {
            State::TwoFingerPending => {
                if self.two_finger_tap_eligible
                    && let Some(finger) = remaining.first()
                    && elapsed(self.start_time, event.event_time_ms) <= self.thresholds.two_finger_tap_max_duration
                {
                    self.tap_remaining = Some((finger.id, self.point(*finger)));
                    self.state = State::TwoFingerTapPending;
                    return;
                }
                if self.two_finger_tap_eligible {
                    log::debug!("no right click: two fingers held {} ms", ms(self.start_time, event.event_time_ms));
                } else {
                    log::debug!("no right click: the first finger was already scrolling or pressing");
                }
            }
            State::TwoFingerScroll => {
                let at = self.anchor(self.last_midpoint);
                self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at, phase: ScrollPhase::Ended });
                self.start_momentum_if_flicked(at, event.event_time_ms);
            }
            State::Holding | State::Dragging | State::Scrolling | State::TrackpadHolding | State::TrackpadDragging => {
                self.end_current_gesture()
            }
            _ => {}
        }
        self.state = State::WaitingForRelease;
    }

    /// The finger still down after a quick two-finger tap must not wander off.
    fn two_finger_tap_move(&mut self, event: &TouchEvent) {
        let Some((id, tap_point)) = self.tap_remaining else { return };
        let finger = event.pointers.iter().copied().find(|p| p.id == id).unwrap_or(event.pointers[0]);
        if self.point(finger).distance(tap_point) > self.slop(self.thresholds.tap_max_distance) {
            log::debug!("no right click: the remaining finger moved");
            self.tap_remaining = None;
            self.state = State::WaitingForRelease;
        }
    }

    // MARK: - Three or four fingers

    fn begin_multi_finger(&mut self, pointers: &[TouchPointer]) {
        self.stop_momentum();
        self.end_current_gesture(); // scroll ended without momentum, held button released, no click
        self.last_tap = None;
        self.tap_remaining = None;
        self.two_finger_tap_eligible = false;
        self.navigated = false;
        self.swipe_travel = (0.0, 0.0);
        self.rebaseline_swipe(pointers);
        self.state = State::MultiFinger;
    }

    /// Restarts centroid tracking for a new finger set, keeping the travel so far.
    fn rebaseline_swipe(&mut self, pointers: &[TouchPointer]) {
        self.last_centroid = self.centroid(pointers);
        self.swipe_ids = sorted_ids(pointers);
    }

    fn multi_finger_move(&mut self, event: &TouchEvent) {
        if sorted_ids(&event.pointers) != self.swipe_ids {
            // Finger set changed: never count the centroid jump.
            self.rebaseline_swipe(&event.pointers);
            return;
        }
        let c = self.centroid(&event.pointers);
        self.swipe_travel.0 += c.x - self.last_centroid.x;
        self.swipe_travel.1 += c.y - self.last_centroid.y;
        self.last_centroid = c;
        if self.navigated {
            return; // one navigation per gesture
        }
        let (ax, ay) = (self.swipe_travel.0.abs(), self.swipe_travel.1.abs());
        if ax.max(ay) <= self.slop(self.thresholds.swipe_min_distance) {
            return;
        }
        self.navigated = true;
        // Content follows the fingers, as on a trackpad.
        let action = if ax >= ay {
            if self.swipe_travel.0 < 0.0 { SemanticAction::DesktopNext } else { SemanticAction::DesktopPrevious }
        } else if self.swipe_travel.1 < 0.0 {
            SemanticAction::Overview
        } else {
            SemanticAction::AppWindows
        };
        log::debug!("swipe: {action:?}");
        self.emit(GestureAction::Semantic(action));
    }

    fn centroid(&self, pointers: &[TouchPointer]) -> Point {
        if pointers.is_empty() {
            return Point::ZERO;
        }
        let (mut x, mut y) = (0.0, 0.0);
        for pointer in pointers {
            let q = self.point(*pointer);
            x += q.x;
            y += q.y;
        }
        let n = pointers.len() as f64;
        Point::new(x / n, y / n)
    }

    /// Restarts distance/midpoint tracking (finger set changed) without a jump.
    fn rebaseline(&mut self, a: TouchPointer, b: TouchPointer, time: u32) {
        let (p1, p2) = (self.point(a), self.point(b));
        self.last_pinch_distance = p1.distance(p2);
        self.last_midpoint = p1.midpoint(p2);
        self.last_move_time = time;
    }

    // MARK: - Scrolling and momentum

    fn emit_scroll(&mut self, dx: f64, dy: f64, at: Point, dt: f64) {
        let sx = dx * self.thresholds.scroll_sensitivity;
        let sy = dy * self.thresholds.scroll_sensitivity;
        let phase = if self.scroll_began { ScrollPhase::Changed } else { ScrollPhase::Began };
        self.emit(GestureAction::Scroll { dx: sx, dy: sy, at, phase });
        self.scroll_began = true;
        if dt > 0.0 && dt < 0.1 {
            let inst = (sx / dt, sy / dt);
            self.velocity = if self.velocity == (0.0, 0.0) {
                inst
            } else {
                (0.6 * inst.0 + 0.4 * self.velocity.0, 0.6 * inst.1 + 0.4 * self.velocity.1)
            };
        } else {
            self.velocity = (0.0, 0.0); // after a pause the old speed means nothing
        }
    }

    fn start_momentum_if_flicked(&mut self, at: Point, up_time: u32) {
        if elapsed(self.last_move_time, up_time) > self.thresholds.momentum_max_idle {
            return;
        }
        let k = self.thresholds.momentum_tick * self.thresholds.momentum_gain;
        let step = (self.velocity.0 * k, self.velocity.1 * k);
        if step.0.abs().max(step.1.abs()) <= self.thresholds.momentum_start_threshold {
            return;
        }
        self.momentum_step = step;
        self.momentum_point = at;
        self.momentum_started = false;
        self.schedule_momentum_tick();
    }

    fn schedule_momentum_tick(&mut self) {
        self.arm(Timer::Momentum);
    }

    fn momentum_tick(&mut self) {
        let (dx, dy) = self.momentum_step;
        if dx.abs().max(dy.abs()) < self.thresholds.momentum_stop_threshold {
            self.finish_momentum();
            return;
        }
        let phase = if self.momentum_started { ScrollPhase::Momentum } else { ScrollPhase::MomentumBegan };
        self.emit(GestureAction::Scroll { dx, dy, at: self.momentum_point, phase });
        self.momentum_started = true;
        self.momentum_step = (dx * self.thresholds.momentum_decay, dy * self.thresholds.momentum_decay);
        self.schedule_momentum_tick();
    }

    fn finish_momentum(&mut self) {
        if self.momentum_started {
            let at = self.momentum_point;
            self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at, phase: ScrollPhase::MomentumEnded });
        }
        self.momentum_started = false;
        self.momentum_step = (0.0, 0.0);
    }

    fn stop_momentum(&mut self) {
        self.disarm(Timer::Momentum);
        self.finish_momentum();
    }

    /// Closes the current gesture cleanly (button up, scroll ended) and goes idle.
    fn end_current_gesture(&mut self) {
        self.disarm(Timer::LongPress);
        match self.state {
            State::Holding => self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.start_point }),
            State::Dragging => self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.last_point }),
            State::Scrolling => {
                let at = self.last_point;
                self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at, phase: ScrollPhase::Ended });
            }
            State::TwoFingerScroll => {
                let at = self.anchor(self.last_midpoint);
                self.emit(GestureAction::Scroll { dx: 0.0, dy: 0.0, at, phase: ScrollPhase::Ended });
            }
            State::TrackpadHolding | State::TrackpadDragging => {
                self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.cursor })
            }
            _ => {}
        }
        self.state = State::Idle;
    }

    // MARK: - Trackpad mode, one finger

    fn is_trackpad_one_finger_state(&self) -> bool {
        matches!(
            self.state,
            State::TrackpadPending
                | State::TrackpadPointing
                | State::TrackpadTapHold
                | State::TrackpadHolding
                | State::TrackpadDragging
        )
    }

    /// Where two-finger scroll/zoom act: the cursor in trackpad mode, the fingers in touch mode.
    fn anchor(&self, fingers: Point) -> Point {
        if self.mode == InputMode::Trackpad { self.cursor } else { fingers }
    }

    /// Picks up the system cursor if it is on this display; otherwise keeps ours (centre at first).
    fn sync_cursor(&mut self) {
        let (w, h) = (self.display_size.width, self.display_size.height);
        let system = self.cursor_provider.as_mut().and_then(|provider| provider());
        match system {
            Some(p) if p.x >= 0.0 && p.y >= 0.0 && p.x < w && p.y < h => {
                self.cursor = p;
                self.cursor_initialized = true;
            }
            _ if !self.cursor_initialized => {
                self.cursor = Point::new(w / 2.0, h / 2.0);
                self.cursor_initialized = true;
            }
            _ => {}
        }
        self.cursor = self.clamp_to_display(self.cursor);
    }

    fn trackpad_down(&mut self, event: &TouchEvent) {
        self.stop_momentum();
        if self.state != State::Idle {
            self.end_current_gesture();
        }
        self.sync_cursor();
        let pointer = action_pointer(event);
        let p = self.point(pointer);
        self.primary_id = pointer.id;
        self.start_point = p;
        self.last_point = p;
        self.start_time = event.event_time_ms;
        self.last_move_time = event.event_time_ms;
        self.cursor_at_down = self.cursor;
        self.finger_speed = 0.0;
        match self.last_tap {
            Some((time, _)) if elapsed(time, event.event_time_ms) <= self.thresholds.trackpad_double_tap_interval => {
                self.state = State::TrackpadTapHold;
                self.arm(Timer::LongPress); // resting after a tap presses the button
            }
            _ => {
                self.last_tap = None;
                self.state = State::TrackpadPending;
            }
        }
    }

    fn trackpad_move(&mut self, event: &TouchEvent) {
        let p = self.point(self.primary_pointer(event));
        let t = event.event_time_ms;
        let dt = elapsed(self.last_move_time, t);
        if dt < self.thresholds.min_move_interval {
            return; // coalesce; the delta accumulates
        }
        let beyond_slop = p.distance(self.start_point) > self.slop(self.thresholds.tap_max_distance);
        let (dx, dy) = (p.x - self.last_point.x, p.y - self.last_point.y);
        match self.state {
            State::TrackpadPending => {
                // The cursor follows at once (precise nudges); a tap snaps back to where it started.
                if beyond_slop {
                    log::debug!("trackpad: pointer moves");
                    self.state = State::TrackpadPointing;
                }
                self.move_cursor(dx, dy, dt);
                self.emit(GestureAction::Move { to: self.cursor });
            }
            State::TrackpadPointing => {
                self.move_cursor(dx, dy, dt);
                self.emit(GestureAction::Move { to: self.cursor });
            }
            State::TrackpadTapHold => {
                // Undecided: keep the cursor still (a double tap clicks where the first tap did).
                // Point and time both stay at touch-down, so the travel applied once it moves
                // is timed over the whole hold (no false acceleration from the jump).
                if !beyond_slop {
                    return;
                }
                self.disarm(Timer::LongPress);
                self.state = State::TrackpadDragging;
                self.last_tap = None;
                log::debug!("trackpad: double-tap-and-hold → drag");
                self.emit(GestureAction::ButtonDown { button: MouseButton::Left, at: self.cursor });
                self.move_cursor(dx, dy, dt);
                self.emit(GestureAction::Drag { to: self.cursor });
            }
            // Jitter within the slop leaves the press where it is; point and time
            // stay at touch-down as in TrackpadTapHold.
            State::TrackpadHolding => {
                if !beyond_slop {
                    return;
                }
                self.state = State::TrackpadDragging;
                log::debug!("trackpad: drag after a hold");
                self.move_cursor(dx, dy, dt);
                self.emit(GestureAction::Drag { to: self.cursor });
            }
            State::TrackpadDragging => {
                self.move_cursor(dx, dy, dt);
                self.emit(GestureAction::Drag { to: self.cursor });
            }
            _ => {}
        }
        self.last_point = p;
        self.last_move_time = t;
    }

    fn trackpad_up(&mut self, event: &TouchEvent) {
        let p = self.point(self.primary_pointer(event));
        let t = event.event_time_ms;
        let held = elapsed(self.start_time, t);
        let held_ms = ms(self.start_time, t);
        let away = p.distance(self.start_point);
        let in_place = away <= self.slop(self.thresholds.tap_max_distance);
        let is_tap = held <= self.thresholds.trackpad_tap_max_duration && in_place;
        self.disarm(Timer::LongPress);
        match self.state {
            State::TrackpadPending if is_tap => {
                self.cursor = self.cursor_at_down;
                log::debug!("trackpad tap after {held_ms} ms");
                self.emit(GestureAction::Click { at: self.cursor, count: 1 });
                self.last_tap = Some((t, self.cursor));
            }
            State::TrackpadTapHold if is_tap => {
                self.cursor = self.cursor_at_down;
                log::debug!("trackpad double tap after {held_ms} ms");
                self.emit(GestureAction::Click { at: self.cursor, count: 2 });
                self.last_tap = None; // a third tap starts over
            }
            State::TrackpadTapHold if in_place && held >= self.thresholds.long_press_time => {
                // Held long enough by the phone's clock, but the timer has not run yet:
                // the click the held button would have made.
                log::debug!("trackpad: click: tap then hold {held_ms} ms");
                self.emit(GestureAction::Click { at: self.cursor, count: 1 });
                self.last_tap = None;
            }
            State::TrackpadHolding => {
                log::debug!("trackpad: left button up: hold {held_ms} ms");
                self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.cursor });
                self.last_tap = None;
            }
            State::TrackpadDragging => {
                self.emit(GestureAction::ButtonUp { button: MouseButton::Left, at: self.cursor });
                self.last_tap = None;
            }
            _ => {
                // A slow or moved touch, or a slow second touch: no click (a resting finger never presses).
                match self.state {
                    State::TrackpadPending if in_place => log::debug!("trackpad: no click (held {held_ms} ms)"),
                    State::TrackpadPending => log::debug!("trackpad: no click (lifted {} dp away)", self.dp(away)),
                    State::TrackpadTapHold => log::debug!("trackpad: no double click (held {held_ms} ms)"),
                    _ => {}
                }
                self.last_tap = None;
            }
        }
        self.state = State::Idle;
    }

    /// Moves the cursor by a finger delta (display points) with pointer acceleration.
    fn move_cursor(&mut self, dx: f64, dy: f64, dt: f64) {
        let speed = if dt > 0.0 { dx.hypot(dy) / self.points_per_dp / dt } else { 0.0 };
        self.finger_speed = if self.finger_speed == 0.0 { speed } else { 0.5 * speed + 0.5 * self.finger_speed };
        let gain = Self::trackpad_gain(self.finger_speed, &self.thresholds);
        self.cursor = self.clamp_to_display(Point::new(self.cursor.x + dx * gain, self.cursor.y + dy * gain));
    }

    /// Pointer acceleration: the base gain up to the slow speed, rising
    /// smoothly (smoothstep) to base × max acceleration at the fast speed.
    /// Never decreases with speed.
    pub fn trackpad_gain(speed_dp_per_second: f64, thresholds: &GestureThresholds) -> f64 {
        let lo = thresholds.trackpad_slow_speed;
        let hi = thresholds.trackpad_fast_speed.max(lo + 1.0);
        let x = ((speed_dp_per_second - lo) / (hi - lo)).clamp(0.0, 1.0);
        let s = x * x * (3.0 - 2.0 * x);
        thresholds.trackpad_base_gain * (1.0 + (thresholds.trackpad_max_acceleration - 1.0) * s)
    }

    fn clamp_to_display(&self, p: Point) -> Point {
        let (w, h) = (self.display_size.width, self.display_size.height);
        Point::new(p.x.max(0.0).min((w - 1.0).max(0.0)), p.y.max(0.0).min((h - 1.0).max(0.0)))
    }

    // MARK: - Geometry

    fn primary_pointer(&self, event: &TouchEvent) -> TouchPointer {
        event.pointers.iter().copied().find(|p| p.id == self.primary_id).unwrap_or(event.pointers[0])
    }

    /// Normalized → display-local points, clamped onto the display.
    fn point(&self, p: TouchPointer) -> Point {
        fn unit(v: f32) -> f64 {
            if v.is_finite() { (v as f64).clamp(0.0, 1.0) } else { 0.0 }
        }
        let (w, h) = (self.display_size.width, self.display_size.height);
        Point::new((unit(p.x) * w).min((w - 1.0).max(0.0)), (unit(p.y) * h).min((h - 1.0).max(0.0)))
    }

    fn slop(&self, dp: f64) -> f64 {
        dp * self.points_per_dp
    }

    /// A display distance in whole phone dp (diagnostics).
    fn dp(&self, points: f64) -> i64 {
        (points / self.points_per_dp) as i64
    }
}

/// The pointer the action refers to, else the first.
fn action_pointer(event: &TouchEvent) -> TouchPointer {
    event.pointers.iter().copied().find(|p| p.id == event.action_pointer_id).unwrap_or(event.pointers[0])
}

fn sorted_ids(pointers: &[TouchPointer]) -> Vec<u8> {
    let mut ids: Vec<u8> = pointers.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    ids
}

/// Seconds between two wrapping millisecond timestamps.
fn elapsed(from: u32, to: u32) -> f64 {
    to.wrapping_sub(from) as f64 / 1000.0
}

/// Whole milliseconds between two timestamps (diagnostics).
fn ms(from: u32, to: u32) -> i64 {
    i64::from(to.wrapping_sub(from))
}
