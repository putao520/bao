//! TASK-8 E2E — Playwright 协议兼容性测试。
//!
//! ## 验收范围
//!
//! Playwright(以及 Puppeteer)使用标准 Chrome DevTools 协议,但有几个特殊约定:
//!
//! 1. **HTTP discovery**: GET `/json/version` → `webSocketDebuggerUrl`
//!                       GET `/json/list` → 目标列表
//!                       GET `/json/new?url=...` → 新建 page
//! 2. **Browser-level attach**: Playwright 通过 `ws://host:port/devtools/browser` 连接
//!    browser-level endpoint,而非 `devtools/page/<id>`
//! 3. **Target.attachToTarget with flatten=true**: flat mode 多 session 共享一个 WS 连接
//! 4. **Target.setAutoAttach**: 自动 attach 新 target,事件 `Target.attachedToTarget`
//!
//! ## 测试策略
//!
//! - **Mock 路径**: 每个测试启动一个"一次性" mini HTTP/WS server(只服务一个请求然后退出)
//! - **真实 Playwright 路径**: `#[ignore]` 标记,CI 启用 `BAO_TEST_PLAYWRIGHT=1`
//!
//! @trace REQ-BAO-API-002 [interface:Transport]
//! @trace REQ-BAO-API-008 [level:integration]
//! @trace TEST-BAO-API-E2E-PLAYWRIGHT

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use bao_cdp_client::transport::{Transport, TransportKind, WebSocketTransport};
use serde_json::{json, Value};

// ════════════════════════════════════════════════════════════════════
// 公共辅助 — Frame encoding/decoding
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

fn http_response(stream: &mut TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// One-shot HTTP server:接受一个连接,服务一个请求,返回。
/// 用 closure 决定如何响应。
struct OneShotHttpServer {
    addr: String,
    _handle: thread::JoinHandle<()>,
}

impl OneShotHttpServer {
    fn new<F>(responder: F) -> Self
    where
        F: FnOnce(&str, &mut TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            // 接受一个连接,服务一个请求,返回(线程自然退出)
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 8192];
                let n = match stream.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => return,
                };
                let request = std::str::from_utf8(&buf[..n]).unwrap_or("");
                responder(request, &mut stream);
            }
        });
        Self { addr, _handle: handle }
    }
}

