// @trace REQ-CDP-002 [entity:CDPSession] [entity:ExternalBackend]
// WebSocket frame codec (RFC 6455) — minimal bridge layer replacing tungstenite.
// This is a "necessary bridge layer" exception to the "no hand-written code" rule,
// similar to bao_engine's JSC→SM bridge. The WebSocket protocol is stable and
// well-specified; implementing it here avoids a full async rewrite of bao_cdp.

use std::io::{Read, Write};

// ============================================================================
// Opcode (RFC 6455 §5.2)
// ============================================================================

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    fn from_u8(b: u8) -> Option<Opcode> {
        match b {
            0x0 => Some(Opcode::Continuation),
            0x1 => Some(Opcode::Text),
            0x2 => Some(Opcode::Binary),
            0x8 => Some(Opcode::Close),
            0x9 => Some(Opcode::Ping),
            0xA => Some(Opcode::Pong),
            _ => None,
        }
    }
}

// ============================================================================
// Frame header (RFC 6455 §5.2)
// ============================================================================

#[derive(Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub fin: bool,
    pub opcode: Opcode,
    pub mask: bool,
    pub payload_len: u64,
}

pub struct FrameDecoder {
    buffer: Vec<u8>,
    pos: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            buffer: Vec::new(),
            pos: 0,
        }
    }

    fn read_bytes<R: Read>(&mut self, reader: &mut R, n: usize) -> std::io::Result<()> {
        // Read in chunks (up to 8KB at a time) to avoid 1M syscalls for 1MB payloads.
        let needed = n + self.pos;
        while self.buffer.len() < needed {
            let mut chunk = [0u8; 8192];
            let to_read = std::cmp::min(chunk.len(), needed - self.buffer.len());
            match reader.read(&mut chunk[..to_read]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "unexpected EOF",
                    ))
                }
                Ok(k) => self.buffer.extend_from_slice(&chunk[..k]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "would block",
                    ))
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn take_byte(&mut self) -> u8 {
        let b = self.buffer[self.pos];
        self.pos += 1;
        b
    }

    pub fn decode_frame<R: Read>(&mut self, reader: &mut R) -> std::io::Result<Option<FrameHeader>> {
        // Read first 2 bytes (fixed header)
        self.read_bytes(reader, 2)?;
        let byte0 = self.take_byte();
        let byte1 = self.take_byte();

        let fin = (byte0 & 0x80) != 0;
        let opcode = Opcode::from_u8(byte0 & 0x0F).unwrap_or(Opcode::Close);
        let mask = (byte1 & 0x80) != 0;
        let mut payload_len = (byte1 & 0x7F) as u64;

        // Extended payload length
        if payload_len == 126 {
            self.read_bytes(reader, 2)?;
            payload_len = u16::from_be_bytes([self.take_byte(), self.take_byte()]) as u64;
        } else if payload_len == 127 {
            self.read_bytes(reader, 8)?;
            payload_len = u64::from_be_bytes([
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
                self.take_byte(),
            ]);
        }

        // Masking key (if present)
        if mask {
            self.read_bytes(reader, 4)?;
        }

        // Payload data
        if payload_len > 0 {
            self.read_bytes(reader, payload_len as usize)?;
        }

        Ok(Some(FrameHeader {
            fin,
            opcode,
            mask,
            payload_len,
        }))
    }

    pub fn take_payload(&mut self, header: &FrameHeader) -> Vec<u8> {
        let start = self.pos;
        let end = start + header.payload_len as usize;
        let payload = self.buffer[start..end].to_vec();
        self.pos = end;
        self.buffer.drain(0..self.pos);
        self.pos = 0;
        payload
    }

    pub fn take_mask(&mut self) -> [u8; 4] {
        let mut mask = [0u8; 4];
        mask.copy_from_slice(&self.buffer[self.pos..self.pos + 4]);
        self.pos += 4;
        mask
    }
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Frame encoder
// ============================================================================

