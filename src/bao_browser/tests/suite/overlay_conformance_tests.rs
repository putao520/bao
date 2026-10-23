// REQ-CDP-007-C6 Overlay.highlightNode — real-engine pixel acceptance (E4 of
// the overlay contract chain, design /tmp/cdp-007-overlay-design.md §7).
//
// Driver: the same wire a real client speaks — bao_cdp_client's
// WebSocketTransport shape against the production CdpServer + real servo
// (the harness shape of playwright_full_flow_loop_tests). The acceptance
// chain runs over the CDP `Page.captureScreenshot` face:
//
//   Overlay.highlightNode{nodeId, highlightSpec} → screenshot → the element's
//   box-model quads carry the spec colors; everything outside them is
//   untouched (== baseline) → Overlay.hideHighlight restores the pre-highlight
//   frame → a navigation clears the slot (recapture stays clean) → an
//   unresolvable nodeId is an explicit error and arms nothing.
//
// Pixel contract: each layer gets a FULLY OPAQUE pure color (a=1.0), and the
// raster blends straight-alpha source-over where `sa==255` returns `src`
// verbatim (bao_browser/src/overlay.rs `blend_over`) — so every pixel inside
// a layer's visible band is EXACTLY the layer color (its fill and its opaque
// 1px boundary ring share the same RGB), which makes the assertions exact
// integer band areas instead of sampled tolerances:
//
//   content 120x80 = 9600   (red)
//   padding ring 140x100 − 120x80 = 4400   (green)
//   border ring 160x120 − 140x100 = 5200   (blue)
//   margin ring 170x130 − 160x120 = 2900   (yellow)
//
// The red band's bounding box must equal the CDP `DOM.getBoxModel` content
// quad (and the yellow band's the margin quad) — the quad→screenshot
// coordinate identity (viewport == screenshot, no transform, design §4/§9).
// The geometry is cross-checked against the page's own
// getBoundingClientRect, not against hardcoded CSS arithmetic.
// @trace REQ-CDP-007 [req:REQ-CDP-007] [criterion:6] [level:e2e]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BaoWsRegistry, BrowserRuntime, PageConfig};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
use bao_cdp_client::bridge::translate;
use bun_uws::ws_client::{RecvOutcome, WebSocketClient};
use cdp_server::{CdpServer, EventSender, ServerConfig};
use image::RgbaImage;
use serde_json::{json, Value};

/// The exact band colors (opaque ⇒ the raster writes them verbatim).
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// The CDP Overlay.HighlightSpec under test: four opaque pure colors.
fn spec() -> Value {
    json!({
    "contentColor": {"r": 1, "g": 0, "b": 0, "a": 1},
    "paddingColor": {"r": 0, "g": 1, "b": 0, "a": 1},
    "borderColor":  {"r": 0, "g": 0, "b": 1, "a": 1},
    "marginColor":  {"r": 1, "g": 1, "b": 0, "a": 1}
    })
}

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Loopback origin serving the two test documents (http, not data: — servo's
/// screenshot path needs a painted page; data: pages may never paint, W43).
fn spawn_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let req = String::from_utf8_lossy(&buf[..buf.len().min(2048)]).to_string();
            // Page 1: the highlighted element. Single line, no '#' characters.
            // Page 2: a different shallow document (the navigation-invalidate
            // scenario — no such element, and the positional id space of the
            // fresh document resolves the old highlight target to nothing).
            let path = req.split(' ').nth(1).unwrap_or("").to_string();
            let body = if path.starts_with("/overlay-2") {
                "<html><head><style>body{margin:0}</style></head><body><p id=other>second</p></body></html>".to_string()
            } else {
                "<html><head><style>body{margin:0}</style></head><body><div id=t style=\"position:absolute;left:50px;top:40px;width:120px;height:80px;border:10px solid transparent;padding:10px;margin:5px\"></div></body></html>".to_string()
            };
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = s.write_all(body.as_bytes());
        }
    });
    port
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

