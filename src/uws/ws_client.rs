//! High-level WebSocket client API — owned by `bun_uws`.
//!
//! `ws_connect(url)` returns a [`WebSocketClient`] backed by a `std::net::TcpStream`.
//! Frames are RFC 6455-encoded via [`crate::ws_codec`]; the upgrade handshake uses
//! [`crate::ws_handshake::client_handshake`]. Client→server frames are masked
//! (RFC 6455 §5.1 mandates masking); the masking key is supplied per-frame via
//! [`crate::ws_codec::gen_mask_key`].
//!
//! uWS C++ upstream ships an unfinished `ClientApp.h` (the body of `connect` is
//! empty), so a uWS-C++-backed WS client does not exist. This module is the
//! `bun_uws`-owned equivalent: synchronous Rust, masks correctly, reuses the
//! codec that bao_cdp / bao_cdp_client previously held.
//!
//! @trace REQ-CDP-UWS-001

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::ws_codec::{apply_mask, gen_mask_key, FrameDecoder, Opcode};
use crate::ws_handshake::{client_handshake, HandshakeError};

/// Default per-frame read timeout — controls how long [`WebSocketClient::recv`]
/// blocks before returning [`RecvOutcome::Timeout`].
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(100);

/// Errors returned by the client.
#[derive(Debug)]
pub enum WsClientError {
    /// URL parsing failure (wrong scheme / malformed).
    InvalidUrl,
    /// TCP connect or DNS resolution failure.
    Connect(std::io::Error),
    /// RFC 6455 handshake failure.
    Handshake(HandshakeError),
    /// Frame I/O failure.
    Io(std::io::Error),
    /// Server sent a Close frame.
    Closed,
}

impl std::fmt::Display for WsClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsClientError::InvalidUrl => write!(f, "invalid ws URL"),
            WsClientError::Connect(e) => write!(f, "connect: {}", e),
            WsClientError::Handshake(e) => write!(f, "handshake: {:?}", e),
            WsClientError::Io(e) => write!(f, "io: {}", e),
            WsClientError::Closed => write!(f, "connection closed"),
        }
    }
}

impl std::error::Error for WsClientError {}

impl From<std::io::Error> for WsClientError {
    fn from(e: std::io::Error) -> Self {
        WsClientError::Io(e)
    }
}

impl From<HandshakeError> for WsClientError {
    fn from(e: HandshakeError) -> Self {
        WsClientError::Handshake(e)
    }
}

/// Outcome of a [`WebSocketClient::recv`] call.
#[derive(Debug)]
pub enum RecvOutcome {
    /// A text/binary frame arrived.
    Message(Opcode, Vec<u8>),
    /// No full frame available within the read timeout.
    Timeout,
    /// Server closed the connection.
    Closed,
}

/// Parse `ws://host:port/path` into `(host, port, path)`.
pub fn parse_ws_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("ws://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rfind(':') {
        Some(i) => {
            let port: u16 = authority[i + 1..].parse().ok()?;
            (&authority[..i], port)
        }
        None => (authority, 80),
    };
    Some((host.to_string(), port, path.to_string()))
}

/// Synchronous WebSocket client over a `TcpStream`.
pub struct WebSocketClient {
    stream: TcpStream,
    decoder: FrameDecoder,
    closed: bool,
    read_timeout: Duration,
}

impl std::fmt::Debug for WebSocketClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketClient")
            .field("closed", &self.closed)
            .field("read_timeout", &self.read_timeout)
            .finish()
    }
}

