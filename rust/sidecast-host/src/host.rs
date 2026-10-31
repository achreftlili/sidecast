//! What both front ends (the command line and the Windows tray app) do to run
//! a host: pick the backends, load the settings, build the server config, and
//! describe server events in the log.

use std::sync::Arc;

use sidecast_core::protocol::DEFAULT_PORT;
use sidecast_server::backend::{Backends, EncoderFactory};
use sidecast_server::data::{DataDir, HostSettings};
use sidecast_server::{ServerConfig, ServerEvent, ServerHandle};

use crate::options::Options;
use crate::platform::{self, MonitorChoice, PlatformOptions};
use crate::test_pattern;

/// Everything needed to start a server.
pub struct Prepared {
    pub data_dir: DataDir,
    /// The saved settings (defaults in test mode), as a UI edits and saves them.
    pub settings: HostSettings,
    pub config: ServerConfig,
    pub backends: Backends,
    /// Shared with the platform backends: a UI changes the monitor choice here.
    pub platform: PlatformOptions,
    pub test_mode: bool,
}

/// Encoders built into this binary (platform backends add hardware ones first).
pub fn builtin_encoders() -> Vec<Arc<dyn EncoderFactory>> {
    #[cfg(feature = "openh264")]
    return vec![Arc::new(sidecast_server::openh264_encoder::OpenH264Factory)];
    #[cfg(not(feature = "openh264"))]
    Vec::new()
}

/// Loads settings and creates the backends for `options`.
pub fn prepare(options: &Options) -> Result<Prepared, String> {
    let data_dir = options
        .data_dir
        .clone()
        .or_else(DataDir::default_location)
        .map(DataDir::new)
        .ok_or("no data directory: pass --data-dir")?;
    let saved = if options.test_pattern { HostSettings::default() } else { data_dir.load_settings() };
    let mut settings = saved.stream_settings();
    if let Some(fps) = options.fps {
        settings.fps = fps;
    }
    if let Some(mbps) = options.max_mbps {
        settings.max_bitrate_mbps = mbps;
    }

    let platform_options =
        PlatformOptions { monitor: MonitorChoice::new(options.monitor.clone().or_else(|| saved.monitor.clone())) };
    let backends = if options.test_pattern {
        let mut backends = Backends::new(Arc::new(test_pattern::TestPatternDisplay), builtin_encoders());
        backends.input = Some(Arc::new(test_pattern::LoggingInput));
        backends.mic = Some(Arc::new(test_pattern::LoggingMic::new(options.test_mic_active)));
        backends
    } else {
        let mut backends = platform::backends(&platform_options).map_err(|e| e.to_string())?;
        backends.encoders.extend(builtin_encoders());
        backends
    };
    if backends.encoders.is_empty() {
        return Err("no video encoder: build with the `openh264` feature or a platform encoder".into());
    }

    let mut config = ServerConfig::new(
        data_dir.clone(),
        options.port.unwrap_or(if options.test_pattern { DEFAULT_PORT } else { saved.port }),
    );
    config.advertise = options.mdns;
    config.settings = settings;
    config.keep_awake = saved.keep_awake;
    if options.test_pattern {
        // As on the Mac: real interface addresses, but no Tailscale CLI lookup in test mode.
        config.tailscale_lookup = false;
        config.fixed_pairing_code = options.test_pairing_code.clone();
        config.log_stats = true;
        config.server_name =
            Some(options.name.clone().unwrap_or_else(|| format!("{} (test)", sidecast_server::computer_name())));
    } else {
        config.server_name = options.name.clone();
        config.log_stats = options.verbose;
    }
    Ok(Prepared {
        data_dir,
        settings: saved,
        config,
        backends,
        platform: platform_options,
        test_mode: options.test_pattern,
    })
}

/// Logs one server event (the command line's whole user interface).
pub fn log_event(event: &ServerEvent, test_mode: bool) {
    match event {
        ServerEvent::PairingStarted { device_name, code, expires_in, .. } => {
            if test_mode {
                log::info!("event: pairing started with {device_name} — code {code}");
            } else {
                log::info!("PAIRING: enter {code} on {device_name} (valid {} s)", expires_in.as_secs());
            }
        }
        ServerEvent::PairingEnded { paired, .. } => log::info!("event: pairing ended (paired: {paired})"),
        ServerEvent::StreamingStarted { device_name, plan, data_saver } => log::info!(
            "event: streaming to {device_name} ({} {}x{}@{}{})",
            plan.codec,
            plan.encode_width,
            plan.encode_height,
            plan.fps,
            if *data_saver { ", data saver" } else { "" }
        ),
        ServerEvent::StreamingStopped => log::info!("event: streaming stopped"),
        ServerEvent::InputModeChanged(mode) => log::info!("event: input mode {}", mode.as_str()),
        ServerEvent::Listening { .. } | ServerEvent::Stats(_) | ServerEvent::PairedDevicesChanged => {}
    }
}

/// Logs the identity and addresses of a server that just started.
pub fn log_started(server: &ServerHandle, prepared: &Prepared, fixed_code: bool) {
    let mode = if prepared.test_mode { "test mode" } else { "host" };
    let hello = server.server_hello();
    log::info!("{mode}: data dir {}", prepared.data_dir.root().display());
    log::info!("{mode}: {}", sidecast_server::describe_identity(server));
    log::info!("{mode}: platform {} backend {:?}", hello.platform, prepared.backends);
    if fixed_code {
        log::info!("{mode}: fixed pairing code in use");
    }
    let addresses: Vec<String> =
        hello.addresses.unwrap_or_default().iter().map(|a| format!("{} ({})", a.host, a.kind)).collect();
    log::info!("{mode}: SERVER_HELLO addresses {}", addresses.join(", "));
}
