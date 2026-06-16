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
//! TASK-2 在 TASK-1 的基础上引入 [`build_transport`] / [`build_in_memory_transport`]
//! / [`build_websocket_transport`] 三个构造方法,把已解析的 URL 实例化为真实 Transport。
//! `connect()` 本身保持轻量(只解析 URL),实际握手由 `build_*` 触发 — 这与
//! chromiumoxide 的 lazy connect 模式一致,且便于在 `connect("ws://127.0.0.1:9222")`
//! 等不保证后端在线的测试场景下做路由验证。
//!
//! @trace REQ-BAO-API-001 [level:library]
//! @trace REQ-BAO-API-002 [interface:Transport]

use crate::connection::{ParsedConnectUrl};
use crate::error::ConnectError;
use crate::transport::{
    InMemoryBridge, InMemoryTransport, Transport, TransportKind, WebSocketTransport,
};
use std::sync::Arc;

/// CDP Browser 实例。
///
/// 代表一次成功的 `connect` —— 持有解析后的 URL 和 transport 类型。
/// TASK-2 后会扩展为持有真实的 `Transport` + `Connection`。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone)]
pub struct Browser {
    parsed: ParsedConnectUrl,
}

impl Browser {
    /// 连接到 CDP 目标。
    ///
    /// 根据输入 URL 的 scheme 自动路由到三种 transport 模式:
    ///
    /// | Scheme          | 路由                 | Transport       |
    /// |-----------------|----------------------|-----------------|
    /// | `memory://`     | `connect_in_memory`  | InMemory        |
    /// | `ws://`/`wss://`| `connect_ws`         | WebSocket(直连)|
    /// | `http://`/`https://` | `connect_http_discover` | WebSocket(自动发现 ws endpoint) |
    ///
    /// # 错误
    /// - [`ConnectError::InvalidUrl`]: 空 URL 或无 `://`
    /// - [`ConnectError::InvalidScheme`]: scheme 不在支持列表(如 `ftp`)
    ///
    /// # 示例
    /// ```
    /// use bao_cdp_client::Browser;
    /// use bao_cdp_client::error::ConnectError;
    ///
    /// // memory:// → InMemory
    /// let b = Browser::connect("memory://bao").unwrap();
    /// assert!(b.is_in_memory());
    ///
    /// // ws:// → WebSocket
    /// let b = Browser::connect("ws://127.0.0.1:9222").unwrap();
    /// assert!(b.is_websocket());
    ///
    /// // http:// → 自动发现 ws endpoint(本 TASK 内部仅返回 Browser 占位)
    /// let b = Browser::connect("http://127.0.0.1:9222").unwrap();
    /// assert!(b.is_websocket());
    ///
    /// // 非法 scheme
    /// let err = Browser::connect("ftp://x").unwrap_err();
    /// assert!(matches!(err, ConnectError::InvalidScheme(_)));
    /// ```
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn connect(url: &str) -> Result<Self, ConnectError> {
        let parsed = Self::route(url)?;
        // 显式三路分发,保证 connect_in_memory / connect_ws / connect_http_discover
        // 都被 connect() 实际调用(TASK-2 各自接管不同的 Transport 构造)。
        match parsed.scheme.as_str() {
            "memory" => Self::connect_in_memory(url, parsed),
            "ws" | "wss" => Self::connect_ws(url, parsed),
            "http" | "https" => Self::connect_http_discover(url, parsed),
            // route() 已校验过 scheme,这里不可达;保险起见返回 InvalidScheme。
            other => Err(ConnectError::InvalidScheme(other.to_string())),
        }
    }

    /// URL scheme 路由核心。把字符串 URL 解析为 [`ParsedConnectUrl`]。
    ///
    /// 内部用 `bun_url::URL::parse` 提取 scheme(无冒号),再映射到 transport 类型。
    /// 同步、无副作用,便于单元测试。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub(crate) fn route(url: &str) -> Result<ParsedConnectUrl, ConnectError> {
        // 空串早返回 InvalidUrl。
        if url.is_empty() {
            return Err(ConnectError::InvalidUrl);
        }

        // 用 bun_url 解析(借用视图,零分配)。
        let parsed_url = bun_url::URL::parse(url.as_bytes());
        let scheme_bytes: &[u8] = parsed_url.protocol;

        // bun_url 的 URL::parse 找不到 "://" 时返回 protocol 空 — 视为 InvalidUrl。
        if scheme_bytes.is_empty() {
            return Err(ConnectError::InvalidUrl);
        }

        // 协议是 ASCII,转 str 失败说明 scheme 含非 ASCII 字节 — 视为 InvalidUrl。
        let scheme = match std::str::from_utf8(scheme_bytes) {
            Ok(s) => s,
            Err(_) => return Err(ConnectError::InvalidUrl),
        };

