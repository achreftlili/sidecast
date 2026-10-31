//! Port of `FlowControlTests.swift` and the bitrate-limit tests of `RemoteAccessTests.swift`
//! (PROTOCOL §5 data saver, §6 flow control).

use sidecast_core::protocol::StreamLimits;
use sidecast_core::streaming::{AbrConfig, AbrDecision, AdaptiveBitrateController, BitrateBounds, InFlightTracker};

const MBPS: i64 = 1_000_000;

fn decrease(reason: &str) -> Option<AbrDecision> {
    Some(AbrDecision::Decrease { reason: reason.to_owned() })
}

// MARK: - InFlightTracker

#[test]
fn at_most_four_in_flight_including_encoding() {
    let mut t = InFlightTracker::default();
    assert!(t.try_reserve());
    assert_eq!(t.mark_sent(0.0), 0);
    assert!(t.try_reserve());
    assert_eq!(t.mark_sent(0.0), 1);
    assert!(t.try_reserve()); // still encoding
    assert!(t.try_reserve());
    assert_eq!(t.in_flight(), 4);
    assert!(!t.try_reserve()); // 5th frame is dropped before encoding
    t.cancel_reservation(); // encoder produced nothing for one
    assert!(t.try_reserve());
}

#[test]
fn ack_returns_rtt_and_frees_slots() {
    let mut t = InFlightTracker::default();
    for i in 0..4 {
        assert!(t.try_reserve());
        assert_eq!(t.mark_sent(i as f64), i);
    }
    assert!(!t.try_reserve());
    assert!((t.ack(0, 0.5).unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(t.in_flight(), 3);
    assert_eq!(t.ack(0, 0.6), None); // duplicate
    assert!(t.try_reserve());
}

#[test]
fn acks_are_cumulative() {
    let mut t = InFlightTracker::default();
    for _ in 0..4 {
        t.try_reserve();
        t.mark_sent(0.0);
    }
    assert!(t.ack(2, 1.0).is_some()); // 0 and 1 implicitly acked
    assert_eq!(t.sent_unacked(), 1);
    assert!(t.ack(3, 1.0).is_some());
    assert_eq!(t.sent_unacked(), 0);
}

#[test]
fn unknown_future_ack_does_not_crash() {
    let mut t = InFlightTracker::default();
    t.try_reserve();
    t.mark_sent(0.0);
    assert_eq!(t.ack(1000, 1.0), None);
    assert_eq!(t.sent_unacked(), 0);
}

// MARK: - AdaptiveBitrateController

fn controller(config: AbrConfig) -> AdaptiveBitrateController {
    AdaptiveBitrateController::new(config, 0.0)
}

#[test]
fn defaults_start_at_20_mbps() {
    let c = controller(AbrConfig::default());
    assert_eq!(c.bitrate(), 20 * MBPS);
    assert_eq!(c.increase_step(), 2_500_000); // 5 % of 50 Mbps
}

#[test]
fn drop_decreases_by_25_percent_down_to_min() {
    let mut c = controller(AbrConfig { warmup_acks: 0, ..AbrConfig::default() });
    let mut expected = 20.0 * MBPS as f64;
    for second in 1..=10 {
        c.record_rtt(0.005);
        c.record_drop(1);
        assert_eq!(c.tick(second as f64), decrease("1 dropped"));
        expected = (expected * 0.75).max(4.0 * MBPS as f64);
        assert!((c.bitrate() - expected as i64).abs() <= 2, "{} vs {expected}", c.bitrate());
    }
    assert_eq!(c.bitrate(), 4 * MBPS);
}

#[test]
fn high_smoothed_rtt_decreases() {
    let mut c = controller(AbrConfig::default());
    c.record_rtt(0.005);
    assert_eq!(c.tick(1.0), Some(AbrDecision::Hold));
    for _ in 0..40 {
        c.record_rtt(0.120); // srtt converges well above min + 40 ms
    }
    assert_eq!(c.tick(2.0), decrease("rtt"));
    assert_eq!(c.bitrate(), 15 * MBPS);
}

#[test]
fn increase_after_two_clean_windows_capped_at_max() {
    let mut c = controller(AbrConfig { max_bitrate: 30 * MBPS, ..AbrConfig::default() });
    assert_eq!(c.increase_step(), 1_500_000); // max(1 Mbps, 5 % of 30 Mbps)
    let mut now = 0.0;
    let mut clean_window = |c: &mut AdaptiveBitrateController| {
        c.record_rtt(0.004);
        now += 1.0;
        c.tick(now)
    };
    assert_eq!(clean_window(&mut c), Some(AbrDecision::Hold));
    assert_eq!(clean_window(&mut c), Some(AbrDecision::Increase));
    assert_eq!(c.bitrate(), 21_500_000);
    assert_eq!(clean_window(&mut c), Some(AbrDecision::Hold));
    assert_eq!(clean_window(&mut c), Some(AbrDecision::Increase));
    for _ in 0..40 {
        clean_window(&mut c);
    }
    assert_eq!(c.bitrate(), 30 * MBPS);
}

#[test]
fn minimum_step_is_one_mbps() {
    let c = controller(AbrConfig { start_bitrate: 5 * MBPS, max_bitrate: 10 * MBPS, ..AbrConfig::default() });
    assert_eq!(c.increase_step(), MBPS);
}

#[test]
fn drop_resets_clean_streak() {
    let mut c = controller(AbrConfig { warmup_acks: 0, ..AbrConfig::default() });
    c.record_rtt(0.004);
    c.tick(1.0); // clean #1
    c.record_drop(1);
    c.record_rtt(0.004);
    c.tick(2.0); // congested
    assert_eq!(c.bitrate(), 15 * MBPS);
    c.record_rtt(0.004);
    assert_eq!(c.tick(3.0), Some(AbrDecision::Hold)); // clean #1 again, no increase yet
    c.record_rtt(0.004);
    assert_eq!(c.tick(4.0), Some(AbrDecision::Increase));
}

#[test]
fn drops_during_warm_up_are_ignored() {
    let mut c = controller(AbrConfig { warmup_acks: 3, ..AbrConfig::default() });
    c.record_drop(3); // before any ack
    c.record_rtt(0.004);
    c.record_rtt(0.004);
    c.record_drop(1); // 2 acks: still warming up
    assert_eq!(c.tick(1.0), Some(AbrDecision::Hold));
    assert_eq!(c.bitrate(), 20 * MBPS);
    c.record_rtt(0.004);
    c.record_drop(1);
    assert_eq!(c.tick(2.0), decrease("1 dropped"));
}

#[test]
fn window_without_acks_holds() {
    let mut c = controller(AbrConfig::default());
    assert_eq!(c.tick(1.0), Some(AbrDecision::Hold));
    assert_eq!(c.tick(2.0), Some(AbrDecision::Hold));
    assert_eq!(c.bitrate(), 20 * MBPS);
}

#[test]
fn tick_before_window_ends_returns_none() {
    let mut c = AdaptiveBitrateController::new(AbrConfig::default(), 10.0);
    assert_eq!(c.tick(10.5), None);
    assert!(c.tick(11.0).is_some());
}

#[test]
fn lowering_max_clamps_immediately() {
    let mut c = controller(AbrConfig::default());
    c.set_max_bitrate(10 * MBPS);
    assert_eq!(c.bitrate(), 10 * MBPS);
    c.set_max_bitrate(MBPS); // below min: max is raised to min
    assert_eq!(c.bitrate(), 4 * MBPS);
}

// MARK: - Client limits (RemoteAccessTests)

fn limits(kbps: Option<i64>, fps: Option<i64>) -> StreamLimits {
    StreamLimits::new(kbps, fps)
}

fn triple(b: BitrateBounds) -> [i64; 3] {
    [b.floor, b.ceiling, b.start()]
}

#[test]
fn bitrate_bounds() {
    assert_eq!(triple(BitrateBounds::new(50, None)), [4_000_000, 50_000_000, 20_000_000]);
    assert_eq!(triple(BitrateBounds::new(50, Some(&StreamLimits::DATA_SAVER))), [4_000_000, 4_000_000, 4_000_000]);
    assert_eq!(
        triple(BitrateBounds::new(50, Some(&limits(Some(2_500), None)))),
        [2_500_000, 2_500_000, 2_500_000],
        "floor = limit below 4 Mbps"
    );
    assert_eq!(
        triple(BitrateBounds::new(50, Some(&limits(Some(10_000), None)))),
        [4_000_000, 10_000_000, 10_000_000],
        "start clamped to the max"
    );
    assert_eq!(BitrateBounds::new(10, Some(&limits(Some(20_000), None))).ceiling, 10_000_000);
    assert_eq!(BitrateBounds::new(50, Some(&limits(None, Some(30)))).ceiling, 50_000_000);
}

#[test]
fn adaptive_controller_stays_inside_the_limit() {
    let bounds = BitrateBounds::new(50, Some(&limits(Some(3_000), None)));
    let mut abr = AdaptiveBitrateController::new(AbrConfig::from_bounds(bounds, 1.0), 0.0);
    assert_eq!(abr.bitrate(), 3_000_000);
    // Clean windows never push it above the limit…
    let mut now = 0.0;
    for _ in 0..10 {
        for _ in 0..40 {
            abr.record_rtt(0.01);
        }
        now += 1.0;
        abr.tick(now);
    }
    assert_eq!(abr.bitrate(), 3_000_000);
    // …and congestion never drops it below the (lowered) floor.
    for _ in 0..5 {
        abr.record_drop(1);
        now += 1.0;
        abr.tick(now);
    }
    assert_eq!(abr.bitrate(), 3_000_000);
    // Settings changes keep the limit applied.
    abr.set_bounds(BitrateBounds::new(100, Some(&limits(Some(3_000), None))));
    assert_eq!(abr.config().max_bitrate, 3_000_000);
    abr.set_bounds(BitrateBounds::new(100, None));
    assert_eq!(abr.config().max_bitrate, 100_000_000);
    assert_eq!(abr.config().min_bitrate, 4_000_000);
}
