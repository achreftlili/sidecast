//! TLS listener, DNS-SD advertisement and session management, ported from
//! the Mac app's `SidecastServer.swift`. Only one phone streams at a time;
//! only an *authenticated* newcomer replaces it.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sidecast_core::protocol::{
    AudioPacket, ByeReason, Hello, HostPlatform, InputMode, MicState, ServerAddress, ServerHello,
};
use sidecast_core::security::{PairingCode, SystemRandom, to_hex};
use sidecast_core::store::PairedDevice;
use sidecast_core::streaming::{EncoderCapability, StreamPlan, StreamPlanner, StreamSettings};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::addresses::{self, TailscaleName};
use crate::backend::{Backends, MicController, SessionId};
use crate::data::{DataDir, PairedDeviceStore};
use crate::events::{ServerEvent, StreamStats};
use crate::mdns::Advertisement;
use crate::session::{self, SessionCommand};
use crate::tls::{TlsError, TlsIdentity};

/// Callback for [`ServerEvent`]s; called from server tasks, keep it short.
pub type EventHandler = Arc<dyn Fn(ServerEvent) + Send + Sync>;
/// SERVER_HELLO `addresses` provider, called for every HELLO.
pub type AddressProvider = Arc<dyn Fn() -> Vec<ServerAddress> + Send + Sync>;

/// How to run a server.
#[derive(Clone)]
pub struct ServerConfig {
    pub data_dir: DataDir,
    /// 0 picks a free port (tests).
    pub port: u16,
    /// IPv4 wildcard by default: phones connect over IPv4 (LAN or Tailscale).
    pub bind: IpAddr,
    /// Shown on the phone; defaults to the computer's name.
    pub server_name: Option<String>,
    /// SERVER_HELLO `platform` and TXT `os`; defaults to the OS this runs on.
    pub platform: HostPlatform,
    /// Advertise `_sidecast._tcp` over DNS-SD.
    pub advertise: bool,
    /// Look up the Tailscale MagicDNS name with the `tailscale` CLI.
    pub tailscale_lookup: bool,
    /// Test mode only: every pairing uses this code. Never set it in a real install.
    pub fixed_pairing_code: Option<PairingCode>,
    pub settings: StreamSettings,
    /// Keep the computer awake while streaming (needs `Backends::power`).
    pub keep_awake: bool,
    /// Log per-second stream statistics.
    pub log_stats: bool,
    /// Replaces the interface/Tailscale address lookup (tests).
    pub addresses: Option<AddressProvider>,
}

