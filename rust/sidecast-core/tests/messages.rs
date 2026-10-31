//! Framing and message codecs: ports of `FramerTests.swift`, `MessageTests.swift`,
//! `MicMessageTests.swift` (protocol parts), the INPUT_MODE test of
//! `TrackpadTests.swift` and the SERVER_HELLO / HELLO data-saver tests of
//! `RemoteAccessTests.swift`, plus the SERVER_HELLO `platform` field.

use serde_json::Value;
use sidecast_core::protocol::{
    AddressKind, AudioPacket, AuthRequest, AuthResult, Bye, ByeReason, ClientMessage, ClientNetwork, CodecInfo, Frame,
    FrameParser, Hello, HostPlatform, InputMode, InputModeMessage, MAX_PAYLOAD_LENGTH, MessageType, MicControl,
    MicState, MicStateReason, PairChallenge, ProtocolError, ScreenInfo, ServerAddress, ServerHello, ServerMessage,
    StreamConfig, StreamLimits, TouchAction, TouchEvent, TouchPointer, VideoFrame, json_b64,
};
use sidecast_core::security::to_hex;

fn client(msg_type: MessageType, payload: &[u8]) -> Result<Option<ClientMessage>, ProtocolError> {
    ClientMessage::decode(&Frame::new(msg_type, payload.to_vec()))
}

fn server(msg_type: MessageType, payload: &[u8]) -> Result<Option<ServerMessage>, ProtocolError> {
    ServerMessage::decode(&Frame::new(msg_type, payload.to_vec()))
}

fn round_trip_server(message: &ServerMessage) -> Option<ServerMessage> {
    let mut parser = FrameParser::new();
    parser.push(&message.encode());
    ServerMessage::decode(&parser.next_frame().unwrap().unwrap()).unwrap()
}

fn round_trip_client(message: &ClientMessage) -> Option<ClientMessage> {
    ClientMessage::decode(&message.frame()).unwrap()
}

/// The JSON payload of an encoded server message, as text.
fn server_json(message: &ServerMessage) -> String {
    String::from_utf8(message.encode()[5..].to_vec()).unwrap()
}

fn server_value(message: &ServerMessage) -> Value {
    serde_json::from_slice(&message.encode()[5..]).unwrap()
}

fn hello_payload(message: &Hello) -> String {
    String::from_utf8(ClientMessage::Hello(message.clone()).frame().payload).unwrap()
}

// MARK: - Framer

#[test]
fn byte_by_byte_delivery() {
    let messages =
        [ClientMessage::PairStart, ClientMessage::Ping(42), ClientMessage::FrameAck(9), ClientMessage::KeyframeRequest];
    let wire: Vec<u8> = messages.iter().flat_map(ClientMessage::encode).collect();
    let mut parser = FrameParser::new();
    let mut decoded = Vec::new();
    for byte in wire {
        parser.push(&[byte]);
        while let Some(frame) = parser.next_frame().unwrap() {
            decoded.push(ClientMessage::decode(&frame).unwrap().unwrap());
        }
    }
    assert_eq!(decoded, messages);
    assert_eq!(parser.buffered_len(), 0);
}

#[test]
fn split_across_arbitrary_chunks() {
    let big = VideoFrame::new(1, false, 5, vec![7; 300_000]);
    let wire = [ServerMessage::VideoFrame(big.clone()).encode(), ServerMessage::Pong(3).encode()].concat();
    let mut parser = FrameParser::new();
    let mut frames = Vec::new();
    let sizes = [1, 4, 5, 1000, 65_536, 3];
    let (mut i, mut k) = (0, 0);
    while i < wire.len() {
        let n = sizes[k % sizes.len()].min(wire.len() - i);
        parser.push(&wire[i..i + n]);
        frames.extend(parser.drain().unwrap());
        i += n;
        k += 1;
    }
    assert_eq!(frames.len(), 2);
    assert_eq!(ServerMessage::decode(&frames[0]).unwrap(), Some(ServerMessage::VideoFrame(big)));
    assert_eq!(ServerMessage::decode(&frames[1]).unwrap(), Some(ServerMessage::Pong(3)));
}

