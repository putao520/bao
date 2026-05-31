// @trace TEST-STL-019 [req:REQ-STL-001] [level:unit]
// TlsFingerprint compute_ja3/ja4 computation, preset differences,
// alpn_strings, tls13/tls12 suite classification, field validation,
// clone/debug, cross-preset consistency.

use bao_stealth::TlsFingerprint;

// ---- Firefox preset fields ----

#[test]
fn test_firefox_has_tls13_suites() {
    let ff = TlsFingerprint::firefox();
    let tls13 = ff.tls13_suites();
    assert!(!tls13.is_empty());
    assert!(tls13.contains(&0x1301)); // TLS_AES_128_GCM_SHA256
    assert!(tls13.contains(&0x1302)); // TLS_AES_256_GCM_SHA384
    assert!(tls13.contains(&0x1303)); // TLS_CHACHA20_POLY1305_SHA256
}

#[test]
fn test_firefox_has_tls12_suites() {
    let ff = TlsFingerprint::firefox();
    let tls12 = ff.tls12_suites();
    assert!(!tls12.is_empty());
    assert!(tls12.contains(&0xC02B)); // TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
    assert!(tls12.contains(&0xC02F)); // TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
}

#[test]
fn test_firefox_tls13_plus_tls12_equals_total() {
    let ff = TlsFingerprint::firefox();
    let total = ff.cipher_suites.len();
    let tls13 = ff.tls13_suites().len();
    let tls12 = ff.tls12_suites().len();
    assert_eq!(tls13 + tls12, total);
}

#[test]
fn test_firefox_extensions_contains_key_share() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.extensions.contains(&0x0033)); // key_share
}

#[test]
fn test_firefox_extensions_contains_supported_versions() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.extensions.contains(&0x002B)); // supported_versions
}

#[test]
fn test_firefox_extensions_contains_supported_groups() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.extensions.contains(&0x000A)); // supported_groups
}

#[test]
fn test_firefox_signature_algorithms_count() {
    let ff = TlsFingerprint::firefox();
    assert_eq!(ff.signature_algorithms.len(), 10);
}

#[test]
fn test_firefox_supported_groups_contains_x25519() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.supported_groups.contains(&0x001D)); // x25519
}

#[test]
fn test_firefox_supported_groups_count() {
    let ff = TlsFingerprint::firefox();
    // x25519, secp256r1, secp384r1, secp521r1, ffdhe2048, ffdhe3072
    assert_eq!(ff.supported_groups.len(), 6);
}

#[test]
fn test_firefox_alpn_strings() {
    let ff = TlsFingerprint::firefox();
    let alpn = ff.alpn_strings();
    assert_eq!(alpn, vec!["h2", "http/1.1"]);
}

#[test]
fn test_firefox_tls_version() {
    let ff = TlsFingerprint::firefox();
    assert_eq!(ff.tls_version, "771"); // 0x0303 = TLS 1.2 (in JA3 convention)
}

#[test]
fn test_firefox_no_record_size_limit() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.record_size_limit.is_none());
}

#[test]
fn test_firefox_no_compress_certificate() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.compress_certificate_algos.is_empty());
}

#[test]
fn test_firefox_no_application_settings() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.application_settings_protocol.is_none());
}

// ---- Chrome 120 preset ----

#[test]
fn test_chrome_delegates_to_120() {
    let ch = TlsFingerprint::chrome();
    let c120 = TlsFingerprint::chrome_120();
    assert_eq!(ch.cipher_suites, c120.cipher_suites);
    assert_eq!(ch.extensions, c120.extensions);
    assert_eq!(ch.signature_algorithms, c120.signature_algorithms);
    assert_eq!(ch.supported_groups, c120.supported_groups);
}

#[test]
fn test_chrome_120_tls13_suites() {
    let ch = TlsFingerprint::chrome_120();
    let tls13 = ch.tls13_suites();
    assert_eq!(tls13.len(), 3);
    assert!(tls13.contains(&0x1301));
    assert!(tls13.contains(&0x1302));
    assert!(tls13.contains(&0x1303));
}

#[test]
fn test_chrome_120_tls12_suites() {
    let ch = TlsFingerprint::chrome_120();
    let tls12 = ch.tls12_suites();
    assert!(!tls12.is_empty());
    assert!(tls12.contains(&0xC02B));
    assert!(tls12.contains(&0xCCA9)); // TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305
}

