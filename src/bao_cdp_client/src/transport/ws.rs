//! WebSocketTransport — 外部 Chrome / Chromium 的 CDP 客户端模式。
//!
//! 复用 [`bao_cdp::ws_codec`](RFC 6455 帧编解码) 与
//! [`bao_cdp::ws_handshake`](WebSocket 握手) 完成 CDP 客户端:
//!
//! 1. `connect(url)`:
//!    - `TcpStream::connect(host:port)`
//!    - `ws_handshake::client_handshake(stream, host, path)`(REQ-BAO-API-002)
//!    - 包装为 `WebSocketTransport`
//! 2. `send_command`:
//!    - 构造 JSON-RPC request(分配 next id),`FrameEncoder::encode_text` 写入
//!    - 同步读 frame,根据 `id` 字段匹配响应 / `method` 字段判定事件
//! 3. `recv_event`:
//!    - 阻塞读 frame,无事件则 timeout 返回 `Ok(None)`
//!    - 自动处理 Ping/Pong 心跳
//! 4. `close`:
//!    - 发送 WS Close frame(1000),`TcpStream::shutdown`
//!
//! ## Masking(RFC 6455 §5.1)
//!
//! 客户端→服务端的帧**必须**带 mask。我们扩展 bao_cdp::ws_codec 的编码器
//! 客户端 masking 不依赖该模块(避免污染上游 bao_cdp),而在本模块内手写
//! mask XOR — 这是"必要的桥接层"例外,与 bao_engine 的 JSC→SM 桥接同性质。
//!
//! ## 同步语义
//!
//! 与 [`super::in_memory::InMemoryTransport`] 一致,使用阻塞 I/O:
//! `TcpStream::set_read_timeout` 控制 `recv_event` 超时。
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

use crate::error::{CdpError, Result};

use super::r#trait::{CdpEvent, Transport};
use super::TransportKind;

/// 默认读超时(控制 recv_event 的最大阻塞时长)。
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(100);

/// 默认命令超时(发送后等待响应的最长时间)。
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// WebSocketTransport — 外部 Chrome CDP 客户端模式。
///
/// 持有:
/// - `TcpStream`(原始字节流)
/// - `FrameDecoder`(累积 + 解析 RFC 6455 帧,复用 bao_cdp::ws_codec)
/// - `next_id`(JSON-RPC request id 计数器)
/// - pending 事件队列(`recv_event` 时,若读到的是事件而非响应,缓存起来)
///
/// 注:客户端→服务端帧必须 mask(RFC 6455 §5.1),而 `bao_cdp::ws_codec::FrameEncoder`
/// 当前仅支持 server-side unmasked。所以客户端模式在 [`encode_text_masked`] 内
/// 手写 mask 编码 — 这是"必要的桥接层"例外(同 bao_engine JSC→SM 桥接)。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
pub struct WebSocketTransport {
    stream: TcpStream,
    decoder: bao_cdp::ws_codec::FrameDecoder,
    next_id: u64,
    pending_events: std::collections::VecDeque<CdpEvent>,
    closed: bool,
    read_timeout: Duration,
    command_timeout: Duration,
}

impl std::fmt::Debug for WebSocketTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketTransport")
            .field("next_id", &self.next_id)
            .field("closed", &self.closed)
            .field("read_timeout", &self.read_timeout)
            .field("pending_events", &self.pending_events.len())
            .finish()
    }
}

