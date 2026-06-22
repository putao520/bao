//! TASK-8 E2E — External Chrome (WebSocket) 模式端到端测试。
//!
//! ## 验收范围
//!
//! 1. **ws_handshake**: mock ws server → WebSocketTransport::connect → 完整握手
//! 2. **command_round_trip**: send_command → mock response → 解析
//! 3. **event_subscription**: subscribe event → mock push → 接收
//! 4. **multi_command_sequence**: 多个命令按序往返
//! 5. **error_recovery**: JSON-RPC error / 超时
//! 6. **real_chrome**: 真实 Chrome (`ws://127.0.0.1:9222`),graceful skip + 环境变量
//!
//! ## 策略
//!
//! - **Mock 路径**: 启动 in-process mini ws server(bao_cdp::ws_handshake::server_handshake)
//! - **真 Chrome 路径**: 检查 `BAO_TEST_CHROME_URL` 环境变量,有则跑,无则 graceful skip(eprintln + return)
//!
//! @trace REQ-BAO-API-002 [interface:Transport]
//! @trace TEST-BAO-API-E2E-EXTERNAL

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use bao_cdp_client::transport::{Transport, TransportKind, WebSocketTransport};
use bao_cdp_client::CdpError;
use serde_json::{json, Value};

// ════════════════════════════════════════════════════════════════════
// 公共辅助 — mini CDP server(参考 transport_ws_handshake.rs)
// ════════════════════════════════════════════════════════════════════

fn apply_mask(payload: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}

fn encode_text_unmasked(payload: &str) -> Vec<u8> {
    let opcode = 0x1u8;
    let fin = 0x80u8;
    let payload_bytes = payload.as_bytes();
    let len = payload_bytes.len();
    let mut buf = Vec::with_capacity(payload_bytes.len() + 14);
    buf.push(fin | opcode);
    if len < 126 {
        buf.push(len as u8);
    } else if len <= u16::MAX as usize {
        buf.push(126u8);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(127u8);
        buf.extend_from_slice(&(len as u64).to_be_bytes());
    }
    buf.extend_from_slice(payload_bytes);
    buf
}

/// Frame decoding helper:从 stream 读一个 frame,unmask 后返回 payload bytes。
fn read_one_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut decoder = bao_cdp::ws_codec::FrameDecoder::new();
    let header = match decoder.decode_frame(stream) {
        Ok(Some(h)) => h,
        _ => return None,
    };
    let payload = if header.mask {
        let mask = decoder.take_mask();
        let mut p = decoder.take_payload(&header);
        apply_mask(&mut p, &mask);
        p
    } else {
        decoder.take_payload(&header)
    };
    Some(payload)
}

struct MiniCdpServer {
    addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl MiniCdpServer {
    fn new<F>(handler: F) -> Self
    where
        F: FnOnce(TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handler(stream);
            }
        });
        Self { addr, handle: Some(handle) }
    }

    fn url(&self) -> String {
        format!("ws://{}/devtools/page/test", self.addr)
    }
}

impl Drop for MiniCdpServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.join().ok();
        }
    }
}

