//! The Sidecast host for Windows and Linux, shared by its two front ends: the
//! command-line `sidecast-host` (test-pattern mode on any OS, the real
//! backends on Windows) and the Windows tray app `sidecast-tray`.
//!
//! With `--test-pattern` the host runs the real server (TLS, pairing, flow
//! control, encoder) on a synthetic pattern and logs input instead of
//! injecting it. Otherwise it uses this OS's platform backends
//! ([`platform`]): on Windows a virtual monitor from the Virtual Display
//! Driver (or an existing monitor), DXGI capture, Media Foundation encoders,
//! SendInput, a keep-awake guard and an optional virtual audio cable.

pub mod host;
pub mod logger;
pub mod options;
pub mod platform;
pub mod test_pattern;
#[cfg(windows)]
pub mod tray;
