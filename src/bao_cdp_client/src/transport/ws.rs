//! WebSocketTransport — 外部 Chrome / Chromium 的 CDP 客户端模式。
//!
//! REQ-CDP-UWS-001: 现在使用 [`bun_uws::ws_client::WebSocketClient`] 完成
//! RFC 6455 握手 + 客户端→服务端帧 masking + 帧编解码。所有 WebSocket 表面
//! 入口统一在 `bun_uws`，本 crate 仅依赖 `bun_uws`(无 tungstenite)。
//!
//! 1. `connect(url)`:
//!    - `bun_uws::ws_client::WebSocketClient::connect(url)` 完成 TCP + 握手
//!    - 内部已配置 100ms 默认读超时
//! 2. `send_command`:
//!    - 构造 JSON-RPC request(分配 next id),`client.send_text(json)`
//!    - 同步读 frame,根据 `id` 字段匹配响应 / `method` 字段判定事件
//! 3. `recv_event`:
//!    - 阻塞读 frame,无事件则 timeout 返回 `Ok(None)`
//!    - 自动处理 Ping/Pong 心跳(`bun_uws::ws_client` 内置)
//! 4. `close`:
//!    - 发送 WS Close frame(1000),`shutdown` 流
//!
//! ## Transport trait 接口不变
//!
//! 上层 API (Browser / CDPRdpBridge) 不受影响。`send_command` / `recv_event`
//! / `close` / `set_command_timeout` / `set_event_timeout` 行为与旧实现一致。
//!
//! @trace REQ-BAO-API-002 [interface:Transport]
//! @trace REQ-CDP-UWS-001

use std::time::Duration;

use serde_json::Value;

use bun_uws::ws_client::{parse_ws_url, RecvOutcome, WebSocketClient, WsClientError};

use crate::error::{CdpError, Result};

use super::r#trait::{CdpEvent, Transport};
use super::TransportKind;

/// 默认命令超时(发送后等待响应的最长时间)。
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// WebSocketTransport — 外部 Chrome CDP 客户端模式。
///
/// 包装 [`bun_uws::ws_client::WebSocketClient`]。Transport trait 接口不变。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
/// @trace REQ-CDP-UWS-001
pub struct WebSocketTransport {
    client: WebSocketClient,
    next_id: u64,
    pending_events: std::collections::VecDeque<CdpEvent>,
    command_timeout: Duration,
}

impl std::fmt::Debug for WebSocketTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketTransport")
            .field("next_id", &self.next_id)
            .field("command_timeout", &self.command_timeout)
            .field("pending_events", &self.pending_events.len())
            .finish()
    }
}

/// Map [`WsClientError`] to [`CdpError`].
fn map_ws_err(e: WsClientError) -> CdpError {
    match e {
        WsClientError::InvalidUrl => CdpError::HandshakeError("invalid ws URL".to_string()),
        WsClientError::Connect(io) => CdpError::IoError(io),
        WsClientError::Handshake(h) => {
            CdpError::HandshakeError(format!("handshake failed: {:?}", h))
        }
        WsClientError::Io(io) => CdpError::IoError(io),
        WsClientError::Closed => CdpError::ConnectionClosed,
    }
}

impl WebSocketTransport {
    /// 连接到 WebSocket CDP endpoint。
    ///
    /// `url` 必须是 `ws://host:port/path` 形式(`wss://` TLS 后续 TASK 增加)。
    ///
    /// # 错误
    /// - [`CdpError::HandshakeError`]:URL 格式错误 / 握手失败
    /// - [`CdpError::IoError`]:TCP 连接失败
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn connect(url: &str) -> Result<Self> {
        // Validate URL shape up-front so callers see a clean HandshakeError
        // rather than a connect-time IoError on a malformed authority.
        if parse_ws_url(url).is_none() {
            return Err(CdpError::HandshakeError(format!("invalid ws URL: {}", url)));
        }
        let client = WebSocketClient::connect(url).map_err(map_ws_err)?;
        Ok(Self {
            client,
            next_id: 1,
            pending_events: std::collections::VecDeque::new(),
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        })
    }

    /// 用已建立的 `TcpStream` 完成 handshake 并包装。
    ///
    /// 主要用于测试(mock server 跑在本地端口)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn connect_on_stream(stream: std::net::TcpStream, host: &str, path: &str) -> Result<Self> {
        let client = WebSocketClient::connect_on_stream(stream, host, path).map_err(map_ws_err)?;
        Ok(Self {
            client,
            next_id: 1,
            pending_events: std::collections::VecDeque::new(),
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        })
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    /// 当前 next id(测试断言用)。
    pub fn current_id(&self) -> u64 {
        self.next_id
    }

