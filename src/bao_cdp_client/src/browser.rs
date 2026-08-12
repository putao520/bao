//! Browser 入口 — `Browser::connect(url)` 通过 URL scheme 路由到三种 transport。
//!
//! 路由规则(DEC-URL-001):
//! - `memory://...` → `connect_in_memory` → InMemoryTransport
//! - `ws://...` / `wss://...` → `connect_ws` → WebSocketTransport(直连)
//! - `http://...` / `https://...` → `connect_http_discover` → 先 GET `/json/version` 发现 ws endpoint 再连
//!
//! 其他 scheme 一律返回 [`ConnectError::InvalidScheme`]。空串或无 `://` 返回
//! [`ConnectError::InvalidUrl`]。
//!
//! `Browser` 持有 `Connection`(封装 `Box<dyn Transport>`),提供:
//! - `connect(url)` → 解析 URL → 构建 Transport → 创建 Connection → 返回 Browser
//! - `new_page()` → 发送 `Target.createTarget` CDP 命令
//! - `pages()` → 发送 `Target.getTargets` CDP 命令
//! - `version()` → 发送 `Browser.getVersion` CDP 命令
//!
//! @trace REQ-BAO-API-001 [level:library]
//! @trace REQ-BAO-API-002 [interface:Transport]

use crate::connection::{Connection, ConnectionConfig, ParsedConnectUrl};
use crate::error::ConnectError;
use crate::transport::{
    InMemoryBridge, InMemoryTransport, Transport, TransportKind, WebSocketTransport,
};
use std::sync::Arc;

/// CDP Browser 实例。
///
/// 代表一次成功的 `connect` —— 持有解析后的 URL、Connection 和 transport 类型。
///
/// @trace REQ-BAO-API-001 [level:library]
pub struct Browser {
    parsed: ParsedConnectUrl,
    connection: Option<Connection>,
}

impl Browser {
    /// 连接到 CDP 目标(解析 URL,按需创建 Connection)。
    ///
    /// 根据输入 URL 的 scheme 自动路由到三种 transport 模式:
    ///
    /// | Scheme          | 行为                                |
    /// |-----------------|--------------------------------------|
    /// | `memory://`     | 解析 URL,返回 Browser(无 Connection,需 `connect_with_bridge` 接入) |
    /// | `ws://`/`wss://`| 解析 URL,返回 Browser(无 Connection,需 `connect_with_discovered_ws` 接入) |
    /// | `http://`/`https://` | 解析 URL,返回 Browser(无 Connection,同 ws://) |
    ///
    /// 这是 lazy connect 模式:不触发网络 I/O,不阻塞,瞬时返回。
    /// 实际的 Transport 建立通过 `connect_with_bridge`(memory://)
    /// 或 `connect_with_discovered_ws`(ws://) 完成。
    ///
    /// # 错误
    /// - [`ConnectError::InvalidUrl`]: 空 URL 或无 `://`
    /// - [`ConnectError::InvalidScheme`]: scheme 不在支持列表(如 `ftp`)
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn connect(url: &str) -> Result<Self, ConnectError> {
        let parsed = Self::route(url)?;
        // Lazy connect:只解析 URL,不创建 Transport/Connection。
        // 实际连接通过 connect_with_bridge / connect_with_discovered_ws 完成。
        Ok(Browser {
            parsed,
            connection: None,
        })
    }

