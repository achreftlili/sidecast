//! The scripted fake phone: TLS → HELLO → pairing or AUTH → stream for N
//! seconds, acking and checking every frame, sending input, then a report.
//! Every deviation from PROTOCOL.md is recorded as a failure.

use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sidecast_core::annexb::inspect_access_unit;
use sidecast_core::protocol::{
    AddressKind, AudioPacket, AuthRequest, AuthResult, ByeReason, ClientMessage, CursorPosition, Frame, FrameParser,
    Hello, InputModeMessage, KeyEvent, KeyModifiers, MicControl, MicState, MicStateReason, PAIRING_ATTEMPTS,
    PAIRING_LIFETIME, PairChallenge, PairProof, ProtocolKey, ScreenInfo, ServerHello, ServerMessage, StreamConfig,
    TouchEvent, VideoFrame, feature, json_b64,
};
use sidecast_core::security::{
    NONCE_LENGTH, RandomSource, SystemRandom, certificate_fingerprint, device_token, pairing_proof, to_hex,
};
use sidecast_core::streaming::{StreamPlanner, VideoCodec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_rustls::TlsConnector;

use crate::decoder::FrameDecoder;
use crate::gestures::ProbeGesture;
use crate::options::{KeyboardInput, Options};
use crate::state::{Pairing, ProbeState};
use crate::stats::Stats;

/// Trust on first pairing (PROTOCOL §2): any certificate is accepted here —
/// its fingerprint is checked against the pinned one before any secret is
/// sent — but handshake signatures are verified, so the host must hold the key.
#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hello,
    Authenticating,
    Streaming,
    Finished,
}

/// One scheduled TOUCH (or a log line), relative to the start of the script.
struct Scheduled {
    at: Duration,
    action: ScriptAction,
}

enum ScriptAction {
    Log(String),
    Touch(TouchEvent),
}

struct Mic {
    started: Instant,
    seq: u32,
    sample_index: u64,
}

/// Runs the probe; returns the process exit code.
pub async fn run(options: Options) -> i32 {
    let watchdog = Duration::from_secs_f64(options.seconds + options.stall_after.unwrap_or(0.0) + 30.0);
    match tokio::time::timeout(watchdog, run_inner(options)).await {
        Ok(code) => code,
        Err(_) => {
            println!("probe: FAILURE watchdog: the run did not finish");
            println!("probe: RESULT FAIL");
            1
        }
    }
}

async fn run_inner(options: Options) -> i32 {
    let address = match (options.host.as_str(), options.port).to_socket_addrs().map(|mut a| a.next()) {
        Ok(Some(address)) => address,
        _ => return fail_early(&format!("cannot resolve {}", options.host)),
    };
    let tcp = match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(address)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => return fail_early(&format!("cannot connect to {address}: {error}")),
        Err(_) => return fail_early(&format!("timed out connecting to {address}")),
    };
    let _ = tcp.set_nodelay(true);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = match rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    {
        Ok(builder) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
            .with_no_client_auth(),
        Err(error) => return fail_early(&format!("TLS setup: {error}")),
    };
    let server_name = match ServerName::try_from(options.host.clone()) {
        Ok(name) => name,
        Err(_) => return fail_early(&format!("invalid host name {}", options.host)),
    };
    let tls = match tokio::time::timeout(
        Duration::from_secs(10),
        TlsConnector::from(Arc::new(config)).connect(server_name, tcp),
    )
    .await
    {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => return fail_early(&format!("TLS handshake failed: {error}")),
        Err(_) => return fail_early("TLS handshake timed out"),
    };
    let (_, connection) = tls.get_ref();
    let version = match connection.protocol_version() {
        Some(rustls::ProtocolVersion::TLSv1_3) => "1.3",
        Some(rustls::ProtocolVersion::TLSv1_2) => "1.2",
        _ => "?",
    };
    let Some(leaf) = connection.peer_certificates().and_then(|c| c.first()) else {
        return fail_early("no server certificate");
    };
    let fingerprint = certificate_fingerprint(leaf);
    println!("probe: TLS {version} up, server certificate fingerprint {}", to_hex(&fingerprint));

    let (mut reader, mut writer) = tokio::io::split(tls);
    let (out, mut outgoing) = unbounded_channel::<Vec<u8>>();
    let writer_task = tokio::spawn(async move {
        while let Some(data) = outgoing.recv().await {
            if writer.write_all(&data).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });

    let mut probe = Probe::new(options, fingerprint, out);
    probe.send_hello();
    let mut parser = FrameParser::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut ping = tokio::time::interval(Duration::from_secs(1));
    let mut second = tokio::time::interval(Duration::from_secs(1));
    let mut fast = tokio::time::interval(Duration::from_millis(4));
    fast.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while probe.phase != Phase::Finished {
        let deadline = probe.next_deadline();
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) => probe.closed(None),
                Err(error) => probe.closed(Some(error.to_string())),
                Ok(n) => {
                    parser.push(&buffer[..n]);
                    loop {
                        match parser.next_frame() {
                            Ok(Some(frame)) => probe.handle(frame),
                            Ok(None) => break,
                            Err(error) => {
                                probe.fail(format!("framing error from server: {error}"));
                                break;
                            }
                        }
                        if probe.phase == Phase::Finished {
                            break;
                        }
                    }
                }
            },
            _ = ping.tick(), if probe.phase == Phase::Streaming => probe.send_ping(),
            _ = second.tick(), if probe.phase == Phase::Streaming && probe.stats_started => {
                println!("{}", probe.stats.second_line());
            }
            _ = fast.tick(), if probe.has_fast_work() => probe.fast_tick(),
            _ = tokio::time::sleep_until(deadline) => probe.on_deadline(),
        }
    }
    drop(probe.out.take());
    let _ = tokio::time::timeout(Duration::from_secs(2), writer_task).await;
    probe.report()
}

