//! `sidecast-host`: the Sidecast host for Windows and Linux, on the command line.
//!
//! With `--test-pattern` it runs the real server (TLS, pairing, flow control,
//! encoder) on a synthetic pattern and logs input instead of injecting it, on
//! any OS including macOS. Without it, it uses this OS's platform backends
//! (Windows and Linux). On Linux it also shows a tray icon and the pairing
//! code as a notification (`--no-tray` to keep only the console). The Windows
//! tray app, `sidecast-tray`, runs the same host with a notification-area icon.

use std::process::ExitCode;
use std::sync::Arc;

use sidecast_host::options::{Options, USAGE};
use sidecast_host::platform::{DesktopUi, UiCommand};
use sidecast_host::{host, logger, platform};
use sidecast_server::{ServerEvent, ServerHandle};

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
    if options.diagnose {
        print!("{}", platform::diagnose());
        return ExitCode::SUCCESS;
    }
    match run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            log::error!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(options: Options) -> Result<(), String> {
    let prepared = host::prepare(&options)?;
    let test_mode = prepared.test_mode;
    let mode = if test_mode { "test mode" } else { "host" };

    // Tray clicks and paired-device changes are handled below, where the server handle lives.
    let (requests, mut pending) = tokio::sync::mpsc::unbounded_channel::<UiCommand>();
    let (devices_changed, mut device_changes) = tokio::sync::mpsc::unbounded_channel::<()>();
    let ui: Option<Arc<dyn DesktopUi>> = if test_mode || options.no_tray {
        None
    } else {
        platform::start_desktop_ui(move |command| {
            let _ = requests.send(command);
        })
    };
    let event_ui = ui.clone();
    let on_event = Arc::new(move |event: ServerEvent| {
        if let Some(ui) = &event_ui {
            ui.server_event(&event);
        }
        if matches!(event, ServerEvent::PairedDevicesChanged) {
            let _ = devices_changed.send(());
        }
        host::log_event(&event, test_mode);
    });
    let server = sidecast_server::start(prepared.config.clone(), prepared.backends.clone(), on_event)
        .await
        .map_err(|e| e.to_string())?;
    host::log_started(&server, &prepared, options.test_pairing_code.is_some());

    let devices = |server: &ServerHandle| {
        server.paired_devices().into_iter().map(|d| (d.device_id, d.device_name)).collect::<Vec<_>>()
    };
    if let Some(ui) = &ui {
        ui.set_devices(devices(&server));
    }
    let shutdown = wait_for_shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            Some(command) = pending.recv() => match command {
                UiCommand::Quit => {
                    log::info!("{mode}: Quit chosen in the tray");
                    break;
                }
                UiCommand::Forget { device_id, name } => match server.forget_device(&device_id) {
                    Ok(true) => log::info!("{mode}: forgot {name}"),
                    Ok(false) => log::info!("{mode}: {name} was not paired"),
                    Err(error) => log::error!("{mode}: cannot forget {name}: {error}"),
                },
            },
            Some(()) = device_changes.recv() => {
                if let Some(ui) = &ui {
                    ui.set_devices(devices(&server));
                }
            }
        }
    }
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
