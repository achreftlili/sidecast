//! The session state machine of PROTOCOL §5–§7, end to end over TLS with
//! fake platform backends: pairing, authentication, takeover, flow control,
//! input, microphone, timeouts and BYE reasons.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::*;
use sidecast_core::annexb::inspect_access_unit;
use sidecast_core::input::{InputEvent, Key, MouseButton, PointerEventKind};
use sidecast_core::protocol::{
    AudioPacket, AuthRequest, ClientMessage, HostPlatform, InputMode, InputModeMessage, KeyEvent, KeyModifiers,
    MessageType, MicState, ProtocolKey, ServerMessage, TouchAction, TouchEvent, TouchPointer, VideoFrame,
};
use sidecast_core::streaming::{StreamSettings, VideoCodec};
use sidecast_server::ServerEvent;
use sidecast_server::backend::BackendError;

fn touch(t: u32, action: TouchAction, x: f32, y: f32) -> ClientMessage {
    ClientMessage::Touch(TouchEvent::new(t, action, 0, vec![TouchPointer::new(0, x, y)]))
}

#[tokio::test]
async fn pairs_streams_and_authenticates_again() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;

    let mut phone = TestClient::connect("phone-a").await;
    let hello = phone.hello(&[]).await;
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.server_name, "Test host");
    assert_eq!(hello.port, Some(port().into()));
    assert_eq!(hello.host_platform(), HostPlatform::current());
    assert_eq!(phone.fingerprint, server.handle.fingerprint());

    phone.send(ClientMessage::PairStart).await;
    let challenge = phone
        .expect("PAIR_CHALLENGE", |m| match m {
            ServerMessage::PairChallenge(c) => Some(*c),
            _ => None,
        })
        .await;
    assert_eq!((challenge.expires_in_sec, challenge.attempts_left), (120, 5));
    // A wrong code costs an attempt; the window stays open.
    let wrong = phone.proof("654321");
    phone.send(ClientMessage::PairProof(wrong)).await;
    let result = phone.auth_result().await;
    assert_eq!((result.ok, result.reason.as_deref(), result.attempts_left), (false, Some("bad_code"), Some(4)));
    let right = phone.proof(CODE);
    phone.send(ClientMessage::PairProof(right)).await;
    let result = phone.auth_result().await;
    assert!(result.ok);
    let token = result.token.expect("token");
    assert_eq!(token.len(), 32);

    let config = phone.stream_config().await;
    assert_eq!((config.codec.as_str(), config.width, config.height, config.fps), ("h264", 1920, 1080, 60));
    let first = phone.video_frame().await;
    assert_eq!(first.seq, 0);
    assert!(first.is_keyframe());
    inspect_access_unit(VideoCodec::H264, &first.data, true).unwrap();
    phone.send(ClientMessage::FrameAck(0)).await;
    phone.send(ClientMessage::Ping(42)).await;
    phone.expect("PONG", |m| matches!(m, ServerMessage::Pong(42)).then_some(())).await;
    // Sequence numbers increase by one.
    let next = phone.video_frame().await;
    assert_eq!(next.seq, 1);
    drop(phone);

    assert_eq!(server.handle.paired_devices().iter().map(|d| d.device_id.as_str()).collect::<Vec<_>>(), ["phone-a"]);

    // Reconnect with the token: AUTH, no new token, streaming restarts at seq 0.
    let mut again = TestClient::connect("phone-a").await;
    again.hello(&[]).await;
    again.send(ClientMessage::Auth(AuthRequest { device_id: "phone-a".into(), token })).await;
    let result = again.auth_result().await;
    assert!(result.ok && result.token.is_none());
    again.stream_config().await;
    assert_eq!(again.video_frame().await.seq, 0);

    let events = server.events.lock().unwrap().clone();
    assert!(events.iter().any(|e| matches!(e, ServerEvent::PairingStarted { code, .. } if code.digits() == CODE)));
    assert!(events.iter().any(|e| matches!(e, ServerEvent::PairingEnded { paired: true, .. })));
    assert!(events.iter().any(|e| matches!(e, ServerEvent::StreamingStarted { .. })));
    server.stop().await;
}

