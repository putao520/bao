// @trace REQ-STL-003 [req:REQ-STL-003] [level:integration]
// BUN-EVOLUTION R53-A phase-2 canvas-face live coverage: per-WebViewId
// canvas noise configuration. Before R53-A phase 2 the canvas noise seed
// (`canvas_noise.rs` three process-global atomics, written by every page
// install via `servo::set_canvas_noise_seed`) was LAST-WRITE-WINS: with two
// pages carrying different `PageConfig.stealth_profile`s, BOTH pages'
// canvas readbacks rode the LAST-installed page's seed — silent
// cross-contamination inside a single runtime, and a stealth-free page
// created after a stealthed page inherited the stealthed page's noise
// (the paint-thread `GetImageData` choke point reads the global for
// every canvas regardless of owner).
//
//   ① `per_page_divergent_canvas_noise_live` — two pages (Firefox
//      canvas-seed 42 / Chrome canvas-seed 137) in ONE runtime; the same
//      64×64 flat-gray canvas drawn and read back through
//      `getImageData` on each page must yield DIFFERENT deterministic
//      digests (each page rides ITS OWN profile's seed). Before the fix
//      both readbacks rode the last page's seed → identical digests (RED).
//
//   ② `stealth_free_page_canvas_zero_noise_live` — a stealth-free page
//      (`stealth_profile: None`) created AFTER a stealthed page must read
//      back its canvas BYTE-EXACT (zero noise): the keyed registry's
//      explicit `None` entry keeps stealth-free pages stealth-free instead
//      of inheriting the earlier page's process-global seed (RED
//      symptom: flips > 0 on the stealth-free page).
//
//   ③ `worker_offscreencanvas_rides_host_page_profile_live` — an
//      OffscreenCanvas created INSIDE each page's Worker rides the HOST
//      page's profile (the R53-A ownership ruling: worker-realm canvases
//      belong to their owning page, identity threaded at
//      `CanvasState::new` via `GlobalScope::egress_webview_id`): the two
//      pages' worker digests must DIFFER and each worker's two readbacks
//      must be IDENTICAL (deterministic per seed). Before the fix both
//      workers rode the last-installed global seed → identical digests
//      (RED).
//
// All assertions are on the paint-thread noise layer ONLY (the JS-layer
// canvas hooks were removed in W2 — the `GetImageData` choke point is the
// single noise surface). A flat rgb(128,128,128) fill makes every channel
// land exactly on a u8 rounding boundary, so each byte encodes the SIGN of
// the deterministic noise at that coordinate: a 64×64 readback is a 12288-
// sample sign map — two seeds produce different maps with overwhelming
// probability, and zero noise is byte-exact all-128.
//
// Environment gating (same as stealth_per_page_wire_tests): real servo
// rendering requires DISPLAY (Xvfb).
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_per_page_canvas)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use bao_stealth::StealthProfile;

/// Serializes servo-touching tests inside one isolated test process.
static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

/// Skip-guard helper: returns true if the test should skip (no network/display).
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

/// The servo JS bridge may return a JS string value as `"..."` (quoted). Strip
/// a single outer quote pair if present so the caller sees the raw value.
fn unquote_bridge(mut s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        s = inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    s
}

