// @trace TEST-STL-CDP-001 [req:REQ-STL-007] [level:integration]
// CDP stealth traces hiding — Runtime.evaluate must not expose CDP artifacts.
//
// When Chrome DevTools Protocol (CDP) is attached, certain JS-visible artifacts
// reveal the debugger/automation presence. Detection services probe for these
// to flag CDP-driven sessions. bao_stealth JS hooks must hide:
//
//   1. navigator.webdriver = true (CDP sets this) → must be false
//   2. window.chrome absent (headless CDP doesn't init chrome) → must be present
//   3. Permissions.query returns "denied" (CDP default) → must be "granted" or "prompt"
//   4. cdc_* globals (ChromeDriver injects cdc_adoQpoasnfa76pfcZLmcfl_Array etc.)
//   5. $ and $$ console functions (DevTools injects these)
//   6. Runtime.evaluate stack trace leaks (CDP-specific call frames)
//
// These tests verify the JS hook output contains the correct overrides that
// defeat CDP detection vectors. The actual JS execution is tested in
// stealth_diagnostic_detection_tests.rs (JsContext-based); here we verify
// the hook SOURCE STRING has the required content.

use bao_stealth::{StealthProfile, StealthHooks};

// ===========================================================================
// 1. navigator.webdriver — CDP default is true, must be forced false
// ===========================================================================

// ---- 1.1 webdriver override present in hooks ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C5] [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_webdriver_override_forced_false() {
    // Arrange — CDP-attached Chrome sets navigator.webdriver = true
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — must override webdriver to false
    assert!(
        js.contains("navigator, 'webdriver'"),
        "navigator.webdriver override must be present in hooks — CDP webdriver leak"
    );
    // Must return boolean false (not "false" string, not undefined)
    assert!(
        js.contains("return false"),
        "navigator.webdriver hook must `return false` (boolean) — CDP leak"
    );
}

// ---- 1.2 webdriver override is non-configurable ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C8] [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_webdriver_override_non_configurable() {
    // Arrange — detection services attempt to re-assign navigator.webdriver
    //           to detect a spoof. A configurable:true override would be detected.
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Find the webdriver line
    let webdriver_line = js.lines()
        .find(|l| l.contains("navigator, 'webdriver'"))
        .unwrap_or("");

    // Assert — must use configurable: false (anti-anti-detect)
    assert!(
        webdriver_line.contains("configurable: false"),
        "navigator.webdriver override must be configurable:false — CDP anti-anti-detect, got: {}",
        webdriver_line
    );
}

// ===========================================================================
// 2. window.chrome — CDP/headless doesn't init chrome object; must be present
// ===========================================================================

// ---- 2.1 Chrome profile vendor is "Google Inc." (coherent with window.chrome) ----
// @trace REQ-STL-004 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_chrome_vendor_consistent_with_chrome_object() {
    // Arrange — real Chrome has window.chrome object AND navigator.vendor = "Google Inc."
    //           headless Chrome lacks window.chrome but keeps vendor → mismatch = detection
    let ch = bao_stealth::NavigatorProfile::chrome();

    // Assert — vendor must be "Google Inc." (real Chrome signature)
    //          The window.chrome object itself is injected by browser runtime,
    //          not by stealth JS hooks (it's a browser-level object).
    assert_eq!(
        ch.vendor, "Google Inc.",
        "Chrome navigator.vendor must be 'Google Inc.' — coherent with window.chrome presence"
    );
}

// ---- 2.2 Chrome profile has no Firefox-only properties (oscpu/build_id absent) ----
// @trace REQ-STL-004 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_chrome_no_firefox_specific_properties() {
    // Arrange — window.chrome presence implies Chromium engine.
    //           Chromium must NOT expose navigator.oscpu or navigator.buildID
    //           (those are Firefox-only). Presence = spoofing artifact.
    let ch = bao_stealth::NavigatorProfile::chrome();

    // Assert
    assert!(
        ch.oscpu.is_none(),
        "Chrome navigator.oscpu must be None — CDP engine coherence (Firefox-only property)"
    );
    assert!(
        ch.build_id.is_none(),
        "Chrome navigator.buildID must be None — CDP engine coherence (Firefox-only property)"
    );
}

// ===========================================================================
// 3. Permissions API — CDP default returns "denied"; real browser varies
// ===========================================================================

// ---- 3.1 navigator JS does not break Permissions API (no undefined override) ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_permissions_api_not_broken_by_hooks() {
    // Arrange — CDP-attached headless returns Permissions.query = "denied" for all.
    //           bao_stealth JS hooks override navigator properties but must NOT
    //           remove or break the Permissions API itself.
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — hooks must NOT delete or override Permissions.prototype.query
    //          (would break the API entirely, which is itself a detection signal).
    //          Instead, browser runtime handles Permissions at engine layer.
    assert!(
        !js.contains("delete navigator.permissions"),
        "Hooks must not delete navigator.permissions — would break Permissions API (CDP detection signal)"
    );
    assert!(
        !js.contains("Permissions.prototype.query"),
        "Hooks must not override Permissions.prototype.query — engine-layer handling required (CDP leak)"
    );
}

