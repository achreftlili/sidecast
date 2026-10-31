//! Captured pixels → what the encoders take: 32-bit RGB frames from PipeWire
//! (BGRx and friends) → I420, BT.709 limited range (what OpenH264's VUI
//! signals), with the pointer composited from PipeWire's cursor metadata and a
//! nearest-neighbour resize when the encoder needs a smaller frame (codec
//! limits). Pure, so it is unit-tested on every OS.

use sidecast_server::backend::{CapturedFrame, PixelData};

/// Byte order of a 32-bit pixel. The alpha / padding byte is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    /// B, G, R, x in memory (SPA `BGRx` / `BGRA`), what Mutter prefers.
    Bgrx,
    Rgbx,
    Xrgb,
    Xbgr,
}

impl PixelLayout {
    /// Byte offsets of R, G and B within a pixel.
    pub const fn offsets(self) -> (usize, usize, usize) {
        match self {
            PixelLayout::Bgrx => (2, 1, 0),
            PixelLayout::Rgbx => (0, 1, 2),
            PixelLayout::Xrgb => (1, 2, 3),
            PixelLayout::Xbgr => (3, 2, 1),
        }
    }
}

/// The latest captured frame, tightly packed (`stride = width × 4`).
#[derive(Debug, Clone)]
pub struct FrameStore {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub layout: PixelLayout,
}

impl FrameStore {
    /// Copies `height` rows of `width` pixels from a buffer with `stride` bytes per row.
    /// `None` when the source is too short for that geometry.
    pub fn copy_from(src: &[u8], width: usize, height: usize, stride: usize, layout: PixelLayout) -> Option<Self> {
        let row = width.checked_mul(4)?;
        if width == 0 || height == 0 || stride < row || src.len() < stride * (height - 1) + row {
            return None;
        }
        let mut data = Vec::with_capacity(row * height);
        for y in 0..height {
            data.extend_from_slice(&src[y * stride..y * stride + row]);
        }
        Some(FrameStore { data, width, height, layout })
    }

    /// Replaces the contents in place, reusing the allocation.
    pub fn update_from(&mut self, src: &[u8], width: usize, height: usize, stride: usize, layout: PixelLayout) -> bool {
        let row = width * 4;
        if width == 0 || height == 0 || stride < row || src.len() < stride * (height - 1) + row {
            return false;
        }
        self.data.clear();
        for y in 0..height {
            self.data.extend_from_slice(&src[y * stride..y * stride + row]);
        }
        (self.width, self.height, self.layout) = (width, height, layout);
        true
    }

    /// Blends `cursor` with its hotspot at (`x`, `y`) into the frame, runs `f`,
    /// then restores the pixels underneath (the stored frame stays clean for
    /// the next cursor position).
    pub fn with_cursor<R>(&mut self, cursor: Option<(&CursorSprite, i32, i32)>, f: impl FnOnce(&FrameStore) -> R) -> R {
        let Some((sprite, x, y)) = cursor else { return f(self) };
        let left = x - sprite.hotspot.0;
        let top = y - sprite.hotspot.1;
        // The visible part of the sprite.
        let x0 = left.max(0) as usize;
        let y0 = top.max(0) as usize;
        let x1 = (left + sprite.width as i32).clamp(0, self.width as i32) as usize;
        let y1 = (top + sprite.height as i32).clamp(0, self.height as i32) as usize;
        if x0 >= x1 || y0 >= y1 {
            return f(self);
        }
        let row = self.width * 4;
        let saved: Vec<u8> = (y0..y1).flat_map(|fy| self.data[fy * row + x0 * 4..fy * row + x1 * 4].to_vec()).collect();
        let (ro, go, bo) = self.layout.offsets();
        for fy in y0..y1 {
            let sy = (fy as i32 - top) as usize;
            for fx in x0..x1 {
                let sx = (fx as i32 - left) as usize;
                let s = &sprite.rgba[(sy * sprite.width + sx) * 4..][..4];
                let a = s[3] as u32;
                if a == 0 {
                    continue;
                }
                let d = &mut self.data[fy * row + fx * 4..][..4];
                // Premultiplied "over": out = src + dst × (1 − α).
                let over = |src: u8, dst: u8| (src as u32 + (dst as u32 * (255 - a) + 127) / 255).min(255) as u8;
                d[ro] = over(s[0], d[ro]);
                d[go] = over(s[1], d[go]);
                d[bo] = over(s[2], d[bo]);
            }
        }
        let result = f(self);
        let span = (x1 - x0) * 4;
        for (i, fy) in (y0..y1).enumerate() {
            self.data[fy * row + x0 * 4..fy * row + x1 * 4].copy_from_slice(&saved[i * span..(i + 1) * span]);
        }
        result
    }
}

