//! Frames of a screencast PipeWire stream (Mutter's virtual monitor or a
//! portal stream) → I420 for the encoder, on a thread running a PipeWire loop.
//!
//! - We offer only CPU-readable formats (32-bit RGB, no DMA-BUF modifiers), so
//!   the compositor hands us SHM / MemFd buffers we can map.
//! - Mutter creates a virtual monitor at the size we negotiate; for the portal
//!   we accept what it offers.
//! - The pointer comes as cursor metadata (position + sprite): it is
//!   composited into the frames and shared with the input sink (CURSOR).
//!   Mutter sends the sprite only when it changes (not for the pointer it
//!   starts with), so a plain arrow stands in until the first one arrives.
//! - Flow control: the latest picture is always kept (a cheap copy), but it is
//!   converted and submitted only when `FrameSink::try_reserve` gives a slot; a
//!   dropped picture is sent as soon as there is room again, so a burst that
//!   ends on a dropped frame still ends on the right picture. Mutter sends
//!   nothing while the screen is static: the latest picture is repeated every
//!   [`IDLE_REPEAT`] so the server's 2 s keyframe rule keeps the phone's
//!   keyframes coming (the Mac host does the same with a coarser watchdog).

/// How often a static picture is repeated: keyframe gaps stay ≤ 2 s + this.
const IDLE_REPEAT: std::time::Duration = std::time::Duration::from_millis(250);

use std::cell::RefCell;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use pw::spa::buffer::meta::MetaCursor;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};
use sidecast_server::backend::{CapturedFrame, FrameSink, PixelData};
use sidecast_server::clock::monotonic_micros;

use super::pixels::{self, CursorSprite, FrameStore, PixelLayout};

/// What to capture and how to deliver it.
#[derive(Debug)]
pub struct CaptureConfig {
    pub node_id: u32,
    /// The portal's PipeWire connection (`OpenPipeWireRemote`); `None`: the
    /// session's PipeWire daemon (Mutter's streams live there).
    pub remote_fd: Option<OwnedFd>,
    /// The size to ask for, in pixels.
    pub size: (u32, u32),
    /// Insist on `size` (Mutter then sizes the virtual monitor to it); otherwise
    /// take what the compositor offers.
    pub fixed_size: bool,
    pub max_fps: u32,
    /// The encoder's frame size.
    pub encode: (usize, usize),
}

/// The pointer as the compositor reports it, shared with the input sink.
#[derive(Debug, Default)]
pub struct SharedCursor {
    state: Mutex<CursorState>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CursorState {
    /// Cursor metadata has arrived at least once.
    pub reported: bool,
    /// Position in stream pixels; `None` while the pointer is on another monitor.
    pub position: Option<(f64, f64)>,
}

impl SharedCursor {
    pub fn get(&self) -> CursorState {
        *self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set(&self, position: Option<(f64, f64)>) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = CursorState { reported: true, position };
    }
}

enum Command {
    Refresh,
    Stop,
}

/// A running capture; dropping it stops the PipeWire stream.
pub struct PipeWireCapture {
    commands: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// The negotiated frame size in pixels.
    pub size: (u32, u32),
}

impl std::fmt::Debug for PipeWireCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipeWireCapture").field("size", &self.size).finish_non_exhaustive()
    }
}

impl PipeWireCapture {
    /// Connects to the node and waits (up to 10 s) for the format negotiation,
    /// after which Mutter's virtual monitor exists.
    pub fn start(config: CaptureConfig, frames: FrameSink, cursor: Arc<SharedCursor>) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (commands, receiver) = pw::channel::channel();
        let thread = std::thread::Builder::new()
            .name("sidecast-pipewire-capture".into())
            .spawn(move || {
                if let Err(error) = run(config, frames, cursor, receiver, ready_tx.clone()) {
                    let _ = ready_tx.send(Err(format!("PipeWire: {error}")));
                }
            })
            .map_err(|e| format!("cannot start the capture thread: {e}"))?;
        let mut capture = PipeWireCapture { commands, thread: Some(thread), size: (0, 0) };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(size)) => {
                capture.size = size;
                Ok(capture)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err("the screencast stream never negotiated a format".into()),
        }
    }

    /// Re-delivers the latest picture (a keyframe request on a static screen).
    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Everything the loop's callbacks share (one thread, so `Rc<RefCell>`).
