//! How the Virtual Display Driver (github.com/VirtualDrivers/Virtual-Display-Driver,
//! MIT, Microsoft-signed releases) is configured. Pure, so it is tested on
//! every OS; `vdd.rs` does the file, registry and pipe I/O.
//!
//! - `vdd_settings.xml`, in `C:\VirtualDisplayDriver` (or the folder named by
//!   `VDDPATH` under `HKLM\SOFTWARE\MikeTheTech\VirtualDisplayDriver`), lists
//!   the number of virtual monitors and the resolutions they offer; each
//!   `<resolution>` is offered at its own `<refresh_rate>` and at every
//!   `<g_refresh_rate>`. The driver reads it when its adapter starts.
//! - The named pipe `\\.\pipe\MTTVirtualDisplayPipe` (open to every user)
//!   takes one UTF-16 command per connection: `RELOAD_DRIVER` restarts the
//!   adapter with the settings re-read, `SETDISPLAYCOUNT n` sets the monitor
//!   count (it treats 0 as 1, so monitors are removed by detaching them from
//!   the desktop instead).

pub const PIPE_NAME: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
pub const REGISTRY_KEY: &str = r"SOFTWARE\MikeTheTech\VirtualDisplayDriver";
pub const REGISTRY_PATH_VALUE: &str = "VDDPATH";
pub const DEFAULT_FOLDER: &str = r"C:\VirtualDisplayDriver";
pub const SETTINGS_FILE: &str = "vdd_settings.xml";
pub const RELEASES_URL: &str = "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases";

/// The pipe commands Sidecast sends.
pub const RELOAD_DRIVER: &str = "RELOAD_DRIVER";

pub fn set_display_count(count: u32) -> String {
    format!("SETDISPLAYCOUNT {count}")
}

/// Whether a display device belongs to the driver, from what GDI reports:
/// the adapter's description ("Virtual Display Driver") and hardware id
/// (`Root\MttVDD`), or the monitor name its EDID carries ("VDD by MTT").
pub fn is_vdd_device(adapter: &str, adapter_id: &str, monitor: &str, monitor_id: &str) -> bool {
    let has = |text: &str, needle: &str| text.to_ascii_lowercase().contains(needle);
    has(adapter, "virtual display driver")
        || has(adapter_id, "mttvdd")
        || has(monitor, "vdd by mtt")
        || has(monitor_id, "mttvdd")
}

/// A command as the driver reads it from its message-mode pipe: UTF-16LE,
/// no terminator.
pub fn command_bytes(command: &str) -> Vec<u8> {
    command.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// A display mode in pixels and hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

impl Mode {
    pub const fn new(width: u32, height: u32, refresh: u32) -> Self {
        Mode { width, height, refresh }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.refresh)
    }
}

/// What `vdd_settings.xml` says that matters to Sidecast.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VddSettings {
    pub monitor_count: Option<u32>,
    /// `<resolution>` entries (width, height, own refresh rate).
    pub modes: Vec<Mode>,
    pub global_refresh_rates: Vec<u32>,
}

impl VddSettings {
    /// Reads the parts of the file Sidecast uses; anything unexpected is skipped.
    pub fn parse(xml: &str) -> VddSettings {
        let xml = strip_comments(xml);
        let number = |text: &str| text.trim().parse::<f64>().ok().filter(|v| *v > 0.0).map(|v| v.round() as u32);
        let mut settings = VddSettings {
            monitor_count: element_text(&xml, "monitors").and_then(|m| element_text(m, "count")).and_then(number),
            ..VddSettings::default()
        };
        if let Some(global) = element_text(&xml, "global") {
            settings.global_refresh_rates = elements(global, "g_refresh_rate").filter_map(number).collect();
        }
        if let Some(list) = element_text(&xml, "resolutions") {
            for entry in elements(list, "resolution") {
                let field = |name| element_text(entry, name).and_then(number);
                if let (Some(width), Some(height)) = (field("width"), field("height")) {
                    // The driver reads a missing rate as 30 Hz.
                    settings.modes.push(Mode::new(width, height, field("refresh_rate").unwrap_or(30)));
                }
            }
        }
        settings
    }