// ════════════════════════════════════════════════════════════════════
// §1 完整 WS 握手 — Browser::connect("ws://...") → build_websocket_transport
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_ws_handshake_full_chain() {
    // Arrange
    use bao_cdp_client::Browser;

    let server = MiniCdpServer::new(|mut stream| {
        // Act
        let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
        thread::sleep(Duration::from_millis(100));
    });

    // Step 1: Browser::connect 路由 ws:// → WebSocket
    let browser = Browser::connect(&server.url()).expect("route ws://");
    // Assert
    assert!(browser.is_websocket());
    assert_eq!(browser.transport_kind(), TransportKind::WebSocket);

    // Step 2: build_websocket_transport 触发 TCP + WebSocket 握手
    let mut transport = browser.build_websocket_transport().expect("ws handshake");
    assert_eq!(transport.kind(), TransportKind::WebSocket);
    let _ = transport.close();
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_ws_handshake_refused_returns_error() {
    // Arrange
    use bao_cdp_client::Browser;

    // 用一个几乎确定关闭的端口
    let url = "ws://127.0.0.1:1/x";
    // Act
    let browser = Browser::connect(url).expect("route succeeds");
    // Assert
    assert!(browser.is_websocket());
    let err = browser.build_websocket_transport().unwrap_err();
    let msg = err.to_string();
    // bao_cdp_client 把所有 TCP/握手失败统一为 ConnectionFailed
    assert!(
        msg.contains("ConnectionFailed") || msg.contains("ws connect") || msg.contains("refused"),
        "got: {msg}"
    );
}

// ════════════════════════════════════════════════════════════════════
// §2 命令往返 — send_command → mock response → 解析
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_command_round_trip_simple() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Read 1 frame, return JSON response echoing method
        let payload = match read_one_frame(&mut stream) {
            // Act
            Some(p) => p,
            None => return,
        };
        let v: Value = match serde_json::from_slice(&payload) {
            Ok(v) => v,
            Err(_) => return,
        };
        let response = json!({
            "id": v["id"],
            "result": {"echoedMethod": v["method"]},
        });
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    let r = t
        .send_command("Page.navigate", json!({"url":"https://example.com"}), None)
        .expect("round-trip");
    // Assert
    assert_eq!(r["echoedMethod"], "Page.navigate");
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_command_round_trip_multiple() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // 服务 5 个 frame
        for i in 0..5 {
            let payload = match read_one_frame(&mut stream) {
                // Act
                Some(p) => p,
                None => return,
            };
            let v: Value = serde_json::from_slice(&payload).unwrap();
            let response = json!({
                "id": v["id"],
                "result": {"seq": i, "method": v["method"]},
            });
            let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
        }
        thread::sleep(Duration::from_millis(50));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    for i in 0..5 {
        let r = t
            .send_command(&format!("Page.cmd{i}"), json!({}), None)
            .unwrap_or_else(|e| panic!("cmd {i} failed: {e:?}"));
        // Assert
        assert_eq!(r["seq"], i, "response seq must match loop index");
    }
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_command_with_session_id_passes_through() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Act
        let payload = read_one_frame(&mut stream).expect("frame");
        let v: Value = serde_json::from_slice(&payload).unwrap();
        // Echo sessionId back
        let session = v.get("sessionId").cloned().unwrap_or(Value::Null);
        let response = json!({"id": v["id"], "result": {"echoedSession": session}});
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    let r = t
        .send_command("Target.sendMessageToTarget", json!({"msg":"hi"}), Some("TARGET-99"))
        .unwrap();
    // Assert
    assert_eq!(r["echoedSession"], "TARGET-99");
}

// ════════════════════════════════════════════════════════════════════
// §3 事件订阅 — server push event → client recv
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_event_subscription_single_push() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Push one event(无 id,有 method)
        let event = json!({
            "method": "Page.frameNavigated",
            "params": {"url": "https://example.com"},
        });
        // Act
        let frame = encode_text_unmasked(&serde_json::to_string(&event).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_secs(2));
    let ev = t.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Page.frameNavigated");
    assert_eq!(ev.params["url"], "https://example.com");
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_event_subscription_with_session_id() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // 事件带 sessionId(对应 sub-target session)
        let event = json!({
            "method": "Network.requestWillBeSent",
            "params": {"requestId": "REQ-1"},
            "sessionId": "TARGET-SUB",
        });
        // Act
        let frame = encode_text_unmasked(&serde_json::to_string(&event).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(200));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_secs(2));
    let ev = t.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Network.requestWillBeSent");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-SUB"));
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_event_subscription_sequence() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // 连续 push 3 个事件
        let events = vec![
            // Act
            json!({"method":"Page.frameStartedLoading","params":{"frameId":"F"}}),
            json!({"method":"Page.frameNavigated","params":{"frameId":"F","url":"https://x"}}),
            json!({"method":"Page.frameStoppedLoading","params":{"frameId":"F"}}),
        ];
        for ev in events {
            let frame = encode_text_unmasked(&serde_json::to_string(&ev).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(200));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_event_timeout(Duration::from_secs(2));
    let mut methods = Vec::new();
    while let Ok(Some(ev)) = t.recv_event() {
        methods.push(ev.method);
    }
    // Assert
    assert_eq!(methods.len(), 3, "expected 3 events, got {}", methods.len());
    assert_eq!(methods[0], "Page.frameStartedLoading");
    assert_eq!(methods[1], "Page.frameNavigated");
    assert_eq!(methods[2], "Page.frameStoppedLoading");
}

