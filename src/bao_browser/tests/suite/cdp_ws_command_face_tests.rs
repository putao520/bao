// REQ-CDP WS command-face e2e: real WebSocket round-trips against a live
// BrowserRuntime + CdpServer wired through BaoWsRegistry (the production wiring
// run_browser performs). Asserts the Playwright-direct-connect minimal face:
// Target.getTargets / Target.attachToTarget(flatten) / Page.navigate /
// Runtime.evaluate all reach the real servo PagePool via the bridge.
// @trace REQ-CDP-001 [entity:CdpServer]
// @trace REQ-CDP-005 [req:REQ-CDP-005] [level:e2e]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BrowserRuntime, BaoWsRegistry, PageConfig};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::{bridge_channel, main_frame_id_for_target};
use bun_uws::ws_client::{RecvOutcome, WebSocketClient};
use cdp_server::{CdpServer, EventSender, ServerConfig};
use serde_json::{json, Value};

/// Bind a TcpListener to 127.0.0.1:0 to reserve an ephemeral port, then
/// release it for the CdpServer to bind (tiny race window, test-only).
fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct WsCdp {
    client: WebSocketClient,
    next_id: i64,
}

impl WsCdp {
    fn connect(url: &str) -> Self {
        // Bounded connect retry: under full-suite CPU load the CdpServer
        // thread can lag its bind behind the client's first connect, and
        // the reserve-and-release ephemeral port in pick_free_port() can be
        // momentarily taken by a concurrently-starting test process — a
        // one-shot connect loses that race (observed as ConnectionRefused,
        // BCE-20260824-CDPWS-CONNECT).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut client = None;
        while std::time::Instant::now() < deadline {
            match WebSocketClient::connect(url) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let mut client = client.expect("ws connect (bounded 10s retry exhausted)");
        client.set_read_timeout(Duration::from_secs(5));
        WsCdp {
            client,
            next_id: 1,
        }
    }

    /// Send a command and wait for the matching response id (events are
    /// skipped). Returns the full response object.
    fn send(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method });
        if !params.is_null() {
            msg["params"] = params;
        }
        self.client
            .send_text(&serde_json::to_string(&msg).unwrap())
            .expect("ws send");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                    // event or unrelated response — keep reading
                }
                RecvOutcome::Timeout => {
                    continue;
                }
                RecvOutcome::Closed => panic!("ws closed waiting for {method} response"),
            }
        }
        panic!("timeout waiting for {method} response");
    }

    /// Send a raw message object (carrying a sessionId) and wait for the
    /// matching response id.
    fn send_raw(&mut self, msg: Value) -> Value {
        let id = msg["id"].as_i64().unwrap();
        self.client
            .send_text(&serde_json::to_string(&msg).unwrap())
            .expect("ws send");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed waiting for raw response"),
            }
        }
        panic!("timeout waiting for raw response");
    }
}

