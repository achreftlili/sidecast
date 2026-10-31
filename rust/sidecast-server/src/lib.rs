//! The Sidecast host server: TLS listener, pairing and authentication, the
//! session state machine of PROTOCOL §5–§7, flow control and adaptive
//! bitrate, input timing, microphone routing and DNS-SD advertisement.
//!
//! Platform code plugs in through the traits of [`backend`]: a display /
//! capture backend, video encoders, and optional input, microphone and power
//! backends. With the `openh264` feature the crate also provides a software
//! H.264 encoder ([`openh264_encoder::OpenH264Factory`]).
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use sidecast_server::{backend::Backends, data::DataDir, ServerConfig};
//! # async fn run(backends: Backends) -> Result<(), sidecast_server::ServerError> {
//! let config = ServerConfig::new(DataDir::new("/var/lib/sidecast"), 47800);
//! let server = sidecast_server::start(config, backends, Arc::new(|event| println!("{event:?}"))).await?;
//! // … until the user quits:
//! server.shutdown().await;
//! # Ok(()) }
//! ```

pub mod addresses;
pub mod backend;
pub mod clock;
pub mod data;
pub mod events;
mod input;
mod mdns;
#[cfg(feature = "openh264")]
pub mod openh264_encoder;
mod outbox;
mod server;
mod session;
mod stream;
pub mod tls;

pub use events::{ServerEvent, StreamStats};
pub use server::{
    AddressProvider, EventHandler, ServerConfig, ServerError, ServerHandle, computer_name, describe_identity, start,
};
