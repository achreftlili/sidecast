//! Real platform backends. Each OS module returns the [`Backends`] its host
//! uses; see `rust/README.md` for what each one has to provide.
//!
//! - Windows (ROADMAP P2, `windows/`): a virtual monitor from the signed
//!   Virtual Display Driver (or an existing monitor), DXGI Desktop
//!   Duplication, Media Foundation encoders, SendInput, SetThreadExecutionState
//!   and an optional virtual audio cable for the phone's microphone.
//! - Linux (ROADMAP P3, `linux/`): Mutter ScreenCast `RecordVirtual` (or an
//!   existing monitor) + PipeWire on GNOME, the xdg-desktop-portal elsewhere on
//!   Wayland, the RemoteDesktop session for input, a logind inhibitor, a
//!   PipeWire virtual source, GStreamer hardware encoders with OpenH264 as the
//!   fallback, and a tray icon.

// The pure parts of both (key, pixel and coordinate mapping; Windows: the
// driver's settings file) are compiled and unit-tested on every OS.
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "windows", test))]
pub mod windows;

use std::sync::{Arc, Mutex};

use sidecast_server::ServerEvent;
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
    #[cfg(target_os = "linux")]
    report.push_str(&linux::diagnose());
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    report.push_str("No platform backends on this OS yet; --test-pattern works everywhere.\n");
    let encoders: Vec<String> = crate::host::builtin_encoders().iter().map(|e| e.name().to_owned()).collect();
    report.push_str(&format!(
        "Built-in encoders: {}\n",
        if encoders.is_empty() { "none".into() } else { encoders.join(", ") }
    ));
    report
}

/// A click in the console host's desktop integration (the Linux tray icon menu).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum UiCommand {
    Forget { device_id: String, name: String },
    Quit,
}

/// The console host's desktop integration (tray icon, notifications), where
/// the OS has one. (Windows has its own tray app, `sidecast-tray`.)
pub trait DesktopUi: Send + Sync {
    /// Status, pairing code and streaming changes.
    fn server_event(&self, event: &ServerEvent);

    /// The paired phones as (device id, name).
    fn set_devices(&self, devices: Vec<(String, String)>);
}

/// Starts the tray icon of the console host where this OS has one (Linux: a
/// StatusNotifierItem); `None` elsewhere.
pub fn start_desktop_ui(commands: impl Fn(UiCommand) + Send + Sync + 'static) -> Option<Arc<dyn DesktopUi>> {
    #[cfg(target_os = "linux")]
    return Some(Arc::new(linux::start_desktop_ui(commands)));
    #[cfg(not(target_os = "linux"))]
    {
        let _ = commands;
        None
    }
}
