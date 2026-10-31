//! Port of `GestureEngineTests.swift`: one- and two-finger touch gestures.

mod common;

use common::{Harness, SIZE, click, left_up, moved, p};
use sidecast_core::Size;
use sidecast_core::input::{GestureAction, MouseButton, ScrollPhase};
use sidecast_core::protocol::{TouchAction, TouchEvent, TouchPointer};

// MARK: taps

#[test]
fn tap_clicks() {
    let mut h = Harness::new();
    h.down(1000, 100.0, 100.0);
    h.move_to(1050, 104.0, 102.0); // within slop
    h.up(1100, 104.0, 102.0);
    assert_eq!(h.actions, [moved(100.0, 100.0), click(104.0, 102.0, 1)]);
}

#[test]
fn double_tap() {
    let mut h = Harness::new();
    h.down(0, 100.0, 100.0);
    h.up(80, 100.0, 100.0);
    h.down(200, 105.0, 103.0);
    h.up(280, 105.0, 103.0);
    assert_eq!(h.actions, [moved(100.0, 100.0), click(100.0, 100.0, 1), moved(105.0, 103.0), click(100.0, 100.0, 2)]);
    // A third quick tap is a fresh single click.
    h.down(400, 100.0, 100.0);
    h.up(450, 100.0, 100.0);
    assert_eq!(h.actions.last(), Some(&click(100.0, 100.0, 1)));
}

#[test]
fn slow_second_tap_is_another_single_click() {
    let mut h = Harness::new();
    h.down(0, 100.0, 100.0);
    h.up(80, 100.0, 100.0);
    h.down(900, 100.0, 100.0);
    h.up(980, 100.0, 100.0);
    assert_eq!(h.clicks().iter().filter(|a| matches!(a, GestureAction::Click { count: 1, .. })).count(), 2);
}

#[test]
fn far_second_tap_is_another_single_click() {
    let mut h = Harness::new();
    h.down(0, 100.0, 100.0);
    h.up(80, 100.0, 100.0);
    h.down(150, 300.0, 300.0);
    h.up(200, 300.0, 300.0);
    assert_eq!(h.actions.last(), Some(&click(300.0, 300.0, 1)));
}

#[test]
fn timestamps_may_wrap() {
    let mut h = Harness::new();
    h.down(u32::MAX - 50, 10.0, 10.0);
    h.up(30, 10.0, 10.0); // 81 ms later across the wrap
    assert_eq!(h.actions.last(), Some(&click(10.0, 10.0, 1)));
}

// MARK: long press

#[test]
fn long_press_right_clicks() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.up(650, 201.0, 200.0);
    assert_eq!(h.actions, [moved(200.0, 200.0), GestureAction::RightClick { at: p(201.0, 200.0) }]);
}

#[test]
fn long_press_drag() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.move_to(300, 202.0, 200.0); // still within slop, too early
    h.move_to(600, 260.0, 200.0); // held 0.6 s, then moved → drag
    h.move_to(620, 300.0, 210.0);
    h.up(640, 300.0, 210.0);
    assert_eq!(
        h.actions,
        [
            moved(200.0, 200.0),
            GestureAction::ButtonDown { button: MouseButton::Left, at: p(200.0, 200.0) },
            GestureAction::Drag { to: p(260.0, 200.0) },
            GestureAction::Drag { to: p(300.0, 210.0) },
            GestureAction::Drag { to: p(300.0, 210.0) },
            left_up(300.0, 210.0),
        ]
    );
    assert!(!h.engine.is_button_held());
}

#[test]
fn reset_releases_held_button() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.move_to(600, 260.0, 200.0);
    assert!(h.engine.is_button_held());
    h.reset();
    assert_eq!(h.actions.last(), Some(&left_up(260.0, 200.0)));
    assert!(!h.engine.is_button_held());
    // Idle reset emits nothing.
    let count = h.actions.len();
    h.reset();
    assert_eq!(h.actions.len(), count);
}

#[test]
fn cancel_releases_button_without_clicking() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.move_to(600, 260.0, 200.0);
    h.touch(700, TouchAction::Cancel, &[(0, 260.0, 200.0)], None);
    assert_eq!(h.actions.last(), Some(&left_up(260.0, 200.0)));
    h.down(800, 10.0, 10.0);
    h.touch(850, TouchAction::Cancel, &[(0, 10.0, 10.0)], None);
    assert!(!h.has_click());
}

#[test]
fn down_without_up_releases_previous_drag() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.move_to(600, 260.0, 200.0);
    h.down(1000, 50.0, 50.0); // UP was lost
    assert_eq!(h.actions[h.actions.len() - 2..], [left_up(260.0, 200.0), moved(50.0, 50.0)]);
}

// MARK: scrolling

#[test]
fn one_finger_scroll_phases() {
    let mut h = Harness::new();
    h.down(0, 500.0, 300.0);
    h.move_to(100, 500.0, 280.0); // crosses the 15 pt slop → scroll began
    h.move_to(200, 500.0, 270.0);
    h.up(400, 500.0, 270.0); // lifted long after the last move: no momentum
    let s = h.scrolls();
    assert_eq!(h.scroll_phases(), [ScrollPhase::Began, ScrollPhase::Changed, ScrollPhase::Ended]);
    assert!((s[0].1 - -24.0).abs() < 1e-9); // -20 × 1.2
    assert!((s[1].1 - -12.0).abs() < 1e-9);
    h.advance(1.0);
    assert_eq!(h.scrolls().len(), 3);
    assert!(!h.has_click());
}