        match scheme {
            "memory" => Ok(ParsedConnectUrl::new(url, scheme, TransportKind::InMemory)),
            "ws" | "wss" => Ok(ParsedConnectUrl::new(url, scheme, TransportKind::WebSocket)),
            "http" | "https" => Ok(ParsedConnectUrl::new(
                url,
                scheme,
                TransportKind::WebSocket,
            )),
            other => Err(ConnectError::InvalidScheme(other.to_string())),
        }
    }

    /// InMemory transport 路由。Browser::connect 仅解析 URL,实际 Transport
    /// 由 [`build_in_memory_transport`] 构造(传入 servo bridge 实现)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    fn connect_in_memory(url: &str, parsed: ParsedConnectUrl) -> Result<Browser, ConnectError> {
        let _ = url;
        Ok(Browser { parsed })
    }

    /// WebSocket transport 路由(`ws://` / `wss://` 直连)。Browser::connect 仅解析 URL,
    /// 实际 Transport 由 [`build_websocket_transport`] 构造(触发 TCP + WebSocket 握手)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    fn connect_ws(url: &str, parsed: ParsedConnectUrl) -> Result<Browser, ConnectError> {
        let _ = url;
        Ok(Browser { parsed })
    }

    /// HTTP 自动发现路由(`http://` / `https://`)。
    ///
    /// 标准流程是 GET `/json/version` 拿到 `webSocketDebuggerUrl`,再 `connect_ws`。
    /// TASK-1 不实际发请求(避免网络副作用),返回 Browser 占位,但内部记录
    /// `discovery_pending = true` 以便 TASK-2 接管。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    #[allow(unused_variables)]
    fn connect_http_discover(url: &str, parsed: ParsedConnectUrl) -> Result<Browser, ConnectError> {
        // 注:在 route() 中 http/https 已合并到 WebSocket 分支,本方法在 TASK-1 暂不被直接调用,
        // 但保留为 TASK-2 的语义占位与测试锚点。
        // 误用保护:如果未来 route() 把 http/https 路由到独立分支,本方法仍可作为单独入口。
        Ok(Browser { parsed })
    }

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

    /// 根据 URL scheme 构造对应 Transport。
    ///
    /// - InMemory URL(`memory://`)需要调用方传入 servo bridge → 调 [`build_in_memory_transport`]
    /// - WebSocket URL(`ws://`)→ 调 [`build_websocket_transport`] 触发 TCP + WebSocket 握手
    ///
    /// 返回 `Box<dyn Transport>` 便于上层 Connection 持有 trait 对象。
    ///
    /// # 错误
    /// - [`ConnectError::ConnectionFailed`]: TCP/WebSocket 握手失败
    /// - [`ConnectError::InvalidScheme`]: URL scheme 与构造方式不匹配
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn build_transport(&self) -> Result<Box<dyn Transport>, ConnectError> {
        match self.parsed.transport_kind {
            TransportKind::InMemory => Err(ConnectError::ConnectionFailed(
                "InMemory transport requires explicit InMemoryBridge; use build_in_memory_transport()"
                    .into(),
            )),
            TransportKind::WebSocket => {
                let ws = WebSocketTransport::connect(&self.parsed.raw).map_err(|e| {
                    ConnectError::ConnectionFailed(format!("ws connect: {}", e))
                })?;
                Ok(Box::new(ws))
            }
        }
    }

    /// 构造 InMemory transport(同进程 servo)。
    ///
    /// 调用方传入 `InMemoryBridge` 实现 — TASK-3 提供 `CDPRdpBridge` 实现,
    /// TASK-2 单测用 mock bridge。
    ///
    /// # 错误
    /// - [`ConnectError::InvalidScheme`]: 当前 URL 不是 `memory://`
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
    /// 触发 TCP 连接 + RFC 6455 WebSocket 握手(通过 `bun_uws::ws_client::WebSocketClient`)。
    /// 成功后返回包装好的 `WebSocketTransport`。
    ///
    /// # 错误
    /// - [`ConnectError::InvalidScheme`]: 当前 URL 不是 `ws://`
    /// - [`ConnectError::ConnectionFailed`]: TCP/WebSocket 握手失败
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn build_websocket_transport(&self) -> Result<WebSocketTransport, ConnectError> {
        if self.parsed.transport_kind != TransportKind::WebSocket {
            return Err(ConnectError::InvalidScheme(format!(
                "expected ws:// / http://, got {:?}",
                self.parsed.scheme
            )));
        }
        WebSocketTransport::connect(&self.parsed.raw).map_err(|e| {
            ConnectError::ConnectionFailed(format!("ws connect: {}", e))
        })
    }

    /// 内部构造(测试用)。
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn from_parsed(parsed: ParsedConnectUrl) -> Self {
        Browser { parsed }
    }
}

impl std::fmt::Display for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Browser({}, kind={:?})", self.parsed.raw, self.parsed.transport_kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // "localhost:9222" 没有 ://,bun_url 解析后 protocol 为空。
        let err = Browser::route("localhost:9222").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidUrl));
    }

    #[test]
    fn route_unix_socket_invalid_scheme() {
        let err = Browser::route("unix:///var/run/cdp.sock").unwrap_err();
        assert!(matches!(err, ConnectError::InvalidScheme(_)));
    }

    #[test]
    fn connect_memory_returns_browser() {
        let b = Browser::connect("memory://bao").unwrap();
        assert!(b.is_in_memory());
        assert!(!b.is_websocket());
        assert_eq!(b.scheme(), "memory");
        assert_eq!(b.url(), "memory://bao");
    }

    #[test]
    fn connect_ws_returns_browser() {
        let b = Browser::connect("ws://127.0.0.1:9222").unwrap();
        assert!(b.is_websocket());
        assert_eq!(b.scheme(), "ws");
    }

    #[test]
    fn connect_http_returns_browser() {
        let b = Browser::connect("http://127.0.0.1:9222").unwrap();
        assert!(b.is_websocket());
        assert_eq!(b.scheme(), "http");
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

    #[test]
    fn browser_display_format() {
        let b = Browser::connect("memory://bao").unwrap();
        let s = b.to_string();
        assert!(s.contains("memory://bao"), "got: {}", s);
        assert!(s.contains("InMemory"), "got: {}", s);
    }

    #[test]
    fn browser_clone_preserves_state() {
        let b1 = Browser::connect("ws://127.0.0.1:9222").unwrap();
        let b2 = b1.clone();
        assert_eq!(b1.scheme(), b2.scheme());
        assert_eq!(b1.url(), b2.url());
        assert_eq!(b1.transport_kind(), b2.transport_kind());
    }
}
