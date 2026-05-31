// @trace TEST-STL-025 [req:REQ-STL-001,REQ-STL-007] [level:unit]
// TlsFingerprint deep tests: all presets (firefox/chrome/chrome_120/chrome_latest),
// compute_ja3/ja4 format validation, alpn_strings, tls13/tls12 suite classification,
// StealthProfile cross-preset completeness, clone/debug, edge cases.

use bao_stealth::{TlsFingerprint, StealthProfile, StealthEngine};

// ---- TlsFingerprint firefox ----

#[test]
fn test_tls_firefox_cipher_suites_count() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.cipher_suites.len(), 15);
}

#[test]
fn test_tls_firefox_extensions_count() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.extensions.len(), 17);
}

#[test]
fn test_tls_firefox_signature_algorithms_count() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.signature_algorithms.len(), 10);
}

#[test]
fn test_tls_firefox_supported_groups() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.supported_groups, vec![0x001D, 0x0017, 0x0018, 0x0019, 0x0100, 0x0101]);
}

#[test]
fn test_tls_firefox_alpn() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.alpn_protocols.len(), 2);
    assert_eq!(fp.alpn_protocols[0], b"h2".to_vec());
    assert_eq!(fp.alpn_protocols[1], b"http/1.1".to_vec());
}

#[test]
fn test_tls_firefox_tls_version() {
    let fp = TlsFingerprint::firefox();
    assert_eq!(fp.tls_version, "771");
}

#[test]
fn test_tls_firefox_no_record_size_limit() {
    let fp = TlsFingerprint::firefox();
    assert!(fp.record_size_limit.is_none());
}

#[test]
fn test_tls_firefox_no_compress_certificate() {
    let fp = TlsFingerprint::firefox();
    assert!(fp.compress_certificate_algos.is_empty());
}

#[test]
fn test_tls_firefox_no_app_settings() {
    let fp = TlsFingerprint::firefox();
    assert!(fp.application_settings_protocol.is_none());
}

#[test]
fn test_tls_firefox_ja3_hash_not_empty() {
    let fp = TlsFingerprint::firefox();
    assert!(!fp.ja3_hash.is_empty());
}

// ---- TlsFingerprint chrome_120 ----

#[test]
fn test_tls_chrome_120_cipher_suites_count() {
    let fp = TlsFingerprint::chrome_120();
    assert_eq!(fp.cipher_suites.len(), 13);
}

#[test]
fn test_tls_chrome_120_supported_groups() {
    let fp = TlsFingerprint::chrome_120();
    assert_eq!(fp.supported_groups, vec![0x001D, 0x0017, 0x0018]);
}

#[test]
fn test_tls_chrome_120_no_record_size_limit() {
    let fp = TlsFingerprint::chrome_120();
    assert!(fp.record_size_limit.is_none());
}

#[test]
fn test_tls_chrome_aliases_chrome_120() {
    let ch = TlsFingerprint::chrome();
    let ch120 = TlsFingerprint::chrome_120();
    assert_eq!(ch.cipher_suites, ch120.cipher_suites);
    assert_eq!(ch.extensions, ch120.extensions);
    assert_eq!(ch.signature_algorithms, ch120.signature_algorithms);
}

// ---- TlsFingerprint chrome_latest ----

#[test]
fn test_tls_chrome_latest_has_extra_extensions() {
    let fp = TlsFingerprint::chrome_latest();
    assert!(fp.extensions.contains(&0x001C)); // extended_master_secret
    assert!(fp.extensions.contains(&0x0039)); // compress_certificate
}

#[test]
fn test_tls_chrome_latest_record_size_limit() {
    let fp = TlsFingerprint::chrome_latest();
    assert_eq!(fp.record_size_limit, Some(0x4001));
}

#[test]
fn test_tls_chrome_latest_compress_certificate() {
    let fp = TlsFingerprint::chrome_latest();
    assert_eq!(fp.compress_certificate_algos, vec![0x0002, 0x0001]);
}

#[test]
fn test_tls_chrome_latest_app_settings() {
    let fp = TlsFingerprint::chrome_latest();
    assert_eq!(fp.application_settings_protocol, Some("h2"));
}

#[test]
fn test_tls_chrome_latest_more_extensions_than_chrome_120() {
    let ch120 = TlsFingerprint::chrome_120();
    let latest = TlsFingerprint::chrome_latest();
    assert!(latest.extensions.len() > ch120.extensions.len());
}

