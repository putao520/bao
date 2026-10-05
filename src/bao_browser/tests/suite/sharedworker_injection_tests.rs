// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:integration]
// SharedWorker enablement + per-Worker stealth delivery live tests (REQ-BRW-004
// third worker scope, user ruling "we are already independent" — e82).
//
// Defect under test (e81 profile, /tmp/e81-sharedworker-probe.md):
//   1. The bao vendor snapshot ships the complete upstream SharedWorker
//      implementation (9fc8f7389) but gates it behind
//      `dom_sharedworker_enabled: false` (config/prefs.rs — the ONLY bao-local
//      divergence), so `SharedWorkerBinding::ConstructorEnabled` keeps the
//      interface object off the Window global: `new SharedWorker()` threw
//      `ReferenceError: SharedWorker is not defined` (the L2 127-cell
//      "SharedWorker is not defined" variant tax).
//   2. `SharedWorkerGlobalScope::run_shared_worker_scope` never drained the
//      embedder worker-scope tiers, so even enabled the shared realm ran with
//      ZERO stealth injection — a bare, fingerprintable realm (constitution A:
//      the enablement and the drain ship together, or the fix is itself a
//      detection vector).
//
// Fix under test:
//   - bao embedder flip: `preferences.dom_sharedworker_enabled = true`
//     (src/bao_browser/src/lib.rs, fifth of the IDB/OffscreenCanvas/SW/WebGL2
//     pref series).
//   - vendor drain mirror: the one-shot scope tier + the non-consuming
//     per-Worker injector tier delivered in `run_shared_worker_scope` after
//     the constructor handshake, before the script fetch (mirror of the
//     Dedicated path in dedicatedworkerglobalscope.rs). The interfaces-ready
//     phase needs no new site: the SharedWorker script load completes through
//     `WorkerGlobalScope::on_complete`, which already drains those tiers
//     keyed by `GlobalScope::webview_id()` — that accessor already resolves
//     this scope.
//
// Probes (worker_multi_injection_tests form, per SharedWorker):
//   ctor=        typeof SharedWorker — must be "function" (the L2 literal)
//   ua=          navigator.userAgent inside the shared realm — must EXACTLY
//                equal the page's profile UA (engine getter, criterion #12)
//   audiohooked= AudioBuffer.prototype.getChannelData contains 'detNoise'
//                (W1a JS hook — satisfiable only by the POST-interfaces
//                delivery through on_complete, criterion #15)
//   port=        bidirectional MessagePort roundtrip page→worker→page
//                (the S-family connect surface works end to end)
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb).
// Skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(sharedworker)'

#![allow(dead_code)]

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig};
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
    // bounded by the process (same shape as worker_multi_injection_tests).
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

/// Create a live BrowserRuntime + one stealth page.
fn live_page(profile: StealthProfile) -> (BrowserRuntime, bao_browser::PageHandle) {
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(profile),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    (runtime, page)
}

/// SharedWorker body: on the first connect it posts the injection probe
/// (`OK|...`), then echoes every further message back as `ECHO:<data>`.
const SHARED_WORKER_PROBE: &str = r#"
var __connected = false;
onconnect = function (e) {
  var p = e.ports[0];
  p.onmessage = function (ev) { p.postMessage('ECHO:' + String(ev.data)); };
  if (__connected) { return; }
  __connected = true;
  var __r = (function () {
    try {
      var ua = String(navigator.userAgent);
      var nm = String(self.name);
      var audioHooked = typeof AudioBuffer !== 'undefined'
        && String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1;
      return 'OK|ua=' + ua + '|audiohooked=' + (audioHooked ? 1 : 0) + '|wname=' + nm;
    } catch (err) {
      return 'THROW:' + ((err && err.message) ? err.message : String(err));
    }
  })();
  p.postMessage(__r);
};
"#;

