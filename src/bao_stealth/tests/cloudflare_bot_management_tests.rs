// @trace TEST-STL-CF-001 [req:REQ-STL-001,REQ-STL-002,REQ-STL-007] [level:integration]
// Cloudflare Bot Management confrontation — multi-vector detection that
// combines TLS JA3/JA4 + HTTP/2 fingerprint + JS challenge (__cf_bm cookie).
//
// Cloudflare bot detection is layered:
//   1. Network layer: TLS ClientHello fingerprint (JA3/JA4) must match a known
//      browser hash, else blocked at edge.
//   2. Protocol layer: HTTP/2 SETTINGS frame parameters (Akamai fingerprint)
//      and WINDOW_UPDATE size must match browser defaults.
//   3. JS challenge layer: cf-chl endpoint runs JS that probes navigator,
//      screen, WebGL, audio — must look like a real browser.
//   4. Session layer: __cf_bm cookie issued after challenge; timing entropy
//      of mouse/keyboard during challenge affects trust score.
//
// These tests verify bao_stealth produces TLS + HTTP/2 + JS challenge vectors
// that match real browser fingerprints, defeating Cloudflare's first three
// layers without behavioral artifacts.

use bao_stealth::{
    StealthEngine, StealthProfile, StealthHooks,
    TlsFingerprint, Http2Fingerprint,
};

// ===========================================================================
// 1. TLS JA3 must match known browser hash (Cloudflare edge blocks unknowns)
// ===========================================================================

// ---- 1.1 Firefox JA3 must be the canonical Firefox 128 hash ----
// @trace REQ-STL-001-C1 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_firefox_ja3_matches_known_browser_hash() {
    // Arrange — Cloudflare maintains a hash DB of JA3 fingerprints per browser.
    //           Unknown JA3 hashes are flagged as bot. Firefox 128 has a known JA3.
    // Act
    let fp = TlsFingerprint::firefox();
    let ja3 = fp.compute_ja3();

    // Assert — JA3 must follow the canonical Firefox structure:
    //   - Starts with TLS version 771 (TLS 1.2 record, negotiates 1.3)
    //   - Cipher suites include TLS 1.3 (4865-4866-4867) + Firefox TLS 1.2 set
    //   - Extensions include GREASE 65037 (0xFE0D) — Firefox-specific
    assert!(
        ja3.starts_with("771,4865-"),
        "Firefox JA3 must start with '771,4865-' (TLS 1.3 AES_128_GCM) — got: {}",
        &ja3[..ja3.len().min(40)]
    );
    // Must contain Firefox-specific cipher 0x0067 (DHE-RSA-AES256-SHA256)
    assert!(
        ja3.contains("-103"),
        "Firefox JA3 must include cipher 103 (0x0067) — Firefox TLS 1.2 signature"
    );
    // Must contain GREASE extension 65037 (0xFE0D) — present in Firefox/Chrome
    assert!(
        ja3.contains("-65037-") || ja3.contains("-65037,"),
        "JA3 must include GREASE extension 65037 — modern browser signature"
    );
}

// ---- 1.2 Chrome JA3 must be the canonical Chrome hash ----
// @trace REQ-STL-001-C1 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_chrome_ja3_matches_known_browser_hash() {
    // Arrange — Cloudflare has a known Chrome 120+ JA3 hash
    // Act
    let fp = TlsFingerprint::chrome();
    let ja3 = fp.compute_ja3();

    // Assert — Chrome JA3 structure:
    //   - Chrome omits 0x0067/0x0033 cipher (only Firefox has DHE)
    //   - Must NOT contain "158" (0x009E DHE-RSA-AES128-GCM — Firefox-only)
    assert!(
        !ja3.contains("-158-") && !ja3.contains("158-"),
        "Chrome JA3 must NOT include cipher 158 (Firefox DHE) — got: {}",
        &ja3[..ja3.len().min(60)]
    );
    // Must contain GREASE extension 65037
    assert!(
        ja3.contains("-65037-") || ja3.contains("-65037,"),
        "Chrome JA3 must include GREASE 65037"
    );
    // Chrome JA3 cipher count is smaller than Firefox
    let cipher_count = ja3.split(',').nth(1).unwrap_or("").split('-').count();
    assert!(
        cipher_count <= 14,
        "Chrome JA3 cipher count {} must be ≤ 14 (Chrome trims DHE suites)",
        cipher_count
    );
}

