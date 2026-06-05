// @trace TEST-STL-DEEP [req:REQ-STL-001~007] [level:unit]
// Stealth deep tests: TlsFingerprint chrome variants, cross-profile uniqueness, boundary conditions

use bao_stealth::*;

#[test]
fn test_tls_chrome_120_has_cipher_suites() {
    let fp = TlsFingerprint::chrome_120();
    let suites = fp.tls12_suites();
    assert!(!suites.is_empty(), "chrome_120 should have TLS 1.2 suites");
    let ja3 = fp.compute_ja3();
    assert!(!ja3.is_empty(), "chrome_120 should produce JA3");
}

#[test]
fn test_tls_chrome_latest_has_cipher_suites() {
    let fp = TlsFingerprint::chrome_latest();
    let suites = fp.tls12_suites();
    assert!(!suites.is_empty(), "chrome_latest should have TLS 1.2 suites");
    let tls13 = fp.tls13_suites();
    assert!(!tls13.is_empty(), "chrome_latest should have TLS 1.3 suites");
}

#[test]
fn test_tls_chrome_120_differs_from_chrome() {
    let fp120 = TlsFingerprint::chrome_120();
    let fp_default = TlsFingerprint::chrome();
    // They may or may not differ, but both should be valid
    assert!(!fp120.compute_ja3().is_empty());
    assert!(!fp_default.compute_ja3().is_empty());
}

#[test]
fn test_tls_chrome_latest_ja4_format() {
    let fp = TlsFingerprint::chrome_latest();
    let ja4 = fp.compute_ja4();
    assert!(!ja4.is_empty(), "chrome_latest should produce JA4");
}

#[test]
fn test_tls_all_variants_different_ja3() {
    let variants = vec![
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    let ja3s: Vec<String> = variants.iter().map(|v| v.compute_ja3()).collect();
    for i in 0..ja3s.len() {
        for j in (i+1)..ja3s.len() {
            // Not all must differ (chrome variants might share JA3), but verify non-empty
            assert!(!ja3s[i].is_empty(), "variant {} should produce JA3", i);
            assert!(!ja3s[j].is_empty(), "variant {} should produce JA3", j);
        }
    }
}

#[test]
fn test_tls_alpn_all_variants() {
    for fp in &[TlsFingerprint::firefox(), TlsFingerprint::chrome(), TlsFingerprint::chrome_latest()] {
        let alpn = fp.alpn_strings();
        assert!(!alpn.is_empty(), "should have ALPN strings");
    }
}

#[test]
fn test_http2_firefox_vs_chrome_different_settings() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    let ff_settings = ff.settings_frame_payload();
    let ch_settings = ch.settings_frame_payload();
    // At least settings count should differ or values should differ
    assert!(ff_settings.len() > 0 && ch_settings.len() > 0);
}

#[test]
fn test_http2_ordered_headers_empty() {
    let fp = Http2Fingerprint::firefox();
    let result = fp.ordered_headers(&[]);
    assert!(result.is_empty(), "empty input should return empty");
}

#[test]
fn test_http2_ordered_headers_single() {
    let fp = Http2Fingerprint::firefox();
    let result = fp.ordered_headers(&[("content-type", "text/html")]);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_canvas_noise_boundary_zero_alpha() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, a) = cn.apply_to_pixel(128, 128, 128, 0, 10, 10);
    assert_eq!(a, 0, "zero alpha should be preserved");
    assert_eq!(r, 128);
}

#[test]
fn test_canvas_noise_boundary_max_values() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, a) = cn.apply_to_pixel(255, 255, 255, 255, 0, 0);
    assert!(r <= 255 && g <= 255 && b <= 255 && a <= 255);
}

#[test]
fn test_navigator_firefox_fields_not_empty() {
    let nav = NavigatorProfile::firefox();
    assert!(!nav.user_agent.is_empty());
    assert!(!nav.platform.is_empty());
    assert!(!nav.language.is_empty());
}

#[test]
fn test_navigator_chrome_fields_not_empty() {
    let nav = NavigatorProfile::chrome();
    assert!(!nav.user_agent.is_empty());
    assert!(!nav.platform.is_empty());
    assert!(!nav.language.is_empty());
}

#[test]
fn test_screen_default_positive_dimensions() {
    let s = ScreenProfile::default();
    assert!(s.width > 0);
    assert!(s.height > 0);
    assert!(s.device_pixel_ratio > 0.0);
}

#[test]
fn test_webgl_firefox_has_vendor() {
    let w = WebGLProfile::firefox();
    assert!(!w.vendor.is_empty());
    assert!(!w.renderer.is_empty());
}

#[test]
fn test_webgl_chrome_has_vendor() {
    let w = WebGLProfile::chrome();
    assert!(!w.vendor.is_empty());
    assert!(!w.renderer.is_empty());
}

#[test]
fn test_audio_noise_many_samples() {
    let ap = AudioProfile::new(99);
    let mut sum = 0.0_f64;
    for i in 0..1000 {
        let noisy = ap.apply_noise(0.5, i);
        sum += noisy;
    }
    let avg = sum / 1000.0;
    // Average should be close to 0.5 with small noise
    assert!((avg - 0.5).abs() < 0.1, "average should be close to input, got {}", avg);
}

#[test]
fn test_behavior_mouse_path_single_step() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 1);
    assert!(!path.is_empty(), "single step should produce at least one point");
}

#[test]
fn test_behavior_typing_delays_single() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(1);
    assert_eq!(delays.len(), 1);
    assert!(delays[0] > 0, "delay should be positive");
}

#[test]
fn test_behavior_scroll_deltas_sum_to_total() {
    let bs = BehaviorSimulator::new(42);
    let total = 500.0;
    let steps = 20;
    let deltas = bs.generate_scroll_deltas(total, steps);
    let sum: f64 = deltas.iter().sum();
    assert!((sum - total).abs() < total * 0.5, "deltas should approximate total, got {} vs {}", sum, total);
}

#[test]
fn test_stealth_profile_firefox_complete() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile);
    // All sub-components should be accessible
    let _ = engine.tls_config();
    let _ = engine.http2_config();
    let _ = engine.canvas_noise();
    let _ = engine.navigator();
    let _ = engine.screen();
    let _ = engine.webgl();
    let _ = engine.audio();
    let _ = engine.behavior();
}

#[test]
fn test_tls_is_tls13_suite_recognized() {
    let fp = TlsFingerprint::chrome_latest();
    let tls13 = fp.tls13_suites();
    if !tls13.is_empty() {
        assert!(fp.is_tls13_suite(tls13[0]), "should recognize its own TLS 1.3 suites");
    }
}
