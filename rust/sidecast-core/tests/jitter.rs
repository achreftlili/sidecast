//! Port of `AudioJitterBufferTests.swift` (PROTOCOL §7.2, host side).

use sidecast_core::audio::{AudioJitterBuffer, AudioSequenceTracker, JitterConfig, JitterStats, SequenceEvent};
use sidecast_core::protocol::AudioPacket;

/// Small numbers so behaviour is easy to follow: 10-frame packets, play at 30, ceiling 60.
fn make_buffer() -> AudioJitterBuffer {
    AudioJitterBuffer::new(JitterConfig::new(30, 60))
}

/// A packet whose samples encode (seq, index) so the output shows what was kept.
fn packet(seq: u32) -> AudioPacket {
    AudioPacket { seq, samples: (0..10).map(|i| (seq * 100 + i + 1) as i16).collect() }
}

fn read(b: &mut AudioJitterBuffer, n: usize) -> Vec<f32> {
    let mut out = vec![f32::NAN; n];
    b.read(&mut out);
    out
}

/// Sample values back as the ints that were pushed (0 = silence).
fn ints(floats: &[f32]) -> Vec<i32> {
    floats.iter().map(|f| (f * 32_768.0).round() as i32).collect()
}

fn range(r: std::ops::RangeInclusive<i32>) -> Vec<i32> {
    r.collect()
}

#[test]
fn converts_int16_to_float() {
    let mut b = AudioJitterBuffer::new(JitterConfig::new(1, 100));
    b.push(&AudioPacket { seq: 0, samples: vec![i16::MIN, 16_384, 0, -16_384, i16::MAX] });
    let out = read(&mut b, 5);
    assert_eq!(out, [-1.0, 0.5, 0.0, -0.5, 32_767.0 / 32_768.0]);
}

#[test]
fn buffers_until_target_then_plays_in_order() {
    let mut b = make_buffer();
    b.push(&packet(0));
    b.push(&packet(1));
    assert_eq!(ints(&read(&mut b, 10)), [0; 10], "20 < 30 queued: still buffering");
    assert!(b.is_buffering());
    b.push(&packet(2));
    assert_eq!(ints(&read(&mut b, 15)), [range(1..=10), range(101..=105)].concat());
    assert!(!b.is_buffering());
    assert_eq!(b.queued_frames(), 15);
}

#[test]
fn underrun_plays_silence_and_rebuffers() {
    let mut b = make_buffer();
    for s in 0..3 {
        b.push(&packet(s));
    }
    read(&mut b, 25);
    assert_eq!(ints(&read(&mut b, 10)), [range(206..=210), vec![0; 5]].concat(), "the shortfall is silence");
    assert_eq!(b.stats().underruns, 1);
    assert!(b.is_buffering());
    // Re-buffers to the target before playing again.
    b.push(&packet(3));
    b.push(&packet(4));
    assert_eq!(ints(&read(&mut b, 5)), [0; 5]);
    b.push(&packet(5));
    assert_eq!(ints(&read(&mut b, 5)), range(301..=305));
}

#[test]
fn nothing_queued_is_silence() {
    let mut b = make_buffer();
    assert_eq!(ints(&read(&mut b, 64)), [0; 64]);
    assert_eq!(b.stats().silent_frames, 64);
}

#[test]
fn overflow_drops_the_oldest_back_to_target() {
    let mut b = make_buffer();
    for s in 0..7 {
        b.push(&packet(s)); // 70 > 60 queued
    }
    assert_eq!(b.queued_frames(), 30);
    assert_eq!(b.stats().dropped_frames, 40);
    // The newest 30 frames survive: packets 4, 5, 6.
    assert_eq!(ints(&read(&mut b, 30)), [range(401..=410), range(501..=510), range(601..=610)].concat());
}

