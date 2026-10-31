//! The display layout from Mutter's `org.gnome.Mutter.DisplayConfig`, as
//! plain data, and the change that gives Sidecast's virtual monitor a scale
//! (200 % for a HiDPI stream, like the Mac's 2× virtual display) while every
//! other monitor keeps its mode, scale and place. Pure, so it is unit-tested
//! on every OS; `display_config` talks to Mutter.

/// A physical (or virtual) monitor and its current mode.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub connector: String,
    /// Mode id, width and height in pixels.
    pub current_mode: Option<(String, i32, i32)>,
    /// Scales Mutter accepts for the current mode.
    pub scales: Vec<f64>,
}

/// A logical monitor: where monitors sit in the desktop, in logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalMonitor {
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: u32,
    pub primary: bool,
    pub connectors: Vec<String>,
}

/// One entry of `ApplyMonitorsConfig`: a logical monitor and (connector, mode id) pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicalConfig {
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: u32,
    pub primary: bool,
    pub monitors: Vec<(String, String)>,
}

/// The monitor Sidecast just added: one that was not there before, at the stream's size.
pub fn find_new_monitor<'a>(
    before: &[String],
    monitors: &'a [MonitorInfo],
    size: (u32, u32),
) -> Option<&'a MonitorInfo> {
    monitors.iter().find(|m| {
        !before.contains(&m.connector)
            && m.current_mode.as_ref().is_some_and(|(_, w, h)| (*w as u32, *h as u32) == size)
    })
}

/// An existing monitor to mirror (`--monitor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenMonitor {
    pub connector: String,
    /// Its current mode, in pixels.
    pub size: (u32, u32),
    /// Its place in the desktop.
    pub position: (i32, i32),
}

/// The monitor `name` designates: a connector (`DP-1`, case-insensitive) or `primary`.
pub fn find_monitor(logical: &[LogicalMonitor], monitors: &[MonitorInfo], name: &str) -> Result<ChosenMonitor, String> {
    let name = name.trim();
    let connector = if name.eq_ignore_ascii_case("primary") {
        logical.iter().find(|l| l.primary).and_then(|l| l.connectors.first()).ok_or("no primary monitor")?.clone()
    } else {
        monitors.iter().find(|m| m.connector.eq_ignore_ascii_case(name)).map(|m| m.connector.clone()).ok_or_else(
            || {
                let known: Vec<&str> = monitors.iter().map(|m| m.connector.as_str()).collect();
                format!("no monitor {name} (connected: {}, or `primary`)", known.join(", "))
            },
        )?
    };
    let monitor = monitors.iter().find(|m| m.connector == connector).ok_or("the monitor is not in the layout")?;
    let (_, w, h) = monitor.current_mode.as_ref().ok_or_else(|| format!("{connector} is off"))?;
    let position = logical.iter().find(|l| l.connectors.contains(&connector)).map_or((0, 0), |l| (l.x, l.y));
    Ok(ChosenMonitor { connector, size: (*w as u32, *h as u32), position })
}

fn logical_width(monitors: &[MonitorInfo], logical: &LogicalMonitor, scale: f64) -> i32 {
    let pixels = logical
        .connectors
        .iter()
        .filter_map(|c| monitors.iter().find(|m| &m.connector == c)?.current_mode.as_ref().map(|(_, w, h)| (*w, *h)))
        .map(|(w, h)| if logical.transform % 2 == 1 { h } else { w })
        .max()
        .unwrap_or(0);
    (pixels as f64 / scale).round() as i32
}

/// How Mutter places monitors (`layout-mode` of `GetCurrentState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Positions in logical pixels: a monitor's width there shrinks with its scale.
    Logical,
    /// Positions in physical pixels (Mutter without its `scale-monitor-framebuffer`
    /// feature). There Mutter 50 reads a scaled virtual stream's input positions
    /// in other units than its frames (seen on a headless Mutter 50: a tap at
    /// pixel (1320, 540) of a 200 % monitor put the pointer in its corner), so
    /// the scale is left alone.
    Physical,
}

impl LayoutMode {
    /// `1` physical, `2` logical (`org.gnome.Mutter.DisplayConfig`).
    pub fn from_dbus(raw: Option<u32>) -> Option<LayoutMode> {
        match raw {
            Some(1) => Some(LayoutMode::Physical),
            Some(2) => Some(LayoutMode::Logical),
            _ => None,
        }
    }
}

