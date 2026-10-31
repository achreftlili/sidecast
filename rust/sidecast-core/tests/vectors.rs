//! PROTOCOL.md §8 test vectors, reproduced exactly (port of `ProtocolVectorTests.swift`).

use sidecast_core::protocol::{
    AudioPacket, ClientMessage, CursorPosition, Frame, FrameParser, HostPlatform, KeyEvent, KeyModifiers, MessageType,
    PairProof, ProtocolKey, ServerMessage, TouchAction, TouchEvent, TouchPointer, VideoFrame, json_b64,
};
use sidecast_core::security::{
    PairingCode, PairingOutcome, PairingSession, device_token, from_hex, pairing_proof, sha256, to_hex,
};

fn hex(s: &str) -> Vec<u8> {
    from_hex(&s.replace(' ', "")).expect("valid hex")
}

fn decode_client(wire: &[u8]) -> Option<ClientMessage> {
    let mut parser = FrameParser::new();
    parser.push(wire);
    let frame = parser.next_frame().unwrap().expect("one complete frame");
    assert_eq!(parser.buffered_len(), 0);
    ClientMessage::decode(&frame).unwrap()
}

fn decode_server(wire: &[u8]) -> Option<ServerMessage> {
    let mut parser = FrameParser::new();
    parser.push(wire);
    ServerMessage::decode(&parser.next_frame().unwrap().expect("one complete frame")).unwrap()
}

// §8.1
#[test]
fn pairing_proof_vector() {
    let fingerprint = sha256(b"sidecast-test-cert");
    assert_eq!(to_hex(&fingerprint), "004ac6c710efca069afd7657c7262b316b18e1595f710ea8a5d2289d04b9d196");
    let nonce = hex("000102030405060708090a0b0c0d0e0f");
    assert_eq!(json_b64::encode(&nonce), "AAECAwQFBgcICQoLDA0ODw==");

    let proof = pairing_proof("123456", &fingerprint, &nonce);
    assert_eq!(to_hex(&proof), "e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe");
    assert_eq!(json_b64::encode(&proof), "40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=");

    // The server-side state machine accepts exactly this proof.
    let mut session = PairingSession::new(PairingCode::new("123456").unwrap(), 0.0);
    assert_eq!(session.verify(&nonce, &proof, &fingerprint, 1.0), PairingOutcome::Success);
}