#[test]
fn flick_starts_momentum_that_decays_and_ends() {
    let mut h = Harness::new();
    h.down(0, 500.0, 400.0);
    let mut y = 400.0;
    for i in 1..=6 {
        y -= 20.0;
        h.move_to(i * 10, 500.0, y);
    }
    h.up(65, 500.0, y);
    let before = h.scrolls().len();
    assert_eq!(h.scroll_phases().last(), Some(&ScrollPhase::Ended));
    h.advance(3.0);
    let momentum = h.scrolls()[before..].to_vec();
    assert_eq!(momentum.first().map(|s| s.3), Some(ScrollPhase::MomentumBegan));
    assert_eq!(momentum.last().map(|s| s.3), Some(ScrollPhase::MomentumEnded));
    let magnitudes: Vec<f64> = momentum[..momentum.len() - 1].iter().map(|s| s.1.abs()).collect();
    assert!(magnitudes.len() > 5);
    assert!(magnitudes.windows(2).all(|w| w[0] >= w[1]), "decaying: {magnitudes:?}");
    assert!(momentum[..momentum.len() - 1].iter().all(|s| s.1 < 0.0)); // same direction as the flick
    assert_eq!(h.pending_count(), 0);
}

#[test]
fn new_touch_stops_momentum() {
    let mut h = Harness::new();
    h.down(0, 500.0, 400.0);
    for i in 1..=6 {
        h.move_to(i * 10, 500.0, 400.0 - i as f64 * 20.0);
    }
    h.up(65, 500.0, 280.0);
    h.advance(0.05);
    h.down(200, 100.0, 100.0);
    assert_eq!(h.scroll_phases().last(), Some(&ScrollPhase::MomentumEnded));
    let count = h.scrolls().len();
    h.advance(2.0);
    assert_eq!(h.scrolls().len(), count);
}

// MARK: two fingers

#[test]
fn two_finger_scroll() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 400.0, 200.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 400.0, 200.0), (1, 500.0, 200.0)], Some(1));
    h.touch(60, TouchAction::Move, &[(0, 400.0, 230.0), (1, 500.0, 230.0)], None); // midpoint +30 → scroll
    h.touch(110, TouchAction::Move, &[(0, 400.0, 250.0), (1, 500.0, 250.0)], None);
    h.touch(400, TouchAction::PointerUp, &[(0, 400.0, 250.0), (1, 500.0, 250.0)], Some(1));
    h.touch(420, TouchAction::Move, &[(0, 380.0, 260.0)], None); // leftover finger is ignored
    h.touch(450, TouchAction::Up, &[(0, 380.0, 260.0)], None);
    assert_eq!(h.scroll_phases(), [ScrollPhase::Began, ScrollPhase::Changed, ScrollPhase::Ended]);
    assert!((h.scrolls()[1].1 - 24.0).abs() < 1e-9);
    assert!(!h.has_click());
    assert!(h.zooms().is_empty());
}

#[test]
fn pinch_zooms() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 450.0, 250.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 450.0, 250.0), (1, 550.0, 250.0)], Some(1));
    for i in 1..=10u32 {
        let spread = i as f64 * 10.0;
        h.touch(10 + i * 20, TouchAction::Move, &[(0, 450.0 - spread, 250.0), (1, 550.0 + spread, 250.0)], None);
    }
    let zooms: Vec<i32> = h.zooms().into_iter().map(|z| z.0).collect();
    assert!(!zooms.is_empty());
    assert!(zooms.iter().all(|d| *d > 0));
    assert_eq!(zooms.iter().sum::<i32>(), 90); // 0.5 × (300 − 120): growth after the 20 dp threshold

    h.actions.clear();
    for i in 1..=5u32 {
        let spread = 100.0 - i as f64 * 15.0;
        h.touch(300 + i * 20, TouchAction::Move, &[(0, 450.0 - spread, 250.0), (1, 550.0 + spread, 250.0)], None);
    }
    assert!(h.zooms().iter().all(|z| z.0 < 0));
}

#[test]
fn second_finger_during_drag_releases_button() {
    let mut h = Harness::new();
    h.down(0, 200.0, 200.0);
    h.move_to(600, 260.0, 200.0);
    h.touch(700, TouchAction::PointerDown, &[(0, 260.0, 200.0), (1, 300.0, 200.0)], Some(1));
    assert_eq!(h.actions.last(), Some(&left_up(260.0, 200.0)));
    assert!(!h.engine.is_button_held());
}

// MARK: geometry

#[test]
fn coordinates_are_clamped_onto_the_display() {
    let mut h = Harness::new();
    h.handle(&TouchEvent::new(0, TouchAction::Down, 0, vec![TouchPointer::new(0, 1.3, -0.2)]));
    assert_eq!(h.actions, [moved(1023.0, 0.0)]);
    // Non-finite coordinates must not crash.
    h.handle(&TouchEvent::new(10, TouchAction::Up, 0, vec![TouchPointer::new(0, f32::NAN, f32::INFINITY)]));
}

#[test]
fn slop_scales_with_points_per_dp() {
    let mut h = Harness::new();
    h.configure(SIZE, 2.0);
    h.down(0, 100.0, 100.0);
    h.move_to(50, 125.0, 100.0); // 25 pt = 12.5 dp < 15 dp → still a tap
    h.up(100, 125.0, 100.0);
    assert_eq!(h.actions.last(), Some(&click(125.0, 100.0, 1)));
}

#[test]
fn ignored_without_display_size() {
    let mut h = Harness::new();
    h.configure(Size::ZERO, 1.0);
    h.down(0, 1.0, 1.0);
    h.up(10, 1.0, 1.0);
    assert!(h.actions.is_empty());
}
