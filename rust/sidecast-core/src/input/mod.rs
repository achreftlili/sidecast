//! Touch, trackpad and keyboard input, turned into platform-neutral actions
//! that each host maps onto its own input API (PROTOCOL §7).
//!
//! Pipeline: TOUCH → [`GestureEngine`] → [`GestureAction`] →
//! [`click_timeline`] (human-like timing) → [`InputEvent`] → the host's input
//! sink. KEY → [`KeyStroke`]; TEXT → [`typing`].

mod action;
mod cursor;
mod gesture;
mod keyboard;
mod timeline;

pub use action::{GestureAction, GestureThresholds, MouseButton, ScrollPhase, SemanticAction};
pub use cursor::{CursorDecision, CursorThrottle};
pub use gesture::{GestureEngine, PendingTimer};
pub use keyboard::{Key, KeyStroke, typing};
pub use timeline::{
    ClickTiming, PointerEvent, PointerEventKind, TimelineStep, click_timeline, describe_timeline, play_timeline,
};

use crate::geometry::Point;
use crate::protocol::KeyModifiers;

/// One input operation for a host's input sink, in display-local points
/// (origin = top-left of the streamed display). Pointer events arrive already
/// timed (the server waits between a click's move, press and release).
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Pointer move, button press/release or drag (see [`PointerEventKind`]).
    Pointer(PointerEvent),
    /// Pixel deltas; positive `dy` scrolls content down with the finger (natural scrolling).
    Scroll { dx: f64, dy: f64, at: Point, phase: ScrollPhase },
    /// Pinch-zoom step (positive = zoom in); a Mac sends ⌘-scroll, others Ctrl-wheel.
    Zoom { delta: i32, at: Point },
    /// A desktop/system action the host maps to its own shortcut or API.
    Semantic(SemanticAction),
    /// A special key with modifiers held (KEY, PROTOCOL §7.1).
    Key { key: Key, modifiers: KeyModifiers },
    /// Text to type as Unicode, independent of the keyboard layout (TEXT).
    Text(String),
}
