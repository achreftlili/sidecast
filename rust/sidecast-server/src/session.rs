//! One phone connection: the PROTOCOL §5 state machine (HELLO →
//! SERVER_HELLO → AUTH | PAIR_START/PAIR_PROOF → streaming), ported from the
//! Mac app's `ClientSession.swift`. Everything runs on the connection's task.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use sidecast_core::protocol::{
    AUTH_TIMEOUT, AuthResult, Bye, ByeReason, ClientMessage, Frame, FrameParser, Hello, MessageType, MicControl,
    PAIRING_ATTEMPTS, PAIRING_LIFETIME, PairChallenge, PairProof, ProtocolError, ServerMessage,
};
use sidecast_core::security::{PairingOutcome, PairingSession, SystemRandom, device_token};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_rustls::TlsAcceptor;

use crate::backend::{ClientInfo, SessionId};
use crate::clock::{instant_at, now_secs};
use crate::outbox::Outbox;
use crate::server::Hub;
use crate::stream::{StartOutcome, StreamController, StreamEvent, StreamFailure};

/// Requests from the hub (other sessions, settings, the microphone backend).
pub(crate) enum SessionCommand {
    Bye(ByeReason),
    /// Another phone started pairing: this pairing window ends.
    ExpirePairing,
    MicControl(bool),
    Settings(sidecast_core::streaming::StreamSettings),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingHello,
    AwaitingAuth,
    Pairing,
    Authenticated,
    Closed,
}

/// Runs one connection to its end.
pub(crate) async fn run(tcp: TcpStream, peer: SocketAddr, hub: Arc<Hub>, acceptor: TlsAcceptor) {
    let (commands_tx, mut commands) = unbounded_channel();
    let Some(id) = hub.register(commands_tx.clone()) else {
        log::info!("server: too many connections, rejecting {peer}");
        return;
    };
    let tls = match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            log::info!("session {peer}: TLS handshake failed: {error}");
            hub.session_closed(id);
            return;
        }
        Err(_) => {
            log::info!("session {peer}: TLS handshake timed out");
            hub.session_closed(id);
            return;
        }
    };
    let (mut reader, writer) = tokio::io::split(tls);
    let (out, writer_task) = Outbox::spawn(writer);
    let (stream_tx, mut stream_events) = unbounded_channel();
    let mut session = Session::new(id, peer, hub.clone(), out, stream_tx, commands_tx);

    let mut parser = FrameParser::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(1), Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while !session.is_closed() {
        let deadline = session.next_deadline();
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) => session.peer_closed(None),
                Ok(n) => {
                    parser.push(&buffer[..n]);
                    loop {
                        match parser.next_frame() {
                            Ok(Some(frame)) => session.handle_frame(frame),
                            Ok(None) => break,
                            Err(error) => {
                                log::error!("session {peer}: protocol error: {error}");
                                session.close();
                            }
                        }
                        if session.is_closed() {
                            break;
                        }
                    }
                }
                Err(error) => session.peer_closed(Some(error)),
            },
            Some(command) = commands.recv() => session.handle_command(command),
            Some(event) = stream_events.recv() => session.handle_stream_event(event),
            _ = tick.tick() => session.tick(),
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                session.on_deadline();
            }
        }
    }
    session.finished();
    drop(session);
    // A capture that finished starting while the session closed: tear it down properly.
    stream_events.close();
    while let Ok(event) = stream_events.try_recv() {
        crate::stream::discard(event);
    }
    // Let a final BYE leave (the writer gives up after 2 s on a stuck peer).
    let _ = tokio::time::timeout(Duration::from_secs(3), writer_task).await;
}

struct Session {
    id: SessionId,
    peer: SocketAddr,
    hub: Arc<Hub>,
    out: Outbox,
    stream_events: UnboundedSender<StreamEvent>,
    commands: UnboundedSender<SessionCommand>,
    phase: Phase,
    hello: Option<Hello>,
    pairing: Option<PairingSession>,
    pairing_deadline: Option<f64>,
    auth_deadline: f64,
    stream: Option<StreamController>,
    /// The last MIC_CONTROL sent was active:true (a new session starts with the mic off).
    mic_requested: bool,
}

