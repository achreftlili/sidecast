//! The Windows `DisplayBackend`: which monitor a phone gets, and its capture.
//!
//! - With the Virtual Display Driver installed, one of its monitors is set
//!   to the phone's resolution and attached to the desktop (extending it, to
//!   the right of the other monitors) while the phone streams; afterwards it
//!   goes back to how it was (detached again, or its previous mode). A
//!   resolution the driver does not offer yet is added to its settings file
//!   and the driver reloaded (DECISIONS Q37).
//! - Without it (or when that fails, or when the user picked a monitor), an
//!   existing monitor is streamed: the chosen one, else a secondary one,
//!   else the primary (mirroring). A monitor of another shape is letterboxed.

use std::path::Path;
use std::time::{Duration, Instant};

use sidecast_core::streaming::StreamPlan;
use sidecast_core::{Point, Size};
use sidecast_server::backend::{BackendError, CaptureSession, ClientInfo, DisplayBackend, DisplayGeometry, FrameSink};

use super::capture::Capture;
use super::display::{self, Monitor, Placement};
use super::mapping::Rect;
use super::vdd;
use super::vdd_config::{Mode, RELEASES_URL, RELOAD_DRIVER, add_mode, best_mode, set_display_count};
use crate::platform::MonitorChoice;

/// Streams a virtual monitor of the Virtual Display Driver, or an existing monitor.
#[derive(Debug)]
pub struct WindowsDisplay {
    choice: MonitorChoice,
}

impl WindowsDisplay {
    pub fn new(choice: MonitorChoice) -> Self {
        WindowsDisplay { choice }
    }

    /// Picks (and for a virtual monitor, prepares) the monitor for `plan`.
    fn target(&self, plan: &StreamPlan) -> Result<Target, BackendError> {
        let monitors = display::monitors();
        if let Some(name) = self.choice.get() {
            match chosen(&monitors, &name) {
                Some(monitor) => {
                    log::info!("display: streaming the chosen monitor {}", monitor.describe());
                    return Ok(Target::existing(monitor));
                }
                None => log::warn!("display: the chosen monitor {name} is not attached; choosing automatically"),
            }
        }
        let status = vdd::Status::probe();
        if status.usable() {
            let wanted = Mode::new(
                plan.display.pixel_width() as u32,
                plan.display.pixel_height() as u32,
                plan.display.refresh_rate.max(1) as u32,
            );
            match virtual_monitor(wanted) {
                Ok(target) => return Ok(target),
                Err(error) => log::warn!(
                    "display: the Virtual Display Driver could not provide a {wanted} monitor ({error}); streaming an existing monitor instead"
                ),
            }
        } else {
            log::info!(
                "display: the Virtual Display Driver is {}; streaming an existing monitor instead of an extra screen (install the driver from {RELEASES_URL})",
                status.describe()
            );
        }
        let monitors = display::monitors();
        let pick = automatic(&monitors).ok_or_else(|| BackendError::display_failed("no monitor is attached"))?;
        log::info!(
            "display: streaming {} ({})",
            pick.describe(),
            if pick.primary { "the primary monitor: mirroring" } else { "a secondary monitor" }
        );
        Ok(Target::existing(pick))
    }
}

impl DisplayBackend for WindowsDisplay {
    fn name(&self) -> &str {
        "windows-dxgi"
    }

    fn preflight(&self) -> Result<(), BackendError> {
        if display::monitors().iter().any(|m| m.attached) {
            Ok(())
        } else {
            Err(BackendError::display_failed("no monitor is attached to this desktop (a remote or locked session?)"))
        }
    }

