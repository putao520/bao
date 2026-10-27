//! CdpSession — Playwright/Puppeteer 形的 CDP 子会话对象。
//!
//! REQ-LIB-003 AC2:`page.createCDPSession()` 返回 CDPSession,支持
//! send/on 消息。会话由 [`Page::create_cdp_session`](super::page::Page::
//! create_cdp_session) 经 `Target.attachToTarget {targetId, flatten: true}`
//! 协议往返铸造 —— sessionId 唯一真源是服务端 session 表(bao_browser
//! `ws_registry`),客户端永不伪造 session(与 `BridgeSenderBackend` /
//! `bao_cdp::protocol` 的 not_supported 裁决同源:「never a fabricated
//! sessionId」)。
//!
//! # Method 面(Playwright CDPSession 生态,同步形)
//!
//! - `send(method, params) -> Result<Value>` — 会话域命令(直通既有
//!   `Connection::send_command_with_session` 原语,session_id 贯通 transport)
//! - `on(method, handler)` / `off(method, id)` — 会话域事件订阅(镜像
//!   `Connection::on_event`/`off_event` 惯形,wrapper 按 session_id 过滤;
//!   handler 在连接被 `recv_event`/`drain_events` 驱动时分发)
//! - `detach()` — 发送 `Target.detachFromTarget`(既有 session 关闭链:
//!   ws_registry 从 sessionId→target 路由表摘除)+ 本地 detached 置位
//! - `session_id()` / `target_id()` / `is_detached()`
//!
//! # fail-closed 边界(禁 silent-fake)
//!
//! - 无 Connection(memory://bao lazy 形 / 无 servo bridge 的 CLI `bao run`
//!   模式):create 即 `ConnectionClosed`,不铸造本地假 session —— 镜像
//!   node_inspector Session 的 fail-closed 形态(「Linking bao_cdp in would
//!   yield a mode-dependent inert session — the exact silent-fake class this
//!   pass eradicates」)。
//! - attach 响应缺 sessionId → `ProtocolError`,不吞。
//! - detached 后 send/detach → `ConnectionClosed`(Playwright「Session
//!   closed」对齐),不做静默 no-op。
//!
//! # 事件形态取舍(drain vs on)
//!
//! 选 `on` 形态:session 域 pull 循环必须跳过跨 session 事件,而 Transport
//! 无 push-back 面(跳过即静默吞事件);`on` 只是 Connection 既有 handler
//! 注册惯形 + session_id 谓词,零新通道。
//!
//! @trace REQ-LIB-003 [class:CdpSession]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use serde_json::Value;

use crate::connection::{Connection, EventListener, EventListenerId};
use crate::error::CdpError;
use crate::transport::CdpEvent;

/// CDP 子会话(Playwright CDPSession 同构)。
///
/// 持有:服务端铸造的 session_id、attach 时的 target_id、共享 Connection
/// 引用(与 Page 同一连接,session 域命令以 sessionId 区分)、detached 标记。
///
/// @trace REQ-LIB-003 [class:CdpSession]
pub struct CdpSession {
    /// 服务端 mint 的会话 ID(`Target.attachToTarget` 响应)。
    session_id: String,
    /// attach 时的目标 ID。
    target_id: String,
    /// 共享 Connection 引用(同一 Browser 的所有 Page/Session 共用)。
    connection: Rc<RefCell<Connection>>,
    /// 是否已 detach(`detach()` 成功后置位)。
    detached: Cell<bool>,
}

impl std::fmt::Debug for CdpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpSession")
            .field("session_id", &self.session_id)
            .field("target_id", &self.target_id)
            .field("detached", &self.detached.get())
            .finish()
    }
}