#[tokio::test]
async fn five_wrong_codes_lock_the_pairing_out() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("p").await;
    phone.hello(&[]).await;
    phone.send(ClientMessage::PairStart).await;
    phone.expect("PAIR_CHALLENGE", |m| matches!(m, ServerMessage::PairChallenge(_)).then_some(())).await;
    for left in (1..=4).rev() {
        let proof = phone.proof("000000");
        phone.send(ClientMessage::PairProof(proof)).await;
        assert_eq!(phone.auth_result().await.attempts_left, Some(left));
    }
    let proof = phone.proof("000000");
    phone.send(ClientMessage::PairProof(proof)).await;
    let result = phone.auth_result().await;
    assert_eq!((result.reason.as_deref(), result.attempts_left), (Some("too_many_attempts"), Some(0)));
    assert_eq!(phone.expect_close().await.as_deref(), Some("too_many_attempts"));
    assert!(server.handle.paired_devices().is_empty());
    server.stop().await;
}

#[tokio::test]
async fn unknown_devices_versions_and_early_messages_are_refused() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;

    let mut phone = TestClient::connect("stranger").await;
    phone.hello(&[]).await;
    phone.send(ClientMessage::Auth(AuthRequest { device_id: "stranger".into(), token: vec![1; 32] })).await;
    let result = phone.auth_result().await;
    assert_eq!((result.ok, result.reason.as_deref()), (false, Some("unknown_device")));
    assert_eq!(phone.expect_close().await.as_deref(), Some("unknown_device"));

    let mut future = TestClient::connect("v2").await;
    future.send_raw(&frame(MessageType::Hello as u8, br#"{"protocol":2,"somethingNew":true}"#)).await;
    assert_eq!(future.expect_close().await.as_deref(), Some("protocol_mismatch"));

    let mut early = TestClient::connect("early").await;
    early.send(ClientMessage::Ping(1)).await;
    assert_eq!(early.expect_close().await.as_deref(), Some("protocol_mismatch"));

    // A declared payload above 16 MiB closes the connection without a BYE.
    let mut huge = TestClient::connect("huge").await;
    huge.send_raw(&[0x42, 0x01, 0x00, 0x00, 0x01]).await;
    assert_eq!(huge.expect_close().await, None);
    server.stop().await;
}

#[tokio::test]
async fn an_authenticated_phone_takes_over_and_unknown_types_are_skipped() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut a = TestClient::connect("a").await;
    a.pair(&[]).await;
    a.stream_config().await;
    // Unknown message types are skipped, not fatal.
    a.send_raw(&frame(0x42, &[1, 2, 3])).await;
    a.send(ClientMessage::Ping(7)).await;
    a.expect("PONG", |m| matches!(m, ServerMessage::Pong(7)).then_some(())).await;

    // A second phone that only connects does not disturb the stream…
    let mut idle = TestClient::connect("idle").await;
    idle.hello(&[]).await;
    a.video_frame().await;

    // …but an authenticated one replaces it.
    let mut b = TestClient::connect("b").await;
    b.pair(&[]).await;
    assert_eq!(a.expect_close().await.as_deref(), Some("replaced"));
    b.stream_config().await;
    assert!(b.video_frame().await.is_keyframe());
    server.stop().await;
}

#[tokio::test]
async fn flow_control_keeps_at_most_four_frames_in_flight() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("slow").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    // Frames come every 5 ms; without acks only four may be out.
    let mut unacked: Vec<VideoFrame> = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), phone.next()).await {
            Ok(Some(ServerMessage::VideoFrame(frame))) => unacked.push(frame),
            Ok(Some(_)) => {}
            Ok(None) => panic!("closed"),
            Err(_) => {}
        }
    }
    assert_eq!(unacked.len(), 4, "exactly four frames in flight");
    assert_eq!(unacked.iter().map(|f| f.seq).collect::<Vec<_>>(), [0, 1, 2, 3]);
    // A cumulative ack frees the window; the stream continues without a gap.
    phone.send(ClientMessage::FrameAck(3)).await;
    assert_eq!(phone.video_frame().await.seq, 4);
    // A keyframe request is answered with a keyframe.
    phone.send(ClientMessage::FrameAck(4)).await;
    phone.send(ClientMessage::KeyframeRequest).await;
    let mut keyframe = false;
    for _ in 0..10 {
        let frame = phone.video_frame().await;
        phone.send(ClientMessage::FrameAck(frame.seq)).await;
        if frame.is_keyframe() {
            keyframe = true;
            break;
        }
    }
    assert!(keyframe, "no keyframe after KEYFRAME_REQUEST");
    server.stop().await;
}

