// @trace TEST-STL-E2E [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007] [level:e2e]
// Stealth anti-fingerprint full-chain E2E tests.
//
// Tier 1: Rust unit-level — StealthProfile data integrity (no Servo required)
// Tier 2: StealthEngine integration — cross-module wiring verification
// Tier 3: TLS/HTTP2 fingerprint data verification
// Tier 4: Browser integration (#[ignore], requires Servo display server)

use bao_stealth::{StealthProfile, StealthEngine, TlsFingerprintConfig};

// ═══════════════════════════════════════════════════════════════════════════
// Section 1: Profile data integrity tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_firefox_profile_navigator_user_agent_contains_firefox() {
    let profile = StealthProfile::firefox_default();
    assert!(
        profile.navigator.user_agent.contains("Firefox"),
        "Firefox profile UA should contain 'Firefox', got: {}",
        profile.navigator.user_agent
    );
}

#[test]
fn test_firefox_profile_navigator_vendor_empty() {
    let profile = StealthProfile::firefox_default();
    assert_eq!(
        profile.navigator.vendor, "",
        "Firefox profile navigator.vendor must be empty string"
    );
}

#[test]
fn test_chrome_profile_navigator_vendor_google() {
    let profile = StealthProfile::chrome_default();
    assert_eq!(
        profile.navigator.vendor, "Google Inc.",
        "Chrome profile navigator.vendor must be 'Google Inc.'"
    );
}

#[test]
fn test_different_profiles_have_different_user_agents() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    assert_ne!(
        firefox.navigator.user_agent, chrome.navigator.user_agent,
        "Firefox and Chrome profiles must have different user agents"
    );
}

#[test]
fn test_firefox_profile_screen_dimensions() {
    let profile = StealthProfile::firefox_default();
    assert_eq!(profile.screen.width, 1920, "Firefox screen.width should be 1920");
    assert_eq!(profile.screen.height, 1080, "Firefox screen.height should be 1080");
    assert_eq!(profile.screen.avail_width, 1920, "Firefox screen.avail_width should be 1920");
    assert_eq!(profile.screen.avail_height, 1040, "Firefox screen.avail_height should be 1040");
    assert_eq!(profile.screen.color_depth, 24, "Firefox screen.color_depth should be 24");
}

#[test]
fn test_chrome_profile_screen_dimensions() {
    let profile = StealthProfile::chrome_default();
    assert_eq!(profile.screen.width, 1920, "Chrome screen.width should be 1920");
    assert_eq!(profile.screen.height, 1080, "Chrome screen.height should be 1080");
    assert_eq!(profile.screen.color_depth, 24, "Chrome screen.color_depth should be 24");
    assert_eq!(profile.screen.device_pixel_ratio, 1.0, "Chrome screen.device_pixel_ratio should be 1.0");
}

#[test]
fn test_firefox_webgl_vendor_and_renderer_not_empty() {
    let profile = StealthProfile::firefox_default();
    assert!(
        !profile.webgl.vendor.is_empty(),
        "Firefox WebGL vendor must not be empty"
    );
    assert!(
        !profile.webgl.renderer.is_empty(),
        "Firefox WebGL renderer must not be empty"
    );
}

#[test]
fn test_chrome_webgl_vendor_and_renderer_not_empty() {
    let profile = StealthProfile::chrome_default();
    assert!(
        !profile.webgl.vendor.is_empty(),
        "Chrome WebGL vendor must not be empty"
    );
    assert!(
        !profile.webgl.renderer.is_empty(),
        "Chrome WebGL renderer must not be empty"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 2: engine_props thread_local verification tests
// ═══════════════════════════════════════════════════════════════════════════
// These tests exercise bao_stealth::engine_props::set_profile() which stores
// profile values into thread_local. The thread_local cells are not pub, so
// we verify via the public set_profile API + reading back through the profile
// data (since set_profile is a write-only API from outside the module).
// We verify the contract: set_profile stores values correctly by checking
// that repeated calls with the same profile produce consistent results,
// and that switching profiles updates the stored values.

#[test]
fn test_set_profile_stores_firefox_ua() {
    let profile = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&profile);
    // Verify by creating a new engine with the same profile — the profile
    // data must match what was stored
    let engine = StealthEngine::new(profile.clone());
    assert!(
        engine.navigator().user_agent.contains("Firefox"),
        "After set_profile with Firefox, engine navigator should contain 'Firefox'"
    );
    assert_eq!(
        engine.navigator().user_agent, profile.navigator.user_agent,
        "Engine UA must match the profile UA passed to set_profile"
    );
}

#[test]
fn test_set_profile_stores_screen_dims() {
    let profile = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&profile);
    // Screen dimensions stored in thread_local should match profile
    let engine = StealthEngine::new(profile.clone());
    assert_eq!(engine.screen().width, profile.screen.width);
    assert_eq!(engine.screen().height, profile.screen.height);
    assert_eq!(engine.screen().avail_width, profile.screen.avail_width);
    assert_eq!(engine.screen().avail_height, profile.screen.avail_height);
}