// ════════════════════════════════════════════════════════════════════
// §4 错误恢复 — JSON-RPC error / 超时 / 连接关闭
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_jsonrpc_error_returned_as_protocol_error() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
            return;
        }
        // Act
        let payload = read_one_frame(&mut stream).expect("frame");
        let v: Value = serde_json::from_slice(&payload).unwrap();
        let response = json!({
            "id": v["id"],
            "error": {"code": -32601, "message": "method not found"},
        });
        let frame = encode_text_unmasked(&serde_json::to_string(&response).unwrap());
        let _ = stream.write_all(&frame);
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    let err = t.send_command("Bogus.method", json!({}), None).unwrap_err();
    let msg = err.to_string();
    // Assert
    assert!(
        msg.contains("CDP protocol error") || msg.contains("method not found"),
        "got: {msg}"
    );
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_command_timeout_returns_timeout_error() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        // Act
        let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
        // 不响应,client 必须超时
        thread::sleep(Duration::from_millis(500));
    });

    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.set_command_timeout(Duration::from_millis(50));
    let err = t.send_command("X", json!({}), None).unwrap_err();
    match err {
        CdpError::Timeout(_) | CdpError::IoError(_) | CdpError::ConnectionClosed => {}
        // Assert
        other => panic!("expected timeout/io/closed, got {:?}", other),
    }
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_external_close_then_send_returns_connection_closed() {
    // Arrange
    let server = MiniCdpServer::new(|mut stream| {
        // Act
        let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
        thread::sleep(Duration::from_millis(100));
    });
    let mut t = WebSocketTransport::connect(&server.url()).unwrap();
    t.close().unwrap();
    let err = t.send_command("X", json!({}), None).unwrap_err();
    // Assert
    assert!(matches!(err, CdpError::ConnectionClosed));
}

// ════════════════════════════════════════════════════════════════════
// §5 真 Chrome E2E — graceful skip + BAO_TEST_CHROME_URL 环境变量
// ════════════════════════════════════════════════════════════════════

