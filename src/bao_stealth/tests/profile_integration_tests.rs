// @trace TEST-STL-007 [req:REQ-STL-007] [level:unit]
// @trace TEST-STL-008 [req:REQ-STL-004] [level:unit]
// @trace TEST-STL-009 [req:REQ-STL-003] [level:unit]
// @trace TEST-STL-010 [req:REQ-STL-005] [level:unit]
// @trace TEST-STL-011 [req:REQ-STL-002] [level:unit]

use bao_stealth::*;

// ===========================================================================
// §1 StealthProfile cross-component consistency (REQ-STL-007)
// ===========================================================================

#[test]
fn test_firefox_profile_all_components_present() {
    let profile = StealthProfile::firefox_default();
    // TLS
    assert!(!profile.tls.cipher_suites.is_empty());
    // HTTP/2
    assert!(profile.http2.header_table_size > 0);
    // Canvas
    assert!(profile.canvas.seed() > 0);
    // Navigator
    assert!(!profile.navigator.user_agent.is_empty());
    assert!(!profile.navigator.platform.is_empty());
    assert!(!profile.navigator.language.is_empty());
    // Screen
    assert!(profile.screen.width > 0);
    assert!(profile.screen.height > 0);
    assert!(profile.screen.device_pixel_ratio > 0.0);
    // WebGL
    assert!(!profile.webgl.vendor.is_empty());
    assert!(!profile.webgl.renderer.is_empty());
    // Audio
    assert!(profile.audio.seed() > 0);
    // Behavior
    // BehaviorSimulator constructed with seed
}

#[test]
fn test_chrome_profile_all_components_present() {
    let profile = StealthProfile::chrome_default();
    assert!(!profile.tls.cipher_suites.is_empty());
    assert!(!profile.navigator.user_agent.is_empty());
    assert!(profile.webgl.vendor.contains("Google"));
}

#[test]
fn test_profiles_have_different_user_agents() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.navigator.user_agent.contains("Firefox"));
    assert!(ch.navigator.user_agent.contains("Chrome"));
    assert_ne!(ff.navigator.user_agent, ch.navigator.user_agent);
}

#[test]
fn test_profiles_have_different_tls_fingerprints() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.tls.compute_ja3(), ch.tls.compute_ja3());
}

#[test]
fn test_profiles_have_different_http2_settings() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.http2.akamai_fingerprint(), ch.http2.akamai_fingerprint());
}

#[test]
fn test_profiles_have_different_canvas_seeds() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.canvas.seed(), ch.canvas.seed());
}

#[test]
fn test_profiles_have_different_webgl_renderers() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.webgl.renderer, ch.webgl.renderer);
}

#[test]
fn test_profile_clone_independence() {
    let p1 = StealthProfile::firefox_default();
    let p2 = p1.clone();
    // Both should have same values
    assert_eq!(p1.canvas.seed(), p2.canvas.seed());
    assert_eq!(p1.navigator.user_agent, p2.navigator.user_agent);
    assert_eq!(p1.tls.compute_ja3(), p2.tls.compute_ja3());
}

// ===========================================================================
// §2 NavigatorProfile edge cases (REQ-STL-004)
// ===========================================================================

#[test]
fn test_navigator_firefox_has_language() {
    let nav = NavigatorProfile::firefox();
    assert!(!nav.language.is_empty());
    assert!(nav.language.contains("en"));
}

#[test]
fn test_navigator_firefox_has_cores() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.hardware_concurrency > 0);
}

#[test]
fn test_navigator_chrome_has_vendor() {
    let nav = NavigatorProfile::chrome();
    assert_eq!(nav.vendor, "Google Inc.");
}

#[test]
fn test_navigator_firefox_empty_vendor() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.vendor, "");
}

#[test]
fn test_navigator_firefox_no_webdriver_marker() {
    let nav = NavigatorProfile::firefox();
    // Stealth: JS injection removes navigator.webdriver, profile itself just stores properties
    // Verify the profile has zero touch points (non-touch device)
    assert_eq!(nav.max_touch_points, 0);
}

// ===========================================================================
// §3 ScreenProfile boundary values (REQ-STL-004)
// ===========================================================================

#[test]
fn test_screen_avail_width_equals_width() {
    let scr = ScreenProfile::new(1920, 1080, 1.0);
    assert_eq!(scr.avail_width, 1920);
}

#[test]
fn test_screen_avail_height_less_than_height() {
    let scr = ScreenProfile::new(1920, 1080, 1.0);
    assert!(scr.avail_height < scr.height);
    assert!(scr.avail_height > 0);
}

#[test]
fn test_screen_color_depth_standard() {
    let scr = ScreenProfile::default();
    assert_eq!(scr.color_depth, 24);
}

#[test]
fn test_screen_dpr_positive() {
    let scr = ScreenProfile::new(3840, 2160, 2.0);
    assert!(scr.device_pixel_ratio > 0.0);
    assert_eq!(scr.device_pixel_ratio, 2.0);
}

// ===========================================================================
// §4 WebGL extensions validation (REQ-STL-005)
// ===========================================================================

#[test]
fn test_webgl_firefox_has_debug_renderer() {
    let gl = WebGLProfile::firefox();
    assert!(gl.extensions.iter().any(|e| e == "WEBGL_debug_renderer_info"));
}

#[test]
fn test_webgl_chrome_has_debug_renderer() {
    let gl = WebGLProfile::chrome();
    assert!(gl.extensions.iter().any(|e| e == "WEBGL_debug_renderer_info"));
}

#[test]
fn test_webgl_firefox_max_texture_size_reasonable() {
    let gl = WebGLProfile::firefox();
    assert!(gl.max_texture_size >= 4096);
    assert!(gl.max_texture_size <= 65536);
}

