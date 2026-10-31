//! The phone's microphone as a PipeWire source, "Sidecast Microphone": it
//! exists only while a phone that offers its microphone streams, and the
//! phone records only while an app records from it. PipeWire runs the node
//! only while something consumes it, so the stream's `Streaming` state is
//! "an app is recording" and maps to MIC_CONTROL active:true (like the Mac's
//! driver, which watches whether its device runs). The node does not become
//! the default input: pick it in Settings → Sound or in the app.
//!
//! AUDIO packets go into `sidecast_core`'s jitter buffer; PipeWire's
//! real-time callback reads 48 kHz mono `f32` from it.

use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use pw::spa::pod::{Object, Pod, Value};
use sidecast_core::audio::AudioJitterBuffer;
use sidecast_core::protocol::{AudioPacket, MicState};
use sidecast_server::backend::{MicController, MicSink, SessionId};

/// The node's name in PipeWire (and so in `pw-record --target`).
pub const NODE_NAME: &str = "sidecast-microphone";
pub const NODE_DESCRIPTION: &str = "Sidecast Microphone";

type Jitter = Arc<Mutex<AudioJitterBuffer>>;

#[derive(Default)]
pub struct PipeWireMic {
    session: Mutex<Option<MicSession>>,
}

impl std::fmt::Debug for PipeWireMic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipeWireMic").finish_non_exhaustive()
    }
}

struct MicSession {
    id: SessionId,
    jitter: Jitter,
    _source: VirtualSource,
}

impl PipeWireMic {
    fn session(&self) -> std::sync::MutexGuard<'_, Option<MicSession>> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl MicSink for PipeWireMic {
    fn session_started(&self, session: SessionId, device_name: &str, supports_mic: bool, control: MicController) {
        // A new stream replaces the previous one's source.
        drop(self.session().take());
        if !supports_mic {
            log::info!("mic: {device_name} does not offer its microphone");
            return;
        }
        let jitter: Jitter = Arc::default();
        let on_recording = {
            let jitter = jitter.clone();
            move |recording: bool| {
                if recording {
                    jitter.lock().unwrap_or_else(|p| p.into_inner()).reset();
                }
                log::info!(
                    "mic: an app {} recording → MIC_CONTROL active:{recording}",
                    if recording { "started" } else { "stopped" }
                );
                control.set_active(recording);
            }
        };
        match VirtualSource::start(device_name, jitter.clone(), on_recording) {
            Ok(source) => {
                log::info!("mic: “{NODE_DESCRIPTION}” is available while {device_name} streams");
                *self.session() = Some(MicSession { id: session, jitter, _source: source });
            }
            Err(error) => log::warn!("mic: cannot create the PipeWire source: {error}"),
        }
    }

    fn session_ended(&self, session: SessionId) {
        let mut current = self.session();
        if current.as_ref().is_some_and(|s| s.id == session) {
            *current = None;
            log::info!("mic: “{NODE_DESCRIPTION}” removed");
        }
    }

    fn audio(&self, session: SessionId, packet: &AudioPacket) {
        if let Some(current) = self.session().as_ref().filter(|s| s.id == session) {
            current.jitter.lock().unwrap_or_else(|p| p.into_inner()).push(packet);
        }
    }

    fn mic_state(&self, session: SessionId, state: &MicState) {
        if self.session().as_ref().is_some_and(|s| s.id == session) {
            log::info!(
                "mic: phone microphone capturing={}{}",
                state.capturing,
                state.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()
            );
        }
    }
}

/// A PipeWire `Audio/Source` node on its own thread; dropping it removes the node.
struct VirtualSource {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl VirtualSource {
    fn start(device_name: &str, jitter: Jitter, on_recording: impl Fn(bool) + Send + 'static) -> Result<Self, String> {
        let (stop, stop_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        // The phone's name becomes a PipeWire property: no NUL bytes in C strings.
        let device_name = device_name.replace('\0', "");
        let thread = std::thread::Builder::new()
            .name("sidecast-pipewire-mic".into())
            .spawn(move || {
                let result = run_source(&device_name, jitter, on_recording, stop_rx, ready_tx.clone());
                if let Err(error) = result {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .map_err(|e| e.to_string())?;
        let source = VirtualSource { stop, thread: Some(thread) };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(source),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("PipeWire did not answer".into()),
        }
    }
}

impl Drop for VirtualSource {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct SourceData {
    jitter: Jitter,
    scratch: Vec<f32>,
}

fn run_source(
    device_name: &str,
    jitter: Jitter,
    on_recording: impl Fn(bool) + 'static,
    stop: pw::channel::Receiver<()>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let error = |e: pw::Error| e.to_string();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(error)?;
    let core = context.connect_rc(None).map_err(|e| format!("cannot connect to PipeWire: {e}"))?;
    let rate = AudioPacket::SAMPLE_RATE;
    let stream = pw::stream::StreamBox::new(
        &core,
        NODE_DESCRIPTION,
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::NODE_NAME => NODE_NAME,
            *pw::keys::NODE_DESCRIPTION => NODE_DESCRIPTION,
            *pw::keys::NODE_NICK => device_name,
            *pw::keys::NODE_LATENCY => format!("480/{rate}"),
            *pw::keys::AUDIO_CHANNELS => "1",
            "audio.position" => "MONO",
        },
    )
    .map_err(error)?;

    let _listener = stream
        .add_local_listener_with_user_data(SourceData { jitter, scratch: Vec::new() })
        .state_changed(move |_, _, old, new| {
            log::debug!("mic: PipeWire source {old:?} → {new:?}");
            match (&old, &new) {
                (_, pw::stream::StreamState::Streaming) => on_recording(true),
                (pw::stream::StreamState::Streaming, _) => on_recording(false),
                _ => {}
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let requested = buffer.requested() as usize;
            let Some(out) = buffer.datas_mut().first_mut() else { return };
            let Some(bytes) = out.data() else { return };
            let capacity = bytes.len() / 4;
            let frames = if requested > 0 { requested.min(capacity) } else { capacity.min(1024) };
            data.scratch.resize(frames, 0.0);
            // Never block the real-time thread: a busy buffer means silence this cycle.
            match data.jitter.try_lock() {
                Ok(mut jitter) => {
                    jitter.read(&mut data.scratch);
                }
                Err(_) => data.scratch.fill(0.0),
            }
            for (sample, out) in data.scratch.iter().zip(bytes.as_chunks_mut::<4>().0) {
                *out = sample.to_le_bytes();
            }
            let chunk = out.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = 4;
            *chunk.size_mut() = (frames * 4) as u32;
        })
        .register()
        .map_err(error)?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(1);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let format = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|e| format!("{e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&format).ok_or("invalid audio format")?];
    // No AUTOCONNECT: a source waits for apps to link to it.
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(error)?;
    let _ = ready.send(Ok(()));

    let quit = mainloop.downgrade();
    let _stop = stop.attach(mainloop.loop_(), move |()| {
        if let Some(mainloop) = quit.upgrade() {
            mainloop.quit();
        }
    });
    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}
