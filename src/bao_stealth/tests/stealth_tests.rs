// @trace TEST-STL-001 [req:REQ-STL-001] [level:unit]
// @trace TEST-STL-002 [req:REQ-STL-002] [level:unit]
// @trace TEST-STL-003 [req:REQ-STL-003] [level:unit]
// @trace TEST-STL-004 [req:REQ-STL-004] [level:unit]
// @trace TEST-STL-005 [req:REQ-STL-005] [level:unit]
// @trace TEST-STL-006 [req:REQ-STL-006] [level:unit]

use bao_stealth::*;

// ===========================================================================
// §1 TLS Fingerprint (REQ-STL-001)
// ===========================================================================

#[test]
fn test_tls_firefox_has_cipher_suites() {
    let fp = TlsFingerprint::firefox();
    assert!(!fp.cipher_suites.is_empty());
    assert!(fp.cipher_suites.contains(&0x1301)); // TLS_AES_128_GCM_SHA256
}

#[test]
fn test_tls_chrome_has_cipher_suites() {
    let fp = TlsFingerprint::chrome();
    assert!(!fp.cipher_suites.is_empty());
    assert!(fp.cipher_suites.contains(&0x1301));
}

#[test]
fn test_tls_compute_ja3_format() {
    let fp = TlsFingerprint::firefox();
    let ja3 = fp.compute_ja3();
    // JA3 format: "771,<ciphers>,<exts>,<curves>,<sigs>"
    assert!(ja3.starts_with("771,"));
    let parts: Vec<&str> = ja3.split(',').collect();
    assert_eq!(parts.len(), 5);
}

#[test]
fn test_tls_compute_ja4_format() {
    let fp = TlsFingerprint::firefox();
    let ja4 = fp.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_tls_tls13_vs_tls12_separation() {
    let fp = TlsFingerprint::firefox();
    let tls13 = fp.tls13_suites();
    let tls12 = fp.tls12_suites();
    assert!(!tls13.is_empty());
    assert!(!tls12.is_empty());
    assert_eq!(tls13.len() + tls12.len(), fp.cipher_suites.len());
}

#[test]
fn test_tls_is_tls13_suite() {
    let fp = TlsFingerprint::chrome();
    assert!(fp.is_tls13_suite(0x1301));
    assert!(fp.is_tls13_suite(0x1302));
    assert!(fp.is_tls13_suite(0x1303));
    assert!(!fp.is_tls13_suite(0xC02B)); // TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
}

#[test]
fn test_tls_alpn_strings() {
    let fp = TlsFingerprint::firefox();
    let alpn = fp.alpn_strings();
    assert!(alpn.contains(&"h2"));
    assert!(alpn.contains(&"http/1.1"));
}

#[test]
fn test_tls_chrome_latest_has_record_size_limit() {
    let fp = TlsFingerprint::chrome_latest();
    assert!(fp.record_size_limit.is_some());
    assert!(fp.compress_certificate_algos.len() > 0);
    assert_eq!(fp.application_settings_protocol, Some("h2"));
}

#[test]
fn test_tls_ja3_deterministic() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.compute_ja3(), fp.compute_ja3());
}

// ===========================================================================
// §2 HTTP/2 Fingerprint (REQ-STL-002)
// ===========================================================================

#[test]
fn test_http2_firefox_default_values() {
    let fp = Http2Fingerprint::firefox();
    assert_eq!(fp.header_table_size, 65536);
    assert!(!fp.enable_push);
    assert_eq!(fp.initial_window_size, 131072);
}

#[test]
fn test_http2_chrome_different_from_firefox() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.initial_window_size, ch.initial_window_size);
    assert_ne!(ff.pseudo_header_order, ch.pseudo_header_order);
}

#[test]
fn test_http2_akamai_fingerprint_format() {
    let fp = Http2Fingerprint::firefox();
    let ak = fp.akamai_fingerprint();
    let parts: Vec<&str> = ak.split(':').collect();
    assert_eq!(parts.len(), 6);
    assert_eq!(parts[0], "65536");
}

