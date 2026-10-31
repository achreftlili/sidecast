//! The phone's microphone as a Windows microphone, through a virtual audio
//! cable the user installed (VB-CABLE): the phone's 48 kHz mono audio is
//! played into the cable's playback side ("CABLE Input") with WASAPI, and
//! apps record its recording side ("CABLE Output"). Without a cable the
//! host offers no microphone. Sidecast never opens a real microphone.
//!
//! The phone only records while an app records from the cable (as on the
//! Mac): a thread per stream watches the recording endpoint's audio sessions
//! twice a second, sends MIC_CONTROL when that changes, and plays the jitter
//! buffer into the cable while it is active.

// FFI module: every `unsafe` block says why it is sound.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sidecast_core::audio::AudioJitterBuffer;
use sidecast_core::protocol::{AudioPacket, MicState};
use sidecast_server::backend::{MicController, MicSink, SessionId};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AudioSessionStateActive, DEVICE_STATE_ACTIVE, EDataFlow, IAudioClient,
    IAudioRenderClient, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eCapture, eRender,
};
use windows::Win32::Media::KernelStreaming::{SPEAKER_FRONT_CENTER, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

use super::cable::{cable_output_name, pick_cable};
use super::util::{com_thread, pcwstr, wide};

/// An audio endpoint: its id (for `GetDevice`) and friendly name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    id: String,
    name: String,
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    com_thread();
    // SAFETY: plain COM object creation.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn endpoints(flow: EDataFlow) -> Vec<Endpoint> {
    let Ok(enumerator) = enumerator() else { return Vec::new() };
    let mut found = Vec::new();
    // SAFETY: COM calls on live interfaces; the id string is CoTaskMem we free once.
    unsafe {
        let Ok(collection) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return found };
        for i in 0..collection.GetCount().unwrap_or(0) {
            let Ok(device) = collection.Item(i) else { continue };
            let Ok(raw_id) = device.GetId() else { continue };
            let id = raw_id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw_id.0.cast()));
            let name = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|value| value.to_string())
                .unwrap_or_default();
            found.push(Endpoint { id, name });
        }
    }
    found
}

fn device(id: &str) -> windows::core::Result<IMMDevice> {
    let id = wide(id);
    // SAFETY: `id` is NUL-terminated and outlives the call.
    unsafe { enumerator()?.GetDevice(pcwstr(&id)) }
}

/// Playback endpoint names, for `--diagnose`.
pub fn playback_names() -> Vec<String> {
    endpoints(eRender).into_iter().map(|e| e.name).collect()
}

/// The phone microphone through a virtual audio cable.
#[derive(Debug)]
pub struct VirtualCableMic {
    playback: Endpoint,
    recording: Endpoint,
    shared: Arc<Mutex<Shared>>,
    worker: Mutex<Option<Worker>>,
}

#[derive(Debug, Default)]
struct Shared {
    session: Option<SessionId>,
    jitter: AudioJitterBuffer,
}