    /// 读取一帧,自动处理 Ping/Pong(`bun_uws::ws_client` 内置),返回
    /// 携带 text/binary payload 的事件,或在 timeout 时返回 `Ok(None)`。
    fn read_data_frame(&mut self) -> Result<Option<Vec<u8>>> {
        match self.client.recv().map_err(map_ws_err)? {
            RecvOutcome::Message(_op, payload) => Ok(Some(payload)),
            RecvOutcome::Timeout => Ok(None),
            RecvOutcome::Closed => Err(CdpError::ConnectionClosed),
        }
    }

    /// 写一帧 text(由 `bun_uws::ws_client::send_text` 自动 mask)。
    fn write_text_frame(&mut self, payload: &str) -> Result<()> {
        self.client.send_text(payload).map_err(map_ws_err)
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
        if self.client.is_closed() {
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

        let deadline = std::time::Instant::now() + self.command_timeout;
        loop {
            if std::time::Instant::now() > deadline {
                return Err(CdpError::Timeout(format!(
                    "command {} (id={}) timed out after {:?}",
                    method, id, self.command_timeout
                )));
            }
            match self.read_data_frame()? {
                None => continue, // WouldBlock — try again
                Some(payload) => {
                    let v: Value = serde_json::from_slice(&payload)?;
                    if let Some(resp_id) = v.get("id").and_then(|i| i.as_u64()) {
                        if resp_id == id {
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
        if self.client.is_closed() {
            return Err(CdpError::ConnectionClosed);
        }
        if let Some(ev) = self.pending_events.pop_front() {
            return Ok(Some(ev));
        }
        match self.read_data_frame()? {
            None => Ok(None),
            Some(payload) => {
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
        self.client.close().map_err(map_ws_err)
    }

    fn set_command_timeout(&mut self, timeout: Duration) {
        self.command_timeout = timeout;
    }

    fn set_event_timeout(&mut self, timeout: Duration) {
        self.client.set_read_timeout(timeout);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bun_uws::ws_codec::{apply_mask, Opcode};
    use bun_uws::ws_handshake::server_handshake;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

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
        let err = WebSocketTransport::connect("not a url").unwrap_err();
        assert!(matches!(err, CdpError::HandshakeError(_)));
    }

    #[test]
    fn ws_transport_connect_refused() {
        let err = WebSocketTransport::connect("ws://127.0.0.1:1/x").unwrap_err();
        match err {
            CdpError::HandshakeError(_) | CdpError::IoError(_) => {}
            other => panic!("expected handshake/io err, got {:?}", other),
        }
    }

    #[test]
    fn ws_transport_debug_format() {
        let server = EchoServer::start();
        let t = WebSocketTransport::connect(&server.url()).unwrap();
        let s = format!("{:?}", t);
        assert!(s.contains("WebSocketTransport"));
        assert!(s.contains("next_id"));
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
                    if server_handshake(&mut stream).is_err() {
                        return;
                    }
                    let mut decoder = bun_uws::ws_codec::FrameDecoder::new();
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
        let mut buf = Vec::with_capacity(payload.len() + 2);
        buf.push(0x81); // FIN + Text
        buf.push(payload.len() as u8);
        buf.extend_from_slice(payload.as_bytes());
        buf
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
                    if server_handshake(&mut stream).is_err() {
                        return;
                    }
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
            Self {
                addr,
                _handle: handle,
            }
        }

        fn url(&self) -> String {
            format!("ws://{}/test", self.addr)
        }
    }

    #[test]
    fn ws_transport_recv_event_gets_pushed_event() {
        let server = EventPushServer::start("Page.frameNavigated");
        let mut t = WebSocketTransport::connect(&server.url()).unwrap();
        t.set_event_timeout(Duration::from_secs(2));
        let ev = t.recv_event().expect("recv ok").expect("got an event");
        assert_eq!(ev.method, "Page.frameNavigated");
        assert_eq!(ev.params["hello"], "world");
    }

    #[test]
    fn ws_transport_recv_event_returns_none_on_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let _h = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = server_handshake(&mut stream);
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
        let stream = TcpStream::connect(&server.addr).unwrap();
        let mut t = WebSocketTransport::connect_on_stream(stream, "127.0.0.1", "/test").unwrap();
        let result = t
            .send_command("Foo.bar", serde_json::json!({}), None)
            .unwrap();
        assert_eq!(result["echoedMethod"], "Foo.bar");
    }

    // Keep the `Opcode` import reachable so future tests touching the codec
    // type don't trip an unused-import lint when this test set grows.
    #[test]
    fn opcode_import_anchor() {
        assert_eq!(Opcode::Text as u8, 0x1);
    }
}