/// The WS-client half of the e2e. Runs on a helper thread while the main
/// thread drives the servo event loop (BrowserRuntime holds Rc<Servo> — the loop
/// must stay on the thread that created it, exactly like run_browser).
fn client_phase(ws_url: String, page_id: usize, done: Arc<AtomicBool>) {
    let mut cdp = WsCdp::connect(&ws_url);

    // 1. Target.getTargets — real PagePool enumeration via the bridge.
    let resp = cdp.send("Target.getTargets", json!({}));
    assert!(
        resp.get("error").is_none(),
        "getTargets must succeed: {resp}"
    );
    let infos = resp["result"]["targetInfos"].as_array().expect("targetInfos");
    assert!(
        infos
            .iter()
            .any(|i| i["targetId"].as_str() == Some(&page_id.to_string())),
        "the real page id must be listed: {infos:?}"
    );

    // 2. Page.navigate — a real servo navigation must happen.
    let html = "<html><head><title>bao-ws-e2e</title></head><body><h1>ok</h1></body></html>";
    let url = format!("data:text/html;charset=utf-8,{html}");
    let resp = cdp.send("Page.navigate", json!({ "url": url }));
    assert!(resp.get("error").is_none(), "navigate must succeed: {resp}");
    // REQ-CDP-004 (v7 path B): the response frameId is the per-target
    // main-frame id — the same value every frame event for this target
    // carries — never the bare PageId (targetId namespace).
    let frame_id = resp["result"]["frameId"].as_str().expect("frameId");
    assert_eq!(
        frame_id,
        main_frame_id_for_target(&page_id.to_string()),
        "frameId = per-target event-stream main-frame id"
    );
    assert_ne!(
        frame_id,
        page_id.to_string(),
        "frameId must not be the PageId (targetId namespace)"
    );

    // 3. Runtime.evaluate — poll until the navigation landed, then assert
    //    the document title genuinely reflects the navigated document.
    //    Transient evaluate errors are expected mid-navigation (the old
    //    document is being torn down) — only the final state is asserted.
    let mut title = String::new();
    let mut last_error = Value::Null;
    for _ in 0..200 {
        let resp = cdp.send(
            "Runtime.evaluate",
            json!({ "expression": "document.title", "returnByValue": true }),
        );
        if let Some(err) = resp.get("error") {
            last_error = err.clone();
        } else if let Some(t) = resp["result"]["result"]["value"].as_str() {
            if t == "bao-ws-e2e" {
                title = t.to_string();
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        title, "bao-ws-e2e",
        "evaluate must observe the navigated document (last error: {last_error})"
    );

    // 4. Flattened session routing (the Playwright browser-connection mode):
    //    attach to the page target, then route a command through sessionId.
    let resp = cdp.send(
        "Target.attachToTarget",
        json!({ "targetId": page_id.to_string(), "flatten": true }),
    );
    assert!(
        resp.get("error").is_none(),
        "attachToTarget must succeed: {resp}"
    );
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    assert!(!session_id.is_empty());

    let flat = cdp.send_raw(json!({
        "id": cdp.next_id,
        "method": "Runtime.evaluate",
        "params": { "expression": "1 + 41", "returnByValue": true },
        "sessionId": session_id,
    }));
    cdp.next_id += 1;
    assert!(
        flat.get("error").is_none(),
        "flat evaluate must succeed: {flat}"
    );
    assert_eq!(
        flat["result"]["result"]["value"], 42,
        "sessionId routing reaches the page"
    );

    // 5. Explicit-error contract through the WS face: Fetch.enable must
    //    surface the no-interception-facility error, never a canned ok.
    let resp = cdp.send("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*" }] }));
    let err = resp["error"]
        .as_object()
        .expect("Fetch.enable must fail explicitly");
    assert_eq!(err["code"], -32000);
    assert!(err["message"]
        .as_str()
        .unwrap()
        .contains("no request interception facility"));

    done.store(true, Ordering::Relaxed);
}