#[derive(Debug)]
struct Worker {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl VirtualCableMic {
    /// The installed cable, or `None` (then the host offers no microphone).
    pub fn find() -> Option<VirtualCableMic> {
        let playback = endpoints(eRender);
        let name = pick_cable(playback.iter().map(|e| e.name.as_str()))?.to_owned();
        let playback = playback.into_iter().find(|e| e.name == name)?;
        let wanted = cable_output_name(&playback.name);
        let Some(recording) = endpoints(eCapture).into_iter().find(|e| e.name.eq_ignore_ascii_case(&wanted)) else {
            log::info!("mic: found {} but not its recording side {wanted}; no phone microphone", playback.name);
            return None;
        };
        log::info!("mic: the phone's microphone plays into {} (apps record {})", playback.name, recording.name);
        Some(VirtualCableMic { playback, recording, shared: Arc::default(), worker: Mutex::new(None) })
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn stop_worker(&self) {
        let worker = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(worker) = worker {
            worker.stop.store(true, Ordering::Relaxed);
            let _ = worker.thread.join();
        }
    }
}

impl MicSink for VirtualCableMic {
    fn session_started(&self, session: SessionId, device_name: &str, supports_mic: bool, control: MicController) {
        self.stop_worker();
        {
            let mut shared = self.shared();
            shared.session = Some(session);
            shared.jitter.reset();
        }
        if !supports_mic {
            log::info!("mic: {device_name} has no microphone to share");
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let context = WorkerContext {
            playback: self.playback.id.clone(),
            recording: self.recording.id.clone(),
            recording_name: self.recording.name.clone(),
            shared: self.shared.clone(),
            control,
            stop: stop.clone(),
        };
        match std::thread::Builder::new().name("sidecast-mic".into()).spawn(move || context.run()) {
            Ok(thread) => *self.worker.lock().unwrap_or_else(|p| p.into_inner()) = Some(Worker { stop, thread }),
            Err(error) => log::warn!("mic: cannot start: {error}"),
        }
    }

    fn session_ended(&self, session: SessionId) {
        if self.shared().session != Some(session) {
            return;
        }
        self.stop_worker();
        let mut shared = self.shared();
        shared.session = None;
        shared.jitter.reset();
    }

    fn audio(&self, session: SessionId, packet: &AudioPacket) {
        let mut shared = self.shared();
        if shared.session == Some(session) {
            shared.jitter.push(packet);
        }
    }

    fn mic_state(&self, session: SessionId, state: &MicState) {
        if self.shared().session == Some(session) {
            log::info!(
                "mic: phone microphone {}{}",
                if state.capturing { "on" } else { "off" },
                state.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
            );
        }
    }
}

impl Drop for VirtualCableMic {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

struct WorkerContext {
    playback: String,
    recording: String,
    recording_name: String,
    shared: Arc<Mutex<Shared>>,
    control: MicController,
    stop: Arc<AtomicBool>,
}

impl WorkerContext {
    fn run(self) {
        com_thread();
        let mut active = false;
        let mut player: Option<Player> = None;
        let mut checked: Option<Instant> = None;
        while !self.stop.load(Ordering::Relaxed) {
            if checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
                checked = Some(Instant::now());
                let recording = recording_active(&self.recording);
                if recording != active {
                    active = recording;
                    log::info!(
                        "mic: an app {} {} → MIC_CONTROL active:{active}",
                        if active { "records from" } else { "stopped recording from" },
                        self.recording_name
                    );
                    self.control.set_active(active);
                    self.shared.lock().unwrap_or_else(|p| p.into_inner()).jitter.reset();
                    player = None;
                    if active {
                        match Player::open(&self.playback) {
                            Ok(p) => player = Some(p),
                            Err(error) => log::warn!("mic: cannot play into the cable: {error}"),
                        }
                    }
                }
            }
            match &mut player {
                Some(p) => {
                    if let Err(error) = p.feed(&self.shared) {
                        log::warn!("mic: playback stopped: {error}");
                        player = None;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        if active {
            self.control.set_active(false);
        }
    }
}

/// Whether any app records from the endpoint (an active audio session).
fn recording_active(id: &str) -> bool {
    let check = || -> windows::core::Result<bool> {
        // SAFETY: COM calls on live interfaces.
        unsafe {
            let manager: IAudioSessionManager2 = device(id)?.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for i in 0..sessions.GetCount()? {
                if sessions.GetSession(i)?.GetState()? == AudioSessionStateActive {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    };
    check().unwrap_or(false)
}

/// A shared-mode WASAPI stream into the cable: 48 kHz mono float, which
/// Windows converts to the device's own format.
struct Player {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    frames: u32,
}

impl Player {
    fn open(id: &str) -> windows::core::Result<Player> {
        let format = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: 1,
                nSamplesPerSec: AudioPacket::SAMPLE_RATE,
                nAvgBytesPerSec: AudioPacket::SAMPLE_RATE * 4,
                nBlockAlign: 4,
                wBitsPerSample: 32,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
            dwChannelMask: SPEAKER_FRONT_CENTER,
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        };
        // SAFETY: COM calls on live interfaces; `format` outlives Initialize;
        // the event handle is closed in Drop.
        unsafe {
            let client: IAudioClient = device(id)?.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                200_000, // 20 ms
                0,
                (&raw const format).cast::<WAVEFORMATEX>(),
                None,
            )?;
            let event = CreateEventW(None, false, false, PCWSTR::null())?;
            let player = Player { render: client.GetService()?, frames: client.GetBufferSize()?, client, event };
            player.client.SetEventHandle(player.event)?;
            player.client.Start()?;
            Ok(player)
        }
    }

    /// Waits for the device (≤ 50 ms) and tops its buffer up from the jitter buffer.
    fn feed(&mut self, shared: &Mutex<Shared>) -> windows::core::Result<()> {
        // SAFETY: a valid event handle; the render buffer is written only
        // between GetBuffer and ReleaseBuffer, within the size granted.
        unsafe {
            if WaitForSingleObject(self.event, 50) != WAIT_OBJECT_0 {
                return Ok(());
            }
            let free = self.frames.saturating_sub(self.client.GetCurrentPadding()?);
            if free == 0 {
                return Ok(());
            }
            let data = self.render.GetBuffer(free)?;
            let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize);
            shared.lock().unwrap_or_else(|p| p.into_inner()).jitter.read(out);
            self.render.ReleaseBuffer(free, 0)
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: stops the stream we started and closes the handle we created.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}
