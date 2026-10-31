//! The Windows tray app (`sidecast-tray`): the host server with an icon in
//! the notification area. Its menu shows the status and the address to add
//! by IP, the screen to stream, whether the phone controls the computer, the
//! paired phones (with Forget), Start at login, the log and Quit. A pairing
//! code appears in a notification and, large, in a small always-on-top
//! window. Logs go to `%LOCALAPPDATA%\Sidecast\sidecast.log`.
//!
//! Threads: the window, menus and notifications live on the main thread's
//! message loop; the server runs on a tokio runtime and reports through
//! [`ServerEvent`]s that are posted to the window.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

mod autostart;
mod icon;
mod pairing;

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use sidecast_server::data::{DataDir, HostSettings};
use sidecast_server::{ServerEvent, ServerHandle};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DispatchMessageW,
    FindWindowW, GetCursorPos, GetMessageW, HICON, HMENU, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION,
    MB_OK, MB_YESNO, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_SHOWNORMAL, SetForegroundWindow,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_NULL, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

use crate::host::{self, Prepared};
use crate::logger;
use crate::options::Options;
use crate::platform::PlatformOptions;
use crate::platform::windows::util::{init_process, pcwstr, wide};
use crate::platform::windows::{display, vdd, vdd_config};

/// The notification-area icon calls back with this message.
const WM_TRAY: u32 = WM_APP + 1;
/// A server event is waiting in the channel.
const WM_SERVER_EVENT: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// The hidden window's class, which `--quit` looks for.
const WINDOW_CLASS: &str = "SidecastTray";

// Menu commands.
const CMD_TOUCH: usize = 10;
const CMD_AUTOSTART: usize = 11;
const CMD_OPEN_LOG: usize = 12;
const CMD_QUIT: usize = 13;
const CMD_GET_DRIVER: usize = 14;
const CMD_SCREEN_AUTO: usize = 100;
const CMD_SCREEN_FIRST: usize = 101;
const CMD_FORGET_FIRST: usize = 300;

/// Runs the tray app until Quit. `--quit` asks a running one to quit (the
/// installer uses it before replacing the files).
pub fn run(args: impl Iterator<Item = String>) -> ExitCode {
    init_process();
    let args: Vec<String> = args.collect();
    if args.iter().any(|a| a == "--quit") {
        return quit_running();
    }
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) => {
            message_box(None, &error.0, MB_ICONERROR);
            return ExitCode::from(2);
        }
    };
    let Some(_instance) = single_instance(options.test_pattern) else {
        message_box(None, "Sidecast is already running: its icon is in the notification area.", MB_ICONINFORMATION);
        return ExitCode::SUCCESS;
    };
    let log_path = log_path();
    if let Err(error) = logger::init_file(options.verbose, &log_path) {
        logger::init(options.verbose);
        log::warn!("tray: cannot write {}: {error}", log_path.display());
    }
    log::info!("tray: Sidecast {} starting (log {})", env!("CARGO_PKG_VERSION"), log_path.display());
    match start(options, log_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            log::error!("tray: {message}");
            message_box(None, &format!("Sidecast could not start:\n\n{message}"), MB_ICONERROR);
            ExitCode::FAILURE
        }
    }
}

fn start(options: Options, log_path: PathBuf) -> Result<(), String> {
    let prepared = host::prepare(&options)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sidecast-runtime")
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    let hwnd = create_window()?;
    // HWND is not Send; the raw value is, and PostMessageW works from any thread.
    let raw_hwnd = hwnd.0 as isize;
    let (events_tx, events_rx) = mpsc::channel();
    let test_mode = prepared.test_mode;
    let on_event = Arc::new(move |event: ServerEvent| {
        host::log_event(&event, test_mode);
        if matches!(event, ServerEvent::Stats(_)) {
            return;
        }
        if events_tx.send(event).is_ok() {
            // SAFETY: posting to a window handle is allowed from any thread;
            // a stale handle only makes the call fail.
            let _ = unsafe { PostMessageW(Some(HWND(raw_hwnd as *mut _)), WM_SERVER_EVENT, WPARAM(0), LPARAM(0)) };
        }
    });
    let server = runtime
        .block_on(sidecast_server::start(prepared.config.clone(), prepared.backends.clone(), on_event))
        .map_err(|e| e.to_string())?;
    host::log_started(&server, &prepared, options.test_pairing_code.is_some());

    let taskbar_created = {
        let name = wide("TaskbarCreated");
        // SAFETY: a NUL-terminated name.
        unsafe { RegisterWindowMessageW(pcwstr(&name)) }
    };
    let app = App::new(hwnd, server, runtime, events_rx, prepared, log_path, taskbar_created);
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        app.add_icon();
        if app.test_mode {
            app.notify("Sidecast (test pattern)", "Streams a test pattern; nothing is captured or injected.");
        }
    });

    let mut msg = MSG::default();
    // SAFETY: the standard message loop of this thread.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if let Some(app) = APP.with(|cell| cell.borrow_mut().take()) {
        app.shut_down();
    }
    Ok(())
}