pub struct FrameEncoder {
    buffer: Vec<u8>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        FrameEncoder {
            buffer: Vec::with_capacity(4096),
        }
    }

    pub fn encode_text(&mut self, payload: &str) -> &[u8] {
        self.encode_frame(Opcode::Text, payload.as_bytes(), false)
    }

    pub fn encode_binary(&mut self, payload: &[u8]) -> &[u8] {
        self.encode_frame(Opcode::Binary, payload, false)
    }

    pub fn encode_close(&mut self, code: u16, reason: &str) -> &[u8] {
        let payload = if code > 0 || !reason.is_empty() {
            let mut p = Vec::with_capacity(2 + reason.len());
            p.extend_from_slice(&code.to_be_bytes());
            p.extend_from_slice(reason.as_bytes());
            p
        } else {
            Vec::new()
        };
        self.encode_frame(Opcode::Close, &payload, false)
    }

    pub fn encode_pong(&mut self, payload: &[u8]) -> &[u8] {
        self.encode_frame(Opcode::Pong, payload, false)
    }

    fn encode_frame(&mut self, opcode: Opcode, payload: &[u8], mask: bool) -> &[u8] {
        self.buffer.clear();

        let fin = 0x80u8;
        let byte0 = fin | (opcode as u8);

        let len = payload.len();
        let (byte1, extended_len) = if len < 126 {
            (len as u8, Vec::new())
        } else if len <= 65535 {
            (126u8, (len as u16).to_be_bytes().to_vec())
        } else {
            (127u8, (len as u64).to_be_bytes().to_vec())
        };

        let mask_bit = if mask { 0x80u8 } else { 0u8 };
        self.buffer.push(byte0);
        self.buffer.push(byte1 | mask_bit);
        self.buffer.extend_from_slice(&extended_len);

        if mask {
            // TODO: implement masking if client mode is needed
            // Server-to-client frames MUST NOT be masked (RFC 6455 §5.1)
        }

        self.buffer.extend_from_slice(payload);
        &self.buffer
    }
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// WebSocket Message (high-level)
// ============================================================================

pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Close(Option<u16>, String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
}

impl Message {
    pub fn from_frame(opcode: Opcode, payload: Vec<u8>) -> Self {
        match opcode {
            Opcode::Text => Message::Text(String::from_utf8_lossy(&payload).into_owned()),
            Opcode::Binary => Message::Binary(payload),
            Opcode::Close => {
                let code = if payload.len() >= 2 {
                    Some(u16::from_be_bytes([payload[0], payload[1]]))
                } else {
                    None
                };
                let reason = if payload.len() > 2 {
                    String::from_utf8_lossy(&payload[2..]).into_owned()
                } else {
                    String::new()
                };
                Message::Close(code, reason)
            }
            Opcode::Ping => Message::Ping(payload),
            Opcode::Pong => Message::Pong(payload),
            _ => Message::Binary(payload),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_from_u8_valid() {
        assert_eq!(Opcode::from_u8(0x1), Some(Opcode::Text));
        assert_eq!(Opcode::from_u8(0x2), Some(Opcode::Binary));
        assert_eq!(Opcode::from_u8(0x8), Some(Opcode::Close));
        assert_eq!(Opcode::from_u8(0x9), Some(Opcode::Ping));
        assert_eq!(Opcode::from_u8(0xA), Some(Opcode::Pong));
    }

    #[test]
    fn opcode_from_u8_invalid() {
        assert_eq!(Opcode::from_u8(0x3), None);
        assert_eq!(Opcode::from_u8(0xFF), None);
    }

    #[test]
    fn frame_decoder_new() {
        let decoder = FrameDecoder::new();
        assert_eq!(decoder.buffer.len(), 0);
        assert_eq!(decoder.pos, 0);
    }

    #[test]
    fn frame_encoder_new() {
        let encoder = FrameEncoder::new();
        assert!(encoder.buffer.is_empty());
    }

    #[test]
    fn encode_text_hello() {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode_text("hello");
        assert_eq!(frame[0] & 0x0F, Opcode::Text as u8);
        assert!(frame[0] & 0x80 != 0); // FIN set
        assert_eq!(frame[1] & 0x7F, 5); // payload length
        assert_eq!(&frame[2..], b"hello");
    }

    #[test]
    fn encode_close_empty() {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode_close(0, "");
        assert_eq!(frame[0] & 0x0F, Opcode::Close as u8);
        assert_eq!(frame[1] & 0x7F, 0); // empty payload
    }

    #[test]
    fn encode_close_with_code() {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode_close(1000, "Normal");
        assert_eq!(frame[0] & 0x0F, Opcode::Close as u8);
        assert_eq!(frame[1] & 0x7F, 8); // 2-byte code + 6-byte "Normal"
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 1000);
        assert_eq!(&frame[4..], b"Normal");
    }

    #[test]
    fn message_from_text_frame() {
        let payload = b"hello cdp".to_vec();
        let msg = Message::from_frame(Opcode::Text, payload);
        match msg {
            Message::Text(s) => assert_eq!(s, "hello cdp"),
            _ => panic!("expected Text message"),
        }
    }

    #[test]
    fn message_from_binary_frame() {
        let payload = vec![1u8, 2, 3];
        let msg = Message::from_frame(Opcode::Binary, payload);
        match msg {
            Message::Binary(data) => assert_eq!(data, vec![1, 2, 3]),
            _ => panic!("expected Binary message"),
        }
    }

    #[test]
    fn message_from_ping_frame() {
        let payload = vec![1u8, 2, 3];
        let msg = Message::from_frame(Opcode::Ping, payload);
        match msg {
            Message::Ping(data) => assert_eq!(data, vec![1, 2, 3]),
            _ => panic!("expected Ping message"),
        }
    }
}