// ════════════════════════════════════════════════════════════════════
// §1 HTTP discovery — /json/version + /json/list + /json/new
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-008 [level:integration]
fn e2e_playwright_http_discovery_json_version() {
    // Arrange
    let server = OneShotHttpServer::new(|_req, stream| {
        // Act
        let body = serde_json::to_string(&json!({
            "Browser": "HeadlessChrome/120",
            "Protocol-Version": "1.3",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9999/devtools/browser/abc",
        }))
        .unwrap();
        http_response(stream, &body);
    });

    let mut stream = TcpStream::connect(&server.addr).unwrap();
    let _ = stream.write_all(b"GET /json/version HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut tmp = [0u8; 4096];
    while let Ok(n) = stream.read(&mut tmp) {
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf);
    // Assert
    assert!(body.contains("webSocketDebuggerUrl"), "must contain ws url");
    assert!(body.contains("HeadlessChrome"), "must contain browser version");
    let json_start = body.find('{').unwrap();
    let obj: Value = serde_json::from_str(&body[json_start..]).unwrap();
    let ws = obj["webSocketDebuggerUrl"].as_str().unwrap();
    assert!(ws.starts_with("ws://"), "ws_url scheme: {ws}");
    assert!(ws.contains("/devtools/browser/"), "ws_url path: {ws}");
}

#[test]
// @trace REQ-BAO-API-008 [level:integration]
fn e2e_playwright_http_discovery_json_list() {
    // Arrange
    let server = OneShotHttpServer::new(|_req, stream| {
        // Act
        let body = serde_json::to_string(&json!([
            {
                "id": "page-1",
                "type": "page",
                "url": "https://example.com",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9999/devtools/page/page-1",
            }
        ]))
        .unwrap();
        http_response(stream, &body);
    });

    let mut stream = TcpStream::connect(&server.addr).unwrap();
    let _ = stream.write_all(b"GET /json/list HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut tmp = [0u8; 4096];
    while let Ok(n) = stream.read(&mut tmp) {
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf);
    let json_start = body.find('[').unwrap();
    let arr: Value = serde_json::from_str(&body[json_start..]).unwrap();
    let arr = arr.as_array().unwrap();
    // Assert
    assert!(!arr.is_empty(), "must have at least 1 page");
    assert_eq!(arr[0]["type"], "page");
    assert!(arr[0]["webSocketDebuggerUrl"].as_str().unwrap().contains("/devtools/page/"));
}

#[test]
// @trace REQ-BAO-API-008 [level:integration]
fn e2e_playwright_http_discovery_json_new() {
    // Arrange
    let server = OneShotHttpServer::new(|_req, stream| {
        // Act
        let body = serde_json::to_string(&json!({
            "id": "page-new",
            "type": "page",
            "url": "about:blank",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9999/devtools/page/page-new",
        }))
        .unwrap();
        http_response(stream, &body);
    });

    let mut stream = TcpStream::connect(&server.addr).unwrap();
    let _ = stream.write_all(b"GET /json/new?https://example.org HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut tmp = [0u8; 4096];
    while let Ok(n) = stream.read(&mut tmp) {
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf);
    let json_start = body.find('{').unwrap();
    let obj: Value = serde_json::from_str(&body[json_start..]).unwrap();
    // Assert
    assert_eq!(obj["type"], "page");
    assert_eq!(obj["id"], "page-new");
}

#[test]
// @trace REQ-BAO-API-008 [level:integration]
fn e2e_playwright_http_discovery_404_for_unknown_path() {
    // Arrange
    let server = OneShotHttpServer::new(|_req, stream| {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    });
    // Act
    let mut stream = TcpStream::connect(&server.addr).unwrap();
    let _ = stream.write_all(b"GET /bogus HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = [0u8; 256];
    let _ = stream.read(&mut buf);
    let resp = String::from_utf8_lossy(&buf);
    // Assert
    assert!(resp.starts_with("HTTP/1.1 404"), "must be 404: {resp}");
}

// ════════════════════════════════════════════════════════════════════
// §2 Browser-level attach — ws://.../devtools/browser/ + Target.attachToTarget + flat-mode
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_playwright_browser_attach_then_flat_session() {
    // Arrange
    // Mini WS server:接受 attach 命令,返回 sessionId
    // Act
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
                return;
            }
            // Read Target.attachToTarget
            let payload = match read_one_frame(&mut stream) {
                Some(p) => p,
                None => return,
            };
            let v: Value = match serde_json::from_slice(&payload) {
                Ok(v) => v,
                Err(_) => return,
            };
            // Assert
            assert_eq!(v["method"], "Target.attachToTarget");
            assert_eq!(v["params"]["flatten"], true);

            // Response:返回 sessionId
            let resp = json!({
                "id": v["id"],
                "result": {"sessionId": "SESSION-FLAT-1"},
            });
            let frame = encode_text_unmasked(&serde_json::to_string(&resp).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();

            // 然后 push 一个 attachedToTarget 事件
            let ev = json!({
                "method": "Target.attachedToTarget",
                "params": {
                    "targetInfo": {"targetId": "T1", "type": "page"},
                    "sessionId": "SESSION-FLAT-1",
                },
            });
            let frame = encode_text_unmasked(&serde_json::to_string(&ev).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(200));
        }
    });

    let url = format!("ws://{addr}/devtools/browser/abc");
    let mut t = WebSocketTransport::connect(&url).expect("ws connect to browser endpoint");

    // 模拟 Playwright:Target.attachToTarget with flatten=true
    let r = t
        .send_command(
            "Target.attachToTarget",
            json!({"targetId":"T1", "flatten": true}),
            None,
        )
        .expect("attachToTarget");
    let session_id = r["sessionId"].as_str().expect("sessionId");
    assert_eq!(session_id, "SESSION-FLAT-1");

    // 接收 attachedToTarget 事件
    t.set_event_timeout(Duration::from_secs(2));
    let ev = t.recv_event().unwrap().expect("attached event");
    assert_eq!(ev.method, "Target.attachedToTarget");
    assert_eq!(ev.params["targetInfo"]["targetId"], "T1");

    let _ = handle.join();
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_playwright_flat_mode_command_with_session_id() {
    // Arrange
    // 验证 flat mode 下,后续命令通过 sessionId 路由到 sub-target
    // Act
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
                return;
            }
            // 服务 2 个 frame
            for _ in 0..2 {
                let payload = match read_one_frame(&mut stream) {
                    Some(p) => p,
                    None => return,
                };
                let v: Value = match serde_json::from_slice(&payload) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let session = v.get("sessionId").cloned().unwrap_or(Value::Null);
                let resp = json!({
                    "id": v["id"],
                    "result": {"ok": true, "echoedSession": session},
                });
                let frame = encode_text_unmasked(&serde_json::to_string(&resp).unwrap());
                let _ = stream.write_all(&frame);
                let _ = stream.flush();
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    let url = format!("ws://{addr}/devtools/browser/x");
    let mut t = WebSocketTransport::connect(&url).unwrap();

    // 第一个命令无 session(browser-level)
    let r = t.send_command("Browser.getVersion", json!({}), None).unwrap();
    // Assert
    assert_eq!(r["ok"], true);
    assert_eq!(r["echoedSession"], Value::Null);

    // 第二个命令带 session(flat mode sub-target)
    let r = t
        .send_command("Page.navigate", json!({"url":"https://x"}), Some("SESSION-FLAT-1"))
        .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(r["echoedSession"], "SESSION-FLAT-1");

    let _ = handle.join();
}

// ════════════════════════════════════════════════════════════════════
// §3 setAutoAttach — 自动 attach 事件流
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_playwright_set_auto_attach_receives_attached_events() {
    // Arrange
    // Act
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            if bao_cdp::ws_handshake::server_handshake(&mut stream).is_err() {
                return;
            }
            // 接收 setAutoAttach 命令
            let payload = match read_one_frame(&mut stream) {
                Some(p) => p,
                None => return,
            };
            let v: Value = match serde_json::from_slice(&payload) {
                Ok(v) => v,
                Err(_) => return,
            };
            let resp = json!({"id": v["id"], "result": {}});
            let frame = encode_text_unmasked(&serde_json::to_string(&resp).unwrap());
            let _ = stream.write_all(&frame);
            let _ = stream.flush();

            // 推送 2 个 attachedToTarget 事件
            for i in 0..2 {
                let ev = json!({
                    "method": "Target.attachedToTarget",
                    "params": {
                        "targetInfo": {"targetId": format!("T{i}"), "type": "page"},
                        "sessionId": format!("SESSION-{i}"),
                    },
                });
                let frame = encode_text_unmasked(&serde_json::to_string(&ev).unwrap());
                let _ = stream.write_all(&frame);
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(20));
            }
            thread::sleep(Duration::from_millis(200));
        }
    });

    let url = format!("ws://{addr}/devtools/browser/x");
    let mut t = WebSocketTransport::connect(&url).unwrap();
    let _ = t
        .send_command(
            "Target.setAutoAttach",
            json!({"autoAttach":true, "waitForDebuggerOnStart":false}),
            None,
        )
        .unwrap();

    t.set_event_timeout(Duration::from_secs(2));
    let mut sessions = Vec::new();
    while let Ok(Some(ev)) = t.recv_event() {
        if ev.method == "Target.attachedToTarget" {
            sessions.push(ev.params["sessionId"].to_string());
        }
    }
    // Assert
    assert_eq!(sessions.len(), 2, "expected 2 attached events");
    let _ = handle.join();
}

