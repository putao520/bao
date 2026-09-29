// @trace TEST-BRW-001 [req:REQ-BRW-001] [sm:PageLifecycle] [level:e2e]
// BCE (PageState never left Navigating, 2026-08-19) regression:
// the stored state machine had NO writer for the SPEC 02-SYSTEM
// PageLifecycle `Navigating → Interactive on load_complete` transition —
// get_state() reported Navigating forever while the page was fully loaded
// (title/DOM/evaluate ready; the v-w0 smoke poll stayed Navigating for 45s).
// Root fix: get_state() projects servo's LoadStatus (single source of truth,
// written by notify_load_status_changed) — Complete → Interactive — and
// navigate/reload/go_back/go_forward reset load_status to Started so a
// second navigation can't read the previous load's stale Complete.
//
// Real-path assertions: state reaches Interactive, and the transition time
// matches document.title readiness within ±2s; a re-navigation immediately
// reports Navigating (stale-Complete race regression pin).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PageState};
use bao_stealth::StealthProfile;

const TITLE: &str = "page-state-fixture";

/// servo/BrowserRuntime carry process-global slots (one JSContext per thread;
/// embedder state) — two runtimes racing in one test binary deadlock one
/// side. Serialize the tests in this suite.
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl Fixture {
    fn spawn() -> Self {
        let body = format!(
            "<!DOCTYPE html><html><head><title>{TITLE}</title></head><body><p>fixture</p></body></html>"
        )
        .into_bytes();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("pagestate-fixture".into())
            .spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let mut req = [0u8; 2048];
                            let _ = tcp.read(&mut req);
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = tcp.write_all(head.as_bytes());
                            let _ = tcp.write_all(&body);
                            let _ = tcp.flush();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn fixture");
        Fixture { port, shutdown }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Poll until `cond` holds or the deadline expires; pump the page's callback
/// drain (evaluate_js_web("")) each pass so servo load events advance both
/// `load_status` and the DOM. Returns the elapsed time when cond first held.
fn poll_until(page: &PageHandle, timeout: Duration, cond: &dyn Fn() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js_web(""); // pump the callback drain
        if cond() {
            return Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[test]
fn pagestate_reaches_interactive_in_step_with_title() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
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

    page.navigate(&fixture.url()).expect("navigate");

    let start = Instant::now();
    let t_state = poll_until(&page, Duration::from_secs(30), &|| {
        page.get_state() == PageState::Interactive
    });
    let t_title = poll_until(
        &page,
        Duration::from_secs(30),
        &|| matches!(page.evaluate_js_web("document.title"), Ok(t) if t.contains(TITLE)),
    );
    let total = start.elapsed();

    assert!(
        t_state.is_some(),
        "get_state() must reach Interactive (was stuck Navigating forever pre-fix); total {total:?}"
    );
    assert!(t_title.is_some(), "document.title never became ready");
    let delta = t_state.unwrap().abs_diff(t_title.unwrap());
    assert!(
        delta <= Duration::from_secs(2),
        "Interactive transition must track title readiness ±2s (state {:?}, title {:?})",
        t_state,
        t_title
    );
    assert_eq!(page.get_state(), PageState::Interactive);

    // Stale-Complete race pin: a second navigation must immediately report
    // Navigating (load_status reset), never the previous load's Interactive.
    page.navigate(&fixture.url()).expect("re-navigate");
    assert_eq!(
        page.get_state(),
        PageState::Navigating,
        "re-navigation must reset to Navigating immediately (stale Complete)"
    );
    let t2 = poll_until(&page, Duration::from_secs(30), &|| {
        page.get_state() == PageState::Interactive
    });
    assert!(t2.is_some(), "re-navigation must reach Interactive again");
}

#[test]
fn verbatim_readme_path1_no_external_pump() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // BCE (pump-contract restore, 2026-08-19) regression pin: the README
    // path1 snippet has NO external pump — navigate → wait_for_pipeline_ready
    // → evaluate must observe the NAVIGATED page. Pre-fix, wait returned at
    // the about:blank first frame and the pending load had no driver.
    let fixture = Fixture::spawn();
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

    page.navigate(&fixture.url()).expect("navigate");
    page.wait_for_pipeline_ready(Duration::from_secs(30))
        .expect("wait_for_pipeline_ready");

    // No pump loop here — the wait contract itself must have driven the load.
    let url = page.current_url().unwrap_or_default();
    let title = page.evaluate_js_web("document.title").unwrap_or_default();
    let state = page.get_state();
    assert!(
        url.contains("127.0.0.1"),
        "verbatim wait must leave the NAVIGATED url (got {url:?})"
    );
    assert!(
        title.contains(TITLE),
        "verbatim wait must expose the loaded document's title (got {title:?})"
    );
    assert_eq!(state, PageState::Interactive);
}

// ═══════════════════════════════════════════════════════════════════════
// W16 #15-B: PageLifecycle explicit state machine — Tier I (e2e edges) and
// Tier S (shutdown). The transition table itself is exhaustively unit-tested
// in page.rs (`page_lifecycle_transition_matrix_exhaustive_48_cells`).
// @trace TEST-BRW-001 [req:REQ-BRW-001] [sm:PageLifecycle] [level:e2e]
// ═══════════════════════════════════════════════════════════════════════

/// A listener whose responses are parked far beyond the test window: the
/// page is observably Navigating (load Started, never Complete) for the
/// whole mid-load test, but the exchange is never left permanently silent.
/// Each connection either gets its response after `RESPONSE_DELAY` or is
/// closed on shutdown, whichever comes first.
///
/// Why the delay + close instead of silence forever: `ServoInner::drop`
/// spins on the embedder channel until the ScriptThreads join and has NO
/// timeout — an in-flight fetch parked on a forever-silent socket wedges
/// runtime teardown indefinitely (discovered by this suite; the tests
/// therefore also drop the fixture explicitly before their epilogue, so
/// the EOF reaches the fetch before the runtime drops).
struct StalledFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl StalledFixture {
    /// Parked-response delay: an order of magnitude beyond the mid-load
    /// assertions, short enough to self-terminate the exchange if a test
    /// ever tears down without dropping the fixture first.
    const RESPONSE_DELAY: Duration = Duration::from_secs(8);

    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("stalled-fixture".into())
            .spawn(move || {
                let body = b"<!DOCTYPE html><html><body><p>stalled</p></body></html>";
                listener.set_nonblocking(true).expect("nonblocking");
                'outer: while !flag.load(Ordering::SeqCst) {
                    let mut tcp = match listener.accept() {
                        Ok((tcp, _)) => tcp,
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(_) => return,
                    };
                    let mut req = [0u8; 2048];
                    let _ = tcp.read(&mut req);
                    // Hold the connection open (the load stays in flight)
                    // until the delay elapses or shutdown wins.
                    let start = Instant::now();
                    while !flag.load(Ordering::SeqCst) && start.elapsed() < Self::RESPONSE_DELAY
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if flag.load(Ordering::SeqCst) {
                        break 'outer; // drop tcp → EOF unblocks the fetch
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = tcp.write_all(head.as_bytes());
                    let _ = tcp.write_all(body);
                    let _ = tcp.flush();
                }
            })
            .expect("spawn stalled fixture");
        StalledFixture { port, shutdown }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for StalledFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn make_runtime(cfg: BaoConfig) -> Option<BrowserRuntime> {
    match BrowserRuntime::new(cfg) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            None
        }
    }
}

fn make_page(runtime: &BrowserRuntime, url: Option<String>) -> PageHandle {
    runtime
        .create_page(&PageConfig {
            url,
            ..Default::default()
        })
        .expect("create_page")
}

fn poll_interactive(page: &PageHandle, timeout: Duration) {
    let hit = poll_until(page, timeout, &|| page.get_state() == PageState::Interactive);
    assert!(hit.is_some(), "page never reached Interactive");
}

/// T5/T6 (G1): release into the idle pool materializes Idle on the page
/// state; reacquiring materializes Interactive again. The pool-level
/// idle map and the page-level PageState move in lockstep.
#[test]
fn pagestate_release_then_reacquire_roundtrip_idle_interactive() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    page.navigate(&fixture.url()).expect("navigate");
    poll_interactive(&page, Duration::from_secs(30));

    let id = page.id();
    runtime.page_pool().release_page(id);
    // The old handle shares the inner Rc — it must observe Idle, not a
    // stale Interactive (T5).
    assert_eq!(page.get_state(), PageState::Idle, "T5: released page must read Idle");
    {
        let stats = runtime.page_pool().stats();
        assert_eq!((stats.active, stats.idle), (0, 1), "pool maps after release");
    }

    let back = runtime.page_pool().get_page(id).expect("T6: get_page from idle");
    assert_eq!(back.get_state(), PageState::Interactive, "T6: reacquired page");
    // Same inner Rc — the original handle observes the reacquire too.
    assert_eq!(page.get_state(), PageState::Interactive);
    {
        let stats = runtime.page_pool().stats();
        assert_eq!((stats.active, stats.idle), (1, 0), "pool maps after reacquire");
    }
}

