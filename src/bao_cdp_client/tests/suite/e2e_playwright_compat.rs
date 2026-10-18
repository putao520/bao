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
//! - **真实 Playwright 全流程**: delegate 形态(cov-audit-cdp-20261014 补实裁决)——
//!   真客户端 oracle 由 bao_browser tests/suite 的 `puppeteer_real_lifecycle_e2e`
//!   (真 Node puppeteer-core)与 `playwright_style_full_flow_three_rounds`
//!   (hermetic 生产 CdpServer + 真 servo)承担;本文件 §4 只做存在性断言守护委派锚
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

// BCE-20261018-STALEBIN: single physical source for the `bao` binary
// three-arm behavior canary, shared with the bao_browser suite helper.
// `#[path]` inclusion adds no workspace dependency edge; `env!("CARGO_MANIFEST_DIR")`
// expands in THIS crate, and this crate is a direct child of `src/` exactly
// like bao_browser, so the helper's workspace-root derivation is identical.
// One source = no copy drift (the BCE's root cause was divergent ladders).
#[path = "../../../bao_browser/tests/suite/common/bao_bin_canary.rs"]
mod bao_bin_canary;

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
        Self {
            addr,
            _handle: handle,
        }
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
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
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
    assert!(
        body.contains("HeadlessChrome"),
        "must contain browser version"
    );
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
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
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
    assert!(arr[0]["webSocketDebuggerUrl"]
        .as_str()
        .unwrap()
        .contains("/devtools/page/"));
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
    let _ =
        stream.write_all(b"GET /json/new?https://example.org HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
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
    let r = t
        .send_command("Browser.getVersion", json!({}), None)
        .unwrap();
    // Assert
    assert_eq!(r["ok"], true);
    assert_eq!(r["echoedSession"], Value::Null);

    // 第二个命令带 session(flat mode sub-target)
    let r = t
        .send_command(
            "Page.navigate",
            json!({"url":"https://x"}),
            Some("SESSION-FLAT-1"),
        )
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
// §4 真实 Playwright E2E — delegate 形态(真客户端 oracle 由 bao_browser 承担)
// ════════════════════════════════════════════════════════════════════

/// Delegate 形态存在性断言:确认被指派的真客户端 oracle 承担者仍在场。
///
/// cov-audit-cdp-20261014 清理候选收口(占位测试补实):§4 两个
/// e2e_real_playwright_* 测试从「env 门内零断言」占位改判为 delegate 形态——
/// 真客户端全流程 oracle 已由 bao_browser tests/suite 两个承担者完整覆盖;
/// 本 crate(bao_cdp_client)无 bao_browser / cdp-server 依赖,进程内无法构造
/// 真 servo 全流程服务端(新增依赖超出合同 scope);本文件的 wire 面(HTTP
/// discovery / browser-attach / flat-session / auto-attach)已由 §1-§3 覆盖。
/// (与 e2e_internal_servo.rs 的同名 helper 同形——单套件二进制内两文件各持
/// 一份,避免为 6 行 helper 触碰 main.rs 的 mod 声明。)
fn assert_delegate_bearer(rel_path: &str, fn_name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_path);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("delegate bearer missing: {} ({e})", path.display()));
    assert!(
        src.contains(&format!("fn {fn_name}")),
        "delegate bearer {rel_path} no longer defines {fn_name} — re-audit the delegation"
    );
}

/// 真实 Playwright 全流程(connect→goto→evaluate→screenshot PNG→network→disconnect)。
///
/// **delegate**(cov-audit-cdp-20261014 补实裁决):真客户端 oracle 由下述承担者
/// 完整覆盖,本测试只做存在性断言守护委派锚:
/// - `bao_browser/tests/suite/puppeteer_e2e_tests.rs::puppeteer_real_lifecycle_e2e`
///   —— 真 Node puppeteer-core(connectOverCDP 同 wire 族)对真 servo 生产栈:
///   connect→newPage→goto(loopback)→network→evaluate→type→screenshot PNG
///   magic→close→disconnect;honest-skip 仅缺 node/npm。
/// - `bao_browser/tests/suite/playwright_full_flow_loop_tests.rs::playwright_style_full_flow_three_rounds`
///   —— hermetic 真 WS 面:生产 CdpServer + 真 servo,3 轮 navigate→
///   frameNavigated 事件→settle→evaluate→input→loopback fetch→screenshot PNG
///   →second nav→close。
///
/// @trace REQ-CDP-001 [level:e2e]
#[test]
fn e2e_real_playwright_full_flow() {
    // Assert — 委派锚存在性(两承担者文件与测试函数仍在场)
    assert_delegate_bearer(
        "../bao_browser/tests/suite/puppeteer_e2e_tests.rs",
        "puppeteer_real_lifecycle_e2e",
    );
    assert_delegate_bearer(
        "../bao_browser/tests/suite/playwright_full_flow_loop_tests.rs",
        "playwright_style_full_flow_three_rounds",
    );
}