fn fail_early(why: &str) -> i32 {
    println!("probe: FAILURE {why}");
    println!("probe: RESULT FAIL");
    1
}

struct Probe {
    options: Options,
    state: ProbeState,
    phase: Phase,
    out: Option<UnboundedSender<Vec<u8>>>,
    fingerprint: [u8; 32],
    server_hello: Option<ServerHello>,
    awaiting_challenge: bool,
    code_index: usize,
    /// Reason of a failed AUTH_RESULT whose BYE we are waiting for, and since when.
    auth_failure: Option<(String, Instant)>,
    config: Option<(VideoCodec, StreamConfig)>,
    decoder: Option<FrameDecoder>,
    decoding: bool,
    expected_seq: Option<u32>,
    expect_keyframe: bool,
    keyframe_requested_at: Option<Instant>,
    stats: Stats,
    stats_started: bool,
    failures: Vec<String>,
    pings: HashSet<u64>,
    epoch: Instant,
    input_started: bool,
    script: Vec<Scheduled>,
    script_start: Option<Instant>,
    script_next: usize,
    mic: Option<Mic>,
    mic_controls: Vec<bool>,
    mic_packets: usize,
    cursor_count: u32,
    cursor_per_second: HashMap<u64, u32>,
    last_cursor: Option<CursorPosition>,
    streaming_since: Option<Instant>,
    stalled_at: Option<Instant>,
}

impl Probe {
    fn new(options: Options, fingerprint: [u8; 32], out: UnboundedSender<Vec<u8>>) -> Probe {
        let state = ProbeState::load(options.state_path.as_deref());
        Probe {
            options,
            state,
            phase: Phase::Hello,
            out: Some(out),
            fingerprint,
            server_hello: None,
            awaiting_challenge: false,
            code_index: 0,
            auth_failure: None,
            config: None,
            decoder: None,
            decoding: false,
            expected_seq: None,
            expect_keyframe: false,
            keyframe_requested_at: None,
            stats: Stats::default(),
            stats_started: false,
            failures: Vec::new(),
            pings: HashSet::new(),
            epoch: Instant::now(),
            input_started: false,
            script: Vec::new(),
            script_start: None,
            script_next: 0,
            mic: None,
            mic_controls: Vec::new(),
            mic_packets: 0,
            cursor_count: 0,
            cursor_per_second: HashMap::new(),
            last_cursor: None,
            streaming_since: None,
            stalled_at: None,
        }
    }

    fn send(&self, message: ClientMessage) {
        if self.stalled_at.is_some() {
            return; // silent on purpose (liveness test)
        }
        if let Some(out) = &self.out {
            let _ = out.send(message.encode());
        }
    }

