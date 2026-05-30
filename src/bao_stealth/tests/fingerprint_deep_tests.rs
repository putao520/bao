// @trace TEST-STL-013-FINGERPRINT [req:REQ-STL-001,REQ-STL-002] [level:unit]
// TLS/HTTP2 fingerprint deep validation: JA3/JA4 computation, Akamai fingerprint,
// cipher suite classification, ALPN handling, header ordering, profile differentiation.

use bao_stealth::{TlsFingerprint, Http2Fingerprint, StealthProfile};

// ---- JA3 hash computation ----

#[test]
fn test_chrome_ja3_format() {
    let tls = TlsFingerprint::chrome();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
    // Should have 4 comma-separated fields
    assert!(ja3.starts_with("771,") && ja3.matches(',').count() >= 3);
    let parts: Vec<&str> = ja3.splitn(4, ',').collect();
    assert_eq!(parts.len(), 4);
    // Cipher suites should be dash-separated
    assert!(parts[1].contains("-"));
}

#[test]
fn test_firefox_ja3_format() {
    let tls = TlsFingerprint::firefox();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
    let parts: Vec<&str> = ja3.splitn(4, ',').collect();
    assert_eq!(parts.len(), 4);
}

#[test]
fn test_chrome_latest_ja3_format() {
    let tls = TlsFingerprint::chrome_latest();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
}

#[test]
fn test_ja3_deterministic() {
    let ja3_1 = TlsFingerprint::chrome().compute_ja3();
    let ja3_2 = TlsFingerprint::chrome().compute_ja3();
    assert_eq!(ja3_1, ja3_2);
}

#[test]
fn test_chrome_firefox_ja3_differ() {
    let ch = TlsFingerprint::chrome().compute_ja3();
    let ff = TlsFingerprint::firefox().compute_ja3();
    assert_ne!(ch, ff);
}

#[test]
fn test_chrome_vs_chrome_latest_ja3_differ() {
    let ch = TlsFingerprint::chrome_120().compute_ja3();
    let latest = TlsFingerprint::chrome_latest().compute_ja3();
    // Chrome latest has additional extensions, JA3 should differ
    assert_ne!(ch, latest);
}

// ---- JA4 fingerprint computation ----

#[test]
fn test_chrome_ja4_format() {
    let tls = TlsFingerprint::chrome();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
    assert!(ja4.contains("_"));
}

#[test]
fn test_firefox_ja4_format() {
    let tls = TlsFingerprint::firefox();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_ja4_deterministic() {
    let ja4_1 = TlsFingerprint::chrome().compute_ja4();
    let ja4_2 = TlsFingerprint::chrome().compute_ja4();
    assert_eq!(ja4_1, ja4_2);
}

#[test]
fn test_chrome_firefox_ja4_differ() {
    let ch = TlsFingerprint::chrome().compute_ja4();
    let ff = TlsFingerprint::firefox().compute_ja4();
    assert_ne!(ch, ff);
}

// ---- Cipher suite classification ----

#[test]
fn test_tls13_suites_chrome() {
    let tls = TlsFingerprint::chrome();
    let tls13 = tls.tls13_suites();
    assert!(!tls13.is_empty());
    for suite in &tls13 {
        assert!((0x1301..=0x1303).contains(suite));
    }
}

#[test]
fn test_tls12_suites_chrome() {
    let tls = TlsFingerprint::chrome();
    let tls12 = tls.tls12_suites();
    assert!(!tls12.is_empty());
    for suite in &tls12 {
        assert!(!(0x1301..=0x1303).contains(suite));
    }
}

#[test]
fn test_tls13_tls12_partition() {
    let tls = TlsFingerprint::chrome();
    let total = tls.cipher_suites.len();
    let tls13 = tls.tls13_suites().len();
    let tls12 = tls.tls12_suites().len();
    assert_eq!(total, tls13 + tls12);
}

#[test]
fn test_is_tls13_suite() {
    let tls = TlsFingerprint::chrome();
    assert!(tls.is_tls13_suite(0x1301));
    assert!(tls.is_tls13_suite(0x1302));
    assert!(tls.is_tls13_suite(0x1303));
    assert!(!tls.is_tls13_suite(0xC02B));
    assert!(!tls.is_tls13_suite(0x009E));
}

#[test]
fn test_firefox_tls13_suites() {
    let tls = TlsFingerprint::firefox();
    assert!(!tls.tls13_suites().is_empty());
}

// ---- ALPN handling ----

#[test]
fn test_chrome_alpn_strings() {
    let tls = TlsFingerprint::chrome();
    let alpn = tls.alpn_strings();
    assert!(alpn.contains(&"h2"));
    assert!(alpn.contains(&"http/1.1"));
}

#[test]
fn test_firefox_alpn_strings() {
    let tls = TlsFingerprint::firefox();
    let alpn = tls.alpn_strings();
    assert!(alpn.contains(&"h2"));
}

// ---- TLS version field ----

#[test]
fn test_tls_version_is_numeric() {
    let tls = TlsFingerprint::chrome();
    assert!(tls.tls_version.parse::<u32>().is_ok());
    assert_eq!(tls.tls_version, "771");
}

// ---- Chrome latest specific features ----

#[test]
fn test_chrome_latest_has_record_size_limit() {
    let tls = TlsFingerprint::chrome_latest();
    assert!(tls.record_size_limit.is_some());
}

#[test]
fn test_chrome_120_no_record_size_limit() {
    let tls = TlsFingerprint::chrome_120();
    assert!(tls.record_size_limit.is_none());
}

#[test]
fn test_chrome_latest_has_compress_cert() {
    let tls = TlsFingerprint::chrome_latest();
    assert!(!tls.compress_certificate_algos.is_empty());
}

#[test]
fn test_chrome_latest_has_app_settings() {
    let tls = TlsFingerprint::chrome_latest();
    assert!(tls.application_settings_protocol.is_some());
}

#[test]
fn test_chrome_latest_more_extensions_than_120() {
    let ch120 = TlsFingerprint::chrome_120();
    let latest = TlsFingerprint::chrome_latest();
    assert!(latest.extensions.len() > ch120.extensions.len());
}

// ---- HTTP/2 Akamai fingerprint ----

#[test]
fn test_chrome_akamai_fingerprint_format() {
    let h2 = Http2Fingerprint::chrome();
    let fp = h2.akamai_fingerprint();
    assert!(fp.contains(":"));
    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 6);
}