/// Page-side driver: construct ONE SharedWorker from a data: URL, keep the
/// handle on `window.__swHandle`, wire its port to the sinks (`OK|` probe →
/// `window.<probe_sink>`, `ECHO:` → `window.__swEcho`), and report what the
/// constructor itself observed. The page→worker message is sent by a FOLLOW-UP
/// evaluate (after the probe landed and the worker's port handler is
/// registered), not inside this driver — a pre-connect send relies on the
/// MessagePort queue draining order instead.
fn make_sharedworker_driver(encoded: &str, name: &str, probe_sink: &str) -> String {
    format!(
        r#"
        (function () {{
            window.__swCtor = typeof SharedWorker;
            if (typeof SharedWorker === 'undefined') {{
                return 'sharedworker-undefined';
            }}
            try {{
                var w = new SharedWorker("data:text/javascript,{body}", {{ name: "{wname}" }});
                window.__swHandle = w;
                window.{sink} = null;
                window.__swEcho = null;
                w.onerror = function (ev) {{
                    window.{sink} = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                w.port.onmessage = function (e) {{
                    var d = String(e.data);
                    if (d.indexOf('ECHO:') === 0) {{
                        window.__swEcho = d;
                    }} else {{
                        window.{sink} = d;
                    }}
                }};
                return 'sharedworker-created';
            }} catch (e) {{
                window.{sink} = 'CREATE-ERROR:' + String(e);
                return 'sharedworker-create-failed';
            }}
        }})();
        "#,
        body = encoded,
        wname = name,
        sink = probe_sink,
    )
}

/// Poll `window.<sink>` until it is set (or the timeout elapses).
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

/// Assert ONE SharedWorker's probe carries the full injection state.
fn assert_sharedworker_injection(tag: &str, r: &str, main_ua: &str, expected_name: &str) {
    assert!(
        r.starts_with("OK|"),
        "{tag}: probe must complete, got: {r}"
    );
    let ua = marker_field(r, "ua")
        .unwrap_or_else(|| panic!("{tag}: probe must carry ua=, got: {r}"));
    assert_eq!(
        ua, main_ua,
        "{tag}: SharedWorker realm engine getter must serve the page profile UA \
         (bare-realm defect: undrained scope served the servo native UA)"
    );
    assert!(
        r.contains("|audiohooked=1|") || r.ends_with("|audiohooked=1"),
        "{tag}: SharedWorker realm must carry the W1a audio hook — satisfiable \
         only by the post-interfaces delivery through `WorkerGlobalScope::on_complete` \
         (drain-missing defect: bare, fingerprintable shared realm), got: {r}"
    );
    assert_eq!(
        marker_field(r, "wname"),
        Some(expected_name),
        "{tag}: constructor options.name must reach the shared realm (self.name)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §1 — Constructor enabled (L2 literal) + port roundtrip + full injection
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12..17] SharedWorker stealth inheritance
///
/// Live, stealth page. BEFORE the fix: `typeof SharedWorker` was "undefined"
/// (the L2 `SharedWorker is not defined` variant tax) and the shared realm had
/// no embedder drain. Asserts: (1) the constructor exists on the Window
/// global, (2) the connect/port surface round-trips page→worker→page, (3) the
/// shared realm's engine getter serves the page profile UA (first drain point)
/// and the W1a audio hook is installed (post-interfaces delivery through
/// `on_complete`).
#[test]
fn sharedworker_ctor_port_roundtrip_and_stealth_injection() {
    if should_skip() {
        return;
    }
    let _ser = lock_serializer();
    let (_runtime, page) = live_page(StealthProfile::firefox_default());

    // The page's OWN engine-getter UA — the shared realm must match exactly.
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

    let driver = make_sharedworker_driver(&encode_worker_body(SHARED_WORKER_PROBE), "e82-probe", "__swProbe");
    let created = page.evaluate_js_web(&driver);
    match created {
        Ok(s) => assert!(
            s.contains("sharedworker-created"),
            "SharedWorker creation must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("sharedworker dispatch failed: {e}"),
    }

    // (1) The pref gate is the ONLY gate — the interface object is defined.
    let ctor = unquote_bridge(
        page.evaluate_js_web("typeof SharedWorker")
            .expect("typeof SharedWorker read must succeed")
            .trim()
            .to_string(),
    );
    assert_eq!(
        ctor, "function",
        "`typeof SharedWorker` must be \"function\" on the Window global — \
         \"undefined\" is the L2 pref-off / interface-absent literal"
    );

    // (2) + (3) The worker's probe message proves the port surface AND the
    // realm injection state in one delivery.
    let probe = wait_for_sink(&page, "__swProbe", Duration::from_secs(45))
        .unwrap_or_else(|| panic!("shared worker probe did not arrive within timeout"));
    eprintln!("[sharedworker-injection] probe: {probe}");
    assert_sharedworker_injection("sharedworker#1", &probe, &main_ua, "e82-probe");

    // (2) Page→worker→page echo: the port roundtrip in the other direction,
    // sent only after the worker's port handler is provably registered.
    let _ = page.evaluate_js_web("window.__swEcho = null; window.__swHandle.port.postMessage('ping');");
    let echo = wait_for_sink(&page, "__swEcho", Duration::from_secs(45))
        .unwrap_or_else(|| panic!("port echo did not arrive within timeout"));
    assert_eq!(
        echo, "ECHO:ping",
        "MessagePort roundtrip page→worker→page must return the echoed payload"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §2 — Second SharedWorker of the same page is injected too (the
//      non-consuming injector tier must cover shared scopes beyond the first)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12..17] per-Worker injector tier on shared scopes
///
/// Creates TWO differently-named SharedWorkers on one page. The one-shot scope
/// tier can only be consumed once, so the SECOND shared worker is proof the
/// non-consuming injector tier covers SharedWorker scopes — not a bare
/// fingerprintable realm.
#[test]
fn sharedworker_second_instance_still_injected() {
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

    let body = encode_worker_body(SHARED_WORKER_PROBE);
    for i in 0..2 {
        let sink = format!("__swP{}", i + 1);
        let driver = make_sharedworker_driver(&body, &format!("e82-s{i}"), &sink);
        let created = page.evaluate_js_web(&driver);
        match created {
            Ok(s) => assert!(
                s.contains("sharedworker-created"),
                "shared worker #{} creation must succeed, got: {s}",
                i + 1
            ),
            Err(e) => panic!("shared worker #{} dispatch failed: {e}", i + 1),
        }
        let r = wait_for_sink(&page, &sink, Duration::from_secs(45))
            .unwrap_or_else(|| panic!("shared worker #{} probe did not arrive", i + 1));
        eprintln!("[sharedworker-injection] worker-#{}: {r}", i + 1);
        assert_sharedworker_injection(
            &format!("sharedworker#{}", i + 1),
            &r,
            &main_ua,
            &format!("e82-s{i}"),
        );
    }
}
