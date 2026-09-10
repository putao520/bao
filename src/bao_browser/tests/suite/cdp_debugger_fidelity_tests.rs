// REQ-CDP Debugger fidelity e2e (SM-EVOLUTION #27 裁决 2/3 live closure):
// the page-realm SM Debugger glue must surface REAL data — a breakpoint hit
// emits non-empty callFrames whose location comes from native
// `getOffsetLocation` (not the hardcoded 0), setBreakpointByUrl binds via
// native `getLineOffsets` (the former `offsetLine` call targeted a
// nonexistent API), possibleBreakpoints comes from native
// `getPossibleBreakpoints` (not line synthesis), blackbox fails explicitly
// (SM has no native face), and the environment bridge face returns the new
// native shape. The fidelity triangle: scriptParsed.startLine,
// setBreakpointByUrl's resolved location, and the paused frame's location
// are three INDEPENDENT native reads that must all agree on the same line.
// @trace REQ-CDP-003 [req:REQ-CDP-003] [level:e2e]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BaoRuntime, BaoWsRegistry, PageConfig};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeSender};
use bun_uws::ws_client::{RecvOutcome, WebSocketClient};
use cdp_server::{CdpServer, EventSender, ServerConfig};
use serde_json::{json, Value};

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal WS CDP client with an event backlog: responses are matched by id
/// and every event read while waiting is retained for later `take_event`.
struct WsCdp {
    client: WebSocketClient,
    next_id: i64,
    events: Vec<Value>,
}

impl WsCdp {
    fn connect(url: &str) -> Self {
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
        client.set_read_timeout(Duration::from_millis(250));
        WsCdp {
            client,
            next_id: 1,
            events: Vec::new(),
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
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                    if v.get("method").is_some() {
                        self.events.push(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed waiting for {method} response"),
            }
        }
        panic!("timeout waiting for {method} response");
    }

    /// Wait (bounded) for an event with the given method, checking the
    /// backlog first. Events read along the way are retained.
    fn take_event(&mut self, method: &str, timeout: Duration) -> Value {
        self.try_take_event(method, timeout)
            .unwrap_or_else(|| panic!("{method} event never arrived"))
    }

    /// Soft variant: None on timeout instead of panicking (event-stream
    /// drains where exhaustion is the normal end).
    fn try_take_event(&mut self, method: &str, timeout: Duration) -> Option<Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(pos) = self
                .events
                .iter()
                .position(|e| e["method"].as_str() == Some(method))
            {
                return Some(self.events.remove(pos));
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("method").is_some() {
                        self.events.push(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed waiting for {method} event"),
            }
        }
    }

    /// Assert no paused event carrying the given breakpoint id arrives
    /// within the window (removal proof — filtered to that exact id so
    /// unrelated pauses can't mask a removal failure).
    fn assert_no_paused_for(&mut self, bp_id: &str, window: Duration) {
        let deadline = std::time::Instant::now() + window;
        while std::time::Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("method").is_some() {
                        if v["method"].as_str() == Some("Debugger.paused") {
                            let hits = v["params"]["hitBreakpoints"].as_array();
                            if hits.is_some_and(|h| {
                                h.iter().any(|b| b.as_str() == Some(bp_id))
                            }) {
                                panic!("removed breakpoint {bp_id} still fires: {v}");
                            }
                        }
                        self.events.push(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed in negative window"),
            }
        }
    }
}

/// The inline page script: `bpTarget` carries the store line the breakpoint
/// binds to, `dbgStmtTarget` the `debugger;` line. The trailing marker
/// comments locate both lines in the REAL source text fetched via
/// getScriptSource — no hardcoded line guesses.
const PAGE_JS: &str = "\
function bpTarget(a) {
    var prod = a * 41;
    window.__bao_bp_marker = true; // bao-dbg-fid-bp
    return prod + 1;
}
function dbgStmtTarget() {
    debugger; // bao-dbg-fid-dbg
    return 7;
}
window.__bao_ready = true;";

