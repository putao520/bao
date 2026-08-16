// @trace TEST-BRW-001-IDB [req:REQ-BRW-001] [level:e2e]
// indexedDB end-to-end over servo's REAL IDB implementation.
//
// Upstream `dom_indexeddb_enabled` defaults to false (experimental); bao is
// a full browser runtime, so `BaoRuntime::new` flips it ON via the
// `ServoBuilder::preferences` override surface (vendor defaults untouched —
// `Servo::new` ends with `prefs::set(preferences.unwrap_or_default())`, so
// the builder is the only durable injection point). `GlobalScope::
// obtain_storage_key` reads the pref at runtime per IDB open, which is what
// these tests exercise: a real page (http origin — data: URLs are opaque
// origins and storage keys fail by spec) opening a database, running an
// upgrade transaction, and round-tripping put/get through servo's Idb
// machinery.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, PageConfig, PageHandle, PageState};

/// Minimal H1 fixture serving one fixed document on every request — enough
/// origin (http://127.0.0.1:PORT) for a non-opaque storage key.
struct PageFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

impl PageFixture {
    fn spawn(html: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind page fixture");
        let port = listener.local_addr().unwrap().port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_c = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name("idb-page-fixture".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("nonblocking listener");
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.read(&mut [0u8; 2048]); // drain request head
                            let body = html.as_bytes();
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = tcp.write_all(head.as_bytes());
                            let _ = tcp.write_all(body);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn page fixture");
        PageFixture { port, shutdown }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for PageFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Pump servo's loop while waiting for `page`-side `globalThis.__idbState`
/// to leave 'pending'. IDB callbacks (onupgradeneeded/onsuccess/tx.oncomplete)
/// fire on servo's script thread as its event loop turns.
fn wait_for_idb_state(page: &PageHandle, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = page.evaluate_js("");
        if let Ok(state) = page.evaluate_js_web("String(globalThis.__idbState)") {
            let state = state.trim().trim_matches('"').to_string();
            if state != "pending" {
                return Some(state);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Full roundtrip: open (with upgrade) → put → get → byte-exact value. This
/// is servo's real IDB implementation end to end — the pref flip is what
/// makes `obtain_storage_key` return a key instead of None.
#[test]
fn indexeddb_open_put_get_roundtrip() {
    bun_core::Output::init_test();

    let fixture = PageFixture::spawn("<!DOCTYPE html><html><body><p>idb</p></body></html>");

    let runtime = bao_browser::BaoRuntime::new(BaoConfig::default())
        .expect("BaoRuntime::new");
    let pool = runtime.page_pool();

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(fixture.url()),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            }
            Err(e) => {
                eprintln!("page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    let page = page.expect("page creation failed after retries");
    wait_for_load(&page, 5000);

    // Wire the full roundtrip in the PAGE realm. State machine surfaces every
    // failure mode distinctly (opaque-origin storage key, blocked open,
    // transaction abort) instead of a bare hang.
    let issued = page.evaluate_js_web(
        r#"
        globalThis.__idbState = 'pending';
        globalThis.__idbDetail = '';
        if (typeof indexedDB === 'undefined') {
            globalThis.__idbState = 'no-indexedDB-global';
        } else {
            var req = indexedDB.open('bao-idb-e2e', 1);
            req.onupgradeneeded = function(e) {
                var db = e.target.result;
                if (!db.objectStoreNames.contains('kv')) db.createObjectStore('kv');
            };
            req.onsuccess = function(e) {
                var db = e.target.result;
                var tx = db.transaction('kv', 'readwrite');
                tx.objectStore('kv').put('bao-roundtrip-value', 'e2e-key');
                tx.onabort = function() {
                    globalThis.__idbDetail = String(tx.error && tx.error.name);
                    globalThis.__idbState = 'tx-aborted';
                };
                tx.oncomplete = function() {
                    var tx2 = db.transaction('kv', 'readonly');
                    var getReq = tx2.objectStore('kv').get('e2e-key');
                    getReq.onsuccess = function() {
                        globalThis.__idbDetail = String(getReq.result);
                        globalThis.__idbState = 'ok';
                        db.close();
                    };
                    getReq.onerror = function() {
                        globalThis.__idbDetail = String(getReq.error && getReq.error.name);
                        globalThis.__idbState = 'get-error';
                    };
                };
            };
            req.onerror = function() {
                globalThis.__idbDetail = String(req.error && req.error.name);
                globalThis.__idbState = 'open-error';
            };
            req.onblocked = function() { globalThis.__idbState = 'open-blocked'; };
        }
        'issued'
    "#,
    );
    assert_eq!(
        issued.unwrap_or_default().trim().trim_matches('"'),
        "issued",
        "IDB wiring must eval cleanly in the page realm"
    );

    let state = wait_for_idb_state(&page, Duration::from_secs(15));
    let detail = page
        .evaluate_js_web("String(globalThis.__idbDetail)")
        .unwrap_or_default();
    let detail = detail.trim().trim_matches('"').to_string();
    assert_eq!(
        state.as_deref(),
        Some("ok"),
        "indexedDB roundtrip must complete (state={:?} detail={:?}) — \
         pref dom_indexeddb_enabled must be flipped on at the builder surface",
        state, detail
    );
    assert_eq!(
        detail, "bao-roundtrip-value",
        "get() must return the exact put() payload"
    );
}