    fn start(
        &self,
        plan: &StreamPlan,
        client: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError> {
        let mut target = self.target(plan)?;
        let capture = Capture::start(&target.device, plan, frames)?;
        let geometry = DisplayGeometry {
            size: Size::new(plan.display.width as f64, plan.display.height as f64),
            origin: Point::new(target.rect.left as f64, target.rect.top as f64),
            id: Some(target.device.clone()),
        };
        log::info!(
            "display: {} for {} at ({}, {}) {}x{} px",
            target.device,
            client.device_name,
            target.rect.left,
            target.rect.top,
            target.rect.width,
            target.rect.height
        );
        Ok(Box::new(WindowsCapture { capture: Some(capture), restore: target.restore.take(), geometry }))
    }
}

/// The monitor a stream uses, and how to put it back.
struct Target {
    device: String,
    rect: Rect,
    restore: Option<Restore>,
}

impl Target {
    fn existing(monitor: &Monitor) -> Target {
        Target { device: monitor.device.clone(), rect: monitor.rect, restore: None }
    }
}

/// How a virtual monitor was before the stream.
#[derive(Debug)]
struct Restore {
    device: String,
    /// `None`: it was detached.
    previous: Option<(Rect, u32)>,
}

impl Restore {
    fn apply(self) {
        let placement = match self.previous {
            None => Placement::Detached,
            Some((rect, refresh)) => Placement::Attached {
                mode: Mode::new(rect.width as u32, rect.height as u32, refresh),
                position: (rect.left, rect.top),
            },
        };
        match display::place(&self.device, placement) {
            Ok(()) => log::info!(
                "display: virtual monitor {} {}",
                self.device,
                if self.previous.is_some() { "back to its previous mode" } else { "detached" }
            ),
            Err(error) => log::warn!("display: cannot restore the virtual monitor: {error}"),
        }
    }
}

/// A running capture; dropping it stops capture, then restores the monitor.
struct WindowsCapture {
    capture: Option<Capture>,
    restore: Option<Restore>,
    geometry: DisplayGeometry,
}

impl CaptureSession for WindowsCapture {
    fn geometry(&self) -> DisplayGeometry {
        self.geometry.clone()
    }

    fn request_refresh(&mut self) {
        if let Some(capture) = &self.capture {
            capture.refresh();
        }
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        drop(self.capture.take());
        if let Some(restore) = self.restore.take() {
            restore.apply();
        }
    }
}

/// The attached monitor the user chose: a device name, or `primary`.
fn chosen<'a>(monitors: &'a [Monitor], name: &str) -> Option<&'a Monitor> {
    monitors.iter().filter(|m| m.attached).find(|m| {
        if name.eq_ignore_ascii_case("primary") { m.primary } else { m.device.eq_ignore_ascii_case(name.trim()) }
    })
}

/// A secondary monitor if there is one, else the primary.
fn automatic(monitors: &[Monitor]) -> Option<&Monitor> {
    let attached = || monitors.iter().filter(|m| m.attached);
    attached().find(|m| !m.primary).or_else(|| attached().find(|m| m.primary)).or_else(|| attached().next())
}