/// Asks every running tray app to quit (as its Quit item does) and waits up to 10 s.
fn quit_running() -> ExitCode {
    let class = wide(WINDOW_CLASS);
    for _ in 0..50 {
        // SAFETY: a NUL-terminated class name; posting to another process's window is allowed.
        match unsafe { FindWindowW(pcwstr(&class), PCWSTR::null()) } {
            Ok(hwnd) if !hwnd.is_invalid() => {
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            _ => return ExitCode::SUCCESS,
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    ExitCode::FAILURE
}

/// `%LOCALAPPDATA%\Sidecast\sidecast.log`.
fn log_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("Sidecast")
        .join("sidecast.log")
}

/// A per-session mutex: one tray app per user session (a test-pattern one may run beside it).
fn single_instance(test_mode: bool) -> Option<HANDLE> {
    let name = wide(if test_mode { r"Local\SidecastTrayTestPattern" } else { r"Local\SidecastTray" });
    // SAFETY: a NUL-terminated name; the handle lives until the process exits.
    let handle = unsafe { CreateMutexW(None, false, pcwstr(&name)) }.ok()?;
    // SAFETY: reads this thread's last error, set by CreateMutexW.
    (unsafe { GetLastError() } != ERROR_ALREADY_EXISTS).then_some(handle)
}

fn message_box(owner: Option<HWND>, text: &str, style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) {
    let (text, caption) = (wide(text), wide("Sidecast"));
    // SAFETY: NUL-terminated strings that outlive the modal call.
    unsafe { MessageBoxW(owner, pcwstr(&text), pcwstr(&caption), style | MB_OK) };
}

fn create_window() -> Result<HWND, String> {
    let class = wide(WINDOW_CLASS);
    let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), lpszClassName: pcwstr(&class), ..Default::default() };
    // SAFETY: `wc` and the class name are valid for the call (the name is copied).
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err("cannot register the window class".into());
    }
    let title = wide("Sidecast");
    // SAFETY: a registered class and NUL-terminated strings. The window stays
    // hidden: it only receives the icon's and the server's messages.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            pcwstr(&class),
            pcwstr(&title),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(|e| format!("cannot create the window: {e}"))
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app; never while a modal loop (menu, message box) runs,
/// since those dispatch messages back into the window procedure.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Status {
    #[default]
    Waiting,
    Pairing {
        device: String,
        code: String,
    },
    Streaming {
        device: String,
        detail: String,
    },
}

struct App {
    hwnd: HWND,
    server: Option<ServerHandle>,
    runtime: tokio::runtime::Runtime,
    events: Receiver<ServerEvent>,
    data_dir: DataDir,
    settings: HostSettings,
    platform: PlatformOptions,
    test_mode: bool,
    log_path: PathBuf,
    taskbar_created: u32,
    status: Status,
    /// While pairing, the status to go back to.
    before_pairing: Status,
    icons: [Option<HICON>; 2],
    pairing: pairing::PairingWindow,
}

/// What a menu choice asks for (handled after the menu closed).
enum Action {
    Touch,
    Autostart,
    OpenLog,
    GetDriver,
    Quit,
    Screen(Option<String>),
    Forget { id: String, name: String },
}

