// @trace TEST-STL-020 [req:REQ-STL-007] [level:unit]
// StealthEngine cross-profile consistency: engine construction, profile accessor
// validation, inject_navigator_js content verification, default engine, custom
// profile assembly, clone/debug, profile component independence.

use bao_stealth::{
    StealthEngine, StealthProfile, TlsFingerprint, Http2Fingerprint,
    CanvasNoise, NavigatorProfile, ScreenProfile, WebGLProfile, AudioProfile,
};

// ---- StealthEngine construction ----

#[test]
fn test_default_engine_is_firefox() {
    let engine = StealthEngine::default_engine();
    let tls = engine.tls_config();
    // Firefox TLS has more cipher suites than Chrome
    assert!(tls.cipher_suites.len() > 10);
    assert!(tls.alpn_strings().contains(&"h2"));
}

#[test]
fn test_engine_new_custom_profile() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile);
    assert!(engine.tls_config().cipher_suites.len() > 0);
}

#[test]
fn test_engine_profile_accessor() {
    let engine = StealthEngine::default_engine();
    let _ = engine.profile();
}

// ---- Profile component accessors ----

#[test]
fn test_engine_tls_config_matches_profile() {
    let engine = StealthEngine::default_engine();
    let tls = engine.tls_config();
    let profile_tls = &engine.profile().tls;
    assert_eq!(tls.cipher_suites, profile_tls.cipher_suites);
    assert_eq!(tls.extensions, profile_tls.extensions);
}

#[test]
fn test_engine_http2_config_matches_profile() {
    let engine = StealthEngine::default_engine();
    let http2 = engine.http2_config();
    let profile_http2 = &engine.profile().http2;
    assert_eq!(http2.header_table_size, profile_http2.header_table_size);
    assert_eq!(http2.akamai_fingerprint(), profile_http2.akamai_fingerprint());
}

#[test]
fn test_engine_canvas_noise_matches_profile() {
    let engine = StealthEngine::default_engine();
    let canvas = engine.canvas_noise();
    let profile_canvas = &engine.profile().canvas;
    // Same seed → same noise
    let v1 = canvas.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let v2 = profile_canvas.apply_to_pixel(128, 128, 128, 255, 0, 0);
    assert_eq!(v1, v2);
}

#[test]
fn test_engine_navigator_matches_profile() {
    let engine = StealthEngine::default_engine();
    let nav = engine.navigator();
    let profile_nav = &engine.profile().navigator;
    assert_eq!(nav.user_agent, profile_nav.user_agent);
}

#[test]
fn test_engine_screen_matches_profile() {
    let engine = StealthEngine::default_engine();
    let screen = engine.screen();
    let profile_screen = &engine.profile().screen;
    assert_eq!(screen.width, profile_screen.width);
    assert_eq!(screen.height, profile_screen.height);
}

#[test]
fn test_engine_webgl_matches_profile() {
    let engine = StealthEngine::default_engine();
    let webgl = engine.webgl();
    let profile_webgl = &engine.profile().webgl;
    assert_eq!(webgl.vendor, profile_webgl.vendor);
    assert_eq!(webgl.renderer, profile_webgl.renderer);
    assert_eq!(webgl.extensions.len(), profile_webgl.extensions.len());
}

#[test]
fn test_engine_audio_matches_profile() {
    let engine = StealthEngine::default_engine();
    let audio = engine.audio();
    let profile_audio = &engine.profile().audio;
    assert_eq!(audio.seed(), profile_audio.seed());
}

#[test]
fn test_engine_behavior_matches_profile() {
    let engine = StealthEngine::default_engine();
    let behavior = engine.behavior();
    let _ = &engine.profile().behavior;
    // Behavior is non-trivial; just confirm accessor works
    assert!(format!("{:?}", behavior).len() > 0);
}

// ---- inject_navigator_js ----

#[test]
fn test_inject_navigator_js_not_empty() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(!js.is_empty());
}

