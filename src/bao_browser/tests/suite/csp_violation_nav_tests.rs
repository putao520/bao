// @trace TEST-BRW-001 [req:REQ-BRW-001] [sm:PageLifecycle] [level:e2e]
// Regression lock (2026-10-04, WPT fetch/metadata/report.https.sub.html):
// a CSP-violating stylesheet subresource during navigation killed the WHOLE
// PROCESS (SIGSEGV, wptrunner saw RemoteDisconnected ~0.3s after the
// navigation POST). Root cause (core dump /tmp/bao-core.2169694.*):
//
//   JS::DescribeScriptedCaller (jsapi.cpp:4952 `if (!cx->compartment())`)
//   read `realm_` at wrapper+0xb0 == 0xffffffffffffffff → fault at -1.
//
// vendor/servo components/script/dom/security/csp.rs
// `compute_scripted_caller_source_position` called the DEPRECATED raw
// `describe_scripted_caller(&*cx as *const _ as *mut _)` — the cast chain
// passes the ADDRESS OF THE RUST `js::context::JSContext` WRAPPER object
// (8-byte `{ ptr: NonNull<RawJSContext>, no_gc }`), not the raw SpiderMonkey
// JSContext. `DescribeScriptedCaller` then read `realm_` (JSContext+0xb0)
// out of adjacent stack memory. Fix = `describe_scripted_caller_safe(cx)`
// (upstream servo form, same as console.rs's W12-B fix).
//
// Carrier here mirrors the WPT page's mechanism (IDN/https was incidental):
// a document served with `Content-Security-Policy: style-src 'self'` plus a
// cross-origin stylesheet link → net fetch reports style-src violations →
// `report_csp_violations(cx, violations, None, None)` → the CSP report task
// runs `compute_scripted_caller_source_position`. Pre-fix that SIGSEGVs the
// process the moment the violation report task runs; post-fix the page stays
// interactive and the securitypolicyviolation event is observable.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PageState};

/// servo/BrowserRuntime carry process-global slots (one JSContext per thread;
/// embedder state) — serialize runtime users in this suite (see
/// pagestate_lifecycle_tests RUNTIME_LOCK).
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One minimal HTTP server bound to an ephemeral 127.0.0.1 port. `extra_head`
/// is emitted verbatim as extra response headers (the CSP header carrier);
/// `body` is the fixed response payload.
struct StaticServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl StaticServer {
    fn spawn(name: &'static str, extra_head: &str, content_type: &str, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        let head = format!("{extra_head}Content-Type: {content_type}\r\nConnection: close\r\n");
        std::thread::Builder::new()
            .name(format!("{name}").into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let mut req = [0u8; 2048];
                            let _ = tcp.read(&mut req);
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\n{head}Content-Length: {}\r\n\r\n",
                                body.len()
                            );
                            let _ = tcp.write_all(resp.as_bytes());
                            let _ = tcp.write_all(body.as_bytes());
                            let _ = tcp.flush();
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn fixture");
        StaticServer { port, shutdown }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Pump the page's callback drain until `cond` holds or the deadline expires.
fn poll_until(page: &PageHandle, timeout: Duration, cond: &dyn Fn() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js_web(""); // pump the callback drain
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// The CSP report task must not kill the process. Pre-fix this test died with
/// SIGSEGV inside `JS::DescribeScriptedCaller` ~0.3s after the stylesheet
/// fetch completed (the whole test process vanished — nextest reports a crash,
/// not an assertion failure).
#[test]
fn csp_violating_stylesheet_subresource_does_not_kill_the_process() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Cross-origin style source: a different 127.0.0.1 port is a different
    // origin, so `style-src 'self'` blocks it and reports a violation.
    let stylesheet = StaticServer::spawn(
        "csp-css-fixture",
        "",
        "text/css",
        ".blocked { color: red; }".to_string(),
    );
    let page_body = format!(
        "<!DOCTYPE html><html><head>\
         <title>csp-violation-nav</title>\
         <script>window.__violations = 0;\
           document.addEventListener('securitypolicyviolation', function () {{\
             window.__violations++;\
           }});</script>\
         <link rel=\"stylesheet\" href=\"{css_url}blocked.css\">\
         </head><body><p>csp nav fixture</p></body></html>",
        css_url = stylesheet.url(),
    );
    // style-src 'self' (inline <script> is script-src territory, untouched).
    let page_server = StaticServer::spawn(
        "csp-page-fixture",
        "Content-Security-Policy: style-src 'self'\r\n",
        "text/html",
        page_body,
    );

    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("create_page");

    page.navigate(&page_server.url()).expect("navigate");

    // The crash window is the CSP violation report task, queued right after
    // the stylesheet fetch completes — survive it and keep driving the page.
    let interactive = poll_until(&page, Duration::from_secs(30), &|| {
        page.get_state() == PageState::Interactive
    });
    let title_ok = poll_until(&page, Duration::from_secs(10), &|| {
        matches!(page.evaluate_js_web("document.title"), Ok(t) if t.contains("csp-violation-nav"))
    });
    assert!(
        interactive,
        "page must reach Interactive across the CSP violation report task (pre-fix: process SIGSEGV)"
    );
    assert!(title_ok, "document.title never became readable after the CSP violation");

    // The violation must actually have been reported into the page — this is
    // the proof the fixed code path (compute_scripted_caller_source_position
    // → describe_scripted_caller_safe) executed, not that the fetch silently
    // never happened.
    let count = |page: &PageHandle| -> u32 {
        page.evaluate_js_web("String(window.__violations)")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let violations = poll_until(&page, Duration::from_secs(10), &|| count(&page) >= 1);
    assert!(
        violations,
        "expected ≥1 securitypolicyviolation for the cross-origin stylesheet (last: {})",
        count(&page)
    );
}

/// Negative control for the carrier above: the SAME page without the CSP
/// header must NOT produce a securitypolicyviolation (proves the violation in
/// the positive test comes from the style-src policy, i.e. the report task
/// path, and not from some unrelated stylesheet failure).
#[test]
fn same_page_without_csp_header_reports_no_violation() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let stylesheet = StaticServer::spawn(
        "nocsp-css-fixture",
        "",
        "text/css",
        ".allowed { color: green; }".to_string(),
    );
    let page_body = format!(
        "<!DOCTYPE html><html><head><title>nocsp-nav</title>\
         <script>window.__violations = 0;\
           document.addEventListener('securitypolicyviolation', function () {{\
             window.__violations++;\
           }});</script>\
         <link rel=\"stylesheet\" href=\"{css_url}allowed.css\">\
         </head><body><p>no csp fixture</p></body></html>",
        css_url = stylesheet.url(),
    );
    let page_server = StaticServer::spawn(
        "nocsp-page-fixture",
        "",
        "text/html",
        page_body,
    );

    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .expect("create_page");
    page.navigate(&page_server.url()).expect("navigate");

    poll_until(&page, Duration::from_secs(30), &|| {
        page.get_state() == PageState::Interactive
    });
    poll_until(&page, Duration::from_secs(10), &|| {
        matches!(page.evaluate_js_web("document.title"), Ok(t) if t.contains("nocsp-nav"))
    });
    // Give any (wrong) violation task a bounded window to surface, then
    // require the counter to still be zero.
    std::thread::sleep(Duration::from_secs(2));
    let _ = page.evaluate_js_web("");
    let count: u32 = page
        .evaluate_js_web("String(window.__violations)")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    assert_eq!(
        count, 0,
        "no CSP header → no securitypolicyviolation expected"
    );
}
