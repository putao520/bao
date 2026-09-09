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
//         W3b (§5): WebGL2 同形态通道 + getShaderPrecisionFormat 解
//         as_window 锚 + dom_webgl2_enabled embedder flip
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
/// Note: the basic getParameter probe uses 0x1F01 (UNMASKED_RENDERER_WEBGL).
/// CORRECTED (e36): an earlier comment claimed `gl.getParameter(0x1F02)`
/// returned `undefined` "from servo's native GetParameter in BOTH realms — a
/// pre-existing upstream quirk". That was a misdiagnosis: the worker realm
/// (zero Bao wrappers) always returned the correct "WebGL 1.0" string, and
/// the `undefined` came ONLY from the Bao-side double stealth injection
/// dead-looping un-intercepted params in the Window realm (see §6 and
/// .claude/prompts/brw004-getparameter-evidence.md). servo upstream is
/// innocent — do NOT file an upstream issue for this.
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

// ═══════════════════════════════════════════════════════════════════════
// §5 C14 (W3b) — WebGL2 OffscreenCanvas INSIDE a Worker realm + worker
//                 getShaderPrecisionFormat (Bao vendor patch:
//                 WebGL2RenderingContext::new_inherited decoupled from
//                 &Window + new_in_worker + offscreencanvas WebGL2 dispatch
//                 + GetShaderPrecisionFormat reflect on the owning global +
//                 dom_webgl2_enabled embedder flip)
// ═══════════════════════════════════════════════════════════════════════