    fn violation(&mut self, why: impl Into<String>) {
        let why = why.into();
        println!("probe: VIOLATION {why}");
        self.failures.push(why);
    }

    fn fail(&mut self, why: impl Into<String>) {
        if self.phase == Phase::Finished {
            return;
        }
        self.failures.push(why.into());
        self.finish();
    }

    fn finish(&mut self) {
        self.phase = Phase::Finished;
    }

    fn send_hello(&mut self) {
        let mut hello = Hello::new(
            &self.state.device_id,
            "sidecast-probe (rust)",
            ScreenInfo::new(self.options.width, self.options.height, 420, self.options.refresh_rate),
            self.options.codecs(),
        );
        if self.options.mic {
            hello.features.push(feature::MIC.to_owned());
        }
        hello.network = self.options.network.clone();
        hello.limits = self.options.limits;
        self.send(ClientMessage::Hello(hello));
    }

    // MARK: - Incoming

    fn handle(&mut self, frame: Frame) {
        let message = match ServerMessage::decode(&frame) {
            Ok(Some(message)) => message,
            Ok(None) => {
                println!("probe: skipping unknown message type 0x{:02x}", frame.msg_type);
                return;
            }
            Err(error) => return self.fail(format!("protocol error from server: {error}")),
        };
        match message {
            ServerMessage::ServerHello(hello) => self.handle_server_hello(hello),
            ServerMessage::PairChallenge(challenge) => self.handle_challenge(challenge),
            ServerMessage::AuthResult(result) => self.handle_auth_result(result),
            ServerMessage::StreamConfig(config) => self.handle_stream_config(config),
            ServerMessage::VideoFrame(video) => self.handle_video(video, frame.payload.len() + 5),
            ServerMessage::Pong(t) => {
                if self.pings.remove(&t) {
                    self.stats.rtts.push(self.now_micros().saturating_sub(t) as f64 / 1e6);
                } else {
                    self.violation(format!("PONG {t} does not echo a PING we sent"));
                }
            }
            ServerMessage::MicControl(control) => self.handle_mic_control(control),
            ServerMessage::Cursor(position) => self.handle_cursor(position),
            ServerMessage::Bye(bye) => {
                if let Some((reason, _)) = self.auth_failure.clone() {
                    return self.conclude_auth_failure(&reason, Some(&bye.reason));
                }
                if bye.known_reason().is_none() {
                    self.violation(format!("BYE with unknown reason {:?}", bye.reason));
                }
                self.fail(format!("server sent BYE {}", bye.reason));
            }
        }
    }

    fn handle_server_hello(&mut self, hello: ServerHello) {
        if self.phase != Phase::Hello {
            return self.fail("unexpected SERVER_HELLO");
        }
        if hello.protocol != sidecast_core::protocol::VERSION {
            return self.fail(format!("server protocol {}", hello.protocol));
        }
        if hello.server_id.is_empty() {
            self.violation("SERVER_HELLO without serverId");
        }
        println!(
            "probe: SERVER_HELLO {} serverId {} port {} platform {}{}",
            hello.server_name,
            hello.server_id,
            hello.port.map_or("-".to_owned(), |p| p.to_string()),
            hello.platform,
            if hello.host_platform().is_none() { " (unknown)" } else { "" }
        );
        if let Some(expected) = self.options.expect_platform
            && hello.host_platform() != Some(expected)
        {
            self.violation(format!("platform {}, expected {}", hello.platform, expected.as_str()));
        }
        for address in hello.addresses.iter().flatten() {
            println!("probe:   address {} ({})", address.host, address.kind);
            if address.address_kind().is_none() {
                self.violation(format!("unknown address kind {:?}", address.kind));
            }
        }
        if let Some(addresses) = &hello.addresses {
            let ranks: Vec<u8> = addresses
                .iter()
                .filter_map(|a| a.address_kind())
                .map(|k| match k {
                    AddressKind::Lan => 0,
                    AddressKind::Tailscale => 1,
                    AddressKind::TailscaleDns => 2,
                })
                .collect();
            if !ranks.is_sorted() {
                self.violation("addresses are not ordered LAN, tailscale, tailscale-dns");
            }
        } else {
            println!("probe:   no addresses advertised");
        }
        let server_id = hello.server_id.clone();
        self.server_hello = Some(hello);
        self.phase = Phase::Authenticating;
        if let Some(pairing) = self.state.servers.get(&server_id) {
            if pairing.fingerprint != to_hex(&self.fingerprint) {
                return self
                    .fail(format!("This computer's identity changed — pair again (pinned {})", pairing.fingerprint));
            }
            let Some(token) = json_b64::decode(&pairing.token) else { return self.fail("corrupt token in state") };
            println!("probe: pinned fingerprint matches; sending AUTH");
            let device_id = self.state.device_id.clone();
            self.send(ClientMessage::Auth(AuthRequest { device_id, token }));
        } else if !self.options.codes.is_empty() {
            println!("probe: not paired yet; sending PAIR_START");
            self.awaiting_challenge = true;
            self.send(ClientMessage::PairStart);
        } else {
            self.fail("not paired with this computer and no --code given");
        }
    }

