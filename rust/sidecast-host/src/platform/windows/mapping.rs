//! Coordinates and wheel units for input injection. Pure, so it is tested on
//! every OS.
//!
//! Display points (the phone's view of the streamed display, top-left origin)
//! map onto the picture inside the encoded frame, and that picture onto the
//! monitor's rectangle in desktop pixels. The picture fills the frame when a
//! virtual monitor has the phone's aspect ratio; an existing monitor of
//! another shape is letterboxed, centred (see [`fit`]). The host is
//! per-monitor DPI aware, so desktop coordinates are physical pixels.

use sidecast_core::input::ScrollPhase;
use sidecast_core::{Point, Size};

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Rect { left, top, width, height }
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right() && y >= self.top && y < self.bottom()
    }
}

/// Where a `source`-sized picture sits inside a `frame`, scaled to fit with
/// its aspect ratio kept and centred: `(x, y, width, height)` in frame units.
pub fn fit(source: (f64, f64), frame: (f64, f64)) -> (f64, f64, f64, f64) {
    let (sw, sh) = source;
    let (fw, fh) = frame;
    if sw <= 0.0 || sh <= 0.0 || fw <= 0.0 || fh <= 0.0 {
        return (0.0, 0.0, fw.max(0.0), fh.max(0.0));
    }
    let scale = (fw / sw).min(fh / sh);
    let (w, h) = (sw * scale, sh * scale);
    ((fw - w) / 2.0, (fh - h) / 2.0, w, h)
}

/// Display points ↔ desktop pixels of the streamed monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayMapping {
    monitor: Rect,
    /// The picture of the monitor inside the display, in points.
    content: (f64, f64, f64, f64),
}

impl DisplayMapping {
    /// `size`: the display in points (the server's `DisplayGeometry::size`).
    pub fn new(monitor: Rect, size: Size) -> Self {
        let content = fit((monitor.width as f64, monitor.height as f64), (size.width, size.height));
        DisplayMapping { monitor, content }
    }

    pub fn monitor(&self) -> Rect {
        self.monitor
    }

    /// The desktop pixel for a display point, clamped onto the monitor (a
    /// touch on a letterbox border goes to the nearest edge).
    pub fn to_desktop(&self, p: Point) -> (i32, i32) {
        let (cx, cy, cw, ch) = self.content;
        let u = if cw > 0.0 { (p.x - cx) / cw } else { 0.0 };
        let v = if ch > 0.0 { (p.y - cy) / ch } else { 0.0 };
        let m = self.monitor;
        let x = (u * m.width as f64).floor().clamp(0.0, (m.width - 1).max(0) as f64) as i32;
        let y = (v * m.height as f64).floor().clamp(0.0, (m.height - 1).max(0) as f64) as i32;
        (m.left + x, m.top + y)
    }

    /// The display point of a desktop pixel (its centre), or `None` when the
    /// pixel is not on this monitor.
    pub fn to_display(&self, x: i32, y: i32) -> Option<Point> {
        let m = self.monitor;
        if m.is_empty() || !m.contains(x, y) {
            return None;
        }
        let (cx, cy, cw, ch) = self.content;
        let u = ((x - m.left) as f64 + 0.5) / m.width as f64;
        let v = ((y - m.top) as f64 + 0.5) / m.height as f64;
        Some(Point::new(cx + u * cw, cy + v * ch))
    }
}

/// `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` coordinates for a desktop
/// pixel: 0…65535 across the virtual screen. Windows maps a normalized `n`
/// back to `left + floor(n × width / 65536)`, so rounding up lands exactly
/// on the pixel (the usual `x × 65535 / width` is off by one on wide desktops).
pub fn absolute_coordinates(x: i32, y: i32, virtual_screen: Rect) -> (i32, i32) {
    fn axis(value: i32, origin: i32, extent: i32) -> i32 {
        if extent <= 0 {
            return 0;
        }
        let offset = (value - origin).clamp(0, extent - 1) as i64;
        ((offset * 65_536 + extent as i64 - 1) / extent as i64).min(65_535) as i32
    }
    (axis(x, virtual_screen.left, virtual_screen.width), axis(y, virtual_screen.top, virtual_screen.height))
}

