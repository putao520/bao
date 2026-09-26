// @trace TEST-CDP-SRV [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-003,REQ-CDP-004,REQ-CDP-005,REQ-CDP-006,REQ-CDP-007,REQ-CDP-008] [level:integration]
//
// REQ-CDP-001~008 — CDP Server / domain dispatch, exercised from the
// bao_runtime suite over the crates that ARE in this crate's dependency
// closure (dev-deps `cdp-server` + `bao_cdp`, normal dep `bao_cdp_client`).
//
// What is REAL here (no mocks of the middleware under test):
// - `cdp_server::CdpServer` binds a real TCP listener, serves the HTTP
//   discovery endpoints and upgrades WebSocket sessions (REQ-CDP-001).
// - `bao_cdp_client::WebSocketTransport` is the product's real RFC 6455
//   client — the same client `Browser::connect("ws://…")` uses.
// - Domain commands are served by the REAL bao_cdp dispatch
//   (`bao_cdp::handle_command`), registered into the real DomainRegistry
//   through the documented `DomainHandler` embedder seam, one handler per
//   domain — the same registry shape production installs.
// - The bridge-less (stateless) domain face is asserted for what it is:
//   fail-closed errors (never fabricated page data) and truthful acks.
//   Commands that need the live servo page (navigate/screenshot/body) must
//   error explicitly — asserting that is the bridge-less contract.
//
// Layer boundary note: the WS-session registry (Target.attachToTarget
// minting) lives in bao_browser, which is NOT in this crate's dependency
// closure; deep servo-backed conformance lives in its SPEC-designated
// location (bao_cdp_client/tests/suite/cdp_conformance/,
// cdp-server/tests/suite/, bao_browser/tests/suite/).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_cdp_client::{Transport, WebSocketTransport};
use cdp_server::{
    CdpServer, DomainHandler, DomainRegistry, EventSender, ServerConfig, TargetInfo, TargetProvider,
};
use serde_json::{json, Value};

const TID: &str = "target-itest";

// ---------------------------------------------------------------------------
// Fixtures: the embedder seams (DomainHandler / TargetProvider), filled with
// the real product dispatch instead of servo.
// ---------------------------------------------------------------------------

/// Serves every command through the real `bao_cdp::handle_command`
/// (bridge = None → the stateless domain face).
struct BaoDomainDispatch {
    domain: &'static str,
}

impl DomainHandler for BaoDomainDispatch {
    fn domain_name(&self) -> &'static str {
        self.domain
    }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, cdp_server::CdpError> {
        let msg = bao_cdp::CdpMessage {
            id: Some(0),
            method: command.to_string(),
            params: Some(params.clone()),
            session_id: None,
        };
        let resp = bao_cdp::handle_command(msg, TID, &Some(params), None);
        match (resp.result, resp.error) {
            (Some(result), _) => Ok(result),
            (None, Some(err)) => Err(err),
            (None, None) => Ok(json!({})),
        }
    }
}

/// The `TargetProvider` embedder seam with real mutable state — the same
/// trait bao_browser's ServoTargetProvider implements in production.
struct TestTargetProvider {
    targets: Mutex<Vec<TargetInfo>>,
    port: u16,
    next_id: std::sync::atomic::AtomicUsize,
}

impl TargetProvider for TestTargetProvider {
    fn list_targets(&self) -> Vec<TargetInfo> {
        self.targets.lock().unwrap().clone()
    }

    fn create_target(&self, url: &str) -> Result<TargetInfo, String> {
        use std::sync::atomic::Ordering;
        let mut guard = self.targets.lock().unwrap();
        let n = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let info = TargetInfo {
            id: format!("page-{}", n),
            target_type: "page".into(),
            title: "Bao".into(),
            url: url.to_string(),
            web_socket_debugger_url: format!("ws://127.0.0.1:{}/devtools/page/page-{}", self.port, n),
        };
        guard.push(info.clone());
        Ok(info)
    }

    fn close_target(&self, target_id: &str) -> Result<(), String> {
        let mut guard = self.targets.lock().unwrap();
        let before = guard.len();
        guard.retain(|t| t.id != target_id);
        if guard.len() == before {
            Err(format!("no such target: {}", target_id))
        } else {
            Ok(())
        }
    }

    fn activate_target(&self, target_id: &str) -> Result<(), String> {
        let guard = self.targets.lock().unwrap();
        if guard.iter().any(|t| t.id == target_id) {
            Ok(())
        } else {
            Err(format!("no such target: {}", target_id))
        }
    }
}

