// @trace TEST-BRW-004 [req:REQ-BRW-004,REQ-BRW-4] [criterion:19] [level:integration]
// ServiceWorker navigator.serviceWorker.controller assignment live tests —
// REQ-BRW-004 C19 controller wave.
//
// Verifies, on the live servo path (real registration → real activation →
// real page-realm attribute read), the controller assignment chain added by
// the controller-wave vendor patch:
//   - serviceworkercontainer.rs: GetController returns the (previously
//     hardcoded-None) `controller` field; refresh_controller stores the
//     page-side ServiceWorker object whenever a registration answer (register
//     resolve or getRegistration match) carries an active worker whose scope
//     prefixes the page URL.
//   - serviceworker_manager.rs: install()'s Resolve Job Promise moved after
//     the waiting→active transitions so the resolved info carries
//     active_worker, making the register resolve the activation notification.
//
// Probes:
//   1. NULL-BY-DEFAULT: a fresh page with no registration for its origin
//      reads `navigator.serviceWorker.controller === null`.
//   2. ACTIVATED-ASSIGNMENT: after register('/sw.js') settles, the same page
//      (in scope) reads a controller that is `instanceof ServiceWorker` with
//      `scriptURL` = the SW script URL.
//   3. NO-SW-PAGE: a page on a different origin (no registration) still
//      reads `controller === null`.
//   4. FOLLOWS-NEW-ACTIVE: after unregister + register('/sw2.js') (a fresh
//      registration with a new worker id — the only active-worker change the
//      current upstream job state machine produces), controller follows to
//      the new worker's scriptURL.
//
// Verdict transport: window globals + external polling (same vehicle as
// serviceworker_mediation_tests; the controller attribute is read
// synchronously in-page, so no XHR round-trip is needed here).
//
// Known boundary (deliberately minimal subset, per the wave contract): only
// the container that registered or queried getRegistration is refreshed —
// the manager keeps a single client callback per registration, and
// navigation SW-ification is not implemented upstream, so a same-scope page
// that never touches the SW API keeps controller === null. Probe 3 pins the
// null side of that contract; it is not a regression of this patch.
//
// Environment gating (same as serviceworker_mediation_tests):
// real servo rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(serviceworker_controller)'

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

/// Trivial SW scripts — controller semantics need no fetch listener. The two
/// bodies exist so probe 4 can distinguish which worker is active by
/// scriptURL alone.
const SW_SCRIPT_V1: &str = "var swControllerTag = 'v1';";
const SW_SCRIPT_V2: &str = "var swControllerTag = 'v2';";

/// Minimal HTTP fixture: `/` → page HTML, `/sw.js` → v1 script, `/sw2.js` →
/// v2 script. Same shape as the mediation fixture, minus the API probes.
struct SwControllerFixture {
    shutdown: Arc<AtomicBool>,
    port: u16,
}

impl SwControllerFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sw controller fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("sw-controller-fixture".into())
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
                            let (content_type, body): (&str, String) = if path.starts_with("/sw.js") {
                                ("application/javascript", SW_SCRIPT_V1.into())
                            } else if path.starts_with("/sw2.js") {
                                ("application/javascript", SW_SCRIPT_V2.into())
                            } else {
                                (
                                    "text/html",
                                    "<html><body>sw-controller fixture</body></html>".into(),
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
            .expect("spawn sw controller fixture thread");
        SwControllerFixture { shutdown, port }
    }
}

impl Drop for SwControllerFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn wait_for<F: Fn() -> Option<T>, T>(poll: F, timeout: Duration, what: &str) -> Option<T> {
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

/// Page-realm controller probe: reads the attribute synchronously and packs
/// the verdict. 'null' while unassigned; once assigned
/// '<instanceof> <typeof-if> <scriptURL>'.
fn controller_probe_js(out_var: &str) -> String {
    format!(
        "window.{out_var} = (function () {{ \
           try {{ \
             var c = navigator.serviceWorker.controller; \
             if (!c) {{ return 'null'; }} \
             return String(c instanceof ServiceWorker) + \
               ' if=' + (typeof ServiceWorker !== 'undefined') + \
               ' url=' + c.scriptURL + \
               ' state=' + c.state; \
           }} catch (e) {{ return 'THREW:' + e; }} \
         }})(); window.{out_var}"
    )
}

/// Dispatch `navigator.serviceWorker.register(path)` recording the promise
/// outcome in `window.__swReg` (same shape as the mediation test).
fn register_dispatch_js(path: &str) -> String {
    format!(
        "window.__swReg = 'pending'; \
         try {{ \
           navigator.serviceWorker.register('{path}').then( \
             function () {{ window.__swReg = 'ok'; }}, \
             function (e) {{ window.__swReg = 'error:' + e; }}); \
         }} catch (err) {{ window.__swReg = 'threw:' + err; }} \
         window.__swReg"
    )
}

fn wait_register_ok(page: &bao_browser::PageHandle, what: &str) {
    let outcome = wait_for(
        || match page.evaluate_js_web("window.__swReg") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        },
        Duration::from_secs(20),
        what,
    )
    .unwrap_or_else(|| "<no settlement>".to_string());
    eprintln!("[sw-controller] {what} = {outcome}");
    assert!(
        outcome.contains("ok"),
        "{what} must resolve ok on the live path, got: {outcome}"
    );
}

