// REQ-CDP-006 C6 — Fetch request interception LIVE-ENGINE acceptance
// (design /tmp/cdp-006-interception-design.md §4 event order + §6 "真引擎内"
// half — the half e235 honestly could not reach from its crate: bao_cdp_client
// tests carry no bao_browser dependency, so the engine side of the chain was
// structurally unreachable there. This file is the same-shape twin of the
// overlay chain's Part B, overlay_conformance_tests).
//
// Driver: the wire a real client speaks — CDP over WebSocket against the
// production CdpServer + real servo (harness shape of
// playwright_full_flow_loop_tests / overlay_conformance_tests). The FULL
// interception chain is exercised live, no mock anywhere:
//
//   wire  Fetch.enable{patterns} → bridge → page interception slot
//   page  servo net → embedder round trip (pump drain) → delegate
//         load_web_resource → WebResourceLoad held → wire
//         Fetch.requestPaused (real request fields)
//   wire  Fetch.continueRequest / fulfillRequest / Fetch.disable → bridge →
//         fetch_resolve → servo answers the held load → net proceeds
//
// Three cases (design §8 closed loop + §3.2 verbs):
//   1. pause + continue — requestPaused carries the real request; the
//      ORIGINAL request completes over the real network (200 + origin body,
//      page-visible); design §4 structural order holds on the wire:
//      requestWillBeSent < requestPaused < responseReceived.
//   2. fulfill — a synthetic response (status / headers / base64 body)
//      downlinks to the page; body ≠ origin body proves the response did
//      not traverse the network.
//   3. disable — drains the pending pause (the paused request proceeds, not
//      wedged) and subsequent matching requests flow with zero pauses.
//
// Honest boundaries on the asserted face:
// - Network.loadingFinished is NOT asserted here: since the live-path
//   terminal taps landed (bb5087139), ServoEvent::NetworkLoadingFinish has a
//   producer, but asserting completion on THIS file's interception-minted
//   requestIds is out of its scope — completion is proven by
//   requestWillBeSent + responseReceived + the page-visible 200 + origin
//   body instead (network_loading_finish_live_tests.rs owns the terminal
//   event coverage).
// - requestPaused.requestId is the interception face's own minted serial
//   ("cdpreq-*"), NOT the net tap's servo RequestId — despite the event
//   translator's docstring claiming a shared source (design §4 wording).
//   Correlation here is by URL; equality between the two id spaces is not
//   asserted (it does not hold on this wire).
// @trace REQ-CDP-006 [domain:Fetch] [level:e2e]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bao_browser::{handle_bridge_command, BaoConfig, BrowserRuntime, BaoWsRegistry, PageConfig, PageHandle};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::{bridge_channel, BridgeReceiver};
use bao_cdp_client::bridge::{translate, ServoEvent};
use bun_uws::ws_client::{RecvOutcome, WebSocketClient};
use cdp_server::{CdpServer, EventBroadcaster, EventSender, ServerConfig};
use serde_json::{json, Value};

/// The closed resourceType wire set (bao_browser's destination mapping —
/// `webview_state.rs destination_debug_name_to_resource_type`; a value
/// outside this set would mean the mapping drifted).
const RESOURCE_TYPES: [&str; 9] = [
    "Document",
    "Script",
    "Stylesheet",
    "Image",
    "Font",
    "Media",
    "Manifest",
    "Fetch",
    "Other",
];

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Loopback origin serving the test document and the interception targets:
/// `/page` → the HTML document; `/net/<name>` → 200 text/plain with body
/// `origin-body-<name>` (the body the page must see when a request really
/// traversed the network — the discriminator against synthesized responses).
fn spawn_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split(' ').nth(1).unwrap_or("").to_string();
            let (ctype, body) = if path.starts_with("/net/") {
                let name = path.trim_start_matches("/net/");
                ("text/plain", format!("origin-body-{name}"))
            } else {
                (
                    "text/html",
                    "<html><head><title>fetch-live</title></head><body><div id=mark>live</div></body></html>"
                        .to_string(),
                )
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    port
}

// ─────────────────────────────────────────────────────────────────────────
// Production stack + pump (the engine half of the oracle)
// ─────────────────────────────────────────────────────────────────────────

