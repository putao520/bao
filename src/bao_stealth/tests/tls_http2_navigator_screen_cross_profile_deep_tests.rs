// @trace TEST-STL-051 [req:REQ-STL-001,REQ-STL-002,REQ-STL-004,REQ-STL-007] [level:unit]
// TLS chrome_latest fields, HTTP2 ordered_headers edge cases,
// ScreenProfile::new boundary values, NavigatorProfile cross-profile consistency.

use bao_stealth::{TlsFingerprint, Http2Fingerprint, NavigatorProfile, ScreenProfile, StealthProfile, StealthEngine};

// ============================================================================
// TLS: chrome_latest specific fields
// ============================================================================

#[test]
fn test_chrome_latest_has_record_size_limit() {
    let tls = TlsFingerprint::chrome_latest();
    assert_eq!(tls.record_size_limit, Some(0x4001));
}

#[test]
fn test_chrome_latest_compress_cert_algos() {
    let tls = TlsFingerprint::chrome_latest();
    assert_eq!(tls.compress_certificate_algos, vec![0x0002, 0x0001]);
}

#[test]
fn test_chrome_latest_application_settings_h2() {
    let tls = TlsFingerprint::chrome_latest();
    assert_eq!(tls.application_settings_protocol, Some("h2"));
}

#[test]
fn test_chrome_latest_extensions_count() {
    let tls = TlsFingerprint::chrome_latest();
    // Has extra extensions 0x001C and 0x0039 compared to chrome_120
    assert!(tls.extensions.len() > TlsFingerprint::chrome_120().extensions.len());
}

#[test]
fn test_firefox_record_size_limit_none() {
    let tls = TlsFingerprint::firefox();
    assert!(tls.record_size_limit.is_none());
}

#[test]
fn test_firefox_compress_cert_empty() {
    let tls = TlsFingerprint::firefox();
    assert!(tls.compress_certificate_algos.is_empty());
}

#[test]
fn test_firefox_application_settings_none() {
    let tls = TlsFingerprint::firefox();
    assert!(tls.application_settings_protocol.is_none());
}

#[test]
fn test_chrome_120_record_size_limit_none() {
    let tls = TlsFingerprint::chrome_120();
    assert!(tls.record_size_limit.is_none());
}

#[test]
fn test_chrome_120_compress_cert_empty() {
    let tls = TlsFingerprint::chrome_120();
    assert!(tls.compress_certificate_algos.is_empty());
}

#[test]
fn test_chrome_120_application_settings_none() {
    let tls = TlsFingerprint::chrome_120();
    assert!(tls.application_settings_protocol.is_none());
}

#[test]
fn test_tls13_suite_count_firefox() {
    let tls = TlsFingerprint::firefox();
    let tls13 = tls.tls13_suites();
    assert_eq!(tls13.len(), 3); // 0x1301, 0x1303, 0x1302
}

#[test]
fn test_tls13_suite_count_chrome_latest() {
    let tls = TlsFingerprint::chrome_latest();
    let tls13 = tls.tls13_suites();
    assert_eq!(tls13.len(), 3); // 0x1301, 0x1302, 0x1303
}

#[test]
fn test_tls12_suite_count_firefox() {
    let tls = TlsFingerprint::firefox();
    let tls12 = tls.tls12_suites();
    assert_eq!(tls12.len(), 12); // 15 total - 3 TLS 1.3
}

#[test]
fn test_tls12_suite_count_chrome_latest() {
    let tls = TlsFingerprint::chrome_latest();
    let tls12 = tls.tls12_suites();
    assert_eq!(tls12.len(), 10); // 13 total - 3 TLS 1.3
}

#[test]
fn test_tls13_partition_completeness_chrome_120() {
    let tls = TlsFingerprint::chrome_120();
    let tls13 = tls.tls13_suites();
    let tls12 = tls.tls12_suites();
    assert_eq!(tls13.len() + tls12.len(), tls.cipher_suites.len());
}

#[test]
fn test_firefox_chrome_share_some_cipher_suites() {
    let ff = TlsFingerprint::firefox();
    let cr = TlsFingerprint::chrome_latest();
    let shared: Vec<_> = ff.cipher_suites.iter().filter(|c| cr.cipher_suites.contains(c)).collect();
    assert!(shared.len() > 5);
}

#[test]
fn test_firefox_has_more_cipher_suites_than_chrome() {
    let ff = TlsFingerprint::firefox();
    let cr = TlsFingerprint::chrome_latest();
    assert!(ff.cipher_suites.len() > cr.cipher_suites.len());
}

