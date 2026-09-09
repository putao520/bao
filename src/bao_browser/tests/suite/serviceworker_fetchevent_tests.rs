// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:19] [level:integration]
// ServiceWorker FetchEvent pipeline live tests — REQ-BRW-004 C19 S2a.
//
// Verifies, on the live servo path (real SW registration → real
// ServiceWorkerGlobalScope thread):
//   1. `navigator.serviceWorker.register()` drives the upstream register job
//      through install/activate and spawns the SW scope.
//   2. The SW realm exposes the FetchEvent surface added by the S2a vendor
//      patch (vendor/servo/components/script/dom/serviceworker/fetchevent.rs):
//        - `FetchEvent` constructor object present on the global,
//        - `new FetchEvent('fetch', {request})` → `instanceof FetchEvent`,
//        - `event.request instanceof Request` (the mediated Request),
//        - `typeof event.respondWith === 'function'`,
//        - `typeof event.waitUntil === 'function'` (ExtendableEvent surface),
//        - `respondWith` twice rejects with InvalidStateError,
//        - `self.addEventListener('fetch', …)` + `self.onfetch` wiring works
//          and listeners fire with a correctly-shaped event on dispatch.
//
// Result transport: the SW probe publishes its JSON verdict by `fetch()`-ing
// `/sw-probe?result=…` on the test's own HTTP fixture (no SW→page channel
// exists yet in servo — clients.postMessage surface is still commented
// upstream). The response side of the mediator channel (respondWith settle →
// CustomResponse → response_chan.send) is wired by S2a but is only LIVE once
// S2b produces mediators from the net layer (upstream never constructs a
// CustomResponseMediator — e33 recon §③); it is compile- and
// registration-verified here, live-fire deferred to S2b.
//
// Environment gating (same as worker_fingerprint_consistency_tests):
// real servo rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(serviceworker_fetchevent)'

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

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

/// The ServiceWorker script the test registers. Runs at SW scope init (after
/// `define_all_exposed_interfaces`), introspects the FetchEvent surface, then
/// publishes the verdict through the fixture so the test can assert on it.
/// Absolute publish URLs (`__ORIGIN__` templated by the test). Primary publish
/// channel is SYNCHRONOUS XHR: it has no promise/event-loop dependency, and if
/// it fails the failure reason travels on the next sync XHR — nothing is
/// swallowed. `typeof fetch`/`typeof XMLHttpRequest` are part of the verdict so
/// a missing network surface is visible even when every egress channel is dead.
const SW_PROBE_JS: &str = r#"
function __sync(path) {
  try {
    var x = new XMLHttpRequest();
    x.open('GET', path, false);
    x.send(null);
    return 'status=' + x.status;
  } catch (e) {
    return 'xhr-threw:' + e;
  }
}
var __result = { href: null };
try { __result.href = String(self.location.href); } catch (eL) { __result.href = 'no-location:' + eL; }
__result.fetchType = typeof fetch;
__result.xhrType = typeof XMLHttpRequest;
try {
  __result.ctorType = typeof FetchEvent;
  __result.requestCtor = typeof Request;
  __result.responseCtor = typeof Response;
  var __req = new Request('http://127.0.0.1:1/probe-request');
  var __ev = new FetchEvent('fetch', { request: __req });
  __result.instanceOf = __ev instanceof FetchEvent;
  __result.requestIsRequest = __ev.request instanceof Request;
  __result.respondWithIsFn = typeof __ev.respondWith === 'function';
  __result.waitUntilIsFn = typeof __ev.waitUntil === 'function';
  __result.requestUrl = __ev.request.url;
  // respondWith must reject a second registration on the same event.
  try {
    __ev.respondWith(Promise.resolve(new Response('x')));
    __ev.respondWith(Promise.resolve(new Response('y')));
    __result.doubleRespondWith = 'accepted-twice';
  } catch (e2) {
    __result.doubleRespondWith = (e2 && e2.name) ? e2.name : String(e2);
  }
  // Listener + onfetch wiring; dispatchEvent runs them synchronously.
  self.__listenerSeen = null;
  self.__onfetchSeen = false;
  self.addEventListener('fetch', function (e) {
    self.__listenerSeen = {
      instanceOf: e instanceof FetchEvent,
      requestIsRequest: e.request instanceof Request,
      respondWithIsFn: typeof e.respondWith === 'function'
    };
  });
  self.onfetch = function (e) { self.__onfetchSeen = true; };
  __result.onfetchSet = typeof self.onfetch === 'function';
  var __ev2 = new FetchEvent('fetch', {
    request: new Request('http://127.0.0.1:1/dispatched')
  });
  self.dispatchEvent(__ev2);
  __result.listenerFired = self.__listenerSeen !== null;
  __result.listenerShape = self.__listenerSeen;
  __result.onfetchFired = self.__onfetchSeen === true;
} catch (err) {
  __result.error = String(err);
}
var __pub = __sync('__ORIGIN__sw-probe?result=' + encodeURIComponent(JSON.stringify(__result)));
__sync('__ORIGIN__sw-publish-check?' + encodeURIComponent(__pub));
if (typeof fetch === 'function') {
  fetch('__ORIGIN__sw-heartbeat-async').then(function () {}, function () {});
}
"#;

