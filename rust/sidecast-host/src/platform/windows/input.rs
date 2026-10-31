//! Input injection with `SendInput`: absolute pointer positions on the
//! virtual desktop (so the pointer lands on the streamed monitor wherever it
//! is), buttons, wheel and Ctrl+wheel zoom, keys with their scan codes, and
//! text as Unicode independent of the keyboard layout.
//!
//! Windows refuses injected input into windows of elevated apps (UIPI) and
//! on the secure desktop; `SendInput` then reports fewer events sent, which
//! is logged once per stream.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::time::{Duration, Instant};

use sidecast_core::input::{InputEvent, MouseButton, PointerEventKind, typing};
use sidecast_core::{Point, Size};
use sidecast_server::backend::{BackendError, DisplayGeometry, InputBackend, InputSink};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::display;
use super::keymap::{Chord, VirtualKey, semantic_chord, unicode_transitions, vk};
use super::mapping::{DisplayMapping, Rect, WheelAccumulator, ZoomAccumulator, absolute_coordinates};

/// UTF-16 units per `SendInput` batch of text (cut between graphemes).
const TEXT_BATCH: usize = 64;

/// Creates a [`SendInputSink`] per stream.
#[derive(Debug, Default)]
pub struct SendInputBackend;

impl InputBackend for SendInputBackend {
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError> {
        let device = geometry.id.clone().ok_or_else(|| {
            BackendError::new(
                sidecast_server::backend::BackendErrorKind::Other,
                "the stream did not say which monitor it shows",
            )
        })?;
        Ok(Box::new(SendInputSink::new(device, geometry.size, geometry.origin)))
    }
}

/// Injects one stream's input onto its monitor.
pub struct SendInputSink {
    device: String,
    size: Size,
    mapping: DisplayMapping,
    virtual_screen: Rect,
    refreshed: Option<Instant>,
    left_down: bool,
    right_down: bool,
    wheel: WheelAccumulator,
    zoom: ZoomAccumulator,
    blocked_reported: bool,
}

impl SendInputSink {
    /// `origin`: the monitor's top-left when the stream started (used until
    /// its current rectangle is read).
    pub fn new(device: String, size: Size, origin: Point) -> Self {
        let fallback = Rect::new(origin.x as i32, origin.y as i32, size.width as i32, size.height as i32);
        let mut sink = SendInputSink {
            device,
            size,
            mapping: DisplayMapping::new(fallback, size),
            virtual_screen: fallback,
            refreshed: None,
            left_down: false,
            right_down: false,
            wheel: WheelAccumulator::default(),
            zoom: ZoomAccumulator::default(),
            blocked_reported: false,
        };
        sink.refresh();
        sink
    }

    /// Re-reads where the monitor is (at most twice a second): the user may
    /// rearrange monitors or change the resolution while streaming.
    fn refresh(&mut self) {
        if self.refreshed.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        self.refreshed = Some(Instant::now());
        if let Some((rect, _)) = display::current_mode(&self.device) {
            self.mapping = DisplayMapping::new(rect, self.size);
        }
        let screen = display::virtual_screen();
        if !screen.is_empty() {
            self.virtual_screen = screen;
        }
    }

    fn send(&mut self, inputs: &[INPUT]) {
        if inputs.is_empty() {
            return;
        }
        // SAFETY: `inputs` is a valid slice of initialized INPUT structures.
        let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) } as usize;
        if sent < inputs.len() && !self.blocked_reported {
            self.blocked_reported = true;
            log::warn!(
                "input: Windows blocked injected input ({sent} of {} events): an app running as administrator is in front, or the secure desktop (UAC, lock screen) is showing",
                inputs.len()
            );
        }
    }

    /// An absolute move to a display point, with extra flags (buttons, wheel).
    fn pointer(&mut self, at: Point, extra: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
        let (x, y) = self.mapping.to_desktop(at);
        let (nx, ny) = absolute_coordinates(x, y, self.virtual_screen);
        mouse(nx, ny, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | extra, data)
    }

    fn press_keys(&mut self, transitions: impl IntoIterator<Item = (VirtualKey, bool)>) {
        let inputs: Vec<INPUT> = transitions.into_iter().map(|(key, down)| keyboard(key, down)).collect();
        self.send(&inputs);
    }
}

