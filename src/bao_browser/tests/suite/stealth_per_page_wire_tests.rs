// @trace REQ-STL-001 [req:REQ-STL-001] [level:integration]
// BUN-EVOLUTION R53-A net-face live coverage: per-WebViewId stealth wire
// configuration (TLS + H2). Before R53-A the TLS wire config
// (`connector.rs STEALTH_TLS_CONFIG`) and the h2 fingerprint snapshot
// (`bao_stealth GLOBAL_HTTP2_FINGERPRINT`) were process-global
// LAST-WRITE-WINS: with two pages carrying different
// `PageConfig.stealth_profile`s, BOTH pages' egress rode the LAST-created
// page's fingerprint — silent cross-contamination inside a single runtime.
//
//   ① `per_page_divergent_wire_profiles_live` — two pages (Firefox / Chrome
//      profiles) in ONE runtime; each page's own subresource egress must
//      present ITS profile's ClientHello (profile-derived supported-groups
//      anchor — Firefox keeps P-521=25, Chrome stops at P-384=24) and the
//      two pages' JA3 strings must DIFFER. Before the fix both presented
//      the last page's profile (RED symptom: identical JA3 / wrong anchor).
//
//   ② `sw_egress_rides_host_page_profile_under_divergence_live` — the
//      SW-realm forwarded fetch (`respondWith(fetch(https://…))`) must ride
//      the REGISTERING page's profile even while a second page runs a
//      different profile (the R53-A ownership ruling: SW/worker-realm
//      egress belongs to its host page, the ScopeThings webview_id
//      inheritance made explicit on the net Request). Asserts the SW
//      ClientHello matches the host page's Firefox anchor and NOT the
//      coexisting Chrome page's.
//
// Environment gating (same as sw_stealth_profile_tests): real servo
// rendering requires DISPLAY (Xvfb) and network I/O.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_per_page)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;
use common::client_hello::{CaptureServer, ClientHello};

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig};
use bao_stealth::StealthProfile;

/// Serializes servo-touching phases inside one isolated test process.
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

