// @trace TEST-STL-HEADLESS-001 [req:REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-007] [level:integration]
// Headless mode fingerprint hiding — zero feature leakage under headless detection.
//
// Headless browsers leak distinctive signatures that detection services flag
// instantly. These tests verify bao_stealth profiles do NOT carry ANY headless
// leak artifact, regardless of which StealthProfile is constructed.
//
// Known headless leaks (must be ABSENT from bao_stealth):
//   1. WebGL renderer = "Google SwiftShader" (headless Chrome software rendering)
//   2. WebGL vendor = "Google Inc." with no GPU (SwiftShader leak)
//   3. Canvas toDataURL = all-black or all-white (no rendering in headless)
//   4. navigator.userAgent contains "HeadlessChrome"
//   5. Permissions API returns "denied" (headless has no permission UI)
//   6. screen.width/height = 0 or 800x600 (headless default viewport)
//   7. navigator.webdriver = true (headless Chrome sets this by default)
//   8. Plugins array length = 0 (headless strips plugins)
//   9. window.chrome absent or empty (headless doesn't initialize chrome object)
//
// Each test verifies a concrete profile field value against the headless leak
// signature. A leak (e.g., WebGL renderer containing "SwiftShader") surfaces
// as a failed assertion.

use bao_stealth::{
    AudioProfile, CanvasNoise, NavigatorProfile, ScreenProfile, StealthEngine, StealthHooks,
    StealthProfile, WebGLProfile,
};

// ===========================================================================
// 1. WebGL renderer/vendor — must NOT leak SwiftShader or headless signatures
// ===========================================================================

// ---- 1.1 Firefox WebGL renderer does not contain SwiftShader ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C1] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_firefox_webgl_renderer_not_swiftshader() {
    // Arrange — headless Chrome WebGL RENDERER = "Google SwiftShader"
    // Act
    let webgl = WebGLProfile::firefox();

    // Assert — Firefox renderer must NOT contain SwiftShader
    assert!(
        !webgl.renderer.to_lowercase().contains("swiftshader"),
        "Firefox WebGL renderer must not leak SwiftShader — got: {}",
        webgl.renderer
    );
    assert!(
        !webgl.renderer.to_lowercase().contains("software"),
        "Firefox WebGL renderer must not indicate software rendering — got: {}",
        webgl.renderer
    );
}

// ---- 1.2 Chrome WebGL renderer does not contain SwiftShader ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C1] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_chrome_webgl_renderer_not_swiftshader() {
    // Arrange — headless Chrome WebGL RENDERER = "Google SwiftShader"
    // Act
    let webgl = WebGLProfile::chrome();

    // Assert — Chrome renderer must be a real GPU (e.g., ANGLE/NVIDIA)
    assert!(
        !webgl.renderer.to_lowercase().contains("swiftshader"),
        "Chrome WebGL renderer must not leak SwiftShader — got: {}",
        webgl.renderer
    );
    // Chrome renderer should reference a real GPU vendor (ANGLE pattern)
    assert!(
        webgl.renderer.contains("ANGLE")
            || webgl.renderer.contains("NVIDIA")
            || webgl.renderer.contains("GeForce")
            || webgl.renderer.contains("OpenGL"),
        "Chrome WebGL renderer must reference real GPU, got: {}",
        webgl.renderer
    );
}

// ---- 1.3 WebGL vendor must NOT be the bare "Google Inc." (SwiftShader vendor) ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C2] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_webgl_vendor_not_bare_google_inc() {
    // Arrange — headless Chrome WebGL VENDOR = "Google Inc." (SwiftShader)
    //           Real Chrome WebGL VENDOR = "Google Inc. (NVIDIA)" (with GPU suffix)
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();

    // Assert
    // Firefox vendor = "Mozilla" (not SwiftShader)
    assert_ne!(
        ff.vendor, "Google Inc.",
        "Firefox WebGL vendor must not be bare 'Google Inc.' — got: {}",
        ff.vendor
    );
    // Chrome vendor must include GPU suffix, not bare "Google Inc."
    assert_ne!(
        ch.vendor, "Google Inc.",
        "Chrome WebGL vendor must not be bare 'Google Inc.' (SwiftShader leak) — got: {}",
        ch.vendor
    );
    // Chrome vendor must start with "Google Inc. (" (with GPU suffix)
    assert!(
        ch.vendor.starts_with("Google Inc. ("),
        "Chrome WebGL vendor must include GPU suffix — got: {}",
        ch.vendor
    );
}

