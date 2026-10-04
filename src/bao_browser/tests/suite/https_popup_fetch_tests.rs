// @trace TEST-BRW-HTTPS-POPUP-FETCH [req:REQ-BRW-002] [level:e2e]
// HTTPS (self-signed) opener → window.open(HTTPS popup) → popup fetch() →
// bounded settle. RED lock for the D2 defect face (e63 forensics).
//
// Root cause chain (probe-verified, test-ci instrumented binary):
//   1. bao's deferred page injection installed the Node-stack fetch override
//      (`bun_runtime::fetch_api::install_fetch_global`) onto every page realm,
//      shadowing servo's WHATWG fetch.
//   2. The override builds bun_http AsyncHTTP options straight from the JS
//      init — with no `init.tls` it defaults `rejectUnauthorized = true` and
//      never consults servo's `ignore_certificate_errors` / `certificate_path`
//      posture (the WPT/tooling CA path).
//   3. An HTTPS fetch after the injection to a non-public-CA target therefore
//      completed the TLS handshake with err_no=18 (self-signed) and died in
//      HTTPContext::on_handshake (`reject_unauthorized &&
//      did_have_handshaking_error` → close_and_fail → RST). The page saw
//      `TypeError: fetch failed`.
//   4. HTTP popups never exercise verification, which is why the defect
//      presented as a pure "HTTPS popup fetch silently dropped" axis.
//
// Fix (底层统一用户裁决 2026-10-04): page realms (document present) are
// EXCLUDED from the override — servo WHATWG fetch owns the page transport
// (the 2026-08-15 page-network unification, 5623b4b7); Node/bun engine
// realms keep the override unchanged. The intermediate TLS-posture
// propagation shape (commit 723d41af) was overturned: it legalized the
// dual-stack split instead of removing it.
//
// The popup reports its fetch outcome over a SYNC XHR marker channel — sync
// XHR is NOT overridden (servo-net path, honors the ignore posture) — so the
// observable does not depend on the async fetch implementation.
//
// This test pins the egress AND settle faces: the popup's async fetch must
// reach the wire under `ignore_certificate_errors` (pre-fix it never did)
// AND settle (servo fetch → bridge → honors the posture end-to-end), plus
// the fetch-binding identity guardrail across the deferred injection
// (page realms no longer get the Node-stack override).
//
// Graceful strategy (suite convention): requires BAO_TEST_REAL_SERVO=1 and a
// display server; absent either → skip (never a false red).

#![allow(dead_code)]

use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};

const POPUP_PAGE: &str = r#"<!DOCTYPE html><html><body><script>
globalThis.__pop = { state: "pending", ran: true };
fetch("/marker/pop-fetch").then(
  (r) => { report("ok:" + r.status); },
  (e) => { report("err:" + e); }
);
function report(state) {
  globalThis.__pop.state = state;
  try {
    const x = new XMLHttpRequest();
    x.open("GET", "/marker/pop-rep?state=" + encodeURIComponent(state), false);
    x.send(null);
  } catch (err) {}
}
</script></body></html>"#;

const OPENER_PAGE: &str = r#"<!DOCTYPE html><html><body><script>
globalThis.__op = { state: "pending" };
fetch("/marker/op-fetch").then(
  (r) => { globalThis.__op.state = "ok:" + r.status; },
  (e) => { globalThis.__op.state = "err:" + e; }
);
</script></body></html>"#;

// ---------------------------------------------------------------------------
// Fixture — one self-signed HTTPS server, page + marker routes
//
// The server is a python3 ThreadingHTTPServer subprocess with the SAME wire
// shape as the WPT carrier and the e63 probe matrix (python `ssl` TLS server,
// ServerHello first / encrypted flight after a scheduling gap). This is the
// geometry the D2 defect reproduces under; an in-process Rust TLS server that
// flushes its whole handshake flight in one segment trips a DIFFERENT latent
// client-side stall (handshake completes, first request write never flushed)
// and is not the D2 face under test.
// ---------------------------------------------------------------------------

struct HttpsPopupFixture {
    port: u16,
    cert_dir: std::path::PathBuf,
    hits_path: std::path::PathBuf,
    child: std::process::Child,
}

const SERVER_PY: &str = r#"
import http.server, ssl, sys, threading

PORT = int(sys.argv[1])
CERT, KEY, HITS = sys.argv[2], sys.argv[3], sys.argv[4]