/// Capture one screenshot through the CDP face and decode it. The first
/// capture is the channel probe: a bridge ERROR response there means the GL
/// composite channel is dead in this environment (#40 headless family) and
/// the caller loud-skips; after the channel has proven alive, an error is a
/// hard failure. Data that decodes but is not PNG is always a hard failure.
fn capture_png(t: &mut WsCdp) -> Result<Option<RgbaImage>, String> {
    for _ in 0..20 {
        let r = send(t, "Page.captureScreenshot", json!({ "format": "png" }));
        if r.get("error").is_some() {
            return Ok(None);
        }
        match r["result"]["data"].as_str() {
            Some(data) => {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap_or_default();
                if bytes.len() >= 8 && bytes[0] == 0x89 && bytes[1] == 0x50 {
                    let img = image::load_from_memory(&bytes)
                        .map_err(|e| format!("screenshot PNG decode failed: {e}"))?
                        .to_rgba8();
                    return Ok(Some(img));
                }
                // Painted-but-not-yet frame (mid-navigation placeholder) — retry.
            }
            None => return Err(format!("screenshot response lacks data: {r}")),
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err("screenshot never produced a decodable PNG frame (20 retries)".into())
}

fn count_color(img: &RgbaImage, c: [u8; 4]) -> u64 {
    img.pixels().filter(|p| p.0 == c).count() as u64
}

/// Inclusive bounding box of every pixel exactly equal to `c`.
fn color_bbox(img: &RgbaImage, c: [u8; 4]) -> Option<(u32, u32, u32, u32)> {
    let mut bbox = None;
    for (x, y, p) in img.enumerate_pixels() {
        if p.0 == c {
            bbox = match bbox {
                None => Some((x, y, x, y)),
                Some((x0, y0, x1, y1)) => Some((x0.min(x), y0.min(y), x1.max(x), y1.max(y))),
            };
        }
    }
    bbox
}

/// The four exact band areas of the test document's box model (see the
/// module comment): content area and the three ring areas.
const BAND_AREAS: [(&str, [u8; 4], u64); 4] = [
    ("content", RED, 9600),
    ("padding", GREEN, 4400),
    ("border", BLUE, 5200),
    ("margin", YELLOW, 2900),
];

fn assert_band_counts(img: &RgbaImage, phase: &str) {
    for (name, color, want) in BAND_AREAS {
        let got = count_color(img, color);
        assert_eq!(
            got, want,
            "{phase}: {name} band must be exactly {want} pixels of {color:?}, got {got} \
             (quad-area mismatch = geometry or raster drift)"
        );
    }
}

fn assert_no_highlight(img: &RgbaImage, phase: &str) {
    for (name, color, _) in BAND_AREAS {
        let got = count_color(img, color);
        assert_eq!(
            got, 0,
            "{phase}: no {name}-color pixel may exist, got {got} (highlight must be gone)"
        );
    }
}

/// Parse one CDP box-model quad ([x1,y1, x2,y2, x3,y3, x4,y4], axis-aligned)
/// into (x, y, w, h).
fn quad_xywh(quad: &Value) -> (f64, f64, f64, f64) {
    let c: Vec<f64> = quad
        .as_array()
        .expect("quad is an 8-number array")
        .iter()
        .map(|v| v.as_f64().expect("quad coordinate"))
        .collect();
    assert_eq!(c.len(), 8, "quad is a 4-corner list");
    (c[0], c[1], c[4] - c[0], c[5] - c[1])
}

struct WsCdp {
    client: WebSocketClient,
    next_id: i64,
    /// Events seen while waiting for command responses — buffered (not
    /// dropped): a discarded event edge is unrecoverable on the wire
    /// (playwright_full_flow_loop_tests e124 note).
    pending_events: Vec<Value>,
}

impl WsCdp {
    fn connect(url: &str) -> Self {
        // Bounded connect retry (full-suite load can lag the server's bind
        // behind the client's first connect — BCE-20260824-CDPWS-CONNECT).
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
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match self.client.recv().expect("ws recv") {
                RecvOutcome::Message(_op, payload) => {
                    let v: Value = serde_json::from_slice(&payload).expect("valid json frame");
                    if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                        return v;
                    }
                    if v.get("method").is_some() && self.pending_events.len() < 1000 {
                        self.pending_events.push(v);
                    }
                }
                RecvOutcome::Timeout => continue,
                RecvOutcome::Closed => panic!("ws closed waiting for {method} response"),
            }
        }
        panic!("timeout waiting for {method} response");
    }
}

#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn overlay_pixel_acceptance_full_chain() {
    let origin = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            panic!("runtime init failed: {e}");
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            // 显式 stealth-free:本测试用 `getBoundingClientRect().width`
            // 字符串精确落地判据(settle "160")与 ≤1px 的 CDP quad/rect 对齐
            // —— 默认身份(裁决 2026-10-21)的 clientrects ±0.5px 噪声钩子会
            // 让精确宽度读取永不命中,故显式退出。
            stealth_disabled: true,
            ..Default::default()
        })
        .expect("create_page");

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(60));
    let (event_tx, servo_event_rx) = bao_ipc::unbounded::<bao_cdp_client::bridge::ServoEvent>();
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

    // Client on a helper thread; main thread owns the pump loop (Rc<Servo>).
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());
    let client = std::thread::spawn(move || client_scenarios(&ws_url, origin, tx));

    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let result = loop {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        // Event pump: servo events → CDP events → session-tagged broadcast.
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
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Err(payload) = client.join() {
                    std::panic::resume_unwind(payload);
                }
                panic!("[overlay] client thread ended without a result or panic");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if std::time::Instant::now() > deadline {
            panic!("[overlay] acceptance client did not finish within 180s");
        }
        std::thread::yield_now();
    };
    let _ = client.join();
    runtime
        .page_pool()
        .close_page(page.id())
        .expect("close page");
    result.expect("overlay pixel acceptance chain must pass");
}

