//! Synthetic multi-touch gestures for `--gesture NAME`, sequenced like
//! Android (port of the Swift probe's `ProbeGestures.swift`): DOWN for the
//! first finger, POINTER_DOWN (all pointers so far) for each next one, MOVE
//! every 8 ms with every pointer (actionPointerId = first pointer), then
//! POINTER_UP (lifting pointer still listed) down to the last UP.

use sidecast_core::protocol::{TouchAction, TouchPointer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeGesture {
    Tap,
    Tap2,
    Swipe3Left,
    Swipe3Right,
    Swipe3Up,
    Swipe3Down,
    Swipe4Left,
    Tap3,
    Scroll2,
    TrackpadMove,
    TrackpadTap,
    TrackpadDoubletapDrag,
}

/// One TOUCH, `ms` after the gesture starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub ms: u32,
    pub action: TouchAction,
    pub action_pointer_id: u8,
    pub pointers: Vec<TouchPointer>,
}

impl ProbeGesture {
    const ALL: [(&'static str, ProbeGesture); 12] = [
        ("tap", ProbeGesture::Tap),
        ("tap2", ProbeGesture::Tap2),
        ("swipe3-left", ProbeGesture::Swipe3Left),
        ("swipe3-right", ProbeGesture::Swipe3Right),
        ("swipe3-up", ProbeGesture::Swipe3Up),
        ("swipe3-down", ProbeGesture::Swipe3Down),
        ("swipe4-left", ProbeGesture::Swipe4Left),
        ("tap3", ProbeGesture::Tap3),
        ("scroll2", ProbeGesture::Scroll2),
        ("trackpad-move", ProbeGesture::TrackpadMove),
        ("trackpad-tap", ProbeGesture::TrackpadTap),
        ("trackpad-doubletap-drag", ProbeGesture::TrackpadDoubletapDrag),
    ];

    pub fn parse(name: &str) -> Option<ProbeGesture> {
        Self::ALL.iter().find(|(n, _)| *n == name).map(|(_, g)| *g)
    }

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, g)| *g == self).map_or("?", |(n, _)| n)
    }

    /// What the host's gesture engine should make of it.
    pub fn expected(self) -> &'static str {
        match self {
            ProbeGesture::Tap => "click",
            ProbeGesture::Tap2 => "right click",
            ProbeGesture::Swipe3Left | ProbeGesture::Swipe4Left => "semantic desktop.next",
            ProbeGesture::Swipe3Right => "semantic desktop.previous",
            ProbeGesture::Swipe3Up => "semantic overview",
            ProbeGesture::Swipe3Down => "semantic app.windows",
            ProbeGesture::Tap3 => "nothing",
            ProbeGesture::Scroll2 => "scroll began…ended (+ momentum)",
            ProbeGesture::TrackpadMove => "trackpad: cursor moves right ≈ 0.6 × 20 % of the display",
            ProbeGesture::TrackpadTap => "trackpad: click at the cursor (not at the finger)",
            ProbeGesture::TrackpadDoubletapDrag => "trackpad: click, then button down + drag + button up at the cursor",
        }
    }

    pub fn steps(self) -> Vec<Step> {
        let mut s = Script::default();
        match self {
            ProbeGesture::Tap => {
                s.down(&[(0.5, 0.5)], 12);
                s.wait(60);
                s.up(12);
            }
            ProbeGesture::Tap2 => {
                s.down(&[(0.45, 0.5), (0.55, 0.5)], 12);
                s.wait(30);
                s.moved(0.002, 0.001, 16); // finger jitter, within the slop
                s.wait(40);
                s.up(12);
            }
            ProbeGesture::Swipe3Left => s.swipe(3, (0.55, 0.5), -0.25, 0.0),
            ProbeGesture::Swipe3Right => s.swipe(3, (0.3, 0.5), 0.25, 0.0),
            ProbeGesture::Swipe3Up => s.swipe(3, (0.45, 0.65), 0.0, -0.3),
            ProbeGesture::Swipe3Down => s.swipe(3, (0.45, 0.35), 0.0, 0.3),
            ProbeGesture::Swipe4Left => s.swipe(4, (0.5, 0.5), -0.25, 0.0),
            ProbeGesture::Tap3 => {
                s.down(&[(0.4, 0.5), (0.47, 0.52), (0.54, 0.5)], 12);
                s.wait(20);
                s.moved(0.002, -0.002, 24);
                s.wait(20);
                s.up(12);
            }
            ProbeGesture::Scroll2 => {
                s.down(&[(0.45, 0.35), (0.55, 0.35)], 12);
                s.wait(20);
                s.moved(0.0, 0.25, 160); // a flick: momentum should follow
                s.up(8);
            }
            ProbeGesture::TrackpadMove => {
                s.down(&[(0.3, 0.5)], 12);
                s.wait(16);
                s.moved(0.2, 0.0, 1_000); // slow: about 180 dp/s on a phone
                s.up(12);
            }
            ProbeGesture::TrackpadTap => {
                s.down(&[(0.8, 0.2)], 12); // far from the cursor on purpose
                s.wait(70);
                s.up(12);
            }
            ProbeGesture::TrackpadDoubletapDrag => {
                s.down(&[(0.5, 0.6)], 12);
                s.wait(60);
                s.up(12);
                s.wait(120); // second touch within 350 ms
                s.down(&[(0.5, 0.6)], 12);
                s.wait(60);
                s.moved(-0.1, 0.05, 300);
                s.up(12);
            }
        }
        s.steps
    }
}