#[test]
fn test_http2_settings_frame_payload() {
    let fp = Http2Fingerprint::chrome();
    let settings = fp.settings_frame_payload();
    assert!(!settings.is_empty());
    // Each setting is (id, value)
    for (id, _) in &settings {
        assert!(id > &0u16);
    }
}

#[test]
fn test_http2_ordered_headers_pseudo_first() {
    let fp = Http2Fingerprint::chrome();
    let headers = vec![
        ("content-length", "100"),
        (":method", "GET"),
        (":path", "/"),
        ("host", "example.com"),
        (":authority", "example.com"),
        (":scheme", "https"),
    ];
    let ordered = fp.ordered_headers(&headers);
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":authority");
    assert_eq!(ordered[2].0, ":scheme");
    assert_eq!(ordered[3].0, ":path");
}

// ===========================================================================
// §3 Canvas Noise (REQ-STL-003)
// ===========================================================================

#[test]
fn test_canvas_deterministic() {
    let cn = CanvasNoise::new(42);
    let p1 = cn.apply_to_pixel(128, 128, 128, 255, 10, 20);
    let p2 = cn.apply_to_pixel(128, 128, 128, 255, 10, 20);
    assert_eq!(p1, p2);
}

#[test]
fn test_canvas_different_positions_differ() {
    let cn = CanvasNoise::new(42);
    let p1 = cn.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let p2 = cn.apply_to_pixel(128, 128, 128, 255, 100, 200);
    // Extremely unlikely to be identical
    assert_ne!(p1, p2);
}

#[test]
fn test_canvas_alpha_preserved() {
    let cn = CanvasNoise::new(42);
    let (_, _, _, a) = cn.apply_to_pixel(100, 100, 100, 200, 50, 50);
    assert_eq!(a, 200);
}

#[test]
fn test_canvas_noise_stays_in_range() {
    let cn = CanvasNoise::new(42);
    for x in 0..100u32 {
        for y in 0..100u32 {
            let (r, g, b, _) = cn.apply_to_pixel(0, 0, 0, 255, x, y);
            assert!(r <= 255);
            assert!(g <= 255);
            assert!(b <= 255);
        }
    }
}

#[test]
fn test_canvas_different_seeds_differ() {
    let cn1 = CanvasNoise::new(42);
    let cn2 = CanvasNoise::new(137);
    // Use multiple positions to verify seeds produce different output
    let mut differ = false;
    for x in 0..50u32 {
        for y in 0..50u32 {
            let p1 = cn1.apply_to_pixel(200, 180, 160, 255, x, y);
            let p2 = cn2.apply_to_pixel(200, 180, 160, 255, x, y);
            if p1 != p2 { differ = true; break; }
        }
        if differ { break; }
    }
    assert!(differ, "different seeds should produce different pixels");
}

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_zero_seed_panics() {
    CanvasNoise::new(0);
}

// ===========================================================================
// §4 Navigator Profile (REQ-STL-004)
// ===========================================================================

#[test]
fn test_navigator_firefox_profile() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.user_agent.contains("Firefox"));
    assert_eq!(nav.platform, "Linux x86_64");
    assert!(nav.oscpu.is_some());
    assert_eq!(nav.vendor, "");
}

#[test]
fn test_navigator_chrome_profile() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.user_agent.contains("Chrome"));
    assert_eq!(nav.vendor, "Google Inc.");
    assert!(nav.oscpu.is_none());
}

#[test]
fn test_screen_default() {
    let scr = ScreenProfile::default();
    assert_eq!(scr.width, 1920);
    assert_eq!(scr.height, 1080);
    assert_eq!(scr.device_pixel_ratio, 1.0);
    assert_eq!(scr.color_depth, 24);
}

#[test]
fn test_screen_custom() {
    let scr = ScreenProfile::new(2560, 1440, 2.0);
    assert_eq!(scr.width, 2560);
    assert_eq!(scr.height, 1440);
    assert_eq!(scr.device_pixel_ratio, 2.0);
    assert_eq!(scr.avail_height, 1400); // height - 40
}