/// A running CdpServer on a real port. Drop requests shutdown.
struct TestServer {
    port: u16,
    provider: Option<Arc<TestTargetProvider>>,
    stop: Arc<AtomicBool>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn spawn_server(with_provider: bool) -> TestServer {
    let port = free_port();
    let config = ServerConfig {
        port,
        ..ServerConfig::default()
    };
    let registry = Arc::new(DomainRegistry::<BaoDomainDispatch>::new());
    for d in [
        "Target",
        "Page",
        "Runtime",
        "DOM",
        "Network",
        "CSS",
        "Emulation",
        "Input",
        "Overlay",
        "Debugger",
        "Log",
        "Fetch",
        "Browser",
    ] {
        registry
            .register(BaoDomainDispatch { domain: d })
            .expect("domain register");
    }
    let mut server = CdpServer::with_registry(config, registry);
    let provider = if with_provider {
        let p = Arc::new(TestTargetProvider {
            targets: Mutex::new(vec![TargetInfo {
                id: "page-1".into(),
                target_type: "page".into(),
                title: "Bao".into(),
                url: "about:blank".into(),
                web_socket_debugger_url: format!(
                    "ws://127.0.0.1:{}/devtools/page/page-1",
                    port
                ),
            }]),
            port,
            next_id: std::sync::atomic::AtomicUsize::new(1),
        });
        server.set_target_provider(Arc::clone(&p) as Arc<dyn TargetProvider>);
        Some(p)
    } else {
        None
    };
    let stop = server.stop_handle();
    let mut srv = server;
    std::thread::spawn(move || {
        let _ = srv.run();
    });
    wait_ready(port);
    TestServer {
        port,
        provider,
        stop,
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("local_addr")
        .port()
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "CDP server on port {} never came up",
            port
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Raw HTTP GET against the discovery endpoint; reads until EOF or a short
/// read timeout (covers both Connection: close and keep-alive servers).
fn http_get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    )
    .expect("write request");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn ws_connect(port: u16, target: &str) -> WebSocketTransport {
    WebSocketTransport::connect(&format!(
        "ws://127.0.0.1:{}/devtools/page/{}",
        port, target
    ))
    .expect("ws connect")
}

fn ws_err_text(err: &bao_cdp_client::CdpError) -> String {
    format!("{}", err)
}

fn body_of(http: &str) -> &str {
    match http.find("\r\n\r\n") {
        Some(i) => &http[i + 4..],
        None => http,
    }
}

// ---------------------------------------------------------------------------
// REQ-CDP-001 — CdpServer: HTTP discovery, WebSocket, JSON-RPC 2.0, sessions
// ---------------------------------------------------------------------------

/// "bao browser 启动后 curl http://127.0.0.1:9222/json/version 返回 JSON 含
/// \"webSocketDebuggerUrl\"" — same wire contract on an ephemeral port.
#[test]
fn req_cdp_001_http_discovery_json_version() {
    let server = spawn_server(false);
    let http = http_get(server.port, "/json/version");
    assert!(http.starts_with("HTTP/1.1 200"), "status line: {}", http);
    let body: Value = serde_json::from_str(body_of(&http).trim()).expect("json body");
    assert_eq!(body["Protocol-Version"], "1.3");
    let ws_url = body["webSocketDebuggerUrl"].as_str().expect("ws url");
    assert!(
        ws_url.starts_with(&format!("ws://127.0.0.1:{}/devtools/browser", server.port)),
        "webSocketDebuggerUrl: {}",
        ws_url
    );
    assert!(body["Browser"].as_str().unwrap_or("").contains("Bao"));
}

/// Target discovery surface over HTTP: target listing (`/json` AND the
/// `/json/list` alias — both must serve the list), `/json/new` in both wire
/// forms (Chrome's `?url=` key-value and the legacy bare-URL form),
/// `/json/close`, `/json/activate` — all backed by the TargetProvider
/// embedder seam, and every reply carries a proper HTTP status line.
#[test]
fn req_cdp_001_http_target_list_new_close_activate() {
    let server = spawn_server(true);

    // Both listing aliases serve the identical payload.
    let http = http_get(server.port, "/json");
    assert!(http.starts_with("HTTP/1.1 200"), "status line: {}", http);
    let list: Value = serde_json::from_str(body_of(&http).trim()).expect("json list");
    let entries = list.as_array().expect("target array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], "page-1");
    assert_eq!(entries[0]["type"], "page");
    assert_eq!(entries[0]["url"], "about:blank");
    assert!(entries[0]["web_socket_debugger_url"].is_string());

    let http = http_get(server.port, "/json/list");
    assert!(
        http.starts_with("HTTP/1.1 200"),
        "/json/list alias must serve the list, got: {}",
        http
    );
    let list_alias: Value = serde_json::from_str(body_of(&http).trim()).expect("json list alias");
    assert_eq!(list_alias.as_array().map(|a| a.len()), Some(1));

    // Chrome's key-value form: the `url=` key is stripped, the target URL is
    // the decoded value — the key never reaches the provider.
    let http = http_get(server.port, "/json/new?url=https://example.com/key-form");
    let created: Value = serde_json::from_str(body_of(&http).trim()).expect("json new key form");
    assert_eq!(created["url"], "https://example.com/key-form");
    assert_eq!(created["id"], "page-2");
    assert_eq!(server.provider.as_ref().unwrap().list_targets().len(), 2);

    // Legacy bare-URL form still works.
    let http = http_get(server.port, "/json/new?https://example.com/bare");
    let created2: Value = serde_json::from_str(body_of(&http).trim()).expect("json new bare form");
    assert_eq!(created2["url"], "https://example.com/bare");

    // Activate replies with a real HTTP status line + the documented body.
    let http = http_get(server.port, "/json/activate/page-1");
    assert!(
        http.starts_with("HTTP/1.1 200"),
        "activate must carry a status line: {}",
        http
    );
    assert_eq!(body_of(&http).trim(), "Target activated");

    let http = http_get(server.port, "/json/close/page-1");
    let closed: Value = serde_json::from_str(body_of(&http).trim()).expect("close json");
    assert_eq!(closed["success"], true);
    assert_eq!(closed["targetId"], "page-1");
    let left = server.provider.as_ref().unwrap().list_targets();
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|t| t.id != "page-1"));

    // Closing an unknown target is an HTTP 500 with a status line, never a
    // silent ok and never a bare unparseable body.
    let http = http_get(server.port, "/json/close/page-1");
    assert!(
        http.starts_with("HTTP/1.1 500"),
        "double close must fail with an HTTP 500 status line: {}",
        http
    );
}

/// Without a TargetProvider, close/activate/new must reply with an HTTP 500
/// status line — never a silent connection drop with zero bytes (the
/// historical defect: the no-provider branch returned without responding).
#[test]
fn req_cdp_001_discovery_without_provider_responds_500() {
    let server = spawn_server(false);

    for path in ["/json/close/x", "/json/activate/x", "/json/new"] {
        let http = http_get(server.port, path);
        assert!(
            http.starts_with("HTTP/1.1 500"),
            "{} without provider must respond HTTP 500, got: {:?}",
            path,
            http
        );
    }
}

/// JSON-RPC 2.0 command/response roundtrip over the real WebSocket transport.
#[test]
fn req_cdp_001_ws_jsonrpc_roundtrip() {
    let server = spawn_server(false);
    let mut ws = ws_connect(server.port, TID);
    let result = ws
        .send_command("Browser.getVersion", json!({}), None)
        .expect("roundtrip");
    assert!(
        result["product"].as_str().unwrap_or("").starts_with("Bao"),
        "result: {}",
        result
    );
    assert_eq!(result["protocolVersion"], "1.3");
    assert!(result["userAgent"].is_string());
    Transport::close(&mut ws).expect("close");
}

/// Unknown method over the wire → JSON-RPC error -32601 (method not found).
#[test]
fn req_cdp_001_ws_unknown_method_method_not_found() {
    let server = spawn_server(false);
    let mut ws = ws_connect(server.port, TID);
    let err = ws
        .send_command("NoSuchDomain.method", json!({}), None)
        .expect_err("must be an error");
    let text = ws_err_text(&err);
    assert!(text.contains("-32601"), "err: {}", text);
    assert!(text.contains("wasn't found"), "err: {}", text);
    Transport::close(&mut ws).ok();
}

/// Malformed frame → JSON-RPC -32600 with a null id (parse-level rejection).
#[test]
fn req_cdp_001_ws_invalid_json_invalid_request() {
    use bao_cdp::ws_codec::Opcode;
    use bun_uws::ws_client::{RecvOutcome, WebSocketClient};

    let server = spawn_server(false);
    let mut raw =
        WebSocketClient::connect(&format!("ws://127.0.0.1:{}/devtools/page/{}", server.port, TID))
            .expect("raw ws connect");
    raw.send_text("not json").expect("send raw frame");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got: Option<Value> = None;
    while Instant::now() < deadline {
        match raw.recv().expect("recv") {
            RecvOutcome::Message(op, bytes) if op == Opcode::Text => {
                got = serde_json::from_slice(&bytes).ok();
                break;
            }
            RecvOutcome::Message(..) => continue,
            RecvOutcome::Timeout => continue,
            RecvOutcome::Closed => break,
        }
    }
    let resp = got.expect("invalid-request response");
    assert!(resp["id"].is_null(), "resp: {}", resp);
    assert_eq!(resp["error"]["code"], -32600, "resp: {}", resp);
}

/// Concurrent sessions: two independent WS clients multiplex on one server.
#[test]
fn req_cdp_001_concurrent_sessions_multiplexed() {
    let server = Arc::new(spawn_server(false));
    let mut handles = Vec::new();
    for t in 0..2 {
        let srv = Arc::clone(&server);
        handles.push(std::thread::spawn(move || {
            let mut ws = ws_connect(srv.port, &format!("t-conc-{}", t));
            for i in 0..3 {
                let result = ws
                    .send_command("Browser.getVersion", json!({ "attempt": i }), None)
                    .expect("roundtrip");
                assert!(result["product"].is_string(), "result: {}", result);
            }
            Transport::close(&mut ws).ok();
        }));
    }
    for h in handles {
        h.join().expect("client thread");
    }
}

/// A disconnected client must not wedge the server: the next session is
/// served immediately (connection teardown releases the session slot).
#[test]
fn req_cdp_001_client_disconnect_then_new_session_serves() {
    let server = spawn_server(false);
    let mut a = ws_connect(server.port, TID);
    let r = a
        .send_command("Browser.getVersion", json!({}), None)
        .expect("first session works");
    assert!(r["product"].is_string(), "result: {}", r);
    Transport::close(&mut a).expect("close a");

    let mut b = ws_connect(server.port, "target-b");
    let r = b
        .send_command("Browser.getVersion", json!({}), None)
        .expect("session after disconnect works");
    assert!(r["product"].is_string(), "result: {}", r);
    Transport::close(&mut b).ok();

    // Discovery endpoint still responsive after the teardown.
    let http = http_get(server.port, "/json/version");
    assert!(http.starts_with("HTTP/1.1 200"), "{}", http);
}

// ---------------------------------------------------------------------------
// REQ-CDP-002 ~ 008 — domain dispatch (stateless face + wire enable)
// ---------------------------------------------------------------------------

fn dispatch(method: &str, params: Value) -> bao_cdp::CdpResponse {
    let msg = bao_cdp::CdpMessage {
        id: Some(7),
        method: method.to_string(),
        params: Some(params.clone()),
        session_id: None,
    };
    bao_cdp::handle_command(msg, TID, &Some(params), None)
}

fn ok_result(resp: &bao_cdp::CdpResponse) -> Value {
    resp.result.clone().expect("expected ok response")
}

fn err_code(resp: &bao_cdp::CdpResponse) -> i64 {
    resp.error.as_ref().expect("expected error response").code
}

/// REQ-CDP-002 — Runtime domain (stateless face; the SM Debugger-mapped face
/// needs the live page bridge and is covered by bao_cdp_client conformance).
#[test]
fn req_cdp_002_runtime_domain_stateless_face() {
    // Wire check first: Runtime.enable flows through server → registry →
    // the real handler.
    let server = spawn_server(false);
    let mut ws = ws_connect(server.port, TID);
    let r = ws
        .send_command("Runtime.enable", json!({}), None)
        .expect("Runtime.enable over the wire");
    assert_eq!(r, json!({}), "Chrome semantics: enable returns no context id");
    Transport::close(&mut ws).ok();

    let r = ok_result(&dispatch("Runtime.evaluate", json!({"expression": "1+1"})));
    assert_eq!(r["result"]["type"], "undefined");
    assert!(r["exceptionDetails"].is_null());

    let r = ok_result(&dispatch(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function(){return 1}"}),
    ));
    assert_eq!(r["result"]["type"], "undefined");

    let r = ok_result(&dispatch("Runtime.getProperties", json!({})));
    assert_eq!(r["result"], json!([]));

    ok_result(&dispatch("Runtime.releaseObject", json!({"objectId": "x"})));
    ok_result(&dispatch(
        "Runtime.releaseObjectGroup",
        json!({"objectGroup": "g"}),
    ));
    ok_result(&dispatch("Runtime.runIfWaitingForDebugger", json!({})));

    let resp = dispatch("Runtime.noSuchMethod", json!({}));
    assert_eq!(err_code(&resp), -32601);
}

/// REQ-CDP-003 — Debugger domain (stateless face).
#[test]
fn req_cdp_003_debugger_domain_stateless_face() {
    ok_result(&dispatch("Debugger.enable", json!({})));
    ok_result(&dispatch("Debugger.disable", json!({})));

    let r = ok_result(&dispatch(
        "Debugger.setBreakpointByUrl",
        json!({"url": "https://example.com/a.js", "lineNumber": 3, "columnNumber": 0}),
    ));
    assert!(r["breakpointId"].is_string(), "resp: {}", r);
    assert_eq!(r["locations"], json!([]));

    for m in ["pause", "resume", "stepOver", "stepInto", "stepOut"] {
        ok_result(&dispatch(&format!("Debugger.{}", m), json!({})));
    }
    ok_result(&dispatch(
        "Debugger.removeBreakpoint",
        json!({"breakpointId": "1"}),
    ));

    assert_eq!(err_code(&dispatch("Debugger.bogus", json!({}))), -32601);
}

/// REQ-CDP-004 — Page domain: stateless acks + fail-closed page commands.
#[test]
fn req_cdp_004_page_domain_stateless_face() {
    ok_result(&dispatch("Page.enable", json!({})));
    ok_result(&dispatch("Page.disable", json!({})));
    ok_result(&dispatch("Page.setLifecycleEventsEnabled", json!({"enabled": true})));

    // Page commands that own a real renderer must fail closed without the
    // servo bridge — never a fabricated frameId/image.
    assert_eq!(err_code(&dispatch("Page.navigate", json!({"url": "https://example.com"}))), -32603);
    assert_eq!(
        err_code(&dispatch("Page.captureScreenshot", json!({"format": "png"}))),
        -32603
    );
    assert_eq!(
        err_code(&dispatch("Page.getFrameTree", json!({}))),
        -32603
    );

    // Param validation before the bridge boundary.
    assert_eq!(
        err_code(&dispatch("Page.setContent", json!({"html": ""}))),
        -32602
    );

    // Empty init-script registration is Chrome-compatible: fresh ids, no echo.
    let r = ok_result(&dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": ""}),
    ));
    let id1 = r["identifier"].as_str().expect("identifier").to_string();
    assert!(id1.starts_with("script"), "id: {}", id1);
    let r = ok_result(&dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": ""}),
    ));
    let id2 = r["identifier"].as_str().expect("identifier").to_string();
    assert_ne!(id1, id2, "identifiers must be fresh per registration");

    assert_eq!(err_code(&dispatch("Page.getNavigationHistory", json!({}))), -32000);
    assert_eq!(err_code(&dispatch("Page.bogus", json!({}))), -32601);
}