    /// Whether the driver's monitors offer `mode`.
    pub fn offers(&self, mode: Mode) -> bool {
        self.modes.iter().any(|m| {
            (m.width, m.height) == (mode.width, mode.height)
                && (m.refresh == mode.refresh || self.global_refresh_rates.contains(&mode.refresh))
        })
    }
}

/// The offered mode closest to `wanted` when it is not offered itself: the
/// same size at the nearest refresh rate, else the closest aspect ratio (the
/// picture is letterboxed the least), preferring the largest size that does
/// not exceed `wanted`, then the nearest refresh rate.
pub fn best_mode(offered: &[Mode], wanted: Mode) -> Option<Mode> {
    let aspect = |m: &Mode| m.width as f64 / m.height.max(1) as f64;
    let wanted_aspect = aspect(&wanted);
    offered.iter().copied().filter(|m| m.width > 0 && m.height > 0).min_by_key(|m| {
        let same_size = (m.width, m.height) == (wanted.width, wanted.height);
        let aspect_gap = ((aspect(m) / wanted_aspect).ln().abs() * 100.0).round() as i64;
        let exceeds = m.width > wanted.width || m.height > wanted.height;
        let area = m.width as i64 * m.height as i64;
        let refresh_gap = (m.refresh as i64 - wanted.refresh as i64).abs();
        (!same_size, aspect_gap, exceeds, if exceeds { area } else { -area }, refresh_gap)
    })
}

/// `xml` with a `<resolution>` for `mode` added at the end of `<resolutions>`
/// (a `<resolutions>` element is created when there is none), indented like
/// the entries around it. The rest of the file is kept byte for byte.
pub fn add_mode(xml: &str, mode: Mode) -> Result<String, String> {
    let entry = |indent: &str, inner: &str| {
        format!(
            "{indent}<resolution>\n{inner}<width>{}</width>\n{inner}<height>{}</height>\n{inner}<refresh_rate>{}</refresh_rate>\n{indent}</resolution>\n",
            mode.width, mode.height, mode.refresh
        )
    };
    if let Some(close) = find_outside_comments(xml, "</resolutions>") {
        // Indent like the closing tag's line, one level deeper.
        let line_start = xml[..close].rfind('\n').map_or(0, |i| i + 1);
        let outer = &xml[line_start..close];
        let outer = if outer.trim().is_empty() { outer } else { "    " };
        let indent = format!("{outer}    ");
        let inner = format!("{indent}    ");
        let insert_at = if xml[line_start..close].trim().is_empty() { line_start } else { close };
        let mut out = String::with_capacity(xml.len() + 160);
        out.push_str(&xml[..insert_at]);
        if insert_at == close {
            out.push('\n');
        }
        out.push_str(&entry(&indent, &inner));
        out.push_str(&xml[insert_at..]);
        return Ok(out);
    }
    let root_close =
        find_outside_comments(xml, "</vdd_settings>").ok_or("vdd_settings.xml has no </vdd_settings> element")?;
    let block = format!("    <resolutions>\n{}    </resolutions>\n", entry("        ", "            "));
    Ok(format!("{}{block}{}", &xml[..root_close], &xml[root_close..]))
}

/// Removes `<!-- … -->` comments (an unterminated one runs to the end).
fn strip_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = rest[start + 4..].find("-->").map_or("", |end| &rest[start + 4 + end + 3..]);
    }
    out.push_str(rest);
    out
}

