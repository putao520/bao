// @trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:15] [level:integration]
// Worker-realm Web Audio (OfflineAudioContext full stack) live tests for
// REQ-BRW-004 criterion #15 (user ruling 2026-09-09 vendor patch).
//
// SPEC criterion under test:
//   C15: worker 侧 Audio 栈暴露 — criterion literal 「worker 内 AudioContext」:
//         实时 AudioContext 构造器 worker 可达(渲染跑在 servo-media 自有
//         AudioRenderThread,调用线程仅做通道控制 + 有界 init 握手,零
//         Window 依赖)+ OfflineAudioContext 全栈(OfflineAudioContext
//         / AudioBuffer / AudioNode / AudioParam / AudioDestinationNode /
//         AudioScheduledSourceNode / OscillatorNode / GainNode /
//         AudioBufferSourceNode / OfflineAudioCompletionEvent);
//         AnalyserNode/Analyser 族与 AudioListener 保持 Window-only)
//
// Vendor patch under test (Bao, upstream stays Window-only):
//   - 12 audio webidl files `[Exposed=Window]` → `[Exposed=(Window,Worker)]`
//     + member-level `[Exposed=Window]` gates on BaseAudioContext members
//     returning Window-only node types (listener + 8 factories) and on the
//     four AudioContext createMedia* members (Window-only type references).
//   - Implementation signatures `&Window` → `&GlobalScope` (constructor/
//     factory/reflect paths), mirroring the in-tree AudioDestinationNode
//     precedent and the C14 W3a/W3b decoupling shape.
//
// Noise assertions (completion ②): the bao_stealth audio JS hooks
// (hooks.rs build_audio_js) carry `typeof` guards and ride the W1a
// worker-scope injection chain — once servo exposes the interfaces the
// hooks go live in the worker realm with the parent page's audio seed,
// with ZERO bao_stealth patches. Proven here by:
//   ① same-seed double render inside one worker: bit-level identical
//   ② same page, Window realm render === Worker realm render (cross-realm
//      fingerprint consistency, same seed)
//   ③ different audio seed (different page profile): renders diverge
//
// Digest method: BigInt FNV-1a over the Float64 bit patterns of every
// rendered sample — bit-exact, no float-to-string rounding hazard.
//
// Environment gating: real servo rendering requires DISPLAY (Xvfb).
// Skipped unless BAO_TEST_NETWORK=1 and DISPLAY are present.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_worker_audio)'

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use bao_stealth::{AudioProfile, StealthProfile};
use std::sync::Mutex;
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

/// Serializes all servo-touching tests in this binary (servo single-instance).
/// BCE-20260627-009.
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

/// Acquire the global serializer lock for the full test duration.
fn lock_serializer() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: `TEST_SERIALIZER` is a `static`, so the lock's lifetime is
    // bounded by the process (same shape as stealth_offscreencanvas_tests).
    unsafe {
        std::mem::transmute::<std::sync::MutexGuard<'_, ()>, std::sync::MutexGuard<'static, ()>>(
            guard,
        )
    }
}

/// URL-encode a JS worker body for a data: URL (minimal percent-encoding).
fn encode_worker_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
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

/// Create a live BaoRuntime + one page with the given stealth profile.
fn live_page(profile: StealthProfile) -> (BaoRuntime, bao_browser::PageHandle) {
    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(profile),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    (runtime, page)
}

