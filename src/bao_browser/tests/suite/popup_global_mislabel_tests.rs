// @trace TEST-BRW-002-POPUP-GLOBAL-MISLABEL [req:REQ-BRW-002] [level:e2e]
// Popup double-load / shared-Window DEAD_GLOBALS mislabel E2E (REQ-BRW-002,
// e63 插针 latent face): a `window.open` popup's first REAL document reuses
// the initial about:blank document's Window object
// (`window_for_replacement` — same reflector, same JS global), and the
// about:blank pipeline's exit fires the realm-discard cancel with that
// SHARED global — marking the live realm DEAD (zombie suppression class:
// in-flight resolves suppressed at the liveness probe, armed bao timers
// purged from BAO_REGISTRY).
//
// Real-path contract under test (no mocks anywhere):
//
//   opener page JS: window.open('/popup', '', '')
//     → WindowProxy::create_auxiliary_browsing_context
//     → spawn_pipeline(initial about:blank, is_initial_about_blank=true)
//     → popup adopted into the pool; deferred injection (init_pending_pages)
//       installs bao natives (setImmediate/fetch) into the CURRENT live
//       realm = the about:blank realm (the real /popup response is still
//       in flight — the fixture delays it, making this deterministic)
//     → LoadUrl(navigate) → pipeline P1 → GET /popup (delayed response)
//     → load() → window_for_replacement: active doc is initial about:blank
//       + same origin → the NEW document REUSES the same Window/global
//     → ActivateDocument → change_session_history → close_pipeline(P0)
//     → handle_exit_pipeline_msg(P0) → realm-discard cancel with the
//       shared global  ← the mislabel under test
//     → the live /popup document's armed bao-timer chain must SURVIVE
//
// The e63 field evidence (memory b3-popup-loop-d1-d2-attribution): two GET
// /popup, load-1's per-global state killed by a pipeline exit that marked
// the shared global — the popup's in-flight fetch resolves were
// ZOMBIE-SUPPRESSED (runtime-fetch era); the timer/registry face stays
// latent on any per-popup-global storage.
//
// Observable: a setImmediate re-arming chain armed on the popup realm
// BEFORE the real document lands (BAO_REGISTRY face — exactly what the
// discard purge removes). RED before the fix: the chain freezes at the
// about:blank pipeline's exit while the /popup document stays live and
// evaluable. The fixture also counts GET /popup (the e47 double-load face
// is logged, not asserted — its fix is a different domain).

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};

const OPENER_TITLE: &str = "popup-mislabel-opener";

// ---------------------------------------------------------------------------
// Fixture — H1 keep-alive server; /g opener, /popup (delayed, gen-stamped),
// /hit marker log.
// ---------------------------------------------------------------------------

struct MislabelFixture {
    shutdown: Arc<AtomicBool>,
    hits: Arc<Mutex<Vec<String>>>,
    popup_loads: Arc<AtomicUsize>,
    port: u16,
}

impl MislabelFixture {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mislabel fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let popup_loads = Arc::new(AtomicUsize::new(0));

        let (s2, h2, p2) = (
            Arc::clone(&shutdown),
            Arc::clone(&hits),
            Arc::clone(&popup_loads),
        );
        std::thread::Builder::new()
            .name("popup-mislabel-fixture".into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking listener");
                while !s2.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            // Per-connection handler thread: /popup sleeps
                            // ~800ms before answering (deterministic
                            // injection-lands-on-about:blank window); a serial
                            // accept loop would freeze the opener's own loads.
                            let (h3, p3) = (Arc::clone(&h2), Arc::clone(&p2));
                            let _ = std::thread::Builder::new()
                                .name("popup-mislabel-conn".into())
                                .spawn(move || Self::serve_connection(tcp, h3, p3));
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn mislabel fixture");

        MislabelFixture {
            shutdown,
            hits,
            popup_loads,
            port,
        }
    }

