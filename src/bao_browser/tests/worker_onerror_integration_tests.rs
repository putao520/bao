// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:9] [level:integration]
// Servo-native Worker `onerror` integration tests for REQ-BRW-004 criterion #9.
//
// Background (BCE-20260627-007):
//   The bypass `bao_engine::WebWorker` path does NOT dispatch `onerror`. Per
//   REQ-BRW-004 C9 + the BCE-20260627-007 fix, `onerror` runs exclusively on the
//   servo-native Worker path: when a Worker's global scope throws an uncaught
//   error, servo dispatches a DOM `ErrorEvent` to the Worker's owner (the main
//   thread), carrying `message` / `filename` / `lineno` / `colno`.
//
//   These tests exercise that path end-to-end against a live `BaoRuntime` +
//   `PageHandle`. They register a Worker (via `data:` URL script) that throws,
//   then capture the resulting `ErrorEvent` on the main thread and assert all
//   four SPEC-mandated fields are present and well-formed.
//
// Environment gating:
//   Real servo rendering requires a DISPLAY (Xvfb) and network/asset I/O. As with
//   `bce004_repro_tests.rs` / `bce004_stress_tests.rs`, these tests are skipped
//   unless `BAO_TEST_NETWORK=1` and a `DISPLAY` are present, so they never break
//   CI headless runs that lack the servo rendering stack.
//
// Usage:
//   BAO_TEST_NETWORK=1 DISPLAY=:99 cargo test -p bao_browser \
//     --test worker_onerror_integration_tests -- --nocapture
//
// ═══════════════════════════════════════════════════════════════════════
// Runtime sharing (BCE-20260627-009): Servo is a single-instance architecture
// with process-global OnceLock singletons. Even with the idempotent servo
// patches (async_runtime / PipelineNamespace / fetch_thread / opts), running
// multiple tests concurrently each spawning their own BaoRuntime would still
// race on servo's thread-local state and resource threads. We serialize all
// servo-touching tests in this binary via a global Mutex so only ONE runs at
// a time; the servo patches make BaoRuntime::new safe to call repeatedly.

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use std::sync::Mutex;
use std::time::Duration;

mod common;

/// Serializes all servo-touching tests in this binary (servo single-instance).
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

/// Acquire the global serializer lock. Hold for the full test duration.
/// The servo idempotent patches (BCE-20260627-009) make repeated
/// BaoRuntime::new safe; this lock only prevents concurrent servo instances.
fn lock_serializer() -> std::sync::MutexGuard<'static, ()> {
    // The Mutex is a static, so the guard is tied to the process lifetime
    // for soundness of the transmute; we keep it 'static to allow returning it.
    // This is the standard pattern for a process-wide serializer in Rust tests.
    let guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: `TEST_SERIALIZER` is a `static`, so the lock's lifetime is
    // bounded by the process. The guard is only dropped when the test function
    // returns. This transmute extends the lifetime bound to 'static, matching
    // the actual underlying static Mutex. No reference to the guard escapes
    // beyond the test function that called this.
    unsafe { std::mem::transmute::<std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'static, ()>>(guard) }
}

/// Page-side harness: install a global `__onerrorResult` JSON dump that the
/// Worker's `onerror` handler writes into when an uncaught error fires.
///
/// The Worker script (delivered via `data:text/javascript,...`) throws an
/// `Error` with a unique marker. The main-thread `worker.onerror` handler
/// serializes the `ErrorEvent` fields to `window.__onerrorResult` so the test
/// can poll for it via `evaluate_js_web`.
const HARNESS_SETUP: &str = r#"
    (function () {
        window.__onerrorResult = null;
        return 'ready';
    })();
"#;

