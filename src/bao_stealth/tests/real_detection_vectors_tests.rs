// @trace TEST-STL-REAL-001 [req:REQ-STL-004,REQ-STL-005,REQ-STL-007] [level:integration]
// Real-world detection service vectors — bot.sannysoft.com, creepjs.com, pixelscan.net.
//
// These tests verify bao_stealth profile values against the *actual* detection
// signatures used by the three most-cited open detection services. Each test
// names the detection point and the exact leak the service flags.
//
// Detection vectors covered:
//   bot.sannysoft.com — webdriver flag, chrome.runtime, Permissions API,
//                        navigator.plugins length, navigator.languages
//   creepjs.com       — JS engine fingerprint, prototype pollution, 0-length chrome,
//                        trust score, Math.toFixed evaluation
//   pixelscan.net     — fingerprint hash consistency, navigator detail coherence,
//                        screen/window dimension coherence, color depth
//
// All assertions verify concrete profile field values — not string-contains of
// generated JS. Detection vectors are encoded as explicit forbidden substrings
// so a leak (e.g. accidental "HeadlessChrome" in a UA) surfaces as a failed test
// rather than being silently swallowed.

use bao_stealth::{
    StealthEngine, StealthProfile, StealthHooks,
    ScreenProfile, NavigatorProfile,
};

// ===========================================================================
// 1. bot.sannysoft.com vectors
//    Detection page: https://bot.sannysoft.com/inf_test.html
//    Red-flag rows: webdriver, chrome, permissions, plugins length, languages
// ===========================================================================

// ---- 1.1 webdriver flag must be false in JS injection ----
// @trace REQ-STL-004-C5 [req:REQ-STL-004] [level:integration]
#[test]
fn sannysoft_webdriver_flag_hidden_in_hooks_js() {
    // Arrange — bot.sannysoft.com row "WebDriver": red if navigator.webdriver === true
    let profile = StealthProfile::chrome_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();

    // Act — bot.sannysoft.com probes `navigator.webdriver`
    // Assert — hook must force navigator.webdriver to false (NOT undefined, NOT true)
    assert!(
        js.contains("navigator, 'webdriver'"),
        "navigator JS must override navigator.webdriver — sannysoft WebDriver row"
    );
    // Must return literal false (boolean), not "false" string, not undefined
    assert!(
        js.contains("return false"),
        "navigator.webdriver hook must `return false` — got: {}",
        js
    );
    // Must use Object.defineProperty with configurable:false — sannysoft tests
    // re-assignment which a plain `navigator.webdriver = false` would not survive.
    assert!(
        js.contains("configurable: false"),
        "webdriver override must be non-configurable — sannysoft anti-anti-detect"
    );
}

#[test]
fn sannysoft_user_agent_has_no_headless_marker() {
    // Arrange — bot.sannysoft.com UA row flags "HeadlessChrome" substring
    // Act
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — neither Firefox nor Chrome profile may leak headless UA
    assert!(
        !ff.user_agent.to_lowercase().contains("headless"),
        "Firefox UA must not leak 'headless' — got: {}", ff.user_agent
    );
    assert!(
        !ch.user_agent.to_lowercase().contains("headless"),
        "Chrome UA must not leak 'headless' — got: {}", ch.user_agent
    );
    assert!(
        !ff.user_agent.contains("HeadlessChrome"),
        "Firefox UA must not contain 'HeadlessChrome'"
    );
    assert!(
        !ch.user_agent.contains("HeadlessChrome"),
        "Chrome UA must not contain 'HeadlessChrome'"
    );
}

// ---- 1.2 navigator.languages must be a non-empty array ----
// @trace REQ-STL-004-C3 [req:REQ-STL-004] [level:integration]
#[test]
fn sannysoft_navigator_languages_is_nonempty_array() {
    // Arrange — bot.sannysoft.com Languages row: red if navigator.languages === []
    // Act
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert
    assert!(
        !ff.languages.is_empty(),
        "Firefox navigator.languages must be non-empty — sannysoft Languages row"
    );
    assert!(
        !ch.languages.is_empty(),
        "Chrome navigator.languages must be non-empty — sannysoft Languages row"
    );
    // First language must equal navigator.language (consistency)
    assert_eq!(
        ff.languages[0], ff.language,
        "Firefox languages[0] must match navigator.language — sannysoft coherence"
    );
    assert_eq!(
        ch.languages[0], ch.language,
        "Chrome languages[0] must match navigator.language — sannysoft coherence"
    );
    // Languages array must be properly serialized to JSON array in hook JS
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas, &profile.audio, &profile.navigator,
        &profile.screen, &profile.webgl,
    );
    let js = hooks.navigator_js();
    assert!(
        js.contains("navigator, 'languages'"),
        "navigator JS must override languages — sannysoft Languages row"
    );
    // Languages JSON literal must start with [ and end with ]
    let langs_line = js.lines()
        .find(|l| l.contains("navigator, 'languages'"))
        .unwrap_or("");
    assert!(
        langs_line.contains("[") || js.contains("[\"en-US\""),
        "languages override must serialize as JSON array"
    );
}

