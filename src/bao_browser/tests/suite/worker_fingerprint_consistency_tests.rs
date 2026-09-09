// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:integration]
// Servo-native Worker live fingerprint-consistency tests for REQ-BRW-004
// criteria #12 (navigator 一致), #16 (无原生泄漏), #17 (跨线程对比一致).
//
// SPEC criteria under test:
//   C12: "CRIT-STL-WK navigator 一致: worker 内 navigator.userAgent/platform/
//         hardwareConcurrency/language(s) === 主线程对应值 (关联 NFR-STL-WORKER-1
//         / DEC-WK-007)"
//   C16: "CRIT-STL-WK 无原生泄漏: worker 内不存在任何未经 stealth 覆盖的 servo
//         原生指纹值 (与无 stealth 基线对比应不同)"
//   C17: "CRIT-STL-WK 跨线程对比一致: new Worker 后 worker 回传指纹摘要 ===
//         主线程指纹摘要 (CreepJS/sannysoft 式 worker-vs-main 比对通过)"
//
// Live path under test (E22 audit verdict: injection chain complete but had
// zero live consistency coverage):
//   page JS `new Worker(data:URL)` → servo native Worker thread →
//   register_worker_scope_callback_native (page-init registration,
//   inject_all_with_profile) → worker_scope_init_native installs
//   set_profile_for_global + install_stealth_props on the Worker global →
//   worker computes its fingerprint surface INSIDE the Worker thread and
//   posts the canonical digest back via self.postMessage → main thread
//   w.onmessage → window sink → test polls via evaluate_js_web.
//
// The canonical digest is computed by ONE JS snippet defined in this test
// file and evaluated verbatim on BOTH sides (worker thread + main thread) —
// same algorithm, two realms (CreepJS worker-vs-main comparison shape).
//
// Environment gating:
//   Real servo rendering requires DISPLAY (Xvfb) and network/asset I/O.
//   Skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(worker_fingerprint_consistency)'

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

// ═══════════════════════════════════════════════════════════════════════
// Shared fingerprint surface (the digest algorithm — test-defined, single
// source, evaluated verbatim on both the Worker thread and the main thread)
// ═══════════════════════════════════════════════════════════════════════

/// Canonical fingerprint digest: sorted `key=value` pairs joined by `|`,
/// covering the navigator surface + screen surface readable on both realms.
///
/// Read guards: a surface that is absent on one realm canonicalizes to
/// `undefined` / `absent` rather than throwing, so a worker-vs-main asymmetry
/// shows up as a VALUE difference (which is exactly what these tests assert
/// on) instead of a JS exception.
const FP_CANONICAL_JS: &str = r#"(function () {
  var g = (typeof self !== 'undefined') ? self : window;
  var n = g.navigator || {};
  var s = (typeof g.screen !== 'undefined' && g.screen !== null) ? g.screen : null;
  function f(x) { return (typeof x === 'undefined') ? 'undefined' : ((x === null) ? 'null' : String(x)); }
  var d = {
    availW: s ? f(s.availWidth) : 'absent',
    deviceMemory: f(n.deviceMemory),
    hwc: f(n.hardwareConcurrency),
    language: f(n.language),
    languages: f(n.languages),
    maxTouchPoints: f(n.maxTouchPoints),
    platform: f(n.platform),
    screenH: s ? f(s.height) : 'absent',
    screenW: s ? f(s.width) : 'absent',
    ua: f(n.userAgent),
    vendor: f(n.vendor),
    webdriver: f(n.webdriver)
  };
  var ks = Object.keys(d).sort();
  var parts = [];
  for (var i = 0; i < ks.length; i++) { parts.push(ks[i] + '=' + d[ks[i]]); }
  return parts.join('|');
})()"#;

/// Worker script body: compute the canonical digest inside the Worker thread
/// and post it back to the main thread (C3 path: self.postMessage).
fn make_worker_body(canonical_expr: &str) -> String {
    format!("var __fp = {expr}; self.postMessage(__fp);", expr = canonical_expr)
}

/// URL-encode a JS worker body for data: URL (minimal percent-encoding).
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