// ---- 1.3 Chrome latest JA3 includes post-2024 extensions ----
// @trace REQ-STL-001-C1 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_chrome_latest_ja3_has_modern_extensions() {
    // Arrange — Cloudflare updates detection for Chrome 130+ (Oct 2024)
    // Act
    let fp = TlsFingerprint::chrome_latest();
    let ja3 = fp.compute_ja3();

    // Assert — chrome_latest adds extension 0x001C (delegated_credentials) and 0x0039
    assert!(
        ja3.contains("-28-") || ja3.contains("-28,"),
        "Chrome latest JA3 must include extension 28 (0x001C delegated_credentials)"
    );
    assert!(
        ja3.contains("-57-") || ja3.contains("-57,") || ja3.ends_with("-57"),
        "Chrome latest JA3 must include extension 57 (0x0039)"
    );
}

// ---- 1.4 JA4 fingerprint — Cloudflare uses JA4 as secondary signal ----
// @trace REQ-STL-001-C2 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_ja4_format_matches_standard() {
    // Arrange — JA4 format: t13dNNNNNN_xxxx (FoxIO standard adopted by Cloudflare)
    // Act
    let fp = TlsFingerprint::firefox();
    let ja4 = fp.compute_ja4();

    // Assert — must start with t13d (TLS 1.3 + Destination SNI present)
    assert!(
        ja4.starts_with("t13d"),
        "JA4 must start with 't13d' (TLS 1.3 + SNI) — Cloudflare JA4 parser"
    );
    // Must contain underscore separator
    assert!(
        ja4.contains('_'),
        "JA4 must contain '_' separator — Cloudflare JA4 parser"
    );
    // Hex suffix must be 12 chars (SHA256 of ALPN, first 12 hex chars)
    let suffix = ja4.split('_').nth(1).unwrap_or("");
    assert_eq!(
        suffix.len(), 12,
        "JA4 suffix must be 12 hex chars — got {} ('{}')", suffix.len(), suffix
    );
    assert!(
        suffix.chars().all(|c| c.is_ascii_hexdigit()),
        "JA4 suffix must be hex — Cloudflare JA4 hash parser"
    );
}

// ---- 1.5 JA4 deterministic — same profile produces same JA4 ----
// @trace REQ-STL-001-C2 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_ja4_deterministic_across_calls() {
    // Arrange — Cloudflare logs JA4 per request; non-deterministic JA4 is a bot signal
    let fp = TlsFingerprint::chrome();

    // Act
    let ja4_a = fp.compute_ja4();
    let ja4_b = fp.compute_ja4();

    // Assert
    assert_eq!(
        ja4_a, ja4_b,
        "JA4 must be deterministic across calls — Cloudflare logs per-request"
    );
}

// ===========================================================================
// 2. HTTP/2 fingerprint — Cloudflare matches Akamai fingerprint string
// ===========================================================================

// ---- 2.1 Firefox HTTP/2 Akamai fingerprint ----
// @trace REQ-STL-002-C1 [req:REQ-STL-002] [level:integration]
#[test]
fn cloudflare_firefox_http2_akamai_fingerprint() {
    // Arrange — Cloudflare matches HTTP/2 SETTINGS against known browser patterns
    // Act
    let fp = Http2Fingerprint::firefox();
    let akamai = fp.akamai_fingerprint();

    // Assert — Firefox HTTP/2 signature:
    //   header_table_size = 65536
    //   enable_push = 0 (Firefox 128+)
    //   max_concurrent_streams = 100
    //   initial_window_size = 131072
    //   max_frame_size = 16384
    //   max_header_list_size = 262144
    assert_eq!(
        akamai, "65536:0:100:131072:16384:262144",
        "Firefox HTTP/2 Akamai fingerprint must match canonical Firefox pattern"
    );
}

