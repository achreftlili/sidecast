//! Port of the gesture parts of `TrackpadTests.swift`: trackpad input mode
//! (PROTOCOL §7.3), a relative cursor and taps at the cursor. The INPUT_MODE
//! codec test lives in `messages.rs`.

mod common;

use std::sync::{Arc, Mutex};

use common::{Harness, click, p};
use sidecast_core::Point;
use sidecast_core::input::{GestureAction, GestureEngine, GestureThresholds, MouseButton, ScrollPhase, SemanticAction};
use sidecast_core::protocol::{InputMode, TouchAction};

const CENTRE: Point = Point::new(512.0, 256.0);

fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

/// One finger from `(x, y)` by `(dx, dy)` in `steps` MOVEs `every` ms apart; returns the end time.
fn slide(h: &mut Harness, t: u32, (x, y): (f64, f64), (dx, dy): (f64, f64), steps: u32, every: u32) -> u32 {
    let mut time = t;
    for i in 1..=steps {
        time = t + i * every;
        let f = i as f64 / steps as f64;
        h.move_to(time, x + dx * f, y + dy * f);
    }
    time
}

fn centre_click(count: u32) -> GestureAction {
    click(CENTRE.x, CENTRE.y, count)
}

// MARK: - Pointing

#[test]
fn touch_mode_is_the_default() {
    assert_eq!(GestureEngine::default().mode(), InputMode::Touch);
}

#[test]
fn one_finger_moves_the_cursor_relatively() {
    let mut h = Harness::trackpad();
    h.down(0, 100.0, 100.0);
    assert!(h.actions.is_empty(), "touch-down does not jump the cursor to the finger");
    let end = slide(&mut h, 0, (100.0, 100.0), (100.0, 0.0), 50, 50); // 40 dp/s: slow, gain 0.6
    h.up(end + 50, 200.0, 100.0);
    assert_close(h.engine.cursor().x, 512.0 + 60.0);
    assert_close(h.engine.cursor().y, 256.0);
    assert_eq!(h.actions.last(), Some(&GestureAction::Move { to: h.engine.cursor() }));
    assert!(h.actions.iter().all(|a| matches!(a, GestureAction::Move { .. })));
}

#[test]
fn cursor_is_clamped_to_the_display() {
    let mut h = Harness::trackpad();
    for i in 0..3 {
        // three full-width swipes to the right
        let t = i * 2000;
        h.down(t, 0.0, 256.0);
        let end = slide(&mut h, t, (0.0, 256.0), (1023.0, 0.0), 100, 10);
        h.up(end + 10, 1023.0, 256.0);
    }
    assert_eq!(h.engine.cursor().x, 1023.0);
    h.down(10_000, 500.0, 511.0);
    let end = slide(&mut h, 10_000, (500.0, 511.0), (0.0, -511.0), 100, 10);
    h.up(end + 10, 500.0, 0.0);
    assert_eq!(h.engine.cursor().y, 0.0);
    h.down(20_000, 1023.0, 0.0);
    for i in 0..2 {
        let t = 20_000 + i * 2000;
        if i > 0 {
            h.down(t, 1023.0, 0.0);
        }
        let end = slide(&mut h, t, (1023.0, 0.0), (-1023.0, 511.0), 100, 10);
        h.up(end + 10, 0.0, 511.0);
    }
    assert_eq!(h.engine.cursor(), p(0.0, 511.0));
}

#[test]
fn acceleration_is_monotonic_and_bounded() {
    let th = GestureThresholds::default();
    assert_close(GestureEngine::trackpad_gain(0.0, &th), 0.6);
    assert_close(GestureEngine::trackpad_gain(150.0, &th), 0.6);
    assert_close(GestureEngine::trackpad_gain(1500.0, &th), 1.5);
    assert_close(GestureEngine::trackpad_gain(50_000.0, &th), 1.5);
    let mut previous = 0.0;
    for step in 0..=600 {
        let v = step as f64 * 5.0;
        let g = GestureEngine::trackpad_gain(v, &th);
        assert!(g >= previous, "gain must never drop as speed rises (v={v})");
        previous = g;
    }
}