    fn handle_challenge(&mut self, challenge: PairChallenge) {
        if !self.awaiting_challenge {
            return self.fail("unexpected PAIR_CHALLENGE");
        }
        self.awaiting_challenge = false;
        println!(
            "probe: PAIR_CHALLENGE expiresInSec={} attemptsLeft={}",
            challenge.expires_in_sec, challenge.attempts_left
        );
        if !(1..=PAIRING_LIFETIME as i64).contains(&challenge.expires_in_sec) {
            self.violation(format!("PAIR_CHALLENGE expiresInSec {} outside 1…120", challenge.expires_in_sec));
        }
        if challenge.attempts_left != PAIRING_ATTEMPTS as i64 {
            self.violation(format!("PAIR_CHALLENGE attemptsLeft {}, expected 5", challenge.attempts_left));
        }
        self.send_proof();
    }

    fn send_proof(&mut self) {
        let code = self.options.codes[self.code_index].clone();
        let mut nonce = [0u8; NONCE_LENGTH];
        SystemRandom.fill(&mut nonce);
        let proof = pairing_proof(&code, &self.fingerprint, &nonce);
        println!("probe: PAIR_PROOF with code {code}");
        self.send(ClientMessage::PairProof(PairProof { nonce: nonce.to_vec(), proof: proof.to_vec() }));
    }

    fn handle_auth_result(&mut self, result: AuthResult) {
        if self.phase != Phase::Authenticating || self.auth_failure.is_some() {
            return self.fail("unexpected AUTH_RESULT");
        }
        let Some(hello) = self.server_hello.clone() else { return self.fail("AUTH_RESULT before SERVER_HELLO") };
        if !result.ok {
            let reason = result.reason.clone().unwrap_or_else(|| "?".into());
            println!(
                "probe: AUTH_RESULT ok=false reason={reason} attemptsLeft={}",
                result.attempts_left.map_or("-".to_owned(), |n| n.to_string())
            );
            match ByeReason::parse(&reason) {
                Some(ByeReason::BadCode) => {
                    if !result.attempts_left.is_some_and(|n| n >= 1) {
                        return self.fail("bad_code must carry attemptsLeft >= 1");
                    }
                    if self.code_index + 1 < self.options.codes.len() {
                        self.code_index += 1;
                        return self.send_proof(); // the host keeps the pairing open: retry
                    }
                    return self.conclude_auth_failure(&reason, None);
                }
                Some(ByeReason::TooManyAttempts) => {
                    if result.attempts_left != Some(0) {
                        return self.fail("too_many_attempts must carry attemptsLeft 0");
                    }
                }
                Some(ByeReason::UnknownDevice) => {
                    self.state.servers.remove(&hello.server_id);
                    let _ = self.state.save(self.options.state_path.as_deref());
                    println!("probe: host forgot us; deleted the stored pairing");
                }
                Some(ByeReason::PairingExpired) => {}
                _ => self.violation(format!("AUTH_RESULT with unexpected reason {reason}")),
            }
            self.auth_failure = Some((reason, Instant::now())); // a BYE must follow
            return;
        }
        if let Some(expected) = &self.options.expect_auth_failure {
            return self.fail(format!("authentication succeeded, expected failure {expected}"));
        }
        if let Some(token) = &result.token {
            if token.len() != device_token::LENGTH {
                self.violation(format!("token of {} bytes, expected 32", token.len()));
            }
            self.state.servers.insert(
                hello.server_id.clone(),
                Pairing {
                    fingerprint: to_hex(&self.fingerprint),
                    token: json_b64::encode(token),
                    server_name: hello.server_name.clone(),
                    host: self.options.host.clone(),
                    port: self.options.port,
                },
            );
            if let Err(error) = self.state.save(self.options.state_path.as_deref()) {
                return self.fail(format!("cannot save state: {error}"));
            }
            println!(
                "probe: PAIRED (token received{})",
                if self.options.state_path.is_some() { ", saved to state" } else { "" }
            );
        } else {
            println!("probe: AUTH ok");
        }
        self.start_streaming();
    }