impl ServerConfig {
    pub fn new(data_dir: DataDir, port: u16) -> Self {
        ServerConfig {
            data_dir,
            port,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            server_name: None,
            platform: HostPlatform::current().unwrap_or(HostPlatform::Linux),
            advertise: true,
            tailscale_lookup: true,
            fixed_pairing_code: None,
            settings: StreamSettings::default(),
            keep_awake: true,
            log_stats: false,
            addresses: None,
        }
    }
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("data_dir", &self.data_dir)
            .field("port", &self.port)
            .field("platform", &self.platform)
            .field("advertise", &self.advertise)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ServerError {
    Io(std::io::Error),
    Tls(TlsError),
    Bind { port: u16, error: std::io::Error },
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::Io(e) => write!(f, "data directory: {e}"),
            ServerError::Tls(e) => write!(f, "{e}"),
            ServerError::Bind { port, error } => write!(f, "cannot listen on port {port}: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

/// The computer's user-visible name.
pub fn computer_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    let name = name.strip_suffix(".local").unwrap_or(&name).trim().to_owned();
    if name.is_empty() { "Computer".to_owned() } else { name }
}

/// Starts listening; the server runs until [`ServerHandle::shutdown`] (or the runtime ends).
pub async fn start(
    config: ServerConfig,
    backends: Backends,
    on_event: EventHandler,
) -> Result<ServerHandle, ServerError> {
    config.data_dir.ensure().map_err(ServerError::Io)?;
    let server_id = config.data_dir.load_or_create_server_id().map_err(ServerError::Io)?;
    let identity = TlsIdentity::load_or_create(&config.data_dir).map_err(ServerError::Tls)?;
    let acceptor = TlsAcceptor::from(identity.server_config().map_err(ServerError::Tls)?);
    let store = PairedDeviceStore::open(config.data_dir.paired_devices_path());

    let listener = TcpListener::bind(SocketAddr::new(config.bind, config.port))
        .await
        .map_err(|error| ServerError::Bind { port: config.port, error })?;
    let local_addr = listener.local_addr().map_err(ServerError::Io)?;
    let server_name = config.server_name.clone().unwrap_or_else(computer_name);

    let addresses: AddressProvider = match config.addresses.clone() {
        Some(provider) => provider,
        None => {
            let tailscale = config.tailscale_lookup.then(TailscaleName::start);
            Arc::new(move || addresses::advertised(tailscale.as_ref()))
        }
    };
    let encoder_capabilities = backends.encoder_capabilities();
    let hub = Arc::new(Hub {
        server_id: server_id.clone(),
        server_name: server_name.clone(),
        port: local_addr.port(),
        platform: config.platform,
        fingerprint: identity.fingerprint(),
        fixed_pairing_code: config.fixed_pairing_code.clone(),
        store,
        backends,
        encoder_capabilities,
        settings: Mutex::new(config.settings.clone()),
        addresses,
        keep_awake: config.keep_awake,
        log_stats: config.log_stats,
        stream_slot: Arc::new(tokio::sync::Mutex::new(())),
        state: Mutex::new(HubState::default()),
        on_event,
        next_id: AtomicU64::new(1),
    });

    let advertisement = if config.advertise {
        match Advertisement::start(&server_name, local_addr.port(), &server_id, config.platform) {
            Ok(advertisement) => Some(advertisement),
            Err(error) => {
                log::warn!("server: DNS-SD advertisement failed ({error}); phones can still add this computer by IP");
                None
            }
        }
    } else {
        None
    };
    log::info!(
        "server: listening on port {} (TLS{})",
        local_addr.port(),
        if advertisement.is_some() { ", DNS-SD" } else { "" }
    );
    hub.emit(ServerEvent::Listening { port: local_addr.port() });

    let accept_hub = hub.clone();
    let accept_task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((tcp, peer)) => {
                    let _ = tcp.set_nodelay(true);
                    tokio::spawn(session::run(tcp, peer, accept_hub.clone(), acceptor.clone()));
                }
                Err(error) => {
                    log::warn!("server: accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(ServerHandle { hub, accept_task, advertisement, local_addr })
}

/// A running server.
pub struct ServerHandle {
    hub: Arc<Hub>,
    accept_task: JoinHandle<()>,
    advertisement: Option<Advertisement>,
    local_addr: SocketAddr,
}

impl fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerHandle").field("local_addr", &self.local_addr).finish_non_exhaustive()
    }
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn server_id(&self) -> &str {
        &self.hub.server_id
    }

    /// SHA-256 of the TLS certificate, as phones pin it.
    pub fn fingerprint(&self) -> [u8; 32] {
        self.hub.fingerprint
    }

    /// What SERVER_HELLO currently says.
    pub fn server_hello(&self) -> ServerHello {
        self.hub.server_hello()
    }

    pub fn paired_devices(&self) -> Vec<PairedDevice> {
        self.hub.store.all()
    }

    /// Applies new settings to the running stream (new STREAM_CONFIG if its plan changes).
    pub fn update_settings(&self, settings: StreamSettings) {
        self.hub.update_settings(settings);
    }

    /// Forgets a paired phone and disconnects it if it is connected.
    pub fn forget_device(&self, device_id: &str) -> std::io::Result<bool> {
        self.hub.forget_device(device_id)
    }

    /// Sends BYE `server_shutdown` to everyone, stops listening and advertising.
    pub async fn shutdown(self) {
        self.accept_task.abort();
        self.hub.shutdown();
        drop(self.advertisement);
        // Give the BYEs a moment to leave.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

struct SessionEntry {
    commands: UnboundedSender<SessionCommand>,
    device_id: Option<String>,
    mic_started: bool,
}

#[derive(Default)]
struct HubState {
    sessions: HashMap<SessionId, SessionEntry>,
    active: Option<SessionId>,
    pairing: Option<SessionId>,
    shutting_down: bool,
}

/// State shared by every session.
pub(crate) struct Hub {
    server_id: String,
    server_name: String,
    port: u16,
    platform: HostPlatform,
    pub fingerprint: [u8; 32],
    fixed_pairing_code: Option<PairingCode>,
    pub store: PairedDeviceStore,
    pub backends: Backends,
    pub encoder_capabilities: Vec<EncoderCapability>,
    settings: Mutex<StreamSettings>,
    addresses: AddressProvider,
    pub keep_awake: bool,
    pub log_stats: bool,
    /// Held by the stream that owns the display, so a takeover waits for the old one.
    pub stream_slot: Arc<tokio::sync::Mutex<()>>,
    state: Mutex<HubState>,
    on_event: EventHandler,
    next_id: AtomicU64,
}

impl Hub {
    const MAX_SESSIONS: usize = 16;

    fn state(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit(&self, event: ServerEvent) {
        (self.on_event)(event);
    }

    fn command(&self, id: SessionId, command: SessionCommand) {
        if let Some(entry) = self.state().sessions.get(&id) {
            let _ = entry.commands.send(command);
        }
    }

    pub fn register(&self, commands: UnboundedSender<SessionCommand>) -> Option<SessionId> {
        let mut state = self.state();
        if state.shutting_down || state.sessions.len() >= Self::MAX_SESSIONS {
            return None;
        }
        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        state.sessions.insert(id, SessionEntry { commands, device_id: None, mic_started: false });
        Some(id)
    }

    pub fn server_hello(&self) -> ServerHello {
        let addresses = (self.addresses)();
        let mut hello = ServerHello::new(&self.server_id, &self.server_name, self.platform);
        hello.port = Some(self.port.into());
        hello.addresses = (!addresses.is_empty()).then_some(addresses);
        hello
    }

    pub fn next_pairing_code(&self) -> PairingCode {
        self.fixed_pairing_code.clone().unwrap_or_else(|| PairingCode::generate(&mut SystemRandom))
    }

    /// One pairing window at a time: a newer request supersedes the older one.
    pub fn pairing_started(&self, id: SessionId, device_name: &str, code: PairingCode) {
        let previous = self.state().pairing.replace(id);
        if let Some(other) = previous.filter(|other| *other != id) {
            self.command(other, SessionCommand::ExpirePairing);
        }
        self.emit(ServerEvent::PairingStarted {
            session: id,
            device_name: device_name.to_owned(),
            code,
            expires_in: Duration::from_secs_f64(sidecast_core::protocol::PAIRING_LIFETIME),
        });
    }

    pub fn pairing_ended(&self, id: SessionId, paired: bool) {
        {
            let mut state = self.state();
            if state.pairing == Some(id) {
                state.pairing = None;
            }
        }
        self.emit(ServerEvent::PairingEnded { session: id, paired });
        if paired {
            self.emit(ServerEvent::PairedDevicesChanged);
        }
    }

    /// An authenticated session wants the stream: plans it and replaces the current phone.
    pub fn claim_stream(&self, id: SessionId, hello: &Hello) -> Result<(StreamPlan, StreamSettings), ByeReason> {
        if let Err(error) = self.backends.display.preflight() {
            log::error!("server: cannot stream: {error}");
            return Err(error.bye_reason());
        }
        let settings = self.settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let plan = StreamPlanner::plan(
            &hello.screen,
            &hello.codecs,
            &settings,
            hello.limits.as_ref(),
            &self.encoder_capabilities,
        )
        .map_err(|error| {
            log::error!("server: no usable stream for {}: {error}", hello.device_name);
            ByeReason::ProtocolMismatch
        })?;
        let previous = {
            let mut state = self.state();
            if let Some(entry) = state.sessions.get_mut(&id) {
                entry.device_id = Some(hello.device_id.clone());
            }
            state.active.replace(id).filter(|previous| *previous != id)
        };
        if let Some(previous) = previous {
            log::info!("server: {} takes over the stream", hello.device_name);
            self.end_mic(previous);
            self.command(previous, SessionCommand::Bye(ByeReason::Replaced));
        }
        Ok((plan, settings))
    }

    /// The session's first STREAM_CONFIG went out: announce it and offer the microphone.
    pub fn stream_configured(
        &self,
        id: SessionId,
        hello: &Hello,
        plan: StreamPlan,
        mic_control: impl Fn(bool) + Send + Sync + 'static,
    ) {
        let start_mic = {
            let mut state = self.state();
            if state.active != Some(id) {
                return;
            }
            match state.sessions.get_mut(&id) {
                Some(entry) if !entry.mic_started && self.backends.mic.is_some() => {
                    entry.mic_started = true;
                    true
                }
                _ => false,
            }
        };
        self.emit(ServerEvent::StreamingStarted {
            device_name: hello.device_name.clone(),
            plan,
            data_saver: hello.limits.is_some(),
        });
        if start_mic && let Some(mic) = &self.backends.mic {
            let control = MicController { send: Arc::new(mic_control) };
            mic.session_started(id, &hello.device_name, hello.supports_mic(), control);
        }
    }

    fn end_mic(&self, id: SessionId) {
        let started = self.state().sessions.get_mut(&id).is_some_and(|e| std::mem::take(&mut e.mic_started));
        if started && let Some(mic) = &self.backends.mic {
            mic.session_ended(id);
        }
    }

    fn mic_target(&self, id: SessionId) -> bool {
        let state = self.state();
        state.active == Some(id) && state.sessions.get(&id).is_some_and(|e| e.mic_started)
    }

    pub fn audio(&self, id: SessionId, packet: &AudioPacket) {
        if self.mic_target(id)
            && let Some(mic) = &self.backends.mic
        {
            mic.audio(id, packet);
        }
    }

    pub fn mic_state(&self, id: SessionId, state: &MicState) {
        if self.mic_target(id)
            && let Some(mic) = &self.backends.mic
        {
            mic.mic_state(id, state);
        }
    }

    pub fn input_mode_changed(&self, mode: InputMode) {
        self.emit(ServerEvent::InputModeChanged(mode));
    }

    pub fn stats(&self, stats: StreamStats) {
        self.emit(ServerEvent::Stats(stats));
    }

    pub fn session_closed(&self, id: SessionId) {
        self.end_mic(id);
        let was_active = {
            let mut state = self.state();
            state.sessions.remove(&id);
            if state.pairing == Some(id) {
                state.pairing = None;
            }
            let was_active = state.active == Some(id);
            if was_active {
                state.active = None;
            }
            was_active
        };
        if was_active {
            self.emit(ServerEvent::StreamingStopped);
        }
    }

    fn update_settings(&self, settings: StreamSettings) {
        *self.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings.clone();
        let active = self.state().active;
        if let Some(active) = active {
            self.command(active, SessionCommand::Settings(settings));
        }
    }

    fn forget_device(&self, device_id: &str) -> std::io::Result<bool> {
        let removed = self.store.remove(device_id)?;
        let connected: Vec<UnboundedSender<SessionCommand>> = self
            .state()
            .sessions
            .values()
            .filter(|e| e.device_id.as_deref() == Some(device_id))
            .map(|e| e.commands.clone())
            .collect();
        for commands in connected {
            let _ = commands.send(SessionCommand::Bye(ByeReason::UnknownDevice));
        }
        self.emit(ServerEvent::PairedDevicesChanged);
        Ok(removed)
    }

    fn shutdown(&self) {
        let mut state = self.state();
        state.shutting_down = true;
        for entry in state.sessions.values() {
            let _ = entry.commands.send(SessionCommand::Bye(ByeReason::ServerShutdown));
        }
    }
}

/// Logs the identity a server runs with (CLI and test mode).
pub fn describe_identity(handle: &ServerHandle) -> String {
    format!("serverId {} certificate fingerprint {}", handle.server_id(), to_hex(&handle.fingerprint()))
}