/// REQ-CDP Runtime object protocol e2e: the callFunctionOn/objectId/release
/// surface Playwright's page.evaluate drives — evaluateHandle (rbv=false)
/// mints objectIds, callFunctionOn calls a stringized function on/with those
/// objects ({value}/{objectId}/{unserializableValue} args), awaitPromise
/// resolves, getProperties roundtrips nested objectIds, and releaseObject/
/// releaseObjectGroup drop registry entries.
fn object_protocol_phase(ws_url: String, done: Arc<AtomicBool>) {
    let mut cdp = WsCdp::connect(&ws_url);

    // Navigate to a stable document first (evaluate needs a live realm).
    let html = "<html><body><div id='d'>ok</div></body></html>";
    let url = format!("data:text/html;charset=utf-8,{html}");
    let resp = cdp.send("Page.navigate", json!({ "url": url }));
    assert!(resp.get("error").is_none(), "navigate must succeed: {resp}");

    // Wait for the NAVIGATED document to exist — gate on content only the
    // data: document can have (`#d`). `!!document.body` is a weak gate: the
    // initial about:blank placeholder also has a body, so under load the
    // gate passes on the placeholder and the navigation commit then swaps
    // the document mid-protocol — the marker/object registry dies with it
    // and callFunctionOn resolves `this` to the fresh global (BCE-20260824-
    // CDPWS-PLACEHOLDER-GATE; objectIds surviving navigation is not a CDP
    // promise, executionContextsCleared/…Created announce the swap).
    let mut ready = false;
    for _ in 0..200 {
        let r = cdp.send(
            "Runtime.evaluate",
            json!({ "expression": "!!document.getElementById('d')", "returnByValue": true }),
        );
        if r["result"]["result"]["value"] == json!(true) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "document never became ready");

    // 1. evaluateHandle (returnByValue=false) → RemoteObject with objectId.
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "({answer: 42})", "returnByValue": false }),
    );
    assert!(resp.get("error").is_none(), "evaluateHandle: {resp}");
    let oid1 = resp["result"]["result"]["objectId"]
        .as_str()
        .expect("evaluateHandle must return an objectId")
        .to_string();
    assert_eq!(resp["result"]["result"]["type"], "object");
    assert_eq!(resp["result"]["result"]["className"], "Object");
    assert!(resp["result"]["exceptionDetails"].is_null());

    // 2. callFunctionOn with objectId `this` + {value} arg, returnByValue.
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "objectId": oid1,
            "functionDeclaration": "function(bump) { return this.answer + bump; }",
            "arguments": [{ "value": 10 }],
            "returnByValue": true,
        }),
    );
    assert!(resp.get("error").is_none(), "callFunctionOn value arg: {resp}");
    assert_eq!(resp["result"]["result"]["value"], 52, "full resp: {resp}");
    assert!(resp["result"]["exceptionDetails"].is_null());

    // 3. {objectId} argument roundtrip: a second handle passed as an arg.
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "({x: 100})", "returnByValue": false }),
    );
    let oid2 = resp["result"]["result"]["objectId"]
        .as_str()
        .expect("second objectId")
        .to_string();
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "objectId": oid1,
            "functionDeclaration": "function(other) { return this.answer + other.x; }",
            "arguments": [{ "objectId": oid2 }],
            "returnByValue": true,
        }),
    );
    assert!(
        resp.get("error").is_none(),
        "callFunctionOn objectId arg: {resp}"
    );
    assert_eq!(resp["result"]["result"]["value"], 142);

    // 4. unserializableValue arg (NaN).
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "objectId": oid1,
            "functionDeclaration": "function(v) { return v !== v ? 'nan' : 'not-nan'; }",
            "arguments": [{ "unserializableValue": "NaN" }],
            "returnByValue": true,
        }),
    );
    assert!(resp.get("error").is_none(), "NaN arg: {resp}");
    assert_eq!(resp["result"]["result"]["value"], "nan");

    // 5. awaitPromise (executionContextId form, this=undefined).
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "functionDeclaration": "function() { return Promise.resolve(7).then(function(v) { return v * 6; }); }",
            "arguments": [],
            "returnByValue": true,
            "awaitPromise": true,
        }),
    );
    assert!(resp.get("error").is_none(), "awaitPromise: {resp}");
    assert_eq!(resp["result"]["result"]["value"], 42);

    // 6. Thrown exception → exceptionDetails, never a fake ok.
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "objectId": oid1,
            "functionDeclaration": "function() { throw new Error('boom-cdp'); }",
            "returnByValue": true,
        }),
    );
    assert!(resp.get("error").is_none(), "throw path: {resp}");
    let text = resp["result"]["exceptionDetails"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("boom-cdp"),
        "exceptionDetails must carry the message, got: {resp}"
    );

    // 7. getProperties roundtrip: nested object values carry real objectIds.
    let resp = cdp.send(
        "Runtime.getProperties",
        json!({ "objectId": oid1, "ownProperties": true }),
    );
    assert!(resp.get("error").is_none(), "getProperties: {resp}");
    let props = resp["result"]["result"]
        .as_array()
        .expect("properties array");
    let answer = props
        .iter()
        .find(|p| p["name"] == json!("answer"))
        .expect("answer property present");
    assert_eq!(answer["value"]["type"], "number");
    assert_eq!(answer["value"]["value"], 42);

    // 8. releaseObject drops the entry: a follow-up getProperties sees an
    //    unregistered object (empty result), proving the release landed.
    let resp = cdp.send(
        "Runtime.releaseObject",
        json!({ "objectId": oid2 }),
    );
    assert!(resp.get("error").is_none(), "releaseObject: {resp}");
    let resp = cdp.send(
        "Runtime.getProperties",
        json!({ "objectId": oid2, "ownProperties": true }),
    );
    assert!(resp.get("error").is_none());
    assert_eq!(
        resp["result"]["result"], json!([]),
        "released objectId must no longer resolve"
    );

    // 9. releaseObjectGroup: entries minted under objectGroup are freed
    //    together (the Playwright context-teardown path).
    let resp = cdp.send(
        "Runtime.callFunctionOn",
        json!({
            "objectId": oid1,
            "functionDeclaration": "function() { return {tag: 'grouped'}; }",
            "returnByValue": false,
            "objectGroup": "e2e-group",
        }),
    );
    let grouped_oid = resp["result"]["result"]["objectId"]
        .as_str()
        .expect("grouped objectId")
        .to_string();
    let resp = cdp.send(
        "Runtime.releaseObjectGroup",
        json!({ "objectGroup": "e2e-group" }),
    );
    assert!(resp.get("error").is_none(), "releaseObjectGroup: {resp}");
    let resp = cdp.send(
        "Runtime.getProperties",
        json!({ "objectId": grouped_oid, "ownProperties": true }),
    );
    assert_eq!(
        resp["result"]["result"], json!([]),
        "released group entries must no longer resolve"
    );

    done.store(true, Ordering::Relaxed);
}