#[test]
fn pair_proof_json_carries_vector_base64() {
    let msg = ClientMessage::PairProof(PairProof {
        nonce: hex("000102030405060708090a0b0c0d0e0f"),
        proof: json_b64::decode("40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=").unwrap(),
    });
    let json = String::from_utf8(msg.frame().payload).unwrap();
    assert!(json.contains(r#""nonce":"AAECAwQFBgcICQoLDA0ODw==""#), "{json}");
    assert!(json.contains(r#""proof":"40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=""#), "{json}");
}

// §8.2
#[test]
fn token_hash_vector() {
    assert_eq!(
        to_hex(&device_token::hash(&[0xAA; 32])),
        "e0e77a507412b120f6ede61f62295b1a7b2ff19d3dcc8f7253e51663470c888e"
    );
}

// §8.3 FRAME_ACK seq = 258
#[test]
fn frame_ack_vector() {
    let wire = hex("11 00 00 00 04 00 00 01 02");
    assert_eq!(ClientMessage::FrameAck(258).encode(), wire);
    assert_eq!(decode_client(&wire), Some(ClientMessage::FrameAck(258)));
}

// §8.3 TOUCH
#[test]
fn touch_vector() {
    let wire = hex("10 00 00 00 10  00 00 03 e8  02  00  01  00  3f 00 00 00  3e 80 00 00");
    let event = TouchEvent::new(1000, TouchAction::Move, 0, vec![TouchPointer::new(0, 0.5, 0.25)]);
    assert_eq!(ClientMessage::Touch(event.clone()).encode(), wire);
    assert_eq!(decode_client(&wire), Some(ClientMessage::Touch(event)));
}

// §8.3 VIDEO_FRAME
#[test]
fn video_frame_vector() {
    let wire = hex("85 00 00 00 12  00 00 00 07  01  00 00 00 00 00 0f 42 40  00 00 00 01 40");
    let frame = VideoFrame::new(7, true, 1_000_000, hex("00 00 00 01 40"));
    assert_eq!(ServerMessage::VideoFrame(frame.clone()).encode(), wire);
    assert_eq!(VideoFrame::encode_frame_parts(7, true, 1_000_000, &frame.data), wire);
    assert_eq!(decode_server(&wire), Some(ServerMessage::VideoFrame(frame)));
}

// §8.3 TEXT "é"
#[test]
fn text_vector() {
    let wire = hex("14 00 00 00 02 c3 a9");
    assert_eq!(ClientMessage::Text("é".into()).encode(), wire);
    assert_eq!(decode_client(&wire), Some(ClientMessage::Text("é".into())));
}

// §8.3 KEY Return (1), no modifiers; then KEY Left (6) with Shift
#[test]
fn key_vectors() {
    let return_wire = hex("15 00 00 00 02 01 00");
    assert_eq!(ClientMessage::Key(KeyEvent::new(ProtocolKey::Return, KeyModifiers::NONE)).encode(), return_wire);
    assert_eq!(decode_client(&return_wire), Some(ClientMessage::Key(KeyEvent::raw(1, 0))));

    let shift_left_wire = hex("15 00 00 00 02 06 01");
    assert_eq!(ClientMessage::Key(KeyEvent::new(ProtocolKey::Left, KeyModifiers::SHIFT)).encode(), shift_left_wire);
    assert_eq!(decode_client(&shift_left_wire), Some(ClientMessage::Key(KeyEvent::raw(6, 1))));

    // Both back to back in one stream, as the vector lists them.
    let mut parser = FrameParser::new();
    parser.push(&[return_wire, shift_left_wire].concat());
    let decoded: Vec<_> = parser.drain().unwrap().iter().map(|f| ClientMessage::decode(f).unwrap()).collect();
    assert_eq!(
        decoded,
        [
            Some(ClientMessage::Key(KeyEvent::new(ProtocolKey::Return, KeyModifiers::NONE))),
            Some(ClientMessage::Key(KeyEvent::new(ProtocolKey::Left, KeyModifiers::SHIFT))),
        ]
    );
}

// §8.3 AUDIO seq = 1, two samples [1, -2]
#[test]
fn audio_vector() {
    let wire = hex("16 00 00 00 08 00 00 00 01 01 00 fe ff");
    let packet = AudioPacket { seq: 1, samples: vec![1, -2] };
    assert_eq!(ClientMessage::Audio(packet.clone()).encode(), wire);
    assert_eq!(decode_client(&wire), Some(ClientMessage::Audio(packet)));
}

// §8.3 CURSOR x = 0.5, y = 0.25
#[test]
fn cursor_vector() {
    let wire = hex("89 00 00 00 08 3f 00 00 00 3e 80 00 00");
    assert_eq!(ServerMessage::Cursor(CursorPosition::new(0.5, 0.25)).encode(), wire);
    assert_eq!(decode_server(&wire), Some(ServerMessage::Cursor(CursorPosition::new(0.5, 0.25))));
}

// §8.3 PING t = 1
#[test]
fn ping_vector() {
    let wire = hex("13 00 00 00 08 00 00 00 00 00 00 00 01");
    assert_eq!(ClientMessage::Ping(1).encode(), wire);
    assert_eq!(decode_client(&wire), Some(ClientMessage::Ping(1)));
    // PONG echoes the same 8 bytes.
    assert_eq!(ServerMessage::Pong(1).encode()[5..], wire[5..]);
}

// §8.4 SERVER_HELLO platform
#[test]
fn server_hello_platform_vectors() {
    let platform = |json: &str| match ServerMessage::decode(&Frame::new(MessageType::ServerHello, json.into())) {
        Ok(Some(ServerMessage::ServerHello(hello))) => hello.host_platform(),
        other => panic!("expected SERVER_HELLO, got {other:?}"),
    };
    let studio = r#"{"protocol":1,"serverId":"S","serverName":"Studio""#;
    assert_eq!(platform(&format!(r#"{studio},"platform":"macos"}}"#)), Some(HostPlatform::Macos));
    assert_eq!(platform(&format!(r#"{studio},"platform":"windows"}}"#)), Some(HostPlatform::Windows));
    assert_eq!(platform(&format!(r#"{studio},"platform":"linux"}}"#)), Some(HostPlatform::Linux));
    assert_eq!(platform(&format!("{studio}}}")), Some(HostPlatform::Macos), "a Mac host older than the field");
    assert_eq!(platform(&format!(r#"{studio},"platform":"beos"}}"#)), None, "unknown: a generic computer");
}
