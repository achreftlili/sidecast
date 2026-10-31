//! Screen capture with DXGI Desktop Duplication: the desktop image of one
//! monitor, the pointer drawn in (Desktop Duplication reports it separately),
//! converted to NV12 at the stream's size.
//!
//! Desktop Duplication rather than Windows.Graphics.Capture: it needs no
//! WinRT, draws no yellow capture border on Windows 10, reports exactly when
//! the desktop or the pointer changed, and works with indirect-display
//! (IddCx) monitors like the Virtual Display Driver's. It stops working
//! (`DXGI_ERROR_ACCESS_LOST`) on mode changes, full-screen switches and the
//! secure desktop (UAC prompts, the lock screen); the capture thread then
//! re-creates it, retrying until the monitor is back.
//!
//! Flow control: one frame is delivered per frame interval at most, and only
//! when the desktop or the pointer changed (or a refresh was asked for). A
//! changed desktop is first copied on the GPU (cheap); it is read back and
//! converted only once [`FrameSink::try_reserve`] grants a slot, so dropped
//! frames cost nothing and the latest picture still goes out when the
//! phone catches up.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sidecast_core::streaming::StreamPlan;
use sidecast_server::backend::{BackendError, CapturedFrame, FrameSink, PixelData};
use sidecast_server::clock::monotonic_micros;
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_MAPPED_RECT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, IDXGIAdapter1,
    IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::convert::{CursorKind, CursorShape, Nv12Converter, draw_cursor, restore_pixels};
use super::util::from_wide;

/// A running capture thread; dropping it stops the thread.
pub struct Capture {
    stop: Arc<AtomicBool>,
    refresh: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Starts duplicating `device` (`\\.\DISPLAYn`) into frames of the plan's
    /// encode size at up to the plan's frame rate. Fails when the monitor
    /// cannot be found within a few seconds; a monitor that is only
    /// temporarily unavailable (secure desktop) starts and delivers later.
    pub fn start(device: &str, plan: &StreamPlan, frames: FrameSink) -> Result<Capture, BackendError> {
        let stop = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let capturer = Capturer {
            device: device.to_owned(),
            encode: (plan.encode_width as usize, plan.encode_height as usize),
            interval: Duration::from_secs_f64(1.0 / plan.fps.max(1) as f64),
            frames,
            stop: stop.clone(),
            refresh: refresh.clone(),
            desktop: Vec::new(),
            desktop_size: (0, 0),
            desktop_valid: false,
            converter: None,
            cursor: None,
            cursor_position: (0, 0),
            cursor_visible: false,
            rotation_warned: false,
        };
        let thread = std::thread::Builder::new()
            .name("sidecast-capture".into())
            .spawn(move || capturer.run(ready_tx))
            .map_err(|e| BackendError::display_failed(format!("cannot start the capture thread: {e}")))?;
        let mut capture = Capture { stop, refresh, thread: Some(thread) };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) | Err(RecvTimeoutError::Timeout) => Ok(capture),
            Ok(Err(message)) => {
                capture.shut_down();
                Err(BackendError::display_failed(message))
            }
            Err(RecvTimeoutError::Disconnected) => {
                capture.shut_down();
                Err(BackendError::display_failed("the capture thread stopped"))
            }
        }
    }

    /// Delivers the current picture again (keyframe requests on a still screen).
    pub fn refresh(&self) {
        self.refresh.store(true, Ordering::Relaxed);
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Why a duplication could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenError {
    /// No DXGI output has that device name (yet).
    NotFound(String),
    /// It exists but cannot be duplicated right now (secure desktop, too many duplications).
    Unavailable(String),
}

impl OpenError {
    fn message(&self) -> &str {
        match self {
            OpenError::NotFound(m) | OpenError::Unavailable(m) => m,
        }
    }
}

/// An open duplication of one output and the device it runs on.
struct Duplication {
    duplication: IDXGIOutputDuplication,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    width: usize,
    height: usize,
    /// WARP and some virtual adapters hand out the image in system memory.
    in_system_memory: bool,
    rotated: bool,
}

impl Duplication {
    fn open(device_name: &str) -> Result<Duplication, OpenError> {
        // SAFETY: plain COM factory creation.
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.map_err(|e| OpenError::Unavailable(format!("DXGI: {e}")))?;
        let (adapter, output) = find_output(&factory, device_name)
            .ok_or_else(|| OpenError::NotFound(format!("no DXGI output {device_name}")))?;
        let mut device = None;
        let mut context = None;
        // SAFETY: the out-pointers are valid Options; the device is created on
        // the adapter that drives the output, which duplication requires.
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| OpenError::Unavailable(format!("Direct3D 11 device: {e}")))?;
        let (device, context) =
            device.zip(context).ok_or_else(|| OpenError::Unavailable("Direct3D 11 returned no device".into()))?;
        // DuplicateOutput1 (Windows 10 1703+) also handles HDR / 10-bit desktops
        // by converting to the formats we list; the original API is the fallback.
        // SAFETY: COM calls on live interfaces; the format slice outlives the call.
        let duplication = unsafe {
            match output.cast::<IDXGIOutput5>() {
                Ok(output5) => output5
                    .DuplicateOutput1(&device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])
                    .or_else(|_| output.cast::<IDXGIOutput1>().and_then(|o| o.DuplicateOutput(&device))),
                Err(_) => output.cast::<IDXGIOutput1>().and_then(|o| o.DuplicateOutput(&device)),
            }
        }
        .map_err(|e| {
            let hint = if e.code() == E_ACCESSDENIED { " (secure desktop: UAC prompt or lock screen?)" } else { "" };
            OpenError::Unavailable(format!("cannot duplicate {device_name}: {e}{hint}"))
        })?;
        // SAFETY: COM call on a live interface.
        let desc = unsafe { duplication.GetDesc() };
        Ok(Duplication {
            duplication,
            device,
            context,
            width: desc.ModeDesc.Width as usize,
            height: desc.ModeDesc.Height as usize,
            in_system_memory: desc.DesktopImageInSystemMemory.as_bool(),
            rotated: !matches!(desc.Rotation, DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED),
        })
    }

    fn staging_texture(&self) -> Result<ID3D11Texture2D, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.width as u32,
            Height: self.height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: `desc` describes a CPU-readable texture; no initial data.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .map_err(|e| format!("cannot create the staging texture: {e}"))?;
        texture.ok_or_else(|| "no staging texture".into())
    }
}