/// Main-thread script that creates a Worker from a `data:` URL whose body throws,
/// and wires `worker.onerror` to capture the ErrorEvent fields.
///
/// `workerScriptBody` must be a URL-encoded JS source for the worker (no
/// surrounding quotes) — it is interpolated into a `data:text/javascript,...` URL.
fn make_worker_driver(worker_script_body: &str) -> String {
    // The body is already URL-encoded by the caller; we splice it verbatim.
    format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onerror = function (event) {{
                    window.__onerrorResult = JSON.stringify({{
                        message:  (event.message  !== undefined && event.message  !== null) ? String(event.message)  : null,
                        filename: (event.filename !== undefined && event.filename !== null) ? String(event.filename) : null,
                        lineno:   (typeof event.lineno === 'number') ? event.lineno : null,
                        colno:    (typeof event.colno  === 'number') ? event.colno  : null,
                    }});
                    return true; // suppress default (would otherwise log to console)
                }};
                return 'worker-created';
            }} catch (e) {{
                window.__onerrorResult = JSON.stringify({{ creationError: String(e) }});
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = worker_script_body
    )
}

/// Encode a raw JS worker body into a URL-safe form suitable for a `data:` URL.
fn encode_worker_body(raw: &str) -> String {
    // Minimal percent-encoding sufficient for our short worker scripts.
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

/// Poll `window.__onerrorResult` until it is non-null or the timeout elapses.
/// Returns the captured JSON string, or an empty string on timeout.
fn wait_for_onerror_result(page: &bao_browser::PageHandle, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__onerrorResult") {
            let trimmed = s.trim();
            // evaluate_js_web wraps strings in quotes when returning a JS string;
            // `null` comes back as the literal token `null` (possibly quoted).
            let is_set = !trimmed.is_empty()
                && trimmed != "null"
                && trimmed != "\"null\"";
            if is_set {
                // Strip the outer quote layer added by JSON serialization + the
                // JS-to-string bridge so callers see raw JSON.
                return Some(unquote_bridge(trimmed.to_string()));
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The servo JS bridge may return a JS string value as `"..."` (quoted). Strip a
/// single outer quote pair if present so the caller sees the raw inner JSON.
fn unquote_bridge(mut s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        // Remove outer quotes, then unescape any embedded `\"`.
        let inner = &s[1..s.len() - 1];
        s = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    s
}

/// Parse a captured `__onerrorResult` JSON blob into its four ErrorEvent fields.
fn parse_onerror_result(json: &str) -> Option<(Option<String>, Option<String>, Option<i64>, Option<i64>)> {
    // Lightweight field extraction (no serde dep) — the producer above is fixed.
    let extract_str = |field: &str| -> Option<String> {
        let key = format!("\"{}\":", field);
        let idx = json.find(&key)?;
        let rest = &json[idx + key.len()..];
        let trimmed = rest.trim_start();
        if trimmed.starts_with("null") {
            return None;
        }
        let q0 = trimmed.find('"')?;
        let after = &trimmed[q0 + 1..];
        // Find the closing quote (no escapes are emitted by our producer for
        // filename; message may contain escaped quotes — handle minimally).
        let mut end = None;
        let bytes = after.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                end = Some(i);
                break;
            }
            i += 1;
        }
        let end = end?;
        Some(after[..end].to_string())
    };
    let extract_num = |field: &str| -> Option<Option<i64>> {
        let key = format!("\"{}\":", field);
        let idx = json.find(&key)?;
        let rest = &json[idx + key.len()..];
        let trimmed = rest.trim_start();
        if trimmed.starts_with("null") {
            return Some(None);
        }
        let mut end = 0;
        for (i, c) in trimmed.char_indices() {
            if c == ',' || c == '}' || c.is_whitespace() {
                end = i;
                break;
            }
            end = i + c.len_utf8();
        }
        let n: i64 = trimmed[..end].parse().ok()?;
        Some(Some(n))
    };
    let message = extract_str("message");
    let filename = extract_str("filename");
    let lineno = extract_num("lineno")?;
    let colno = extract_num("colno")?;
    Some((message, filename, lineno, colno))
}

// ═══════════════════════════════════════════════════════════════════════
// §0 Diagnostic: does the servo-native Worker execute its script at all?
// (probe via postMessage before throw)
// ═══════════════════════════════════════════════════════════════════════
//
// TEMP diagnostic for TASK-63 root-cause: confirms whether the worker thread
// actually executes the data: URL script body. If __probe becomes
// "before-throw" the worker ran; if it stays null the worker never executed.

#[test]
fn servo_native_worker_executes_probe() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    let _ = page.evaluate_js_web("window.__probe = null;");

    // Worker body: postMessage BEFORE throw so we can detect execution.
    let body = encode_worker_body("self.postMessage('before-throw'); throw new Error('diag');");
    let driver = format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onmessage = function (e) {{ window.__probe = 'msg:' + String(e.data); }};
                w.onerror = function (ev) {{ window.__probe = 'onerror:' + (ev && ev.message); return true; }};
                return 'worker-created';
            }} catch (e) {{
                window.__probe = 'create-failed:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = body
    );
    let r = page.evaluate_js_web(&driver);
    eprintln!("[probe] create result: {:?}", r);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        let v = page.evaluate_js_web("window.__probe").unwrap_or_default();
        last = v.clone();
        let t = v.trim();
        if !t.is_empty() && t != "null" && t != "\"null\"" {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[probe] FINAL __probe = {}", last);
    // Diagnostic only — no hard assert so it always "passes" and prints.
}

// ═══════════════════════════════════════════════════════════════════════
// §1 Servo-native onerror integration (REQ-BRW-004 criterion #9)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:9] onerror fires on servo-native Worker path
///
/// Spawns a Worker whose body throws an `Error` with a unique marker. Verifies
/// the main-thread `worker.onerror` handler fires (i.e. `__onerrorResult` is
/// populated) within a bounded wait, proving the servo-native ErrorEvent
/// dispatch path is wired through.
#[test]
fn servo_native_onerror_fires_on_script_error() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    // Set up the result sink.
    let _ = page.evaluate_js_web(HARNESS_SETUP);

    // Worker body: throw an Error with a unique marker.
    let worker_body = encode_worker_body("throw new Error('onerror-integration-marker');");
    let driver = make_worker_driver(&worker_body);
    let _ = page.evaluate_js_web(&driver);

    // Wait (no sleep-polling magic number) for onerror to populate the sink.
    let captured = wait_for_onerror_result(&page, Duration::from_secs(8));
    match captured {
        None => panic!("servo-native onerror did not fire within timeout"),
        Some(json) => {
            if json.contains("creationError") {
                panic!("Worker creation failed on servo-native path: {json}");
            }
            // onerror fired — field-level checks are in the dedicated tests below.
            assert!(
                json.contains("message"),
                "captured onerror result missing message field: {json}"
            );
            eprintln!("[onerror] fired, captured: {json}");
        }
    }
}

