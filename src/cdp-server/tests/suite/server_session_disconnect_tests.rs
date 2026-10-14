// REQ-CDP-001 C5「连接断开后资源正确释放」+ REQ-CDS-001:
// process_sessions_once 单连接断开路径的集成覆盖。
//
// 此前覆盖审计(2026-10-14 coverage-audit-cdp)判 GAP:关停 drain 有
// server_stop_tests、destroy 回调有 protocol_robustness_tests(直调
// registry),但「客户端断开 → server.rs:236 process_sessions_once 的
// begin_close → notify_session_destroyed → 移除 session map → finalize」
// 全路径零测试触达(session.begin_close/finalize 无任何测试调用者)。
//
// process_sessions_once 是 CdpServer 私有方法,断开语义只能经公开面
// 真实驱动:CdpServer::run() 的 10ms 轮询 + 真 WS 客户端(tungstenite)。
// 客户端发送 WebSocket Close 后,服务端 CdpSession::process 读得
// Message::Close → Err(SessionError::Closed),process_sessions_once
// 走完整断开路径。资源释放按实现面真实可断言项观测:
//   ① DomainHandler::on_session_destroyed 被调用,sid 等于该会话的
//      on_session_created sid(notify 路径)
//   ② max_sessions 容量恢复:断开前第二连接被拒(max reached),断开后
//      同一端口可再连接并完成命令往返(session map 移除的直接证据)
//   ③ 断开后新会话的 Test.enable/echo 往返正常(registry/broadcaster
//      资源未被破坏)
// @trace REQ-CDP-001
// @trace REQ-CDS-001

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cdp_server::{CdpError, CdpServer, DomainHandler, DomainRegistry, EventSender, ServerConfig};
use serde_json::{json, Value};
use tungstenite::client;
use tungstenite::Message;

type Ws = tungstenite::WebSocket<std::net::TcpStream>;

// ---------------------------------------------------------------------------
// Spy handler — 记录生命周期回调(protocol_robustness_tests 同款惯例)。
// Arc 共享:注册进 registry(被 server 线程消费)后仍可由测试线程断言。
// ---------------------------------------------------------------------------

struct SpyHandler {
    domain: &'static str,
    created: Mutex<Vec<String>>,
    destroyed: Mutex<Vec<String>>,
}

impl SpyHandler {
    fn new(domain: &'static str) -> Arc<Self> {
        Arc::new(Self {
            domain,
            created: Mutex::new(Vec::new()),
            destroyed: Mutex::new(Vec::new()),
        })
    }

    fn created_ids(&self) -> Vec<String> {
        self.created.lock().unwrap().clone()
    }

    fn destroyed_ids(&self) -> Vec<String> {
        self.destroyed.lock().unwrap().clone()
    }
}

impl DomainHandler for SpyHandler {
    fn domain_name(&self) -> &'static str {
        self.domain
    }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _es: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Test.echo" => Ok(json!({ "params": params })),
            // Domain.enable / 任意命令:ok 响应(route_command 的 enable
            // 分支据此登记 enabled_domains + 首 enable 回调)。
            _ => Ok(json!({})),
        }
    }

    fn on_session_created(&self, session_id: &str) {
        self.created.lock().unwrap().push(session_id.to_string());
    }

    fn on_session_destroyed(&self, session_id: &str) {
        self.destroyed.lock().unwrap().push(session_id.to_string());
    }
}

/// Arc 包装使 SpyHandler 满足 registry.register(handler: H) 的按值签名
/// (protocol_robustness_tests Observed 同款)。
struct Observed {
    inner: Arc<SpyHandler>,
}

impl Observed {
    fn new(inner: Arc<SpyHandler>) -> Self {
        Self { inner }
    }
}

impl DomainHandler for Observed {
    fn domain_name(&self) -> &'static str {
        self.inner.domain_name()
    }
    fn handle_command(&self, c: &str, p: Value, s: &dyn EventSender) -> Result<Value, CdpError> {
        self.inner.handle_command(c, p, s)
    }
    fn on_session_created(&self, sid: &str) {
        self.inner.on_session_created(sid);
    }
    fn on_session_destroyed(&self, sid: &str) {
        self.inner.on_session_destroyed(sid);
    }
}

// ---------------------------------------------------------------------------
// Helpers(integration_tests / server_stop_tests 同款惯例)
// ---------------------------------------------------------------------------

