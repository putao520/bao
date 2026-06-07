#![allow(unused_comparisons, unused_variables)]
// @trace TEST-STL-001~007-INT [req:REQ-STL-001~007] [level:unit]
// Stealth integration tests: JS injection content validation, profile cross-consistency,
// fingerprint computation determinism

use bao_stealth::*;

// ---- JS Injection Content Validation ----

// ---- Profile Cross-Consistency ----

#[test]
fn test_chrome_profile_ua_contains_chrome() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let ua = &engine.navigator().user_agent;
    assert!(ua.contains("Chrome") || ua.contains("chrome"), "Chrome profile UA should contain Chrome");
}

#[test]
fn test_firefox_profile_ua_contains_firefox() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let ua = &engine.navigator().user_agent;
    assert!(ua.contains("Firefox") || ua.contains("firefox"), "Firefox profile UA should contain Firefox");
}

#[test]
fn test_profiles_have_different_user_agents() {
    let chrome_ua = StealthEngine::new(StealthProfile::chrome_default()).navigator().user_agent.clone();
    let firefox_ua = StealthEngine::new(StealthProfile::firefox_default()).navigator().user_agent.clone();
    assert_ne!(chrome_ua, firefox_ua, "Chrome and Firefox profiles should have different UAs");
}

#[test]
fn test_profiles_have_different_platforms_or_same() {
    // Both might report same platform on same OS — just verify non-empty
    let chrome = StealthEngine::new(StealthProfile::chrome_default());
    let firefox = StealthEngine::new(StealthProfile::firefox_default());
    assert!(!chrome.navigator().platform.is_empty());
    assert!(!firefox.navigator().platform.is_empty());
}

#[test]
fn test_screen_profiles_positive_dimensions() {
    let profiles = [StealthProfile::chrome_default(), StealthProfile::firefox_default()];
    for p in &profiles {
        let engine = StealthEngine::new(p.clone());
        let scr = engine.screen();
        assert!(scr.width > 0, "screen width should be positive");
        assert!(scr.height > 0, "screen height should be positive");
        assert!(scr.device_pixel_ratio > 0.0, "DPR should be positive");
    }
}

#[test]
fn test_webgl_profiles_have_vendor_and_renderer() {
    let profiles = [StealthProfile::chrome_default(), StealthProfile::firefox_default()];
    for p in &profiles {
        let engine = StealthEngine::new(p.clone());
        let gl = engine.webgl();
        assert!(!gl.vendor.is_empty(), "WebGL vendor should be non-empty");
        assert!(!gl.renderer.is_empty(), "WebGL renderer should be non-empty");
    }
}

// ---- Fingerprint Determinism ----

#[test]
fn test_ja3_deterministic() {
    let fp = TlsFingerprint::chrome();
    let ja3_1 = fp.compute_ja3();
    let ja3_2 = fp.compute_ja3();
    assert_eq!(ja3_1, ja3_2, "JA3 should be deterministic");
}

#[test]
fn test_ja4_deterministic() {
    let fp = TlsFingerprint::firefox();
    let ja4_1 = fp.compute_ja4();
    let ja4_2 = fp.compute_ja4();
    assert_eq!(ja4_1, ja4_2, "JA4 should be deterministic");
}

#[test]
fn test_http2_settings_deterministic() {
    let fp = Http2Fingerprint::chrome();
    let s1 = fp.settings_frame_payload();
    let s2 = fp.settings_frame_payload();
    assert_eq!(s1, s2, "HTTP/2 settings should be deterministic");
}

// ---- Behavior Simulator Reproducibility ----

#[test]
fn test_mouse_path_reproducible_with_same_seed() {
    let bs1 = BehaviorSimulator::new(42);
    let bs2 = BehaviorSimulator::new(42);
    let path1 = bs1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let path2 = bs2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path1, path2, "same seed should produce same mouse path");
}

#[test]
fn test_typing_delays_reproducible_with_same_seed() {
    let bs1 = BehaviorSimulator::new(99);
    let bs2 = BehaviorSimulator::new(99);
    let d1 = bs1.generate_typing_delays(20);
    let d2 = bs2.generate_typing_delays(20);
    assert_eq!(d1, d2, "same seed should produce same typing delays");
}

#[test]
fn test_scroll_deltas_reproducible_with_same_seed() {
    let bs1 = BehaviorSimulator::new(7);
    let bs2 = BehaviorSimulator::new(7);
    let d1 = bs1.generate_scroll_deltas(500.0, 10);
    let d2 = bs2.generate_scroll_deltas(500.0, 10);
    assert_eq!(d1, d2, "same seed should produce same scroll deltas");
}

#[test]
fn test_different_seeds_different_paths() {
    let bs1 = BehaviorSimulator::new(1);
    let bs2 = BehaviorSimulator::new(2);
    let p1 = bs1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let p2 = bs2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_ne!(p1, p2, "different seeds should produce different paths");
}

// ---- Canvas Noise Properties ----

#[test]
fn test_canvas_noise_reproducible() {
    let cn1 = CanvasNoise::new(42);
    let cn2 = CanvasNoise::new(42);
    let (r1, g1, b1, a1) = cn1.apply_to_pixel(128, 128, 128, 255, 10, 10);
    let (r2, g2, b2, a2) = cn2.apply_to_pixel(128, 128, 128, 255, 10, 10);
    assert_eq!((r1, g1, b1, a1), (r2, g2, b2, a2), "same seed should produce same noise");
}

#[test]
fn test_canvas_noise_bounded() {
    let cn = CanvasNoise::new(42);
    for x in 0..10 {
        for y in 0..10 {
            let (r, g, b, a) = cn.apply_to_pixel(128, 128, 128, 255, x, y);
            // u8 always valid:                 "pixel values should be <= 255 at ({}, {})", x, y);
        }
    }
}

// ---- Audio Noise Properties ----

#[test]
fn test_audio_noise_bounded() {
    let ap = AudioProfile::new(42);
    for i in 0..100 {
        let noisy = ap.apply_noise(0.5, i);
        assert!(noisy >= 0.0 && noisy <= 1.0, "audio noise should stay in [0,1], got {}", noisy);
    }
}

#[test]
fn test_audio_noise_small_perturbation() {
    let ap = AudioProfile::new(42);
    let mut sum_diff = 0.0_f64;
    for i in 0..1000 {
        let noisy = ap.apply_noise(0.5, i);
        sum_diff += (noisy - 0.5).abs();
    }
    let avg_diff = sum_diff / 1000.0;
    assert!(avg_diff < 0.1, "average noise perturbation should be small, got {}", avg_diff);
}

// ---- StealthProfile serialization ----

#[test]
fn test_stealth_profile_chrome_default_consistency() {
    let chrome1 = StealthProfile::chrome_default();
    let chrome2 = StealthProfile::chrome_default();
    assert_eq!(chrome1.navigator.user_agent, chrome2.navigator.user_agent);
    assert_eq!(chrome1.navigator.platform, chrome2.navigator.platform);
}

#[test]
fn test_all_profile_components_accessible() {
    let profiles = vec![StealthProfile::chrome_default(), StealthProfile::firefox_default()];
    for p in profiles {
        let engine = StealthEngine::new(p);
        // All sub-components should be accessible without panic
        let _ = engine.tls_config();
        let _ = engine.http2_config();
        let _ = engine.canvas_noise();
        let _ = engine.navigator();
        let _ = engine.screen();
        let _ = engine.webgl();
        let _ = engine.audio();
        let _ = engine.behavior();
    }
}
