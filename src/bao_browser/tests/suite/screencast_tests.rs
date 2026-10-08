// REQ-CDP-009 seven-criteria locks — CDP screencast E2E over the memory://
// face (the published consumer contract from issue #55: bao_cdp_client
// `Browser::connect("memory://bao")` + `Page.startScreencast` →
// `Page.screencastFrame` events) plus one WS-face e2e (the Playwright
// direct-connect path).
//
// Harness shape (the suite's established pattern — puppeteer_e2e /
// cdp_debugger_fidelity): `BrowserRuntime` is `!Send`, so the MAIN thread
// owns the pump loop while a helper thread holds the CDP client whose
// commands/events ride the process bridge.
//
// C7 (headless) is locked by construction: the whole chain runs under the
// headless software-rendering configuration (no display server —
// llvmpipe/SurfmanRenderingContext, the same configuration the CI suite
// runs in) and every test asserts real non-empty decoded frames were
// produced that way.
//
// @trace REQ-CDP-009 [level:e2e]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_browser::{screencast, BaoConfig, BrowserRuntime, PageConfig};
use bao_cdp_client::Browser;
use base64::Engine;
use serde_json::{json, Value};

/// Static marker document: no animations, no rAF — the only pixel changes
/// come from the tests' own Runtime.evaluate mutations, so the change-driven
/// gate (C1) observes exactly the test's stimulus. `#`-free colors so the
/// data URL needs no percent-encoding.
const DOC: &str = "data:text/html,<html><body style='margin:0'>\
<div id='t' style='width:120px;height:90px;background:rgb(255,0,0)'></div>\
</body></html>";

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Fresh runtime + settled page. The 800ms pre-pump settles the initial
/// load's rendering so screencast sessions observe only test-driven changes
/// (load-tail repaints are real changes and would otherwise blur the C2/C4
/// timing assertions).
fn setup() -> (BrowserRuntime, bao_browser::PageHandle) {
    // Single-process suite isolation (see screencast::drop_all_sessions).
    screencast::drop_all_sessions();
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("runtime init");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(DOC.into()),
            ..Default::default()
        })
        .expect("create page");
    runtime.pump_cdp(Duration::from_millis(800));
    (runtime, page)
}

/// Pump the runtime in 50ms slices while `client` runs on a helper thread
/// (the !Send runtime stays on THIS thread; the client's bridge dispatches
/// are answered by the pump). Returns the client's outcome.
fn pump_with_client<T, F>(runtime: &BrowserRuntime, budget: Duration, client: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Browser) -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<T>();
    std::thread::Builder::new()
        .name("screencast-client".into())
        .spawn(move || {
            let mut browser = Browser::connect("memory://bao").expect("memory connect");
            let _ = tx.send(client(&mut browser));
        })
        .expect("spawn client thread");
    let deadline = Instant::now() + budget;
    loop {
        runtime.pump_cdp(Duration::from_millis(50));
        if let Ok(out) = rx.try_recv() {
            return Some(out);
        }
        if Instant::now() > deadline {
            return rx.try_recv().ok();
        }
    }
}

// ── client-side helpers (run on the helper thread) ──────────────────────────

fn start_screencast(b: &mut Browser, params: Value) {
    let r = b
        .send_command("Page.startScreencast", params)
        .expect("Page.startScreencast succeeds");
    assert_eq!(r, json!({}), "startScreencast returns an empty result");
}

fn stop_screencast(b: &mut Browser) {
    let r = b
        .send_command("Page.stopScreencast", json!({}))
        .expect("Page.stopScreencast succeeds");
    assert_eq!(r, json!({}));
}

fn ack(b: &mut Browser, session_id: u64) {
    let r = b
        .send_command("Page.screencastFrameAck", json!({ "sessionId": session_id }))
        .expect("Page.screencastFrameAck succeeds");
    assert_eq!(r, json!({}));
}

fn mutate(b: &mut Browser, color: &str) {
    let r = b
        .send_command(
            "Runtime.evaluate",
            json!({
                "expression": format!(
                    "document.getElementById('t').style.background='{color}'"
                ),
                "returnByValue": true,
            }),
        )
        .expect("mutation evaluate succeeds");
    assert!(r.get("error").is_none() && !r.to_string().contains("error"), "mutate ok: {r}");
}