/// Builds a timeline; positions are normalized (clamped to 0…1).
#[derive(Default)]
struct Script {
    steps: Vec<Step>,
    t: u32,
    fingers: Vec<TouchPointer>,
}

impl Script {
    fn down(&mut self, positions: &[(f32, f32)], spacing: u32) {
        for (i, (x, y)) in positions.iter().enumerate() {
            if i > 0 {
                self.t += spacing;
            }
            self.fingers.push(TouchPointer::new(i as u8, *x, *y));
            let action = if i == 0 { TouchAction::Down } else { TouchAction::PointerDown };
            self.steps.push(Step { ms: self.t, action, action_pointer_id: i as u8, pointers: self.fingers.clone() });
        }
    }

    fn wait(&mut self, ms: u32) {
        self.t += ms;
    }

    fn moved(&mut self, dx: f32, dy: f32, duration: u32) {
        let every = 8;
        let count = (duration / every).max(1);
        let start = self.fingers.clone();
        for k in 1..=count {
            self.t += every;
            let f = k as f32 / count as f32;
            self.fingers = start
                .iter()
                .map(|p| TouchPointer::new(p.id, (p.x + dx * f).clamp(0.0, 1.0), (p.y + dy * f).clamp(0.0, 1.0)))
                .collect();
            let first = self.fingers[0].id;
            self.steps.push(Step {
                ms: self.t,
                action: TouchAction::Move,
                action_pointer_id: first,
                pointers: self.fingers.clone(),
            });
        }
    }

    fn up(&mut self, spacing: u32) {
        while self.fingers.len() > 1 {
            self.t += spacing;
            let lifting = self.fingers[self.fingers.len() - 1].id;
            self.steps.push(Step {
                ms: self.t,
                action: TouchAction::PointerUp,
                action_pointer_id: lifting,
                pointers: self.fingers.clone(),
            });
            self.fingers.pop();
        }
        self.t += spacing;
        let first = self.fingers[0].id;
        self.steps.push(Step {
            ms: self.t,
            action: TouchAction::Up,
            action_pointer_id: first,
            pointers: self.fingers.clone(),
        });
        self.fingers.clear();
    }

    fn swipe(&mut self, fingers: usize, origin: (f32, f32), dx: f32, dy: f32) {
        let positions: Vec<(f32, f32)> = (0..fingers)
            .map(|i| (origin.0 + i as f32 * 0.06, origin.1 + if i % 2 == 0 { 0.0 } else { 0.02 }))
            .collect();
        self.down(&positions, 12);
        self.wait(16);
        self.moved(dx, dy, 200);
        self.up(12);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gesture_is_a_well_formed_android_sequence() {
        for (name, gesture) in ProbeGesture::ALL {
            assert_eq!(ProbeGesture::parse(name), Some(gesture));
            assert_eq!(gesture.name(), name);
            let steps = gesture.steps();
            assert_eq!(steps.first().map(|s| s.action), Some(TouchAction::Down), "{name}");
            assert_eq!(steps.last().map(|s| s.action), Some(TouchAction::Up), "{name}");
            assert!(steps.windows(2).all(|w| w[0].ms <= w[1].ms), "{name}: time goes forward");
            assert!(steps.iter().all(|s| (1..=10).contains(&s.pointers.len())), "{name}");
        }
        assert_eq!(ProbeGesture::parse("swipe5"), None);
    }
}