/// Playwright BrowserContext 直连生命周期 — REQ-CDS-009 C1-C5+C7 真断言。
///
/// **改判**(原占位观察形态 → 隔离面落地后接真断言):协议面已实装
/// (commit 39a45c9cf:BrowserContextRegistry + Target.createBrowserContext /
/// disposeBrowserContext / getBrowserContexts 命令面 + createTarget
/// (browserContextId) 归属贯通 + disposeBrowserContext closedTargetIds 驱动
/// targetDestroyed / detachedFromTarget),本测试不再依赖委派锚,直接在真
/// bao 引擎上按 Playwright 同款 wire 序列走完整 BrowserContext 生命周期:
///
///   browser.newContext()  → Target.createBrowserContext(唯一 minted id)
///   browser.newPage()     → Target.createTarget{browserContextId} +
///                           Target.attachToTarget{flatten:true}
///   上下文归属真值        → Target.getTargetInfo / getTargets 携
///                           browserContextId
///   context.close()       → Target.disposeBrowserContext → closedTargetIds
///                           + Target.targetDestroyed / detachedFromTarget
///
/// 全程标准 CDP over WebSocket 直连 `/devtools/browser`——这本身就是
/// criterion 7 的直连契约(无需任何 Playwright 适配层)。
///
/// 载具 = 真引擎 BAO_BIN(`bao browser --headless --cdp-port`,与
/// bao_cli_e2e 同款);二进制缺失 → 测试失败(fail-closed,禁 skip)。
///
/// @trace REQ-CDS-009 [criterion:1,2,3,4,5,7] [level:e2e]
#[test]
fn e2e_real_playwright_browser_context_isolation() {
    // ── 引擎进程收割 guard:panic unwind 也 kill+wait,防引擎进程泄漏 ──
    struct EngineGuard(std::process::Child);
    impl Drop for EngineGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // ── bao binary 定位(共享 BCE-20261018-STALEBIN 三臂 canary)──
    // Pins BAO_TEST_BAO_BIN / BAO_BIN are canary-verified (version /
    // `compat --help` / unknown-command nonce echo); a stale, poisoned, or
    // missing binary panics — REQ-CDS-009 e2e is fail-closed, no skip.
    let bao = bao_bin_canary::find_bao_binary();

    // Arrange — spawn 真引擎(headless + 随机 CDP 端口)
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local_addr")
        .port();
    let mut cmd = std::process::Command::new(&bao);
    cmd.args(["browser", "--headless", "--cdp-port", &port.to_string()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn bao browser");
    let mut guard = EngineGuard(child);
    // drain 引擎 stdio(pipe buffer 填满会让引擎写日志时阻塞自己的泵)。
    let pipes: [Option<Box<dyn std::io::Read + Send>>; 2] = [
        guard
            .0
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        guard
            .0
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ];
    for mut pipe in pipes.into_iter().flatten() {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });
    }

    // 等 CDP listener 就绪(bounded 30s;TCP accept = server 线程已起听)
    let ready = {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                ok = true;
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        ok
    };
    assert!(
        ready,
        "bao browser CDP server never listened on 127.0.0.1:{port}"
    );

    // Act — Playwright 同款 browser-level WS 直连(bounded 重试)
    let url = format!("ws://127.0.0.1:{port}/devtools/browser");
    let mut t = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut connected = None;
        while std::time::Instant::now() < deadline {
            match WebSocketTransport::connect(&url) {
                Ok(t) => {
                    connected = Some(t);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
        connected.expect("ws connect /devtools/browser (bounded 10s retry)")
    };
    t.set_command_timeout(Duration::from_secs(30));
    t.set_event_timeout(Duration::from_millis(200));

    // ── C1:createBrowserContext 返回唯一 minted browserContextId ──
    let mint = |t: &mut WebSocketTransport| {
        t.send_command("Target.createBrowserContext", json!({}), None)
            .expect("createBrowserContext")["browserContextId"]
            .as_str()
            .expect("browserContextId is a string")
            .to_string()
    };
    let ctx1 = mint(&mut t);
    assert!(
        ctx1.starts_with("bao-context-") && ctx1 != "bao-default-context",
        "C1: minted context id must be registry-fresh, got {ctx1}"
    );
    let ctx2 = mint(&mut t);
    assert_ne!(
        ctx1, ctx2,
        "C1: two createBrowserContext calls mint distinct ids"
    );

    // ── C3:getBrowserContexts 真值(两个新建都在列)──
    let listed = t
        .send_command("Target.getBrowserContexts", json!({}), None)
        .expect("getBrowserContexts");
    let ids = listed["browserContextIds"].as_array().expect("id array");
    assert!(
        ids.iter().any(|v| v.as_str() == Some(ctx1.as_str())),
        "C3: ctx1 listed, got {listed}"
    );
    assert!(
        ids.iter().any(|v| v.as_str() == Some(ctx2.as_str())),
        "C3: ctx2 listed, got {listed}"
    );

    // ── C4:createTarget 携 browserContextId → 在指定 context 内创建 ──
    let created = t
        .send_command(
            "Target.createTarget",
            json!({"url": "about:blank", "browserContextId": ctx1}),
            None,
        )
        .expect("createTarget in ctx1");
    let target_id = created["targetId"].as_str().expect("targetId").to_string();
    assert!(!target_id.is_empty(), "C4: createTarget mints a target id");

    // Playwright newPage 同款:attach(flat session)后才有 page 会话。
    let attached = t
        .send_command(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .expect("attachToTarget flatten");
    let session = attached["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    assert!(!session.is_empty(), "attach mints a flat session id");

    // ── C5:getTargetInfo(单目标面)归属真值 = ctx1 ──
    let info = t
        .send_command(
            "Target.getTargetInfo",
            json!({"targetId": target_id}),
            Some(&session),
        )
        .expect("getTargetInfo");
    assert_eq!(
        info["targetInfo"]["browserContextId"],
        json!(ctx1),
        "C5: getTargetInfo carries the owning context, got {info}"
    );

    // ── C5:getTargets(枚举面)归属真值 ──
    let targets = t
        .send_command("Target.getTargets", json!({}), None)
        .expect("getTargets");
    let infos = targets["targetInfos"].as_array().expect("targetInfos");
    let ours = infos
        .iter()
        .find(|e| e["targetId"].as_str() == Some(target_id.as_str()))
        .unwrap_or_else(|| panic!("C5: created target listed in getTargets: {targets}"));
    assert_eq!(
        ours["browserContextId"],
        json!(ctx1),
        "C5: getTargets entry carries the owning context, got {targets}"
    );

    // ── C2:disposeBrowserContext → closedTargetIds 连带(context.close())──
    let disposed = t
        .send_command(
            "Target.disposeBrowserContext",
            json!({"browserContextId": ctx1}),
            None,
        )
        .expect("disposeBrowserContext");
    let closed = disposed["closedTargetIds"]
        .as_array()
        .expect("closedTargetIds");
    assert!(
        closed.iter().any(|v| v.as_str() == Some(target_id.as_str())),
        "C2: closedTargetIds names the context's page, got {disposed}"
    );

    // ── C2:关闭事件流(targetDestroyed + detachedFromTarget)——
    // Playwright context.close() 的解析信号;事件与命令响应共线,
    // send_command 期间到达者已在 pending 队列,这里抽干整窗。──
    let mut destroyed = false;
    let mut detached = false;
    let ev_deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < ev_deadline && !(destroyed && detached) {
        match t.recv_event() {
            Ok(Some(ev)) => {
                let tid = ev.params["targetId"].as_str().unwrap_or("");
                if ev.method == "Target.targetDestroyed" && tid == target_id {
                    destroyed = true;
                }
                if ev.method == "Target.detachedFromTarget" && tid == target_id {
                    detached = true;
                }
            }
            Ok(None) => {} // 200ms read timeout — 下一轮
            Err(e) => panic!("C2: wire error during close-event window: {e}"),
        }
    }
    assert!(
        destroyed,
        "C2: Target.targetDestroyed for {target_id} within 15s"
    );
    assert!(
        detached,
        "C2: Target.detachedFromTarget for {target_id} within 15s"
    );

    // ── C3 销毁后真值:ctx1 退役,ctx2 幸存 ──
    let listed = t
        .send_command("Target.getBrowserContexts", json!({}), None)
        .expect("getBrowserContexts after dispose");
    let ids = listed["browserContextIds"].as_array().expect("id array");
    assert!(
        !ids.iter().any(|v| v.as_str() == Some(ctx1.as_str())),
        "C3: disposed ctx1 must leave the registry, got {listed}"
    );
    assert!(
        ids.iter().any(|v| v.as_str() == Some(ctx2.as_str())),
        "C3: survivor ctx2 stays listed, got {listed}"
    );

    drop(guard); // 显式收割引擎(kill + wait)
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

// ════════════════════════════════════════════════════════════════════
// §6 REQ-CDS-009 C6 — context 资源分区隔离判别(真引擎)
// ════════════════════════════════════════════════════════════════════

/// REQ-CDS-009 criterion 6 真值判别:不同 browserContextId 的 cookie /
/// localStorage 互不污染;同 context 的第二个页面共享(判别「分区」而非
/// 「每页隔离」);默认 context 页与显式 context 页互不可见。
///
/// 载具与 §4 同款:真引擎 BAO_BIN + browser-level WS 直连,隔离真值由
/// 页面内的 document.cookie / localStorage 读写给出(HTTP origin 由测试内
/// 置 mini server 提供——cookie/localStorage 都是 origin-scoped,同源不同
/// context 的差值只能来自资源分区)。
///
/// @trace REQ-CDS-009 [criterion:6] [level:e2e]
#[test]
fn e2e_real_playwright_browser_context_partition_isolation() {
    struct EngineGuard(std::process::Child);
    impl Drop for EngineGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // ── mini HTTP origin(cookie/localStorage 需要 tuple origin)──
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind origin :0");
    let origin_port = listener.local_addr().expect("origin local_addr").port();
    thread::spawn(move || {
        let body = b"<html><body>c6-partition</body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    let page_url = format!("http://127.0.0.1:{origin_port}/c6");

    // ── 引擎进程(共享 BCE-20261018-STALEBIN 三臂 canary + guard + stdio drain)──
    let bao = bao_bin_canary::find_bao_binary();

    let port = TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local_addr")
        .port();
    let mut cmd = std::process::Command::new(&bao);
    cmd.args(["browser", "--headless", "--cdp-port", &port.to_string()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn bao browser");
    let mut guard = EngineGuard(child);
    let pipes: [Option<Box<dyn std::io::Read + Send>>; 2] = [
        guard
            .0
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        guard
            .0
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ];
    for mut pipe in pipes.into_iter().flatten() {
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });
    }

    let ready = {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                ok = true;
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        ok
    };
    assert!(ready, "C6: CDP server never listened on 127.0.0.1:{port}");

    let url = format!("ws://127.0.0.1:{port}/devtools/browser");
    let mut t = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut connected = None;
        while std::time::Instant::now() < deadline {
            match WebSocketTransport::connect(&url) {
                Ok(t) => {
                    connected = Some(t);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
        connected.expect("C6: ws connect /devtools/browser")
    };
    t.set_command_timeout(Duration::from_secs(30));
    t.set_event_timeout(Duration::from_millis(200));

    // ── 建 4 页:ctx1 两页(正控)+ ctx2 一页 + 默认 context 一页 ──
    let mint_ctx = |t: &mut WebSocketTransport| {
        t.send_command("Target.createBrowserContext", json!({}), None)
            .expect("createBrowserContext")["browserContextId"]
            .as_str()
            .expect("browserContextId")
            .to_string()
    };
    let ctx1 = mint_ctx(&mut t);
    let ctx2 = mint_ctx(&mut t);

    let open_in = |t: &mut WebSocketTransport,
                   ctx: Option<&str>|
     -> String {
        let mut params = json!({"url": page_url});
        if let Some(ctx) = ctx {
            params["browserContextId"] = json!(ctx);
        }
        let created = t
            .send_command("Target.createTarget", params, None)
            .expect("createTarget");
        let target_id = created["targetId"].as_str().expect("targetId").to_string();
        let attached = t
            .send_command(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
                None,
            )
            .expect("attachToTarget");
        attached["sessionId"].as_str().expect("sessionId").to_string()
    };

    let s1a = open_in(&mut t, Some(ctx1.as_str())); // ctx1 页 A(写者)
    let s1b = open_in(&mut t, Some(ctx1.as_str())); // ctx1 页 B(同 context 共享正控)
    let s2 = open_in(&mut t, Some(ctx2.as_str())); // ctx2 页(隔离判别)
    let sd = open_in(&mut t, None); // 默认 context 页(零回归面)

    // ── 等各页 document 就绪(bounded 轮询;失败页会让后续断言带噪)──
    let eval = |t: &mut WebSocketTransport,
                session: &str,
                expr: &str|
     -> Value {
        t.send_command(
            "Runtime.evaluate",
            json!({"expression": expr, "returnByValue": true}),
            Some(session),
        )
        .unwrap_or_else(|e| panic!("Runtime.evaluate on {session}: {e}"))
    };
    let wait_loaded = |t: &mut WebSocketTransport, session: &str| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut polls = 0u32;
        while std::time::Instant::now() < deadline {
            let state = eval(t, session, "document.readyState")["result"]["value"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            polls += 1;
            if state == "complete" {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        panic!("C6: page session {session} never reached complete");
    };
    for s in [&s1a, &s1b, &s2, &sd] {
        wait_loaded(&mut t, s);
    }

    let eval_string = |t: &mut WebSocketTransport, session: &str, expr: &str| -> Option<String> {
        eval(t, session, expr)["result"]["value"]
            .as_str()
            .map(|s| s.to_owned())
    };

    // ── 写:ctx1 页 A 置 cookie + localStorage ──
    let wrote_cookie = eval_string(
        &mut t,
        &s1a,
        "document.cookie = 'c6=ctx1; path=/'; document.cookie",
    )
    .unwrap_or_default();
    assert!(
        wrote_cookie.contains("c6=ctx1"),
        "C6 正控: ctx1 页 A 写后必能读回自己的 cookie, got {wrote_cookie:?}"
    );
    eval(&mut t, &s1a, "localStorage.setItem('c6k','ctx1v')");

    // ── 判别 1:同 context 第二页共享(cookie + storage)= 分区语义 ──
    let s1b_cookie = eval_string(&mut t, &s1b, "document.cookie").unwrap_or_default();
    assert!(
        s1b_cookie.contains("c6=ctx1"),
        "C6 正控: 同 context 页 B 必须共享页 A 的 cookie(分区而非每页隔离), got {s1b_cookie:?}"
    );
    let s1b_ls = eval_string(&mut t, &s1b, "localStorage.getItem('c6k')");
    assert_eq!(
        s1b_ls.as_deref(),
        Some("ctx1v"),
        "C6 正控: 同 context 页 B 必须共享 localStorage, got {s1b_ls:?}"
    );

    // ── 判别 2:跨 context 互不可见(cookie + storage)──
    let s2_cookie = eval_string(&mut t, &s2, "document.cookie").unwrap_or_default();
    assert!(
        !s2_cookie.contains("c6=ctx1"),
        "C6 隔离: ctx2 页不得读到 ctx1 的 cookie, got {s2_cookie:?}"
    );
    let s2_ls = eval_string(&mut t, &s2, "localStorage.getItem('c6k')");
    assert_ne!(
        s2_ls.as_deref(),
        Some("ctx1v"),
        "C6 隔离: ctx2 页不得读到 ctx1 的 localStorage, got {s2_ls:?}"
    );

    // ── 判别 3:显式 context ↔ 默认 context 互不可见(默认零回归面)──
    let sd_cookie = eval_string(&mut t, &sd, "document.cookie").unwrap_or_default();
    assert!(
        !sd_cookie.contains("c6=ctx1"),
        "C6 隔离: 默认 context 页不得读到 ctx1 的 cookie, got {sd_cookie:?}"
    );

    // ── 反向写读:默认页写,ctx 页读不到;默认页自己读得到 ──
    let d_cookie = eval_string(
        &mut t,
        &sd,
        "document.cookie = 'c6=default; path=/'; document.cookie",
    )
    .unwrap_or_default();
    assert!(
        d_cookie.contains("c6=default"),
        "C6 正控: 默认页写后自己读回, got {d_cookie:?}"
    );
    let s1a_after = eval_string(&mut t, &s1a, "document.cookie").unwrap_or_default();
    assert!(
        !s1a_after.contains("c6=default"),
        "C6 隔离: ctx1 页不得读到默认 context 的 cookie, got {s1a_after:?}"
    );

    drop(guard);
}
