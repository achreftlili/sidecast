//! Paired phones (PROTOCOL §5). The token itself is never stored, only its
//! SHA-256. This is the in-memory model and its JSON form; the server persists it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::protocol::json_b64;
use crate::security::{constant_time_eq, device_token};

/// A phone that completed pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub device_id: String,
    pub device_name: String,
    /// SHA-256 of the device token.
    #[serde(with = "json_b64")]
    pub token_hash: Vec<u8>,
    /// Unix time (seconds).
    pub paired_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    devices: Vec<PairedDevice>,
}

/// The set of paired devices, keyed by `deviceId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairedDevices {
    devices: BTreeMap<String, PairedDevice>,
}

impl PairedDevices {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses the persisted JSON (`{"version":1,"devices":[…]}`).
    pub fn from_json(data: &[u8]) -> Result<PairedDevices, serde_json::Error> {
        let file: FileFormat = serde_json::from_slice(data)?;
        Ok(PairedDevices { devices: file.devices.into_iter().map(|d| (d.device_id.clone(), d)).collect() })
    }

    /// Pretty JSON, oldest pairing first.
    pub fn to_json(&self) -> Vec<u8> {
        let file = FileFormat { version: 1, devices: self.all().into_iter().cloned().collect() };
        serde_json::to_vec_pretty(&file).unwrap_or_default()
    }

    /// Oldest pairing first.
    pub fn all(&self) -> Vec<&PairedDevice> {
        let mut all: Vec<_> = self.devices.values().collect();
        all.sort_by(|a, b| a.paired_at.cmp(&b.paired_at).then_with(|| a.device_id.cmp(&b.device_id)));
        all
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn get(&self, device_id: &str) -> Option<&PairedDevice> {
        self.devices.get(device_id)
    }

    /// Adds or replaces the pairing for `device_id` (re-pairing replaces the token).
    pub fn add(&mut self, device_id: &str, device_name: &str, token: &[u8], now_unix: u64) -> PairedDevice {
        let device = PairedDevice {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            token_hash: device_token::hash(token).to_vec(),
            paired_at: now_unix,
            last_seen_at: Some(now_unix),
        };
        self.devices.insert(device_id.to_owned(), device.clone());
        device
    }

    /// `true` if it was paired.
    pub fn remove(&mut self, device_id: &str) -> bool {
        self.devices.remove(device_id).is_some()
    }

    /// The device if `token` matches its stored hash (constant-time compare).
    pub fn authenticate(&self, device_id: &str, token: &[u8]) -> Option<&PairedDevice> {
        let device = self.devices.get(device_id)?;
        constant_time_eq(&device_token::hash(token), &device.token_hash).then_some(device)
    }

    /// Records a successful connection (and the device's current name).
    /// Returns `false` for an unknown device.
    pub fn mark_seen(&mut self, device_id: &str, name: Option<&str>, now_unix: u64) -> bool {
        let Some(device) = self.devices.get_mut(device_id) else { return false };
        device.last_seen_at = Some(now_unix);
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            device.device_name = name.to_owned();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_authenticate_serialize_and_forget() {
        let token = [0xAA; 32];
        let mut store = PairedDevices::new();
        assert!(store.is_empty());
        store.add("dev-1", "Pixel 8", &token, 100);
        assert_eq!(store.authenticate("dev-1", &token).map(|d| d.device_name.as_str()), Some("Pixel 8"));
        assert!(store.authenticate("dev-1", &[0xAB; 32]).is_none());
        assert!(store.authenticate("dev-2", &token).is_none());

        // Only the hash is serialized.
        let json = String::from_utf8(store.to_json()).unwrap();
        assert!(!json.contains(&crate::protocol::json_b64::encode(&token)));
        assert!(json.contains(&crate::protocol::json_b64::encode(&device_token::hash(&token))));

        let reloaded = PairedDevices::from_json(json.as_bytes()).unwrap();
        assert_eq!(reloaded, store);
        assert!(reloaded.authenticate("dev-1", &token).is_some());

        assert!(store.remove("dev-1"));
        assert!(!store.remove("dev-1"));
        assert!(store.authenticate("dev-1", &token).is_none());
    }

    #[test]
    fn repairing_replaces_the_token() {
        let mut store = PairedDevices::new();
        store.add("d", "A", &[1; 32], 1);
        store.add("d", "B", &[2; 32], 2);
        assert_eq!(store.len(), 1);
        assert!(store.authenticate("d", &[1; 32]).is_none());
        assert_eq!(store.authenticate("d", &[2; 32]).unwrap().device_name, "B");
    }

    #[test]
    fn corrupt_json_is_an_error() {
        assert!(PairedDevices::from_json(b"not json").is_err());
    }

    #[test]
    fn mark_seen_updates_name_and_time() {
        let mut store = PairedDevices::new();
        store.add("d", "Old", &[0; 32], 1);
        assert!(store.mark_seen("d", Some("New name"), 50));
        assert!(!store.mark_seen("other", None, 50));
        assert!(store.mark_seen("d", Some(""), 60));
        let d = store.get("d").unwrap();
        assert_eq!(d.device_name, "New name");
        assert_eq!(d.last_seen_at, Some(60));
    }

    #[test]
    fn all_is_ordered_by_pairing_time() {
        let mut store = PairedDevices::new();
        store.add("z", "Z", &[0; 32], 1);
        store.add("a", "A", &[0; 32], 2);
        assert_eq!(store.all().iter().map(|d| d.device_id.as_str()).collect::<Vec<_>>(), ["z", "a"]);
    }
}
