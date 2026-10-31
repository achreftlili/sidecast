//! Binary payloads. Integers and floats are big-endian (Java `DataOutputStream`
//! order); AUDIO samples are the one little-endian exception (PROTOCOL §7.2).

use super::frame::encode_frame;
use super::{HEADER_LENGTH, MessageType, ProtocolError};
use crate::geometry::{Point, Size};

/// Reads big-endian values; `None` once the payload runs out.
pub(crate) struct ByteReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        ByteReader { bytes, offset: 0 }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let slice = self.bytes.get(self.offset..self.offset + N)?;
        self.offset += N;
        slice.try_into().ok()
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take::<4>().map(u32::from_be_bytes)
    }

    pub fn u64(&mut self) -> Option<u64> {
        self.take::<8>().map(u64::from_be_bytes)
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
}

/// Android `MotionEvent` action codes carried in TOUCH (PROTOCOL §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TouchAction {
    Down = 0,
    Up = 1,
    Move = 2,
    Cancel = 3,
    PointerDown = 5,
    PointerUp = 6,
}

impl TouchAction {
    pub fn from_u8(raw: u8) -> Option<TouchAction> {
        match raw {
            0 => Some(TouchAction::Down),
            1 => Some(TouchAction::Up),
            2 => Some(TouchAction::Move),
            3 => Some(TouchAction::Cancel),
            5 => Some(TouchAction::PointerDown),
            6 => Some(TouchAction::PointerUp),
            _ => None,
        }
    }
}

/// One finger: normalized to the video frame, (0,0) top-left … (1,1) bottom-right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPointer {
    pub id: u8,
    pub x: f32,
    pub y: f32,
}

impl TouchPointer {
    pub fn new(id: u8, x: f32, y: f32) -> Self {
        TouchPointer { id, x, y }
    }
}

/// TOUCH payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TouchEvent {
    /// Client uptime in ms. Wraps; only differences are meaningful.
    pub event_time_ms: u32,
    /// Raw action byte; see [`TouchEvent::touch_action`].
    pub action: u8,
    pub action_pointer_id: u8,
    pub pointers: Vec<TouchPointer>,
}

impl TouchEvent {
    pub const MAX_POINTERS: usize = 10;

    pub fn new(event_time_ms: u32, action: TouchAction, action_pointer_id: u8, pointers: Vec<TouchPointer>) -> Self {
        TouchEvent { event_time_ms, action: action as u8, action_pointer_id, pointers }
    }

    pub fn touch_action(&self) -> Option<TouchAction> {
        TouchAction::from_u8(self.action)
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.pointers.len() * 9);
        out.extend_from_slice(&self.event_time_ms.to_be_bytes());
        out.push(self.action);
        out.push(self.action_pointer_id);
        out.push(self.pointers.len() as u8);
        for p in &self.pointers {
            out.push(p.id);
            out.extend_from_slice(&p.x.to_bits().to_be_bytes());
            out.extend_from_slice(&p.y.to_bits().to_be_bytes());
        }
        out
    }

    /// Trailing bytes are ignored (forward compatibility).
    pub fn decode(payload: &[u8]) -> Result<TouchEvent, ProtocolError> {
        let truncated = ProtocolError::Truncated(MessageType::Touch);
        let mut r = ByteReader::new(payload);
        let (time, action, pid, count) = match (r.u32(), r.u8(), r.u8(), r.u8()) {
            (Some(t), Some(a), Some(p), Some(c)) => (t, a, p, c),
            _ => return Err(truncated),
        };
        if count == 0 || count as usize > Self::MAX_POINTERS {
            return Err(ProtocolError::Malformed(
                MessageType::Touch,
                format!("pointer count {count} outside 1…{}", Self::MAX_POINTERS),
            ));
        }
        let mut pointers = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match (r.u8(), r.f32(), r.f32()) {
                (Some(id), Some(x), Some(y)) => pointers.push(TouchPointer { id, x, y }),
                _ => return Err(truncated),
            }
        }
        Ok(TouchEvent { event_time_ms: time, action, action_pointer_id: pid, pointers })
    }
}

/// VIDEO_FRAME payload: `seq u32, flags u8, ptsMicros u64, Annex-B data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub seq: u32,
    pub flags: u8,
    pub pts_micros: u64,
    pub data: Vec<u8>,
}

impl VideoFrame {
    pub const HEADER_LENGTH: usize = 13;
    pub const KEYFRAME_FLAG: u8 = 0x01;

    pub fn new(seq: u32, is_keyframe: bool, pts_micros: u64, data: Vec<u8>) -> Self {
        let flags = if is_keyframe { Self::KEYFRAME_FLAG } else { 0 };
        VideoFrame { seq, flags, pts_micros, data }
    }

    pub fn is_keyframe(&self) -> bool {
        self.flags & Self::KEYFRAME_FLAG != 0
    }

