//! Port of `ClickAndCursorTests.swift`: human-like click timelines and the
//! CURSOR stream (PROTOCOL §7.3, DECISIONS Q32).

use std::cell::Cell;

use sidecast_core::input::{
    ClickTiming, CursorDecision, CursorThrottle, GestureAction, GestureThresholds, MouseButton, PointerEvent,
    PointerEventKind, ScrollPhase, SemanticAction, TimelineStep, click_timeline, describe_timeline, play_timeline,
};
use sidecast_core::protocol::{CursorPosition, Frame, FrameParser, MessageType, ServerMessage};
use sidecast_core::{Point, Size};

use PointerEventKind::{ButtonDown, ButtonUp, Dragged, Moved};

const P: Point = Point::new(100.0, 50.0);

fn steps(action: GestureAction) -> Vec<TimelineStep> {
    click_timeline(&action, ClickTiming::default()).expect("a pointer timeline")
}

fn pressed(kind: PointerEventKind, at: Point, click_count: u32, pressure: f64) -> PointerEvent {
    PointerEvent { kind, at, click_count: Some(click_count), pressure: Some(pressure) }
}

// MARK: - Click timelines

#[test]
fn click_is_move_settle_down_hold_up() {
    assert_eq!(
        steps(GestureAction::Click { at: P, count: 1 }),
        [
            TimelineStep::new(0, PointerEvent::new(Moved, P)),
            TimelineStep::new(12, pressed(ButtonDown(MouseButton::Left), P, 1, 1.0)),
            TimelineStep::new(40, pressed(ButtonUp(MouseButton::Left), P, 1, 0.0)),
        ]
    );
}

#[test]
fn double_click_carries_click_count_two() {
    let s = steps(GestureAction::Click { at: P, count: 2 });
    assert_eq!(s.iter().map(|s| s.event.click_count).collect::<Vec<_>>(), [None, Some(2), Some(2)]);
    assert_eq!(s.iter().map(|s| s.delay_ms).collect::<Vec<_>>(), [0, 12, 40]);
}

#[test]
fn right_click_timeline() {
    let s = steps(GestureAction::RightClick { at: P });
    assert_eq!(
        s.iter().map(|s| s.event.kind).collect::<Vec<_>>(),
        [Moved, ButtonDown(MouseButton::Right), ButtonUp(MouseButton::Right)]
    );
    assert_eq!(s.iter().map(|s| s.delay_ms).collect::<Vec<_>>(), [0, 12, 40]);
    assert_eq!(s.iter().map(|s| s.event.pressure).collect::<Vec<_>>(), [None, Some(1.0), Some(0.0)]);
}

#[test]
fn drag_start_moves_then_presses_and_drag_steps_are_immediate() {
    let kinds = |a| steps(a).iter().map(|s| s.event.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds(GestureAction::ButtonDown { button: MouseButton::Left, at: P }),
        [Moved, ButtonDown(MouseButton::Left)]
    );
    assert_eq!(steps(GestureAction::Drag { to: P }), [TimelineStep::new(0, pressed(Dragged, P, 1, 1.0))]);
    assert_eq!(
        steps(GestureAction::ButtonUp { button: MouseButton::Left, at: P }),
        [TimelineStep::new(0, pressed(ButtonUp(MouseButton::Left), P, 1, 0.0))]
    );
    assert_eq!(steps(GestureAction::Move { to: P }), [TimelineStep::new(0, PointerEvent::new(Moved, P))]);
    let timing = ClickTiming::default();
    assert_eq!(
        click_timeline(&GestureAction::Scroll { dx: 1.0, dy: 1.0, at: P, phase: ScrollPhase::Changed }, timing),
        None
    );
    assert_eq!(click_timeline(&GestureAction::Zoom { delta: 1, at: P }, timing), None);
    assert_eq!(click_timeline(&GestureAction::Semantic(SemanticAction::Overview), timing), None);
}

#[test]
fn timing_comes_from_the_thresholds() {
    let mut thresholds = GestureThresholds::default();
    assert_eq!(thresholds.click_timing, ClickTiming { settle_ms: 12, hold_ms: 40 });
    thresholds.click_timing = ClickTiming { settle_ms: 5, hold_ms: 60 };
    let s = click_timeline(&GestureAction::Click { at: P, count: 1 }, thresholds.click_timing).unwrap();
    assert_eq!(s.iter().map(|s| s.delay_ms).collect::<Vec<_>>(), [0, 5, 60]);
}

/// A fake poster with a fake clock: events of later actions never come before an earlier click's button-up.
#[test]
fn playing_keeps_strict_order_across_actions() {
    let clock = Cell::new(0u32);
    let mut log: Vec<(u32, PointerEventKind)> = Vec::new();
    let actions = [
        GestureAction::Click { at: P, count: 1 },
        GestureAction::Move { to: Point::new(300.0, 300.0) },
        GestureAction::RightClick { at: P },
        GestureAction::ButtonDown { button: MouseButton::Left, at: P },
        GestureAction::Drag { to: Point::new(110.0, 50.0) },
        GestureAction::ButtonUp { button: MouseButton::Left, at: Point::new(110.0, 50.0) },
    ];
    for action in actions {
        play_timeline(&steps(action), |ms| clock.set(clock.get() + ms), |e| log.push((clock.get(), e.kind)));
    }
    assert_eq!(
        log.iter().map(|l| l.1).collect::<Vec<_>>(),
        [
            Moved,
            ButtonDown(MouseButton::Left),
            ButtonUp(MouseButton::Left),
            Moved,
            Moved,
            ButtonDown(MouseButton::Right),
            ButtonUp(MouseButton::Right),
            Moved,
            ButtonDown(MouseButton::Left),
            Dragged,
            ButtonUp(MouseButton::Left),
        ]
    );
    let times: Vec<u32> = log.iter().map(|l| l.0).collect();
    assert_eq!(times, [0, 12, 52, 52, 52, 64, 104, 104, 116, 116, 116]);
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "time never goes backwards");
}

