// REQ-CDP-002: CDP WebSocket server — ws_codec-based  @trace REQ-CDP-001
use std::io::{Read, Write};

use crate::ws_codec::{FrameDecoder, FrameEncoder, Message};

/// Read one message from the WebSocket using our frame codec.
///
/// Returns:
/// - `Ok(Some(msg))` — a text/binary frame was decoded
/// - `Ok(None)` — would block / timed out / non-fatal control frame; caller should retry later
/// - `Err(())` — fatal error (connection closed, IO failure, protocol violation)
pub fn read_message<S: Read + Write>(
    decoder: &mut FrameDecoder,
    stream: &mut S,
) -> Result<Option<String>, ()> {
    let header = match decoder.decode_frame(stream) {
        Ok(Some(h)) => h,
        Ok(None) => return Ok(None),
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Stream stalled mid-frame (e.g. large payload across multiple wakeups).
            // buffer/pos are preserved by FrameDecoder, so the next read_message call
            // continues reading where we left off. Returning Ok(None) keeps the
            // session alive — process() will retry on the next event loop tick.
            return Ok(None);
        }
        Err(_) => return Err(()),
    };

    let payload = decoder.take_payload(&header);
    let msg = Message::from_frame(header.opcode, payload);

    match msg {
        Message::Text(text) => Ok(Some(text)),
        Message::Binary(data) => Ok(Some(String::from_utf8_lossy(&data).into_owned())),
        Message::Ping(_) | Message::Pong(_) => Ok(None),
        Message::Close(_, _) => Err(()),
        _ => Ok(None),
    }
}

pub fn write_message<S: Read + Write>(
    encoder: &mut FrameEncoder,
    stream: &mut S,
    data: &str,
) -> Result<(), ()> {
    let frame = encoder.encode_text(data);
    stream.write_all(frame).map_err(|_| ())?;
    stream.flush().map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};

    struct MockStream {
        read_buffer: Vec<u8>,
        write_buffer: Vec<u8>,
    }

    impl MockStream {
        fn new() -> Self {
            MockStream {
                read_buffer: Vec::new(),
                write_buffer: Vec::new(),
            }
        }

        fn add_read_bytes(&mut self, bytes: &[u8]) {
            self.read_buffer.extend_from_slice(bytes);
        }
    }

    impl Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.read_buffer.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "would block",
                ));
            }
            let n = std::cmp::min(buf.len(), self.read_buffer.len());
            buf[..n].copy_from_slice(&self.read_buffer[..n]);
            self.read_buffer.drain(..n);
            Ok(n)
        }
    }

    impl Write for MockStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.write_buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn read_text_message_simple() {
        // Create a simple text frame: FIN=1, opcode=1, len=5, "hello"
        let mut frame = Vec::new();
        frame.push(0x81); // FIN + Text
        frame.push(0x05); // payload length
        frame.extend_from_slice(b"hello");

        let mut stream = MockStream::new();
        stream.add_read_bytes(&frame);

        let mut decoder = FrameDecoder::new();
        let msg = read_message(&mut decoder, &mut stream).unwrap();
        assert_eq!(msg, Some("hello".to_string()));
    }

    #[test]
    fn read_binary_message() {
        let mut frame = Vec::new();
        frame.push(0x82); // FIN + Binary
        frame.push(0x03); // payload length
        frame.extend_from_slice(&[1u8, 2, 3]);

        let mut stream = MockStream::new();
        stream.add_read_bytes(&frame);

        let mut decoder = FrameDecoder::new();
        let msg = read_message(&mut decoder, &mut stream).unwrap();
        assert_eq!(msg, Some("\x01\x02\x03".to_string()));
    }

    #[test]
    fn read_ping_returns_none() {
        let mut frame = Vec::new();
        frame.push(0x89); // FIN + Ping
        frame.push(0x00); // empty payload

        let mut stream = MockStream::new();
        stream.add_read_bytes(&frame);

        let mut decoder = FrameDecoder::new();
        let msg = read_message(&mut decoder, &mut stream).unwrap();
        assert_eq!(msg, None);
    }

    #[test]
    fn read_close_returns_err() {
        let mut frame = Vec::new();
        frame.push(0x88); // FIN + Close
        frame.push(0x00); // empty payload

        let mut stream = MockStream::new();
        stream.add_read_bytes(&frame);

        let mut decoder = FrameDecoder::new();
        let result = read_message(&mut decoder, &mut stream);
        assert!(result.is_err());
    }

    #[test]
    fn write_and_read_roundtrip() {
        let mut stream = MockStream::new();
        let mut encoder = FrameEncoder::new();
        let mut decoder = FrameDecoder::new();

        write_message(&mut encoder, &mut stream, "{\"id\":1}").unwrap();

        // Echo the write buffer back to read buffer
        let write_buffer = stream.write_buffer.clone();
        stream.add_read_bytes(&write_buffer);

        let msg = read_message(&mut decoder, &mut stream).unwrap();
        assert_eq!(msg, Some("{\"id\":1}".to_string()));
    }
}