#[test]
fn incomplete_header_and_payload_wait() {
    let mut parser = FrameParser::new();
    parser.push(&[0x13, 0x00, 0x00]);
    assert_eq!(parser.next_frame(), Ok(None));
    parser.push(&[0x00, 0x08, 0x00]);
    assert_eq!(parser.next_frame(), Ok(None));
    parser.push(&[0; 7]);
    assert_eq!(parser.next_frame(), Ok(Some(Frame::new(MessageType::Ping, vec![0; 8]))));
}

#[test]
fn oversize_length_is_protocol_error() {
    let mut parser = FrameParser::new();
    // 16 MiB + 1
    parser.push(&[0x85, 0x01, 0x00, 0x00, 0x01]);
    assert_eq!(parser.next_frame(), Err(ProtocolError::PayloadTooLarge(16 * 1024 * 1024 + 1)));
}

#[test]
fn exactly_16_mib_is_allowed() {
    assert_eq!(MAX_PAYLOAD_LENGTH, 16 * 1024 * 1024);
    let mut parser = FrameParser::new();
    parser.push(&[0x7F, 0x01, 0x00, 0x00, 0x00]);
    assert_eq!(parser.next_frame(), Ok(None)); // waits for the payload instead of failing
}

#[test]
fn unknown_types_are_skipped() {
    let unknown = Frame { msg_type: 0x42, payload: vec![1, 2, 3] }.encode();
    let server_type_from_client = Frame::new(MessageType::Bye, b"{}".to_vec()).encode();
    let mut parser = FrameParser::new();
    parser.push(&[unknown, server_type_from_client, ClientMessage::Ping(1).encode()].concat());
    let frames = parser.drain().unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(ClientMessage::decode(&frames[0]), Ok(None));
    assert_eq!(ClientMessage::decode(&frames[1]), Ok(None));
    assert_eq!(ClientMessage::decode(&frames[2]), Ok(Some(ClientMessage::Ping(1))));
    assert_eq!(ServerMessage::decode(&Frame { msg_type: 0x99, payload: Vec::new() }), Ok(None));
}

#[test]
fn empty_payload() {
    let wire = ClientMessage::PairStart.encode();
    assert_eq!(wire, [0x03, 0, 0, 0, 0]);
    let mut parser = FrameParser::new();
    parser.push(&wire);
    assert_eq!(ClientMessage::decode(&parser.next_frame().unwrap().unwrap()), Ok(Some(ClientMessage::PairStart)));
}

#[test]
fn truncated_binary_payloads_throw() {
    assert!(client(MessageType::FrameAck, &[0, 1]).is_err());
    assert!(client(MessageType::Ping, &[0]).is_err());
    assert!(server(MessageType::VideoFrame, &[0; 12]).is_err());
    // TOUCH declaring 2 pointers but carrying one.
    let one = TouchEvent::new(1, TouchAction::Down, 0, vec![TouchPointer::new(0, 0.0, 0.0)]);
    let mut payload = one.encode_payload();
    payload[6] = 2;
    assert!(client(MessageType::Touch, &payload).is_err());
}

// MARK: - HELLO, AUTH, PAIR_PROOF, server JSON

const SAMPLE_HELLO: &str = r#"
{
  "protocol": 1,
  "deviceId": "8c1f0000-1111-2222-3333-444455556666",
  "deviceName": "Pixel 8",
  "screen": { "width": 2400, "height": 1080, "densityDpi": 420, "refreshRate": 120.0 },
  "codecs": [
    { "name": "hevc", "maxWidth": 3840, "maxHeight": 2160 },
    { "name": "h264", "maxWidth": 1920, "maxHeight": 1080 }
  ],
  "futureField": {"anything": [1, 2, 3]}
}
"#;

fn decode_hello(json: &str) -> Hello {
    match client(MessageType::Hello, json.as_bytes()) {
        Ok(Some(ClientMessage::Hello(hello))) => hello,
        other => panic!("expected HELLO, got {other:?}"),
    }
}

#[test]
fn hello_decodes_and_ignores_unknown_keys() {
    let hello = decode_hello(SAMPLE_HELLO);
    assert_eq!(hello.device_name, "Pixel 8");
    assert_eq!(hello.screen, ScreenInfo::new(2400, 1080, 420, 120.0));
    assert_eq!(hello.codecs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["hevc", "h264"]);
}