/// Page-side driver: create the Worker from a data: URL and wire
/// `w.onmessage` → `window.__workerFP` (worker→main digest sink) and
/// `w.onerror` → an explicit `WORKER-ERROR:` marker (never silent).
fn make_worker_driver(encoded_body: &str) -> String {
    format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onmessage = function (e) {{ window.__workerFP = String(e.data); }};
                w.onerror = function (ev) {{
                    window.__workerFP = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                return 'worker-created';
            }} catch (e) {{
                window.__workerFP = 'CREATE-ERROR:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = encoded_body
    )
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

/// Poll `window.__workerFP` until the Worker posts its digest (or errors).
/// Returns the unquoted worker-side digest string, or None on timeout.
fn wait_for_worker_fp(page: &bao_browser::PageHandle, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__workerFP") {
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

/// Evaluate the canonical digest on the MAIN thread (same algorithm).
fn read_main_fp(page: &bao_browser::PageHandle) -> String {
    let raw = page
        .evaluate_js_web(FP_CANONICAL_JS)
        .expect("main-thread canonical digest evaluation must succeed");
    unquote_bridge(raw.trim().to_string())
}

/// Parse a canonical digest into its sorted (key, value) pairs.
fn parse_fields(canonical: &str) -> Vec<(String, String)> {
    canonical
        .split('|')
        .filter_map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Per-field diff report between two canonical digests (for diagnostics).
fn field_diffs(a: &str, b: &str) -> Vec<String> {
    let fa = parse_fields(a);
    let fb = parse_fields(b);
    let mut diffs = Vec::new();
    for (ka, va) in &fa {
        match fb.iter().find(|(kb, _)| kb == ka) {
            Some((_, vb)) if va != vb => diffs.push(format!("{ka}: worker/main={va:?} vs other={vb:?}")),
            None => diffs.push(format!("{ka}: missing on other side")),
            _ => {}
        }
    }
    diffs
}

/// Create a live BaoRuntime + one page with the given stealth profile.
/// Fails the test (not skip) once the gated environment has been established.
fn live_page(
    profile: Option<StealthProfile>,
) -> (BaoRuntime, bao_browser::PageHandle) {
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

/// Drive one Worker on `page` and return its worker-thread digest.
/// Asserts the worker executed, posted back via self.postMessage → onmessage
/// (C3 path), and produced a real canonical digest (not an error marker).
fn worker_digest(page: &bao_browser::PageHandle) -> String {
    let _ = page.evaluate_js_web("window.__workerFP = null;");
    let body = encode_worker_body(&make_worker_body(FP_CANONICAL_JS));
    let created = page.evaluate_js_web(&make_worker_driver(&body));
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "Worker creation must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("Worker creation dispatch failed: {e}"),
    }

    let fp = wait_for_worker_fp(page, Duration::from_secs(15)).unwrap_or_else(|| {
        panic!(
            "worker digest did not arrive within timeout — worker postMessage → \
             onmessage (C3) or worker execution (C1) is broken on the live path"
        )
    });
    assert!(
        !fp.starts_with("WORKER-ERROR:"),
        "Worker script threw on the live path: {fp}"
    );
    assert!(
        !fp.starts_with("CREATE-ERROR:"),
        "Worker constructor failed on the live path: {fp}"
    );
    assert!(
        fp.contains("ua=") && fp.contains('|'),
        "canonical digest must carry the fingerprint surface fields, got: {fp}"
    );
    fp
}

// ═══════════════════════════════════════════════════════════════════════
// §1 C12 — worker navigator === main-thread navigator (per-field)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12] worker navigator === main navigator (live)
///
/// E22 audit C12: the stealth injection chain was complete but had no live
/// consistency test. This drives a real servo Worker thread under a stealth
/// profile and asserts, FIELD BY FIELD in Rust, that the Worker's
/// navigator.userAgent / platform / hardwareConcurrency / language(s) equal
/// the main thread's values — and that both equal the profile's configured
/// values (deterministic anchor, not just internal agreement).
#[test]
fn c12_worker_navigator_matches_main_thread_per_field() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile.clone()));

    let worker_fp = worker_digest(&page);
    let main_fp = read_main_fp(&page);

    let worker_fields = parse_fields(&worker_fp);
    let main_fields = parse_fields(&main_fp);

    // Field sets must be identical (same algorithm both sides).
    let wk: Vec<&str> = worker_fields.iter().map(|(k, _)| k.as_str()).collect();
    let mk: Vec<&str> = main_fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        wk, mk,
        "worker and main thread must expose the identical canonical field set"
    );

    // C12 criterion-named fields, asserted per-field with both raw values.
    for key in ["ua", "platform", "hwc", "language", "languages"] {
        let wv = worker_fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let mv = main_fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        assert_eq!(
            wv, mv,
            "C12: worker navigator.{key} must equal main-thread navigator.{key} \
             (worker={wv:?} main={mv:?})"
        );
    }

    // Every remaining surface must agree too (same-profile inheritance).
    for (key, wv) in &worker_fields {
        let mv = main_fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        assert_eq!(
            wv, &mv,
            "C12: surface {key} must match across worker/main (worker={wv:?} main={mv:?})"
        );
    }

    // Deterministic anchor: the shared values ARE the profile's values —
    // proving the surface is profile-driven, not servo-native passthrough.
    let field = |key: &str| {
        worker_fields
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        field("ua"),
        profile.navigator.user_agent,
        "C12: worker navigator.userAgent must equal the stealth profile UA"
    );
    assert_eq!(
        field("platform"),
        profile.navigator.platform,
        "C12: worker navigator.platform must equal the stealth profile platform"
    );
    assert_eq!(
        field("hwc"),
        profile.navigator.hardware_concurrency.to_string(),
        "C12: worker navigator.hardwareConcurrency must equal the profile value"
    );
    assert_eq!(
        field("language"),
        profile.navigator.language,
        "C12: worker navigator.language must equal the profile language"
    );

    eprintln!("[C12] worker navigator === main navigator === profile (per-field) passed");
}

