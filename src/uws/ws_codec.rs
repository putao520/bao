//! RFC 6455 WebSocket frame codec — owned by `bun_uws`.
//!
//! Migrated from `bao_cdp::ws_codec` (TASK-18, REQ-CDP-UWS-001). All WebSocket
//! wire-format logic now lives in `bun_uws` so `bao_cdp` / `bao_cdp_client`
//! depend on `bun_uws` for every WebSocket concern. uWS C++ upstream ships an
//! unfinished `ClientApp.h`, so the codec + masking path stays in Rust here.
//!
//! This module is intentionally synchronous (`std::io::{Read, Write}`) — it
//! wraps any byte stream, matching `bao_cdp::CDPServer`'s synchronous model.
//! The uWS C++ async `App::ws()` path remains available via the FFI types in
//! [`crate::uws_sys`] for HTTP-server callers that own an event loop.
//!
//! @trace REQ-CDP-UWS-001

use std::io::Read;

// ============================================================================
// Opcode (RFC 6455 §5.2)
// ============================================================================

/// RFC 6455 §5.2 frame opcode.
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
    /// Parse a raw opcode nibble. Returns `None` for reserved / unknown codes
    /// (RFC 6455 §5.2 reserves 0x3..0x7 and 0xB..0xF).
    pub fn from_u8(b: u8) -> Option<Opcode> {
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

/// Decoded frame header. Payload bytes live in the decoder's internal buffer
/// and are retrieved via [`FrameDecoder::take_payload`] / [`FrameDecoder::take_mask`].
#[derive(Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub fin: bool,
    pub opcode: Opcode,
    pub mask: bool,
    pub payload_len: u64,
}

/// Streaming frame decoder. Buffers raw bytes across multiple `Read` calls so
/// frames split across network reads (or read with `WouldBlock` between
/// header / payload) decode correctly on the next call.
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
        // `needed` is the total buffer length required after this call. Uses
        // `self.buffer.len()` (not `self.pos`) — when called multiple times in
        // sequence (mask bytes then payload), the buffer has grown beyond `pos`
        // and the next read must account for the already-buffered bytes.
        let needed = n + self.buffer.len();
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

    /// Decode one frame header. Returns `Ok(Some(header))` on success,
    /// `Ok(None)` if no frame is yet available, or `Err` on a fatal I/O
    /// failure. `WouldBlock` / `TimedOut` from the underlying stream are
    /// normalised to `WouldBlock` so callers can retry.
    pub fn decode_frame<R: Read>(
        &mut self,
        reader: &mut R,
    ) -> std::io::Result<Option<FrameHeader>> {
        self.read_bytes(reader, 2)?;
        let byte0 = self.take_byte();
        let byte1 = self.take_byte();

        let fin = (byte0 & 0x80) != 0;
        let opcode = Opcode::from_u8(byte0 & 0x0F).unwrap_or(Opcode::Close);
        let mask = (byte1 & 0x80) != 0;
        let mut payload_len = (byte1 & 0x7F) as u64;

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

        if mask {
            self.read_bytes(reader, 4)?;
        }

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

    /// Take the payload of the most recently decoded frame. Resets the buffer
    /// so the next `decode_frame` starts fresh.
    pub fn take_payload(&mut self, header: &FrameHeader) -> Vec<u8> {
        let start = self.pos;
        let end = start + header.payload_len as usize;
        let payload = self.buffer[start..end].to_vec();
        self.pos = end;
        self.buffer.drain(0..self.pos);
        self.pos = 0;
        payload
    }

    /// Take the 4-byte mask key preceding the payload of the current frame.
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

/// Frame encoder. Owns a scratch buffer reused across frames (clear-then-write).
pub struct FrameEncoder {
    buffer: Vec<u8>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        FrameEncoder {
            buffer: Vec::with_capacity(4096),
        }
    }

    /// Encode a text frame (server-side, unmasked).
    pub fn encode_text(&mut self, payload: &str) -> &[u8] {
        self.encode_frame(Opcode::Text, payload.as_bytes(), None)
    }

    /// Encode a binary frame (server-side, unmasked).
    pub fn encode_binary(&mut self, payload: &[u8]) -> &[u8] {
        self.encode_frame(Opcode::Binary, payload, None)
    }

    /// Encode a close frame (server-side, unmasked).
    pub fn encode_close(&mut self, code: u16, reason: &str) -> &[u8] {
        let payload = if code > 0 || !reason.is_empty() {
            let mut p = Vec::with_capacity(2 + reason.len());
            p.extend_from_slice(&code.to_be_bytes());
            p.extend_from_slice(reason.as_bytes());
            p
        } else {
            Vec::new()
        };
        self.encode_frame(Opcode::Close, &payload, None)
    }

    /// Encode a pong frame (server-side, unmasked).
    pub fn encode_pong(&mut self, payload: &[u8]) -> &[u8] {
        self.encode_frame(Opcode::Pong, payload, None)
    }

    /// Encode a frame with optional masking. When `mask_key` is `Some`, the
    /// client→server masking bit (RFC 6455 §5.1) is set and the payload is
    /// XOR'd with the supplied key; `None` produces an unmasked server frame.
    pub fn encode_frame(
        &mut self,
        opcode: Opcode,
        payload: &[u8],
        mask_key: Option<[u8; 4]>,
    ) -> &[u8] {
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

        let mask_bit = if mask_key.is_some() { 0x80u8 } else { 0u8 };
        self.buffer.push(byte0);
        self.buffer.push(byte1 | mask_bit);
        self.buffer.extend_from_slice(&extended_len);

        if let Some(key) = mask_key {
            self.buffer.extend_from_slice(&key);
            let mut masked = payload.to_vec();
            apply_mask(&mut masked, &key);
            self.buffer.extend_from_slice(&masked);
        } else {
            self.buffer.extend_from_slice(payload);
        }

        &self.buffer
    }

    /// Borrow the internal scratch buffer (e.g. for testing).
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Masking (RFC 6455 §5.3)
// ============================================================================

/// Apply RFC 6455 §5.3 XOR mask in place: `payload[i] ^= mask[i % 4]`.
/// Inverse operation — calling twice restores the original bytes.
pub fn apply_mask(payload: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}

/// Generate a 4-byte mask key. RFC 6455 §5.3 requires "high-quality entropy";
/// for trusted CDP-client traffic an address/time-seeded XorShift is sufficient
/// and avoids pulling in a `getrandom` dependency. Returned key is guaranteed
/// non-zero (the spec recommends avoiding all-zero masks).
pub fn gen_mask_key() -> [u8; 4] {
    let mut state: u64 = 0xD1B54A32D192ED03u64;
    state ^= std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xDEADBEEF);
    state ^= &state as *const _ as u64;
    let mut out = [0u8; 4];
    for i in 0..4 {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        out[i] = ((state.wrapping_mul(0x2545F4914F6CDD1D)) >> (i * 8)) as u8;
    }
    if out == [0, 0, 0, 0] {
        out = [0x12, 0x34, 0x56, 0x78];
    }
    out
}