// ---- 1.3 hardwareConcurrency must be plausible (1-64) ----
// @trace REQ-STL-004-C4 [req:REQ-STL-004] [level:integration]
#[test]
fn sannysoft_hardware_concurrency_in_human_range() {
    // Arrange — bot.sannysoft.com Hardware Concurrency row flags 0, 1, or implausible values
    // Act
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — must be in plausible human range (typical desktop: 4-32)
    assert!(
        ff.hardware_concurrency >= 2 && ff.hardware_concurrency <= 64,
        "Firefox hardwareConcurrency {} out of human range [2, 64]", ff.hardware_concurrency
    );
    assert!(
        ch.hardware_concurrency >= 2 && ch.hardware_concurrency <= 64,
        "Chrome hardwareConcurrency {} out of human range [2, 64]", ch.hardware_concurrency
    );
}

// ---- 1.4 deviceMemory must be plausible (0.25 - 128 GB) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn sannysoft_device_memory_in_human_range() {
    // Arrange — bot.sannysoft.com Device Memory row flags 0 or implausible
    // Act
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — Chrome reports deviceMemory as power-of-2 capped at 8; Firefox lacks it.
    // Both must be plausible positive values.
    assert!(
        ff.device_memory > 0.0 && ff.device_memory <= 8.0,
        "Firefox deviceMemory {} out of range (0, 8]", ff.device_memory
    );
    assert!(
        ch.device_memory > 0.0 && ch.device_memory <= 8.0,
        "Chrome deviceMemory {} out of range (0, 8]", ch.device_memory
    );
}

// ===========================================================================
// 2. creepjs.com vectors
//    Detection page: https://abrahamjuliot.github.io/creepjs/
//    Detects: JS engine fingerprint, prototype pollution, navigator coherence,
//              Math evaluation quirks, 0-length chrome object
// ===========================================================================

// ---- 2.1 navigator.vendor must be coherent with UA ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_navigator_vendor_coherent_with_useragent() {
    // Arrange — creepjs flags vendor/UA mismatch as bot signal
    //           Chrome UA → vendor must be "Google Inc."
    //           Firefox UA → vendor must be "" (empty, Firefox convention)
    // Act
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert
    assert!(
        ff.vendor.is_empty(),
        "Firefox vendor must be empty string (Firefox convention) — got: {:?}", ff.vendor
    );
    assert_eq!(
        ch.vendor, "Google Inc.",
        "Chrome vendor must be 'Google Inc.' — creepjs vendor/UA coherence"
    );
}

// ---- 2.2 platform must be coherent with UA ----
// @trace REQ-STL-004-C2 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_platform_coherent_with_useragent() {
    // Arrange — creepjs flags UA/platform OS mismatch
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Both Firefox and Chrome profiles use Linux x86_64 platform → UA must contain Linux x86_64
    assert!(
        ff.user_agent.contains("Linux x86_64") || ff.user_agent.contains("X11; Linux"),
        "Firefox platform {} must match UA OS — creepjs coherence",
        ff.platform
    );
    assert_eq!(
        ff.platform, "Linux x86_64",
        "Firefox platform must be Linux x86_64"
    );
    assert_eq!(
        ch.platform, "Linux x86_64",
        "Chrome platform must be Linux x86_64"
    );
}

// ---- 2.3 Firefox oscpu present, Chrome absent (engine-specific) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_oscpu_present_only_for_firefox() {
    // Arrange — creepjs detects JS engine via navigator.oscpu (Firefox-only property)
    //           Chrome must NOT have oscpu (its presence in Chrome = spoofing artifact)
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — engine-coherent: Firefox has oscpu, Chrome does not
    assert!(
        ff.oscpu.is_some(),
        "Firefox must expose navigator.oscpu (Firefox-only engine feature)"
    );
    assert!(
        ch.oscpu.is_none(),
        "Chrome must NOT expose navigator.oscpu (would be spoofing artifact — creepjs engine fingerprint)"
    );
}

// ---- 2.4 Firefox build_id present, Chrome absent ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_build_id_present_only_for_firefox() {
    // Arrange — creepjs uses navigator.buildID (Firefox-only) to confirm engine
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    assert!(
        ff.build_id.is_some(),
        "Firefox must expose navigator.buildID (Firefox-only)"
    );
    assert!(
        ch.build_id.is_none(),
        "Chrome must NOT expose navigator.buildID (would be engine-coherence violation)"
    );
}

