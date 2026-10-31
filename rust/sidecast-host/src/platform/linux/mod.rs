//! Linux backends (ROADMAP P3, DECISIONS Q38), GNOME on Wayland first:
//! - display + capture: Mutter's ScreenCast `RecordVirtual` (a real extra
//!   monitor) over PipeWire (`mutter`, `capture`); elsewhere on Wayland the
//!   xdg-desktop-portal, virtual where it can (`portal`); X11 is refused;
//! - input: the RemoteDesktop session linked to that screencast
//!   (`remote_input`, `translate`, `keymap`), swipes as GNOME shortcuts;
//! - encoders: hardware H.264 through GStreamer with the `gstreamer` feature
//!   (`gst_encoder`), OpenH264 always last (added by `main`);
//! - keep-awake: logind (`power`); microphone: a PipeWire source (`mic`);
//! - the tray icon and pairing notifications (`tray`).
//!
//! The pure parts (desktop detection, key/pixel/coordinate mapping, the tray
//! menu) also build in tests on every OS.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod desktop;
mod keymap;
mod menu;
mod monitors;
mod pixels;
mod translate;

#[cfg(target_os = "linux")]
mod capture;
#[cfg(target_os = "linux")]
mod display_config;
#[cfg(all(target_os = "linux", feature = "gstreamer"))]
mod gst_encoder;
#[cfg(target_os = "linux")]
mod mic;
#[cfg(target_os = "linux")]
mod mutter;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod power;
#[cfg(target_os = "linux")]
mod remote_input;
#[cfg(target_os = "linux")]
mod tray;

#[cfg(target_os = "linux")]
pub use self::imp::{backends, diagnose, start_desktop_ui};

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::Arc;

    use sidecast_server::backend::{BackendError, Backends};

    use super::desktop::{self, BackendKind, SessionKind};
    use super::keymap::TextInput;
    use super::mutter::MutterBackend;
    use super::portal::PortalBackend;
    use super::tray::TrayUi;
    use crate::platform::{PlatformOptions, UiCommand};

    fn env(key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    /// The backends for this session: GNOME's own APIs, else the portal.
    pub fn backends(_options: &PlatformOptions) -> Result<Backends, BackendError> {
        let session = desktop::session_kind(
            env("XDG_SESSION_TYPE").as_deref(),
            env("WAYLAND_DISPLAY").as_deref(),
            env("DISPLAY").as_deref(),
        );
        if session == SessionKind::X11 {
            return Err(BackendError::not_supported(
                "X11 sessions are not supported yet: log out, pick “Ubuntu” / “GNOME” (Wayland) with the gear \
                 on the login screen, and start Sidecast again",
            ));
        }
        let forced = match env("SIDECAST_LINUX_BACKEND") {
            Some(raw) => Some(BackendKind::parse(&raw).ok_or_else(|| {
                BackendError::not_supported(format!("SIDECAST_LINUX_BACKEND={raw}: use mutter or portal"))
            })?),
            None => None,
        };
        let text_input = env("SIDECAST_TEXT_INPUT").and_then(|raw| TextInput::parse(&raw)).unwrap_or_default();
        let connection = zbus::block_on(zbus::Connection::session()).map_err(|e| {
            BackendError::not_supported(format!("no D-Bus session bus ({e}): run inside your desktop session"))
        })?;
        let mutter = MutterBackend::available(&connection);
        let kind = desktop::choose_backend(forced, mutter);
        let current_desktop = env("XDG_CURRENT_DESKTOP");
        log::info!(
            "linux: {} backend ({:?} session, desktop {})",
            kind.name(),
            session,
            current_desktop.as_deref().unwrap_or("unknown")
        );
        if !mutter && desktop::is_gnome(current_desktop.as_deref()) {
            log::warn!("linux: GNOME without Mutter's ScreenCast/RemoteDesktop API on this bus; using the portal");
        }
        let mut backends = match kind {
            BackendKind::Mutter => {
                let backend = Arc::new(MutterBackend::new(connection.clone(), text_input));
                let mut backends = Backends::new(backend.clone(), Vec::new());
                backends.input = Some(backend);
                backends
            }
            BackendKind::Portal => {
                let backend = Arc::new(PortalBackend::new(connection.clone(), text_input));
                let mut backends = Backends::new(backend.clone(), Vec::new());
                backends.input = Some(backend);
                backends
            }
        };
        #[cfg(feature = "gstreamer")]
        if let Some(factory) = super::gst_encoder::GstEncoderFactory::detect() {
            backends.encoders.push(Arc::new(factory));
        }
        backends.mic = Some(Arc::new(super::mic::PipeWireMic::default()));
        backends.power = Some(Arc::new(super::power::LogindPower::new(Some(connection))));
        Ok(backends)
    }

    /// What `--diagnose` prints on Linux: the session, the backend it would use, encoders.
    pub fn diagnose() -> String {
        let mut report = String::new();
        let session = desktop::session_kind(
            env("XDG_SESSION_TYPE").as_deref(),
            env("WAYLAND_DISPLAY").as_deref(),
            env("DISPLAY").as_deref(),
        );
        report.push_str(&format!(
            "Session: {session:?}, desktop {}\n",
            env("XDG_CURRENT_DESKTOP").unwrap_or_else(|| "unknown".into())
        ));
        match zbus::block_on(zbus::Connection::session()) {
            Ok(connection) => {
                let mutter = MutterBackend::available(&connection);
                report.push_str(&format!(
                    "Mutter ScreenCast + RemoteDesktop: {}\nBackend: {}\n",
                    if mutter { "available" } else { "not on the session bus" },
                    desktop::choose_backend(None, mutter).name()
                ));
            }
            Err(error) => report.push_str(&format!("No D-Bus session bus: {error}\n")),
        }
        #[cfg(feature = "gstreamer")]
        report.push_str(&format!(
            "Hardware encoder (GStreamer): {}\n",
            super::gst_encoder::GstEncoderFactory::detect()
                .map_or("none works here".to_owned(), |f| sidecast_server::backend::EncoderFactory::name(&f)
                    .to_owned())
        ));
        #[cfg(not(feature = "gstreamer"))]
        report.push_str("Hardware encoder: not built in (build with --features gstreamer)\n");
        report
    }

    /// The tray icon and notifications; clicks arrive on `commands`.
    pub fn start_desktop_ui(commands: impl Fn(UiCommand) + Send + Sync + 'static) -> TrayUi {
        TrayUi::start(commands)
    }
}
