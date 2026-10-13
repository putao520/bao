// CDP mirror-loop integration — the 2026-10-13 release-gate face: frames
// flow OUT through `Page.screencastFrame` while CDP Input commands flow
// BACK IN, the page state changes, and the FOLLOWING frame reflects that
// change. Both halves are established capabilities (REQ-CDP-009 screencast
// trio — screencast_tests.rs; REQ-CDP-007 Input delivery — cdp_handler.rs
// `cmd_mouse_event` / `cmd_key_event` / `cmd_insert_text`); what did not
// exist is the LOOP closed inside one test: out-frame → in-input → page
// change → out-frame-reflects-it.
//
// Harness shape (the suite's screencast_tests.rs convention): the
// `BrowserRuntime` is `!Send`, so the MAIN thread owns the pump loop while
// a helper thread holds the memory:// CDP client; every wait is
// pump-driven (each client command is answered by a pump tick), never a
// bare sleep. All input goes through the REAL CDP Input face —
// `Input.dispatchMouseEvent` / `Input.dispatchKeyEvent` /
// `Input.insertText` → `BridgeCommand` → servo's input pipeline (trusted
// DOM events, no `el.value` JS shortcut) — and all observation through the
// REAL frame face. Element coordinates come from the page's own
// `getBoundingClientRect` geometry, never hardcoded.
//
// @trace REQ-CDP-009 [level:e2e]
// @trace REQ-CDP-007 [level:e2e]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use bao_browser::{screencast, BaoConfig, BrowserRuntime, PageConfig};
use bao_cdp_client::Browser;
use base64::Engine;
use serde_json::{json, Value};

/// Static marker document. The click target cycles its background through
/// three colors (every click is a real pixel change — the ack-gate test
/// needs each stimulus to be visually distinct), records trusted keydowns,
/// and exposes `window.__state()` as the page-state oracle. No animations,
/// no rAF: the only pixel changes come from the test's own Input stimuli.
const HTML: &str = r#"<!DOCTYPE html><html><body style="margin:0">
<div id="t" style="width:160px;height:120px;background:rgb(255,0,0)"></div>
<input id="in" style="width:200px;height:30px;font-size:16px">
<script>
window.__clicks = 0;
var colors = ["rgb(0,0,255)", "rgb(0,128,0)", "rgb(255,255,0)"];
document.getElementById("t").onclick = function () {
  window.__clicks++;
  this.style.background = colors[(window.__clicks - 1) % colors.length];
};
window.__keys = 0;
document.addEventListener("keydown", function (e) {
  if (e.isTrusted) window.__keys++;
});
window.__state = function () {
  return JSON.stringify({
    clicks: window.__clicks,
    keys: window.__keys,
    value: document.getElementById("in").value,
    focused: document.activeElement === document.getElementById("in")
  });
};
</script>
</body></html>"#;

/// Percent-encode an HTML document into a `data:text/html` URL (the suite's
/// editing_e2e shape — no subresources, no network).
fn data_url(html: &str) -> String {
    let mut encoded = String::with_capacity(html.len() * 3);
    for b in html.bytes() {
        match b {
            b'#' => encoded.push_str("%23"),
            b'%' => encoded.push_str("%25"),
            b'\n' => encoded.push_str("%0A"),
            b'\r' => encoded.push_str("%0D"),
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            b'\'' => encoded.push_str("%27"),
            _ => encoded.push(b as char),
        }
    }
    format!("data:text/html;charset=utf-8,{}", encoded)
}

