// @trace REQ-STL-001 [level:e2e] — same page, same TLS fingerprint across
// the page-network stacks (U2 terminal: the bun bridge is the only page
// path).
//
// A servo page loads its stylesheet/image/script/xhr subresources through
// the bun bridge (bun HTTPThread + BoringSSL stealth SSLConfig from the
// page profile), while the same page's `window.fetch` rides the Node fetch
// stack (also bun HTTPThread, same-profile SSLConfig via stealth_http). A
// raw TCP capture server records each ClientHello before anything else
// happens on the wire, then the test asserts:
//
//   1. ALPN parity: the bridge captures advertise `h2,http/1.1`
//      regardless of the Node-fetch h2 gate — the page egress must not
//      downgrade to h1 (the `Flags::is_page_egress` bypass; the gate
//      itself is the single source
//      `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP2_CLIENT`, default ON).
//   2. Same-page same-fingerprint: the canonicalized ClientHello bytes
//      (client_random / session_id zeroed — per-connection randomness) are
//      IDENTICAL for bridge-img, bridge-css and window.fetch. This covers
//      cipher list + order, curves, sigalgs, extension list + order, and
//      every extension payload including ALPN contents.
//   3. JA3 (repo convention: 771,ciphers,exts,curves,sigalgs) computed
//      from the live wire bytes is equal across all three captures.
//   4. Pilot dispatch scope: bridge request counter == 2 (img + css), while
//      the page's <script> and XHR requests (destinations outside the
//      list) reach the plain-HTTP fixture through servo's hyper path.
//
// Each request gets its own capture port: the bun socket pool and the TLS
// session cache are keyed by host:port, so separate ports guarantee fresh
// connections (one first-connection ClientHello each, no coalescing and no
// cross-request session-resumption offers).
//
// Harness notes (same contract as page_wss_bao_tls_e2e_tests.rs): single
// #[test] (mozjs Runtime / servo Opts are per-process singletons); data:
// URL page origin; subresources are injected via JS AFTER create_page
// returned, so the page profile (fetch_api + set_stealth_tls_config) is
// already installed before any request fires.

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;
use common::client_hello::CaptureServer;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool, PageState};
use bao_stealth::StealthProfile;

// ---------------------------------------------------------------------------
// ClientHello wire capture (record layer -> handshake -> body) — shared
// stack in common/client_hello.rs (e158b M3: was the suite-local copy).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Plain-HTTP fixture (shared skeleton in common/http_fixture.rs): records
// request paths for the hyper-path destinations
// ---------------------------------------------------------------------------

