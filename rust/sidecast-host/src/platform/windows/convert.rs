//! Pixel work on the CPU: BGRA desktop images → NV12 frames of the stream's
//! size (BT.709 limited range, like every Sidecast encoder signals), the
//! mouse pointer drawn into the picture (Desktop Duplication leaves it out),
//! and any [`PixelData`] → the tightly packed NV12 a Media Foundation
//! encoder takes. Pure, so it is tested on every OS.

use sidecast_server::backend::PixelData;

use super::mapping::fit;

/// BT.709 limited range, 8-bit fixed point (as `openh264_encoder` uses).
#[inline]
fn luma(b: i32, g: i32, r: i32) -> u8 {
    (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(16, 235) as u8
}

#[inline]
fn chroma(b: i32, g: i32, r: i32) -> (u8, u8) {
    let u = ((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

/// Black in video range.
const BLACK_Y: u8 = 16;
const NEUTRAL_UV: u8 = 128;

/// Converts `src`-sized BGRA images into `dst`-sized NV12 frames, scaled to
/// fit with the aspect ratio kept (black borders). Built once per size pair.
#[derive(Debug, Clone)]
pub struct Nv12Converter {
    src: (usize, usize),
    dst: (usize, usize),
    /// The picture inside the frame: x, y, width, height (even).
    rect: (usize, usize, usize, usize),
    /// Source column / row of each picture column / row.
    x_map: Vec<u32>,
    y_map: Vec<u32>,
    /// Shrinking by 1.5× or more: average 2×2 source pixels per sample, so
    /// small text does not alias as badly as with nearest-neighbour.
    average: bool,
}

impl Nv12Converter {
    /// `dst` dimensions must be even (the planner guarantees it).
    pub fn new(src_width: usize, src_height: usize, dst_width: usize, dst_height: usize) -> Self {
        let (src_w, src_h) = (src_width.max(1), src_height.max(1));
        let (dst_w, dst_h) = (dst_width & !1, dst_height & !1);
        let rect = if (src_w, src_h) == (dst_w, dst_h) {
            (0, 0, dst_w, dst_h)
        } else {
            let (x, y, w, h) = fit((src_w as f64, src_h as f64), (dst_w as f64, dst_h as f64));
            let even = |v: f64| (v.round() as usize) & !1;
            let (w, h) = (even(w).clamp(2.min(dst_w), dst_w), even(h).clamp(2.min(dst_h), dst_h));
            (even(x).min(dst_w - w), even(y).min(dst_h - h), w, h)
        };
        let map = |count: usize, source: usize| -> Vec<u32> {
            (0..count).map(|i| (((2 * i + 1) * source) / (2 * count.max(1))).min(source - 1) as u32).collect()
        };
        let average = src_w * 2 >= rect.2 * 3 || src_h * 2 >= rect.3 * 3;
        Nv12Converter {
            src: (src_w, src_h),
            dst: (dst_w, dst_h),
            rect,
            x_map: map(rect.2, src_w),
            y_map: map(rect.3, src_h),
            average,
        }
    }

    pub fn source_size(&self) -> (usize, usize) {
        self.src
    }

    pub fn frame_size(&self) -> (usize, usize) {
        self.dst
    }

    /// Bytes of the Y plane and of the interleaved UV plane (both with stride = frame width).
    pub fn plane_sizes(&self) -> (usize, usize) {
        (self.dst.0 * self.dst.1, self.dst.0 * self.dst.1 / 2)
    }

    /// Converts one image. `bgra` is `source_size` with `stride` bytes per
    /// row; `y` and `uv` are [`Self::plane_sizes`] long. Large frames are
    /// split across up to four threads.
    pub fn convert(&self, bgra: &[u8], stride: usize, y: &mut [u8], uv: &mut [u8]) {
        let (dst_w, dst_h) = self.dst;
        let (rx, ry, rw, rh) = self.rect;
        assert!(stride >= self.src.0 * 4 && bgra.len() >= stride * (self.src.1 - 1) + self.src.0 * 4);
        assert!(y.len() >= dst_w * dst_h && uv.len() >= dst_w * dst_h / 2);
        if (rx, ry, rw, rh) != (0, 0, dst_w, dst_h) {
            y[..dst_w * dst_h].fill(BLACK_Y);
            uv[..dst_w * dst_h / 2].fill(NEUTRAL_UV);
        }
        if rw == 0 || rh == 0 {
            return;
        }
        let y_rows = &mut y[ry * dst_w..(ry + rh) * dst_w];
        let uv_rows = &mut uv[(ry / 2) * dst_w..((ry + rh) / 2) * dst_w];
        let threads = if rw * rh >= 640 * 480 {
            std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4)
        } else {
            1
        };
        // Bands of an even number of rows, so each owns whole chroma rows.
        let band = (rh.div_ceil(threads) + 1) & !1;
        if threads == 1 {
            self.convert_rows(bgra, stride, 0, y_rows, uv_rows);
            return;
        }
        std::thread::scope(|scope| {
            for (i, (y_band, uv_band)) in
                y_rows.chunks_mut(band * dst_w).zip(uv_rows.chunks_mut(band / 2 * dst_w)).enumerate()
            {
                scope.spawn(move || self.convert_rows(bgra, stride, i * band, y_band, uv_band));
            }
        });
    }

    /// Picture rows `first..` (relative to the picture's top) into the given
    /// frame rows (full frame width).
    fn convert_rows(&self, bgra: &[u8], stride: usize, first: usize, y_out: &mut [u8], uv_out: &mut [u8]) {
        let (dst_w, _) = self.dst;
        let (rx, _, rw, _) = self.rect;
        let (src_w, src_h) = self.src;
        let rows = y_out.len() / dst_w;
        let sample = |sx: u32, sy: u32| -> (i32, i32, i32) {
            let (sx, sy) = (sx as usize, sy as usize);
            let p = sy * stride + sx * 4;
            if !self.average {
                return (bgra[p] as i32, bgra[p + 1] as i32, bgra[p + 2] as i32);
            }
            let dx = if sx + 1 < src_w { 4 } else { 0 };
            let dy = if sy + 1 < src_h { stride } else { 0 };
            let mut sum = (0, 0, 0);
            for q in [p, p + dx, p + dy, p + dy + dx] {
                sum.0 += bgra[q] as i32;
                sum.1 += bgra[q + 1] as i32;
                sum.2 += bgra[q + 2] as i32;
            }
            ((sum.0 + 2) >> 2, (sum.1 + 2) >> 2, (sum.2 + 2) >> 2)
        };
        for pair in 0..rows / 2 {
            let (row0, row1) = (first + 2 * pair, first + 2 * pair + 1);
            let (sy0, sy1) = (self.y_map[row0], self.y_map[row1]);
            let (top, bottom) = y_out[2 * pair * dst_w..(2 * pair + 2) * dst_w].split_at_mut(dst_w);
            let uv_line = &mut uv_out[pair * dst_w..(pair + 1) * dst_w];
            for col in (0..rw).step_by(2) {
                let (sx0, sx1) = (self.x_map[col], self.x_map[col + 1]);
                let a = sample(sx0, sy0);
                let b = sample(sx1, sy0);
                let c = sample(sx0, sy1);
                let d = sample(sx1, sy1);
                let x = rx + col;
                top[x] = luma(a.0, a.1, a.2);
                top[x + 1] = luma(b.0, b.1, b.2);
                bottom[x] = luma(c.0, c.1, c.2);
                bottom[x + 1] = luma(d.0, d.1, d.2);
                let (u, v) = chroma(
                    (a.0 + b.0 + c.0 + d.0 + 2) >> 2,
                    (a.1 + b.1 + c.1 + d.1 + 2) >> 2,
                    (a.2 + b.2 + c.2 + d.2 + 2) >> 2,
                );
                uv_line[x] = u;
                uv_line[x + 1] = v;
            }
        }
    }
}

/// The kinds of pointer shape Desktop Duplication reports
/// (`DXGI_OUTDUPL_POINTER_SHAPE_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    /// 1 bpp AND mask above a 1 bpp XOR mask (the shape is half as tall as the data).
    Monochrome,
    /// 32 bpp BGRA with straight (not premultiplied) alpha.
    Color,
    /// 32 bpp BGR whose top byte selects: 0 = replace the pixel, 0xFF = XOR it.
    MaskedColor,
}

/// A pointer shape as Desktop Duplication delivers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    pub kind: CursorKind,
    pub width: usize,
    /// Rows of `data` (for [`CursorKind::Monochrome`], both masks).
    pub height: usize,
    /// Bytes per row of `data`.
    pub pitch: usize,
    pub data: Vec<u8>,
}

impl CursorShape {
    /// Height of the drawn pointer.
    pub fn image_height(&self) -> usize {
        if self.kind == CursorKind::Monochrome { self.height / 2 } else { self.height }
    }
}

/// Desktop pixels a drawn pointer covered, to put them back afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPixels {
    x: usize,
    y: usize,
    width: usize,
    pixels: Vec<u8>,
}