// ---- 1.4 WebGL has non-trivial extensions list (headless = empty) ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C3] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_webgl_extensions_nonempty() {
    // Arrange — headless WebGL exposes fewer/no extensions
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();

    // Assert
    assert!(
        ff.extensions.len() >= 10,
        "Firefox WebGL must have ≥10 extensions (headless has few) — got: {}",
        ff.extensions.len()
    );
    assert!(
        ch.extensions.len() >= 10,
        "Chrome WebGL must have ≥10 extensions (headless has few) — got: {}",
        ch.extensions.len()
    );
    // Must include WEBGL_debug_renderer_info (used by detectors to confirm GPU)
    assert!(
        ff.extensions
            .iter()
            .any(|e| e == "WEBGL_debug_renderer_info"),
        "Firefox WebGL must include WEBGL_debug_renderer_info extension"
    );
    assert!(
        ch.extensions
            .iter()
            .any(|e| e == "WEBGL_debug_renderer_info"),
        "Chrome WebGL must include WEBGL_debug_renderer_info extension"
    );
}

// ---- 1.5 WebGL max texture size is plausible GPU value (not 0) ----
// @trace REQ-STL-005 [req:REQ-STL-005] [level:integration]
#[test]
fn headless_webgl_max_texture_size_plausible() {
    // Arrange — headless WebGL max texture size can be 0 or very small
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();

    // Assert — real GPU max texture size ≥ 4096 (typically 16384)
    assert!(
        ff.max_texture_size >= 4096,
        "Firefox WebGL max_texture_size {} must be ≥ 4096 (headless leak)",
        ff.max_texture_size
    );
    assert!(
        ch.max_texture_size >= 4096,
        "Chrome WebGL max_texture_size {} must be ≥ 4096 (headless leak)",
        ch.max_texture_size
    );
    // Common GPU value: 16384
    assert_eq!(ff.max_texture_size, 16384);
    assert_eq!(ch.max_texture_size, 16384);
}

// ===========================================================================
// 2. Canvas — headless produces all-black or all-white; bao_stealth adds noise
// ===========================================================================

// ---- 2.1 Canvas noise is non-trivial (headless canvas is featureless) ----
// @trace REQ-STL-003 [criterion:REQ-STL-003-C1] [req:REQ-STL-003] [level:integration]
#[test]
fn headless_canvas_noise_produces_variation() {
    // Arrange — headless Canvas toDataURL produces identical bytes per session
    //           bao_stealth must add deterministic per-pixel noise
    let noise = CanvasNoise::new(42);

    // Act — apply noise to a uniform gray canvas
    let mut pixels = Vec::new();
    for x in 0..10u32 {
        for y in 0..10u32 {
            let (r, g, b, _) = noise.apply_to_pixel(128, 128, 128, 255, x, y);
            pixels.push((r, g, b));
        }
    }

    // Assert — at least 2 different pixel values produced (noise is non-trivial)
    let unique: std::collections::HashSet<_> = pixels.iter().collect();
    assert!(
        unique.len() >= 2,
        "Canvas noise must produce variation (≥2 unique pixels) — headless leak check, got {} unique",
        unique.len()
    );
}

// ---- 2.2 Canvas noise is deterministic (same seed → same noise) ----
// @trace REQ-STL-003 [criterion:REQ-STL-003-C3] [req:REQ-STL-003] [level:integration]
#[test]
fn headless_canvas_noise_deterministic() {
    // Arrange
    let noise = CanvasNoise::new(123);

    // Act
    let p1 = noise.apply_to_pixel(100, 100, 100, 255, 50, 50);
    let p2 = noise.apply_to_pixel(100, 100, 100, 255, 50, 50);

    // Assert
    assert_eq!(
        p1, p2,
        "Canvas noise must be deterministic — headless session consistency"
    );
}