/// The byte offset of `needle` in `xml`, skipping commented-out text.
fn find_outside_comments(xml: &str, needle: &str) -> Option<usize> {
    let mut offset = 0;
    let mut rest = xml;
    loop {
        let comment = rest.find("<!--");
        let found = rest.find(needle);
        match (found, comment) {
            (Some(f), Some(c)) if c < f => {
                let end = rest[c + 4..].find("-->")? + c + 4 + 3;
                offset += end;
                rest = &rest[end..];
            }
            (Some(f), _) => return Some(offset + f),
            (None, _) => return None,
        }
    }
}

/// The text inside the first `<name>…</name>` (comments already stripped).
fn element_text<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    elements(xml, name).next()
}

/// The inner text of every top-level `<name>…</name>` in order.
fn elements<'a>(xml: &'a str, name: &str) -> impl Iterator<Item = &'a str> + 'a {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut rest = xml;
    std::iter::from_fn(move || {
        let start = rest.find(&open)? + open.len();
        let end = rest[start..].find(&close)? + start;
        let inner = &rest[start..end];
        rest = &rest[end + close.len()..];
        Some(inner)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The relevant parts of the vdd_settings.xml the driver ships (25.7).
    const SHIPPED: &str = "<?xml version='1.0' encoding='utf-8'?>
<!-- Virtual Display Driver (HDR) - Minimal Configuration File -->
<vdd_settings>
    <monitors>
        <count>1</count>
    </monitors>
    <gpu>
        <friendlyname>default</friendlyname>
    </gpu>
    <global>
        <!-- Global refresh rates - applied to all resolutions -->
        <g_refresh_rate>60</g_refresh_rate>
        <g_refresh_rate>90</g_refresh_rate>
        <g_refresh_rate>120</g_refresh_rate>
    </global>
    <resolutions>
        <resolution>
            <width>1920</width>
            <height>1080</height>
            <refresh_rate>60</refresh_rate>
        </resolution>
        <!-- <resolution><width>800</width><height>600</height><refresh_rate>60</refresh_rate></resolution> -->
        <resolution>
            <width>2560</width>
            <height>1440</height>
            <refresh_rate>60</refresh_rate>
        </resolution>
    </resolutions>
    <auto_resolutions>
        <preferred_mode>
            <fallback_width>1920</fallback_width>
            <fallback_height>1080</fallback_height>
        </preferred_mode>
    </auto_resolutions>
</vdd_settings>
";

    #[test]
    fn recognizes_the_drivers_devices() {
        assert!(is_vdd_device("Virtual Display Driver", r"Root\MttVDD", "Generic PnP Monitor", ""));
        assert!(is_vdd_device("", "", "VDD by MTT", ""));
        assert!(is_vdd_device("", "", "", r"MONITOR\MttVDD\0001"));
        assert!(!is_vdd_device("NVIDIA GeForce RTX 4070", r"PCI\VEN_10DE", "DELL U2720Q", r"MONITOR\DELA1A2"));
        assert!(!is_vdd_device("Microsoft Basic Display Adapter", "", "Generic PnP Monitor", ""));
    }

    #[test]
    fn pipe_commands_are_utf16le() {
        assert_eq!(command_bytes("PING"), [b'P', 0, b'I', 0, b'N', 0, b'G', 0]);
        assert_eq!(set_display_count(1), "SETDISPLAYCOUNT 1");
        assert_eq!(command_bytes(RELOAD_DRIVER).len(), 2 * RELOAD_DRIVER.len());
    }

    #[test]
    fn reads_the_shipped_settings() {
        let s = VddSettings::parse(SHIPPED);
        assert_eq!(s.monitor_count, Some(1));
        assert_eq!(
            s.modes,
            [Mode::new(1920, 1080, 60), Mode::new(2560, 1440, 60)],
            "the commented-out mode is ignored"
        );
        assert_eq!(s.global_refresh_rates, [60, 90, 120]);
        assert!(s.offers(Mode::new(1920, 1080, 60)));
        assert!(s.offers(Mode::new(2560, 1440, 120)), "global rates apply to every resolution");
        assert!(!s.offers(Mode::new(2560, 1440, 144)));
        assert!(!s.offers(Mode::new(2400, 1080, 60)));
        assert_eq!(VddSettings::parse("garbage"), VddSettings::default());
    }

    #[test]
    fn adds_a_phone_mode_and_keeps_everything_else() {
        let phone = Mode::new(2400, 1080, 120);
        let edited = add_mode(SHIPPED, phone).unwrap();
        let s = VddSettings::parse(&edited);
        assert!(s.offers(phone));
        assert_eq!(s.modes.len(), 3);
        assert_eq!(s.modes[2], phone);
        // Only lines were added; the original text survives in order.
        let mut original = SHIPPED.lines();
        let mut next = original.next();
        for line in edited.lines() {
            if Some(line) == next {
                next = original.next();
            }
        }
        assert_eq!(next, None, "every original line is still there");
        assert!(edited.contains(
            "        <resolution>\n            <width>2400</width>\n            <height>1080</height>\n            <refresh_rate>120</refresh_rate>\n        </resolution>\n    </resolutions>"
        ), "{edited}");
        // Idempotent use: the caller checks `offers` first; adding twice lists it twice but parses fine.
        assert_eq!(VddSettings::parse(&add_mode(&edited, phone).unwrap()).modes.len(), 4);
    }

    #[test]
    fn adds_a_resolutions_element_when_missing() {
        let xml = "<vdd_settings>\n    <monitors><count>2</count></monitors>\n</vdd_settings>\n";
        let edited = add_mode(xml, Mode::new(1280, 720, 60)).unwrap();
        let s = VddSettings::parse(&edited);
        assert_eq!((s.monitor_count, s.modes.as_slice()), (Some(2), &[Mode::new(1280, 720, 60)][..]));
        assert!(add_mode("<other/>", Mode::new(1, 1, 1)).is_err());
        // A closing tag that only appears inside a comment does not count.
        assert!(
            add_mode("<!-- </resolutions> --><vdd_settings></vdd_settings>", Mode::new(8, 8, 60))
                .unwrap()
                .contains("<resolutions>\n")
        );
    }

    #[test]
    fn picks_the_closest_offered_mode() {
        let offered = [
            Mode::new(3840, 2160, 60),
            Mode::new(2560, 1440, 60),
            Mode::new(2400, 1080, 60),
            Mode::new(2400, 1080, 120),
            Mode::new(1920, 1080, 60),
            Mode::new(1280, 576, 60),
        ];
        assert_eq!(best_mode(&offered, Mode::new(2400, 1080, 120)), Some(Mode::new(2400, 1080, 120)));
        assert_eq!(best_mode(&offered, Mode::new(2400, 1080, 90)), Some(Mode::new(2400, 1080, 60)));
        // 20:9 wanted, not offered: the largest 20:9 mode that fits.
        assert_eq!(best_mode(&offered, Mode::new(2800, 1260, 60)), Some(Mode::new(2400, 1080, 60)));
        // 16:9 wanted: the largest 16:9 that fits.
        assert_eq!(best_mode(&offered, Mode::new(3000, 1688, 60)), Some(Mode::new(2560, 1440, 60)));
        // Nothing fits: the smallest of the closest shape.
        assert_eq!(best_mode(&offered, Mode::new(1000, 450, 60)), Some(Mode::new(1280, 576, 60)));
        assert_eq!(best_mode(&[], Mode::new(1, 1, 60)), None);
    }

    #[test]
    fn a_resolutions_closing_tag_on_the_same_line() {
        let xml = "<vdd_settings><resolutions><resolution><width>1</width><height>1</height></resolution></resolutions></vdd_settings>";
        let s = VddSettings::parse(&add_mode(xml, Mode::new(2400, 1080, 60)).unwrap());
        assert_eq!(s.modes, [Mode::new(1, 1, 30), Mode::new(2400, 1080, 60)]);
    }
}
