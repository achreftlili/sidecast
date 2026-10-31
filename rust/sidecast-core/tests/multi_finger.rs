//! Port of `MultiFingerGestureTests.swift`: two-finger tap (right click) and
//! 3/4-finger swipes (desktops, overview, app windows).

mod common;

use common::{Finger, Harness, SIZE, click, left_up, moved, p};
use sidecast_core::input::{GestureAction, ScrollPhase, SemanticAction};
use sidecast_core::protocol::TouchAction;

/// Android order: DOWN for the first finger, POINTER_DOWN (all pointers so far) for each next one, 10 ms apart.
fn fingers_down(h: &mut Harness, t: u32, fingers: &[Finger]) -> u32 {
    h.touch(t, TouchAction::Down, &fingers[..1], None);
    for i in 1..fingers.len() {
        h.touch(t + i as u32 * 10, TouchAction::PointerDown, &fingers[..=i], Some(fingers[i].0));
    }
    t + (fingers.len() as u32 - 1) * 10
}

/// POINTER_UP from the last finger back to the second, then UP for the first, 10 ms apart.
fn fingers_up(h: &mut Harness, t: u32, fingers: &[Finger]) -> u32 {
    let mut time = t;
    for i in (1..fingers.len()).rev() {
        h.touch(time, TouchAction::PointerUp, &fingers[..=i], Some(fingers[i].0));
        time += 10;
    }
    h.touch(time, TouchAction::Up, &fingers[..1], None);
    time
}

fn shifted(fingers: &[Finger], dx: f64, dy: f64) -> Vec<Finger> {
    fingers.iter().map(|&(id, x, y)| (id, x + dx, y + dy)).collect()
}

/// Moves all fingers by (dx, dy) in `steps` MOVEs, 8 ms apart. Returns (end time, fingers).
fn move_all(h: &mut Harness, t: u32, fingers: &[Finger], dx: f64, dy: f64, steps: u32) -> (u32, Vec<Finger>) {
    let mut time = t;
    for i in 1..=steps {
        time = t + i * 8;
        let f = i as f64 / steps as f64;
        h.touch(time, TouchAction::Move, &shifted(fingers, dx * f, dy * f), None);
    }
    (time, shifted(fingers, dx, dy))
}

fn fingers(n: usize) -> Vec<Finger> {
    (0..n).map(|i| (i as u8, 400.0 + i as f64 * 40.0, if i % 2 == 0 { 250.0 } else { 260.0 })).collect()
}

/// A whole n-finger swipe; returns the navigations so far.
fn swipe(h: &mut Harness, n: usize, dx: f64, dy: f64) -> Vec<SemanticAction> {
    let start = fingers(n);
    let t = fingers_down(h, 0, &start);
    let (end, moved) = move_all(h, t + 8, &start, dx, dy, 20);
    fingers_up(h, end + 10, &moved);
    h.navigations()
}

// MARK: - Two-finger tap

#[test]
fn two_finger_tap_right_clicks_where_the_first_finger_landed() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(20, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(40, TouchAction::Move, &[(0, 302.0, 201.0), (1, 361.0, 206.0)], None); // jitter within the slop
    h.touch(110, TouchAction::PointerUp, &[(0, 302.0, 201.0), (1, 361.0, 206.0)], Some(1));
    h.touch(130, TouchAction::Up, &[(0, 302.0, 201.0)], None);
    assert_eq!(h.actions, [moved(300.0, 200.0), GestureAction::RightClick { at: p(300.0, 200.0) }]);
}

#[test]
fn two_finger_tap_when_the_first_finger_lifts_first() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(15, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(100, TouchAction::PointerUp, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(0));
    h.touch(120, TouchAction::Up, &[(1, 360.0, 205.0)], None);
    assert_eq!(h.right_clicks(), [p(300.0, 200.0)]);
    assert!(!h.has_click());
}

#[test]
fn slow_two_finger_tap_does_nothing() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(20, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(250, TouchAction::PointerUp, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(350, TouchAction::Up, &[(0, 300.0, 200.0)], None); // 350 ms > 300 ms
    assert!(h.right_clicks().is_empty());
    h.actions.clear();
    h.touch(1000, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(1020, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(1350, TouchAction::PointerUp, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1)); // first lift too late
    h.touch(1360, TouchAction::Up, &[(0, 300.0, 200.0)], None);
    assert!(h.right_clicks().is_empty());
    assert!(!h.has_click());
}

#[test]
fn two_finger_scroll_is_not_a_right_click() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 400.0, 200.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 400.0, 200.0), (1, 500.0, 200.0)], Some(1));
    h.touch(60, TouchAction::Move, &[(0, 400.0, 230.0), (1, 500.0, 230.0)], None);
    h.touch(100, TouchAction::Move, &[(0, 400.0, 250.0), (1, 500.0, 250.0)], None);
    h.touch(150, TouchAction::PointerUp, &[(0, 400.0, 250.0), (1, 500.0, 250.0)], Some(1));
    h.touch(160, TouchAction::Up, &[(0, 400.0, 250.0)], None);
    assert!(h.right_clicks().is_empty());
    assert_eq!(h.scroll_phases()[..3], [ScrollPhase::Began, ScrollPhase::Changed, ScrollPhase::Ended]);
}

