//! Port of `PairingTests.swift`: pairing codes, the pairing window, proofs and tokens.

use std::collections::{HashSet, VecDeque};

use sidecast_core::security::{
    PairingCode, PairingOutcome, PairingSession, RandomSource, SystemRandom, constant_time_eq, device_token,
    pairing_proof,
};

/// Replays a fixed byte sequence (then zeros).
struct ScriptedRandom(VecDeque<u8>);

impl ScriptedRandom {
    fn new(bytes: &[u8]) -> Self {
        ScriptedRandom(bytes.iter().copied().collect())
    }
}

impl RandomSource for ScriptedRandom {
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf {
            *b = self.0.pop_front().unwrap_or(0);
        }
    }
}

const FINGERPRINT: [u8; 32] = [0x11; 32];
const NONCE: [u8; 16] = [0x22; 16];

fn proof(code: &str) -> [u8; 32] {
    pairing_proof(code, &FINGERPRINT, &NONCE)
}

fn session(code: &str, started_at: f64) -> PairingSession {
    PairingSession::new(PairingCode::new(code).unwrap(), started_at)
}

#[test]
fn code_format_and_display() {
    let code = PairingCode::generate(&mut ScriptedRandom::new(&[0, 0, 0x30, 0x39])); // 12345
    assert_eq!(code.digits(), "012345");
    assert_eq!(code.display_string(), "012 345");
    assert_eq!(code.to_string(), "012 345");
    assert!(PairingCode::new("12345").is_none());
    assert!(PairingCode::new("12345a").is_none());
    assert!(PairingCode::new("１２３４５６").is_none()); // full-width digits are not ASCII
}

#[test]
fn code_generation_rejects_biased_range() {
    // 0xFFFFFFFF ≥ 4_294_000_000 is rejected; the next draw (999_999) is used.
    let code = PairingCode::generate(&mut ScriptedRandom::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x0F, 0x42, 0x3F]));
    assert_eq!(code.digits(), "999999");
    // 4_293_999_999 (just below the limit) is accepted → 999_999.
    let edge = PairingCode::generate(&mut ScriptedRandom::new(&[0xFF, 0xF1, 0x3D, 0x7F]));
    assert_eq!(edge.digits(), "999999");
}

#[test]
fn system_codes_are_six_digits_and_vary() {
    let codes: HashSet<String> =
        (0..50).map(|_| PairingCode::generate(&mut SystemRandom).digits().to_owned()).collect();
    assert!(codes.iter().all(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit())));
    assert!(codes.len() > 40);
}

#[test]
fn debug_output_hides_the_code() {
    let code = PairingCode::new("482913").unwrap();
    assert!(!format!("{code:?}").contains("482"));
}

#[test]
fn correct_proof_succeeds() {
    let mut s = session("482913", 100.0);
    assert_eq!(s.verify(&NONCE, &proof("482913"), &FINGERPRINT, 110.0), PairingOutcome::Success);
    assert!(s.is_finished());
    // Replays after success are refused.
    assert_eq!(s.verify(&NONCE, &proof("482913"), &FINGERPRINT, 111.0), PairingOutcome::Expired);
}

#[test]
fn proof_over_different_certificate_fails() {
    let mut s = session("482913", 0.0);
    let mitm = pairing_proof("482913", &[0x99; 32], &NONCE);
    assert_eq!(s.verify(&NONCE, &mitm, &FINGERPRINT, 1.0), PairingOutcome::BadCode { attempts_left: 4 });
}

#[test]
fn wrong_codes_count_down_then_lock_out() {
    let mut s = session("000001", 0.0);
    assert_eq!(s.seconds_remaining(0.0), 120);
    for left in (1..=4).rev() {
        assert_eq!(
            s.verify(&NONCE, &proof("999999"), &FINGERPRINT, 1.0),
            PairingOutcome::BadCode { attempts_left: left }
        );
    }
    assert_eq!(s.verify(&NONCE, &proof("999999"), &FINGERPRINT, 2.0), PairingOutcome::TooManyAttempts);
    assert_eq!(s.attempts_left(), 0);
    // Even the right code is refused once locked out.
    assert_eq!(s.verify(&NONCE, &proof("000001"), &FINGERPRINT, 3.0), PairingOutcome::Expired);
}

#[test]
fn retry_after_wrong_code_can_succeed() {
    let mut s = session("135790", 0.0);
    assert_eq!(s.verify(&NONCE, &proof("135791"), &FINGERPRINT, 1.0), PairingOutcome::BadCode { attempts_left: 4 });
    assert_eq!(s.verify(&NONCE, &proof("135790"), &FINGERPRINT, 2.0), PairingOutcome::Success);
}

#[test]
fn expiry() {
    let mut s = session("135790", 10.0);
    assert_eq!(s.seconds_remaining(129.5), 1);
    assert!(!s.is_expired(129.9));
    assert_eq!(s.verify(&NONCE, &proof("135790"), &FINGERPRINT, 130.0), PairingOutcome::Expired);
    assert_eq!(s.seconds_remaining(500.0), 0);
}

#[test]
fn malformed_proof_lengths_cost_an_attempt() {
    let mut s = session("135790", 0.0);
    assert_eq!(s.verify(&[0; 8], &proof("135790"), &FINGERPRINT, 1.0), PairingOutcome::BadCode { attempts_left: 4 });
    assert_eq!(s.verify(&NONCE, &[0; 31], &FINGERPRINT, 1.0), PairingOutcome::BadCode { attempts_left: 3 });
}

#[test]
fn constant_time_equals() {
    assert!(constant_time_eq(&[1, 2, 3], &[1, 2, 3]));
    assert!(!constant_time_eq(&[1, 2, 3], &[1, 2, 4]));
    assert!(!constant_time_eq(&[1, 2], &[1, 2, 3]));
    assert!(constant_time_eq(&[], &[]));
}

#[test]
fn tokens_are_random_32_bytes() {
    let a = device_token::generate(&mut SystemRandom);
    let b = device_token::generate(&mut SystemRandom);
    assert_eq!(a.len(), 32);
    assert_ne!(a, b);
}
