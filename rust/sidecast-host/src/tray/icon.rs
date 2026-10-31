//! The tray icon, drawn at run time (no resource compiler needed): a
//! landscape phone, blue while waiting and green while a phone streams.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HGDIOBJ,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO, SM_CXSMICON};

/// Which icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Idle,
    Streaming,
}

impl Look {
    /// Straight (not premultiplied) BGRA body colour.
    fn color(self) -> [u8; 3] {
        match self {
            Look::Idle => [0xFF, 0x7C, 0x4F],      // #4F7CFF
            Look::Streaming => [0x59, 0xA5, 0x22], // #22A559
        }
    }
}

/// `size × size` BGRA pixels (top row first): a rounded landscape phone in
/// the look's colour with a light screen, anti-aliased by 4×4 supersampling.
pub fn pixels(size: usize, look: Look) -> Vec<u8> {
    let s = size as f64;
    let body = (s * 0.04, s * 0.20, s * 0.96, s * 0.80);
    let radius = s * 0.14;
    let inset = s * 0.10;
    let screen = (body.0 + inset, body.1 + inset * 0.8, body.2 - inset, body.3 - inset * 0.8);
    let inside = |r: (f64, f64, f64, f64), radius: f64, x: f64, y: f64| {
        if x < r.0 || x > r.2 || y < r.1 || y > r.3 {
            return false;
        }
        let cx = x.clamp(r.0 + radius, r.2 - radius);
        let cy = y.clamp(r.1 + radius, r.3 - radius);
        (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
    };
    let [b, g, r] = look.color();
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (mut body_hits, mut screen_hits) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let (px, py) = (x as f64 + (sx as f64 + 0.5) / 4.0, y as f64 + (sy as f64 + 0.5) / 4.0);
                    if inside(body, radius, px, py) {
                        body_hits += 1;
                        if inside(screen, radius * 0.4, px, py) {
                            screen_hits += 1;
                        }
                    }
                }
            }
            if body_hits == 0 {
                continue;
            }
            // The screen is a light tint of the body colour.
            let mix = |c: u8| ((c as u32 * (body_hits - screen_hits) + 0xF2 * screen_hits) / body_hits) as u8;
            let p = &mut out[(y * size + x) * 4..(y * size + x) * 4 + 4];
            p.copy_from_slice(&[mix(b), mix(g), mix(r), (body_hits * 255 / 16) as u8]);
        }
    }
    out
}

/// The notification-area icon size for the system DPI.
pub fn tray_size() -> usize {
    // SAFETY: no pointers.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    size.clamp(16, 64) as usize
}

/// Creates an icon (the caller destroys it with DestroyIcon when replaced).
pub fn create(look: Look) -> windows::core::Result<HICON> {
    let size = tray_size();
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size as i32,
        biHeight: -(size as i32), // top-down
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    let image = pixels(size, look);
    // SAFETY: the DIB section is size × size × 4 bytes, which `image` fills
    // exactly; the bitmaps are deleted once the icon has copied them.
    unsafe {
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(image.as_ptr(), bits.cast::<u8>(), image.len());
        // An all-zero AND mask: the alpha channel decides transparency.
        let mask_bits = vec![0u8; size.div_ceil(16) * 2 * size];
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_coloured_phone_on_transparency() {
        let px = pixels(32, Look::Streaming);
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corners are transparent");
        assert_eq!(at(16, 2)[3], 0, "above the phone is transparent");
        assert_eq!(at(16, 16)[3], 255, "the middle is opaque");
        assert_eq!(&at(3, 16)[..3], &[0x59, 0xA5, 0x22], "the edge has the body colour");
        assert!(at(16, 16)[..3].iter().all(|c| *c > 0xE0), "the screen is light");
        assert_ne!(pixels(32, Look::Idle), px);
    }
}