#[test]
fn test_compute_ja3_firefox_format() {
    let tls = TlsFingerprint::firefox();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
    assert!(ja3.contains("-"));
}

#[test]
fn test_compute_ja3_chrome_latest_format() {
    let tls = TlsFingerprint::chrome_latest();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
}

#[test]
fn test_compute_ja4_firefox_format_prefix() {
    let tls = TlsFingerprint::firefox();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_chrome_latest_format_prefix() {
    let tls = TlsFingerprint::chrome_latest();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_firefox_chrome_ja3_differ() {
    assert_ne!(
        TlsFingerprint::firefox().compute_ja3(),
        TlsFingerprint::chrome_latest().compute_ja3()
    );
}

#[test]
fn test_firefox_chrome_ja4_differ() {
    assert_ne!(
        TlsFingerprint::firefox().compute_ja4(),
        TlsFingerprint::chrome_latest().compute_ja4()
    );
}

// ============================================================================
// HTTP2: ordered_headers edge cases
// ============================================================================

#[test]
fn test_ordered_headers_mixed_pseudo_and_regular() {
    let h2 = Http2Fingerprint::firefox();
    let headers = vec![
        ("accept", "text/html"),
        (":method", "GET"),
        ("host", "example.com"),
        (":path", "/"),
    ];
    let ordered = h2.ordered_headers(&headers);
    // Pseudo headers should come first in firefox order: :method, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
    // Regular headers follow in original order
    assert_eq!(ordered[2].0, "accept");
    assert_eq!(ordered[3].0, "host");
}

#[test]
fn test_ordered_headers_chrome_different_order() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        (":path", "/"),
        (":method", "GET"),
        (":scheme", "https"),
        (":authority", "example.com"),
    ];
    let ordered = h2.ordered_headers(&headers);
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":authority");
    assert_eq!(ordered[2].0, ":scheme");
    assert_eq!(ordered[3].0, ":path");
}

#[test]
fn test_ordered_headers_all_regular_no_pseudo() {
    let h2 = Http2Fingerprint::firefox();
    let headers = vec![
        ("accept", "text/html"),
        ("content-type", "application/json"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "accept");
    assert_eq!(ordered[1].0, "content-type");
}

#[test]
fn test_ordered_headers_preserves_values() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        (":method", "POST"),
        (":path", "/api/data"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered.iter().find(|(k, _)| *k == ":method").unwrap().1, "POST");
    assert_eq!(ordered.iter().find(|(k, _)| *k == ":path").unwrap().1, "/api/data");
}

#[test]
fn test_settings_frame_payload_firefox_count() {
    let h2 = Http2Fingerprint::firefox();
    let payload = h2.settings_frame_payload();
    assert_eq!(payload.len(), 6);
}

#[test]
fn test_settings_frame_payload_chrome_count() {
    let h2 = Http2Fingerprint::chrome();
    let payload = h2.settings_frame_payload();
    assert_eq!(payload.len(), 6);
}

#[test]
fn test_settings_frame_payload_ids() {
    let h2 = Http2Fingerprint::firefox();
    let payload = h2.settings_frame_payload();
    let ids: Vec<u16> = payload.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![0x01, 0x03, 0x04, 0x02, 0x05, 0x06]);
}

#[test]
fn test_akamai_fingerprint_firefox_format() {
    let h2 = Http2Fingerprint::firefox();
    let fp = h2.akamai_fingerprint();
    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 6);
}

#[test]
fn test_akamai_fingerprint_chrome_format() {
    let h2 = Http2Fingerprint::chrome();
    let fp = h2.akamai_fingerprint();
    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 6);
}

#[test]
fn test_firefox_chrome_akamai_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().akamai_fingerprint(),
        Http2Fingerprint::chrome().akamai_fingerprint()
    );
}

#[test]
fn test_firefox_chrome_window_update_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().window_update_size,
        Http2Fingerprint::chrome().window_update_size
    );
}

#[test]
fn test_firefox_chrome_initial_window_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().initial_window_size,
        Http2Fingerprint::chrome().initial_window_size
    );
}

#[test]
fn test_firefox_chrome_concurrent_streams_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().max_concurrent_streams,
        Http2Fingerprint::chrome().max_concurrent_streams
    );
}

#[test]
fn test_both_presets_enable_push_false() {
    assert!(!Http2Fingerprint::firefox().enable_push);
    assert!(!Http2Fingerprint::chrome().enable_push);
}