impl App {
    fn new(
        hwnd: HWND,
        server: ServerHandle,
        runtime: tokio::runtime::Runtime,
        events: Receiver<ServerEvent>,
        prepared: Prepared,
        log_path: PathBuf,
        taskbar_created: u32,
    ) -> App {
        App {
            hwnd,
            server: Some(server),
            runtime,
            events,
            data_dir: prepared.data_dir,
            settings: prepared.settings,
            platform: prepared.platform,
            test_mode: prepared.test_mode,
            log_path,
            taskbar_created,
            status: Status::Waiting,
            before_pairing: Status::Waiting,
            icons: [icon::create(icon::Look::Idle).ok(), icon::create(icon::Look::Streaming).ok()],
            pairing: pairing::PairingWindow::default(),
        }
    }

    fn icon_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            ..Default::default()
        }
    }

    fn tooltip(&self) -> String {
        let name = if self.test_mode { "Sidecast (test pattern)" } else { "Sidecast" };
        match &self.status {
            Status::Waiting => format!("{name}: waiting for a phone"),
            Status::Pairing { device, .. } => format!("{name}: pairing with {device}"),
            Status::Streaming { device, .. } => format!("{name}: streaming to {device}"),
        }
    }

    fn fill_icon(&self, data: &mut NOTIFYICONDATAW) {
        let streaming = matches!(self.status, Status::Streaming { .. });
        data.uFlags |= NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.hIcon = self.icons[usize::from(streaming)].or(self.icons[0]).unwrap_or_default();
        copy_into(&mut data.szTip, &self.tooltip());
    }

    fn add_icon(&self) {
        let mut data = self.icon_data();
        data.uFlags = NIF_MESSAGE;
        data.uCallbackMessage = WM_TRAY;
        self.fill_icon(&mut data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: `data` is a fully initialized NOTIFYICONDATAW for our window.
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                log::warn!("tray: cannot add the notification-area icon");
            }
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    fn update_icon(&self) {
        let mut data = self.icon_data();
        self.fill_icon(&mut data);
        // SAFETY: as in add_icon.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// A notification (a toast on Windows 10/11) from the icon.
    fn notify(&self, title: &str, text: &str) {
        let mut data = self.icon_data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, text);
        // SAFETY: as in add_icon.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn handle_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                ServerEvent::PairingStarted { device_name, code, expires_in, .. } => {
                    let code = code.digits().to_owned();
                    if !matches!(self.status, Status::Pairing { .. }) {
                        self.before_pairing = self.status.clone();
                    }
                    self.status = Status::Pairing { device: device_name.clone(), code: code.clone() };
                    self.pairing.show(&device_name, &code, expires_in.as_secs());
                    self.notify(
                        &format!("Pair {device_name}"),
                        &format!("Enter {} on the phone.", pairing::spaced(&code)),
                    );
                }
                ServerEvent::PairingEnded { paired, .. } => {
                    self.pairing.close();
                    if matches!(self.status, Status::Pairing { .. }) {
                        let device = match &self.status {
                            Status::Pairing { device, .. } => device.clone(),
                            _ => String::new(),
                        };
                        self.status = std::mem::take(&mut self.before_pairing);
                        if paired {
                            self.notify("Phone paired", &format!("{device} can now use this computer as a screen."));
                        }
                    }
                }
                ServerEvent::StreamingStarted { device_name, plan, data_saver } => {
                    self.status = Status::Streaming {
                        device: device_name,
                        detail: format!(
                            "{} {}×{} at {} fps{}",
                            plan.codec,
                            plan.encode_width,
                            plan.encode_height,
                            plan.fps,
                            if data_saver { ", data saver" } else { "" }
                        ),
                    };
                }
                ServerEvent::StreamingStopped => {
                    if matches!(self.status, Status::Streaming { .. }) {
                        self.status = Status::Waiting;
                    }
                    self.before_pairing = Status::Waiting;
                }
                ServerEvent::Listening { .. }
                | ServerEvent::InputModeChanged(_)
                | ServerEvent::Stats(_)
                | ServerEvent::PairedDevicesChanged => {}
            }
        }
        self.update_icon();
    }

    /// The menu, and what each command means.
    fn build_menu(&self) -> (HMENU, Vec<(usize, Action)>) {
        let mut actions = Vec::new();
        // SAFETY: menu creation and items with NUL-terminated strings that
        // outlive each call (AppendMenuW copies them).
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let item = |menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let text = wide(text);
                let _ = AppendMenuW(menu, flags, id, pcwstr(&text));
            };
            let separator = |menu: HMENU| {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            };
            let status = match &self.status {
                Status::Waiting => "Waiting for a phone".to_owned(),
                Status::Pairing { device, code } => format!("Pairing with {device}: code {}", pairing::spaced(code)),
                Status::Streaming { device, detail } => format!("Streaming to {device} ({detail})"),
            };
            item(menu, MF_STRING | MF_GRAYED, 0, &status);
            if let Some(server) = &self.server {
                let hello = server.server_hello();
                let port = server.local_addr().port();
                let address = hello
                    .addresses
                    .unwrap_or_default()
                    .into_iter()
                    .next()
                    .map_or(format!("Port {port}"), |a| format!("Add by IP: {}:{port}", a.host));
                item(menu, MF_STRING | MF_GRAYED, 0, &address);
            }
            separator(menu);

            if !self.test_mode {
                let screens = CreatePopupMenu().unwrap_or_default();
                let chosen = self.platform.monitor.get();
                let check = |selected: bool| if selected { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
                let status = vdd::Status::probe();
                let automatic = if status.usable() {
                    "Automatic: an extra virtual screen"
                } else {
                    "Automatic: a second monitor, else mirror the main one"
                };
                item(screens, MF_STRING | check(chosen.is_none()), CMD_SCREEN_AUTO, automatic);
                actions.push((CMD_SCREEN_AUTO, Action::Screen(None)));
                let monitors: Vec<display::Monitor> =
                    display::monitors().into_iter().filter(|m| m.attached && !m.is_virtual_display()).collect();
                for (i, monitor) in monitors.iter().enumerate() {
                    let id = CMD_SCREEN_FIRST + i;
                    let selected = chosen.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&monitor.device));
                    let label = format!(
                        "Mirror {} ({}×{}{})",
                        monitor.device.trim_start_matches(r"\\.\"),
                        monitor.rect.width,
                        monitor.rect.height,
                        if monitor.primary { ", main" } else { "" }
                    );
                    item(screens, MF_STRING | check(selected), id, &label);
                    actions.push((id, Action::Screen(Some(monitor.device.clone()))));
                }
                separator(screens);
                if status.usable() {
                    item(screens, MF_STRING | MF_GRAYED, 0, "Virtual Display Driver installed");
                } else {
                    item(screens, MF_STRING, CMD_GET_DRIVER, "Get the Virtual Display Driver for an extra screen…");
                    actions.push((CMD_GET_DRIVER, Action::GetDriver));
                }
                item(screens, MF_STRING | MF_GRAYED, 0, "Changes apply to the next connection");
                item(menu, MF_STRING | MF_POPUP, screens.0 as usize, "Screen");
            }

            let touch = if self.settings.touch_enabled { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
            item(menu, MF_STRING | touch, CMD_TOUCH, "Phone controls this computer");
            actions.push((CMD_TOUCH, Action::Touch));

            let phones = CreatePopupMenu().unwrap_or_default();
            let devices = self.server.as_ref().map(ServerHandle::paired_devices).unwrap_or_default();
            if devices.is_empty() {
                item(phones, MF_STRING | MF_GRAYED, 0, "No paired phones");
            }
            for (i, device) in devices.iter().enumerate() {
                let id = CMD_FORGET_FIRST + i;
                item(phones, MF_STRING, id, &format!("Forget {}…", device.device_name));
                actions.push((id, Action::Forget { id: device.device_id.clone(), name: device.device_name.clone() }));
            }
            item(menu, MF_STRING | MF_POPUP, phones.0 as usize, "Paired phones");
            separator(menu);

            let autostart = if autostart::is_enabled() { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
            item(menu, MF_STRING | autostart, CMD_AUTOSTART, "Start at login");
            actions.push((CMD_AUTOSTART, Action::Autostart));
            item(menu, MF_STRING, CMD_OPEN_LOG, "Open log");
            actions.push((CMD_OPEN_LOG, Action::OpenLog));
            item(menu, MF_STRING, CMD_QUIT, "Quit Sidecast");
            actions.push((CMD_QUIT, Action::Quit));
            (menu, actions)
        }
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::Touch => {
                self.settings.touch_enabled = !self.settings.touch_enabled;
                self.save_settings();
                if let Some(server) = &self.server {
                    server.update_settings(self.settings.stream_settings());
                }
            }
            Action::Screen(monitor) => {
                self.settings.monitor = monitor.clone();
                self.platform.monitor.set(monitor);
                self.save_settings();
            }
            Action::Autostart => {
                if let Err(error) = autostart::set_enabled(!autostart::is_enabled()) {
                    log::warn!("tray: {error}");
                }
            }
            Action::OpenLog => open(&self.log_path.display().to_string()),
            Action::GetDriver => open(vdd_config::RELEASES_URL),
            Action::Quit => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
            }
            Action::Forget { id, name } => {
                if let Some(server) = &self.server {
                    match server.forget_device(&id) {
                        Ok(_) => self.notify("Phone forgotten", &format!("{name} has to pair again to connect.")),
                        Err(error) => log::warn!("tray: cannot forget {name}: {error}"),
                    }
                }
            }
        }
    }

    fn save_settings(&self) {
        if self.test_mode {
            return; // test mode never touches the saved settings
        }
        if let Err(error) = self.data_dir.save_settings(&self.settings) {
            log::warn!("tray: cannot save the settings: {error}");
        }
    }

    fn shut_down(mut self) {
        self.pairing.close();
        let data = self.icon_data();
        // SAFETY: removes our icon; the icons are destroyed once.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            for icon in self.icons.iter_mut().filter_map(Option::take) {
                let _ = DestroyIcon(icon);
            }
        }
        if let Some(server) = self.server.take() {
            log::info!("tray: quitting");
            self.runtime.block_on(server.shutdown());
        }
    }
}