/// The current layout with `connector` at `scale` (logical layout mode only).
/// Monitors to its right move by the change in its width, so the layout stays
/// connected.
pub fn with_scale(
    logical: &[LogicalMonitor],
    monitors: &[MonitorInfo],
    mode: LayoutMode,
    connector: &str,
    scale: f64,
) -> Result<Vec<LogicalConfig>, String> {
    if mode == LayoutMode::Physical {
        return Err("GNOME uses its physical layout mode (fractional scaling off), where a scaled virtual \
                    monitor's input does not line up; staying at 100 %"
            .into());
    }
    let monitor = monitors.iter().find(|m| m.connector == connector).ok_or("the monitor is not in the layout")?;
    if !monitor.scales.iter().any(|s| (s - scale).abs() < 1e-6) {
        return Err(format!("Mutter does not offer {:.0} % for {connector} ({:?})", scale * 100.0, monitor.scales));
    }
    let target = logical.iter().find(|l| l.connectors.iter().any(|c| c == connector)).ok_or("no logical monitor")?;
    let old_width = logical_width(monitors, target, target.scale);
    let delta = logical_width(monitors, target, scale) - old_width;
    let right_edge = target.x + old_width;
    let mut configs = Vec::new();
    for l in logical {
        let mut entry_monitors = Vec::new();
        for c in &l.connectors {
            let mode = monitors
                .iter()
                .find(|m| &m.connector == c)
                .and_then(|m| m.current_mode.as_ref())
                .ok_or_else(|| format!("no current mode for {c}"))?;
            entry_monitors.push((c.clone(), mode.0.clone()));
        }
        let is_target = std::ptr::eq(l, target);
        configs.push(LogicalConfig {
            x: if !is_target && l.x >= right_edge { l.x + delta } else { l.x },
            y: l.y,
            scale: if is_target { scale } else { l.scale },
            transform: l.transform,
            primary: l.primary,
            monitors: entry_monitors,
        });
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(connector: &str, w: i32, h: i32, scales: &[f64]) -> MonitorInfo {
        MonitorInfo {
            connector: connector.into(),
            current_mode: Some((format!("{w}x{h}@60.000"), w, h)),
            scales: scales.to_vec(),
        }
    }

    fn logical(x: i32, scale: f64, primary: bool, connector: &str) -> LogicalMonitor {
        LogicalMonitor { x, y: 0, scale, transform: 0, primary, connectors: vec![connector.into()] }
    }

    #[test]
    fn finds_the_monitor_sidecast_added() {
        let monitors = [monitor("eDP-1", 2400, 1080, &[1.0]), monitor("Meta-1", 2400, 1080, &[1.0, 2.0])];
        let before = ["eDP-1".to_owned()];
        assert_eq!(find_new_monitor(&before, &monitors, (2400, 1080)).unwrap().connector, "Meta-1");
        assert!(find_new_monitor(&before, &monitors, (1920, 1080)).is_none(), "wrong size");
        assert!(find_new_monitor(&["eDP-1".into(), "Meta-1".into()], &monitors, (2400, 1080)).is_none());
    }

    #[test]
    fn scales_the_virtual_monitor_and_keeps_the_rest() {
        // What headless Mutter reports with a Sidecast monitor to the right.
        let monitors = [monitor("Meta-0", 1280, 720, &[1.0]), monitor("Meta-1", 2400, 1080, &[1.0, 2.0])];
        let layout = [logical(0, 1.0, true, "Meta-0"), logical(1280, 1.0, false, "Meta-1")];
        let configs = with_scale(&layout, &monitors, LayoutMode::Logical, "Meta-1", 2.0).unwrap();
        assert_eq!(
            configs,
            [
                LogicalConfig {
                    x: 0,
                    y: 0,
                    scale: 1.0,
                    transform: 0,
                    primary: true,
                    monitors: vec![("Meta-0".into(), "1280x720@60.000".into())]
                },
                LogicalConfig {
                    x: 1280,
                    y: 0,
                    scale: 2.0,
                    transform: 0,
                    primary: false,
                    monitors: vec![("Meta-1".into(), "2400x1080@60.000".into())]
                },
            ]
        );
    }

    #[test]
    fn monitors_to_the_right_follow_the_narrower_virtual_monitor() {
        let monitors = [
            monitor("DP-1", 1920, 1080, &[1.0]),
            monitor("Meta-1", 2400, 1080, &[1.0, 2.0]),
            monitor("HDMI-1", 1920, 1080, &[1.0]),
        ];
        let layout =
            [logical(0, 1.0, true, "DP-1"), logical(1920, 1.0, false, "Meta-1"), logical(4320, 1.0, false, "HDMI-1")];
        let configs = with_scale(&layout, &monitors, LayoutMode::Logical, "Meta-1", 2.0).unwrap();
        assert_eq!(configs.iter().map(|c| (c.x, c.scale)).collect::<Vec<_>>(), [(0, 1.0), (1920, 2.0), (3120, 1.0)]);
        // Physical layout (fractional scaling off): left at 100 %, input would not line up.
        assert!(with_scale(&layout, &monitors, LayoutMode::Physical, "Meta-1", 2.0).is_err());
        assert_eq!(LayoutMode::from_dbus(Some(1)), Some(LayoutMode::Physical));
        assert_eq!(LayoutMode::from_dbus(Some(2)), Some(LayoutMode::Logical));
        assert_eq!(LayoutMode::from_dbus(None), None);
    }

    #[test]
    fn finds_the_monitor_to_mirror_by_connector_or_primary() {
        let monitors = [monitor("eDP-1", 2560, 1600, &[1.0, 2.0]), monitor("DP-1", 1920, 1080, &[1.0])];
        let layout = [logical(0, 2.0, false, "DP-1"), logical(1920, 2.0, true, "eDP-1")];
        let chosen = |name| find_monitor(&layout, &monitors, name);
        assert_eq!(
            chosen("dp-1").unwrap(),
            ChosenMonitor { connector: "DP-1".into(), size: (1920, 1080), position: (0, 0) }
        );
        assert_eq!(
            chosen("primary").unwrap(),
            ChosenMonitor { connector: "eDP-1".into(), size: (2560, 1600), position: (1920, 0) }
        );
        assert_eq!(chosen("HDMI-2").unwrap_err(), "no monitor HDMI-2 (connected: eDP-1, DP-1, or `primary`)");
    }

    #[test]
    fn refuses_scales_mutter_does_not_offer() {
        let monitors = [monitor("Meta-1", 1280, 720, &[1.0])];
        let layout = [logical(0, 1.0, true, "Meta-1")];
        assert!(with_scale(&layout, &monitors, LayoutMode::Logical, "Meta-1", 2.0).is_err());
        assert!(with_scale(&layout, &monitors, LayoutMode::Logical, "Meta-9", 1.0).is_err());
    }
}