#[test]
fn fast_swipes_travel_further_than_slow_ones() {
    let mut h = Harness::trackpad();
    h.down(0, 200.0, 100.0);
    let end = slide(&mut h, 0, (200.0, 100.0), (300.0, 0.0), 100, 50); // 60 dp/s
    h.up(end + 50, 500.0, 100.0);
    let slow = h.engine.cursor().x - 512.0;
    assert!((slow - 180.0).abs() < 1e-6, "a slow swipe gets the base gain: {slow}");
    h.set_mode(InputMode::Touch);
    h.set_mode(InputMode::Trackpad);
    let before = h.engine.cursor().x;
    h.down(20_000, 500.0, 300.0); // stays on the display
    let end = slide(&mut h, 20_000, (500.0, 300.0), (-300.0, 0.0), 10, 8); // 3750 dp/s
    h.up(end + 8, 200.0, 300.0);
    let fast = before - h.engine.cursor().x;
    assert!(fast > slow * 2.0);
    assert!(fast <= 300.0 * 1.5 + 1e-6);
}

#[test]
fn starts_from_the_system_cursor_when_it_is_on_the_display() {
    let mut h = Harness::trackpad();
    let system = Arc::new(Mutex::new(Some(p(100.0, 50.0))));
    let shared = Arc::clone(&system);
    h.engine.set_cursor_provider(move || *shared.lock().unwrap());
    h.down(0, 500.0, 300.0);
    let end = slide(&mut h, 0, (500.0, 300.0), (50.0, 0.0), 25, 50);
    h.up(end + 50, 550.0, 300.0);
    assert_close(h.engine.cursor().x, 130.0);
    // The user moved the real mouse meanwhile: the next touch starts from there.
    *system.lock().unwrap() = Some(p(300.0, 300.0));
    h.down(5000, 500.0, 300.0);
    h.up(5060, 500.0, 300.0);
    assert_eq!(h.clicks().last(), Some(&click(300.0, 300.0, 1)));
    // Off this display (or unknown): keep our own cursor.
    *system.lock().unwrap() = Some(p(-10.0, 5.0));
    h.down(10_000, 500.0, 300.0);
    h.up(10_060, 500.0, 300.0);
    assert_eq!(h.clicks().last(), Some(&click(300.0, 300.0, 1)));
}

#[test]
fn cursor_starts_at_the_display_centre() {
    let mut h = Harness::trackpad();
    h.engine.set_cursor_provider(|| Some(Point::new(5000.0, 5000.0))); // not on this display
    h.down(0, 900.0, 400.0);
    h.up(70, 900.0, 400.0);
    assert_eq!(h.actions, [centre_click(1)]);
}

// MARK: - Taps

#[test]
fn tap_clicks_at_the_cursor_not_the_finger() {
    let mut h = Harness::trackpad();
    h.down(0, 900.0, 400.0);
    h.up(80, 900.0, 400.0);
    assert_eq!(h.actions, [centre_click(1)]);
}

#[test]
fn jitter_during_a_tap_does_not_move_the_click() {
    let mut h = Harness::trackpad();
    h.down(0, 900.0, 400.0);
    h.move_to(40, 904.0, 402.0); // within the slop: the cursor nudges…
    assert_ne!(h.engine.cursor(), CENTRE);
    h.up(80, 904.0, 402.0); // …but the tap clicks where it started
    assert_eq!(h.actions.last(), Some(&centre_click(1)));
    assert_eq!(h.engine.cursor(), CENTRE);
}

#[test]
fn slow_tap_and_long_press_do_nothing() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(300, 500.0, 300.0); // > 250 ms
    h.down(1000, 500.0, 300.0);
    h.up(1900, 500.0, 300.0); // a long press
    assert!(h.clicks().is_empty());
    assert!(h.right_clicks().is_empty(), "no long-press right click in trackpad mode");
}

#[test]
fn moving_beyond_the_slop_never_clicks() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.move_to(40, 540.0, 300.0);
    h.up(80, 540.0, 300.0);
    assert!(h.clicks().is_empty());
}