fn client_scenarios(ws_url: &str, origin: u16, tx: std::sync::mpsc::Sender<Result<(), String>>) {
    let run = (|| -> Result<(), String> {
        let mut t = WsCdp::connect(ws_url);

        // CDP semantics: enable the domains a real client enables first.
        let r = send(&mut t, "Page.enable", json!({}));
        if r.get("error").is_some() {
            return Err(format!("Page.enable: {r}"));
        }
        let r = send(&mut t, "Overlay.enable", json!({}));
        if r.get("error").is_some() {
            return Err(format!("Overlay.enable: {r}"));
        }

        // Navigate to the highlight document and settle on content only the
        // real document has (the div's border-box width = 120 + 2*10 + 2*10).
        let r = send(
            &mut t,
            "Page.navigate",
            json!({ "url": format!("http://127.0.0.1:{origin}/overlay-1") }),
        );
        if r.get("error").is_some() {
            return Err(format!("navigate overlay-1: {r}"));
        }
        settle(
            &mut t,
            "(function(){var el=document.getElementById('t');return el?String(el.getBoundingClientRect().width):'none';})()",
            "160",
        )
        .map_err(|e| format!("overlay-1 settle: {e}"))?;

        // Screenshot channel probe (see capture_png): a dead composite
        // channel loud-skips the whole face (suite precedent) instead of
        // failing on an environment limitation.
        let Some(baseline) = capture_png(&mut t)? else {
            eprintln!(
                "[overlay] screenshot channel unavailable in this environment \
                 (bridge error on first capture) — pixel acceptance skipped per \
                 servo_render_pipeline §9 probe+skip precedent"
            );
            return Ok(());
        };

        // Baseline precondition: no band color anywhere before the highlight.
        assert_no_highlight(&baseline, "baseline (pre-highlight)");

        // Node identity through the DOM face (the canonical positional id).
        let qs = send(&mut t, "DOM.querySelector", json!({ "selector": "#t" }));
        if qs.get("error").is_some() {
            return Err(format!("DOM.querySelector: {qs}"));
        }
        let node_id = qs["result"]["nodeId"]
            .as_i64()
            .expect("querySelector returns nodeId");
        assert!(node_id >= 1, "live element has a real canonical id");

        // Geometry provenance: the CDP border quad must equal the page's own
        // border box (getBoundingClientRect) — one identity, two faces.
        let bm = send(&mut t, "DOM.getBoxModel", json!({ "nodeId": node_id }));
        if bm.get("error").is_some() {
            return Err(format!("DOM.getBoxModel: {bm}"));
        }
        let (bx, by, bw, bh) = quad_xywh(&bm["result"]["model"]["border"]);
        let rect = eval_js(
            &mut t,
            "(function(){var r=document.getElementById('t').getBoundingClientRect();return JSON.stringify({x:r.left,y:r.top,w:r.width,h:r.height});})()",
        );
        let rect: Value = serde_json::from_str(&rect).expect("rect JSON");
        let (rx, ry, rw, rh) = (
            rect["x"].as_f64().unwrap(),
            rect["y"].as_f64().unwrap(),
            rect["w"].as_f64().unwrap(),
            rect["h"].as_f64().unwrap(),
        );
        assert!(
            (bx - rx).abs() < 1.0
                && (by - ry).abs() < 1.0
                && (bw - rw).abs() < 1.0
                && (bh - rh).abs() < 1.0,
            "CDP border quad ({bx},{by},{bw},{bh}) must match the page rect ({rx},{ry},{rw},{rh})"
        );
        // The whole highlight must sit inside the captured frame (the band
        // counts below assume no clipping).
        let (fw, fh) = (baseline.width() as f64, baseline.height() as f64);
        assert!(
            bx >= 0.0 && by >= 0.0 && bx + bw <= fw && by + bh <= fh,
            "highlight box ({bx},{by},{bw},{bh}) must fit the captured frame {fw}x{fh}"
        );

        // ── Scenario 1: highlight → the quads carry the spec colors ────
        let r = send(
            &mut t,
            "Overlay.highlightNode",
            json!({ "nodeId": node_id, "highlightSpec": spec() }),
        );
        if r.get("error").is_some() {
            return Err(format!("Overlay.highlightNode: {r}"));
        }
        let highlighted = capture_png(&mut t)?.expect("channel proven alive");
        assert_band_counts(&highlighted, "highlighted");

        // Coordinate identity: the red band's bounding box IS the content
        // quad; the yellow band's IS the margin quad (viewport == screenshot).
        let (cx, cy, cw, ch) = quad_xywh(&bm["result"]["model"]["content"]);
        let red_bbox = color_bbox(&highlighted, RED).expect("red band exists");
        assert_eq!(
            red_bbox,
            (
                cx.round() as u32,
                cy.round() as u32,
                (cx + cw).round() as u32 - 1,
                (cy + ch).round() as u32 - 1
            ),
            "red band bbox {red_bbox:?} must equal the content quad ({cx},{cy},{cw},{ch})"
        );
        let (mx, my, mw, mh) = quad_xywh(&bm["result"]["model"]["margin"]);
        let yellow_bbox = color_bbox(&highlighted, YELLOW).expect("yellow band exists");
        assert_eq!(
            yellow_bbox,
            (
                mx.round() as u32,
                my.round() as u32,
                (mx + mw).round() as u32 - 1,
                (my + mh).round() as u32 - 1
            ),
            "yellow band bbox {yellow_bbox:?} must equal the margin quad ({mx},{my},{mw},{mh})"
        );

        // Outside the highlight the frame is untouched: exact band counts
        // already prove nothing extra got painted; spot-check equality with
        // the baseline frame for the reading.
        for (px, py) in [(10u32, 10u32), (300, 200)] {
            assert_eq!(
                highlighted.get_pixel(px, py),
                baseline.get_pixel(px, py),
                "pixel ({px},{py}) outside the highlight must equal the baseline"
            );
        }

        // ── Scenario 2: hideHighlight → the pre-highlight frame returns ─
        let r = send(&mut t, "Overlay.hideHighlight", json!({}));
        if r.get("error").is_some() {
            return Err(format!("Overlay.hideHighlight: {r}"));
        }
        let restored = capture_png(&mut t)?.expect("channel proven alive");
        assert_no_highlight(&restored, "after hideHighlight");
        for (px, py) in [(10u32, 10u32), (300, 200)] {
            assert_eq!(
                restored.get_pixel(px, py),
                baseline.get_pixel(px, py),
                "restored frame pixel ({px},{py}) must equal the baseline"
            );
        }

        // ── Scenario 3: navigation clears the slot (no stale highlight) ─
        let r = send(
            &mut t,
            "Overlay.highlightNode",
            json!({ "nodeId": node_id, "highlightSpec": spec() }),
        );
        if r.get("error").is_some() {
            return Err(format!("re-arm highlightNode: {r}"));
        }
        let rearmed = capture_png(&mut t)?.expect("channel proven alive");
        assert_band_counts(&rearmed, "re-armed before navigation");

        let r = send(
            &mut t,
            "Page.navigate",
            json!({ "url": format!("http://127.0.0.1:{origin}/overlay-2") }),
        );
        if r.get("error").is_some() {
            return Err(format!("navigate overlay-2: {r}"));
        }
        settle(
            &mut t,
            "(function(){var el=document.getElementById('other');return el?el.textContent:'none';})()",
            "second",
        )
        .map_err(|e| format!("overlay-2 settle: {e}"))?;
        let post_nav = capture_png(&mut t)?.expect("channel proven alive");
        assert_no_highlight(&post_nav, "after navigation (slot cleared)");

        // ── Scenario 4: unresolvable nodeId → explicit error, arms nothing ─
        // Positional id 999999 walks past the shallow document tree and
        // resolves to nothing; the eager identity validation must reject it
        // instead of silently arming a dead highlight (design §6).
        //
        // Error class -32000: the page's identity verdicts are domain-owned —
        // the browser side wraps the `__bao_err` outcome in
        // `BridgeError::domain` (the Overlay identity resolution in
        // cdp_handler.rs), and `bridge_send` maps
        // `BridgeErrorKind::DomainError` to -32000, the code Chrome's
        // DevTools uses for "No node with given id found" (transport-class
        // plumbing failures stay -32603). The message names the dead
        // identity — the semantic anchor.
        let r = send(
            &mut t,
            "Overlay.highlightNode",
            json!({ "nodeId": 999_999, "highlightSpec": spec() }),
        );
        let err = r
            .get("error")
            .unwrap_or_else(|| panic!("unknown nodeId must fail explicitly, got: {r}"));
        assert_eq!(
            err["code"], -32000,
            "domain-owned identity verdict → Chrome's ServerError class: {err}"
        );
        assert!(
            err["message"]
                .as_str()
                .unwrap_or("")
                .contains("No node with given id found"),
            "error names the dead identity: {err}"
        );
        let after_err = capture_png(&mut t)?.expect("channel proven alive");
        assert_no_highlight(&after_err, "after rejected highlightNode (nothing armed)");

        Ok(())
    })();
    let _ = tx.send(run);
}