#[test]
fn ws_command_face_page_navigate_and_evaluate_roundtrip() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<bao_cdp_client::bridge::ServoEvent>();
    runtime.set_event_channel(event_tx);

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let port = pick_free_port();
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry);
    server.set_target_provider(Arc::new(ServoTargetProvider::new(
        bridge_tx,
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    let broadcaster = server.broadcaster();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    let done = Arc::new(AtomicBool::new(false));
    let page_id = page.id();
    let client = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || client_phase(ws_url, page_id, done))
    };

    // Main thread: the run_with_bridge loop shape (servo spin + bridge drain
    // + event translation), bounded by the client's done flag.
    use bao_cdp_client::bridge::translate;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            for cdp_event in translate(servo_event) {
                broadcaster.send_event(&cdp_event.method, cdp_event.params);
            }
        }
        std::thread::yield_now();
    }

    client.join().expect("client phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "client phase must have completed all assertions"
    );
}

/// Same dual-thread harness as the navigate/evaluate roundtrip, driving the
/// Runtime object-protocol client phase (evaluateHandle / callFunctionOn /
/// getProperties / releaseObject / releaseObjectGroup over live WS).
#[test]
fn ws_runtime_object_protocol_roundtrip() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(60));
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<bao_cdp_client::bridge::ServoEvent>();
    runtime.set_event_channel(event_tx);

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let port = pick_free_port();
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry);
    server.set_target_provider(Arc::new(ServoTargetProvider::new(
        bridge_tx,
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    let broadcaster = server.broadcaster();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    let done = Arc::new(AtomicBool::new(false));
    let client = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || object_protocol_phase(ws_url, done))
    };

    // Main thread: the run_with_bridge loop shape (servo spin + bridge drain
    // + event translation), bounded by the client's done flag. The await-
    // Promise path polls from inside the bridge handler; each poll's
    // evaluate spins this same servo loop via spin_servo, so pending promise
    // chains progress even while the drain call is blocked.
    use bao_cdp_client::bridge::translate;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            for cdp_event in translate(servo_event) {
                broadcaster.send_event(&cdp_event.method, cdp_event.params);
            }
        }
        std::thread::yield_now();
    }

    client.join().expect("object protocol phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "object protocol phase must have completed all assertions"
    );
}