/// REQ-CDP-005 — DOM domain (stateless face).
#[test]
fn req_cdp_005_dom_domain_stateless_face() {
    ok_result(&dispatch("DOM.enable", json!({})));

    let r = ok_result(&dispatch("DOM.getDocument", json!({})));
    let root = &r["root"];
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["nodeType"], 9);
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["children"][0]["nodeName"], "HTML");

    let r = ok_result(&dispatch("DOM.querySelector", json!({"selector": "#a"})));
    assert_eq!(r["nodeId"], 0);
    let r = ok_result(&dispatch("DOM.querySelectorAll", json!({"selector": "div"})));
    assert_eq!(r["nodeIds"], json!([]));

    let r = ok_result(&dispatch("DOM.getBoxModel", json!({})));
    let content = r["model"]["content"].as_array().expect("content quad");
    assert_eq!(content.len(), 8);

    let r = ok_result(&dispatch("DOM.describeNode", json!({})));
    assert_eq!(r["node"]["nodeName"], "HTML");

    ok_result(&dispatch(
        "DOM.setAttributeValue",
        json!({"nodeId": 1, "name": "id", "value": "x"}),
    ));

    // Real outerHTML needs the live document — explicit error, not canned html.
    assert_eq!(err_code(&dispatch("DOM.getOuterHTML", json!({"nodeId": 1}))), -32603);
    assert_eq!(err_code(&dispatch("DOM.bogus", json!({}))), -32601);
}