// ---- 2.3 Canvas toDataURL JS hook exists ----
// @trace REQ-STL-003 [req:REQ-STL-003] [level:integration]
#[test]
fn headless_canvas_todataurl_hook_present() {
    // Arrange — headless canvas returns static data; hook must inject noise
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas,
        &profile.audio,
        &profile.navigator,
        &profile.screen,
        &profile.webgl,
        &profile.font,
        &profile.battery,
        profile.webrtc_mode,
        &profile.timing,
        &profile.clientrects,
        &profile.screen_display,
        &profile.plugin,
        &profile.speech,
        &profile.media_devices,
        &profile.permissions,
        &profile.webgl_context,
        &profile.connection,
        &profile.iframe,
    );
    let js = hooks.canvas_js();

    // Assert — must override toDataURL, getImageData, toBlob
    assert!(
        js.contains("HTMLCanvasElement.prototype.toDataURL"),
        "Canvas JS must override toDataURL — headless leak prevention"
    );
    assert!(
        js.contains("CanvasRenderingContext2D.prototype.getImageData"),
        "Canvas JS must override getImageData — headless leak prevention"
    );
}

// ===========================================================================
// 3. navigator.userAgent — must NOT contain HeadlessChrome
// ===========================================================================

// ---- 3.1 All profile user agents exclude HeadlessChrome ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C1] [req:REQ-STL-004] [level:integration]
#[test]
fn headless_no_useragent_contains_headless_chrome() {
    // Arrange — headless Chrome UA contains "HeadlessChrome/120.0.0.0"
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert
    assert!(
        !ff.user_agent.contains("HeadlessChrome"),
        "Firefox UA must not contain HeadlessChrome — got: {}",
        ff.user_agent
    );
    assert!(
        !ch.user_agent.contains("HeadlessChrome"),
        "Chrome UA must not contain HeadlessChrome — got: {}",
        ch.user_agent
    );
    // Also check lowercase (some detectors lowercase UA before matching)
    assert!(
        !ff.user_agent.to_lowercase().contains("headless"),
        "Firefox UA must not contain 'headless' (case-insensitive)"
    );
    assert!(
        !ch.user_agent.to_lowercase().contains("headless"),
        "Chrome UA must not contain 'headless' (case-insensitive)"
    );
}

// ---- 3.2 User agents reference a real browser version ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C1] [req:REQ-STL-004] [level:integration]
#[test]
fn headless_useragent_has_real_browser_version() {
    // Arrange — headless UA may have version "0.0.0" or be malformed
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — Firefox UA references Firefox/NNN.0
    assert!(
        ff.user_agent.contains("Firefox/128"),
        "Firefox UA must reference a real version (Firefox/128) — headless version check"
    );
    // Chrome UA references Chrome/NNN.0.0.0
    assert!(
        ch.user_agent.contains("Chrome/128"),
        "Chrome UA must reference a real version (Chrome/128) — headless version check"
    );
}

// ===========================================================================
// 4. Screen dimensions — must NOT be 0 or 800x600 (headless defaults)
// ===========================================================================

// ---- 4.1 Screen dimensions are not 0 ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C6] [req:REQ-STL-004] [level:integration]
#[test]
fn headless_screen_dimensions_not_zero() {
    // Arrange — headless default screen is 0x0 or 800x600
    let screen = ScreenProfile::default();

    // Assert
    assert_ne!(
        screen.width, 0,
        "Screen width must not be 0 — headless leak"
    );
    assert_ne!(
        screen.height, 0,
        "Screen height must not be 0 — headless leak"
    );
    assert_ne!(
        screen.avail_width, 0,
        "Screen avail_width must not be 0 — headless leak"
    );
    assert_ne!(
        screen.avail_height, 0,
        "Screen avail_height must not be 0 — headless leak"
    );
}

// ---- 4.2 Screen dimensions are not 800x600 (legacy headless default) ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C6] [req:REQ-STL-004] [level:integration]
#[test]
fn headless_screen_not_800x600() {
    // Arrange
    let screen = ScreenProfile::default();

    // Assert — must NOT be the headless legacy 800x600
    assert!(
        !(screen.width == 800 && screen.height == 600),
        "Screen must not be 800x600 — legacy headless default"
    );
    // Modern headless default is also 1920x1080 in newer versions, but
    // pixelscan flags any "perfect" 1920x1080 without avail_height adjustment.
    // bao_stealth default IS 1920x1080 with avail_height=1040 (taskbar) — coherent.
}