impl CdpSession {
    /// 构造会话(仅 [`Page::create_cdp_session`] 调用 —— session 只能来自
    /// 服务端 attach 往返,构造器不设 pub 以封死客户端伪造面)。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub(crate) fn new(
        session_id: impl Into<String>,
        target_id: impl Into<String>,
        connection: Rc<RefCell<Connection>>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            target_id: target_id.into(),
            connection,
            detached: Cell::new(false),
        }
    }

    /// 服务端铸造的会话 ID。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// attach 时的目标 ID。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// 是否已 detach。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn is_detached(&self) -> bool {
        self.detached.get()
    }

    /// 发送会话域 CDP 命令,返回响应 result。
    ///
    /// 直通既有 `Connection::send_command_with_session` 原语 —— method
    /// 字符串与 serde_json params 通道全复用,session_id 作为 flattened
    /// 会话路由键贯通 transport。
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: 已 detach 或连接已断开
    /// - `CdpError::ProtocolError`: 远端返回 JSON-RPC error
    /// - `CdpError::Timeout`: 命令超时
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn send(&self, method: &str, params: Value) -> crate::error::Result<Value> {
        if self.detached.get() {
            return Err(CdpError::ConnectionClosed);
        }
        self.connection.borrow_mut().send_command_with_session(
            method,
            params,
            Some(&self.session_id),
        )
    }

    /// 注册会话域事件 handler,返回 listener ID(可用于 [`CdpSession::off`])。
    ///
    /// wrapper 在 Connection 既有分发面(`recv_event`/`drain_events` 驱动)
    /// 上按 session_id 过滤:仅本会话事件触发 handler,跨 session 事件
    /// 原样流过(不影响其他 handler —— 无事件吞没)。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn on(&self, method: &str, handler: EventListener) -> EventListenerId {
        let session_id = self.session_id.clone();
        let filtered: EventListener = Arc::new(move |ev: CdpEvent| {
            if ev.session_id.as_deref() == Some(session_id.as_str()) {
                handler(ev);
            }
        });
        self.connection.borrow_mut().on_event(method, filtered)
    }

    /// 移除会话域事件 handler。
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn off(&self, method: &str, listener_id: EventListenerId) -> bool {
        self.connection.borrow_mut().off_event(method, listener_id)
    }

    /// Detach — 关闭会话。
    ///
    /// 发送 `Target.detachFromTarget {sessionId}`(既有 session 关闭链:
    /// ws_registry 从路由表摘除该会话,后续会话域命令在服务端即
    /// session-not-found),成功后置本地 detached。重复 detach 返回
    /// `ConnectionClosed`(Playwright「Session closed」对齐,不做静默 no-op)。
    ///
    /// # 错误
    /// - `CdpError::ConnectionClosed`: 已 detach,或 detach 命令发送失败
    ///   (连接已断开)
    /// - `CdpError::ProtocolError`: 服务端拒绝(会话状态保持不变 —— 失败
    ///   即失败,不置 detached)
    ///
    /// @trace REQ-LIB-003 [class:CdpSession]
    pub fn detach(&self) -> crate::error::Result<()> {
        if self.detached.get() {
            return Err(CdpError::ConnectionClosed);
        }
        self.connection.borrow_mut().send_command_with_session(
            "Target.detachFromTarget",
            serde_json::json!({"sessionId": self.session_id}),
            None,
        )?;
        self.detached.set(true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::browser::Browser as HighLevelBrowser;
    use crate::api::browser_context::BrowserContext;
    use crate::api::page::Page;
    use crate::transport::{InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// Mock bridge:服务端 session 表的最小同构(attach 铸造 / detach 摘除 /
    /// 会话域命令按表路由),协议形与 bao_browser ws_registry 一致
    /// (flatten 必需、sessionId 缺失即 session-not-found)。
    struct SessionMockBridge {
        sessions: Mutex<HashMap<String, String>>,
        history: Mutex<Vec<(String, Value, Option<String>)>>,
        next_id: AtomicU32,
    }

    impl SessionMockBridge {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(HashMap::new()),
                history: Mutex::new(Vec::new()),
                next_id: AtomicU32::new(1),
            }
        }

        fn has_session(&self, sid: &str) -> bool {
            self.sessions.lock().unwrap().contains_key(sid)
        }
    }

    impl InMemoryBridge for SessionMockBridge {
        fn dispatch_command(
            &self,
            method: &str,
            params: Value,
            session_id: Option<&str>,
        ) -> InMemoryBridgeResponse {
            self.history.lock().unwrap().push((
                method.to_string(),
                params.clone(),
                session_id.map(Into::into),
            ));
            match method {
                "Target.attachToTarget" => {
                    let flatten = params
                        .get("flatten")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let target_id = params
                        .get("targetId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !flatten {
                        return InMemoryBridgeResponse::Err(
                            "only flatten=true is implemented".into(),
                        );
                    }
                    let sid = format!("sess-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(sid.clone(), target_id.to_string());
                    InMemoryBridgeResponse::Ok(serde_json::json!({ "sessionId": sid }))
                }
                "Target.detachFromTarget" => {
                    let sid = params
                        .get("sessionId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    self.sessions.lock().unwrap().remove(sid);
                    InMemoryBridgeResponse::Ok(serde_json::json!({}))
                }
                _ => match session_id {
                    Some(sid) if self.has_session(sid) => InMemoryBridgeResponse::Ok(
                        serde_json::json!({ "echoMethod": method, "session": sid }),
                    ),
                    Some(sid) => InMemoryBridgeResponse::Err(format!(
                        "Session with given id not found: {sid}"
                    )),
                    None => InMemoryBridgeResponse::Ok(Value::Null),
                },
            }
        }
    }

    /// 构造绑定 Connection 的 Page,返回 (page, 事件 sender, connection 克隆)。
    fn make_connected_page(
        bridge: Arc<dyn InMemoryBridge>,
    ) -> (Rc<Page>, bao_ipc::Sender<CdpEvent>, Rc<RefCell<Connection>>) {
        let browser = Rc::new(HighLevelBrowser::new_for_test("memory://bao"));
        let ctx: Rc<BrowserContext> = HighLevelBrowser::new_context_for_test(&browser);
        let transport = InMemoryTransport::new(bridge);
        let sender = transport.event_sender();
        let conn = Rc::new(RefCell::new(Connection::from_transport(Box::new(
            transport,
        ))));
        let page = Rc::new(Page::new_with_connection(
            "7",
            Rc::downgrade(&ctx),
            Some(conn.clone()),
        ));
        (page, sender, conn)
    }

    #[test]
    fn create_returns_server_minted_session() {
        let (page, _sender, _conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        assert_eq!(session.session_id(), "sess-1");
        assert_eq!(session.target_id(), "7");
        assert!(!session.is_detached());
    }

    #[test]
    fn create_sends_attach_with_flatten_and_page_target() {
        let bridge = Arc::new(SessionMockBridge::new());
        let (page, _sender, _conn) = make_connected_page(bridge.clone());
        let _ = page.create_cdp_session().unwrap();
        let history = bridge.history.lock().unwrap();
        let (method, params, sid) = &history[0];
        assert_eq!(method, "Target.attachToTarget");
        assert_eq!(params["targetId"], "7");
        assert_eq!(params["flatten"], true);
        assert!(sid.is_none(), "attach 是 target 级命令,顶层发送");
    }

    #[test]
    fn send_passes_session_id_to_transport() {
        let bridge = Arc::new(SessionMockBridge::new());
        let (page, _sender, _conn) = make_connected_page(bridge.clone());
        let session = page.create_cdp_session().unwrap();
        let resp = session
            .send(
                "Page.navigate",
                serde_json::json!({"url": "https://example.com"}),
            )
            .unwrap();
        assert_eq!(resp["session"], "sess-1");
        assert_eq!(resp["echoMethod"], "Page.navigate");
        let history = bridge.history.lock().unwrap();
        let (method, _params, sid) = &history[1];
        assert_eq!(method, "Page.navigate");
        assert_eq!(sid.as_deref(), Some("sess-1"));
    }

    #[test]
    fn send_on_unrouted_session_fails_closed() {
        // 服务端表中无此会话时(服务端先行摘除),命令如实报错 —— 不伪造成功。
        let bridge = Arc::new(SessionMockBridge::new());
        let (page, _sender, _conn) = make_connected_page(bridge);
        let session = page.create_cdp_session().unwrap();
        let err = session
            .send("Page.navigate", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, CdpError::ProtocolError(_)));
    }

    #[test]
    fn detach_sends_protocol_command_and_unroutes() {
        let bridge = Arc::new(SessionMockBridge::new());
        let (page, _sender, _conn) = make_connected_page(bridge.clone());
        let session = page.create_cdp_session().unwrap();
        session.detach().unwrap();
        assert!(session.is_detached());
        // 服务端路由表已摘除。
        assert!(!bridge.has_session("sess-1"));
        let history = bridge.history.lock().unwrap();
        let (method, params, sid) = &history[1];
        assert_eq!(method, "Target.detachFromTarget");
        assert_eq!(params["sessionId"], "sess-1");
        assert!(sid.is_none(), "detach 是 browser 级命令,按参数路由");
    }

    #[test]
    fn double_detach_fails_closed() {
        let (page, _sender, _conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        session.detach().unwrap();
        let err = session.detach().unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn send_after_detach_fails_closed() {
        let (page, _sender, _conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        session.detach().unwrap();
        let err = session
            .send("Page.navigate", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, CdpError::ConnectionClosed));
    }

    #[test]
    fn on_receives_only_matching_session_events() {
        let (page, sender, conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let handler: EventListener = Arc::new(move |_ev: CdpEvent| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        session.on("Network.requestWillBeSent", handler);

        // 跨 session 事件:不触发本 handler。
        sender
            .send(
                CdpEvent::new("Network.requestWillBeSent", serde_json::json!({}))
                    .with_session("sess-OTHER"),
            )
            .unwrap();
        // 本 session 事件:触发。
        sender
            .send(
                CdpEvent::new("Network.requestWillBeSent", serde_json::json!({}))
                    .with_session("sess-1"),
            )
            .unwrap();
        conn.borrow_mut().drain_events().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn off_removes_session_handler() {
        let (page, sender, conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let handler: EventListener = Arc::new(move |_ev: CdpEvent| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let id = session.on("Page.frameNavigated", handler);
        assert!(session.off("Page.frameNavigated", id));

        sender
            .send(
                CdpEvent::new("Page.frameNavigated", serde_json::json!({})).with_session("sess-1"),
            )
            .unwrap();
        conn.borrow_mut().drain_events().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn debug_shape_carries_session_identity() {
        let (page, _sender, _conn) = make_connected_page(Arc::new(SessionMockBridge::new()));
        let session = page.create_cdp_session().unwrap();
        let s = format!("{:?}", session);
        assert!(s.contains("CdpSession"));
        assert!(s.contains("sess-1"));
        assert!(s.contains("detached"));
    }
}
