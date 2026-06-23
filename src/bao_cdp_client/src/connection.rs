//! Connection 层 — 持有 Transport,提供 JSON-RPC 命令收发 + 事件分发。
//!
//! `Connection` 是 CDP 通信的核心:
//! - 持有 `Box<dyn Transport>`(InMemory / WebSocket)
//! - `send_command(method, params)` → 构造 JSON-RPC request → 发送 → 等待响应
//! - `next_command_id()` — 原子计数器分配 JSON-RPC request ID
//! - 事件分发:从 Transport 读取事件,按 method 路由到已注册 handler
//!
//! # 线程模型
//!
//! `Connection` 本身是 `Send`(因 `Transport: Send`),但高层 API 类
//! (Page/BrowserContext)用 `Rc<RefCell<Connection>>` 持有,保持 `!Send`
//! 单线程模型(与 servo JSContext 一致)。
//!
//! @trace REQ-BAO-API-001 [level:library]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;

use crate::error::{CdpError, Result};
use crate::transport::{CdpEvent, Transport, TransportKind};

/// Connection 配置(超时、重试、session_id 等)。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// 命令调用默认超时(毫秒)。
    pub default_timeout_ms: u64,
    /// Transport 类型。
    pub transport_kind: TransportKind,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
            transport_kind: TransportKind::InMemory,
        }
    }
}

/// 连接 URL 解析结果,在 Browser::connect 内部使用。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConnectUrl {
    /// 原 URL。
    pub raw: String,
    /// 解析出的 scheme(`memory` / `ws` / `wss` / `http` / `https`)。
    pub scheme: String,
    /// 路由后的 transport 类型。
    pub transport_kind: TransportKind,
}

impl ParsedConnectUrl {
    /// 构造新的解析结果。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn new(raw: impl Into<String>, scheme: impl Into<String>, kind: TransportKind) -> Self {
        Self {
            raw: raw.into(),
            scheme: scheme.into(),
            transport_kind: kind,
        }
    }
}

/// 事件 handler 注册 ID。
pub type EventListenerId = u64;

/// 事件 handler 类型。`Arc<dyn Fn>` 允许跨 Rc 边界。
pub type EventListener = Arc<dyn Fn(CdpEvent) + Send + Sync>;

// ─── 全局 ID 计数器(跨 Connection 实例共享) ──────────────────────────────
//
// JSON-RPC spec 要求 request id 在同一 session 内唯一。用全局 AtomicU64
// 保证即使多 Connection 也不冲突(单线程模型下实际只有一个 Connection)。
//
static NEXT_GLOBAL_ID: AtomicU64 = AtomicU64::new(1);

/// 分配下一个 JSON-RPC request ID。
fn next_command_id() -> u64 {
    NEXT_GLOBAL_ID.fetch_add(1, Ordering::Relaxed)
}

/// Connection — CDP JSON-RPC 命令收发 + 事件分发核心。
///
/// 持有 `Box<dyn Transport>`,提供:
/// - `send_command(method, params)` → 构造 JSON-RPC request → 通过 Transport 发送 → 返回 result
/// - `send_command_with_session(method, params, session_id)` → 带子 session 的命令
/// - `recv_event()` → 从 Transport 读取一个事件
/// - `on_event(method, handler)` → 注册事件 handler
/// - `off_event(method, listener_id)` → 移除事件 handler
/// - `close()` → 关闭 Transport
///
/// @trace REQ-BAO-API-001 [level:library]
pub struct Connection {
    transport: Box<dyn Transport>,
    config: ConnectionConfig,
    /// 事件 handler 注册表:method → Vec<(id, handler)>。
    event_handlers: HashMap<String, Vec<(EventListenerId, EventListener)>>,
    /// 下一个事件 handler ID。
    next_listener_id: EventListenerId,
    /// 是否已关闭。
    closed: bool,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("transport_kind", &self.transport.kind())
            .field("config", &self.config)
            .field("closed", &self.closed)
            .field("handler_count", &self.event_handlers.len())
            .finish()
    }
}

