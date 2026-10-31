use super::binary::{ByteReader, encode_u32, encode_u64};
use super::frame::encode_frame;
use super::json::{decode_json, encode_json};
use super::{
    AudioPacket, AuthRequest, AuthResult, Bye, CursorPosition, Frame, Hello, InputModeMessage, KeyEvent, MessageType,
    MicControl, MicState, PairChallenge, PairProof, ProtocolError, ServerHello, StreamConfig, TouchEvent, VideoFrame,
    text,
};

/// Messages sent by the phone (client) to the host (server).
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Hello(Hello),
    Auth(AuthRequest),
    PairStart,
    PairProof(PairProof),
    Touch(TouchEvent),
    FrameAck(u32),
    KeyframeRequest,
    Ping(u64),
    /// Text typed on the phone (PROTOCOL §7.1).
    Text(String),
    Key(KeyEvent),
    /// Microphone audio (PROTOCOL §7.2).
    Audio(AudioPacket),
    MicState(MicState),
    /// Touch / trackpad (PROTOCOL §7.3).
    InputMode(InputModeMessage),
}

impl ClientMessage {
    pub fn message_type(&self) -> MessageType {
        match self {
            ClientMessage::Hello(_) => MessageType::Hello,
            ClientMessage::Auth(_) => MessageType::Auth,
            ClientMessage::PairStart => MessageType::PairStart,
            ClientMessage::PairProof(_) => MessageType::PairProof,
            ClientMessage::Touch(_) => MessageType::Touch,
            ClientMessage::FrameAck(_) => MessageType::FrameAck,
            ClientMessage::KeyframeRequest => MessageType::KeyframeRequest,
            ClientMessage::Ping(_) => MessageType::Ping,
            ClientMessage::Text(_) => MessageType::Text,
            ClientMessage::Key(_) => MessageType::Key,
            ClientMessage::Audio(_) => MessageType::Audio,
            ClientMessage::MicState(_) => MessageType::MicState,
            ClientMessage::InputMode(_) => MessageType::InputMode,
        }
    }

    pub fn frame(&self) -> Frame {
        let payload = match self {
            ClientMessage::Hello(m) => encode_json(m),
            ClientMessage::Auth(m) => encode_json(m),
            ClientMessage::PairStart | ClientMessage::KeyframeRequest => Vec::new(),
            ClientMessage::PairProof(m) => encode_json(m),
            ClientMessage::Touch(t) => t.encode_payload(),
            ClientMessage::FrameAck(seq) => seq.to_be_bytes().to_vec(),
            ClientMessage::Ping(t) => t.to_be_bytes().to_vec(),
            // No validation: tests build bad frames on purpose.
            ClientMessage::Text(s) => s.as_bytes().to_vec(),
            ClientMessage::Key(k) => k.encode_payload(),
            ClientMessage::Audio(a) => a.encode_payload(),
            ClientMessage::MicState(m) => encode_json(m),
            ClientMessage::InputMode(m) => encode_json(m),
        };
        Frame::new(self.message_type(), payload)
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            ClientMessage::FrameAck(seq) => encode_u32(MessageType::FrameAck, *seq),
            ClientMessage::Ping(t) => encode_u64(MessageType::Ping, *t),
            _ => self.frame().encode(),
        }
    }

    /// Decodes a frame sent by a client. `Ok(None)` for types that are not
    /// client messages of this protocol version: they must be skipped.
    pub fn decode(frame: &Frame) -> Result<Option<ClientMessage>, ProtocolError> {
        let Some(t) = frame.message_type() else { return Ok(None) };
        let p = frame.payload.as_slice();
        let message = match t {
            MessageType::Hello => ClientMessage::Hello(Hello::decode(p)?),
            MessageType::Auth => ClientMessage::Auth(decode_json(p, t)?),
            MessageType::PairStart => ClientMessage::PairStart,
            MessageType::PairProof => ClientMessage::PairProof(decode_json(p, t)?),
            MessageType::Touch => ClientMessage::Touch(TouchEvent::decode(p)?),
            MessageType::FrameAck => {
                ClientMessage::FrameAck(ByteReader::new(p).u32().ok_or(ProtocolError::Truncated(t))?)
            }
            MessageType::KeyframeRequest => ClientMessage::KeyframeRequest,
            MessageType::Ping => ClientMessage::Ping(ByteReader::new(p).u64().ok_or(ProtocolError::Truncated(t))?),
            MessageType::Text => ClientMessage::Text(text::decode(p)?),
            MessageType::Key => ClientMessage::Key(KeyEvent::decode(p)?),
            MessageType::Audio => ClientMessage::Audio(AudioPacket::decode(p)?),
            MessageType::MicState => ClientMessage::MicState(decode_json(p, t)?),
            MessageType::InputMode => ClientMessage::InputMode(decode_json(p, t)?),
            _ => return Ok(None),
        };
        Ok(Some(message))
    }
}