// ═══════════════════════════════════════════════════════════════════════
// §2 C17 — worker digest === main digest (CreepJS-style comparison)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:17] worker digest === main digest (live)
///
/// E22 audit C17: the repo had zero worker-vs-main fingerprint comparison.
/// The canonical digest (navigator surface + screen surface, sorted,
/// test-defined algorithm) is computed inside the Worker thread and posted
/// back, then computed on the main thread with the SAME snippet, and the two
/// digests are asserted byte-equal — the CreepJS/sannysoft worker-vs-main
/// comparison shape from the criterion text.
#[test]
fn c17_worker_fingerprint_digest_equals_main_thread_digest() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(Some(profile));

    let worker_fp = worker_digest(&page);
    let main_fp = read_main_fp(&page);

    // The digest must be non-trivial on both sides.
    assert!(
        worker_fp.len() > 64 && main_fp.len() > 64,
        "digest must be non-trivial (worker={} chars, main={} chars)",
        worker_fp.len(),
        main_fp.len()
    );
    assert!(
        !worker_fp.contains("undefined=undefined") && !main_fp.contains("undefined=undefined"),
        "digest must not degenerate"
    );

    assert_eq!(
        worker_fp, main_fp,
        "C17: worker digest must equal main-thread digest\nworker: {worker_fp}\nmain:   {main_fp}"
    );

    eprintln!("[C17] worker digest === main digest passed: {worker_fp}");
}