/// 真实 Chrome 完整 E2E 测试。启用方式:
/// ```sh
/// BAO_TEST_CHROME_URL=ws://127.0.0.1:9222 cargo test e2e_real_chrome_navigation_and_screenshot
/// ```
/// 默认(无 BAO_TEST_CHROME_URL)graceful skip(eprintln + return)。
///
/// ## CDP 流程说明
/// Runtime / Page 都是 **page-domain**,不能在 browser-level session(无 sessionId)
/// 调用,必须先 `Target.createTarget` → `Target.attachToTarget(flatten:true)` 拿到
/// page sessionId,然后在该 session 上调用 page-domain method。
///
/// flatten 模式下所有命令必须显式传 `sessionId` 字段(已通过 send_command 第 3 参实现)。
#[test]
fn e2e_real_chrome_navigation_and_screenshot() {
    // Arrange — 必须显式启用(real chrome 测试)
    let url = match std::env::var("BAO_TEST_CHROME_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("[skip] 环境不可用: BAO_TEST_CHROME_URL not set (real chrome E2E)");
            return;
        }
    };
    let mut t = WebSocketTransport::connect(&url).expect("ws connect to real chrome");
    t.set_command_timeout(Duration::from_secs(10));

    // Act — Step 1: Target.createTarget(browser-level,创建新 page)
    let r = t
        .send_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("createTarget");
    let target_id = r["targetId"]
        .as_str()
        .expect("targetId in createTarget response")
        .to_string();

    // Act — Step 2: Target.attachToTarget(flatten:true)拿到 page sessionId
    let r = t
        .send_command(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attachToTarget");
    let session_id = r["sessionId"]
        .as_str()
        .expect("sessionId in attachToTarget response")
        .to_string();

    // Act — Step 3: Page.navigate(在 page session 上)
    let nav = t
        .send_command(
            "Page.navigate",
            json!({"url":"https://example.com"}),
            Some(&session_id),
        )
        .expect("navigate");
    assert!(nav["frameId"].is_string(), "navigate returns frameId");

    // Act — Step 4: 等待 page load ready
    // 真实 Chrome 异步加载页面,navigate 返回时 DOM 可能尚未 ready。
    // 轮询 Page.getFrameTree 直到 main frame url 已切换 + 不再 loading,
    // 最多等 5 秒(避免无网络环境永久阻塞)。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        let tree = match t.send_command("Page.getFrameTree", json!({}), Some(&session_id)) {
            Ok(v) => v,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let main_url = tree["frameTree"]["frame"]["url"]
            .as_str()
            .unwrap_or("");
        if main_url.contains("example.com") {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    // 即使 ready=false(网络限制无法加载)也继续尝试 screenshot—about:blank 至少可截图
    let _ = ready;

    // Act — Step 5: Page.captureScreenshot(在同一个 page session 上)
    // flatten 模式下 sessionId 必传(传 step 2 拿到的 session_id)
    let r = t
        .send_command(
            "Page.captureScreenshot",
            json!({"format":"png"}),
            Some(&session_id),
        )
        .expect("screenshot");
    // Assert — 必须返回非空 base64 data
    assert!(
        r["data"].is_string(),
        "screenshot must return base64 data, got: {r}"
    );
    let data = r["data"].as_str().expect("data as str");
    assert!(!data.is_empty(), "screenshot base64 must be non-empty");

    // Cleanup — 关闭 target
    let _ = t.send_command(
        "Target.closeTarget",
        json!({"targetId": target_id}),
        None,
    );
}

#[test]
fn e2e_real_chrome_runtime_evaluate() {
    // Arrange — 必须显式启用(real chrome 测试)
    let url = match std::env::var("BAO_TEST_CHROME_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("[skip] 环境不可用: BAO_TEST_CHROME_URL not set (real chrome E2E)");
            return;
        }
    };
    let mut t = WebSocketTransport::connect(&url).expect("ws connect");
    t.set_command_timeout(Duration::from_secs(10));

    // Act — Step 1: Target.createTarget(browser-level,创建新 page)
    let r = t
        .send_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("createTarget");
    let target_id = r["targetId"]
        .as_str()
        .expect("targetId in createTarget response")
        .to_string();

    // Act — Step 2: Target.attachToTarget(flatten:true)拿到 page sessionId
    // Runtime 是 page-domain,必须在 page session 上调用,不能在 browser session。
    let r = t
        .send_command(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attachToTarget");
    let session_id = r["sessionId"]
        .as_str()
        .expect("sessionId in attachToTarget response")
        .to_string();

    // Act — Step 3: Runtime.evaluate(在 page session 上,sessionId 必传)
    let r = t
        .send_command(
            "Runtime.evaluate",
            json!({"expression": "1 + 1"}),
            Some(&session_id),
        )
        .expect("evaluate");
    // Assert — Chrome 返回 {result:{type:"number", value:2, ...}}
    assert_eq!(r["result"]["value"], 2, "1+1 must equal 2, got: {r}");

    // Cleanup — 关闭 target
    let _ = t.send_command(
        "Target.closeTarget",
        json!({"targetId": target_id}),
        None,
    );
}