    /// The complete wire frame (header + payload) in one allocation, without
    /// copying `data` twice.
    pub fn encode_frame_parts(seq: u32, is_keyframe: bool, pts_micros: u64, data: &[u8]) -> Vec<u8> {
        let payload_length = Self::HEADER_LENGTH + data.len();
        let mut out = Vec::with_capacity(HEADER_LENGTH + payload_length);
        out.push(MessageType::VideoFrame as u8);
        out.extend_from_slice(&(payload_length as u32).to_be_bytes());
        out.extend_from_slice(&seq.to_be_bytes());
        out.push(if is_keyframe { Self::KEYFRAME_FLAG } else { 0 });
        out.extend_from_slice(&pts_micros.to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    pub fn encode_frame(&self) -> Vec<u8> {
        let mut out = Self::encode_frame_parts(self.seq, false, self.pts_micros, &self.data);
        out[HEADER_LENGTH + 4] = self.flags;
        out
    }

    pub fn decode(payload: &[u8]) -> Result<VideoFrame, ProtocolError> {
        if payload.len() < Self::HEADER_LENGTH {
            return Err(ProtocolError::Truncated(MessageType::VideoFrame));
        }
        let mut r = ByteReader::new(payload);
        let (seq, flags, pts) = (r.u32().unwrap_or(0), r.u8().unwrap_or(0), r.u64().unwrap_or(0));
        Ok(VideoFrame { seq, flags, pts_micros: pts, data: payload[Self::HEADER_LENGTH..].to_vec() })
    }
}

/// AUDIO payload (PROTOCOL §7.2): `seq u32 BE`, then PCM signed 16-bit
/// little-endian, mono, 48 kHz, 1…4800 samples (480 = 10 ms recommended).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub seq: u32,
    pub samples: Vec<i16>,
}

impl AudioPacket {
    pub const SAMPLE_RATE: u32 = 48_000;
    pub const CHANNELS: u32 = 1;
    pub const MAX_SAMPLES: usize = 4_800;
    pub const RECOMMENDED_SAMPLES: usize = 480;

    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.samples.len() * 2);
        out.extend_from_slice(&self.seq.to_be_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// Odd sample bytes, 0 samples or more than 4800 are malformed.
    pub fn decode(payload: &[u8]) -> Result<AudioPacket, ProtocolError> {
        if payload.len() < 4 {
            return Err(ProtocolError::Truncated(MessageType::Audio));
        }
        let sample_bytes = payload.len() - 4;
        if !sample_bytes.is_multiple_of(2) {
            return Err(ProtocolError::Malformed(MessageType::Audio, format!("odd sample byte count {sample_bytes}")));
        }
        let count = sample_bytes / 2;
        if !(1..=Self::MAX_SAMPLES).contains(&count) {
            return Err(ProtocolError::Malformed(
                MessageType::Audio,
                format!("{count} samples outside 1…{}", Self::MAX_SAMPLES),
            ));
        }
        let seq = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let samples = payload[4..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        Ok(AudioPacket { seq, samples })
    }
}

/// CURSOR payload (PROTOCOL §7.3): the host's pointer, normalized to the
/// streamed display; (-1, -1) = not on this display. 8 bytes, f32 BE.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPosition {
    pub x: f32,
    pub y: f32,
}

impl CursorPosition {
    /// The pointer is on another screen.
    pub const OFF_DISPLAY: CursorPosition = CursorPosition { x: -1.0, y: -1.0 };

    pub fn new(x: f32, y: f32) -> Self {
        CursorPosition { x, y }
    }

    /// The client shows its pointer only for 0…1 on both axes.
    pub fn is_on_display(&self) -> bool {
        (0.0..=1.0).contains(&self.x) && (0.0..=1.0).contains(&self.y)
    }

    /// A display-local point normalized to `size`, clamped to 0…1.
    pub fn from_point(point: Point, size: Size) -> Self {
        if !(size.width > 0.0 && size.height > 0.0 && point.x.is_finite() && point.y.is_finite()) {
            return Self::OFF_DISPLAY;
        }
        CursorPosition {
            x: (point.x / size.width).clamp(0.0, 1.0) as f32,
            y: (point.y / size.height).clamp(0.0, 1.0) as f32,
        }
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8);
        out.extend_from_slice(&self.x.to_bits().to_be_bytes());
        out.extend_from_slice(&self.y.to_bits().to_be_bytes());
        out
    }

    /// Trailing bytes are ignored (forward compatibility).
    pub fn decode(payload: &[u8]) -> Result<CursorPosition, ProtocolError> {
        let mut r = ByteReader::new(payload);
        match (r.f32(), r.f32()) {
            (Some(x), Some(y)) => Ok(CursorPosition { x, y }),
            _ => Err(ProtocolError::Truncated(MessageType::Cursor)),
        }
    }
}

/// `seq` (FRAME_ACK) or `t` (PING/PONG) payloads.
pub(crate) fn encode_u32(msg_type: MessageType, value: u32) -> Vec<u8> {
    encode_frame(msg_type as u8, &value.to_be_bytes())
}

pub(crate) fn encode_u64(msg_type: MessageType, value: u64) -> Vec<u8> {
    encode_frame(msg_type as u8, &value.to_be_bytes())
}