    /// HTTP/1.1 keep-alive loop with Content-Length framing (suite fixture
    /// convention — close-delimited responses mask wire defects).
    fn serve_connection(
        mut tcp: TcpStream,
        hits: Arc<Mutex<Vec<String>>>,
        popup_loads: Arc<AtomicUsize>,
    ) {
        let _ = tcp.set_nonblocking(false);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
        let mut buf = [0u8; 8192];
        loop {
            let mut head = Vec::new();
            loop {
                match tcp.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        head.extend_from_slice(&buf[..n]);
                        if head.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    },
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock ||
                            e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        return
                    },
                    Err(_) => return,
                }
            }
            let head_str = String::from_utf8_lossy(&head).to_string();
            let request_path = head_str
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            let path = request_path.split('?').next().unwrap_or("/").to_string();

            let (status, body): (&str, Vec<u8>) = if path == "/g" {
                (
                    "200 OK",
                    format!(
                        "<!DOCTYPE html><html><head><title>{OPENER_TITLE}</title></head>\
                         <body><script>window.open('/popup', '', '');</script>\
                         <p>opener</p></body></html>"
                    )
                    .into_bytes(),
                )
            } else if path == "/popup" {
                // Deterministic ordering: hold the REAL document back long
                // enough for the pool's deferred popup injection to land on
                // the initial about:blank realm first (the test arms its
                // chain inside that window).
                std::thread::sleep(Duration::from_millis(800));
                let gen = popup_loads.fetch_add(1, Ordering::SeqCst) + 1;
                (
                    "200 OK",
                    format!(
                        "<!DOCTYPE html><html><head><title>p{gen}</title></head>\
                         <body><script>\
                         window.__gen = {gen};\
                         fetch('/hit?src=inline&gen={gen}');\
                         </script><p>popup</p></body></html>"
                    )
                    .into_bytes(),
                )
            } else if path == "/hit" {
                hits.lock().unwrap().push(request_path.clone());
                ("200 OK", b"ok".to_vec())
            } else {
                ("404 Not Found", b"".to_vec())
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n\
                 Cache-Control: no-store\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            if tcp.write_all(response.as_bytes()).is_err() {
                return;
            }
            if !body.is_empty() && tcp.write_all(&body).is_err() {
                return;
            }
            let _ = tcp.flush();
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{port}{path}", port = self.port)
    }

    fn hit_paths(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

impl Drop for MislabelFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Helpers (realm_discard_timers_tests / window_open_tests forms)
// ---------------------------------------------------------------------------

fn js(page: &bao_browser::PageHandle, expr: &str) -> String {
    page.evaluate_js_web(expr).unwrap_or_default()
}

/// Arm a perpetual bao-timer chain in the CURRENT realm (BAO_REGISTRY face —
/// the exact storage the realm-discard purge removes): each tick bumps
/// `window.__ticks` and re-arms via setImmediate. A fetch side channel
/// (`/hit?c=k`) corroborates execution.
fn arm_chain(page: &bao_browser::PageHandle) -> String {
    page.evaluate_js_web(
        "(function() { \
           window.__ticks = 0; \
           var k = 0; \
           function tick() { \
             k++; \
             window.__ticks = k; \
             fetch('/hit?c=' + k); \
             setImmediate(tick); \
           } \
           setImmediate(tick); \
           return 'armed'; \
         })()",
    )
    .unwrap_or_default()
}

/// Pump the runtime (wakes every ScriptThread; each wake runs the embedder
/// pump that fires due bao timers) while polling `cond`.
fn pump_until(
    runtime: &BrowserRuntime,
    cond: &dyn Fn() -> bool,
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        runtime.pump_cdp(Duration::from_millis(25));
    }
    cond()
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// The popup's LIVE realm (shared Window global of the initial about:blank
/// and the real /popup document) must survive the about:blank pipeline's
/// exit: armed bao timers keep ticking after the real document is active.
#[test]
fn popup_shared_global_survives_replacement_pipeline_exit() {
    bun_core::Output::init_test();
    if !common::run_isolated(
        "popup_global_mislabel_tests::popup_shared_global_survives_replacement_pipeline_exit",
    ) {
        return;
    }
    let fixture = MislabelFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool: &PagePool = runtime.page_pool();

    // 1. Opener page — its inline script opens the popup on load.
    let opener = pool
        .create_page(&PageConfig {
            url: Some(fixture.url("/g")),
            ..Default::default()
        })
        .expect("opener page");
    opener
        .wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("opener ready");
    assert!(
        pump_until(
            &runtime,
            &|| js(&opener, "document.title").contains(OPENER_TITLE),
            Duration::from_secs(15),
        ),
        "opener never loaded"
    );

    // 2. Popup adopted (delegate → pool accounting). Drain the deferred init:
    //    the /popup response is still held back by the fixture, so the
    //    injection lands on the popup's CURRENT live realm — the initial
    //    about:blank realm (whose global the real document will reuse).
    let adopted = pump_until(
        &runtime,
        &|| pool.stats().total_created >= 2,
        Duration::from_secs(20),
    );
    assert!(adopted, "popup was never adopted into the pool");
    let initialized = pool.init_pending_pages();
    assert_eq!(
        initialized, 1,
        "exactly one popup awaited deferred init (injection must land on the \
         about:blank realm BEFORE the real /popup document arrives)"
    );

    let child_id = pool
        .live_page_ids()
        .into_iter()
        .find(|id| *id != opener.id())
        .expect("popup page id in pool");
    let child = pool.get_page(child_id).expect("popup handle");

    // 3. Arm the bao-timer chain on the popup's live realm (about:blank
    //    global) while the real document is still in flight.
    let armed = arm_chain(&child);
    assert!(armed.contains("armed"), "chain dispatch failed: {armed:?}");
    let ticks_before_doc = js(&child, "window.__ticks");
    // Diagnostic baseline for the discard-cancel counter, taken BEFORE the
    // real document lands (the mislabeled exit fires inside the
    // document-arrival pump window — a later snapshot would miss it).
    let events_before = bun_runtime::timers::realm_discard_events_total();

    // 4. The real /popup document lands and REUSES the about:blank realm's
    //    Window (window_for_replacement) — same global, same chain.
    let doc_ready = pump_until(
        &runtime,
        &|| js(&child, "String(window.__gen || '')") != "",
        Duration::from_secs(20),
    );
    assert!(
        doc_ready,
        "real /popup document never landed (gen stamp missing)"
    );
    assert_eq!(
        js(&child, "String(window.__gen)"),
        "1",
        "popup document generation stamp"
    );

    // 5. The chain survives the document replacement itself (same global).
    let ticks_after_doc = js(&child, "String(window.__ticks)");
    assert_ne!(
        ticks_after_doc, "undefined",
        "chain state lost across the about:blank → /popup replacement"
    );

    // 6. Drive the ScriptThread past the about:blank pipeline's exit
    //    (ActivateDocument → change_session_history → ExitPipeline(P0) →
    //    realm-discard cancel with the SHARED global — now correctly
    //    skipped). The chain must keep ticking through it.
    let alive = pump_until(
        &runtime,
        &|| {
            let t = js(&child, "String(window.__ticks)");
            t != "undefined"
                && ticks_after_doc != "undefined"
                && t.parse::<u64>().unwrap_or(0)
                    > ticks_after_doc.parse::<u64>().unwrap_or(0)
        },
        Duration::from_secs(5),
    );
    let events_after = bun_runtime::timers::realm_discard_events_total();
    eprintln!(
        "[popup-mislabel] discard events before={events_before} after={events_after}; \
         ticks before_doc={ticks_before_doc} after_doc={ticks_after_doc}; \
         popup GETs={}",
        fixture.popup_loads.load(Ordering::SeqCst)
    );
    assert!(
        alive,
        "RED (e77): the popup realm's bao-timer chain FROZE across the \
         about:blank pipeline exit — the replacement pipeline's discard \
         marked the shared live global DEAD (zombie suppression class). \
         ticks: {ticks_after_doc} → frozen; hits: {:?}",
        fixture.hit_paths()
    );

    // 7. Corroboration: the chain kept egressing (the fetch side channel).
    let chain_hits = fixture
        .hit_paths()
        .into_iter()
        .filter(|p| p.starts_with("/hit?c="))
        .count();
    assert!(
        chain_hits >= 3,
        "chain execution stopped egressing after the replacement exit \
         (chain /hit count: {chain_hits})"
    );

    // 8. No zombie EXECUTIONS against marked realms (the purge must stop
    //    them, not run them).
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "zombie: discarded-realm bao timers EXECUTED"
    );

    pool.close_page(child_id).expect("close popup");
    pool.close_page(opener.id()).expect("close opener");
}
