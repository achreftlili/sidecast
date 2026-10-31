//! What a host persists in its data directory: the server id, the TLS
//! identity, settings and paired phones. Files are written atomically and,
//! on Unix, readable by the owner only.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sidecast_core::protocol::DEFAULT_PORT;
use sidecast_core::security::{SystemRandom, uuid_v4};
use sidecast_core::store::{PairedDevice, PairedDevices};
use sidecast_core::streaming::StreamSettings;

use crate::clock::unix_now;

/// The host's data directory.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DataDir { root: root.into() }
    }

    /// The per-user default: `%APPDATA%\Sidecast` on Windows,
    /// `$XDG_DATA_HOME/sidecast` (or `~/.local/share/sidecast`) on Linux. On
    /// macOS (development only) a folder distinct from the Mac app's.
    pub fn default_location() -> Option<PathBuf> {
        let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        if cfg!(target_os = "windows") {
            env("APPDATA").map(|p| p.join("Sidecast"))
        } else if cfg!(target_os = "macos") {
            env("HOME").map(|p| p.join("Library/Application Support/Sidecast Rust Host"))
        } else {
            env("XDG_DATA_HOME")
                .map(|p| p.join("sidecast"))
                .or_else(|| env("HOME").map(|p| p.join(".local/share/sidecast")))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn server_id_path(&self) -> PathBuf {
        self.root.join("server-id")
    }

    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn paired_devices_path(&self) -> PathBuf {
        self.root.join("paired-devices.json")
    }

    pub fn certificate_path(&self) -> PathBuf {
        self.root.join("tls-certificate.der")
    }

    pub fn private_key_path(&self) -> PathBuf {
        self.root.join("tls-private-key.der")
    }

    /// Creates the directory (owner-only on Unix).
    pub fn ensure(&self) -> io::Result<()> {
        create_private_dir(&self.root)
    }

    /// Stable UUID identifying this install (TXT `id=`, SERVER_HELLO `serverId`).
    pub fn load_or_create_server_id(&self) -> io::Result<String> {
        if let Ok(text) = fs::read_to_string(self.server_id_path()) {
            let id = text.trim();
            if is_uuid(id) {
                return Ok(id.to_owned());
            }
        }
        let id = uuid_v4(&mut SystemRandom);
        write_private(&self.server_id_path(), id.as_bytes())?;
        Ok(id)
    }

    /// Saved settings, or defaults when missing or unreadable.
    pub fn load_settings(&self) -> HostSettings {
        fs::read(self.settings_path())
            .ok()
            .and_then(|data| serde_json::from_slice::<HostSettings>(&data).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save_settings(&self, settings: &HostSettings) -> io::Result<()> {
        let data = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
        write_private(&self.settings_path(), &data)
    }
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Writes `data` to `path` atomically (temporary file + rename) with
/// owner-only permissions on Unix. On Windows the per-user data directory
/// already restricts access.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid_v4(&mut SystemRandom)));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// User settings of a host (`settings.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HostSettings {
    pub hi_dpi: bool,
    pub resolution_scale: f64,
    pub fps: i64,
    pub max_bitrate_mbps: i64,
    /// "Phone controls the computer": TOUCH, TEXT and KEY.
    pub touch_enabled: bool,
    pub port: u16,
    pub keep_awake: bool,
    pub mic_enabled: bool,
    /// Hosts that can also stream an existing monitor (Windows without a
    /// virtual display driver): which one, by the platform's name for it.
    /// `None` = automatic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<String>,
}

impl Default for HostSettings {
    fn default() -> Self {
        let stream = StreamSettings::default();
        HostSettings {
            hi_dpi: stream.hi_dpi,
            resolution_scale: stream.resolution_scale,
            fps: stream.fps,
            max_bitrate_mbps: stream.max_bitrate_mbps,
            touch_enabled: stream.touch_enabled,
            port: DEFAULT_PORT,
            keep_awake: true,
            mic_enabled: true,
            monitor: None,
        }
    }
}

impl HostSettings {
    /// Out-of-range values fall back to their defaults.
    pub fn sanitized(mut self) -> Self {
        let defaults = HostSettings::default();
        if !StreamSettings::ALLOWED_SCALES.contains(&self.resolution_scale) {
            self.resolution_scale = defaults.resolution_scale;
        }
        if !StreamSettings::ALLOWED_FPS.contains(&self.fps) {
            self.fps = defaults.fps;
        }
        self.max_bitrate_mbps = self
            .max_bitrate_mbps
            .clamp(*StreamSettings::BITRATE_RANGE_MBPS.start(), *StreamSettings::BITRATE_RANGE_MBPS.end());
        if self.port < 1024 {
            self.port = defaults.port;
        }
        self.monitor = self.monitor.map(|m| m.trim().to_owned()).filter(|m| !m.is_empty());
        self
    }

    pub fn stream_settings(&self) -> StreamSettings {
        StreamSettings {
            hi_dpi: self.hi_dpi,
            resolution_scale: self.resolution_scale,
            fps: self.fps,
            max_bitrate_mbps: self.max_bitrate_mbps,
            touch_enabled: self.touch_enabled,
        }
    }
}

/// Paired phones persisted as JSON. A corrupt file is moved aside
/// (`.corrupt`) rather than silently overwritten. Thread-safe.
#[derive(Debug)]
pub struct PairedDeviceStore {
    path: PathBuf,
    devices: Mutex<PairedDevices>,
}

impl PairedDeviceStore {
    /// Loads the store; a missing file is an empty store.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let devices = match fs::read(&path) {
            Ok(data) => PairedDevices::from_json(&data).unwrap_or_else(|error| {
                log::error!("paired devices: {} is unreadable ({error}); moving it aside", path.display());
                let aside = path.with_extension("json.corrupt");
                let _ = fs::remove_file(&aside);
                let _ = fs::rename(&path, &aside);
                PairedDevices::new()
            }),
            Err(_) => PairedDevices::new(),
        };
        PairedDeviceStore { path, devices: Mutex::new(devices) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PairedDevices> {
        self.devices.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Oldest pairing first.
    pub fn all(&self) -> Vec<PairedDevice> {
        self.lock().all().into_iter().cloned().collect()
    }

    pub fn get(&self, device_id: &str) -> Option<PairedDevice> {
        self.lock().get(device_id).cloned()
    }

    /// Adds or replaces the pairing for `device_id`; persisted before returning.
    pub fn add(&self, device_id: &str, device_name: &str, token: &[u8]) -> io::Result<PairedDevice> {
        let mut devices = self.lock();
        let device = devices.add(device_id, device_name, token, unix_now());
        write_private(&self.path, &devices.to_json())?;
        Ok(device)
    }

    pub fn remove(&self, device_id: &str) -> io::Result<bool> {
        let mut devices = self.lock();
        if !devices.remove(device_id) {
            return Ok(false);
        }
        write_private(&self.path, &devices.to_json())?;
        Ok(true)
    }

    /// The device if `token` matches its stored hash (constant-time compare).
    pub fn authenticate(&self, device_id: &str, token: &[u8]) -> Option<PairedDevice> {
        self.lock().authenticate(device_id, token).cloned()
    }

    /// Records a successful connection (and the device's current name).
    pub fn mark_seen(&self, device_id: &str, name: Option<&str>) {
        let mut devices = self.lock();
        if devices.mark_seen(device_id, name, unix_now())
            && let Err(error) = write_private(&self.path, &devices.to_json())
        {
            log::warn!("paired devices: cannot save: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sidecast-data-{name}-{}", uuid_v4(&mut SystemRandom)));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn server_id_is_created_once_and_kept() {
        let dir = DataDir::new(scratch("id"));
        let first = dir.load_or_create_server_id().unwrap();
        assert!(is_uuid(&first));
        assert_eq!(dir.load_or_create_server_id().unwrap(), first);
        // A corrupt file is replaced.
        fs::write(dir.server_id_path(), "garbage").unwrap();
        assert_ne!(dir.load_or_create_server_id().unwrap(), first);
        let _ = fs::remove_dir_all(dir.root());
    }

    #[test]
    fn store_persists_only_hashes_with_owner_only_permissions() {
        let root = scratch("store");
        let path = root.join("nested/paired-devices.json");
        let token = [0xAA; 32];
        let store = PairedDeviceStore::open(&path);
        assert!(store.all().is_empty());
        store.add("dev-1", "Pixel 8", &token).unwrap();
        assert!(store.authenticate("dev-1", &token).is_some());

        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&sidecast_core::protocol::json_b64::encode(&token)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let reloaded = PairedDeviceStore::open(&path);
        assert_eq!(reloaded.all().len(), 1);
        assert!(reloaded.remove("dev-1").unwrap());
        assert!(PairedDeviceStore::open(&path).authenticate("dev-1", &token).is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_store_is_moved_aside() {
        let root = scratch("corrupt");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("p.json");
        fs::write(&path, "not json").unwrap();
        assert!(PairedDeviceStore::open(&path).all().is_empty());
        assert!(root.join("p.json.corrupt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn settings_round_trip_and_sanitize() {
        let dir = DataDir::new(scratch("settings"));
        assert_eq!(dir.load_settings(), HostSettings::default());
        let custom = HostSettings {
            fps: 30,
            max_bitrate_mbps: 20,
            monitor: Some(r"\\.\DISPLAY2".into()),
            ..HostSettings::default()
        };
        dir.save_settings(&custom).unwrap();
        assert_eq!(dir.load_settings(), custom);
        fs::write(dir.settings_path(), r#"{"monitor":"  "}"#).unwrap();
        assert_eq!(dir.load_settings().monitor, None, "a blank monitor means automatic");
        fs::write(dir.settings_path(), r#"{"fps":75,"maxBitrateMbps":500,"resolutionScale":0.3}"#).unwrap();
        let fixed = dir.load_settings();
        assert_eq!((fixed.fps, fixed.max_bitrate_mbps, fixed.resolution_scale), (60, 100, 1.0));
        let _ = fs::remove_dir_all(dir.root());
    }
}