fn copy_into(buffer: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(buffer.len() - 1).collect();
    buffer[..units.len()].copy_from_slice(&units);
    buffer[units.len()] = 0;
}

fn open(target: &str) {
    let (verb, file) = (wide("open"), wide(target));
    // SAFETY: NUL-terminated strings for a shell "open".
    unsafe { ShellExecuteW(None, pcwstr(&verb), pcwstr(&file), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
}

/// Shows the menu at the pointer and performs the chosen command.
fn show_menu(hwnd: HWND) {
    let Some((menu, actions)) = with_app(|app| app.build_menu()) else { return };
    let mut at = POINT::default();
    // SAFETY: the classic notification-area menu sequence: the window comes to
    // the foreground so the menu closes when clicking elsewhere, and WM_NULL
    // afterwards lets it close properly. The menu is destroyed once.
    let command = unsafe {
        let _ = GetCursorPos(&mut at);
        let _ = SetForegroundWindow(hwnd);
        let command =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        command.0 as usize
    };
    let Some((_, action)) = actions.into_iter().find(|(id, _)| *id == command && command != 0) else { return };
    if let Action::Forget { name, .. } = &action {
        let text = wide(&format!("Forget {name}? It will have to pair again to connect."));
        let caption = wide("Sidecast");
        // SAFETY: NUL-terminated strings that outlive the modal call.
        let answer = unsafe { MessageBoxW(Some(hwnd), pcwstr(&text), pcwstr(&caption), MB_YESNO | MB_ICONQUESTION) };
        if answer != IDYES {
            return;
        }
    }
    with_app(|app| app.perform(action));
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in the low word of lparam.
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_CONTEXTMENU || event == NIN_SELECT || event == (NIN_SELECT | NINF_KEY) {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_SERVER_EVENT => {
            with_app(App::handle_events);
            LRESULT(0)
        }
        WM_ENDSESSION if wparam.0 != 0 => {
            // Windows is logging off: say goodbye to the phone while we can.
            if let Some(app) = APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut a| a.take())) {
                app.shut_down();
            }
            LRESULT(0)
        }
        // `sidecast-tray --quit` (the installer): quit like the menu's Quit,
        // keeping the window until the icon is removed.
        WM_CLOSE | WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // Explorer restarted: the icon has to be added again.
            if with_app(|app| app.taskbar_created == message).unwrap_or(false) {
                with_app(|app| app.add_icon());
                return LRESULT(0);
            }
            // SAFETY: default handling with the arguments we were given.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}
