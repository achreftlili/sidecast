//! `sidecast-host`: the Sidecast host for Windows and Linux.
//!
//! With `--test-pattern` it runs the real server (TLS, pairing, flow control,
//! encoder) on a synthetic pattern and logs input instead of injecting it, on
//! any OS including macOS. Without it, it uses this OS's platform backends
//! (separate pull requests; until then it says so and exits).

mod logger;
mod options;
mod platform;
mod test_pattern;

use std::process::ExitCode;
use std::sync::Arc;

use options::{Options, USAGE};
use sidecast_core::protocol::DEFAULT_PORT;
use sidecast_server::backend::{Backends, EncoderFactory};
use sidecast_server::data::DataDir;
use sidecast_server::{ServerConfig, ServerEvent};

#[tokio::main]
async fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) if options.help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    logger::init(options.verbose);
    match run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            log::error!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Encoders built into this binary (platform backends add hardware ones).
fn builtin_encoders() -> Vec<Arc<dyn EncoderFactory>> {
    #[cfg(feature = "openh264")]
    return vec![Arc::new(sidecast_server::openh264_encoder::OpenH264Factory)];
    #[cfg(not(feature = "openh264"))]
    Vec::new()
}

async fn run(options: Options) -> Result<(), String> {
    let data_dir = options
        .data_dir
        .clone()
        .or_else(DataDir::default_location)
        .map(DataDir::new)
        .ok_or("no data directory: pass --data-dir")?;
    let saved = if options.test_pattern { Default::default() } else { data_dir.load_settings() };
    let mut settings = saved.stream_settings();
    if let Some(fps) = options.fps {
        settings.fps = fps;
    }
    if let Some(mbps) = options.max_mbps {
        settings.max_bitrate_mbps = mbps;
    }

    let backends = if options.test_pattern {
        let mut backends = Backends::new(Arc::new(test_pattern::TestPatternDisplay), builtin_encoders());
        backends.input = Some(Arc::new(test_pattern::LoggingInput));
        backends.mic = Some(Arc::new(test_pattern::LoggingMic::new(options.test_mic_active)));
        backends
    } else {
        let mut backends = platform::backends().map_err(|e| e.to_string())?;
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
    let test_mode = options.test_pattern;
    let on_event = Arc::new(move |event: ServerEvent| match event {
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
            if data_saver { ", data saver" } else { "" }
        ),
        ServerEvent::StreamingStopped => log::info!("event: streaming stopped"),
        ServerEvent::InputModeChanged(mode) => log::info!("event: input mode {}", mode.as_str()),
        ServerEvent::Listening { .. } | ServerEvent::Stats(_) | ServerEvent::PairedDevicesChanged => {}
    });

    let mode = if test_mode { "test mode" } else { "host" };
    let server = sidecast_server::start(config, backends.clone(), on_event).await.map_err(|e| e.to_string())?;
    let hello = server.server_hello();
    log::info!("{mode}: data dir {}", data_dir.root().display());
    log::info!("{mode}: {}", sidecast_server::describe_identity(&server));
    log::info!("{mode}: platform {} backend {backends:?}", hello.platform);
    if options.test_pairing_code.is_some() {
        log::info!("{mode}: fixed pairing code in use");
    }
    let addresses: Vec<String> =
        hello.addresses.unwrap_or_default().iter().map(|a| format!("{} ({})", a.host, a.kind)).collect();
    log::info!("{mode}: SERVER_HELLO addresses {}", addresses.join(", "));

    wait_for_shutdown_signal().await;
    log::info!("{mode}: shutting down");
    server.shutdown().await;
    Ok(())
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
