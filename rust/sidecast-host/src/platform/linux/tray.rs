//! The tray icon (a StatusNotifierItem with a `com.canonical.dbusmenu` menu:
//! status, paired devices with Forget, Quit) and the pairing code as a
//! desktop notification. Ubuntu shows StatusNotifierItems through its
//! AppIndicator extension; on a GNOME without it there is no icon, and the
//! console log and notifications still work. Implemented directly on zbus
//! (no tray crate) to keep the dependencies permissive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde::Serialize;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, StructureBuilder, Type, Value};

use sidecast_server::ServerEvent;

use super::menu::{self, MenuAction, MenuItem, TrayState};
use crate::platform::{DesktopUi, UiCommand};

/// What the tray and notifications show.
#[derive(Debug, Clone, PartialEq)]
pub enum UiUpdate {
    Status { text: String, streaming: bool },
    PairingStarted { device: String, code: String, expires_in: Duration },
    PairingEnded,
    Devices(Vec<(String, String)>),
}

/// A handle to the tray thread; usable from any thread.
#[derive(Debug)]
pub struct TrayUi {
    updates: Mutex<mpsc::Sender<UiUpdate>>,
    port: AtomicU16,
}

impl TrayUi {
    /// Starts the tray thread. Clicks arrive on `commands`.
    pub fn start(commands: impl Fn(UiCommand) + Send + Sync + 'static) -> TrayUi {
        let (updates, receiver) = mpsc::channel();
        let commands: Arc<dyn Fn(UiCommand) + Send + Sync> = Arc::new(commands);
        let spawned = std::thread::Builder::new().name("sidecast-tray".into()).spawn(move || {
            if let Err(error) = run(receiver, commands) {
                log::warn!("tray: unavailable ({error}); the console keeps working");
            }
        });
        if let Err(error) = spawned {
            log::warn!("tray: cannot start its thread: {error}");
        }
        TrayUi { updates: Mutex::new(updates), port: AtomicU16::new(0) }
    }

    fn update(&self, update: UiUpdate) {
        let _ = self.updates.lock().unwrap_or_else(|p| p.into_inner()).send(update);
    }

    fn waiting(&self) -> UiUpdate {
        let port = self.port.load(Ordering::Relaxed);
        UiUpdate::Status { text: format!("Waiting for a phone (port {port})"), streaming: false }
    }
}

impl DesktopUi for TrayUi {
    fn server_event(&self, event: &ServerEvent) {
        let update = match event {
            ServerEvent::Listening { port } => {
                self.port.store(*port, Ordering::Relaxed);
                self.waiting()
            }
            ServerEvent::PairingStarted { device_name, code, expires_in, .. } => UiUpdate::PairingStarted {
                device: device_name.clone(),
                code: code.digits().to_owned(),
                expires_in: *expires_in,
            },
            ServerEvent::PairingEnded { .. } => UiUpdate::PairingEnded,
            ServerEvent::StreamingStarted { device_name, plan, .. } => UiUpdate::Status {
                text: format!(
                    "Streaming to {device_name} ({} {}×{} at {} fps)",
                    plan.codec, plan.encode_width, plan.encode_height, plan.fps
                ),
                streaming: true,
            },
            ServerEvent::StreamingStopped => self.waiting(),
            ServerEvent::InputModeChanged(_) | ServerEvent::Stats(_) | ServerEvent::PairedDevicesChanged => return,
        };
        self.update(update);
    }

    fn set_devices(&self, devices: Vec<(String, String)>) {
        self.update(UiUpdate::Devices(devices));
    }
}

type Shared = Arc<Mutex<TrayState>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, TrayState> {
    state.lock().unwrap_or_else(|p| p.into_inner())
}

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ICON: &str = "video-display";

/// StatusNotifierItem images: (width, height, ARGB32 bytes).
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
/// (icon name, pixmaps, title, description).
type ToolTip = (String, Pixmaps, String, String);

struct Item {
    state: Shared,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "sidecast"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Sidecast"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        if lock(&self.state).pairing.is_some() { "NeedsAttention" } else { "Active" }
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ICON
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ICON
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        (ICON.into(), Vec::new(), "Sidecast".into(), menu::tooltip(&lock(&self.state)))
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).unwrap_or_default()
    }

    fn activate(&self, _x: i32, _y: i32) {}

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

/// One dbusmenu node: `(ia{sv}av)`.
#[derive(Debug, Serialize, Type)]
struct Layout(i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn properties(item: &MenuItem) -> HashMap<String, OwnedValue> {
    let mut props = HashMap::new();
    let mut put = |key: &str, value: Value<'_>| {
        if let Ok(value) = OwnedValue::try_from(value) {
            props.insert(key.to_owned(), value);
        }
    };
    if item.separator {
        put("type", Value::from("separator"));
    } else {
        put("label", Value::from(item.label.replace('_', "__")));
        put("enabled", Value::from(item.enabled));
    }
    if !item.children.is_empty() {
        put("children-display", Value::from("submenu"));
    }
    props
}

fn layout_value(item: &MenuItem, depth: i32) -> Option<Value<'static>> {
    let children: Vec<Value<'static>> =
        if depth == 0 { Vec::new() } else { item.children.iter().filter_map(|c| layout_value(c, depth - 1)).collect() };
    let props: HashMap<String, Value<'static>> =
        properties(item).into_iter().map(|(k, v)| (k, Value::from(v))).collect();
    StructureBuilder::new().add_field(item.id).add_field(props).add_field(children).build().ok().map(Value::from)
}