    /// 连接到 CDP 目标并传入 InMemoryBridge(memory:// 模式)。
    ///
    /// 与 `connect` 类似,但立即为 memory:// 创建 Connection。
    /// 其他 scheme 仍返回 Browser(无 Connection)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn connect_with_bridge(
        url: &str,
        bridge: Arc<dyn InMemoryBridge>,
    ) -> Result<Self, ConnectError> {
        let parsed = Self::route(url)?;
        if parsed.scheme == "memory" {
            let transport = InMemoryTransport::new(bridge);
            let config = ConnectionConfig {
                default_timeout_ms: 30_000,
                transport_kind: TransportKind::InMemory,
            };
            let connection = Connection::new(Box::new(transport), config);
            Ok(Browser {
                parsed,
                connection: Some(connection),
            })
        } else {
            // 非 memory:// scheme:仅解析 URL,不创建 Connection
            Ok(Browser {
                parsed,
                connection: None,
            })
        }
    }

    /// URL scheme 路由核心。把字符串 URL 解析为 [`ParsedConnectUrl`]。
    ///
    /// 内部用 `bun_url::URL::parse` 提取 scheme(无冒号),再映射到 transport 类型。
    /// 同步、无副作用,便于单元测试。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub(crate) fn route(url: &str) -> Result<ParsedConnectUrl, ConnectError> {
        if url.is_empty() {
            return Err(ConnectError::InvalidUrl);
        }

        let parsed_url = bun_url::URL::parse(url.as_bytes());
        let scheme_bytes: &[u8] = parsed_url.protocol;

        if scheme_bytes.is_empty() {
            return Err(ConnectError::InvalidUrl);
        }

        let scheme = match std::str::from_utf8(scheme_bytes) {
            Ok(s) => s,
            Err(_) => return Err(ConnectError::InvalidUrl),
        };

        match scheme {
            "memory" => Ok(ParsedConnectUrl::new(url, scheme, TransportKind::InMemory)),
            "ws" | "wss" => Ok(ParsedConnectUrl::new(url, scheme, TransportKind::WebSocket)),
            "http" | "https" => Ok(ParsedConnectUrl::new(url, scheme, TransportKind::WebSocket)),
            other => Err(ConnectError::InvalidScheme(other.to_string())),
        }
    }

    /// HTTP 发现完成后,用发现的 ws URL 创建 Connection。
    ///
    /// 触发实际的 WebSocket 连接(会阻塞直到 TCP+WS 握手完成或超时)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn connect_with_discovered_ws(ws_url: &str) -> Result<Self, ConnectError> {
        let parsed = Self::route(ws_url)?;
        let ws = WebSocketTransport::connect(&parsed.raw)
            .map_err(|e| ConnectError::ConnectionFailed(format!("ws connect: {}", e)))?;
        let config = ConnectionConfig {
            default_timeout_ms: 30_000,
            transport_kind: TransportKind::WebSocket,
        };
        let connection = Connection::new(Box::new(ws), config);
        Ok(Browser {
            parsed,
            connection: Some(connection),
        })
    }

    // ─── CDP 命令方法 ──────────────────────────────────────────────────────

    /// 发送 CDP 命令(通过 Connection)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn send_command(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::error::Result<serde_json::Value> {
        match &mut self.connection {
            Some(conn) => conn.send_command(method, params),
            None => Err(crate::error::CdpError::ConnectionClosed),
        }
    }

    /// 发送 CDP 命令(带 session_id)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn send_command_with_session(
        &mut self,
        method: &str,
        params: serde_json::Value,
        session_id: &str,
    ) -> crate::error::Result<serde_json::Value> {
        match &mut self.connection {
            Some(conn) => conn.send_command_with_session(method, params, Some(session_id)),
            None => Err(crate::error::CdpError::ConnectionClosed),
        }
    }

    /// 获取 Browser 版本信息(CDP `Browser.getVersion`)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn version(&mut self) -> crate::error::Result<serde_json::Value> {
        self.send_command("Browser.getVersion", serde_json::json!({}))
    }

    /// 获取所有 target 列表(CDP `Target.getTargets`)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn pages(&mut self) -> crate::error::Result<serde_json::Value> {
        self.send_command("Target.getTargets", serde_json::json!({}))
    }

    /// 创建新 page/tab(CDP `Target.createTarget`)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn new_page(&mut self, url: &str) -> crate::error::Result<serde_json::Value> {
        self.send_command("Target.createTarget", serde_json::json!({"url": url}))
    }

    /// 接收一个 CDP 事件。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn recv_event(&mut self) -> crate::error::Result<Option<crate::transport::CdpEvent>> {
        match &mut self.connection {
            Some(conn) => conn.recv_event(),
            None => Err(crate::error::CdpError::ConnectionClosed),
        }
    }

    /// 注册事件 handler。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn on_event(
        &mut self,
        method: &str,
        handler: crate::connection::EventListener,
    ) -> crate::connection::EventListenerId {
        match &mut self.connection {
            Some(conn) => conn.on_event(method, handler),
            None => 0,
        }
    }

    /// 移除事件 handler。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn off_event(
        &mut self,
        method: &str,
        listener_id: crate::connection::EventListenerId,
    ) -> bool {
        match &mut self.connection {
            Some(conn) => conn.off_event(method, listener_id),
            None => false,
        }
    }

    /// 关闭 Connection。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn close_connection(&mut self) -> crate::error::Result<()> {
        match &mut self.connection {
            Some(conn) => conn.close(),
            None => Ok(()),
        }
    }

    // ─── 属性访问 ──────────────────────────────────────────────────────────

    /// 原 URL。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn url(&self) -> &str {
        &self.parsed.raw
    }

    /// 已解析的 scheme(`memory` / `ws` / `wss` / `http` / `https`)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn scheme(&self) -> &str {
        &self.parsed.scheme
    }

    /// Transport 类型。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn transport_kind(&self) -> TransportKind {
        self.parsed.transport_kind
    }

    /// 是否 InMemory 连接。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn is_in_memory(&self) -> bool {
        self.parsed.transport_kind == TransportKind::InMemory
    }

    /// 是否 WebSocket 连接(包括 ws/wss/http/https — 后者会先 discover)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn is_websocket(&self) -> bool {
        self.parsed.transport_kind == TransportKind::WebSocket
    }

    /// 是否有 Connection(即 transport 已建立)。
    pub fn has_connection(&self) -> bool {
        self.connection.is_some()
    }

    /// 获取 Connection 的可变引用。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn connection_mut(&mut self) -> Option<&mut Connection> {
        self.connection.as_mut()
    }

    /// 根据 URL scheme 构造对应 Transport。
    ///
    /// - InMemory URL(`memory://`)需要调用方传入 servo bridge → 调 [`build_in_memory_transport`]
    /// - WebSocket URL(`ws://`)→ 调 [`build_websocket_transport`] 触发 TCP + WebSocket 握手
    ///
    /// 返回 `Box<dyn Transport>` 便于上层 Connection 持有 trait 对象。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn build_transport(&self) -> Result<Box<dyn Transport>, ConnectError> {
        match self.parsed.transport_kind {
            TransportKind::InMemory => Err(ConnectError::ConnectionFailed(
                "InMemory transport requires explicit InMemoryBridge; use connect_with_bridge()"
                    .into(),
            )),
            TransportKind::WebSocket => {
                let ws = WebSocketTransport::connect(&self.parsed.raw)
                    .map_err(|e| ConnectError::ConnectionFailed(format!("ws connect: {}", e)))?;
                Ok(Box::new(ws))
            }
        }
    }

    /// 构造 InMemory transport(同进程 servo)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn build_in_memory_transport(
        &self,
        bridge: Arc<dyn InMemoryBridge>,
    ) -> Result<InMemoryTransport, ConnectError> {
        if self.parsed.transport_kind != TransportKind::InMemory {
            return Err(ConnectError::InvalidScheme(format!(
                "expected memory://, got {:?}",
                self.parsed.scheme
            )));
        }
        Ok(InMemoryTransport::new(bridge))
    }

    /// 构造 WebSocket transport(外部 Chrome)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn build_websocket_transport(&self) -> Result<WebSocketTransport, ConnectError> {
        if self.parsed.transport_kind != TransportKind::WebSocket {
            return Err(ConnectError::InvalidScheme(format!(
                "expected ws:// / http://, got {:?}",
                self.parsed.scheme
            )));
        }
        WebSocketTransport::connect(&self.parsed.raw)
            .map_err(|e| ConnectError::ConnectionFailed(format!("ws connect: {}", e)))
    }
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("url", &self.parsed.raw)
            .field("scheme", &self.parsed.scheme)
            .field("transport_kind", &self.parsed.transport_kind)
            .field("connected", &self.connection.is_some())
            .finish()
    }
}

