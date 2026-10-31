//! Platform-neutral [`InputEvent`]s → the calls of a Linux remote-desktop
//! session (Mutter's `org.gnome.Mutter.RemoteDesktop.Session` or the
//! RemoteDesktop portal, which share their vocabulary). Pure, so it is
//! unit-tested on every OS; `remote_input` executes the calls.

use sidecast_core::input::{InputEvent, MouseButton, PointerEventKind, ScrollPhase};
use sidecast_core::{Point, Size};

use super::keymap::{self, TextInput, TextStep, evdev};

/// One call on the remote-desktop session. Positions are in stream pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RemoteCall {
    MotionAbsolute {
        x: f64,
        y: f64,
    },
    /// evdev button code (`BTN_LEFT`…).
    Button {
        button: i32,
        pressed: bool,
    },
    /// Smooth (finger) scroll in pixels, positive = content moves up (scroll down);
    /// `finish` = the fingers lifted, so apps may start kinetic scrolling.
    Axis {
        dx: f64,
        dy: f64,
        finish: bool,
    },
    /// Wheel clicks: axis 0 = vertical (positive = down), 1 = horizontal.
    AxisDiscrete {
        axis: u32,
        steps: i32,
    },
    /// evdev key code.
    Keycode {
        keycode: u32,
        pressed: bool,
    },
    Keysym {
        keysym: u32,
        pressed: bool,
    },
}

/// Maps display points (the server's coordinate space) to the stream's pixels
/// and tracks what is held, so `release_all` can undo it.
#[derive(Debug, Clone)]
pub struct Translator {
    display: Size,
    stream: (u32, u32),
    text: TextInput,
    last_motion: Option<(f64, f64)>,
    buttons: Vec<i32>,
    keys: Vec<u32>,
    scrolling: bool,
}

impl Translator {
    /// `display`: the streamed display in points; `stream`: its frames in pixels.
    pub fn new(display: Size, stream: (u32, u32), text: TextInput) -> Self {
        Translator { display, stream, text, last_motion: None, buttons: Vec::new(), keys: Vec::new(), scrolling: false }
    }

    fn scale(&self) -> (f64, f64) {
        if !self.display.is_usable() || self.stream.0 == 0 || self.stream.1 == 0 {
            return (1.0, 1.0);
        }
        (self.stream.0 as f64 / self.display.width, self.stream.1 as f64 / self.display.height)
    }

    /// A display point in stream pixels, clamped onto the stream.
    pub fn to_stream(&self, p: Point) -> (f64, f64) {
        let (sx, sy) = self.scale();
        let max_x = (self.stream.0.max(1) - 1) as f64;
        let max_y = (self.stream.1.max(1) - 1) as f64;
        ((p.x * sx).clamp(0.0, max_x), (p.y * sy).clamp(0.0, max_y))
    }

    /// Stream pixels in display points, or `None` when outside the stream.
    pub fn to_display(&self, x: f64, y: f64) -> Option<Point> {
        let (sx, sy) = self.scale();
        let inside = x >= 0.0 && y >= 0.0 && x < self.stream.0 as f64 && y < self.stream.1 as f64;
        inside.then(|| Point::new(x / sx, y / sy))
    }

    /// Where the last injected motion put the pointer, in display points.
    pub fn last_pointer(&self) -> Option<Point> {
        self.last_motion.and_then(|(x, y)| self.to_display(x, y))
    }

    fn motion(&mut self, at: Point, calls: &mut Vec<RemoteCall>) {
        let (x, y) = self.to_stream(at);
        if self.last_motion != Some((x, y)) {
            self.last_motion = Some((x, y));
            calls.push(RemoteCall::MotionAbsolute { x, y });
        }
    }

    fn key(&mut self, keycode: u32, pressed: bool, calls: &mut Vec<RemoteCall>) {
        if pressed {
            self.keys.push(keycode);
        } else if let Some(i) = self.keys.iter().rposition(|k| *k == keycode) {
            self.keys.remove(i);
        }
        calls.push(RemoteCall::Keycode { keycode, pressed });
    }

    /// Modifiers down, key down and up, modifiers up in reverse order.
    fn chord(&mut self, modifiers: &[u32], key: u32, calls: &mut Vec<RemoteCall>) {
        for m in modifiers {
            self.key(*m, true, calls);
        }
        self.key(key, true, calls);
        self.key(key, false, calls);
        for m in modifiers.iter().rev() {
            self.key(*m, false, calls);
        }
    }