// ---- 2.2 Chrome HTTP/2 Akamai fingerprint ----
// @trace REQ-STL-002-C1 [req:REQ-STL-002] [level:integration]
#[test]
fn cloudflare_chrome_http2_akamai_fingerprint() {
    // Arrange — Chrome HTTP/2 differs from Firefox in window sizes
    // Act
    let fp = Http2Fingerprint::chrome();
    let akamai = fp.akamai_fingerprint();

    // Assert — Chrome signature:
    //   max_concurrent_streams = 1000 (Chrome-specific)
    //   initial_window_size = 6291456 (Chrome-specific, much larger than Firefox)
    assert_eq!(
        akamai, "65536:0:1000:6291456:16384:262144",
        "Chrome HTTP/2 Akamai fingerprint must match canonical Chrome pattern"
    );
}

// ---- 2.3 HTTP/2 WINDOW_UPDATE size (Cloudflare secondary signal) ----
// @trace REQ-STL-002-C2 [req:REQ-STL-002] [level:integration]
#[test]
fn cloudflare_http2_window_update_size_matches_browser() {
    // Arrange — Cloudflare checks WINDOW_UPDATE frame size separately from SETTINGS
    // Act
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();

    // Assert
    assert_eq!(
        ff.window_update_size, 131072,
        "Firefox WINDOW_UPDATE must be 131072 — Cloudflare HTTP/2 check"
    );
    assert_eq!(
        ch.window_update_size, 15663105,
        "Chrome WINDOW_UPDATE must be 15663105 — Cloudflare HTTP/2 check"
    );
    assert_ne!(
        ff.window_update_size, ch.window_update_size,
        "Firefox/Chrome WINDOW_UPDATE must differ — distinguishability"
    );
}

// ---- 2.4 Pseudo header order (Cloudflare detects reordered headers) ----
// @trace REQ-STL-002-C4 [req:REQ-STL-002] [level:integration]
#[test]
fn cloudflare_http2_pseudo_header_order() {
    // Arrange — Cloudflare validates pseudo-header order matches browser
    //           Firefox: :method :path :authority :scheme
    //           Chrome:  :method :authority :scheme :path
    // Act
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();

    // Assert
    assert_eq!(
        ff.pseudo_header_order,
        vec![":method", ":path", ":authority", ":scheme"],
        "Firefox pseudo-header order — Cloudflare HTTP/2 parser"
    );
    assert_eq!(
        ch.pseudo_header_order,
        vec![":method", ":authority", ":scheme", ":path"],
        "Chrome pseudo-header order — Cloudflare HTTP/2 parser"
    );
    assert_ne!(
        ff.pseudo_header_order, ch.pseudo_header_order,
        "Firefox/Chrome pseudo-header order must differ"
    );
}

// ===========================================================================
// 3. ALPN protocol coherence (Cloudflare detects h2 + http/1.1 both advertised)
// ===========================================================================

// ---- 3.1 ALPN must include both h2 and http/1.1 ----
// @trace REQ-STL-001 [req:REQ-STL-001] [level:integration]
#[test]
fn cloudflare_alpn_includes_h2_and_http11() {
    // Arrange — Cloudflare expects browsers to advertise both h2 and http/1.1
    // Act
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    let ff_alpn: Vec<&str> = ff.alpn_strings();
    let ch_alpn: Vec<&str> = ch.alpn_strings();

    // Assert
    assert!(
        ff_alpn.contains(&"h2") && ff_alpn.contains(&"http/1.1"),
        "Firefox ALPN must include 'h2' and 'http/1.1' — Cloudflare ALPN coherence"
    );
    assert!(
        ch_alpn.contains(&"h2") && ch_alpn.contains(&"http/1.1"),
        "Chrome ALPN must include 'h2' and 'http/1.1' — Cloudflare ALPN coherence"
    );
}