/// Fresh runtime + settled page (screencast_tests::setup shape: the 800ms
/// pre-pump settles the initial load's rendering so screencast sessions
/// observe only test-driven changes).
fn setup() -> (BrowserRuntime, bao_browser::PageHandle) {
    // Single-process suite isolation (see screencast::drop_all_sessions).
    screencast::drop_all_sessions();
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("runtime init");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(data_url(HTML)),
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
        .name("mirror-loop-client".into())
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

/// `Runtime.evaluate` returning a JSON-parsed value (returnByValue=true →
/// `result.result.value` carries the stringified payload).
fn eval_json(b: &mut Browser, expression: &str) -> Value {
    let r = b
        .send_command(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .expect("Runtime.evaluate succeeds");
    // The production evaluate response carries `exceptionDetails` as an
    // explicit top-level member — `null` when the evaluation succeeded.
    let errored = r.get("error").is_some()
        || r.get("exceptionDetails").is_some_and(|d| !d.is_null());
    assert!(!errored, "evaluate ok: {r} ({expression})");
    // The memory:// bridge returns the handler payload unwrapped
    // (`result: {type, value}`); the WS JSON-RPC face adds one envelope
    // (`result: {result: {type, value}}`). Accept both.
    let s = r["result"]["result"]["value"]
        .as_str()
        .or_else(|| r["result"]["value"].as_str())
        .unwrap_or("");
    serde_json::from_str(s).unwrap_or(Value::Null)
}

fn state(b: &mut Browser) -> Value {
    eval_json(b, "window.__state()")
}

/// Poll `window.__state()` (each poll is a pump-answered round trip) until
/// `accept` holds; returns the last observed state either way.
fn poll_state(b: &mut Browser, budget: Duration, mut accept: impl FnMut(&Value) -> bool) -> Value {
    let deadline = Instant::now() + budget;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = state(b);
        if accept(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    last
}

/// Real geometry from the page: the element's bounding-box center in CSS px
/// (the same viewport space CDP Input x/y addresses).
fn element_center(b: &mut Browser, id: &str) -> (f64, f64) {
    let v = eval_json(
        b,
        &format!(
            "(function(){{var e=document.getElementById('{id}');\
             var r=e.getBoundingClientRect();\
             return JSON.stringify({{x:r.x+r.width/2,y:r.y+r.height/2}});}})()"
        ),
    );
    let x = v["x"].as_f64().unwrap_or(0.0);
    let y = v["y"].as_f64().unwrap_or(0.0);
    assert!(
        x > 0.0 && y > 0.0,
        "element #{id} must have real geometry, got ({x},{y}) from {v}"
    );
    (x, y)
}

/// CDP click at (x, y): a mousePressed/mouseReleased pair — servo
/// synthesizes the trusted `click` from the Down+Up pair at the same point.
fn cdp_click(b: &mut Browser, x: f64, y: f64) {
    for ty in ["mousePressed", "mouseReleased"] {
        let r = b
            .send_command(
                "Input.dispatchMouseEvent",
                json!({
                    "type": ty,
                    "x": x,
                    "y": y,
                    "button": "left",
                    "clickCount": 1
                }),
            )
            .expect("Input.dispatchMouseEvent succeeds");
        assert_eq!(r, json!({}), "{ty} returns an empty result");
    }
}

/// Collect screencastFrame params until `stop_when` says enough (the
/// screencast_tests::collect_frames shape; acking variant below).
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

/// Acking variant — every arrival is acked on the spot, keeping the ack
/// gate out of tests that exercise the loop's frame half.
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

/// Drain load-tail frames, acking throughout, until `quiet` passes with no
/// arrival (screencast_tests::wait_quiet_acking shape).
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
// The loop, click face: startScreencast → frame(red) → Input click at the
// element's real center → page state changes (clicks=1, real geometry
// proven) → the NEXT frame arrives and decodes DIFFERENT (blue).
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:帧经 Page.screencastFrame 事件下发(data 含 base64 帧与 metadata)]
// @trace REQ-CDP-007 [criterion:Input.dispatchMouseEvent 注入鼠标事件]
#[test]
fn cdp_screencast_input_loop_click_changes_page_and_next_frame() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(40), |b| {
        start_screencast(b, json!({ "format": "png" }));

        // The pre-click frame: first arrival (red state), acked. 20s — this
        // wait is engine cold-start + first-GL-composite boot margin, whose
        // variance on shared hosts swamps a tight budget (V: 41.8s vs 3.8s
        // isolated); the behavior budgets below stay tight.
        let before = collect_frames_acking(b, Duration::from_secs(20), |f| !f.is_empty())
            .into_iter()
            .next()
            .expect("pre-click frame must arrive on session start");
        let before_bytes = decode_frame(&before);
        assert!(
            before_bytes.len() > 100,
            "pre-click frame carries real image data ({}B)",
            before_bytes.len()
        );
        // Drain load-tail repaints so the next arrival is click-driven.
        let _tail = wait_quiet_acking(b, Duration::from_millis(700));

        // Input back in: click the target at its REAL center.
        let (cx, cy) = element_center(b, "t");
        assert_eq!(
            state(b)["clicks"].as_u64(),
            Some(0),
            "no clicks before the Input stimulus"
        );
        cdp_click(b, cx, cy);

        // The page state changed through the trusted click (servo
        // synthesizes it from the Down+Up pair — no JS shortcut exists on
        // this face).
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["clicks"].as_u64().unwrap_or(0) >= 1
        });
        assert_eq!(
            st["clicks"].as_u64(),
            Some(1),
            "CDP Input click must drive the page's onclick: {st}"
        );

        // And the FOLLOWING frame reflects it: first post-click arrival,
        // decodable and pixel-distinct from the pre-click frame.
        let after = collect_frames_acking(b, Duration::from_secs(6), |f| !f.is_empty())
            .into_iter()
            .next()
            .expect("post-click frame must arrive (the change is real)");
        let after_bytes = decode_frame(&after);
        assert_eq!(
            &after_bytes[0..4],
            &[0x89, 0x50, 0x4E, 0x47],
            "post-click frame is PNG"
        );
        assert_ne!(
            before_bytes, after_bytes,
            "the post-click frame must differ from the pre-click frame"
        );
        stop_screencast(b);
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// The loop, keyboard face: an Input click focuses the input (servo's
// mousedown runs the focusing steps on click-focusable targets) →
// Input.insertText types through the real text-input engine (trusted
// keydowns, .value updates) → Input.dispatchKeyEvent Backspace deletes →
// value observable. No script focus() — the whole flow is Input-driven.
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-007 [criterion:Input.dispatchKeyEvent 注入键盘事件]
#[test]
fn cdp_screencast_input_loop_typing_updates_input_value() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(30), |b| {
        // Click the field at its real center: place the caret the way a real
        // mirror-view user would.
        let (fx, fy) = element_center(b, "in");
        cdp_click(b, fx, fy);
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["focused"].as_bool() == Some(true)
        });
        assert_eq!(
            st["focused"].as_bool(),
            Some(true),
            "CDP Input click on the input must focus it: {st}"
        );

        // insertText: the dedicated non-physical-key insertion face (per-char
        // trusted Down/Up inside the bridge).
        let r = b
            .send_command("Input.insertText", json!({ "text": "bao" }))
            .expect("Input.insertText succeeds");
        assert_eq!(r, json!({}), "insertText returns an empty result");
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["value"].as_str() == Some("bao")
        });
        assert_eq!(
            st["value"].as_str(),
            Some("bao"),
            "Input.insertText must update input.value through the engine: {st}"
        );
        let keys_after_insert = st["keys"].as_u64().unwrap_or(0);
        assert!(
            keys_after_insert >= 3,
            "typed characters must arrive as trusted keydowns, got {keys_after_insert}: {st}"
        );

        // dispatchKeyEvent Backspace: the physical-key face deletes the last
        // character.
        for ty in ["keyDown", "keyUp"] {
            let r = b
                .send_command(
                    "Input.dispatchKeyEvent",
                    json!({ "type": ty, "key": "Backspace", "code": "Backspace" }),
                )
                .expect("Input.dispatchKeyEvent succeeds");
            assert_eq!(r, json!({}), "{ty} returns an empty result");
        }
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["value"].as_str() == Some("ba")
        });
        assert_eq!(
            st["value"].as_str(),
            Some("ba"),
            "Input.dispatchKeyEvent Backspace must delete through the engine: {st}"
        );
        let keys_after_bs = st["keys"].as_u64().unwrap_or(0);
        assert!(
            keys_after_bs > keys_after_insert,
            "the Backspace keyDown must be trusted too: {st}"
        );
    });
    out.expect("client phase completed");
}