struct State {
    frames: FrameSink,
    cursor: Arc<SharedCursor>,
    encode: (usize, usize),
    ready: Option<mpsc::Sender<Result<(u32, u32), String>>>,
    size: (u32, u32),
    layout: Option<PixelLayout>,
    picture: Option<FrameStore>,
    sprite: Option<CursorSprite>,
    pointer: Option<(i32, i32)>,
    /// A picture or pointer change not delivered yet (no flow-control slot).
    pending: bool,
    last_submit: Instant,
    warned_unreadable: bool,
}

impl State {
    /// Converts and submits the latest picture if flow control has room.
    fn submit(&mut self) {
        let Some(picture) = self.picture.as_mut() else { return };
        let Some(slot) = self.frames.try_reserve() else {
            self.pending = true;
            return;
        };
        let (width, height) = self.encode;
        let cursor = self.sprite.as_ref().zip(self.pointer).map(|(sprite, (x, y))| (sprite, x, y));
        let planes = picture.with_cursor(cursor, |frame| pixels::to_i420(frame, width, height));
        let (y_stride, uv_stride) = (planes.width, planes.width / 2);
        slot.submit(CapturedFrame {
            pts_micros: monotonic_micros(),
            width: planes.width as u32,
            height: planes.height as u32,
            pixels: PixelData::I420 { y: planes.y, u: planes.u, v: planes.v, y_stride, uv_stride },
        });
        self.pending = false;
        self.last_submit = Instant::now();
    }

    /// A buffer from the stream: cursor metadata and maybe a new picture.
    /// `true` when what is drawn changed.
    fn on_buffer(&mut self, buffer: &mut pw::buffer::Buffer<'_>) -> bool {
        let mut changed = false;
        if let Some(meta) = buffer.find_meta::<MetaCursor>() {
            changed |= self.on_cursor(meta);
        }
        let Some(layout) = self.layout else { return changed };
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        if let Some(data) = buffer.datas_mut().first_mut() {
            let chunk = data.chunk();
            let (offset, size, flags) = (chunk.offset() as usize, chunk.size() as usize, chunk.flags());
            let stride = match chunk.stride() {
                s if s > 0 => s as usize,
                _ => width * 4,
            };
            // Size 0 (or corrupted): a cursor-only update, no new picture.
            if size > 0 && !flags.contains(spa::buffer::ChunkFlags::CORRUPTED) {
                match data.data() {
                    Some(bytes) if offset < bytes.len() => {
                        let bytes = &bytes[offset..bytes.len().min(offset + size)];
                        let stored = match self.picture.as_mut() {
                            Some(picture) => picture.update_from(bytes, width, height, stride, layout),
                            None => {
                                self.picture = FrameStore::copy_from(bytes, width, height, stride, layout);
                                self.picture.is_some()
                            }
                        };
                        changed |= stored;
                    }
                    _ if !self.warned_unreadable => {
                        self.warned_unreadable = true;
                        log::warn!("capture: a {:?} buffer we cannot map; frames skipped", data.type_());
                    }
                    _ => {}
                }
            }
        }
        changed
    }

    /// `true` when what is drawn changed.
    fn on_cursor(&mut self, meta: &MetaCursor) -> bool {
        let before = self.pointer;
        let mut sprite_changed = false;
        // Mutter sets id 0 while the pointer is outside this stream.
        if meta.id() == 0 {
            self.pointer = None;
            self.cursor.set(None);
        } else {
            let p = meta.position();
            self.pointer = Some((p.x, p.y));
            self.cursor.set(Some((p.x as f64, p.y as f64)));
            if let Some(bitmap) = meta.bitmap() {
                let size = bitmap.size();
                let hotspot = meta.hotspot();
                let sprite = match (bitmap_layout(bitmap.format()), bitmap.bitmap_data()) {
                    _ if size.width == 0 || size.height == 0 => None, // a hidden pointer
                    (Some(layout), Some(data)) => CursorSprite::from_bitmap(
                        data,
                        size.width as usize,
                        size.height as usize,
                        bitmap.stride().unsigned_abs() as usize,
                        layout,
                        (hotspot.x, hotspot.y),
                    ),
                    (None, _) => {
                        log::debug!("capture: pointer image in unsupported format {:?}", bitmap.format());
                        None
                    }
                    (Some(_), None) => None,
                };
                log::debug!(
                    "capture: pointer image {}",
                    sprite.as_ref().map_or("hidden".to_owned(), |s| format!("{}x{}", s.width, s.height))
                );
                sprite_changed = sprite != self.sprite;
                self.sprite = sprite;
            }
        }
        if before != self.pointer {
            log::debug!("capture: pointer at {:?} px (cursor metadata)", self.pointer);
        }
        // Only what is drawn: the position matters while there is an image.
        sprite_changed || (before != self.pointer && self.sprite.is_some())
    }