/// @trace REQ-BRW-004 [criterion:19] navigator.serviceWorker.controller
/// assignment end to end (live)
///
/// After SW activation the same-scope page's navigator.serviceWorker.controller
/// is a ServiceWorker instance whose scriptURL is the SW script URL; pages
/// without a registration read null; the controller follows the new active
/// worker across an unregister + re-register replacement.
#[test]
fn c19_sw_controller_assignment_live() {
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    let fixture = SwControllerFixture::spawn();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");

    // ① NULL-BY-DEFAULT — fresh page, no registration for this origin yet.
    let pre = page
        .evaluate_js_web(&controller_probe_js("__c0"))
        .expect("controller probe must not fail");
    eprintln!("[sw-controller] ① pre-register = {pre:?}");
    assert!(pre.contains("null"), "① pre-register controller must be null: {pre}");

    // ② ACTIVATED-ASSIGNMENT — register, then the controller must be a
    // ServiceWorker for /sw.js. The manager resolves the register job after
    // the waiting→active transitions, and refresh_controller assigns the
    // controller in the same task that settles the promise, so by the time
    // __swReg flips to 'ok' the attribute is already assigned; the retry
    // loop is insurance for task-ordering variance.
    let initial = page
        .evaluate_js_web(&register_dispatch_js("/sw.js"))
        .expect("register dispatch must not fail");
    eprintln!("[sw-controller] register dispatched, immediate eval = {initial:?}");
    wait_register_ok(&page, "serviceWorker.register('/sw.js') settlement");

    let assigned = wait_for(
        || match page.evaluate_js_web(&controller_probe_js("__c1")) {
            Ok(s) if s.contains("null") || s.contains("THREW") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        },
        Duration::from_secs(20),
        "controller assignment after activation",
    )
    .unwrap_or_else(|| "<never assigned>".to_string());
    eprintln!("[sw-controller] ② assigned = {assigned:?}");
    assert!(
        assigned.starts_with("true "),
        "② controller must be instanceof ServiceWorker: {assigned}"
    );
    assert!(
        assigned.contains("url=http://") && assigned.contains("/sw.js"),
        "② controller.scriptURL must be the SW script URL: {assigned}"
    );

    // ③ NO-SW-PAGE — a page on a different origin (fresh fixture, no
    // registration) reads null.
    let fixture2 = SwControllerFixture::spawn();
    let origin2 = format!("http://127.0.0.1:{}/", fixture2.port);
    let page2 = runtime
        .create_page(&PageConfig {
            url: Some(origin2),
            ..Default::default()
        })
        .expect("gated live test: create_page 2 must succeed");
    let uncontrolled = page2
        .evaluate_js_web(&controller_probe_js("__c2"))
        .expect("controller probe 2 must not fail");
    eprintln!("[sw-controller] ③ no-SW page = {uncontrolled:?}");
    assert!(
        uncontrolled.contains("null"),
        "③ page without a registration must read controller === null: {uncontrolled}"
    );

    // ④ FOLLOWS-NEW-ACTIVE — unregister (the manager drops the registration,
    // terminating the v1 worker thread before resolving), then register
    // /sw2.js: a fresh registration whose Try Activate promotes a new worker
    // (new ServiceWorkerId). The controller must follow to the new
    // worker's scriptURL.
    let unregister_js = "window.__swUn = 'pending'; \
         navigator.serviceWorker.getRegistration().then( \
           function (r) { \
             if (!r) { window.__swUn = 'noreg'; return; } \
             r.unregister().then( \
               function (v) { window.__swUn = 'un:' + v; }, \
               function (e) { window.__swUn = 'err:' + e; }); \
           }, \
           function (e) { window.__swUn = 'err:' + e; }); \
         window.__swUn";
    let un_outcome = {
        let initial = page
            .evaluate_js_web(unregister_js)
            .expect("unregister dispatch must not fail");
        eprintln!("[sw-controller] unregister dispatched, immediate eval = {initial:?}");
        // Poll READ-ONLY: the dispatch JS above resets __swUn on every run,
        // so re-evaluating it would never observe the settlement (the .then
        // callbacks only run between evaluate calls).
        wait_for(
            || match page.evaluate_js_web("window.__swUn") {
                Ok(s) if s.contains("pending") => None,
                Ok(s) => Some(s),
                Err(_) => None,
            },
            Duration::from_secs(20),
            "getRegistration + unregister settlement",
        )
    }
    .unwrap_or_else(|| "<no settlement>".to_string());
    eprintln!("[sw-controller] ④ unregister = {un_outcome:?}");
    assert!(
        un_outcome.contains("un:true"),
        "④ unregister must resolve true, got: {un_outcome}"
    );

    let initial2 = page
        .evaluate_js_web(&register_dispatch_js("/sw2.js"))
        .expect("register 2 dispatch must not fail");
    eprintln!("[sw-controller] register 2 dispatched, immediate eval = {initial2:?}");
    wait_register_ok(&page, "serviceWorker.register('/sw2.js') settlement");

    let followed = wait_for(
        || match page.evaluate_js_web(&controller_probe_js("__c3")) {
            Ok(s) if s.contains("/sw2.js") => Some(s),
            Ok(_) => None,
            Err(_) => None,
        },
        Duration::from_secs(20),
        "controller following the new active worker",
    )
    .unwrap_or_else(|| "<never followed>".to_string());
    eprintln!("[sw-controller] ④ followed = {followed:?}");
    assert!(
        followed.starts_with("true "),
        "④ controller must remain instanceof ServiceWorker: {followed}"
    );
    assert!(
        followed.contains("/sw2.js"),
        "④ controller must follow the new active worker's scriptURL: {followed}"
    );
}