    /// Ends a run whose authentication failed: PASS only if `--expect-auth-failure` named this reason.
    fn conclude_auth_failure(&mut self, reason: &str, bye: Option<&str>) {
        if let Some(bye) = bye {
            println!("probe: BYE {bye}");
        }
        if reason == ByeReason::TooManyAttempts.as_str() && bye != Some(reason) {
            self.failures.push(format!("expected BYE too_many_attempts, got {}", bye.unwrap_or("none")));
        }
        if self.options.expect_auth_failure.as_deref() == Some(reason) {
            println!("probe: authentication failed with {reason}, as expected");
        } else {
            self.failures.push(format!("authentication failed: {reason}"));
        }
        self.finish();
    }

    fn handle_stream_config(&mut self, config: StreamConfig) {
        if self.phase != Phase::Streaming {
            return self.fail("STREAM_CONFIG before authentication");
        }
        let Some(codec) = VideoCodec::parse(&config.codec) else {
            return self.fail(format!("unknown codec {}", config.codec));
        };
        println!("probe: STREAM_CONFIG {} {}x{}@{}", config.codec, config.width, config.height, config.fps);
        let offered = self.options.codecs();
        match offered.iter().find(|c| VideoCodec::parse(&c.name) == Some(codec)) {
            None => self.violation(format!("STREAM_CONFIG codec {codec} was not offered")),
            Some(info) => {
                let (w, h) = (info.max_width.max(info.max_height), info.max_width.min(info.max_height));
                if config.width > w || config.height > h {
                    self.violation(format!("{}x{} exceeds the decoder limit {w}x{h}", config.width, config.height));
                }
            }
        }
        if config.width <= 0 || config.height <= 0 || config.width % 2 != 0 || config.height % 2 != 0 {
            self.violation(format!("stream size {}x{} must be positive and even", config.width, config.height));
        }
        let panel = StreamPlanner::effective_fps(120, self.options.refresh_rate);
        let max_fps = StreamPlanner::limited_fps(panel, self.options.limits.as_ref());
        if config.fps < 1 || config.fps > max_fps {
            self.violation(format!("STREAM_CONFIG fps {} outside 1…{max_fps}", config.fps));
        }
        if let Some(expected) = self.options.expect_codec
            && expected != codec
        {
            self.violation(format!("stream codec {codec}, expected {expected}"));
        }
        self.decoder = None;
        self.decoding = self.options.decode && codec == VideoCodec::H264;
        if self.decoding {
            self.decoder = FrameDecoder::new();
            self.decoding = self.decoder.is_some();
        }
        self.config = Some((codec, config));
        self.expect_keyframe = true;
        self.stats.configs += 1;
        self.stats.restart_keyframe_cadence();
        // Like the phone: ask for a keyframe after every decoder (re)configuration.
        self.keyframe_requested_at = Some(Instant::now());
        self.send(ClientMessage::KeyframeRequest);
    }