fn client_phase(ws_url: String, bridge: BridgeSender, page_id: usize, done: Arc<AtomicBool>) {
    let mut cdp = WsCdp::connect(&ws_url);

    // 0. Navigate to the marker document and wait for the inline script to
    //    have run (`__bao_ready` is the last statement).
    let html = format!("<html><body><script>{PAGE_JS}</script></body></html>");
    let url = format!("data:text/html;charset=utf-8,{}", html.replace('\n', "%0A"));
    let resp = cdp.send("Page.navigate", json!({ "url": url }));
    assert!(resp.get("error").is_none(), "navigate must succeed: {resp}");
    let mut ready = false;
    for _ in 0..200 {
        let r = cdp.send(
            "Runtime.evaluate",
            json!({ "expression": "window.__bao_ready === true", "returnByValue": true }),
        );
        if r["result"]["result"]["value"] == json!(true) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "inline marker script never ran");

    // 1. Debugger.enable — installs the page-realm Debugger and flushes
    //    scriptParsed for pre-existing scripts.
    let resp = cdp.send("Debugger.enable", json!({}));
    assert!(resp.get("error").is_none(), "enable must succeed: {resp}");

    // 2. scriptParsed for the marker script (url carries the marker text).
    let mut script_id = String::new();
    let mut start_line0 = i64::MAX;
    let mut marker_script_ids: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let Some(evt) = cdp.try_take_event("Debugger.scriptParsed", Duration::from_millis(500))
        else {
            break; // event stream drained
        };
        let url = evt["params"]["url"].as_str().unwrap_or_default();
        if url.contains("bao-dbg-fid-bp") {
            if script_id.is_empty() {
                script_id = evt["params"]["scriptId"].as_str().expect("scriptId").to_string();
                start_line0 = evt["params"]["startLine"].as_i64().expect("startLine");
            }
            // SM surfaces eagerly-compiled inner functions as their own
            // Debugger.Script sharing the same url — collect them all; a
            // paused frame may execute in one of these, not the top-level.
            marker_script_ids
                .push(evt["params"]["scriptId"].as_str().expect("scriptId").to_string());
        }
    }
    assert!(!script_id.is_empty(), "marker script never parsed");
    assert!(start_line0 >= 0, "scriptParsed startLine must be 0-origin: {start_line0}");

    // 3. getScriptSource — REAL source text; locate the two target lines by
    //    marker (0-based index within the script text) and lift them into
    //    absolute CDP 0-origin lines via scriptParsed.startLine.
    let resp = cdp.send(
        "Debugger.getScriptSource",
        json!({ "scriptId": script_id }),
    );
    assert!(resp.get("error").is_none(), "getScriptSource: {resp}");
    let source = resp["result"]["scriptSource"].as_str().expect("scriptSource");
    assert!(source.contains("function bpTarget"), "real source text, got: {source:?}");
    let line_of = |marker: &str| -> i64 {
        let idx = source
            .split('\n')
            .position(|l| l.contains(marker))
            .unwrap_or_else(|| panic!("marker {marker} missing from source"));
        start_line0 + idx as i64
    };
    let cdp_bp_line = line_of("bao-dbg-fid-bp");
    let cdp_dbg_line = line_of("bao-dbg-fid-dbg");

    // 4. setBreakpointByUrl — binds via native getLineOffsets; the resolved
    //    location must be a REAL location (from getOffsetLocation) on the
    //    exact marker line, in the exact parsed script.
    let resp = cdp.send(
        "Debugger.setBreakpointByUrl",
        json!({ "urlRegex": "bao-dbg-fid", "lineNumber": cdp_bp_line }),
    );
    assert!(resp.get("error").is_none(), "setBreakpointByUrl: {resp}");
    let bp_id = resp["result"]["breakpointId"].as_str().expect("breakpointId").to_string();
    let locations = resp["result"]["locations"].as_array().expect("locations array");
    assert!(!locations.is_empty(), "resolved locations must be non-empty: {resp}");
    assert_eq!(
        locations[0]["scriptId"].as_str(), Some(script_id.as_str()),
        "resolved script must be the parsed marker script: {resp}"
    );
    assert_eq!(
        locations[0]["lineNumber"].as_i64(), Some(cdp_bp_line),
        "resolved location must be the REAL marker line (getOffsetLocation, not a request echo): {resp}"
    );
    assert!(locations[0]["columnNumber"].as_i64().is_some_and(|c| c >= 0));

    // 5. Trigger the breakpoint and assert the paused event carries REAL
    //    frames: non-empty callFrames, frame 0 is bpTarget at the marker
    //    line, and the scope chain is the native environment chain.
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "bpTarget(2)", "returnByValue": true }),
    );
    assert_eq!(resp["result"]["result"]["value"], 83, "bpTarget must complete (notification-grade pause): {resp}");

    let evt = cdp.take_event("Debugger.paused", Duration::from_secs(10));
    let params = &evt["params"];
    assert_eq!(params["reason"].as_str(), Some("breakpoint"), "paused: {evt}");
    let hits = params["hitBreakpoints"].as_array().expect("hitBreakpoints");
    assert!(hits.iter().any(|b| b.as_str() == Some(bp_id.as_str())), "paused: {evt}");
    let frames = params["callFrames"].as_array().expect("callFrames array");
    assert!(!frames.is_empty(), "callFrames must be REAL and non-empty (former glue emitted []): {evt}");
    let f0 = &frames[0];
    assert_eq!(f0["functionName"].as_str(), Some("bpTarget"), "frame 0 fn: {evt}");
    assert_eq!(
        f0["location"]["scriptId"].as_str(), Some(script_id.as_str()),
        "frame 0 script: {evt}"
    );
    assert_eq!(
        f0["location"]["lineNumber"].as_i64(), Some(cdp_bp_line),
        "frame 0 location must be the REAL hit line via getOffsetLocation (former glue hardcoded 0): {evt}"
    );
    assert!(f0["location"]["columnNumber"].as_i64().is_some_and(|c| c >= 0));
    let chain = f0["scopeChain"].as_array().expect("scopeChain array");
    assert!(!chain.is_empty(), "scopeChain must come from the native env chain: {evt}");
    // bpTarget's innermost env is its function scope -> native scopeKind
    // 'function' maps to CDP 'local'; the object is DESCRIBED (className),
    // never minted a fake objectId (the former glue did: 'local-<idx>').
    assert_eq!(chain[0]["type"].as_str(), Some("local"), "innermost scope: {evt}");
    assert!(
        chain[0]["object"].get("objectId").is_none(),
        "scope objects must not carry fake objectIds: {evt}"
    );
    assert!(
        chain[0]["object"]["className"].as_str().is_some(),
        "scope object is described by className: {evt}"
    );
    assert_eq!(f0["this"]["type"].as_str(), Some("object"), "captured this: {evt}");

    // 6. getPossibleBreakpoints — native entries for THIS script; the line
    //    we just broke on must be among them (real cross-check, the former
    //    glue synthesized every line of every script).
    let resp = cdp.send(
        "Debugger.getPossibleBreakpoints",
        json!({ "start": { "scriptId": script_id } }),
    );
    assert!(resp.get("error").is_none(), "getPossibleBreakpoints: {resp}");
    let locs = resp["result"]["locations"].as_array().expect("locations array");
    assert!(!locs.is_empty(), "native possible-breakpoint entries must be non-empty: {resp}");
    assert!(
        locs.iter().all(|l| l["scriptId"].as_str() == Some(script_id.as_str())),
        "all entries belong to the queried script: {resp}"
    );
    assert!(
        locs.iter().any(|l| l["lineNumber"].as_i64() == Some(cdp_bp_line)),
        "the proven-breakable marker line must be listed: {resp}"
    );

    // 7. blackbox/unblackbox — explicit not-supported (SM has no native
    //    face; the former silent ok was a fake success). 裁决 3.
    for m in ["Debugger.blackbox", "Debugger.unblackbox"] {
        let resp = cdp.send(m, json!({ "scriptId": script_id }));
        let err = resp["error"]
            .as_object()
            .unwrap_or_else(|| panic!("{m} must fail explicitly: {resp}"));
        assert_eq!(err["code"], -32000, "{m} code: {resp}");
        assert!(
            err["message"].as_str().unwrap_or_default().contains("no native blackbox"),
            "{m} message must state the native gap: {resp}"
        );
    }

    // 8. removeBreakpoint — precise removal; the removed id must never fire
    //    again (bounded negative window).
    let resp = cdp.send("Debugger.removeBreakpoint", json!({ "breakpointId": bp_id }));
    assert!(resp.get("error").is_none(), "removeBreakpoint: {resp}");
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "bpTarget(3)", "returnByValue": true }),
    );
    assert_eq!(resp["result"]["result"]["value"], 124, "post-removal run: {resp}");
    cdp.assert_no_paused_for(&bp_id, Duration::from_secs(3));

    // 9. `debugger;` statement — paused with the REAL statement location
    //    (the former glue hardcoded lineNumber/columnNumber 0).
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({ "expression": "dbgStmtTarget()", "returnByValue": true }),
    );
    assert_eq!(resp["result"]["result"]["value"], 7, "dbgStmtTarget must complete: {resp}");
    let evt = cdp.take_event("Debugger.paused", Duration::from_secs(10));
    assert_eq!(
        evt["params"]["reason"].as_str(), Some("debuggerStatement"),
        "debugger-statement pause: {evt}"
    );
    let frames = evt["params"]["callFrames"].as_array().expect("callFrames");
    assert!(!frames.is_empty(), "debugger-statement frames non-empty: {evt}");
    assert_eq!(
        frames[0]["functionName"].as_str(), Some("dbgStmtTarget"),
        "frame 0 fn: {evt}"
    );
    assert_eq!(
        frames[0]["location"]["lineNumber"].as_i64(), Some(cdp_dbg_line),
        "debugger-statement location must be the REAL line (former glue hardcoded 0): {evt}"
    );
    // SM may execute the statement in the function's own eagerly-compiled
    // script (same url, same coordinates) rather than the top-level one —
    // the fidelity claim is the LINE, plus script identity within the
    // marker url's parsed script set.
    assert!(
        frames[0]["location"]["scriptId"]
            .as_str()
            .is_some_and(|sid| marker_script_ids.iter().any(|m| m == sid)),
        "debugger-statement frame script must belong to the marker document: {evt}"
    );

    // 10. Environment bridge face (bridge-level command; no CDP WS method
    //     routes here) — the new native walk returns an ARRAY of env
    //     records. Idle (no live debuggee frame) the array is empty but the
    //     SHAPE is the native one; the former glue answered a hardcoded
    //     `{environment: {}}` object regardless of state.
    let resp = bridge.send(BridgeCommand::DebuggerGetEnvironment {
        target_id: page_id.to_string(),
        frame_actor_id: String::new(),
    });
    let env_face = resp.result.expect("DebuggerGetEnvironment must succeed");
    assert!(
        env_face["environment"].is_array(),
        "environment must be the native array shape (former glue: {{environment:{{}}}}): {env_face}"
    );

    let _ = cdp.send("Debugger.disable", json!({}));

    done.store(true, Ordering::Relaxed);
}