impl WebSocketClient {
    /// Connect to `ws://host:port/path`. Performs TCP connect, the RFC 6455
    /// client handshake, and wires up the default read timeout.
    pub fn connect(url: &str) -> Result<Self, WsClientError> {
        let (host, port, path) =
            parse_ws_url(url).ok_or(WsClientError::InvalidUrl)?;
        let addr = format!("{}:{}", host, port);
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| {
                WsClientError::Connect(std::io::Error::new(std::io::ErrorKind::Other, e))
            })?
            .next()
            .ok_or(WsClientError::InvalidUrl)?;
        let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))
            .map_err(WsClientError::Connect)?;
        stream.set_nodelay(true).ok();
        Self::connect_on_stream(stream, &host, &path)
    }

    /// Wrap an already-connected stream and complete the handshake on it.
    pub fn connect_on_stream(
        mut stream: TcpStream,
        host: &str,
        path: &str,
    ) -> Result<Self, WsClientError> {
        client_handshake(&mut stream, host, path)?;
        stream.set_read_timeout(Some(DEFAULT_READ_TIMEOUT)).ok();
        stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
        Ok(Self {
            stream,
            decoder: FrameDecoder::new(),
            closed: false,
            read_timeout: DEFAULT_READ_TIMEOUT,
        })
    }

    /// Whether [`close`](Self::close) has been called or the peer has closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Configure the underlying read timeout (controls [`recv`](Self::recv)).
    pub fn set_read_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
        let _ = self.stream.set_read_timeout(Some(timeout));
    }

    /// Get the underlying stream's read timeout (for inspection / tests).
    pub fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    /// Borrow the raw stream (e.g. for callers that need to write a non-frame
    /// byte sequence). Borrowing is read-write so test harnesses can poll.
    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }

    /// Send a text frame. Masks per RFC 6455 §5.1.
    pub fn send_text(&mut self, payload: &str) -> Result<(), WsClientError> {
        self.send_frame(Opcode::Text, payload.as_bytes())
    }

    /// Send a binary frame. Masks per RFC 6455 §5.1.
    pub fn send_binary(&mut self, payload: &[u8]) -> Result<(), WsClientError> {
        self.send_frame(Opcode::Binary, payload)
    }

    /// Send a masked frame of the given opcode.
    pub fn send_frame(&mut self, opcode: Opcode, payload: &[u8]) -> Result<(), WsClientError> {
        if self.closed {
            return Err(WsClientError::Closed);
        }
        let key = gen_mask_key();
        let mut frame = Vec::with_capacity(payload.len() + 14);
        let fin = 0x80u8;
        frame.push(fin | (opcode as u8));
        let mask_bit = 0x80u8;
        let len = payload.len();
        if len < 126 {
            frame.push((len as u8) | mask_bit);
        } else if len <= u16::MAX as usize {
            frame.push(126u8 | mask_bit);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(127u8 | mask_bit);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        frame.extend_from_slice(&key);
        let mut masked = payload.to_vec();
        apply_mask(&mut masked, &key);
        frame.extend_from_slice(&masked);
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Read one frame. Handles ping (auto-pongs) and close automatically.
    /// Text/binary/continuation frames bubble up as
    /// [`RecvOutcome::Message`]. WouldBlock / TimedOut → [`RecvOutcome::Timeout`].
    pub fn recv(&mut self) -> Result<RecvOutcome, WsClientError> {
        loop {
            let header = match self.decoder.decode_frame(&mut self.stream) {
                Ok(Some(h)) => h,
                Ok(None) => return Ok(RecvOutcome::Timeout),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Ok(RecvOutcome::Timeout);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.closed = true;
                    return Ok(RecvOutcome::Closed);
                }
                Err(e) => return Err(WsClientError::Io(e)),
            };

            let payload = if header.mask {
                let mask_key = self.decoder.take_mask();
                let mut p = self.decoder.take_payload(&header);
                apply_mask(&mut p, &mask_key);
                p
            } else {
                self.decoder.take_payload(&header)
            };

            match header.opcode {
                Opcode::Ping => {
                    // Echo Pong with the same payload (RFC 6455 §5.5.2).
                    self.send_frame(Opcode::Pong, &payload)?;
                    continue;
                }
                Opcode::Pong => continue,
                Opcode::Close => {
                    self.closed = true;
                    return Ok(RecvOutcome::Closed);
                }
                Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                    return Ok(RecvOutcome::Message(header.opcode, payload));
                }
            }
        }
    }

    /// Send a Close frame (code 1000, empty reason) and shutdown the stream.
    /// Idempotent.
    pub fn close(&mut self) -> Result<(), WsClientError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut payload = Vec::with_capacity(2);
        payload.extend_from_slice(&1000u16.to_be_bytes());
        let _ = self.send_frame(Opcode::Close, &payload);
        let _ = self.stream.flush();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        Ok(())
    }
}