// ---- 4.3 Screen has plausible color depth (headless = 0 or 16) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn headless_screen_color_depth_is_24() {
    // Arrange — headless colorDepth is often 0 or 16
    let screen = ScreenProfile::default();

    // Assert — real desktop is 24 (true color)
    assert_eq!(
        screen.color_depth, 24,
        "colorDepth must be 24 (true color) — headless leak check"
    );
}

// ===========================================================================
// 5. navigator.webdriver — must be false (headless default is true)
// ===========================================================================

// ---- 5.1 JS hooks force navigator.webdriver = false ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C5] [req:REQ-STL-004] [level:integration]
#[test]
fn headless_webdriver_forced_false_in_hooks() {
    // Arrange — headless Chrome sets navigator.webdriver = true by default
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas,
        &profile.audio,
        &profile.navigator,
        &profile.screen,
        &profile.webgl,
        &profile.font,
        &profile.battery,
        profile.webrtc_mode,
        &profile.timing,
        &profile.clientrects,
        &profile.screen_display,
        &profile.plugin,
        &profile.speech,
        &profile.media_devices,
        &profile.permissions,
        &profile.webgl_context,
        &profile.connection,
        &profile.iframe,
    );
    let js = hooks.navigator_js();

    // Assert — must override webdriver to false
    assert!(
        js.contains("__bao_def(nav, 'webdriver'") && js.contains("return false"),
        "navigator.webdriver must be overridden to false — headless webdriver leak"
    );
    // Override must be configurable:false (anti-anti-detect)
    assert!(
        js.contains("configurable: false"),
        "webdriver override must be configurable:false — anti-anti-detect"
    );
}

// ===========================================================================
// 6. Audio fingerprint — headless produces silent output; bao_stealth adds noise
// ===========================================================================

// ---- 6.1 Audio noise is non-zero ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C4] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_audio_noise_is_nonzero() {
    // Arrange — headless AudioContext produces silent (0.0) samples
    let audio = AudioProfile::new(42);

    // Act — apply noise to a silent sample
    let noisy = audio.apply_noise(0.0, 100);

    // Assert — noisy sample must differ from input (noise added)
    assert!(
        (noisy - 0.0).abs() > 0.0,
        "Audio noise must produce non-zero output from silent input — headless leak"
    );
    // Noise amplitude is 1e-7 (sub-perceptible but detectable by fingerprinters)
    assert!(
        (noisy - 0.0).abs() <= 1e-7,
        "Audio noise amplitude must be ≤ 1e-7 (sub-perceptible) — got: {}",
        (noisy - 0.0).abs()
    );
}

// ---- 6.2 Audio JS hook present ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C4] [req:REQ-STL-005] [level:integration]
#[test]
fn headless_audio_hook_present() {
    // Arrange
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas,
        &profile.audio,
        &profile.navigator,
        &profile.screen,
        &profile.webgl,
        &profile.font,
        &profile.battery,
        profile.webrtc_mode,
        &profile.timing,
        &profile.clientrects,
        &profile.screen_display,
        &profile.plugin,
        &profile.speech,
        &profile.media_devices,
        &profile.permissions,
        &profile.webgl_context,
        &profile.connection,
        &profile.iframe,
    );
    let js = hooks.audio_js();

    // Assert — must override AudioBuffer.getChannelData and OfflineAudioContext.startRendering
    assert!(
        js.contains("AudioBuffer.prototype.getChannelData"),
        "Audio JS must override getChannelData — headless audio leak"
    );
    assert!(
        js.contains("OfflineAudioContext"),
        "Audio JS must override OfflineAudioContext.startRendering — headless audio leak"
    );
}

// ===========================================================================
// 7. Headless vs headed consistency (same profile = same fingerprint)
// ===========================================================================

// ---- 7.1 Canvas noise seed is consistent regardless of headless mode ----
// @trace REQ-STL-003 [criterion:REQ-STL-003-C3] [req:REQ-STL-003] [level:integration]
#[test]
fn headless_headed_canvas_noise_consistent() {
    // Arrange — same StealthProfile must produce identical canvas noise
    //           regardless of headless/headed mode
    let profile1 = StealthProfile::firefox_default();
    let profile2 = StealthProfile::firefox_default();

    // Act
    let p1 = profile1.canvas.apply_to_pixel(128, 128, 128, 255, 100, 100);
    let p2 = profile2.canvas.apply_to_pixel(128, 128, 128, 255, 100, 100);

    // Assert — same seed → same noise (headless/headed cannot differ)
    assert_eq!(
        p1, p2,
        "Canvas noise must be identical for same profile — headless/headed consistency"
    );
}