// ---- compute_ja3 ----

#[test]
fn test_compute_ja3_firefox_format() {
    let fp = TlsFingerprint::firefox();
    let ja3 = fp.compute_ja3();
    assert!(ja3.starts_with("771,"));
    let parts: Vec<&str> = ja3.split(',').collect();
    assert_eq!(parts.len(), 5);
    // Cipher suites should be dash-separated
    assert!(parts[1].contains("-"));
}

#[test]
fn test_compute_ja3_chrome_format() {
    let fp = TlsFingerprint::chrome();
    let ja3 = fp.compute_ja3();
    assert!(ja3.starts_with("771,"));
}

#[test]
fn test_compute_ja3_deterministic() {
    let fp = TlsFingerprint::firefox();
    let j1 = fp.compute_ja3();
    let j2 = fp.compute_ja3();
    assert_eq!(j1, j2);
}

#[test]
fn test_compute_ja3_differs_between_presets() {
    let ff = TlsFingerprint::firefox().compute_ja3();
    let ch = TlsFingerprint::chrome().compute_ja3();
    assert_ne!(ff, ch);
}

#[test]
fn test_compute_ja3_latest_differs_from_chrome_120() {
    let ch120 = TlsFingerprint::chrome_120().compute_ja3();
    let latest = TlsFingerprint::chrome_latest().compute_ja3();
    assert_ne!(ch120, latest);
}

// ---- compute_ja4 ----

#[test]
fn test_compute_ja4_firefox_format() {
    let fp = TlsFingerprint::firefox();
    let ja4 = fp.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_chrome_format() {
    let fp = TlsFingerprint::chrome();
    let ja4 = fp.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_deterministic() {
    let fp = TlsFingerprint::firefox();
    let j1 = fp.compute_ja4();
    let j2 = fp.compute_ja4();
    assert_eq!(j1, j2);
}

#[test]
fn test_compute_ja4_contains_underscore() {
    let fp = TlsFingerprint::firefox();
    let ja4 = fp.compute_ja4();
    assert!(ja4.contains("_"));
}

// ---- alpn_strings ----

#[test]
fn test_alpn_strings_firefox() {
    let fp = TlsFingerprint::firefox();
    let alpns = fp.alpn_strings();
    assert_eq!(alpns, vec!["h2", "http/1.1"]);
}

#[test]
fn test_alpn_strings_chrome() {
    let fp = TlsFingerprint::chrome();
    let alpns = fp.alpn_strings();
    assert_eq!(alpns, vec!["h2", "http/1.1"]);
}

// ---- tls13/tls12 suite classification ----

#[test]
fn test_tls13_suites_firefox() {
    let fp = TlsFingerprint::firefox();
    let tls13 = fp.tls13_suites();
    assert_eq!(tls13, vec![0x1301, 0x1303, 0x1302]);
}

#[test]
fn test_tls12_suites_firefox() {
    let fp = TlsFingerprint::firefox();
    let tls12 = fp.tls12_suites();
    assert!(tls12.len() > 0);
    assert!(!tls12.iter().any(|&s| fp.is_tls13_suite(s)));
}

#[test]
fn test_tls13_plus_tls12_equals_total() {
    let fp = TlsFingerprint::firefox();
    let total = fp.cipher_suites.len();
    let tls13 = fp.tls13_suites().len();
    let tls12 = fp.tls12_suites().len();
    assert_eq!(total, tls13 + tls12);
}

#[test]
fn test_is_tls13_suite() {
    let fp = TlsFingerprint::firefox();
    assert!(fp.is_tls13_suite(0x1301));
    assert!(fp.is_tls13_suite(0x1302));
    assert!(fp.is_tls13_suite(0x1303));
    assert!(!fp.is_tls13_suite(0xC02B));
    assert!(!fp.is_tls13_suite(0x0000));
}

// ---- clone/debug ----

#[test]
fn test_tls_clone() {
    let fp = TlsFingerprint::firefox();
    let cloned = fp.clone();
    assert_eq!(fp.cipher_suites, cloned.cipher_suites);
    assert_eq!(fp.extensions, cloned.extensions);
    assert_eq!(fp.signature_algorithms, cloned.signature_algorithms);
    assert_eq!(fp.supported_groups, cloned.supported_groups);
    assert_eq!(fp.alpn_protocols, cloned.alpn_protocols);
}

#[test]
fn test_tls_debug() {
    let fp = TlsFingerprint::firefox();
    let debug = format!("{:?}", fp);
    assert!(debug.contains("TlsFingerprint") || debug.contains("cipher_suites"));
}

// ---- StealthProfile cross-preset ----

#[test]
fn test_stealth_profile_firefox_all_fields_set() {
    let p = StealthProfile::firefox_default();
    assert!(!p.tls.cipher_suites.is_empty());
    assert!(!p.http2.pseudo_header_order.is_empty());
    assert!(p.canvas.seed() == 42);
    assert!(!p.navigator.user_agent.is_empty());
    assert!(p.screen.width > 0);
    assert!(!p.webgl.vendor.is_empty());
    assert!(p.audio.seed() == 42);
}

#[test]
fn test_stealth_profile_chrome_all_fields_set() {
    let p = StealthProfile::chrome_default();
    assert!(!p.tls.cipher_suites.is_empty());
    assert!(!p.http2.pseudo_header_order.is_empty());
    assert!(p.canvas.seed() == 137);
    assert!(!p.navigator.user_agent.is_empty());
    assert!(p.screen.width > 0);
    assert!(!p.webgl.vendor.is_empty());
    assert!(p.audio.seed() == 137);
}

#[test]
fn test_stealth_profile_firefox_chrome_tls_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.tls.cipher_suites, ch.tls.cipher_suites);
}