/// REQ-CDP-006 — Network domain (stateless face).
#[test]
fn req_cdp_006_network_domain_stateless_face() {
    ok_result(&dispatch("Network.enable", json!({})));
    ok_result(&dispatch("Network.disable", json!({})));

    let r = ok_result(&dispatch("Network.getCookies", json!({"urls": ["https://example.com"]})));
    assert_eq!(r["cookies"], json!([]));
    let r = ok_result(&dispatch(
        "Network.setCookie",
        json!({"name": "k", "value": "v", "url": "https://example.com"}),
    ));
    assert_eq!(r["success"], true, "CDP SetCookieReturnObject shape");
    ok_result(&dispatch("Network.clearBrowserCache", json!({})));
    ok_result(&dispatch("Network.setCacheDisabled", json!({"cacheDisabled": true})));

    // Stored response bodies do not exist without the browser — explicit
    // error, never an empty-body fake success.
    assert_eq!(
        err_code(&dispatch("Network.getResponseBody", json!({"requestId": "r1"}))),
        -32603
    );
    // Per-target extra headers have no servo facility — explicit error.
    assert_eq!(
        err_code(&dispatch(
            "Network.setExtraHTTPHeaders",
            json!({"headers": {"X": "y"}})
        )),
        -32603
    );
    // Batch cookie setter requires its parameter.
    assert_eq!(err_code(&dispatch("Network.setCookies", json!({}))), -32602);
}