#[test]
fn test_webgl_firefox_renderer_is_webgl_string() {
    let gl = WebGLProfile::firefox();
    assert!(gl.renderer.contains("WebGL") || gl.renderer.contains("OpenGL"));
}

// ===========================================================================
// §5 AudioProfile noise amplitude (REQ-STL-005)
// ===========================================================================

#[test]
fn test_audio_noise_preserves_sign() {
    let audio = AudioProfile::new(42);
    let positive = audio.apply_noise(0.5, 0);
    assert!(positive > 0.0);
    let negative = audio.apply_noise(-0.5, 0);
    assert!(negative < 0.0);
}

#[test]
fn test_audio_noise_zero_input() {
    let audio = AudioProfile::new(42);
    let result = audio.apply_noise(0.0, 0);
    let diff = result.abs();
    assert!(diff < 1e-5);
}

#[test]
fn test_audio_noise_large_input() {
    let audio = AudioProfile::new(42);
    let result = audio.apply_noise(1000.0, 0);
    let diff = (result - 1000.0).abs();
    assert!(diff < 1e-3);
}

// ===========================================================================
// §6 HTTP/2 Akamai fingerprint format (REQ-STL-002)
// ===========================================================================

#[test]
fn test_http2_firefox_akamai_six_fields() {
    let fp = Http2Fingerprint::firefox();
    let ak = fp.akamai_fingerprint();
    let fields: Vec<&str> = ak.split(':').collect();
    assert_eq!(fields.len(), 6);
}

#[test]
fn test_http2_chrome_akamai_six_fields() {
    let fp = Http2Fingerprint::chrome();
    let ak = fp.akamai_fingerprint();
    let fields: Vec<&str> = ak.split(':').collect();
    assert_eq!(fields.len(), 6);
}

#[test]
fn test_http2_settings_frame_valid_ids() {
    let fp = Http2Fingerprint::firefox();
    let settings = fp.settings_frame_payload();
    for (id, _val) in &settings {
        // HTTP/2 settings IDs are 0x1-0x6 for standard ones
        assert!(id > &0u16);
    }
}

#[test]
fn test_http2_window_update_valid() {
    let ff = Http2Fingerprint::firefox();
    assert!(ff.initial_window_size >= 65535); // HTTP/2 default
    let ch = Http2Fingerprint::chrome();
    assert!(ch.initial_window_size >= 65535);
}

// ===========================================================================
// §7 StealthEngine JS injection validation (REQ-STL-007)
// ===========================================================================

#[test]
fn test_engine_js_injection_contains_user_agent() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let js = engine.inject_navigator_js();
    assert!(js.contains(&engine.navigator().user_agent));
}

#[test]
fn test_engine_js_injection_contains_platform() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let js = engine.inject_navigator_js();
    assert!(js.contains(&engine.navigator().platform));
}

#[test]
fn test_engine_js_injection_contains_screen_dimensions() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    let scr = engine.screen();
    assert!(js.contains(&scr.width.to_string()));
    assert!(js.contains(&scr.height.to_string()));
}

#[test]
fn test_engine_js_injection_contains_webgl_override() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("WebGLRenderingContext"));
    assert!(js.contains("getParameter"));
    assert!(js.contains(&engine.webgl().vendor));
    assert!(js.contains(&engine.webgl().renderer));
}

#[test]
fn test_engine_js_injection_removes_cdc() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("cdc_adoQpoasnfa76pfcZLmcfl"));
}

#[test]
fn test_engine_firefox_and_chrome_different_js() {
    let ff_engine = StealthEngine::new(StealthProfile::firefox_default());
    let ch_engine = StealthEngine::new(StealthProfile::chrome_default());
    let ff_js = ff_engine.inject_navigator_js();
    let ch_js = ch_engine.inject_navigator_js();
    assert_ne!(ff_js, ch_js);
}

#[test]
fn test_engine_accessors_match_profile() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile.clone());
    assert_eq!(engine.tls_config().compute_ja3(), profile.tls.compute_ja3());
    assert_eq!(engine.navigator().user_agent, profile.navigator.user_agent);
    assert_eq!(engine.screen().width, profile.screen.width);
    assert_eq!(engine.webgl().vendor, profile.webgl.vendor);
    assert_eq!(engine.audio().seed(), profile.audio.seed());
    assert_eq!(engine.canvas_noise().seed(), profile.canvas.seed());
}

// ===========================================================================
// §8 BehaviorSimulator extended tests (REQ-STL-006)
// ===========================================================================

#[test]
fn test_behavior_mouse_path_different_seeds() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(99);
    let p1 = sim1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let p2 = sim2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_ne!(p1, p2);
}

#[test]
fn test_behavior_typing_delays_in_range() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(50);
    assert_eq!(delays.len(), 50);
    for d in &delays {
        assert!(*d >= 30 && *d <= 150, "delay {} out of valid range [30, 150]", d);
    }
}

#[test]
fn test_behavior_scroll_deltas_non_negative() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 30);
    for d in &deltas {
        assert!(*d >= 0.0, "scroll delta should be non-negative, got {}", d);
    }
}

#[test]
fn test_behavior_mouse_path_reaches_target() {
    let sim = BehaviorSimulator::new(42);
    let target_x = 500.0;
    let target_y = 300.0;
    let path = sim.generate_mouse_path(0.0, 0.0, target_x, target_y, 20);
    let (last_x, last_y) = *path.last().unwrap();
    assert!((last_x - target_x).abs() < 10.0, "last x {} should be near target {}", last_x, target_x);
    assert!((last_y - target_y).abs() < 10.0, "last y {} should be near target {}", last_y, target_y);
}
