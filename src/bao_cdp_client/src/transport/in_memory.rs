//! InMemoryTransport — 同进程 servo CDP 桥接。
//!
//! ## 架构(DEC-CDP-002)
//!
//! ```text
//!   CDP Client thread ──[Command]──> servo ScriptThread
//!   CDP Client thread <──[Response]── servo ScriptThread
//!   CDP Client thread <──[Event]─── servo ScriptThread  (event channel)
//! ```
//!
//! servo ScriptThread `!Send`(持有 `Rc<RefCell<...>>`),不可跨线程直调。
//! 通过 `InMemoryBridge` trait 抽象桥接逻辑,TASK-2 提供基于 `std::sync::mpsc`
//! 的可测试实现,TASK-3 替换为真实的 servo `CDPRdpBridge`。
//!
//! ## 同步语义
//!
//! `send_command` 阻塞等待响应(`recv_timeout` 避免无限阻塞)。
//! `recv_event` 阻塞等待事件(同样 `recv_timeout`,超时返回 `Ok(None)`)。
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::bridge::event_translator::{translate, ServoEvent};
use crate::error::{CdpError, Result};

use super::r#trait::{CdpEvent, Transport};
use super::TransportKind;

/// InMemory bridge 响应(命令响应 + 推送事件共用一种载荷)。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
#[derive(Debug, Clone)]
pub enum InMemoryBridgeResponse {
    /// 命令成功响应(result 字段)。
    Ok(Value),
    /// 命令失败(JSON-RPC error message)。
    Err(String),
}

/// InMemory 桥接 trait — 抽象 servo ScriptThread 调用。
///
/// TASK-2 用 `MockInMemoryBridge` 单元测试;TASK-3 用真实 servo
/// `CDPRdpBridge` 实现该 trait(内部通过 servo `DevtoolScriptControlMsg`
/// 等 IPC 通信)。
///
/// # 设计要点
///
/// - `Send + Sync`:可被 Arc 包装跨线程共享(虽然 servo ScriptThread !Send,
///   bridge 内部用 channel 转换为 Send-safe 边界)
/// - 单一方法 `dispatch_command` 返回 `InMemoryBridgeResponse`,
///   InMemoryTransport 负责把 channel 推送的事件拼装成 `CdpEvent`
///
/// @trace REQ-BAO-API-002 [interface:Transport]
pub trait InMemoryBridge: Send + Sync {
    /// 派发 CDP 命令到 servo,同步返回响应。
    ///
    /// 实现要点(TASK-3):
    /// 1. 通过 servo ScriptThread sender 把命令转为 `DevtoolScriptControlMsg`
    /// 2. 在 servo actor 内执行(A 类机械映射 / B 类 Eval 合成)
    /// 3. 通过响应 channel 返回 `InMemoryBridgeResponse`
    ///
    /// 实现要点(测试 mock):直接返回预设响应。
    fn dispatch_command(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> InMemoryBridgeResponse;
}

/// InMemoryTransport — 跨线程同步 CDP 桥接 Transport 实现。
///
/// 持有:
/// - `Arc<dyn InMemoryBridge>`:命令派发(servo 端)
/// - `event_rx`:事件接收 channel(InMemoryBridge 侧 push,CDP client 侧 recv)
/// - `event_tx`:`event_rx` 的 sender 克隆,作为构造时的"事件入口"接口
/// - `servo_event_rx`(可选):servo 7 类事件接收 channel(REQ-BAO-API-003)。
///   通过 [`InMemoryTransport::attach_servo_event_receiver`] 接入,
///   `recv_event` 会优先消费 servo 事件(translate 为 CDP event 后返回);
///   无 servo 事件时 fallback 到普通 `event_rx`(供测试 mock 直接 push CDP event)。
/// - `pending_cdp_events`:servo 事件 translate 后可能产生多个 CDP event,
///   未返回的暂存在这里。
///
/// 关闭语义:`close()` drop sender/receiver,后续命令返回 `ConnectionClosed`。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
/// @trace REQ-BAO-API-003 [interface:Transport]
pub struct InMemoryTransport {
    bridge: Arc<dyn InMemoryBridge>,
    event_tx: Sender<CdpEvent>,
    event_rx: Receiver<CdpEvent>,
    /// servo 7 类事件接收端(可选)。注入后由 `recv_event` 优先消费。
    servo_event_rx: Option<Receiver<ServoEvent>>,
    /// servo 事件 translate 后未发出的 CdpEvent 暂存(一对多场景)。
    pending_cdp_events: std::collections::VecDeque<CdpEvent>,
    closed: bool,
    command_timeout: Duration,
    event_timeout: Duration,
}

impl std::fmt::Debug for InMemoryTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryTransport")
            .field("closed", &self.closed)
            .field("command_timeout", &self.command_timeout)
            .field("event_timeout", &self.event_timeout)
            .finish()
    }
}