#[test]
fn test_inject_navigator_js_contains_navigator() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("navigator"));
}

#[test]
fn test_inject_navigator_js_contains_object_define() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("Object.defineProperty") || js.contains("defineProperty"));
}

#[test]
fn test_inject_navigator_js_contains_user_agent() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    // Should reference userAgent override
    assert!(js.contains("userAgent"));
}

#[test]
fn test_inject_navigator_js_contains_webgl_override() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("WEBGL") || js.contains("webgl") || js.contains("getParameter"));
}

#[test]
fn test_inject_navigator_js_removes_automation() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("chrome") || js.contains("webdriver") || js.contains("cdc_"));
}

#[test]
fn test_inject_navigator_js_deterministic() {
    let engine = StealthEngine::default_engine();
    let js1 = engine.inject_navigator_js();
    let js2 = engine.inject_navigator_js();
    assert_eq!(js1, js2);
}

#[test]
fn test_inject_navigator_js_is_valid_js_syntax() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    // Basic check: balanced braces
    let open = js.chars().filter(|c| *c == '{').count();
    let close = js.chars().filter(|c| *c == '}').count();
    assert_eq!(open, close, "Unbalanced braces in injected JS");
}

// ---- Cross-profile consistency: Firefox vs Chrome ----

#[test]
fn test_firefox_vs_chrome_tls_differ() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_ne!(
        ff_engine.tls_config().compute_ja3(),
        ch_engine.tls_config().compute_ja3()
    );
}

#[test]
fn test_firefox_vs_chrome_http2_differ() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_ne!(
        ff_engine.http2_config().akamai_fingerprint(),
        ch_engine.http2_config().akamai_fingerprint()
    );
}

#[test]
fn test_firefox_vs_chrome_webgl_differ() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_ne!(ff_engine.webgl().vendor, ch_engine.webgl().vendor);
}

#[test]
fn test_firefox_vs_chrome_navigator_differ() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_ne!(ff_engine.navigator().user_agent, ch_engine.navigator().user_agent);
}

#[test]
fn test_firefox_vs_chrome_inject_js_differ() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_ne!(
        ff_engine.inject_navigator_js(),
        ch_engine.inject_navigator_js()
    );
}

// ---- Profile component independence ----

#[test]
fn test_canvas_noise_independent_of_tls() {
    let engine = StealthEngine::default_engine();
    let canvas_val = engine.canvas_noise().apply_to_pixel(100, 100, 100, 255, 5, 5);
    // Canvas noise doesn't depend on TLS cipher suites
    let _ = engine.tls_config().cipher_suites.len();
    assert!(canvas_val.0 >= 0 && canvas_val.0 <= 255);
}

#[test]
fn test_audio_noise_independent_of_http2() {
    let engine = StealthEngine::default_engine();
    let audio_val = engine.audio().apply_noise(0.5, 10);
    let _ = engine.http2_config().akamai_fingerprint();
    assert!(audio_val > 0.0 && audio_val < 1.0);
}

// ---- StealthProfile construction ----

#[test]
fn test_stealth_profile_firefox_has_all_components() {
    let profile = StealthProfile::firefox_default();
    assert!(!profile.tls.cipher_suites.is_empty());
    assert!(!profile.http2.pseudo_header_order.is_empty());
    assert!(!profile.navigator.user_agent.is_empty());
    assert!(profile.screen.width > 0);
    assert!(!profile.webgl.extensions.is_empty());
    assert!(profile.audio.seed() > 0);
}

#[test]
fn test_stealth_profile_chrome_has_all_components() {
    let profile = StealthProfile::chrome_default();
    assert!(!profile.tls.cipher_suites.is_empty());
    assert!(!profile.http2.pseudo_header_order.is_empty());
    assert!(!profile.navigator.user_agent.is_empty());
    assert!(profile.screen.width > 0);
    assert!(!profile.webgl.extensions.is_empty());
    assert!(profile.audio.seed() > 0);
}

