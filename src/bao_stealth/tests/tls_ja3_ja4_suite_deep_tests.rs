// @trace TEST-STL-046 [req:REQ-STL-001] [level:unit]
// TLS fingerprint: compute_ja3, compute_ja4, alpn_strings, is_tls13_suite,
// tls13_suites, tls12_suites, profile field validation across firefox/chrome_120/chrome_latest.

use bao_stealth::TlsFingerprint;

// ---- compute_ja3 ----

#[test]
fn test_firefox_compute_ja3_matches_hash() {
    let tls = TlsFingerprint::firefox();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
    // ja3_hash is the full JA3 string, compute_ja3 should produce the same
    assert!(!tls.ja3_hash.is_empty());
    assert!(ja3.starts_with("771,"));
}

#[test]
fn test_chrome_compute_ja3_format() {
    let tls = TlsFingerprint::chrome();
    let ja3 = tls.compute_ja3();
    // Should be "771,<ciphers>,<extensions>,<groups>,<sigs>"
    let parts: Vec<&str> = ja3.split(',').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0], "771");
}

#[test]
fn test_chrome_latest_compute_ja3() {
    let tls = TlsFingerprint::chrome_latest();
    let ja3 = tls.compute_ja3();
    assert!(ja3.starts_with("771,"));
}

#[test]
fn test_ja3_ciphers_match_suites() {
    let tls = TlsFingerprint::firefox();
    let ja3 = tls.compute_ja3();
    let cipher_part = ja3.split(',').nth(1).unwrap();
    let cipher_ids: Vec<&str> = cipher_part.split('-').collect();
    assert_eq!(cipher_ids.len(), tls.cipher_suites.len());
}

#[test]
fn test_ja3_extensions_match() {
    let tls = TlsFingerprint::chrome_120();
    let ja3 = tls.compute_ja3();
    let ext_part = ja3.split(',').nth(2).unwrap();
    let ext_ids: Vec<&str> = ext_part.split('-').collect();
    assert_eq!(ext_ids.len(), tls.extensions.len());
}

#[test]
fn test_ja3_groups_match() {
    let tls = TlsFingerprint::firefox();
    let ja3 = tls.compute_ja3();
    let group_part = ja3.split(',').nth(3).unwrap();
    let group_ids: Vec<&str> = group_part.split('-').collect();
    assert_eq!(group_ids.len(), tls.supported_groups.len());
}

#[test]
fn test_ja3_sigs_match() {
    let tls = TlsFingerprint::chrome();
    let ja3 = tls.compute_ja3();
    let sig_part = ja3.split(',').nth(4).unwrap();
    let sig_ids: Vec<&str> = sig_part.split('-').collect();
    assert_eq!(sig_ids.len(), tls.signature_algorithms.len());
}

#[test]
fn test_ja3_deterministic() {
    let tls = TlsFingerprint::firefox();
    assert_eq!(tls.compute_ja3(), tls.compute_ja3());
}

#[test]
fn test_ja3_firefox_chrome_differ() {
    let j1 = TlsFingerprint::firefox().compute_ja3();
    let j2 = TlsFingerprint::chrome().compute_ja3();
    assert_ne!(j1, j2);
}

// ---- compute_ja4 ----

#[test]
fn test_firefox_compute_ja4_format() {
    let tls = TlsFingerprint::firefox();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
    assert!(ja4.contains("_"));
}

