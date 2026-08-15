// REQ-CDP WS command-face e2e: real WebSocket round-trips against a live
// BaoRuntime + CdpServer wired through BaoWsRegistry (the production wiring
// run_browser performs). Asserts the Playwright-direct-connect minimal face:
// Target.getTargets / Target.attachToTarget(flatten) / Page.navigate /
// Runtime.evaluate all reach the real servo PagePool via the bridge.
// @trace REQ-CDP-001 [entity:CdpServer]
// @trace REQ-CDP-005 [req:REQ-CDP-005] [level:e2e]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BaoRuntime, BaoWsRegistry, PageConfig};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
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
        let mut client = WebSocketClient::connect(url).expect("ws connect");
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
/// thread drives the servo event loop (BaoRuntime holds Rc<Servo> — the loop
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
    let frame_id = resp["result"]["frameId"].as_str().expect("frameId");
    assert_eq!(frame_id, page_id.to_string(), "frameId = real page id");

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

#[test]
fn ws_command_face_page_navigate_and_evaluate_roundtrip() {
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let (event_subscriber, servo_event_rx) = bao_cdp_client::bridge::EventSubscriber::new();
    runtime.set_event_channel(event_subscriber.sender());

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