/// Worker script body: run the OffscreenCanvas WebGL2 pipeline INSIDE the
/// Worker thread and post a compact digest back. The clear color is BLUE
/// (distinct from the WebGL1 §3 RED) so a digest mix-up between the two
/// channel dispatch arms cannot pass silently. The pixel readback of a
/// `clearColor(0,0,1,1)` clear is asserted EXACT (fixed-point 1.0 ⇒ 255).
const WORKER_WEBGL2_BODY: &str = r#"
var __r = (function () {
  try {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    var c = new OffscreenCanvas(64, 48);
    var gl = c.getContext('webgl2');
    if (!gl) { return 'NULL-WEBGL2-CTX'; }
    if (gl.drawingBufferWidth !== 64 || gl.drawingBufferHeight !== 48) {
      return 'BAD-SIZE:' + gl.drawingBufferWidth + 'x' + gl.drawingBufferHeight;
    }
    gl.clearColor(0, 0, 1, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    var px = new Uint8Array(4);
    gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
    var err = gl.getError();
    if (err !== gl.NO_ERROR) { return 'GL-ERROR:' + err; }
    return 'OK:' + px[0] + ',' + px[1] + ',' + px[2] + ',' + px[3];
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// @trace REQ-BRW-004 [criterion:14] worker-realm OffscreenCanvas WebGL2
/// pipeline (live, servo-native Worker thread, stealth page)
///
/// The whole pipeline runs inside the Worker: construction,
/// getContext('webgl2') non-null (before the W3b vendor patch the WebGL2 arm
/// of `get_or_init_webgl2_context` Window-downcast the global and returned
/// null in workers), drawingBuffer geometry, clear + readPixels round-trip,
/// getError()==NO_ERROR.
#[test]
fn c14_worker_offscreencanvas_webgl2_pipeline_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let _ = page.evaluate_js_web("window.__ocResult = null;");
    let body = encode_worker_body(WORKER_WEBGL2_BODY);
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
            "worker WebGL2 digest did not arrive within timeout — worker postMessage → onmessage \
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
    assert_eq!(
        r, "OK:0,0,255,255",
        "worker-realm OffscreenCanvas WebGL2 pipeline must create a context, clear blue and read \
         back exactly"
    );
}

/// Worker script body: probe `getShaderPrecisionFormat` in the worker on BOTH
/// context types (webgl2 FRAGMENT_SHADER/HIGH_FLOAT and webgl
/// VERTEX_SHADER/HIGH_INT — the implementation is the shared base method
/// whose reflection anchor W3b moved from `as_window()` to the owning
/// global). Every field must be an integer ≥ 0 (legal GL precision values,
/// not throw / not undefined / not null).
const WORKER_SPF_BODY: &str = r#"
var __r = (function () {
  try {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    function check(sp, tag) {
      if (!sp || typeof sp !== 'object') { return 'BAD-' + tag + ':' + String(sp); }
      if (!Number.isInteger(sp.rangeMin) || !Number.isInteger(sp.rangeMax) ||
          !Number.isInteger(sp.precision)) {
        return 'BAD-' + tag + '-FIELDS:' + sp.rangeMin + ',' + sp.rangeMax + ',' + sp.precision;
      }
      if (sp.rangeMin < 0 || sp.rangeMax < 0 || sp.precision < 0) {
        return 'NEG-' + tag + ':' + sp.rangeMin + ',' + sp.rangeMax + ',' + sp.precision;
      }
      return null;
    }
    var c2 = new OffscreenCanvas(32, 24);
    var gl2 = c2.getContext('webgl2');
    if (!gl2) { return 'NULL-WEBGL2-CTX'; }
    var sp2 = gl2.getShaderPrecisionFormat(gl2.FRAGMENT_SHADER, gl2.HIGH_FLOAT);
    var bad = check(sp2, 'SP2');
    if (bad) { return bad; }
    var c1 = new OffscreenCanvas(32, 24);
    var gl1 = c1.getContext('webgl');
    if (!gl1) { return 'NULL-WEBGL-CTX'; }
    var sp1 = gl1.getShaderPrecisionFormat(gl1.VERTEX_SHADER, gl1.HIGH_INT);
    bad = check(sp1, 'SP1');
    if (bad) { return bad; }
    return 'OK:' + sp2.rangeMin + ',' + sp2.rangeMax + ',' + sp2.precision +
           ':' + sp1.rangeMin + ',' + sp1.rangeMax + ',' + sp1.precision;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// @trace REQ-BRW-004 [criterion:14] worker-realm getShaderPrecisionFormat
/// (live, servo-native Worker thread, both WebGL1 and WebGL2 contexts)
///
/// Before the W3b vendor patch the base `GetShaderPrecisionFormat` reflected
/// the result via `self.global().as_window()`, which panics ("expected a
/// Window scope") in a worker. This probe must return a legal
/// WebGLShaderPrecisionFormat (three integers ≥ 0) on BOTH context types.
#[test]
fn c14_worker_webgl_getshaderprecisionformat_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let _ = page.evaluate_js_web("window.__ocResult = null;");
    let body = encode_worker_body(WORKER_SPF_BODY);
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
            "worker shader-precision digest did not arrive within timeout — worker postMessage → \
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
    let Some(fields) = r.strip_prefix("OK:") else {
        panic!("worker getShaderPrecisionFormat must return legal values, digest: {r}")
    };
    let values: Vec<&str> = fields.split(':').collect();
    assert_eq!(
        values.len(),
        2,
        "digest must carry webgl2 and webgl precision triples: {r}"
    );
    for triple in values {
        let parts: Vec<&str> = triple.split(',').collect();
        assert_eq!(parts.len(), 3, "precision triple shape: {r}");
        for p in parts {
            let v: i64 = p
                .parse()
                .unwrap_or_else(|_| panic!("precision field must be an integer, digest: {r}"));
            assert!(v >= 0, "precision field must be ≥ 0, digest: {r}");
        }
    }
}

/// @trace REQ-BRW-004 [criterion:14] window-realm canvas WebGL2 control probe
/// (live, direct probe of the UNCHANGED Window path through the refactored
/// WebGL2 new/new_inherited + the re-anchored GetShaderPrecisionFormat)
#[test]
fn c14_window_realm_webgl2_control_probe_works() {
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
    var gl = c.getContext('webgl2');
    if (!gl) { return 'NULL-WEBGL2-CTX'; }
    gl.clearColor(1, 0, 1, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    var px = new Uint8Array(4);
    gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
    var err = gl.getError();
    if (err !== gl.NO_ERROR) { return 'GL-ERROR:' + err; }
    var sp = gl.getShaderPrecisionFormat(gl.FRAGMENT_SHADER, gl.HIGH_FLOAT);
    if (!sp || typeof sp !== 'object' || !Number.isInteger(sp.precision) || sp.precision < 0) {
      return 'BAD-SP:' + String(sp && sp.precision);
    }
    return 'OK:' + px[0] + ',' + px[1] + ',' + px[2] + ',' + px[3] +
           ':SP' + sp.rangeMin + ',' + sp.rangeMax + ',' + sp.precision;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#;
    let raw = page
        .evaluate_js_web(probe)
        .expect("window-realm WebGL2 control probe evaluation must succeed");
    let r = unquote_bridge(raw.trim().to_string());
    assert!(
        r.starts_with("OK:255,0,255,255:SP"),
        "window-realm canvas WebGL2 must create a context, clear magenta, read back exactly and \
         return legal shader precision (digest: {r})"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §6 e36 BCE regression — double stealth injection dead-loop. Root cause:
// PagePool::create_page AND BaoRuntime::create_page each injected, so the
// second install_webgl_override stored the FIRST pass's JS hook into
// __originalGetParameter__; every un-intercepted getParameter (0x1F02
// VERSION, 0x8B8C GLSL_ES_VERSION, and ALL other non-intercepted enums)
// dead-looped JS hook ↔ native override and surfaced as literal `undefined`
// with getError()==NO_ERROR. The fix: single injection in
// PagePool::create_page + install idempotency guard + profile-gated install
// (stealth_profile: None ⇒ NO stealth chain at all).
// Evidence: .claude/prompts/brw004-getparameter-evidence.md
// ═══════════════════════════════════════════════════════════════════════

/// Window-realm probe digest for the un-intercepted getParameter path on a
/// canvas WebGL1 context. `ORIG=1` ⇔ __originalGetParameter__ stringifies as
/// `[native code]` (the TRUE servo native — under double injection it was the
/// JS hook source instead); `HOOK=1` ⇔ proto.getParameter is the stealth JS
/// hook (outer layer present); V1F02/V8B8C are the raw pass-through values.
fn e36_window_probe_js() -> &'static str {
    r#"
(function () {
  try {
    var c = document.createElement('canvas');
    c.width = 32; c.height = 16;
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    var proto = Object.getPrototypeOf(gl);
    var orig = proto.__originalGetParameter__;
    var v1 = gl.getParameter(0x1F02);
    var v2 = gl.getParameter(0x8B8C);
    var err = gl.getError();
    return 'OK:'
      + (typeof v1 === 'undefined' ? 'UNDEF' : String(v1)) + '|'
      + (typeof v2 === 'undefined' ? 'UNDEF' : String(v2)) + '|'
      + 'ORIG=' + (orig ? (String(orig).indexOf('[native code]') !== -1 ? 1 : 0) : -1) + '|'
      + 'HOOK=' + (String(proto.getParameter).indexOf('dbgRenderer') !== -1 ? 1 : 0) + '|'
      + 'ERR=' + err;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#
}

/// Worker-realm probe: same two un-intercepted enums through the zero-Bao-
/// wrapper servo native path (worker scopes never receive the Window
/// injection chain). The digest shape matches the window probe's OK arm.
const E36_WORKER_GETPARAM_BODY: &str = r#"
var __r = (function () {
  try {
    var c = new OffscreenCanvas(64, 48);
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    var v1 = gl.getParameter(0x1F02);
    var v2 = gl.getParameter(0x8B8C);
    return 'OK:'
      + (typeof v1 === 'undefined' ? 'UNDEF' : String(v1)) + '|'
      + (typeof v2 === 'undefined' ? 'UNDEF' : String(v2));
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// e36 completion ①(live): Window realm un-intercepted getParameter returns
/// the TRUE servo values — identical to the worker realm's zero-wrapper
/// results — and `__originalGetParameter__` holds the servo native, not a
/// stealth JS hook.
#[test]
fn e36_window_getparameter_unintercepted_matches_worker_native() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    // Window side.
    let raw = page
        .evaluate_js_web(e36_window_probe_js())
        .expect("e36 window getParameter probe must evaluate");
    let win = unquote_bridge(raw.trim().to_string());
    assert!(
        win.starts_with("OK:"),
        "window probe must succeed (double-injection regression?), digest: {win}"
    );
    assert!(
        win.contains("OK:WebGL 1.0|WebGL GLSL ES 1.0|"),
        "Window realm un-intercepted getParameter(0x1F02/0x8B8C) must return the servo native \
         strings, not undefined (digest: {win})"
    );
    assert!(
        win.contains("|ORIG=1|"),
        "__originalGetParameter__ must be the servo native ([native code]), not a stealth JS \
         hook — double-injection signature (digest: {win})"
    );
    assert!(
        win.contains("|HOOK=1|"),
        "proto.getParameter must still be the stealth JS hook (interception capability intact, \
         digest: {win})"
    );
    assert!(
        win.ends_with("|ERR=0"),
        "probe must leave getError()==NO_ERROR (digest: {win})"
    );

    // Worker side (same page, zero-wrapper servo native).
    let _ = page.evaluate_js_web("window.__ocResult = null;");
    let body = encode_worker_body(E36_WORKER_GETPARAM_BODY);
    let created = page.evaluate_js_web(&make_worker_driver(&body));
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "Worker creation must succeed, got: {s}"
        ),
        Err(e) => panic!("Worker creation dispatch failed: {e}"),
    }
    let wr = wait_for_worker_result(&page, Duration::from_secs(30))
        .unwrap_or_else(|| panic!("worker getParameter digest did not arrive"));
    assert!(
        !wr.starts_with("WORKER-ERROR:") && !wr.starts_with("CREATE-ERROR:"),
        "worker probe must not error: {wr}"
    );
    assert_eq!(
        wr, "OK:WebGL 1.0|WebGL GLSL ES 1.0",
        "worker realm (zero Bao wrappers) un-intercepted getParameter must keep returning the \
         servo native strings"
    );
    let win_values = &win["OK:".len()..];
    let wr_values = &wr["OK:".len()..];
    assert_eq!(
        win_values.split("|ORIG").next().unwrap_or(""),
        wr_values,
        "Window realm un-intercepted values must EQUAL the worker realm's native values \
         (e36 completion ①: 与 worker 零包装一致)"
    );
}

/// e36 completion ②(live): the interception capability is fully intact —
/// JS-hook-layer enums (0x9246/0x9245/0x0D33) and native-override-layer enums
/// (0x1F00/0x1F01) still return the stealth profile's configured values.
#[test]
fn e36_window_getparameter_intercepted_enums_still_profile_driven() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile.clone()));

    let probe = r#"
(function () {
  try {
    var c = document.createElement('canvas');
    c.width = 32; c.height = 16;
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    return 'OK:' + String(gl.getParameter(0x9246)) + '|'
                 + String(gl.getParameter(0x9245)) + '|'
                 + String(gl.getParameter(0x0D33)) + '|'
                 + String(gl.getParameter(0x1F00)) + '|'
                 + String(gl.getParameter(0x1F01)) + '|'
                 + 'ERR=' + gl.getError();
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#;
    let raw = page
        .evaluate_js_web(probe)
        .expect("intercepted-enum probe must evaluate");
    let r = unquote_bridge(raw.trim().to_string());
    let expected = format!(
        "OK:{}|{}|{}|{}|{}|ERR=0",
        profile.webgl.renderer,
        profile.webgl.vendor,
        profile.webgl.max_texture_size,
        profile.webgl.vendor,     // 0x1F00 UNMASKED_VENDOR (native override layer)
        profile.webgl.renderer,   // 0x1F01 UNMASKED_RENDERER (native override layer)
    );
    assert_eq!(
        r, expected,
        "intercepted enums must return the profile values on both stealth layers (JS hook + \
         native override)"
    );
}

/// e36 completion ③(live): a stealth-free page (`stealth_profile: None`)
/// carries NO stealth chain at all — no __originalGetParameter__ slot, no JS
/// hook on getParameter, pure servo-native surfaces returning true values.
#[test]
fn e36_stealth_free_page_has_no_stealth_chain() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let (_runtime, page) = live_page(None);

    let probe = r#"
(function () {
  try {
    if (typeof WebGLRenderingContext === 'undefined') { return 'ABSENT:WebGLRenderingContext'; }
    var proto = WebGLRenderingContext.prototype;
    var hasOrig = typeof proto.__originalGetParameter__ !== 'undefined';
    var gpSrc = String(proto.getParameter);
    var hooked = gpSrc.indexOf('dbgRenderer') !== -1;
    var native = gpSrc.indexOf('[native code]') !== -1;
    var c = document.createElement('canvas');
    c.width = 32; c.height = 16;
    var gl = c.getContext('webgl');
    if (!gl) { return 'NULL-WEBGL-CTX'; }
    var v1 = gl.getParameter(0x1F02);
    var v2 = gl.getParameter(0x8B8C);
    return 'OK:HASORIG=' + (hasOrig ? 1 : 0) + '|HOOKED=' + (hooked ? 1 : 0)
      + '|NATIVE=' + (native ? 1 : 0)
      + '|V1F02=' + (typeof v1 === 'undefined' ? 'UNDEF' : String(v1))
      + '|V8B8C=' + (typeof v2 === 'undefined' ? 'UNDEF' : String(v2))
      + '|ERR=' + gl.getError();
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()"#;
    let raw = page
        .evaluate_js_web(probe)
        .expect("stealth-free probe must evaluate");
    let r = unquote_bridge(raw.trim().to_string());
    assert!(
        r.starts_with("OK:"),
        "stealth-free probe must succeed (digest: {r})"
    );
    assert!(
        r.contains("HASORIG=0|"),
        "stealth_profile: None must NOT install __originalGetParameter__ (digest: {r})"
    );
    assert!(
        r.contains("|HOOKED=0|"),
        "stealth_profile: None must NOT install the JS getParameter hook (digest: {r})"
    );
    assert!(
        r.contains("|NATIVE=1|"),
        "stealth_profile: None getParameter must be the servo native function (digest: {r})"
    );
    assert!(
        r.contains("|V1F02=WebGL 1.0|"),
        "stealth-free Window realm must return the true servo VERSION string (digest: {r})"
    );
    assert!(
        r.contains("|V8B8C=WebGL GLSL ES 1.0|"),
        "stealth-free Window realm must return the true servo GLSL ES string (digest: {r})"
    );
    assert!(
        r.ends_with("|ERR=0"),
        "stealth-free probe must leave getError()==NO_ERROR (digest: {r})"
    );
}