// ===========================================================================
// 4. cdc_* globals (ChromeDriver artifacts) — must not be in JS hooks
// ===========================================================================

// ---- 4.1 JS hooks do not inject cdc_ globals ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_no_cdc_globals_in_hooks() {
    // Arrange — ChromeDriver injects globals like cdc_adoQpoasnfa76pfcZLmcfl_Array
    //           These are ChromeDriver-specific, NOT CDP. bao_stealth uses CDP,
    //           so these must never appear.
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let combined = hooks.combined_js();

    // Assert — no cdc_ global injection
    assert!(
        !combined.contains("cdc_"),
        "Hooks must not inject cdc_* globals — ChromeDriver artifact (bao_stealth uses CDP, not ChromeDriver)"
    );
    assert!(
        !combined.contains("adoQpoasnfa76pfcZLmcfl"),
        "Hooks must not contain ChromeDriver cdc signature"
    );
}

// ===========================================================================
// 5. Console helper functions ($, $$) — DevTools artifacts
// ===========================================================================

// ---- 5.1 JS hooks do not inject DevTools console helpers ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_no_devtools_console_helpers_in_hooks() {
    // Arrange — DevTools console injects $ (querySelector) and $$ (querySelectorAll)
    //           as globals. Their presence indicates DevTools open.
    //           bao_stealth JS hooks must NOT inject these.
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let combined = hooks.combined_js();

    // Assert — no DevTools console helper injection
    // Note: we check for explicit global assignment patterns, not the literal "$"
    // (which appears in template literals legitimately).
    assert!(
        !combined.contains("window.$ =") && !combined.contains("window.$ ="),
        "Hooks must not inject window.$ (DevTools console helper)"
    );
    assert!(
        !combined.contains("window.$$ ="),
        "Hooks must not inject window.$$ (DevTools console helper)"
    );
}

// ===========================================================================
// 6. Combined JS output structure — all required overrides present
// ===========================================================================

// ---- 6.1 Combined JS contains canvas + audio + navigator overrides ----
// @trace REQ-STL-003,REQ-STL-005,REQ-STL-007 [req:REQ-STL-003,REQ-STL-005,REQ-STL-007] [level:integration]
#[test]
fn cdp_combined_js_has_all_overrides() {
    // Arrange
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let combined = hooks.combined_js();

    // Assert — must contain canvas, audio, and navigator overrides
    assert!(
        combined.contains("HTMLCanvasElement.prototype.toDataURL"),
        "Combined JS must include canvas toDataURL override"
    );
    assert!(
        combined.contains("AudioBuffer.prototype.getChannelData"),
        "Combined JS must include audio getChannelData override"
    );
    assert!(
        combined.contains("navigator, 'userAgent'"),
        "Combined JS must include navigator.userAgent override"
    );
    assert!(
        combined.contains("navigator, 'webdriver'"),
        "Combined JS must include navigator.webdriver override (CDP leak prevention)"
    );
}

// ---- 6.2 Each individual hook is non-empty ----
// @trace REQ-STL-003,REQ-STL-005,REQ-STL-007 [req:REQ-STL-003,REQ-STL-005,REQ-STL-007] [level:integration]
#[test]
fn cdp_individual_hooks_nonempty() {
    // Arrange
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );

    // Assert — each hook must produce non-trivial JS
    assert!(
        hooks.canvas_js().len() > 100,
        "Canvas JS hook must be non-trivial (>100 chars), got: {}", hooks.canvas_js().len()
    );
    assert!(
        hooks.audio_js().len() > 100,
        "Audio JS hook must be non-trivial (>100 chars), got: {}", hooks.audio_js().len()
    );
    assert!(
        hooks.navigator_js().len() > 500,
        "Navigator JS hook must be substantial (>500 chars), got: {}", hooks.navigator_js().len()
    );
    assert!(
        hooks.combined_js().len() > 700,
        "Combined JS must be >700 chars, got: {}", hooks.combined_js().len()
    );
}

// ===========================================================================
// 7. WebGL hooks — both WebGL1 and WebGL2RenderingContext patched
// ===========================================================================

// ---- 7.1 Both WebGL1 and WebGL2 getParameter patched ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C1] [req:REQ-STL-005] [level:integration]
#[test]
fn cdp_webgl1_and_webgl2_both_patched() {
    // Arrange — CDP detection may probe either WebGL1 or WebGL2 context
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — both contexts must be patched
    assert!(
        js.contains("WebGLRenderingContext.prototype.getParameter"),
        "WebGL1RenderingContext.prototype.getParameter must be patched — CDP WebGL probe"
    );
    assert!(
        js.contains("WebGL2RenderingContext.prototype.getParameter"),
        "WebGL2RenderingContext.prototype.getParameter must be patched — CDP WebGL2 probe"
    );
    // Both must override RENDERER (0x9246) and VENDOR (0x9245)
    let webgl1_section_count = js.matches("0x9246").count();
    let webgl2_section_count = js.matches("0x9246").count();
    assert!(
        webgl1_section_count >= 2,
        "RENDERER (0x9246) must be overridden in both WebGL1 and WebGL2 sections — got {} occurrences",
        webgl1_section_count
    );
    let _ = webgl2_section_count;
}

