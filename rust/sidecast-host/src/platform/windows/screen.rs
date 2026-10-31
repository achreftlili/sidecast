//! The Windows `DisplayBackend`: which monitor a phone gets, and its capture.
//!
//! - With the Virtual Display Driver installed, one of its monitors is set
//!   to the phone's resolution and attached to the desktop (extending it,
//!   where the user last put it, else to the right of the other monitors)
//!   while the phone streams, and detached again afterwards, so no empty
//!   screen stays where windows and the pointer get lost. (When it is the
//!   only display, as on a headless PC, its previous mode comes back
//!   instead.) A resolution the driver does not offer yet is added to its
//!   settings file and the driver reloaded (DECISIONS Q37).
//! - Without it (or when that fails, or when the user picked a monitor), an
//!   existing monitor is streamed: the chosen one, else a secondary one,
//!   else the primary (mirroring). A monitor of another shape is letterboxed.

use std::path::Path;
use std::sync::{Arc, Mutex};
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

/// Where the user last had the virtual monitor (top-left, desktop pixels).
type LastPosition = Arc<Mutex<Option<(i32, i32)>>>;

/// Streams a virtual monitor of the Virtual Display Driver, or an existing monitor.
#[derive(Debug)]
pub struct WindowsDisplay {
    choice: MonitorChoice,
    last_position: LastPosition,
}

impl WindowsDisplay {
    pub fn new(choice: MonitorChoice) -> Self {
        WindowsDisplay { choice, last_position: LastPosition::default() }
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
            match virtual_monitor(wanted, &self.last_position) {
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

/// What to do with a virtual monitor after its stream.
#[derive(Debug)]
struct Restore {
    device: String,
    /// Its mode before the stream (`None`: it was detached).
    previous: Option<(Rect, u32)>,
    last_position: LastPosition,
}

impl Restore {
    fn apply(self) {
        // The user may have moved it during the stream: start there next time.
        if let Some((rect, _)) = display::current_mode(&self.device) {
            *self.last_position.lock().unwrap_or_else(|p| p.into_inner()) = Some((rect.left, rect.top));
        }
        let others = display::monitors().into_iter().any(|m| m.attached && m.device != self.device);
        let placement = match (others, self.previous) {
            (true, _) | (false, None) => Placement::Detached,
            // The only display (a headless PC): detaching it would leave no desktop.
            (false, Some((rect, refresh))) => Placement::Attached {
                mode: Mode::new(rect.width as u32, rect.height as u32, refresh),
                position: (rect.left, rect.top),
            },
        };
        match display::place(&self.device, placement) {
            Ok(()) => log::info!(
                "display: virtual monitor {} {}",
                self.device,
                if placement == Placement::Detached { "detached" } else { "back to its previous mode" }
            ),
            Err(error) => log::warn!("display: cannot put the virtual monitor back: {error}"),
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

/// Where to attach `monitor`: where it is now; else where the user last had
/// it; else to the right of everything, top-aligned with the primary monitor.
fn positions(monitor: &Monitor, last_position: &LastPosition) -> Vec<(i32, i32)> {
    if monitor.attached {
        return vec![(monitor.rect.left, monitor.rect.top)];
    }
    let mut positions: Vec<(i32, i32)> =
        last_position.lock().unwrap_or_else(|p| p.into_inner()).iter().copied().collect();
    let others: Vec<Monitor> =
        display::monitors().into_iter().filter(|m| m.attached && m.device != monitor.device).collect();
    let right = others.iter().map(|m| m.rect.right()).max().unwrap_or(0);
    let top = others.iter().find(|m| m.primary).map_or(0, |m| m.rect.top);
    positions.push((right, top));
    positions
}

/// Attaches `device` at the first of `modes` Windows accepts, trying each
/// position. Windows refuses modes the monitor does not offer, which also
/// works for a detached monitor that lists no modes.
fn attach(device: &str, modes: &[Mode], positions: &[(i32, i32)]) -> Result<Mode, String> {
    let mut errors = Vec::new();
    for mode in modes {
        for position in positions {
            match display::place(device, Placement::Attached { mode: *mode, position: *position }) {
                Ok(()) => return Ok(*mode),
                Err(error) => errors.push(error),
            }
        }
    }
    Err(if errors.is_empty() { "no mode to try".into() } else { errors.join("; ") })
}

/// One of the driver's monitors, attached at `wanted` (or the closest mode it offers).
fn virtual_monitor(wanted: Mode, last_position: &LastPosition) -> Result<Target, String> {
    let mut monitor = match vdd::monitors().into_iter().max_by_key(|m| m.attached) {
        Some(monitor) => monitor,
        None => {
            // Installed and running, but with no monitor: ask for one.
            vdd::send(&set_display_count(1))?;
            wait_for(Duration::from_secs(10), || vdd::monitors().into_iter().next())
                .ok_or("the driver did not add a monitor")?
        }
    };
    let previous = if monitor.attached { display::current_mode(&monitor.device) } else { None };

    // 1. Straight at the phone's mode: works whenever the driver offers it.
    let mut placed = attach(&monitor.device, &[wanted], &positions(&monitor, last_position));
    // 2. Not offered: add it to the driver's settings, restart the driver
    //    (its monitors leave and come back), and attach the one that returns.
    if placed.is_err() {
        match offer_mode(wanted) {
            Ok(()) => {
                let found = wait_for(Duration::from_secs(15), || {
                    vdd::monitors().into_iter().find_map(|m| {
                        attach(&m.device, &[wanted], &positions(&m, last_position)).ok().map(|mode| (m, mode))
                    })
                });
                match found {
                    Some((m, mode)) => {
                        monitor = m;
                        placed = Ok(mode);
                    }
                    None => log::warn!("display: the driver restarted but does not take {wanted} yet"),
                }
            }
            Err(error) => log::warn!("display: {error}"),
        }
    }
    // 3. Still refused: the closest mode it offers (the picture is scaled).
    if placed.is_err() {
        if let Some(m) = vdd::monitors().into_iter().max_by_key(|m| m.attached) {
            monitor = m;
        }
        let offered = display::modes(&monitor.device);
        if let Some(closest) = best_mode(&offered, wanted) {
            log::warn!("display: the virtual monitor does not take {wanted}; using {closest} (the picture is scaled)");
            let same_size = offered.iter().copied().filter(|m| (m.width, m.height) == (closest.width, closest.height));
            let candidates: Vec<Mode> = std::iter::once(closest).chain(same_size.filter(|m| *m != closest)).collect();
            placed = attach(&monitor.device, &candidates, &positions(&monitor, last_position));
        }
    }
    let mode = placed.map_err(|e| format!("cannot attach the virtual monitor: {e}"))?;
    let restore = Restore { device: monitor.device.clone(), previous, last_position: last_position.clone() };
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

/// Makes the driver offer `mode`: adds it to its settings file if missing,
/// then restarts the driver so it reads them (also when the file already
/// had it, in case it was edited since the driver started).
fn offer_mode(mode: Mode) -> Result<(), String> {
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
    vdd::send(RELOAD_DRIVER)
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