/// T7 (G1): an idle page past the pool's idle_ttl is reclaimed through the
/// `Idle --idle_ttl_expired--> Closing --cleanup_complete--> Closed` path.
#[test]
fn pagestate_idle_ttl_reclaim_closes_idle_page() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
    let cfg = BaoConfig {
        idle_ttl: Duration::from_millis(150),
        ..Default::default()
    };
    let runtime = match make_runtime(cfg) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    page.navigate(&fixture.url()).expect("navigate");
    poll_interactive(&page, Duration::from_secs(30));

    let id = page.id();
    runtime.page_pool().release_page(id);
    assert_eq!(page.get_state(), PageState::Idle);
    std::thread::sleep(Duration::from_millis(400)); // > idle_ttl

    let reclaimed = runtime.page_pool().check_idle_pages();
    assert_eq!(reclaimed, 1, "T7: exactly the expired page reclaimed");
    assert_eq!(page.get_state(), PageState::Closed, "T7: reclaimed page reads Closed");
    let stats = runtime.page_pool().stats();
    assert_eq!((stats.active, stats.idle), (0, 0));
    assert_eq!(stats.total_destroyed, 1, "T7: destroy counter");
}

/// T3: closing a page whose load is still in flight enters Closing via
/// close_during_load and reaches Closed.
#[test]
fn pagestate_close_during_load_reaches_closed() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let stalled = StalledFixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    page.navigate(&stalled.url()).expect("navigate to stalled fixture");
    assert_eq!(
        page.get_state(),
        PageState::Navigating,
        "mid-load page must be observably Navigating"
    );

    page.close().expect("close mid-load");
    assert_eq!(page.get_state(), PageState::Closed, "T3: close_during_load → Closed");
    assert!(!page.is_alive());

    // Drop the fixture BEFORE the test epilogue: locals drop in reverse
    // declaration order, so `runtime` (declared after `stalled`) would drop
    // first and its ServoInner::drop join-spin would park on the in-flight
    // fetch until the fixture's delayed response — the explicit EOF keeps
    // teardown prompt (see StalledFixture).
    drop(stalled);
}

