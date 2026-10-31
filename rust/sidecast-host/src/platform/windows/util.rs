//! Small Win32 helpers shared by the Windows backends.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoIncrementMTAUsage, CoInitializeEx};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::PCWSTR;

/// A NUL-terminated UTF-16 copy of `text`, for `PCWSTR` parameters.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `PCWSTR` for a buffer from [`wide`]; the buffer must outlive the call.
pub fn pcwstr(buffer: &[u16]) -> PCWSTR {
    PCWSTR(buffer.as_ptr())
}

/// The text of a fixed-size UTF-16 buffer up to its first NUL.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Process-wide setup, before any window or capture: per-monitor DPI
/// awareness, so every coordinate (monitor rectangles, `SendInput`,
/// `GetCursorPos`, Desktop Duplication) is in physical pixels and
/// `DuplicateOutput1` is allowed; and a COM apartment for the whole process.
/// Idempotent.
pub fn init_process() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: a process-wide setter without pointers; it fails harmlessly
        // when the awareness was already set (by a manifest or an earlier call).
        if let Err(error) = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
            log::debug!("windows: DPI awareness not changed ({error})");
        }
        // SAFETY: no pointers. Keeps the multithreaded COM apartment alive for
        // the whole process, so Media Foundation and WASAPI objects work from
        // any thread (threads that never call CoInitializeEx join it implicitly).
        if let Err(error) = unsafe { CoIncrementMTAUsage() } {
            log::warn!("windows: COM is unavailable ({error})");
        }
    });
}

/// Joins the calling thread to the multithreaded COM apartment (idempotent per thread).
pub fn com_thread() {
    thread_local! {
        static JOINED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    JOINED.with(|joined| {
        if !joined.get() {
            // SAFETY: no pointers; S_FALSE (already joined) is fine, and a thread
            // already in a single-threaded apartment keeps it (RPC_E_CHANGED_MODE).
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            joined.set(true);
        }
    });
}
