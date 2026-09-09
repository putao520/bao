// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:13] [criterion:14] [level:integration]
// OffscreenCanvas worker-realm live tests for REQ-BRW-004 criteria #13/#14.
//
// SPEC criterion under test:
//   C13: "CRIT-STL-WK OffscreenCanvas: worker 内 new OffscreenCanvas(w,h) +
//         getContext('2d') + 绘制 + getImageData 可用 (用户裁决 2026-09-09
//         破例立法; E26 侦察: 上游实现完整, 唯一门 = prefs.rs 默认 false)"
//   C14: WebGL1 worker 通道 — worker 内 new OffscreenCanvas + getContext('webgl')
//         非 null + 基本 getParameter (用户裁决 2026-09-09 破例 vendor patch;
//         E26 断点: WorkerGlobalScopeInit.webgl_chan 在 new_inherited 被丢弃 /
//         WebGLRenderingContext::new_inherited 锚死 &Window / offscreencanvas
//         WebGL 路径 Window downcast)
//
// Mechanism under test: the ONLY gate is the WebIDL
// `Pref="dom_offscreen_canvas_enabled"` exposure check (per-realm). The
// vendor implementation is complete and the 2d render path has no Window
// dependency (`canvas_state` rides `script_to_constellation_chan`, which the
// worker `GlobalScope` owns too). Bao flips the pref at its embedder
// initialization site (`bao_browser/src/lib.rs`, same shape as the
// `dom_indexeddb_enabled` flip) — vendor defaults stay untouched.
//
// Live path under test (same shape as worker_fingerprint_consistency_tests):
//   page JS `new Worker(data:URL)` → servo native Worker thread →
//   `new OffscreenCanvas(64,48)` → `getContext('2d')` → `fillRect` →
//   `getImageData` → digest string via `self.postMessage` → main thread
//   `w.onmessage` → `window.__ocResult` → test polls via evaluate_js_web.
//   Pixel assertions are EXACT (fully opaque fill ⇒ no premultiply drift).
//
// Environment gating:
//   Real servo rendering requires DISPLAY (Xvfb). Skipped unless
//   BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_offscreencanvas)'

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use bao_stealth::StealthProfile;
use std::sync::Mutex;
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

/// Serializes all servo-touching tests in this binary (servo single-instance).
/// BCE-20260627-009.
static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

/// Skip-guard helper: returns true if the test should skip (no network/display).
fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Acquire the global serializer lock for the full test duration.
fn lock_serializer() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: `TEST_SERIALIZER` is a `static`, so the lock's lifetime is
    // bounded by the process. The guard is only dropped when the test function
    // returns. This transmute extends the lifetime bound to 'static, matching
    // the actual underlying static Mutex.
    unsafe {
        std::mem::transmute::<std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'static, ()>>(
            guard,
        )
    }
}

/// URL-encode a JS worker body for data: URL (minimal percent-encoding).
/// `#` in particular MUST be encoded (it would truncate the URL as a
/// fragment), which matters here because the body carries `#FF0000`.
fn encode_worker_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

/// The servo JS bridge may return a JS string value as `"..."` (quoted). Strip
/// a single outer quote pair if present so the caller sees the raw value.
fn unquote_bridge(mut s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        s = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    s
}

/// Create a live BaoRuntime + one page with the given stealth profile.
/// Fails the test (not skip) once the gated environment has been established.
fn live_page(profile: Option<StealthProfile>) -> (BaoRuntime, bao_browser::PageHandle) {
    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: profile,
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    (runtime, page)
}