/// Draws `cursor` with its top-left at (`x`, `y`) into a BGRA image and
/// returns what it covered (`None` when it is entirely off the image).
pub fn draw_cursor(
    image: &mut [u8],
    stride: usize,
    (width, height): (usize, usize),
    cursor: &CursorShape,
    (x, y): (i32, i32),
) -> Option<SavedPixels> {
    let (cw, ch) = (cursor.width as i64, cursor.image_height() as i64);
    let (x0, y0) = ((x as i64).max(0), (y as i64).max(0));
    let (x1, y1) = ((x as i64 + cw).min(width as i64), (y as i64 + ch).min(height as i64));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
    let mut saved = SavedPixels { x: x0, y: y0, width: x1 - x0, pixels: Vec::with_capacity((x1 - x0) * (y1 - y0) * 4) };
    for row in y0..y1 {
        saved.pixels.extend_from_slice(&image[row * stride + x0 * 4..row * stride + x1 * 4]);
    }
    for row in y0..y1 {
        let cy = (row as i64 - y as i64) as usize;
        for col in x0..x1 {
            let cx = (col as i64 - x as i64) as usize;
            let p = &mut image[row * stride + col * 4..row * stride + col * 4 + 4];
            match cursor.kind {
                CursorKind::Color => {
                    let s = &cursor.data[cy * cursor.pitch + cx * 4..cy * cursor.pitch + cx * 4 + 4];
                    let a = s[3] as u32;
                    for i in 0..3 {
                        p[i] = ((s[i] as u32 * a + p[i] as u32 * (255 - a) + 127) / 255) as u8;
                    }
                }
                CursorKind::MaskedColor => {
                    let s = &cursor.data[cy * cursor.pitch + cx * 4..cy * cursor.pitch + cx * 4 + 4];
                    if s[3] == 0 {
                        p[..3].copy_from_slice(&s[..3]);
                    } else {
                        for i in 0..3 {
                            p[i] ^= s[i];
                        }
                    }
                }
                CursorKind::Monochrome => {
                    let bit = 0x80u8 >> (cx % 8);
                    let and = cursor.data[cy * cursor.pitch + cx / 8] & bit != 0;
                    let xor = cursor.data[(cy + cursor.image_height()) * cursor.pitch + cx / 8] & bit != 0;
                    for c in &mut p[..3] {
                        *c = (if and { *c } else { 0 }) ^ (if xor { 0xFF } else { 0 });
                    }
                }
            }
        }
    }
    Some(saved)
}

