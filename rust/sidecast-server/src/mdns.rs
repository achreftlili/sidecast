//! DNS-SD advertisement of `_sidecast._tcp` with TXT `v=1`, `id=`, `os=`
//! (PROTOCOL §1), through the pure-Rust `mdns-sd` responder.

use mdns_sd::{ServiceDaemon, ServiceInfo};
use sidecast_core::protocol::{HostPlatform, SERVICE_TYPE, txt};

/// A registered service; unregistered when dropped.
pub(crate) struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    pub fn start(
        instance_name: &str,
        port: u16,
        server_id: &str,
        platform: HostPlatform,
    ) -> Result<Advertisement, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let host = format!("{}.local.", dns_label(instance_name));
        let properties: Vec<(&str, String)> = txt::record(server_id, platform).into();
        let info =
            ServiceInfo::new(&format!("{SERVICE_TYPE}.local."), instance_name, &host, (), port, &properties[..])?
                .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info)?;
        Ok(Advertisement { daemon, fullname })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// A DNS host label from a computer name: ASCII letters, digits and hyphens.
fn dns_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(63)
        .collect();
    if label.is_empty() { "sidecast-host".to_owned() } else { label }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dns_label_is_safe() {
        assert_eq!(super::dns_label("Achref's PC (test)"), "achref-s-pc--test");
        assert_eq!(super::dns_label("🖥"), "sidecast-host");
    }
}