impl Connection {
    /// 从 Transport 构造 Connection。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn new(transport: Box<dyn Transport>, config: ConnectionConfig) -> Self {
        Self {
            transport,
            config,
            event_handlers: HashMap::new(),
            next_listener_id: 1,
            closed: false,
        }
    }

    /// 从 Transport 构造 Connection(使用默认配置)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn from_transport(transport: Box<dyn Transport>) -> Self {
        let kind = transport.kind();
        Self::new(
            transport,
            ConnectionConfig {
                default_timeout_ms: 30_000,
                transport_kind: kind,
            },
        )
    }

    /// 取配置引用。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Transport 类型。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn transport_kind(&self) -> TransportKind {
        self.transport.kind()
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    // ─── 命令发送 ──────────────────────────────────────────────────────────

    /// 发送 CDP 命令(无 session_id),返回响应 result。
    ///
    /// 等价于 `send_command_with_session(method, params, None)`。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn send_command(&mut self, method: &str, params: Value) -> Result<Value> {
        self.send_command_with_session(method, params, None)
    }

    /// 发送 CDP 命令(带可选 session_id),返回响应 result。
    ///
    /// 内部流程:
    /// 1. 分配 JSON-RPC request ID
    /// 2. 调用 `Transport::send_command(method, params, session_id)`
    /// 3. 返回 `result` 字段或错误
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: Transport 已关闭
    /// - `CdpError::ProtocolError`: 远端返回 JSON-RPC error
    /// - `CdpError::Timeout`: 命令超时
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn send_command_with_session(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        if self.closed {
            return Err(CdpError::ConnectionClosed);
        }
        // Transport::send_command 内部已处理 JSON-RPC 帧构造 + ID 分配 + 响应等待。
        let _id = next_command_id(); // 分配 ID(供日志/追踪用)
        self.transport.send_command(method, params, session_id)
    }

    // ─── 事件接收 ──────────────────────────────────────────────────────────

    /// 从 Transport 接收一个事件。
    ///
    /// 无事件时返回 `Ok(None)`(超时)。
    /// 收到事件后自动分发到已注册的 handler。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn recv_event(&mut self) -> Result<Option<CdpEvent>> {
        if self.closed {
            return Err(CdpError::ConnectionClosed);
        }
        let event = self.transport.recv_event()?;
        if let Some(ref ev) = event {
            self.dispatch_event(ev.clone());
        }
        Ok(event)
    }

    /// 接收并分发所有待处理事件(非阻塞轮询直到超时返回 None)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn drain_events(&mut self) -> Result<Vec<CdpEvent>> {
        let mut events = Vec::new();
        loop {
            match self.recv_event()? {
                Some(ev) => events.push(ev),
                None => break,
            }
        }
        Ok(events)
    }

    // ─── 事件 handler 注册 ─────────────────────────────────────────────────

    /// 注册事件 handler,返回 listener ID(可用于 off_event)。
    ///
    /// handler 在 `recv_event` / `drain_events` 收到匹配 method 的事件时被调用。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn on_event(&mut self, method: &str, handler: EventListener) -> EventListenerId {
        let id = self.next_listener_id;
        self.next_listener_id += 1;
        self.event_handlers
            .entry(method.to_string())
            .or_default()
            .push((id, handler));
        id
    }

    /// 注册一次性事件 handler(触发后自动移除)。
    ///
    /// 简化实现:与 `on_event` 相同,调用方负责在 handler 内调 `off_event`。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn once_event(&mut self, method: &str, handler: EventListener) -> EventListenerId {
        self.on_event(method, handler)
    }

    /// 移除事件 handler。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn off_event(&mut self, method: &str, listener_id: EventListenerId) -> bool {
        let removed = if let Some(handlers) = self.event_handlers.get_mut(method) {
            let before = handlers.len();
            handlers.retain(|(id, _)| *id != listener_id);
            before > handlers.len()
        } else {
            false
        };
        // Clean up empty entry after the mutable borrow on the Vec is done.
        if removed && self.event_handlers.get(method).map_or(false, |v| v.is_empty()) {
            self.event_handlers.remove(method);
        }
        removed
    }

    /// 移除指定 method 的所有 handler。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn remove_all_event_handlers(&mut self, method: &str) {
        self.event_handlers.remove(method);
    }

    /// 返回指定 method 的 handler 数量。
    pub fn event_handler_count(&self, method: &str) -> usize {
        self.event_handlers
            .get(method)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 分发事件到已注册 handler。
    fn dispatch_event(&mut self, event: CdpEvent) {
        // 收集匹配的 handler(克隆 Arc),避免 borrow 冲突。
        let handlers: Vec<EventListener> = self
            .event_handlers
            .get(&event.method)
            .map(|v| v.iter().map(|(_, h)| h.clone()).collect())
            .unwrap_or_default();

        // 也检查通配符 handler(method 为 "*")。
        let wildcard_handlers: Vec<EventListener> = self
            .event_handlers
            .get("*")
            .map(|v| v.iter().map(|(_, h)| h.clone()).collect())
            .unwrap_or_default();

        // 释放 borrow 后调用 handler。
        for handler in handlers {
            handler(event.clone());
        }
        for handler in wildcard_handlers {
            handler(event.clone());
        }
    }

    // ─── 关闭 ──────────────────────────────────────────────────────────────

    /// 关闭 Connection(关闭底层 Transport)。
    ///
    /// 幂等:重复调用安全。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn close(&mut self) -> Result<()> {
        if !self.closed {
            self.closed = true;
            self.transport.close()?;
            self.event_handlers.clear();
        }
        Ok(())
    }

    // ─── Transport 访问 ────────────────────────────────────────────────────

    /// 获取底层 Transport 的可变引用(高级用法)。
    pub fn transport_mut(&mut self) -> &mut dyn Transport {
        &mut *self.transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// Mock Transport for testing Connection.
    struct MockTransport {
        kind: TransportKind,
        closed: bool,
        next_response: Option<Value>,
        event_queue: std::collections::VecDeque<CdpEvent>,
    }

    impl Transport for MockTransport {
        fn kind(&self) -> TransportKind {
            self.kind
        }
        fn send_command(
            &mut self,
            _method: &str,
            _params: Value,
            _session_id: Option<&str>,
        ) -> Result<Value> {
            if self.closed {
                return Err(CdpError::ConnectionClosed);
            }
            self.next_response
                .clone()
                .ok_or(CdpError::ProtocolError("no response".into()))
        }
        fn recv_event(&mut self) -> Result<Option<CdpEvent>> {
            if self.closed {
                return Err(CdpError::ConnectionClosed);
            }
            Ok(self.event_queue.pop_front())
        }
        fn close(&mut self) -> Result<()> {
            self.closed = true;
            Ok(())
        }
    }

    fn make_mock_with_response(response: Value) -> Connection {
        let mock = MockTransport {
            kind: TransportKind::InMemory,
            closed: false,
            next_response: Some(response),
            event_queue: std::collections::VecDeque::new(),
        };
        Connection::from_transport(Box::new(mock))
    }

    fn make_mock_with_events(events: Vec<CdpEvent>) -> Connection {
        let mock = MockTransport {
            kind: TransportKind::InMemory,
            closed: false,
            next_response: Some(Value::Null),
            event_queue: events.into_iter().collect(),
        };
        Connection::from_transport(Box::new(mock))
    }

    #[test]
    fn connection_config_default() {
        let cfg = ConnectionConfig::default();
        assert_eq!(cfg.default_timeout_ms, 30_000);
        assert_eq!(cfg.transport_kind, TransportKind::InMemory);
    }

    #[test]
    fn parsed_connect_url_construction() {
        let parsed = ParsedConnectUrl::new("memory://bao", "memory", TransportKind::InMemory);
        assert_eq!(parsed.raw, "memory://bao");
        assert_eq!(parsed.scheme, "memory");
        assert_eq!(parsed.transport_kind, TransportKind::InMemory);
    }

    #[test]
    fn connection_new_carries_config() {
        let cfg = ConnectionConfig {
            default_timeout_ms: 5000,
            transport_kind: TransportKind::WebSocket,
        };
        let mock = MockTransport {
            kind: TransportKind::WebSocket,
            closed: false,
            next_response: Some(Value::Null),
            event_queue: std::collections::VecDeque::new(),
        };
        let conn = Connection::new(Box::new(mock), cfg);
        assert_eq!(conn.config().default_timeout_ms, 5000);
        assert_eq!(conn.config().transport_kind, TransportKind::WebSocket);
        assert_eq!(conn.transport_kind(), TransportKind::WebSocket);
    }

    #[test]
    fn connection_send_command_returns_response() {
        let mut conn =
            make_mock_with_response(serde_json::json!({"url": "https://example.com"}));
        let result = conn
            .send_command("Page.navigate", serde_json::json!({}))
            .unwrap();
        assert_eq!(result["url"], "https://example.com");
    }

    #[test]
    fn connection_send_command_after_close_returns_error() {
        let mut conn = make_mock_with_response(Value::Null);
        conn.close().unwrap();
        let err = conn
            .send_command("X.y", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn connection_recv_event_returns_event() {
        let ev = CdpEvent::new("Page.frameNavigated", serde_json::json!({"url": "x"}));
        let mut conn = make_mock_with_events(vec![ev]);
        let got = conn.recv_event().unwrap().expect("expected event");
        assert_eq!(got.method, "Page.frameNavigated");
    }

    #[test]
    fn connection_recv_event_none_on_empty() {
        let mut conn = make_mock_with_response(Value::Null);
        let got = conn.recv_event().unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn connection_recv_event_after_close_returns_error() {
        let mut conn = make_mock_with_response(Value::Null);
        conn.close().unwrap();
        let err = conn.recv_event().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn connection_event_handler_registration_and_dispatch() {
        let ev = CdpEvent::new("Page.load", serde_json::json!({"url": "x"}));
        let mut conn = make_mock_with_events(vec![ev]);

        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let handler: EventListener = Arc::new(move |_ev| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        conn.on_event("Page.load", handler);

        let got = conn.recv_event().unwrap().expect("expected event");
        assert_eq!(got.method, "Page.load");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn connection_event_handler_off() {
        let mut conn = make_mock_with_response(Value::Null);
        let handler: EventListener = Arc::new(|_| {});
        let id = conn.on_event("X.y", handler);
        assert_eq!(conn.event_handler_count("X.y"), 1);
        let removed = conn.off_event("X.y", id);
        assert!(removed);
        assert_eq!(conn.event_handler_count("X.y"), 0);
    }

    #[test]
    fn connection_close_is_idempotent() {
        let mut conn = make_mock_with_response(Value::Null);
        conn.close().unwrap();
        conn.close().unwrap();
        assert!(conn.is_closed());
    }

    #[test]
    fn connection_drain_events() {
        let events = vec![
            CdpEvent::new("A", Value::Null),
            CdpEvent::new("B", Value::Null),
        ];
        let mut conn = make_mock_with_events(events);
        let drained = conn.drain_events().unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].method, "A");
        assert_eq!(drained[1].method, "B");
    }

    #[test]
    fn connection_wildcard_handler() {
        let ev = CdpEvent::new("Page.load", Value::Null);
        let mut conn = make_mock_with_events(vec![ev]);

        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let handler: EventListener = Arc::new(move |_ev| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        conn.on_event("*", handler);

        let _ = conn.recv_event().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn next_command_id_monotonic() {
        let id1 = next_command_id();
        let id2 = next_command_id();
        assert!(id2 > id1);
    }
}