#[test]
fn pinch_is_not_a_right_click() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 450.0, 250.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 450.0, 250.0), (1, 550.0, 250.0)], Some(1));
    h.touch(40, TouchAction::Move, &[(0, 420.0, 250.0), (1, 580.0, 250.0)], None); // spread 60 > 20 dp → pinch
    h.touch(80, TouchAction::Move, &[(0, 400.0, 250.0), (1, 600.0, 250.0)], None);
    h.touch(120, TouchAction::PointerUp, &[(0, 400.0, 250.0), (1, 600.0, 250.0)], Some(1));
    h.touch(130, TouchAction::Up, &[(0, 400.0, 250.0)], None);
    assert!(h.right_clicks().is_empty());
    assert!(!h.zooms().is_empty());
}

#[test]
fn remaining_finger_wandering_cancels_the_tap() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(20, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(80, TouchAction::PointerUp, &[(0, 300.0, 200.0), (1, 360.0, 205.0)], Some(1));
    h.touch(100, TouchAction::Move, &[(0, 340.0, 200.0)], None); // 40 pt > slop
    h.touch(120, TouchAction::Up, &[(0, 340.0, 200.0)], None);
    assert_eq!(h.actions, [moved(300.0, 200.0)]);
}

#[test]
fn second_finger_after_a_one_finger_scroll_is_no_tap() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(30, TouchAction::Move, &[(0, 300.0, 240.0)], None); // one-finger scroll
    h.touch(60, TouchAction::PointerDown, &[(0, 300.0, 240.0), (1, 360.0, 240.0)], Some(1));
    h.touch(90, TouchAction::PointerUp, &[(0, 300.0, 240.0), (1, 360.0, 240.0)], Some(1));
    h.touch(110, TouchAction::Up, &[(0, 300.0, 240.0)], None);
    assert!(h.right_clicks().is_empty());
    assert_eq!(h.scroll_phases(), [ScrollPhase::Began, ScrollPhase::Ended]);
}

#[test]
fn two_finger_tap_does_not_turn_the_next_tap_into_a_double_click() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(60, TouchAction::Up, &[(0, 300.0, 200.0)], None); // single click
    h.touch(150, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(160, TouchAction::PointerDown, &[(0, 300.0, 200.0), (1, 360.0, 200.0)], Some(1));
    h.touch(220, TouchAction::PointerUp, &[(0, 300.0, 200.0), (1, 360.0, 200.0)], Some(1));
    h.touch(230, TouchAction::Up, &[(0, 300.0, 200.0)], None); // right click
    h.touch(300, TouchAction::Down, &[(0, 300.0, 200.0)], None);
    h.touch(350, TouchAction::Up, &[(0, 300.0, 200.0)], None); // single again
    assert_eq!(h.clicks(), [click(300.0, 200.0, 1), click(300.0, 200.0, 1)]);
    assert_eq!(h.right_clicks(), [p(300.0, 200.0)]);
}

// MARK: - 3/4-finger swipes

#[test]
fn three_finger_swipe_directions_follow_the_trackpad() {
    let mut h = Harness::new();
    assert_eq!(swipe(&mut h, 3, -200.0, 0.0), [SemanticAction::DesktopNext], "fingers left → desktop to the right");
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, 200.0, 0.0), [SemanticAction::DesktopPrevious], "fingers right → desktop to the left");
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, 0.0, -150.0), [SemanticAction::Overview]);
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, 0.0, 150.0), [SemanticAction::AppWindows]);
}

#[test]
fn swipe_emits_nothing_but_the_navigation() {
    let mut h = Harness::new();
    swipe(&mut h, 3, -200.0, 0.0);
    assert_eq!(h.actions, [moved(400.0, 250.0), GestureAction::Semantic(SemanticAction::DesktopNext)]);
}

#[test]
fn four_finger_swipe() {
    let mut h = Harness::new();
    assert_eq!(swipe(&mut h, 4, -200.0, 0.0), [SemanticAction::DesktopNext]);
    h.actions.clear();
    assert_eq!(swipe(&mut h, 4, 0.0, -120.0), [SemanticAction::Overview]);
    assert!(!h.has_click());
}