/// G0: a page created with an initial URL leaves Created without an explicit
/// navigate — the builder load is a Navigate transition and the projection
/// then carries it to Interactive.
#[test]
fn pagestate_initial_url_page_reaches_interactive_without_explicit_navigate() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some(fixture.url()));
    assert_ne!(
        page.get_state(),
        PageState::Created,
        "G0: initial-URL page must not be stuck in Created (got {:?})",
        page.get_state()
    );
    poll_interactive(&page, Duration::from_secs(30));
    // The projection is backed by the real document, not just the flag.
    let title = page.evaluate_js_web("document.title").unwrap_or_default();
    assert!(title.contains(TITLE), "G0: loaded initial document (title {title:?})");
}

/// G5: close-after-close is idempotent; every operating face after close
/// fails with "page is closed"; unknown ids are no-ops.
#[test]
fn pagestate_negative_paths_after_close() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    let id = page.id();

    page.close().expect("first close");
    page.close().expect("second close is idempotent");
    assert_eq!(page.get_state(), PageState::Closed);
    assert!(!page.is_alive());

    let err = page.navigate("about:blank").expect_err("navigate after close");
    assert!(
        err.to_string().contains("page is closed"),
        "navigate after close must say page is closed (got {err:?})"
    );
    let err = page
        .evaluate_js_web("1+1")
        .expect_err("evaluate after close");
    assert!(
        err.to_string().contains("page is closed"),
        "evaluate after close must say page is closed (got {err:?})"
    );

    // Unknown ids: get_page → None, release_page → observable no-op.
    assert!(runtime.page_pool().get_page(424_242).is_none());
    let before = runtime.page_pool().stats();
    runtime.page_pool().release_page(424_242);
    let after = runtime.page_pool().stats();
    assert_eq!(
        (before.active, before.idle, before.total_created, before.total_destroyed),
        (after.active, after.idle, after.total_created, after.total_destroyed),
        "release of unknown id must be a no-op"
    );

    // The pool-side close of the already-handle-closed page is also Ok
    // (double close across both faces).
    runtime.page_pool().close_page(id).expect("pool close after handle close");
}

/// G2 regression pin: a page released while its load is still in flight is
/// NOT pseudo-Idle — it keeps Navigating in the idle pool (so a later TTL
/// reclaim enters Closing via close_during_load, closing a page that never
/// finished loading, never an "idle" one that silently became active).
#[test]
fn pagestate_release_mid_load_stays_navigating_then_close_during_load() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let stalled = StalledFixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    page.navigate(&stalled.url()).expect("navigate to stalled fixture");
    assert_eq!(page.get_state(), PageState::Navigating);

    let id = page.id();
    runtime.page_pool().release_page(id);
    assert_eq!(
        page.get_state(),
        PageState::Navigating,
        "G2: mid-load release must NOT report pseudo-Idle"
    );

    // Reacquire of a still-Navigating page performs no Idle transition.
    let back = runtime.page_pool().get_page(id).expect("reacquire mid-load page");
    assert_eq!(back.get_state(), PageState::Navigating);

    runtime.page_pool().close_page(id).expect("close mid-load from pool");
    assert_eq!(page.get_state(), PageState::Closed);
    assert_eq!(runtime.page_pool().stats().total_destroyed, 1);

    // Same teardown rationale as T3: EOF the parked fetch before the
    // runtime's join spin (see StalledFixture).
    drop(stalled);
}