/// One wheel notch (`WHEEL_DELTA`).
pub const WHEEL_DELTA: i32 = 120;

/// Wheel units per display point of finger travel: a notch (3 lines of
/// ~20 points) per 60 points, close to a precision touchpad.
pub const WHEEL_UNITS_PER_POINT: f64 = 2.0;

/// Pixel scrolling → wheel units, carrying fractions between events.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelAccumulator {
    remainder: (f64, f64),
}

impl WheelAccumulator {
    /// `(vertical, horizontal)` wheel deltas for a scroll of `dx, dy` points
    /// in natural direction (content follows the finger). Positive vertical
    /// wheel scrolls up, which is content moving down with the finger; positive
    /// horizontal wheel scrolls right, which is content moving left. Windows
    /// has no scroll phases: the end of a gesture only drops the remainder.
    pub fn scroll(&mut self, dx: f64, dy: f64, phase: ScrollPhase) -> (i32, i32) {
        if matches!(phase, ScrollPhase::Began | ScrollPhase::MomentumBegan) {
            self.remainder = (0.0, 0.0);
        }
        let vertical = dy * WHEEL_UNITS_PER_POINT + self.remainder.0;
        let horizontal = -dx * WHEEL_UNITS_PER_POINT + self.remainder.1;
        let (v, h) = (vertical.trunc(), horizontal.trunc());
        self.remainder = (vertical - v, horizontal - h);
        if matches!(phase, ScrollPhase::Ended | ScrollPhase::MomentumEnded) {
            self.remainder = (0.0, 0.0);
        }
        (v as i32, h as i32)
    }

    pub fn reset(&mut self) {
        self.remainder = (0.0, 0.0);
    }
}

/// Wheel units per pinch step: a pinch across half the display is a few notches.
pub const ZOOM_WHEEL_UNITS_PER_STEP: i32 = 6;

/// Pinch steps → whole Ctrl+wheel notches (many apps zoom once per notch and
/// ignore smaller deltas), carrying the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZoomAccumulator {
    units: i32,
}

impl ZoomAccumulator {
    /// The wheel delta to send with Ctrl held (a multiple of
    /// [`WHEEL_DELTA`], positive = zoom in), or 0.
    pub fn zoom(&mut self, delta: i32) -> i32 {
        self.units = self.units.saturating_add(delta.saturating_mul(ZOOM_WHEEL_UNITS_PER_STEP));
        let notches = self.units / WHEEL_DELTA;
        self.units -= notches * WHEEL_DELTA;
        notches * WHEEL_DELTA
    }