#[test]
fn test_firefox_akamai_fingerprint_format() {
    let h2 = Http2Fingerprint::firefox();
    let fp = h2.akamai_fingerprint();
    let parts: Vec<&str> = fp.split(':').collect();
    assert_eq!(parts.len(), 6);
}

#[test]
fn test_chrome_firefox_akamai_differ() {
    let ch = Http2Fingerprint::chrome().akamai_fingerprint();
    let ff = Http2Fingerprint::firefox().akamai_fingerprint();
    assert_ne!(ch, ff);
}

// ---- HTTP/2 settings frame ----

#[test]
fn test_chrome_settings_frame() {
    let h2 = Http2Fingerprint::chrome();
    let settings = h2.settings_frame_payload();
    assert!(!settings.is_empty());
    // Each setting is (identifier, value)
    for (id, _val) in &settings {
        assert!(*id > 0);
    }
}

#[test]
fn test_firefox_settings_frame() {
    let h2 = Http2Fingerprint::firefox();
    let settings = h2.settings_frame_payload();
    assert!(!settings.is_empty());
}

#[test]
fn test_chrome_settings_has_push_disabled() {
    let h2 = Http2Fingerprint::chrome();
    let settings = h2.settings_frame_payload();
    // SETTINGS_ENABLE_PUSH (0x03) should be 0
    let push_setting = settings.iter().find(|(id, _)| *id == 0x03);
    assert!(push_setting.is_some());
    assert_eq!(push_setting.unwrap().1, 0);
}

// ---- HTTP/2 header ordering ----

#[test]
fn test_chrome_pseudo_header_order() {
    let h2 = Http2Fingerprint::chrome();
    assert_eq!(h2.pseudo_header_order, vec![":method", ":authority", ":scheme", ":path"]);
}

#[test]
fn test_firefox_pseudo_header_order() {
    let h2 = Http2Fingerprint::firefox();
    assert_eq!(h2.pseudo_header_order, vec![":method", ":path", ":authority", ":scheme"]);
}

#[test]
fn test_ordered_headers_places_pseudo_first() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        ("content-type", "text/html"),
        (":path", "/index.html"),
        ("accept", "*/*"),
        (":method", "GET"),
        (":authority", "example.com"),
    ];
    let ordered = h2.ordered_headers(&headers);
    // Pseudo headers should come first in chrome order
    assert!(ordered[0].0.starts_with(':'));
    assert!(ordered[1].0.starts_with(':'));
    assert!(ordered[2].0.starts_with(':'));
    // Then regular headers
    assert!(!ordered[3].0.starts_with(':'));
}

#[test]
fn test_ordered_headers_preserves_all() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        ("a", "1"), ("b", "2"), ("c", "3"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered.len(), 3);
}

#[test]
fn test_ordered_headers_empty_input() {
    let h2 = Http2Fingerprint::chrome();
    let ordered = h2.ordered_headers(&[]);
    assert!(ordered.is_empty());
}

// ---- HTTP/2 field values ----

#[test]
fn test_chrome_window_size() {
    let h2 = Http2Fingerprint::chrome();
    assert!(h2.initial_window_size > 0);
    assert!(h2.window_update_size > 0);
}

#[test]
fn test_firefox_window_size() {
    let h2 = Http2Fingerprint::firefox();
    assert!(h2.initial_window_size > 0);
    assert!(h2.window_update_size > 0);
}

#[test]
fn test_chrome_firefox_window_sizes_differ() {
    let ch = Http2Fingerprint::chrome();
    let ff = Http2Fingerprint::firefox();
    assert_ne!(ch.initial_window_size, ff.initial_window_size);
}

#[test]
fn test_max_frame_size_reasonable() {
    let ch = Http2Fingerprint::chrome();
    let ff = Http2Fingerprint::firefox();
    // HTTP/2 spec minimum is 16384
    assert!(ch.max_frame_size >= 16384);
    assert!(ff.max_frame_size >= 16384);
}

// ---- Profile integration ----

#[test]
fn test_stealth_profile_tls_matches_standalone() {
    let profile = StealthProfile::chrome_default();
    let standalone = TlsFingerprint::chrome();
    assert_eq!(profile.tls.cipher_suites, standalone.cipher_suites);
    assert_eq!(profile.tls.extensions, standalone.extensions);
}

#[test]
fn test_stealth_profile_http2_matches_standalone() {
    let profile = StealthProfile::chrome_default();
    let standalone = Http2Fingerprint::chrome();
    assert_eq!(profile.http2.header_table_size, standalone.header_table_size);
    assert_eq!(profile.http2.akamai_fingerprint(), standalone.akamai_fingerprint());
}

#[test]
fn test_stealth_profile_firefox_tls_matches() {
    let profile = StealthProfile::firefox_default();
    let standalone = TlsFingerprint::firefox();
    assert_eq!(profile.tls.cipher_suites, standalone.cipher_suites);
}
