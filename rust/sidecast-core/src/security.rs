//! Pairing (PROTOCOL §5), device tokens and certificate fingerprints.

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::protocol::{PAIRING_ATTEMPTS, PAIRING_LIFETIME};

/// Source of cryptographically secure random bytes (injectable for tests).
pub trait RandomSource {
    fn fill(&mut self, buf: &mut [u8]);

    fn bytes(&mut self, count: usize) -> Vec<u8> {
        let mut out = vec![0; count];
        self.fill(&mut out);
        out
    }
}

/// The operating system's CSPRNG.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRandom;

impl RandomSource for SystemRandom {
    fn fill(&mut self, buf: &mut [u8]) {
        // Without the OS generator nothing secure can continue.
        getrandom::fill(buf).expect("the system random number generator failed");
    }
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Certificate fingerprint: SHA-256 of the leaf certificate's DER encoding (PROTOCOL §2).
pub fn certificate_fingerprint(der: &[u8]) -> [u8; 32] {
    sha256(der)
}

/// Compares two byte strings in time independent of where they differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Lowercase hexadecimal.
pub fn to_hex(bytes: &[u8]) -> String {
    use fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Parses lower- or uppercase hex; `None` if malformed.
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[hi, lo]| {
            let hi = (*hi as char).to_digit(16)?;
            let lo = (*lo as char).to_digit(16)?;
            Some((hi << 4 | lo) as u8)
        })
        .collect()
}

/// A random (version 4) UUID in the canonical upper-case form, as Swift's
/// `UUID().uuidString` writes it.
pub fn uuid_v4(random: &mut dyn RandomSource) -> String {
    let mut b = [0u8; 16];
    random.fill(&mut b);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex = to_hex(&b).to_ascii_uppercase();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Device tokens issued at pairing. Only `SHA-256(token)` is ever stored.
pub mod device_token {
    use super::RandomSource;

    pub const LENGTH: usize = 32;

    pub fn generate(random: &mut dyn RandomSource) -> [u8; LENGTH] {
        let mut token = [0u8; LENGTH];
        random.fill(&mut token);
        token
    }

    pub fn hash(token: &[u8]) -> [u8; 32] {
        super::sha256(token)
    }
}

/// The 6-digit code shown on the host during pairing.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    /// Exactly six ASCII digits, `000000`–`999999`.
    pub fn new(digits: &str) -> Option<PairingCode> {
        (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit())).then(|| PairingCode(digits.to_owned()))
    }

    /// Uniformly random code (rejection sampling, no modulo bias).
    pub fn generate(random: &mut dyn RandomSource) -> PairingCode {
        const RANGE: u64 = 1_000_000;
        const SPACE: u64 = 1 << 32;
        const LIMIT: u64 = SPACE - SPACE % RANGE; // 4_294_000_000
        loop {
            let mut b = [0u8; 4];
            random.fill(&mut b);
            let value = u32::from_be_bytes(b) as u64;
            if value < LIMIT {
                return PairingCode(format!("{:06}", value % RANGE));
            }
        }
    }

    pub fn digits(&self) -> &str {
        &self.0
    }

    /// "123 456", as shown to the user.
    pub fn display_string(&self) -> String {
        format!("{} {}", &self.0[..3], &self.0[3..])
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_string())
    }
}

/// Debug never prints the digits: they are a secret while pairing runs.
impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(******)")
    }
}

pub const NONCE_LENGTH: usize = 16;
pub const PROOF_LENGTH: usize = 32;

/// `proof = HMAC-SHA256(key = ASCII(code), message = certFingerprint ‖ nonce)` (PROTOCOL §5).
pub fn pairing_proof(code: &str, cert_fingerprint: &[u8], nonce: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(code.as_bytes()).expect("HMAC accepts any key length");
    mac.update(cert_fingerprint);
    mac.update(nonce);
    mac.finalize().into_bytes().into()
}

/// Result of checking one PAIR_PROOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingOutcome {
    Success,
    BadCode { attempts_left: u32 },
    TooManyAttempts,
    Expired,
}

/// One pairing window: a code, a deadline and an attempt budget. Pure state
/// machine; the caller supplies the time (seconds, monotonic).
#[derive(Debug, Clone)]
pub struct PairingSession {
    code: PairingCode,
    started_at: f64,
    lifetime: f64,
    attempts_left: u32,
    finished: bool,
}

impl PairingSession {
    pub fn new(code: PairingCode, started_at: f64) -> Self {
        Self::with_limits(code, started_at, PAIRING_LIFETIME, PAIRING_ATTEMPTS)
    }

    pub fn with_limits(code: PairingCode, started_at: f64, lifetime: f64, attempts: u32) -> Self {
        PairingSession { code, started_at, lifetime, attempts_left: attempts, finished: false }
    }

    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    pub fn attempts_left(&self) -> u32 {
        self.attempts_left
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn expires_at(&self) -> f64 {
        self.started_at + self.lifetime
    }

    pub fn is_expired(&self, now: f64) -> bool {
        now >= self.expires_at()
    }

    /// Whole seconds left, never negative.
    pub fn seconds_remaining(&self, now: f64) -> u64 {
        (self.expires_at() - now).ceil().max(0.0) as u64
    }

    /// Verifies a proof computed over `cert_fingerprint` (the host's own leaf).
    /// After `Success`, `TooManyAttempts` or `Expired` the session is finished
    /// and every further call returns `Expired`.
    pub fn verify(&mut self, nonce: &[u8], proof: &[u8], cert_fingerprint: &[u8], now: f64) -> PairingOutcome {
        if self.finished {
            return PairingOutcome::Expired;
        }
        if self.is_expired(now) {
            self.finished = true;
            return PairingOutcome::Expired;
        }
        if nonce.len() == NONCE_LENGTH && proof.len() == PROOF_LENGTH {
            let expected = pairing_proof(self.code.digits(), cert_fingerprint, nonce);
            if constant_time_eq(&expected, proof) {
                self.finished = true;
                return PairingOutcome::Success;
            }
        }
        self.attempts_left = self.attempts_left.saturating_sub(1);
        if self.attempts_left == 0 {
            self.finished = true;
            return PairingOutcome::TooManyAttempts;
        }
        PairingOutcome::BadCode { attempts_left: self.attempts_left }
    }
}