#[test]
fn only_one_navigation_per_gesture_even_for_long_or_back_and_forth_swipes() {
    let mut h = Harness::new();
    let start = fingers(3);
    let t = fingers_down(&mut h, 0, &start);
    let (time, now) = move_all(&mut h, t + 8, &start, -600.0, 0.0, 60); // 10× the threshold
    let (time, now) = move_all(&mut h, time, &now, 700.0, 0.0, 60); // and back the other way
    let (time, now) = move_all(&mut h, time, &now, 0.0, -300.0, 30);
    fingers_up(&mut h, time + 10, &now);
    assert_eq!(h.navigations(), [SemanticAction::DesktopNext]);
}

#[test]
fn diagonal_swipe_uses_the_dominant_axis() {
    let mut h = Harness::new();
    assert_eq!(swipe(&mut h, 3, -80.0, -50.0), [SemanticAction::DesktopNext]);
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, 40.0, 90.0), [SemanticAction::AppWindows]);
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, 50.0, -70.0), [SemanticAction::Overview]);
}

#[test]
fn small_three_finger_jitter_does_nothing() {
    let mut h = Harness::new();
    let start = fingers(3);
    let t = fingers_down(&mut h, 0, &start);
    let (time, now) = move_all(&mut h, t + 8, &start, -25.0, 15.0, 5);
    let (time, now) = move_all(&mut h, time, &now, 45.0, -30.0, 5);
    fingers_up(&mut h, time + 10, &now);
    assert!(h.navigations().is_empty());
    assert_eq!(h.actions, [moved(400.0, 250.0)]);
}

#[test]
fn three_finger_tap_does_nothing() {
    let mut h = Harness::new();
    let start = fingers(3);
    let t = fingers_down(&mut h, 0, &start);
    h.touch(t + 20, TouchAction::Move, &shifted(&start, 1.0, 1.0), None);
    fingers_up(&mut h, t + 60, &shifted(&start, 1.0, 1.0));
    assert_eq!(h.actions, [moved(400.0, 250.0)], "no click, no right click, no navigation");
    // The next one-finger tap is a plain single click.
    h.touch(500, TouchAction::Down, &[(0, 100.0, 100.0)], None);
    h.touch(550, TouchAction::Up, &[(0, 100.0, 100.0)], None);
    assert_eq!(h.actions.last(), Some(&click(100.0, 100.0, 1)));
}

#[test]
fn third_finger_mid_two_finger_scroll_ends_the_scroll_without_momentum_or_click() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 400.0, 200.0)], None);
    h.touch(10, TouchAction::PointerDown, &[(0, 400.0, 200.0), (1, 500.0, 200.0)], Some(1));
    for i in 1..=6u32 {
        // A fast two-finger flick that would normally coast.
        let y = 200.0 + i as f64 * 20.0;
        h.touch(10 + i * 10, TouchAction::Move, &[(0, 400.0, y), (1, 500.0, y)], None);
    }
    assert_eq!(h.scroll_phases().first(), Some(&ScrollPhase::Began));
    h.touch(75, TouchAction::PointerDown, &[(0, 400.0, 320.0), (1, 500.0, 320.0), (2, 450.0, 330.0)], Some(2));
    assert_eq!(h.scroll_phases().last(), Some(&ScrollPhase::Ended));
    h.advance(2.0);
    assert!(!h.scroll_phases().contains(&ScrollPhase::MomentumBegan), "no momentum after a third finger");
    assert_eq!(h.pending_count(), 0);
    let scroll_count = h.scroll_phases().len();
    h.touch(90, TouchAction::Move, &[(0, 400.0, 330.0), (1, 500.0, 330.0), (2, 450.0, 340.0)], None);
    h.touch(100, TouchAction::PointerUp, &[(0, 400.0, 330.0), (1, 500.0, 330.0), (2, 450.0, 340.0)], Some(2));
    h.touch(110, TouchAction::Move, &[(0, 400.0, 360.0), (1, 500.0, 360.0)], None); // two fingers left: still no scroll
    h.touch(120, TouchAction::PointerUp, &[(0, 400.0, 360.0), (1, 500.0, 360.0)], Some(1));
    h.touch(130, TouchAction::Up, &[(0, 400.0, 360.0)], None);
    assert_eq!(h.scroll_phases().len(), scroll_count);
    assert!(!h.has_click());
    assert!(h.right_clicks().is_empty());
}