    fn tap_keysym(keysym: u32, calls: &mut Vec<RemoteCall>) {
        calls.push(RemoteCall::Keysym { keysym, pressed: true });
        calls.push(RemoteCall::Keysym { keysym, pressed: false });
    }

    pub fn translate(&mut self, event: &InputEvent) -> Vec<RemoteCall> {
        let mut calls = Vec::new();
        match event {
            InputEvent::Pointer(pointer) => {
                self.motion(pointer.at, &mut calls);
                let (button, pressed) = match pointer.kind {
                    PointerEventKind::Moved | PointerEventKind::Dragged => return calls,
                    PointerEventKind::ButtonDown(b) => (b, true),
                    PointerEventKind::ButtonUp(b) => (b, false),
                };
                let button = match button {
                    MouseButton::Left => evdev::BTN_LEFT,
                    MouseButton::Right => evdev::BTN_RIGHT,
                };
                let held = self.buttons.contains(&button);
                if pressed && !held {
                    self.buttons.push(button);
                    calls.push(RemoteCall::Button { button, pressed });
                } else if !pressed && held {
                    self.buttons.retain(|b| *b != button);
                    calls.push(RemoteCall::Button { button, pressed });
                }
            }
            InputEvent::Scroll { dx, dy, at, phase } => match phase {
                // Like a real touchpad: finger scrolling, then a "finish" that lets
                // apps (GTK, Firefox, Chromium) run their own kinetic scrolling, so
                // the engine's synthetic momentum is not replayed on top of it.
                ScrollPhase::Began | ScrollPhase::Changed => {
                    if *phase == ScrollPhase::Began || !self.scrolling {
                        self.motion(*at, &mut calls);
                    }
                    self.scrolling = true;
                    if *dx != 0.0 || *dy != 0.0 {
                        calls.push(RemoteCall::Axis { dx: -dx, dy: -dy, finish: false });
                    }
                }
                ScrollPhase::Ended => {
                    if std::mem::take(&mut self.scrolling) {
                        calls.push(RemoteCall::Axis { dx: 0.0, dy: 0.0, finish: true });
                    }
                }
                ScrollPhase::MomentumBegan | ScrollPhase::Momentum | ScrollPhase::MomentumEnded => {}
            },
            InputEvent::Zoom { delta, at } => {
                // Ctrl+wheel: up (negative steps) zooms in.
                self.motion(*at, &mut calls);
                if *delta != 0 {
                    self.key(evdev::KEY_LEFTCTRL, true, &mut calls);
                    calls.push(RemoteCall::AxisDiscrete { axis: 0, steps: -delta });
                    self.key(evdev::KEY_LEFTCTRL, false, &mut calls);
                }
            }
            InputEvent::Semantic(action) => {
                let chord = keymap::semantic_chord(*action);
                self.chord(&chord.modifiers, chord.key, &mut calls);
            }
            InputEvent::Key { key, modifiers } => {
                self.chord(&keymap::modifier_keys(*modifiers), keymap::evdev_key(*key), &mut calls);
            }
            InputEvent::Text(text) => {
                for step in keymap::text_steps(text, self.text) {
                    match step {
                        TextStep::Keysym(keysym) => Self::tap_keysym(keysym, &mut calls),
                        TextStep::UnicodeEntry(code_point) => {
                            self.key(evdev::KEY_LEFTCTRL, true, &mut calls);
                            self.key(evdev::KEY_LEFTSHIFT, true, &mut calls);
                            Self::tap_keysym(keymap::keysym::LOWER_U, &mut calls);
                            self.key(evdev::KEY_LEFTSHIFT, false, &mut calls);
                            self.key(evdev::KEY_LEFTCTRL, false, &mut calls);
                            for digit in keymap::hex_keysyms(code_point) {
                                Self::tap_keysym(digit, &mut calls);
                            }
                            Self::tap_keysym(keymap::keysym::SPACE, &mut calls);
                        }
                    }
                }
            }
        }
        calls
    }