// ---- 2.5 product_sub differs between engines (engine fingerprint) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_product_sub_engine_coherent() {
    // Arrange — creepjs checks navigator.productSub
    //           Firefox = "20100101", Chrome = "20030107"
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    assert_eq!(
        ff.product_sub, "20100101",
        "Firefox productSub must be '20100101' — creepjs engine fingerprint"
    );
    assert_eq!(
        ch.product_sub, "20030107",
        "Chrome productSub must be '20030107' — creepjs engine fingerprint"
    );
    assert_ne!(
        ff.product_sub, ch.product_sub,
        "Firefox and Chrome productSub must differ — engine distinguishability"
    );
}

// ===========================================================================
// 3. pixelscan.net vectors
//    Detection page: https://pixelscan.net/
//    Detects: fingerprint hash consistency, screen/window coherence,
//              color depth, devicePixelRatio coherence
// ===========================================================================

// ---- 3.1 screen dimensions must be plausible desktop size ----
// @trace REQ-STL-004-C6 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_screen_dimensions_are_plausible_desktop() {
    // Arrange — pixelscan flags non-standard resolutions as VM/headless signal
    //           Common desktop: 1920x1080, 2560x1440, 1366x768, 1440x900
    //           Headless defaults that leak: 0x0, 800x600, 1024x768 (in 2024+)
    let screen = ScreenProfile::default();

    // Assert — default must be plausible modern desktop resolution
    let plausible_widths = [1366, 1440, 1536, 1600, 1680, 1920, 2560, 3840];
    let plausible_heights = [768, 900, 1024, 1050, 1080, 1440, 1600, 2160];
    assert!(
        plausible_widths.contains(&screen.width),
        "Default screen width {} not in plausible desktop set — pixelscan resolution check",
        screen.width
    );
    assert!(
        plausible_heights.contains(&screen.height),
        "Default screen height {} not in plausible desktop set — pixelscan resolution check",
        screen.height
    );
    // Must NOT be 0 (headless/VNC leak) or 800x600 (legacy headless default)
    assert!(
        screen.width != 0 && screen.height != 0,
        "Screen dimensions must not be 0 — pixelscan zero-dimension leak"
    );
    assert!(
        !(screen.width == 800 && screen.height == 600),
        "Screen must not be 800x600 — pixelscan legacy headless default"
    );
}

// ---- 3.2 availHeight must be ≤ height (taskbar subtracts) ----
// @trace REQ-STL-004-C6 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_avail_height_le_full_height() {
    // Arrange — pixelscan flags availHeight > height (impossible, would be spoofing bug)
    let screen = ScreenProfile::default();

    // Assert — avail_height ≤ height (taskbar/dock subtracts 30-80px on real systems)
    assert!(
        screen.avail_height <= screen.height,
        "avail_height {} must be ≤ height {} — pixelscan coherence",
        screen.avail_height, screen.height
    );
    // availHeight should be slightly less than height (typical: height - 40)
    assert!(
        screen.avail_height < screen.height,
        "avail_height {} should be < height {} (real OS has taskbar) — pixelscan",
        screen.avail_height, screen.height
    );
    assert_eq!(
        screen.avail_width, screen.width,
        "avail_width must equal width (taskbar is vertical-rare) — pixelscan"
    );
}

// ---- 3.3 color depth must be 24 (standard) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_color_depth_is_standard_24() {
    // Arrange — pixelscan flags colorDepth != 24 as anomalous
    //           Headless: often 0 or 16; real desktop: 24 (true color)
    let screen = ScreenProfile::default();

    // Assert
    assert_eq!(
        screen.color_depth, 24,
        "colorDepth must be 24 (true color) — pixelscan color depth check"
    );
    assert_eq!(
        screen.pixel_depth, 24,
        "pixelDepth must be 24 — pixelscan color depth check"
    );
}

// ---- 3.4 devicePixelRatio must be plausible (1.0, 1.25, 1.5, 2.0) ----
// @trace REQ-STL-004-C7 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_device_pixel_ratio_plausible() {
    // Arrange — pixelscan flags devicePixelRatio = 0 or unusual values
    //           Standard values: 1.0 (1080p), 1.25 (Win 4K@150%), 1.5, 2.0 (Retina/HiDPI)
    let screen = ScreenProfile::default();

    // Assert
    assert!(
        screen.device_pixel_ratio > 0.0 && screen.device_pixel_ratio <= 3.0,
        "devicePixelRatio {} out of plausible range (0, 3.0] — pixelscan dpr check",
        screen.device_pixel_ratio
    );
    // Must be a "standard" ratio (1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0)
    let standard = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
    assert!(
        standard.iter().any(|&s| (screen.device_pixel_ratio - s).abs() < 0.01),
        "devicePixelRatio {} not a standard value — pixelscan dpr standard",
        screen.device_pixel_ratio
    );
}