/// The adapter and output whose GDI device name is `device_name`.
fn find_output(factory: &IDXGIFactory1, device_name: &str) -> Option<(IDXGIAdapter1, IDXGIOutput)> {
    for a in 0.. {
        // SAFETY: COM enumeration; an error ends it.
        let adapter = unsafe { factory.EnumAdapters1(a) }.ok()?;
        for o in 0.. {
            // SAFETY: as above.
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            // SAFETY: COM call on a live interface.
            if let Ok(desc) = unsafe { output.GetDesc() }
                && from_wide(&desc.DeviceName).eq_ignore_ascii_case(device_name)
            {
                return Some((adapter, output));
            }
        }
    }
    None
}

/// Every DXGI output (adapter description, device name, attached), for `--diagnose`.
pub fn outputs() -> Vec<String> {
    let mut found = Vec::new();
    // SAFETY: plain COM factory creation.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return found };
    for a in 0.. {
        // SAFETY: COM enumeration; an error ends it.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(a) }) else { break };
        // SAFETY: COM call on a live interface.
        let name = unsafe { adapter.GetDesc1() }.map(|d| from_wide(&d.Description)).unwrap_or_default();
        for o in 0.. {
            // SAFETY: as above.
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            // SAFETY: COM call on a live interface.
            if let Ok(desc) = unsafe { output.GetDesc() } {
                let r = desc.DesktopCoordinates;
                found.push(format!(
                    "{} on {name}: {}x{} at ({}, {}){}",
                    from_wide(&desc.DeviceName),
                    r.right - r.left,
                    r.bottom - r.top,
                    r.left,
                    r.top,
                    if desc.AttachedToDesktop.as_bool() { "" } else { ", detached" }
                ));
            }
        }
    }
    found
}

enum Acquired {
    Timeout,
    Frame { info: DXGI_OUTDUPL_FRAME_INFO, resource: IDXGIResource },
}

struct Capturer {
    device: String,
    encode: (usize, usize),
    interval: Duration,
    frames: FrameSink,
    stop: Arc<AtomicBool>,
    refresh: Arc<AtomicBool>,
    /// The latest desktop image, BGRA with stride = width × 4.
    desktop: Vec<u8>,
    desktop_size: (usize, usize),
    desktop_valid: bool,
    converter: Option<Nv12Converter>,
    cursor: Option<CursorShape>,
    /// Top-left of the pointer shape, in the monitor's pixels.
    cursor_position: (i32, i32),
    cursor_visible: bool,
    rotation_warned: bool,
}