/// Worker script body: run the full OffscreenCanvas 2d pipeline INSIDE the
/// Worker thread and post a compact digest back. Every failure mode gets an
/// explicit marker (never silent): interface absent / size wrong / null
/// context / thrown exception — each distinguishable in the assertion below.
const WORKER_OC_BODY: &str = r#"
var __r = (function () {
  try {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    var c = new OffscreenCanvas(64, 48);
    if (c.width !== 64 || c.height !== 48) { return 'BAD-SIZE:' + c.width + 'x' + c.height; }
    var ctx = c.getContext('2d');
    if (!ctx) { return 'NULL-CTX'; }
    if (ctx.canvas !== c) { return 'BAD-BACKREF'; }
    ctx.fillStyle = '#FF0000';
    ctx.fillRect(0, 0, 64, 48);
    var d = ctx.getImageData(0, 0, 1, 1).data;
    return 'OK:' + c.width + 'x' + c.height + ':' + d[0] + ',' + d[1] + ',' + d[2] + ',' + d[3];
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// Page-side driver: create the Worker from a data: URL and wire
/// `w.onmessage` → `window.__ocResult` (worker→main sink) and `w.onerror` →
/// an explicit `WORKER-ERROR:` marker (never silent).
fn make_worker_driver(encoded_body: &str) -> String {
    format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onmessage = function (e) {{ window.__ocResult = String(e.data); }};
                w.onerror = function (ev) {{
                    window.__ocResult = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                return 'worker-created';
            }} catch (e) {{
                window.__ocResult = 'CREATE-ERROR:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = encoded_body
    )
}

/// Poll `window.__ocResult` until the Worker posts its digest (or errors).
/// Returns the unquoted digest string, or None on timeout.
fn wait_for_worker_result(page: &bao_browser::PageHandle, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__ocResult") {
            let trimmed = s.trim();
            let is_set = !trimmed.is_empty() && trimmed != "null" && trimmed != "\"null\"";
            if is_set {
                return Some(unquote_bridge(trimmed.to_string()));
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Assert a worker-side OffscreenCanvas digest is the exact success shape
/// `OK:64x48:255,0,0,255` (opaque red fill read back through getImageData).
fn assert_worker_digest_ok(result: Option<String>) {
    let r = result.unwrap_or_else(|| {
        panic!(
            "worker OffscreenCanvas digest did not arrive within timeout — worker postMessage → \
             onmessage or worker execution is broken on the live path"
        )
    });
    assert!(
        !r.starts_with("WORKER-ERROR:"),
        "worker script threw before posting a digest: {r}"
    );
    assert!(
        !r.starts_with("CREATE-ERROR:"),
        "Worker constructor failed on the live path: {r}"
    );
    assert_eq!(
        r, "OK:64x48:255,0,0,255",
        "worker-realm OffscreenCanvas 2d pipeline must construct, draw and read back exactly"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §1 C13 — OffscreenCanvas available and functional INSIDE a Worker realm
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:13] worker-realm OffscreenCanvas 2d pipeline
/// (live, servo-native Worker thread, stealth page)
///
/// The whole pipeline runs inside the Worker: interface exposure (the pref
/// gate), construction, getContext('2d'), fillRect, getImageData readback.
/// The main thread only receives the digest string — every asserted fact is
/// computed in the Worker realm.
#[test]
fn c13_worker_offscreencanvas_2d_pipeline_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let _ = page.evaluate_js_web("window.__ocResult = null;");
    let body = encode_worker_body(WORKER_OC_BODY);
    let created = page.evaluate_js_web(&make_worker_driver(&body));
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "Worker creation must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("Worker creation dispatch failed: {e}"),
    }

    assert_worker_digest_ok(wait_for_worker_result(&page, Duration::from_secs(15)));
}

// ═══════════════════════════════════════════════════════════════════════
// §2 C13 — Window-realm exposure (direct probe that the pref flip landed)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:13] window-realm OffscreenCanvas exposure
/// (live, direct pref-flip probe)
///
/// Before the flip `OffscreenCanvas` is `undefined` in BOTH realms (the
/// WebIDL Pref gate removes the interface). This Window-realm probe isolates
/// "pref not applied" from "worker canvas path broken" if §1 ever regresses,
/// and proves the flip is per-embedder-config, not worker-specific.
#[test]
fn c13_window_realm_offscreencanvas_exposed_after_pref_flip() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let probe = r#"
(function () {
  try {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    var c = new OffscreenCanvas(32, 16);
    var ctx = c.getContext('2d');
    if (!ctx) { return 'NULL-CTX'; }
    ctx.fillStyle = 'rgb(0,255,0)';
    ctx.fillRect(0, 0, 32, 16);
    var d = ctx.getImageData(0, 0, 1, 1).data;
    return 'OK:' + d[0] + ',' + d[1] + ',' + d[2] + ',' + d[3];
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#;
    let raw = page
        .evaluate_js_web(probe)
        .expect("window-realm OffscreenCanvas probe evaluation must succeed");
    let r = unquote_bridge(raw.trim().to_string());
    assert_eq!(
        r, "OK:0,255,0,255",
        "window-realm OffscreenCanvas must be exposed and functional after the pref flip"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §3 C14 — WebGL1 OffscreenCanvas INSIDE a Worker realm (Bao vendor patch:
//          WorkerGlobalScope.webgl_chan + WebGLRenderingContext::new_inherited
//          decoupled from &Window + offscreencanvas worker dispatch)
// ═══════════════════════════════════════════════════════════════════════

/// Worker script body: run the OffscreenCanvas WebGL1 pipeline INSIDE the
/// Worker thread and post a compact digest back. Markers distinguish: interface
/// absent / null context / wrong drawing buffer size / non-string getParameter /
/// GL error / thrown exception. The pixel readback of a `clearColor(1,0,0,1)`
/// clear is asserted EXACT (fixed-point 1.0 ⇒ 255, no premultiply drift).
///
/// Note: the basic getParameter probe uses 0x1F01, which in servo's webidl is
/// RENDERER (desktop-GL constant ordering) and returns the driver's VERSION
/// string. `gl.VERSION` (0x1F02) currently returns `undefined` from servo's
/// native GetParameter in BOTH realms — a pre-existing upstream quirk, not a
/// worker-channel issue (verified identical on the Window path).
const WORKER_WEBGL_BODY: &str = r#"
var __r = (function () {
  try {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    var c = new OffscreenCanvas(64, 48);
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    if (gl.drawingBufferWidth !== 64 || gl.drawingBufferHeight !== 48) {
      return 'BAD-SIZE:' + gl.drawingBufferWidth + 'x' + gl.drawingBufferHeight;
    }
    var version = gl.getParameter(0x1F01);
    if (typeof version !== 'string' || version.length === 0) {
      return 'BAD-VERSION:' + String(version);
    }
    gl.clearColor(1, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    var px = new Uint8Array(4);
    gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
    var err = gl.getError();
    if (err !== gl.NO_ERROR) { return 'GL-ERROR:' + err; }
    return 'OK:' + px[0] + ',' + px[1] + ',' + px[2] + ',' + px[3] + ':' + version;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// @trace REQ-BRW-004 [criterion:14] worker-realm OffscreenCanvas WebGL1
/// pipeline (live, servo-native Worker thread, stealth page)
///
/// The whole pipeline runs inside the Worker: construction, getContext('webgl')
/// non-null (requires the inherited parent `Window` WebGL channel — before the
/// vendor patch this returned null because the WebGL1 path Window-downcast the
/// global), drawingBuffer geometry, getParameter(VERSION), clear + readPixels
/// round-trip, getError()==NO_ERROR.
#[test]
fn c14_worker_offscreencanvas_webgl1_pipeline_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let _ = page.evaluate_js_web("window.__ocResult = null;");
    let body = encode_worker_body(WORKER_WEBGL_BODY);
    let created = page.evaluate_js_web(&make_worker_driver(&body));
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "Worker creation must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("Worker creation dispatch failed: {e}"),
    }

    let r = wait_for_worker_result(&page, Duration::from_secs(30)).unwrap_or_else(|| {
        panic!(
            "worker WebGL digest did not arrive within timeout — worker postMessage → onmessage \
             or worker execution is broken on the live path"
        )
    });
    assert!(
        !r.starts_with("WORKER-ERROR:"),
        "worker script threw before posting a digest: {r}"
    );
    assert!(
        !r.starts_with("CREATE-ERROR:"),
        "Worker constructor failed on the live path: {r}"
    );
    assert!(
        r.starts_with("OK:255,0,0,255:"),
        "worker-realm OffscreenCanvas WebGL1 pipeline must create a context, clear red and \
         read back exactly (digest: {r})"
    );
    let version = r
        .strip_prefix("OK:255,0,0,255:")
        .expect("digest already prefix-asserted");
    assert!(
        !version.is_empty(),
        "getParameter(VERSION) must be a non-empty string (digest: {r})"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 C14 — Window-realm canvas WebGL1 control probe (the UNCHANGED Window
//          path through the refactored new_inherited). Isolates
//          "environment cannot do GL" from "worker wiring broken".
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:14] window-realm canvas WebGL1 control probe
/// (live, direct probe of the Window path)
#[test]
fn c14_window_realm_webgl1_control_probe_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let probe = r#"
(function () {
  try {
    if (typeof HTMLCanvasElement === 'undefined') { return 'ABSENT:HTMLCanvasElement'; }
    var c = document.createElement('canvas');
    c.width = 32; c.height = 16;
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    var version = gl.getParameter(0x1F01);
    if (typeof version !== 'string' || version.length === 0) {
      return 'BAD-VERSION:' + String(version);
    }
    gl.clearColor(0, 1, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    var px = new Uint8Array(4);
    gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
    var err = gl.getError();
    if (err !== gl.NO_ERROR) { return 'GL-ERROR:' + err; }
    return 'OK:' + px[0] + ',' + px[1] + ',' + px[2] + ',' + px[3] + ':' + version;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#;
    let raw = page
        .evaluate_js_web(probe)
        .expect("window-realm WebGL control probe evaluation must succeed");
    let r = unquote_bridge(raw.trim().to_string());
    assert!(
        r.starts_with("OK:0,255,0,255:"),
        "window-realm canvas WebGL1 must create a context, clear green and read back exactly \
         (digest: {r})"
    );
}
