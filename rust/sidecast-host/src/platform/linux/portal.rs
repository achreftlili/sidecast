//! Other Wayland desktops (KDE Plasma 6, wlroots compositors…): the
//! xdg-desktop-portal's combined RemoteDesktop + ScreenCast session. The user
//! confirms in the portal's dialog once per host run. A *virtual* source
//! (an extra monitor) is requested where the portal offers one; otherwise
//! the user picks a monitor to mirror.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, NotifyKeyboardKeysymOptions,
    NotifyPointerAxisDiscreteOptions, NotifyPointerAxisOptions, NotifyPointerButtonOptions,
    NotifyPointerMotionAbsoluteOptions, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::screencast::{CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use sidecast_core::streaming::StreamPlan;
use sidecast_core::{Point, Size};
use sidecast_server::backend::{
    BackendError, CaptureSession, ClientInfo, DisplayBackend, DisplayGeometry, FrameSink, InputBackend, InputSink,
};

use super::capture::{CaptureConfig, PipeWireCapture, SharedCursor};
use super::keymap::TextInput;
use super::remote_input::{RemoteApi, RemoteInputSink};
use super::translate::{RemoteCall, Translator};

/// A started portal session (shared by the capture and the input sink).
struct PortalSession {
    remote: RemoteDesktop,
    session: Session<RemoteDesktop>,
    node_id: u32,
    /// The stream's size in the portal's (logical) coordinates, used for input.
    logical_size: Option<(u32, u32)>,
    position: Option<(i32, i32)>,
    virtual_source: bool,
}

impl PortalSession {
    async fn start(
        connection: &zbus::Connection,
        restore_token: Option<String>,
    ) -> Result<(PortalSession, OwnedFd, Option<String>), String> {
        let failed = |what: &'static str| move |e: ashpd::Error| format!("portal {what}: {e}");
        let remote = RemoteDesktop::with_connection(connection.clone()).await.map_err(failed("RemoteDesktop"))?;
        let screencast = Screencast::with_connection(connection.clone()).await.map_err(failed("ScreenCast"))?;
        let session = remote.create_session(CreateSessionOptions::default()).await.map_err(failed("CreateSession"))?;
        let result = async {
            remote
                .select_devices(
                    &session,
                    SelectDevicesOptions::default()
                        .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                        .set_persist_mode(PersistMode::Application)
                        .set_restore_token(restore_token.as_deref()),
                )
                .await
                .map_err(failed("SelectDevices"))?;
            let types = screencast.available_source_types().await.unwrap_or(SourceType::Monitor.into());
            let virtual_source = types.contains(SourceType::Virtual);
            let modes = screencast.available_cursor_modes().await.unwrap_or_default();
            let cursor_mode =
                if modes.contains(CursorMode::Metadata) { CursorMode::Metadata } else { CursorMode::Embedded };
            screencast
                .select_sources(
                    &session,
                    SelectSourcesOptions::default()
                        .set_cursor_mode(cursor_mode)
                        .set_sources(BitFlags::from(if virtual_source {
                            SourceType::Virtual
                        } else {
                            SourceType::Monitor
                        }))
                        .set_multiple(false),
                )
                .await
                .map_err(failed("SelectSources"))?;
            log::info!(
                "portal: asking for {} (the desktop may show a dialog)",
                if virtual_source { "a virtual monitor" } else { "a monitor to mirror" }
            );
            let started = remote
                .start(&session, None, StartOptions::default())
                .await
                .map_err(failed("Start"))?
                .response()
                .map_err(failed("Start (declined?)"))?;
            if !started.devices().contains(DeviceType::Pointer) {
                log::warn!("portal: pointer control was not granted");
            }
            let stream = started.streams().first().ok_or("the portal returned no screen")?;
            let fd = screencast
                .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
                .await
                .map_err(failed("OpenPipeWireRemote"))?;
            let logical_size = stream.size().filter(|(w, h)| *w > 0 && *h > 0).map(|(w, h)| (w as u32, h as u32));
            Ok::<_, String>((
                stream.pipe_wire_node_id(),
                logical_size,
                stream.position(),
                fd,
                started.restore_token().map(str::to_owned),
                virtual_source,
            ))
        }
        .await;
        match result {
            Ok((node_id, logical_size, position, fd, token, virtual_source)) => {
                Ok((PortalSession { remote, session, node_id, logical_size, position, virtual_source }, fd, token))
            }
            Err(error) => {
                let _ = session.close().await;
                Err(error)
            }
        }
    }
}

struct Active {
    session: Arc<PortalSession>,
    cursor: Arc<SharedCursor>,
    input_size: (u32, u32),
    id: String,
}

/// The portal display and input backends (one value implements both).
pub struct PortalBackend {
    connection: zbus::Connection,
    active: Arc<Mutex<Option<Active>>>,
    restore_token: Mutex<Option<String>>,
    text_input: TextInput,
}

impl PortalBackend {
    pub fn new(connection: zbus::Connection, text_input: TextInput) -> Self {
        PortalBackend { connection, active: Arc::default(), restore_token: Mutex::default(), text_input }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl DisplayBackend for PortalBackend {
    fn name(&self) -> &str {
        "xdg-desktop-portal"
    }

    fn start(
        &self,
        plan: &StreamPlan,
        client: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError> {
        let token = lock(&self.restore_token).clone();
        let (session, fd, token) =
            zbus::block_on(PortalSession::start(&self.connection, token)).map_err(BackendError::permission_missing)?;
        if token.is_some() {
            *lock(&self.restore_token) = token;
        }
        let session = Arc::new(session);
        let size = (plan.display.pixel_width() as u32, plan.display.pixel_height() as u32);
        let cursor = Arc::new(SharedCursor::default());
        let config = CaptureConfig {
            node_id: session.node_id,
            remote_fd: Some(fd),
            size,
            fixed_size: false,
            max_fps: plan.display.refresh_rate.max(plan.fps).clamp(1, 240) as u32,
            encode: (plan.encode_width as usize, plan.encode_height as usize),
        };
        let capture = match PipeWireCapture::start(config, frames, cursor.clone()) {
            Ok(capture) => capture,
            Err(error) => {
                let _ = zbus::block_on(session.session.close());
                return Err(BackendError::display_failed(error));
            }
        };
        log::info!(
            "portal: {} {}x{} px for {} (PipeWire node {})",
            if session.virtual_source { "virtual monitor" } else { "mirrored monitor" },
            capture.size.0,
            capture.size.1,
            client.device_name,
            session.node_id
        );
        let id = format!("portal-node-{}", session.node_id);
        let (x, y) = session.position.unwrap_or_default();
        let geometry = DisplayGeometry {
            size: Size::new(plan.display.width as f64, plan.display.height as f64),
            origin: Point::new(x as f64, y as f64),
            id: Some(id.clone()),
        };
        *lock(&self.active) = Some(Active {
            session: session.clone(),
            cursor,
            input_size: session.logical_size.unwrap_or(capture.size),
            id: id.clone(),
        });
        Ok(Box::new(PortalCapture { active: self.active.clone(), capture: Some(capture), session, id, geometry }))
    }
}

impl InputBackend for PortalBackend {
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError> {
        let active = lock(&self.active);
        let Some(active) = active.as_ref().filter(|a| geometry.id.as_deref() == Some(a.id.as_str())) else {
            return Err(BackendError::display_failed("no portal session for this display"));
        };
        let translator = Translator::new(geometry.size, active.input_size, self.text_input);
        let api = PortalRemote { session: active.session.clone() };
        Ok(Box::new(RemoteInputSink::new(Box::new(api), translator, active.cursor.clone())?))
    }
}

struct PortalCapture {
    active: Arc<Mutex<Option<Active>>>,
    capture: Option<PipeWireCapture>,
    session: Arc<PortalSession>,
    id: String,
    geometry: DisplayGeometry,
}

impl CaptureSession for PortalCapture {
    fn geometry(&self) -> DisplayGeometry {
        self.geometry.clone()
    }

    fn request_refresh(&mut self) {
        if let Some(capture) = &self.capture {
            capture.refresh();
        }
    }
}

impl Drop for PortalCapture {
    fn drop(&mut self) {
        {
            let mut active = lock(&self.active);
            if active.as_ref().is_some_and(|a| a.id == self.id) {
                *active = None;
            }
        }
        drop(self.capture.take());
        if let Err(error) = zbus::block_on(self.session.session.close()) {
            log::debug!("portal: closing the session: {error}");
        }
        log::info!("portal: session closed");
    }
}

struct PortalRemote {
    session: Arc<PortalSession>,
}

fn key_state(pressed: bool) -> KeyState {
    if pressed { KeyState::Pressed } else { KeyState::Released }
}

impl RemoteApi for PortalRemote {
    fn call(&self, call: &RemoteCall) -> Result<(), String> {
        let (r, s) = (&self.session.remote, &self.session.session);
        let node = self.session.node_id;
        let result = zbus::block_on(async {
            match *call {
                RemoteCall::MotionAbsolute { x, y } => {
                    r.notify_pointer_motion_absolute(s, node, x, y, NotifyPointerMotionAbsoluteOptions::default()).await
                }
                RemoteCall::Button { button, pressed } => {
                    r.notify_pointer_button(s, button, key_state(pressed), NotifyPointerButtonOptions::default()).await
                }
                RemoteCall::Axis { dx, dy, finish } => {
                    r.notify_pointer_axis(s, dx, dy, NotifyPointerAxisOptions::default().set_finish(finish)).await
                }
                RemoteCall::AxisDiscrete { axis, steps } => {
                    let axis = if axis == 0 { Axis::Vertical } else { Axis::Horizontal };
                    r.notify_pointer_axis_discrete(s, axis, steps, NotifyPointerAxisDiscreteOptions::default()).await
                }
                RemoteCall::Keycode { keycode, pressed } => {
                    let options = NotifyKeyboardKeycodeOptions::default();
                    r.notify_keyboard_keycode(s, keycode as i32, key_state(pressed), options).await
                }
                RemoteCall::Keysym { keysym, pressed } => {
                    let options = NotifyKeyboardKeysymOptions::default();
                    r.notify_keyboard_keysym(s, keysym as i32, key_state(pressed), options).await
                }
            }
        });
        result.map_err(|e| e.to_string())
    }
}