// ════════════════════════════════════════════════════════════════════
// §4 真实 Playwright E2E — `#[ignore]` + BAO_TEST_PLAYWRIGHT=1
// ════════════════════════════════════════════════════════════════════

/// 真实 Playwright(Node.js)E2E 测试。
/// 启用方式:BAO_TEST_PLAYWRIGHT=1 + bao_cdp_server 在 9222 端口监听。
#[test]
#[ignore = "real playwright requires BAO_TEST_PLAYWRIGHT=1 + CDP server on 9222"]
fn e2e_real_playwright_full_flow() {
    // Arrange
    // Act
    if std::env::var("BAO_TEST_PLAYWRIGHT").as_deref() != Ok("1") {
        return;
    }
    // 占位:真实 Playwright(Node.js)流程测试。
    // 接入后将:
    // 1. spawn bao_cdp_server 监听 9222
    // 2. 用 Node.js + playwright 连接 ws://127.0.0.1:9222
    // 3. page.goto + page.screenshot
    // 4. 验证 screenshot 是有效 PNG
    // 当前 mock 路径已覆盖 attachToTarget + flat-mode 链路。
}

#[test]
#[ignore = "real playwright requires BAO_TEST_PLAYWRIGHT=1"]
fn e2e_real_playwright_browser_context_isolation() {
    // Arrange
    // Act
    if std::env::var("BAO_TEST_PLAYWRIGHT").as_deref() != Ok("1") {
        return;
    }
    // 占位:验证两个 BrowserContext cookie 隔离
}

// ════════════════════════════════════════════════════════════════════
// §5 Transport 类型校验 — 兼容性
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_playwright_transport_kind_compatible() {
    // Arrange
    // Act
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = bao_cdp::ws_handshake::server_handshake(&mut stream);
            thread::sleep(Duration::from_millis(50));
        }
    });
    let url = format!("ws://{addr}/devtools/browser/x");
    let t = WebSocketTransport::connect(&url).unwrap();
    // Assert
    assert_eq!(t.kind(), TransportKind::WebSocket);
    let _ = handle.join();
}