impl WebSocketTransport {
    /// 连接到 WebSocket CDP endpoint。
    ///
    /// `url` 必须是 `ws://host:port/path` 形式(不含 `wss://` TLS,后续 TASK 增加 TLS)。
    ///
    /// # 错误
    /// - [`CdpError::HandshakeError`]:URL 格式错误 / 握手失败
    /// - [`CdpError::IoError`]:TCP 连接失败
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn connect(url: &str) -> Result<Self> {
        let (host, port, path) = parse_ws_url(url).ok_or_else(|| {
            CdpError::HandshakeError(format!("invalid ws URL: {}", url))
        })?;
        let addr = format!("{}:{}", host, port);
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| CdpError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?
            .next()
            .ok_or_else(|| CdpError::HandshakeError(format!("no addr resolved: {}", addr)))?;

        let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))?;
        stream.set_nodelay(true).ok();
        Self::connect_on_stream(stream, &host, &path)
    }

    /// 用已建立的 `TcpStream` 完成 handshake 并包装。
    ///
    /// 主要用于测试(mock server 跑在本地端口)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn connect_on_stream(mut stream: TcpStream, host: &str, path: &str) -> Result<Self> {
        bao_cdp::ws_handshake::client_handshake(&mut stream, host, path)
            .map_err(|_| CdpError::HandshakeError("client handshake failed".into()))?;
        stream.set_read_timeout(Some(DEFAULT_READ_TIMEOUT)).ok();
        stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
        Ok(Self {
            stream,
            decoder: bao_cdp::ws_codec::FrameDecoder::new(),
            next_id: 1,
            pending_events: std::collections::VecDeque::new(),
            closed: false,
            read_timeout: DEFAULT_READ_TIMEOUT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        })
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// 当前 next id(测试断言用)。
    pub fn current_id(&self) -> u64 {
        self.next_id
    }

    /// 读一个 frame,返回 (opcode, payload)。处理心跳/重试。
    fn read_frame(&mut self) -> Result<Option<(bao_cdp::ws_codec::Opcode, Vec<u8>)>> {
        use bao_cdp::ws_codec::Opcode;
        loop {
            let header = match self.decoder.decode_frame(&mut self.stream) {
                Ok(Some(h)) => h,
                Ok(None) => return Ok(None),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return Ok(None);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(CdpError::ConnectionClosed);
                }
                Err(e) => return Err(CdpError::IoError(e)),
            };

            let payload = if header.mask {
                let mask_key = self.decoder.take_mask();
                let mut payload = self.decoder.take_payload(&header);
                apply_mask(&mut payload, &mask_key);
                payload
            } else {
                self.decoder.take_payload(&header)
            };

            match header.opcode {
                Opcode::Ping => {
                    // Echo Pong with same payload.
                    let frame = encode_pong_with_payload(&payload, true);
                    self.stream.write_all(&frame).map_err(CdpError::IoError)?;
                    continue; // read next frame
                }
                Opcode::Pong => continue, // ignore, read next frame
                Opcode::Close => {
                    return Err(CdpError::ConnectionClosed);
                }
                Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                    return Ok(Some((header.opcode, payload)));
                }
            }
        }
    }

    /// 写一帧(客户端必须 mask)。
    fn write_text_frame(&mut self, payload: &str) -> Result<()> {
        let frame = encode_text_masked(payload);
        self.stream.write_all(&frame).map_err(CdpError::IoError)?;
        self.stream.flush().map_err(CdpError::IoError)?;
        Ok(())
    }
}

impl Transport for WebSocketTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::WebSocket
    }

    fn send_command(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        if self.closed {
            return Err(CdpError::ConnectionClosed);
        }

        let id = self.next_id;
        self.next_id += 1;

        let mut req = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(sid) = session_id {
            req["sessionId"] = serde_json::Value::String(sid.to_string());
        }
        let json = serde_json::to_string(&req)?;
        self.write_text_frame(&json)?;

        // Loop reading frames until we see a response with our id (events queued).
        let deadline = std::time::Instant::now() + self.command_timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err(CdpError::Timeout(format!(
                    "command {} (id={}) timed out after {:?}",
                    method, id, self.command_timeout
                )));
            }
            match self.read_frame()? {
                None => continue, // WouldBlock — try again
                Some((_op, payload)) => {
                    let v: Value = serde_json::from_slice(&payload)?;
                    if let Some(resp_id) = v.get("id").and_then(|i| i.as_u64()) {
                        if resp_id == id {
                            // Match — extract result/error.
                            if let Some(err) = v.get("error") {
                                return Err(CdpError::ProtocolError(err.to_string()));
                            }
                            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                        }
                        // Different id — unexpected in single-thread model; drop.
                        continue;
                    }
                    if v.get("method").is_some() {
                        // Event — queue for later recv_event.
                        let method = v["method"].as_str().unwrap_or("").to_string();
                        let params = v.get("params").cloned().unwrap_or(Value::Null);
                        let session_id = v
                            .get("sessionId")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        let mut ev = CdpEvent::new(method, params);
                        if let Some(sid) = session_id {
                            ev = ev.with_session(sid);
                        }
                        self.pending_events.push_back(ev);
                        continue;
                    }
                    // Unknown frame structure — skip.
                    continue;
                }
            }
        }
    }

    fn recv_event(&mut self) -> Result<Option<CdpEvent>> {
        if self.closed {
            return Err(CdpError::ConnectionClosed);
        }
        // Fast path: queued events from send_command.
        if let Some(ev) = self.pending_events.pop_front() {
            return Ok(Some(ev));
        }
        match self.read_frame()? {
            None => Ok(None),
            Some((_op, payload)) => {
                let v: Value = serde_json::from_slice(&payload)?;
                let method = v["method"].as_str().unwrap_or("").to_string();
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let session_id = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                let mut ev = CdpEvent::new(method, params);
                if let Some(sid) = session_id {
                    ev = ev.with_session(sid);
                }
                Ok(Some(ev))
            }
        }
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // Best-effort: send WS Close frame, then shutdown.
        let close_frame = encode_close_masked(1000, "");
        let _ = self.stream.write_all(&close_frame);
        let _ = self.stream.flush();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        Ok(())
    }

    fn set_command_timeout(&mut self, timeout: Duration) {
        self.command_timeout = timeout;
    }

    fn set_event_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
        let _ = self.stream.set_read_timeout(Some(timeout));
    }
}