// ===========================================================================
// §5 WebGL & Audio (REQ-STL-005)
// ===========================================================================

#[test]
fn test_webgl_firefox() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.vendor, "Mozilla");
    assert!(!gl.extensions.is_empty());
    assert!(gl.extensions.contains(&"WEBGL_debug_renderer_info".to_string()));
    assert_eq!(gl.max_texture_size, 16384);
}

#[test]
fn test_webgl_chrome() {
    let gl = WebGLProfile::chrome();
    assert!(gl.vendor.contains("Google"));
    assert!(!gl.extensions.is_empty());
}

#[test]
fn test_audio_noise_deterministic() {
    let audio = AudioProfile::new(42);
    let s1 = audio.apply_noise(0.5, 100);
    let s2 = audio.apply_noise(0.5, 100);
    assert_eq!(s1, s2);
}

#[test]
fn test_audio_noise_adds_small_perturbation() {
    let audio = AudioProfile::new(42);
    let original = 0.5_f64;
    let noisy = audio.apply_noise(original, 0);
    // Noise amplitude is 1e-7, so difference should be tiny
    let diff = (noisy - original).abs();
    assert!(diff < 1e-5);
}

#[test]
fn test_audio_noise_different_indices_differ() {
    let audio = AudioProfile::new(42);
    let s1 = audio.apply_noise(0.5, 0);
    let s2 = audio.apply_noise(0.5, 1);
    assert_ne!(s1, s2);
}

// ===========================================================================
// §6 Behavior Simulator (REQ-STL-006)
// ===========================================================================

#[test]
fn test_behavior_mouse_path_starts_and_ends() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path.len(), 10);
    let (x0, y0) = path[0];
    let (xn, yn) = path[9];
    assert!(x0.abs() < 5.0);
    assert!(y0.abs() < 5.0);
    assert!((xn - 100.0).abs() < 5.0);
    assert!((yn - 100.0).abs() < 5.0);
}

#[test]
fn test_behavior_mouse_path_deterministic() {
    let sim = BehaviorSimulator::new(42);
    let p1 = sim.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    let p2 = sim.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    assert_eq!(p1, p2);
}

#[test]
fn test_behavior_typing_delays() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(10);
    assert_eq!(delays.len(), 10);
    for d in &delays {
        assert!(d >= &30 && d <= &150, "delay {} out of range", d);
    }
}

#[test]
fn test_behavior_scroll_deltas() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 30);
    assert_eq!(deltas.len(), 30);
    let total: f64 = deltas.iter().sum();
    // Total scroll should approximate target
    assert!(total > 500.0 && total < 1500.0, "total scroll {} unexpected", total);
}

// ===========================================================================
// §7 StealthEngine integration (REQ-STL-007)
// ===========================================================================

#[test]
fn test_engine_default_is_firefox() {
    let engine = StealthEngine::default_engine();
    assert!(engine.navigator().user_agent.contains("Firefox"));
    assert!(engine.tls_config().ja3_hash.contains("4865"));
}

#[test]
fn test_engine_firefox_profile() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    assert!(engine.navigator().user_agent.contains("Firefox"));
    assert!(engine.canvas_noise().seed() > 0);
    assert!(engine.audio().seed() > 0);
}

#[test]
fn test_engine_chrome_profile() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    assert!(engine.navigator().user_agent.contains("Chrome"));
    assert_eq!(engine.navigator().vendor, "Google Inc.");
}

#[test]
fn test_inject_navigator_js_contains_overrides() {
    let engine = StealthEngine::default_engine();
    let js = engine.inject_navigator_js();
    assert!(js.contains("navigator"));
    assert!(js.contains("userAgent"));
    assert!(js.contains("webdriver"));
    assert!(js.contains("screen"));
    assert!(js.contains("devicePixelRatio"));
    assert!(js.contains("WebGL"));
    assert!(js.contains("cdc_adoQpoasnfa76pfcZLmcfl"));
}