impl Session {
    fn new(
        id: SessionId,
        peer: SocketAddr,
        hub: Arc<Hub>,
        out: Outbox,
        stream_events: UnboundedSender<StreamEvent>,
        commands: UnboundedSender<SessionCommand>,
    ) -> Self {
        log::info!("server: connection from {peer}");
        Session {
            id,
            peer,
            hub,
            out,
            stream_events,
            commands,
            phase: Phase::AwaitingHello,
            hello: None,
            pairing: None,
            pairing_deadline: None,
            auth_deadline: now_secs() + AUTH_TIMEOUT,
            stream: None,
            mic_requested: false,
        }
    }

    fn device_name(&self) -> &str {
        self.hello.as_ref().map_or("unknown device", |h| h.device_name.as_str())
    }

    fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }

    fn send(&self, message: ServerMessage) {
        self.out.send(message.encode());
    }

    /// Sends BYE `reason`, then closes.
    fn bye(&mut self, reason: ByeReason) {
        if self.is_closed() {
            return;
        }
        log::info!("session {} ({}): BYE {reason}", self.peer, self.device_name());
        self.tear_down();
        self.send(ServerMessage::Bye(Bye::new(reason)));
        self.out.close();
    }

    /// Closes without a BYE (dead peer, protocol violation, timeout).
    fn close(&mut self) {
        if self.is_closed() {
            return;
        }
        self.tear_down();
        self.out.close();
    }

    fn peer_closed(&mut self, error: Option<std::io::Error>) {
        if let Some(error) = error {
            log::debug!("session {}: connection ended: {error}", self.peer);
        }
        self.close();
    }

    fn tear_down(&mut self) {
        self.mic_requested = false;
        if self.pairing.is_some() {
            self.finish_pairing(false);
        }
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
        self.phase = Phase::Closed;
    }

    /// The task is about to exit.
    fn finished(&mut self) {
        log::info!("session {} ({}): closed", self.peer, self.device_name());
        self.hub.session_closed(self.id);
    }

    // MARK: - Incoming

    fn handle_frame(&mut self, frame: Frame) {
        if let Some(stream) = &mut self.stream {
            stream.note_activity();
        }
        let message = match ClientMessage::decode(&frame) {
            Ok(message) => message,
            Err(ProtocolError::VersionMismatch(version)) => {
                log::info!("session {}: protocol {version} not supported", self.peer);
                return self.bye(ByeReason::ProtocolMismatch);
            }
            Err(error) => {
                use MessageType::*;
                let tolerated = matches!(frame.message_type(), Some(Text | Key | Audio | MicState | InputMode))
                    || (frame.message_type() == Some(Touch) && self.phase == Phase::Authenticated);
                if tolerated {
                    // PROTOCOL §7.1–7.3: malformed input and mic payloads are ignored, never fatal.
                    log::debug!("session {}: ignoring malformed message: {error}", self.peer);
                    return;
                }
                log::error!("session {}: protocol error: {error}", self.peer);
                return self.bye(ByeReason::ProtocolMismatch);
            }
        };
        let Some(message) = message else { return }; // unknown type: skipped

        match (self.phase, message) {
            (Phase::AwaitingHello, ClientMessage::Hello(hello)) => {
                log::info!(
                    "session {}: HELLO from {} screen {}x{} codecs {:?}",
                    self.peer,
                    hello.device_name,
                    hello.screen.width,
                    hello.screen.height,
                    hello
                        .codecs
                        .iter()
                        .map(|c| format!("{} {}x{}", c.name, c.max_width, c.max_height))
                        .collect::<Vec<_>>()
                );
                self.hello = Some(hello);
                self.phase = Phase::AwaitingAuth;
                self.send(ServerMessage::ServerHello(self.hub.server_hello()));
            }
            (Phase::AwaitingHello, ClientMessage::InputMode(_)) => {
                log::debug!("session {}: ignoring INPUT_MODE before HELLO", self.peer);
            }
            (Phase::AwaitingHello, other) => {
                log::error!("session {}: {} before HELLO", self.peer, other.message_type());
                self.bye(ByeReason::ProtocolMismatch);
            }
            (_, ClientMessage::Ping(t)) => self.send(ServerMessage::Pong(t)),
            (Phase::AwaitingAuth | Phase::Pairing, ClientMessage::Auth(request)) => {
                self.handle_auth(&request.device_id, &request.token)
            }
            (Phase::AwaitingAuth | Phase::Pairing, ClientMessage::PairStart) => self.start_pairing(),
            (Phase::Pairing, ClientMessage::PairProof(proof)) => self.handle_proof(proof),
            (Phase::Authenticated, ClientMessage::FrameAck(seq)) => {
                if let Some(stream) = &mut self.stream {
                    stream.handle_ack(seq);
                }
            }
            (Phase::Authenticated, ClientMessage::KeyframeRequest) => {
                if let Some(stream) = &mut self.stream {
                    stream.request_keyframe();
                }
            }
            (Phase::Authenticated, ClientMessage::Touch(event)) => {
                if let Some(stream) = &mut self.stream {
                    stream.handle_touch(event);
                }
            }
            (Phase::Authenticated, ClientMessage::Text(text)) => {
                if let Some(stream) = &mut self.stream {
                    stream.handle_text(text);
                }
            }
            (Phase::Authenticated, ClientMessage::Key(event)) => {
                if let Some(stream) = &mut self.stream {
                    stream.handle_key(event);
                }
            }
            (Phase::Authenticated, ClientMessage::Audio(packet)) => {
                // Without a pending active:true it is ignored (PROTOCOL §7.2); not logged, it would be 100/s.
                if self.mic_requested {
                    self.hub.audio(self.id, &packet);
                }
            }
            (Phase::Authenticated, ClientMessage::MicState(state)) => {
                if self.mic_requested {
                    self.hub.mic_state(self.id, &state);
                } else {
                    log::debug!("session {}: ignoring MIC_STATE (mic not requested)", self.peer);
                }
            }
            (Phase::Authenticated, ClientMessage::InputMode(message)) => match message.input_mode() {
                Some(mode) => {
                    if let Some(stream) = &mut self.stream
                        && stream.set_input_mode(mode)
                    {
                        self.hub.input_mode_changed(mode);
                    }
                }
                None => log::debug!("session {}: ignoring unknown input mode {:?}", self.peer, message.mode),
            },
            (_, ClientMessage::Audio(_)) => {} // before authentication: ignored, not logged
            (phase, other) => log::debug!("session {}: ignoring {} in {phase:?}", self.peer, other.message_type()),
        }
    }

    fn handle_auth(&mut self, device_id: &str, token: &[u8]) {
        if self.phase == Phase::Pairing {
            self.finish_pairing(false);
        }
        let Some(device) = self.hub.store.authenticate(device_id, token) else {
            log::info!("session {}: AUTH failed for device {device_id}", self.peer);
            self.send(ServerMessage::AuthResult(AuthResult::failed(ByeReason::UnknownDevice, None)));
            return self.bye(ByeReason::UnknownDevice);
        };
        self.hub.store.mark_seen(&device.device_id, self.hello.as_ref().map(|h| h.device_name.as_str()));
        log::info!("session {}: authenticated {}", self.peer, device.device_name);
        self.send(ServerMessage::AuthResult(AuthResult::ok(None)));
        self.did_authenticate();
    }

    fn start_pairing(&mut self) {
        let code = self.hub.next_pairing_code();
        let device_name = self.device_name().to_owned();
        self.hub.pairing_started(self.id, &device_name, code.clone());
        let now = now_secs();
        log::info!("session {}: pairing with {device_name}", self.peer);
        self.pairing = Some(PairingSession::new(code, now));
        self.pairing_deadline = Some(now + PAIRING_LIFETIME);
        self.phase = Phase::Pairing;
        self.send(ServerMessage::PairChallenge(PairChallenge {
            attempts_left: PAIRING_ATTEMPTS as i64,
            expires_in_sec: PAIRING_LIFETIME as i64,
        }));
    }

    fn handle_proof(&mut self, proof: PairProof) {
        let (Some(pairing), Some(hello)) = (self.pairing.as_mut(), self.hello.clone()) else { return };
        let outcome = pairing.verify(&proof.nonce, &proof.proof, &self.hub.fingerprint, now_secs());
        match outcome {
            PairingOutcome::Success => {
                let token = device_token::generate(&mut SystemRandom);
                if let Err(error) = self.hub.store.add(&hello.device_id, &hello.device_name, &token) {
                    log::error!("session {}: could not save pairing: {error}", self.peer);
                    self.finish_pairing(false);
                    return self.bye(ByeReason::ServerShutdown);
                }
                log::info!("session {}: paired {}", self.peer, hello.device_name);
                self.finish_pairing(true);
                self.send(ServerMessage::AuthResult(AuthResult::ok(Some(token.to_vec()))));
                self.did_authenticate();
            }
            PairingOutcome::BadCode { attempts_left } => {
                log::info!("session {}: wrong pairing code, {attempts_left} attempts left", self.peer);
                self.send(ServerMessage::AuthResult(AuthResult::failed(
                    ByeReason::BadCode,
                    Some(attempts_left.into()),
                )));
            }
            PairingOutcome::TooManyAttempts => {
                self.send(ServerMessage::AuthResult(AuthResult::failed(ByeReason::TooManyAttempts, Some(0))));
                self.finish_pairing(false);
                self.bye(ByeReason::TooManyAttempts);
            }
            PairingOutcome::Expired => {
                self.send(ServerMessage::AuthResult(AuthResult::failed(ByeReason::PairingExpired, None)));
                self.finish_pairing(false);
                self.bye(ByeReason::PairingExpired);
            }
        }
    }

    /// Ends a pairing that timed out or that another phone's pairing superseded.
    fn expire_pairing(&mut self) {
        if self.phase != Phase::Pairing {
            return;
        }
        self.send(ServerMessage::AuthResult(AuthResult::failed(ByeReason::PairingExpired, None)));
        self.finish_pairing(false);
        self.bye(ByeReason::PairingExpired);
    }

    fn finish_pairing(&mut self, paired: bool) {
        self.pairing_deadline = None;
        if self.pairing.take().is_none() {
            return;
        }
        if self.phase == Phase::Pairing {
            self.phase = Phase::AwaitingAuth;
        }
        self.hub.pairing_ended(self.id, paired);
    }

    fn did_authenticate(&mut self) {
        self.phase = Phase::Authenticated;
        let Some(hello) = self.hello.clone() else { return self.bye(ByeReason::ProtocolMismatch) };
        let (plan, settings) = match self.hub.claim_stream(self.id, &hello) {
            Ok(claim) => claim,
            Err(reason) => return self.bye(reason),
        };
        let client = ClientInfo {
            device_id: hello.device_id.clone(),
            device_name: hello.device_name.clone(),
            screen: hello.screen.clone(),
            codecs: hello.codecs.clone(),
            network: hello.network.clone(),
            limits: hello.limits,
            supports_mic: hello.supports_mic(),
        };
        let mut stream = StreamController::new(
            client,
            plan,
            settings,
            self.hub.backends.clone(),
            self.out.clone(),
            self.stream_events.clone(),
            self.hub.stream_slot.clone(),
            self.hub.keep_awake,
            self.hub.log_stats,
        );
        stream.start();
        self.stream = Some(stream);
    }

    // MARK: - Hub commands and stream events

    fn handle_command(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::Bye(reason) => self.bye(reason),
            SessionCommand::ExpirePairing => self.expire_pairing(),
            SessionCommand::MicControl(active) => self.send_mic_control(active),
            SessionCommand::Settings(settings) => {
                if let Some(stream) = &mut self.stream {
                    stream.update_settings(settings, &self.hub.encoder_capabilities);
                }
            }
        }
    }

    /// MIC_CONTROL only to an authenticated, streaming client that advertised `mic`.
    fn send_mic_control(&mut self, active: bool) {
        let supports_mic = self.hello.as_ref().is_some_and(Hello::supports_mic);
        if self.phase != Phase::Authenticated || self.stream.is_none() || !supports_mic || active == self.mic_requested
        {
            return;
        }
        self.mic_requested = active;
        self.send(ServerMessage::MicControl(if active { MicControl::START } else { MicControl::STOP }));
        log::info!("session {} ({}): MIC_CONTROL active={active}", self.peer, self.device_name());
    }

    fn handle_stream_event(&mut self, event: StreamEvent) {
        let Some(stream) = &mut self.stream else { return crate::stream::discard(event) };
        match event {
            StreamEvent::Started { generation, result } => match stream.on_started(generation, result) {
                Ok(StartOutcome::Configured { first: true }) => {
                    let plan = *stream.plan();
                    if let Some(hello) = self.hello.clone() {
                        let mic = self.commands.clone();
                        self.hub.stream_configured(self.id, &hello, plan, move |active| {
                            let _ = mic.send(SessionCommand::MicControl(active));
                        });
                    }
                }
                Ok(_) => {}
                Err(StreamFailure(reason)) => self.fail(reason),
            },
            StreamEvent::Encoded { generation, frame, pts_micros, requested_keyframe } => {
                stream.on_encoded(generation, frame, pts_micros, requested_keyframe)
            }
            StreamEvent::EncoderFailed { generation, error } => {
                if stream.is_current_encoder(generation) {
                    log::error!("stream: encoder keeps failing: {error}");
                    self.bye(ByeReason::DisplayFailed);
                }
            }
            StreamEvent::Cursor { position, force } => stream.on_cursor(position, force),
        }
    }

    fn fail(&mut self, reason: Option<ByeReason>) {
        match reason {
            Some(reason) => self.bye(reason),
            None => self.close(),
        }
    }

    // MARK: - Timers

    fn tick(&mut self) {
        let Some(stream) = &mut self.stream else { return };
        match stream.tick() {
            Ok(Some(stats)) => self.hub.stats(stats),
            Ok(None) => {}
            Err(StreamFailure(reason)) => self.fail(reason),
        }
    }

    fn next_deadline(&self) -> Option<tokio::time::Instant> {
        let mut deadlines: Vec<std::time::Instant> = Vec::with_capacity(3);
        if self.phase != Phase::Authenticated {
            deadlines.push(instant_at(self.auth_deadline));
        }
        if let Some(at) = self.pairing_deadline {
            deadlines.push(instant_at(at));
        }
        if let Some(at) = self.stream.as_ref().and_then(StreamController::next_deadline) {
            deadlines.push(at);
        }
        deadlines.into_iter().min().map(tokio::time::Instant::from_std)
    }

    fn on_deadline(&mut self) {
        let now = now_secs();
        if self.phase != Phase::Authenticated && self.phase != Phase::Closed && now >= self.auth_deadline {
            log::info!("session {}: not authenticated within {} s — closing", self.peer, AUTH_TIMEOUT as u32);
            return self.close();
        }
        if self.pairing_deadline.is_some_and(|at| now >= at) {
            self.pairing_deadline = None;
            self.expire_pairing();
        }
        if let Some(stream) = &mut self.stream {
            stream.on_deadline();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
    }
}