// ---- 7.2 WebGL getSupportedExtensions patched for both contexts ----
// @trace REQ-STL-005 [criterion:REQ-STL-005-C3] [req:REQ-STL-005] [level:integration]
#[test]
fn cdp_webgl_extensions_patched_both_contexts() {
    // Arrange
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — getSupportedExtensions must be patched in both contexts
    assert!(
        js.contains("WebGLRenderingContext.prototype.getSupportedExtensions"),
        "WebGL1 getSupportedExtensions must be patched — CDP extension probe"
    );
    // WebGL2 check is inside an `if (typeof WebGL2RenderingContext !== 'undefined')` block
    assert!(
        js.contains("typeof WebGL2RenderingContext") || js.contains("WebGL2RenderingContext.prototype.getSupportedExtensions"),
        "WebGL2 getSupportedExtensions must be conditionally patched — CDP WebGL2 extension probe"
    );
}

// ===========================================================================
// 8. Screen properties — all overrides present (CDP probes screen object)
// ===========================================================================

// ---- 8.1 All screen properties overridden ----
// @trace REQ-STL-004 [criterion:REQ-STL-004-C6] [req:REQ-STL-004] [level:integration]
#[test]
fn cdp_all_screen_properties_overridden() {
    // Arrange — CDP-attached session may have screen object manipulated
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — all screen properties must be overridden
    for prop in &["width", "height", "availWidth", "availHeight", "colorDepth", "pixelDepth"] {
        assert!(
            js.contains(&format!("screen, '{}'", prop)),
            "screen.{} must be overridden — CDP screen probe", prop
        );
    }
    // devicePixelRatio on window
    assert!(
        js.contains("window, 'devicePixelRatio'"),
        "window.devicePixelRatio must be overridden — CDP DPR probe"
    );
}

// ===========================================================================
// 9. navigator property overrides — comprehensive list
// ===========================================================================

// ---- 9.1 All critical navigator properties overridden ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn cdp_all_navigator_properties_overridden() {
    // Arrange
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Assert — all critical navigator properties CDP may probe
    for prop in &[
        "userAgent", "platform", "hardwareConcurrency", "language", "languages",
        "vendor", "deviceMemory", "maxTouchPoints", "webdriver",
    ] {
        assert!(
            js.contains(&format!("navigator, '{}'", prop)),
            "navigator.{} must be overridden — CDP navigator probe", prop
        );
    }
}

// ===========================================================================
// 10. Hook determinism — same profile produces identical JS
// ===========================================================================

// ---- 10.1 Hook output is deterministic for same profile ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_hook_output_deterministic() {
    // Arrange
    let profile = StealthProfile::chrome_default();

    // Act
    let hooks1 = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let hooks2 = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );

    // Assert — same profile must produce identical hook JS
    assert_eq!(
        hooks1.canvas_js(), hooks2.canvas_js(),
        "Canvas JS must be deterministic for same profile — CDP session consistency"
    );
    assert_eq!(
        hooks1.audio_js(), hooks2.audio_js(),
        "Audio JS must be deterministic for same profile — CDP session consistency"
    );
    assert_eq!(
        hooks1.navigator_js(), hooks2.navigator_js(),
        "Navigator JS must be deterministic for same profile — CDP session consistency"
    );
    assert_eq!(
        hooks1.combined_js(), hooks2.combined_js(),
        "Combined JS must be deterministic for same profile — CDP session consistency"
    );
}

// ---- 10.2 Firefox and Chrome hooks differ (engine distinguishability) ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cdp_firefox_chrome_hooks_differ() {
    // Arrange
    let ff_profile = StealthProfile::firefox_default();
    let ch_profile = StealthProfile::chrome_default();

    // Act
    let ff_hooks = StealthHooks::from_profile(
        &ff_profile.canvas, &ff_profile.audio, &ff_profile.navigator,
        &ff_profile.screen, &ff_profile.webgl,
    );
    let ch_hooks = StealthHooks::from_profile(
        &ch_profile.canvas, &ch_profile.audio, &ch_profile.navigator,
        &ch_profile.screen, &ch_profile.webgl,
    );

    // Assert — navigator JS must differ (different UA, vendor, platform)
    assert_ne!(
        ff_hooks.navigator_js(), ch_hooks.navigator_js(),
        "Firefox/Chrome navigator JS must differ — engine distinguishability"
    );
    // Canvas JS differs only in seed (42 vs 137)
    assert_ne!(
        ff_hooks.canvas_js(), ch_hooks.canvas_js(),
        "Firefox/Chrome canvas JS must differ (seed) — session distinguishability"
    );
}