#[test]
fn third_finger_releases_a_held_button() {
    let mut h = Harness::new();
    h.touch(0, TouchAction::Down, &[(0, 200.0, 200.0)], None);
    h.touch(600, TouchAction::Move, &[(0, 260.0, 200.0)], None); // long-press drag
    assert!(h.engine.is_button_held());
    h.touch(650, TouchAction::PointerDown, &[(0, 260.0, 200.0), (1, 300.0, 200.0), (2, 340.0, 200.0)], Some(2));
    assert!(!h.engine.is_button_held());
    assert_eq!(h.actions.last(), Some(&left_up(260.0, 200.0)));
}

#[test]
fn finger_set_changes_never_count_as_travel() {
    let mut h = Harness::new();
    let three = fingers(3); // x 400…480
    let t = fingers_down(&mut h, 0, &three);
    let mut four = three.clone();
    four.push((3, 950.0, 250.0)); // centroid jumps ~+120 pt
    h.touch(t + 10, TouchAction::PointerDown, &four, Some(3));
    h.touch(t + 20, TouchAction::Move, &shifted(&four, 2.0, 0.0), None);
    h.touch(t + 30, TouchAction::PointerUp, &shifted(&four, 2.0, 0.0), Some(3)); // and back
    h.touch(t + 40, TouchAction::Move, &shifted(&three, 4.0, 0.0), None);
    h.touch(t + 50, TouchAction::PointerUp, &shifted(&three, 4.0, 0.0), Some(2)); // down to two fingers: no jump either
    let six = shifted(&three, 6.0, 0.0);
    h.touch(t + 60, TouchAction::Move, &six[..2], None);
    h.touch(t + 70, TouchAction::PointerUp, &six[..2], Some(1));
    h.touch(t + 80, TouchAction::Up, &six[..1], None);
    assert!(h.navigations().is_empty());
    assert_eq!(h.actions.len(), 1);
}

#[test]
fn moves_with_a_different_finger_set_rebaseline() {
    let mut h = Harness::new();
    let three = fingers(3);
    let t = fingers_down(&mut h, 0, &three);
    // A MOVE whose pointers differ from the tracked set (no POINTER_* in between) must not jump.
    h.touch(t + 10, TouchAction::Move, &[(0, 900.0, 250.0), (1, 940.0, 250.0), (5, 980.0, 250.0)], None);
    h.touch(t + 20, TouchAction::Move, &[(0, 905.0, 250.0), (1, 945.0, 250.0), (5, 985.0, 250.0)], None);
    assert!(h.navigations().is_empty());
}

#[test]
fn swipe_threshold_is_in_phone_dp() {
    let mut h = Harness::new();
    h.configure(SIZE, 2.0); // 60 dp = 120 pt
    assert!(swipe(&mut h, 3, -100.0, 0.0).is_empty(), "100 pt = 50 dp");
    h.actions.clear();
    assert_eq!(swipe(&mut h, 3, -140.0, 0.0), [SemanticAction::DesktopNext], "140 pt = 70 dp");
}

#[test]
fn one_and_two_finger_gestures_still_work_after_a_swipe() {
    let mut h = Harness::new();
    swipe(&mut h, 3, -200.0, 0.0);
    h.actions.clear();
    h.touch(1000, TouchAction::Down, &[(0, 100.0, 100.0)], None);
    h.touch(1050, TouchAction::Up, &[(0, 100.0, 100.0)], None);
    assert_eq!(h.actions, [moved(100.0, 100.0), click(100.0, 100.0, 1)]);
    h.actions.clear();
    h.touch(2000, TouchAction::Down, &[(0, 400.0, 200.0)], None);
    h.touch(2010, TouchAction::PointerDown, &[(0, 400.0, 200.0), (1, 500.0, 200.0)], Some(1));
    h.touch(2060, TouchAction::Move, &[(0, 400.0, 230.0), (1, 500.0, 230.0)], None);
    h.touch(2400, TouchAction::PointerUp, &[(0, 400.0, 230.0), (1, 500.0, 230.0)], Some(1));
    h.touch(2420, TouchAction::Up, &[(0, 400.0, 230.0)], None);
    assert_eq!(h.scroll_phases(), [ScrollPhase::Began, ScrollPhase::Ended]);
}

#[test]
fn cancel_and_reset_during_a_swipe_emit_nothing() {
    let mut h = Harness::new();
    let t = fingers_down(&mut h, 0, &fingers(3));
    h.touch(t + 10, TouchAction::Cancel, &fingers(3), None);
    h.reset();
    assert_eq!(h.actions, [moved(400.0, 250.0)]);
}

// MARK: - Semantic actions (replaces the Mac's Control+arrow shortcut test)

#[test]
fn semantic_actions_have_stable_names() {
    let names: Vec<&str> = SemanticAction::ALL.iter().map(|a| a.name()).collect();
    assert_eq!(names, ["desktop.previous", "desktop.next", "overview", "app.windows"]);
}
