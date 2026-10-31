//! The [`InputSink`] of both Linux remote-desktop APIs. Events are translated
//! on the server's input task (pure, see `translate`) and executed in order on
//! a worker thread, so a D-Bus round trip never stalls touch handling.

use std::sync::{Arc, mpsc};

use sidecast_core::Point;
use sidecast_core::input::InputEvent;
use sidecast_server::backend::{BackendError, InputSink};

use super::capture::SharedCursor;
use super::translate::{RemoteCall, Translator};

/// One remote-desktop session that can execute [`RemoteCall`]s.
pub trait RemoteApi: Send + 'static {
    fn call(&self, call: &RemoteCall) -> Result<(), String>;
}

pub struct RemoteInputSink {
    translator: Translator,
    calls: mpsc::Sender<Vec<RemoteCall>>,
    cursor: Arc<SharedCursor>,
}

impl RemoteInputSink {
    pub fn new(
        api: Box<dyn RemoteApi>,
        translator: Translator,
        cursor: Arc<SharedCursor>,
    ) -> Result<Self, BackendError> {
        let (calls, batches) = mpsc::channel::<Vec<RemoteCall>>();
        std::thread::Builder::new()
            .name("sidecast-remote-input".into())
            .spawn(move || {
                let mut failures = 0u32;
                // Ends when the sink (the sender) is dropped, after the queue drains.
                while let Ok(batch) = batches.recv() {
                    for call in &batch {
                        log_call(call);
                        if let Err(error) = api.call(call) {
                            failures += 1;
                            if failures <= 3 {
                                log::warn!("input: {call:?} failed: {error}");
                            }
                        }
                    }
                }
            })
            .map_err(|e| BackendError::new(sidecast_server::backend::BackendErrorKind::Other, e.to_string()))?;
        Ok(RemoteInputSink { translator, calls, cursor })
    }

    fn send(&self, calls: Vec<RemoteCall>) {
        if !calls.is_empty() {
            let _ = self.calls.send(calls);
        }
    }
}

/// Debug lines the headless-GNOME end-to-end test reads.
fn log_call(call: &RemoteCall) {
    match *call {
        RemoteCall::MotionAbsolute { x, y } => log::debug!("input: motion to ({x:.1}, {y:.1}) px"),
        RemoteCall::Button { button, pressed } => {
            log::debug!("input: button {button:#x} {}", if pressed { "down" } else { "up" })
        }
        RemoteCall::Axis { dx, dy, finish } => {
            log::debug!("input: scroll ({dx:.1}, {dy:.1}){}", if finish { " finish" } else { "" })
        }
        RemoteCall::AxisDiscrete { axis, steps } => log::debug!("input: wheel axis {axis} steps {steps}"),
        RemoteCall::Keycode { keycode, pressed } => {
            log::debug!("input: keycode {keycode} {}", if pressed { "down" } else { "up" })
        }
        RemoteCall::Keysym { keysym, pressed } => {
            log::debug!("input: keysym {keysym:#x} {}", if pressed { "down" } else { "up" })
        }
    }
}

impl InputSink for RemoteInputSink {
    fn inject(&mut self, event: &InputEvent) {
        let calls = self.translator.translate(event);
        self.send(calls);
    }

    /// The compositor's pointer (cursor metadata) once it reports one, else
    /// where we last moved it.
    fn pointer_position(&mut self) -> Option<Point> {
        let cursor = self.cursor.get();
        if cursor.reported {
            cursor.position.and_then(|(x, y)| self.translator.to_display(x, y))
        } else {
            self.translator.last_pointer()
        }
    }

    fn release_all(&mut self) {
        let calls = self.translator.release_all();
        self.send(calls);
    }
}