#[test]
fn double_tap_is_a_double_click() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(200, 700.0, 100.0); // anywhere on the phone: it clicks at the cursor
    h.up(260, 700.0, 100.0);
    h.down(400, 500.0, 300.0); // a third tap starts over
    h.up(450, 500.0, 300.0);
    assert_eq!(h.clicks(), [centre_click(1), centre_click(2), centre_click(1)]);
}

#[test]
fn second_tap_too_late_is_another_single_click() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(500, 500.0, 300.0); // 440 ms after the first release
    h.up(560, 500.0, 300.0);
    assert_eq!(h.clicks(), [centre_click(1), centre_click(1)]);
}

#[test]
fn double_tap_and_hold_drags() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(200, 500.0, 300.0);
    h.move_to(250, 505.0, 300.0); // within the slop: undecided, cursor still
    assert_eq!(h.engine.cursor(), CENTRE);
    let end = slide(&mut h, 300, (505.0, 300.0), (35.0, 0.0), 20, 50); // slow, beyond the slop
    assert!(h.engine.is_button_held());
    h.up(end + 20, 540.0, 300.0);
    assert!(!h.engine.is_button_held());
    assert_eq!(h.actions[0], centre_click(1));
    assert_eq!(h.actions[1], GestureAction::ButtonDown { button: MouseButton::Left, at: CENTRE });
    let drags = h.drags();
    assert!(!drags.is_empty());
    assert_close(drags.last().unwrap().x, 512.0 + 40.0 * 0.6); // relative drag, whole travel applied
    assert_eq!(h.actions.last(), Some(&GestureAction::ButtonUp { button: MouseButton::Left, at: h.engine.cursor() }));
    assert_eq!(h.clicks().len(), 1, "no double click when it became a drag");
}

// MARK: - Two or more fingers

#[test]
fn two_finger_tap_right_clicks_at_the_cursor() {
    let mut h = Harness::trackpad();
    h.touch(0, TouchAction::Down, &[(0, 800.0, 400.0)], None);
    h.touch(15, TouchAction::PointerDown, &[(0, 800.0, 400.0), (1, 860.0, 400.0)], Some(1));
    h.touch(90, TouchAction::PointerUp, &[(0, 800.0, 400.0), (1, 860.0, 400.0)], Some(1));
    h.touch(110, TouchAction::Up, &[(0, 800.0, 400.0)], None);
    assert_eq!(h.actions, [GestureAction::RightClick { at: CENTRE }]);
}

#[test]
fn two_finger_scroll_happens_at_the_cursor() {
    let mut h = Harness::trackpad();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 400.0, 200.0)], Some(1));
    for i in 1..=6u32 {
        // a flick downwards
        let y = 200.0 + i as f64 * 20.0;
        h.touch(10 + i * 10, TouchAction::Move, &[(0, 300.0, y), (1, 400.0, y)], None);
    }
    h.touch(75, TouchAction::PointerUp, &[(0, 300.0, 320.0), (1, 400.0, 320.0)], Some(1));
    h.touch(80, TouchAction::Up, &[(0, 300.0, 320.0)], None);
    h.advance(3.0);
    let scrolls = h.scrolls();
    assert_eq!(scrolls.first().map(|s| s.3), Some(ScrollPhase::Began));
    assert!(scrolls.iter().any(|s| s.3 == ScrollPhase::MomentumBegan), "momentum is reused");
    assert!(scrolls.iter().all(|s| s.2 == CENTRE), "scrolls at the cursor, not at the fingers");
    assert!(h.right_clicks().is_empty());
    assert_eq!(h.engine.cursor(), CENTRE, "scrolling does not move the cursor");
}

#[test]
fn pinch_zooms_at_the_cursor() {
    let mut h = Harness::trackpad();
    h.touch(0, TouchAction::Down, &[(0, 450.0, 250.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 450.0, 250.0), (1, 550.0, 250.0)], Some(1));
    for i in 1..=6u32 {
        let spread = i as f64 * 15.0;
        h.touch(10 + i * 20, TouchAction::Move, &[(0, 450.0 - spread, 250.0), (1, 550.0 + spread, 250.0)], None);
    }
    let zooms = h.zooms();
    assert!(!zooms.is_empty());
    assert!(zooms.iter().all(|&(d, at)| d > 0 && at == CENTRE));
}

