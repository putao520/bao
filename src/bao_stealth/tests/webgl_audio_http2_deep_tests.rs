// @trace TEST-STL-017 [req:REQ-STL-005] [level:unit]
// @trace TEST-STL-018 [req:REQ-STL-002] [level:unit]
// WebGLProfile presets, AudioProfile noise, Http2Fingerprint Akamai format,
// settings frame, ordered headers, clone/debug, edge cases.

use bao_stealth::{WebGLProfile, AudioProfile, Http2Fingerprint};

// ---- WebGLProfile ----

#[test]
fn test_webgl_firefox_vendor() {
    let w = WebGLProfile::firefox();
    assert_eq!(w.vendor, "Mozilla");
}

#[test]
fn test_webgl_firefox_renderer_contains_webgl() {
    let w = WebGLProfile::firefox();
    assert!(w.renderer.contains("WebGL"));
}

#[test]
fn test_webgl_firefox_has_extensions() {
    let w = WebGLProfile::firefox();
    assert!(!w.extensions.is_empty());
    assert!(w.extensions.iter().any(|e| e.contains("ANGLE")));
    assert!(w.extensions.iter().any(|e| e.contains("OES")));
    assert!(w.extensions.iter().any(|e| e.contains("WEBGL")));
}

#[test]
fn test_webgl_firefox_max_texture_size() {
    let w = WebGLProfile::firefox();
    assert_eq!(w.max_texture_size, 16384);
    assert_eq!(w.max_renderbuffer_size, 16384);
}

#[test]
fn test_webgl_firefox_viewport_dims() {
    let w = WebGLProfile::firefox();
    assert_eq!(w.max_viewport_dims, [16384, 16384]);
}

#[test]
fn test_webgl_chrome_vendor() {
    let w = WebGLProfile::chrome();
    assert!(w.vendor.contains("Google"));
}

#[test]
fn test_webgl_chrome_renderer_contains_angle() {
    let w = WebGLProfile::chrome();
    assert!(w.renderer.contains("ANGLE"));
}

#[test]
fn test_webgl_chrome_has_extensions() {
    let w = WebGLProfile::chrome();
    assert!(!w.extensions.is_empty());
}

#[test]
fn test_webgl_firefox_more_extensions_than_chrome() {
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();
    assert!(ff.extensions.len() > ch.extensions.len());
}

#[test]
fn test_webgl_firefox_vs_chrome_differ() {
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();
    assert_ne!(ff.vendor, ch.vendor);
    assert_ne!(ff.renderer, ch.renderer);
}

#[test]
fn test_webgl_clone() {
    let w = WebGLProfile::firefox();
    let cloned = w.clone();
    assert_eq!(cloned.vendor, w.vendor);
    assert_eq!(cloned.extensions.len(), w.extensions.len());
    assert_eq!(cloned.max_texture_size, w.max_texture_size);
}

#[test]
fn test_webgl_debug() {
    let w = WebGLProfile::chrome();
    let debug = format!("{:?}", w);
    assert!(debug.contains("Google"));
    assert!(debug.contains("ANGLE"));
}

// ---- AudioProfile ----

#[test]
fn test_audio_new_seed() {
    let a = AudioProfile::new(42);
    assert_eq!(a.seed(), 42);
}

#[test]
fn test_audio_default_amplitude() {
    let a = AudioProfile::new(1);
    assert!((a.noise_amplitude() - 1e-7).abs() < f64::EPSILON);
}

#[test]
fn test_audio_default_sample_rate() {
    let a = AudioProfile::new(1);
    assert_eq!(a.sample_rate(), 44100);
}

#[test]
fn test_audio_apply_noise_deterministic() {
    let a = AudioProfile::new(42);
    let s1 = a.apply_noise(1.0, 100);
    let s2 = a.apply_noise(1.0, 100);
    assert_eq!(s1, s2);
}

