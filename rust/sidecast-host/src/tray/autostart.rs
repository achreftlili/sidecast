//! "Start at login": the per-user `Run` key, which needs no administrator
//! rights and which Windows' own Startup apps settings list and can turn off.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::path::Path;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

use crate::platform::windows::util::{from_wide, pcwstr, wide};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Sidecast";

/// The command line the `Run` key stores for `exe`: the path in quotes (it
/// may contain spaces), no arguments.
pub fn command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

fn current_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(command(&exe))
}

/// What the `Run` key holds for Sidecast, if anything.
fn stored() -> Option<String> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: NUL-terminated names; `buffer`/`size` describe a writable buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&key),
            pcwstr(&value),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    status.is_ok().then(|| from_wide(&buffer))
}

/// Sidecast starts at login (this copy of it).
pub fn is_enabled() -> bool {
    match (stored(), current_command()) {
        (Some(stored), Some(current)) => stored.eq_ignore_ascii_case(&current),
        _ => false,
    }
}

/// Turns starting at login on (for this executable) or off.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    if enabled {
        let command = current_command().ok_or("cannot find this program's path")?;
        let data = wide(&command);
        // SAFETY: NUL-terminated names; `data` is a NUL-terminated REG_SZ of the given byte length.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                pcwstr(&key),
                pcwstr(&value),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        };
        status.ok().map_err(|e| format!("cannot turn on start at login: {e}"))
    } else {
        // SAFETY: NUL-terminated names.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, pcwstr(&key), pcwstr(&value)) };
        if status.is_ok() || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("cannot turn off start at login: {}", windows::core::Error::from(status.to_hresult())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_quotes_the_path() {
        let exe = Path::new(r"C:\Users\Ada Lovelace\AppData\Local\Programs\Sidecast\sidecast-tray.exe");
        assert_eq!(command(exe), r#""C:\Users\Ada Lovelace\AppData\Local\Programs\Sidecast\sidecast-tray.exe""#);
    }
}