/// Window-realm canvas probe: draw a flat rgb(128,128,128) 64×64 canvas and
/// read the FULL buffer back through `getImageData` (the W2 paint-thread
/// choke point). Digest = `OK:<flips>:<h>` where `flips` counts pixels with
/// ANY channel ≠ 128 (the noise sign map density) and `h` is a 32-bit
/// rolling hash over every byte (full-content discrimination). Zero noise ⇒
/// `OK:0:<flat-hash>` byte-exact; different seeds ⇒ different (flips, h).
const CANVAS_PROBE_JS: &str = r#"
(function () {
  try {
    var c = document.createElement('canvas');
    c.width = 64; c.height = 64;
    var x = c.getContext('2d');
    if (!x) { window.__canvasProbe = 'NULL-CTX'; return; }
    x.fillStyle = 'rgb(128,128,128)';
    x.fillRect(0, 0, 64, 64);
    var d = x.getImageData(0, 0, 64, 64).data;
    var flips = 0, h = 0;
    for (var i = 0; i < d.length; i += 4) {
      if (d[i] !== 128 || d[i+1] !== 128 || d[i+2] !== 128) { flips++; }
      h = (Math.imul(h, 31) + d[i] + d[i+1] * 7 + d[i+2] * 13) | 0;
    }
    window.__canvasProbe = 'OK:' + flips + ':' + h;
  } catch (e) {
    window.__canvasProbe = 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})()
"#;

/// Run the window-realm probe on `page`, returning the raw digest.
fn run_canvas_probe(page: &bao_browser::PageHandle) -> String {
    // The probe is an IIFE with no return value; the digest lands on
    // `window.__canvasProbe` synchronously — read THAT back (the dispatch
    // return is `undefined` by construction).
    let _ = page.evaluate_js_web(CANVAS_PROBE_JS);
    let digest = page
        .evaluate_js_web("window.__canvasProbe")
        .expect("canvas probe readback must succeed");
    let digest = unquote_bridge(digest);
    assert!(
        digest.starts_with("OK:") || digest.starts_with("THROW:") || digest.starts_with("NULL-CTX"),
        "canvas probe must leave an explicit marker, got: {digest}"
    );
    digest
}

/// Parse `OK:<flips>:<h>` into its flips count.
fn digest_flips(digest: &str) -> u64 {
    assert!(digest.starts_with("OK:"), "probe must have succeeded: {digest}");
    digest
        .trim_start_matches("OK:")
        .split(':')
        .next()
        .and_then(|f| f.parse().ok())
        .unwrap_or_else(|| panic!("malformed probe digest: {digest}"))
}

// ═══════════════════════════════════════════════════════════════════════
// ① Two pages, divergent canvas seeds — each page's readback rides ITS seed
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-STL-003 [criterion:REQ-STL-003] per-page canvas noise
/// isolation (R53-A phase 2, live paint-thread readback)
///
/// ONE runtime, TWO pages: page F (firefox_default, canvas seed 42) created
/// FIRST, page C (chrome_default, canvas seed 137) created LAST — page C's
/// install is exactly the process-global write that pre-R53 contaminated
/// every other page's canvas readback. The R53-A contract holds iff:
///   1. page F's two readbacks are identical (deterministic noise),
///   2. page C's two readbacks are identical,
///   3. page F's digest ≠ page C's digest (each rides its own seed),
///   4. both digests have flips > 0 (noise actually engaged on both).
/// Before the fix both readbacks rode the LAST-installed seed → identical
/// digests — that is the RED this test pins.
#[test]
fn per_page_divergent_canvas_noise_live() {
    if !common::run_isolated(
        "stealth_per_page_canvas_tests::per_page_divergent_canvas_noise_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

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

    // Probing F AFTER C's install is deliberate: pre-R53 that is the
    // contamination window (global already holds C's seed 137).
    let f1 = run_canvas_probe(&page_f);
    let f2 = run_canvas_probe(&page_f);
    let c1 = run_canvas_probe(&page_c);
    let c2 = run_canvas_probe(&page_c);
    eprintln!("[per-page-canvas] F: {f1} / {f2}");
    eprintln!("[per-page-canvas] C: {c1} / {c2}");

    assert_eq!(f1, f2, "① page F readback must be deterministic (same seed)");
    assert_eq!(c1, c2, "① page C readback must be deterministic (same seed)");
    assert_ne!(
        f1, c1,
        "① R53-A VIOLATION: two divergent-seed pages produced the SAME canvas \
         readback — one process-global noise seed served both (last-write-wins)"
    );
    assert!(
        digest_flips(&f1) > 0,
        "① page F's readback must carry ITS profile's noise (flips > 0), got {f1}"
    );
    assert!(
        digest_flips(&c1) > 0,
        "① page C's readback must carry ITS profile's noise (flips > 0), got {c1}"
    );

    eprintln!("[per-page-canvas] === ① GREEN: per-page canvas noise isolated ===");
}

// ═══════════════════════════════════════════════════════════════════════
// ② Stealth-free page after a stealthed page reads back byte-exact
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-STL-003 [criterion:REQ-STL-003] stealth-free page canvas
/// zero-noise under coexistence (R53-A phase 2 keyed explicit-None entry)
///
/// Page S (firefox, seed 42) then page N (`stealth_profile: None`) in ONE
/// runtime. Page N's canvas readback must be BYTE-EXACT zero noise
/// (flips == 0 on the flat fill) — the keyed registry's explicit `None`
/// entry — while page S's stays noisy (flips > 0). Pre-R53 the global kept
/// page S's seed 42 and page N's readback inherited it (RED: flips > 0 on
/// the stealth-free page).
#[test]
fn stealth_free_page_canvas_zero_noise_live() {
    if !common::run_isolated(
        "stealth_per_page_canvas_tests::stealth_free_page_canvas_zero_noise_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

    let page_s = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(StealthProfile::firefox_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page S must succeed");
    pump(&page_s, 300);

    let page_n = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: None,
            ..Default::default()
        })
        .expect("gated live test: create_page N must succeed");
    pump(&page_n, 300);

    let s1 = run_canvas_probe(&page_s);
    let n1 = run_canvas_probe(&page_n);
    let n2 = run_canvas_probe(&page_n);
    eprintln!("[stealth-free-canvas] S (firefox): {s1}");
    eprintln!("[stealth-free-canvas] N (no profile): {n1} / {n2}");

    assert!(
        digest_flips(&s1) > 0,
        "② precondition: the stealthed page's readback must be noisy, got {s1}"
    );
    assert_eq!(n1, n2, "② stealth-free readback must be deterministic");
    assert_eq!(
        digest_flips(&n1),
        0,
        "② R53-A VIOLATION: a stealth-free page's canvas readback must be \
         byte-exact (zero noise — keyed explicit-None entry), got {n1} — it \
         inherited an earlier stealthed page's process-global seed"
    );

    eprintln!("[stealth-free-canvas] === ② GREEN: stealth-free page reads back byte-exact ===");
}

// ═══════════════════════════════════════════════════════════════════════
// ③ Worker-realm OffscreenCanvas rides the HOST page's profile
// ═══════════════════════════════════════════════════════════════════════

/// Worker body: TWO independent OffscreenCanvas 64×48 flat-gray readbacks
/// (stability pair), digested with the same flips/hash scheme, joined by
/// `|`. `ABSENT:OffscreenCanvas` if the worker realm lacks the interface.
const WORKER_CANVAS_BODY: &str = r#"
var __r = (function () {
  function probe() {
    if (typeof OffscreenCanvas === 'undefined') { return 'ABSENT:OffscreenCanvas'; }
    var c = new OffscreenCanvas(64, 48);
    var ctx = c.getContext('2d');
    if (!ctx) { return 'NULL-CTX'; }
    ctx.fillStyle = 'rgb(128,128,128)';
    ctx.fillRect(0, 0, 64, 48);
    var d = ctx.getImageData(0, 0, 64, 48).data;
    var flips = 0, h = 0;
    for (var i = 0; i < d.length; i += 4) {
      if (d[i] !== 128 || d[i+1] !== 128 || d[i+2] !== 128) { flips++; }
      h = (Math.imul(h, 31) + d[i] + d[i+1] * 7 + d[i+2] * 13) | 0;
    }
    return 'OK:' + flips + ':' + h;
  }
  try {
    return probe() + '|' + probe();
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
self.postMessage(__r);
"#;

fn encode_worker_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            },
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            },
        }
    }
    out
}