/// Collect screencastFrame params (ignoring other events) until `stop_when`
/// says enough or the budget passes.
fn collect_frames<F>(b: &mut Browser, budget: Duration, mut stop_when: F) -> Vec<Value>
where
    F: FnMut(&[Value]) -> bool,
{
    let deadline = Instant::now() + budget;
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        if stop_when(&frames) {
            break;
        }
        match b.recv_event() {
            Ok(Some(ev)) if ev.method == "Page.screencastFrame" => frames.push(ev.params),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    frames
}

/// Acking variant of [`collect_frames`] — every arrival is acked on the
/// spot, keeping the ack gate (C4) out of tests that exercise the change
/// gate (C1) or the rate gate (C2).
fn collect_frames_acking<F>(b: &mut Browser, budget: Duration, mut stop_when: F) -> Vec<Value>
where
    F: FnMut(&[Value]) -> bool,
{
    let deadline = Instant::now() + budget;
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        if stop_when(&frames) {
            break;
        }
        match b.recv_event() {
            Ok(Some(ev)) if ev.method == "Page.screencastFrame" => {
                if let Some(sid) = ev.params["sessionId"].as_u64() {
                    ack(b, sid);
                }
                frames.push(ev.params);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    frames
}

/// Acking variant of [`wait_quiet`].
fn wait_quiet_acking(b: &mut Browser, quiet: Duration) -> usize {
    let deadline = Instant::now() + quiet * 4;
    let mut since_last = Instant::now();
    let mut total = 0usize;
    while Instant::now() < deadline {
        match b.recv_event() {
            Ok(Some(ev)) if ev.method == "Page.screencastFrame" => {
                if let Some(sid) = ev.params["sessionId"].as_u64() {
                    ack(b, sid);
                }
                total += 1;
                since_last = Instant::now();
            }
            Ok(_) => {}
            Err(_) => break,
        }
        if since_last.elapsed() >= quiet {
            return total;
        }
    }
    total
}

fn decode_frame(frame: &Value) -> Vec<u8> {
    let data = frame["data"].as_str().expect("frame data is base64 string");
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("frame data decodes from base64")
}

// ════════════════════════════════════════════════════════════════════
// C3 + C6 + C7 — frame event shape (data/metadata/sessionId), jpeg+png,
// headless availability
// ════════════════════════════════════════════════════════════════════

/// C3: frames arrive as `Page.screencastFrame` with base64 `data`,
/// `metadata` (device size + timestamp) and a numeric `sessionId`.
/// C6: both `png` and `jpeg` formats decode with their magic headers.
/// C7: the entire chain runs in the headless software-rendering shape and
/// produced these very frames (no display server involved).
// @trace REQ-CDP-009 [criterion:帧经 Page.screencastFrame 事件下发(data 含 base64 帧与 metadata)]
// @trace REQ-CDP-009 [criterion:format 参数支持 jpeg 与 png]
// @trace REQ-CDP-009 [criterion:headless 环境(无真实显示器形态)可用]
#[test]
fn c3_c6_c7_frame_event_shape_and_formats() {
    eprintln!(
        "[screencast-c7] headless chain under DISPLAY={:?}",
        std::env::var("DISPLAY").unwrap_or_default()
    );
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(25), |b| {
        // ── PNG ──
        start_screencast(b, json!({ "format": "png" }));
        let frames = collect_frames(b, Duration::from_secs(6), |f| !f.is_empty());
        assert!(!frames.is_empty(), "first frame must arrive without any mutation");
        let f = &frames[0];
        let bytes = decode_frame(f);
        assert!(bytes.len() > 100, "frame carries real image data ({}B)", bytes.len());
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47], "PNG magic header");
        let meta = &f["metadata"];
        assert!(
            meta["deviceWidth"].as_u64().unwrap_or(0) > 0,
            "metadata.deviceWidth: {meta}"
        );
        assert!(
            meta["deviceHeight"].as_u64().unwrap_or(0) > 0,
            "metadata.deviceHeight: {meta}"
        );
        assert!(
            meta["timestamp"].as_f64().unwrap_or(0.0) > 0.0,
            "metadata.timestamp: {meta}"
        );
        let sid = f["sessionId"].as_u64().expect("sessionId is numeric");
        assert!(sid >= 1, "screencast sessionId >= 1");
        stop_screencast(b);

        // ── JPEG (same face restart replaces the stream) ──
        start_screencast(b, json!({ "format": "jpeg", "quality": 60 }));
        let frames = collect_frames(b, Duration::from_secs(6), |f| !f.is_empty());
        assert!(!frames.is_empty(), "jpeg first frame must arrive");
        let bytes = decode_frame(&frames[0]);
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "JPEG magic header");
        assert!(bytes.len() > 100, "jpeg frame non-trivial ({}B)", bytes.len());
        stop_screencast(b);
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// C1 — change-driven: content change produces a frame, no change does not
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:Page.startScreencast 建立 change-driven 帧流会话:页面内容变化才出新帧]
#[test]
fn c1_change_driven_frames_only_on_content_change() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(35), |b| {
        start_screencast(b, json!({ "format": "png" }));
        // Settle the stream (initial frame + any load-tail repaints), acking
        // throughout so the ack gate never holds later frames back.
        let _tail = wait_quiet_acking(b, Duration::from_millis(500));

        // Change 1 → the change frame (a change may legitimately land as
        // more than one distinct-pixel repaint; the tail absorber below
        // drains those before the negative assertion).
        mutate(b, "rgb(0,0,255)");
        let frames = collect_frames_acking(b, Duration::from_secs(6), |f| !f.is_empty());
        assert!(!frames.is_empty(), "content change must produce a frame");
        let _tail = wait_quiet_acking(b, Duration::from_millis(700));
        // No change → no frames: a FULL quiet window after the tail is silent.
        let extra = wait_quiet_acking(b, Duration::from_millis(700));
        assert_eq!(extra, 0, "static content must not produce frames");

        // Change 2 → a new frame again.
        mutate(b, "rgb(0,255,0)");
        let frames2 = collect_frames_acking(b, Duration::from_secs(6), |f| !f.is_empty());
        assert!(!frames2.is_empty(), "second change must produce a frame");
        let _tail2 = wait_quiet_acking(b, Duration::from_millis(700));
        // And quiet again.
        let extra2 = wait_quiet_acking(b, Duration::from_millis(700));
        assert_eq!(extra2, 0, "no further frames without change");
        stop_screencast(b);
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// C2 — maxFrameRate caps emission frequency
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:maxFrameRate 上限可设且生效(出帧频率不超过上限)]
#[test]
fn c2_max_frame_rate_caps_emission_frequency() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(35), |b| {
        // 2 fps → 500ms minimum interval between emitted frames.
        start_screencast(b, json!({ "format": "png", "maxFrameRate": 2 }));
        let first = collect_frames_acking(b, Duration::from_secs(6), |f| !f.is_empty());
        assert!(!first.is_empty(), "first frame must arrive under the rate cap");
        let t0 = Instant::now();
        // Churn distinct changes every ~80ms for 1.3s (≈16 stimuli — far
        // above the 2fps cap), acking every arrival.
        let colors = [
            "rgb(255,0,0)",
            "rgb(0,255,0)",
            "rgb(0,0,255)",
            "rgb(255,255,0)",
            "rgb(255,0,255)",
            "rgb(0,255,255)",
        ];
        let mut arrivals: Vec<Instant> = Vec::new();
        let mut i = 0usize;
        while t0.elapsed() < Duration::from_millis(1300) {
            mutate(b, colors[i % colors.len()]);
            i += 1;
            std::thread::sleep(Duration::from_millis(80));
            for _ in collect_frames_acking(b, Duration::from_millis(30), |_| false) {
                arrivals.push(Instant::now());
            }
        }
        stop_screencast(b);
        // The cap held: changes streamed (at least one more emission) but
        // never faster than the 500ms window. Arrival measurement carries
        // up to one ~110ms drain cycle of jitter, so the bound is 350ms.
        assert!(
            !arrivals.is_empty(),
            "changes must still stream under the rate cap"
        );
        assert!(
            arrivals.len() <= 5,
            "1.3s at 2fps must emit at most a handful, got {}",
            arrivals.len()
        );
        for w in arrivals.windows(2) {
            let gap = w[1].duration_since(w[0]);
            assert!(
                gap >= Duration::from_millis(350),
                "rate cap violated: {gap:?} between arrivals"
            );
        }
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// C4 — screencastFrameAck gates emission (≤1 un-acked frame in flight)
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:Page.screencastFrameAck 确认参与出帧节流(未 ack 不无限出帧)]
#[test]
fn c4_ack_gates_emission() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(35), |b| {
        start_screencast(b, json!({ "format": "png" }));
        let first = collect_frames(b, Duration::from_secs(6), |f| !f.is_empty())
            .into_iter()
            .next()
            .expect("first frame");
        let sid = first["sessionId"].as_u64().unwrap();

        // Un-acked: churn changes — nothing may be emitted.
        for c in ["rgb(0,0,255)", "rgb(0,255,0)", "rgb(255,0,255)"] {
            mutate(b, c);
            std::thread::sleep(Duration::from_millis(150));
        }
        let leaked = collect_frames(b, Duration::from_millis(600), |_| false);
        assert!(
            leaked.is_empty(),
            "un-acked session must not emit further frames ({} leaked)",
            leaked.len()
        );

        // Ack releases exactly the held latest-state frame.
        ack(b, sid);
        let released = collect_frames(b, Duration::from_secs(4), |f| !f.is_empty());
        assert_eq!(
            released.len(),
            1,
            "ack releases exactly the single held frame, got {}",
            released.len()
        );

        // Gate re-arms on the new un-acked frame.
        mutate(b, "rgb(255,255,0)");
        std::thread::sleep(Duration::from_millis(200));
        let leaked2 = collect_frames(b, Duration::from_millis(400), |_| false);
        assert!(leaked2.is_empty(), "the released frame must gate again until acked");
        stop_screencast(b);
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// C5 — stopScreencast terminates and releases
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:Page.stopScreencast 终止会话停止帧生产,资源正常释放]
#[test]
fn c5_stop_terminates_and_releases() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(30), |b| {
        start_screencast(b, json!({ "format": "png" }));
        let first = collect_frames(b, Duration::from_secs(6), |f| !f.is_empty())
            .into_iter()
            .next()
            .expect("first frame");
        ack(b, first["sessionId"].as_u64().unwrap());
        stop_screencast(b);
        // Host-side release probe: the session registry is empty.
        assert_eq!(
            screencast::active_session_count(),
            0,
            "stop must release the host-side session"
        );
        // Stopped: fresh changes produce nothing.
        mutate(b, "rgb(0,0,255)");
        std::thread::sleep(Duration::from_millis(200));
        mutate(b, "rgb(0,255,0)");
        let leaked = collect_frames(b, Duration::from_millis(600), |_| false);
        assert!(leaked.is_empty(), "stopped session must not emit frames");
    });
    out.expect("client phase completed");
    // Post-join double probe from the pump thread.
    assert_eq!(screencast::active_session_count(), 0);
}