#[tokio::test]
async fn input_reaches_the_sink_with_human_like_clicks() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("typist").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    phone.video_frame().await; // the input sink exists once the stream runs

    phone.send(touch(1000, TouchAction::Down, 0.5, 0.5)).await;
    phone.send(touch(1060, TouchAction::Up, 0.5, 0.5)).await;
    phone.send(ClientMessage::Text("a\tb 👋".into())).await;
    phone.send(ClientMessage::Key(KeyEvent::new(ProtocolKey::Left, KeyModifiers::SHIFT))).await;
    phone.send(ClientMessage::Key(KeyEvent::raw(99, 0))).await; // unknown: ignored
    // Malformed input is ignored, never fatal (PROTOCOL §7.1).
    phone.send_raw(&frame(MessageType::Text as u8, &[0xFF])).await;
    phone.send_raw(&frame(MessageType::Key as u8, &[1])).await;
    phone.send_raw(&frame(MessageType::Touch as u8, &[0, 0, 0, 1, 0, 0, 0])).await;
    phone.send(ClientMessage::Ping(9)).await;
    phone.expect("PONG", |m| matches!(m, ServerMessage::Pong(9)).then_some(())).await;
    tokio::time::sleep(Duration::from_millis(200)).await; // the click holds the button 40 ms

    let events = server.input.events.lock().unwrap().clone();
    let pointer: Vec<(PointerEventKind, Option<u32>)> = events
        .iter()
        .filter_map(|e| match e {
            InputEvent::Pointer(p) => Some((p.kind, p.click_count)),
            _ => None,
        })
        .collect();
    // DOWN moves the pointer; the tap is move → press → release with click count 1.
    assert_eq!(
        pointer,
        [
            (PointerEventKind::Moved, None),
            (PointerEventKind::Moved, None),
            (PointerEventKind::ButtonDown(MouseButton::Left), Some(1)),
            (PointerEventKind::ButtonUp(MouseButton::Left), Some(1)),
        ]
    );
    // 1920×1080 at 2× HiDPI is a 960×540 pt display: the centre.
    let InputEvent::Pointer(press) = &events[2] else { panic!() };
    assert_eq!((press.at.x, press.at.y), (480.0, 270.0));
    let rest: Vec<&InputEvent> = events.iter().filter(|e| !matches!(e, InputEvent::Pointer(_))).collect();
    assert_eq!(
        rest,
        [&InputEvent::Text("ab 👋".into()), &InputEvent::Key { key: Key::Left, modifiers: KeyModifiers::SHIFT }]
    );
    // Disconnect releases everything the sink held.
    let input = server.input.clone();
    server.stop().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(input.released.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn trackpad_mode_reports_the_pointer() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("trackpad").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    phone.video_frame().await;
    phone.send(ClientMessage::InputMode(InputModeMessage::new(InputMode::Trackpad))).await;
    // Switching reports the pointer at once: not on this display yet.
    let first = phone
        .expect("CURSOR", |m| match m {
            ServerMessage::Cursor(c) => Some(*c),
            _ => None,
        })
        .await;
    assert!(!first.is_on_display());
    // A tap clicks at the trackpad cursor (the display centre at first) and reports it.
    phone.send(touch(1000, TouchAction::Down, 0.9, 0.1)).await;
    phone.send(touch(1060, TouchAction::Up, 0.9, 0.1)).await;
    let next = phone
        .expect("CURSOR", |m| match m {
            ServerMessage::Cursor(c) => Some(*c),
            _ => None,
        })
        .await;
    assert_eq!((next.x, next.y), (0.5, 0.5));
    assert!(
        server.events.lock().unwrap().iter().any(|e| matches!(e, ServerEvent::InputModeChanged(InputMode::Trackpad)))
    );
    server.stop().await;
}