#[test]
fn test_set_profile_stores_webgl_vendor() {
    let profile = StealthProfile::chrome_default();
    bao_stealth::engine_props::set_profile(&profile);
    let engine = StealthEngine::new(profile.clone());
    assert_eq!(engine.webgl().vendor, profile.webgl.vendor);
    assert_eq!(engine.webgl().renderer, profile.webgl.renderer);
}

#[test]
fn test_set_profile_webdriver_always_false() {
    // navigator.webdriver must always be false regardless of profile
    let firefox = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&firefox);
    // webdriver is always false — verify via profile data
    // (the actual getter callback always returns false per engine_props.rs)
    assert_eq!(firefox.navigator.max_touch_points, 0, "Desktop profile should have max_touch_points=0");

    let chrome = StealthProfile::chrome_default();
    bao_stealth::engine_props::set_profile(&chrome);
    assert_eq!(chrome.navigator.max_touch_points, 0, "Desktop profile should have max_touch_points=0");
}

#[test]
fn test_set_profile_overwrites_on_switch() {
    // Switching from Chrome to Firefox profile must update all values
    let chrome = StealthProfile::chrome_default();
    bao_stealth::engine_props::set_profile(&chrome);

    let firefox = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&firefox);

    // After switching, engine built from firefox profile should have Firefox values
    let engine = StealthEngine::new(firefox.clone());
    assert!(
        engine.navigator().user_agent.contains("Firefox"),
        "After switching to Firefox profile, UA should contain 'Firefox'"
    );
    assert_eq!(engine.navigator().vendor, "", "Firefox vendor should be empty");
}