#[test]
fn sequence_gap_is_filled_with_silence() {
    let mut b = make_buffer();
    b.push(&packet(0));
    b.push(&packet(3)); // 1 and 2 never came
    assert_eq!(b.stats().missing_packets, 2);
    assert_eq!(b.stats().gap_fill_frames, 20);
    assert_eq!(b.queued_frames(), 40);
    assert_eq!(ints(&read(&mut b, 40)), [range(1..=10), vec![0; 20], range(301..=310)].concat());
}

#[test]
fn huge_gap_is_capped_and_cannot_flood_the_buffer() {
    let mut b = make_buffer();
    b.push(&packet(0));
    b.push(&AudioPacket { seq: 100_000, samples: vec![7; 10] });
    assert_eq!(b.stats().missing_packets, 99_999);
    assert_eq!(b.stats().gap_fill_frames, 60, "fill capped at the ceiling");
    assert_eq!(b.queued_frames(), 30, "then trimmed back to the target");
    assert_eq!(ints(&read(&mut b, 30)), [vec![0; 20], vec![7; 10]].concat(), "the newest audio is kept, at the end");
}

#[test]
fn backwards_seq_is_a_restart_without_gap_fill() {
    let mut b = make_buffer();
    b.push(&packet(50));
    b.push(&packet(0)); // the phone restarted its numbering
    b.push(&packet(1));
    assert_eq!(b.stats().restarts, 1);
    assert_eq!(b.stats().missing_packets, 0);
    assert_eq!(b.queued_frames(), 30);
}

#[test]
fn reset_empties_and_forgets_the_sequence() {
    let mut b = make_buffer();
    for s in 0..4 {
        b.push(&packet(s));
    }
    b.reset();
    assert_eq!(b.queued_frames(), 0);
    assert!(b.is_buffering());
    b.push(&packet(0)); // seq 0 again after a new active:true: no restart, no gap
    assert_eq!(b.stats().restarts, 0);
    assert_eq!(b.stats().missing_packets, 0);
    let stats = b.take_stats();
    assert_eq!(stats.packets, 5);
    assert_eq!(b.stats(), JitterStats::default());
}

#[test]
fn default_configuration_is_40_and_150_milliseconds() {
    let c = JitterConfig::default();
    assert_eq!(c.target_frames, 48_000 * 40 / 1000);
    assert_eq!(c.max_frames, 48_000 * 150 / 1000);
}

#[test]
fn steady_ten_millisecond_packets_never_underrun() {
    let mut b = AudioJitterBuffer::default();
    let mut seq = 0;
    let mut push = |b: &mut AudioJitterBuffer| {
        b.push(&AudioPacket { seq, samples: vec![1000; 480] });
        seq += 1;
    };
    // Prime with 40 ms, then alternate 480-frame packets in / one 512-frame render out, rate-matched.
    for _ in 0..4 {
        push(&mut b);
    }
    let (mut pushed, mut pulled) = (4 * 480, 0);
    for _ in 0..2000 {
        while pushed - pulled < 1_920 + 512 {
            push(&mut b);
            pushed += 480;
        }
        read(&mut b, 512);
        pulled += 512;
    }
    assert_eq!(b.stats().underruns, 0);
    assert_eq!(b.stats().dropped_frames, 0);
}

// MARK: - Sequence tracker

#[test]
fn sequence_tracker() {
    let mut t = AudioSequenceTracker::new();
    assert_eq!(t.observe(5), SequenceEvent::First);
    assert_eq!(t.observe(6), SequenceEvent::InOrder);
    assert_eq!(t.observe(9), SequenceEvent::Gap { missing: 2 });
    assert_eq!(t.observe(3), SequenceEvent::Restart);
    assert_eq!(t.observe(4), SequenceEvent::InOrder);
    t.reset();
    assert_eq!(t.observe(0), SequenceEvent::First);
    // Wrap-around is in order, not a restart.
    let mut w = AudioSequenceTracker::new();
    w.observe(u32::MAX);
    assert_eq!(w.observe(0), SequenceEvent::InOrder);
    assert_eq!(w.observe(2), SequenceEvent::Gap { missing: 1 });
}