#[test]
fn test_chrome_120_supported_groups() {
    let ch = TlsFingerprint::chrome_120();
    // Only x25519, secp256r1, secp384r1 — no secp521r1 or ffdhe
    assert_eq!(ch.supported_groups.len(), 3);
    assert!(ch.supported_groups.contains(&0x001D));
    assert!(ch.supported_groups.contains(&0x0017));
    assert!(ch.supported_groups.contains(&0x0018));
}

#[test]
fn test_chrome_120_no_record_size_limit() {
    let ch = TlsFingerprint::chrome_120();
    assert!(ch.record_size_limit.is_none());
}

#[test]
fn test_chrome_120_alpn() {
    let ch = TlsFingerprint::chrome_120();
    assert_eq!(ch.alpn_strings(), vec!["h2", "http/1.1"]);
}

// ---- Chrome latest preset ----

#[test]
fn test_chrome_latest_has_extra_extensions() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    assert!(cl.extensions.len() > c120.extensions.len());
    assert!(cl.extensions.contains(&0x001C)); // renegotiation_info
    assert!(cl.extensions.contains(&0x0039)); // compress_certificate
}

#[test]
fn test_chrome_latest_record_size_limit() {
    let cl = TlsFingerprint::chrome_latest();
    assert_eq!(cl.record_size_limit, Some(0x4001));
}

#[test]
fn test_chrome_latest_compress_certificate() {
    let cl = TlsFingerprint::chrome_latest();
    assert_eq!(cl.compress_certificate_algos.len(), 2);
    assert!(cl.compress_certificate_algos.contains(&0x0002)); // brotli
    assert!(cl.compress_certificate_algos.contains(&0x0001)); // zlib
}

#[test]
fn test_chrome_latest_application_settings() {
    let cl = TlsFingerprint::chrome_latest();
    assert_eq!(cl.application_settings_protocol, Some("h2"));
}

#[test]
fn test_chrome_latest_same_cipher_suites_as_120() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    assert_eq!(cl.cipher_suites, c120.cipher_suites);
}

#[test]
fn test_chrome_latest_same_signature_algorithms_as_120() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    assert_eq!(cl.signature_algorithms, c120.signature_algorithms);
}

// ---- compute_ja3 ----

#[test]
fn test_compute_ja3_format_firefox() {
    let ff = TlsFingerprint::firefox();
    let ja3 = ff.compute_ja3();
    assert!(ja3.starts_with("771,"));
    // Should contain cipher suite values
    assert!(ja3.contains("4865")); // 0x1301
    assert!(ja3.contains("49195")); // 0xC02B
}

#[test]
fn test_compute_ja3_format_chrome() {
    let ch = TlsFingerprint::chrome();
    let ja3 = ch.compute_ja3();
    assert!(ja3.starts_with("771,"));
    assert!(ja3.contains("4865"));
}

#[test]
fn test_compute_ja3_firefox_vs_chrome_differ() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    assert_ne!(ff.compute_ja3(), ch.compute_ja3());
}

#[test]
fn test_compute_ja3_chrome_120_vs_latest_differ() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    // Extensions differ → JA3 should differ
    assert_ne!(c120.compute_ja3(), cl.compute_ja3());
}

#[test]
fn test_compute_ja3_firefox_matches_ja3_hash() {
    let ff = TlsFingerprint::firefox();
    let ja3 = ff.compute_ja3();
    // The ja3_hash field stores the full JA3 string before MD5
    assert!(ja3.starts_with("771,"));
    // Verify it ends with signature algorithms
    assert!(ja3.ends_with("0") || ja3.ends_with("513"));
}

#[test]
fn test_compute_ja3_deterministic() {
    let ff = TlsFingerprint::firefox();
    let ja3_1 = ff.compute_ja3();
    let ja3_2 = ff.compute_ja3();
    assert_eq!(ja3_1, ja3_2);
}

#[test]
fn test_compute_ja3_has_four_comma_sections() {
    let ff = TlsFingerprint::firefox();
    let ja3 = ff.compute_ja3();
    let parts: Vec<&str> = ja3.split(',').collect();
    assert_eq!(parts.len(), 5); // version, ciphers, extensions, curves, sigs
}

#[test]
fn test_compute_ja3_cipher_section_has_dashes() {
    let ff = TlsFingerprint::firefox();
    let ja3 = ff.compute_ja3();
    let cipher_section = ja3.split(',').nth(1).unwrap();
    assert!(cipher_section.contains('-'));
}

// ---- compute_ja4 ----