// ════════════════════════════════════════════════════════════════════
// The loop, gate face: after Input-driven changes the ack throttle still
// holds — one un-acked frame in flight, zero leaked frames for further
// Input changes, ack releases exactly one.
// ════════════════════════════════════════════════════════════════════

// @trace REQ-CDP-009 [criterion:Page.screencastFrameAck 确认参与出帧节流(未 ack 不无限出帧)]
// @trace REQ-CDP-007 [criterion:Input.dispatchMouseEvent 注入鼠标事件]
#[test]
fn cdp_screencast_input_loop_ack_gate_holds_under_input_stimulus() {
    let (runtime, _page) = setup();
    let out = pump_with_client(&runtime, Duration::from_secs(45), |b| {
        start_screencast(b, json!({ "format": "png" }));

        // Settle: first frame acked, load tail drained. Same boot margin as
        // the click test's pre-click wait (20s); behavior budgets stay tight.
        let first = collect_frames_acking(b, Duration::from_secs(20), |f| !f.is_empty())
            .into_iter()
            .next()
            .expect("first frame must arrive");
        let first_bytes = decode_frame(&first);
        let _tail = wait_quiet_acking(b, Duration::from_millis(700));

        // Input change #1 (red → blue): exactly one frame flows, then the
        // un-acked gate holds it.
        let (cx, cy) = element_center(b, "t");
        cdp_click(b, cx, cy);
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["clicks"].as_u64() == Some(1)
        });
        assert_eq!(st["clicks"].as_u64(), Some(1), "click #1 landed: {st}");
        let held = collect_frames(b, Duration::from_secs(6), |f| f.len() >= 1)
            .into_iter()
            .next()
            .expect("the change frame must flow before the gate re-arms");
        let held_bytes = decode_frame(&held);
        let sid = held["sessionId"].as_u64().expect("held frame sessionId");
        assert_ne!(
            first_bytes, held_bytes,
            "the held frame must reflect the Input-driven change"
        );

        // Input change #2 (blue → green) with the gate still closed: nothing
        // may leak.
        cdp_click(b, cx, cy);
        let st = poll_state(b, Duration::from_secs(8), |s| {
            s["clicks"].as_u64() == Some(2)
        });
        assert_eq!(st["clicks"].as_u64(), Some(2), "click #2 landed: {st}");
        let leaked = collect_frames(b, Duration::from_millis(800), |_| false);
        assert!(
            leaked.is_empty(),
            "un-acked session must not leak frames for Input changes ({} leaked)",
            leaked.len()
        );

        // Ack releases exactly one frame (the latest state).
        ack(b, sid);
        let released = collect_frames(b, Duration::from_secs(4), |f| !f.is_empty());
        assert_eq!(
            released.len(),
            1,
            "ack releases exactly the single held frame, got {}",
            released.len()
        );
        assert_eq!(
            &decode_frame(&released[0])[0..4],
            &[0x89, 0x50, 0x4E, 0x47],
            "released frame is PNG"
        );
        stop_screencast(b);
    });
    out.expect("client phase completed");
}