impl Capturer {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn run(mut self, ready: SyncSender<Result<(), String>>) {
        let mut ready = Some(ready);
        let started = Instant::now();
        let mut last_error: Option<OpenError> = None;
        while !self.stopped() {
            match Duplication::open(&self.device) {
                Ok(duplication) => {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                    if last_error.take().is_some() {
                        log::info!("capture: {} is available again", self.device);
                    }
                    log::info!(
                        "capture: duplicating {} ({}x{}{}) into {}x{} frames",
                        self.device,
                        duplication.width,
                        duplication.height,
                        if duplication.in_system_memory { ", system memory" } else { "" },
                        self.encode.0,
                        self.encode.1
                    );
                    if let Err(reason) = self.pump(&duplication) {
                        log::info!("capture: {reason}; re-creating the duplication");
                    }
                }
                Err(error) => {
                    // Not found at all after a while: the stream cannot start.
                    // Unavailable (secure desktop): start anyway, frames follow.
                    if started.elapsed() > Duration::from_secs(5)
                        && let Some(ready) = ready.take()
                    {
                        let _ = ready.send(match &error {
                            OpenError::NotFound(message) => Err(message.clone()),
                            OpenError::Unavailable(_) => Ok(()),
                        });
                    }
                    if last_error.as_ref() != Some(&error) {
                        log::warn!("capture: {}; retrying", error.message());
                        last_error = Some(error);
                    }
                    self.sleep(Duration::from_millis(250));
                }
            }
        }
    }