/// A pointer image: premultiplied RGBA, tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSprite {
    pub width: usize,
    pub height: usize,
    /// The pixel of the image that sits at the pointer position.
    pub hotspot: (i32, i32),
    pub rgba: Vec<u8>,
}

impl CursorSprite {
    /// From a PipeWire cursor bitmap (Mutter sends premultiplied RGBA).
    pub fn from_bitmap(
        data: &[u8],
        width: usize,
        height: usize,
        stride: usize,
        layout: PixelLayout,
        hotspot: (i32, i32),
    ) -> Option<Self> {
        let store = FrameStore::copy_from(data, width, height, stride, layout)?;
        let (ro, go, bo) = layout.offsets();
        let ao = 6 - ro - go - bo; // the remaining byte of 0..=3
        let rgba = store.data.as_chunks::<4>().0.iter().flat_map(|p| [p[ro], p[go], p[bo], p[ao]]).collect();
        Some(CursorSprite { width, height, hotspot, rgba })
    }

    /// A plain arrow (white, black outline, hotspot at the tip) for when the
    /// compositor has not sent its pointer image yet.
    pub fn arrow() -> Self {
        const ARROW: [&str; 19] = [
            "B",
            "BB",
            "BWB",
            "BWWB",
            "BWWWB",
            "BWWWWB",
            "BWWWWWB",
            "BWWWWWWB",
            "BWWWWWWWB",
            "BWWWWWWWWB",
            "BWWWWWWWWWB",
            "BWWWWWWBBBBB",
            "BWWWBWWB",
            "BWWBBWWB",
            "BWB  BWWB",
            "BB   BWWB",
            "B     BWWB",
            "      BWWB",
            "       BB",
        ];
        let width = ARROW.iter().map(|row| row.len()).max().unwrap_or(0);
        let mut rgba = vec![0u8; width * ARROW.len() * 4];
        for (y, row) in ARROW.iter().enumerate() {
            for (x, cell) in row.bytes().enumerate() {
                let pixel = match cell {
                    b'B' => [0, 0, 0, 255],
                    b'W' => [255, 255, 255, 255],
                    _ => continue,
                };
                rgba[(y * width + x) * 4..][..4].copy_from_slice(&pixel);
            }
        }
        CursorSprite { width, height: ARROW.len(), hotspot: (0, 0), rgba }
    }
}

/// Three I420 planes, tightly packed (`y`: width, `u`/`v`: width / 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420Planes {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// BT.709 limited range, as in the OpenH264 encoder.
#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(16, 235) as u8
}

#[inline]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

/// Converts `frame` to I420 of `out_width × out_height` (even sizes; odd ones
/// are rounded down), resizing with nearest-neighbour sampling when the sizes
/// differ. Chroma is the average of each 2×2 block.
pub fn to_i420(frame: &FrameStore, out_width: usize, out_height: usize) -> I420Planes {
    let (w, h) = (out_width & !1, out_height & !1);
    let (cw, ch) = (w / 2, h / 2);
    let mut planes = I420Planes { width: w, height: h, y: vec![0; w * h], u: vec![0; cw * ch], v: vec![0; cw * ch] };
    if w == 0 || h == 0 || frame.width == 0 || frame.height == 0 {
        return planes;
    }
    // Source column (byte offset) and row of every output column / row.
    let columns: Vec<usize> = (0..w).map(|x| (x * frame.width / w).min(frame.width - 1) * 4).collect();
    let rows: Vec<usize> = (0..h).map(|y| (y * frame.height / h).min(frame.height - 1)).collect();
    let (ro, go, bo) = frame.layout.offsets();
    let stride = frame.width * 4;
    for cy in 0..ch {
        let top = &frame.data[rows[2 * cy] * stride..][..stride];
        let bottom = &frame.data[rows[2 * cy + 1] * stride..][..stride];
        let (y_top, y_bottom) = planes.y[2 * cy * w..(2 * cy + 2) * w].split_at_mut(w);
        for cx in 0..cw {
            let (mut r, mut g, mut b) = (0i32, 0i32, 0i32);
            for (dx, column) in [columns[2 * cx], columns[2 * cx + 1]].into_iter().enumerate() {
                for (line, out) in [(top, &mut *y_top), (bottom, &mut *y_bottom)] {
                    let p = &line[column..column + 4];
                    let (pr, pg, pb) = (p[ro] as i32, p[go] as i32, p[bo] as i32);
                    out[2 * cx + dx] = luma(pr, pg, pb);
                    r += pr;
                    g += pg;
                    b += pb;
                }
            }
            let (u, v) = chroma((r + 2) / 4, (g + 2) / 4, (b + 2) / 4);
            planes.u[cy * cw + cx] = u;
            planes.v[cy * cw + cx] = v;
        }
    }
    planes
}