#[test]
fn describe() {
    assert_eq!(
        describe_timeline(&steps(GestureAction::Click { at: P, count: 2 })),
        "moved (100, 50) → +12 ms leftDown (100, 50) cs2 → +40 ms leftUp (100, 50) cs2"
    );
}

#[test]
fn pointer_location() {
    assert_eq!(GestureAction::Click { at: P, count: 1 }.pointer_location(), Some(P));
    assert_eq!(GestureAction::Drag { to: P }.pointer_location(), Some(P));
    assert_eq!(GestureAction::Scroll { dx: 0.0, dy: 0.0, at: P, phase: ScrollPhase::Ended }.pointer_location(), None);
    assert_eq!(GestureAction::Semantic(SemanticAction::AppWindows).pointer_location(), None);
}

// MARK: - CURSOR codec

// §8.3 CURSOR x = 0.5, y = 0.25
#[test]
fn cursor_vector() {
    let wire = [0x89, 0x00, 0x00, 0x00, 0x08, 0x3f, 0x00, 0x00, 0x00, 0x3e, 0x80, 0x00, 0x00];
    assert_eq!(ServerMessage::Cursor(CursorPosition::new(0.5, 0.25)).encode(), wire);
    let mut parser = FrameParser::new();
    parser.push(&wire);
    assert_eq!(
        ServerMessage::decode(&parser.next_frame().unwrap().unwrap()),
        Ok(Some(ServerMessage::Cursor(CursorPosition::new(0.5, 0.25))))
    );
}

#[test]
fn cursor_off_display_and_truncation() {
    let off = ServerMessage::Cursor(CursorPosition::OFF_DISPLAY).encode();
    assert_eq!(off[5..], [0xbf, 0x80, 0, 0, 0xbf, 0x80, 0, 0]); // -1.0, -1.0
    assert!(!CursorPosition::OFF_DISPLAY.is_on_display());
    assert!(CursorPosition::new(0.0, 1.0).is_on_display());
    assert!(ServerMessage::decode(&Frame::new(MessageType::Cursor, vec![0x3f, 0, 0, 0])).is_err());
}

#[test]
fn cursor_normalisation() {
    let size = Size::new(1200.0, 540.0);
    assert_eq!(CursorPosition::from_point(Point::new(600.0, 135.0), size), CursorPosition::new(0.5, 0.25));
    assert_eq!(CursorPosition::from_point(Point::new(-20.0, 9000.0), size), CursorPosition::new(0.0, 1.0), "clamped");
    assert_eq!(CursorPosition::from_point(Point::new(1.0, 1.0), Size::ZERO), CursorPosition::OFF_DISPLAY);
}

// MARK: - Throttle

fn pos(x: f32) -> CursorPosition {
    CursorPosition::new(x, 0.5)
}

#[test]
fn throttle_sends_at_most_sixty_per_second_and_keeps_the_last_position() {
    let mut throttle = CursorThrottle::default();
    let mut sent: Vec<(f64, CursorPosition)> = Vec::new();
    let mut flush_at: Option<f64> = None;
    // 1000 positions in one second (a 1 kHz mover), flushing whenever asked to.
    for i in 0..1000 {
        let now = i as f64 / 1000.0;
        if let Some(at) = flush_at
            && now >= at
        {
            flush_at = None;
            if let Some(p) = throttle.flush(now) {
                sent.push((now, p));
            }
        }
        match throttle.offer(pos(i as f32 / 1000.0), now, false) {
            CursorDecision::Send(p) => sent.push((now, p)),
            CursorDecision::Deferred { until } => flush_at = flush_at.or(Some(until)),
            CursorDecision::Unchanged => {}
        }
    }
    if let Some(at) = flush_at
        && let Some(p) = throttle.flush(at)
    {
        sent.push((at, p));
    }
    assert!(sent.len() <= 61);
    assert!(sent.len() >= 55);
    for pair in sent.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= 1.0 / 60.0 - 1e-9);
    }
    assert_eq!(sent.last().map(|s| s.1), Some(pos(0.999)), "the final position is always delivered");
}

#[test]
fn throttle_skips_unchanged_unless_forced() {
    let mut throttle = CursorThrottle::default();
    assert_eq!(throttle.offer(pos(0.1), 0.0, false), CursorDecision::Send(pos(0.1)));
    assert_eq!(throttle.offer(pos(0.1), 1.0, false), CursorDecision::Unchanged);
    assert_eq!(throttle.offer(pos(0.1), 2.0, true), CursorDecision::Send(pos(0.1)));
    assert_eq!(throttle.offer(pos(0.2), 2.001, false), CursorDecision::Deferred { until: 2.0 + 1.0 / 60.0 });
    assert_eq!(throttle.flush(2.005), None, "too early");
    assert_eq!(throttle.flush(2.02), Some(pos(0.2)));
    assert_eq!(throttle.flush(3.0), None, "nothing pending");
    throttle.reset();
    assert_eq!(
        throttle.offer(pos(0.2), 3.0, false),
        CursorDecision::Send(pos(0.2)),
        "after reset the client is assumed to know nothing"
    );
}