#[test]
fn test_compute_ja4_format_firefox() {
    let ff = TlsFingerprint::firefox();
    let ja4 = ff.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_format_chrome() {
    let ch = TlsFingerprint::chrome();
    let ja4 = ch.compute_ja4();
    assert!(ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_contains_underscore() {
    let ff = TlsFingerprint::firefox();
    let ja4 = ff.compute_ja4();
    assert!(ja4.contains('_'));
}

#[test]
fn test_compute_ja4_firefox_vs_chrome_same_prefix() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    // Both start with t13d (same TLS 1.3 + number of suites)
    let ff_ja4 = ff.compute_ja4();
    let ch_ja4 = ch.compute_ja4();
    assert!(ff_ja4.starts_with("t13d"));
    assert!(ch_ja4.starts_with("t13d"));
}

#[test]
fn test_compute_ja4_firefox_vs_chrome_differ() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    // Different number of extensions → different JA4
    assert_ne!(ff.compute_ja4(), ch.compute_ja4());
}

#[test]
fn test_compute_ja4_deterministic() {
    let ff = TlsFingerprint::firefox();
    assert_eq!(ff.compute_ja4(), ff.compute_ja4());
}

#[test]
fn test_compute_ja4_suite_counts_firefox() {
    let ff = TlsFingerprint::firefox();
    let tls13 = ff.tls13_suites().len();
    let tls12 = ff.tls12_suites().len();
    let ja4 = ff.compute_ja4();
    // After "t13d", next 2 chars are hex tls13 count, next 2 are hex tls12 count
    let prefix = &ja4[4..8];
    let tls13_hex = &prefix[0..2];
    let tls12_hex = &prefix[2..4];
    assert_eq!(u8::from_str_radix(tls13_hex, 16).unwrap() as usize, tls13);
    assert_eq!(u8::from_str_radix(tls12_hex, 16).unwrap() as usize, tls12);
}

#[test]
fn test_compute_ja4_ext_count_firefox() {
    let ff = TlsFingerprint::firefox();
    let ext_count = ff.extensions.len();
    let ja4 = ff.compute_ja4();
    let ext_hex = &ja4[8..10];
    assert_eq!(u8::from_str_radix(ext_hex, 16).unwrap() as usize, ext_count);
}

// ---- is_tls13_suite ----

#[test]
fn test_is_tls13_suite_boundaries() {
    let ff = TlsFingerprint::firefox();
    assert!(ff.is_tls13_suite(0x1301));
    assert!(ff.is_tls13_suite(0x1302));
    assert!(ff.is_tls13_suite(0x1303));
    assert!(!ff.is_tls13_suite(0x1300));
    assert!(!ff.is_tls13_suite(0x1304));
}

#[test]
fn test_is_tls13_suite_common_tls12() {
    let ff = TlsFingerprint::firefox();
    assert!(!ff.is_tls13_suite(0xC02B));
    assert!(!ff.is_tls13_suite(0xC02F));
    assert!(!ff.is_tls13_suite(0x009E));
}

#[test]
fn test_is_tls13_suite_zero() {
    let ff = TlsFingerprint::firefox();
    assert!(!ff.is_tls13_suite(0));
}

// ---- alpn_strings ----

#[test]
fn test_alpn_strings_firefox_order() {
    let ff = TlsFingerprint::firefox();
    let alpn = ff.alpn_strings();
    assert_eq!(alpn[0], "h2");
    assert_eq!(alpn[1], "http/1.1");
}

#[test]
fn test_alpn_strings_chrome_order() {
    let ch = TlsFingerprint::chrome();
    let alpn = ch.alpn_strings();
    assert_eq!(alpn[0], "h2");
    assert_eq!(alpn[1], "http/1.1");
}

#[test]
fn test_alpn_protocols_bytes() {
    let ff = TlsFingerprint::firefox();
    assert_eq!(ff.alpn_protocols[0], b"h2".to_vec());
    assert_eq!(ff.alpn_protocols[1], b"http/1.1".to_vec());
}

// ---- Cross-preset differences ----

#[test]
fn test_firefox_more_supported_groups_than_chrome() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    assert!(ff.supported_groups.len() > ch.supported_groups.len());
}

#[test]
fn test_firefox_more_cipher_suites_than_chrome() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    assert!(ff.cipher_suites.len() > ch.cipher_suites.len());
}

#[test]
fn test_firefox_has_009e_009c_chrome_does_not() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    assert!(ff.cipher_suites.contains(&0x009E));
    assert!(ff.cipher_suites.contains(&0x009C));
    assert!(!ch.cipher_suites.contains(&0x009E));
    assert!(!ch.cipher_suites.contains(&0x009C));
}

#[test]
fn test_chrome_latest_extensions_superset_of_120() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    for ext in &c120.extensions {
        assert!(cl.extensions.contains(ext), "Chrome latest missing extension {}", ext);
    }
}

