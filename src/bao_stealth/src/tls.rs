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
            ja3_hash: "771,4865-4867-4866-49195-49199-49196-49200-158-156-52393-52392-49171-49161-51-103,0-5-10-11-13-18-21-22-23-27-35-43-45-51-65037-16-0,29-23-24-25-256-257,1027-2052-1025-1283-2053-1281-2054-1537-515-513",
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
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49161-51-103,0-5-10-11-13-18-21-22-23-27-35-43-45-51-65037-16-0,29-23-24,1027-2052-1025-1283-2053-1281-2054-1537-515-513",
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
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49161-51-103,0-5-10-11-13-18-21-22-23-27-35-43-45-51-65037-16-0-28-57,29-23-24,1027-2052-1025-1283-2053-1281-2054-1537-515-513",
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
        use sha2::{Sha256, Digest};

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
            let mut hasher = Sha256::new();
            hasher.update(alpn_joined.as_bytes());
            let result = hasher.finalize();
            format!("{:x}", result)
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

    /// Convert TLS 1.2 cipher suite IDs to BoringSSL OpenSSL name string
    /// (colon-separated, e.g. "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256")
    pub fn tls12_cipher_list_string(&self) -> String {
        self.tls12_suites()
            .iter()
            .filter_map(|&id| cipher_suite_openssl_name(id))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Convert TLS 1.3 cipher suite IDs to BoringSSL name string
    /// (colon-separated, e.g. "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384")
    pub fn tls13_cipher_suites_string(&self) -> String {
        self.tls13_suites()
            .iter()
            .filter_map(|&id| cipher_suite_openssl_name(id))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Convert supported group IDs to BoringSSL curves list string
    /// (colon-separated, e.g. "X25519:P-256:P-384")
    pub fn curves_list_string(&self) -> String {
        self.supported_groups
            .iter()
            .filter_map(|&id| group_openssl_name(id))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Convert signature algorithm IDs to BoringSSL sigalgs list string
    /// (colon-separated, e.g. "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256")
    pub fn sigalgs_list_string(&self) -> String {
        self.signature_algorithms
            .iter()
            .filter_map(|&id| sigalg_openssl_name(id))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// Map IANA cipher suite ID to BoringSSL OpenSSL name.
/// Covers TLS 1.3 + common TLS 1.2 suites used in browser fingerprints.
fn cipher_suite_openssl_name(id: u16) -> Option<&'static str> {
    match id {
        // TLS 1.3
        0x1301 => Some("TLS_AES_128_GCM_SHA256"),
        0x1302 => Some("TLS_AES_256_GCM_SHA384"),
        0x1303 => Some("TLS_CHACHA20_POLY1305_SHA256"),
        // TLS 1.2 ECDHE
        0xC02B => Some("ECDHE-ECDSA-AES128-GCM-SHA256"),
        0xC02F => Some("ECDHE-RSA-AES128-GCM-SHA256"),
        0xC02C => Some("ECDHE-ECDSA-AES256-GCM-SHA384"),
        0xC030 => Some("ECDHE-RSA-AES256-GCM-SHA384"),
        // TLS 1.2 DHE
        0x009E => Some("DHE-RSA-AES128-GCM-SHA256"),
        0x009C => Some("DHE-RSA-AES256-GCM-SHA384"),
        // TLS 1.2 ECDHE CBC
        0xCCA9 => Some("ECDHE-ECDSA-CHACHA20-POLY1305"),
        0xCCA8 => Some("ECDHE-RSA-CHACHA20-POLY1305"),
        // TLS 1.2 legacy CBC
        0xC013 => Some("ECDHE-RSA-AES128-SHA"),
        0xC009 => Some("ECDHE-ECDSA-AES128-SHA"),
        0x0033 => Some("DHE-RSA-AES128-SHA"),
        0x0067 => Some("DHE-RSA-AES256-SHA256"),
        _ => None,
    }
}

/// Map IANA supported group ID to BoringSSL group name.
fn group_openssl_name(id: u16) -> Option<&'static str> {
    match id {
        0x001D => Some("X25519"),
        0x0017 => Some("P-256"),
        0x0018 => Some("P-384"),
        0x0019 => Some("P-521"),
        0x0100 => Some("ffdhe2048"),
        0x0101 => Some("ffdhe3072"),
        _ => None,
    }
}

/// Map IANA signature algorithm ID to BoringSSL sigalg name.
fn sigalg_openssl_name(id: u16) -> Option<&'static str> {
    match id {
        0x0403 => Some("ecdsa_secp256r1_sha256"),
        0x0503 => Some("ecdsa_secp384r1_sha384"),
        0x0603 => Some("ecdsa_secp521r1_sha512"),
        0x0804 => Some("rsa_pss_rsae_sha256"),
        0x0805 => Some("rsa_pss_rsae_sha384"),
        0x0806 => Some("rsa_pss_rsae_sha512"),
        0x0401 => Some("rsa_pkcs1_sha256"),
        0x0501 => Some("rsa_pkcs1_sha384"),
        0x0601 => Some("rsa_pkcs1_sha512"),
        0x0203 => Some("ecdsa_sha1"),
        0x0201 => Some("rsa_pkcs1_sha1"),
        _ => None,
    }
}

/// Pre-computed BoringSSL configuration strings derived from a [`TlsFingerprint`].
///
/// This is an intermediate representation that bridges `bao_stealth::TlsFingerprint`
/// (IANA u16 IDs) to `bun_http::ssl_config::SSLConfig` (C string pointers for
/// BoringSSL API calls). Created once per profile, then used to populate
/// `SSLConfig` fields before TLS handshake.
///
/// Usage:
/// ```ignore
/// let config = TlsFingerprintConfig::from_fingerprint(&stealth_profile.tls);
/// // Then in bao_runtime, write config.tls12_cipher_list into SSLConfig
/// ```
#[derive(Debug, Clone)]
pub struct TlsFingerprintConfig {
    /// TLS 1.2 cipher list in OpenSSL format (colon-separated).
    /// e.g. "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256"
    pub tls12_cipher_list: String,
    /// TLS 1.3 cipher suites (colon-separated).
    /// e.g. "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256"
    pub tls13_cipher_suites: String,
    /// Supported groups/curves (colon-separated).
    /// e.g. "X25519:P-256:P-384"
    pub curves_list: String,
    /// Signature algorithms (colon-separated).
    /// e.g. "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256"
    pub sigalgs_list: String,
}

impl TlsFingerprintConfig {
    /// Build from a [`TlsFingerprint`] by converting IANA u16 IDs to BoringSSL
    /// OpenSSL name strings.
    pub fn from_fingerprint(fp: &TlsFingerprint) -> Self {
        TlsFingerprintConfig {
            tls12_cipher_list: fp.tls12_cipher_list_string(),
            tls13_cipher_suites: fp.tls13_cipher_suites_string(),
            curves_list: fp.curves_list_string(),
            sigalgs_list: fp.sigalgs_list_string(),
        }
    }

    /// Whether any TLS fingerprint fields are non-empty.
    pub fn has_fingerprint(&self) -> bool {
        !self.tls12_cipher_list.is_empty()
            || !self.tls13_cipher_suites.is_empty()
            || !self.curves_list.is_empty()
            || !self.sigalgs_list.is_empty()
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

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    // compute_ja3() must produce output matching stored ja3_hash
    #[test]
    fn test_compute_ja3_matches_stored_hash_firefox() {
        let fp = TlsFingerprint::firefox();
        let computed = fp.compute_ja3();
        assert_eq!(computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Firefox — computed: {}", computed);
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    #[test]
    fn test_compute_ja3_matches_stored_hash_chrome() {
        let fp = TlsFingerprint::chrome();
        let computed = fp.compute_ja3();
        assert_eq!(computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Chrome — computed: {}", computed);
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    #[test]
    fn test_compute_ja3_matches_stored_hash_chrome_latest() {
        let fp = TlsFingerprint::chrome_latest();
        let computed = fp.compute_ja3();
        assert_eq!(computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Chrome latest — computed: {}", computed);
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

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    // JA4 must be deterministic — same fingerprint produces same JA4
    #[test]
    fn test_compute_ja4_deterministic() {
        let fp = TlsFingerprint::firefox();
        let ja4_a = fp.compute_ja4();
        let ja4_b = fp.compute_ja4();
        assert_eq!(ja4_a, ja4_b, "JA4 must be deterministic");
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    // JA4 alpn_hash portion must be valid hex (SHA256 output)
    #[test]
    fn test_compute_ja4_alpn_hash_is_hex() {
        let fp = TlsFingerprint::firefox();
        let ja4 = fp.compute_ja4();
        let parts: Vec<&str> = ja4.split('_').collect();
        assert!(parts.len() >= 2, "JA4 should have underscore separator: {}", ja4);
        let hash_part = parts.last().unwrap();
        assert!(hash_part.len() >= 12, "ALPN hash part should be at least 12 chars: {}", hash_part);
        assert!(hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "ALPN hash must be valid hex: {}", hash_part);
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    // JA4 must differ between Firefox and Chrome profiles
    #[test]
    fn test_compute_ja4_differs_between_profiles() {
        let ff = TlsFingerprint::firefox();
        let ch = TlsFingerprint::chrome();
        assert_ne!(ff.compute_ja4(), ch.compute_ja4(),
            "Firefox and Chrome must produce different JA4 fingerprints");
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

    // ─── BoringSSL string conversion ──────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_tls12_cipher_list_firefox_nonempty() {
        let fp = TlsFingerprint::firefox();
        let list = fp.tls12_cipher_list_string();
        assert!(!list.is_empty(), "TLS 1.2 cipher list should not be empty");
        assert!(list.contains("ECDHE"), "Should contain ECDHE suites");
    }

    #[test]
    fn test_tls12_cipher_list_chrome_nonempty() {
        let fp = TlsFingerprint::chrome();
        let list = fp.tls12_cipher_list_string();
        assert!(!list.is_empty());
    }

    #[test]
    fn test_tls13_cipher_suites_firefox() {
        let fp = TlsFingerprint::firefox();
        let list = fp.tls13_cipher_suites_string();
        assert!(list.contains("TLS_AES_128_GCM_SHA256"), "Should contain TLS 1.3 AES-128");
        assert!(list.contains("TLS_AES_256_GCM_SHA384"), "Should contain TLS 1.3 AES-256");
        assert!(list.contains("TLS_CHACHA20_POLY1305_SHA256"), "Should contain TLS 1.3 ChaCha20");
    }

    #[test]
    fn test_tls13_cipher_suites_chrome() {
        let fp = TlsFingerprint::chrome();
        let list = fp.tls13_cipher_suites_string();
        assert!(list.contains("TLS_AES_128_GCM_SHA256"));
    }

    #[test]
    fn test_curves_list_firefox() {
        let fp = TlsFingerprint::firefox();
        let list = fp.curves_list_string();
        assert!(list.contains("X25519"), "Should contain X25519");
        assert!(list.contains("P-256"), "Should contain P-256");
    }

    #[test]
    fn test_curves_list_chrome() {
        let fp = TlsFingerprint::chrome();
        let list = fp.curves_list_string();
        assert!(list.contains("X25519"));
        assert!(list.contains("P-256"));
    }

    #[test]
    fn test_sigalgs_list_firefox() {
        let fp = TlsFingerprint::firefox();
        let list = fp.sigalgs_list_string();
        assert!(list.contains("ecdsa_secp256r1_sha256"), "Should contain ECDSA P-256");
        assert!(list.contains("rsa_pss_rsae_sha256"), "Should contain RSA-PSS");
    }

    #[test]
    fn test_sigalgs_list_chrome() {
        let fp = TlsFingerprint::chrome();
        let list = fp.sigalgs_list_string();
        assert!(list.contains("ecdsa_secp256r1_sha256"));
    }

    #[test]
    fn test_tls12_cipher_list_colon_separated() {
        let fp = TlsFingerprint::firefox();
        let list = fp.tls12_cipher_list_string();
        // Should be colon-separated with no leading/trailing colons
        assert!(!list.starts_with(':'), "No leading colon");
        assert!(!list.ends_with(':'), "No trailing colon");
        assert!(!list.contains("::"), "No double colons");
    }

    #[test]
    fn test_curves_list_colon_separated() {
        let fp = TlsFingerprint::firefox();
        let list = fp.curves_list_string();
        assert!(!list.starts_with(':'));
        assert!(!list.ends_with(':'));
    }

    #[test]
    fn test_sigalgs_list_colon_separated() {
        let fp = TlsFingerprint::firefox();
        let list = fp.sigalgs_list_string();
        assert!(!list.starts_with(':'));
        assert!(!list.ends_with(':'));
    }

    #[test]
    fn test_firefox_chrome_different_tls12_cipher_lists() {
        let ff = TlsFingerprint::firefox();
        let ch = TlsFingerprint::chrome();
        // Firefox has more TLS 1.2 suites than Chrome
        assert_ne!(ff.tls12_cipher_list_string(), ch.tls12_cipher_list_string());
    }

    #[test]
    fn test_cipher_suite_openssl_name_known_ids() {
        assert_eq!(cipher_suite_openssl_name(0x1301), Some("TLS_AES_128_GCM_SHA256"));
        assert_eq!(cipher_suite_openssl_name(0xC02B), Some("ECDHE-ECDSA-AES128-GCM-SHA256"));
        assert_eq!(cipher_suite_openssl_name(0xFFFF), None);
    }

    #[test]
    fn test_group_openssl_name_known_ids() {
        assert_eq!(group_openssl_name(0x001D), Some("X25519"));
        assert_eq!(group_openssl_name(0x0017), Some("P-256"));
        assert_eq!(group_openssl_name(0xFFFF), None);
    }

    #[test]
    fn test_sigalg_openssl_name_known_ids() {
        assert_eq!(sigalg_openssl_name(0x0403), Some("ecdsa_secp256r1_sha256"));
        assert_eq!(sigalg_openssl_name(0x0804), Some("rsa_pss_rsae_sha256"));
        assert_eq!(sigalg_openssl_name(0xFFFF), None);
    }

    // ─── TlsFingerprintConfig ───────────────────────────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_fingerprint_config_from_firefox() {
        let fp = TlsFingerprint::firefox();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        assert!(!config.tls12_cipher_list.is_empty());
        assert!(!config.tls13_cipher_suites.is_empty());
        assert!(!config.curves_list.is_empty());
        assert!(!config.sigalgs_list.is_empty());
    }

    #[test]
    fn test_fingerprint_config_from_chrome() {
        let fp = TlsFingerprint::chrome();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        assert!(!config.tls12_cipher_list.is_empty());
        assert!(!config.tls13_cipher_suites.is_empty());
        assert!(!config.curves_list.is_empty());
        assert!(!config.sigalgs_list.is_empty());
    }

    #[test]
    fn test_fingerprint_config_from_chrome_latest() {
        let fp = TlsFingerprint::chrome_latest();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        assert!(config.has_fingerprint());
    }

    #[test]
    fn test_fingerprint_config_has_fingerprint_true() {
        let fp = TlsFingerprint::firefox();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        assert!(config.has_fingerprint());
    }

    #[test]
    fn test_fingerprint_config_has_fingerprint_false() {
        let config = TlsFingerprintConfig {
            tls12_cipher_list: String::new(),
            tls13_cipher_suites: String::new(),
            curves_list: String::new(),
            sigalgs_list: String::new(),
        };
        assert!(!config.has_fingerprint());
    }

    #[test]
    fn test_fingerprint_config_firefox_chrome_different() {
        let ff_config = TlsFingerprintConfig::from_fingerprint(&TlsFingerprint::firefox());
        let ch_config = TlsFingerprintConfig::from_fingerprint(&TlsFingerprint::chrome());
        assert_ne!(ff_config.tls12_cipher_list, ch_config.tls12_cipher_list);
    }

    #[test]
    fn test_fingerprint_config_clone() {
        let fp = TlsFingerprint::firefox();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        let cloned = config.clone();
        assert_eq!(config.tls12_cipher_list, cloned.tls12_cipher_list);
        assert_eq!(config.tls13_cipher_suites, cloned.tls13_cipher_suites);
        assert_eq!(config.curves_list, cloned.curves_list);
        assert_eq!(config.sigalgs_list, cloned.sigalgs_list);
    }

    #[test]
    fn test_fingerprint_config_debug() {
        let fp = TlsFingerprint::firefox();
        let config = TlsFingerprintConfig::from_fingerprint(&fp);
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("TlsFingerprintConfig"));
        assert!(debug_str.contains("tls12_cipher_list"));
    }
}
