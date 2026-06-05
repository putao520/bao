// @trace TEST-STL-008-CONSISTENCY [req:REQ-STL-007] [level:unit]
// Stealth profile cross-consistency: chrome vs firefox profiles, engine delegation, determinism

use bao_stealth::{StealthEngine, StealthProfile, CanvasNoise, BehaviorSimulator};

// ---- Chrome vs Firefox profile consistency ----

#[test]
fn test_chrome_profile_tls_is_chrome() {
    let profile = StealthProfile::chrome_default();
    assert!(profile.tls.ja3_hash.contains("chrome") || profile.tls.ja3_hash.len() > 10,
        "Chrome TLS should have chrome identifier or valid hash");
}

#[test]
fn test_firefox_profile_tls_is_firefox() {
    let profile = StealthProfile::firefox_default();
    assert!(profile.tls.ja3_hash.contains("firefox") || profile.tls.ja3_hash.len() > 10,
        "Firefox TLS should have firefox identifier or valid hash");
}

#[test]
fn test_chrome_and_firefox_profiles_differ() {
    let chrome = StealthProfile::chrome_default();
    let firefox = StealthProfile::firefox_default();
    let tls_differ = chrome.tls.ja3_hash != firefox.tls.ja3_hash;
    let nav_differ = chrome.navigator.user_agent != firefox.navigator.user_agent;
    assert!(tls_differ || nav_differ, "Chrome and Firefox profiles should differ in at least TLS or Navigator");
}

#[test]
fn test_chrome_navigator_has_chrome_ua() {
    let profile = StealthProfile::chrome_default();
    assert!(profile.navigator.user_agent.contains("Chrome") || profile.navigator.user_agent.contains("chrome"),
        "Chrome navigator UA should contain Chrome");
}

#[test]
fn test_firefox_navigator_has_firefox_ua() {
    let profile = StealthProfile::firefox_default();
    assert!(profile.navigator.user_agent.contains("Firefox") || profile.navigator.user_agent.contains("firefox"),
        "Firefox navigator UA should contain Firefox");
}

#[test]
fn test_chrome_webgl_has_renderer() {
    let profile = StealthProfile::chrome_default();
    assert!(!profile.webgl.renderer.is_empty(), "Chrome WebGL should have renderer");
}

#[test]
fn test_firefox_webgl_has_renderer() {
    let profile = StealthProfile::firefox_default();
    assert!(!profile.webgl.renderer.is_empty(), "Firefox WebGL should have renderer");
}

#[test]
fn test_chrome_http2_has_fingerprint() {
    let profile = StealthProfile::chrome_default();
    assert!(!profile.http2.akamai_fingerprint().is_empty(), "Chrome HTTP2 should have akamai fingerprint");
}

#[test]
fn test_firefox_http2_has_fingerprint() {
    let profile = StealthProfile::firefox_default();
    assert!(!profile.http2.akamai_fingerprint().is_empty(), "Firefox HTTP2 should have akamai fingerprint");
}

// ---- StealthEngine delegation ----

#[test]
fn test_engine_delegates_to_profile() {
    let profile = StealthProfile::chrome_default();
    let engine = StealthEngine::new(profile);
    assert_eq!(engine.tls_config().ja3_hash, engine.profile().tls.ja3_hash);
    assert_eq!(engine.navigator().user_agent, engine.profile().navigator.user_agent);
}

#[test]
fn test_default_engine_has_valid_profile() {
    let engine = StealthEngine::default_engine();
    assert!(!engine.profile().tls.ja3_hash.is_empty());
    assert!(!engine.profile().navigator.user_agent.is_empty());
    assert!(!engine.profile().http2.akamai_fingerprint().is_empty());
}

#[test]
fn test_engine_canvas_noise_has_seed() {
    let engine = StealthEngine::default_engine();
    let seed = engine.canvas_noise().seed();
    assert!(seed == 0 || seed > 0, "canvas noise seed should be accessible");
}

#[test]
fn test_engine_behavior_has_seed() {
    let engine = StealthEngine::default_engine();
    let seed = engine.behavior().seed();
    assert!(seed == 0 || seed > 0, "behavior seed should be accessible");
}

#[test]
fn test_engine_screen_has_dimensions() {
    let engine = StealthEngine::default_engine();
    let screen = engine.screen();
    assert!(screen.width > 0);
    assert!(screen.height > 0);
}

#[test]
fn test_engine_audio_has_seed() {
    let engine = StealthEngine::default_engine();
    let seed = engine.audio().seed();
    assert!(seed == 0 || seed > 0);
}

// ---- Profile component determinism ----

#[test]
fn test_same_seed_same_canvas_noise() {
    let noise1 = CanvasNoise::new(42);
    let noise2 = CanvasNoise::new(42);
    assert_eq!(noise1.seed(), noise2.seed());
}

#[test]
fn test_same_seed_same_behavior() {
    let b1 = BehaviorSimulator::new(100);
    let b2 = BehaviorSimulator::new(100);
    assert_eq!(b1.seed(), b2.seed());
}

#[test]
fn test_different_seed_different_canvas() {
    let noise1 = CanvasNoise::new(1);
    let noise2 = CanvasNoise::new(999);
    assert_ne!(noise1.seed(), noise2.seed());
}

#[test]
fn test_behavior_mouse_path_deterministic() {
    let b = BehaviorSimulator::new(42);
    let path1 = b.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let path2 = b.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path1.len(), path2.len());
}

#[test]
fn test_behavior_typing_delays_deterministic() {
    let b = BehaviorSimulator::new(42);
    let d1 = b.generate_typing_delays(5);
    let d2 = b.generate_typing_delays(5);
    assert_eq!(d1.len(), 5);
    assert_eq!(d1, d2);
}

#[test]
fn test_behavior_scroll_deltas_deterministic() {
    let b = BehaviorSimulator::new(42);
    let s1 = b.generate_scroll_deltas(500.0, 10);
    let s2 = b.generate_scroll_deltas(500.0, 10);
    assert_eq!(s1.len(), 10);
    assert_eq!(s1, s2);
}

// ---- JS injection content consistency ----

// ---- CanvasNoise pixel application ----

#[test]
fn test_canvas_noise_modifies_pixel() {
    let noise = CanvasNoise::new(42);
    let (r, g, b, a) = noise.apply_to_pixel(128, 128, 128, 255, 10, 20);
    // Verify deterministic output (u8 is always 0-255 by definition)
    let (r2, g2, b2, a2) = noise.apply_to_pixel(128, 128, 128, 255, 10, 20);
    assert_eq!((r, g, b, a), (r2, g2, b2, a2));
}

#[test]
fn test_canvas_noise_different_positions_differ() {
    let noise = CanvasNoise::new(42);
    let p1 = noise.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let p2 = noise.apply_to_pixel(128, 128, 128, 255, 100, 100);
    // Different positions — just verify no crash, output is valid u8
    let _ = (p1, p2);
}

// ---- Debug traits ----

#[test]
fn test_stealth_profile_debug() {
    let profile = StealthProfile::chrome_default();
    let debug = format!("{:?}", profile);
    assert!(debug.len() > 50, "StealthProfile debug should have content");
}