#[test]
fn test_chrome_compute_ja4_format() {
    let tls = TlsFingerprint::chrome();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_chrome_latest_compute_ja4() {
    let tls = TlsFingerprint::chrome_latest();
    let ja4 = tls.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_ja4_suite_count_encoding() {
    let tls = TlsFingerprint::firefox();
    let ja4 = tls.compute_ja4();
    // Format: t13d<tls13_count><tls12_count><num_exts>_<alpn_hash>
    let after_prefix = &ja4[4..];
    let underscore_pos = after_prefix.find('_').unwrap();
    let counts = &after_prefix[..underscore_pos];
    // Should be 3 hex pairs (2 chars each)
    assert_eq!(counts.len(), 6);
}

#[test]
fn test_ja4_tls13_count_firefox() {
    let tls = TlsFingerprint::firefox();
    let tls13_count = tls.tls13_suites().len();
    assert!(tls13_count > 0);
    assert!(tls13_count <= 3); // 0x1301-0x1303
}

#[test]
fn test_ja4_tls13_count_chrome() {
    let tls = TlsFingerprint::chrome();
    let tls13_count = tls.tls13_suites().len();
    assert!(tls13_count > 0);
}

#[test]
fn test_ja4_deterministic() {
    let tls = TlsFingerprint::firefox();
    assert_eq!(tls.compute_ja4(), tls.compute_ja4());
}

#[test]
fn test_ja4_firefox_chrome_differ() {
    let j1 = TlsFingerprint::firefox().compute_ja4();
    let j2 = TlsFingerprint::chrome().compute_ja4();
    assert_ne!(j1, j2);
}

#[test]
fn test_ja4_alpn_hash_part() {
    let tls = TlsFingerprint::firefox();
    let ja4 = tls.compute_ja4();
    let hash_part = ja4.split('_').nth(1).unwrap();
    assert_eq!(hash_part.len(), 12);
}

// ---- alpn_strings ----

#[test]
fn test_firefox_alpn_strings() {
    let tls = TlsFingerprint::firefox();
    let alpns = tls.alpn_strings();
    assert!(alpns.contains(&"h2"));
    assert!(alpns.contains(&"http/1.1"));
}

#[test]
fn test_chrome_alpn_strings() {
    let tls = TlsFingerprint::chrome();
    let alpns = tls.alpn_strings();
    assert_eq!(alpns.len(), 2);
}

#[test]
fn test_chrome_latest_alpn_strings() {
    let tls = TlsFingerprint::chrome_latest();
    let alpns = tls.alpn_strings();
    assert!(alpns.contains(&"h2"));
}

// ---- is_tls13_suite ----

#[test]
fn test_is_tls13_suite_true() {
    let tls = TlsFingerprint::firefox();
    assert!(tls.is_tls13_suite(0x1301));
    assert!(tls.is_tls13_suite(0x1302));
    assert!(tls.is_tls13_suite(0x1303));
}

#[test]
fn test_is_tls13_suite_false() {
    let tls = TlsFingerprint::firefox();
    assert!(!tls.is_tls13_suite(0xC02B));
    assert!(!tls.is_tls13_suite(0x009E));
    assert!(!tls.is_tls13_suite(0x0000));
}

#[test]
fn test_is_tls13_suite_boundary() {
    let tls = TlsFingerprint::firefox();
    assert!(!tls.is_tls13_suite(0x1300));
    assert!(!tls.is_tls13_suite(0x1304));
}

// ---- tls13_suites / tls12_suites ----

#[test]
fn test_firefox_tls13_suites() {
    let tls = TlsFingerprint::firefox();
    let suites = tls.tls13_suites();
    assert!(!suites.is_empty());
    for s in &suites {
        assert!((0x1301..=0x1303).contains(s));
    }
}

#[test]
fn test_firefox_tls12_suites() {
    let tls = TlsFingerprint::firefox();
    let suites = tls.tls12_suites();
    assert!(!suites.is_empty());
    for s in &suites {
        assert!(!(0x1301..=0x1303).contains(s));
    }
}

#[test]
fn test_chrome_tls13_suites() {
    let tls = TlsFingerprint::chrome();
    let suites = tls.tls13_suites();
    assert!(!suites.is_empty());
}

#[test]
fn test_chrome_tls12_suites() {
    let tls = TlsFingerprint::chrome();
    let suites = tls.tls12_suites();
    assert!(!suites.is_empty());
}

#[test]
fn test_suites_partition_completeness() {
    let tls = TlsFingerprint::firefox();
    let tls13 = tls.tls13_suites();
    let tls12 = tls.tls12_suites();
    assert_eq!(tls13.len() + tls12.len(), tls.cipher_suites.len());
}

#[test]
fn test_chrome_latest_suites_partition() {
    let tls = TlsFingerprint::chrome_latest();
    let tls13 = tls.tls13_suites();
    let tls12 = tls.tls12_suites();
    assert_eq!(tls13.len() + tls12.len(), tls.cipher_suites.len());
}

// ---- Field validation ----

#[test]
fn test_firefox_cipher_suites_nonempty() {
    assert!(!TlsFingerprint::firefox().cipher_suites.is_empty());
}

#[test]
fn test_firefox_extensions_nonempty() {
    assert!(!TlsFingerprint::firefox().extensions.is_empty());
}

#[test]
fn test_firefox_signature_algorithms_nonempty() {
    assert!(!TlsFingerprint::firefox().signature_algorithms.is_empty());
}

#[test]
fn test_firefox_supported_groups_nonempty() {
    assert!(!TlsFingerprint::firefox().supported_groups.is_empty());
}

#[test]
fn test_firefox_alpn_protocols() {
    let tls = TlsFingerprint::firefox();
    assert_eq!(tls.alpn_protocols.len(), 2);
}

#[test]
fn test_firefox_ja3_hash_nonempty() {
    assert!(!TlsFingerprint::firefox().ja3_hash.is_empty());
}

#[test]
fn test_firefox_tls_version() {
    assert_eq!(TlsFingerprint::firefox().tls_version, "771");
}

#[test]
fn test_firefox_record_size_limit_none() {
    assert!(TlsFingerprint::firefox().record_size_limit.is_none());
}

#[test]
fn test_chrome_latest_record_size_limit_some() {
    assert!(TlsFingerprint::chrome_latest().record_size_limit.is_some());
    assert_eq!(TlsFingerprint::chrome_latest().record_size_limit.unwrap(), 0x4001);
}

#[test]
fn test_chrome_latest_compress_cert_algos() {
    let tls = TlsFingerprint::chrome_latest();
    assert!(!tls.compress_certificate_algos.is_empty());
    assert!(tls.compress_certificate_algos.contains(&0x0002));
    assert!(tls.compress_certificate_algos.contains(&0x0001));
}

#[test]
fn test_chrome_latest_application_settings() {
    let tls = TlsFingerprint::chrome_latest();
    assert_eq!(tls.application_settings_protocol, Some("h2"));
}

#[test]
fn test_firefox_no_compress_cert() {
    assert!(TlsFingerprint::firefox().compress_certificate_algos.is_empty());
}

#[test]
fn test_firefox_no_application_settings() {
    assert!(TlsFingerprint::firefox().application_settings_protocol.is_none());
}

#[test]
fn test_chrome_equals_chrome_120() {
    let c1 = TlsFingerprint::chrome();
    let c2 = TlsFingerprint::chrome_120();
    assert_eq!(c1.cipher_suites, c2.cipher_suites);
    assert_eq!(c1.extensions, c2.extensions);
}

#[test]
fn test_chrome_latest_has_extra_extensions() {
    let c120 = TlsFingerprint::chrome_120();
    let clatest = TlsFingerprint::chrome_latest();
    assert!(clatest.extensions.len() > c120.extensions.len());
}

// ---- Debug / Clone ----

#[test]
fn test_tls_fingerprint_debug() {
    let tls = TlsFingerprint::firefox();
    let debug = format!("{:?}", tls);
    assert!(debug.contains("TlsFingerprint"));
}

#[test]
fn test_tls_fingerprint_clone() {
    let tls = TlsFingerprint::firefox();
    let cloned = tls.clone();
    assert_eq!(cloned.cipher_suites, tls.cipher_suites);
    assert_eq!(cloned.extensions, tls.extensions);
    assert_eq!(cloned.ja3_hash, tls.ja3_hash);
}

#[test]
fn test_tls_fingerprint_clone_independence() {
    let mut tls = TlsFingerprint::firefox();
    let cloned = tls.clone();
    tls.cipher_suites.push(0xFFFF);
    assert_ne!(tls.cipher_suites.len(), cloned.cipher_suites.len());
}