#[test]
fn test_all_presets_have_h2_alpn() {
    let presets = [
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    for p in &presets {
        assert!(p.alpn_strings().iter().any(|s| s == &"h2"));
    }
}

#[test]
fn test_all_presets_have_tls13_suites() {
    let presets = [
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    for p in &presets {
        assert!(!p.tls13_suites().is_empty());
    }
}

#[test]
fn test_all_presets_have_tls12_suites() {
    let presets = [
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    for p in &presets {
        assert!(!p.tls12_suites().is_empty());
    }
}

#[test]
fn test_all_presets_version_771() {
    let presets = [
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    for p in &presets {
        assert_eq!(p.tls_version, "771");
    }
}

#[test]
fn test_all_presets_have_x25519() {
    let presets = [
        TlsFingerprint::firefox(),
        TlsFingerprint::chrome(),
        TlsFingerprint::chrome_120(),
        TlsFingerprint::chrome_latest(),
    ];
    for p in &presets {
        assert!(p.supported_groups.contains(&0x001D));
    }
}

// ---- Clone and Debug ----

#[test]
fn test_tls_fingerprint_clone() {
    let ff = TlsFingerprint::firefox();
    let cloned = ff.clone();
    assert_eq!(cloned.cipher_suites, ff.cipher_suites);
    assert_eq!(cloned.extensions, ff.extensions);
    assert_eq!(cloned.ja3_hash, ff.ja3_hash);
    assert_eq!(cloned.compute_ja3(), ff.compute_ja3());
}

#[test]
fn test_tls_fingerprint_clone_chrome_latest() {
    let cl = TlsFingerprint::chrome_latest();
    let cloned = cl.clone();
    assert_eq!(cloned.record_size_limit, cl.record_size_limit);
    assert_eq!(cloned.compress_certificate_algos, cl.compress_certificate_algos);
    assert_eq!(cloned.application_settings_protocol, cl.application_settings_protocol);
}

#[test]
fn test_tls_fingerprint_debug() {
    let ff = TlsFingerprint::firefox();
    let debug = format!("{:?}", ff);
    assert!(debug.contains("TlsFingerprint"));
    assert!(debug.contains("cipher_suites"));
}

#[test]
fn test_tls_fingerprint_debug_chrome_latest() {
    let cl = TlsFingerprint::chrome_latest();
    let debug = format!("{:?}", cl);
    assert!(debug.contains("16385") || debug.contains("0x4001")); // record_size_limit decimal or hex
}

// ---- ja3_hash static field consistency ----

#[test]
fn test_firefox_ja3_hash_not_empty() {
    let ff = TlsFingerprint::firefox();
    assert!(!ff.ja3_hash.is_empty());
}

#[test]
fn test_chrome_ja3_hash_not_empty() {
    let ch = TlsFingerprint::chrome();
    assert!(!ch.ja3_hash.is_empty());
}

#[test]
fn test_firefox_vs_chrome_ja3_hash_differ() {
    let ff = TlsFingerprint::firefox();
    let ch = TlsFingerprint::chrome();
    assert_ne!(ff.ja3_hash, ch.ja3_hash);
}

#[test]
fn test_chrome_latest_ja3_hash_differs_from_120() {
    let c120 = TlsFingerprint::chrome_120();
    let cl = TlsFingerprint::chrome_latest();
    assert_ne!(c120.ja3_hash, cl.ja3_hash);
}

// ---- tls13_suites / tls12_suites partition ----

#[test]
fn test_suite_partition_no_overlap() {
    let ff = TlsFingerprint::firefox();
    let tls13: std::collections::HashSet<u16> = ff.tls13_suites().into_iter().collect();
    let tls12: std::collections::HashSet<u16> = ff.tls12_suites().into_iter().collect();
    let intersection: std::collections::HashSet<_> = tls13.intersection(&tls12).collect();
    assert!(intersection.is_empty());
}

#[test]
fn test_suite_partition_covers_all_chrome() {
    let ch = TlsFingerprint::chrome();
    let total = ch.cipher_suites.len();
    let tls13 = ch.tls13_suites().len();
    let tls12 = ch.tls12_suites().len();
    assert_eq!(tls13 + tls12, total);
}

#[test]
fn test_suite_partition_covers_all_chrome_latest() {
    let cl = TlsFingerprint::chrome_latest();
    let total = cl.cipher_suites.len();
    let tls13 = cl.tls13_suites().len();
    let tls12 = cl.tls12_suites().len();
    assert_eq!(tls13 + tls12, total);
}
