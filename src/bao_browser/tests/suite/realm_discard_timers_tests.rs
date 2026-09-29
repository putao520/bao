// RED-1 P-A (user ruling 2026-09-10) — same-registered-domain navigation
// discards the old page realm on the REUSED ScriptThread (constellation
// event-loop reuse), and the discarded realm's bao timers must die with
// the realm (browser navigation semantics: a document's timers are
// destroyed on navigation, Chrome non-bfcache path).
//
// Before the realm-discard cancel bridge (vendor patch:
// `Window::clear_js_runtime` → `register_bao_realm_discard_cancel` →
// `bun_runtime::timers::cancel_timers_for_global`), the old realm's
// BAO_REGISTRY entries survived the discard: every deadline fired a
// zombie callback into the `WindowState::Zombie` realm, a re-arming
// setImmediate chain ran forever (its fetch()s kept egressing — the
// observable these tests pin), and the raw-rooted `global_root` pinned
// the old realm against GC (per-navigation accumulation, #29 input).
//
// Arming path: page realms keep servo's WebIDL setTimeout/setInterval
// (BCE page-realm timer shadowing) — the BAO_REGISTRY face installed per
// page realm is `setImmediate` (install_timer_globals) plus the bao
// `fetch` override (the zombie's observable side channel). The chain
// re-arms a setImmediate each tick, which is the exact "interval
// equivalent" that used to survive the discard.
//
// Axes:
//   same-domain nav  — armed chain stops at nav + registry purge probe
//                      (realm_discard_cancelled_total) + churn N-cycle
//                      zero accumulation.
//   cross-host nav   — different registered domain (localhost vs
//                      127.0.0.1): existing path zero regression.
//   page close       — existing discard-随线程 path zero regression.

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};

const TITLE_A: &str = "realm-discard-a";
const TITLE_B: &str = "realm-discard-b";

// ---------------------------------------------------------------------------
// Minimal H1 fixture: /a, /b pages + /hit hit-log (server-side zombie
// observable — a zombie realm's fetch still egresses and lands here).
// ---------------------------------------------------------------------------

struct DiscardFixture {
    shutdown: Arc<AtomicBool>,
    hits: Arc<Mutex<Vec<(String, Instant)>>>,
    port: u16,
}

fn page_body(title: &str, marker: &str) -> Vec<u8> {
    format!(
        "<!DOCTYPE html><html><head><title>{title}</title></head>\
         <body><p id=\"marker\">{marker}</p></body></html>"
    )
    .into_bytes()
}