// ────────────────────────────────────────────────────────────────────────────
// URL parsing + RFC 6455 frame masking helpers
// ────────────────────────────────────────────────────────────────────────────

/// Parse `ws://host:port/path` into (host, port, path).
/// Returns None on parse failure.
fn parse_ws_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("ws://")?;
    // split at first '/' (path separator)
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rfind(':') {
        Some(i) => {
            let port: u16 = authority[i + 1..].parse().ok()?;
            (&authority[..i], port)
        }
        None => (authority, 80), // default ws port
    };
    Some((host.to_string(), port, path.to_string()))
}

/// Apply RFC 6455 §5.3 mask: payload[i] ^= mask[i % 4]
fn apply_mask(payload: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}

/// Generate 4-byte mask key. RFC 6455 §5.3 requires "non-zero" entropy.
fn gen_mask_key() -> [u8; 4] {
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
    // RFC 6455 §5.3: mask SHOULD be non-zero (avoid fallback to all-zero).
    if out == [0, 0, 0, 0] {
        out = [0x12, 0x34, 0x56, 0x78];
    }
    out
}

/// Encode a text frame with client masking(RFC 6455 §5.1 mandates masking for client→server).
fn encode_text_masked(payload: &str) -> Vec<u8> {
    encode_frame(bao_cdp::ws_codec::Opcode::Text, payload.as_bytes(), true)
}

/// Encode a close frame with client masking.
fn encode_close_masked(code: u16, reason: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(2 + reason.len());
    payload.extend_from_slice(&code.to_be_bytes());
    payload.extend_from_slice(reason.as_bytes());
    encode_frame(bao_cdp::ws_codec::Opcode::Close, &payload, true)
}

/// Encode a pong frame with client masking (used for echo).
fn encode_pong_with_payload(payload: &[u8], mask: bool) -> Vec<u8> {
    encode_frame(bao_cdp::ws_codec::Opcode::Pong, payload, mask)
}

