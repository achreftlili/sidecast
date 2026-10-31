//! Shared harness for the gesture-engine suites: drives a [`GestureEngine`] on
//! a 1024 × 512 display and replays its momentum timer deterministically, with
//! the same semantics as the Swift tests' `ManualScheduler` (a timer scheduled
//! at `now + delay` fires only when time is advanced past it).

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

use sidecast_core::input::{GestureAction, GestureEngine, MouseButton, ScrollPhase, SemanticAction};
use sidecast_core::protocol::{InputMode, TouchAction, TouchEvent, TouchPointer};
use sidecast_core::{Point, Size};

/// Power-of-two size: integer positions are exact as normalized Float32.
pub const SIZE: Size = Size::new(1024.0, 512.0);

pub type Finger = (u8, f64, f64);

pub fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

pub struct Harness {
    pub engine: GestureEngine,
    pub actions: Vec<GestureAction>,
    now: f64,
    /// (generation, due time) of the armed momentum timer.
    armed: Option<(u64, f64)>,
}

impl Harness {
    /// 1 point per dp: slop 15, swipe threshold 60.
    pub fn new() -> Self {
        let mut engine = GestureEngine::default();
        engine.configure(SIZE, 1.0);
        Harness { engine, actions: Vec::new(), now: 0.0, armed: None }
    }

    pub fn trackpad() -> Self {
        let mut h = Self::new();
        h.set_mode(InputMode::Trackpad);
        h
    }

    /// Records output and mirrors the engine's timer, as `ManualScheduler`
    /// would have stored the closure the engine scheduled.
    fn absorb(&mut self, out: Vec<GestureAction>) {
        self.actions.extend(out);
        match self.engine.pending_timer() {
            Some(t) if self.armed.map(|a| a.0) != Some(t.generation) => {
                self.armed = Some((t.generation, self.now + t.delay));
            }
            Some(_) => {}
            None => self.armed = None,
        }
    }

    pub fn handle(&mut self, event: &TouchEvent) {
        let out = self.engine.handle(event);
        self.absorb(out);
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        let out = self.engine.set_mode(mode);
        self.absorb(out);
    }

    pub fn reset(&mut self) {
        let out = self.engine.reset();
        self.absorb(out);
    }

    pub fn configure(&mut self, size: Size, points_per_dp: f64) {
        self.engine.configure(size, points_per_dp);
    }

    /// Advances time, running due timers in order (including ones they schedule).
    pub fn advance(&mut self, by: f64) {
        let target = self.now + by;
        while let Some((generation, due)) = self.armed {
            if due > target {
                break;
            }
            self.now = self.now.max(due);
            self.armed = None;
            let out = self.engine.fire_timer(generation);
            self.absorb(out);
        }
        self.now = target;
    }

    pub fn pending_count(&self) -> usize {
        usize::from(self.engine.pending_timer().is_some())
    }

    /// A TOUCH with positions in display points; `pid` defaults to the first finger.
    pub fn touch(&mut self, t: u32, action: TouchAction, fingers: &[Finger], pid: Option<u8>) {
        let pointers = fingers
            .iter()
            .map(|&(id, x, y)| TouchPointer::new(id, (x / SIZE.width) as f32, (y / SIZE.height) as f32))
            .collect();
        let event = TouchEvent::new(t, action, pid.unwrap_or(fingers[0].0), pointers);
        self.handle(&event);
    }

    pub fn down(&mut self, t: u32, x: f64, y: f64) {
        self.touch(t, TouchAction::Down, &[(0, x, y)], None);
    }

    pub fn move_to(&mut self, t: u32, x: f64, y: f64) {
        self.touch(t, TouchAction::Move, &[(0, x, y)], None);
    }

    pub fn up(&mut self, t: u32, x: f64, y: f64) {
        self.touch(t, TouchAction::Up, &[(0, x, y)], None);
    }

    // MARK: - Views of the output

    pub fn scrolls(&self) -> Vec<(f64, f64, Point, ScrollPhase)> {
        self.actions
            .iter()
            .filter_map(|a| match *a {
                GestureAction::Scroll { dx, dy, at, phase } => Some((dx, dy, at, phase)),
                _ => None,
            })
            .collect()
    }

    pub fn scroll_phases(&self) -> Vec<ScrollPhase> {
        self.scrolls().into_iter().map(|s| s.3).collect()
    }

    pub fn clicks(&self) -> Vec<GestureAction> {
        self.actions.iter().copied().filter(|a| matches!(a, GestureAction::Click { .. })).collect()
    }

    pub fn has_click(&self) -> bool {
        !self.clicks().is_empty()
    }

    pub fn right_clicks(&self) -> Vec<Point> {
        self.actions
            .iter()
            .filter_map(|a| match *a {
                GestureAction::RightClick { at } => Some(at),
                _ => None,
            })
            .collect()
    }

    pub fn zooms(&self) -> Vec<(i32, Point)> {
        self.actions
            .iter()
            .filter_map(|a| match *a {
                GestureAction::Zoom { delta, at } => Some((delta, at)),
                _ => None,
            })
            .collect()
    }

    pub fn navigations(&self) -> Vec<SemanticAction> {
        self.actions
            .iter()
            .filter_map(|a| match *a {
                GestureAction::Semantic(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    pub fn drags(&self) -> Vec<Point> {
        self.actions
            .iter()
            .filter_map(|a| match *a {
                GestureAction::Drag { to } => Some(to),
                _ => None,
            })
            .collect()
    }
}

pub fn click(x: f64, y: f64, count: u32) -> GestureAction {
    GestureAction::Click { at: p(x, y), count }
}

pub fn moved(x: f64, y: f64) -> GestureAction {
    GestureAction::Move { to: p(x, y) }
}

pub fn left_up(x: f64, y: f64) -> GestureAction {
    GestureAction::ButtonUp { button: MouseButton::Left, at: p(x, y) }
}