/// Shared JS helpers: bit-exact digest over rendered samples. Spliced into
/// both the worker body and the window probe so BOTH realms hash identically.
const DIGEST_JS: &str = r#"
function __baoDigest(d) {
  var f = new Float64Array(1);
  var u = new Uint8Array(f.buffer);
  var h = 0xcbf29ce484222325n;
  var M = 0x100000001b3n;
  var mask = 0xffffffffffffffffn;
  for (var i = 0; i < d.length; i++) {
    f[0] = d[i];
    for (var b = 0; b < 8; b++) {
      h ^= BigInt(u[b]);
      h = (h * M) & mask;
    }
  }
  return h.toString(16);
}
function __baoRenderFp() {
  var c = new OfflineAudioContext(1, 4410, 44100);
  var osc = c.createOscillator();
  var gain = c.createGain();
  osc.connect(gain);
  gain.connect(c.destination);
  osc.start(0);
  return c.startRendering().then(function (b) {
    var d = b.getChannelData(0);
    var nonzero = 0;
    for (var i = 0; i < d.length; i++) { if (d[i] !== 0) nonzero++; }
    return { digest: __baoDigest(d), nonzero: nonzero };
  });
}
function __baoRtFp() {
  // Real-time-context fingerprint carrier (C15 literal): the audiofp vector
  // on a real-time AudioContext is createBuffer → getChannelData (the hooked
  // getter adds deterministic per-seed noise). Realtime output itself goes to
  // the servo-media sink and is not JS-retrievable, so the buffer data IS the
  // observable fingerprint surface for this class.
  var c = new AudioContext();
  var b = c.createBuffer(1, 4410, 44100);
  var d = b.getChannelData(0);
  var nonzero = 0;
  for (var i = 0; i < d.length; i++) { if (d[i] !== 0) nonzero++; }
  return { digest: __baoDigest(d), nonzero: nonzero };
}
"#;

/// Worker body: exposure surface + completion ① full chain + same-seed double
/// render + hook presence. Every failure mode gets an explicit marker.
const WORKER_AUDIO_BODY: &str = r#"
var __r = (async function () {
  try {
    // ── Exposure surface (vendor patch) ─────────────────────────────────
    if (typeof OfflineAudioContext === 'undefined') { return 'ABSENT:OfflineAudioContext'; }
    if (typeof AudioBuffer === 'undefined') { return 'ABSENT:AudioBuffer'; }
    if (typeof AudioNode === 'undefined') { return 'ABSENT:AudioNode'; }
    if (typeof AudioParam === 'undefined') { return 'ABSENT:AudioParam'; }
    if (typeof AudioDestinationNode === 'undefined') { return 'ABSENT:AudioDestinationNode'; }
    if (typeof AudioScheduledSourceNode === 'undefined') { return 'ABSENT:AudioScheduledSourceNode'; }
    if (typeof OscillatorNode === 'undefined') { return 'ABSENT:OscillatorNode'; }
    if (typeof GainNode === 'undefined') { return 'ABSENT:GainNode'; }
    if (typeof AudioBufferSourceNode === 'undefined') { return 'ABSENT:AudioBufferSourceNode'; }
    if (typeof OfflineAudioCompletionEvent === 'undefined') { return 'ABSENT:OfflineAudioCompletionEvent'; }
    // ── Real-time AudioContext exposure (C15 criterion literal) ──────────
    if (typeof AudioContext === 'undefined') { return 'ABSENT:AudioContext'; }
    // createMedia* members stay behind member-level [Exposed=Window] gates
    // (Window-only type references) — they must NOT appear in the worker.
    if (typeof AudioContext.prototype.createMediaElementSource !== 'undefined') { return 'LEAK:createMediaElementSource'; }
    if (typeof AudioContext.prototype.createMediaStreamSource !== 'undefined') { return 'LEAK:createMediaStreamSource'; }
    if (typeof AudioContext.prototype.createMediaStreamTrackSource !== 'undefined') { return 'LEAK:createMediaStreamTrackSource'; }
    if (typeof AudioContext.prototype.createMediaStreamDestination !== 'undefined') { return 'LEAK:createMediaStreamDestination'; }
    var rt = new AudioContext();
    var rtState = rt.state;
    if (['suspended', 'running', 'closed'].indexOf(rtState) === -1) { return 'BADSTATE:' + rtState; }
    if (typeof rt.createBuffer !== 'function') { return 'ABSENT:rt.createBuffer'; }
    if (typeof rt.destination === 'undefined') { return 'ABSENT:rt.destination'; }
    // Window-only classes must NOT leak into the worker surface.
    if (typeof AnalyserNode !== 'undefined') { return 'LEAK:AnalyserNode'; }
    if (typeof AudioListener !== 'undefined') { return 'LEAK:AudioListener'; }
    var probe = new OfflineAudioContext(1, 8, 8000);
    if (typeof probe.createAnalyser !== 'undefined') { return 'LEAK:createAnalyser'; }
    if (typeof probe.createBiquadFilter !== 'undefined') { return 'LEAK:createBiquadFilter'; }
    if (typeof probe.listener !== 'undefined') { return 'LEAK:listener'; }
    if (typeof probe.createOscillator !== 'function') { return 'ABSENT:createOscillator'; }
    if (typeof probe.createGain !== 'function') { return 'ABSENT:createGain'; }
    if (typeof probe.createBuffer !== 'function') { return 'ABSENT:createBuffer'; }
    if (typeof probe.destination === 'undefined') { return 'ABSENT:destination'; }

    // ── Completion ① full chain: render 44100 samples ──────────────────
    var ctx = new OfflineAudioContext(1, 44100, 44100);
    var buf = await ctx.startRendering();
    var ch0 = buf.getChannelData(0);
    var okType = ch0 instanceof Float32Array;
    var okLen = ch0.length === 44100;

    // ── Hook presence (W1a worker injection chain, second drain point) ─
    var hooked = String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1;
    // All-class evidence (C16 worker fidelity): the same second-drain fix
    // lands the ENTIRE W1a guarded-hook family — the WebGL JS hook
    // (getParameter) was skipped by the identical pre-interfaces timing.
    var webglHooked = 'n/a';
    if (typeof WebGLRenderingContext !== 'undefined') {
      webglHooked = String(WebGLRenderingContext.prototype.getParameter).indexOf('dbgRenderer') !== -1 ? 1 : 0;
    }

    // ── Same-seed double render (bit-level determinism) ────────────────
    DIGEST_PLACEHOLDER
    var r1 = await __baoRenderFp();
    var r2 = await __baoRenderFp();
    var same = r1.digest === r2.digest;

    // ── Real-time noise arms (same-seed determinism on the rt carrier) ──
    var rt1 = __baoRtFp();
    var rt2 = __baoRtFp();
    var rtSame = rt1.digest === rt2.digest;

    return 'OK:type=' + (okType ? 1 : 0) + ':len=' + (okLen ? 1 : 0)
         + ':hooked=' + (hooked ? 1 : 0)
         + ':wglhooked=' + webglHooked
         + ':r1=' + r1.digest + ':r2=' + r2.digest + ':same=' + (same ? 1 : 0)
         + ':nonzero=' + r1.nonzero
         + ':rtstate=' + rtState
         + ':rt1=' + rt1.digest + ':rt2=' + rt2.digest + ':rtsame=' + (rtSame ? 1 : 0)
         + ':rtnonzero=' + rt1.nonzero;
  } catch (e) {
    return 'THROW:' + ((e && e.message) ? e.message : String(e));
  }
})();
__r.then(function (v) { self.postMessage(v); },
         function (e) { self.postMessage('REJECT:' + String(e)); });