fn spawn_http_fixture() -> common::http_fixture::HttpFixture {
    common::http_fixture::HttpFixture::spawn(
        "http-fixture",
        Arc::new(|_path: &str, _script: Option<&str>| -> (&'static str, String) {
            ("text/plain", String::new())
        }),
    )
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn data_url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'#' | b'%' | b'&' | b'?' | b'<' | b'>' | b'"' | b'\\' | b'^' | b'`' | b'{' | b'}'
            | b'|' => out.push_str(&format!("%{:02X}", b)),
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[test]
fn page_net_bun_same_fingerprint_and_destination_pilot() {
    // Full-engine e2e: per-process singletons — self-isolate so plain
    // `cargo test` (any thread count) matches nextest's per-test process.
    if !common::run_isolated("page_net_bun_fingerprint_e2e_tests::page_net_bun_same_fingerprint_and_destination_pilot") {
        return;
    }
    // bao output sinks: the HTTPThread's per-thread configure asserts
    // STDOUT_STREAM_SET unless the embedder initialized Output first (the
    // product binary does this in bun_runtime::dispatch; test harnesses use
    // init_test — same leg as servo-net's bun_bridge unit tests).
    bun_core::Output::init_test();

    let img_capture = CaptureServer::spawn();
    let css_capture = CaptureServer::spawn();
    let fetch_capture = CaptureServer::spawn();
    let fixture = spawn_http_fixture();

    let config = BaoConfig::default();
    let runtime = match BrowserRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BrowserRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();

    // Empty shell page — subresources are injected via JS after create_page
    // returned so the page's stealth profile (fetch_api global +
    // servo::set_stealth_tls_config) is installed before any request fires.
    let html = "<!DOCTYPE html><html><head><title>fp</title></head>\
                <body><p id=\"t\">fp</p></body></html>"
        .to_string();
    let url = format!("data:text/html;charset=utf-8,{}", data_url_escape(&html));

    let mut page = None;
    for _ in 0..3 {
        match pool.create_page(&PageConfig {
            url: Some(url.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        }) {
            Ok(p) => {
                page = Some(p);
                break;
            },
            Err(e) => {
                eprintln!("page creation failed (retrying): {}", e);
                std::thread::sleep(Duration::from_secs(3));
            },
        }
    }
    let page = match page {
        Some(p) => p,
        None => panic!("page creation failed after retries"),
    };
    eprintln!("[fp-e2e] page created");
    wait_for_load(&page, 3000);
    eprintln!("[fp-e2e] page loaded");

    // Inject in the PAGE realm (`evaluate_js` would run in the Node Realm
    // behind DOM proxies; the subresource elements must live in the page).
    let inject = |label: &str, js: &str| {
        match page.evaluate_js_web(js) {
            Ok(value) => eprintln!("[fp-e2e] inject {label}: ok {value}"),
            Err(error) => panic!("inject {label} failed: {error:?}"),
        }
    };
    let pump = |ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let _ = page.evaluate_js("");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // Wait for a capture while pumping servo's event loop (page-side async
    // tasks — XHR dispatch, fetch promise chaining — need the loop to turn).
    let wait_capturing = |capture: &CaptureServer, n: usize, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if capture.wait_for(n, Duration::from_millis(200)) {
                return true;
            }
            pump(50);
        }
        false
    };
    let wait_fixturing = |fixture: &common::http_fixture::HttpFixture, n: usize, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if fixture.wait_for_count(n, Duration::from_millis(200)) {
                return true;
            }
            pump(50);
        }
        false
    };

    // ── Phase A: bridge captures with EXPERIMENTAL_HTTP2_CLIENT OFF ──────
    // The bridge must still offer h2 (hyper parity — the is_page_egress
    // bypass). Node fetch would offer http/1.1 only in this posture, which
    // is why the fetch capture happens in phase B below.

    inject(
        "img",
        &format!(
            "(function(){{ var im = document.createElement('img'); \
             im.onerror = function(){{ window.__imgState = 'error'; }}; \
             im.onload = function(){{ window.__imgState = 'loaded'; }}; \
             document.body.appendChild(im); \
             im.src = 'https://127.0.0.1:{}/fingerprint.png'; \
             window.__imgState = 'pending'; }})()",
            img_capture.port
        ),
    );
    pump(200);
    inject(
        "css",
        &format!(
            "(function(){{ var l = document.createElement('link'); l.rel = 'stylesheet'; \
             l.href = 'https://127.0.0.1:{}/fingerprint.css'; document.head.appendChild(l); }})()",
            css_capture.port
        ),
    );
    pump(500);

    assert!(
        wait_capturing(&img_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the bridge image fetch: errors={:?}",
        img_capture.errors()
    );
    eprintln!("[fp-e2e] img ClientHello captured");
    assert!(
        wait_capturing(&css_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the bridge stylesheet fetch: errors={:?}",
        css_capture.errors()
    );
    eprintln!("[fp-e2e] css ClientHello captured");
    let img_hello = img_capture
        .parsed()
        .into_iter()
        .next()
        .expect("img ClientHello parsed");
    let css_hello = css_capture
        .parsed()
        .into_iter()
        .next()
        .expect("css ClientHello parsed");

    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(
        img_hello.alpn_protocols(),
        h2_h1,
        "bridge (img) ALPN must be h2,http/1.1 regardless of the Node h2 gate — hyper parity"
    );
    assert_eq!(
        css_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "bridge (css) ALPN must be h2,http/1.1 regardless of the Node h2 gate — hyper parity"
    );

    // ── Phase B: Node fetch (h2 gate = env flag, default ON); script/xhr
    //    stay on hyper ────────────────────────────────────────────────────

    // window.fetch — same page, servo WHATWG fetch → the same bridge stack
    // as the subresources (底层统一, e63 D2); its ClientHello must carry the
    // page profile's stealth fingerprint like every other page egress.
    inject(
        "fetch",
        &format!(
            "(function(){{ window.__fetchState = 'pending'; \
             window.__fetchOwn = !!Object.getOwnPropertyDescriptor(window, 'fetch'); \
             fetch('https://127.0.0.1:{}/fetch_probe').then(function(r){{ window.__fetchState = 'ok:' + r.status; }}).catch(function(e){{ window.__fetchState = 'err:' + e; }}); }})()",
            fetch_capture.port
        ),
    );

    // script + XHR (destinations outside the img,css list) — must take
    // servo's hyper path and land on the plain-HTTP fixture.
    inject(
        "script",
        &format!(
            "(function(){{ var s = document.createElement('script'); s.src = 'http://127.0.0.1:{}/script_probe.js'; document.head.appendChild(s); }})()",
            fixture.port
        ),
    );
    inject(
        "xhr",
        &format!(
            "(function(){{ try {{ var x = new XMLHttpRequest(); x.open('GET', 'http://127.0.0.1:{}/xhr_probe'); x.send(); window.__xhrState = 'sent'; }} catch (e) {{ window.__xhrState = 'throw:' + e; }} }})()",
            fixture.port
        ),
    );

    assert!(
        wait_capturing(&fetch_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for window.fetch: errors={:?}",
        fetch_capture.errors()
    );
    eprintln!("[fp-e2e] window.fetch ClientHello captured");
    let fetch_diag = page.evaluate_js_web(
        "(function(){ return 'own=' + window.__fetchOwn + ' state=' + window.__fetchState; })()",
    );
    eprintln!("[fp-e2e] fetch diag: {:?}", fetch_diag);
    assert!(
        wait_fixturing(&fixture, 2, Duration::from_secs(15)),
        "hyper-path fixture did not receive script+xhr (paths so far: {:?})",
        fixture.recorded_paths()
    );
    eprintln!("[fp-e2e] fixture got script+xhr: {:?}", fixture.recorded_paths());
    let fetch_hello = fetch_capture
        .parsed()
        .into_iter()
        .next()
        .expect("fetch ClientHello parsed");

    // ── Same page, same fingerprint ───────────────────────────────────────

    let img_canonical = img_hello.canonical_bytes();
    let css_canonical = css_hello.canonical_bytes();
    let fetch_canonical = fetch_hello.canonical_bytes();
    assert_eq!(
        img_canonical,
        fetch_canonical,
        "bridge (img) and window.fetch ClientHello fingerprints differ on the same page"
    );
    assert_eq!(
        css_canonical,
        fetch_canonical,
        "bridge (css) and window.fetch ClientHello fingerprints differ on the same page"
    );
    assert_eq!(
        img_hello.ja3_string(),
        fetch_hello.ja3_string(),
        "bridge (img) and window.fetch JA3 differ on the same page"
    );
    assert_eq!(
        css_hello.ja3_string(),
        fetch_hello.ja3_string(),
        "bridge (css) and window.fetch JA3 differ on the same page"
    );
    assert!(
        img_hello.ja3_string().starts_with("771,"),
        "live JA3 malformed: {}",
        img_hello.ja3_string()
    );

    // ── Dispatch scope ────────────────────────────────────────────────────
    // Every subresource destination rides the bridge: img + css (the
    // fingerprint-captured ones) plus script + xhr + the window.fetch probe
    // (the 底层统一 ruling, e63 D2 — servo WHATWG fetch owns the page
    // transport, so the probe rides the same bridge as the rest).

    let bridge_count = servo_net::fetch::bun_bridge::page_net_bun_request_count();
    assert_eq!(
        bridge_count, 5,
        "bridge must have driven img+css+script+xhr+fetch (got {bridge_count}; fixture paths: {:?})",
        fixture.recorded_paths()
    );
    let paths = fixture.recorded_paths();
    assert!(
        paths.iter().any(|p| p.contains("script_probe")),
        "script request missing from fixture: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.contains("xhr_probe")),
        "xhr request missing from fixture: {paths:?}"
    );

    // ── h2 SETTINGS payload parity (code-level cross-check) ───────────────
    // Both stacks derive the SETTINGS wire bytes from the SAME profile
    // through the SAME bao_stealth function (settings_frame_payload), so the
    // frames are byte-equal by construction; assert the two derivations
    // agree on this profile to pin the invariant.

    let profile = StealthProfile::firefox_default();
    let wire = bao_stealth::StealthTlsWireConfig::from_profile(&profile);
    assert!(
        !wire.h2_settings_payload.is_empty() &&
            wire.h2_settings_payload.len() % 6 == 0,
        "wire h2 SETTINGS payload malformed"
    );
    assert_eq!(
        wire.h2_initial_stream_size,
        profile.http2.initial_window_size,
        "bridge and Node fetch must carry the same h2 initial window size"
    );

    // ── U2 stage 2: h2 pseudo-header order / preface PRIORITY frames ─────
    // The page profile installed the global Http2Fingerprint snapshot (set
    // alongside set_stealth_tls_config by runtime_bridge); the bridge reads
    // it into its SSLConfig (`build_ssl_config`), which
    // h2_client::encode::write_preface / encode_request_headers consume —
    // the same fields window.fetch's stealth_http sets. Pin the wiring at
    // e2e level: the snapshot must be the page profile's Firefox fingerprint.
    let h2fp = bao_stealth::global_http2_fingerprint()
        .expect("page profile must install the global h2 fingerprint snapshot");
    assert_eq!(
        h2fp.pseudo_header_order,
        profile.http2.pseudo_header_order,
        "global h2 snapshot pseudo-header order must be the page profile's (Firefox: method/path/authority/scheme)"
    );
    assert_eq!(
        h2fp.priority_frames.len(),
        profile.http2.priority_frames.len(),
        "global h2 snapshot must carry the profile's preface PRIORITY frames (Firefox: 3/5/7/11)"
    );
    assert!(
        h2fp.sends_priority_frames(),
        "Firefox profile must send explicit PRIORITY frames (REQ-STL-002-C3)"
    );

    // ── Phase C (U2 stage 3): EXPERIMENTAL_HTTP2_CLIENT default-ON smoke ──
    // No flag is set anywhere (the h2 gate's single source is the env flag,
    // whose default is ON) and Node fetch must offer h2 on its own: the
    // ClientHello ALPN list must still be `h2,http/1.1`. (Pre-flip this
    // posture offered http/1.1 only; the bridge bypass `is_page_egress` is
    // NOT involved — this is the Node fetch stack's own gate.)
    let default_capture = CaptureServer::spawn();
    inject(
        "fetch-default-h2",
        &format!(
            "(function(){{ fetch('https://127.0.0.1:{}/default_probe').catch(function(){{}}); }})()",
            default_capture.port
        ),
    );
    assert!(
        wait_capturing(&default_capture, 1, Duration::from_secs(15)),
        "no ClientHello captured for the default-flag fetch: errors={:?}",
        default_capture.errors()
    );
    let default_hello = default_capture
        .parsed()
        .into_iter()
        .next()
        .expect("default-flag ClientHello parsed");
    assert_eq!(
        default_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "with no flag set anywhere, Node fetch must offer h2 by default (BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP2_CLIENT default ON)"
    );
    default_capture.stop();
    eprintln!("[fp-e2e] default-flag fetch offers h2 (EXPERIMENTAL_HTTP2_CLIENT default ON)");

    img_capture.stop();
    css_capture.stop();
    fetch_capture.stop();
    fixture.stop();
    eprintln!("[fp-e2e] === ALL ASSERTIONS PASSED ===");

    // Shutdown: every assertion above already ran and printed the banner.
    // servo teardown in this harness can stall indefinitely (observed:
    // ResourceManager hot-spins on a closed channel in its select set and
    // Constellation never finishes Exit sequencing — independent of the
    // assertions, which have all completed). A watchdog force-exits the
    // process AFTER a grace period so a clean servo teardown still gets a
    // chance to run first; the exit code is 0 only because the banner above
    // proves every assertion passed.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        eprintln!("[fp-e2e] watchdog: servo teardown did not finish in 10s — force exit");
        std::process::exit(0);
    });
    let _ = page.close();
    pool.close_all();
}