/// Minimal multi-path HTTP fixture: `/` → page HTML, `/sw.js` → SW script,
/// anything else recorded (the probe publishes via `/sw-probe?result=…`,
/// heartbeats via `/sw-heartbeat`, failures via `/sw-probe-error`).
struct SwHttpFixture {
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    sw_script: Arc<Mutex<Option<String>>>,
    port: u16,
}

impl SwHttpFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sw fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sw_script: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let script_c = Arc::clone(&sw_script);
        std::thread::Builder::new()
            .name("sw-fixture".into())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 2048];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none() &&
                                Instant::now() < deadline
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
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            paths_c.lock().unwrap().push(path.clone());
                            let sw_script_body = script_c.lock().unwrap().clone();
                            let (content_type, body): (&str, String) =
                                if path.starts_with("/sw.js") {
                                    match sw_script_body {
                                        Some(script) => ("application/javascript", script),
                                        None => (
                                            "text/plain",
                                            "sw fixture script not set".into(),
                                        ),
                                    }
                                } else {
                                    (
                                        "text/html",
                                        "<html><body>sw-fetch-event fixture</body></html>".into(),
                                    )
                                };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
                                ct = content_type,
                                len = body.len()
                            );
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn sw fixture thread");
        SwHttpFixture {
            shutdown,
            paths,
            sw_script,
            port,
        }
    }

    fn set_script(&self, script: String) {
        *self.sw_script.lock().unwrap() = Some(script);
    }

    fn recorded_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }

    /// Returns the decoded `result` JSON of the first `/sw-probe` request.
    fn probe_result(&self) -> Option<String> {
        self.recorded_paths()
            .into_iter()
            .find(|p| p.starts_with("/sw-probe?result="))
            .map(|p| {
                let raw = p.trim_start_matches("/sw-probe?result=");
                urldecode(raw)
            })
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    },
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    },
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            },
            b => {
                out.push(b);
                i += 1;
            },
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Drop for SwHttpFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn wait_for<F: Fn() -> Option<T>, T>(mut poll: F, timeout: Duration, what: &str) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(v) = poll() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    eprintln!("[timeout] {what}");
    None
}