#[tokio::test]
async fn microphone_is_requested_and_audio_routed() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    // AUDIO before authentication is ignored.
    let mut phone = TestClient::connect("singer").await;
    phone.hello(&[mic_feature()]).await;
    phone.send(ClientMessage::Audio(AudioPacket { seq: 0, samples: vec![1; 480] })).await;
    phone.send(ClientMessage::PairStart).await;
    phone.expect("PAIR_CHALLENGE", |m| matches!(m, ServerMessage::PairChallenge(_)).then_some(())).await;
    let proof = phone.proof(CODE);
    phone.send(ClientMessage::PairProof(proof)).await;
    assert!(phone.auth_result().await.ok);
    phone.stream_config().await;
    let control = phone
        .expect("MIC_CONTROL", |m| match m {
            ServerMessage::MicControl(c) => Some(*c),
            _ => None,
        })
        .await;
    assert!(control.active);
    assert_eq!((control.sample_rate, control.channels), (Some(48_000), Some(1)));
    phone.send(ClientMessage::MicState(MicState::capturing())).await;
    for seq in 0..3 {
        phone.send(ClientMessage::Audio(AudioPacket { seq, samples: vec![seq as i16; 480] })).await;
    }
    // Malformed audio is ignored.
    phone.send_raw(&frame(MessageType::Audio as u8, &[0, 0, 0, 9, 1])).await;
    phone.send(ClientMessage::Ping(3)).await;
    phone.expect("PONG", |m| matches!(m, ServerMessage::Pong(3)).then_some(())).await;
    assert_eq!(server.mic.audio.lock().unwrap().iter().map(|p| p.seq).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(server.mic.states.lock().unwrap().clone(), [MicState::capturing()]);
    assert_eq!(server.mic.started.lock().unwrap().len(), 1);
    drop(phone);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.mic.ended.lock().unwrap().len(), 1);
    server.stop().await;
}

#[tokio::test]
async fn phones_without_mic_never_get_mic_control() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("quiet").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    for _ in 0..20 {
        match phone.next().await.expect("open") {
            ServerMessage::MicControl(_) => panic!("MIC_CONTROL sent to a phone without the mic feature"),
            ServerMessage::VideoFrame(frame) => phone.send(ClientMessage::FrameAck(frame.seq)).await,
            _ => {}
        }
    }
    assert_eq!(server.mic.started.lock().unwrap().clone().into_iter().map(|(_, mic)| mic).collect::<Vec<_>>(), [false]);
    server.stop().await;
}

#[tokio::test]
async fn a_silent_streaming_client_is_dropped_after_five_seconds() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("silent").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    phone.auto_ack = false;
    let silent_since = Instant::now();
    assert_eq!(phone.expect_close().await, None, "closed without a BYE");
    let silent = silent_since.elapsed().as_secs_f64();
    assert!((4.5..7.5).contains(&silent), "dropped after {silent:.1} s");
    server.stop().await;
}

#[tokio::test]
async fn settings_changes_reconfigure_the_stream() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("settings").await;
    phone.pair(&[]).await;
    assert_eq!(phone.stream_config().await.fps, 60);
    phone.video_frame().await;
    server.handle.update_settings(StreamSettings { fps: 30, max_bitrate_mbps: 10, ..StreamSettings::default() });
    let config = phone.stream_config().await;
    assert_eq!(config.fps, 30);
    // The new encoder starts with a keyframe.
    let frame = phone
        .expect("a frame", |m| match m {
            ServerMessage::VideoFrame(f) => Some(f.clone()),
            _ => None,
        })
        .await;
    assert!(frame.is_keyframe());
    assert_eq!(server.display.starts.load(Ordering::Relaxed), 2);
    assert!(server.encoders.bitrates.lock().unwrap().iter().all(|b| *b <= 20_000_000));
    server.stop().await;
}

#[tokio::test]
async fn forgetting_a_device_and_shutting_down_say_bye() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay::default()).await;
    let mut phone = TestClient::connect("forgotten").await;
    phone.pair(&[]).await;
    phone.stream_config().await;
    assert!(server.handle.forget_device("forgotten").unwrap());
    assert_eq!(phone.expect_close().await.as_deref(), Some("unknown_device"));
    assert!(server.handle.paired_devices().is_empty());

    let mut other = TestClient::connect("other").await;
    other.pair(&[]).await;
    other.stream_config().await;
    let handle_dir = server.data_dir.clone();
    server.handle.shutdown().await;
    assert_eq!(other.expect_close().await.as_deref(), Some("server_shutdown"));
    let _ = std::fs::remove_dir_all(handle_dir);
}

#[tokio::test]
async fn a_backend_that_cannot_capture_says_why() {
    let _port = PORT_LOCK.lock().await;
    let server = start_server(FakeDisplay {
        refuse: Some(BackendError::permission_missing("screen capture not allowed")),
        ..FakeDisplay::default()
    })
    .await;
    let mut phone = TestClient::connect("blocked").await;
    phone.pair(&[]).await;
    assert_eq!(phone.expect_close().await.as_deref(), Some("permission_missing"));
    server.stop().await;
}
