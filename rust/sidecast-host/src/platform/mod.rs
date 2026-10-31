//! Real platform backends. Each OS module returns the [`Backends`] its host
//! uses; see `rust/README.md` for what each one has to provide.
//!
//! - Windows (ROADMAP P2): IddCx virtual monitor (Virtual Display Driver),
//!   Windows.Graphics.Capture, NVENC/AMF/QSV/Media Foundation, SendInput /
//!   InjectTouchInput, SetThreadExecutionState.
//! - Linux (ROADMAP P3): Mutter ScreenCast `RecordVirtual` + PipeWire,
//!   RemoteDesktop portal (libei), logind inhibitor, a PipeWire virtual
//!   source, VA-API/NVENC with OpenH264 as the fallback.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

use sidecast_server::backend::{BackendError, Backends};

/// The backends of the OS this binary runs on.
pub fn backends() -> Result<Backends, BackendError> {
    #[cfg(target_os = "windows")]
    return windows::backends();
    #[cfg(target_os = "linux")]
    return linux::backends();
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    Err(BackendError::not_supported(
        "this host is for Windows and Linux; on a Mac use the Sidecast app (make install-mac)",
    ))
}
