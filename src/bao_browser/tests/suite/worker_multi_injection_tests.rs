// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:integration]
// Multi-Worker per-Worker stealth delivery live tests (REQ-BRW-004, user
// ruling 2026-09-09 vendor patch — e43 multi-worker gap).
//
// Defect under test: both worker-scope vendor drain queues
// (EMBEDDER_WORKER_SCOPE_CALLBACKS / EMBEDDER_WORKER_INTERFACES_READY_CALLBACKS,
// script_thread.rs) were FnOnce consume-once — the FIRST `new Worker()` of a
// page drained them and every SECOND and later Worker of the SAME page ran
// with ZERO stealth injection: no engine getters (navigator.userAgent was the
// servo native, not the profile UA) and no W1a JS hooks (audio getChannelData
// / webgl getParameter). A bare fingerprintable Worker, masked by every
// single-Worker test.
//
// Fix under test: a NON-consuming per-WebView injector tier
// (register_worker_scope_injector / register_worker_interfaces_ready_injector,
// delivered at both Dedicated-Worker drain points to EVERY Worker of the
// webview; the consume-once tier and the ServiceWorker one-shot drain stay
// untouched).
//
// Probes (fa084a64 probe form, per Worker):
//   ua=          navigator.userAgent — must EXACTLY equal the page's profile
//                UA (per-Worker engine getter, criterion #12)
//   audiohooked= AudioBuffer.prototype.getChannelData contains 'detNoise'
//                (W1a JS hook via the second drain point, criterion #15)
//   wglhooked=   WebGLRenderingContext.prototype.getParameter contains
//                'dbgRenderer' (same W1a family)
//   orignative=  __originalGetParameter__ exists and does NOT contain
//                'dbgRenderer' — the e36 gate: the saved "original" must stay
//                the servo native, proving the multi delivery never re-saved
//                the JS hook into the original slot (no double-define loop)
//   permgetter=  navigator.userAgent is a non-configurable accessor
//                (define_permanent_getter "prior install" arm kept the first
//                getter across repeated deliveries)
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb).
// Skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(worker_multi_injection)'

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
    // bounded by the process (same shape as stealth_worker_audio_tests).
    unsafe {
        std::mem::transmute::<std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'static, ()>>(
            guard,
        )
    }
}

/// URL-encode a JS worker body for a data: URL (minimal percent-encoding).
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

/// Create a live BaoRuntime + one stealth page.
fn live_page(profile: StealthProfile) -> (BaoRuntime, bao_browser::PageHandle) {
    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(profile),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    (runtime, page)
}