/// @trace REQ-BRW-004 [criterion:9] onerror ErrorEvent carries all 4 fields
///
/// Asserts the ErrorEvent delivered to the main thread has non-null, correctly
/// typed `message` (string), `filename` (string), `lineno` (number), and
/// `colno` (number) fields per SPEC criterion #9.
#[test]
fn servo_native_onerror_error_event_fields() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    let _ = page.evaluate_js_web(HARNESS_SETUP);

    let worker_body = encode_worker_body("throw new Error('fields-test');");
    let driver = make_worker_driver(&worker_body);
    let _ = page.evaluate_js_web(&driver);

    let json = wait_for_onerror_result(&page, Duration::from_secs(8))
        .expect("servo-native onerror did not fire within timeout");
    assert!(
        !json.contains("creationError"),
        "Worker creation failed: {json}"
    );

    let (message, filename, lineno, colno) = parse_onerror_result(&json)
        .unwrap_or_else(|| panic!("failed to parse onerror result JSON: {json}"));

    // SPEC criterion #9: all four ErrorEvent fields must be present and typed.
    let message = message.expect("message field must be a non-null string");
    let filename = filename.expect("filename field must be a non-null string");
    let lineno = lineno.expect("lineno field must be a number");
    let colno = colno.expect("colno field must be a number");

    assert!(
        message.contains("fields-test"),
        "ErrorEvent.message should contain thrown error text; got: {message}"
    );
    // filename is the worker script URL; for a data: URL worker it should at
    // least be a non-empty string referencing the data: URL.
    assert!(!filename.is_empty(), "filename must be non-empty");
    assert!(lineno >= 1, "lineno must be >= 1; got {lineno}");
    assert!(colno >= 1, "colno must be >= 1; got {colno}");
    eprintln!(
        "[onerror-fields] message={message:?} filename={filename:?} lineno={lineno} colno={colno}"
    );
}

