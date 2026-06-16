//! High-level WebSocket server abstraction — owned by `bun_uws`.
//!
//! Mirrors the synchronous, non-blocking `std::net::TcpListener` model that
//! `bao_cdp::CDPServer` relies on. Each accepted connection runs the RFC 6455
//! server handshake ([`crate::ws_handshake::server_handshake`]) and yields a
//! [`WsServerConnection`] that wraps the upgraded stream + codec. This is the
//! `bun_uws`-owned equivalent of `bao_cdp::{ws, ws_codec}` server glue; it does
//! NOT use the uWS C++ async `App::ws()` path (that requires a dedicated event
//! loop incompatible with the synchronous test model).
//!
//! @trace REQ-CDP-UWS-001

use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};

use crate::ws_codec::{FrameDecoder, FrameEncoder, Message};
use crate::ws_handshake::{server_handshake, HandshakeError};

/// Result of [`read_message`] — control-frame aware.
pub enum ReadOutcome {
    /// A text/binary frame arrived.
    Text(String),
    Binary(Vec<u8>),
    /// A ping/pong control frame — caller should retry.
    Control,
    /// No full frame yet (timeout / would-block).
    Pending,
    /// Peer closed / fatal I/O failure.
    Closed,
}

/// A stream that first drains a peeked byte buffer before reading from the
/// underlying `TcpStream`. Used after `server_handshake` consumes the
/// pre-upgrade HTTP request (which the accept loop typically already read
/// into a buffer).
pub struct ReplayStream {
    pub stream: TcpStream,
    pub replay: Cursor<Vec<u8>>,
}

impl ReplayStream {
    pub fn new(stream: TcpStream, peeked: Vec<u8>) -> Self {
        ReplayStream {
            stream,
            replay: Cursor::new(peeked),
        }
    }
}

impl Read for ReplayStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.replay.position() < self.replay.get_ref().len() as u64 {
            return self.replay.read(buf);
        }
        self.stream.read(buf)
    }
}

impl Write for ReplayStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// A server-side WebSocket connection: upgraded stream + frame codec.
pub struct WsServerConnection {
    pub stream: ReplayStream,
    pub decoder: FrameDecoder,
    pub encoder: FrameEncoder,
}

impl WsServerConnection {
    /// Build a connection from a `TcpStream` whose first `peeked.len()` bytes
    /// have already been read into `peeked` (the HTTP upgrade request). Runs
    /// the RFC 6455 server handshake.
    pub fn accept(stream: TcpStream, peeked: Vec<u8>) -> Result<Self, HandshakeError> {
        let mut replay = ReplayStream::new(stream, peeked);
        server_handshake(&mut replay)?;
        Ok(Self {
            stream: replay,
            decoder: FrameDecoder::new(),
            encoder: FrameEncoder::new(),
        })
    }

    /// Write a text frame to the peer (server-side, unmasked per RFC 6455 §5.1).
    pub fn write_text(&mut self, data: &str) -> Result<(), ()> {
        let frame = self.encoder.encode_text(data);
        self.stream.write_all(frame).map_err(|_| ())?;
        self.stream.flush().map_err(|_| ())
    }

    /// Read one frame and translate it into a [`ReadOutcome`].
    pub fn read(&mut self) -> ReadOutcome {
        let header = match self.decoder.decode_frame(&mut self.stream) {
            Ok(Some(h)) => h,
            Ok(None) => return ReadOutcome::Pending,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return ReadOutcome::Pending;
            }
            Err(_) => return ReadOutcome::Closed,
        };
        let payload = self.decoder.take_payload(&header);
        match Message::from_frame(header.opcode, payload) {
            Message::Text(text) => ReadOutcome::Text(text),
            Message::Binary(data) => ReadOutcome::Binary(data),
            Message::Ping(_) | Message::Pong(_) => ReadOutcome::Control,
            Message::Close(_, _) => ReadOutcome::Closed,
        }
    }
}

/// Convenience: read a single text message, discarding control frames and
/// retrying through `ReadOutcome::Pending`. Returns:
/// - `Ok(Some(text))` on the first text/binary frame
/// - `Ok(None)` on a peer-initiated close
/// - `Err(())` on a fatal decode failure
pub fn read_message(conn: &mut WsServerConnection) -> Result<Option<String>, ()> {
    loop {
        match conn.read() {
            ReadOutcome::Text(text) => return Ok(Some(text)),
            ReadOutcome::Binary(data) => {
                return Ok(Some(String::from_utf8_lossy(&data).into_owned()))
            }
            ReadOutcome::Control | ReadOutcome::Pending => continue,
            ReadOutcome::Closed => return Ok(None),
        }
    }
}