impl std::fmt::Display for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Browser({}, kind={:?}, connected={})",
            self.parsed.raw,
            self.parsed.transport_kind,
            self.connection.is_some()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CdpError;
    use crate::transport::CdpEvent;

    // ─── Mock Transport ────────────────────────────────────────────────────
    struct MockTransport {
        kind: TransportKind,
        closed: bool,
        next_response: Option<serde_json::Value>,
    }

    impl Transport for MockTransport {
        fn kind(&self) -> TransportKind {
            self.kind
        }
        fn send_command(
            &mut self,
            _m: &str,
            _p: serde_json::Value,
            _s: Option<&str>,
        ) -> crate::error::Result<serde_json::Value> {
            if self.closed {
                return Err(CdpError::ConnectionClosed);
            }
            self.next_response
                .clone()
                .ok_or(CdpError::ProtocolError("no response".into()))
        }
        fn recv_event(&mut self) -> crate::error::Result<Option<CdpEvent>> {
            if self.closed {
                return Err(CdpError::ConnectionClosed);
            }
            Ok(None)
        }
        fn close(&mut self) -> crate::error::Result<()> {
            self.closed = true;
            Ok(())
        }
    }

    // ─── URL 路由测试 ──────────────────────────────────────────────────────

    #[test]
    fn route_memory() {
        let p = Browser::route("memory://bao").unwrap();
        assert_eq!(p.scheme, "memory");
        assert_eq!(p.transport_kind, TransportKind::InMemory);
        assert_eq!(p.raw, "memory://bao");
    }

    #[test]
    fn route_ws() {
        let p = Browser::route("ws://127.0.0.1:9222").unwrap();
        assert_eq!(p.scheme, "ws");
        assert_eq!(p.transport_kind, TransportKind::WebSocket);
    }

    #[test]
    fn route_wss() {
        let p = Browser::route("wss://example.com:443/devtools").unwrap();
        assert_eq!(p.scheme, "wss");
        assert_eq!(p.transport_kind, TransportKind::WebSocket);
    }

    #[test]
    fn route_http() {
        let p = Browser::route("http://127.0.0.1:9222").unwrap();
        assert_eq!(p.scheme, "http");
        assert_eq!(p.transport_kind, TransportKind::WebSocket);
    }

    #[test]
    fn route_https() {
        let p = Browser::route("https://127.0.0.1:9443").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.transport_kind, TransportKind::WebSocket);
    }

    #[test]
    fn route_ftp_invalid_scheme() {
        let err = Browser::route("ftp://example.com").unwrap_err();
        match err {
            ConnectError::InvalidScheme(s) => assert_eq!(s, "ftp"),
            other => panic!("expected InvalidScheme, got {:?}", other),
        }
    }

    #[test]
    fn route_empty_invalid_url() {
        let err = Browser::route("").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidUrl));
    }

    #[test]
    fn route_no_scheme_invalid_url() {
        let err = Browser::route("localhost:9222").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidUrl));
    }

    #[test]
    fn route_unix_socket_invalid_scheme() {
        let err = Browser::route("unix:///var/run/cdp.sock").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidScheme(_)));
    }

    // ─── connect 测试 ──────────────────────────────────────────────────────

    #[test]
    fn connect_memory_returns_browser_no_connection() {
        let b = Browser::connect("memory://bao").unwrap();
        assert!(b.is_in_memory());
        assert!(!b.is_websocket());
        assert_eq!(b.scheme(), "memory");
        assert_eq!(b.url(), "memory://bao");
        assert!(!b.has_connection());
    }

    #[test]
    fn connect_ws_returns_browser_no_connection() {
        // Lazy connect:ws:// 只解析 URL,不触发实际 WebSocket 连接。
        let b = Browser::connect("ws://127.0.0.1:9222").unwrap();
        assert!(b.is_websocket());
        assert_eq!(b.scheme(), "ws");
        assert!(!b.has_connection());
    }

    #[test]
    fn connect_http_returns_browser_no_connection() {
        let b = Browser::connect("http://127.0.0.1:9222").unwrap();
        assert!(b.is_websocket());
        assert_eq!(b.scheme(), "http");
        assert!(!b.has_connection());
    }

    #[test]
    fn connect_invalid_scheme() {
        let err = Browser::connect("ftp://x").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidScheme(_)));
    }

    #[test]
    fn connect_empty_invalid_url() {
        let err = Browser::connect("").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidUrl));
    }

    // ─── connect_with_bridge 测试 ──────────────────────────────────────────

    /// Mock InMemoryBridge for testing.
    struct MockBridge;

    impl InMemoryBridge for MockBridge {
        fn dispatch_command(
            &self,
            method: &str,
            _params: serde_json::Value,
            _session_id: Option<&str>,
        ) -> crate::transport::InMemoryBridgeResponse {
            match method {
                "Browser.getVersion" => crate::transport::InMemoryBridgeResponse::Ok(
                    serde_json::json!({"product": "HeadlessChrome/120", "userAgent": "Mozilla/5.0"}),
                ),
                "Target.getTargets" => crate::transport::InMemoryBridgeResponse::Ok(
                    serde_json::json!({"targetInfos": []}),
                ),
                "Target.createTarget" => crate::transport::InMemoryBridgeResponse::Ok(
                    serde_json::json!({"targetId": "TARGET-NEW-1"}),
                ),
                _ => crate::transport::InMemoryBridgeResponse::Ok(serde_json::Value::Null),
            }
        }
    }

    #[test]
    fn connect_with_bridge_creates_connection() {
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let b = Browser::connect_with_bridge("memory://bao", bridge).unwrap();
        assert!(b.is_in_memory());
        assert!(b.has_connection());
    }

    #[test]
    fn browser_send_command_with_bridge() {
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let mut b = Browser::connect_with_bridge("memory://bao", bridge).unwrap();
        let result = b.version().unwrap();
        assert_eq!(result["product"], "HeadlessChrome/120");
    }

    #[test]
    fn browser_pages_with_bridge() {
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let mut b = Browser::connect_with_bridge("memory://bao", bridge).unwrap();
        let result = b.pages().unwrap();
        assert!(result["targetInfos"].is_array());
    }

    #[test]
    fn browser_new_page_with_bridge() {
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let mut b = Browser::connect_with_bridge("memory://bao", bridge).unwrap();
        let result = b.new_page("https://example.com").unwrap();
        assert_eq!(result["targetId"], "TARGET-NEW-1");
    }

    #[test]
    fn browser_send_command_without_connection_returns_error() {
        let mut b = Browser::connect("memory://bao").unwrap();
        let err = b.send_command("X.y", serde_json::json!({})).unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    // ─── Display ────────────────────────────────────────────────────────────

    #[test]
    fn browser_display_format() {
        let b = Browser::connect("memory://bao").unwrap();
        let s = b.to_string();
        assert!(s.contains("memory://bao"), "got: {}", s);
        assert!(s.contains("InMemory"), "got: {}", s);
    }

    #[test]
    fn browser_with_bridge_display_shows_connected() {
        let bridge: Arc<dyn InMemoryBridge> = Arc::new(MockBridge);
        let b = Browser::connect_with_bridge("memory://bao", bridge).unwrap();
        let s = b.to_string();
        assert!(s.contains("connected=true"), "got: {}", s);
    }
}