/// Convenience: connect, return a [`WebSocketClient`]. Equivalent to
/// [`WebSocketClient::connect`].
pub fn ws_connect(url: &str) -> Result<WebSocketClient, WsClientError> {
    WebSocketClient::connect(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws_codec::{apply_mask, Opcode};
    use crate::ws_handshake::server_handshake;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parse_ws_url_basic() {
        let (h, p, path) = parse_ws_url("ws://localhost:9222/devtools/page/abc").unwrap();
        assert_eq!(h, "localhost");
        assert_eq!(p, 9222);
        assert_eq!(path, "/devtools/page/abc");
    }

    #[test]
    fn parse_ws_url_no_path() {
        let (h, p, path) = parse_ws_url("ws://127.0.0.1:9222").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 9222);
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_no_port_defaults_to_80() {
        let (h, p, _path) = parse_ws_url("ws://example.com/ws").unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 80);
    }

    #[test]
    fn parse_ws_url_rejects_non_ws() {
        assert!(parse_ws_url("wss://x").is_none());
        assert!(parse_ws_url("http://x").is_none());
        assert!(parse_ws_url("garbage").is_none());
    }

    fn encode_server_text(payload: &str) -> Vec<u8> {
        let mut buf = Vec::with_capacity(payload.len() + 2);
        buf.push(0x81); // FIN + Text
        buf.push(payload.len() as u8); // server: no mask bit
        buf.extend_from_slice(payload.as_bytes());
        buf
    }

    /// Minimal echo WS server: handshake, read one frame, echo "ECHO:<payload>".
    struct EchoServer {
        addr: String,
        _handle: thread::JoinHandle<()>,
    }

    impl EchoServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    if server_handshake(&mut stream).is_err() {
                        return;
                    }
                    let mut decoder = FrameDecoder::new();
                    let header = match decoder.decode_frame(&mut stream) {
                        Ok(Some(h)) => h,
                        _ => return,
                    };
                    let payload = if header.mask {
                        let mask = decoder.take_mask();
                        let mut p = decoder.take_payload(&header);
                        apply_mask(&mut p, &mask);
                        p
                    } else {
                        decoder.take_payload(&header)
                    };
                    let text = String::from_utf8_lossy(&payload).into_owned();
                    let echo = format!("ECHO:{}", text);
                    let _ = stream.write_all(&encode_server_text(&echo));
                    let _ = stream.flush();
                    thread::sleep(Duration::from_millis(50));
                }
            });
            Self { addr, _handle: handle }
        }

        fn url(&self) -> String {
            format!("ws://{}/test", self.addr)
        }
    }

    #[test]
    fn ws_connect_send_and_recv() {
        let server = EchoServer::start();
        let mut c = ws_connect(&server.url()).expect("connect");
        c.send_text("hello").unwrap();
        // Read the response.
        let outcome = c.recv().unwrap();
        match outcome {
            RecvOutcome::Message(_op, payload) => {
                assert_eq!(String::from_utf8(payload).unwrap(), "ECHO:hello");
            }
            _ => panic!("expected message, got {:?}", outcome),
        }
    }

    #[test]
    fn ws_connect_invalid_url() {
        let err = ws_connect("not a url").unwrap_err();
        assert!(matches!(err, WsClientError::InvalidUrl));
    }

    #[test]
    fn ws_connect_refused_host() {
        let err = ws_connect("ws://127.0.0.1:1/x").unwrap_err();
        assert!(matches!(err, WsClientError::Connect(_)));
    }

    #[test]
    fn close_is_idempotent() {
        let server = EchoServer::start();
        let mut c = ws_connect(&server.url()).unwrap();
        c.close().unwrap();
        c.close().unwrap();
        assert!(c.is_closed());
    }

    #[test]
    fn send_binary_frame_round_trip_layout() {
        // Verify the encoder path: text and binary share send_frame.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let _h = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            server_handshake(&mut s).unwrap();
            let mut decoder = FrameDecoder::new();
            let header = decoder.decode_frame(&mut s).unwrap().unwrap();
            assert_eq!(header.opcode, Opcode::Binary);
            assert!(header.mask);
        });
        let mut c = WebSocketClient::connect(&format!("ws://{}/t", addr)).unwrap();
        c.send_binary(&[1, 2, 3, 4]).unwrap();
        c.close().unwrap();
        let _ = _h.join();
    }
}