/// Regression for the ws_registry unit face: the bridge-less dispatch of a
/// Fetch command stays an explicit error and page-session commands carry the
/// WS session's target (also covered by ws_registry unit tests).
#[test]
fn ws_registry_fetch_explicit_error_and_target_routing_unit() {
    let (tx, rx) = bridge_channel(Duration::from_millis(200));
    let keeper = tx.clone();
    std::thread::spawn(move || {
        let _keeper = keeper;
        loop {
            let handled = rx.try_process(|cmd| match cmd {
                bao_cdp::servo_bridge::BridgeCommand::EvaluateJs { .. } => {
                    bao_cdp::servo_bridge::BridgeResponse {
                        result: Ok(json!({
                            "result": { "type": "number", "value": 7 }
                        })),
                    }
                }
                _ => bao_cdp::servo_bridge::BridgeResponse {
                    result: Ok(json!({})),
                },
            });
            if !handled {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });

    let registry = BaoWsRegistry::new(tx);
    // The registry is consumed as Arc<dyn RegistryDispatch> by CdpServer —
    // exercise the same dispatch surface.
    let dispatch: Arc<dyn cdp_server::RegistryDispatch> = Arc::new(registry);
    let msg = cdp_server::CdpMessage {
        id: Some(1),
        method: "Runtime.evaluate".into(),
        params: Some(json!({"expression": "7"})),
        session_id: None,
    };
    struct Nop;
    impl cdp_server::EventSender for Nop {
        fn send_event(&self, _: &str, _: Value) {}
    }
    let r = dispatch
        .dispatch_message(&msg, "42", &Nop)
        .expect("dispatch must route the domain")
        .expect("evaluate must succeed");
    assert_eq!(r["result"]["value"], 7);

    let fetch = cdp_server::CdpMessage {
        id: Some(2),
        method: "Fetch.enable".into(),
        params: Some(json!({"patterns": []})),
        session_id: None,
    };
    let err = dispatch
        .dispatch_message(&fetch, "42", &Nop)
        .expect("Fetch domain is served")
        .expect_err("Fetch.enable must fail");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("no request interception facility"));
}

/// ISSUE #20 (B: "CDP page evaluate 默认遵循 Page Realm 权限"): a
/// Runtime.evaluate issued through the CDP WS face must execute in the
/// PAGE realm — the Node/Bun host capability names must NOT be reachable,
/// while the web face is provably alive (so the "undefined" answers cannot
/// come from an empty/broken realm). Same dual-thread harness as the
/// navigate/evaluate roundtrip above.
// @trace REQ-CDP-005 [req:REQ-CDP-005] [level:e2e] [issue:20]
#[test]
fn ws_cdp_evaluate_stays_in_page_realm() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<bao_cdp_client::bridge::ServoEvent>();
    runtime.set_event_channel(event_tx);

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let port = pick_free_port();
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry);
    server.set_target_provider(Arc::new(ServoTargetProvider::new(
        bridge_tx,
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    let broadcaster = server.broadcaster();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    let done = Arc::new(AtomicBool::new(false));
    let client = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || page_realm_evaluate_phase(ws_url, done))
    };

    use bao_cdp_client::bridge::translate;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            for cdp_event in translate(servo_event) {
                broadcaster.send_event(&cdp_event.method, cdp_event.params);
            }
        }
        std::thread::yield_now();
    }

    client.join().expect("page-realm evaluate phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "page-realm evaluate phase must have completed all assertions"
    );
}

/// WS-client half: every CDP-issued typeof probe for a host capability must
/// answer "undefined" on the page realm, while the web face answers.
fn page_realm_evaluate_phase(ws_url: String, done: Arc<AtomicBool>) {
    let mut cdp = WsCdp::connect(&ws_url);

    // Wait for a live web face first (same poll shape as the roundtrip's
    // title poll): proves the realm answers at all before pinning denials.
    let mut web_alive = false;
    for _ in 0..200 {
        let resp = cdp.send(
            "Runtime.evaluate",
            json!({ "expression": "typeof document", "returnByValue": true }),
        );
        if resp["result"]["result"]["value"] == json!("object") {
            web_alive = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(web_alive, "CDP evaluate must reach a live page realm (typeof document)");

    // The capability denials: Node/Bun host names unreachable via CDP.
    for expr in ["require", "process", "Buffer", "Bun", "module", "__dirname"] {
        let resp = cdp.send(
            "Runtime.evaluate",
            json!({ "expression": format!("typeof {expr}"), "returnByValue": true }),
        );
        assert!(
            resp.get("error").is_none(),
            "typeof {expr} must evaluate cleanly: {resp}"
        );
        assert_eq!(
            resp["result"]["result"]["value"],
            json!("undefined"),
            "CDP evaluate must NOT expose host capability '{expr}' on the page realm"
        );
    }

    // The web face stays functional under the same CDP session.
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "typeof fetch", "returnByValue": true }),
    );
    assert_eq!(
        resp["result"]["result"]["value"],
        json!("function"),
        "page web face must stay reachable via CDP evaluate"
    );

    done.store(true, Ordering::Relaxed);
}