    /// Delivers what flow control dropped, and repeats a static picture.
    fn on_tick(&mut self) {
        if self.pending || self.last_submit.elapsed() >= IDLE_REPEAT {
            self.submit();
        }
    }
}

fn layout_of(format: VideoFormat) -> Option<PixelLayout> {
    match format {
        VideoFormat::BGRx | VideoFormat::BGRA => Some(PixelLayout::Bgrx),
        VideoFormat::RGBx | VideoFormat::RGBA => Some(PixelLayout::Rgbx),
        VideoFormat::xRGB | VideoFormat::ARGB => Some(PixelLayout::Xrgb),
        VideoFormat::xBGR | VideoFormat::ABGR => Some(PixelLayout::Xbgr),
        _ => None,
    }
}

fn bitmap_layout(format: VideoFormat) -> Option<PixelLayout> {
    layout_of(format)
}

fn serialize(object: Object) -> Result<Vec<u8>, String> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("cannot build a PipeWire parameter: {e:?}"))
}

fn range<T: spa::pod::CanonicalFixedSizedPod>(default: T, min: T, max: T) -> Choice<T> {
    Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default, min, max })
}

/// The formats we read: 32-bit RGB in system memory, at `size` (exactly, or as
/// a preference), up to `max_fps` (0/1 = frames on damage, as Mutter likes).
fn enum_format(config: &CaptureConfig) -> Result<Vec<u8>, String> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    let formats = [
        VideoFormat::BGRx,
        VideoFormat::BGRA,
        VideoFormat::RGBx,
        VideoFormat::RGBA,
        VideoFormat::xRGB,
        VideoFormat::xBGR,
        VideoFormat::ARGB,
        VideoFormat::ABGR,
    ];
    let size = Rectangle { width: config.size.0, height: config.size.1 };
    let size = if config.fixed_size {
        Value::Rectangle(size)
    } else {
        Value::Choice(ChoiceValue::Rectangle(range(
            size,
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 },
        )))
    };
    let fps = config.max_fps.max(1);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property::new(FormatProperties::MediaType.as_raw(), Value::Id(Id(MediaType::Video.as_raw()))),
            Property::new(FormatProperties::MediaSubtype.as_raw(), Value::Id(Id(MediaSubtype::Raw.as_raw()))),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                Value::Choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: Id(formats[0].as_raw()),
                        alternatives: formats.iter().map(|f| Id(f.as_raw())).collect(),
                    },
                ))),
            ),
            Property::new(FormatProperties::VideoSize.as_raw(), size),
            Property::new(FormatProperties::VideoFramerate.as_raw(), Value::Fraction(Fraction { num: 0, denom: 1 })),
            Property::new(
                FormatProperties::VideoMaxFramerate.as_raw(),
                Value::Choice(ChoiceValue::Fraction(range(
                    Fraction { num: fps, denom: 1 },
                    Fraction { num: 1, denom: 1 },
                    Fraction { num: fps, denom: 1 },
                ))),
            ),
        ],
    };
    serialize(object)
}

/// After the format: ask for cursor metadata (position + sprite) and the header.
fn meta_params() -> Result<Vec<Vec<u8>>, String> {
    use spa::sys;
    let meta = |kind: u32, size: Value| Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(sys::SPA_PARAM_META_type, Value::Id(Id(kind))),
            Property::new(sys::SPA_PARAM_META_size, size),
        ],
    };
    let cursor_size = |w: usize, h: usize| {
        (std::mem::size_of::<sys::spa_meta_cursor>() + std::mem::size_of::<sys::spa_meta_bitmap>() + w * h * 4) as i32
    };
    Ok(vec![
        serialize(meta(
            sys::SPA_META_Cursor,
            Value::Choice(ChoiceValue::Int(range(cursor_size(64, 64), cursor_size(1, 1), cursor_size(512, 512)))),
        ))?,
        serialize(meta(sys::SPA_META_Header, Value::Int(std::mem::size_of::<sys::spa_meta_header>() as i32)))?,
    ])
}

