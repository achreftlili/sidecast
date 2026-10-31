//! Which Linux desktop this host runs in, and so which backend it uses:
//! GNOME's own D-Bus API (Mutter) when it is there, the xdg-desktop-portal
//! elsewhere on Wayland, and a clear error on X11.

/// The graphical session, from the standard environment variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Wayland,
    X11,
    /// No session variables (e.g. started over SSH or by a test harness):
    /// try the D-Bus APIs anyway.
    Unknown,
}

/// `XDG_SESSION_TYPE` says it; without it `WAYLAND_DISPLAY` / `DISPLAY` hint at it.
pub fn session_kind(
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    x11_display: Option<&str>,
) -> SessionKind {
    let set = |v: Option<&str>| v.is_some_and(|v| !v.trim().is_empty());
    match xdg_session_type.map(|t| t.trim().to_ascii_lowercase()).as_deref() {
        Some("wayland") => SessionKind::Wayland,
        Some("x11") => SessionKind::X11,
        _ if set(wayland_display) => SessionKind::Wayland,
        _ if set(x11_display) => SessionKind::X11,
        _ => SessionKind::Unknown,
    }
}

/// Whether `XDG_CURRENT_DESKTOP` (a colon-separated list) names GNOME
/// (Ubuntu's session says `ubuntu:GNOME`).
pub fn is_gnome(current_desktop: Option<&str>) -> bool {
    current_desktop.is_some_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("gnome")))
}

/// Where the virtual display and input come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// `org.gnome.Mutter.ScreenCast` `RecordVirtual` + `org.gnome.Mutter.RemoteDesktop`.
    Mutter,
    /// The ScreenCast and RemoteDesktop portals (KDE Plasma 6, wlroots…), with
    /// the user's consent dialog.
    Portal,
}

impl BackendKind {
    /// `SIDECAST_LINUX_BACKEND=mutter|portal` overrides the automatic choice.
    pub fn parse(raw: &str) -> Option<BackendKind> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mutter" | "gnome" => Some(BackendKind::Mutter),
            "portal" => Some(BackendKind::Portal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Mutter => "mutter",
            BackendKind::Portal => "portal",
        }
    }
}

/// The backend to use: an override wins, then Mutter whenever its D-Bus API
/// is on the session bus, else the portal.
pub fn choose_backend(forced: Option<BackendKind>, mutter_available: bool) -> BackendKind {
    match forced {
        Some(kind) => kind,
        None if mutter_available => BackendKind::Mutter,
        None => BackendKind::Portal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_session_type() {
        assert_eq!(session_kind(Some("wayland"), None, Some(":0")), SessionKind::Wayland);
        assert_eq!(session_kind(Some("x11"), None, Some(":0")), SessionKind::X11);
        assert_eq!(session_kind(Some("X11 "), None, None), SessionKind::X11);
        assert_eq!(session_kind(None, Some("wayland-0"), Some(":0")), SessionKind::Wayland, "XWayland sets both");
        assert_eq!(session_kind(Some("tty"), None, Some(":1")), SessionKind::X11);
        assert_eq!(session_kind(None, Some(""), None), SessionKind::Unknown);
        assert_eq!(session_kind(None, None, None), SessionKind::Unknown);
    }

    #[test]
    fn recognises_gnome_and_ubuntu_sessions() {
        assert!(is_gnome(Some("GNOME")));
        assert!(is_gnome(Some("ubuntu:GNOME")));
        assert!(is_gnome(Some("gnome-classic:GNOME")));
        assert!(!is_gnome(Some("KDE")));
        assert!(!is_gnome(Some("sway")));
        assert!(!is_gnome(None));
    }

    #[test]
    fn chooses_mutter_when_available_unless_overridden() {
        assert_eq!(choose_backend(None, true), BackendKind::Mutter);
        assert_eq!(choose_backend(None, false), BackendKind::Portal);
        assert_eq!(choose_backend(Some(BackendKind::Portal), true), BackendKind::Portal);
        assert_eq!(BackendKind::parse(" Portal"), Some(BackendKind::Portal));
        assert_eq!(BackendKind::parse("gnome"), Some(BackendKind::Mutter));
        assert_eq!(BackendKind::parse("x11"), None);
        assert_eq!(BackendKind::Mutter.name(), "mutter");
    }
}