impl DiscardFixture {
    fn spawn() -> Self {
        let body_a = Arc::new(page_body(TITLE_A, "a"));
        let body_b = Arc::new(page_body(TITLE_B, "b"));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind discard fixture");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<(String, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let (s2, h2) = (Arc::clone(&shutdown), Arc::clone(&hits));
        std::thread::Builder::new()
            .name("realm-discard-fixture".into())
            .spawn(move || {
                while !s2.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            // Per-connection handler thread: the chain test's
                            // fetches arrive in bursts; a serial accept loop
                            // with per-conn read deadlines would serialize
                            // acceptances and smear pre-cancel hits into the
                            // post-nav observation window (fixture lag, not
                            // product behavior).
                            let (h3, ba, bb) =
                                (Arc::clone(&h2), Arc::clone(&body_a), Arc::clone(&body_b));
                            let _ = std::thread::Builder::new()
                                .name("discard-fixture-conn".into())
                                .spawn(move || {
                                    let mut tcp = tcp;
                                    let _ =
                                        tcp.set_read_timeout(Some(Duration::from_millis(500)));
                                    let mut buf = Vec::new();
                                    let mut tmp = [0u8; 2048];
                                    let deadline = Instant::now() + Duration::from_millis(500);
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
                                    let (ct, body) = if path.starts_with("/hit") {
                                        // Record at REQUEST time — the observable.
                                        h3.lock().unwrap().push((path, Instant::now()));
                                        ("text/plain", Vec::new())
                                    } else if path.starts_with("/hang") {
                                        // ISSUE #25: slow completion face — the
                                        // response lands AFTER the discard
                                        // (page close marks the realm), so any
                                        // post-discard dispatch of the fetch's
                                        // ConcurrentTask completion would be a
                                        // zombie re-entry.
                                        std::thread::sleep(Duration::from_millis(6000));
                                        ("application/json", b"{\"ok\":true}".to_vec())
                                    } else if path.starts_with("/frame") {
                                        // ISSUE #25 generalization: iframe page
                                        // whose script arms a 300s-stereo
                                        // offline render — its pipeline gets
                                        // closed (removed) while the render is
                                        // still in flight, and the settle must
                                        // be dropped at dispatch.
                                        (
                                            "text/html",
                                            b"<!doctype html><title>frame</title><script>new OfflineAudioContext(2, 44100 * 300, 44100).startRendering().then(function(){fetch('/hit?tag=AUDIO-DONE');});</script>"
                                                .to_vec(),
                                        )
                                    } else if path.starts_with("/b") {
                                        ("text/html", (*bb).clone())
                                    } else {
                                        // /a and anything else: page A
                                        ("text/html", (*ba).clone())
                                    };
                                    let resp = format!(
                                        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                        body.len()
                                    );
                                    let _ = tcp.write_all(resp.as_bytes());
                                    let _ = tcp.write_all(&body);
                                    let _ = tcp.shutdown(std::net::Shutdown::Both);
                                });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn realm-discard fixture");
        DiscardFixture {
            shutdown,
            hits,
            port,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn hit_count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }

    fn hit_paths(&self) -> Vec<String> {
        self.hits.lock().unwrap().iter().map(|(p, _)| p.clone()).collect()
    }
}

impl Drop for DiscardFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Arm a perpetual bao-timer chain in the CURRENT page realm: each tick
/// fetches /hit (server-side observable) and re-arms via setImmediate —
/// the BAO_REGISTRY face page realms carry. Each fetch carries TWO
/// counters: `k` is a closure-local invocation index (immune to the
/// post-discard window-proxy swap) — every distinct k is one real tick
/// execution; a repeated k is a transport-level re-send of one fetch, not
/// a new tick. `n` reads through `window` (swaps to NaN once the proxy
/// points at the navigated-to realm).
fn arm_chain(page: &PageHandle, tag: &str) -> String {
    page.evaluate_js_web(&format!(
        "(function() {{ \
           window.__ticks = 0; \
           var k = 0; \
           function tick() {{ \
             k++; \
             window.__ticks++; \
             fetch('/hit?tag={tag}&k=' + k + '&n=' + window.__ticks); \
             setImmediate(tick); \
           }} \
           setImmediate(tick); \
           return 'armed'; \
         }})()"
    ))
    .unwrap_or_default()
}

/// Pump the page's ScriptThread (every evaluate wakes it — and each wake
/// runs the embedder pump that fires due bao timers) until `cond` holds
/// or the deadline expires.
fn pump_until(page: &PageHandle, timeout: Duration, cond: &dyn Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js_web("");
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Wait until the page's document.title contains `title` (navigation
/// landed AND the new realm executes JS).
fn wait_title(page: &PageHandle, title: &str, timeout: Duration) -> bool {
    pump_until(page, timeout, &|| {
        matches!(page.evaluate_js_web("document.title"), Ok(t) if t.contains(title))
    })
}

// ---------------------------------------------------------------------------
// Axis 1: same-registered-domain navigation discards the old realm's bao
// timers — the RED-1 core.
// ---------------------------------------------------------------------------

#[test]
fn same_domain_nav_discards_old_realm_bao_timers() {
    if !common::run_isolated("realm_discard_timers_tests::same_domain_nav_discards_old_realm_bao_timers")
    {
        return;
    }
    let fixture = DiscardFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/a")),
            ..Default::default()
        })
        .expect("create_page");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page A ready");
    assert!(wait_title(&page, TITLE_A, Duration::from_secs(15)), "page A title");

    // Arm the chain in page A's realm (bao natives are installed into the
    // realm that exists at page creation — fetch + setImmediate).
    let armed = arm_chain(&page, "nav1");
    assert!(armed.contains("armed"), "chain dispatch failed: {armed:?}");

    // Chain alive pre-nav: ≥3 zombie-observable hits (proves the
    // BAO_REGISTRY face fires through the embedder pump).
    let alive = pump_until(&page, Duration::from_secs(15), &|| fixture.hit_count() >= 3);
    assert!(
        alive,
        "pre-nav sanity failed: armed chain never fired (hits: {:?})",
        fixture.hit_paths()
    );

    // Same-registered-domain navigation (same host, new path): the
    // constellation REUSES this ScriptThread and discards page A's realm.
    let events_before = bun_runtime::timers::realm_discard_events_total();
    page.navigate(&fixture.url("/b")).expect("navigate /b");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page B ready");
    assert!(wait_title(&page, TITLE_B, Duration::from_secs(15)), "page B title");

    // The old pipeline's exit is deferred relative to the new page's
    // readiness (bao's lazy event-loop pumping delivers it late —
    // sometimes only at teardown), so the deterministic discard trigger
    // is the page CLOSE at the end of this test. The zombie-fire assert
    // below (executions against discarded realms) holds for the whole
    // flow regardless.

    // Grace pump (~500ms): drive the ScriptThread so the ticks from the
    // nav-commit → old-pipeline-clear window (bounded, browser-like — the
    // proxy swaps at commit, the discard cancel lands at pipeline exit)
    // execute and their fetches enter the egress pipeline before the
    // observation window.
    let quiesce = Instant::now();
    while quiesce.elapsed() < Duration::from_millis(500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Pump the ScriptThread for 2s: every wake runs the embedder pump, so
    // a surviving (zombie) chain entry would fire ~10x/s. The zombie-fire
    // probe counts EXECUTIONS against the discarded global — immune to the
    // fixture/HTTP arrival lag of the pre-discard fetches (their egress
    // queue keeps landing hits for a while; that is not execution).
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(100));
    }

    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "RED-1 zombie: the discarded realm's bao timers EXECUTED after the \
         same-domain navigation (hits for context: {:?})",
        fixture.hit_paths()
    );
    // New realm healthy after the navigation.
    assert!(wait_title(&page, TITLE_B, Duration::from_secs(5)));

