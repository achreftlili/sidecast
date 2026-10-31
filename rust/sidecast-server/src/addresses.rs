//! The host's reachable addresses for SERVER_HELLO (PROTOCOL §1.1): LAN and
//! Tailscale interface addresses, plus the Tailscale MagicDNS name when the
//! `tailscale` command line tool is installed.

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sidecast_core::net;
use sidecast_core::protocol::{AddressKind, ServerAddress};

/// This computer's IPv4 addresses worth advertising, LAN first.
pub fn interface_addresses() -> Vec<ServerAddress> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    let found: Vec<ServerAddress> = interfaces
        .iter()
        .filter(|i| i.ip().is_ipv4())
        .filter_map(|i| {
            let ip = i.ip().to_string();
            net::classify(&i.name, &ip).map(|kind| ServerAddress::new(&ip, kind))
        })
        .collect();
    net::ordered(&found)
}

/// The MagicDNS name (`Self.DNSName` of `tailscale status --json`), looked up
/// on a background thread with a 2 s timeout and refreshed every 60 s.
/// No Tailscale simply means no name.
#[derive(Debug, Clone, Default)]
pub struct TailscaleName {
    name: Arc<Mutex<Option<String>>>,
}

impl TailscaleName {
    const CANDIDATES: [&'static str; 5] = [
        "tailscale",
        "/usr/bin/tailscale",
        "/usr/local/bin/tailscale",
        "C:\\Program Files\\Tailscale\\tailscale.exe",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ];

    /// Starts the background refresh; it stops when every clone is dropped.
    pub fn start() -> TailscaleName {
        let lookup = TailscaleName::default();
        let weak = Arc::downgrade(&lookup.name);
        let spawned = std::thread::Builder::new().name("sidecast-tailscale".into()).spawn(move || {
            loop {
                let found = Self::query();
                match weak.upgrade() {
                    Some(slot) => {
                        let mut name = slot.lock().unwrap_or_else(|p| p.into_inner());
                        if *name != found {
                            log::info!("tailscale: MagicDNS name {}", found.as_deref().unwrap_or("none"));
                            *name = found;
                        }
                    }
                    None => return,
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        });
        if let Err(error) = spawned {
            log::warn!("tailscale: cannot start the lookup thread: {error}");
        }
        lookup
    }

    pub fn name(&self) -> Option<String> {
        self.name.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn query() -> Option<String> {
        Self::CANDIDATES.iter().find_map(|cli| {
            let mut child = Command::new(cli)
                .args(["status", "--json"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                }
            }
            let output = child.wait_with_output().ok()?;
            output.status.success().then(|| net::magic_dns_name(&output.stdout)).flatten()
        })
    }
}

/// Interface addresses plus the MagicDNS name, ordered (PROTOCOL §1.1).
pub fn advertised(tailscale: Option<&TailscaleName>) -> Vec<ServerAddress> {
    let mut all = interface_addresses();
    if let Some(name) = tailscale.and_then(TailscaleName::name) {
        all.push(ServerAddress::new(&name, AddressKind::TailscaleDns));
    }
    net::ordered(&all)
}