/// Drive servo's ScriptThread with a no-op evaluate (the pump pattern every
/// live harness here uses) so page-realm async dispatches progress.
fn pump(page: &bao_browser::PageHandle, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Dispatch `navigator.serviceWorker.register(path)` recording the promise
/// outcome in `window.__swReg`; poll-read afterwards (same shape as
/// sw_stealth_profile_tests).
fn register_sw(page: &bao_browser::PageHandle, path: &str) -> String {
    let js = format!(
        "window.__swReg = 'pending'; \
         try {{ \
           navigator.serviceWorker.register('{path}').then( \
             function () {{ window.__swReg = 'ok'; }}, \
             function (e) {{ window.__swReg = 'error:' + e; }}); \
         }} catch (err) {{ window.__swReg = 'threw:' + err; }} \
         window.__swReg"
    );
    let _ = page.evaluate_js_web(&js);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let settled = match page.evaluate_js_web("window.__swReg") {
            Ok(s) if s.contains("pending") => None,
            Ok(s) => Some(s),
            Err(_) => None,
        };
        if let Some(s) = settled {
            return s;
        }
        if Instant::now() > deadline {
            return "<no settlement>".to_string();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Expected JA3 supported-groups field for a profile: the profile's group
/// ids minus FFDHE (0x0100..=0x010D), which the shared BoringSSL builder
/// drops (no BoringSSL implementation — connector.rs notes). Firefox keeps
/// secp521r1 (25) → "8-29-23-24-25"; Chrome stops at secp384r1 →
/// "6-29-23-24" (the leading entry is the extension's 2-byte list length
/// parsed as a group id — the suite-wide `ja3_string` convention, which
/// cancels out in equality comparisons; here it is mirrored explicitly so
/// absolute anchors work).
fn expected_curves_field(profile: &StealthProfile) -> String {
    let groups: Vec<String> = profile
        .tls
        .supported_groups
        .iter()
        .copied()
        .filter(|id| !(0x0100..=0x010D).contains(id))
        .map(|id| id.to_string())
        .collect();
    format!("{}-{}", groups.len() * 2, groups.join("-"))
}

// ---------------------------------------------------------------------------
// ClientHello wire capture (record layer -> handshake -> body) — shared
// stack in common/client_hello.rs (e158b M3: was the suite-local copy).
// ---------------------------------------------------------------------------

/// Dispatch an `<img>` subresource to `https://127.0.0.1:<port>/<path>` on
/// the page (the proven egress shape — img/css/fetch/xhr all ride the same
/// bridge + stealth SSLConfig).
fn dispatch_img(page: &bao_browser::PageHandle, port: u16, path: &str) {
    let js = format!(
        "(function() {{ var im = document.createElement('img'); \
           im.onerror = function(){{}}; im.onload = function(){{}}; \
           document.body.appendChild(im); \
           im.src = 'https://127.0.0.1:{port}/{path}'; }})()"
    );
    let _ = page.evaluate_js_web(&js);
}

/// Wait (bounded) for `capture` to record at least one ClientHello, pumping
/// the page meanwhile.
fn await_hello(
    page: &bao_browser::PageHandle,
    capture: &CaptureServer,
    what: &str,
) -> ClientHello {
    let deadline = Instant::now() + Duration::from_secs(20);
    while capture.count() == 0 {
        assert!(
            Instant::now() <= deadline,
            "① no ClientHello captured for {what}: errors={:?}",
            capture.errors()
        );
        pump(page, 100);
    }
    capture
        .first_parsed()
        .unwrap_or_else(|| panic!("{what}: ClientHello captured but failed to parse"))
}

// ═══════════════════════════════════════════════════════════════════════════
// ① Two pages, divergent profiles — each page's egress rides ITS profile
// ═══════════════════════════════════════════════════════════════════════════

/// @trace REQ-STL-001 [criterion:REQ-STL-001] per-page stealth wire profile
/// isolation (R53-A net face, live wire capture)
///
/// ONE runtime, TWO pages: page F carries the Firefox profile, page C the
/// Chrome profile. Each page dispatches an https subresource to its own
/// capture server. The R53-A contract holds iff:
///   1. page F's ClientHello carries the FIREFOX supported-groups anchor
///      ("8-29-23-24-25" — P-521 present),
///   2. page C's carries the CHROME anchor ("6-29-23-24" — no P-521),
///   3. the two JA3 strings differ,
///   4. both ALPN lists offer `h2,http/1.1` (no h1 downgrade on either).
/// Before the fix both pages rode the LAST-installed profile (identical
/// JA3, wrong anchor on one side) — that is the RED this test pins.
#[test]
fn per_page_divergent_wire_profiles_live() {
    if !common::run_isolated(
        "stealth_per_page_wire_tests::per_page_divergent_wire_profiles_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // The bun bridge HTTPThread's per-thread configure asserts an output
    // sink (same leg as page_net_bun_fingerprint_e2e_tests).
    bun_core::Output::init_test();

    let capture_f = CaptureServer::spawn();
    let capture_c = CaptureServer::spawn();

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");

    // Page F FIRST: its install must not be overwritten by page C's.
    let page_f = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page F must succeed");
    pump(&page_f, 300);

    let page_c = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page C must succeed");
    pump(&page_c, 300);

    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    let ff_curves = expected_curves_field(&firefox);
    let ch_curves = expected_curves_field(&chrome);
    assert_ne!(
        ff_curves, ch_curves,
        "test precondition: Firefox/Chrome profiles must diverge on the wire \
         (supported groups)"
    );

    dispatch_img(&page_f, capture_f.port, "page_f_probe");
    let hello_f = await_hello(&page_f, &capture_f, "page F (Firefox profile)");
    eprintln!(
        "[per-page] page F ja3={} curves={}",
        hello_f.ja3_string(),
        hello_f.curves_field()
    );

    dispatch_img(&page_c, capture_c.port, "page_c_probe");
    let hello_c = await_hello(&page_c, &capture_c, "page C (Chrome profile)");
    eprintln!(
        "[per-page] page C ja3={} curves={}",
        hello_c.ja3_string(),
        hello_c.curves_field()
    );

    // THE per-page assertions (R53-A): each page rides ITS OWN profile.
    assert_eq!(
        hello_f.curves_field(),
        ff_curves,
        "① R53-A VIOLATION: page F (installed FIRST, Firefox profile) must \
         present the Firefox supported groups ({ff_curves}), got \
         {} — a later Chrome page's install overwrote it (process-global \
         last-write-wins)",
        hello_f.curves_field()
    );
    assert_eq!(
        hello_c.curves_field(),
        ch_curves,
        "① R53-A VIOLATION: page C (installed LAST, Chrome profile) must \
         present the Chrome supported groups ({ch_curves}), got {}",
        hello_c.curves_field()
    );
    assert_ne!(
        hello_f.ja3_string(),
        hello_c.ja3_string(),
        "① R53-A VIOLATION: two divergent-profile pages must present \
         different JA3 fingerprints — identical JA3 means one process-global \
         wire config served both"
    );
    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(hello_f.alpn_protocols(), h2_h1, "① page F ALPN must be h2,http/1.1");
    assert_eq!(
        hello_c.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "① page C ALPN must be h2,http/1.1"
    );

    capture_f.stop();
    capture_c.stop();
    eprintln!("[per-page] === ① GREEN: per-page divergent wire profiles isolated ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ② SW-realm forwarded fetch rides the REGISTERING page's profile while a
//    divergent-profile page coexists
// ═══════════════════════════════════════════════════════════════════════════

/// Minimal plain-HTTP fixture for the SW origin (shared skeleton in
/// common/http_fixture.rs): `/` → page HTML, `/sw.js` → the templated SW
/// script (same shape as sw_stealth_profile_tests' fixture, trimmed to the
/// two paths used here).
fn spawn_sw_origin_fixture() -> common::http_fixture::HttpFixture {
    common::http_fixture::HttpFixture::spawn(
        "sw-origin-fixture",
        Arc::new(|path: &str, script: Option<&str>| -> (&'static str, String) {
            if path.starts_with("/sw.js") {
                // Unset script serves an empty JS body (unwrap_or_default).
                ("application/javascript", script.unwrap_or("").to_string())
            } else {
                (
                    "text/html",
                    "<html><body>sw origin fixture</body></html>".to_string(),
                )
            }
        }),
    )
}
/// @trace REQ-BRW-004 [criterion:19] [level:integration] SW-realm egress
/// rides the REGISTERING page's stealth profile under multi-page profile
/// divergence (R53-A ownership ruling, live wire capture)
///
/// Page F (Firefox, SW origin) registers a SW whose fetch handler forwards
/// `/api/forward` to an https capture server. Page C (Chrome, about:blank)
/// coexists in the same runtime — created AFTER page F so the pre-R53
/// process-global fallback bucket holds the Chrome profile. The ruling
/// holds iff the SW-realm ClientHello carries the REGISTERING page's
/// Firefox anchor ("8-29-23-24-25"), NOT the Chrome one.
#[test]
fn sw_egress_rides_host_page_profile_under_divergence_live() {
    if !common::run_isolated(
        "stealth_per_page_wire_tests::sw_egress_rides_host_page_profile_under_divergence_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let fixture = spawn_sw_origin_fixture();
    let origin = format!("http://127.0.0.1:{}/", fixture.port);
    let capture_sw = CaptureServer::spawn();
    fixture.set_script(format!(
        "self.addEventListener('fetch', function (e) {{ \
           var u = String(e.request.url); \
           if (u.indexOf('/api/forward') !== -1) {{ \
             e.respondWith(fetch('https://127.0.0.1:{port}/sw_forwarded')); \
           }} \
         }});",
        port = capture_sw.port
    ));

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");

    // Host page FIRST (Firefox) — the SW's owner.
    let page_f = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page F must succeed");
    pump(&page_f, 500);

    // Divergent page C (Chrome) AFTER — its install is the process-global
    // fallback, exactly the contamination source R53-A removes.
    let page_c = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page C must succeed");
    pump(&page_c, 300);

    let reg = register_sw(&page_f, "/sw.js");
    eprintln!("[sw-egress] register outcome = {reg}");
    assert!(
        reg.contains("ok"),
        "② serviceWorker.register must resolve ok (live-path prerequisite), got: {reg}"
    );

    // Trigger the SW-forwarded fetch (retry-bounded: activation is
    // asynchronous and pre-activation attempts pass through natively).
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let xhr_js = "(function() { try { \
             var x = new XMLHttpRequest(); \
             x.open('GET', '/api/forward'); x.send(); \
           } catch (e) {} })()";
        let _ = page_f.evaluate_js_web(xhr_js);
        let inner_deadline = Instant::now() + Duration::from_secs(2);
        while capture_sw.count() == 0 && Instant::now() < inner_deadline && Instant::now() < deadline
        {
            pump(&page_f, 100);
        }
        if capture_sw.count() > 0 || Instant::now() > deadline {
            break;
        }
    }
    assert!(
        capture_sw.count() > 0,
        "② no ClientHello captured for the SW realm's forwarded fetch — the \
         SW sub-fetch never egressed: errors={:?}",
        capture_sw.errors()
    );
    let sw_hello = capture_sw
        .first_parsed()
        .expect("SW-forwarded ClientHello parsed");
    eprintln!(
        "[sw-egress] SW-forwarded ja3={} curves={}",
        sw_hello.ja3_string(),
        sw_hello.curves_field()
    );

    let firefox = StealthProfile::firefox_default();
    let ff_curves = expected_curves_field(&firefox);
    let ch_curves = expected_curves_field(&StealthProfile::chrome_default());

    // THE ownership assertion: the SW realm rides the REGISTERING page's
    // (Firefox) wire profile, not the coexisting Chrome page's and not a
    // stealth-free ClientHello.
    assert_eq!(
        sw_hello.curves_field(),
        ff_curves,
        "② R53-A OWNERSHIP VIOLATION: the SW-realm forwarded fetch must ride \
         the REGISTERING page's Firefox profile ({ff_curves}), got {} — \
         riding the coexisting Chrome page's profile or no stealth at all \
         means the SW egress did not resolve to its host page",
        sw_hello.curves_field()
    );
    assert_ne!(
        sw_hello.curves_field(),
        ch_curves,
        "② SW egress must NOT carry the Chrome profile of a page that never \
         registered this worker"
    );
    assert_eq!(
        sw_hello.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "② SW-forwarded ALPN must be h2,http/1.1 (stealth H2 profile, not an \
         h1 downgrade bypass)"
    );

    let _ = page_c.evaluate_js("");
    capture_sw.stop();
    eprintln!("[sw-egress] === ② GREEN: SW egress rides the host page profile under divergence ===");
}

// ═══════════════════════════════════════════════════════════════════════════
// ③ fetch() egress rides the CALLING page's profile under divergence
//    (R53-A fetch-face keyed-per-Realm resolution)
// ═══════════════════════════════════════════════════════════════════════════

/// Dispatch a page-realm `fetch('https://…')` probe (window.fetch is the
/// bao_runtime fetch stack: `fetch_api::fetch_fn` resolves the profile,
/// `fetch_async` drives the bun HTTPThread with the stealth SSLConfig — the
/// ClientHello is already on the wire when the handshake fails).
fn dispatch_fetch(page: &bao_browser::PageHandle, port: u16, path: &str) {
    let js = format!(
        "(function() {{ window.__fetchState = 'pending'; \
           fetch('https://127.0.0.1:{port}/{path}') \
             .then(function(r) {{ window.__fetchState = 'ok:' + r.status; }}, \
                   function(e) {{ window.__fetchState = 'err:' + e; }}); }})()"
    );
    let _ = page.evaluate_js_web(&js);
}

/// @trace REQ-STL-001 [criterion:REQ-STL-001] [level:integration] per-page
/// stealth wire profile isolation on the fetch() face (R53-A fetch keyed
/// resolution, live wire capture)
///
/// ONE runtime, TWO pages (Firefox installed FIRST, Chrome SECOND — both
/// share one servo ScriptThread). Each page dispatches window.fetch to its
/// own capture server. The contract holds iff:
///   1. page F's fetch ClientHello carries the FIREFOX supported-groups
///      anchor ("8-29-23-24-25" — P-521 present),
///   2. page C's carries the CHROME anchor ("6-29-23-24"),
///   3. the two JA3 strings differ.
/// Before the keyed fetch resolution the profile came from the
/// LAST-INSTALL-WINS thread-local (`TL_STEALTH_PROFILE`): page F (installed
/// first) presented page C's Chrome fingerprint — that is the RED this test
/// pins (T14 control: both pages' JA3 identical / page F wrong anchor).
#[test]
fn stealth_per_page_fetch_profiles_live() {
    if !common::run_isolated("stealth_per_page_wire_tests::stealth_per_page_fetch_profiles_live") {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let capture_f = CaptureServer::spawn();
    let capture_c = CaptureServer::spawn();

    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");

    // Page F FIRST — its fetch must not ride page C's later install.
    let page_f = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page F must succeed");
    pump(&page_f, 300);

    let page_c = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page C must succeed");
    pump(&page_c, 300);

    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    let ff_curves = expected_curves_field(&firefox);
    let ch_curves = expected_curves_field(&chrome);
    assert_ne!(
        ff_curves, ch_curves,
        "test precondition: Firefox/Chrome profiles must diverge on the wire \
         (supported groups)"
    );

    dispatch_fetch(&page_f, capture_f.port, "fetch_probe_f");
    let hello_f = await_hello(&page_f, &capture_f, "page F fetch (Firefox profile)");
    eprintln!(
        "[fetch-per-page] page F fetch ja3={} curves={}",
        hello_f.ja3_string(),
        hello_f.curves_field()
    );

    dispatch_fetch(&page_c, capture_c.port, "fetch_probe_c");
    let hello_c = await_hello(&page_c, &capture_c, "page C fetch (Chrome profile)");
    eprintln!(
        "[fetch-per-page] page C fetch ja3={} curves={}",
        hello_c.ja3_string(),
        hello_c.curves_field()
    );

    // THE fetch-face assertions (R53-A): each page's fetch rides ITS OWN
    // profile, not the last-installed page's thread-local leftover.
    assert_eq!(
        hello_f.curves_field(),
        ff_curves,
        "③ R53-A FETCH VIOLATION: page F (installed FIRST, Firefox profile) \
         fetch must present the Firefox supported groups ({ff_curves}), got \
         {} — the last-install-wins thread-local served page C's Chrome \
         profile to page F's egress",
        hello_f.curves_field()
    );
    assert_eq!(
        hello_c.curves_field(),
        ch_curves,
        "③ R53-A FETCH VIOLATION: page C (installed LAST, Chrome profile) \
         fetch must present the Chrome supported groups ({ch_curves}), got {}",
        hello_c.curves_field()
    );
    assert_ne!(
        hello_f.ja3_string(),
        hello_c.ja3_string(),
        "③ R53-A FETCH VIOLATION: two divergent-profile pages' fetches must \
         present different JA3 fingerprints — identical JA3 means one \
         thread-local profile served both pages"
    );
    let h2_h1: Vec<Vec<u8>> = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    assert_eq!(hello_f.alpn_protocols(), h2_h1, "③ page F fetch ALPN must be h2,http/1.1");
    assert_eq!(
        hello_c.alpn_protocols(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        "③ page C fetch ALPN must be h2,http/1.1"
    );

    capture_f.stop();
    capture_c.stop();
    eprintln!("[fetch-per-page] === ③ GREEN: fetch egress rides the calling page profile ===");
}

/// Keyed fetch-face contract pin (no servo needed — mirrors the
/// compartment_isolation_tests scenario-3 simulation idiom: two "pages"
/// registered on one logical thread, distinct globals, distinct profiles).
///
/// `profile_for_global` is the fetch read point's authoritative source
/// (fetch_api::current_fetch_profile), so it must hand back the FULL
/// registration-time profile — including the TLS/HTTP2 wire face the flat
/// RealmProfile projection does not model:
///   1. each keyed global resolves to ITS profile's wire anchor (the Firefox
///      P-521 groups vs the Chrome list),
///   2. a Node-Realm-style alias resolves to the registering page's full
///      wire profile,
///   3. an unregistered global resolves to None (the caller's TLS fallback
///      contract),
///   4. the resolved profile is wire-equivalent to the registered one
///      (cipher suites + curves + sigalgs + ALPN round-trip).
#[test]
fn fetch_profile_keyed_getter_resolves_wire_face() {
    // Process-global store: nextest gives every test its own process, but
    // start clean regardless (direct re-runs, libtest harnesses).
    bao_stealth::engine_props::clear_all_realm_profiles();

    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    let page_a_global = 0x0005_AAAA_0000usize;
    let page_b_global = 0x0005_BBBB_0000usize;
    let node_realm_global = 0x0005_C0DE_0000usize;

    bao_stealth::engine_props::set_profile_for_global(page_a_global, &firefox);
    bao_stealth::engine_props::set_profile_for_global(page_b_global, &chrome);

    // ① keyed hit: each global resolves to its own FULL profile.
    let got_a = bao_stealth::engine_props::profile_for_global(page_a_global)
        .expect("keyed page A must resolve its registered profile");
    let got_b = bao_stealth::engine_props::profile_for_global(page_b_global)
        .expect("keyed page B must resolve its registered profile");
    assert_eq!(
        got_a.tls.supported_groups, firefox.tls.supported_groups,
        "page A keyed resolution must carry the Firefox TLS wire face"
    );
    assert_eq!(
        got_b.tls.supported_groups, chrome.tls.supported_groups,
        "page B keyed resolution must carry the Chrome TLS wire face"
    );
    assert_ne!(
        got_a.tls.supported_groups, got_b.tls.supported_groups,
        "the two pages' keyed wire faces must diverge (Firefox keeps P-521)"
    );
    assert_eq!(
        got_a.http2.header_table_size, firefox.http2.header_table_size,
        "keyed resolution must carry the HTTP/2 face, not just TLS"
    );

    // ② alias: the Node Realm global rides the registering page's profile.
    bao_stealth::engine_props::register_global_alias(page_a_global, node_realm_global);
    let got_alias = bao_stealth::engine_props::profile_for_global(node_realm_global)
        .expect("aliased Node Realm global must resolve the page profile");
    assert_eq!(
        got_alias.tls.supported_groups, firefox.tls.supported_groups,
        "Node Realm alias must carry the registering page's FULL wire profile"
    );

    // ③ miss: unregistered global → None (caller falls back to its TLS).
    assert!(
        bao_stealth::engine_props::profile_for_global(0x0005_DEAD_0000).is_none(),
        "unregistered global must resolve None — the fetch read point's \
         TLS fallback contract"
    );

    // ④ wire round-trip: the resolved profile yields the same JA3 wire
    // inputs as the registered one (cipher suites / curves / sigalgs).
    let expected_curves: Vec<u16> = firefox
        .tls
        .supported_groups
        .iter()
        .copied()
        .filter(|id| !(0x0100..=0x010D).contains(id))
        .collect();
    assert_eq!(
        got_a.tls.supported_groups.iter().copied().filter(|id| !(0x0100..=0x010D).contains(id))
            .collect::<Vec<u16>>(),
        expected_curves,
        "keyed resolution wire round-trip: supported groups (FFDHE-filtered) \
         must equal the registered profile's"
    );
    assert_eq!(
        got_a.tls.cipher_suites, firefox.tls.cipher_suites,
        "keyed resolution wire round-trip: cipher suites must equal the \
         registered profile's"
    );
    assert_eq!(
        got_a.tls.signature_algorithms, firefox.tls.signature_algorithms,
        "keyed resolution wire round-trip: signature algorithms must equal \
         the registered profile's"
    );
    assert_eq!(
        got_a.tls.alpn_protocols, firefox.tls.alpn_protocols,
        "keyed resolution wire round-trip: ALPN must equal the registered \
         profile's"
    );

    bao_stealth::engine_props::clear_all_realm_profiles();
}
