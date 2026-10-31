//! GNOME on Wayland: a virtual monitor from Mutter's ScreenCast
//! `RecordVirtual`, captured over PipeWire, and input through a RemoteDesktop
//! session linked to it (the API gnome-remote-desktop uses). No consent
//! dialog: these D-Bus APIs are open to the user's own session, and GNOME
//! shows its "screen is being shared" indicator while they run.
//!
//! One session per stream: the server drops the previous capture before it
//! starts the next one, and the stream's object path (the display id in
//! [`DisplayGeometry`]) ties the input sink to the capture.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_lite::StreamExt;
use sidecast_core::Size;
use sidecast_core::streaming::StreamPlan;
use sidecast_server::backend::{
    BackendError, CaptureSession, ClientInfo, DisplayBackend, DisplayGeometry, FrameSink, InputBackend, InputSink,
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::capture::{CaptureConfig, PipeWireCapture, SharedCursor};
use super::keymap::TextInput;
use super::remote_input::{RemoteApi, RemoteInputSink};
use super::translate::{RemoteCall, Translator};

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    default_path = "/org/gnome/Mutter/RemoteDesktop"
)]
trait RemoteDesktop {
    fn create_session(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.gnome.Mutter.RemoteDesktop.Session", default_service = "org.gnome.Mutter.RemoteDesktop")]
trait RemoteDesktopSession {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn notify_keyboard_keycode(&self, keycode: u32, state: bool) -> zbus::Result<()>;
    fn notify_keyboard_keysym(&self, keysym: u32, state: bool) -> zbus::Result<()>;
    fn notify_pointer_button(&self, button: i32, state: bool) -> zbus::Result<()>;
    fn notify_pointer_axis(&self, dx: f64, dy: f64, flags: u32) -> zbus::Result<()>;
    fn notify_pointer_axis_discrete(&self, axis: u32, steps: i32) -> zbus::Result<()>;
    fn notify_pointer_motion_absolute(&self, stream: &str, x: f64, y: f64) -> zbus::Result<()>;
    #[zbus(property)]
    fn session_id(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast"
)]
trait ScreenCast {
    fn create_session(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    #[zbus(property)]
    fn version(&self) -> zbus::Result<i32>;
}

#[zbus::proxy(interface = "org.gnome.Mutter.ScreenCast.Session", default_service = "org.gnome.Mutter.ScreenCast")]
trait ScreenCastSession {
    fn record_virtual(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.gnome.Mutter.ScreenCast.Stream", default_service = "org.gnome.Mutter.ScreenCast")]
trait ScreenCastStream {
    #[zbus(signal, name = "PipeWireStreamAdded")]
    fn pipewire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn parameters(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

/// `NotifyPointerAxis` flags (org.gnome.Mutter.RemoteDesktop.Session).
const AXIS_FINISH: u32 = 1;
const AXIS_SOURCE_FINGER: u32 = 4;
/// `RecordVirtual` `cursor-mode`: the pointer as stream metadata, not in the pixels.
const CURSOR_MODE_METADATA: u32 = 2;

/// Resolves to `None` after `duration`.
async fn timeout<T>(duration: Duration, future: impl Future<Output = T>) -> Option<T> {
    futures_lite::future::or(async { Some(future.await) }, async {
        async_io::Timer::after(duration).await;
        None
    })
    .await
}

fn failed(what: &str) -> impl Fn(zbus::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// A started RemoteDesktop session with one virtual-monitor stream.
struct MutterSession {
    remote: RemoteDesktopSessionProxy<'static>,
    stream_path: String,
    node_id: u32,
    /// The virtual monitor's place in the desktop (logical pixels), when Mutter says.
    position: Option<(i32, i32)>,
}

impl MutterSession {
    async fn start(connection: &zbus::Connection) -> Result<MutterSession, String> {
        let path = RemoteDesktopProxy::new(connection)
            .await
            .map_err(failed("Mutter's RemoteDesktop API"))?
            .create_session()
            .await
            .map_err(failed("RemoteDesktop.CreateSession"))?;
        let remote = RemoteDesktopSessionProxy::builder(connection)
            .path(path)
            .map_err(failed("session path"))?
            .build()
            .await
            .map_err(failed("RemoteDesktop session"))?;
        match Self::record(connection, remote.clone()).await {
            Ok(session) => Ok(session),
            Err(error) => {
                let _ = remote.stop().await;
                Err(error)
            }
        }
    }

    async fn record(
        connection: &zbus::Connection,
        remote: RemoteDesktopSessionProxy<'static>,
    ) -> Result<MutterSession, String> {
        let session_id = remote.session_id().await.map_err(failed("RemoteDesktop SessionId"))?;
        let screencast = ScreenCastProxy::new(connection).await.map_err(failed("Mutter's ScreenCast API"))?;
        let version = screencast.version().await.unwrap_or(0);
        let path = screencast
            .create_session(HashMap::from([("remote-desktop-session-id", Value::from(session_id.as_str()))]))
            .await
            .map_err(failed("ScreenCast.CreateSession"))?;
        let session = ScreenCastSessionProxy::builder(connection)
            .path(path)
            .map_err(failed("screencast path"))?
            .build()
            .await
            .map_err(failed("ScreenCast session"))?;
        let stream_path =
            session.record_virtual(HashMap::from([("cursor-mode", Value::U32(CURSOR_MODE_METADATA))])).await.map_err(
                |e| format!("ScreenCast.RecordVirtual (needs GNOME 42 or later; ScreenCast API v{version}): {e}"),
            )?;
        let stream = ScreenCastStreamProxy::builder(connection)
            .path(stream_path.clone())
            .map_err(failed("stream path"))?
            .build()
            .await
            .map_err(failed("ScreenCast stream"))?;
        let mut added = stream.receive_pipewire_stream_added().await.map_err(failed("PipeWireStreamAdded"))?;
        // A linked screencast starts with its RemoteDesktop session.
        remote.start().await.map_err(failed("RemoteDesktop.Start"))?;
        let node_id = match timeout(Duration::from_secs(10), added.next()).await {
            Some(Some(signal)) => *signal.args().map_err(failed("PipeWireStreamAdded"))?.node_id(),
            _ => return Err("Mutter never announced the PipeWire stream".into()),
        };
        let position = stream.parameters().await.ok().and_then(|p| {
            let value = p.get("position")?;
            <(i32, i32)>::try_from(value.try_clone().ok()?).ok()
        });
        Ok(MutterSession { remote, stream_path: stream_path.to_string(), node_id, position })
    }

    fn stop(&self) {
        if let Err(error) = zbus::block_on(self.remote.stop()) {
            log::debug!("mutter: stopping the session: {error}");
        }
    }
}

/// The input side of a running stream, found by its stream path.
struct Active {
    stream_path: String,
    remote: RemoteDesktopSessionProxy<'static>,
    cursor: Arc<SharedCursor>,
    stream_size: (u32, u32),
}

/// The GNOME display and input backends (one value implements both).
pub struct MutterBackend {
    connection: zbus::Connection,
    active: Arc<Mutex<Option<Active>>>,
    text_input: TextInput,
}

impl MutterBackend {
    pub fn new(connection: zbus::Connection, text_input: TextInput) -> Self {
        MutterBackend { connection, active: Arc::default(), text_input }
    }

    /// Whether Mutter's ScreenCast and RemoteDesktop APIs are on the session bus.
    pub fn available(connection: &zbus::Connection) -> bool {
        zbus::block_on(async {
            let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else { return false };
            for name in ["org.gnome.Mutter.ScreenCast", "org.gnome.Mutter.RemoteDesktop"] {
                let Ok(name) = zbus::names::BusName::try_from(name) else { return false };
                if !dbus.name_has_owner(name).await.unwrap_or(false) {
                    return false;
                }
            }
            true
        })
    }
}

impl DisplayBackend for MutterBackend {
    fn name(&self) -> &str {
        "gnome-mutter"
    }

    fn start(
        &self,
        plan: &StreamPlan,
        client: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError> {
        let size = (plan.display.pixel_width() as u32, plan.display.pixel_height() as u32);
        let session = zbus::block_on(MutterSession::start(&self.connection)).map_err(BackendError::display_failed)?;
        let cursor = Arc::new(SharedCursor::default());
        let config = CaptureConfig {
            node_id: session.node_id,
            remote_fd: None,
            size,
            fixed_size: true,
            max_fps: plan.display.refresh_rate.max(plan.fps).clamp(1, 240) as u32,
            encode: (plan.encode_width as usize, plan.encode_height as usize),
        };
        let capture = match PipeWireCapture::start(config, frames, cursor.clone()) {
            Ok(capture) => capture,
            Err(error) => {
                session.stop();
                return Err(BackendError::display_failed(error));
            }
        };
        if capture.size != size {
            log::warn!("mutter: asked for a {}x{} monitor, got {}x{}", size.0, size.1, capture.size.0, capture.size.1);
        }
        log::info!(
            "mutter: virtual monitor {}x{} px for {}{} (stream {}, PipeWire node {})",
            capture.size.0,
            capture.size.1,
            client.device_name,
            session.position.map(|(x, y)| format!(" at ({x}, {y})")).unwrap_or_default(),
            session.stream_path,
            session.node_id
        );
        let (x, y) = session.position.unwrap_or_default();
        let geometry = DisplayGeometry {
            size: Size::new(plan.display.width as f64, plan.display.height as f64),
            origin: sidecast_core::Point::new(x as f64, y as f64),
            id: Some(session.stream_path.clone()),
        };
        *lock(&self.active) = Some(Active {
            stream_path: session.stream_path.clone(),
            remote: session.remote.clone(),
            cursor,
            stream_size: capture.size,
        });
        Ok(Box::new(MutterCapture { active: self.active.clone(), capture: Some(capture), session, geometry }))
    }
}

fn lock(active: &Mutex<Option<Active>>) -> std::sync::MutexGuard<'_, Option<Active>> {
    active.lock().unwrap_or_else(|p| p.into_inner())
}

impl InputBackend for MutterBackend {
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError> {
        let active = lock(&self.active);
        let Some(active) = active.as_ref().filter(|a| geometry.id.as_deref() == Some(a.stream_path.as_str())) else {
            return Err(BackendError::display_failed("no Mutter session for this display"));
        };
        let api = MutterRemote { remote: active.remote.clone(), stream_path: active.stream_path.clone() };
        let translator = Translator::new(geometry.size, active.stream_size, self.text_input);
        Ok(Box::new(RemoteInputSink::new(Box::new(api), translator, active.cursor.clone())?))
    }
}

struct MutterCapture {
    active: Arc<Mutex<Option<Active>>>,
    capture: Option<PipeWireCapture>,
    session: MutterSession,
    geometry: DisplayGeometry,
}

impl CaptureSession for MutterCapture {
    fn geometry(&self) -> DisplayGeometry {
        self.geometry.clone()
    }

    fn request_refresh(&mut self) {
        if let Some(capture) = &self.capture {
            capture.refresh();
        }
    }
}

impl Drop for MutterCapture {
    fn drop(&mut self) {
        {
            let mut active = lock(&self.active);
            if active.as_ref().is_some_and(|a| a.stream_path == self.session.stream_path) {
                *active = None;
            }
        }
        drop(self.capture.take());
        // Stopping the session removes the virtual monitor.
        self.session.stop();
        log::info!("mutter: virtual monitor removed");
    }
}

/// Executes input on Mutter's RemoteDesktop session (positions relative to the stream).
struct MutterRemote {
    remote: RemoteDesktopSessionProxy<'static>,
    stream_path: String,
}

impl RemoteApi for MutterRemote {
    fn call(&self, call: &RemoteCall) -> Result<(), String> {
        let r = &self.remote;
        let result = zbus::block_on(async {
            match *call {
                RemoteCall::MotionAbsolute { x, y } => r.notify_pointer_motion_absolute(&self.stream_path, x, y).await,
                RemoteCall::Button { button, pressed } => r.notify_pointer_button(button, pressed).await,
                RemoteCall::Axis { dx, dy, finish } => {
                    let flags = AXIS_SOURCE_FINGER | if finish { AXIS_FINISH } else { 0 };
                    r.notify_pointer_axis(dx, dy, flags).await
                }
                RemoteCall::AxisDiscrete { axis, steps } => r.notify_pointer_axis_discrete(axis, steps).await,
                RemoteCall::Keycode { keycode, pressed } => r.notify_keyboard_keycode(keycode, pressed).await,
                RemoteCall::Keysym { keysym, pressed } => r.notify_keyboard_keysym(keysym, pressed).await,
            }
        });
        result.map_err(|e| e.to_string())
    }
}