    /// Releases every button and key still held and ends a scroll (disconnect safety).
    pub fn release_all(&mut self) -> Vec<RemoteCall> {
        let mut calls = Vec::new();
        if std::mem::take(&mut self.scrolling) {
            calls.push(RemoteCall::Axis { dx: 0.0, dy: 0.0, finish: true });
        }
        for button in std::mem::take(&mut self.buttons) {
            calls.push(RemoteCall::Button { button, pressed: false });
        }
        for keycode in std::mem::take(&mut self.keys).into_iter().rev() {
            calls.push(RemoteCall::Keycode { keycode, pressed: false });
        }
        calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::input::{Key, PointerEvent, SemanticAction};
    use sidecast_core::protocol::KeyModifiers;

    fn translator() -> Translator {
        // A HiDPI display: 1200×540 points streamed as 2400×1080 pixels.
        Translator::new(Size::new(1200.0, 540.0), (2400, 1080), TextInput::UnicodeEntry)
    }

    fn pointer(kind: PointerEventKind, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent::new(kind, Point::new(x, y)))
    }

    #[test]
    fn points_map_to_stream_pixels_and_back() {
        let t = translator();
        assert_eq!(t.to_stream(Point::new(600.0, 270.0)), (1200.0, 540.0));
        assert_eq!(t.to_stream(Point::new(-5.0, 9999.0)), (0.0, 1079.0), "clamped onto the stream");
        assert_eq!(t.to_display(1200.0, 540.0), Some(Point::new(600.0, 270.0)));
        assert_eq!(t.to_display(2400.0, 10.0), None, "right of the stream");
        assert_eq!(t.to_display(-1.0, 10.0), None);
        // Without HiDPI points are pixels; a degenerate display falls back to 1:1.
        let flat = Translator::new(Size::new(1280.0, 720.0), (1280, 720), TextInput::Keysyms);
        assert_eq!(flat.to_stream(Point::new(12.5, 7.0)), (12.5, 7.0));
        let odd = Translator::new(Size::ZERO, (100, 100), TextInput::Keysyms);
        assert_eq!(odd.to_stream(Point::new(5.0, 5.0)), (5.0, 5.0));
    }

    #[test]
    fn a_click_moves_then_presses_and_releases_once() {
        let mut t = translator();
        assert_eq!(
            t.translate(&pointer(PointerEventKind::Moved, 10.0, 20.0)),
            [RemoteCall::MotionAbsolute { x: 20.0, y: 40.0 }]
        );
        assert_eq!(
            t.translate(&pointer(PointerEventKind::ButtonDown(MouseButton::Left), 10.0, 20.0)),
            [RemoteCall::Button { button: 0x110, pressed: true }],
            "no motion when the pointer is already there"
        );
        assert!(
            t.translate(&pointer(PointerEventKind::ButtonDown(MouseButton::Left), 10.0, 20.0)).is_empty(),
            "a held button is not pressed twice"
        );
        assert_eq!(
            t.translate(&pointer(PointerEventKind::Dragged, 11.0, 20.0)),
            [RemoteCall::MotionAbsolute { x: 22.0, y: 40.0 }]
        );
        assert_eq!(
            t.translate(&pointer(PointerEventKind::ButtonUp(MouseButton::Left), 11.0, 20.0)),
            [RemoteCall::Button { button: 0x110, pressed: false }]
        );
        assert!(t.translate(&pointer(PointerEventKind::ButtonUp(MouseButton::Left), 11.0, 20.0)).is_empty());
        assert_eq!(
            t.translate(&pointer(PointerEventKind::ButtonDown(MouseButton::Right), 5.0, 5.0)),
            [RemoteCall::MotionAbsolute { x: 10.0, y: 10.0 }, RemoteCall::Button { button: 0x111, pressed: true }]
        );
        assert_eq!(t.last_pointer(), Some(Point::new(5.0, 5.0)));
        assert_eq!(t.release_all(), [RemoteCall::Button { button: 0x111, pressed: false }]);
        assert!(t.release_all().is_empty());
    }