/// @trace REQ-BRW-004 [criterion:19] SW realm exposes the FetchEvent pipeline (live)
///
/// Registers a real service worker against the fixture origin, waits for the
/// SW scope to come up, and asserts the FetchEvent surface from INSIDE the SW
/// realm (the probe publishes its JSON verdict through the fixture).
///
/// Un-IGNORED (2026-09-09, C19 close-out): the publish channel is live — the
/// upstream `WorkerGlobalScope::new_script_pair` TODO (which panicked with
/// "need to implement a sender for ServiceWorker" the moment the probe's sync
/// XHR ran) now has the missing third arm: BAO PATCH adds
/// `ServiceWorkerGlobalScope::new_script_pair` (serviceworkerglobalscope.rs)
/// + `ScriptEventLoopReceiver::ServiceWorker` (messaging.rs) + the
/// `downcast::<ServiceWorkerGlobalScope>()` arm (workerglobalscope.rs), all
/// mechanical mirrors of the existing Dedicated/Shared arms. The respondWith
/// settle side stays compile-verified only until S2b produces mediators from
/// the net layer (see the header note above).
#[test]
fn c19_sw_realm_exposes_fetchevent_pipeline_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = SwHttpFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    // The script is templated with the origin AFTER the port is known; the
    // accept thread picks it up before any request can arrive (the page that
    // triggers /sw.js is only created below).
    fixture.set_script(SW_PROBE_JS.replace("__ORIGIN__", &origin));

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");

    // Register the SW from page JS and record the promise outcome.
    let register_js = format!(
        "window.__swReg = 'pending'; \
         try {{ \
           navigator.serviceWorker.register('/sw.js').then( \
             function () {{ window.__swReg = 'ok'; }}, \
             function (e) {{ window.__swReg = 'error:' + e; }}); \
         }} catch (err) {{ window.__swReg = 'threw:' + err; }} \
         window.__swReg"
    );
    let initial = page
        .evaluate_js_web(&register_js)
        .expect("register dispatch must not fail");
    eprintln!("[sw-test] register dispatched, immediate eval = {initial:?}");

    let reg = wait_for(
        || {
            match page.evaluate_js_web("window.__swReg") {
                Ok(s) if s.contains("pending") => None,
                Ok(s) => Some(s),
                Err(_) => None,
            }
        },
        Duration::from_secs(20),
        "serviceWorker.register promise settlement",
    );
    let reg = reg.unwrap_or_else(|| "<no settlement>".to_string());
    eprintln!("[sw-test] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "serviceWorker.register must resolve on the live path, got: {reg}"
    );

    // The SW probe publishes its verdict through the fixture.
    let verdict = wait_for(
        || fixture.probe_result(),
        Duration::from_secs(25),
        "SW probe publish (/sw-probe?result=…)",
    )
    .unwrap_or_else(|| {
        let seen = fixture.recorded_paths();
        let errors: Vec<String> = seen
            .iter()
            .filter(|p| p.starts_with("/sw-probe-error"))
            .cloned()
            .collect();
        panic!(
            "SW probe verdict never arrived — SW scope did not start or its fetch() \
             publish failed. Recorded fixture paths: {seen:?}; error-channel reports: \
             {errors:?}"
        )
    });
    eprintln!("[sw-test] probe verdict = {verdict}");

    // Parse the flat JSON with string matching (fields are booleans/strings
    // with stable shapes; no serde dependency in this suite).
    assert!(
        verdict.contains("\"error\"") == false,
        "SW probe reported a JS error: {verdict}"
    );
    assert!(
        verdict.contains("\"ctorType\":\"function\""),
        "FetchEvent constructor must be exposed on the SW global: {verdict}"
    );
    assert!(
        verdict.contains("\"requestCtor\":\"function\""),
        "Request must be reachable in the SW realm: {verdict}"
    );
    assert!(
        verdict.contains("\"responseCtor\":\"function\""),
        "Response must be reachable in the SW realm: {verdict}"
    );
    assert!(
        verdict.contains("\"instanceOf\":true"),
        "constructed event must satisfy `instanceof FetchEvent`: {verdict}"
    );
    assert!(
        verdict.contains("\"requestIsRequest\":true"),
        "event.request must satisfy `instanceof Request`: {verdict}"
    );
    assert!(
        verdict.contains("\"respondWithIsFn\":true"),
        "respondWith must be a function: {verdict}"
    );
    assert!(
        verdict.contains("\"waitUntilIsFn\":true"),
        "waitUntil (ExtendableEvent surface) must be a function: {verdict}"
    );
    assert!(
        verdict.contains("\"requestUrl\":\"http://127.0.0.1:1/probe-request\""),
        "event.request.url must carry the constructed request URL: {verdict}"
    );
    assert!(
        verdict.contains("\"doubleRespondWith\":\"InvalidStateError\""),
        "second respondWith call must reject with InvalidStateError: {verdict}"
    );
    assert!(
        verdict.contains("\"onfetchSet\":true"),
        "self.onfetch must be settable: {verdict}"
    );
    assert!(
        verdict.contains("\"listenerFired\":true"),
        "dispatchEvent must run the fetch listener: {verdict}"
    );
    assert!(
        verdict.contains("\"instanceOf\":true"),
        "listener event shape must carry instanceof FetchEvent: {verdict}"
    );
    assert!(
        verdict.contains("\"respondWithIsFn\":true"),
        "listener event must expose respondWith: {verdict}"
    );
    assert!(
        verdict.contains("\"onfetchFired\":true"),
        "onfetch handler must fire on dispatch: {verdict}"
    );
}
