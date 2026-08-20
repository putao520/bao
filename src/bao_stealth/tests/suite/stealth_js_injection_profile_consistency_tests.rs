// @trace TEST-STL-045 [req:REQ-STL-001,REQ-STL-007] [level:unit]
// StealthEngine JS injection completeness, StealthProfile field consistency,
// cross-profile isolation, default engine accessor coverage.

use bao_stealth::*;

// ---- StealthEngine default ----

#[test]
fn test_default_engine_is_firefox() {
    let engine = StealthEngine::default_engine();
    assert!(engine.navigator().user_agent.contains("Firefox"));
}

#[test]
fn test_default_engine_tls_config() {
    let engine = StealthEngine::default_engine();
    let tls = engine.tls_config();
    assert!(!tls.cipher_suites.is_empty());
}

#[test]
fn test_default_engine_http2_config() {
    let engine = StealthEngine::default_engine();
    let h2 = engine.http2_config();
    assert!(h2.header_table_size > 0);
}

#[test]
fn test_default_engine_canvas_noise() {
    let engine = StealthEngine::default_engine();
    let cn = engine.canvas_noise();
    assert!(cn.seed() > 0);
}

#[test]
fn test_default_engine_navigator() {
    let engine = StealthEngine::default_engine();
    let nav = engine.navigator();
    assert!(!nav.user_agent.is_empty());
    assert!(!nav.platform.is_empty());
    assert!(!nav.language.is_empty());
    assert!(nav.hardware_concurrency > 0);
}

#[test]
fn test_default_engine_screen() {
    let engine = StealthEngine::default_engine();
    let scr = engine.screen();
    assert!(scr.width > 0);
    assert!(scr.height > 0);
    assert!(scr.device_pixel_ratio > 0.0);
}

#[test]
fn test_default_engine_webgl() {
    let engine = StealthEngine::default_engine();
    let gl = engine.webgl();
    assert!(!gl.vendor.is_empty());
    assert!(!gl.renderer.is_empty());
}

#[test]
fn test_default_engine_audio() {
    let engine = StealthEngine::default_engine();
    let audio = engine.audio();
    // AudioProfile should have valid seed
    assert!(audio.seed() > 0);
}

#[test]
fn test_default_engine_behavior() {
    let engine = StealthEngine::default_engine();
    let bh = engine.behavior();
    assert!(bh.seed() > 0);
}

// ---- StealthEngine profile accessor ----

#[test]
fn test_engine_profile_accessor() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let profile = engine.profile();
    assert!(profile.navigator.user_agent.contains("Chrome"));
}

// ---- engine-layer injection checks ----

// ---- StealthProfile consistency ----

#[test]
fn test_firefox_profile_has_firefox_navigator() {
    let p = StealthProfile::firefox_default();
    assert!(p.navigator.user_agent.contains("Firefox"));
    assert!(p.navigator.platform.contains("Linux") || p.navigator.platform.contains("Win"));
}

#[test]
fn test_chrome_profile_has_chrome_navigator() {
    let p = StealthProfile::chrome_default();
    assert!(p.navigator.user_agent.contains("Chrome"));
}

#[test]
fn test_firefox_profile_has_tls() {
    let p = StealthProfile::firefox_default();
    assert!(!p.tls.cipher_suites.is_empty());
}

#[test]
fn test_chrome_profile_has_tls() {
    let p = StealthProfile::chrome_default();
    assert!(!p.tls.cipher_suites.is_empty());
}

#[test]
fn test_profiles_canvas_seeds_differ() {
    let fp = StealthProfile::firefox_default();
    let cp = StealthProfile::chrome_default();
    assert_ne!(fp.canvas.seed(), cp.canvas.seed());
}

#[test]
fn test_profiles_behavior_seeds_differ() {
    let fp = StealthProfile::firefox_default();
    let cp = StealthProfile::chrome_default();
    assert_ne!(fp.behavior.seed(), cp.behavior.seed());
}

#[test]
fn test_firefox_profile_has_http2() {
    let p = StealthProfile::firefox_default();
    assert!(p.http2.header_table_size > 0);
}

#[test]
fn test_chrome_profile_has_http2() {
    let p = StealthProfile::chrome_default();
    assert!(p.http2.header_table_size > 0);
}

#[test]
fn test_profile_clone_independence() {
    let p1 = StealthProfile::firefox_default();
    let mut p2 = p1.clone();
    p2.navigator.user_agent = "Modified".into();
    assert!(p1.navigator.user_agent.contains("Firefox"));
    assert_eq!(p2.navigator.user_agent, "Modified");
}

#[test]
fn test_profile_debug_format() {
    let p = StealthProfile::firefox_default();
    let debug = format!("{:?}", p);
    assert!(debug.contains("StealthProfile") || debug.contains("tls"));
}

// ---- StealthProfile::firefox_default specific ----

#[test]
fn test_firefox_default_canvas_seed_42() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.canvas.seed(), 42);
}

#[test]
fn test_firefox_default_audio_seed_42() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.audio.seed(), 42);
}

#[test]
fn test_firefox_default_behavior_seed_42() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.behavior.seed(), 42);
}

// ---- StealthProfile::chrome_default specific ----

#[test]
fn test_chrome_default_canvas_seed_137() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.canvas.seed(), 137);
}

#[test]
fn test_chrome_default_audio_seed_137() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.audio.seed(), 137);
}

#[test]
fn test_chrome_default_behavior_seed_137() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.behavior.seed(), 137);
}

// ---- StealthEngine cross-profile isolation ----