// ── Tier S: shutdown order (single-threaded Rc ownership guarantees the
// sequence; these e2e assertions pin its observable consequences) ────────

/// S1: close_all drains BOTH maps exactly once per page.
#[test]
fn shutdown_close_all_drains_active_and_idle_maps() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let p_active = make_page(&runtime, Some("about:blank".into()));
    p_active.navigate(&fixture.url()).expect("navigate active");
    poll_interactive(&p_active, Duration::from_secs(30));

    let p_idle = make_page(&runtime, Some("about:blank".into()));
    p_idle.navigate(&fixture.url()).expect("navigate idle");
    poll_interactive(&p_idle, Duration::from_secs(30));
    runtime.page_pool().release_page(p_idle.id());

    {
        let stats = runtime.page_pool().stats();
        assert_eq!((stats.active, stats.idle, stats.total_created), (1, 1, 2));
    }

    runtime.page_pool().close_all();
    let stats = runtime.page_pool().stats();
    assert_eq!((stats.active, stats.idle), (0, 0), "S1: both maps drained");
    assert_eq!(stats.total_destroyed, 2, "S1: each page destroyed exactly once");
    assert_eq!(p_active.get_state(), PageState::Closed);
    assert_eq!(p_idle.get_state(), PageState::Closed);
}

/// S2 (registry cleanliness, consequence of the close order): after a
/// stealthed page closes, a fresh stealth-free page inherits none of its
/// engine-level navigator overrides (BUG-ENG-366 class probe — keyed
/// registry cleanup at close; address-reuse inheritance would surface here
/// as the probe UA leaking into the stealth-free page).
#[test]
fn shutdown_stealth_registry_clean_no_inheritance() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::spawn();
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let probe_ua = "Mozilla/5.0 (X11; Linux x86_64) BaoS2Probe/16.0 W16".to_string();

    let ua_of = |page: &PageHandle| -> String {
        let hit = poll_until(
            page,
            Duration::from_secs(30),
            &|| {
                matches!(page.evaluate_js_web("document.readyState"), Ok(ref s) if s == "complete")
            },
        );
        assert!(hit.is_some(), "page never reached readyState complete");
        page.evaluate_js_web("navigator.userAgent").expect("read userAgent")
    };

    // 1. Native baseline (stealth-free).
    let p1 = make_page(&runtime, Some(fixture.url()));
    let ua_native = ua_of(&p1);
    runtime.page_pool().close_page(p1.id()).expect("close p1");

    // 2. Stealthed page with a unique probe UA, then close it.
    let mut profile = StealthProfile::firefox_default();
    profile.navigator.user_agent = probe_ua.clone();
    let p2 = runtime
        .create_page(&PageConfig {
            url: Some(fixture.url()),
            stealth_profile: Some(profile),
            ..Default::default()
        })
        .expect("create stealthed page");
    assert_eq!(ua_of(&p2), probe_ua, "probe page must run the stealth UA");
    runtime.page_pool().close_page(p2.id()).expect("close p2");

    // 3. Fresh stealth-free page: native UA back, no probe residue.
    let p3 = make_page(&runtime, Some(fixture.url()));
    let ua_after = ua_of(&p3);
    assert_eq!(
        ua_after, ua_native,
        "S2: stealth-free page after stealthed close must read the native UA (no inheritance)"
    );
    assert!(!ua_after.contains("BaoS2Probe"), "S2: probe UA leaked");
}

/// S3 (liveness smoke — honestly scoped): dropping a runtime with a live
/// page AND a pending Node-realm timer completes within a bounded window.
/// This is a join-determinism smoke, not an ordering proof: the close order
/// itself (close_all → CDP token clear → Rc<Servo> final release) is
/// guaranteed by the single-threaded Rc ownership structure, which cannot be
/// instrumented cross-thread without vendor changes (out of W16 scope).
#[test]
fn shutdown_drop_runtime_with_pending_timer_is_bounded() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = match make_runtime(BaoConfig::default()) {
        Some(r) => r,
        None => return,
    };
    let page = make_page(&runtime, Some("about:blank".into()));
    let armed = page
        .evaluate_js("(function(){ setTimeout(function(){}, 86400000); return 'armed'; })()")
        .expect("arm pending node-realm timer");
    assert_eq!(armed, "armed");

    let start = Instant::now();
    drop(page);
    drop(runtime);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "S3: runtime drop with live page + pending timer must not hang (took {elapsed:?})"
    );
}