/// Low-level frame encoder with optional masking.
fn encode_frame(opcode: bao_cdp::ws_codec::Opcode, payload: &[u8], mask: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(payload.len() + 14);
    let fin = 0x80u8;
    buf.push(fin | (opcode as u8));
    let mask_bit = if mask { 0x80u8 } else { 0u8 };

    let len = payload.len();
    if len < 126 {
        buf.push((len as u8) | mask_bit);
    } else if len <= u16::MAX as usize {
        buf.push(126u8 | mask_bit);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(127u8 | mask_bit);
        buf.extend_from_slice(&(len as u64).to_be_bytes());
    }

    let mask_key = if mask {
        let k = gen_mask_key();
        buf.extend_from_slice(&k);
        Some(k)
    } else {
        None
    };

    if let Some(k) = mask_key {
        let mut masked = payload.to_vec();
        apply_mask(&mut masked, &k);
        buf.extend_from_slice(&masked);
    } else {
        buf.extend_from_slice(payload);
    }
    buf
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
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
    fn parse_ws_url_no_port() {
        let (h, p, _path) = parse_ws_url("ws://example.com/ws").unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 80); // default
    }

    #[test]
    fn parse_ws_url_invalid_scheme() {
        assert!(parse_ws_url("wss://x").is_none()); // wss not yet supported
        assert!(parse_ws_url("http://x").is_none());
        assert!(parse_ws_url("garbage").is_none());
    }

    #[test]
    fn apply_mask_round_trip() {
        let original = b"hello world".to_vec();
        let mut buf = original.clone();
        let key = [0x37u8, 0xfa, 0x21, 0x3d];
        apply_mask(&mut buf, &key);
        assert_ne!(buf, original, "mask did not change payload");
        apply_mask(&mut buf, &key); // unmask
        assert_eq!(buf, original, "double-mask did not restore");
    }

    #[test]
    fn encode_text_masked_frame_layout() {
        let frame = encode_text_masked("hi");
        assert_eq!(frame[0] & 0x0F, bao_cdp::ws_codec::Opcode::Text as u8);
        assert!(frame[0] & 0x80 != 0); // FIN
        assert!(frame[1] & 0x80 != 0); // mask bit
        assert_eq!(frame[1] & 0x7F, 2); // length
        // 4 mask bytes follow
        let mask = [frame[2], frame[3], frame[4], frame[5]];
        let mut payload = frame[6..].to_vec();
        apply_mask(&mut payload, &mask);
        assert_eq!(&payload, b"hi");
    }

    #[test]
    fn encode_close_masked_frame_layout() {
        let frame = encode_close_masked(1000, "");
        assert_eq!(frame[0] & 0x0F, bao_cdp::ws_codec::Opcode::Close as u8);
        assert!(frame[1] & 0x80 != 0); // mask
        assert_eq!(frame[1] & 0x7F, 2); // length = 2 (code only)
    }

    /// Helper: spin up a minimal WebSocket server that completes the client
    /// handshake (server side), then echoes back a single JSON response for
    /// any incoming text frame.
    struct EchoServer {
        addr: String,
        _port: u16,
        _handle: thread::JoinHandle<()>,
    }

    impl EchoServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let addr = format!("127.0.0.1:{}", port);
            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    // 1) server-side handshake
                    if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
                        return;
                    }
                    // 2) read 1 frame, echo back response with same id
                    let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
                    let header = match decoder.decode_frame(&mut stream) {
                        Ok(Some(h)) => h,
                        Ok(None) => return,
                        Err(_) => return,
                    };
                    let payload = if header.mask {
                        let mask = decoder.take_mask();
                        let mut p = decoder.take_payload(&header);
                        apply_mask(&mut p, &mask);
                        p
                    } else {
                        decoder.take_payload(&header)
                    };
                    let v: Value = match serde_json::from_slice(&payload) {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    let id = v.get("id").cloned().unwrap_or(Value::Null);
                    let method = v.get("method").cloned().unwrap_or(Value::Null);
                    let response = serde_json::json!({
                        "id": id,
                        "result": {"echoedMethod": method},
                    });
                    let resp_json = serde_json::to_string(&response).unwrap();
                    let resp_frame = encode_text_unmasked_for_server(&resp_json);
                    let _ = stream.write_all(&resp_frame);
                    let _ = stream.flush();
                    // 3) server stays alive briefly to allow close
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            Self {
                addr,
                _port: port,
                _handle: handle,
            }
        }

        fn url(&self) -> String {
            format!("ws://{}/test", self.addr)
        }
    }

    /// Server-side frame encoder (NO masking — RFC 6455 §5.1 mandates server→client unmasked).
    fn encode_text_unmasked_for_server(payload: &str) -> Vec<u8> {
        encode_frame(bao_cdp::ws_codec::Opcode::Text, payload.as_bytes(), false)
    }

    #[test]
    fn ws_transport_connect_and_send_command() {
        let server = EchoServer::start();
        let url = server.url();
        let mut t = WebSocketTransport::connect(&url).expect("connect");
        assert_eq!(t.kind(), TransportKind::WebSocket);
        assert_eq!(t.current_id(), 1);

        let result = t
            .send_command("Page.getTitle", serde_json::json!({}), None)
            .expect("send_command");
        assert_eq!(result["echoedMethod"], "Page.getTitle");
        assert_eq!(t.current_id(), 2, "id counter should increment");
    }

    #[test]
    fn ws_transport_close_is_safe() {
        let server = EchoServer::start();
        let mut t = WebSocketTransport::connect(&server.url()).unwrap();
        t.close().unwrap();
        assert!(t.is_closed());
        // After close, send_command → ConnectionClosed.
        let err = t
            .send_command("X.y", serde_json::json!({}), None)
            .unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn ws_transport_close_idempotent() {
        let server = EchoServer::start();
        let mut t = WebSocketTransport::connect(&server.url()).unwrap();
        t.close().unwrap();
        t.close().unwrap();
        t.close().unwrap();
    }

    #[test]
    fn ws_transport_connect_invalid_url() {
        let err = WebSocketTransport::connect("ws://no-such-host-9999:65535/x").unwrap_err();
        // Either HandshakeError or IoError.
        match err {
            CdpError::HandshakeError(_) | CdpError::IoError(_) => {}
            other => panic!("expected handshake/io err, got {:?}", other),
        }
    }

    #[test]
    fn ws_transport_parse_url_invalid_for_connect() {
        let err = WebSocketTransport::connect("not a url").unwrap_err();
        assert!(matches!(err, CdpError::HandshakeError(_)));
    }

    #[test]
    fn ws_transport_debug_format() {
        let server = EchoServer::start();
        let t = WebSocketTransport::connect(&server.url()).unwrap();
        let s = format!("{:?}", t);
        assert!(s.contains("WebSocketTransport"));
        assert!(s.contains("next_id"));
    }

    /// Server that pushes one CDP event after handshake (for recv_event test).
    struct EventPushServer {
        addr: String,
        _handle: thread::JoinHandle<()>,
    }

    impl EventPushServer {
        fn start(event_method: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
                        return;
                    }
                    // Push one event.
                    let event = serde_json::json!({
                        "method": event_method,
                        "params": {"hello": "world"},
                    });
                    let json = serde_json::to_string(&event).unwrap();
                    let frame = encode_text_unmasked_for_server(&json);
                    let _ = stream.write_all(&frame);
                    let _ = stream.flush();
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            Self { addr, _handle: handle }
        }

        fn url(&self) -> String {
            format!("ws://{}/test", self.addr)
        }
    }

    #[test]
    fn ws_transport_recv_event_gets_pushed_event() {
        let server = EventPushServer::start("Page.frameNavigated");
        let mut t = WebSocketTransport::connect(&server.url()).unwrap();
        // Use a generous timeout for CI.
        t.set_event_timeout(Duration::from_secs(2));
        let ev = t.recv_event().expect("recv ok").expect("got an event");
        assert_eq!(ev.method, "Page.frameNavigated");
        assert_eq!(ev.params["hello"], "world");
    }

    #[test]
    fn ws_transport_recv_event_returns_none_on_timeout() {
        // Use a server that doesn't push anything.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let _h = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        let mut t = WebSocketTransport::connect(&format!("ws://{}/test", addr)).unwrap();
        t.set_event_timeout(Duration::from_millis(20));
        let ev = t.recv_event().unwrap();
        assert!(ev.is_none(), "expected timeout → None");
    }

    #[test]
    fn ws_transport_event_after_close_returns_connection_closed() {
        let server = EventPushServer::start("X");
        let mut t = WebSocketTransport::connect(&server.url()).unwrap();
        t.close().unwrap();
        let err = t.recv_event().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn ws_transport_connect_on_stream_works() {
        let server = EchoServer::start();
        // Manually create stream and call connect_on_stream.
        let stream = TcpStream::connect(&server.addr).unwrap();
        let mut t = WebSocketTransport::connect_on_stream(stream, "127.0.0.1", "/test").unwrap();
        let result = t
            .send_command("Foo.bar", serde_json::json!({}), None)
            .unwrap();
        assert_eq!(result["echoedMethod"], "Foo.bar");
    }

    #[test]
    fn gen_mask_key_is_nonzero() {
        for _ in 0..10 {
            let k = gen_mask_key();
            assert_ne!(k, [0, 0, 0, 0]);
        }
    }
}