// ═══════════════════════════════════════════════════════════════════════
// §3 C16 — stealth worker surface differs from no-stealth baseline
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:16] no native leak vs no-stealth baseline (live)
///
/// E22 audit C16: no test compared the stealth Worker surface against a
/// no-stealth baseline. Baseline phase runs FIRST (clean process state —
/// no stealth thread-locals/globals installed yet), collects the Worker
/// digest from a profile-less page; the stealth phase then collects the
/// Worker digest from a profiled page in the same runtime (worker scope
/// callbacks are webview-partitioned — BCE-20260627-009 — so the baseline
/// worker can never consume the stealth page's callback).
///
/// Asserts:
///   1. At least one surface differs between baseline and stealth worker
///      (criterion text: "与无 stealth 基线对比应不同").
///   2. The stealth worker's covered surfaces equal the profile's configured
///      values (proves the surface is profile-driven — no servo-native
///      passthrough).
#[test]
fn c16_worker_surface_differs_from_no_stealth_baseline() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

    // ── Phase 1: no-stealth baseline worker (created before any stealth
    //    install runs in this process).
    let page_base = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: None,
            ..Default::default()
        })
        .expect("gated live test: baseline create_page must succeed");
    let baseline_fp = worker_digest(&page_base);

    // ── Phase 2: stealth worker from a profiled page.
    let page_stealth = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(profile.clone()),
            ..Default::default()
        })
        .expect("gated live test: stealth create_page must succeed");
    let stealth_fp = worker_digest(&page_stealth);

    // Diagnostic table (always printed under --nocapture).
    for d in field_diffs(&stealth_fp, &baseline_fp) {
        eprintln!("[C16] diff {d}");
    }

    // 1. The stealth worker surface must differ from the no-stealth baseline.
    assert_ne!(
        stealth_fp, baseline_fp,
        "C16: stealth worker surface must differ from the no-stealth baseline \
         (identical surfaces mean stealth is not covering the worker at all)"
    );
    let diff_count = field_diffs(&stealth_fp, &baseline_fp).len();
    assert!(
        diff_count >= 1,
        "C16: at least one surface must differ between stealth worker and \
         no-stealth baseline"
    );

    // 2. The stealth worker's covered surfaces ARE the profile values —
    //    no servo-native passthrough.
    let stealth_fields = parse_fields(&stealth_fp);
    let get = |key: &str| {
        stealth_fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        get("ua"),
        profile.navigator.user_agent,
        "C16: stealth worker userAgent must be the profile UA (no native passthrough)"
    );
    assert_eq!(
        get("platform"),
        profile.navigator.platform,
        "C16: stealth worker platform must be the profile platform"
    );
    assert_eq!(
        get("hwc"),
        profile.navigator.hardware_concurrency.to_string(),
        "C16: stealth worker hardwareConcurrency must be the profile value"
    );
    assert_eq!(
        get("language"),
        profile.navigator.language,
        "C16: stealth worker language must be the profile language"
    );
    assert_eq!(
        get("languages"),
        profile.navigator.languages.join(","),
        "C16: stealth worker languages must be the profile languages"
    );

    eprintln!(
        "[C16] stealth vs baseline: {diff_count} surfaces differ; stealth worker \
         equals profile on covered surfaces — no native passthrough"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 Unit checks (no servo required)
// ═══════════════════════════════════════════════════════════════════════

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] digest field set parity
///
/// The digest algorithm must produce the same field set regardless of which
/// values the realm resolves — sort order + key set are the comparison
/// contract between the two realms.
#[test]
fn canonical_digest_field_set_is_stable() {
    let fields = parse_fields(
        "hwc=8|language=en-US|platform=Linux x86_64|ua=Mozilla/5.0 test|vendor=",
    );
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["hwc", "language", "platform", "ua", "vendor"]);
    assert_eq!(
        fields.iter().find(|(k, _)| k == "ua").map(|(_, v)| v.clone()),
        Some("Mozilla/5.0 test".to_string())
    );
    // Empty value must survive parsing (vendor="" in the firefox profile).
    assert_eq!(
        fields.iter().find(|(k, _)| k == "vendor").map(|(_, v)| v.clone()),
        Some(String::new())
    );
}

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] worker body builder
#[test]
fn worker_body_embeds_canonical_expression() {
    let body = make_worker_body(FP_CANONICAL_JS);
    assert!(body.starts_with("var __fp = (function () {"));
    assert!(body.contains("self.postMessage(__fp);"));
    assert!(body.contains("hardwareConcurrency"));
}

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] data: URL encoding
#[test]
fn encode_worker_body_percent_encodes_for_data_url() {
    let enc = encode_worker_body("var a='x|y'; self.postMessage(a);");
    assert!(!enc.contains('\''), "quotes must be percent-encoded");
    assert!(!enc.contains('|'), "pipe must be percent-encoded");
    assert!(!enc.contains(' '), "spaces must be percent-encoded");
    assert!(enc.contains("self.postMessage"));
}