"#;

/// Assemble the worker body with the digest helpers spliced in.
fn worker_audio_body() -> String {
    WORKER_AUDIO_BODY.replace("DIGEST_PLACEHOLDER", DIGEST_JS)
}

/// Page-side driver: create the Worker from a data: URL and wire
/// `w.onmessage` → `window.__auResult` (worker→main sink).
fn make_worker_driver(encoded_body: &str) -> String {
    format!(
        r#"
        (function () {{
            try {{
                var w = new Worker("data:text/javascript,{body}");
                w.onmessage = function (e) {{ window.__auResult = String(e.data); }};
                w.onerror = function (ev) {{
                    window.__auResult = 'WORKER-ERROR:' + ((ev && ev.message) ? ev.message : 'unknown');
                    return true;
                }};
                return 'worker-created';
            }} catch (e) {{
                window.__auResult = 'CREATE-ERROR:' + String(e);
                return 'worker-create-failed';
            }}
        }})();
        "#,
        body = encoded_body
    )
}

/// Poll `window.__auResult` until the Worker posts its digest (or errors).
fn wait_for_worker_result(page: &bao_browser::PageHandle, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__auResult") {
            let trimmed = s.trim();
            let is_set = !trimmed.is_empty() && trimmed != "null" && trimmed != "\"null\"";
            if is_set {
                return Some(unquote_bridge(trimmed.to_string()));
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run the worker probe on the given page; asserts the generic transport
/// markers and returns the digest string (must start with `OK:`).
fn run_worker_probe(page: &bao_browser::PageHandle) -> String {
    let _ = page.evaluate_js_web("window.__auResult = null;");
    let body = encode_worker_body(&worker_audio_body());
    let created = page.evaluate_js_web(&make_worker_driver(&body));
    match created {
        Ok(s) => assert!(
            s.contains("worker-created"),
            "Worker creation must succeed on the servo-native path, got: {s}"
        ),
        Err(e) => panic!("Worker creation dispatch failed: {e}"),
    }
    let r = wait_for_worker_result(page, Duration::from_secs(45)).unwrap_or_else(|| {
        panic!("worker audio digest did not arrive within timeout — worker postMessage → \
                onmessage or worker execution is broken on the live path")
    });
    assert!(
        !r.starts_with("WORKER-ERROR:"),
        "worker script threw before posting a digest: {r}"
    );
    assert!(
        !r.starts_with("CREATE-ERROR:"),
        "Worker constructor failed on the live path: {r}"
    );
    assert!(
        !r.starts_with("REJECT:"),
        "worker render promise must resolve: {r}"
    );
    assert!(
        !r.starts_with("THROW:"),
        "worker audio probe threw inside the worker: {r}"
    );
    r
}

/// Extract `r1=` from an `OK:` worker digest.
fn worker_digest_r1(r: &str) -> &str {
    let fields: Vec<&str> = r.split(':').collect();
    fields
        .iter()
        .find_map(|f| f.strip_prefix("r1="))
        .unwrap_or_else(|| panic!("digest must carry r1=, got: {r}"))
}

/// Extract `rt1=` (real-time carrier digest) from an `OK:` digest.
fn worker_digest_rt(r: &str) -> &str {
    let fields: Vec<&str> = r.split(':').collect();
    fields
        .iter()
        .find_map(|f| f.strip_prefix("rt1="))
        .unwrap_or_else(|| panic!("digest must carry rt1=, got: {r}"))
}

// ═══════════════════════════════════════════════════════════════════════
// §1 C15 completion ① — OfflineAudioContext full stack INSIDE a Worker
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:15] worker-realm OfflineAudioContext full
/// stack (live, servo-native Worker thread, stealth page)
///
/// Everything runs inside the Worker: the whole C15 worker surface is
/// exposed (real-time AudioContext + 10 offline interfaces + the completion
/// event), `new AudioContext()` constructs with a legal state and its
/// createBuffer → getChannelData carrier is noise-covered, the Window-only
/// surface does NOT leak (AnalyserNode / AudioListener +
/// the gated member-level factories incl. the four createMedia* members),
/// `new OfflineAudioContext(1, 44100, 44100)`
/// renders and `startRendering()` resolves to an AudioBuffer whose
/// `getChannelData(0)` is a Float32Array of length 44100. Also pins the
/// same-seed bit-level determinism on BOTH carriers (offline render + the
/// real-time buffer carrier) and the bao_stealth audio hook presence on the
/// W1a worker injection chain.
#[test]
fn c15_worker_offlineaudiocontext_full_stack_works() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(profile);

    let r = run_worker_probe(&page);
    assert!(r.starts_with("OK:"), "worker audio probe must succeed: {r}");
    assert!(
        r.contains(":type=1:"),
        "worker getChannelData(0) must be a Float32Array (completion ①), digest: {r}"
    );
    assert!(
        r.contains(":len=1:"),
        "worker rendered buffer length must be 44100 (completion ①), digest: {r}"
    );
    assert!(
        r.contains(":hooked=1:"),
        "bao_stealth audio JS hook must be live in the worker realm (W1a injection chain; \
         typeof guards activate now that servo exposes AudioBuffer/OfflineAudioContext), \
         digest: {r}"
    );
    assert!(
        r.contains(":wglhooked=1:"),
        "all-class fix evidence (C16 worker fidelity): the worker-realm WebGL JS hook \
         (getParameter) must ALSO be installed by the second drain point — it was skipped \
         by the same pre-interfaces timing as the audio hook, digest: {r}"
    );
    assert!(
        r.contains(":same=1"),
        "same-seed double render inside one worker must be bit-level identical \
         (completion ② determinism arm), digest: {r}"
    );
    let nonzero: i64 = r
        .split(':')
        .find_map(|f| f.strip_prefix("nonzero="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1);
    assert!(
        nonzero > 0,
        "the rendered oscillator graph must produce non-zero samples (render pipeline live), \
         digest: {r}"
    );

    // ── Real-time AudioContext (C15 criterion literal) ───────────────────
    let rt_state = r
        .split(':')
        .find_map(|f| f.strip_prefix("rtstate="))
        .unwrap_or("<missing>");
    assert!(
        ["suspended", "running", "closed"].contains(&rt_state),
        "worker new AudioContext() must construct with a legal AudioContextState, \
         got {rt_state}, digest: {r}"
    );
    assert!(
        r.contains(":rtsame=1"),
        "worker real-time carrier (AudioContext.createBuffer → hooked getChannelData) must \
         be same-seed bit-level deterministic (realtime determinism arm), digest: {r}"
    );
    let rt_nonzero: i64 = r
        .split(':')
        .find_map(|f| f.strip_prefix("rtnonzero="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1);
    assert!(
        rt_nonzero > 0,
        "worker real-time carrier must carry non-zero noise samples (audio hook live on the \
         AudioContext-created buffer, zero bao_stealth patch), digest: {r}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §2 C15 completion ② — cross-realm fingerprint consistency (same page,
//    same seed): Window render digest === Worker render digest
// ═══════════════════════════════════════════════════════════════════════

/// Window-realm probe: run the IDENTICAL fingerprint render on the page's
/// Window realm; the digest lands on `window.__auWinResult` and is polled.
fn window_render_probe_js() -> String {
    format!(
        r#"
(function () {{
  try {{
    if (typeof OfflineAudioContext === 'undefined') {{ window.__auWinResult = 'ABSENT:OfflineAudioContext'; return; }}
{digest}
    var rt = __baoRtFp();
    __baoRenderFp().then(function (r) {{
      window.__auWinResult = 'OK:r1=' + r.digest + ':nonzero=' + r.nonzero + ':rt1=' + rt.digest;
    }}, function (e) {{
      window.__auWinResult = 'REJECT:' + String(e);
    }});
  }} catch (e) {{
    window.__auWinResult = 'THROW:' + ((e && e.message) ? e.message : String(e));
  }}
}})()"#,
        digest = DIGEST_JS
    )
}

/// Poll `window.__auWinResult` (window-side async sink).
fn wait_for_window_result(page: &bao_browser::PageHandle, timeout: Duration) -> Option<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(s) = page.evaluate_js_web("window.__auWinResult") {
            let trimmed = s.trim();
            let is_set = !trimmed.is_empty() && trimmed != "null" && trimmed != "\"null\"";
            if is_set {
                return Some(unquote_bridge(trimmed.to_string()));
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// @trace REQ-BRW-004 [criterion:15] cross-realm audio noise consistency
/// (live, same page, Window realm vs Worker realm)
///
/// The worker realm inherits the page's stealth profile (REALM_PROFILES
/// keyed by the worker global address, backfilled by the W1a worker-scope
/// callback), so the audio JS hooks run with the SAME seed in both realms.
/// The classic fingerprint vector (Oscillator → Gain → destination →
/// startRendering → getChannelData) AND the real-time carrier
/// (AudioContext.createBuffer → hooked getChannelData) must therefore
/// produce BIT-IDENTICAL output in both realms of the same page — the
/// cross-realm fingerprint consistency arm of completion ②, pinned on both
/// carriers. Any divergence is a detector-visible inconsistency (different
/// fingerprints for the same "browser").
#[test]
fn c15_worker_window_cross_realm_noise_consistency() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(profile);

    // Worker side first (reuses the full §1 probe; asserts transport health).
    let wr = run_worker_probe(&page);
    assert!(wr.starts_with("OK:"), "worker audio probe must succeed: {wr}");
    let worker_digest = worker_digest_r1(&wr);
    let worker_rt_digest = worker_digest_rt(&wr);

    // Window side on the SAME page.
    let _ = page.evaluate_js_web("window.__auWinResult = null;");
    let raw = page
        .evaluate_js_web(&window_render_probe_js())
        .expect("window audio probe must evaluate");
    assert!(
        raw.trim().is_empty() || raw.trim() == "undefined",
        "window probe is fire-and-forget (result via __auWinResult), got: {raw}"
    );
    let win = wait_for_window_result(&page, Duration::from_secs(30))
        .unwrap_or_else(|| panic!("window render digest did not arrive within timeout"));
    assert!(win.starts_with("OK:"), "window audio probe must succeed: {win}");
    let window_digest = worker_digest_r1(&win);
    let window_rt_digest = worker_digest_rt(&win);

    assert_eq!(
        worker_digest, window_digest,
        "cross-realm fingerprint consistency (completion ②): the same page must render the \
         same-seed audio fingerprint BIT-IDENTICALLY in the Worker realm ({worker_digest}) and \
         the Window realm ({window_digest})"
    );
    assert_eq!(
        worker_rt_digest, window_rt_digest,
        "cross-realm consistency on the REAL-TIME carrier (C15 literal): the same page must \
         produce the bit-identical AudioContext.createBuffer → getChannelData fingerprint in \
         the Worker realm ({worker_rt_digest}) and the Window realm ({window_rt_digest})"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §3 C15 completion ② — different audio seed must diverge (noise is
//    seed-driven, not a constant)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:15] worker audio noise diverges across
/// seeds (live, two pages with different audio seeds)
///
/// A second page whose profile carries a different audio seed (777 vs the
/// firefox_default 42) must render a DIFFERENT worker-side fingerprint —
/// proving the worker-realm noise is actually driven by the profile seed
/// (the hook's `detNoise`), not a constant offset — on BOTH carriers (the
/// offline render vector and the real-time AudioContext buffer carrier).
/// Also re-pins determinism on the second page (same=1, rtsame=1).
#[test]
fn c15_worker_noise_diverges_across_seeds() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile_a = StealthProfile::firefox_default();
    let (_runtime_a, page_a) = live_page(profile_a);
    let ra = run_worker_probe(&page_a);
    assert!(ra.starts_with("OK:"), "page A worker probe must succeed: {ra}");
    assert!(
        ra.contains(":same=1"),
        "page A same-seed determinism must hold, digest: {ra}"
    );
    assert!(
        ra.contains(":rtsame=1"),
        "page A real-time carrier determinism must hold, digest: {ra}"
    );
    let digest_a = worker_digest_r1(&ra);
    let rt_digest_a = worker_digest_rt(&ra);

    let mut profile_b = StealthProfile::firefox_default();
    profile_b.audio = AudioProfile::new(777);
    let (_runtime_b, page_b) = live_page(profile_b);
    let rb = run_worker_probe(&page_b);
    assert!(rb.starts_with("OK:"), "page B worker probe must succeed: {rb}");
    assert!(
        rb.contains(":same=1"),
        "page B same-seed determinism must hold, digest: {rb}"
    );
    assert!(
        rb.contains(":rtsame=1"),
        "page B real-time carrier determinism must hold, digest: {rb}"
    );
    let digest_b = worker_digest_r1(&rb);
    let rt_digest_b = worker_digest_rt(&rb);

    assert_ne!(
        digest_a, digest_b,
        "different audio seeds must produce different worker-realm fingerprints (completion ② \
         divergence arm) — got identical digests {digest_a} for seeds 42 and 777"
    );
    assert_ne!(
        rt_digest_a, rt_digest_b,
        "different audio seeds must produce different worker-realm REAL-TIME carrier \
         fingerprints (divergence arm on the AudioContext buffer carrier) — got identical \
         digests {rt_digest_a} for seeds 42 and 777"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 C15 completion ③ — Window-realm audio family zero regression (the
//    real-time AudioContext class and the gated Window-only members keep
//    their exact Window-side shape and behavior)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:15] window-realm audio family regression
/// pin (live, direct probe of the UNCHANGED Window surface)
///
/// The C15 vendor patch only WIDENS exposure (`Exposed=Window` →
/// `(Window,Worker)`, now including the real-time AudioContext) and
/// mechanically swaps `&Window` for `&GlobalScope` behind the same codegen'd
/// Window wrappers. This probe pins the Window side: OfflineAudioContext
/// still renders exactly, the real-time AudioContext interface keeps its
/// full shape (constructs with a legal state through the re-anchored
/// `&GlobalScope` constructor, and retains the Window-only createMedia*
/// members), the gated members (listener / createAnalyser /
/// createBiquadFilter ...) remain present on the Window surface, and the
/// audio noise hook is still installed on the Window realm's AudioBuffer
/// prototype.
#[test]
fn c15_window_audio_family_zero_regression() {
    if should_skip() {
        return;
    }
    let _guard = lock_serializer();

    let profile = StealthProfile::firefox_default();
    let (_runtime, page) = live_page(profile);

    let _ = page.evaluate_js_web("window.__auWinResult = null;");
    let probe = format!(
        r#"
(function () {{
  try {{
    // Real-time class intact on the Window surface (now (Window,Worker); the
    // constructor goes through the re-anchored &GlobalScope path).
    if (typeof AudioContext === 'undefined') {{ window.__auWinResult = 'ABSENT:AudioContext'; return; }}
    var rt = new AudioContext();
    if (['suspended', 'running', 'closed'].indexOf(rt.state) === -1) {{ window.__auWinResult = 'BADSTATE:' + rt.state; return; }}
    if (typeof AudioContext.prototype.createMediaElementSource !== 'function') {{ window.__auWinResult = 'ABSENT:createMediaElementSource'; return; }}
    if (typeof AudioContext.prototype.createMediaStreamSource !== 'function') {{ window.__auWinResult = 'ABSENT:createMediaStreamSource'; return; }}
    if (typeof AudioContext.prototype.createMediaStreamDestination !== 'function') {{ window.__auWinResult = 'ABSENT:createMediaStreamDestination'; return; }}
    // Gated members still fully present on the Window surface.
    if (typeof AnalyserNode === 'undefined') {{ window.__auWinResult = 'ABSENT:AnalyserNode'; return; }}
    if (typeof AudioListener === 'undefined') {{ window.__auWinResult = 'ABSENT:AudioListener'; return; }}
    if (typeof OfflineAudioContext.prototype.createAnalyser !== 'function') {{ window.__auWinResult = 'ABSENT:createAnalyser'; return; }}
    if (typeof OfflineAudioContext.prototype.createBiquadFilter !== 'function') {{ window.__auWinResult = 'ABSENT:createBiquadFilter'; return; }}
    if (typeof OfflineAudioContext.prototype.createPanner !== 'function') {{ window.__auWinResult = 'ABSENT:createPanner'; return; }}
    // `listener` is an accessor: read it on a real instance (reading it on the
    // prototype invokes the getter with a non-instance `this` and throws).
    var probeCtx = new OfflineAudioContext(1, 8, 8000);
    if (typeof probeCtx.listener === 'undefined') {{ window.__auWinResult = 'ABSENT:listener'; return; }}
    // Hook still installed on the Window realm.
    var hooked = String(AudioBuffer.prototype.getChannelData).indexOf('detNoise') !== -1;
    // Offline render through the re-anchored (&GlobalScope) code paths.
{digest}
    __baoRenderFp().then(function (r) {{
      window.__auWinResult = 'OK:hooked=' + (hooked ? 1 : 0)
        + ':rtstate=' + rt.state
        + ':r1=' + r.digest + ':nonzero=' + r.nonzero;
    }}, function (e) {{
      window.__auWinResult = 'REJECT:' + String(e);
    }});
  }} catch (e) {{
    window.__auWinResult = 'THROW:' + ((e && e.message) ? e.message : String(e));
  }}
}})()"#,
        digest = DIGEST_JS
    );
    let raw = page
        .evaluate_js_web(&probe)
        .expect("window regression probe must evaluate");
    assert!(
        raw.trim().is_empty() || raw.trim() == "undefined",
        "window regression probe is fire-and-forget, got: {raw}"
    );
    let r = wait_for_window_result(&page, Duration::from_secs(30))
        .unwrap_or_else(|| panic!("window regression digest did not arrive within timeout"));
    assert!(
        r.starts_with("OK:"),
        "window audio family must keep its full pre-C15 shape, digest: {r}"
    );
    assert!(
        r.contains(":hooked=1:"),
        "Window-realm audio hook must still be installed, digest: {r}"
    );
    let nonzero: i64 = r
        .split(':')
        .find_map(|f| f.strip_prefix("nonzero="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1);
    assert!(
        nonzero > 0,
        "Window-realm offline render must still produce samples, digest: {r}"
    );
}