POPUP_PAGE = open(sys.argv[5], encoding="utf-8").read()
OPENER_PAGE = open(sys.argv[6], encoding="utf-8").read()

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def _serve(self):
        path = self.path
        with open(HITS, "a", encoding="utf-8") as f:
            f.write(path + "\n")
            f.flush()
        if path == "/popup":
            ctype, body = "text/html", POPUP_PAGE.encode()
        elif path == "/opener":
            ctype, body = "text/html", OPENER_PAGE.encode()
        else:
            ctype, body = "text/plain", b"ok"
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-cache, no-store, must-revalidate")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self._serve()
    def do_POST(self):
        self._serve()

srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(CERT, KEY)
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
with open(HITS, "a", encoding="utf-8") as f:
    f.write("/ready\n")
srv.serve_forever()
"#;

impl HttpsPopupFixture {
    fn spawn() -> Self {
        let (cert_pem, key_pem) = bao_boringssl_bridge::generate_self_signed_pem("localhost", 1)
            .expect("self-signed pem");
        let dir = std::env::temp_dir().join(format!(
            "e63-https-popup-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        let popup_path = dir.join("popup.html");
        let opener_path = dir.join("opener.html");
        let hits_path = dir.join("hits.log");
        std::fs::write(&cert_path, cert_pem).expect("write cert");
        std::fs::write(&key_path, key_pem).expect("write key");
        std::fs::write(&popup_path, POPUP_PAGE).expect("write popup page");
        std::fs::write(&opener_path, OPENER_PAGE).expect("write opener page");
        let server_py = dir.join("server.py");
        std::fs::write(&server_py, SERVER_PY).expect("write server.py");

        // Port 0: the python server binds an OS-assigned port and logs it via
        // its own bind result — recover it from the ready marker + the test's
        // own listener-free probe. Simplest reliable form: pick a port here.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let child = std::process::Command::new("python3")
            .arg(&server_py)
            .arg(port.to_string())
            .arg(&cert_path)
            .arg(&key_path)
            .arg(&hits_path)
            .arg(&popup_path)
            .arg(&opener_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn python3 TLS fixture (python3 must be on PATH)");

        // Wait for the ready marker (server bound + TLS wrapped).
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = std::fs::read_to_string(&hits_path) {
                if s.contains("/ready") {
                    break;
                }
            }
            if Instant::now() > deadline {
                panic!("TLS fixture did not become ready in 10s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        HttpsPopupFixture {
            port,
            cert_dir: dir,
            hits_path,
            child,
        }
    }

    fn base(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> Vec<String> {
        std::fs::read_to_string(&self.hits_path)
            .unwrap_or_default()
            .lines()
            .map(|l| l.to_string())
            .collect()
    }

    fn hit_count(&self, prefix: &str) -> usize {
        self.hits().iter().filter(|p| p.starts_with(prefix)).count()
    }

    fn hit_matching(&self, needle: &str) -> Option<String> {
        self.hits().into_iter().find(|p| p.contains(needle))
    }
}

impl Drop for HttpsPopupFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.cert_dir);
    }
}

// ---------------------------------------------------------------------------
// Harness (suite convention: graceful skip without a real servo display)
// ---------------------------------------------------------------------------

fn runtime_or_skip(tag: &str) -> Option<BrowserRuntime> {
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        eprintln!("[skip] {tag}: BAO_TEST_REAL_SERVO != 1");
        return None;
    }
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] {tag}: no DISPLAY or WAYLAND_DISPLAY");
        return None;
    }
    let config = BaoConfig {
        // WPT posture: the servo page network (bridge + sync XHR) must accept
        // the self-signed fixture; the popup's async fetch must inherit the
        // same posture (the face this test pins).
        ignore_certificate_errors: true,
        ..BaoConfig::default()
    };
    match BrowserRuntime::new(config) {
        Ok(runtime) => Some(runtime),
        Err(e) => {
            eprintln!("[skip] {tag}: BrowserRuntime::new failed: {e}");
            None
        }
    }
}

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(
            page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// RED lock (e63 D2): an HTTPS popup's async `fetch()` must settle against a
/// self-signed HTTPS target under the runtime's `ignore_certificate_errors`
/// posture. Pre-fix the deferred-injection fetch override enforces strict
/// verification and the fetch dies post-handshake (`TypeError: fetch failed`).
#[test]
fn https_popup_fetch_settles_under_ignore_certificate_errors() {
    bun_core::Output::init_test();

    let fixture = HttpsPopupFixture::spawn();
    let Some(runtime) = runtime_or_skip("https_popup_fetch") else {
        return;
    };
    let pool = runtime.page_pool();

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(format!("{}/opener", fixture.base())),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            },
            Err(e) => {
                eprintln!("page creation failed (retrying): {e}");
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    let Some(page) = page else {
        panic!("create_page failed after retries");
    };
    wait_for_load(&page, 15_000);

    // Guardrail ① (竞态消除的直接证据): pin the opener's `fetch` binding
    // BEFORE the deferred injection can run, then require the SAME function
    // object after it. Under the old shape the injection REPLACED the page's
    // fetch binding mid-flight (servo fetch → Node-stack override — a
    // per-call transport/fingerprint switch inside one page). Page realms
    // no longer get the override, so the binding must be stable.
    let _ = page.evaluate_js_web("window.__f1 = fetch");

    // Open the popup through the real window.open path, then drain the
    // pump-side deferred init BEFORE the popup document's script runs — this
    // pins the injection-first ordering deterministically (the defect's
    // losing order; post-fix both orders must settle).
    let opened = page
        .evaluate_js_web(&format!(
            "String(window.open('{}/popup') !== null)",
            fixture.base()
        ))
        .expect("opener evaluate");
    assert_eq!(
        opened, "true",
        "window.open must return a non-null Window (e47 landed) — got {opened}"
    );
    let initialized = pool.init_pending_pages();
    assert_eq!(initialized, 1, "exactly one popup awaited deferred init");
    let same_fetch = page
        .evaluate_js_web("String(fetch === window.__f1)")
        .expect("fetch identity evaluate");
    assert_eq!(
        same_fetch, "true",
        "page fetch binding must survive the deferred injection (page realms \
         are excluded from the Node-stack override) — got {same_fetch}"
    );

    let popup_id = pool
        .live_page_ids()
        .into_iter()
        .find(|id| *id != page.id())
        .expect("popup page pooled after window.open");
    let popup = pool.get_page(popup_id).expect("popup handle");

    // D2 defect face: the popup's async fetch must EGRESS. Pre-fix (the
    // strict-TLS override) it never reached the wire — RST after the TLS
    // handshake, zero server-side bytes, `TypeError: fetch failed` (the
    // e59 "no error, no network bytes" signature). Post-fix the request
    // rides the inherited posture and reaches the fixture.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && fixture.hit_count("/marker/pop-fetch") == 0 {
        pool.paint_pages_needing_repaint();
        let _ = pool.init_pending_pages();
        runtime.pump_cdp(Duration::from_millis(100));
    }
    assert!(
        fixture.hit_count("/marker/pop-fetch") > 0,
        "popup fetch must reach the fixture over TLS (e63 D2 egress face) — \
         hits: {:?}",
        fixture.hits()
    );

    // Same-harness control: the OPENER page's fetch (its ScriptThread pump is
    // the proven fetch_axis B-axis shape) must settle ok — proves the
    // posture-inheritance fix end-to-end (request + response + Promise
    // settlement) for a runtime-fetch call, not just egress.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut opener_state = "pending".to_string();
    while Instant::now() < deadline {
        if let Ok(v) = page.evaluate_js_web("String(globalThis.__op && __op.state)") {
            let t = v.trim().trim_matches('"').to_string();
            if t.starts_with("ok:") || t.starts_with("err:") {
                opener_state = t.replace("%3A", ":");
                break;
            }
        }
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        opener_state.starts_with("ok:"),
        "opener-page runtime fetch must settle ok under ignore_certificate_errors \
         — got `{opener_state}`"
    );

    // D2 settle face: the popup's own fetch must settle ok (servo fetch →
    // bridge → honors the ignore posture end-to-end) and report over the
    // sync-XHR marker channel. The current popup document may be the SECOND
    // load (the popup double-loads — a known navigation/lifecycle face in
    // the e47/e62 domain); whichever load's realm the page currently carries
    // issues its own fetch, which must settle.
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut pop_state = "pending".to_string();
    while Instant::now() < deadline {
        pool.paint_pages_needing_repaint();
        let _ = pool.init_pending_pages();
        if let Ok(v) = popup.evaluate_js_web(
            "String(globalThis.__pop && __pop.state ? __pop.state : 'pending')",
        ) {
            let t = v.trim().trim_matches('"').to_string();
            if t.starts_with("ok:") || t.starts_with("err:") {
                pop_state = t.replace("%3A", ":");
                break;
            }
        }
        let _ = popup.evaluate_js("");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        pop_state.starts_with("ok:"),
        "popup fetch must settle ok under ignore_certificate_errors (servo \
         fetch owns the page transport) — got `{pop_state}`; hits: {:?}",
        fixture.hits()
    );
    assert!(
        fixture.hit_count("/marker/pop-rep") > 0,
        "popup script must report its fetch outcome over sync XHR — \
         hits: {:?}",
        fixture.hits()
    );

    eprintln!(
        "[https_popup_fetch] hits={:?} opener_state={opener_state} pop_state={pop_state}",
        fixture.hits()
    );

    pool.close_all();
}