/// A frame in GStreamer's default raw-video layout (rows padded to 4 bytes,
/// planes back to back), for an `appsrc` without video metadata: the caps
/// format name and the bytes. `None` for native surfaces or short planes.
#[cfg_attr(not(feature = "gstreamer"), allow(dead_code))]
pub fn pack_for_gstreamer(frame: &CapturedFrame) -> Option<(&'static str, Vec<u8>)> {
    let round4 = |n: usize| (n + 3) & !3;
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::new();
    let mut copy_plane = |src: &[u8], stride: usize, row: usize, rows: usize, out_stride: usize| -> Option<()> {
        if stride < row || src.len() < stride * rows.saturating_sub(1) + row {
            return None;
        }
        for y in 0..rows {
            out.extend_from_slice(&src[y * stride..y * stride + row]);
            out.resize(out.len() + out_stride - row, 0);
        }
        Some(())
    };
    let format = match &frame.pixels {
        PixelData::I420 { y, u, v, y_stride, uv_stride } => {
            copy_plane(y, *y_stride, w, h, round4(w))?;
            copy_plane(u, *uv_stride, cw, ch, round4(cw))?;
            copy_plane(v, *uv_stride, cw, ch, round4(cw))?;
            "I420"
        }
        PixelData::Nv12 { y, uv, y_stride, uv_stride } => {
            copy_plane(y, *y_stride, w, h, round4(w))?;
            copy_plane(uv, *uv_stride, cw * 2, ch, round4(cw * 2))?;
            "NV12"
        }
        PixelData::Bgra { data, stride } => {
            copy_plane(data, *stride, w * 4, h, w * 4)?;
            "BGRx"
        }
        PixelData::Native(_) => return None,
    };
    Some((format, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, layout: PixelLayout, rgb: (u8, u8, u8)) -> FrameStore {
        let (ro, go, bo) = layout.offsets();
        let mut pixel = [0xffu8; 4];
        (pixel[ro], pixel[go], pixel[bo]) = rgb;
        FrameStore { data: pixel.repeat(width * height), width, height, layout }
    }

    #[test]
    fn layouts_name_their_byte_offsets() {
        for layout in [PixelLayout::Bgrx, PixelLayout::Rgbx, PixelLayout::Xrgb, PixelLayout::Xbgr] {
            let (r, g, b) = layout.offsets();
            assert!(r != g && g != b && r != b && r.max(g).max(b) <= 3, "{layout:?}");
        }
        assert_eq!(PixelLayout::Bgrx.offsets(), (2, 1, 0));
    }

    #[test]
    fn white_black_and_red_are_bt709_video_range_in_every_layout() {
        for layout in [PixelLayout::Bgrx, PixelLayout::Rgbx, PixelLayout::Xrgb, PixelLayout::Xbgr] {
            let white = to_i420(&solid(4, 2, layout, (255, 255, 255)), 4, 2);
            assert!(white.y.iter().all(|&y| y == 235), "{layout:?}");
            assert!(white.u.iter().chain(&white.v).all(|&c| c == 128));
            let black = to_i420(&solid(4, 2, layout, (0, 0, 0)), 4, 2);
            assert!(black.y.iter().all(|&y| y == 16));
            let red = to_i420(&solid(2, 2, layout, (255, 0, 0)), 2, 2);
            // BT.709 red: Y 63, Cb 102, Cr 240 (video range).
            assert_eq!((red.y[0], red.u[0], red.v[0]), (63, 102, 240), "{layout:?}");
        }
    }

    #[test]
    fn plane_sizes_and_chroma_averaging() {
        // Left half black, right half white: 2×2 blocks are uniform.
        let mut frame = solid(8, 4, PixelLayout::Bgrx, (0, 0, 0));
        for y in 0..4 {
            for x in 4..8 {
                frame.data[(y * 8 + x) * 4..][..3].fill(255);
            }
        }
        let planes = to_i420(&frame, 8, 4);
        assert_eq!((planes.y.len(), planes.u.len(), planes.v.len()), (32, 8, 8));
        assert_eq!(&planes.y[..8], &[16, 16, 16, 16, 235, 235, 235, 235]);
        // A block straddling black and white averages to mid grey.
        let mut mixed = solid(2, 2, PixelLayout::Rgbx, (0, 0, 0));
        mixed.data[..3].fill(255);
        mixed.data[4..7].fill(255);
        let p = to_i420(&mixed, 2, 2);
        assert_eq!((p.u[0], p.v[0]), (128, 128));
        assert_eq!(&p.y, &[235, 235, 16, 16]);
        // Odd output sizes are rounded down to even.
        let odd = to_i420(&frame, 7, 3);
        assert_eq!((odd.width, odd.height, odd.y.len(), odd.u.len()), (6, 2, 12, 3));
    }

    #[test]
    fn resizes_with_nearest_neighbour() {
        // 4×4 quadrants: red, green / blue, white → 2×2 picks one pixel of each.
        let mut frame = solid(4, 4, PixelLayout::Bgrx, (0, 0, 0));
        let colours = [[(255, 0, 0), (0, 255, 0)], [(0, 0, 255), (255, 255, 255)]];
        for y in 0..4 {
            for x in 0..4 {
                let (r, g, b) = colours[y / 2][x / 2];
                let p = &mut frame.data[(y * 4 + x) * 4..][..4];
                (p[2], p[1], p[0]) = (r, g, b);
            }
        }
        let small = to_i420(&frame, 2, 2);
        assert_eq!(small.y, [luma(255, 0, 0), luma(0, 255, 0), luma(0, 0, 255), luma(255, 255, 255)]);
        // Upscaling repeats pixels.
        let big = to_i420(&solid(2, 2, PixelLayout::Bgrx, (255, 255, 255)), 6, 4);
        assert!(big.y.iter().all(|&y| y == 235) && big.y.len() == 24);
    }

    #[test]
    fn copies_strided_rows_and_rejects_short_buffers() {
        // 2×2 pixels in rows of 12 bytes (4 bytes of padding each).
        let src: Vec<u8> = (0..24).collect();
        let frame = FrameStore::copy_from(&src, 2, 2, 12, PixelLayout::Bgrx).unwrap();
        assert_eq!(frame.data, [0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert!(FrameStore::copy_from(&src, 2, 3, 12, PixelLayout::Bgrx).is_none(), "too short");
        assert!(FrameStore::copy_from(&src, 4, 2, 12, PixelLayout::Bgrx).is_none(), "stride below the row");
        assert!(FrameStore::copy_from(&src, 0, 2, 12, PixelLayout::Bgrx).is_none());
        let mut store = frame.clone();
        assert!(store.update_from(&src[..20], 2, 2, 12, PixelLayout::Rgbx), "the last row needs no padding");
        assert_eq!(store.layout, PixelLayout::Rgbx);
        assert!(!store.update_from(&src[..10], 2, 2, 12, PixelLayout::Rgbx));
    }

    #[test]
    fn cursor_is_blended_premultiplied_and_then_removed() {
        let mut frame = solid(4, 4, PixelLayout::Bgrx, (0, 0, 0));
        let original = frame.data.clone();
        // 2×2 sprite: opaque white, transparent, half-transparent black (shadow), half white.
        let sprite = CursorSprite {
            width: 2,
            height: 2,
            hotspot: (1, 1),
            rgba: vec![255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 128, 128, 128, 128, 128],
        };
        // Hotspot at (2, 2): the sprite covers (1, 1)…(2, 2).
        let seen = frame.with_cursor(Some((&sprite, 2, 2)), |f| {
            let px = |x: usize, y: usize| f.data[(y * 4 + x) * 4..][..3].to_vec();
            (px(1, 1), px(2, 1), px(1, 2), px(2, 2), px(0, 0))
        });
        assert_eq!(seen.0, [255, 255, 255]);
        assert_eq!(seen.1, [0, 0, 0], "transparent pixels leave the frame alone");
        assert_eq!(seen.2, [0, 0, 0]);
        assert_eq!(seen.3, [128, 128, 128], "premultiplied over black");
        assert_eq!(seen.4, [0, 0, 0], "outside the sprite");
        assert_eq!(frame.data, original, "restored afterwards");

        // Partly off the frame (top-left corner) and fully off it.
        let corner = frame.with_cursor(Some((&sprite, 0, 0)), |f| f.data[..3].to_vec());
        assert_eq!(corner, [128, 128, 128]);
        assert_eq!(frame.data, original);
        frame.with_cursor(Some((&sprite, 100, -50)), |f| assert_eq!(f.data, original));
        frame.with_cursor(None, |f| assert_eq!(f.data, original));
    }

    #[test]
    fn fallback_arrow_is_outlined_with_its_tip_as_hotspot() {
        let arrow = CursorSprite::arrow();
        assert_eq!((arrow.width, arrow.height, arrow.hotspot), (12, 19, (0, 0)));
        assert_eq!(arrow.rgba.len(), 12 * 19 * 4);
        let px = |x: usize, y: usize| &arrow.rgba[(y * 12 + x) * 4..][..4];
        assert_eq!(px(0, 0), [0, 0, 0, 255], "black tip");
        assert_eq!(px(1, 2), [255, 255, 255, 255], "white inside");
        assert_eq!(px(11, 0), [0, 0, 0, 0], "transparent outside");
        let mut frame = solid(32, 32, PixelLayout::Bgrx, (100, 100, 100));
        let drawn = frame.with_cursor(Some((&arrow, 5, 5)), |f| (f.data[(5 * 32 + 5) * 4], f.data[(7 * 32 + 6) * 4]));
        assert_eq!(drawn, (0, 255));
    }

    #[test]
    fn cursor_bitmaps_are_read_in_their_layout() {
        // One BGRA pixel: blue 10, green 20, red 30, alpha 40.
        let sprite = CursorSprite::from_bitmap(&[10, 20, 30, 40], 1, 1, 4, PixelLayout::Bgrx, (0, 0)).unwrap();
        assert_eq!(sprite.rgba, [30, 20, 10, 40]);
        let sprite = CursorSprite::from_bitmap(&[40, 30, 20, 10], 1, 1, 4, PixelLayout::Xrgb, (3, 4)).unwrap();
        assert_eq!((sprite.rgba.as_slice(), sprite.hotspot), (&[30, 20, 10, 40][..], (3, 4)));
        assert!(CursorSprite::from_bitmap(&[0; 3], 1, 1, 4, PixelLayout::Rgbx, (0, 0)).is_none());
    }

    #[test]
    fn packs_frames_in_gstreamer_layout() {
        let frame = |pixels| CapturedFrame { pts_micros: 0, width: 6, height: 2, pixels };
        // I420 6×2 with padded source strides: Y rows 8 → 8, U/V rows of 3 → padded to 4.
        let i420 = frame(PixelData::I420 {
            y: (0..16).collect(),
            u: vec![1, 2, 3, 0, 0],
            v: vec![4, 5, 6, 0, 0],
            y_stride: 8,
            uv_stride: 5,
        });
        let (format, bytes) = pack_for_gstreamer(&i420).unwrap();
        assert_eq!(format, "I420");
        assert_eq!(bytes, [0, 1, 2, 3, 4, 5, 0, 0, 8, 9, 10, 11, 12, 13, 0, 0, 1, 2, 3, 0, 4, 5, 6, 0]);
        let nv12 = frame(PixelData::Nv12 { y: vec![9; 12], uv: vec![7; 6], y_stride: 6, uv_stride: 6 });
        let (format, bytes) = pack_for_gstreamer(&nv12).unwrap();
        assert_eq!((format, bytes.len()), ("NV12", 8 * 2 + 8));
        let bgra = frame(PixelData::Bgra { data: vec![3; 48], stride: 24 });
        assert_eq!(pack_for_gstreamer(&bgra).map(|(f, b)| (f, b.len())), Some(("BGRx", 48)));
        let short = frame(PixelData::Bgra { data: vec![3; 40], stride: 24 });
        assert!(pack_for_gstreamer(&short).is_none());
        assert!(pack_for_gstreamer(&frame(PixelData::Native(Box::new(())))).is_none());
    }
}
