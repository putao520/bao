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

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PageState};

const TITLE: &str = "page-state-fixture";

/// servo/BaoRuntime carry process-global slots (one JSContext per thread;
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
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
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
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
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