/// Run the worker-realm OffscreenCanvas probe on `page`; polls the
/// `window.__wkCanvas` sink. Returns the raw `a|b` digest pair.
fn run_worker_canvas_probe(page: &bao_browser::PageHandle) -> String {
    let driver = format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onmessage = function (e) {{ window.__wkCanvas = String(e.data); }};
                w.onerror = function (ev) {{
                    window.__wkCanvas = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                return 'worker-created';
            }} catch (e) {{
                window.__wkCanvas = 'CREATE-ERROR:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = encode_worker_body(WORKER_CANVAS_BODY)
    );
    let _ = page.evaluate_js_web(&driver);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__wkCanvas") {
            let trimmed = s.trim();
            let is_set = !trimmed.is_empty() &&
                trimmed != "null" &&
                trimmed != "\"null\"" &&
                trimmed != "undefined" &&
                trimmed != "\"undefined\"";
            if is_set {
                return unquote_bridge(trimmed.to_string());
            }
        }
        assert!(
            Instant::now() <= deadline,
            "③ worker canvas digest did not arrive within timeout (worker postMessage → \
             onmessage broken on the live path)"
        );
        pump(page, 100);
    }
}

/// @trace REQ-STL-003 [criterion:REQ-STL-003] worker-realm OffscreenCanvas
/// noise rides the HOST page's profile (R53-A phase-2 identity threading
/// at `CanvasState::new` — the worker's canvas creation carries
/// `egress_webview_id`)
///
/// Page F (firefox, seed 42) and page C (chrome, seed 137) in ONE runtime;
/// each spawns a Worker that reads back an identical OffscreenCanvas. The
/// contract holds iff:
///   1. each worker's two readbacks are identical (deterministic per seed),
///   2. page F's worker digest ≠ page C's worker digest (each worker canvas
///      rides its HOST page's profile, not the process-global last write).
/// Pre-R53 both workers rode the last-installed global seed → identical
/// digests (RED).
#[test]
fn worker_offscreencanvas_rides_host_page_profile_live() {
    if !common::run_isolated(
        "stealth_per_page_canvas_tests::worker_offscreencanvas_rides_host_page_profile_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    bun_core::Output::init_test();

    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");

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

    let wf = run_worker_canvas_probe(&page_f);
    let wc = run_worker_canvas_probe(&page_c);
    eprintln!("[worker-canvas] F worker: {wf}");
    eprintln!("[worker-canvas] C worker: {wc}");

    for (tag, digest) in [("F", &wf), ("C", &wc)] {
        assert!(
            !digest.starts_with("WORKER-ERROR:") && !digest.starts_with("CREATE-ERROR:"),
            "③ page {tag} worker failed: {digest}"
        );
        assert!(
            !digest.starts_with("ABSENT:") && !digest.starts_with("THROW:"),
            "③ page {tag} worker canvas probe failed: {digest}"
        );
    }

    let (f_a, f_b) = wf
        .split_once('|')
        .unwrap_or_else(|| panic!("③ F worker digest pair malformed: {wf}"));
    let (c_a, c_b) = wc
        .split_once('|')
        .unwrap_or_else(|| panic!("③ C worker digest pair malformed: {wc}"));
    assert_eq!(
        f_a, f_b,
        "③ page F's worker readbacks must be deterministic (same seed, same coords)"
    );
    assert_eq!(
        c_a, c_b,
        "③ page C's worker readbacks must be deterministic (same seed, same coords)"
    );
    assert_ne!(
        f_a, c_a,
        "③ R53-A VIOLATION: two host pages' worker OffscreenCanvas readbacks are \
         identical — worker canvases rode one process-global seed instead of \
         their host pages' profiles"
    );
    assert!(
        digest_flips(f_a) > 0,
        "③ page F's worker readback must carry its host profile's noise, got {f_a}"
    );
    assert!(
        digest_flips(c_a) > 0,
        "③ page C's worker readback must carry its host profile's noise, got {c_a}"
    );

    eprintln!("[worker-canvas] === ③ GREEN: worker OffscreenCanvas rides host page profile ===");
}