/// Puts back what [`draw_cursor`] covered.
pub fn restore_pixels(image: &mut [u8], stride: usize, saved: &SavedPixels) {
    let row_bytes = saved.width * 4;
    for (i, chunk) in saved.pixels.chunks_exact(row_bytes).enumerate() {
        let start = (saved.y + i) * stride + saved.x * 4;
        image[start..start + row_bytes].copy_from_slice(chunk);
    }
}

/// `pixels` of a `width × height` frame as tightly packed NV12 (Y plane,
/// then interleaved UV), what Media Foundation encoders take.
pub fn packed_nv12(pixels: &PixelData, width: usize, height: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let (w, h) = (width, height);
    if w % 2 != 0 || h % 2 != 0 || w == 0 || h == 0 {
        return Err(format!("{w}x{h} is not an even frame size"));
    }
    out.resize(w * h * 3 / 2, 0);
    let (out_y, out_uv) = out.split_at_mut(w * h);
    let short = |what: &str| Err(format!("{what} plane too short for {w}x{h}"));
    match pixels {
        PixelData::Nv12 { y, uv, y_stride, uv_stride } => {
            if *y_stride < w || *uv_stride < w || y.len() < y_stride * (h - 1) + w {
                return short("Y");
            }
            if uv.len() < uv_stride * (h / 2 - 1) + w {
                return short("UV");
            }
            for row in 0..h {
                out_y[row * w..row * w + w].copy_from_slice(&y[row * y_stride..row * y_stride + w]);
            }
            for row in 0..h / 2 {
                out_uv[row * w..row * w + w].copy_from_slice(&uv[row * uv_stride..row * uv_stride + w]);
            }
        }
        PixelData::I420 { y, u, v, y_stride, uv_stride } => {
            let cw = w / 2;
            if *y_stride < w || *uv_stride < cw || y.len() < y_stride * (h - 1) + w {
                return short("Y");
            }
            if u.len() < uv_stride * (h / 2 - 1) + cw || v.len() < uv_stride * (h / 2 - 1) + cw {
                return short("U/V");
            }
            for row in 0..h {
                out_y[row * w..row * w + w].copy_from_slice(&y[row * y_stride..row * y_stride + w]);
            }
            for row in 0..h / 2 {
                let line = &mut out_uv[row * w..row * w + w];
                for col in 0..cw {
                    line[2 * col] = u[row * uv_stride + col];
                    line[2 * col + 1] = v[row * uv_stride + col];
                }
            }
        }
        PixelData::Bgra { data, stride } => {
            if *stride < w * 4 || data.len() < stride * (h - 1) + w * 4 {
                return short("BGRA");
            }
            Nv12Converter::new(w, h, w, h).convert(data, *stride, out_y, out_uv);
        }
        PixelData::Native(_) => return Err("a native surface, not CPU pixels".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter().copied().cycle().take(w * h * 4).collect()
    }

    fn planes(c: &Nv12Converter) -> (Vec<u8>, Vec<u8>) {
        let (y, uv) = c.plane_sizes();
        (vec![0; y], vec![0; uv])
    }

    #[test]
    fn white_black_and_primaries_are_bt709_video_range() {
        let c = Nv12Converter::new(4, 2, 4, 2);
        let (mut y, mut uv) = planes(&c);
        c.convert(&solid(4, 2, [255, 255, 255, 255]), 16, &mut y, &mut uv);
        assert_eq!((y[0], uv[0], uv[1]), (235, 128, 128));
        c.convert(&solid(4, 2, [0, 0, 0, 255]), 16, &mut y, &mut uv);
        assert_eq!((y[0], uv[0], uv[1]), (16, 128, 128));
        // Pure red: BT.709 Y 63, Cb 102, Cr 240 (± rounding).
        c.convert(&solid(4, 2, [0, 0, 255, 255]), 16, &mut y, &mut uv);
        assert!((62..=64).contains(&y[0]) && (101..=103).contains(&uv[0]) && uv[1] == 240, "{y:?} {uv:?}");
    }

    #[test]
    fn same_size_conversion_keeps_every_pixel() {
        // Left half black, right half white, with a stride wider than the row.
        let (w, h, stride) = (8, 4, 40);
        let mut src = vec![0u8; stride * h];
        for row in 0..h {
            for col in 4..8 {
                src[row * stride + col * 4..row * stride + col * 4 + 4].copy_from_slice(&[255; 4]);
            }
        }
        let c = Nv12Converter::new(w, h, w, h);
        let (mut y, mut uv) = planes(&c);
        c.convert(&src, stride, &mut y, &mut uv);
        assert_eq!(&y[..8], &[16, 16, 16, 16, 235, 235, 235, 235]);
        assert_eq!(y[..8], y[24..32]);
        assert!(uv.iter().all(|v| *v == 128));
    }

    #[test]
    fn a_differently_shaped_monitor_is_letterboxed_in_black() {
        // A white 16:9 picture in a 20:9 frame: black bars left and right.
        let c = Nv12Converter::new(1920, 1080, 2400, 1080);
        assert_eq!(c.rect, (240, 0, 1920, 1080));
        let (mut y, mut uv) = planes(&c);
        c.convert(&solid(1920, 1080, [255; 4]), 1920 * 4, &mut y, &mut uv);
        assert_eq!((y[0], y[239], y[240], y[2159], y[2160]), (16, 16, 235, 235, 16));
        assert_eq!(uv[0], 128);
        // A 4K monitor shrinks into a phone-sized frame (with 2×2 averaging).
        let c = Nv12Converter::new(3840, 2160, 2400, 1080);
        assert!(c.average);
        assert_eq!(c.rect, (240, 0, 1920, 1080));
        let (mut y, mut uv) = planes(&c);
        c.convert(&solid(3840, 2160, [0, 0, 0, 255]), 3840 * 4, &mut y, &mut uv);
        assert!(y.iter().all(|v| *v == 16));
    }

    #[test]
    fn threads_produce_the_same_frame_as_one() {
        let (w, h) = (1280, 720);
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let big = Nv12Converter::new(w, h, w, h);
        let (mut y1, mut uv1) = planes(&big);
        big.convert(&src, w * 4, &mut y1, &mut uv1);
        let (mut y2, mut uv2) = planes(&big);
        big.convert_rows(&src, w * 4, 0, &mut y2, &mut uv2);
        assert!(y1 == y2 && uv1 == uv2);
    }

    #[test]
    fn cursors_draw_and_restore() {
        let (w, h) = (8, 8);
        let mut image = solid(w, h, [100, 100, 100, 255]);
        let original = image.clone();
        // A 2×2 colour pointer: opaque red, transparent, half-transparent white, opaque green.
        let color = CursorShape {
            kind: CursorKind::Color,
            width: 2,
            height: 2,
            pitch: 8,
            data: vec![0, 0, 255, 255, 9, 9, 9, 0, 255, 255, 255, 128, 0, 255, 0, 255],
        };
        let saved = draw_cursor(&mut image, w * 4, (w, h), &color, (3, 3)).unwrap();
        let px = |img: &[u8], x: usize, y: usize| img[(y * w + x) * 4..(y * w + x) * 4 + 3].to_vec();
        assert_eq!(px(&image, 3, 3), [0, 0, 255]);
        assert_eq!(px(&image, 4, 3), [100, 100, 100]);
        assert_eq!(px(&image, 3, 4), [178, 178, 178]);
        assert_eq!(px(&image, 4, 4), [0, 255, 0]);
        restore_pixels(&mut image, w * 4, &saved);
        assert_eq!(image, original);

        // Partly off the image (top-left), and entirely off.
        assert!(draw_cursor(&mut image, w * 4, (w, h), &color, (-1, -1)).is_some());
        assert_eq!(px(&image, 0, 0), [0, 255, 0]);
        assert!(draw_cursor(&mut image, w * 4, (w, h), &color, (8, 0)).is_none());
    }

    #[test]
    fn monochrome_and_masked_pointers() {
        let (w, h) = (8, 2);
        // 8×1 monochrome: AND 0b1111_0000 (keep left half), XOR 0b1010_1010.
        let mono = CursorShape { kind: CursorKind::Monochrome, width: 8, height: 2, pitch: 1, data: vec![0xF0, 0xAA] };
        let mut image = solid(w, h, [10, 20, 30, 255]);
        draw_cursor(&mut image, w * 4, (w, h), &mono, (0, 0)).unwrap();
        let first: Vec<u8> = (0..8).map(|x| image[x * 4]).collect();
        // AND 1 keeps the pixel, AND 0 clears it; XOR 1 inverts.
        assert_eq!(first, [!10, 10, !10, 10, 0xFF, 0, 0xFF, 0]);
        assert_eq!(image[w * 4], 10, "the second row is untouched");

        let masked = CursorShape {
            kind: CursorKind::MaskedColor,
            width: 2,
            height: 1,
            pitch: 8,
            data: vec![1, 2, 3, 0x00, 0xFF, 0xFF, 0xFF, 0xFF],
        };
        let mut image = solid(w, h, [10, 20, 30, 255]);
        draw_cursor(&mut image, w * 4, (w, h), &masked, (0, 1)).unwrap();
        assert_eq!(&image[w * 4..w * 4 + 8], &[1, 2, 3, 255, !10, !20, !30, 255]);
    }

    #[test]
    fn encoder_input_from_every_cpu_format() {
        let (w, h) = (4, 2);
        let mut out = Vec::new();
        let i420 = PixelData::I420 { y: vec![50; 8], u: vec![60, 61], v: vec![70, 71], y_stride: 4, uv_stride: 2 };
        packed_nv12(&i420, w, h, &mut out).unwrap();
        assert_eq!(out, [50, 50, 50, 50, 50, 50, 50, 50, 60, 70, 61, 71]);

        let nv12 = PixelData::Nv12 { y: vec![1; 12], uv: vec![2; 6], y_stride: 6, uv_stride: 6 };
        packed_nv12(&nv12, w, h, &mut out).unwrap();
        assert_eq!(out, [1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2]);

        let bgra = PixelData::Bgra { data: vec![255; 32], stride: 16 };
        packed_nv12(&bgra, w, h, &mut out).unwrap();
        assert_eq!(&out[..8], &[235; 8]);

        assert!(packed_nv12(&nv12, 8, 2, &mut out).is_err(), "planes too small");
        assert!(packed_nv12(&i420, 3, 2, &mut out).is_err(), "odd size");
        assert!(packed_nv12(&PixelData::Native(Box::new(1u8)), w, h, &mut out).is_err());
    }
}