struct Menu {
    state: Shared,
    revision: Arc<AtomicU32>,
    commands: Arc<dyn Fn(UiCommand) + Send + Sync>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    fn get_layout(&self, parent_id: i32, recursion_depth: i32, _property_names: Vec<String>) -> (u32, Layout) {
        let root = menu::build_menu(&lock(&self.state));
        let parent = root.find(parent_id).unwrap_or(&root);
        let depth = if recursion_depth < 0 { i32::MAX } else { recursion_depth };
        let children = if depth == 0 {
            Vec::new()
        } else {
            parent
                .children
                .iter()
                .filter_map(|c| layout_value(c, depth - 1))
                .filter_map(|v| OwnedValue::try_from(v).ok())
                .collect()
        };
        (self.revision.load(Ordering::SeqCst), Layout(parent.id, properties(parent), children))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let root = menu::build_menu(&lock(&self.state));
        root.walk()
            .into_iter()
            .filter(|i| ids.is_empty() || ids.contains(&i.id))
            .map(|i| (i.id, properties(i)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let root = menu::build_menu(&lock(&self.state));
        let item = root.find(id).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no menu item {id}")))?;
        let mut props = properties(item);
        props.remove(&name).ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name}")))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        let root = menu::build_menu(&lock(&self.state));
        match root.find(id).and_then(|item| item.action.clone()) {
            Some(MenuAction::Forget { device_id, name }) => (self.commands)(UiCommand::Forget { device_id, name }),
            Some(MenuAction::Quit) => (self.commands)(UiCommand::Quit),
            None => {}
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, data, timestamp) in events {
            self.event(id, event_id, data, timestamp);
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
trait StatusNotifierWatcher {
    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
}

fn run(updates: mpsc::Receiver<UiUpdate>, commands: Arc<dyn Fn(UiCommand) + Send + Sync>) -> zbus::Result<()> {
    let state: Shared = Arc::default();
    let revision = Arc::new(AtomicU32::new(1));
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let connection = zbus::blocking::connection::Builder::session()?
        .name(name.as_str())?
        .serve_at(ITEM_PATH, Item { state: state.clone() })?
        .serve_at(MENU_PATH, Menu { state: state.clone(), revision: revision.clone(), commands })?
        .build()?;
    let notifications = NotificationsProxyBlocking::new(&connection).ok();
    let mut registered = register(&connection, &name);
    let mut notification: Option<u32> = None;
    loop {
        let update = match updates.recv_timeout(Duration::from_secs(10)) {
            Ok(update) => update,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The AppIndicator extension may be enabled later.
                if !registered {
                    registered = register(&connection, &name);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        match &update {
            UiUpdate::PairingStarted { device, code, expires_in } => {
                notification = notify_pairing(notifications.as_ref(), device, code, *expires_in, notification);
            }
            UiUpdate::PairingEnded => {
                if let (Some(proxy), Some(id)) = (notifications.as_ref(), notification.take()) {
                    let _ = proxy.close_notification(id);
                }
            }
            _ => {}
        }
        let menu_changed = {
            let mut state = lock(&state);
            let before = menu::build_menu(&state);
            match update {
                UiUpdate::Status { text, streaming } => (state.status, state.streaming) = (text, streaming),
                UiUpdate::PairingStarted { device, code, .. } => state.pairing = Some((device, code)),
                UiUpdate::PairingEnded => state.pairing = None,
                UiUpdate::Devices(devices) => state.devices = devices,
            }
            menu::build_menu(&state) != before
        };
        emit_changes(&connection, &state, &revision, menu_changed);
    }
}

fn register(connection: &zbus::blocking::Connection, name: &str) -> bool {
    match StatusNotifierWatcherProxyBlocking::new(connection).and_then(|w| w.register_status_notifier_item(name)) {
        Ok(()) => {
            log::info!("tray: icon registered");
            true
        }
        Err(error) => {
            log::debug!("tray: no StatusNotifierWatcher yet ({error})");
            false
        }
    }
}

fn emit_changes(connection: &zbus::blocking::Connection, state: &Shared, revision: &AtomicU32, menu_changed: bool) {
    let server = connection.object_server();
    if let Ok(item) = server.interface::<_, Item>(ITEM_PATH) {
        let status = if lock(state).pairing.is_some() { "NeedsAttention" } else { "Active" };
        let emitter = item.signal_emitter();
        let _ = zbus::block_on(Item::new_tool_tip(emitter));
        let _ = zbus::block_on(Item::new_status(emitter, status));
    }
    if menu_changed && let Ok(menu) = server.interface::<_, Menu>(MENU_PATH) {
        let next = revision.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = zbus::block_on(Menu::layout_updated(menu.signal_emitter(), next, 0));
    }
}

fn notify_pairing(
    proxy: Option<&NotificationsProxyBlocking<'_>>,
    device: &str,
    code: &str,
    expires_in: Duration,
    replaces: Option<u32>,
) -> Option<u32> {
    let proxy = proxy?;
    let summary = format!("Pair {device} with this computer");
    let body = format!("Enter {} on the phone (valid {} s)", menu::format_code(code), expires_in.as_secs());
    let hints = HashMap::from([("urgency", Value::U8(2)), ("desktop-entry", Value::from("sidecast"))]);
    let timeout = expires_in.as_millis().min(i32::MAX as u128) as i32;
    match proxy.notify("Sidecast", replaces.unwrap_or(0), ICON, &summary, &body, &[], hints, timeout) {
        Ok(id) => Some(id),
        Err(error) => {
            log::warn!("tray: cannot show the pairing notification: {error}");
            None
        }
    }
}
