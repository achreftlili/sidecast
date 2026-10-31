//! Human-like click timelines (PROTOCOL §7.3, DECISIONS Q32): Chromium and
//! Electron apps (VS Code) ignore zero-duration synthetic clicks, so a click
//! is played as move → settle → press → hold → release.

use super::{GestureAction, MouseButton};
use crate::geometry::Point;

/// Delays of human-like clicks in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickTiming {
    /// Pointer move → button down.
    pub settle_ms: u32,
    /// Button down → button up.
    pub hold_ms: u32,
}

impl Default for ClickTiming {
    fn default() -> Self {
        ClickTiming { settle_ms: 12, hold_ms: 40 }
    }
}

/// What a pointer event does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerEventKind {
    Moved,
    ButtonDown(MouseButton),
    ButtonUp(MouseButton),
    /// Motion while the left button is held (some platforms need a distinct drag event).
    Dragged,
}

/// One low-level pointer event to inject.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerEventKind,
    /// Display-local point.
    pub at: Point,
    /// Click count of a press/release (2 = second click of a double click); `None` for plain moves.
    pub click_count: Option<u32>,
    /// 1 while a button is down, 0 on release (`None` for plain moves).
    pub pressure: Option<f64>,
}

impl PointerEvent {
    pub fn new(kind: PointerEventKind, at: Point) -> Self {
        PointerEvent { kind, at, click_count: None, pressure: None }
    }

    fn pressed(kind: PointerEventKind, at: Point, click_count: u32, pressure: f64) -> Self {
        PointerEvent { kind, at, click_count: Some(click_count), pressure: Some(pressure) }
    }
}

/// An event preceded by a wait.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineStep {
    pub delay_ms: u32,
    pub event: PointerEvent,
}

impl TimelineStep {
    pub fn new(delay_ms: u32, event: PointerEvent) -> Self {
        TimelineStep { delay_ms, event }
    }
}

/// Turns pointer/button actions into timed event sequences:
/// - click: move → settle → down(count n) → hold → up(count n)
/// - right click: move → settle → right down → hold → right up
/// - drag start: move → settle → down; drag steps and releases are immediate
///
/// Scroll, zoom and semantic actions are not pointer timelines (`None`).
pub fn click_timeline(action: &GestureAction, timing: ClickTiming) -> Option<Vec<TimelineStep>> {
    use PointerEventKind::*;
    let steps = match *action {
        GestureAction::Move { to } => vec![TimelineStep::new(0, PointerEvent::new(Moved, to))],
        GestureAction::Click { at, count } => {
            let n = count.max(1);
            vec![
                TimelineStep::new(0, PointerEvent::new(Moved, at)),
                TimelineStep::new(timing.settle_ms, PointerEvent::pressed(ButtonDown(MouseButton::Left), at, n, 1.0)),
                TimelineStep::new(timing.hold_ms, PointerEvent::pressed(ButtonUp(MouseButton::Left), at, n, 0.0)),
            ]
        }
        GestureAction::RightClick { at } => vec![
            TimelineStep::new(0, PointerEvent::new(Moved, at)),
            TimelineStep::new(timing.settle_ms, PointerEvent::pressed(ButtonDown(MouseButton::Right), at, 1, 1.0)),
            TimelineStep::new(timing.hold_ms, PointerEvent::pressed(ButtonUp(MouseButton::Right), at, 1, 0.0)),
        ],
        GestureAction::ButtonDown { button, at } => vec![
            TimelineStep::new(0, PointerEvent::new(Moved, at)),
            TimelineStep::new(timing.settle_ms, PointerEvent::pressed(ButtonDown(button), at, 1, 1.0)),
        ],
        GestureAction::Drag { to } => vec![TimelineStep::new(0, PointerEvent::pressed(Dragged, to, 1, 1.0))],
        GestureAction::ButtonUp { button, at } => {
            vec![TimelineStep::new(0, PointerEvent::pressed(ButtonUp(button), at, 1, 0.0))]
        }
        GestureAction::Scroll { .. } | GestureAction::Zoom { .. } | GestureAction::Semantic(_) => return None,
    };
    Some(steps)
}

/// Plays steps strictly in order: `sleep(ms)` before each delayed event, then `post`.
/// The caller runs it on its serial input context, so later events wait for it.
pub fn play_timeline(steps: &[TimelineStep], mut sleep: impl FnMut(u32), mut post: impl FnMut(&PointerEvent)) {
    for step in steps {
        if step.delay_ms > 0 {
            sleep(step.delay_ms);
        }
        post(&step.event);
    }
}

/// "moved (10, 20) → +12 ms leftDown (10, 20) cs1 → +40 ms leftUp (10, 20) cs1" (logs).
pub fn describe_timeline(steps: &[TimelineStep]) -> String {
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let e = &step.event;
            let kind = match e.kind {
                PointerEventKind::Moved => "moved",
                PointerEventKind::ButtonDown(MouseButton::Left) => "leftDown",
                PointerEventKind::ButtonUp(MouseButton::Left) => "leftUp",
                PointerEventKind::ButtonDown(MouseButton::Right) => "rightDown",
                PointerEventKind::ButtonUp(MouseButton::Right) => "rightUp",
                PointerEventKind::Dragged => "leftDragged",
            };
            let mut base = format!("{kind} ({:.0}, {:.0})", e.at.x, e.at.y);
            if let Some(n) = e.click_count {
                base.push_str(&format!(" cs{n}"));
            }
            if i == 0 && step.delay_ms == 0 { base } else { format!("+{} ms {base}", step.delay_ms) }
        })
        .collect::<Vec<_>>()
        .join(" → ")
}
