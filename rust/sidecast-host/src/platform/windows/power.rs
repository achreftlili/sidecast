//! Keeping Windows awake while a phone streams: `SetThreadExecutionState`
//! with `ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED | ES_CONTINUOUS`, so
//! neither idle sleep nor the display timeout cut a stream. Nothing changes
//! the user's power settings, and Windows drops the request by itself if
//! Sidecast exits.
//!
//! The request belongs to the thread that made it, and the server keeps and
//! drops the guard on arbitrary runtime threads, so each guard owns a small
//! thread that sets the state, waits, and clears it.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::sync::mpsc;
use std::thread::JoinHandle;

use sidecast_server::backend::{BackendError, PowerGuard};
use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE, SetThreadExecutionState,
};

/// The keep-awake backend.
#[derive(Debug, Default)]
pub struct ExecutionState;

/// Holds the request until dropped.
struct AwakeGuard {
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PowerGuard for ExecutionState {
    fn keep_awake(&self, reason: &str) -> Result<Box<dyn Send>, BackendError> {
        let (release, released) = mpsc::channel::<()>();
        let (started, result) = mpsc::sync_channel::<bool>(1);
        let reason = reason.to_owned();
        let thread = std::thread::Builder::new()
            .name("sidecast-keep-awake".into())
            .spawn(move || {
                // SAFETY: no pointers; affects only this thread's request.
                let previous =
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
                let _ = started.send(previous != EXECUTION_STATE(0));
                if previous == EXECUTION_STATE(0) {
                    return;
                }
                log::info!("power: keeping the computer and its display awake ({reason})");
                // Until the guard is dropped (its sender closes the channel).
                let _ = released.recv();
                // SAFETY: as above; ES_CONTINUOUS alone clears this thread's request.
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                log::info!("power: sleep allowed again");
            })
            .map_err(|e| BackendError::new(sidecast_server::backend::BackendErrorKind::Other, e.to_string()))?;
        let guard = AwakeGuard { release: Some(release), thread: Some(thread) };
        match result.recv() {
            Ok(true) => Ok(Box::new(guard)),
            _ => Err(BackendError::new(
                sidecast_server::backend::BackendErrorKind::Other,
                "SetThreadExecutionState refused the request",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_awake_sets_and_clears_the_execution_state() {
        let guard = ExecutionState.keep_awake("test").expect("Windows accepts the request");
        drop(guard);
        // A second guard after the first works too (each has its own thread).
        drop(ExecutionState.keep_awake("test again").unwrap());
    }
}
