//! `sidecast-probe`: a scripted fake phone for conformance testing of any
//! Sidecast host (the Rust host, the Mac app, future ones). Exits 0 on PASS,
//! 1 on any protocol violation, 2 on bad arguments.

mod client;
mod decoder;
mod gestures;
mod options;
mod state;
mod stats;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let options = match options::Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    ExitCode::from(client::run(options).await as u8)
}
