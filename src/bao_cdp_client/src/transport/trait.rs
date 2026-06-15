//! Transport trait 定义。
//!
//! [`Transport`] 抽象三种操作:
//! - `send_command(method, params, session_id) -> Result<Value>`:发送 CDP
//!   命令,阻塞等待响应(同步语义,内部用 channel/TcpStream 阻塞读)。
//! - `recv_event() -> Result<Option<CdpEvent>>`:接收服务器推送事件。
//!   无事件时返回 `Ok(None)`(根据底层 channel 是否非阻塞 / timeout 语义决定)。
//! - `close() -> Result<()>`:优雅关闭 Transport。
//!
//! # 同步 vs 异步设计说明
//!
//! Plan MD 明确要求"**不引入 tokio**"。`Transport` trait 使用同步方法,
//! 由调用方决定调度策略(`bun_event_loop` 任务 / std::thread)。
//! - InMemoryTransport:`std::sync::mpsc` channel + `recv_timeout`(避免无限阻塞)
//! - WebSocketTransport:`std::net::TcpStream` 设置 `set_read_timeout` 后阻塞读
//!
//! 这种模式与 `bao_cdp::servo_bridge::BridgeSender` 完全一致(DEC-CDP-002)。
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

use crate::error::Result;
use serde_json::Value;
use std::time::Duration;

use super::TransportKind;

/// CDP 服务器推送事件结构。
///
/// 对应 JSON-RPC 通知帧(无 `id` 字段,有 `method` 字段)。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
#[derive(Debug, Clone)]
pub struct CdpEvent {
    /// CDP method 名(如 `Page.frameNavigated`、`Runtime.exceptionThrown`)。
    pub method: String,
    /// 事件参数(JSON object)。
    pub params: Value,
    /// 子会话 ID(Target.attachTarget 后,事件归属子 session)。
    pub session_id: Option<String>,
}

impl CdpEvent {
    /// 构造新事件。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            method: method.into(),
            params,
            session_id: None,
        }
    }

    /// 带子 session ID 构造。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

/// Transport 抽象。
///
/// 实现 [`Transport`] 的具体类型必须保证:
/// - `Send + Sync`(可跨线程持有,虽然 InMemory 模式下底层 channel 是 !Send 友好的)
/// - `send_command` 在 close 后返回 [`crate::error::CdpError::ConnectionClosed`]
/// - `recv_event` 在 close 后返回 [`crate::error::CdpError::ConnectionClosed`]
/// - `close` 是幂等的(重复调用安全)
///
/// @trace REQ-BAO-API-002 [interface:Transport]
pub trait Transport: Send {
    /// 返回 transport 类型标识。
    fn kind(&self) -> TransportKind;

    /// 发送 CDP 命令,阻塞等待响应。
    ///
    /// # 参数
    /// - `method`: CDP method 名(如 `Page.navigate`)
    /// - `params`: JSON 参数对象(序列化后的 JSON-RPC params 字段)
    /// - `session_id`: 子会话 ID(`None` 表示顶层 session)
    ///
    /// # 返回
    /// - `Ok(Value)`: 命令响应的 result 字段(JSON-RPC response.result)
    /// - `Err(CdpError::Timeout)`: 超过默认/配置超时
    /// - `Err(CdpError::ConnectionClosed)`: Transport 已关闭
    /// - `Err(CdpError::ProtocolError)`: 远端返回 JSON-RPC error(包含 -32601 等)
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    fn send_command(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value>;

    /// 接收服务器推送事件,阻塞直到事件到达或超时。
    ///
    /// # 返回
    /// - `Ok(Some(CdpEvent))`: 收到一个事件
    /// - `Ok(None)`: 超时未收到事件(channel 非阻塞 / recv_timeout 返回)
    /// - `Err(CdpError::ConnectionClosed)`: Transport 已关闭
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    fn recv_event(&mut self) -> Result<Option<CdpEvent>>;

    /// 优雅关闭 Transport。
    ///
    /// 关闭后:
    /// - 内部 channel sender 被丢弃
    /// - 底层 TcpStream 发送 WS Close 帧并 shutdown(WebSocketTransport)
    /// - 后续 send_command / recv_event 返回 `ConnectionClosed`
    ///
    /// 幂等:重复调用安全。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    fn close(&mut self) -> Result<()>;

    /// 设置默认命令超时。
    ///
    /// 默认实现记录但不强制(子类可覆写)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    fn set_command_timeout(&mut self, _timeout: Duration) {}

    /// 设置默认事件接收超时。
    ///
    /// 默认实现记录但不强制(子类可覆写)。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    fn set_event_timeout(&mut self, _timeout: Duration) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_event_new_basic() {
        let e = CdpEvent::new("Page.frameNavigated", serde_json::json!({"url": "about:blank"}));
        assert_eq!(e.method, "Page.frameNavigated");
        assert_eq!(e.params["url"], "about:blank");
        assert!(e.session_id.is_none());
    }

    #[test]
    fn cdp_event_with_session() {
        let e = CdpEvent::new("Network.requestWillBeSent", serde_json::json!({}))
            .with_session("TARGET-1");
        assert_eq!(e.session_id.as_deref(), Some("TARGET-1"));
    }

    #[test]
    fn cdp_event_clone_preserves_fields() {
        let e1 = CdpEvent::new("Log.entryAdded", serde_json::json!({"text": "hi"}));
        let e2 = e1.clone();
        assert_eq!(e1.method, e2.method);
        assert_eq!(e1.params, e2.params);
        assert_eq!(e1.session_id, e2.session_id);
    }

    /// Stub Transport for trait-object smoke test. Real implementations live
    /// in `in_memory.rs` / `ws.rs`. The stub here validates that `Transport`
    /// can be used as a trait object.
    struct StubTransport {
        kind: TransportKind,
        closed: bool,
        next_event: Option<CdpEvent>,
    }

    impl Transport for StubTransport {
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
                return Err(crate::error::CdpError::ConnectionClosed);
            }
            Ok(Value::Null)
        }
        fn recv_event(&mut self) -> Result<Option<CdpEvent>> {
            if self.closed {
                return Err(crate::error::CdpError::ConnectionClosed);
            }
            Ok(self.next_event.take())
        }
        fn close(&mut self) -> Result<()> {
            self.closed = true;
            Ok(())
        }
    }

    #[test]
    fn transport_trait_object_works() {
        let mut t: Box<dyn Transport> = Box::new(StubTransport {
            kind: TransportKind::InMemory,
            closed: false,
            next_event: Some(CdpEvent::new("E", Value::Null)),
        });
        assert_eq!(t.kind(), TransportKind::InMemory);
        let r = t.send_command("Test.method", Value::Null, None).unwrap();
        assert!(r.is_null());
        let ev = t.recv_event().unwrap();
        assert!(ev.is_some());
        assert_eq!(ev.unwrap().method, "E");
        // Second recv → None (queue drained).
        let ev2 = t.recv_event().unwrap();
        assert!(ev2.is_none());
        t.close().unwrap();
        // After close → ConnectionClosed.
        let err = t.send_command("X", Value::Null, None).unwrap_err();
        assert!(matches!(err, crate::error::CdpError::ConnectionClosed));
    }
}