    fn sleep(&self, total: Duration) {
        let until = Instant::now() + total;
        while !self.stopped() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Captures until stopped (`Ok`) or the duplication is lost (`Err`).
    fn pump(&mut self, d: &Duplication) -> Result<(), String> {
        if d.rotated && !self.rotation_warned {
            log::warn!("capture: {} is rotated; it is streamed in its unrotated orientation", self.device);
            self.rotation_warned = true;
        }
        if self.desktop_size != (d.width, d.height) {
            self.desktop_size = (d.width, d.height);
            self.desktop = vec![0; d.width * d.height * 4];
            self.desktop_valid = false;
            self.converter = Some(Nv12Converter::new(d.width, d.height, self.encode.0, self.encode.1));
        }
        let staging = if d.in_system_memory { None } else { Some(d.staging_texture()?) };
        // After a re-creation, show the current picture again.
        let mut pending = self.desktop_valid;
        let mut gpu_copy_pending = false;
        let mut first = true;
        let mut next_due = Instant::now();
        while !self.stopped() {
            if self.refresh.swap(false, Ordering::Relaxed) {
                pending = true;
            }
            let now = Instant::now();
            let wait = if pending { next_due.saturating_duration_since(now) } else { Duration::from_millis(100) };
            match acquire(&d.duplication, wait)? {
                Acquired::Timeout => {}
                Acquired::Frame { info, resource } => {
                    let copied = self.take_frame(d, &info, &resource, staging.as_ref(), first);
                    first = false;
                    // SAFETY: the frame acquired above is released exactly once.
                    let released = unsafe { d.duplication.ReleaseFrame() };
                    let changed = copied?;
                    gpu_copy_pending |= changed.desktop && staging.is_some();
                    pending |= changed.desktop || changed.pointer;
                    if let Err(error) = released
                        && error.code() == DXGI_ERROR_ACCESS_LOST
                    {
                        return Err("desktop duplication lost".into());
                    }
                }
            }
            if pending && Instant::now() >= next_due {
                next_due = (next_due + self.interval).max(Instant::now());
                // One reservation per frame interval: a refused one is one dropped frame.
                if let Some(slot) = self.frames.try_reserve() {
                    if gpu_copy_pending && let Some(staging) = &staging {
                        self.read_back(d, staging)?;
                        gpu_copy_pending = false;
                    }
                    if self.desktop_valid {
                        slot.submit(self.compose());
                    }
                    pending = false;
                }
            }
        }
        Ok(())
    }

    /// Takes what an acquired frame says: a new desktop image (copied on the
    /// GPU into `staging`, or straight to `desktop` when it is in system
    /// memory) and pointer changes.
    fn take_frame(
        &mut self,
        d: &Duplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: &IDXGIResource,
        staging: Option<&ID3D11Texture2D>,
        first: bool,
    ) -> Result<Changes, String> {
        let mut changes = Changes::default();
        if info.LastPresentTime != 0 || info.AccumulatedFrames > 0 || first {
            changes.desktop = true;
            match staging {
                Some(staging) => {
                    let texture: ID3D11Texture2D =
                        resource.cast().map_err(|e| format!("the desktop image is not a texture: {e}"))?;
                    // SAFETY: both textures live on `d.device` and have the same size and format.
                    unsafe { d.context.CopyResource(staging, &texture) };
                }
                None => self.copy_system_memory(d)?,
            }
        }
        if info.LastMouseUpdateTime != 0 {
            let p = info.PointerPosition;
            let visible = p.Visible.as_bool();
            let position = (p.Position.x, p.Position.y);
            changes.pointer = visible != self.cursor_visible || (visible && position != self.cursor_position);
            self.cursor_visible = visible;
            self.cursor_position = position;
        }
        if info.PointerShapeBufferSize > 0 {
            let mut data = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut required = 0u32;
            let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            // SAFETY: `data` is as large as the size DXGI reported for this frame.
            let fetched = unsafe {
                d.duplication.GetFramePointerShape(
                    data.len() as u32,
                    data.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape,
                )
            };
            if fetched.is_ok() {
                let kind = match shape.Type as i32 {
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 => Some(CursorKind::Monochrome),
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 => Some(CursorKind::Color),
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 => Some(CursorKind::MaskedColor),
                    _ => None,
                };
                let (width, height, pitch) = (shape.Width as usize, shape.Height as usize, shape.Pitch as usize);
                let bytes_needed = pitch * height;
                self.cursor = kind.filter(|_| bytes_needed <= data.len() && width > 0 && height > 0).map(|kind| {
                    data.truncate(bytes_needed);
                    CursorShape { kind, width, height, pitch, data }
                });
                changes.pointer = true;
            }
        }
        Ok(changes)
    }

    /// The desktop image of a system-memory duplication, straight into `desktop`.
    fn copy_system_memory(&mut self, d: &Duplication) -> Result<(), String> {
        // SAFETY: only valid while the frame is held, which it is (released by the caller after this).
        let mapped: DXGI_MAPPED_RECT =
            unsafe { d.duplication.MapDesktopSurface() }.map_err(|e| format!("cannot map the desktop: {e}"))?;
        let pitch = mapped.Pitch as usize;
        // SAFETY: the mapping covers `height` rows of `pitch` bytes until UnMapDesktopSurface.
        let source = unsafe { std::slice::from_raw_parts(mapped.pBits, pitch * d.height) };
        copy_rows(source, pitch, &mut self.desktop, d.width, d.height);
        self.desktop_valid = true;
        // SAFETY: matches the successful MapDesktopSurface above.
        let _ = unsafe { d.duplication.UnMapDesktopSurface() };
        Ok(())
    }

    /// Reads the staging texture back into `desktop`.
    fn read_back(&mut self, d: &Duplication, staging: &ID3D11Texture2D) -> Result<(), String> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: a CPU-readable staging texture of this device; unmapped below.
        unsafe { d.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .map_err(|e| format!("cannot read the desktop image back: {e}"))?;
        let pitch = mapped.RowPitch as usize;
        // SAFETY: the mapping covers `height` rows of `RowPitch` bytes until Unmap.
        let source = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, pitch * d.height) };
        copy_rows(source, pitch, &mut self.desktop, d.width, d.height);
        // SAFETY: matches the successful Map above.
        unsafe { d.context.Unmap(staging, 0) };
        self.desktop_valid = true;
        Ok(())
    }

    /// The current desktop with the pointer, as an NV12 frame.
    fn compose(&mut self) -> CapturedFrame {
        let (w, h) = self.desktop_size;
        let saved = match (&self.cursor, self.cursor_visible) {
            (Some(cursor), true) => draw_cursor(&mut self.desktop, w * 4, (w, h), cursor, self.cursor_position),
            _ => None,
        };
        let converter = self.converter.as_ref().expect("a converter exists once the desktop has a size");
        let (y_len, uv_len) = converter.plane_sizes();
        let (mut y, mut uv) = (vec![0u8; y_len], vec![0u8; uv_len]);
        converter.convert(&self.desktop, w * 4, &mut y, &mut uv);
        if let Some(saved) = saved {
            restore_pixels(&mut self.desktop, w * 4, &saved);
        }
        let (fw, fh) = converter.frame_size();
        CapturedFrame {
            pts_micros: monotonic_micros(),
            width: fw as u32,
            height: fh as u32,
            pixels: PixelData::Nv12 { y, uv, y_stride: fw, uv_stride: fw },
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Changes {
    desktop: bool,
    pointer: bool,
}

fn copy_rows(source: &[u8], pitch: usize, desktop: &mut [u8], width: usize, height: usize) {
    let row = width * 4;
    for (y, line) in desktop.chunks_exact_mut(row).take(height).enumerate() {
        line.copy_from_slice(&source[y * pitch..y * pitch + row]);
    }
}

/// Waits up to `wait` for a new desktop frame or pointer change.
fn acquire(duplication: &IDXGIOutputDuplication, wait: Duration) -> Result<Acquired, String> {
    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource = None;
    let millis = wait.as_millis().min(u32::MAX as u128) as u32;
    // SAFETY: valid out-pointers; the caller releases every acquired frame.
    match unsafe { duplication.AcquireNextFrame(millis, &mut info, &mut resource) } {
        Ok(()) => match resource {
            Some(resource) => Ok(Acquired::Frame { info, resource }),
            None => Err("no desktop resource in the acquired frame".into()),
        },
        Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => Ok(Acquired::Timeout),
        Err(error) if error.code() == DXGI_ERROR_ACCESS_LOST => {
            Err("desktop duplication lost (mode change, full-screen app or secure desktop)".into())
        }
        Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => Err("the monitor is gone".into()),
        Err(error) => Err(format!("cannot acquire a frame: {error}")),
    }
}