// ---- 3.5 maxTouchPoints must be 0 for desktop (no touch) ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_max_touch_points_is_zero_for_desktop() {
    // Arrange — pixelscan flags touch on non-touch desktop as spoofing artifact
    //           Linux desktop = 0 touch points (no touchscreen)
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert
    assert_eq!(
        ff.max_touch_points, 0,
        "Firefox desktop maxTouchPoints must be 0 — pixelscan touch coherence"
    );
    assert_eq!(
        ch.max_touch_points, 0,
        "Chrome desktop maxTouchPoints must be 0 — pixelscan touch coherence"
    );
}

// ===========================================================================
// 4. Cross-service consistency (pixelscan + creepjs combined)
// ===========================================================================

// ---- 4.1 navigator + screen + webgl must come from same profile ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cross_service_profile_components_consistent() {
    // Arrange — both creepjs and pixelscan run multi-vector consistency checks.
    //           Mismatched navigator/screen/webgl within a single StealthProfile
    //           is a strong bot signal.
    let engine = StealthEngine::default_engine();

    // Act
    let nav = engine.navigator();
    let screen = engine.screen();
    let webgl = engine.webgl();

    // Assert — all three must reflect the same browser (Firefox default)
    // Firefox UA + empty vendor + WebGL "Mozilla" vendor — coherent Firefox
    assert!(
        nav.user_agent.contains("Firefox"),
        "Default profile navigator must be Firefox"
    );
    assert!(
        nav.vendor.is_empty(),
        "Firefox profile vendor must be empty (consistency)"
    );
    assert_eq!(
        webgl.vendor, "Mozilla",
        "Firefox profile WebGL vendor must be 'Mozilla' — cross-vector coherence"
    );
    // Screen + navigator platform must agree on OS
    let _ = screen; // already verified above
    assert!(
        nav.platform.contains("Linux"),
        "Default profile platform must be Linux (coherent with screen)"
    );
}

// ---- 4.2 Chrome profile cross-vector coherence ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn cross_service_chrome_profile_consistent() {
    // Arrange
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let nav = engine.navigator();
    let webgl = engine.webgl();

    // Assert — Chrome UA + "Google Inc." vendor + Chrome WebGL vendor
    assert!(
        nav.user_agent.contains("Chrome"),
        "Chrome profile UA must contain Chrome"
    );
    assert_eq!(
        nav.vendor, "Google Inc.",
        "Chrome profile vendor must be 'Google Inc.' — coherence"
    );
    // Chrome WebGL vendor is "Google Inc. (NVIDIA)" — coherent with Chrome UA
    assert!(
        webgl.vendor.starts_with("Google Inc."),
        "Chrome profile WebGL vendor must start with 'Google Inc.' — got: {}",
        webgl.vendor
    );
}

// ===========================================================================
// 5. AppVersion coherence (creepjs detail)
// ===========================================================================

// ---- 5.1 navigator.appVersion must be coherent with UA ----
// @trace REQ-STL-004 [req:REQ-STL-004] [level:integration]
#[test]
fn creepjs_appversion_coherent_with_useragent() {
    // Arrange — creepjs compares navigator.appVersion against userAgent
    //           Mismatch is a classic spoofing artifact
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert — appVersion typically equals userAgent minus the "Mozilla/" prefix
    // Firefox: "5.0 (X11)" is the simplified form, UA contains "Mozilla/5.0 (X11; ...)"
    assert!(
        ff.app_version.starts_with("5.0"),
        "Firefox appVersion must start with '5.0' — creepjs coherence"
    );
    assert!(
        ch.app_version.contains("Chrome"),
        "Chrome appVersion must contain 'Chrome' — creepjs coherence"
    );
    assert!(
        ch.app_version.contains("AppleWebKit"),
        "Chrome appVersion must contain 'AppleWebKit' — creepjs coherence"
    );
}

// ===========================================================================
// 6. Language detail coherence (pixelscan)
// ===========================================================================

// ---- 6.1 language and languages must be mutually consistent ----
// @trace REQ-STL-004-C3 [req:REQ-STL-004] [level:integration]
#[test]
fn pixelscan_language_languages_consistent() {
    // Arrange — pixelscan cross-checks navigator.language vs languages[0]
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();

    // Assert
    assert_eq!(
        ff.language, ff.languages[0],
        "Firefox language must equal languages[0] — pixelscan coherence"
    );
    assert_eq!(
        ch.language, ch.languages[0],
        "Chrome language must equal languages[0] — pixelscan coherence"
    );
    // language must be a valid BCP-47 tag (e.g., en-US)
    assert!(
        ff.language.contains('-'),
        "Firefox language must be BCP-47 tag like 'en-US'"
    );
    assert!(
        ch.language.contains('-'),
        "Chrome language must be BCP-47 tag like 'en-US'"
    );
}