/// Worker body, shape A: probe the per-Worker injection state and post the
/// `|`-separated marker line (UA strings contain `:` — `|` is the separator,
/// same as the FP canonical digest form).
const WORKER_PROBE_A: &str = r#"
var __r = (function () {
  try {
    var ua = String(navigator.userAgent);
    var audioHooked = typeof AudioBuffer !== 'undefined'
      && String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1;
    var wglHooked = 'n/a';
    var origNative = 'n/a';
    if (typeof WebGLRenderingContext !== 'undefined') {
      wglHooked = String(WebGLRenderingContext.prototype.getParameter).indexOf('dbgRenderer') !== -1 ? 1 : 0;
      var orig = WebGLRenderingContext.prototype.__originalGetParameter__;
      origNative = (typeof orig === 'function' && String(orig).indexOf('dbgRenderer') === -1) ? 1 : 0;
    }
    var d = Object.getOwnPropertyDescriptor(navigator, 'userAgent');
    var permGetter = d && d.configurable === false && typeof d.get === 'function' ? 1 : 0;
    return 'OK|ua=' + ua + '|audiohooked=' + (audioHooked ? 1 : 0)
         + '|wglhooked=' + wglHooked + '|orignative=' + origNative
         + '|permgetter=' + permGetter;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

/// Worker body, shape B: a deliberately DIFFERENT script (different names,
/// different probe order, plus a hardwareConcurrency sanity read) proving
/// per-Worker delivery is not tied to script identity. Same marker format.
const WORKER_PROBE_B: &str = r#"
(function () {
  var out = null;
  try {
    var uaStr = '' + navigator.userAgent;
    var hwc = '' + navigator.hardwareConcurrency;
    var audioOk = false;
    if (typeof AudioBuffer !== 'undefined') {
      var gcdSrc = '' + AudioBuffer.prototype.getChannelData;
      audioOk = gcdSrc.indexOf('detNoise') !== -1;
    }
    var wglOk = 'n/a';
    var origOk = 'n/a';
    if (typeof WebGLRenderingContext !== 'undefined') {
      var gpSrc = '' + WebGLRenderingContext.prototype.getParameter;
      wglOk = gpSrc.indexOf('dbgRenderer') !== -1 ? '1' : '0';
      var origFn = WebGLRenderingContext.prototype.__originalGetParameter__;
      origOk = (typeof origFn === 'function' && ('' + origFn).indexOf('dbgRenderer') === -1) ? '1' : '0';
    }
    var desc = Object.getOwnPropertyDescriptor(navigator, 'userAgent');
    var permOk = desc && desc.configurable === false && typeof desc.get === 'function' ? '1' : '0';
    out = 'OK|ua=' + uaStr + '|audiohooked=' + (audioOk ? '1' : '0')
        + '|wglhooked=' + wglOk + '|orignative=' + origOk
        + '|permgetter=' + permOk + '|hwc=' + hwc;
  } catch (e) {
    out = 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
  self.postMessage(out);
})();
"#;

/// Page-side driver: create BOTH Workers back-to-back from data: URLs
/// (concurrent — both in flight before either completes, stressing the
/// non-consuming registry from two Worker threads) and wire each to its own
/// sink (`window.__mwA` / `window.__mwB`).
fn make_two_worker_driver(encoded_a: &str, encoded_b: &str) -> String {
    format!(
        r#"
        (function () {{
            try {{
                var a = new Worker("data:text/javascript,{body_a}");
                a.onmessage = function (e) {{ window.__mwA = String(e.data); }};
                a.onerror = function (ev) {{
                    window.__mwA = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                var b = new Worker("data:text/javascript,{body_b}");
                b.onmessage = function (e) {{ window.__mwB = String(e.data); }};
                b.onerror = function (ev) {{
                    window.__mwB = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                return 'two-workers-created';
            }} catch (e) {{
                window.__mwA = 'CREATE-ERROR:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body_a = encoded_a,
        body_b = encoded_b,
    )
}

/// Poll `window.<sink>` until the Worker posts its marker (or errors).
fn wait_for_sink(page: &bao_browser::PageHandle, sink: &str, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    let expr = format!("window.{sink}");
    loop {
        if let Ok(s) = page.evaluate_js_web(&expr) {
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

/// Extract a `|`-separated marker field from an `OK|...` worker result.
fn marker_field<'a>(r: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    r.split('|').find_map(|f| f.strip_prefix(&prefix))
}

/// Assert ONE Worker's result carries the FULL per-Worker injection state.
fn assert_full_injection(tag: &str, r: &str, main_ua: &str) {
    assert!(
        r.starts_with("OK|"),
        "{tag}: worker probe must complete, got: {r}"
    );
    let ua = marker_field(r, "ua")
        .unwrap_or_else(|| panic!("{tag}: result must carry ua=, got: {r}"));
    assert_eq!(
        ua, main_ua,
        "{tag}: per-Worker engine getter must serve the page profile UA \
         (consume-once defect: bare Worker served the servo native UA)"
    );
    assert!(
        r.contains("|audiohooked=1|") || r.ends_with("|audiohooked=1"),
        "{tag}: audio getChannelData JS hook must be installed (W1a second \
         drain, fa084a64 probe form), got: {r}"
    );
    assert!(
        r.contains("|wglhooked=1|") || r.ends_with("|wglhooked=1"),
        "{tag}: webgl getParameter JS hook must be installed (same W1a \
         family), got: {r}"
    );
    assert!(
        r.contains("|orignative=1|") || r.ends_with("|orignative=1"),
        "{tag}: e36 gate — __originalGetParameter__ must stay the servo \
         native across the multi delivery (no double-define / re-save loop), \
         got: {r}"
    );
    assert!(
        r.contains("|permgetter=1|") || r.ends_with("|permgetter=1"),
        "{tag}: navigator.userAgent must stay a non-configurable accessor \
         getter (define_permanent_getter prior-install arm held), got: {r}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §1 Completion ① — same page, TWO Workers (different scripts), EACH fully
//    injected; completion ③ — no double-define across the multi delivery
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12..17] per-Worker stealth delivery
///
/// Live, stealth page, one page, TWO Workers created back-to-back from
/// DIFFERENT scripts (probe shapes A and B). BEFORE the per-Worker injector
/// tier, Worker B received zero injection (the consume-once queue was
/// emptied by Worker A's scope drain). Asserts BOTH workers independently:
/// per-Worker engine getter (UA === page profile UA), audio + webgl W1a JS
/// hooks, and the double-define guards (e36 `__originalGetParameter__`
/// stays native; userAgent stays a PERMANENT accessor).
#[test]
fn worker_multi_injection_two_workers_each_fully_injected() {
    if should_skip() {
        return;
    }
    let _ser = lock_serializer();
    let (_runtime, page) = live_page(StealthProfile::firefox_default());

    // The page's OWN engine-getter UA — every Worker must match it exactly.
    let main_ua = unquote_bridge(
        page.evaluate_js_web("navigator.userAgent")
            .expect("main-thread navigator.userAgent read must succeed")
            .trim()
            .to_string(),
    );
    assert!(
        main_ua.contains("Firefox"),
        "page must run the Firefox stealth profile, got UA: {main_ua}"
    );

    // Reset the sinks, then create BOTH workers back-to-back.
    let _ = page.evaluate_js_web("window.__mwA = null; window.__mwB = null;");
    let driver = make_two_worker_driver(
        &encode_worker_body(WORKER_PROBE_A),
        &encode_worker_body(WORKER_PROBE_B),
    );
    let created = page.evaluate_js_web(&driver);
    match created {
        Ok(s) => assert!(
            s.contains("two-workers-created"),
            "both Worker creations must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("two-worker dispatch failed: {e}"),
    }

    let a = wait_for_sink(&page, "__mwA", Duration::from_secs(45))
        .unwrap_or_else(|| panic!("worker A result did not arrive within timeout"));
    let b = wait_for_sink(&page, "__mwB", Duration::from_secs(45))
        .unwrap_or_else(|| panic!("worker B result did not arrive within timeout"));
    eprintln!("[multi-injection] worker-A(1st): {a}");
    eprintln!("[multi-injection] worker-B(2nd): {b}");

    assert_full_injection("worker-A(1st)", &a, &main_ua);
    // THE gap: the 2nd Worker of the same page must be as injected as the
    // 1st — not a bare fingerprintable Worker.
    assert_full_injection("worker-B(2nd)", &b, &main_ua);
}

// ═══════════════════════════════════════════════════════════════════════
// §2 Completion ① third Worker (repeat delivery) — the non-consuming tier
//    keeps delivering; still no double-define
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12..17] per-Worker stealth delivery
///
/// Creates THREE sequential Workers on one page (alternating script shapes
/// A/B/A) and asserts EVERY one — in particular the 3rd, two past the
/// consume-once horizon — receives the full injection, with the
/// double-define guards holding across the repeated deliveries.
#[test]
fn worker_multi_injection_third_worker_still_fully_injected() {
    if should_skip() {
        return;
    }
    let _ser = lock_serializer();
    let (_runtime, page) = live_page(StealthProfile::firefox_default());

    let main_ua = unquote_bridge(
        page.evaluate_js_web("navigator.userAgent")
            .expect("main-thread navigator.userAgent read must succeed")
            .trim()
            .to_string(),
    );

    // Three sequential single-worker drivers (each its own script instance),
    // each posting to its own sink.
    for (i, sink) in ["__mwX1", "__mwX2", "__mwX3"].iter().enumerate() {
        let _ = page.evaluate_js_web(&format!("window.{sink} = null;"));
        let body = if i % 2 == 0 { WORKER_PROBE_A } else { WORKER_PROBE_B };
        let driver = format!(
            r#"
            (function () {{
                try {{
                    var w = new Worker("data:text/javascript,{body}");
                    w.onmessage = function (e) {{ window.{sink} = String(e.data); }};
                    w.onerror = function (ev) {{
                        window.{sink} = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                        return true;
                    }};
                    return 'worker-created';
                }} catch (e) {{
                    window.{sink} = 'CREATE-ERROR:' + String(e);
                    return 'worker-create-failed';
                }}
            }})();
            "#,
            body = encode_worker_body(body),
            sink = sink,
        );
        let created = page.evaluate_js_web(&driver);
        match created {
            Ok(s) => assert!(
                s.contains("worker-created"),
                "worker #{} creation must succeed, got: {s}",
                i + 1
            ),
            Err(e) => panic!("worker #{} dispatch failed: {e}", i + 1),
        }
        let r = wait_for_sink(&page, sink, Duration::from_secs(45))
            .unwrap_or_else(|| panic!("worker #{} result did not arrive within timeout", i + 1));
        assert_full_injection(&format!("worker-#{}", i + 1), &r, &main_ua);
    }
}