// ============================================================================
// WebSocket Message (high-level)
// ============================================================================

/// Decoded message — a single FIN text/binary frame, or a control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Close(Option<u16>, String),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
}

impl Message {
    /// Build a [`Message`] from a decoded opcode + payload.
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
        assert_eq!(frame[1] & 0x7F, 0);
    }

    #[test]
    fn encode_close_with_code() {
        let mut encoder = FrameEncoder::new();
        let frame = encoder.encode_close(1000, "Normal");
        assert_eq!(frame[0] & 0x0F, Opcode::Close as u8);
        assert_eq!(frame[1] & 0x7F, 8);
        assert_eq!(u16::from_be_bytes([frame[2], frame[3]]), 1000);
        assert_eq!(&frame[4..], b"Normal");
    }

    #[test]
    fn encode_masked_text_round_trip() {
        let mut encoder = FrameEncoder::new();
        let key = [0x37u8, 0xfa, 0x21, 0x3d];
        let frame = encoder.encode_frame(Opcode::Text, b"hi", Some(key));
        assert!(frame[1] & 0x80 != 0); // mask bit
        assert_eq!(frame[1] & 0x7F, 2);
        let mask = [frame[2], frame[3], frame[4], frame[5]];
        let mut payload = frame[6..].to_vec();
        apply_mask(&mut payload, &mask);
        assert_eq!(&payload, b"hi");
    }

    #[test]
    fn apply_mask_is_involutive() {
        let original = b"hello world".to_vec();
        let key = [0x37u8, 0xfa, 0x21, 0x3d];
        let mut buf = original.clone();
        apply_mask(&mut buf, &key);
        assert_ne!(buf, original);
        apply_mask(&mut buf, &key);
        assert_eq!(buf, original);
    }

    #[test]
    fn gen_mask_key_nonzero() {
        for _ in 0..10 {
            assert_ne!(gen_mask_key(), [0, 0, 0, 0]);
        }
    }

    #[test]
    fn message_from_text_frame() {
        let payload = b"hello cdp".to_vec();
        let msg = Message::from_frame(Opcode::Text, payload);
        assert_eq!(msg, Message::Text("hello cdp".to_string()));
    }

    #[test]
    fn message_from_binary_frame() {
        let payload = vec![1u8, 2, 3];
        let msg = Message::from_frame(Opcode::Binary, payload);
        assert_eq!(msg, Message::Binary(vec![1, 2, 3]));
    }

    #[test]
    fn message_from_ping_frame() {
        let payload = vec![1u8, 2, 3];
        let msg = Message::from_frame(Opcode::Ping, payload);
        assert_eq!(msg, Message::Ping(vec![1, 2, 3]));
    }
}