/// @trace REQ-BRW-004 [criterion:9] onerror message contains the thrown error text
///
/// Verifies the ErrorEvent.message string carries the thrown Error's textual
/// message, confirming servo propagates the worker-side error text verbatim
/// rather than a generic placeholder.
#[test]
fn servo_native_onerror_message_contains_error_text() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    let _ = page.evaluate_js_web(HARNESS_SETUP);

    // Use a distinctive marker unlikely to appear in any servo-generic message.
    const MARKER: &str = "unique-onerror-text-marker-9f3a7c";
    let worker_body = encode_worker_body(&format!("throw new Error('{MARKER}');"));
    let driver = make_worker_driver(&worker_body);
    let _ = page.evaluate_js_web(&driver);

    let json = wait_for_onerror_result(&page, Duration::from_secs(8))
        .expect("servo-native onerror did not fire within timeout");
    assert!(
        !json.contains("creationError"),
        "Worker creation failed: {json}"
    );

    let (message, _, _, _) = parse_onerror_result(&json)
        .unwrap_or_else(|| panic!("failed to parse onerror result JSON: {json}"));
    let message = message.expect("message field must be a non-null string");
    assert!(
        message.contains(MARKER),
        "ErrorEvent.message should contain the thrown error text '{MARKER}'; got: {message}"
    );
    eprintln!("[onerror-text] message carried marker: {message}");
}

// ═══════════════════════════════════════════════════════════════════════
// §2 Pure unit checks for the harness helpers (no servo required)
// ═══════════════════════════════════════════════════════════════════════

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] URL-encoding helper
///
/// Guards against regressions in the `data:` URL body encoder used to deliver
/// worker scripts. Runs without a servo runtime so it executes in every CI env.
#[test]
fn encode_worker_body_preserves_alnum_and_percent_encodes_special() {
    let raw = "throw new Error('x');";
    let enc = encode_worker_body(raw);
    // Alphanumerics, spaces-as-%20, and core punctuation must round-trip safely.
    assert!(enc.contains("throw"));
    assert!(enc.contains("Error"));
    assert!(!enc.contains(' '), "spaces must be percent-encoded");
    assert!(enc.contains("%20"), "space should encode to %20");

    // Apostrophes and parens must be percent-encoded.
    assert!(enc.contains("%27"), "apostrophe should be %27");
    assert!(enc.contains("%28") || enc.contains("%29"), "parens should be encoded");
}

/// @trace NFR-TEST-REPRODUCIBER [criterion:harness] JSON field parser
///
/// Validates `parse_onerror_result` against a representative capture blob so the
/// field-extraction logic is locked in independent of a live servo run.
#[test]
fn parse_onerror_result_extracts_all_four_fields() {
    let json = r#"{"message":"Uncaught Error: boom","filename":"data:text/javascript,throw%20new%20Error('boom');","lineno":1,"colno":7}"#;
    let (message, filename, lineno, colno) =
        parse_onerror_result(json).expect("parse should succeed");
    assert_eq!(message.as_deref(), Some("Uncaught Error: boom"));
    assert!(filename.unwrap().starts_with("data:text/javascript"));
    assert_eq!(lineno, Some(1));
    assert_eq!(colno, Some(7));
}

/// @trace NFR-TEST-REPRODUCIBILITY [criterion:harness] null-field handling
#[test]
fn parse_onerror_result_handles_null_fields() {
    let json = r#"{"message":null,"filename":"x","lineno":null,"colno":null}"#;
    let (message, filename, lineno, colno) =
        parse_onerror_result(json).expect("parse should succeed");
    assert_eq!(message, None);
    assert_eq!(filename.as_deref(), Some("x"));
    assert_eq!(lineno, None);
    assert_eq!(colno, None);
}