fn run(
    config: CaptureConfig,
    frames: FrameSink,
    cursor: Arc<SharedCursor>,
    commands: pw::channel::Receiver<Command>,
    ready: mpsc::Sender<Result<(u32, u32), String>>,
) -> Result<(), String> {
    pw::init();
    let error = |e: pw::Error| e.to_string();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(error)?;
    let core = match config.remote_fd.as_ref().map(|fd| fd.try_clone()) {
        Some(Ok(fd)) => context.connect_fd_rc(fd, None),
        Some(Err(e)) => return Err(format!("cannot use the portal's PipeWire connection: {e}")),
        None => context.connect_rc(None),
    }
    .map_err(|e| format!("cannot connect to PipeWire: {e}"))?;

    let state = Rc::new(RefCell::new(State {
        frames,
        cursor,
        encode: config.encode,
        ready: Some(ready),
        size: (0, 0),
        layout: None,
        picture: None,
        // Mutter sends the pointer image only when it changes: until then, a plain arrow.
        sprite: Some(CursorSprite::arrow()),
        pointer: None,
        pending: false,
        last_submit: Instant::now(),
        warned_unreadable: false,
    }));

    let stream = pw::stream::StreamBox::new(
        &core,
        "sidecast-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(error)?;

    let _listener = stream
        .add_local_listener_with_user_data(state.clone())
        .state_changed(|_, state, old, new| {
            log::debug!("capture: PipeWire stream {old:?} → {new:?}");
            if let pw::stream::StreamState::Error(message) = new {
                let mut state = state.borrow_mut();
                match state.ready.take() {
                    Some(ready) => {
                        let _ = ready.send(Err(format!("the screencast stream failed: {message}")));
                    }
                    None => log::error!("capture: the screencast stream failed: {message}"),
                }
            }
        })
        .param_changed(|stream, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }
            let size = (info.size().width, info.size().height);
            let mut state = state.borrow_mut();
            state.layout = layout_of(info.format());
            state.size = size;
            state.picture = None;
            log::info!(
                "capture: PipeWire format {:?} {}x{} (max {}/{} fps)",
                info.format(),
                size.0,
                size.1,
                info.max_framerate().num,
                info.max_framerate().denom
            );
            let ready = state.ready.take();
            match (state.layout, ready) {
                (None, Some(ready)) => {
                    let _ = ready.send(Err(format!("unsupported pixel format {:?}", info.format())));
                }
                (Some(_), Some(ready)) => {
                    let _ = ready.send(Ok(size));
                }
                _ => {}
            }
            match meta_params() {
                Ok(params) => {
                    let mut pods: Vec<&Pod> = params.iter().filter_map(|p| Pod::from_bytes(p)).collect();
                    if let Err(e) = stream.update_params(&mut pods) {
                        log::warn!("capture: cannot ask for cursor metadata: {e}");
                    }
                }
                Err(e) => log::warn!("capture: {e}"),
            }
        })
        .process(|stream, state| {
            // Every queued buffer in order (a cursor-only update may follow a
            // picture), but one conversion for all of them.
            let mut state = state.borrow_mut();
            let mut changed = false;
            while let Some(mut buffer) = stream.dequeue_buffer() {
                changed |= state.on_buffer(&mut buffer);
            }
            if changed {
                state.submit();
            }
        })
        .register()
        .map_err(error)?;

    let format = enum_format(&config)?;
    let mut params = [Pod::from_bytes(&format).ok_or("invalid format parameter")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(config.node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| format!("cannot connect to node {}: {e}", config.node_id))?;

    let tick_state = state.clone();
    let timer = mainloop.loop_().add_timer(move |_| tick_state.borrow_mut().on_tick());
    let interval = Duration::from_millis(1000 / config.max_fps.clamp(10, 120) as u64);
    timer.update_timer(Some(interval), Some(interval));

    let command_state = state.clone();
    let quit = mainloop.downgrade();
    let _commands = commands.attach(mainloop.loop_(), move |command| match command {
        Command::Refresh => command_state.borrow_mut().submit(),
        Command::Stop => {
            if let Some(mainloop) = quit.upgrade() {
                mainloop.quit();
            }
        }
    });

    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}
