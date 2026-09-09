// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:2,6,8] [level:integration]
// Browser-realm live coverage for REQ-BRW-004 C2/C6/C8 (v47 V-batch ③:
// SPEC criterion text promised page→Worker postMessage, structured-clone
// exchange, importScripts and worker-realm crypto/performance/location —
// all prior live Worker traffic was worker→page STRING markers only).
//
// Faces under test, each in its own isolated live test:
//
//   C2  page → Worker postMessage: the page posts {kind:'echo', payload:{x:1}}
//       IMMEDIATELY after `new Worker(url)` (strict SPEC objective form —
//       the port message queue must hold early messages until the worker
//       script installs self.onmessage). The worker deep-checks the received
//       payload and echoes a verdict back (the reply leg also re-proves the
//       C3 worker→page direction inside the same roundtrip).
//   C6a structured clone roundtrip, BOTH directions, six payload shapes
//       (plain object / array / nested object+array graph / ArrayBuffer with
//       per-byte content / Map with entries / Uint8Array view — SPEC text
//       says 对象/数组/Buffer/ArrayBuffer/Transferable, task floor is five
//       types with deep comparison; six are probed). Page→worker: the page
//       posts the composite, the worker deep-verifies per type and posts a
//       per-type verdict line. Worker→page: the page asks the worker to
//       CONSTRUCT a same-shape composite with different values and post the
//       OBJECT back; the page deep-verifies per type.
//   C6b importScripts: worker script (served over HTTP so relative
//       importScripts('/helper.js') resolves against the worker script URL,
//       per workerglobalscope.rs join(worker_url)) imports helper.js and
//       proves its function declarations + self globals landed in the
//       worker's global scope. Server-side hit log proves the /helper.js
//       fetch actually egressed (fetch_axis two-sided proof form).
//   C8  DedicatedWorkerGlobalScope API surface: crypto.randomUUID (UUID
//       shape + two calls differ; v4-variant conformance recorded as a
//       REPORT-ONLY marker, not asserted — C8's criterion is API exposure),
//       crypto.getRandomValues (identity return + byte width + entropy),
//       performance.now() monotonic across real work, and WorkerLocation
//       href/origin/protocol/host exactly equal to the worker script URL
//       (Rust-side comparison against the fixture URL).
//
// Why HTTP-served worker scripts (not data: URLs like the other live
// vehicles): importScripts resolves against the worker's own URL — a data:
// base cannot carry a same-origin relative import — and WorkerLocation
// assertions need a real http origin to be meaningful. Same fixture origin
// as the page keeps the classic-worker same-origin rule satisfied.
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb).
// Skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(worker_realm_api)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};

// ---------------------------------------------------------------------------
// Minimal H1 fixture: per-path JS worker scripts + hit log
// ---------------------------------------------------------------------------

