//! Monitors as GDI sees them: enumeration, display modes, attaching,
//! positioning and detaching. The legacy display-settings API needs no
//! administrator rights and drives indirect-display (IddCx) monitors such as
//! the Virtual Display Driver's like any other.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use windows::Win32::Foundation::POINTL;
use windows::Win32::Graphics::Gdi::{
    CDS_NORESET, CDS_TYPE, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE, DISP_CHANGE_BADMODE,
    DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_MIRRORING_DRIVER,
    DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION,
    ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_FLAGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayDevicesW,
    EnumDisplaySettingsExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows::core::PCWSTR;

use super::mapping::Rect;
use super::util::{from_wide, pcwstr, wide};
use super::vdd_config::{Mode, is_vdd_device};

/// One display source (`\\.\DISPLAYn`) and the monitor on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// GDI device name, e.g. `\\.\DISPLAY2`.
    pub device: String,
    /// The adapter's description ("NVIDIA GeForce …", "Virtual Display Driver").
    pub adapter: String,
    pub adapter_id: String,
    /// The monitor's name ("Generic PnP Monitor", "VDD by MTT").
    pub monitor_name: String,
    pub monitor_id: String,
    /// Part of the desktop (extended or primary).
    pub attached: bool,
    pub primary: bool,
    /// Desktop rectangle in physical pixels (empty when detached).
    pub rect: Rect,
    pub refresh: u32,
}

impl Monitor {
    /// A monitor of the Virtual Display Driver.
    pub fn is_virtual_display(&self) -> bool {
        is_vdd_device(&self.adapter, &self.adapter_id, &self.monitor_name, &self.monitor_id)
    }

    /// "\\.\DISPLAY2 — 1920×1080 (primary)" for logs and menus.
    pub fn describe(&self) -> String {
        let mut text = format!("{} ({})", self.device, self.adapter);
        if self.attached {
            text.push_str(&format!(" {}x{}", self.rect.width, self.rect.height));
        } else {
            text.push_str(" detached");
        }
        if self.primary {
            text.push_str(", primary");
        }
        text
    }
}

fn device_info(parent: PCWSTR, index: u32) -> Option<DISPLAY_DEVICEW> {
    let mut device = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // SAFETY: `device` is a valid, sized DISPLAY_DEVICEW; `parent` is null or
    // a NUL-terminated string that outlives the call.
    unsafe { EnumDisplayDevicesW(parent, index, &mut device, 0) }.as_bool().then_some(device)
}

/// Every display source with a monitor, attached or not (mirroring drivers left out).
pub fn monitors() -> Vec<Monitor> {
    let mut found = Vec::new();
    for index in 0.. {
        let Some(adapter) = device_info(PCWSTR::null(), index) else { break };
        if adapter.StateFlags & DISPLAY_DEVICE_MIRRORING_DRIVER == DISPLAY_DEVICE_MIRRORING_DRIVER {
            continue;
        }
        let device = from_wide(&adapter.DeviceName);
        let name = wide(&device);
        let Some(monitor) = device_info(pcwstr(&name), 0) else { continue };
        let attached = adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == DISPLAY_DEVICE_ATTACHED_TO_DESKTOP;
        let (rect, refresh) = if attached { current_mode(&device).unwrap_or_default() } else { Default::default() };
        found.push(Monitor {
            adapter: from_wide(&adapter.DeviceString),
            adapter_id: from_wide(&adapter.DeviceID),
            monitor_name: from_wide(&monitor.DeviceString),
            monitor_id: from_wide(&monitor.DeviceID),
            attached,
            primary: adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE == DISPLAY_DEVICE_PRIMARY_DEVICE,
            rect,
            refresh,
            device,
        });
    }
    found
}

fn settings(device: &str, mode: ENUM_DISPLAY_SETTINGS_MODE) -> Option<DEVMODEW> {
    let name = wide(device);
    let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: `name` is NUL-terminated and `dm` a sized DEVMODEW, both alive for the call.
    unsafe { EnumDisplaySettingsExW(pcwstr(&name), mode, &mut dm, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }
        .as_bool()
        .then_some(dm)
}

/// The desktop rectangle and refresh rate of an attached monitor.
pub fn current_mode(device: &str) -> Option<(Rect, u32)> {
    let dm = settings(device, ENUM_CURRENT_SETTINGS)?;
    // SAFETY: for display devices the DEVMODEW union holds the display variant.
    let position = unsafe { dm.Anonymous1.Anonymous2.dmPosition };
    let rect = Rect::new(position.x, position.y, dm.dmPelsWidth as i32, dm.dmPelsHeight as i32);
    (!rect.is_empty()).then_some((rect, dm.dmDisplayFrequency))
}

/// The modes a monitor offers (deduplicated, largest first).
pub fn modes(device: &str) -> Vec<Mode> {
    let mut modes: Vec<Mode> = (0..)
        .map_while(|i| settings(device, ENUM_DISPLAY_SETTINGS_MODE(i)))
        .map(|dm| Mode::new(dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency))
        .collect();
    modes.sort_by(|a, b| b.cmp(a));
    modes.dedup();
    modes
}

/// What to make of a monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Attached at `mode` with its top-left at `position` (desktop pixels).
    Attached { mode: Mode, position: (i32, i32) },
    /// Removed from the desktop (the monitor stays connected, unused).
    Detached,
}

fn describe(code: DISP_CHANGE) -> String {
    match code {
        DISP_CHANGE_BADMODE => "mode not supported".into(),
        other => format!("DISP_CHANGE {}", other.0),
    }
}

/// Applies `placement` to a monitor and makes Windows reconfigure the desktop.
pub fn place(device: &str, placement: Placement) -> Result<(), String> {
    let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    dm.dmFields = DM_POSITION | DM_PELSWIDTH | DM_PELSHEIGHT;
    let (position, (width, height)) = match placement {
        Placement::Attached { mode, position } => {
            dm.dmFields |= DM_DISPLAYFREQUENCY;
            dm.dmDisplayFrequency = mode.refresh;
            (position, (mode.width, mode.height))
        }
        // A zero size at the origin is how the API detaches a monitor.
        Placement::Detached => ((0, 0), (0, 0)),
    };
    dm.dmPelsWidth = width;
    dm.dmPelsHeight = height;
    dm.Anonymous1.Anonymous2.dmPosition = POINTL { x: position.0, y: position.1 };
    let name = wide(device);
    // SAFETY: `name` and `dm` are valid for the call. CDS_NORESET stores the
    // change; the second call (all null) applies every stored change at once.
    let staged =
        unsafe { ChangeDisplaySettingsExW(pcwstr(&name), Some(&dm), None, CDS_UPDATEREGISTRY | CDS_NORESET, None) };
    if staged != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("cannot set {device} to {placement:?}: {}", describe(staged)));
    }
    // SAFETY: as above; null arguments mean "apply the stored settings".
    let applied = unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if applied != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("cannot apply the display change for {device}: {}", describe(applied)));
    }
    Ok(())
}

/// The bounding rectangle of all attached monitors, in physical pixels.
pub fn virtual_screen() -> Rect {
    // SAFETY: GetSystemMetrics has no preconditions.
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}