// ─── Target 路由波 probe (REQ-CDP-004): multi-page phantom-frame containment ──
//
// v7 波实证的幻影帧类缺陷:先切 per-target frame id 而事件仍全广播时,
// puppeteer 的 FrameManager 会把 A 页的 frameNavigated 误读为 B 页主 frame
// (isMainFrame-unknown-id 分支造出幻影帧)。本探针是它的机械判据:两页各
// attach 一个 flattened session(真实 connect 形态),真实导航 A 页后,B
// session 的 tagged 事件流(FrameManager 的全部输入)必须零收 A 的任何
// frame 事件,同时 A session 收到的 frame 事件携带 per-target 主 frame id。

/// Event sender that records every delivery face separately: untagged
/// broadcasts, and per-flattened-session tagged events (the stream a
/// client-side FrameManager consumes for that target).
#[derive(Default)]
struct RecordingSender {
    untagged: std::sync::Mutex<Vec<(String, Value)>>,
    tagged: std::sync::Mutex<Vec<(String, String, Value)>>,
}

impl RecordingSender {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl EventSender for RecordingSender {
    fn send_event(&self, method: &str, params: Value) {
        self.untagged
            .lock()
            .unwrap()
            .push((method.to_string(), params));
    }
    fn send_session_event(&self, session_id: &str, method: &str, params: Value) {
        self.tagged.lock().unwrap().push((
            session_id.to_string(),
            method.to_string(),
            params,
        ));
    }
}

// @trace TEST-CDP-004 [req:REQ-CDP-004] [level:e2e]
#[test]
fn multi_page_target_routing_phantom_frame_probe() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    // Two real pages — each stamps its own CDP target at creation.
    let page_a = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page a");
    let page_b = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page b");
    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<bao_cdp_client::bridge::ServoEvent>();
    runtime.set_event_channel(event_tx);

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let recorder = RecordingSender::new();
    let browser_target = "__browser__";
    use cdp_server::{CdpMessage, RegistryDispatch};

    // One flattened session per page (the Puppeteer/Playwright connect
    // shape: browser-endpoint attach, one session per target).
    let attach = |target: usize| -> String {
        registry
            .dispatch_message(
                &CdpMessage {
                    id: Some(1),
                    method: "Target.attachToTarget".into(),
                    params: Some(json!({
                        "targetId": target.to_string(),
                        "flatten": true
                    })),
                    session_id: None,
                },
                browser_target,
                &*recorder,
            )
            .expect("attach dispatch")
            .expect("attach ok")["sessionId"]
            .as_str()
            .expect("minted sessionId")
            .to_string()
    };
    let sid_a = attach(page_a.id());
    let sid_b = attach(page_b.id());
    assert_ne!(sid_a, sid_b, "each page attaches its own session");

    // Navigate page A through ITS session — a real servo navigation. The
    // command's bridge round-trip is answered by the pump loop below, so the
    // dispatch itself runs on a helper thread (the anchor-test shape: the
    // !Send runtime stays on the main thread, the dispatch waits on the
    // bridge concurrently).
    let marker_a = "probe-page-a-marker";
    let nav_url = format!(
        "data:text/html;charset=utf-8,<html><head><title>{marker_a}</title></head><body>{marker_a}</body></html>"
    );
    let nav_thread = {
        let registry = Arc::clone(&registry);
        let recorder = Arc::clone(&recorder);
        let sid_a = sid_a.clone();
        std::thread::spawn(move || {
            registry
                .dispatch_message(
                    &CdpMessage {
                        id: Some(2),
                        method: "Page.navigate".into(),
                        params: Some(json!({ "url": nav_url })),
                        session_id: Some(sid_a),
                    },
                    "__browser__",
                    &*recorder,
                )
                .expect("navigate dispatch")
                .expect("navigate ok")
        })
    };