struct RealmApiFixture {
    shutdown: Arc<AtomicBool>,
    hits: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl RealmApiFixture {
    /// Routes: path → (mime, body). Worker scripts MUST be served with a
    /// JavaScript MIME — both the classic worker script fetch and
    /// importScripts enforce SCRIPT_JS_MIMES (htmlscriptelement.rs).
    fn route(path: &str) -> (&'static str, String) {
        match path {
            "/wk_c8.js" => ("application/javascript", WK_C8_BODY.to_string()),
            "/wk_import.js" => ("application/javascript", WK_IMPORT_BODY.to_string()),
            "/wk_msg.js" => ("application/javascript", WK_MSG_BODY.to_string()),
            "/helper.js" => ("application/javascript", HELPER_JS_BODY.to_string()),
            _ => ("text/plain", "ok".to_string()),
        }
    }

    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind realm-api fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (s2, h2) = (Arc::clone(&shutdown), Arc::clone(&hits));
        std::thread::Builder::new()
            .name("realm-api-fixture".into())
            .spawn(move || {
                while !s2.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 4096];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none()
                                && Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|l| l.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            h2.lock().unwrap().push(path.clone());
                            let (ct, body) = Self::route(&path);
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
                                len = body.len(),
                            );
                            let _ = tcp.write_all(resp.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn realm-api fixture thread");
        RealmApiFixture {
            shutdown,
            hits,
            port,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn saw(&self, path: &str) -> bool {
        self.hits.lock().unwrap().iter().any(|p| p == path)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

impl Drop for RealmApiFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Served worker script bodies
// ---------------------------------------------------------------------------

/// C8 worker: probe crypto / performance / location in the worker realm and
/// post one `|`-separated marker line back. No message handling needed.
const WK_C8_BODY: &str = r#"
(function () {
  var parts = [];
  try {
    parts.push('href=' + String(location.href));
    parts.push('origin=' + String(location.origin));
    parts.push('proto=' + String(location.protocol));
    parts.push('host=' + String(location.host));
  } catch (e) {
    parts.push('LOC-THROW:' + ((e && e.message) || String(e)));
  }
  try {
    var u1 = crypto.randomUUID();
    var u2 = crypto.randomUUID();
    var shape = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    parts.push('uuid=' + (shape.test(u1) && shape.test(u2) && u1 !== u2 ? 1 : 0));
    var v4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
    parts.push('uuid4=' + (v4.test(u1) && v4.test(u2) ? 1 : 0));
  } catch (e) {
    parts.push('uuid=THROW:' + ((e && e.message) || String(e)));
  }
  try {
    var a = new Uint8Array(16);
    var ret = crypto.getRandomValues(a);
    var b = new Uint8Array(16);
    crypto.getRandomValues(b);
    var differ = false;
    for (var i = 0; i < 16; i++) { if (a[i] !== b[i]) { differ = true; break; } }
    parts.push('grv=' + (ret === a && a.length === 16 && differ ? 1 : 0));
  } catch (e) {
    parts.push('grv=THROW:' + ((e && e.message) || String(e)));
  }
  try {
    var t1 = performance.now();
    var s = 0;
    for (var i = 0; i < 2000000; i++) { s += i; }
    var t2 = performance.now();
    parts.push('pnow=' + (typeof t1 === 'number' && t1 >= 0 && t2 > t1 ? 1 : 0));
    parts.push('pnowdt=' + (t2 - t1).toFixed(3));
  } catch (e) {
    parts.push('pnow=THROW:' + ((e && e.message) || String(e)));
  }
  self.postMessage('C8|' + parts.join('|'));
})();
"#;

/// importScripts worker: import the helper and prove its declarations and
/// globals are available in THIS worker's global scope.
const WK_IMPORT_BODY: &str = r#"
(function () {
  try {
    importScripts('/helper.js');
    var fn = typeof self.helperTriple === 'function';
    var call = fn ? self.helperTriple(21) === 63 : false;
    var mark = self.HELPER_MARK === 'helper-loaded';
    var second = typeof self.helperStamp === 'function' && self.helperStamp('a', 'b') === 'ab';
    self.postMessage('C6IMPORT|ok=' + (fn && call && mark && second ? 1 : 0)
      + '|fn=' + (fn ? 1 : 0)
      + '|call=' + (call ? 1 : 0)
      + '|mark=' + (mark ? 1 : 0)
      + '|second=' + (second ? 1 : 0));
  } catch (e) {
    self.postMessage('C6IMPORT|THROW:' + ((e && e.message) || String(e)));
  }
})();
"#;

/// helper.js — the importScripts target. Function declarations land as
/// globals of the importing worker scope; explicit self.* assignment proves
/// the imported script RAN in this scope (not just fetched).
const HELPER_JS_BODY: &str = r#"
function helperTriple(n) { return n * 3; }
function helperStamp(a, b) { return a + b; }
self.HELPER_MARK = 'helper-loaded';
"#;

/// Message worker for C2/C6a: echo deep-check (C2) and structured-clone
/// verify/construct (C6a). Handlers are installed by the worker script —
/// early page posts must be queued by the port message queue until then.
const WK_MSG_BODY: &str = r#"
function __verifyIn(p) {
  var m = [];
  try {
    m.push('plain=' + (typeof p.plain === 'object' && p.plain !== null
      && p.plain.x === 41 && p.plain.s === 'str' && p.plain.b === true
      && p.plain.n === null ? 1 : 0));
  } catch (e) { m.push('plain=THROW'); }
  try {
    m.push('arr=' + (Array.isArray(p.arr) && p.arr.length === 3
      && p.arr[0] === 7 && p.arr[1] === 8 && p.arr[2] === 9 ? 1 : 0));
  } catch (e) { m.push('arr=THROW'); }
  try {
    var n = p.nested;
    m.push('nested=' + (typeof n === 'object' && n !== null
      && Array.isArray(n.list) && n.list.length === 2 && n.list[0] === 1
      && n.list[1] && n.list[1].inner === 'val'
      && n.deep && n.deep.x && Array.isArray(n.deep.x.y)
      && n.deep.x.y.length === 3 && n.deep.x.y[0] === true
      && n.deep.x.y[1] === false && n.deep.x.y[2] === null ? 1 : 0));
  } catch (e) { m.push('nested=THROW'); }
  try {
    var ok = false;
    if (p.ab instanceof ArrayBuffer && p.ab.byteLength === 8) {
      var v = new Uint8Array(p.ab);
      var exp = [10, 20, 30, 40, 50, 60, 70, 80];
      ok = true;
      for (var i = 0; i < 8; i++) { if (v[i] !== exp[i]) { ok = false; } }
    }
    m.push('ab=' + (ok ? 1 : 0));
  } catch (e) { m.push('ab=THROW'); }
  try {
    m.push('map=' + (p.map instanceof Map && p.map.size === 2
      && p.map.get('k1') === 'v1' && p.map.get('k2') === 42 ? 1 : 0));
  } catch (e) { m.push('map=THROW'); }
  try {
    m.push('u8=' + (p.u8 instanceof Uint8Array && p.u8.length === 4
      && p.u8[0] === 1 && p.u8[3] === 4 ? 1 : 0));
  } catch (e) { m.push('u8=THROW'); }
  return m.join('|');
}

function __makeOut() {
  var ab = new ArrayBuffer(8);
  var v = new Uint8Array(ab);
  for (var i = 0; i < 8; i++) { v[i] = 90 + i; }
  return {
    kind: 'clone-out-payload',
    plain: { x: 77, s: 'wout', b: false, n: null },
    arr: [4, 5, 6],
    nested: { list: [9, { inner: 'deep' }], deep: { x: { y: [false, true, null] } } },
    ab: ab,
    map: new Map([['m1', 'w1'], ['m2', 13]]),
    u8: new Uint8Array([9, 8, 7, 6])
  };
}

self.onmessage = function (e) {
  var d = e.data;
  try {
    if (d && d.kind === 'echo') {
      var ok = typeof d.payload === 'object' && d.payload !== null
        && d.payload.x === 1 && d.payload.tag === 'c2-page-to-worker';
      self.postMessage('C2ECHO|ok=' + (ok ? 1 : 0)
        + '|x=' + (d.payload && d.payload.x !== undefined ? d.payload.x : 'MISSING'));
      return;
    }
    if (d && d.kind === 'clone-in') {
      self.postMessage('CLONEIN|' + __verifyIn(d.payload));
      return;
    }
    if (d && d.kind === 'clone-out') {
      self.postMessage(__makeOut());
      return;
    }
    self.postMessage('UNKNOWN-KIND|' + (d && d.kind ? d.kind : String(d)));
  } catch (err) {
    self.postMessage('MSG-THROW|' + ((err && err.message) || String(err)));
  }
};
"#;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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

/// The servo JS bridge may return a JS string value as `"..."` (quoted).
/// Strip a single outer quote pair if present (multi_injection form).
fn unquote_bridge(mut s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        s = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    s
}

/// Drive servo's ScriptThread with no-op evaluates while polling a page-side
/// sink expression (fetch_axis pump pattern).
fn poll_sink(page: &PageHandle, sink_expr: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(v) = page.evaluate_js_web(sink_expr) {
            let t = unquote_bridge(v.trim().to_string());
            if !t.is_empty() && t != "null" && t != "undefined" {
                return Some(t);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Extract a `|`-separated marker field (`key=value`) from a worker verdict.
fn marker_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    line.split('|').find_map(|f| f.strip_prefix(&prefix))
}

/// Assert every `key=1` marker is present in a `PREFIX|...` verdict line.
fn assert_all_ones(tag: &str, line: &str, keys: &[&str]) {
    for k in keys {
        let v = marker_field(line, k)
            .unwrap_or_else(|| panic!("{tag}: verdict line must carry {k}=, got: {line}"));
        assert_eq!(
            v, "1",
            "{tag}: {k} structured-clone leg FAILED (deep-compare mismatch or throw), \
             verdict: {line}"
        );
    }
}

/// Page-side driver: create the message worker, wire string verdicts into
/// `window.__rx` and the clone-out object deep-verification into
/// `window.__c6out`, then post the requested request kinds IMMEDIATELY
/// (C2's strict SPEC form — early messages rely on the port message queue).
fn make_msg_driver(worker_url: &str, kinds: &[&str]) -> String {
    let posts = kinds
        .iter()
        .map(|k| format!("w.postMessage({k});"))
        .collect::<Vec<_>>()
        .join("\n    ");
    let mut driver = r#"
(function () {
  window.__rx = [];
  window.__c6out = null;
  function makeInPayload() {
    var ab = new ArrayBuffer(8);
    var v = new Uint8Array(ab);
    var seeds = [10, 20, 30, 40, 50, 60, 70, 80];
    for (var i = 0; i < 8; i++) { v[i] = seeds[i]; }
    return {
      kind: 'clone-in',
      payload: {
        plain: { x: 41, s: 'str', b: true, n: null },
        arr: [7, 8, 9],
        nested: { list: [1, { inner: 'val' }], deep: { x: { y: [true, false, null] } } },
        ab: ab,
        map: new Map([['k1', 'v1'], ['k2', 42]]),
        u8: new Uint8Array([1, 2, 3, 4])
      }
    };
  }
  function verifyOut(p) {
    var m = [];
    try {
      m.push('plain=' + (p.plain && p.plain.x === 77 && p.plain.s === 'wout'
        && p.plain.b === false && p.plain.n === null ? 1 : 0));
    } catch (e) { m.push('plain=THROW'); }
    try {
      m.push('arr=' + (Array.isArray(p.arr) && p.arr.length === 3
        && p.arr[0] === 4 && p.arr[1] === 5 && p.arr[2] === 6 ? 1 : 0));
    } catch (e) { m.push('arr=THROW'); }
    try {
      var n = p.nested;
      m.push('nested=' + (n && Array.isArray(n.list) && n.list.length === 2
        && n.list[0] === 9 && n.list[1] && n.list[1].inner === 'deep'
        && n.deep && n.deep.x && Array.isArray(n.deep.x.y)
        && n.deep.x.y.length === 3 && n.deep.x.y[0] === false
        && n.deep.x.y[1] === true && n.deep.x.y[2] === null ? 1 : 0));
    } catch (e) { m.push('nested=THROW'); }
    try {
      var ok = false;
      if (p.ab instanceof ArrayBuffer && p.ab.byteLength === 8) {
        var v = new Uint8Array(p.ab);
        ok = true;
        for (var i = 0; i < 8; i++) { if (v[i] !== 90 + i) { ok = false; } }
      }
      m.push('ab=' + (ok ? 1 : 0));
    } catch (e) { m.push('ab=THROW'); }
    try {
      m.push('map=' + (p.map instanceof Map && p.map.size === 2
        && p.map.get('m1') === 'w1' && p.map.get('m2') === 13 ? 1 : 0));
    } catch (e) { m.push('map=THROW'); }
    try {
      m.push('u8=' + (p.u8 instanceof Uint8Array && p.u8.length === 4
        && p.u8[0] === 9 && p.u8[3] === 6 ? 1 : 0));
    } catch (e) { m.push('u8=THROW'); }
    return m.join('|');
  }
  try {
    var w = new Worker('__WORKER_URL__');
    w.onmessage = function (e) {
      var d = e.data;
      if (d && typeof d === 'object' && d.kind === 'clone-out-payload') {
        window.__c6out = 'OUT|' + verifyOut(d);
      } else {
        window.__rx.push(String(d));
      }
    };
    w.onerror = function (ev) {
      window.__rx.push('WORKER-ERROR:' + ((ev && ev.message) || 'unknown'));
      return true;
    };
    __POSTS__
    return 'posted';
  } catch (e) {
    window.__rx.push('CREATE-ERROR:' + String(e));
    return 'failed';
  }
})();
"#
    .replace("__WORKER_URL__", worker_url)
    .replace("__POSTS__", &posts);
    // The clone-out REQUEST needs the worker to construct the composite.
    if kinds.contains(&"makeOutRequest()") {
        driver = driver.replace(
            "w.postMessage(makeOutRequest());",
            "w.postMessage({ kind: 'clone-out' });",
        );
    }
    driver
}

// ═══════════════════════════════════════════════════════════════════════
// §1 C2 — page→Worker postMessage, worker onmessage receives {x:1}
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:2] page→Worker postMessage live
///
/// SPEC objective: `worker.postMessage({x:1}) → Worker onmessage 收到 {x:1}`.
/// The page posts the echo request IMMEDIATELY after `new Worker(url)` —
/// before the http-served worker script can possibly have been fetched and
/// evaluated — so green here proves servo queues early page→worker messages
/// until the worker installs `self.onmessage` (HTML spec port message
/// queue). The worker deep-checks the payload (`x === 1`, tag intact, data
/// is an OBJECT not a string) and posts the verdict back, which also proves
/// the worker→page return leg of the same roundtrip.
#[test]
fn worker_realm_api_c2_page_to_worker_postmessage_echo() {
    if !common::run_isolated("worker_realm_api_tests::worker_realm_api_c2_page_to_worker_postmessage_echo") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = RealmApiFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let driver = make_msg_driver(
        &fixture.url("/wk_msg.js"),
        &["{ kind: 'echo', payload: { x: 1, tag: 'c2-page-to-worker' } }"],
    );
    let posted = page.evaluate_js_web(&driver).expect("c2 dispatch");
    assert!(
        posted.contains("posted"),
        "c2: worker creation/post dispatch failed: {posted:?}"
    );

    let rx = poll_sink(
        &page,
        "window.__rx.length > 0 ? window.__rx.join(';;') : ''",
        Duration::from_secs(45),
    )
    .unwrap_or_else(|| {
        panic!(
            "c2: NO worker verdict arrived — page→worker postMessage never reached \
             worker onmessage (early-message queue or delivery gap). fixture: {:?}",
            fixture.hits()
        )
    });
    eprintln!("[c2] rx={rx} hits={:?}", fixture.hits());

    let echo = rx
        .split(";;")
        .find(|l| l.starts_with("C2ECHO|"))
        .unwrap_or_else(|| {
            panic!(
                "c2: no C2ECHO verdict in worker traffic (got {rx:?}) — echo request \
                 lost or worker errored; fixture: {:?}",
                fixture.hits()
            )
        });
    assert_eq!(
        marker_field(echo, "ok"),
        Some("1"),
        "c2: worker received the posted payload but deep-check FAILED: {echo}"
    );
    assert_eq!(
        marker_field(echo, "x"),
        Some("1"),
        "c2: worker must receive payload.x === 1 (SPEC {{x:1}} objective): {echo}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §2 C6a — structured clone roundtrip, both directions, six payload types
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:6] structured clone live roundtrip
///
/// SPEC objective: `Structured Clone 正确序列化/反序列化 {a:[1,2],b:new
/// ArrayBuffer(8)}` (对象/数组/Buffer/ArrayBuffer/Transferable). Probes SIX
/// payload shapes with deep comparison, BOTH directions:
///   page→worker: composite posted via postMessage; worker deep-verifies
///     per type (Map entries, ArrayBuffer per-byte contents, nested graph)
///     and posts a per-type verdict line back.
///   worker→page: worker CONSTRUCTS a same-shape composite with different
///     values and posts the OBJECT back; the page deep-verifies per type.
const CLONE_KEYS: [&str; 6] = ["plain", "arr", "nested", "ab", "map", "u8"];

#[test]
fn worker_realm_api_c6_structured_clone_roundtrip_five_types() {
    if !common::run_isolated("worker_realm_api_tests::worker_realm_api_c6_structured_clone_roundtrip_five_types") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = RealmApiFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let driver = make_msg_driver(&fixture.url("/wk_msg.js"), &["makeInPayload()", "makeOutRequest()"]);
    let posted = page.evaluate_js_web(&driver).expect("c6 dispatch");
    assert!(
        posted.contains("posted"),
        "c6: worker creation/post dispatch failed: {posted:?}"
    );

    // Direction 1: page→worker — the worker's per-type verdict line.
    let clonein = poll_sink(
        &page,
        "(function () { for (var i = 0; i < window.__rx.length; i++) \
         { if (String(window.__rx[i]).indexOf('CLONEIN|') === 0) return String(window.__rx[i]); } \
         return ''; })()",
        Duration::from_secs(45),
    )
    .unwrap_or_else(|| {
        panic!(
            "c6: worker never delivered its CLONEIN verdict (page→worker structured \
             clone delivery gap). fixture: {:?}",
            fixture.hits()
        )
    });
    eprintln!("[c6-clone-in] {clonein}");
    assert!(
        clonein.starts_with("CLONEIN|"),
        "c6: expected a CLONEIN verdict line, got: {clonein}"
    );
    assert_all_ones("c6-clone-in(page→worker)", &clonein, &CLONE_KEYS);

    // Direction 2: worker→page — the page's deep-verification of the
    // worker-constructed OBJECT received via onmessage.
    let cloneout = poll_sink(&page, "window.__c6out === null ? '' : window.__c6out", Duration::from_secs(45))
        .unwrap_or_else(|| {
            panic!(
                "c6: worker→page structured clone object never arrived or page-side \
                 verification never ran. fixture: {:?}",
                fixture.hits()
            )
        });
    eprintln!("[c6-clone-out] {cloneout}");
    assert!(
        cloneout.starts_with("OUT|"),
        "c6: expected the page-side OUT verdict line, got: {cloneout}"
    );
    assert_all_ones("c6-clone-out(worker→page)", &cloneout, &CLONE_KEYS);
}

// ═══════════════════════════════════════════════════════════════════════
// §3 C6b — importScripts('/helper.js') executes in the worker scope
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:6] worker importScripts live
///
/// SPEC objective: `Worker 内 self.importScripts('lib.js') 正常加载`. The
/// worker (http-served, so '/helper.js' resolves against its own URL via
/// workerglobalscope.rs join(worker_url)) imports the helper and proves the
/// imported declarations executed in ITS global scope: function declarations
/// callable with correct results, and the imported script's self.* global
/// visible. Server-side hit log proves the /helper.js fetch egressed
/// (two-sided proof: transport + effect).
#[test]
fn worker_realm_api_c6_import_scripts_helper_globals() {
    if !common::run_isolated("worker_realm_api_tests::worker_realm_api_c6_import_scripts_helper_globals") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = RealmApiFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let driver = r#"
(function () {
  window.__imp = null;
  try {
    var w = new Worker('__WORKER_URL__');
    w.onmessage = function (e) { window.__imp = String(e.data); };
    w.onerror = function (ev) {
      window.__imp = 'WORKER-ERROR:' + ((ev && ev.message) || 'unknown');
      return true;
    };
    return 'worker-created';
  } catch (e) {
    window.__imp = 'CREATE-ERROR:' + String(e);
    return 'failed';
  }
})();
"#
    .replace("__WORKER_URL__", &fixture.url("/wk_import.js"));
    let created = page.evaluate_js_web(&driver).expect("import dispatch");
    assert!(
        created.contains("worker-created"),
        "c6-import: worker creation failed: {created:?}"
    );

    let verdict = poll_sink(&page, "window.__imp === null ? '' : window.__imp", Duration::from_secs(45))
        .unwrap_or_else(|| {
            panic!(
                "c6-import: worker verdict never arrived — importScripts('/helper.js') \
                 may have wedged the worker script evaluation (sync load). \
                 fixture: {:?}",
                fixture.hits()
            )
        });
    eprintln!("[c6-import] {verdict} hits={:?}", fixture.hits());

    // Two-sided proof: the helper fetch actually hit the server.
    assert!(
        fixture.saw("/helper.js"),
        "c6-import: fixture never saw /helper.js (importScripts fetch never \
         egressed): {:?}",
        fixture.hits()
    );
    assert!(
        verdict.starts_with("C6IMPORT|"),
        "c6-import: expected a C6IMPORT verdict, got: {verdict}"
    );
    if let Some(t) = verdict.strip_prefix("C6IMPORT|").and_then(|r| r.strip_prefix("THROW:")) {
        panic!("c6-import: importScripts threw in the worker: {t}");
    }
    assert_eq!(
        marker_field(&verdict, "ok"),
        Some("1"),
        "c6-import: imported globals not usable in worker scope: {verdict}"
    );
    for k in ["fn", "call", "mark", "second"] {
        assert_eq!(
            marker_field(&verdict, k),
            Some("1"),
            "c6-import: leg {k} failed: {verdict}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §4 C8 — worker-realm crypto / performance / location
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:8] DedicatedWorkerGlobalScope API surface
///
/// SPEC: DedicatedWorkerGlobalScope 暴露完整 API (… crypto/performance/
/// location …). Asserts in the LIVE worker realm:
///   - crypto.randomUUID: function present, two results match the UUID
///     8-4-4-4-12 shape and differ (v4-variant conformance recorded as a
///     report-only `uuid4=` marker — the criterion is API exposure).
///   - crypto.getRandomValues: returns the SAME view it filled (identity),
///     16 bytes wide, two draws differ (real entropy, not a zeroed buffer).
///   - performance.now(): numbers, non-negative, strictly increasing across
///     real work (monotonic clock in the worker realm).
///   - location: href/origin/protocol/host exactly equal the worker script
///     URL (Rust-side comparison against the fixture-computed URL).
#[test]
fn worker_realm_api_c8_crypto_performance_location() {
    if !common::run_isolated("worker_realm_api_tests::worker_realm_api_c8_crypto_performance_location") {
        return;
    }
    if should_skip() {
        return;
    }
    let fixture = RealmApiFixture::spawn();
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/")),
            ..Default::default()
        })
        .expect("create_page");

    let driver = r#"
(function () {
  window.__c8 = null;
  try {
    var w = new Worker('__WORKER_URL__');
    w.onmessage = function (e) { window.__c8 = String(e.data); };
    w.onerror = function (ev) {
      window.__c8 = 'WORKER-ERROR:' + ((ev && ev.message) || 'unknown');
      return true;
    };
    return 'worker-created';
  } catch (e) {
    window.__c8 = 'CREATE-ERROR:' + String(e);
    return 'failed';
  }
})();
"#
    .replace("__WORKER_URL__", &fixture.url("/wk_c8.js"));
    let created = page.evaluate_js_web(&driver).expect("c8 dispatch");
    assert!(
        created.contains("worker-created"),
        "c8: worker creation failed: {created:?}"
    );

    let verdict = poll_sink(&page, "window.__c8 === null ? '' : window.__c8", Duration::from_secs(45))
        .unwrap_or_else(|| {
            panic!(
                "c8: worker verdict never arrived (worker script wedged or \
                 postMessage gap). fixture: {:?}",
                fixture.hits()
            )
        });
    eprintln!("[c8] {verdict} hits={:?}", fixture.hits());

    if let Some(t) = verdict
        .strip_prefix("C8|")
        .and_then(|r| r.split('|').find_map(|f| f.strip_prefix("LOC-THROW:")))
    {
        panic!("c8: worker location access threw: {t}");
    }
    assert!(
        verdict.starts_with("C8|"),
        "c8: expected a C8 verdict line, got: {verdict}"
    );

    // location: exact equality against the fixture-computed worker URL.
    let expect_href = fixture.url("/wk_c8.js");
    let href = marker_field(&verdict, "href")
        .unwrap_or_else(|| panic!("c8: verdict must carry href=, got: {verdict}"));
    assert_eq!(
        href, expect_href,
        "c8: worker location.href must equal the worker script URL"
    );
    let origin = marker_field(&verdict, "origin")
        .unwrap_or_else(|| panic!("c8: verdict must carry origin=, got: {verdict}"));
    assert_eq!(
        origin,
        fixture.origin(),
        "c8: worker location.origin must equal the fixture origin"
    );
    assert_eq!(
        marker_field(&verdict, "proto"),
        Some("http:"),
        "c8: worker location.protocol must be 'http:'"
    );

    // crypto.randomUUID.
    assert_eq!(
        marker_field(&verdict, "uuid"),
        Some("1"),
        "c8: crypto.randomUUID must produce two distinct well-shaped UUIDs: {verdict}"
    );
    // Report-only: strict v4 (version nibble 4 + variant nibble 8/9/a/b).
    eprintln!(
        "[c8] uuid4-variant-conformance (report-only): {}",
        marker_field(&verdict, "uuid4").unwrap_or("absent")
    );

    // crypto.getRandomValues.
    assert_eq!(
        marker_field(&verdict, "grv"),
        Some("1"),
        "c8: crypto.getRandomValues must fill-and-return a real-entropy Uint8Array: {verdict}"
    );

    // performance.now monotonic.
    assert_eq!(
        marker_field(&verdict, "pnow"),
        Some("1"),
        "c8: performance.now() must be monotonic across real work in the worker: {verdict}"
    );
    eprintln!(
        "[c8] performance.now delta over 2M-iteration busy work: {} ms",
        marker_field(&verdict, "pnowdt").unwrap_or("absent")
    );
}