    // Deterministic discard trigger: closing the page forces the pipeline
    // exits (both realms of this webview) through
    // handle_exit_pipeline_msg, where the realm-discard cancel hook must
    // run for each discarded realm (the fix's plumbing; pre-fix the
    // counter never moves). The close itself wakes the ScriptThreads via
    // their message channels — no embedder pump needed.
    page.close().expect("close page");
    let close_deadline = Instant::now() + Duration::from_secs(15);
    while bun_runtime::timers::realm_discard_events_total() <= events_before
        && Instant::now() < close_deadline
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        bun_runtime::timers::realm_discard_events_total() > events_before,
        "realm-discard probe: pipeline exits at page close never reached \
         cancel_timers_for_global (events {} -> {})",
        events_before,
        bun_runtime::timers::realm_discard_events_total()
    );
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "RED-1 zombie at close: discarded realms' bao timers EXECUTED"
    );
}

// ---------------------------------------------------------------------------
// Axis 2: navigation churn — N armed pages, each discarded by its own
// same-domain navigation (then closed): every discarded realm purged, zero
// accumulation, and per-global exactness (a later page's armed chain must
// never be collateral damage of an earlier page's discard — pages may share
// a ScriptThread, where BAO_REGISTRY is shared and matching must be
// per-global).
//
// Note: bao natives (fetch/setImmediate) are installed once per page into
// its CREATION realm (consume-once embedder callback; post-nav realms keep
// servo WebIDL surfaces only), so each churn cycle arms on a fresh page's
// creation realm and then navigates it away.
// ---------------------------------------------------------------------------

