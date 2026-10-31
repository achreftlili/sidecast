//! Pure, platform-independent logic shared by every Sidecast host and test
//! client written in Rust: the wire protocol of `docs/PROTOCOL.md`, pairing
//! and tokens, the paired-device model, flow control and stream planning, the
//! gesture engine, keyboard mapping, the microphone jitter buffer and address
//! classification.
//!
//! Nothing here touches the network, the file system or the clock: callers
//! pass bytes in and times in (seconds on any monotonic clock), which keeps
//! every rule unit-testable. It is a port of the Mac app's `SidecastKit` and
//! must stay behaviourally identical to it.

pub mod annexb;
pub mod audio;
pub mod geometry;
pub mod input;
pub mod net;
pub mod protocol;
pub mod security;
pub mod store;
pub mod streaming;

pub use geometry::{Point, Size};