fn find_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// 有界等待 run 循环的 listener bind:TCP connect 成功即 bind 就绪
/// (server_stop_tests 同款探针,5s deadline 吸收 suite 负载)。
fn wait_bind(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "cdp server did not bind to 127.0.0.1:{} within 5s",
            port
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn ws_connect(port: u16, target_id: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{}/devtools/page/{}", port, target_id);
    let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (ws, _) = client(&url, stream).unwrap();
    ws
}

fn ws_send(ws: &mut Ws, text: &str) {
    ws.send(Message::Text(text.to_string().into())).unwrap();
}

/// 读一条文本消息;超时/对端关闭/Close 帧 → None。
fn ws_read_text(ws: &mut Ws) -> Option<String> {
    match ws.read() {
        Ok(Message::Text(t)) => Some(t.to_string()),
        Ok(Message::Binary(b)) => Some(String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "timed out (5s) waiting for: {}",
            what
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_server(port: u16, registry: Arc<cdp_server::DomainRegistry<Observed>>) -> RunningServer {
    let config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(config, registry);
    let stop = server.stop_handle();
    // run() 的结果经 channel 传回:bind 失败等原因可如实断言而非悬挂。
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = tx.send(server.run());
    });
    RunningServer { stop, handle, result: rx }
}

struct RunningServer {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: thread::JoinHandle<()>,
    result: mpsc::Receiver<Result<(), String>>,
}

impl RunningServer {
    fn shutdown(self) {
        self.stop.store(true, Ordering::Release);
        self.handle.join().expect("cdp server thread joined");
        self.result
            .recv_timeout(Duration::from_secs(5))
            .expect("run() returned within 5s of stop")
            .expect("run() returns Ok(()) on cooperative stop");
    }
}

// ---------------------------------------------------------------------------
// §REQ-CDP-001-C5: 单连接断开 → process_sessions_once 全路径
// ---------------------------------------------------------------------------

#[test]
fn single_connection_disconnect_releases_session_resources() {
    let port = find_free_port();
    let page = Arc::new(SpyHandler::new("Test"));
    let reg = Arc::new(DomainRegistry::<Observed>::new());
    reg.register(Observed::new(Arc::clone(&page))).unwrap();
    // max_sessions=1:断开前容量被占用是可断言的(第二连接被拒),
    // 断开后容量恢复即 session map 移除的直接证据。
    let config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .max_sessions(1)
        .build();
    let mut server = CdpServer::with_registry(config, reg);
    let stop = server.stop_handle();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = tx.send(server.run());
    });
    wait_bind(port);

    // A 连接并启用 Test 域(Active + enabled_domains + on_session_created)。
    let mut a = ws_connect(port, "target-a");
    ws_send(&mut a, r#"{"id":1,"method":"Test.enable","params":{}}"#);
    let resp = ws_read_text(&mut a).expect("A received Test.enable response");
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["id"], 1, "Test.enable response: {}", resp);
    assert!(v.get("result").is_some(), "Test.enable ok: {}", resp);

    // A 的 sid 由服务端 generate_session_id 生成,经 created 回调观测。
    wait_until("A on_session_created", || !page.created_ids().is_empty());
    let sid_a = page.created_ids()[0].clone();

    // 断开前:容量已满,第二连接 B 即使完成 WS 握手也会被服务端丢弃
    // (max_sessions 检查在 accept 之后),零响应。
    {
        let mut b = ws_connect(port, "target-b");
        ws_send(&mut b, r#"{"id":1,"method":"Test.enable","params":{}}"#);
        assert!(
            ws_read_text(&mut b).is_none(),
            "max_sessions=1 占满期间第二连接不得被服务(资源未释放的对照)"
        );
    }

    // A 断开:显式 WebSocket Close → 服务端 read_ws_message 得
    // Message::Close → process() Err → process_sessions_once 断开路径
    // (begin_close → notify_session_destroyed → remove → finalize)。
    a.close(None).ok();
    drop(a);

    // ① notify 路径:on_session_destroyed 恰一次,sid = A 的 sid。
    wait_until("A on_session_destroyed", || !page.destroyed_ids().is_empty());
    assert_eq!(
        page.destroyed_ids(),
        vec![sid_a.clone()],
        "断开后恰一次终局通知,sid 对应该会话"
    );

    // ② 容量恢复:B' 可再连接并完成命令往返(session map 已移除)。
    let mut b2 = ws_connect(port, "target-b2");
    ws_send(&mut b2, r#"{"id":2,"method":"Test.echo","params":{"k":"v"}}"#);
    let resp2 = ws_read_text(&mut b2).expect("B' received Test.echo response");
    let v2: Value = serde_json::from_str(&resp2).unwrap();
    assert_eq!(v2["id"], 2, "B' echo response: {}", resp2);
    assert_eq!(
        v2["result"]["params"]["k"], "v",
        "断开后新会话命令往返正常(registry 资源未被破坏)"
    );

    // ③ 终局通知不再发生(断开路径只跑一轮,无重复 finalize/notify)。
    assert_eq!(
        page.destroyed_ids().len(),
        1,
        "断开后终局通知恰一次,无重复"
    );

    drop(b2);
    stop.store(true, Ordering::Release);
    handle.join().expect("cdp server thread joined");
    rx.recv_timeout(Duration::from_secs(5))
        .expect("run() returned within 5s of stop")
        .expect("run() returns Ok(()) on cooperative stop");
}

#[test]
fn disconnect_notifies_destroyed_only_for_enabled_domains() {
    let port = find_free_port();
    let page = Arc::new(SpyHandler::new("Page"));
    let runtime = Arc::new(SpyHandler::new("Runtime"));
    let reg = Arc::new(DomainRegistry::<Observed>::new());
    reg.register(Observed::new(Arc::clone(&page))).unwrap();
    reg.register(Observed::new(Arc::clone(&runtime))).unwrap();
    let server = spawn_server(port, reg);
    wait_bind(port);

    // 只 enable Page;Runtime 保持未 enable。
    let mut a = ws_connect(port, "target-c");
    ws_send(&mut a, r#"{"id":1,"method":"Page.enable","params":{}}"#);
    let resp = ws_read_text(&mut a).expect("Page.enable response");
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert!(v.get("result").is_some(), "Page.enable ok: {}", resp);

    wait_until("Page on_session_created", || !page.created_ids().is_empty());
    let sid = page.created_ids()[0].clone();

    // 断开 → process_sessions_once 取断开前一刻的 enabled_domains 快照
    // (仅 Page),终局通知只发给 enable 过的域。
    a.close(None).ok();
    drop(a);

    wait_until("Page on_session_destroyed", || !page.destroyed_ids().is_empty());
    assert_eq!(page.destroyed_ids(), vec![sid]);
    assert!(
        runtime.destroyed_ids().is_empty(),
        "未 enable 的域不得收到终局通知"
    );

    server.shutdown();
}