    // Pump loop (the run_with_bridge shape): the pump answers the navigate's
    // bridge round-trip while it runs, and real delegate events route through
    // broadcast_for_target with their real target tags. Wait until page A's
    // load completes (frameStoppedLoading only comes from the real delegate
    // path — the synthetic navigate emission never emits it).
    use bao_cdp_client::bridge::translate;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            let target = servo_event.target_id().to_string();
            for cdp_event in translate(servo_event) {
                registry.broadcast_for_target(
                    recorder.as_ref(),
                    &target,
                    &cdp_event.method,
                    cdp_event.params,
                );
            }
        }
        let a_loaded = recorder
            .tagged
            .lock()
            .unwrap()
            .iter()
            .any(|(sid, m, _)| sid == &sid_a && m == "Page.frameStoppedLoading");
        if a_loaded && nav_thread.is_finished() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "page A never completed its load (no real delegate frameStoppedLoading routed to its session)"
        );
        std::thread::yield_now();
    }
    let nav = nav_thread.join().expect("navigate thread must not panic");
    assert_eq!(
        nav["frameId"],
        main_frame_id_for_target(&page_a.id().to_string()),
        "navigate response carries A's per-target main frame id"
    );

    let tagged = recorder.tagged.lock().unwrap();
    let a_events: Vec<&(String, String, Value)> =
        tagged.iter().filter(|(s, _, _)| s == &sid_a).collect();

    // 1. A received the navigation's frame lifecycle on its own session,
    //    carrying A's per-target main frame id.
    let a_nav = a_events
        .iter()
        .find(|(_, m, _)| m == "Page.frameNavigated")
        .expect("A session must receive Page.frameNavigated");
    assert_eq!(
        a_nav.2["frame"]["id"],
        main_frame_id_for_target(&page_a.id().to_string()),
        "frame id is A's per-target main frame id"
    );
    assert!(
        a_events
            .iter()
            .any(|(_, m, _)| m == "Page.frameStoppedLoading"),
        "A session receives its real delegate load-complete events"
    );

    // 2. CONTAINMENT (the phantom-frame mechanical criterion): B's tagged
    //    stream — everything its FrameManager could consume — carries ZERO
    //    events carrying A's identity (A's frame id / A's navigation URL).
    //    B legitimately receives ITS OWN frame events (its initial
    //    about:blank load) — those are targeted delivery working, not
    //    pollution; every frame event on B's stream must carry B's own
    //    per-target frame id.
    let frame_methods = [
        "Page.frameNavigated",
        "Page.frameStartedLoading",
        "Page.frameStoppedLoading",
        "Page.lifecycleEvent",
        "Page.loadEventFired",
    ];
    let a_frame_id = main_frame_id_for_target(&page_a.id().to_string());
    let b_frame_id = main_frame_id_for_target(&page_b.id().to_string());
    let leaked: Vec<&(String, String, Value)> = tagged
        .iter()
        .filter(|(s, m, p)| {
            s == &sid_b
                && frame_methods.contains(&m.as_str())
                && (p["frameId"].as_str() == Some(a_frame_id.as_str())
                    || p["frame"]["id"].as_str() == Some(a_frame_id.as_str())
                    || p["params"]["frameId"].as_str() == Some(a_frame_id.as_str()))
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "B session (FrameManager intake) must receive ZERO frame events carrying A's identity; leaked: {leaked:?}"
    );
    // Positive face: every frame event B receives carries B's own frame id.
    let foreign: Vec<&(String, String, Value)> = tagged
        .iter()
        .filter(|(s, m, p)| {
            s == &sid_b
                && frame_methods.contains(&m.as_str())
                && p["frameId"].as_str().is_some_and(|f| f != b_frame_id)
                && p["frame"]["id"].as_str().is_none()
        })
        .collect();
    assert!(
        foreign.is_empty(),
        "B session frame events must carry B's own per-target frame id; foreign: {foreign:?}"
    );

    // 3. No untagged broadcast degradation either (miss branch is a drop).
    assert!(
        recorder.untagged.lock().unwrap().is_empty(),
        "targeted routing must never degrade to the untagged broadcast"
    );
}