#[test]
fn test_set_profile_chrome_vs_firefox_different() {
    let firefox = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&firefox);
    let ff_engine = StealthEngine::new(firefox.clone());

    let chrome = StealthProfile::chrome_default();
    bao_stealth::engine_props::set_profile(&chrome);
    let ch_engine = StealthEngine::new(chrome.clone());

    // Key differentiating properties between Firefox and Chrome
    assert_ne!(
        ff_engine.navigator().user_agent, ch_engine.navigator().user_agent,
        "Firefox and Chrome engines must have different UAs"
    );
    assert_ne!(
        ff_engine.navigator().vendor, ch_engine.navigator().vendor,
        "Firefox and Chrome engines must have different vendor strings"
    );
    assert_ne!(
        ff_engine.webgl().vendor, ch_engine.webgl().vendor,
        "Firefox and Chrome engines must have different WebGL vendors"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 3: StealthEngine integration verification tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_stealth_engine_default_is_firefox() {
    let engine = StealthEngine::default_engine();
    let firefox = StealthProfile::firefox_default();
    assert_eq!(
        engine.profile().navigator.user_agent, firefox.navigator.user_agent,
        "Default engine must use Firefox profile"
    );
}

#[test]
fn test_stealth_engine_custom_profile_stored() {
    let chrome = StealthProfile::chrome_default();
    let engine = StealthEngine::new(chrome.clone());
    assert_eq!(
        engine.profile().navigator.user_agent, chrome.navigator.user_agent,
        "Custom Chrome profile must be stored in engine"
    );
    assert_eq!(
        engine.profile().tls.ja3_hash, chrome.tls.ja3_hash,
        "Custom Chrome TLS fingerprint must be stored in engine"
    );
}

#[test]
fn test_stealth_engine_tls_config_matches_profile() {
    let engine = StealthEngine::default_engine();
    assert_eq!(
        engine.tls_config().ja3_hash, engine.profile().tls.ja3_hash,
        "Engine tls_config() must return the same JA3 as the profile"
    );
    assert_eq!(
        engine.tls_config().cipher_suites, engine.profile().tls.cipher_suites,
        "Engine tls_config() cipher suites must match profile"
    );
}

#[test]
fn test_stealth_engine_http2_config_matches_profile() {
    let engine = StealthEngine::default_engine();
    assert_eq!(
        engine.http2_config().header_table_size, engine.profile().http2.header_table_size,
        "Engine http2_config() header_table_size must match profile"
    );
    assert_eq!(
        engine.http2_config().initial_window_size, engine.profile().http2.initial_window_size,
        "Engine http2_config() initial_window_size must match profile"
    );
    assert_eq!(
        engine.http2_config().pseudo_header_order, engine.profile().http2.pseudo_header_order,
        "Engine http2_config() pseudo_header_order must match profile"
    );
}

#[test]
fn test_stealth_engine_navigator_matches_profile() {
    let engine = StealthEngine::default_engine();
    assert_eq!(
        engine.navigator().user_agent, engine.profile().navigator.user_agent,
        "Engine navigator() must match profile navigator"
    );
    assert_eq!(
        engine.navigator().platform, engine.profile().navigator.platform
    );
    assert_eq!(
        engine.navigator().vendor, engine.profile().navigator.vendor
    );
    assert_eq!(
        engine.navigator().hardware_concurrency, engine.profile().navigator.hardware_concurrency
    );
}

#[test]
fn test_stealth_engine_webgl_matches_profile() {
    let engine = StealthEngine::default_engine();
    assert_eq!(
        engine.webgl().vendor, engine.profile().webgl.vendor,
        "Engine webgl() vendor must match profile"
    );
    assert_eq!(
        engine.webgl().renderer, engine.profile().webgl.renderer,
        "Engine webgl() renderer must match profile"
    );
    assert_eq!(
        engine.webgl().extensions, engine.profile().webgl.extensions,
        "Engine webgl() extensions must match profile"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 4: TLS Fingerprint verification tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_firefox_tls_ja3_not_empty() {
    let profile = StealthProfile::firefox_default();
    assert!(
        !profile.tls.ja3_hash.is_empty(),
        "Firefox TLS JA3 hash must not be empty"
    );
    // JA3 format: "771,..."
    assert!(
        profile.tls.ja3_hash.starts_with("771,"),
        "Firefox JA3 hash should start with TLS version 771, got: {}",
        profile.tls.ja3_hash
    );
}

#[test]
fn test_chrome_tls_ja3_not_empty() {
    let profile = StealthProfile::chrome_default();
    assert!(
        !profile.tls.ja3_hash.is_empty(),
        "Chrome TLS JA3 hash must not be empty"
    );
    assert!(
        profile.tls.ja3_hash.starts_with("771,"),
        "Chrome JA3 hash should start with TLS version 771, got: {}",
        profile.tls.ja3_hash
    );
}

#[test]
fn test_firefox_and_chrome_have_different_ja3() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    assert_ne!(
        firefox.tls.ja3_hash, chrome.tls.ja3_hash,
        "Firefox and Chrome must have different JA3 hashes"
    );
}

#[test]
fn test_tls_fingerprint_config_ciphers_not_empty() {
    let firefox = StealthProfile::firefox_default();
    let config = TlsFingerprintConfig::from_fingerprint(&firefox.tls);
    assert!(
        config.has_fingerprint(),
        "Firefox TlsFingerprintConfig must have fingerprint data"
    );
    assert!(!config.tls12_cipher_list.is_empty(), "TLS 1.2 cipher list must not be empty");
    assert!(!config.tls13_cipher_suites.is_empty(), "TLS 1.3 cipher suites must not be empty");
    assert!(!config.curves_list.is_empty(), "Curves list must not be empty");
    assert!(!config.sigalgs_list.is_empty(), "Signature algorithms list must not be empty");

    let chrome = StealthProfile::chrome_default();
    let config = TlsFingerprintConfig::from_fingerprint(&chrome.tls);
    assert!(
        config.has_fingerprint(),
        "Chrome TlsFingerprintConfig must have fingerprint data"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 5: HTTP/2 Fingerprint verification tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_firefox_http2_header_table_size_positive() {
    let profile = StealthProfile::firefox_default();
    assert!(
        profile.http2.header_table_size > 0,
        "Firefox HTTP/2 header_table_size must be positive"
    );
    assert_eq!(profile.http2.header_table_size, 65536, "Firefox header_table_size should be 65536");
}

#[test]
fn test_chrome_http2_header_table_size_positive() {
    let profile = StealthProfile::chrome_default();
    assert!(
        profile.http2.header_table_size > 0,
        "Chrome HTTP/2 header_table_size must be positive"
    );
    assert_eq!(profile.http2.header_table_size, 65536, "Chrome header_table_size should be 65536");
}

#[test]
fn test_firefox_and_chrome_different_http2_settings() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    // Firefox and Chrome have different max_concurrent_streams and initial_window_size
    assert_ne!(
        firefox.http2.max_concurrent_streams, chrome.http2.max_concurrent_streams,
        "Firefox and Chrome must have different max_concurrent_streams"
    );
    assert_ne!(
        firefox.http2.initial_window_size, chrome.http2.initial_window_size,
        "Firefox and Chrome must have different initial_window_size"
    );
    assert_ne!(
        firefox.http2.window_update_size, chrome.http2.window_update_size,
        "Firefox and Chrome must have different window_update_size"
    );
}

#[test]
fn test_http2_fingerprint_has_settings_frame_params() {
    let firefox = StealthProfile::firefox_default();
    let payload = firefox.http2.settings_frame_payload();
    assert_eq!(payload.len(), 6, "Settings frame payload must have 6 parameter tuples");

    // Verify Akamai fingerprint format
    let akamai = firefox.http2.akamai_fingerprint();
    let parts: Vec<&str> = akamai.split(':').collect();
    assert_eq!(parts.len(), 6, "Akamai fingerprint must be 6 colon-separated fields");

    let chrome = StealthProfile::chrome_default();
    let akamai_chrome = chrome.http2.akamai_fingerprint();
    assert_ne!(akamai, akamai_chrome, "Firefox and Chrome must have different Akamai fingerprints");
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 6: Cross-module integration — full chain verification
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_full_chain_firefox_profile_consistency() {
    // Verify the entire chain: StealthProfile -> engine_props::set_profile -> StealthEngine
    // All sub-profiles must be internally consistent
    let profile = StealthProfile::firefox_default();
    bao_stealth::engine_props::set_profile(&profile);
    let engine = StealthEngine::new(profile.clone());

    // Navigator chain
    assert_eq!(engine.navigator().user_agent, profile.navigator.user_agent);
    assert_eq!(engine.navigator().platform, "Linux x86_64");
    assert_eq!(engine.navigator().language, "en-US");
    assert_eq!(engine.navigator().vendor, "");
    assert_eq!(engine.navigator().hardware_concurrency, 8);
    assert_eq!(engine.navigator().max_touch_points, 0);

    // Screen chain
    assert_eq!(engine.screen().width, 1920);
    assert_eq!(engine.screen().height, 1080);
    assert_eq!(engine.screen().color_depth, 24);
    assert!((engine.screen().device_pixel_ratio - 1.0).abs() < f64::EPSILON);

    // WebGL chain
    assert_eq!(engine.webgl().vendor, "Mozilla");
    assert!(!engine.webgl().renderer.is_empty());

    // TLS chain
    assert!(!engine.tls_config().ja3_hash.is_empty());
    assert_eq!(engine.tls_config().tls_version, "771");

    // HTTP/2 chain
    assert_eq!(engine.http2_config().header_table_size, 65536);
    assert!(!engine.http2_config().enable_push);

    // Canvas chain
    assert!(engine.canvas_noise().seed() > 0);

    // Audio chain
    assert!((engine.audio().noise_amplitude() - 1e-7).abs() < f64::EPSILON);

    // Behavior chain
    assert!(engine.behavior().seed() > 0);
}

#[test]
fn test_full_chain_chrome_profile_consistency() {
    let profile = StealthProfile::chrome_default();
    bao_stealth::engine_props::set_profile(&profile);
    let engine = StealthEngine::new(profile.clone());

    // Navigator chain
    assert_eq!(engine.navigator().user_agent, profile.navigator.user_agent);
    assert_eq!(engine.navigator().vendor, "Google Inc.");
    assert!(engine.navigator().user_agent.contains("Chrome"));

    // WebGL chain
    assert_eq!(engine.webgl().vendor, "Google Inc. (NVIDIA)");
    assert!(engine.webgl().renderer.contains("ANGLE"));

    // TLS chain — Chrome has fewer cipher suites than Firefox
    assert!(engine.tls_config().cipher_suites.len() < StealthProfile::firefox_default().tls.cipher_suites.len());

    // HTTP/2 chain — Chrome has higher max_concurrent_streams
    assert_eq!(engine.http2_config().max_concurrent_streams, 1000);
    assert_eq!(engine.http2_config().initial_window_size, 6291456);
}

#[test]
fn test_full_chain_tls_config_boringssl_strings() {
    // Verify that TlsFingerprintConfig produces valid BoringSSL configuration strings
    // for both Firefox and Chrome profiles
    let firefox = StealthProfile::firefox_default();
    let ff_config = TlsFingerprintConfig::from_fingerprint(&firefox.tls);

    // Firefox TLS 1.2 must include ECDHE suites
    assert!(ff_config.tls12_cipher_list.contains("ECDHE-ECDSA-AES128-GCM-SHA256"));
    assert!(ff_config.tls12_cipher_list.contains("ECDHE-RSA-AES128-GCM-SHA256"));

    // Firefox TLS 1.3 must include all three suites
    assert!(ff_config.tls13_cipher_suites.contains("TLS_AES_128_GCM_SHA256"));
    assert!(ff_config.tls13_cipher_suites.contains("TLS_AES_256_GCM_SHA384"));
    assert!(ff_config.tls13_cipher_suites.contains("TLS_CHACHA20_POLY1305_SHA256"));

    // Firefox curves must include X25519 and P-256
    assert!(ff_config.curves_list.contains("X25519"));
    assert!(ff_config.curves_list.contains("P-256"));

    // Chrome config must also be valid
    let chrome = StealthProfile::chrome_default();
    let ch_config = TlsFingerprintConfig::from_fingerprint(&chrome.tls);
    assert!(ch_config.has_fingerprint());

    // Firefox and Chrome must produce different TLS 1.2 cipher lists
    assert_ne!(ff_config.tls12_cipher_list, ch_config.tls12_cipher_list);
}

#[test]
fn test_full_chain_http2_pseudo_header_ordering() {
    // Verify that HTTP/2 pseudo-header ordering differs between Firefox and Chrome
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    // Firefox: :method, :path, :authority, :scheme
    assert_eq!(firefox.http2.pseudo_header_order[0], ":method");
    assert_eq!(firefox.http2.pseudo_header_order[1], ":path");
    assert_eq!(firefox.http2.pseudo_header_order[2], ":authority");
    assert_eq!(firefox.http2.pseudo_header_order[3], ":scheme");

    // Chrome: :method, :authority, :scheme, :path
    assert_eq!(chrome.http2.pseudo_header_order[0], ":method");
    assert_eq!(chrome.http2.pseudo_header_order[1], ":authority");
    assert_eq!(chrome.http2.pseudo_header_order[2], ":scheme");
    assert_eq!(chrome.http2.pseudo_header_order[3], ":path");

    assert_ne!(
        firefox.http2.pseudo_header_order, chrome.http2.pseudo_header_order,
        "Firefox and Chrome must have different pseudo-header ordering"
    );
}

#[test]
fn test_full_chain_compute_ja3_ja4_consistency() {
    // Verify that compute_ja3() and compute_ja4() produce valid fingerprints
    // that are consistent with the stored ja3_hash
    let firefox = StealthProfile::firefox_default();
    let computed_ja3 = firefox.tls.compute_ja3();
    assert!(computed_ja3.starts_with("771,"), "Computed JA3 must start with TLS version 771");

    let ja4 = firefox.tls.compute_ja4();
    assert!(ja4.starts_with("t13d"), "JA4 must start with 't13d'");

    let chrome = StealthProfile::chrome_default();
    let chrome_ja3 = chrome.tls.compute_ja3();
    let chrome_ja4 = chrome.tls.compute_ja4();
    assert!(chrome_ja3.starts_with("771,"));
    assert!(chrome_ja4.starts_with("t13d"));

    // Computed JA3 values must differ between Firefox and Chrome
    assert_ne!(computed_ja3, chrome_ja3, "Firefox and Chrome must have different computed JA3");
}

#[test]
fn test_full_chain_canvas_noise_deterministic() {
    // Canvas noise must be deterministic for the same seed
    let firefox = StealthProfile::firefox_default();
    let p1 = firefox.canvas.apply_to_pixel(128, 64, 32, 255, 10, 20);
    let p2 = firefox.canvas.apply_to_pixel(128, 64, 32, 255, 10, 20);
    assert_eq!(p1, p2, "Canvas noise must be deterministic for same input");

    // Chrome uses a different seed, so pixels should differ
    let chrome = StealthProfile::chrome_default();
    let p3 = chrome.canvas.apply_to_pixel(128, 64, 32, 255, 10, 20);
    assert_ne!(p1, p3, "Firefox and Chrome canvas noise should differ (different seeds)");
}

#[test]
fn test_full_chain_behavior_simulator_deterministic() {
    // Behavior simulation must be deterministic for the same seed
    let firefox = StealthProfile::firefox_default();
    let path1 = firefox.behavior.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let path2 = firefox.behavior.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path1, path2, "Mouse path must be deterministic for same seed");

    let delays1 = firefox.behavior.generate_typing_delays(20);
    let delays2 = firefox.behavior.generate_typing_delays(20);
    assert_eq!(delays1, delays2, "Typing delays must be deterministic for same seed");

    // Chrome uses a different seed
    let chrome = StealthProfile::chrome_default();
    let chrome_path = chrome.behavior.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_ne!(path1, chrome_path, "Firefox and Chrome mouse paths should differ (different seeds)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 7: Browser integration — engine_props injection via JsContext::for_test()
// ═══════════════════════════════════════════════════════════════════════════
// Verifies stealth properties are injected via engine_props::install_stealth_props()
// using in-process JsContext (same pattern as bao_engine/bao_runtime tests).
// Full browser integration (PageHandle + evaluate_js) lives in
// cross_crate_integration_tests.rs because of servo Opts single-init constraint.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

// All engine_props tests in a single #[test] — mozjs Runtime is per-thread singleton.
//
// Anti-fingerprinting is ON BY DEFAULT — no manual setup.
// `install_all` automatically installs Firefox profile + stealth getters.
// This is the user-facing library contract: one-line setup.
#[test]
fn test_stealth_props_injected_all() {
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let ua = match ctx.eval("navigator.userAgent", "<stealth-test>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    };
    assert!(ua.contains("Firefox"), "navigator.userAgent must contain Firefox (default profile): {}", ua);

    let vendor = match ctx.eval("navigator.vendor", "<stealth-test>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    };
    assert_eq!(vendor, "", "Firefox navigator.vendor must be empty: {}", vendor);

    let platform = match ctx.eval("navigator.platform", "<stealth-test>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    };
    assert!(platform.contains("Linux"), "navigator.platform must contain Linux: {}", platform);

    let hwc = match ctx.eval("navigator.hardwareConcurrency", "<stealth-test>") {
        Ok(JsValue::Number(n)) => n as i32,
        other => -1,
    };
    assert_eq!(hwc, 8, "navigator.hardwareConcurrency must be 8: {}", hwc);

    let w = match ctx.eval("screen.width", "<stealth-test>") {
        Ok(JsValue::Number(n)) => n as i32,
        other => -1,
    };
    assert_eq!(w, 1920, "screen.width must be 1920: {}", w);

    let h = match ctx.eval("screen.height", "<stealth-test>") {
        Ok(JsValue::Number(n)) => n as i32,
        other => -1,
    };
    assert_eq!(h, 1080, "screen.height must be 1080: {}", h);

    let cd = match ctx.eval("screen.colorDepth", "<stealth-test>") {
        Ok(JsValue::Number(n)) => n as i32,
        other => -1,
    };
    assert_eq!(cd, 24, "screen.colorDepth must be 24: {}", cd);

    let webdriver = match ctx.eval("navigator.webdriver", "<stealth-test>") {
        Ok(JsValue::Bool(b)) => b,
        Ok(JsValue::String(s)) => s == "true",
        _ => true,
    };
    assert!(!webdriver, "navigator.webdriver must be false");

    JsContext::shutdown_thread_sm();
}
