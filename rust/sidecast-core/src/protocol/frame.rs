use super::{HEADER_LENGTH, MAX_PAYLOAD_LENGTH, MessageType, ProtocolError};

/// One framed message: `type: u8, length: u32 BE, payload` (PROTOCOL §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub msg_type: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(msg_type: MessageType, payload: Vec<u8>) -> Self {
        Frame { msg_type: msg_type as u8, payload }
    }

    pub fn empty(msg_type: MessageType) -> Self {
        Self::new(msg_type, Vec::new())
    }

    /// The known message type, or `None` for a type this version does not define.
    pub fn message_type(&self) -> Option<MessageType> {
        MessageType::from_u8(self.msg_type)
    }

    /// Wire encoding (header + payload).
    pub fn encode(&self) -> Vec<u8> {
        encode_frame(self.msg_type, &self.payload)
    }
}

/// Builds `type, length, payload` in one allocation.
pub(crate) fn encode_frame(msg_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LENGTH + payload.len());
    out.push(msg_type);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Incremental frame parser: feed bytes as they arrive (in any chunking),
/// then pull complete frames. A declared length above 16 MiB is a protocol
/// error; the connection must be closed when [`FrameParser::next_frame`] fails.
#[derive(Debug)]
pub struct FrameParser {
    buffer: Vec<u8>,
    read_offset: usize,
    max_payload: usize,
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameParser {
    pub fn new() -> Self {
        Self::with_max_payload(MAX_PAYLOAD_LENGTH)
    }

    pub fn with_max_payload(max_payload: usize) -> Self {
        FrameParser { buffer: Vec::new(), read_offset: 0, max_payload }
    }

    /// Bytes received but not yet returned as frames.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len() - self.read_offset
    }

    pub fn push(&mut self, data: &[u8]) {
        self.compact_if_needed();
        self.buffer.extend_from_slice(data);
    }

    /// The next complete frame, or `None` when more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.buffered_len() < HEADER_LENGTH {
            return Ok(None);
        }
        let start = self.read_offset;
        let header = &self.buffer[start..start + HEADER_LENGTH];
        let msg_type = header[0];
        let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        if length > self.max_payload {
            return Err(ProtocolError::PayloadTooLarge(length));
        }
        let total = HEADER_LENGTH + length;
        if self.buffered_len() < total {
            return Ok(None);
        }
        let payload = self.buffer[start + HEADER_LENGTH..start + total].to_vec();
        self.read_offset += total;
        Ok(Some(Frame { msg_type, payload }))
    }

    /// Every complete frame currently buffered.
    pub fn drain(&mut self) -> Result<Vec<Frame>, ProtocolError> {
        let mut frames = Vec::new();
        while let Some(frame) = self.next_frame()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    /// Drops consumed bytes once they dominate the buffer (amortised O(1) per byte).
    fn compact_if_needed(&mut self) {
        if self.read_offset == 0 {
            return;
        }
        if self.read_offset == self.buffer.len() {
            self.buffer.clear();
            self.read_offset = 0;
        } else if self.read_offset >= 64 * 1024 && self.read_offset * 2 >= self.buffer.len() {
            self.buffer.drain(..self.read_offset);
            self.read_offset = 0;
        }
    }
}