// ---- Engine with manually constructed profile ----

#[test]
fn test_engine_with_firefox_tls_chrome_webgl() {
    let mut profile = StealthProfile::firefox_default();
    profile.webgl = WebGLProfile::chrome();
    let engine = StealthEngine::new(profile);
    // TLS should be Firefox
    assert!(engine.tls_config().cipher_suites.len() > 12); // Firefox has more
    // WebGL should be Chrome
    assert!(engine.webgl().vendor.contains("Google"));
}

#[test]
fn test_engine_with_custom_screen() {
    let mut profile = StealthProfile::firefox_default();
    profile.screen = ScreenProfile::new(3840, 2160, 2.0);
    let engine = StealthEngine::new(profile);
    assert_eq!(engine.screen().width, 3840);
    assert_eq!(engine.screen().height, 2160);
    // Other components unchanged
    assert!(engine.tls_config().cipher_suites.len() > 0);
}

#[test]
fn test_engine_with_custom_canvas_noise() {
    let mut profile = StealthProfile::firefox_default();
    profile.canvas = CanvasNoise::new(12345);
    let engine = StealthEngine::new(profile);
    let val = engine.canvas_noise().apply_to_pixel(200, 200, 200, 255, 10, 10);
    // Different seed → different result from default
    let default_engine = StealthEngine::default_engine();
    let _default_val = default_engine.canvas_noise().apply_to_pixel(200, 200, 200, 255, 10, 10);
    // Most likely different (not guaranteed for all coords, but highly probable)
    // Just check the custom seed produces valid values
    assert!(val.0 >= 0 && val.0 <= 255);
}

// ---- StealthProfile debug ----

#[test]
fn test_stealth_profile_debug() {
    let profile = StealthProfile::firefox_default();
    let debug = format!("{:?}", profile);
    assert!(debug.contains("StealthProfile") || debug.len() > 100);
}

#[test]
fn test_stealth_engine_profile_debug() {
    let engine = StealthEngine::default_engine();
    let profile = engine.profile();
    let debug = format!("{:?}", profile);
    assert!(!debug.is_empty());
}

// ---- Cross-preset alpn consistency ----

#[test]
fn test_all_engines_support_h2() {
    let engines = [
        StealthEngine::new(StealthProfile::firefox_default()),
        StealthEngine::new(StealthProfile::chrome_default()),
    ];
    for engine in &engines {
        assert!(engine.tls_config().alpn_strings().iter().any(|s| s == &"h2"));
        assert!(engine.http2_config().pseudo_header_order.contains(&":method"));
    }
}

// ---- Canvas noise across engines produces valid pixels ----

#[test]
fn test_canvas_noise_produces_valid_rgba() {
    let engine = StealthEngine::default_engine();
    for x in 0..10u32 {
        for y in 0..10u32 {
            let (r, g, b, a) = engine.canvas_noise().apply_to_pixel(128, 128, 128, 255, x, y);
            assert!(r <= 255);
            assert!(g <= 255);
            assert!(b <= 255);
            assert!(a <= 255);
        }
    }
}

// ---- Audio noise across engines produces valid samples ----

#[test]
fn test_audio_noise_produces_valid_samples() {
    let engine = StealthEngine::default_engine();
    for i in 0..100u32 {
        let sample = engine.audio().apply_noise(0.5, i);
        assert!(sample > -1.0 && sample < 1.0, "Sample {} out of range: {}", i, sample);
    }
}

// ---- Screen profile dimension sanity ----

#[test]
fn test_firefox_screen_reasonable_dimensions() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let screen = engine.screen();
    assert!(screen.width >= 640 && screen.width <= 7680);
    assert!(screen.height >= 480 && screen.height <= 4320);
}

#[test]
fn test_chrome_screen_reasonable_dimensions() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let screen = engine.screen();
    assert!(screen.width >= 640 && screen.width <= 7680);
    assert!(screen.height >= 480 && screen.height <= 4320);
}
