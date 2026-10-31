//! Mutter's `org.gnome.Mutter.DisplayConfig`: gives the virtual monitor the
//! stream's scale (200 % when the phone asked for HiDPI). Applied with the
//! "temporary" method: no confirmation dialog, nothing written to the user's
//! saved layout, gone with the virtual monitor.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

use super::monitors::{self, LayoutMode, LogicalConfig, LogicalMonitor, MonitorInfo};

type MonitorSpec = (String, String, String, String);
type Mode = (String, i32, i32, f64, f64, Vec<f64>, HashMap<String, OwnedValue>);
type Monitor = (MonitorSpec, Vec<Mode>, HashMap<String, OwnedValue>);
type Logical = (i32, i32, f64, u32, bool, Vec<MonitorSpec>, HashMap<String, OwnedValue>);
type State = (u32, Vec<Monitor>, Vec<Logical>, HashMap<String, OwnedValue>);
type LogicalEntry<'a> = (i32, i32, f64, u32, bool, Vec<(String, String, HashMap<&'a str, Value<'a>>)>);

#[zbus::proxy(
    interface = "org.gnome.Mutter.DisplayConfig",
    default_service = "org.gnome.Mutter.DisplayConfig",
    default_path = "/org/gnome/Mutter/DisplayConfig"
)]
trait DisplayConfig {
    fn get_current_state(&self) -> zbus::Result<State>;
    fn apply_monitors_config(
        &self,
        serial: u32,
        method: u32,
        logical_monitors: Vec<LogicalEntry<'_>>,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
}

/// `ApplyMonitorsConfig` method: apply without saving or asking.
const METHOD_TEMPORARY: u32 = 1;
struct Layout {
    serial: u32,
    monitors: Vec<MonitorInfo>,
    logical: Vec<LogicalMonitor>,
    mode: Option<LayoutMode>,
}

async fn layout(proxy: &DisplayConfigProxy<'_>) -> zbus::Result<Layout> {
    let (serial, monitors, logical, properties) = proxy.get_current_state().await?;
    let monitors = monitors
        .into_iter()
        .map(|(spec, modes, _)| {
            let current =
                modes.iter().find(|m| m.6.get("is-current").and_then(|v| bool::try_from(v).ok()).unwrap_or(false));
            MonitorInfo {
                connector: spec.0,
                current_mode: current.map(|m| (m.0.clone(), m.1, m.2)),
                scales: current.map(|m| m.5.clone()).unwrap_or_default(),
            }
        })
        .collect();
    let logical = logical
        .into_iter()
        .map(|(x, y, scale, transform, primary, specs, _)| LogicalMonitor {
            x,
            y,
            scale,
            transform,
            primary,
            connectors: specs.into_iter().map(|s| s.0).collect(),
        })
        .collect();
    let mode = LayoutMode::from_dbus(properties.get("layout-mode").and_then(|v| u32::try_from(v).ok()));
    Ok(Layout { serial, monitors, logical, mode })
}

/// The connectors present now (to recognise the monitor a stream adds).
pub async fn connectors(connection: &zbus::Connection) -> Vec<String> {
    let Ok(proxy) = DisplayConfigProxy::new(connection).await else { return Vec::new() };
    layout(&proxy).await.map(|l| l.monitors.into_iter().map(|m| m.connector).collect()).unwrap_or_default()
}

/// One line per monitor for `--diagnose`: connector, mode, scale, primary.
pub async fn describe(connection: &zbus::Connection) -> Result<Vec<String>, String> {
    let proxy = DisplayConfigProxy::new(connection).await.map_err(|e| e.to_string())?;
    let layout = layout(&proxy).await.map_err(|e| format!("DisplayConfig: {e}"))?;
    Ok(layout
        .monitors
        .iter()
        .map(|m| {
            let logical = layout.logical.iter().find(|l| l.connectors.contains(&m.connector));
            let mode = m.current_mode.as_ref().map_or("off".to_owned(), |(_, w, h)| format!("{w}x{h}"));
            let scale = logical.map(|l| format!(" at {:.0} %", l.scale * 100.0)).unwrap_or_default();
            let primary = if logical.is_some_and(|l| l.primary) { ", primary" } else { "" };
            format!("{} {mode}{scale}{primary}", m.connector)
        })
        .collect())
}

/// The existing monitor `name` (a connector or `primary`) designates.
pub async fn find_monitor(connection: &zbus::Connection, name: &str) -> Result<monitors::ChosenMonitor, String> {
    let proxy = DisplayConfigProxy::new(connection).await.map_err(|e| e.to_string())?;
    let layout = layout(&proxy).await.map_err(|e| format!("DisplayConfig: {e}"))?;
    monitors::find_monitor(&layout.logical, &layout.monitors, name)
}

/// The monitor Sidecast added: its connector and its place in the desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMonitor {
    pub connector: String,
    pub position: (i32, i32),
}

/// Sets the scale of the monitor that appeared since `before` at `size` pixels.
pub async fn scale_new_monitor(
    connection: &zbus::Connection,
    before: &[String],
    size: (u32, u32),
    scale: f64,
) -> Result<NewMonitor, String> {
    let proxy = DisplayConfigProxy::new(connection).await.map_err(|e| e.to_string())?;
    let layout = layout(&proxy).await.map_err(|e| format!("DisplayConfig: {e}"))?;
    let mode = layout.mode.ok_or("DisplayConfig reports no layout mode")?;
    let monitor = monitors::find_new_monitor(before, &layout.monitors, size)
        .ok_or("the virtual monitor is not in DisplayConfig")?
        .connector
        .clone();
    let configs = monitors::with_scale(&layout.logical, &layout.monitors, mode, &monitor, scale)?;
    let position = configs
        .iter()
        .find(|c| c.monitors.iter().any(|(connector, _)| *connector == monitor))
        .map_or((0, 0), |c| (c.x, c.y));
    let entries: Vec<LogicalEntry<'_>> = configs
        .into_iter()
        .map(|LogicalConfig { x, y, scale, transform, primary, monitors }| {
            let monitors = monitors.into_iter().map(|(c, mode)| (c, mode, HashMap::new())).collect();
            (x, y, scale, transform, primary, monitors)
        })
        .collect();
    proxy
        .apply_monitors_config(layout.serial, METHOD_TEMPORARY, entries, HashMap::new())
        .await
        .map_err(|e| format!("ApplyMonitorsConfig: {e}"))?;
    Ok(NewMonitor { connector: monitor, position })
}