// ===========================================================================
// 4. JS challenge layer — cf-chl probes navigator/screen/webgl in browser
// ===========================================================================

// ---- 4.1 JS hooks must inject navigator overrides (cf-chl detection) ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cloudflare_js_challenge_navigator_overrides_present() {
    // Arrange — Cloudflare JS challenge (cf-chl) reads navigator.userAgent etc.
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Act — cf-chl probes navigator properties
    // Assert — all cf-chl probe targets must be overridden
    assert!(
        js.contains("navigator, 'userAgent'"),
        "navigator.userAgent must be overridden — cf-chl probe target"
    );
    assert!(
        js.contains("navigator, 'platform'"),
        "navigator.platform must be overridden — cf-chl probe target"
    );
    assert!(
        js.contains("navigator, 'vendor'"),
        "navigator.vendor must be overridden — cf-chl probe target"
    );
    assert!(
        js.contains("navigator, 'hardwareConcurrency'"),
        "navigator.hardwareConcurrency must be overridden — cf-chl probe target"
    );
    assert!(
        js.contains("navigator, 'webdriver'"),
        "navigator.webdriver must be overridden — cf-chl bot signal"
    );
}

// ---- 4.2 JS hooks must override screen properties (cf-chl detection) ----
// @trace REQ-STL-004-C6 [req:REQ-STL-004] [level:integration]
#[test]
fn cloudflare_js_challenge_screen_overrides_present() {
    // Arrange
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — cf-chl probes screen dimensions
    assert!(
        js.contains("screen, 'width'") && js.contains("screen, 'height'"),
        "screen.width/height must be overridden — cf-chl probe"
    );
    assert!(
        js.contains("window, 'devicePixelRatio'"),
        "window.devicePixelRatio must be overridden — cf-chl probe"
    );
}

// ---- 4.3 JS hooks must override WebGL RENDERER/VENDOR (cf-chl detection) ----
// @trace REQ-STL-005-C1 [req:REQ-STL-005] [level:integration]
#[test]
fn cloudflare_js_challenge_webgl_overrides_present() {
    // Arrange — cf-chl probes WebGL debug renderer info
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — must override WebGL getParameter for RENDERER (0x9246) and VENDOR (0x9245)
    assert!(
        js.contains("0x9246"),
        "WebGL RENDERER override (0x9246) must be present — cf-chl WebGL probe"
    );
    assert!(
        js.contains("0x9245"),
        "WebGL VENDOR override (0x9245) must be present — cf-chl WebGL probe"
    );
    // Both WebGLRenderingContext and WebGL2RenderingContext must be patched
    assert!(
        js.contains("WebGLRenderingContext"),
        "WebGLRenderingContext.prototype.getParameter must be patched"
    );
    assert!(
        js.contains("WebGL2RenderingContext"),
        "WebGL2RenderingContext.prototype.getParameter must be patched"
    );
}

// ===========================================================================
// 5. Behavioral trust score (cf-chl includes mouse-move entropy check)
// ===========================================================================

// ---- 5.1 Mouse path entropy — Cloudflare __cf_bm check ----
// @trace REQ-STL-006-C1 [req:REQ-STL-006] [level:integration]
#[test]
fn cloudflare_mouse_path_has_sufficient_entropy() {
    // Arrange — Cloudflare JS challenge measures mouse-move entropy during
    //           the interstitial. Linear or absent mouse movement = bot.
    use bao_stealth::BehaviorSimulator;
    let sim = BehaviorSimulator::new(42);

    // Act — generate a path through the challenge
    let path = sim.generate_human_mouse_path((100.0, 100.0), (700.0, 500.0), 30.0);

    // Assert — path must have multiple waypoints with non-zero movement
    assert!(
        path.len() >= 5,
        "Mouse path must have ≥5 waypoints — Cloudflare movement check"
    );
    // Total distance traveled must be > 0 (not a single point)
    let total_dist: f64 = path.windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    assert!(
        total_dist > 100.0,
        "Mouse path total distance {} must be > 100px — Cloudflare movement entropy",
        total_dist
    );
}