#[test]
fn test_audio_apply_noise_different_indices() {
    let a = AudioProfile::new(42);
    let s1 = a.apply_noise(1.0, 0);
    let s2 = a.apply_noise(1.0, 1);
    let s3 = a.apply_noise(1.0, 2);
    // At least some should differ
    assert!(s1 != s2 || s1 != s3 || s2 != s3, "Different indices should produce different noise");
}

#[test]
fn test_audio_apply_noise_different_seeds() {
    let a1 = AudioProfile::new(1);
    let a2 = AudioProfile::new(2);
    let s1 = a1.apply_noise(1.0, 50);
    let s2 = a2.apply_noise(1.0, 50);
    assert_ne!(s1, s2, "Different seeds should produce different noise");
}

#[test]
fn test_audio_noise_is_small() {
    let a = AudioProfile::new(42);
    for i in 0..1000u32 {
        let noisy = a.apply_noise(1.0, i);
        let diff = (noisy - 1.0).abs();
        assert!(diff < 1e-6, "Noise should be tiny, got diff {} at index {}", diff, i);
    }
}

#[test]
fn test_audio_noise_preserves_sign() {
    let a = AudioProfile::new(42);
    let noisy = a.apply_noise(0.5, 10);
    assert!(noisy > 0.0, "Positive sample should remain positive");
    let noisy_neg = a.apply_noise(-0.5, 10);
    assert!(noisy_neg < 0.0, "Negative sample should remain negative");
}

#[test]
fn test_audio_clone() {
    let a = AudioProfile::new(999);
    let cloned = a.clone();
    assert_eq!(cloned.seed(), 999);
}

#[test]
fn test_audio_debug() {
    let a = AudioProfile::new(42);
    let debug = format!("{:?}", a);
    assert!(debug.contains("AudioProfile"));
}

// ---- Http2Fingerprint ----

#[test]
fn test_http2_firefox_akamai_format() {
    let h = Http2Fingerprint::firefox();
    let fp = h.akamai_fingerprint();
    assert!(fp.contains("65536")); // header_table_size
    assert!(fp.contains("131072")); // initial_window_size
}

#[test]
fn test_http2_chrome_akamai_format() {
    let h = Http2Fingerprint::chrome();
    let fp = h.akamai_fingerprint();
    assert!(fp.contains("65536"));
    assert!(fp.contains("6291456")); // chrome initial_window_size
}

#[test]
fn test_http2_firefox_vs_chrome_differ() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.akamai_fingerprint(), ch.akamai_fingerprint());
}

#[test]
fn test_http2_firefox_enable_push_false() {
    let h = Http2Fingerprint::firefox();
    assert!(!h.enable_push);
}

#[test]
fn test_http2_chrome_enable_push_false() {
    let h = Http2Fingerprint::chrome();
    assert!(!h.enable_push);
}

#[test]
fn test_http2_firefox_max_concurrent_streams() {
    let h = Http2Fingerprint::firefox();
    assert_eq!(h.max_concurrent_streams, 100);
}

#[test]
fn test_http2_chrome_max_concurrent_streams() {
    let h = Http2Fingerprint::chrome();
    assert_eq!(h.max_concurrent_streams, 1000);
}

#[test]
fn test_http2_settings_frame_payload_length() {
    let h = Http2Fingerprint::firefox();
    let payload = h.settings_frame_payload();
    assert_eq!(payload.len(), 6); // 6 settings parameters
}

#[test]
fn test_http2_settings_frame_contains_header_table_size() {
    let h = Http2Fingerprint::firefox();
    let payload = h.settings_frame_payload();
    assert!(payload.iter().any(|(id, val)| *id == 0x01 && *val == 65536));
}

#[test]
fn test_http2_settings_frame_contains_window_size() {
    let h = Http2Fingerprint::chrome();
    let payload = h.settings_frame_payload();
    assert!(payload.iter().any(|(id, val)| *id == 0x02 && *val == 6291456));
}