#[test]
fn same_domain_nav_churn_no_timer_accumulation() {
    if !common::run_isolated("realm_discard_timers_tests::same_domain_nav_churn_no_timer_accumulation")
    {
        return;
    }
    let fixture = DiscardFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    let events_start = bun_runtime::timers::realm_discard_events_total();
    const PAGES: usize = 4;

    for i in 0..PAGES {
        let page = runtime
            .create_page(&PageConfig {
                url: Some(fixture.url(&format!("/a?c={i}"))),
                ..Default::default()
            })
            .expect("create_page");
        page.wait_for_pipeline_ready(Duration::from_secs(30))
            .expect("A ready");
        assert!(
            wait_title(&page, TITLE_A, Duration::from_secs(15)),
            "page {i}: A title"
        );

        // Arm the chain on this page's creation realm and let it fire.
        let armed = arm_chain(&page, &format!("churn{i}"));
        assert!(armed.contains("armed"), "page {i}: dispatch {armed:?}");
        let hits_at_arm = fixture.hit_count();
        let fired = pump_until(&page, Duration::from_secs(15), &|| {
            fixture.hit_count() >= hits_at_arm + 2
        });
        assert!(
            fired,
            "page {i}: chain never fired (hits {:?})",
            fixture.hit_paths()
        );

        // Same-domain nav discards this page's armed realm; its chain
        // must stop (and pages still alive keep theirs — exactness).
        page.navigate(&fixture.url(&format!("/b?c={i}")))
            .expect("navigate B");
        page.wait_for_pipeline_ready(Duration::from_secs(30))
            .expect("B ready");
        assert!(
            wait_title(&page, TITLE_B, Duration::from_secs(15)),
            "page {i}: B title"
        );
        // Grace pump (~400ms): let the nav-commit → clear window's ticks
        // execute and egress before the observation window.
        let quiesce = Instant::now();
        while quiesce.elapsed() < Duration::from_millis(400) {
            let _ = page.evaluate_js_web("");
            std::thread::sleep(Duration::from_millis(50));
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            let _ = page.evaluate_js_web("");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            bun_runtime::timers::zombie_fires_total(),
            0,
            "page {i}: zombie ticks EXECUTED after same-domain nav \
             (accumulation face; hits for context: {:?})",
            fixture.hit_paths()
        );
        page.close().expect("close page");
    }

    // Every discarded realm was carried through the cancel hook — zero
    // accumulation across the whole churn loop (the armed chain died with
    // its realm on every cycle; zombie_fires==0 above is the behavioral
    // proof per cycle). The last page's discard can lag the loop's end;
    // pump for it (bounded).
    let drained = {
        let last = runtime
            .create_page(&PageConfig {
                url: Some(fixture.url("/a?c=final")),
                ..Default::default()
            })
            .expect("create final probe page");
        let ok = pump_until(&last, Duration::from_secs(10), &|| {
            bun_runtime::timers::realm_discard_events_total() - events_start >= PAGES
        });
        let _ = last.close();
        ok
    };
    let events = bun_runtime::timers::realm_discard_events_total() - events_start;
    assert!(
        drained && events >= PAGES,
        "churn discard probe: {PAGES} same-domain navigations must deliver ≥ \
         {PAGES} realm-discard notifications (events {events})"
    );
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "churn: zombie ticks EXECUTED in some discarded realm (hits: {:?})",
        fixture.hit_paths()
    );
}

// ---------------------------------------------------------------------------
// Axis 3: existing discard paths zero regression — cross-host navigation
// (different registered domain → different ScriptThread) and page close.
// ---------------------------------------------------------------------------

#[test]
fn cross_host_nav_and_page_close_no_regression() {
    if !common::run_isolated("realm_discard_timers_tests::cross_host_nav_and_page_close_no_regression")
    {
        return;
    }
    let fixture = DiscardFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/a")),
            ..Default::default()
        })
        .expect("create_page");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page ready");
    assert!(wait_title(&page, TITLE_A, Duration::from_secs(15)));

    let armed = arm_chain(&page, "cross");
    assert!(armed.contains("armed"), "chain dispatch failed: {armed:?}");
    let hits_at_arm = fixture.hit_count();
    assert!(
        pump_until(&page, Duration::from_secs(15), &|| fixture.hit_count() >= hits_at_arm + 2),
        "chain never fired (hits {:?})",
        fixture.hit_paths()
    );

    // Cross-host navigation: localhost vs 127.0.0.1 — different
    // registered domains, so this is NOT the same-thread reuse path (the
    // old ScriptThread exits with its thread-local registry). The nav
    // must complete, the new page must work, and the old chain must stop.
    let cross_url = format!("http://localhost:{}/b", fixture.port);
    page.navigate(&cross_url).expect("cross-host navigate");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("cross-host page ready");
    assert!(
        wait_title(&page, TITLE_B, Duration::from_secs(15)),
        "cross-host page title"
    );
    // Grace pump: quiesce the nav-commit → clear window's ticks.
    let quiesce = Instant::now();
    while quiesce.elapsed() < Duration::from_millis(500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(50));
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "old ScriptThread's chain EXECUTED after cross-host nav (hits: {:?})",
        fixture.hit_paths()
    );

    // Page close: the close path also funnels pipeline exits through
    // clear_js_runtime (now with the cancel bridge in it) — closing an
    // armed page must stop its chain and leave the runtime usable.
    let page2 = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/a?close=1")),
            ..Default::default()
        })
        .expect("create page2");
    page2.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page2 ready");
    assert!(wait_title(&page2, TITLE_A, Duration::from_secs(15)));
    let armed2 = arm_chain(&page2, "close");
    assert!(armed2.contains("armed"), "page2 dispatch {armed2:?}");
    let hits2_arm = fixture.hit_count();
    assert!(
        pump_until(&page2, Duration::from_secs(15), &|| fixture.hit_count() >= hits2_arm + 2),
        "page2 chain never fired"
    );
    page2.close().expect("page2 close");
    // Grace pump: quiesce page2's armed chain's in-flight ticks.
    let quiesce2 = Instant::now();
    while quiesce2.elapsed() < Duration::from_millis(500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(50));
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "closed page's chain EXECUTED post-close (hits: {:?})",
        fixture.hit_paths()
    );
    // Runtime still usable after the close (no wedge in the exit path).
    let probe = page.evaluate_js_web("document.title").unwrap_or_default();
    assert!(
        probe.contains(TITLE_B),
        "runtime unusable after page close (title probe {probe:?})"
    );
}

