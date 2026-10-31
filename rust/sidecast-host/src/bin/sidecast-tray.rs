//! `sidecast-tray`: the Sidecast host for Windows as a notification-area
//! (tray) app. A GUI-subsystem binary, so starting it from the Start menu or
//! at login opens no console window; it logs to
//! `%LOCALAPPDATA%\Sidecast\sidecast.log`. It takes the same options as
//! `sidecast-host` (for example `--verbose`, or `--test-pattern --data-dir DIR`
//! to try the tray without capturing anything).

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    sidecast_host::tray::run(std::env::args().skip(1))
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("sidecast-tray is the Windows tray app; on this OS run sidecast-host (or, on a Mac, the Sidecast app)");
    ExitCode::from(2)
}