#[test]
fn test_http2_firefox_pseudo_header_order() {
    let h = Http2Fingerprint::firefox();
    assert_eq!(h.pseudo_header_order, vec![":method", ":path", ":authority", ":scheme"]);
}

#[test]
fn test_http2_chrome_pseudo_header_order() {
    let h = Http2Fingerprint::chrome();
    assert_eq!(h.pseudo_header_order, vec![":method", ":authority", ":scheme", ":path"]);
}

#[test]
fn test_http2_ordered_headers_reorders() {
    let h = Http2Fingerprint::firefox();
    let headers = vec![
        ("content-length", "100"),
        (":scheme", "https"),
        (":method", "GET"),
        ("host", "example.com"),
        (":path", "/"),
        (":authority", "example.com"),
    ];
    let ordered = h.ordered_headers(&headers);
    // Firefox order: :method, :path, :authority, :scheme, then rest
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
    assert_eq!(ordered[2].0, ":authority");
    assert_eq!(ordered[3].0, ":scheme");
    // Remaining headers in original order
    assert_eq!(ordered[4].0, "content-length");
    assert_eq!(ordered[5].0, "host");
}

#[test]
fn test_http2_ordered_headers_chrome_order() {
    let h = Http2Fingerprint::chrome();
    let headers = vec![
        (":path", "/"),
        (":method", "GET"),
        (":scheme", "https"),
        (":authority", "example.com"),
    ];
    let ordered = h.ordered_headers(&headers);
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":authority");
    assert_eq!(ordered[2].0, ":scheme");
    assert_eq!(ordered[3].0, ":path");
}

#[test]
fn test_http2_ordered_headers_no_pseudo() {
    let h = Http2Fingerprint::firefox();
    let headers = vec![
        ("content-type", "text/html"),
        ("host", "example.com"),
    ];
    let ordered = h.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "content-type");
    assert_eq!(ordered[1].0, "host");
}

#[test]
fn test_http2_ordered_headers_empty() {
    let h = Http2Fingerprint::firefox();
    let headers: Vec<(&str, &str)> = vec![];
    let ordered = h.ordered_headers(&headers);
    assert!(ordered.is_empty());
}

#[test]
fn test_http2_ordered_headers_only_pseudo() {
    let h = Http2Fingerprint::chrome();
    let headers = vec![
        (":method", "POST"),
        (":path", "/api"),
        (":authority", "api.example.com"),
        (":scheme", "https"),
    ];
    let ordered = h.ordered_headers(&headers);
    assert_eq!(ordered.len(), 4);
}

#[test]
fn test_http2_window_update_size_firefox() {
    let h = Http2Fingerprint::firefox();
    assert_eq!(h.window_update_size, 131072);
}

#[test]
fn test_http2_window_update_size_chrome() {
    let h = Http2Fingerprint::chrome();
    assert_eq!(h.window_update_size, 15663105);
}

#[test]
fn test_http2_clone() {
    let h = Http2Fingerprint::firefox();
    let cloned = h.clone();
    assert_eq!(cloned.header_table_size, h.header_table_size);
    assert_eq!(cloned.akamai_fingerprint(), h.akamai_fingerprint());
}

#[test]
fn test_http2_debug() {
    let h = Http2Fingerprint::chrome();
    let debug = format!("{:?}", h);
    assert!(debug.contains("Http2Fingerprint"));
}

// ---- Cross-profile consistency ----

#[test]
fn test_firefox_webgl_matches_firefox_http2() {
    let w = WebGLProfile::firefox();
    let h = Http2Fingerprint::firefox();
    // Both should be "Firefox-like" presets
    assert!(w.vendor.contains("Mozilla"));
    assert!(h.akamai_fingerprint().contains("131072"));
}

#[test]
fn test_chrome_webgl_matches_chrome_http2() {
    let w = WebGLProfile::chrome();
    let h = Http2Fingerprint::chrome();
    assert!(w.vendor.contains("Google"));
    assert!(h.akamai_fingerprint().contains("6291456"));
}
