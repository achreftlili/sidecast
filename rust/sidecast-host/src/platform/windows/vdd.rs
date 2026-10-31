//! The Virtual Display Driver on this computer: is it installed, where are
//! its settings, and its control pipe (see `vdd_config.rs` for the formats).
//! Sidecast never installs or enables the driver: Windows only loads
//! Microsoft-signed drivers, so users install the driver's official release.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, GENERIC_WRITE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, WriteFile,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

use super::display::{self, Monitor};
use super::util::{pcwstr, wide};
use super::vdd_config::{
    DEFAULT_FOLDER, PIPE_NAME, REGISTRY_KEY, REGISTRY_PATH_VALUE, SETTINGS_FILE, VddSettings, command_bytes,
};

/// The driver's folder: `VDDPATH` from the registry, or the default.
pub fn folder() -> PathBuf {
    let key = wide(REGISTRY_KEY);
    let value = wide(REGISTRY_PATH_VALUE);
    let mut buffer = [0u16; 520];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the strings are NUL-terminated and `buffer`/`size` describe a
    // writable buffer; RRF_RT_REG_SZ makes the result NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            pcwstr(&key),
            pcwstr(&value),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    let path = status.is_ok().then(|| super::util::from_wide(&buffer)).filter(|p| !p.trim().is_empty());
    PathBuf::from(path.unwrap_or_else(|| DEFAULT_FOLDER.to_owned()))
}

pub fn settings_path() -> PathBuf {
    folder().join(SETTINGS_FILE)
}

/// The parsed settings file, if readable.
pub fn read_settings() -> Option<(PathBuf, String, VddSettings)> {
    let path = settings_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let settings = VddSettings::parse(&text);
    Some((path, text, settings))
}

/// The driver's control pipe exists (the driver is installed, enabled and running).
pub fn pipe_available() -> bool {
    let name = wide(PIPE_NAME);
    // SAFETY: `name` is NUL-terminated. A 1 ms wait only probes the pipe.
    if unsafe { WaitNamedPipeW(pcwstr(&name), 1) }.as_bool() {
        return true;
    }
    // Busy (another client is connected) still means it exists.
    windows::core::Error::from_thread().code() == ERROR_SEM_TIMEOUT.to_hresult()
}

/// The driver's monitors, attached or not.
pub fn monitors() -> Vec<Monitor> {
    display::monitors().into_iter().filter(Monitor::is_virtual_display).collect()
}

/// What can be said about the driver without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub pipe: bool,
    pub monitors: usize,
    pub settings: Option<PathBuf>,
}

impl Status {
    pub fn probe() -> Status {
        Status { pipe: pipe_available(), monitors: monitors().len(), settings: read_settings().map(|(p, ..)| p) }
    }

    /// Installed and running: Sidecast can use it.
    pub fn usable(&self) -> bool {
        self.pipe || self.monitors > 0
    }

    pub fn describe(&self) -> String {
        match (self.usable(), &self.settings) {
            (true, _) => format!(
                "installed (control pipe {}, {} virtual monitor(s), settings {})",
                if self.pipe { "available" } else { "not available" },
                self.monitors,
                self.settings.as_ref().map_or("not found".to_owned(), |p| p.display().to_string())
            ),
            (false, Some(path)) => {
                format!(
                    "settings found at {} but the driver is not running (disabled in Device Manager?)",
                    path.display()
                )
            }
            (false, None) => "not installed".to_owned(),
        }
    }
}

/// Sends one command over the control pipe.
pub fn send(command: &str) -> Result<(), String> {
    let name = wide(PIPE_NAME);
    let deadline = Instant::now() + Duration::from_secs(3);
    let handle = loop {
        // SAFETY: `name` is NUL-terminated; no security attributes or template.
        match unsafe {
            CreateFileW(
                pcwstr(&name),
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        } {
            Ok(handle) => break handle,
            Err(error) if error.code() == ERROR_PIPE_BUSY.to_hresult() && Instant::now() < deadline => {
                // SAFETY: as above; waits for a free pipe instance.
                let _ = unsafe { WaitNamedPipeW(pcwstr(&name), 500) };
            }
            Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {
                return Err("the Virtual Display Driver is not running (no control pipe)".into());
            }
            Err(error) => return Err(format!("cannot open the Virtual Display Driver's pipe: {error}")),
        }
    };
    let bytes = command_bytes(command);
    let mut written = 0u32;
    // SAFETY: `handle` is an open pipe and `bytes` outlives the synchronous write.
    let result = unsafe { WriteFile(handle, Some(&bytes), Some(&mut written), None) };
    // SAFETY: we own the handle and close it once.
    let _ = unsafe { CloseHandle(handle) };
    result.map_err(|e| format!("cannot send {command} to the Virtual Display Driver: {e}"))?;
    log::info!("display: sent {command} to the Virtual Display Driver");
    Ok(())
}