/// REQ-CDP-007 — CSS / Input / Emulation / Overlay domains (stateless face).
#[test]
fn req_cdp_007_css_input_emulation_overlay_domains() {
    ok_result(&dispatch("CSS.enable", json!({})));
    ok_result(&dispatch("CSS.disable", json!({})));

    ok_result(&dispatch(
        "Input.dispatchMouseEvent",
        json!({"type": "mousePressed", "x": 1.0, "y": 2.0, "button": 0, "clickCount": 1}),
    ));
    ok_result(&dispatch(
        "Input.dispatchKeyEvent",
        json!({"type": "keyDown", "key": "a", "code": "KeyA"}),
    ));
    ok_result(&dispatch("Input.insertText", json!({"text": "hi"})));
    assert_eq!(err_code(&dispatch("Input.bogus", json!({}))), -32601);

    ok_result(&dispatch(
        "Emulation.setDeviceMetricsOverride",
        json!({"width": 800, "height": 600, "deviceScaleFactor": 1.0, "mobile": false}),
    ));
    ok_result(&dispatch("Emulation.clearDeviceMetricsOverride", json!({})));
    ok_result(&dispatch(
        "Emulation.setUserAgentOverride",
        json!({"userAgent": "BaoTest/1.0"}),
    ));
    ok_result(&dispatch(
        "Emulation.setEmulatedMedia",
        json!({"media": "light"}),
    ));

    for m in [
        "Overlay.enable",
        "Overlay.highlightNode",
        "Overlay.hideHighlight",
        "Overlay.setPausedInDebuggerMessage",
    ] {
        ok_result(&dispatch(m, json!({})));
    }
    assert_eq!(err_code(&dispatch("Overlay.bogus", json!({}))), -32601);
}