/// Polls `check` every 250 ms until it returns something or `timeout` passes.
fn wait_for<T>(timeout: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// One of the driver's monitors, attached at `wanted` (or the closest mode it offers).
fn virtual_monitor(wanted: Mode) -> Result<Target, String> {
    let mut monitor = match vdd::monitors().into_iter().max_by_key(|m| m.attached) {
        Some(monitor) => monitor,
        None => {
            // Installed and running, but with no monitor: ask for one.
            vdd::send(&set_display_count(1))?;
            wait_for(Duration::from_secs(10), || vdd::monitors().into_iter().next())
                .ok_or("the driver did not add a monitor")?
        }
    };

    if !display::modes(&monitor.device).contains(&wanted) {
        match offer_mode(wanted) {
            Ok(reloaded) => {
                if reloaded {
                    // The driver restarts its adapter: monitors leave and come back.
                    let found = wait_for(Duration::from_secs(15), || {
                        vdd::monitors().into_iter().find(|m| display::modes(&m.device).contains(&wanted))
                    });
                    match found {
                        Some(m) => monitor = m,
                        None => {
                            log::warn!("display: the driver reloaded but does not offer {wanted} yet");
                            if let Some(m) = vdd::monitors().into_iter().max_by_key(|m| m.attached) {
                                monitor = m;
                            }
                        }
                    }
                }
            }
            Err(error) => log::warn!("display: {error}"),
        }
    }
    let offered = display::modes(&monitor.device);
    let mode = if offered.contains(&wanted) {
        wanted
    } else {
        let mode = best_mode(&offered, wanted).ok_or("the virtual monitor offers no mode")?;
        log::warn!("display: the virtual monitor does not offer {wanted}; using {mode} (the picture is scaled)");
        mode
    };

    let previous = if monitor.attached { display::current_mode(&monitor.device) } else { None };
    let position = match previous {
        Some((rect, _)) => (rect.left, rect.top),
        None => {
            // To the right of everything else, top-aligned with the primary monitor.
            let others = display::monitors();
            let right = others.iter().filter(|m| m.attached).map(|m| m.rect.right()).max().unwrap_or(0);
            let top = others.iter().find(|m| m.primary).map_or(0, |m| m.rect.top);
            (right, top)
        }
    };
    let placement = Placement::Attached { mode, position };
    if let Err(error) = display::place(&monitor.device, placement) {
        // Some setups refuse a refresh rate: try the same size at any offered rate.
        let fallback = offered.iter().find(|m| (m.width, m.height) == (mode.width, mode.height) && **m != mode);
        match fallback {
            Some(other) => display::place(&monitor.device, Placement::Attached { mode: *other, position })
                .map_err(|e| format!("{error}; {e}"))?,
            None => return Err(error),
        }
    }
    let restore = Restore { device: monitor.device.clone(), previous };
    let applied = wait_for(Duration::from_secs(5), || {
        display::current_mode(&monitor.device)
            .filter(|(rect, _)| (rect.width as u32, rect.height as u32) == (mode.width, mode.height))
    });
    let Some((rect, refresh)) = applied else {
        restore.apply();
        return Err(format!("{} did not switch to {mode}", monitor.device));
    };
    log::info!(
        "display: virtual monitor {} attached at {}x{}@{refresh} ({}, {}){}",
        monitor.device,
        rect.width,
        rect.height,
        rect.left,
        rect.top,
        if previous.is_some() { "" } else { ", extending the desktop" }
    );
    Ok(Target { device: monitor.device.clone(), rect, restore: Some(restore) })
}

/// Makes the driver offer `mode`: adds it to its settings file if missing
/// and reloads the driver. `Ok(true)` when the driver was reloaded.
fn offer_mode(mode: Mode) -> Result<bool, String> {
    let (path, text, settings) = vdd::read_settings().ok_or_else(|| {
        format!(
            "cannot read the Virtual Display Driver's settings ({}); add {mode} to its resolutions to stream at the phone's resolution",
            vdd::settings_path().display()
        )
    })?;
    if !settings.offers(mode) {
        let edited = add_mode(&text, mode)?;
        write_replacing(&path, &edited).map_err(|e| {
            format!(
                "cannot add {mode} to {} ({e}); add it with the driver's Virtual Driver Control app, or give your account write access to that file",
                path.display()
            )
        })?;
        log::info!("display: added {mode} to {}", path.display());
    }
    vdd::send(RELOAD_DRIVER)?;
    Ok(true)
}

/// Writes `text` to `path` through a temporary file in the same folder.
fn write_replacing(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("xml.sidecast.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(device: &str, attached: bool, primary: bool) -> Monitor {
        Monitor {
            device: device.into(),
            adapter: "GPU".into(),
            adapter_id: String::new(),
            monitor_name: String::new(),
            monitor_id: String::new(),
            attached,
            primary,
            rect: Rect::new(0, 0, 1920, 1080),
            refresh: 60,
        }
    }

    #[test]
    fn automatic_prefers_a_secondary_monitor() {
        let one = [monitor(r"\\.\DISPLAY1", true, true)];
        assert_eq!(automatic(&one).unwrap().device, r"\\.\DISPLAY1");
        let two = [monitor(r"\\.\DISPLAY1", true, true), monitor(r"\\.\DISPLAY2", true, false)];
        assert_eq!(automatic(&two).unwrap().device, r"\\.\DISPLAY2");
        let detached = [monitor(r"\\.\DISPLAY1", true, true), monitor(r"\\.\DISPLAY2", false, false)];
        assert_eq!(automatic(&detached).unwrap().device, r"\\.\DISPLAY1");
        assert!(automatic(&[]).is_none());
        assert_eq!(chosen(&two, "primary").unwrap().device, r"\\.\DISPLAY1");
        assert_eq!(chosen(&two, r"\\.\display2").unwrap().device, r"\\.\DISPLAY2");
        assert!(chosen(&detached, r"\\.\DISPLAY2").is_none(), "a detached monitor cannot be streamed");
    }
}