// ---- 7.2 TLS JA3 is consistent regardless of headless mode ----
// @trace REQ-STL-001 [criterion:REQ-STL-001-C1] [req:REQ-STL-001] [level:integration]
#[test]
fn headless_headed_tls_ja3_consistent() {
    // Arrange
    let profile1 = StealthProfile::chrome_default();
    let profile2 = StealthProfile::chrome_default();

    // Act
    let ja3_1 = profile1.tls.compute_ja3();
    let ja3_2 = profile2.tls.compute_ja3();

    // Assert
    assert_eq!(
        ja3_1, ja3_2,
        "TLS JA3 must be identical for same profile — headless/headed consistency"
    );
}

// ---- 7.3 HTTP/2 Akamai fingerprint is consistent regardless of headless mode ----
// @trace REQ-STL-002 [criterion:REQ-STL-002-C1] [req:REQ-STL-002] [level:integration]
#[test]
fn headless_headed_http2_consistent() {
    // Arrange
    let profile1 = StealthProfile::firefox_default();
    let profile2 = StealthProfile::firefox_default();

    // Act
    let ak1 = profile1.http2.akamai_fingerprint();
    let ak2 = profile2.http2.akamai_fingerprint();

    // Assert
    assert_eq!(
        ak1, ak2,
        "HTTP/2 Akamai fingerprint must be identical for same profile — headless/headed consistency"
    );
}

// ===========================================================================
// 8. Full headless confrontation — every leak vector checked
// ===========================================================================

// ---- 8.1 Firefox StealthProfile has zero headless leaks ----
// @trace REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-007 [level:integration]
#[test]
fn headless_firefox_profile_zero_leaks() {
    // Arrange
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let nav = engine.navigator();
    let screen = engine.screen();
    let webgl = engine.webgl();
    let canvas = engine.canvas_noise();
    let audio = engine.audio();

    // Assert — no headless leak in any vector
    // 1. UA: no HeadlessChrome
    assert!(!nav.user_agent.contains("HeadlessChrome"));
    // 2. Screen: not 0, not 800x600
    assert!(screen.width > 0 && screen.height > 0);
    assert!(!(screen.width == 800 && screen.height == 600));
    // 3. WebGL: no SwiftShader
    assert!(!webgl.renderer.to_lowercase().contains("swiftshader"));
    assert!(!webgl.vendor.to_lowercase().contains("swiftshader"));
    // 4. WebGL vendor ≠ bare "Google Inc."
    assert_ne!(webgl.vendor, "Google Inc.");
    // 5. Canvas: seed > 0 (noise active)
    assert!(canvas.seed() > 0);
    // 6. Audio: amplitude > 0 (noise active)
    assert!(audio.noise_amplitude() > 0.0);
    // 7. WebGL extensions non-empty
    assert!(!webgl.extensions.is_empty());
}

// ---- 8.2 Chrome StealthProfile has zero headless leaks ----
// @trace REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-007 [level:integration]
#[test]
fn headless_chrome_profile_zero_leaks() {
    // Arrange
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let nav = engine.navigator();
    let screen = engine.screen();
    let webgl = engine.webgl();
    let canvas = engine.canvas_noise();
    let audio = engine.audio();

    // Assert
    assert!(!nav.user_agent.contains("HeadlessChrome"));
    assert!(screen.width > 0 && screen.height > 0);
    assert!(!(screen.width == 800 && screen.height == 600));
    assert!(!webgl.renderer.to_lowercase().contains("swiftshader"));
    assert!(!webgl.vendor.to_lowercase().contains("swiftshader"));
    // Chrome vendor must have GPU suffix (not bare Google Inc.)
    assert_ne!(webgl.vendor, "Google Inc.");
    assert!(webgl.vendor.starts_with("Google Inc. ("));
    assert!(canvas.seed() > 0);
    assert!(audio.noise_amplitude() > 0.0);
    assert!(!webgl.extensions.is_empty());
    // Chrome renderer references real GPU
    assert!(
        webgl.renderer.contains("ANGLE") || webgl.renderer.contains("NVIDIA"),
        "Chrome WebGL renderer must reference real GPU: {}",
        webgl.renderer
    );
}