struct LiveFixture {
    runtime: BrowserRuntime,
    page: PageHandle,
    bridge_rx: BridgeReceiver,
    servo_event_rx: bao_ipc::Receiver<ServoEvent>,
    event_router: Arc<BaoWsRegistry>,
    broadcaster: Arc<EventBroadcaster>,
    ws_url: String,
}

impl LiveFixture {
    /// The full production stack: real servo runtime + page, bridge channel,
    /// event channel, CdpServer on a free port (harness shape of
    /// playwright_full_flow_loop_tests). Err = runtime init failed (e.g.
    /// headless-GL environment) — the caller loud-skips, suite §9 precedent.
    fn spawn() -> Result<LiveFixture, String> {
        let runtime = BrowserRuntime::new(BaoConfig::default()).map_err(|e| e.to_string())?;
        let page = runtime
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(60));
        let (event_tx, servo_event_rx) = bao_ipc::unbounded::<ServoEvent>();
        runtime.set_event_channel(event_tx);
        let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
        let event_router = Arc::clone(&registry);
        let port = pick_free_port();
        let mut server = CdpServer::with_registry(
            ServerConfig::builder().host("127.0.0.1").port(port).build(),
            registry,
        );
        server.set_target_provider(Arc::new(ServoTargetProvider::new(
            bridge_tx.clone(),
            page.id().to_string(),
            "127.0.0.1".into(),
            port,
        )));
        let broadcaster = server.broadcaster();
        std::thread::spawn(move || {
            let _ = server.run();
        });
        let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());
        Ok(LiveFixture {
            runtime,
            page,
            bridge_rx,
            servo_event_rx,
            event_router,
            broadcaster,
            ws_url,
        })
    }

    /// The main-thread pump loop (the only thread allowed to touch
    /// Rc<Servo>): spin the servo event loop (this drains the net→embedder
    /// WebResourceRequested round trip — the pause point of the whole
    /// feature), drain bridge commands (enable/resolve reach the page state
    /// here), and pump servo events → CDP events → WS broadcast, until the
    /// client thread reports its result.
    fn pump_until(
        self,
        client: JoinHandle<()>,
        rx: Receiver<Result<(), String>>,
        label: &str,
    ) -> Result<(), String> {
        let LiveFixture {
            runtime,
            page,
            bridge_rx,
            servo_event_rx,
            event_router,
            broadcaster,
            ..
        } = self;
        let deadline = Instant::now() + Duration::from_secs(180);
        let result = loop {
            runtime.spin_event_loop();
            bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
            while let Ok(servo_event) = servo_event_rx.try_recv() {
                for cdp_event in translate(servo_event) {
                    match cdp_event.session_id.clone() {
                        Some(target) if !target.is_empty() => event_router.broadcast_for_target(
                            broadcaster.as_ref(),
                            &target,
                            &cdp_event.method,
                            cdp_event.params,
                        ),
                        _ => broadcaster.send_event(&cdp_event.method, cdp_event.params),
                    }
                }
            }
            match rx.try_recv() {
                Ok(r) => break r,
                Err(TryRecvError::Disconnected) => {
                    if let Err(payload) = client.join() {
                        std::panic::resume_unwind(payload);
                    }
                    panic!("[{label}] client thread ended without a result or panic");
                }
                Err(TryRecvError::Empty) => {}
            }
            if Instant::now() > deadline {
                panic!("[{label}] acceptance client did not finish within 180s");
            }
            std::thread::yield_now();
        };
        let _ = client.join();
        runtime.page_pool().close_page(page.id()).expect("close page");
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────
// WS client (the same wire a real client speaks)
// ─────────────────────────────────────────────────────────────────────────

struct WsCdp {
    client: WebSocketClient,
    next_id: i64,
    /// Every event frame seen, in wire arrival order — buffered (not
    /// dropped): a discarded event edge is unrecoverable on the wire
    /// (playwright_full_flow_loop_tests e124 note). This buffer is also the
    /// ordered log the §4 event-order assertions scan.
    pending_events: Vec<Value>,
}

/// The request/response URL an event frame carries (the three param shapes
/// on this wire: `request.url` / `response.url` / bare `url`).
fn event_url(v: &Value) -> Option<&str> {
    v["params"]["request"]["url"]
        .as_str()
        .or_else(|| v["params"]["response"]["url"].as_str())
        .or_else(|| v["params"]["url"].as_str())
}

fn event_matches(v: &Value, method: &str, url_hint: &str) -> bool {
    v.get("method").and_then(|m| m.as_str()) == Some(method)
        && event_url(v).is_some_and(|u| u.contains(url_hint))
}

impl WsCdp {
    fn connect(url: &str) -> Self {
        // Bounded connect retry (full-suite load can lag the server's bind
        // behind the client's first connect — BCE-20260824-CDPWS-CONNECT).
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut client = None;
        while Instant::now() < deadline {
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
            pending_events: Vec::new(),
        }
    }

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
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                    if v.get("method").is_some() && self.pending_events.len() < 2000 {
                        self.pending_events.push(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed waiting for {method} response"),
            }
        }
        panic!("timeout waiting for {method} response");
    }

    /// Buffered index of the first event matching (method, url_hint).
    fn buffered_index(&self, method: &str, url_hint: &str) -> Option<usize> {
        self.pending_events
            .iter()
            .position(|v| event_matches(v, method, url_hint))
    }

    /// Bounded pump-driven wait for one event frame: the harness buffer
    /// first (the edge may already have been consumed during a command
    /// response wait), then the wire. Every frame seen along the way —
    /// including the matched one — lands in the buffer in arrival order, so
    /// later ordering assertions scan one complete ordered log.
    fn wait_event(&mut self, method: &str, url_hint: &str, timeout: Duration) -> Option<Value> {
        if let Some(i) = self.buffered_index(method, url_hint) {
            return Some(self.pending_events[i].clone());
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    let matched = event_matches(&v, method, url_hint);
                    if v.get("method").is_some() && self.pending_events.len() < 2000 {
                        self.pending_events.push(v.clone());
                    }
                    if matched {
                        return Some(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => return None,
            }
        }
        None
    }
}

fn send(t: &mut WsCdp, method: &str, params: Value) -> Value {
    t.send(method, params)
}

/// Runtime.evaluate helper: unwrapped `result.result` value as a string.
fn eval_js(t: &mut WsCdp, expr: &str) -> String {
    let r = send(
        t,
        "Runtime.evaluate",
        json!({ "expression": expr, "returnByValue": true }),
    );
    assert!(r.get("error").is_none(), "evaluate error: {r} ({expr})");
    r["result"]["result"]["value"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Poll `expr` until it evaluates to `want` (the page-driven settle gate —
/// content only the real document has, never a weak readyState probe).
fn settle(t: &mut WsCdp, expr: &str, want: &str) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..100 {
        last = eval_js(t, expr);
        if last == want {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("never settled (last={last:?}, want={want:?})"))
}

// ─────────────────────────────────────────────────────────────────────────
// Client-side scenario helpers
// ─────────────────────────────────────────────────────────────────────────

/// Enable the three domains the interception face needs — the CdpServer
/// session pump gates event delivery per enabled domain, so each is a real
/// precondition: `Network.enable` installs the process net tap
/// (requestWillBeSent/responseReceived), `Fetch.enable` arms the
/// interception slot (requestPaused).
fn enable_domains(t: &mut WsCdp, url_pattern: String) -> Result<(), String> {
    for (method, params) in [
        ("Page.enable", json!({})),
        ("Network.enable", json!({})),
        ("Fetch.enable", json!({ "patterns": [{ "urlPattern": url_pattern }] })),
    ] {
        let r = send(t, method, params);
        if r.get("error").is_some() {
            return Err(format!("{method}: {r}"));
        }
    }
    Ok(())
}

/// Navigate to the loopback document and settle on content only the real
/// document has. Navigation happens BEFORE `Fetch.enable` arming in every
/// scenario, so the document request itself is never intercepted.
fn navigate_and_settle(t: &mut WsCdp, origin: u16) -> Result<(), String> {
    let r = send(
        t,
        "Page.navigate",
        json!({ "url": format!("http://127.0.0.1:{origin}/page") }),
    );
    if r.get("error").is_some() {
        return Err(format!("navigate /page: {r}"));
    }
    settle(
        t,
        "(function(){var m=document.getElementById('mark');return m?m.textContent:'none';})()",
        "live",
    )
    .map_err(|e| format!("page settle: {e}"))
}

/// Arm a page-side fetch to `http://127.0.0.1:{origin}/net/{name}` and park
/// the outcome in `globalThis.__r_{name}` as {status, body, hdr}. Status is
/// `"pending"` until the fetch settles; `"rejected"` on network failure.
fn arm_page_fetch(t: &mut WsCdp, origin: u16, name: &str) {
    let js = format!(
        "(function(){{ var k='__r_{name}'; \
         globalThis[k]={{status:'pending',body:'',hdr:''}}; \
         fetch('http://127.0.0.1:{origin}/net/{name}') \
           .then(function(r){{ var st=r.status; var h=r.headers.get('x-bao-mark'); \
             return r.text().then(function(b){{ \
               globalThis[k]={{status:String(st),body:b,hdr:(h===null?'':h)}}; }}); }}, \
             function(e){{ globalThis[k]={{status:'rejected',body:String(e),hdr:''}}; }}); \
         return 'armed'; }})()"
    );
    let r = eval_js(t, &js);
    assert_eq!(r, "armed", "page fetch must arm cleanly");
}

/// The page-visible fetch outcome parked by `arm_page_fetch`. Bracket access
/// (the parked key contains a dash — dot access would parse as subtraction).
fn fetch_outcome(t: &mut WsCdp, name: &str) -> Value {
    let raw = eval_js(t, &format!("JSON.stringify(globalThis['__r_{name}'])"));
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

/// Poll until the parked fetch settles (status ≠ "pending"), bounded —
/// the pump-driven completion gate (never a blind sleep).
fn wait_fetch_outcome(t: &mut WsCdp, name: &str, what: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let v = fetch_outcome(t, name);
        if v["status"].as_str().is_some_and(|s| s != "pending") {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: fetch never settled (still pending)"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The shared assertions on one `Fetch.requestPaused` payload: the real
/// request fields the client must observe (design §3.2 pause face).
fn assert_paused_face(p: &Value, url_hint: &str) -> String {
    assert_eq!(
        p["request"]["method"], "GET",
        "paused request must carry the real method: {p}"
    );
    assert!(
        p["request"]["url"].as_str().is_some_and(|u| u.contains(url_hint)),
        "paused request must carry the real url (…{url_hint}): {p}"
    );
    let interception_id = p["interceptionId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| panic!("requestPaused must carry a non-empty interceptionId: {p}"))
        .to_string();
    assert!(
        !p["requestId"].as_str().unwrap_or("").is_empty(),
        "requestPaused must carry a requestId: {p}"
    );
    assert!(
        RESOURCE_TYPES.contains(&p["resourceType"].as_str().unwrap_or("")),
        "resourceType must be in the closed wire set {RESOURCE_TYPES:?}, got: {p}"
    );
    assert_eq!(
        p["isRedirect"], false,
        "a first-shot fetch is not a redirect: {p}"
    );
    interception_id
}

// ─────────────────────────────────────────────────────────────────────────
// Case 1 — pause + continue: the original request completes over the real
// network, and the design §4 structural order holds on the wire
// ─────────────────────────────────────────────────────────────────────────

#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn fetch_interception_pause_then_continue_completes_over_real_network() {
    let origin = spawn_origin();
    let fixture = match LiveFixture::spawn() {
        Ok(f) => f,
        Err(e) => {
            panic!("runtime init failed: {e}");
        }
    };
    let ws_url = fixture.ws_url.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let client = std::thread::spawn(move || client_pause_continue(&ws_url, origin, tx));
    fixture
        .pump_until(client, rx, "pause-continue")
        .expect("pause+continue live chain must pass");
}

fn client_pause_continue(ws_url: &str, origin: u16, tx: std::sync::mpsc::Sender<Result<(), String>>) {
    let run = (|| -> Result<(), String> {
        let mut t = WsCdp::connect(ws_url);
        enable_domains(&mut t, format!("http://127.0.0.1:{origin}/net/cont*"))?;
        navigate_and_settle(&mut t, origin)?;

        // Arm the page-side fetch — the request that will be paused.
        arm_page_fetch(&mut t, origin, "cont-1");

        // The pause: real request fields on the wire.
        let paused = t
            .wait_event(
                "Fetch.requestPaused",
                "/net/cont-1",
                Duration::from_secs(30),
            )
            .ok_or("no Fetch.requestPaused within 30s — the pause never reached the client")?;
        let interception_id = assert_paused_face(&paused["params"], "/net/cont-1");

        // Design §4 structural order on the wire: the net tap's
        // requestWillBeSent (main_fetch step 1) precedes the pause (the
        // interceptor round trip). A reversed order is a real finding
        // (design §9 risk 2), never a tolerable flake.
        let i_req = t
            .buffered_index("Network.requestWillBeSent", "/net/cont-1")
            .ok_or("no Network.requestWillBeSent for the paused request (Network tap did not fire)")?;
        let i_pause = t
            .buffered_index("Fetch.requestPaused", "/net/cont-1")
            .expect("the pause frame was just seen — it is in the ordered buffer");
        assert!(
            i_req < i_pause,
            "event order: requestWillBeSent (idx {i_req}) must precede requestPaused (idx {i_pause})"
        );

        // Continue: drop the held load → servo answers DoNotIntercept → the
        // ORIGINAL request proceeds into the net stack.
        let r = send(
            &mut t,
            "Fetch.continueRequest",
            json!({ "interceptionId": interception_id }),
        );
        if r.get("error").is_some() {
            return Err(format!("Fetch.continueRequest: {r}"));
        }

        // The page-visible completion: 200 + the origin's REAL body — the
        // request truly traversed the network after the pause, not a
        // fabricated settle.
        let out = wait_fetch_outcome(&mut t, "cont-1", "cont-1 after continueRequest");
        assert_eq!(
            out["status"], "200",
            "continued request must complete 200 over the real network, got {out}"
        );
        assert_eq!(
            out["body"], "origin-body-cont-1",
            "continued request must carry the origin's real body, got {out}"
        );

        // §4 ③a: the response tap fires for the continued request, after the
        // pause. (loadingFinished deliberately not asserted — no live
        // producer; see the module comment.)
        let resp = t
            .wait_event(
                "Network.responseReceived",
                "/net/cont-1",
                Duration::from_secs(15),
            )
            .ok_or("no Network.responseReceived for the continued request")?;
        assert_eq!(
            resp["params"]["response"]["status"], 200,
            "responseReceived carries the real status: {resp}"
        );
        let i_resp = t
            .buffered_index("Network.responseReceived", "/net/cont-1")
            .expect("the response frame was just seen — it is in the ordered buffer");
        assert!(
            i_pause < i_resp,
            "event order: requestPaused (idx {i_pause}) must precede responseReceived (idx {i_resp})"
        );
        Ok(())
    })();
    let _ = tx.send(run);
}

// ─────────────────────────────────────────────────────────────────────────
// Case 2 — fulfillRequest: a synthetic response downlinks to the page
// ─────────────────────────────────────────────────────────────────────────

#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn fetch_interception_fulfill_request_downlinks_synthetic_response() {
    let origin = spawn_origin();
    let fixture = match LiveFixture::spawn() {
        Ok(f) => f,
        Err(e) => {
            panic!("runtime init failed: {e}");
        }
    };
    let ws_url = fixture.ws_url.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let client = std::thread::spawn(move || client_fulfill(&ws_url, origin, tx));
    fixture
        .pump_until(client, rx, "fulfill")
        .expect("fulfill live chain must pass");
}

fn client_fulfill(ws_url: &str, origin: u16, tx: std::sync::mpsc::Sender<Result<(), String>>) {
    let run = (|| -> Result<(), String> {
        let mut t = WsCdp::connect(ws_url);
        enable_domains(&mut t, format!("http://127.0.0.1:{origin}/net/fulfill*"))?;
        navigate_and_settle(&mut t, origin)?;

        arm_page_fetch(&mut t, origin, "fulfill-1");

        let paused = t
            .wait_event(
                "Fetch.requestPaused",
                "/net/fulfill-1",
                Duration::from_secs(30),
            )
            .ok_or("no Fetch.requestPaused within 30s — the pause never reached the client")?;
        let interception_id = assert_paused_face(&paused["params"], "/net/fulfill-1");

        // Fulfill with a response the origin would never produce: status 418,
        // a custom marker header, and a body distinct from the origin's
        // `origin-body-fulfill-1` — the page-visible body is the
        // discriminator that the response was synthesized, not networked.
        let body = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode("synthetic-body-fulfill")
        };
        let r = send(
            &mut t,
            "Fetch.fulfillRequest",
            json!({
                "interceptionId": interception_id,
                "responseCode": 418,
                "responsePhrase": "Teapot",
                "responseHeaders": { "Content-Type": "text/plain", "X-Bao-Mark": "fulfill-synthetic" },
                "body": body
            }),
        );
        if r.get("error").is_some() {
            return Err(format!("Fetch.fulfillRequest: {r}"));
        }

        let out = wait_fetch_outcome(&mut t, "fulfill-1", "fulfill-1 after fulfillRequest");
        assert_eq!(
            out["status"], "418",
            "the synthetic status must downlink to the page, got {out}"
        );
        assert_eq!(
            out["body"], "synthetic-body-fulfill",
            "the synthetic body must downlink verbatim (origin body would be \
             origin-body-fulfill-1 — a network response cannot produce this), got {out}"
        );
        assert_eq!(
            out["hdr"], "fulfill-synthetic",
            "the synthetic custom header must downlink to the page, got {out}"
        );
        Ok(())
    })();
    let _ = tx.send(run);
}

// ─────────────────────────────────────────────────────────────────────────
// Case 3 — Fetch.disable: drains the pending pause (the paused request
// proceeds, not wedged) and subsequent matching requests flow with zero
// pauses (the disable-is-the-exit invariant, design §3.1)
// ─────────────────────────────────────────────────────────────────────────

#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn fetch_interception_disable_drains_pause_and_subsequent_requests_flow_direct() {
    let origin = spawn_origin();
    let fixture = match LiveFixture::spawn() {
        Ok(f) => f,
        Err(e) => {
            panic!("runtime init failed: {e}");
        }
    };
    let ws_url = fixture.ws_url.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let client = std::thread::spawn(move || client_disable(&ws_url, origin, tx));
    fixture
        .pump_until(client, rx, "disable-drain")
        .expect("disable-drain live chain must pass");
}

fn client_disable(ws_url: &str, origin: u16, tx: std::sync::mpsc::Sender<Result<(), String>>) {
    let run = (|| -> Result<(), String> {
        let mut t = WsCdp::connect(ws_url);
        enable_domains(&mut t, format!("http://127.0.0.1:{origin}/net/drain*"))?;
        navigate_and_settle(&mut t, origin)?;

        // First: prove the slot is armed and matching — the request pauses.
        arm_page_fetch(&mut t, origin, "drain-1");
        t.wait_event(
            "Fetch.requestPaused",
            "/net/drain-1",
            Duration::from_secs(30),
        )
        .ok_or("no Fetch.requestPaused within 30s — the slot never armed")?;

        // Disable while a request is paused: the held load drops → servo
        // answers DoNotIntercept → the paused request proceeds over the real
        // network (the drain-is-continue invariant; the fetch is neither
        // wedged nor rejected).
        let r = send(&mut t, "Fetch.disable", json!({}));
        if r.get("error").is_some() {
            return Err(format!("Fetch.disable: {r}"));
        }
        let out1 = wait_fetch_outcome(&mut t, "drain-1", "drain-1 after Fetch.disable");
        assert_eq!(
            out1["status"], "200",
            "the drained pause must complete 200 (drain = continue), got {out1}"
        );
        assert_eq!(
            out1["body"], "origin-body-drain-1",
            "the drained pause must carry the origin's real body, got {out1}"
        );

        // After disable, a matching request flows direct: completes 200 with
        // zero requestPaused. Completion is itself the proof (a held request
        // cannot complete); the buffer scan is the belt-and-braces face.
        arm_page_fetch(&mut t, origin, "drain-2");
        let out2 = wait_fetch_outcome(&mut t, "drain-2", "drain-2 after Fetch.disable");
        assert_eq!(
            out2["status"], "200",
            "post-disable request must flow direct 200, got {out2}"
        );
        assert_eq!(
            out2["body"], "origin-body-drain-2",
            "post-disable request must carry the origin's real body, got {out2}"
        );
        assert!(
            t.buffered_index("Fetch.requestPaused", "/net/drain-2").is_none(),
            "no requestPaused may exist for a post-disable request"
        );
        let pause_count = t
            .pending_events
            .iter()
            .filter(|v| event_matches(v, "Fetch.requestPaused", "/net/drain-"))
            .count();
        assert_eq!(
            pause_count, 1,
            "exactly one pause event total (drain-1 at arm time; the drained \
             load and the post-disable request emit no further pause), got {pause_count}"
        );
        Ok(())
    })();
    let _ = tx.send(run);
}
