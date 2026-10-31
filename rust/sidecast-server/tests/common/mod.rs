//! Fake platform backends and a minimal phone for the server's integration tests.

#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sidecast_core::input::InputEvent;
use sidecast_core::protocol::{
    AudioPacket, ClientMessage, CodecInfo, Frame, FrameParser, Hello, MicState, PairProof, ScreenInfo, ServerHello,
    ServerMessage, feature,
};
use sidecast_core::security::{PairingCode, certificate_fingerprint, pairing_proof};
use sidecast_core::streaming::{EncoderCapability, StreamPlan, VideoCodec};
use sidecast_core::{Point, Size};
use sidecast_server::backend::{
    BackendError, Backends, CaptureSession, CapturedFrame, ClientInfo, DisplayBackend, DisplayGeometry, EncodedFrame,
    EncoderConfig, EncoderFactory, FrameSink, InputBackend, InputSink, MicController, MicSink, PixelData, SessionId,
    VideoEncoder,
};
use sidecast_server::data::DataDir;
use sidecast_server::{ServerConfig, ServerEvent, ServerHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// The port test servers listen on: 47894, so local runs stay clear of a real
/// Sidecast (47800); tests take turns (`PORT_LOCK`). `SIDECAST_TEST_PORT=0`
/// picks a free port per server instead, which CI uses because Windows may
/// refuse to rebind a port that still has connections in TIME_WAIT.
fn configured_port() -> u16 {
    std::env::var("SIDECAST_TEST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(47894)
}

/// The port of the running test server.
static CURRENT_PORT: AtomicU16 = AtomicU16::new(0);

pub fn port() -> u16 {
    CURRENT_PORT.load(Ordering::Relaxed)
}

pub static PORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub const CODE: &str = "123456";

// MARK: - Fake backends

/// Delivers a frame every 5 ms from a thread.
#[derive(Default)]
pub struct FakeDisplay {
    pub refuse: Option<BackendError>,
    pub starts: AtomicU32,
}

struct FakeCapture {
    stop: Arc<AtomicBool>,
    geometry: DisplayGeometry,
}

impl DisplayBackend for FakeDisplay {
    fn name(&self) -> &str {
        "fake"
    }

    fn preflight(&self) -> Result<(), BackendError> {
        self.refuse.clone().map_or(Ok(()), Err)
    }

    fn start(
        &self,
        plan: &StreamPlan,
        _: &ClientInfo,
        frames: FrameSink,
    ) -> Result<Box<dyn CaptureSession>, BackendError> {
        self.starts.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (w, h) = (plan.encode_width as u32, plan.encode_height as u32);
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                frames.submit(CapturedFrame {
                    pts_micros: sidecast_server::clock::monotonic_micros(),
                    width: w,
                    height: h,
                    pixels: PixelData::Native(Box::new(())),
                });
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let size = Size::new(plan.display.width as f64, plan.display.height as f64);
        Ok(Box::new(FakeCapture { stop, geometry: DisplayGeometry::new(size) }))
    }
}

impl CaptureSession for FakeCapture {
    fn geometry(&self) -> DisplayGeometry {
        self.geometry.clone()
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Emits tiny, structurally valid H.264 access units.
#[derive(Default)]
pub struct FakeEncoderFactory {
    pub bitrates: Arc<Mutex<Vec<u32>>>,
}

struct FakeEncoder {
    bitrates: Arc<Mutex<Vec<u32>>>,
}

impl EncoderFactory for FakeEncoderFactory {
    fn name(&self) -> &str {
        "fake-h264"
    }

    fn capabilities(&self) -> Vec<EncoderCapability> {
        vec![EncoderCapability::H264_LEVEL_5]
    }

    fn create(&self, config: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, BackendError> {
        assert_eq!(config.codec, VideoCodec::H264);
        self.bitrates.lock().unwrap().push(config.bitrate);
        Ok(Box::new(FakeEncoder { bitrates: self.bitrates.clone() }))
    }
}

impl VideoEncoder for FakeEncoder {
    fn encode(&mut self, _: &CapturedFrame, force_keyframe: bool) -> Result<Option<EncodedFrame>, BackendError> {
        let data = if force_keyframe {
            vec![0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0x88, 0x84]
        } else {
            vec![0, 0, 0, 1, 0x41, 0x9A, 0x02]
        };
        Ok(Some(EncodedFrame { data, is_keyframe: force_keyframe }))
    }

    fn set_bitrate(&mut self, bits_per_second: u32) {
        self.bitrates.lock().unwrap().push(bits_per_second);
    }
}

/// Records every injected event; the pointer is where the last pointer event put it.
#[derive(Default, Clone)]
pub struct RecordingInput {
    pub events: Arc<Mutex<Vec<InputEvent>>>,
    pub released: Arc<AtomicU32>,
}

struct RecordingSink {
    input: RecordingInput,
    size: Size,
    pointer: Option<Point>,
}

impl InputBackend for RecordingInput {
    fn create(&self, geometry: &DisplayGeometry) -> Result<Box<dyn InputSink>, BackendError> {
        Ok(Box::new(RecordingSink { input: self.clone(), size: geometry.size, pointer: None }))
    }
}

impl InputSink for RecordingSink {
    fn inject(&mut self, event: &InputEvent) {
        if let InputEvent::Pointer(p) = event {
            self.pointer = Some(p.at);
        }
        self.input.events.lock().unwrap().push(event.clone());
    }

    fn pointer_position(&mut self) -> Option<Point> {
        self.pointer.filter(|p| p.x < self.size.width && p.y < self.size.height)
    }

    fn release_all(&mut self) {
        self.input.released.fetch_add(1, Ordering::Relaxed);
    }
}

/// Activates the microphone of every mic-capable session at once and records what arrives.
#[derive(Default)]
pub struct RecordingMic {
    pub started: Mutex<Vec<(SessionId, bool)>>,
    pub ended: Mutex<Vec<SessionId>>,
    pub audio: Mutex<Vec<AudioPacket>>,
    pub states: Mutex<Vec<MicState>>,
}

impl MicSink for RecordingMic {
    fn session_started(&self, session: SessionId, _: &str, supports_mic: bool, control: MicController) {
        self.started.lock().unwrap().push((session, supports_mic));
        control.set_active(true);
    }

    fn session_ended(&self, session: SessionId) {
        self.ended.lock().unwrap().push(session);
    }

    fn audio(&self, _: SessionId, packet: &AudioPacket) {
        self.audio.lock().unwrap().push(packet.clone());
    }

    fn mic_state(&self, _: SessionId, state: &MicState) {
        self.states.lock().unwrap().push(state.clone());
    }
}

// MARK: - Server

pub struct TestServer {
    pub handle: ServerHandle,
    pub input: RecordingInput,
    pub mic: Arc<RecordingMic>,
    pub display: Arc<FakeDisplay>,
    pub encoders: Arc<FakeEncoderFactory>,
    pub events: Arc<Mutex<Vec<ServerEvent>>>,
    pub data_dir: std::path::PathBuf,
}

pub async fn start_server(display: FakeDisplay) -> TestServer {
    let data_dir = std::env::temp_dir().join(format!(
        "sidecast-server-test-{}",
        sidecast_core::security::uuid_v4(&mut sidecast_core::security::SystemRandom)
    ));
    let mut config = ServerConfig::new(DataDir::new(&data_dir), configured_port());
    config.bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    config.advertise = false;
    config.tailscale_lookup = false;
    config.fixed_pairing_code = PairingCode::new(CODE);
    config.server_name = Some("Test host".into());
    config.addresses = Some(Arc::new(Vec::new));
    let display = Arc::new(display);
    let encoders = Arc::new(FakeEncoderFactory::default());
    let input = RecordingInput::default();
    let mic = Arc::new(RecordingMic::default());
    let mut backends = Backends::new(display.clone(), vec![encoders.clone()]);
    backends.input = Some(Arc::new(input.clone()));
    backends.mic = Some(mic.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let handle = sidecast_server::start(config, backends, Arc::new(move |e| sink.lock().unwrap().push(e)))
        .await
        .expect("server starts");
    CURRENT_PORT.store(handle.local_addr().port(), Ordering::Relaxed);
    TestServer { handle, input, mic, display, encoders, events, data_dir }
}

impl TestServer {
    pub async fn stop(self) {
        self.handle.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

// MARK: - Client

#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub struct TestClient {
    reader: ReadHalf<TlsStream<TcpStream>>,
    writer: WriteHalf<TlsStream<TcpStream>>,
    parser: FrameParser,
    pub fingerprint: [u8; 32],
    pub device_id: String,
    /// Ack VIDEO_FRAMEs while waiting for other messages.
    pub auto_ack: bool,
    pub frames_seen: u32,
}

impl TestClient {
    pub async fn connect(device_id: &str) -> TestClient {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
            .with_no_client_auth();
        let tcp = TcpStream::connect(("127.0.0.1", port())).await.expect("connects");
        let tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await
            .expect("TLS handshake");
        let fingerprint = certificate_fingerprint(&tls.get_ref().1.peer_certificates().unwrap()[0]);
        let (reader, writer) = tokio::io::split(tls);
        TestClient {
            reader,
            writer,
            parser: FrameParser::new(),
            fingerprint,
            device_id: device_id.to_owned(),
            auto_ack: true,
            frames_seen: 0,
        }
    }

    pub async fn send(&mut self, message: ClientMessage) {
        self.send_raw(&message.encode()).await;
    }

    pub async fn send_raw(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes).await;
        let _ = self.writer.flush().await;
    }

    /// The next server message; `None` once the server closed. Panics after 8 s.
    pub async fn next(&mut self) -> Option<ServerMessage> {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            if let Some(frame) = self.parser.next_frame().expect("valid framing") {
                return Some(ServerMessage::decode(&frame).expect("valid message").expect("known type"));
            }
            match tokio::time::timeout(Duration::from_secs(8), self.reader.read(&mut buffer)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return None,
                Ok(Ok(n)) => self.parser.push(&buffer[..n]),
                Err(_) => panic!("no message from the server within 8 s"),
            }
        }
    }

    /// Skips (and acks) video frames, CURSORs and PONGs until a message `pick` accepts.
    pub async fn expect<T>(&mut self, what: &str, mut pick: impl FnMut(&ServerMessage) -> Option<T>) -> T {
        loop {
            let Some(message) = self.next().await else { panic!("connection closed while waiting for {what}") };
            if let Some(found) = pick(&message) {
                return found;
            }
            match message {
                ServerMessage::VideoFrame(frame) => {
                    self.frames_seen += 1;
                    if self.auto_ack {
                        self.send(ClientMessage::FrameAck(frame.seq)).await;
                    }
                }
                ServerMessage::Pong(_) | ServerMessage::Cursor(_) | ServerMessage::MicControl(_) => {}
                other => panic!("unexpected {other:?} while waiting for {what}"),
            }
        }
    }

    /// Waits for the server to close (acking frames meanwhile); returns the BYE reason if one came.
    pub async fn expect_close(&mut self) -> Option<String> {
        let mut bye = None;
        while let Some(message) = self.next().await {
            match message {
                ServerMessage::Bye(b) => bye = Some(b.reason),
                ServerMessage::VideoFrame(frame) if self.auto_ack => {
                    self.send(ClientMessage::FrameAck(frame.seq)).await
                }
                _ => {}
            }
        }
        bye
    }

    pub fn hello_message(&self, features: &[&str]) -> Hello {
        let mut hello = Hello::new(
            &self.device_id,
            &format!("phone {}", self.device_id),
            ScreenInfo::new(1920, 1080, 420, 60.0),
            vec![CodecInfo::new("h264", 1920, 1080)],
        );
        hello.features = features.iter().map(|f| f.to_string()).collect();
        hello
    }

    pub async fn hello(&mut self, features: &[&str]) -> ServerHello {
        let hello = self.hello_message(features);
        self.send(ClientMessage::Hello(hello)).await;
        self.expect("SERVER_HELLO", |m| match m {
            ServerMessage::ServerHello(h) => Some(h.clone()),
            _ => None,
        })
        .await
    }

    pub fn proof(&self, code: &str) -> PairProof {
        let nonce = [7u8; 16];
        PairProof { nonce: nonce.to_vec(), proof: pairing_proof(code, &self.fingerprint, &nonce).to_vec() }
    }

    /// HELLO + pairing with the right code; returns the token.
    pub async fn pair(&mut self, features: &[&str]) -> Vec<u8> {
        self.hello(features).await;
        self.send(ClientMessage::PairStart).await;
        self.expect("PAIR_CHALLENGE", |m| matches!(m, ServerMessage::PairChallenge(_)).then_some(())).await;
        let proof = self.proof(CODE);
        self.send(ClientMessage::PairProof(proof)).await;
        let result = self.auth_result().await;
        assert!(result.ok, "{result:?}");
        result.token.expect("a token")
    }

    pub async fn auth_result(&mut self) -> sidecast_core::protocol::AuthResult {
        self.expect("AUTH_RESULT", |m| match m {
            ServerMessage::AuthResult(r) => Some(r.clone()),
            _ => None,
        })
        .await
    }

    pub async fn stream_config(&mut self) -> sidecast_core::protocol::StreamConfig {
        self.expect("STREAM_CONFIG", |m| match m {
            ServerMessage::StreamConfig(c) => Some(c.clone()),
            _ => None,
        })
        .await
    }

    /// The next VIDEO_FRAME (not acked).
    pub async fn video_frame(&mut self) -> sidecast_core::protocol::VideoFrame {
        loop {
            match self.next().await.expect("connection open") {
                ServerMessage::VideoFrame(frame) => {
                    self.frames_seen += 1;
                    return frame;
                }
                ServerMessage::Pong(_) | ServerMessage::Cursor(_) | ServerMessage::MicControl(_) => {}
                other => panic!("unexpected {other:?} while waiting for a frame"),
            }
        }
    }
}

pub fn frame(msg_type: u8, payload: &[u8]) -> Vec<u8> {
    Frame { msg_type, payload: payload.to_vec() }.encode()
}

pub fn mic_feature() -> &'static str {
    feature::MIC
}
