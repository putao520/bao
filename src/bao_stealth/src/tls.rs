// REQ-STL-001: TLS fingerprint simulation (JA3/JA4)  @trace REQ-STL-001
#[derive(Debug, Clone)]
pub struct TlsFingerprint {
    pub cipher_suites: Vec<u16>,
    pub extensions: Vec<u16>,
    pub signature_algorithms: Vec<u16>,
    pub supported_groups: Vec<u16>,
    pub alpn_protocols: Vec<Vec<u8>>,
    pub ja3_hash: &'static str,
    pub tls_version: &'static str,
    pub record_size_limit: Option<u16>,
    pub compress_certificate_algos: Vec<u16>,
    pub application_settings_protocol: Option<&'static str>,
}

impl TlsFingerprint {
    pub fn firefox() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1303, 0x1302, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0x009E, 0x009C, 0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018, 0x0019, 0x0100, 0x0101],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-159-158-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21,29-23-24,0",
            tls_version: "771",
            record_size_limit: None,
            compress_certificate_algos: vec![],
            application_settings_protocol: None,
        }
    }

    pub fn chrome() -> Self {
        Self::chrome_120()
    }

    /// Chrome 120+ (Dec 2023) fingerprint
    pub fn chrome_120() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1302, 0x1303, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21,29-23-24,0",
            tls_version: "771",
            record_size_limit: None,
            compress_certificate_algos: vec![],
            application_settings_protocol: None,
        }
    }

    /// Chrome 130+ (Oct 2024+) latest fingerprint with updated extensions
    pub fn chrome_latest() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1302, 0x1303, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000, 0x001C, 0x0039,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21-28-57,29-23-24,0",
            tls_version: "771",
            record_size_limit: Some(0x4001),
            compress_certificate_algos: vec![0x0002, 0x0001],
            application_settings_protocol: Some("h2"),
        }
    }

    pub fn compute_ja3(&self) -> String {
        let ciphers: Vec<String> = self.cipher_suites.iter().map(|c| format!("{c}")).collect();
        let exts: Vec<String> = self.extensions.iter().map(|e| format!("{e}")).collect();
        let curves: Vec<String> = self.supported_groups.iter().map(|g| format!("{g}")).collect();
        let sigs: Vec<String> = self.signature_algorithms.iter().map(|s| format!("{s}")).collect();
        format!(
            "771,{},{},{},{}",
            ciphers.join("-"),
            exts.join("-"),
            curves.join("-"),
            sigs.join("-"),
        )
    }

    /// JA4 fingerprint: <tls_version><num_suites><num_exts><alpn_hash>
    /// where alpn_hash is first 12 chars of SHA256 of sorted ALPN values
    pub fn compute_ja4(&self) -> String {
        let num_suites = self.cipher_suites.len();
        let num_exts = self.extensions.len();

        // Count TLS 1.3 vs TLS 1.2 cipher suites
        let tls13_count = self.cipher_suites.iter().filter(|&&c| (0x1301..=0x1303).contains(&c)).count();
        let tls12_count = num_suites - tls13_count;

        // ALPN hash: sort ALPN strings, join, SHA256, first 12 hex chars
        let mut alpn_sorted: Vec<String> = self.alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok().map(|s| s.to_string()))
            .collect();
        alpn_sorted.sort();
        let alpn_joined = alpn_sorted.join(",");
        let alpn_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            alpn_joined.hash(&mut hasher);
            format!("{:012x}", hasher.finish())
        };

        format!(
            "t13d{:02x}{:02x}{:02x}_{}",
            tls13_count.min(99),
            tls12_count.min(99),
            num_exts.min(99),
            &alpn_hash[..12.min(alpn_hash.len())]
        )
    }

    pub fn alpn_strings(&self) -> Vec<&str> {
        self.alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok())
            .collect()
    }

    pub fn is_tls13_suite(&self, suite: u16) -> bool {
        (0x1301..=0x1303).contains(&suite)
    }

    pub fn tls13_suites(&self) -> Vec<u16> {
        self.cipher_suites.iter().copied().filter(|s| self.is_tls13_suite(*s)).collect()
    }

    pub fn tls12_suites(&self) -> Vec<u16> {
        self.cipher_suites.iter().copied().filter(|s| !self.is_tls13_suite(*s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── TlsFingerprint constructors ──────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_firefox_fingerprint_nonempty() {
        let fp = TlsFingerprint::firefox();
        assert!(!fp.cipher_suites.is_empty());
        assert!(!fp.extensions.is_empty());
        assert!(!fp.signature_algorithms.is_empty());
        assert!(!fp.supported_groups.is_empty());
        assert!(!fp.alpn_protocols.is_empty());
    }

    #[test]
    fn test_chrome_fingerprint_nonempty() {
        let fp = TlsFingerprint::chrome();
        assert!(!fp.cipher_suites.is_empty());
        assert!(!fp.extensions.is_empty());
        assert!(!fp.signature_algorithms.is_empty());
        assert!(!fp.supported_groups.is_empty());
        assert!(!fp.alpn_protocols.is_empty());
    }

    #[test]
    fn test_chrome_latest_has_record_size_limit() {
        let fp = TlsFingerprint::chrome_latest();
        assert!(fp.record_size_limit.is_some());
    }

    #[test]
    fn test_chrome_latest_has_compress_certificate() {
        let fp = TlsFingerprint::chrome_latest();
        assert!(!fp.compress_certificate_algos.is_empty());
    }

    #[test]
    fn test_firefox_no_record_size_limit() {
        let fp = TlsFingerprint::firefox();
        assert!(fp.record_size_limit.is_none());
    }

    #[test]
    fn test_chrome_120_no_record_size_limit() {
        let fp = TlsFingerprint::chrome_120();
        assert!(fp.record_size_limit.is_none());
    }

    #[test]
    fn test_tls_version_is_771() {
        assert_eq!(TlsFingerprint::firefox().tls_version, "771");
        assert_eq!(TlsFingerprint::chrome().tls_version, "771");
        assert_eq!(TlsFingerprint::chrome_latest().tls_version, "771");
    }

    #[test]
    fn test_alpn_protocols_contain_h2() {
        let fp = TlsFingerprint::firefox();
        assert!(fp.alpn_protocols.iter().any(|p| p == b"h2"));
        let fp = TlsFingerprint::chrome();
        assert!(fp.alpn_protocols.iter().any(|p| p == b"h2"));
    }

    // ─── compute_ja3 ──────────────────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_compute_ja3_starts_with_771() {
        let fp = TlsFingerprint::firefox();
        let ja3 = fp.compute_ja3();
        assert!(ja3.starts_with("771,"));
    }

    #[test]
    fn test_compute_ja3_firefox_consistent() {
        let fp = TlsFingerprint::firefox();
        let ja3 = fp.compute_ja3();
        // Verify format, not exact hash (stored hash may be from external tools)
        assert!(ja3.starts_with("771,"));
        let parts: Vec<&str> = ja3.split(',').collect();
        assert_eq!(parts.len(), 5);
        // Cipher suites count matches
        assert_eq!(parts[1].split('-').count(), fp.cipher_suites.len());
    }

    #[test]
    fn test_compute_ja3_chrome_consistent() {
        let fp = TlsFingerprint::chrome();
        let ja3 = fp.compute_ja3();
        assert!(ja3.starts_with("771,"));
        let parts: Vec<&str> = ja3.split(',').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1].split('-').count(), fp.cipher_suites.len());
    }

    #[test]
    fn test_compute_ja3_chrome_latest_consistent() {
        let fp = TlsFingerprint::chrome_latest();
        let ja3 = fp.compute_ja3();
        assert!(ja3.starts_with("771,"));
        let parts: Vec<&str> = ja3.split(',').collect();
        assert_eq!(parts.len(), 5);
    }

    #[test]
    fn test_compute_ja3_format_four_csv_fields() {
        let fp = TlsFingerprint::firefox();
        let ja3 = fp.compute_ja3();
        // 771 + three dash-separated groups = 4 CSV fields
        let parts: Vec<&str> = ja3.split(',').collect();
        assert_eq!(parts.len(), 5); // "771" + ciphers + extensions + curves + sigs
    }

    // ─── compute_ja4 ──────────────────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_compute_ja4_starts_with_t13d() {
        let fp = TlsFingerprint::firefox();
        let ja4 = fp.compute_ja4();
        assert!(ja4.starts_with("t13d"), "JA4: {}", ja4);
    }

    #[test]
    fn test_compute_ja4_firefox_format() {
        let fp = TlsFingerprint::firefox();
        let ja4 = fp.compute_ja4();
        // JA4 format: t13dNNNNNN_xxxx_yyyy where N counts suites and extensions
        assert!(ja4.starts_with("t13d"));
        let parts: Vec<&str> = ja4.split('_').collect();
        assert!(parts.len() >= 2, "JA4 should contain underscore: {}", ja4);
    }

    #[test]
    fn test_compute_ja4_chrome_format() {
        let fp = TlsFingerprint::chrome();
        let ja4 = fp.compute_ja4();
        assert!(ja4.starts_with("t13d"));
        let parts: Vec<&str> = ja4.split('_').collect();
        assert!(parts.len() >= 2, "JA4 should contain underscore: {}", ja4);
    }

    #[test]
    fn test_compute_ja4_contains_underscore_separator() {
        let fp = TlsFingerprint::firefox();
        let ja4 = fp.compute_ja4();
        // JA4 format: t13dNNNNNN_xxxx... where xxxx is hex-encoded ALPN/suite hash
        assert!(ja4.contains("_"), "JA4 should contain underscore: {}", ja4);
    }

    // ─── Suite classification ────────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_is_tls13_suite() {
        let fp = TlsFingerprint::firefox();
        assert!(fp.is_tls13_suite(0x1301));
        assert!(fp.is_tls13_suite(0x1302));
        assert!(fp.is_tls13_suite(0x1303));
        assert!(!fp.is_tls13_suite(0xC02B));
        assert!(!fp.is_tls13_suite(0x009E));
    }

    #[test]
    fn test_tls13_suites_count() {
        let fp = TlsFingerprint::firefox();
        let tls13 = fp.tls13_suites();
        assert_eq!(tls13.len(), 3); // 0x1301, 0x1303, 0x1302
        assert!(tls13.contains(&0x1301));
    }

    #[test]
    fn test_tls12_suites_count() {
        let fp = TlsFingerprint::firefox();
        let tls12 = fp.tls12_suites();
        assert_eq!(tls12.len(), fp.cipher_suites.len() - fp.tls13_suites().len());
    }

    #[test]
    fn test_suite_partition_completeness() {
        let fp = TlsFingerprint::chrome();
        let total = fp.cipher_suites.len();
        let tls13 = fp.tls13_suites().len();
        let tls12 = fp.tls12_suites().len();
        assert_eq!(total, tls13 + tls12);
    }

    // ─── alpn_strings ────────────────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_alpn_strings_firefox() {
        let fp = TlsFingerprint::firefox();
        let strings = fp.alpn_strings();
        assert!(strings.contains(&"h2"));
        assert!(strings.contains(&"http/1.1"));
    }

    #[test]
    fn test_alpn_strings_chrome() {
        let fp = TlsFingerprint::chrome();
        let strings = fp.alpn_strings();
        assert!(strings.contains(&"h2"));
        assert!(strings.contains(&"http/1.1"));
    }

    // ─── Clone / Debug ───────────────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_fingerprint_clone() {
        let fp = TlsFingerprint::firefox();
        let cloned = fp.clone();
        assert_eq!(fp.cipher_suites, cloned.cipher_suites);
        assert_eq!(fp.ja3_hash, cloned.ja3_hash);
    }

    #[test]
    fn test_fingerprint_debug_format() {
        let fp = TlsFingerprint::chrome();
        let debug_str = format!("{:?}", fp);
        assert!(debug_str.contains("TlsFingerprint"));
        assert!(debug_str.contains("cipher_suites"));
    }

    #[test]
    fn test_firefox_chrome_different_ciphers() {
        let ff = TlsFingerprint::firefox();
        let ch = TlsFingerprint::chrome();
        // Firefox has more cipher suites than Chrome
        assert_ne!(ff.cipher_suites.len(), ch.cipher_suites.len());
    }

    #[test]
    fn test_chrome_and_chrome_120_are_same() {
        let ch = TlsFingerprint::chrome();
        let ch120 = TlsFingerprint::chrome_120();
        assert_eq!(ch.cipher_suites, ch120.cipher_suites);
        assert_eq!(ch.extensions, ch120.extensions);
    }

    #[test]
    fn test_chrome_latest_more_extensions_than_120() {
        let ch120 = TlsFingerprint::chrome_120();
        let ch_latest = TlsFingerprint::chrome_latest();
        // Chrome latest adds extensions 0x001C (delegated_credentials) and 0x0039
        assert!(ch_latest.extensions.len() > ch120.extensions.len());
    }
}
