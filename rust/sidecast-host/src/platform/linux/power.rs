//! Keep-awake while a phone streams: a logind `sleep:idle` block inhibitor
//! (released when its file descriptor is closed), plus GNOME's session
//! inhibitor for automatic suspend when GNOME is running. The screen lock is
//! left alone, and closing a laptop lid still suspends (logind's default).

use std::os::fd::OwnedFd;

use sidecast_server::backend::{BackendError, BackendErrorKind, PowerGuard};
use zbus::zvariant::OwnedFd as ZOwnedFd;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<ZOwnedFd>;
}

#[zbus::proxy(
    interface = "org.gnome.SessionManager",
    default_service = "org.gnome.SessionManager",
    default_path = "/org/gnome/SessionManager"
)]
trait SessionManager {
    fn inhibit(&self, app_id: &str, toplevel_xid: u32, reason: &str, flags: u32) -> zbus::Result<u32>;
    fn uninhibit(&self, cookie: u32) -> zbus::Result<()>;
}

/// `org.gnome.SessionManager.Inhibit` flag: automatic suspend.
const GNOME_INHIBIT_SUSPEND: u32 = 4;

#[derive(Debug)]
pub struct LogindPower {
    session_bus: Option<zbus::Connection>,
}

impl LogindPower {
    pub fn new(session_bus: Option<zbus::Connection>) -> Self {
        LogindPower { session_bus }
    }
}

/// Held while streaming; dropping it releases both inhibitors.
struct Inhibition {
    _logind: Option<OwnedFd>,
    gnome: Option<(SessionManagerProxy<'static>, u32)>,
}

impl Drop for Inhibition {
    fn drop(&mut self) {
        if let Some((proxy, cookie)) = self.gnome.take()
            && let Err(error) = zbus::block_on(proxy.uninhibit(cookie))
        {
            log::debug!("power: GNOME uninhibit: {error}");
        }
        log::info!("power: sleep allowed again");
    }
}

impl PowerGuard for LogindPower {
    fn keep_awake(&self, reason: &str) -> Result<Box<dyn Send>, BackendError> {
        let why = format!("Sidecast is {reason}");
        let logind = zbus::block_on(async {
            let system = zbus::Connection::system().await?;
            LoginManagerProxy::new(&system).await?.inhibit("sleep:idle", "Sidecast", &why, "block").await
        });
        let logind = match logind {
            Ok(fd) => Some(OwnedFd::from(fd)),
            Err(error) => {
                log::warn!("power: logind inhibitor unavailable: {error}");
                None
            }
        };
        let gnome = self.session_bus.as_ref().and_then(|bus| {
            zbus::block_on(async {
                let proxy = SessionManagerProxy::new(bus).await.ok()?;
                let cookie = proxy.inhibit("sidecast", 0, &why, GNOME_INHIBIT_SUSPEND).await.ok()?;
                Some((proxy, cookie))
            })
        });
        if logind.is_none() && gnome.is_none() {
            return Err(BackendError::new(BackendErrorKind::Other, "neither logind nor GNOME accepted an inhibitor"));
        }
        log::info!(
            "power: keeping the computer awake while streaming ({})",
            [logind.as_ref().map(|_| "logind sleep:idle"), gnome.as_ref().map(|_| "GNOME suspend")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(Box::new(Inhibition { _logind: logind, gnome }))
    }
}
