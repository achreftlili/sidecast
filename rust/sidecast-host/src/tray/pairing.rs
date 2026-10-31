//! The pairing code, large, in a small always-on-top window in the middle
//! of the main screen, so it can be read from the phone's distance. It
//! closes when pairing ends, or on a click or Esc.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FF_SWISS, FW_NORMAL, FW_SEMIBOLD,
    FillRect, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, IDC_ARROW, LoadCursorW, RegisterClassW,
    SPI_GETWORKAREA, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, ShowWindow,
    SystemParametersInfoW, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONUP, WM_PAINT, WNDCLASSW, WS_BORDER, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::platform::windows::util::{pcwstr, wide};

const CLASS: &str = "SidecastPairingCode";

thread_local! {
    /// What the window shows: title, code, hint (UI thread only).
    static CONTENT: RefCell<Option<[String; 3]>> = const { RefCell::new(None) };
}

/// "123456" → "123 456", easier to read and type.
pub fn spaced(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() { format!("{} {}", &code[..3], &code[3..]) } else { code.to_owned() }
}

/// The open pairing window, if any (UI thread only).
#[derive(Debug, Default)]
pub struct PairingWindow {
    hwnd: Option<HWND>,
}

impl PairingWindow {
    /// Shows `code` for `device` (replacing a window already open).
    pub fn show(&mut self, device: &str, code: &str, seconds: u64) {
        self.close();
        CONTENT.with(|c| {
            *c.borrow_mut() = Some([
                format!("Pair {device} with this computer"),
                spaced(code),
                format!("Enter this code on the phone. It is valid for {seconds} seconds."),
            ])
        });
        register_class();
        // SAFETY: a fixed scale from the system DPI; no pointers.
        let scale = unsafe { GetDpiForSystem() } as f64 / 96.0;
        let (width, height) = ((440.0 * scale) as i32, (230.0 * scale) as i32);
        let mut area = RECT::default();
        // SAFETY: `area` is a valid out-pointer for SPI_GETWORKAREA.
        let _ = unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&raw mut area).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        let x = area.left + ((area.right - area.left) - width) / 2;
        let y = area.top + ((area.bottom - area.top) - height) / 2;
        let (class, title) = (wide(CLASS), wide("Sidecast pairing code"));
        // SAFETY: a registered class and NUL-terminated strings that outlive the call.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                pcwstr(&class),
                pcwstr(&title),
                WS_POPUP | WS_BORDER,
                x,
                y,
                width,
                height,
                None,
                None,
                None,
                None,
            )
        };
        match hwnd {
            Ok(hwnd) => {
                // SAFETY: a window we just created.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(hwnd);
                }
                self.hwnd = Some(hwnd);
            }
            Err(error) => log::warn!("tray: cannot show the pairing code window: {error}"),
        }
    }

    pub fn close(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: our window; destroying an already-closed one fails harmlessly.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        CONTENT.with(|c| *c.borrow_mut() = None);
    }
}

fn register_class() {
    thread_local! {
        static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if REGISTERED.with(|r| r.replace(true)) {
        return;
    }
    let class = wide(CLASS);
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        // SAFETY: a system cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        lpszClassName: pcwstr(&class),
        ..Default::default()
    };
    // SAFETY: `wc` and the class name are valid for the call (the name is copied).
    unsafe { RegisterClassW(&wc) };
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        // SAFETY: default handling with the arguments we were given.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn paint(hwnd: HWND) {
    let Some([title, code, hint]) = CONTENT.with(|c| c.borrow().clone()) else { return };
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: standard paint cycle on our window: every GDI object created is
    // selected out and deleted before EndPaint.
    unsafe {
        let dc = BeginPaint(hwnd, &mut ps);
        let scale = GetDpiForSystem() as f64 / 96.0;
        let px = |v: f64| (v * scale) as i32;
        let background = CreateSolidBrush(COLORREF(0x00FF_FFFF));
        let mut all = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut all);
        FillRect(dc, &all, background);
        let _ = DeleteObject(HGDIOBJ(background.0));
        SetBkMode(dc, TRANSPARENT);
        let face = wide("Segoe UI");
        let font = |height: i32, weight: i32| {
            CreateFontW(
                -height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                FF_SWISS.0 as u32,
                pcwstr(&face),
            )
        };
        let lines = [
            (title, px(20.0), FW_NORMAL.0 as i32, 0x0033_3333u32, RECT { top: px(18.0), bottom: px(60.0), ..all }),
            (code, px(56.0), FW_SEMIBOLD.0 as i32, 0x00CC_5A1Fu32, RECT { top: px(60.0), bottom: px(150.0), ..all }),
            (hint, px(14.0), FW_NORMAL.0 as i32, 0x0066_6666u32, RECT { top: px(150.0), bottom: px(215.0), ..all }),
        ];
        for (text, height, weight, color, mut rect) in lines {
            rect.left += px(16.0);
            rect.right -= px(16.0);
            let f = font(height, weight);
            let previous = SelectObject(dc, HGDIOBJ(f.0));
            SetTextColor(dc, COLORREF(color));
            let mut text: Vec<u16> = text.encode_utf16().collect();
            let format =
                if height > px(30.0) { DT_CENTER | DT_SINGLELINE | DT_VCENTER } else { DT_CENTER | DT_WORDBREAK };
            DrawTextW(dc, &mut text, &mut rect, format);
            SelectObject(dc, previous);
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_shown_in_two_groups() {
        assert_eq!(spaced("123456"), "123 456");
        assert_eq!(spaced("12345"), "12345");
    }
}