fn mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            // Wheel deltas are signed values in an unsigned field.
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn keyboard(key: VirtualKey, down: bool) -> INPUT {
    // SAFETY: MapVirtualKeyW only reads its arguments. The scan code makes
    // the key look like a real one to apps that read scan codes.
    let scan = unsafe { MapVirtualKeyW(key.code as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(key.code), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn unicode(unit: u16, down: bool) -> INPUT {
    let flags = if down { KEYEVENTF_UNICODE } else { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

impl InputSink for SendInputSink {
    fn inject(&mut self, event: &InputEvent) {
        self.refresh();
        match event {
            InputEvent::Pointer(pointer) => {
                // Clicks arrive already timed (move → 12 ms → down → 40 ms → up);
                // Windows counts double clicks itself from time and distance.
                let button = match pointer.kind {
                    PointerEventKind::Moved | PointerEventKind::Dragged => MOUSE_EVENT_FLAGS(0),
                    PointerEventKind::ButtonDown(MouseButton::Left) => {
                        self.left_down = true;
                        MOUSEEVENTF_LEFTDOWN
                    }
                    PointerEventKind::ButtonUp(MouseButton::Left) => {
                        self.left_down = false;
                        MOUSEEVENTF_LEFTUP
                    }
                    PointerEventKind::ButtonDown(MouseButton::Right) => {
                        self.right_down = true;
                        MOUSEEVENTF_RIGHTDOWN
                    }
                    PointerEventKind::ButtonUp(MouseButton::Right) => {
                        self.right_down = false;
                        MOUSEEVENTF_RIGHTUP
                    }
                };
                let input = self.pointer(pointer.at, MOUSEEVENTF_MOVE_NOCOALESCE | button, 0);
                self.send(&[input]);
            }
            InputEvent::Scroll { dx, dy, at, phase } => {
                let (vertical, horizontal) = self.wheel.scroll(*dx, *dy, *phase);
                if vertical == 0 && horizontal == 0 {
                    return;
                }
                // The wheel goes to the window under the pointer: put it under the fingers first.
                let mut inputs = vec![self.pointer(*at, MOUSE_EVENT_FLAGS(0), 0)];
                if vertical != 0 {
                    inputs.push(mouse(0, 0, MOUSEEVENTF_WHEEL, vertical));
                }
                if horizontal != 0 {
                    inputs.push(mouse(0, 0, MOUSEEVENTF_HWHEEL, horizontal));
                }
                self.send(&inputs);
            }
            InputEvent::Zoom { delta, at } => {
                let wheel = self.zoom.zoom(*delta);
                if wheel == 0 {
                    return;
                }
                let ctrl = VirtualKey { code: vk::LCONTROL, extended: false };
                let inputs = [
                    self.pointer(*at, MOUSE_EVENT_FLAGS(0), 0),
                    keyboard(ctrl, true),
                    mouse(0, 0, MOUSEEVENTF_WHEEL, wheel),
                    keyboard(ctrl, false),
                ];
                self.send(&inputs);
            }
            InputEvent::Semantic(action) => {
                log::debug!("input: {} → Windows shortcut", action.name());
                self.press_keys(semantic_chord(*action).transitions());
            }
            InputEvent::Key { key, modifiers } => self.press_keys(Chord::new(*key, *modifiers).transitions()),
            InputEvent::Text(text) => {
                for chunk in typing::chunks(text, TEXT_BATCH) {
                    let inputs: Vec<INPUT> =
                        unicode_transitions(&chunk).into_iter().map(|(unit, down)| unicode(unit, down)).collect();
                    self.send(&inputs);
                }
            }
        }
    }

    fn pointer_position(&mut self) -> Option<Point> {
        self.refresh();
        let mut p = POINT::default();
        // SAFETY: `p` is a valid out-pointer.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        self.mapping.to_display(p.x, p.y)
    }

    fn release_all(&mut self) {
        let mut inputs = Vec::new();
        if self.left_down {
            inputs.push(mouse(0, 0, MOUSEEVENTF_LEFTUP, 0));
            self.left_down = false;
        }
        if self.right_down {
            inputs.push(mouse(0, 0, MOUSEEVENTF_RIGHTUP, 0));
            self.right_down = false;
        }
        self.send(&inputs);
        self.wheel.reset();
        self.zoom.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::input::PointerEvent;

    /// Moves the real pointer, so it only runs when asked for
    /// (`SIDECAST_TEST_REAL_INPUT=1`, as CI does on its throwaway desktop).
    #[test]
    fn moves_the_real_pointer_onto_the_monitor() {
        if std::env::var_os("SIDECAST_TEST_REAL_INPUT").is_none() {
            eprintln!("skipped: set SIDECAST_TEST_REAL_INPUT=1 to let this test move the pointer");
            return;
        }
        crate::platform::windows::util::init_process();
        let Some(primary) = display::monitors().into_iter().find(|m| m.attached && m.primary) else {
            eprintln!("skipped: no attached primary monitor (no interactive desktop)");
            return;
        };
        // A phone-shaped display in points, letterboxed onto the primary monitor.
        let size = Size::new(1200.0, 540.0);
        let mut sink = SendInputSink::new(primary.device.clone(), size, Point::ZERO);
        let target = Point::new(600.0, 270.0);
        sink.inject(&InputEvent::Pointer(PointerEvent::new(PointerEventKind::Moved, target)));
        std::thread::sleep(Duration::from_millis(100));
        let expected = DisplayMapping::new(primary.rect, size).to_desktop(target);
        let mut actual = POINT::default();
        // SAFETY: a valid out-pointer.
        unsafe { GetCursorPos(&mut actual) }.expect("GetCursorPos");
        assert_eq!((actual.x, actual.y), expected, "the pointer is where the phone touched");
        let back = sink.pointer_position().expect("the pointer is on the streamed monitor");
        assert!((back.x - target.x).abs() < 2.0 && (back.y - target.y).abs() < 2.0, "{back:?}");
        sink.release_all();
    }
}
