//! `sidecast-host`: the Sidecast host for Windows and Linux, on the command line.
//!
//! With `--test-pattern` it runs the real server (TLS, pairing, flow control,
//! encoder) on a synthetic pattern and logs input instead of injecting it, on
//! any OS including macOS. Without it, it uses this OS's platform backends
//! (Windows today; Linux is a separate pull request). The Windows tray app,
//! `sidecast-tray`, runs the same host with a notification-area icon.

use std::process::ExitCode;
use std::sync::Arc;

use sidecast_host::options::{Options, USAGE};
use sidecast_host::{host, logger, platform};
use sidecast_server::ServerEvent;

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
    let on_event = Arc::new(move |event: ServerEvent| host::log_event(&event, test_mode));
    let server = sidecast_server::start(prepared.config.clone(), prepared.backends.clone(), on_event)
        .await
        .map_err(|e| e.to_string())?;
    host::log_started(&server, &prepared, options.test_pairing_code.is_some());

    wait_for_shutdown_signal().await;
    log::info!("{}: shutting down", if test_mode { "test mode" } else { "host" });
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
