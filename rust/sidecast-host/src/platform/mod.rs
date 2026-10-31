//! Real platform backends. Each OS module returns the [`Backends`] its host
//! uses; see `rust/README.md` for what each one has to provide.
//!
//! - Windows (ROADMAP P2, `windows/`): a virtual monitor from the signed
//!   Virtual Display Driver (or an existing monitor), DXGI Desktop
//!   Duplication, Media Foundation encoders, SendInput, SetThreadExecutionState
//!   and an optional virtual audio cable for the phone's microphone.
//! - Linux (ROADMAP P3): Mutter ScreenCast `RecordVirtual` + PipeWire,
//!   RemoteDesktop portal (libei), logind inhibitor, a PipeWire virtual
//!   source, VA-API/NVENC with OpenH264 as the fallback.

#[cfg(target_os = "linux")]
mod linux;
// Its pure parts (key and coordinate mapping, pixel conversion, the driver's
// settings file) are compiled and unit-tested on every OS.
#[cfg(any(target_os = "windows", test))]
pub mod windows;

use std::sync::{Arc, Mutex};

use sidecast_server::backend::{BackendError, Backends};

/// What the host passes to its platform backends.
#[derive(Debug, Clone, Default)]
pub struct PlatformOptions {
    /// Which existing monitor to stream instead of a virtual display.
    pub monitor: MonitorChoice,
}

/// The monitor to stream (`None`: automatic), shared with a user interface
/// that changes it between streams.
#[derive(Debug, Clone, Default)]
pub struct MonitorChoice(Arc<Mutex<Option<String>>>);

impl MonitorChoice {
    pub fn new(monitor: Option<String>) -> Self {
        MonitorChoice(Arc::new(Mutex::new(monitor)))
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub fn set(&self, monitor: Option<String>) {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = monitor;
    }
}

/// The backends of the OS this binary runs on.
pub fn backends(options: &PlatformOptions) -> Result<Backends, BackendError> {
    #[cfg(target_os = "windows")]
    return windows::backends(options);
    #[cfg(target_os = "linux")]
    return linux::backends(options);
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = options;
        Err(BackendError::not_supported(
            "this host is for Windows and Linux; on a Mac use the Sidecast app (make install-mac)",
        ))
    }
}

/// A human-readable report of what this computer offers the host (`--diagnose`).
pub fn diagnose() -> String {
    let mut report = String::new();
    #[cfg(target_os = "windows")]
    report.push_str(&windows::diagnose());
    #[cfg(not(target_os = "windows"))]
    report.push_str("No platform backends on this OS yet; --test-pattern works everywhere.\n");
    let encoders: Vec<String> = crate::host::builtin_encoders().iter().map(|e| e.name().to_owned()).collect();
    report.push_str(&format!(
        "Built-in encoders: {}\n",
        if encoders.is_empty() { "none".into() } else { encoders.join(", ") }
    ));
    report
}
