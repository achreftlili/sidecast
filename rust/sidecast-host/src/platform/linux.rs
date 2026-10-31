//! Linux backends: not implemented yet (ROADMAP P3, a separate pull request).
//!
//! The pull request that implements them provides here, GNOME on Wayland first:
//! - `DisplayBackend`: a virtual monitor and its frames through Mutter's
//!   ScreenCast `RecordVirtual` and PipeWire (KDE, wlroots and X11 later,
//!   behind capability checks);
//! - `EncoderFactory`s: VA-API / NVENC, with OpenH264 (`OpenH264Factory`)
//!   as the fallback;
//! - `InputBackend`: the RemoteDesktop portal (libei), uinput as a fallback,
//!   mapping `SemanticAction`s to the desktop's shortcuts (GNOME:
//!   Super+Page Up/Down, the Activities overview);
//! - `PowerGuard`: a logind idle/sleep inhibitor;
//! - `MicSink`: a PipeWire virtual source fed from the jitter buffer.

use sidecast_server::backend::{BackendError, Backends};

pub fn backends() -> Result<Backends, BackendError> {
    Err(BackendError::not_supported("the Linux host is not supported yet (planned: ROADMAP P3); try --test-pattern"))
}