    fn handle_video(&mut self, video: VideoFrame, wire_bytes: usize) {
        let Some((codec, config)) = self.config.clone() else { return self.fail("VIDEO_FRAME before STREAM_CONFIG") };
        let seq = video.seq;
        self.send(ClientMessage::FrameAck(seq));
        match self.expected_seq {
            Some(expected) if seq != expected => self.stats.seq_gaps += 1,
            None if seq != 0 => self.violation(format!("first seq is {seq}, expected 0")),
            _ => {}
        }
        self.expected_seq = Some(seq.wrapping_add(1));
        if video.flags & !VideoFrame::KEYFRAME_FLAG != 0 {
            self.violation(format!("reserved flag bits set: {:#04x}", video.flags));
        }
        if self.expect_keyframe && !video.is_keyframe() {
            self.violation(format!("frame {seq} after STREAM_CONFIG is not a keyframe"));
        }
        self.expect_keyframe = false;
        let now = Instant::now();
        if video.is_keyframe()
            && let Some(asked) = self.keyframe_requested_at.take()
        {
            println!("probe: keyframe arrived {} ms after KEYFRAME_REQUEST", (now - asked).as_millis());
        }
        if let Err(error) = inspect_access_unit(codec, &video.data, video.is_keyframe()) {
            if self.stats.structure_errors < 5 {
                println!("probe: VIOLATION frame {seq}: {error}");
            }
            self.stats.structure_errors += 1;
        }
        if self.decoding
            && let Some(decoder) = &mut self.decoder
        {
            match decoder.decode(&video.data) {
                Ok(Some((w, h))) => {
                    self.stats.decoded += 1;
                    if w as i64 != config.width || h as i64 != config.height {
                        self.stats.size_mismatches += 1;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if self.stats.decode_errors < 5 {
                        println!("probe: decode error on frame {seq}: {error}");
                    }
                    self.stats.decode_errors += 1;
                }
            }
        }
        if !self.stats_started {
            self.stats_started = true;
            self.stats.start(now);
        }
        self.stats.record_frame(wire_bytes, video.is_keyframe(), now);
        if !self.input_started {
            self.input_started = true;
            self.start_input();
        }
    }

    fn handle_cursor(&mut self, position: CursorPosition) {
        if self.options.input_mode != Some(sidecast_core::protocol::InputMode::Trackpad) {
            return self.violation("CURSOR received although the probe is not in trackpad mode");
        }
        self.cursor_count += 1;
        let second = self.streaming_since.map_or(0, |s| s.elapsed().as_secs());
        *self.cursor_per_second.entry(second).or_default() += 1;
        self.last_cursor = Some(position);
        let shown = if position.is_on_display() { "" } else { " (pointer not on this display)" };
        println!("probe: CURSOR #{} x={:.4} y={:.4}{shown}", self.cursor_count, position.x, position.y);
    }

    fn handle_mic_control(&mut self, control: MicControl) {
        println!(
            "probe: MIC_CONTROL active={}{}{}",
            control.active,
            control.sample_rate.map_or(String::new(), |r| format!(" sampleRate={r}")),
            control.channels.map_or(String::new(), |c| format!(" channels={c}"))
        );
        if !self.options.mic {
            return self.violation("MIC_CONTROL sent to a client that did not advertise mic");
        }
        if self.phase != Phase::Streaming {
            return self.violation("MIC_CONTROL before streaming");
        }
        self.mic_controls.push(control.active);
        if control.active {
            if control.sample_rate.is_some_and(|r| r != AudioPacket::SAMPLE_RATE as i64) {
                self.violation(format!("MIC_CONTROL sampleRate {:?}", control.sample_rate));
            }
            if control.channels.is_some_and(|c| c != AudioPacket::CHANNELS as i64) {
                self.violation(format!("MIC_CONTROL channels {:?}", control.channels));
            }
            self.mic = Some(Mic { started: Instant::now(), seq: 0, sample_index: 0 });
            self.send(ClientMessage::MicState(MicState::capturing()));
            println!("probe: MIC_STATE capturing=true; sending a 440 Hz tone every 10 ms");
        } else if self.mic.take().is_some() {
            self.send(ClientMessage::MicState(MicState::stopped(MicStateReason::Stopped)));
            println!("probe: MIC_STATE capturing=false reason=stopped");
        }
    }

    // MARK: - Streaming

    fn start_streaming(&mut self) {
        self.phase = Phase::Streaming;
        self.streaming_since = Some(Instant::now());
        if let Some(mode) = self.options.input_mode {
            println!("probe: sending INPUT_MODE {}", mode.as_str());
            self.send(ClientMessage::InputMode(InputModeMessage::new(mode)));
        }
        if self.options.keyframe_request {
            // Sent mid-run by `on_deadline`.
            self.keyframe_requested_at = None;
        }
    }

    fn now_micros(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn send_ping(&mut self) {
        if self.stalled_at.is_some() {
            return;
        }
        let t = self.now_micros();
        self.pings.insert(t);
        self.send(ClientMessage::Ping(t));
    }

    /// The input script, once frames flow: gestures (default: a tap), TEXT and KEY.
    fn start_input(&mut self) {
        let mut keyboard = self.options.keyboard.clone();
        let mut gestures = self.options.gestures.clone();
        if self.options.input && keyboard.is_empty() && gestures.is_empty() {
            gestures.push(ProbeGesture::Tap);
            keyboard = vec![
                KeyboardInput::Text("héllo 👋".into()),
                KeyboardInput::Key(KeyEvent::new(ProtocolKey::Return, KeyModifiers::NONE)),
                KeyboardInput::Key(KeyEvent::new(ProtocolKey::Left, KeyModifiers::SHIFT)),
            ];
        }
        for input in keyboard {
            match input {
                KeyboardInput::Text(text) => {
                    println!("probe: sending TEXT {text:?} ({} bytes)", text.len());
                    self.send(ClientMessage::Text(text));
                }
                KeyboardInput::Key(event) => {
                    println!("probe: sending KEY key={} modifiers=0x{:02x}", event.key, event.modifiers);
                    self.send(ClientMessage::Key(event));
                }
            }
        }
        // Gestures in real time, 600 ms apart (beyond the double-tap window).
        let base = (self.now_micros() / 1000) as u32;
        let mut offset = 200u32;
        for gesture in gestures {
            let steps = gesture.steps();
            let last = steps.last().map_or(0, |s| s.ms);
            self.script.push(Scheduled {
                at: Duration::from_millis(offset.into()),
                action: ScriptAction::Log(format!(
                    "probe: gesture {}: {} TOUCH events over {last} ms (expect: {})",
                    gesture.name(),
                    steps.len(),
                    gesture.expected()
                )),
            });
            for step in steps {
                let ms = offset + step.ms;
                let mut event =
                    TouchEvent::new(base.wrapping_add(ms), step.action, step.action_pointer_id, step.pointers);
                event.event_time_ms = base.wrapping_add(ms);
                self.script
                    .push(Scheduled { at: Duration::from_millis(ms.into()), action: ScriptAction::Touch(event) });
            }
            offset += last + 600;
        }
        self.script_start = Some(Instant::now());
    }

    fn has_fast_work(&self) -> bool {
        self.phase == Phase::Streaming && (self.script_next < self.script.len() || self.mic.is_some())
    }

    /// Plays due gesture steps and sends due microphone packets (paced by the clock).
    fn fast_tick(&mut self) {
        if let Some(start) = self.script_start {
            let elapsed = start.elapsed();
            while self.script_next < self.script.len() && self.script[self.script_next].at <= elapsed {
                match &self.script[self.script_next].action {
                    ScriptAction::Log(line) => println!("{line}"),
                    ScriptAction::Touch(event) => self.send(ClientMessage::Touch(event.clone())),
                }
                self.script_next += 1;
            }
        }
        if let Some(mic) = &mut self.mic {
            let frames = AudioPacket::RECOMMENDED_SAMPLES;
            let due = (mic.started.elapsed().as_secs_f64() / 0.010) as u32 + 1;
            let mut packets = Vec::new();
            while mic.seq < due {
                let samples = (0..frames as u64)
                    .map(|i| {
                        let phase = 2.0 * PI * 440.0 * (mic.sample_index + i) as f64 / AudioPacket::SAMPLE_RATE as f64;
                        (phase.sin() * 16_000.0) as i16
                    })
                    .collect();
                mic.sample_index += frames as u64;
                packets.push(AudioPacket { seq: mic.seq, samples });
                mic.seq += 1;
            }
            self.mic_packets += packets.len();
            for packet in packets {
                self.send(ClientMessage::Audio(packet));
            }
        }
    }

    // MARK: - Timers

    fn next_deadline(&self) -> tokio::time::Instant {
        let mut deadline = Instant::now() + Duration::from_secs(3600);
        if let Some((_, since)) = &self.auth_failure {
            deadline = deadline.min(*since + Duration::from_secs(3));
        }
        if let Some(since) = self.streaming_since {
            match (self.stalled_at, self.options.stall_after) {
                (Some(stalled), _) => deadline = deadline.min(stalled + Duration::from_secs(10)),
                (None, Some(stall)) => deadline = deadline.min(since + Duration::from_secs_f64(stall)),
                (None, None) => deadline = deadline.min(since + Duration::from_secs_f64(self.options.seconds)),
            }
            if self.options.keyframe_request && self.stats_started {
                deadline = deadline.min(since + Duration::from_secs_f64(self.options.seconds / 2.0));
            }
        }
        tokio::time::Instant::from_std(deadline)
    }

    fn on_deadline(&mut self) {
        let now = Instant::now();
        if let Some((reason, since)) = self.auth_failure.clone()
            && now >= since + Duration::from_secs(3)
        {
            return self.fail(format!("no BYE within 3 s after AUTH_RESULT {reason}"));
        }
        let Some(since) = self.streaming_since else { return };
        if self.options.keyframe_request
            && self.stats_started
            && self.keyframe_requested_at.is_none()
            && now >= since + Duration::from_secs_f64(self.options.seconds / 2.0)
        {
            println!("probe: sending KEYFRAME_REQUEST");
            self.keyframe_requested_at = Some(now);
            self.send(ClientMessage::KeyframeRequest);
            self.options.keyframe_request = false;
        }
        match (self.stalled_at, self.options.stall_after) {
            (Some(stalled), _) if now >= stalled + Duration::from_secs(10) => {
                self.fail("server did not drop the silent client within 10 s")
            }
            (None, Some(stall)) if now >= since + Duration::from_secs_f64(stall) => {
                println!("probe: going silent (no more acks or pings)");
                self.stalled_at = Some(now);
            }
            (None, None) if now >= since + Duration::from_secs_f64(self.options.seconds) => self.complete(),
            _ => {}
        }
    }

    fn closed(&mut self, error: Option<String>) {
        if self.phase == Phase::Finished {
            return;
        }
        if let Some((reason, _)) = &self.auth_failure {
            let reason = reason.clone();
            return self.fail(format!("connection closed without BYE after AUTH_RESULT {reason}"));
        }
        if let Some(stalled) = self.stalled_at
            && self.phase == Phase::Streaming
        {
            // Liveness check: the server must drop a silent client after ~5 s.
            let silent = stalled.elapsed().as_secs_f64();
            println!("probe: server closed the connection {silent:.1} s after we went silent");
            if !(4.5..=7.5).contains(&silent) {
                self.failures.push(format!("liveness timeout was {silent:.1} s, expected ~5 s"));
            }
            return self.finish();
        }
        self.fail(format!("connection closed unexpectedly{}", error.map(|e| format!(": {e}")).unwrap_or_default()));
    }

    fn complete(&mut self) {
        if self.phase != Phase::Streaming {
            return;
        }
        if let Some((_, config)) = &self.config {
            println!("probe: stream {} {}x{}@{}", config.codec, config.width, config.height, config.fps);
        }
        if self.options.input_mode == Some(sidecast_core::protocol::InputMode::Trackpad) {
            let max_rate = self.cursor_per_second.values().copied().max().unwrap_or(0);
            println!(
                "probe: CURSOR received {}, max {max_rate} in one second, last {}",
                self.cursor_count,
                self.last_cursor.map_or("-".to_owned(), |c| format!("({:.4}, {:.4})", c.x, c.y))
            );
            if max_rate > 62 {
                self.failures.push(format!("CURSOR rate {max_rate}/s exceeds 60/s"));
            }
        }
        if self.options.mic {
            let controls: Vec<&str> = self.mic_controls.iter().map(|a| if *a { "on" } else { "off" }).collect();
            println!(
                "probe: MIC controls received [{}], AUDIO packets sent {} ({} samples)",
                controls.join(","),
                self.mic_packets,
                self.mic_packets * AudioPacket::RECOMMENDED_SAMPLES
            );
        }
        let problems = self.stats.problems(self.decoding);
        self.failures.extend(problems);
        if self.keyframe_requested_at.is_some() {
            self.failures.push("no keyframe after KEYFRAME_REQUEST".into());
        }
        for line in self.stats.summary() {
            println!("{line}");
        }
        self.finish();
    }

    fn report(&self) -> i32 {
        if self.failures.is_empty() {
            println!("probe: RESULT PASS");
            0
        } else {
            for failure in &self.failures {
                println!("probe: FAILURE {failure}");
            }
            println!("probe: RESULT FAIL");
            1
        }
    }
}