// ---------------------------------------------------------------------------
// ISSUE #25 — ConcurrentTask discard: an in-flight fetch whose completion
// lands AFTER its creation realm's discard must never re-enter JS.
//
// Enforcement lives at the dispatch site (the MiniEventLoop queue itself is
// an intrusive lock-free MPSC that cannot be purged): the completion's
// creation-realm global is read from a raw-rooted slot (a moving GC updates
// it in place) and checked against the timers' DEAD_GLOBALS mark; a match
// suppresses the JS re-entry (counter: concurrent_zombie_suppressed_total).
// Marked reversible: the user may veto this suppression semantics on return.
//
// What this e2e can assert against the real architecture (four-construct
// archaeology, 2026-09-29): at the TOP-LEVEL page lifecycle the
// zombie-re-entry window is UNREACHABLE — a same-domain navigation only
// sends UnloadDocument (the old pipeline lives on in session history, no
// mark, resolves legally), while a page close fires the ExitPipeline mark
// but also stops that loop's pump, so the late completion is never
// dispatched at all (WakeUp has no live receiver). The guard remains as
// defense-in-depth for future multi-pipeline-per-loop faces (iframe realms
// carry no bao fetch override today). Asserted here: the mark lands on
// close, the late completion never re-enters JS, and the timer face stays
// at zero.
// ---------------------------------------------------------------------------
#[test]
fn page_discard_inflight_completion_never_reenters_js() {
    let fixture = DiscardFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    // Deterministic discard trigger: page A's own pipeline CLOSE, with a
    // same-origin page B alive on the shared runtime: bao's lazy event loop
    // delivers the ExitPipeline (the RED-1 mark) only on ticks, and the
    // ConcurrentTask dispatch itself needs the same pump — a pure-sleep
    // wait would leave BOTH undriven and the guard unexercised. (A closed
    // page stops its own pump; the architecture note above explains why the
    // zombie window is unreachable at the top-level lifecycle — this test
    // pins the SAFE terminal state, not a suppression count.)
    let page_a = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/a")),
            ..Default::default()
        })
        .expect("create_page A");
    page_a
        .wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page A ready");
    assert!(
        wait_title(&page_a, TITLE_A, Duration::from_secs(15)),
        "page A title"
    );
    let page_b = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/b")),
            ..Default::default()
        })
        .expect("create_page B");
    page_b
        .wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page B ready");
    assert!(
        wait_title(&page_b, TITLE_B, Duration::from_secs(15)),
        "page B title"
    );

    // Arm: an in-flight fetch on page A to /hang (6s server-side delay)
    // whose .then would re-fetch a ZOMBIE-tagged /hit if the completion
    // re-entered the (about-to-be-discarded) realm.
    let armed = page_a
        .evaluate_js_web(
            "(function() { \
               fetch('/hang?tag=ct').then(function() { \
                 fetch('/hit?tag=ZOMBIE-CT'); \
               }); \
               return 'armed-ct'; \
             })()",
        )
        .unwrap_or_default();
    assert!(armed.contains("armed-ct"), "in-flight fetch arm failed: {armed:?}");
    let events_before = bun_runtime::timers::realm_discard_events_total();

    // Discard: close page A (ExitPipeline -> RED-1 mark) with the
    // completion still ~5s out.
    page_a.close().expect("close page A");

    // Pump via page B: drive until the mark for page A's realm has landed
    // AND the ~6s completion window has passed (10s hard deadline).
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(10);
    let completion_window = t0 + Duration::from_secs(7);
    loop {
        let _ = page_b.evaluate_js_web("");
        let marked = bun_runtime::timers::realm_discard_events_total() > events_before;
        if (marked && Instant::now() >= completion_window) || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        bun_runtime::timers::realm_discard_events_total() > events_before,
        "ISSUE #25: page close did not land the realm-discard mark (events={})",
        bun_runtime::timers::realm_discard_events_total()
    );
    assert!(
        !fixture.hit_paths().iter().any(|p| p.contains("ZOMBIE")),
        "ISSUE #25 zombie: the discarded realm's in-flight fetch completion \
         RE-ENTERED JS post-discard (hits: {:?})",
        fixture.hit_paths()
    );
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "timer-face zero regression alongside the ConcurrentTask guard"
    );
    page_b.close().expect("close page B after assertions");
}