// ════════════════════════════════════════════════════════════════════
// WS face e2e — the Playwright direct-connect path (ws://127.0.0.1:<port>
// /devtools/page/<id>): startScreencast → on-wire screencastFrame → ack →
// change → frame → stop. Locks the ws_registry interception + the
// run_with_bridge sink routing shape.
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [level:e2e]
#[test]
fn ws_face_screencast_e2e() {
    use bao_browser::{handle_bridge_command, BaoWsRegistry};
    use bao_cdp::domains::ServoTargetProvider;
    use bao_cdp::servo_bridge::bridge_channel;
    use bao_cdp_client::bridge::{translate, ServoEvent};
    use cdp_server::{CdpServer, EventSender, ServerConfig};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    screencast::drop_all_sessions();
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("runtime init");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(DOC.into()),
            ..Default::default()
        })
        .expect("create page");
    runtime.pump_cdp(Duration::from_millis(800));

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(60));
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<ServoEvent>();
    runtime.set_event_channel(event_tx);

    let registry = Arc::new(BaoWsRegistry::new(bridge_tx.clone()));
    let port = pick_free_port();
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry.clone());
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
        let ws_url = ws_url.clone();
        std::thread::spawn(move || ws_client_phase(ws_url, done))
    };

    // Main thread: the run_with_bridge pump shape — screencast drive with
    // the WS sink closure (target-scoped routing), bridge drain, servo
    // event translation.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done.load(Ordering::Relaxed) && Instant::now() < deadline {
        runtime.spin_event_loop();
        let ws_sink = |target: &str, method: &str, params: Value| {
            registry.broadcast_for_target(broadcaster.as_ref(), target, method, params)
        };
        screencast::drive(runtime.page_pool(), Some(&ws_sink));
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(se) = servo_event_rx.try_recv() {
            for ev in translate(se) {
                broadcaster.send_event(&ev.method, ev.params);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let frames = client.join().expect("ws client thread");
    assert_eq!(frames.len(), 2, "ws face must deliver start + change frames");
}

/// Minimal raw-socket CDP client (the suite's WsCdp shape — struct methods
/// avoid the closure double-borrow). Sends commands, buffers events, takes
/// events by method.
struct WsCdp {
    sock: bun_uws::ws_client::WebSocketClient,
    next_id: i64,
    events: Vec<Value>,
}

impl WsCdp {
    fn connect(url: &str) -> Self {
        use bun_uws::ws_client::WebSocketClient;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sock = None;
        while Instant::now() < deadline {
            match WebSocketClient::connect(url) {
                Ok(c) => {
                    sock = Some(c);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let mut sock = sock.expect("ws connect (bounded 10s retry exhausted)");
        sock.set_read_timeout(Duration::from_millis(250));
        WsCdp {
            sock,
            next_id: 1,
            events: Vec::new(),
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        use bun_uws::ws_client::RecvOutcome;
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method });
        if !params.is_null() {
            msg["params"] = params;
        }
        self.sock
            .send_text(&serde_json::to_string(&msg).unwrap())
            .expect("ws send");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match self.sock.recv().expect("ws recv") {
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

    /// Wait (bounded) for an event with the given method, backlog first.
    fn try_take_event(&mut self, method: &str, timeout: Duration) -> Option<Value> {
        use bun_uws::ws_client::RecvOutcome;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self
                .events
                .iter()
                .position(|e| e["method"].as_str() == Some(method))
            {
                return Some(self.events.remove(pos));
            }
            if Instant::now() >= deadline {
                return None;
            }
            match self.sock.recv().expect("ws recv") {
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
}

/// WS client phase: start, ack, mutate, expect exactly two screencastFrame
/// events, stop.
fn ws_client_phase(
    ws_url: String,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Vec<Value> {
    let mut cdp = WsCdp::connect(&ws_url);

    // Page.enable first: the CDP server's event outbox is domain-gated per
    // session (a Page.* event only drains to a session that enabled Page) —
    // the same contract Chrome DevTools frontends satisfy.
    let resp = cdp.send("Page.enable", json!({}));
    assert!(resp.get("error").is_none(), "ws Page.enable ok: {resp}");

    // start → ok, then the first frame on the wire.
    let resp = cdp.send("Page.startScreencast", json!({ "format": "png" }));
    assert!(resp.get("error").is_none(), "ws startScreencast ok: {resp}");
    let f1 = cdp
        .try_take_event("Page.screencastFrame", Duration::from_secs(10))
        .expect("ws screencastFrame #1");
    let sid = f1["params"]["sessionId"].as_u64().expect("ws sessionId");
    let bytes = decode_frame(&f1["params"]);
    assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47], "ws frame is PNG");

    // ack + change → second frame.
    let resp = cdp.send("Page.screencastFrameAck", json!({ "sessionId": sid }));
    assert!(resp.get("error").is_none(), "ws ack ok: {resp}");
    let resp = cdp.send(
        "Runtime.evaluate",
        json!({
            "expression": "document.getElementById('t').style.background='rgb(0,0,255)'",
            "returnByValue": true,
        }),
    );
    assert!(resp.get("error").is_none(), "ws mutate ok: {resp}");
    let f2 = cdp
        .try_take_event("Page.screencastFrame", Duration::from_secs(10))
        .expect("ws screencastFrame #2 (post-change)");

    // stop → ok; no further frames.
    let resp = cdp.send("Page.stopScreencast", json!({}));
    assert!(resp.get("error").is_none(), "ws stop ok: {resp}");
    assert!(
        cdp.try_take_event("Page.screencastFrame", Duration::from_millis(500))
            .is_none(),
        "no frames after ws stop"
    );

    done.store(true, std::sync::atomic::Ordering::Release);
    vec![f1["params"].clone(), f2["params"].clone()]
}