impl InMemoryTransport {
    /// 构造 InMemory transport,桥接到指定的 `InMemoryBridge` 实现。
    ///
    /// TASK-2 内可传入 mock bridge 用于单元测试;TASK-3 内传入真实 servo bridge。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn new(bridge: Arc<dyn InMemoryBridge>) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<CdpEvent>();
        Self {
            bridge,
            event_tx,
            event_rx,
            servo_event_rx: None,
            pending_cdp_events: std::collections::VecDeque::new(),
            closed: false,
            command_timeout: Duration::from_secs(30),
            event_timeout: Duration::from_millis(100),
        }
    }

    /// 获取事件 sender 克隆 — 供 servo 端(InMemoryBridge 实现)向 CDP client 推送事件。
    ///
    /// 调用方(servo delegate)持有一份 sender,在 servo 触发事件时调用
    /// `event_sender.send(CdpEvent::new(...))` 即可被 `recv_event()` 收到。
    ///
    /// @trace REQ-BAO-API-002 [interface:Transport]
    pub fn event_sender(&self) -> Sender<CdpEvent> {
        self.event_tx.clone()
    }

    /// 接入 servo 事件 receiver(REQ-BAO-API-003)。
    ///
    /// 接入后,`recv_event` 会优先消费 servo 事件并经
    /// [`crate::bridge::event_translator::translate`] 转换为 CDP event。
    /// 无 servo 事件时 fallback 到普通 `event_rx`(供测试直接 push CDP event)。
    ///
    /// 通常与 [`crate::bridge::EventSubscriber::new`] 配合使用:
    /// ```
    /// use bao_cdp_client::bridge::{CDPRdpBridge, EventSubscriber, MockServoBackend, ServoBackend};
    /// use bao_cdp_client::transport::{InMemoryBridge, InMemoryTransport};
    /// use std::sync::Arc;
    ///
    /// let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    /// let bridge = CDPRdpBridge::new(backend);
    /// let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    /// let mut transport = InMemoryTransport::new(bridge_dyn);
    /// let (subscriber, rx) = EventSubscriber::new();
    /// transport.attach_servo_event_receiver(rx);
    /// // 把 subscriber 注册到 servo delegate...
    /// let _ = subscriber;
    /// ```
    ///
    /// @trace REQ-BAO-API-003 [interface:Transport]
    pub fn attach_servo_event_receiver(&mut self, rx: Receiver<ServoEvent>) {
        self.servo_event_rx = Some(rx);
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Transport for InMemoryTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::InMemory
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
        // Bridge 调用是同步阻塞的(servo ScriptThread 通过 channel 转发)。
        // command_timeout 在 TASK-3 真实 servo bridge 实现内体现;此处直接调用。
        match self.bridge.dispatch_command(method, params, session_id) {
            InMemoryBridgeResponse::Ok(v) => Ok(v),
            InMemoryBridgeResponse::Err(msg) => Err(CdpError::ProtocolError(msg)),
        }
    }

    fn recv_event(&mut self) -> Result<Option<CdpEvent>> {
        if self.closed {
            return Err(CdpError::ConnectionClosed);
        }
        // 1. 先返回 pending(translate 一对多产生的剩余事件)
        if let Some(ev) = self.pending_cdp_events.pop_front() {
            return Ok(Some(ev));
        }
        // 2. 优先消费 servo 7 类事件(REQ-BAO-API-003)
        if let Some(servo_rx) = &self.servo_event_rx {
            match servo_rx.recv_timeout(self.event_timeout) {
                Ok(se) => {
                    // translate ServoEvent → Vec<CdpEvent>
                    let mut cdp_events = translate(se);
                    // 第一个直接返回,剩余存 pending
                    if let Some(first) = cdp_events.pop() {
                        for ev in cdp_events.into_iter().rev() {
                            self.pending_cdp_events.push_front(ev);
                        }
                        return Ok(Some(first));
                    }
                    // translate 返回空(理论上不会发生)— 递归一次
                    return self.recv_event();
                }
                Err(RecvTimeoutError::Timeout) => {
                    // servo 无事件,fallback 到普通 event_rx
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // servo 端断开 — 不视为 transport 关闭,继续 fallback event_rx
                }
            }
        }
        // 3. 普通事件 fallback(测试 mock 直接 push CdpEvent)
        match self.event_rx.recv_timeout(self.event_timeout) {
            Ok(ev) => Ok(Some(ev)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CdpError::ConnectionClosed),
        }
    }

    fn close(&mut self) -> Result<()> {
        if !self.closed {
            self.closed = true;
            // drop event sender/receiver implicitly via field; explicit nothing.
        }
        Ok(())
    }

    fn set_command_timeout(&mut self, timeout: Duration) {
        self.command_timeout = timeout;
    }

    fn set_event_timeout(&mut self, timeout: Duration) {
        self.event_timeout = timeout;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Mock bridge for unit testing — TASK-3 will provide the real servo bridge.
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod mock_bridge {
    use super::*;
    use std::sync::Mutex;

    /// Mock bridge:根据 (method, params) 返回预设响应。
    /// 用于测试 InMemoryTransport 的命令派发、错误传播、关闭语义。
    pub struct MockInMemoryBridge {
        /// 命令历史(测试断言用)。
        pub history: Mutex<Vec<(String, Value, Option<String>)>>,
        /// 响应工厂:输入 (method, params, session_id) → response。
        pub responder: Box<dyn Fn(&str, &Value, Option<&str>) -> InMemoryBridgeResponse + Send + Sync>,
    }

    impl MockInMemoryBridge {
        pub fn new<F>(responder: F) -> Self
        where
            F: Fn(&str, &Value, Option<&str>) -> InMemoryBridgeResponse + Send + Sync + 'static,
        {
            Self {
                history: Mutex::new(Vec::new()),
                responder: Box::new(responder),
            }
        }

        /// 返回 Ok(Null) 的简单 mock。
        pub fn ok_null() -> Self {
            Self::new(|_, _, _| InMemoryBridgeResponse::Ok(Value::Null))
        }

        /// 总是返回错误的 mock。
        pub fn always_err(msg: impl Into<String>) -> Self {
            let msg = msg.into();
            Self::new(move |_, _, _| InMemoryBridgeResponse::Err(msg.clone()))
        }
    }

    impl InMemoryBridge for MockInMemoryBridge {
        fn dispatch_command(
            &self,
            method: &str,
            params: Value,
            session_id: Option<&str>,
        ) -> InMemoryBridgeResponse {
            self.history
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone(), session_id.map(|s| s.to_string())));
            (self.responder)(method, &params, session_id)
        }
    }

    #[test]
    fn in_memory_kind_is_in_memory() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        assert_eq!(t.kind(), TransportKind::InMemory);
        assert!(!t.is_closed());
        let _ = t.close();
        assert!(t.is_closed());
    }

    #[test]
    fn in_memory_send_command_returns_ok_response() {
        let bridge = Arc::new(MockInMemoryBridge::new(|_m, _p, _s| {
            InMemoryBridgeResponse::Ok(serde_json::json!({"title": "Test Page"}))
        }));
        let mut t = InMemoryTransport::new(bridge);
        let r = t
            .send_command("Page.getTitle", serde_json::json!({}), None)
            .unwrap();
        assert_eq!(r["title"], "Test Page");
    }

    #[test]
    fn in_memory_send_command_propagates_error() {
        let bridge = Arc::new(MockInMemoryBridge::always_err("method not found"));
        let mut t = InMemoryTransport::new(bridge);
        let err = t
            .send_command("Unknown.method", serde_json::json!({}), None)
            .unwrap_err();
        assert!(matches!(err, CdpError::ProtocolError(_)));
        assert!(err.to_string().contains("method not found"));
    }

    #[test]
    fn in_memory_send_command_after_close_returns_connection_closed() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        t.close().unwrap();
        let err = t
            .send_command("X", serde_json::json!({}), None)
            .unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn in_memory_recv_event_gets_pushed_event() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        // Push event through sender.
        let sender = t.event_sender();
        sender
            .send(CdpEvent::new("Page.frameNavigated", serde_json::json!({"url": "x"})))
            .unwrap();
        // recv with default 100ms timeout should get the event immediately.
        let ev = t.recv_event().unwrap().expect("expected an event");
        assert_eq!(ev.method, "Page.frameNavigated");
        assert_eq!(ev.params["url"], "x");
    }

    #[test]
    fn in_memory_recv_event_returns_none_on_timeout() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        // No event pushed → should time out and return None.
        let ev = t.recv_event().unwrap();
        assert!(ev.is_none());
    }

    #[test]
    fn in_memory_recv_event_after_close_returns_connection_closed() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        t.close().unwrap();
        let err = t.recv_event().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn in_memory_close_is_idempotent() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        t.close().unwrap();
        // Second close should not panic / error.
        t.close().unwrap();
    }

    #[test]
    fn in_memory_session_id_passed_to_bridge() {
        let bridge = Arc::new(MockInMemoryBridge::new(|_m, _p, s| {
            // Echo session_id back in response for assertion.
            let sid = s.unwrap_or("default");
            InMemoryBridgeResponse::Ok(serde_json::json!({"echo": sid}))
        }));
        let mut t = InMemoryTransport::new(bridge);
        let r = t
            .send_command("X.y", serde_json::json!({}), Some("TARGET-99"))
            .unwrap();
        assert_eq!(r["echo"], "TARGET-99");
    }

    #[test]
    fn in_memory_event_timeout_overridable() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let mut t = InMemoryTransport::new(bridge);
        // 1ms timeout → very fast None return.
        t.set_event_timeout(Duration::from_millis(1));
        let start = std::time::Instant::now();
        let ev = t.recv_event().unwrap();
        let elapsed = start.elapsed();
        assert!(ev.is_none());
        assert!(elapsed.as_millis() < 200, "elapsed: {:?}", elapsed);
    }

    #[test]
    fn in_memory_bridge_response_debug() {
        let r1 = InMemoryBridgeResponse::Ok(Value::Null);
        let r2 = InMemoryBridgeResponse::Err("boom".into());
        let s1 = format!("{:?}", r1);
        let s2 = format!("{:?}", r2);
        assert!(s1.contains("Ok"));
        assert!(s2.contains("Err"));
    }

    #[test]
    fn in_memory_transport_debug_format() {
        let bridge = Arc::new(MockInMemoryBridge::ok_null());
        let t = InMemoryTransport::new(bridge);
        let s = format!("{:?}", t);
        assert!(s.contains("InMemoryTransport"));
        assert!(s.contains("closed"));
    }
}