#[test]
fn test_both_presets_share_max_frame_size() {
    assert_eq!(
        Http2Fingerprint::firefox().max_frame_size,
        Http2Fingerprint::chrome().max_frame_size
    );
}

// ============================================================================
// NavigatorProfile cross-profile
// ============================================================================

#[test]
fn test_firefox_chrome_ua_differ() {
    assert_ne!(
        NavigatorProfile::firefox().user_agent,
        NavigatorProfile::chrome().user_agent
    );
}

#[test]
fn test_firefox_oscpu_some_chrome_none() {
    assert!(NavigatorProfile::firefox().oscpu.is_some());
    assert!(NavigatorProfile::chrome().oscpu.is_none());
}

#[test]
fn test_firefox_build_id_some_chrome_none() {
    assert!(NavigatorProfile::firefox().build_id.is_some());
    assert!(NavigatorProfile::chrome().build_id.is_none());
}

#[test]
fn test_firefox_vendor_empty_chrome_google() {
    assert_eq!(NavigatorProfile::firefox().vendor, "");
    assert!(NavigatorProfile::chrome().vendor.contains("Google"));
}

#[test]
fn test_firefox_chrome_product_sub_differ() {
    assert_ne!(
        NavigatorProfile::firefox().product_sub,
        NavigatorProfile::chrome().product_sub
    );
}

#[test]
fn test_firefox_chrome_same_platform() {
    assert_eq!(
        NavigatorProfile::firefox().platform,
        NavigatorProfile::chrome().platform
    );
}

#[test]
fn test_firefox_chrome_same_language() {
    assert_eq!(
        NavigatorProfile::firefox().language,
        NavigatorProfile::chrome().language
    );
}

#[test]
fn test_firefox_chrome_same_hardware_concurrency() {
    assert_eq!(
        NavigatorProfile::firefox().hardware_concurrency,
        NavigatorProfile::chrome().hardware_concurrency
    );
}

#[test]
fn test_firefox_chrome_same_max_touch_points() {
    assert_eq!(
        NavigatorProfile::firefox().max_touch_points,
        NavigatorProfile::chrome().max_touch_points
    );
}

#[test]
fn test_firefox_chrome_app_version_differ() {
    assert_ne!(
        NavigatorProfile::firefox().app_version,
        NavigatorProfile::chrome().app_version
    );
}

#[test]
fn test_firefox_ua_contains_firefox() {
    assert!(NavigatorProfile::firefox().user_agent.contains("Firefox"));
}

#[test]
fn test_chrome_ua_contains_chrome() {
    assert!(NavigatorProfile::chrome().user_agent.contains("Chrome"));
}

#[test]
fn test_firefox_ua_contains_gecko() {
    assert!(NavigatorProfile::firefox().user_agent.contains("Gecko"));
}

#[test]
fn test_chrome_ua_contains_applewebkit() {
    assert!(NavigatorProfile::chrome().user_agent.contains("AppleWebKit"));
}

// ============================================================================
// ScreenProfile boundary values
// ============================================================================

#[test]
fn test_screen_new_avail_height_less_than_height() {
    let s = ScreenProfile::new(1920, 1080, 1.0);
    assert_eq!(s.height, 1080);
    assert_eq!(s.avail_height, 1040); // height - 40
}

#[test]
fn test_screen_new_small_height() {
    let s = ScreenProfile::new(800, 600, 1.0);
    assert_eq!(s.avail_height, 560);
}

#[test]
#[should_panic(expected = "attempt to subtract with overflow")]
fn test_screen_new_tiny_height_panics() {
    let _ = ScreenProfile::new(640, 30, 1.0);
}

#[test]
fn test_screen_new_width_equals_avail_width() {
    let s = ScreenProfile::new(1920, 1080, 1.0);
    assert_eq!(s.width, s.avail_width);
}

#[test]
fn test_screen_default_color_depth_24() {
    let s = ScreenProfile::default();
    assert_eq!(s.color_depth, 24);
}

#[test]
fn test_screen_default_pixel_depth_24() {
    let s = ScreenProfile::default();
    assert_eq!(s.pixel_depth, 24);
}