#[test]
fn three_finger_swipe_navigates_as_in_touch_mode() {
    let mut h = Harness::trackpad();
    h.touch(0, TouchAction::Down, &[(0, 400.0, 250.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 400.0, 250.0), (1, 440.0, 260.0)], Some(1));
    h.touch(20, TouchAction::PointerDown, &[(0, 400.0, 250.0), (1, 440.0, 260.0), (2, 480.0, 250.0)], Some(2));
    for i in 1..=20u32 {
        let dx = -(i as f64) * 10.0;
        h.touch(
            20 + i * 8,
            TouchAction::Move,
            &[(0, 400.0 + dx, 250.0), (1, 440.0 + dx, 260.0), (2, 480.0 + dx, 250.0)],
            None,
        );
    }
    h.touch(200, TouchAction::PointerUp, &[(0, 200.0, 250.0), (1, 240.0, 260.0), (2, 280.0, 250.0)], Some(2));
    h.touch(210, TouchAction::PointerUp, &[(0, 200.0, 250.0), (1, 240.0, 260.0)], Some(1));
    h.touch(220, TouchAction::Up, &[(0, 200.0, 250.0)], None);
    assert_eq!(h.actions, [GestureAction::Semantic(SemanticAction::DesktopNext)]);
}

// MARK: - Mode switches

#[test]
fn mode_switch_mid_trackpad_drag_releases_the_button() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(200, 500.0, 300.0);
    h.move_to(260, 540.0, 300.0);
    assert!(h.engine.is_button_held());
    h.set_mode(InputMode::Touch);
    assert!(!h.engine.is_button_held());
    assert_eq!(h.actions.last(), Some(&GestureAction::ButtonUp { button: MouseButton::Left, at: h.engine.cursor() }));
    // The rest of that touch is ignored in the new mode (no stray click on lift).
    let count = h.actions.len();
    h.up(300, 540.0, 300.0);
    assert_eq!(h.actions.len(), count);
}

#[test]
fn mode_switch_mid_touch_drag_releases_the_button() {
    let mut h = Harness::trackpad();
    h.set_mode(InputMode::Touch);
    h.down(0, 200.0, 200.0);
    h.move_to(600, 260.0, 200.0); // long-press drag (touch mode)
    assert!(h.engine.is_button_held());
    h.set_mode(InputMode::Trackpad);
    assert!(!h.engine.is_button_held());
    assert_eq!(h.actions.last(), Some(&GestureAction::ButtonUp { button: MouseButton::Left, at: p(260.0, 200.0) }));
}

#[test]
fn mode_switch_mid_scroll_ends_the_scroll_without_momentum() {
    let mut h = Harness::trackpad();
    h.set_mode(InputMode::Touch);
    h.touch(0, TouchAction::Down, &[(0, 400.0, 200.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 400.0, 200.0), (1, 500.0, 200.0)], Some(1));
    h.touch(40, TouchAction::Move, &[(0, 400.0, 240.0), (1, 500.0, 240.0)], None);
    h.set_mode(InputMode::Trackpad);
    match h.actions.last() {
        Some(GestureAction::Scroll { phase, .. }) => assert_eq!(*phase, ScrollPhase::Ended),
        other => panic!("expected a scroll, got {other:?}"),
    }
    h.advance(2.0);
    assert_eq!(h.pending_count(), 0);
}

#[test]
fn setting_the_same_mode_changes_nothing() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(200, 500.0, 300.0);
    h.move_to(260, 540.0, 300.0);
    let count = h.actions.len();
    h.set_mode(InputMode::Trackpad);
    assert_eq!(h.actions.len(), count);
    assert!(h.engine.is_button_held());
}

#[test]
fn reset_releases_a_trackpad_drag() {
    let mut h = Harness::trackpad();
    h.down(0, 500.0, 300.0);
    h.up(60, 500.0, 300.0);
    h.down(200, 500.0, 300.0);
    h.move_to(260, 540.0, 300.0);
    h.reset();
    assert!(!h.engine.is_button_held());
    assert_eq!(h.actions.last(), Some(&GestureAction::ButtonUp { button: MouseButton::Left, at: h.engine.cursor() }));
}