/// Messages sent by the host (server) to the phone (client).
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    ServerHello(ServerHello),
    PairChallenge(PairChallenge),
    AuthResult(AuthResult),
    StreamConfig(StreamConfig),
    VideoFrame(VideoFrame),
    /// The PING's 8 bytes, echoed unchanged.
    Pong(u64),
    Bye(Bye),
    MicControl(MicControl),
    /// Trackpad-mode pointer position (PROTOCOL §7.3).
    Cursor(CursorPosition),
}

impl ServerMessage {
    pub fn message_type(&self) -> MessageType {
        match self {
            ServerMessage::ServerHello(_) => MessageType::ServerHello,
            ServerMessage::PairChallenge(_) => MessageType::PairChallenge,
            ServerMessage::AuthResult(_) => MessageType::AuthResult,
            ServerMessage::StreamConfig(_) => MessageType::StreamConfig,
            ServerMessage::VideoFrame(_) => MessageType::VideoFrame,
            ServerMessage::Pong(_) => MessageType::Pong,
            ServerMessage::Bye(_) => MessageType::Bye,
            ServerMessage::MicControl(_) => MessageType::MicControl,
            ServerMessage::Cursor(_) => MessageType::Cursor,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let t = self.message_type() as u8;
        match self {
            ServerMessage::ServerHello(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::PairChallenge(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::AuthResult(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::StreamConfig(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::VideoFrame(f) => f.encode_frame(),
            ServerMessage::Pong(v) => encode_u64(MessageType::Pong, *v),
            ServerMessage::Bye(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::MicControl(m) => encode_frame(t, &encode_json(m)),
            ServerMessage::Cursor(c) => encode_frame(t, &c.encode_payload()),
        }
    }

    /// Decodes a frame sent by a server. `Ok(None)` for unknown types.
    pub fn decode(frame: &Frame) -> Result<Option<ServerMessage>, ProtocolError> {
        let Some(t) = frame.message_type() else { return Ok(None) };
        let p = frame.payload.as_slice();
        let message = match t {
            MessageType::ServerHello => ServerMessage::ServerHello(decode_json(p, t)?),
            MessageType::PairChallenge => ServerMessage::PairChallenge(decode_json(p, t)?),
            MessageType::AuthResult => ServerMessage::AuthResult(decode_json(p, t)?),
            MessageType::StreamConfig => ServerMessage::StreamConfig(decode_json(p, t)?),
            MessageType::VideoFrame => ServerMessage::VideoFrame(VideoFrame::decode(p)?),
            MessageType::Pong => ServerMessage::Pong(ByteReader::new(p).u64().ok_or(ProtocolError::Truncated(t))?),
            MessageType::Bye => ServerMessage::Bye(decode_json(p, t)?),
            MessageType::MicControl => ServerMessage::MicControl(decode_json(p, t)?),
            MessageType::Cursor => ServerMessage::Cursor(CursorPosition::decode(p)?),
            _ => return Ok(None),
        };
        Ok(Some(message))
    }
}