    pub fn reset(&mut self) {
        self.units = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_coordinates_land_on_every_pixel() {
        // How Windows maps a normalized coordinate back to a pixel.
        fn back(n: i32, origin: i32, extent: i32) -> i32 {
            origin + ((n as i64 * extent as i64) >> 16) as i32
        }
        for screen in [
            Rect::new(0, 0, 800, 600),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(-1920, -200, 1920 + 3840 + 2400, 2160),
            Rect::new(0, 0, 7680, 4320),
            Rect::new(0, 0, 65_535 + 1000, 10),
        ] {
            for x in (screen.left..screen.right()).step_by(1 + screen.width as usize / 5000) {
                let (nx, _) = absolute_coordinates(x, screen.top, screen);
                assert!((0..=65_535).contains(&nx));
                if screen.width <= 65_536 {
                    assert_eq!(back(nx, screen.left, screen.width), x, "x={x} on {screen:?}");
                }
            }
            for y in screen.top..screen.bottom() {
                let (_, ny) = absolute_coordinates(screen.left, y, screen);
                assert_eq!(back(ny, screen.top, screen.height), y, "y={y} on {screen:?}");
            }
        }
        // Off-screen points clamp to the edges.
        let screen = Rect::new(0, 0, 1920, 1080);
        assert_eq!(absolute_coordinates(-50, 5000, screen), (0, absolute_coordinates(0, 1079, screen).1));
        assert_eq!(absolute_coordinates(10, 10, Rect::default()), (0, 0));
    }

    #[test]
    fn a_virtual_monitor_with_the_phone_shape_maps_one_to_one() {
        // A HiDPI phone display of 1200×540 points on a 2400×1080 monitor right of the main one.
        let monitor = Rect::new(1920, 0, 2400, 1080);
        let m = DisplayMapping::new(monitor, Size::new(1200.0, 540.0));
        assert_eq!(m.to_desktop(Point::new(0.0, 0.0)), (1920, 0));
        assert_eq!(m.to_desktop(Point::new(600.0, 270.0)), (1920 + 1200, 540));
        assert_eq!(m.to_desktop(Point::new(1199.9, 539.9)), (1920 + 2399, 1079));
        // Past the edge: clamped onto the monitor.
        assert_eq!(m.to_desktop(Point::new(5000.0, -3.0)), (1920 + 2399, 0));
        // And back: the pixel's centre.
        let p = m.to_display(1920 + 1200, 540).unwrap();
        assert!((p.x - 600.25).abs() < 1e-9 && (p.y - 270.25).abs() < 1e-9);
        assert_eq!(m.to_display(1919, 0), None, "the main monitor is not ours");
        assert_eq!(m.to_display(1920 + 2400, 10), None);
    }

    #[test]
    fn a_mirrored_monitor_of_another_shape_is_letterboxed() {
        // A 16:9 1920×1080 primary streamed to a 20:9 phone (2400×1080 points, 1×).
        let m = DisplayMapping::new(Rect::new(0, 0, 1920, 1080), Size::new(2400.0, 1080.0));
        // The picture is 1920 points wide, centred: borders of 240 points.
        assert_eq!(m.to_desktop(Point::new(240.0, 0.0)), (0, 0));
        assert_eq!(m.to_desktop(Point::new(1200.0, 540.0)), (960, 540));
        // Touches on the borders go to the nearest edge.
        assert_eq!(m.to_desktop(Point::new(10.0, 540.0)), (0, 540));
        assert_eq!(m.to_desktop(Point::new(2390.0, 540.0)), (1919, 540));
        let p = m.to_display(0, 0).unwrap();
        assert!((p.x - 240.5).abs() < 1e-9);
        // A 4:3 monitor on the same phone: pillarboxed the other way round works too.
        let (x, y, w, h) = fit((1024.0, 768.0), (2400.0, 1080.0));
        assert_eq!((x, y, w, h), (480.0, 0.0, 1440.0, 1080.0));
        let (x, y, w, h) = fit((3840.0, 1080.0), (2400.0, 1080.0));
        assert!((x, w) == (0.0, 2400.0) && (y - 202.5).abs() < 1e-9 && (h - 675.0).abs() < 1e-9);
    }

    #[test]
    fn scrolling_converts_points_to_wheel_units_in_natural_direction() {
        let mut wheel = WheelAccumulator::default();
        // Finger down 30 points: content moves down = wheel forward (positive).
        assert_eq!(wheel.scroll(0.0, 30.0, ScrollPhase::Began), (60, 0));
        // Finger right 10 points: content moves right = scroll left (negative horizontal wheel).
        assert_eq!(wheel.scroll(10.0, 0.0, ScrollPhase::Changed), (0, -20));
        // Fractions carry over: quarter-point steps (half a wheel unit) add up.
        let mut total = 0;
        for _ in 0..12 {
            total += wheel.scroll(0.0, 0.25, ScrollPhase::Momentum).0;
        }
        assert_eq!(total, 6);
        // Upward finger motion scrolls down.
        assert_eq!(wheel.scroll(0.0, -12.0, ScrollPhase::Changed), (-24, 0));
        wheel.scroll(0.0, 0.4, ScrollPhase::Ended);
        assert_eq!(wheel, WheelAccumulator::default(), "the end of a gesture drops the remainder");
    }

    #[test]
    fn pinching_sends_whole_ctrl_wheel_notches() {
        let mut zoom = ZoomAccumulator::default();
        assert_eq!(zoom.zoom(10), 0, "60 units: not a notch yet");
        assert_eq!(zoom.zoom(10), WHEEL_DELTA);
        assert_eq!(zoom.zoom(-20), -WHEEL_DELTA, "pinching in zooms out");
        assert_eq!(zoom.zoom(100), 5 * WHEEL_DELTA);
        zoom.reset();
        assert_eq!(zoom, ZoomAccumulator::default());
    }
}
