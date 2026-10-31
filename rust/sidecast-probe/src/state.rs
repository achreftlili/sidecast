//! What a phone remembers about paired hosts (PROTOCOL §5 step 4), in the
//! same JSON shape as the Swift probe's `--state` file.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sidecast_core::security::{SystemRandom, uuid_v4};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    /// Lowercase hex SHA-256 of the host certificate.
    pub fingerprint: String,
    /// Base64 device token.
    pub token: String,
    pub server_name: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeState {
    pub device_id: String,
    #[serde(default)]
    pub servers: BTreeMap<String, Pairing>,
}

impl ProbeState {
    /// The saved state, or a new device identity.
    pub fn load(path: Option<&Path>) -> ProbeState {
        path.and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_else(|| ProbeState { device_id: uuid_v4(&mut SystemRandom), servers: BTreeMap::new() })
    }

    pub fn save(&self, path: Option<&Path>) -> std::io::Result<()> {
        let Some(path) = path else { return Ok(()) };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let data = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_swift_probe_format() {
        let swift = r#"{"deviceId":"D","servers":{"S":{"fingerprint":"ab","host":"127.0.0.1","port":47896,"serverName":"Mac (test)","token":"AAAA"}}}"#;
        let state: ProbeState = serde_json::from_str(swift).unwrap();
        assert_eq!(state.device_id, "D");
        assert_eq!(state.servers["S"].port, 47896);
        let again: ProbeState = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(again, state);
    }
}