// ---- 5.2 Typing rhythm during challenge (cf-chl captcha input) ----
// @trace REQ-STL-006-C2 [req:REQ-STL-006] [level:integration]
#[test]
fn cloudflare_typing_rhythm_in_human_range() {
    // Arrange — Cloudflare measures keystroke timing during form-fill challenge
    use bao_stealth::BehaviorSimulator;
    let sim = BehaviorSimulator::new(99);

    // Act — type a 10-char string
    let events = sim.generate_human_typing("hello12345");

    // Assert — per-key delay must be in human range (50-300ms typical)
    for e in &events {
        if !e.is_backspace {
            assert!(
                e.delay_before_ms >= 30 && e.delay_before_ms <= 800,
                "Keystroke delay {}ms out of human range [30, 800] — Cloudflare timing check",
                e.delay_before_ms
            );
        }
    }
    // Must have at least 10 events (one per char minimum)
    assert!(
        events.iter().filter(|e| !e.is_backspace).count() >= 10,
        "Typing events must cover all 10 input chars — Cloudflare completeness"
    );
}

// ===========================================================================
// 6. Combined Cloudflare confrontation — full StealthProfile passes all layers
// ===========================================================================

// ---- 6.1 Firefox StealthProfile passes all Cloudflare layers ----
// @trace REQ-STL-001,REQ-STL-002,REQ-STL-007 [req:REQ-STL-001,REQ-STL-002,REQ-STL-007] [level:integration]
#[test]
fn cloudflare_firefox_profile_passes_all_layers() {
    // Arrange — full profile confrontation
    let engine = StealthEngine::new(StealthProfile::firefox_default());

    // Act — extract each layer's signature
    let tls = engine.tls_config();
    let http2 = engine.http2_config();
    let nav = engine.navigator();
    let webgl = engine.webgl();

    // Assert — Layer 1 (TLS): Firefox JA3 canonical
    assert!(tls.compute_ja3().starts_with("771,4865-"));
    assert!(tls.compute_ja3().contains("65037")); // GREASE

    // Assert — Layer 2 (HTTP/2): Firefox Akamai fingerprint
    assert_eq!(http2.akamai_fingerprint(), "65536:0:100:131072:16384:262144");

    // Assert — Layer 3 (JS challenge): navigator + screen + WebGL Firefox-coherent
    assert!(nav.user_agent.contains("Firefox"));
    assert!(nav.vendor.is_empty());
    assert_eq!(webgl.vendor, "Mozilla");

    // Assert — Layer 4 (Behavior): simulator has a positive seed
    assert!(engine.behavior().seed() > 0);
}

// ---- 6.2 Chrome StealthProfile passes all Cloudflare layers ----
// @trace REQ-STL-001,REQ-STL-002,REQ-STL-007 [req:REQ-STL-001,REQ-STL-002,REQ-STL-007] [level:integration]
#[test]
fn cloudflare_chrome_profile_passes_all_layers() {
    // Arrange
    let engine = StealthEngine::new(StealthProfile::chrome_default());

    // Act
    let tls = engine.tls_config();
    let http2 = engine.http2_config();
    let nav = engine.navigator();
    let webgl = engine.webgl();

    // Assert — Layer 1: Chrome JA3 (no Firefox DHE cipher 158)
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,4865-"));
    assert!(!ja3.contains("-158-") && !ja3.contains("158-"),
        "Chrome JA3 must not include Firefox DHE cipher");

    // Assert — Layer 2: Chrome Akamai fingerprint
    assert_eq!(http2.akamai_fingerprint(), "65536:0:1000:6291456:16384:262144");

    // Assert — Layer 3: Chrome navigator + WebGL coherent
    assert!(nav.user_agent.contains("Chrome"));
    assert_eq!(nav.vendor, "Google Inc.");
    assert!(webgl.vendor.starts_with("Google Inc."));
}