    #[test]
    fn scrolling_behaves_like_a_touchpad() {
        let mut t = translator();
        let scroll = |dx, dy, phase| InputEvent::Scroll { dx, dy, at: Point::new(100.0, 50.0), phase };
        // Finger moves down 3 pt: content follows the finger, i.e. scrolls up.
        assert_eq!(
            t.translate(&scroll(0.0, 3.0, ScrollPhase::Began)),
            [RemoteCall::MotionAbsolute { x: 200.0, y: 100.0 }, RemoteCall::Axis { dx: -0.0, dy: -3.0, finish: false }]
        );
        assert_eq!(
            t.translate(&scroll(1.5, -2.0, ScrollPhase::Changed)),
            [RemoteCall::Axis { dx: -1.5, dy: 2.0, finish: false }]
        );
        assert_eq!(
            t.translate(&scroll(0.0, 0.0, ScrollPhase::Ended)),
            [RemoteCall::Axis { dx: 0.0, dy: 0.0, finish: true }]
        );
        // The engine's momentum is left to the apps' own kinetic scrolling.
        for phase in [ScrollPhase::MomentumBegan, ScrollPhase::Momentum, ScrollPhase::MomentumEnded] {
            assert!(t.translate(&scroll(0.0, 9.0, phase)).is_empty());
        }
        assert!(t.translate(&scroll(0.0, 0.0, ScrollPhase::Ended)).is_empty(), "nothing to finish");
        // A scroll cut short by a disconnect is finished.
        t.translate(&scroll(0.0, 1.0, ScrollPhase::Began));
        assert_eq!(t.release_all(), [RemoteCall::Axis { dx: 0.0, dy: 0.0, finish: true }]);
    }

    #[test]
    fn pinch_is_ctrl_wheel() {
        let mut t = translator();
        assert_eq!(
            t.translate(&InputEvent::Zoom { delta: 2, at: Point::new(1.0, 1.0) }),
            [
                RemoteCall::MotionAbsolute { x: 2.0, y: 2.0 },
                RemoteCall::Keycode { keycode: 29, pressed: true },
                RemoteCall::AxisDiscrete { axis: 0, steps: -2 },
                RemoteCall::Keycode { keycode: 29, pressed: false },
            ]
        );
        assert_eq!(
            t.translate(&InputEvent::Zoom { delta: -1, at: Point::new(1.0, 1.0) }),
            [
                RemoteCall::Keycode { keycode: 29, pressed: true },
                RemoteCall::AxisDiscrete { axis: 0, steps: 1 },
                RemoteCall::Keycode { keycode: 29, pressed: false },
            ]
        );
    }

    #[test]
    fn keys_hold_their_modifiers() {
        let mut t = translator();
        let calls =
            t.translate(&InputEvent::Key { key: Key::Left, modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL });
        assert_eq!(
            calls,
            [
                RemoteCall::Keycode { keycode: 29, pressed: true },
                RemoteCall::Keycode { keycode: 42, pressed: true },
                RemoteCall::Keycode { keycode: 105, pressed: true },
                RemoteCall::Keycode { keycode: 105, pressed: false },
                RemoteCall::Keycode { keycode: 42, pressed: false },
                RemoteCall::Keycode { keycode: 29, pressed: false },
            ]
        );
        assert!(t.release_all().is_empty(), "nothing stays held");
    }

    #[test]
    fn swipes_press_the_gnome_shortcuts() {
        let mut t = translator();
        assert_eq!(
            t.translate(&InputEvent::Semantic(SemanticAction::DesktopNext)),
            [
                RemoteCall::Keycode { keycode: 125, pressed: true },
                RemoteCall::Keycode { keycode: 109, pressed: true },
                RemoteCall::Keycode { keycode: 109, pressed: false },
                RemoteCall::Keycode { keycode: 125, pressed: false },
            ]
        );
        assert_eq!(
            t.translate(&InputEvent::Semantic(SemanticAction::Overview)),
            [RemoteCall::Keycode { keycode: 125, pressed: true }, RemoteCall::Keycode { keycode: 125, pressed: false }]
        );
    }

    #[test]
    fn text_types_keysyms_and_unicode_entry() {
        let mut t = translator();
        let keysym = |k: u32, pressed| RemoteCall::Keysym { keysym: k, pressed };
        let tap = |c: char| [keysym(c as u32, true), keysym(c as u32, false)];
        let calls = t.translate(&InputEvent::Text("aé".into()));
        let mut expected = tap('a').to_vec();
        expected.extend([
            RemoteCall::Keycode { keycode: 29, pressed: true },
            RemoteCall::Keycode { keycode: 42, pressed: true },
        ]);
        expected.extend(tap('u'));
        expected.extend([
            RemoteCall::Keycode { keycode: 42, pressed: false },
            RemoteCall::Keycode { keycode: 29, pressed: false },
        ]);
        expected.extend(tap('e'));
        expected.extend(tap('9'));
        expected.extend(tap(' '));
        assert_eq!(calls, expected);

        let mut keysyms = Translator::new(Size::new(10.0, 10.0), (10, 10), TextInput::Keysyms);
        assert_eq!(keysyms.translate(&InputEvent::Text("é".into())), [keysym(0xe9, true), keysym(0xe9, false)]);
    }
}