#[test]
fn test_stealth_profile_firefox_chrome_navigator_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.navigator.user_agent, ch.navigator.user_agent);
}

#[test]
fn test_stealth_profile_firefox_chrome_webgl_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.webgl.vendor, ch.webgl.vendor);
}

#[test]
fn test_stealth_profile_firefox_chrome_canvas_seeds_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.canvas.seed(), ch.canvas.seed());
}

#[test]
fn test_stealth_profile_firefox_chrome_behavior_seeds_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.behavior.seed(), ch.behavior.seed());
}

#[test]
fn test_stealth_profile_firefox_chrome_http2_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.http2.initial_window_size, ch.http2.initial_window_size);
}

#[test]
fn test_stealth_profile_firefox_chrome_screen_same() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_eq!(ff.screen.width, ch.screen.width);
    assert_eq!(ff.screen.height, ch.screen.height);
}

#[test]
fn test_stealth_profile_clone() {
    let p = StealthProfile::firefox_default();
    let cloned = p.clone();
    assert_eq!(p.tls.cipher_suites, cloned.tls.cipher_suites);
    assert_eq!(p.navigator.user_agent, cloned.navigator.user_agent);
    assert_eq!(p.canvas.seed(), cloned.canvas.seed());
}

#[test]
fn test_stealth_profile_debug() {
    let p = StealthProfile::firefox_default();
    let debug = format!("{:?}", p);
    assert!(debug.contains("StealthProfile") || debug.contains("cipher_suites"));
}

// ---- StealthEngine accessors ----

#[test]
fn test_stealth_engine_tls_config() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let tls = engine.tls_config();
    assert!(!tls.cipher_suites.is_empty());
}

#[test]
fn test_stealth_engine_http2_config() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let h2 = engine.http2_config();
    assert!(!h2.pseudo_header_order.is_empty());
}

#[test]
fn test_stealth_engine_canvas_noise() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let cn = engine.canvas_noise();
    assert!(cn.seed() > 0);
}

#[test]
fn test_stealth_engine_navigator() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let nav = engine.navigator();
    assert!(!nav.user_agent.is_empty());
}

#[test]
fn test_stealth_engine_screen() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let scr = engine.screen();
    assert_eq!(scr.width, 1920);
}

#[test]
fn test_stealth_engine_webgl() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let gl = engine.webgl();
    assert!(!gl.vendor.is_empty());
}

#[test]
fn test_stealth_engine_audio() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let audio = engine.audio();
    assert_eq!(audio.seed(), 42);
}

#[test]
fn test_stealth_engine_behavior() {
    let engine = StealthEngine::new(StealthProfile::firefox_default());
    let behavior = engine.behavior();
    assert!(behavior.seed() > 0);
}

#[test]
fn test_stealth_engine_profile() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let profile = engine.profile();
    assert!(!profile.tls.cipher_suites.is_empty());
}

#[test]
fn test_stealth_engine_default_engine() {
    let engine = StealthEngine::default_engine();
    let nav = engine.navigator();
    assert!(!nav.user_agent.is_empty());
}
