use super::ClickTiming;
use crate::geometry::Point;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
}

/// Phase of a scroll event (maps onto macOS scroll/momentum phases; hosts
/// without phases can ignore it, momentum events then just scroll).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScrollPhase {
    Began,
    Changed,
    Ended,
    MomentumBegan,
    Momentum,
    MomentumEnded,
}

/// System navigation triggered by 3/4-finger swipes. Directions follow a
/// laptop trackpad: content follows the fingers. Each host maps these to its
/// own shortcuts or APIs (macOS: Control+arrows for Spaces, Mission Control
/// and App Exposé; Windows: Win+Ctrl+arrows and Win+Tab; GNOME: Super+Page
/// Up/Down and the activities overview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticAction {
    /// Fingers moved right → the virtual desktop (or full-screen app) to the left.
    DesktopPrevious,
    /// Fingers moved left → the virtual desktop to the right.
    DesktopNext,
    /// Fingers moved up → all windows of all apps (Mission Control, Task View, Activities).
    Overview,
    /// Fingers moved down → the windows of the current app (App Exposé).
    AppWindows,
}

impl SemanticAction {
    pub const ALL: [SemanticAction; 4] = [
        SemanticAction::DesktopPrevious,
        SemanticAction::DesktopNext,
        SemanticAction::Overview,
        SemanticAction::AppWindows,
    ];

    /// Stable dotted name, as the planned protocol v2 semantic actions spell them (ROADMAP P1).
    pub fn name(self) -> &'static str {
        match self {
            SemanticAction::DesktopPrevious => "desktop.previous",
            SemanticAction::DesktopNext => "desktop.next",
            SemanticAction::Overview => "overview",
            SemanticAction::AppWindows => "app.windows",
        }
    }
}

/// Output of the gesture engine, in display-local points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureAction {
    Move {
        to: Point,
    },
    ButtonDown {
        button: MouseButton,
        at: Point,
    },
    /// Pointer motion while the left button is held.
    Drag {
        to: Point,
    },
    ButtonUp {
        button: MouseButton,
        at: Point,
    },
    /// A complete left click; `count` is the click count (2 = double click).
    Click {
        at: Point,
        count: u32,
    },
    RightClick {
        at: Point,
    },
    /// Pixel deltas; positive `dy` scrolls content down with the finger (natural).
    Scroll {
        dx: f64,
        dy: f64,
        at: Point,
        phase: ScrollPhase,
    },
    /// Pinch zoom step (positive = zoom in).
    Zoom {
        delta: i32,
        at: Point,
    },
    /// 3/4-finger swipe: one per gesture.
    Semantic(SemanticAction),
}

impl GestureAction {
    /// Where this action puts the pointer (`None` for scroll, zoom and semantic actions).
    pub fn pointer_location(&self) -> Option<Point> {
        match *self {
            GestureAction::Move { to } | GestureAction::Drag { to } => Some(to),
            GestureAction::Click { at, .. }
            | GestureAction::RightClick { at }
            | GestureAction::ButtonDown { at, .. }
            | GestureAction::ButtonUp { at, .. } => Some(at),
            GestureAction::Scroll { .. } | GestureAction::Zoom { .. } | GestureAction::Semantic(_) => None,
        }
    }
}

/// Gesture tuning, ported from SideScreen's GestureThresholds. Distances are
/// in phone dp; the engine converts them with `points_per_dp` (1 dp ≈ 1/160 inch).
/// Durations are in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct GestureThresholds {
    pub tap_max_distance: f64,
    pub double_tap_max_interval: f64,
    pub double_tap_max_distance: f64,
    /// Held at least this long without moving: release = right click, move = drag.
    pub long_press_time: f64,
    pub scroll_sensitivity: f64,
    pub pinch_min_distance: f64,
    pub zoom_sensitivity: f64,
    /// Moves closer together than this (client time) are coalesced.
    pub min_move_interval: f64,
    /// Momentum starts if the finger lifted within this time of its last move.
    pub momentum_max_idle: f64,
    pub momentum_tick: f64,
    pub momentum_gain: f64,
    pub momentum_decay: f64,
    pub momentum_start_threshold: f64,
    pub momentum_stop_threshold: f64,
    /// Two-finger tap (→ right click): first finger down to last finger up.
    pub two_finger_tap_max_duration: f64,
    /// 3/4-finger swipe: centroid travel (dp, dominant axis) that triggers navigation.
    pub swipe_min_distance: f64,
    // Trackpad mode (PROTOCOL §7.3)
    /// A tap: down to up within this, and within the tap slop.
    pub trackpad_tap_max_duration: f64,
    /// A second touch this soon after a tap's release is a double tap (or tap-and-hold drag).
    pub trackpad_double_tap_interval: f64,
    /// Cursor points per finger point at slow speed: a full phone-width swipe ≈ 60 % of the display.
    pub trackpad_base_gain: f64,
    /// Finger speeds (dp/s) between which the gain ramps up smoothly to `× trackpad_max_acceleration`.
    pub trackpad_slow_speed: f64,
    pub trackpad_fast_speed: f64,
    pub trackpad_max_acceleration: f64,
    /// Human-like click delays (move → down, down → up).
    pub click_timing: ClickTiming,
}

impl Default for GestureThresholds {
    fn default() -> Self {
        GestureThresholds {
            tap_max_distance: 15.0,
            double_tap_max_interval: 0.4,
            double_tap_max_distance: 20.0,
            long_press_time: 0.5,
            scroll_sensitivity: 1.2,
            pinch_min_distance: 20.0,
            zoom_sensitivity: 0.5,
            min_move_interval: 0.008,
            momentum_max_idle: 0.05,
            momentum_tick: 0.016,
            momentum_gain: 3.0,
            momentum_decay: 0.92,
            momentum_start_threshold: 2.0,
            momentum_stop_threshold: 0.5,
            two_finger_tap_max_duration: 0.3,
            swipe_min_distance: 60.0,
            trackpad_tap_max_duration: 0.25,
            trackpad_double_tap_interval: 0.35,
            trackpad_base_gain: 0.6,
            trackpad_slow_speed: 150.0,
            trackpad_fast_speed: 1_500.0,
            trackpad_max_acceleration: 2.5,
            click_timing: ClickTiming::default(),
        }
    }
}