/// Android's org.json writer escapes "/" as "\/" and writes whole floats as ints.
#[test]
fn hello_from_android_json_writer() {
    let android = r#"{"protocol":1,"deviceId":"a\/b-1","deviceName":"Galaxy S24 \/ work","#.to_owned()
        + r#""screen":{"width":2340,"height":1080,"densityDpi":450,"refreshRate":120},"#
        + r#""codecs":[{"name":"hevc","maxWidth":3840,"maxHeight":2160}]}"#;
    let hello = decode_hello(&android);
    assert_eq!(hello.device_id, "a/b-1");
    assert_eq!(hello.device_name, "Galaxy S24 / work");
    assert_eq!(hello.screen.refresh_rate, 120.0);
    assert_eq!(hello.codecs, [CodecInfo::new("hevc", 3840, 2160)]);
}

#[test]
fn hello_without_optional_keys_uses_defaults() {
    let hello = decode_hello(r#"{"protocol":1,"deviceId":"d","screen":{"width":1920,"height":1080}}"#);
    assert_eq!(hello.device_name, "Android phone");
    assert_eq!(hello.screen, ScreenInfo::new(1920, 1080, 160, 60.0));
    assert!(hello.codecs.is_empty());
    // deviceId and screen stay mandatory.
    assert!(client(MessageType::Hello, br#"{"protocol":1,"deviceId":"d"}"#).is_err());
    assert!(client(MessageType::Hello, br#"{"protocol":1,"screen":{"width":1,"height":1}}"#).is_err());
}

#[test]
fn pair_proof_and_auth_with_escaped_slashes() {
    let proof = br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw==","proof":"40sw5wf5dNH6Tj5a\/cUjw\/yaOatMfUP0KvJ9ywQkSv4="}"#;
    let Ok(Some(ClientMessage::PairProof(p))) = client(MessageType::PairProof, proof) else {
        panic!("expected PAIR_PROOF");
    };
    assert_eq!(to_hex(&p.proof), "e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe");
}

#[test]
fn hello_version_mismatch_is_detected_even_for_other_shapes() {
    let v2 = br#"{"protocol": 2, "somethingNew": true}"#;
    assert_eq!(client(MessageType::Hello, v2), Err(ProtocolError::VersionMismatch(2)));
}

#[test]
fn malformed_json_throws() {
    assert!(client(MessageType::Auth, b"{").is_err());
    assert!(client(MessageType::Auth, br#"{"deviceId":"x"}"#).is_err());
}

#[test]
fn auth_token_base64_tolerates_line_breaks() {
    let token = vec![0xAB; 32];
    // Android's Base64.DEFAULT ends with a line break (a JSON "\n" escape).
    let json = format!(r#"{{"deviceId":"d1","token":"{}\n"}}"#, json_b64::encode(&token));
    assert_eq!(
        client(MessageType::Auth, json.as_bytes()),
        Ok(Some(ClientMessage::Auth(AuthRequest { device_id: "d1".into(), token })))
    );
}

#[test]
fn server_json_shapes() {
    let hello = server_value(&ServerMessage::ServerHello(ServerHello::new("S", "Mac", HostPlatform::Macos)));
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["serverId"], "S");
    assert_eq!(hello["serverName"], "Mac");

    let fail = server_value(&ServerMessage::AuthResult(AuthResult::failed(ByeReason::BadCode, Some(4))));
    assert_eq!(fail["ok"], false);
    assert_eq!(fail["reason"], "bad_code");
    assert_eq!(fail["attemptsLeft"], 4);
    assert!(fail.get("token").is_none());

    let ok = server_value(&ServerMessage::AuthResult(AuthResult::ok(Some(vec![0xFF; 32]))));
    let mut keys: Vec<&String> = ok.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["ok", "token"]);
    assert_eq!(ok["token"], json_b64::encode(&[0xFF; 32]));

    let cfg = server_value(&ServerMessage::StreamConfig(StreamConfig::new("hevc", 2400, 1080, 60)));
    assert_eq!(cfg["codec"], "hevc");
    assert_eq!(cfg["width"], 2400);

    assert_eq!(
        server_value(&ServerMessage::Bye(Bye::new(ByeReason::PermissionMissing)))["reason"],
        "permission_missing"
    );
    let challenge =
        server_value(&ServerMessage::PairChallenge(PairChallenge { attempts_left: 5, expires_in_sec: 120 }));
    assert_eq!(challenge["expiresInSec"], 120);
    assert_eq!(challenge["attemptsLeft"], 5);
}

#[test]
fn round_trip_all_server_messages() {
    let all = [
        ServerMessage::ServerHello(ServerHello::new("id", "n", HostPlatform::Linux)),
        ServerMessage::PairChallenge(PairChallenge { attempts_left: 5, expires_in_sec: 120 }),
        ServerMessage::AuthResult(AuthResult::ok(Some(vec![1, 2, 3]))),
        ServerMessage::StreamConfig(StreamConfig::new("h264", 1920, 864, 60)),
        ServerMessage::VideoFrame(VideoFrame::new(0, false, 0, Vec::new())),
        ServerMessage::Pong(u64::MAX),
        ServerMessage::Bye(Bye::new(ByeReason::Replaced)),
    ];
    for m in all {
        assert_eq!(round_trip_server(&m), Some(m));
    }
}

#[test]
fn touch_multi_pointer_round_trip_and_float_byte_order() {
    let event = TouchEvent::new(
        u32::MAX,
        TouchAction::PointerDown,
        1,
        vec![TouchPointer::new(0, -0.01, 1.02), TouchPointer::new(1, 0.75, 0.5)],
    );
    let payload = event.encode_payload();
    assert_eq!(payload.len(), 7 + 2 * 9);
    // 0.75 = 0x3F400000 big-endian
    assert_eq!(payload[17..21], [0x3F, 0x40, 0x00, 0x00]);
    assert_eq!(TouchEvent::decode(&payload), Ok(event));
}

#[test]
fn touch_pointer_count_out_of_range_is_rejected() {
    // eventTimeMs 1, action 0, pointer 0, count 0
    assert!(TouchEvent::decode(&[0, 0, 0, 1, 0, 0, 0]).is_err());
}

// MARK: - AUDIO (PROTOCOL §7.2)

fn assert_audio_malformed(payload: &[u8]) {
    match client(MessageType::Audio, payload) {
        Err(ProtocolError::Malformed(MessageType::Audio, _)) | Err(ProtocolError::Truncated(MessageType::Audio)) => {}
        other => panic!("expected a malformed AUDIO error for {payload:?}, got {other:?}"),
    }
}

#[test]
fn audio_samples_are_little_endian_and_seq_big_endian() {
    let packet = AudioPacket { seq: 0x0102_0304, samples: vec![i16::MIN, i16::MAX, 0, 0x1234] };
    let payload = packet.encode_payload();
    assert_eq!(payload, [0x01, 0x02, 0x03, 0x04, 0x00, 0x80, 0xFF, 0x7F, 0x00, 0x00, 0x34, 0x12]);
    assert_eq!(AudioPacket::decode(&payload), Ok(packet));
}

#[test]
fn audio_accepts_one_to_max_samples() {
    for count in [1, 480, 4800] {
        let packet = AudioPacket { seq: 7, samples: vec![-3; count] };
        assert_eq!(round_trip_client(&ClientMessage::Audio(packet.clone())), Some(ClientMessage::Audio(packet)));
    }
}

#[test]
fn odd_byte_count_is_malformed() {
    assert_audio_malformed(&[0, 0, 0, 1, 0x01]);
    assert_audio_malformed(&[0, 0, 0, 1, 0x01, 0x00, 0xFE]);
}

#[test]
fn zero_samples_is_malformed() {
    assert_audio_malformed(&[0, 0, 0, 1]);
}

#[test]
fn more_than_4800_samples_is_malformed() {
    let mut payload = vec![0, 0, 0, 1];
    payload.resize(4 + 4801 * 2, 0);
    assert_audio_malformed(&payload);
}

#[test]
fn shorter_than_the_seq_is_malformed() {
    assert_audio_malformed(&[]);
    assert_audio_malformed(&[0, 0, 1]);
}

// MARK: - MIC_STATE

#[test]
fn mic_state_decoding() {
    let decode = |json: &str| client(MessageType::MicState, json.as_bytes());
    assert_eq!(decode(r#"{"capturing":true}"#), Ok(Some(ClientMessage::MicState(MicState::capturing()))));
    for reason in MicStateReason::ALL {
        let json = format!(r#"{{"capturing":false,"reason":"{}"}}"#, reason.as_str());
        let Ok(Some(ClientMessage::MicState(state))) = decode(&json) else { panic!("expected MIC_STATE") };
        assert!(!state.capturing);
        assert_eq!(state.known_reason(), Some(reason));
    }
    // Unknown reasons and keys are kept / ignored, not errors.
    let Ok(Some(ClientMessage::MicState(future))) = decode(r#"{"capturing":false,"reason":"thermal","x":1}"#) else {
        panic!("expected MIC_STATE");
    };
    assert_eq!(future.reason.as_deref(), Some("thermal"));
    assert_eq!(future.known_reason(), None);
    assert_eq!(MicStateReason::PermissionDenied.as_str(), "permission_denied");
    // Malformed JSON.
    assert!(decode(r#"{"reason":"stopped"}"#).is_err());
    assert!(decode("{").is_err());
}

#[test]
fn mic_state_round_trip() {
    let state = MicState::stopped(MicStateReason::Background);
    assert_eq!(round_trip_client(&ClientMessage::MicState(state.clone())), Some(ClientMessage::MicState(state)));
}

// MARK: - MIC_CONTROL

#[test]
fn mic_control_json_shapes() {
    assert_eq!(
        server_json(&ServerMessage::MicControl(MicControl::START)),
        r#"{"active":true,"channels":1,"sampleRate":48000}"#
    );
    assert_eq!(server_json(&ServerMessage::MicControl(MicControl::STOP)), r#"{"active":false}"#);
    assert_eq!(ServerMessage::MicControl(MicControl::START).encode()[0], 0x88);
}

#[test]
fn mic_control_round_trip_and_lenient_decoding() {
    for m in [MicControl::START, MicControl::STOP] {
        assert_eq!(round_trip_server(&ServerMessage::MicControl(m)), Some(ServerMessage::MicControl(m)));
    }
    let minimal = server(MessageType::MicControl, br#"{"active":true}"#);
    assert_eq!(
        minimal,
        Ok(Some(ServerMessage::MicControl(MicControl { active: true, channels: None, sample_rate: None })))
    );
}

// MARK: - HELLO features

#[test]
fn hello_features() {
    let base = r#"{"protocol":1,"deviceId":"d","screen":{"width":2400,"height":1080}"#;
    let none = decode_hello(&format!("{base}}}"));
    let mic = decode_hello(&format!(r#"{base},"features":["mic","later"]}}"#));
    assert!(none.features.is_empty());
    assert!(!none.supports_mic());
    assert_eq!(mic.features, ["mic", "later"]);
    assert!(mic.supports_mic());
}

#[test]
fn hello_encodes_features_only_when_present() {
    let screen = ScreenInfo::new(2400, 1080, 420, 60.0);
    let plain = Hello::new("d", "n", screen.clone(), Vec::new());
    let mut with_mic = Hello::new("d", "n", screen, Vec::new());
    with_mic.features = vec!["mic".into()];
    let plain_json = hello_payload(&plain);
    let mic_json = hello_payload(&with_mic);
    assert!(!plain_json.contains("features"), "{plain_json}");
    assert!(mic_json.contains(r#""features":["mic"]"#), "{mic_json}");
    assert_eq!(round_trip_client(&ClientMessage::Hello(with_mic.clone())), Some(ClientMessage::Hello(with_mic)));
}

// MARK: - INPUT_MODE (0x18)

#[test]
fn input_mode_message() {
    let decode = |json: &str| client(MessageType::InputMode, json.as_bytes());
    assert_eq!(
        decode(r#"{"mode":"trackpad"}"#),
        Ok(Some(ClientMessage::InputMode(InputModeMessage::new(InputMode::Trackpad))))
    );
    assert_eq!(
        decode(r#"{"mode":"touch","later":1}"#),
        Ok(Some(ClientMessage::InputMode(InputModeMessage::new(InputMode::Touch))))
    );
    let Ok(Some(ClientMessage::InputMode(unknown))) = decode(r#"{"mode":"stylus"}"#) else {
        panic!("expected INPUT_MODE");
    };
    assert_eq!(unknown.input_mode(), None, "unknown modes decode but are ignored");
    for bad in ["{", r#"{"mode":5}"#, "{}", "[]"] {
        match decode(bad) {
            Err(ProtocolError::Malformed(MessageType::InputMode, _)) => {}
            other => panic!("{bad}: expected malformed INPUT_MODE, got {other:?}"),
        }
    }
    let wire = ClientMessage::InputMode(InputModeMessage::new(InputMode::Trackpad)).encode();
    assert_eq!(wire[0], 0x18);
    assert_eq!(std::str::from_utf8(&wire[5..]).unwrap(), r#"{"mode":"trackpad"}"#);
}

// MARK: - SERVER_HELLO addresses and platform

fn addressed_hello() -> ServerHello {
    let mut hello = ServerHello::new("S", "Mac", HostPlatform::Macos);
    hello.port = Some(47800);
    hello.addresses = Some(vec![
        ServerAddress::new("192.168.1.20", AddressKind::Lan),
        ServerAddress::new("100.101.102.103", AddressKind::Tailscale),
        ServerAddress::new("mac.tail1234.ts.net", AddressKind::TailscaleDns),
    ]);
    hello
}

const ADDRESSES_JSON: &str = r#""addresses":[{"host":"192.168.1.20","kind":"lan"},{"host":"100.101.102.103","kind":"tailscale"},{"host":"mac.tail1234.ts.net","kind":"tailscale-dns"}]"#;

#[test]
fn server_hello_carries_port_and_addresses() {
    let hello = addressed_hello();
    assert_eq!(
        server_json(&ServerMessage::ServerHello(hello.clone())),
        format!(
            r#"{{{ADDRESSES_JSON},"platform":"macos","port":47800,"protocol":1,"serverId":"S","serverName":"Mac"}}"#
        )
    );
    assert_eq!(round_trip_server(&ServerMessage::ServerHello(hello.clone())), Some(ServerMessage::ServerHello(hello)));
    // Without them only the always-sent platform is added to the old shape.
    assert_eq!(
        server_json(&ServerMessage::ServerHello(ServerHello::new("S", "Mac", HostPlatform::Macos))),
        r#"{"platform":"macos","protocol":1,"serverId":"S","serverName":"Mac"}"#
    );
}

// MARK: - SERVER_HELLO platform (port of `HostPlatformTests.swift`)

fn decode_server_hello(json: &str) -> ServerHello {
    match server(MessageType::ServerHello, json.as_bytes()) {
        Ok(Some(ServerMessage::ServerHello(hello))) => hello,
        other => panic!("expected SERVER_HELLO, got {other:?}"),
    }
}

#[test]
fn every_host_always_sends_its_platform() {
    for platform in HostPlatform::ALL {
        let hello = server_value(&ServerMessage::ServerHello(ServerHello::new("S", "Studio", platform)));
        assert_eq!(hello["platform"], platform.as_str());
    }
    let mut windows = addressed_hello();
    windows.platform = HostPlatform::Windows.as_str().into();
    // Sorted keys: `platform` sits between `addresses` and `port`, as the Swift encoder writes it.
    assert_eq!(
        server_json(&ServerMessage::ServerHello(windows)),
        format!(
            r#"{{{ADDRESSES_JSON},"platform":"windows","port":47800,"protocol":1,"serverId":"S","serverName":"Mac"}}"#
        )
    );
}

#[test]
fn decodes_every_known_platform() {
    for platform in HostPlatform::ALL {
        let hello = decode_server_hello(&format!(
            r#"{{"protocol":1,"serverId":"S","serverName":"n","platform":"{}"}}"#,
            platform.as_str()
        ));
        assert_eq!(hello.host_platform(), Some(platform));
        assert_eq!(hello, ServerHello::new("S", "n", platform));
        assert_eq!(HostPlatform::parse(platform.as_str()), Some(platform));
    }
}

#[test]
fn absent_means_an_older_mac_host() {
    let hello = decode_server_hello(r#"{"protocol":1,"serverId":"S","serverName":"n"}"#);
    assert_eq!(hello.platform, "macos");
    assert_eq!(hello.host_platform(), Some(HostPlatform::Macos));
}

#[test]
fn unknown_is_kept_but_has_no_known_platform() {
    let hello = decode_server_hello(r#"{"protocol":1,"serverId":"S","serverName":"n","platform":"haiku"}"#);
    assert_eq!(hello.platform, "haiku");
    assert_eq!(hello.host_platform(), None, "shown as a generic computer");
    // Round-trips unchanged.
    let message = ServerMessage::ServerHello(hello);
    assert_eq!(round_trip_server(&message), Some(message));
}

#[test]
fn malformed_platform_is_ignored_not_fatal() {
    let numeric = decode_server_hello(r#"{"protocol":1,"serverId":"S","serverName":"n","platform":42}"#);
    assert_eq!(numeric.host_platform(), Some(HostPlatform::Macos));
    let null = decode_server_hello(r#"{"protocol":1,"serverId":"S","serverName":"n","platform":null}"#);
    assert_eq!(null.host_platform(), Some(HostPlatform::Macos));
    // The required fields stay required.
    assert!(server(MessageType::ServerHello, br#"{"protocol":1,"serverName":"n","platform":"linux"}"#).is_err());
}

#[test]
fn txt_record() {
    use sidecast_core::protocol::txt;
    let record = txt::record("S", HostPlatform::Macos);
    assert_eq!(record, [("v", "1".to_owned()), ("id", "S".to_owned()), ("os", "macos".to_owned())]);
    assert_eq!(txt::record("S", HostPlatform::Linux)[2], ("os", "linux".to_owned()));
    // Reading it back: absent = an older Mac host; unknown = a generic computer.
    assert_eq!(txt::platform(None), Some(HostPlatform::Macos));
    assert_eq!(txt::platform(Some("windows")), Some(HostPlatform::Windows));
    assert_eq!(txt::platform(Some("beos")), None);
}

#[test]
fn current_platform_is_known_on_supported_systems() {
    if cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux")) {
        assert!(HostPlatform::current().is_some());
    }
}

// MARK: - HELLO network / limits (data saver)

fn hello_with(extra: &str) -> Hello {
    decode_hello(&format!(r#"{{"protocol":1,"deviceId":"d","screen":{{"width":2400,"height":1080}}{extra}}}"#))
}

#[test]
fn network_and_limits_decode() {
    let h = hello_with(r#","network":"metered","limits":{"maxBitrateKbps":4000,"maxFps":30}"#);
    assert_eq!(h.client_network(), Some(ClientNetwork::Metered));
    assert_eq!(h.limits, Some(StreamLimits::DATA_SAVER));
    assert_eq!(h.limits.unwrap().max_bitrate(), Some(4_000_000));
    let none = hello_with("");
    assert_eq!(none.network, None);
    assert_eq!(none.limits, None);
    assert_eq!(hello_with(r#","network":"unmetered""#).client_network(), Some(ClientNetwork::Unmetered));
}

#[test]
fn malformed_network_and_limits_are_ignored_not_fatal() {
    assert_eq!(hello_with(r#","limits":"fast""#).limits, None);
    assert_eq!(hello_with(r#","limits":[1,2]"#).limits, None);
    assert_eq!(hello_with(r#","limits":{"maxBitrateKbps":0,"maxFps":-5}"#).limits, None, "non-positive = no limit");
    assert_eq!(hello_with(r#","limits":{}"#).limits, None);
    assert_eq!(
        hello_with(r#","limits":{"maxBitrateKbps":"4000","maxFps":24}"#).limits,
        Some(StreamLimits::new(None, Some(24)))
    );
    assert_eq!(hello_with(r#","limits":{"maxBitrateKbps":3999.7}"#).limits, Some(StreamLimits::new(Some(3999), None)));
    assert_eq!(hello_with(r#","network":42"#).network, None);
    let future = hello_with(r#","network":"satellite""#);
    assert_eq!(future.network.as_deref(), Some("satellite"));
    assert_eq!(future.client_network(), None);
}

#[test]
fn hello_encodes_network_and_limits_only_when_set() {
    let screen = ScreenInfo::new(2400, 1080, 420, 120.0);
    let plain = Hello::new("d", "n", screen.clone(), Vec::new());
    assert!(!hello_payload(&plain).contains("limits"));
    let mut saver = Hello::new("d", "n", screen, Vec::new());
    saver.network = Some("metered".into());
    saver.limits = Some(StreamLimits::DATA_SAVER);
    let json = hello_payload(&saver);
    assert!(json.contains(r#""limits":{"maxBitrateKbps":4000,"maxFps":30}"#), "{json}");
    assert!(json.contains(r#""network":"metered""#), "{json}");
    assert_eq!(round_trip_client(&ClientMessage::Hello(saver.clone())), Some(ClientMessage::Hello(saver)));
}