#[test]
fn debugger_breakpoint_real_frames_and_locations_e2e() {
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(60));
    let (console_tx, console_rx) = mpsc::channel::<cdp_server::ConsoleMessage>();
    runtime.set_console_log_channel(console_tx);
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
        bridge_tx.clone(),
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    // Production wiring (run_browser shape): the server consumes the
    // ConsoleMessage channel — `__BAO_EVT__` Debugger events ride Path A.
    server.set_console_receiver(console_rx);
    let broadcaster = server.broadcaster();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    let done = Arc::new(AtomicBool::new(false));
    let page_id = page.id();
    let client = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || client_phase(ws_url, bridge_tx, page_id, done))
    };

    // Main thread: the run_with_bridge loop shape (servo spin + bridge drain
    // + Path-B event translation). Debugger events arrive via the server's
    // console_rx drain (Path A) — including with Path B active, which is
    // exactly the delegate-arm condition this wave closes.
    use bao_cdp_client::bridge::translate;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            // Path B (structured ServoEvent) carries no Debugger events —
            // the `__BAO_EVT__` arm in the delegate routes those to Path A —
            // but the rest still translates and broadcasts, run_with_bridge
            // parity.
            for cdp_event in translate(servo_event) {
                broadcaster.send_event(&cdp_event.method, cdp_event.params);
            }
        }
        std::thread::yield_now();
    }

    client.join().expect("debugger fidelity phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "debugger fidelity phase must have completed all assertions"
    );
}