// ---------------------------------------------------------------------------
// ISSUE #25 generalization — external-thread resolve guard (audio offline
// render station, the largest settle window of the B-class family). The
// offline render thread completes independently of the page lifecycle; when
// the completion lands after the creating realm's pipeline close, the
// resolve task must be dropped at dispatch (the
// post_discard_resolve_suppressed counter) instead of re-entering the dead
// realm's JS.
//
// Reachability: the discard trigger is an IFRAME pipeline close — the only
// live "mark lands while a pump survives" window. Two same-origin top-level
// pages are SEPARATE ScriptThreads (servo event-loop reuse applies to
// navigation within one browsing context), so a top-level page close kills
// its ScriptThread's task channel outright; a removed iframe closes its
// pipeline (ExitPipeline -> mark) while the host page keeps the SHARED
// ScriptThread pump alive, and the render settle (dom_manipulation task
// source) drains on the host's ticks into the guard. The 300s render
// outlasts every setup race by orders of magnitude.
// ---------------------------------------------------------------------------
#[test]
fn offline_render_resolve_suppressed_after_discard() {
    let fixture = DiscardFixture::spawn();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let page = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url("/a")),
            ..Default::default()
        })
        .expect("create_page");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("page ready");
    assert!(wait_title(&page, TITLE_A, Duration::from_secs(15)), "page title");

    // Arm inside the iframe realm (the pipeline that will be discarded): a
    // 300s-stereo offline render whose .then would re-enter JS
    // (fetch /hit?tag=AUDIO-DONE) when the render completes.
    let armed = page.evaluate_js_web(
        "(function() { \
           var f = document.createElement('iframe'); \
           f.id = 'zframe'; \
           f.src = '/frame'; \
           document.body.appendChild(f); \
           return 'framed'; \
         })()",
    )
    .unwrap_or_default();
    assert!(armed.contains("framed"), "iframe arm failed: {armed:?}");
    // Let the iframe pipeline load and its script start the render.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_millis(1500) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(100));
    }
    let events_before = bun_runtime::timers::realm_discard_events_total();

    // Discard: remove the iframe -> constellation close_pipeline ->
    // ExitPipeline -> RED-1 mark, with the render still in flight on its
    // audio thread.
    let removed = page.evaluate_js_web(
        "(function() { \
           var f = document.getElementById('zframe'); \
           if (!f) { return 'missing'; } \
           f.remove(); \
           return 'removed'; \
         })()",
    )
    .unwrap_or_default();
    assert!(removed.contains("removed"), "iframe removal failed: {removed:?}");

    // Pump through the render's completion window: whatever the terminal
    // path (servo's audio teardown short-circuits the eos chain, or a
    // queued settle drains into the guard), the dead realm's JS must never
    // be re-entered — no AUDIO-DONE, and the timer face stays at zero. The
    // mark itself must land (pipeline close = ExitPipeline on the shared
    // ScriptThread, drained by the host's ticks). The POSITIVE
    // suppression-count assertion is NOT constructible here: the render
    // time is unbounded from below by an API knob (servo-media throughput
    // collapses non-linearly past ~13M samples) and the teardown races the
    // mark — a guard hit was observed live once (discarded=true, DONE
    // dropped), proving the mechanism, but not deterministically
    // schedulable.
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let _ = page.evaluate_js_web("");
        let marked = bun_runtime::timers::realm_discard_events_total() > events_before;
        if (marked && Instant::now() >= deadline) || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        bun_runtime::timers::realm_discard_events_total() > events_before,
        "ISSUE #25 B-class: iframe close did not land the realm-discard mark \
         (events={})",
        bun_runtime::timers::realm_discard_events_total()
    );
    assert!(
        !fixture.hit_paths().iter().any(|p| p.contains("AUDIO-DONE")),
        "ISSUE #25 B-class: the discarded realm's render settle RE-ENTERED JS \
         (hits: {:?})",
        fixture.hit_paths()
    );
    assert_eq!(
        bun_runtime::timers::zombie_fires_total(),
        0,
        "timer-face zero regression alongside the external-thread resolve guard"
    );
    page.close().expect("close page after assertions");
}