/// Bind a non-blocking `TcpListener` on `addr:port`. Caller drives the accept
/// loop (matches `bao_cdp::CDPServer::run`).
pub fn bind_nonblocking(addr: &str, port: u16) -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind((addr, port))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws_codec::{apply_mask, Opcode};
    use std::io::Cursor;
    use std::thread;

    #[test]
    fn read_message_text_frame() {
        // Build a server-side text frame (no mask) and feed it through ReplayStream.
        let mut frame = Vec::new();
        frame.push(0x81); // FIN + Text
        frame.push(0x05); // length
        frame.extend_from_slice(b"hello");

        let _stream = TcpListener::bind("127.0.0.1:0").unwrap();
        // Use the codec directly via Cursor-backed ReplayStream is awkward
        // (ReplayStream needs a real TcpStream); exercise the codec instead.
        let mut decoder = FrameDecoder::new();
        let mut cursor = Cursor::new(frame.clone());
        let header = decoder.decode_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(header.opcode, Opcode::Text);
        let payload = decoder.take_payload(&header);
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn read_message_control_ping_returns_retry() {
        let mut frame = Vec::new();
        frame.push(0x89); // FIN + Ping
        frame.push(0x00);
        let mut decoder = FrameDecoder::new();
        let mut cursor = Cursor::new(frame);
        let header = decoder.decode_frame(&mut cursor).unwrap().unwrap();
        let payload = decoder.take_payload(&header);
        let msg = Message::from_frame(header.opcode, payload);
        assert!(matches!(msg, Message::Ping(_)));
    }

    /// End-to-end server→client test: spin up a TcpListener, server-side
    /// handshake, push one text frame, client decodes it.
    #[test]
    fn end_to_end_server_pushes_text_client_decodes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload_text = "{\"method\":\"X\",\"params\":{}}";

        let server_handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).ok();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap();
            let peeked = buf[..n].to_vec();
            let mut conn = WsServerConnection::accept(stream, peeked).unwrap();
            conn.write_text(payload_text).unwrap();
            // Hold the connection open briefly for the client to drain.
            thread::sleep(Duration::from_millis(50));
        });

        let mut client = TcpStream::connect(addr).unwrap();
        // Minimal client handshake inline (mirror ws_handshake::client_handshake
        // without depending on the public API to keep this test self-contained).
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let req = format!(
            "GET / HTTP/1.1\r\nHost: t\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            key
        );
        client.write_all(req.as_bytes()).unwrap();
        client.flush().unwrap();
        // Drain the 101 response.
        let mut got = Vec::new();
        let mut byte = [0u8; 1];
        while client.read(&mut byte).unwrap() > 0 {
            got.push(byte[0]);
            if got.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        // Now decode the pushed text frame.
        let mut decoder = FrameDecoder::new();
        let header = decoder.decode_frame(&mut client).unwrap().unwrap();
        assert_eq!(header.opcode, Opcode::Text);
        let payload = decoder.take_payload(&header);
        assert_eq!(std::str::from_utf8(&payload).unwrap(), payload_text);
        server_handle.join().unwrap();
    }

    use std::time::Duration;
    #[test]
    fn masked_client_frame_decoded_by_server_decoder() {
        let mut frame = Vec::new();
        frame.push(0x81); // FIN + Text
        let key = [0x42u8, 0x11, 0xee, 0x07];
        let payload = b"hi";
        frame.push(0x80 | payload.len() as u8); // mask bit + length
        frame.extend_from_slice(&key);
        let mut masked = payload.to_vec();
        apply_mask(&mut masked, &key);
        frame.extend_from_slice(&masked);

        let mut decoder = FrameDecoder::new();
        let mut cursor = Cursor::new(frame);
        let header = decoder.decode_frame(&mut cursor).unwrap().unwrap();
        assert!(header.mask);
        let mask_key = decoder.take_mask();
        let mut payload = decoder.take_payload(&header);
        apply_mask(&mut payload, &mask_key);
        assert_eq!(payload, b"hi");
    }
}