#[test]
fn test_screen_default_dpr_1() {
    let s = ScreenProfile::default();
    assert!((s.device_pixel_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_custom_dpr() {
    let s = ScreenProfile::new(3840, 2160, 2.0);
    assert!((s.device_pixel_ratio - 2.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_hd_resolution() {
    let s = ScreenProfile::new(1280, 720, 1.5);
    assert_eq!(s.width, 1280);
    assert_eq!(s.height, 720);
    assert_eq!(s.avail_height, 680);
}

#[test]
fn test_screen_default_1920x1080() {
    let s = ScreenProfile::default();
    assert_eq!(s.width, 1920);
    assert_eq!(s.height, 1080);
}

#[test]
fn test_screen_debug_contains_values() {
    let s = ScreenProfile::new(1920, 1080, 1.0);
    let dbg = format!("{:?}", s);
    assert!(dbg.contains("ScreenProfile"));
    assert!(dbg.contains("1920"));
    assert!(dbg.contains("1080"));
}

#[test]
fn test_screen_clone() {
    let s = ScreenProfile::new(1920, 1080, 2.0);
    let c = s.clone();
    assert_eq!(c.width, s.width);
    assert_eq!(c.height, s.height);
    assert_eq!(c.device_pixel_ratio, s.device_pixel_ratio);
}

// ============================================================================
// StealthProfile integration: TLS + HTTP2 + Navigator + Screen consistency
// ============================================================================

#[test]
fn test_firefox_profile_tls_is_firefox() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.tls.ja3_hash, TlsFingerprint::firefox().ja3_hash);
}

#[test]
fn test_chrome_profile_tls_is_chrome() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.tls.ja3_hash, TlsFingerprint::chrome().ja3_hash);
}

#[test]
fn test_firefox_profile_http2_is_firefox() {
    let p = StealthProfile::firefox_default();
    assert_eq!(
        p.http2.akamai_fingerprint(),
        Http2Fingerprint::firefox().akamai_fingerprint()
    );
}

#[test]
fn test_chrome_profile_http2_is_chrome() {
    let p = StealthProfile::chrome_default();
    assert_eq!(
        p.http2.akamai_fingerprint(),
        Http2Fingerprint::chrome().akamai_fingerprint()
    );
}

#[test]
fn test_firefox_profile_navigator_vendor_empty() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.navigator.vendor, "");
}

#[test]
fn test_chrome_profile_navigator_vendor_google() {
    let p = StealthProfile::chrome_default();
    assert!(p.navigator.vendor.contains("Google"));
}

#[test]
fn test_firefox_profile_screen_default() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.screen.width, 1920);
    assert_eq!(p.screen.height, 1080);
}

#[test]
fn test_chrome_profile_screen_default() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.screen.width, 1920);
    assert_eq!(p.screen.height, 1080);
}

#[test]
fn test_profiles_differ_in_tls() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.tls.compute_ja3(), cr.tls.compute_ja3());
}

#[test]
fn test_profiles_differ_in_http2() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.http2.akamai_fingerprint(), cr.http2.akamai_fingerprint());
}

#[test]
fn test_profiles_differ_in_navigator() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.navigator.user_agent, cr.navigator.user_agent);
}

#[test]
fn test_profiles_share_screen() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_eq!(ff.screen.width, cr.screen.width);
    assert_eq!(ff.screen.height, cr.screen.height);
}

// ============================================================================
// StealthEngine: accessor delegation
// ============================================================================

#[test]
fn test_engine_firefox_tls_matches_profile() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    assert_eq!(engine.tls_config().ja3_hash, TlsFingerprint::firefox().ja3_hash);
}

#[test]
fn test_engine_chrome_tls_matches_profile() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_eq!(engine.tls_config().compute_ja3(), TlsFingerprint::chrome().compute_ja3());
}

#[test]
fn test_engine_firefox_http2_matches() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    assert_eq!(
        engine.http2_config().akamai_fingerprint(),
        Http2Fingerprint::firefox().akamai_fingerprint()
    );
}

#[test]
fn test_engine_chrome_http2_matches() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    assert_eq!(
        engine.http2_config().akamai_fingerprint(),
        Http2Fingerprint::chrome().akamai_fingerprint()
    );
}

#[test]
fn test_engine_firefox_navigator_vendor() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    assert_eq!(engine.navigator().vendor, "");
}

#[test]
fn test_engine_chrome_navigator_vendor() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    assert!(engine.navigator().vendor.contains("Google"));
}

#[test]
fn test_engine_firefox_screen_dimensions() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    assert_eq!(engine.screen().width, 1920);
    assert_eq!(engine.screen().height, 1080);
}

#[test]
fn test_engine_profile_returns_reference() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let profile = engine.profile();
    assert_eq!(profile.tls.ja3_hash, TlsFingerprint::firefox().ja3_hash);
}