/// REQ-CDP-008 — Target domain (stateless face): target discovery falls back
/// to the session's own target; session minting is owned by the bao_browser
/// WS registry and must refuse here (never a fabricated sessionId).
#[test]
fn req_cdp_008_target_domain_stateless_face() {
    let r = ok_result(&dispatch("Target.getTargets", json!({})));
    let infos = r["targetInfos"].as_array().expect("targetInfos");
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0]["targetId"], TID);
    assert_eq!(infos[0]["type"], "page");
    assert_eq!(infos[0]["attached"], true);
    assert_eq!(infos[0]["browserContextId"], "bao-default-context");

    let r = ok_result(&dispatch("Target.getTargetInfo", json!({})));
    assert_eq!(r["targetInfo"]["targetId"], TID);

    // Page creation/close need the real PagePool bridge — fail closed.
    assert_eq!(
        err_code(&dispatch("Target.createTarget", json!({"url": "about:blank"}))),
        -32603
    );
    assert_eq!(
        err_code(&dispatch("Target.closeTarget", json!({"targetId": TID}))),
        -32603
    );
    // Session-table commands: explicit not-supported with the owner named.
    assert_eq!(
        err_code(&dispatch("Target.attachToTarget", json!({"targetId": TID}))),
        -32000
    );
    ok_result(&dispatch("Target.setAutoAttach", json!({"autoAttach": true})));
    ok_result(&dispatch("Target.setDiscoverTargets", json!({"discover": true})));
    assert_eq!(err_code(&dispatch("Target.bogus", json!({}))), -32601);
}
