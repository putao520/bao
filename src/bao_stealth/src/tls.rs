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
        let curves: Vec<String> = self
            .supported_groups
            .iter()
            .map(|g| format!("{g}"))
            .collect();
        let sigs: Vec<String> = self
            .signature_algorithms
            .iter()
            .map(|s| format!("{s}"))
            .collect();
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
        use bun_sha_hmac::SHA256;

        let num_suites = self.cipher_suites.len();
        let num_exts = self.extensions.len();

        // Count TLS 1.3 vs TLS 1.2 cipher suites
        let tls13_count = self
            .cipher_suites
            .iter()
            .filter(|&&c| (0x1301..=0x1303).contains(&c))
            .count();
        let tls12_count = num_suites - tls13_count;

        // ALPN hash: sort ALPN strings, join, SHA256, first 12 hex chars
        let mut alpn_sorted: Vec<String> = self
            .alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok().map(|s| s.to_string()))
            .collect();
        alpn_sorted.sort();
        let alpn_joined = alpn_sorted.join(",");
        let alpn_hash = {
            let mut hasher = SHA256::init();
            hasher.update(alpn_joined.as_bytes());
            let mut out = [0u8; SHA256::DIGEST];
            hasher.r#final(&mut out);
            out.iter().map(|b| format!("{:02x}", b)).collect::<String>()
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
        self.cipher_suites
            .iter()
            .copied()
            .filter(|s| self.is_tls13_suite(*s))
            .collect()
    }

    pub fn tls12_suites(&self) -> Vec<u16> {
        self.cipher_suites
            .iter()
            .copied()
            .filter(|s| !self.is_tls13_suite(*s))
            .collect()
    }

    /// Convert TLS 1.2 cipher suite IDs to BoringSSL OpenSSL name string
    /// (colon-separated, e.g. "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256")
    ///
    /// DHE suites are dropped: BoringSSL has no DHE_RSA implementation, so
    /// they can never be offered (see [`cipher_suite_boringssl_supported`]).
    pub fn tls12_cipher_list_string(&self) -> String {
        boringssl_cipher_list_string(&self.tls12_suites())
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
    ///
    /// FFDHE groups are dropped: `SSL_(CTX_)set1_groups_list` fails the
    /// WHOLE call on any unrecognized name and BoringSSL implements no FFDHE
    /// group — an unfiltered Firefox list would silently discard the entire
    /// groups fingerprint (see [`group_boringssl_supported`]).
    pub fn curves_list_string(&self) -> String {
        boringssl_curves_list_string(&self.supported_groups)
    }

    /// Convert signature algorithm IDs to BoringSSL sigalgs list string
    /// (colon-separated, e.g. "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256")
    pub fn sigalgs_list_string(&self) -> String {
        boringssl_sigalgs_list_string(&self.signature_algorithms)
    }
}

// ── IANA → OpenSSL/BoringSSL name mapping — SINGLE SOURCE OF TRUTH ────
//
// Every TLS stack in Bao (servo `net::connector`, `bao_runtime`/`bun_http`
// via [`TlsFingerprintConfig`]) MUST resolve IANA cipher/group/sigalg IDs
// through these functions. Local copies of this table are forbidden — the
// servo connector copy diverged to the point where nearly every entry was
// wrong (0x009E mapped to ECDHE-RSA-AES256-GCM-SHA384, 0xC02F/0xC030 had
// RSA/ECDSA swapped, RSA suites 0x002F/0x0035 mapped to ECDHE names).
//
// Every entry is cross-verified against BOTH:
//  - the IANA TLS cipher suite registry (decimal IDs appear in the JA3
//    hash strings of each profile, e.g. 158 = 0x009E),
//  - the BoringSSL actually compiled into Bao: `TLS1_TXT_*` OpenSSL-name
//    constants in `vendor/boringssl/include/openssl/tls1.h` and the
//    `kCiphers` / `kNamedGroups` / `kSignatureAlgorithmNames` tables.

/// Map IANA cipher suite ID to its OpenSSL cipher name (the
/// `TLS1_TXT_*`-style short name BoringSSL's cipher-list parser accepts).
///
/// This is the complete IANA truth table — including suites BoringSSL
/// cannot offer (see [`cipher_suite_boringssl_supported`]).
pub fn cipher_suite_openssl_name(id: u16) -> Option<&'static str> {
    match id {
        // TLS 1.3 (RFC 8446)
        0x1301 => Some("TLS_AES_128_GCM_SHA256"),
        0x1302 => Some("TLS_AES_256_GCM_SHA384"),
        0x1303 => Some("TLS_CHACHA20_POLY1305_SHA256"),
        // TLS 1.2 ECDHE AEAD (RFC 5289 / RFC 7905)
        0xC02B => Some("ECDHE-ECDSA-AES128-GCM-SHA256"),
        0xC02F => Some("ECDHE-RSA-AES128-GCM-SHA256"),
        0xC02C => Some("ECDHE-ECDSA-AES256-GCM-SHA384"),
        0xC030 => Some("ECDHE-RSA-AES256-GCM-SHA384"),
        0xCCA9 => Some("ECDHE-ECDSA-CHACHA20-POLY1305"),
        0xCCA8 => Some("ECDHE-RSA-CHACHA20-POLY1305"),
        // TLS 1.2 DHE (IANA-correct names; NO BoringSSL implementation —
        // see `cipher_suite_boringssl_supported`)
        0x009E => Some("DHE-RSA-AES128-GCM-SHA256"),
        0x009C => Some("DHE-RSA-AES256-GCM-SHA384"),
        0x0033 => Some("DHE-RSA-AES128-SHA"),
        0x0067 => Some("DHE-RSA-AES256-SHA256"),
        // TLS 1.2 ECDHE CBC (RFC 5289)
        0xC013 => Some("ECDHE-RSA-AES128-SHA"),
        0xC009 => Some("ECDHE-ECDSA-AES128-SHA"),
        0xC027 => Some("ECDHE-RSA-AES128-SHA256"),
        0xC028 => Some("ECDHE-RSA-AES256-SHA384"),
        // TLS 1.2 static RSA (RFC 5246)
        0x002F => Some("AES128-SHA"),
        0x0035 => Some("AES256-SHA"),
        _ => None,
    }
}

/// Whether the BoringSSL vendored in Bao implements this cipher suite.
///
/// BoringSSL has NO DHE_RSA cipher suites — `kCiphers`
/// (vendor/boringssl/ssl/ssl_cipher.cc) contains no DHE entry, so the four
/// DHE suites of the Firefox profile cannot be offered by a BoringSSL
/// client. This is an engine limitation, not a mapping error:
/// `SSL_(CTX_)set_cipher_list` silently skips unknown names (non-strict),
/// but a list made solely of unsupported names fails the call.
pub fn cipher_suite_boringssl_supported(id: u16) -> bool {
    !matches!(id, 0x009E | 0x009C | 0x0033 | 0x0067)
}

/// Map IANA supported group ID to its OpenSSL group name
/// (`kNamedGroups`, vendor/boringssl/ssl/ssl_key_share.cc).
///
/// Complete IANA truth — FFDHE groups have no BoringSSL implementation
/// (see [`group_boringssl_supported`]).
pub fn group_openssl_name(id: u16) -> Option<&'static str> {
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

/// Whether the BoringSSL vendored in Bao implements this named group.
///
/// `kNamedGroups` has no FFDHE entry. Unlike cipher lists,
/// `SSL_(CTX_)set1_groups_list` FAILS THE WHOLE CALL on any unrecognized
/// name (`ssl_str_to_group_id` → error), so an unfiltered Firefox curves
/// list ("…:ffdhe2048:ffdhe3072") silently discards the entire groups
/// fingerprint. Builders must filter through this predicate.
pub fn group_boringssl_supported(id: u16) -> bool {
    !matches!(id, 0x0100 | 0x0101)
}

/// Map IANA signature algorithm ID to its BoringSSL sigalg name
/// (`kSignatureAlgorithmNames`, vendor/boringssl/ssl/ssl_privkey.cc).
/// Every mapped algorithm is implemented by BoringSSL.
pub fn sigalg_openssl_name(id: u16) -> Option<&'static str> {
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

// ── BoringSSL list builders (shared by every TLS stack) ────────────────

/// Build a colon-separated OpenSSL cipher-name string for BoringSSL's
/// `SSL_(CTX_)set_cipher_list`, preserving profile order and keeping only
/// suites the engine can actually configure.
///
/// TLS 1.3 suites (0x1301-0x1303) are excluded: per BoringSSL's ssl.h,
/// "TLS 1.3 ciphers do not participate in this mechanism and instead have
/// a built-in preference order" — their names in a cipher list are silent
/// no-ops. DHE suites are excluded via [`cipher_suite_boringssl_supported`].
pub fn boringssl_cipher_list_string(suites: &[u16]) -> String {
    suites
        .iter()
        .copied()
        .filter(|&id| cipher_suite_boringssl_supported(id) && !(0x1301..=0x1303).contains(&id))
        .filter_map(cipher_suite_openssl_name)
        .collect::<Vec<_>>()
        .join(":")
}

/// Build a colon-separated group-name string for BoringSSL's
/// `SSL_(CTX_)set1_groups_list`, preserving profile order.
///
/// FFDHE groups are dropped via [`group_boringssl_supported`] — a single
/// unrecognized name fails the whole engine call.
pub fn boringssl_curves_list_string(groups: &[u16]) -> String {
    groups
        .iter()
        .copied()
        .filter(|&id| group_boringssl_supported(id))
        .filter_map(group_openssl_name)
        .collect::<Vec<_>>()
        .join(":")
}

/// Build a colon-separated sigalg-name string for BoringSSL's
/// `SSL_(CTX_)set1_sigalgs_list`, preserving profile order.
pub fn boringssl_sigalgs_list_string(sigalgs: &[u16]) -> String {
    sigalgs
        .iter()
        .copied()
        .filter_map(sigalg_openssl_name)
        .collect::<Vec<_>>()
        .join(":")
}

/// Pre-computed BoringSSL configuration strings derived from a [`TlsFingerprint`].
///
/// This is an intermediate representation that bridges `bao_stealth::TlsFingerprint`
/// (IANA u16 IDs) to `bun_http::ssl_config::SSLConfig` (C string pointers for
/// BoringSSL API calls). Created once per profile, then used to populate
/// `SSLConfig` fields before TLS handshake.
///
/// Usage:
/// ```no_run
/// use bao_stealth::{TlsFingerprint, TlsFingerprintConfig};
///
/// // Pick a browser TLS fingerprint (Firefox/Chrome), then derive the
/// // BoringSSL configuration strings (cipher lists / curves / sigalgs).
/// let fp = TlsFingerprint::firefox();
/// let config = TlsFingerprintConfig::from_fingerprint(&fp);
///
/// // The derived config carries non-empty OpenSSL/BoringSSL name strings.
/// assert!(config.has_fingerprint());
/// assert!(!config.tls12_cipher_list.is_empty());
/// assert!(!config.tls13_cipher_suites.is_empty());
/// assert!(!config.curves_list.is_empty());
/// assert!(!config.sigalgs_list.is_empty());
/// // Then in bao_runtime, write config.tls12_cipher_list into SSLConfig.
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
        assert_eq!(
            computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Firefox — computed: {}",
            computed
        );
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    #[test]
    fn test_compute_ja3_matches_stored_hash_chrome() {
        let fp = TlsFingerprint::chrome();
        let computed = fp.compute_ja3();
        assert_eq!(
            computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Chrome — computed: {}",
            computed
        );
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    #[test]
    fn test_compute_ja3_matches_stored_hash_chrome_latest() {
        let fp = TlsFingerprint::chrome_latest();
        let computed = fp.compute_ja3();
        assert_eq!(
            computed, fp.ja3_hash,
            "compute_ja3() must equal stored ja3_hash for Chrome latest — computed: {}",
            computed
        );
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
        assert!(
            parts.len() >= 2,
            "JA4 should have underscore separator: {}",
            ja4
        );
        let hash_part = parts.last().unwrap();
        assert!(
            hash_part.len() >= 12,
            "ALPN hash part should be at least 12 chars: {}",
            hash_part
        );
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "ALPN hash must be valid hex: {}",
            hash_part
        );
    }

    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]
    // JA4 must differ between Firefox and Chrome profiles
    #[test]
    fn test_compute_ja4_differs_between_profiles() {
        let ff = TlsFingerprint::firefox();
        let ch = TlsFingerprint::chrome();
        assert_ne!(
            ff.compute_ja4(),
            ch.compute_ja4(),
            "Firefox and Chrome must produce different JA4 fingerprints"
        );
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
        assert_eq!(
            tls12.len(),
            fp.cipher_suites.len() - fp.tls13_suites().len()
        );
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
        assert!(
            list.contains("TLS_AES_128_GCM_SHA256"),
            "Should contain TLS 1.3 AES-128"
        );
        assert!(
            list.contains("TLS_AES_256_GCM_SHA384"),
            "Should contain TLS 1.3 AES-256"
        );
        assert!(
            list.contains("TLS_CHACHA20_POLY1305_SHA256"),
            "Should contain TLS 1.3 ChaCha20"
        );
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
        assert!(
            list.contains("ecdsa_secp256r1_sha256"),
            "Should contain ECDSA P-256"
        );
        assert!(
            list.contains("rsa_pss_rsae_sha256"),
            "Should contain RSA-PSS"
        );
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
    fn test_firefox_chrome_tls12_lists_converge_on_boringssl() {
        // The Firefox/Chrome TLS 1.2 sets differ only in DHE suites, which
        // BoringSSL cannot offer — the offered lists converge. Fingerprint
        // differentiation on BoringSSL comes from groups, sigalgs,
        // extensions and H2 SETTINGS instead.
        let ff = TlsFingerprint::firefox();
        let ch = TlsFingerprint::chrome();
        assert_eq!(ff.tls12_cipher_list_string(), ch.tls12_cipher_list_string());
        // The full profiles still differ (DHE suites present in IANA truth):
        assert_ne!(ff.cipher_suites, ch.cipher_suites);
    }

    #[test]
    fn test_cipher_suite_openssl_name_known_ids() {
        assert_eq!(
            cipher_suite_openssl_name(0x1301),
            Some("TLS_AES_128_GCM_SHA256")
        );
        assert_eq!(
            cipher_suite_openssl_name(0xC02B),
            Some("ECDHE-ECDSA-AES128-GCM-SHA256")
        );
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
        // TLS 1.2 lists converge on BoringSSL (DHE-only difference — see
        // test_firefox_chrome_tls12_lists_converge_on_boringssl); the
        // profiles still differ in offered groups (Firefox keeps P-521).
        assert_ne!(ff_config.curves_list, ch_config.curves_list);
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

    // ─── Single-source mapping table: exhaustive per-code assertions ──
    // Every value cross-verified against vendor/boringssl tls1.h TLS1_TXT_*
    // constants and the IANA registry. Any change here must cite both.
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_cipher_table_exhaustive_iana_truth() {
        // TLS 1.3
        assert_eq!(cipher_suite_openssl_name(0x1301), Some("TLS_AES_128_GCM_SHA256"));
        assert_eq!(cipher_suite_openssl_name(0x1302), Some("TLS_AES_256_GCM_SHA384"));
        assert_eq!(cipher_suite_openssl_name(0x1303), Some("TLS_CHACHA20_POLY1305_SHA256"));
        // TLS 1.2 ECDHE AEAD
        assert_eq!(cipher_suite_openssl_name(0xC02B), Some("ECDHE-ECDSA-AES128-GCM-SHA256"));
        assert_eq!(cipher_suite_openssl_name(0xC02F), Some("ECDHE-RSA-AES128-GCM-SHA256"));
        assert_eq!(cipher_suite_openssl_name(0xC02C), Some("ECDHE-ECDSA-AES256-GCM-SHA384"));
        assert_eq!(cipher_suite_openssl_name(0xC030), Some("ECDHE-RSA-AES256-GCM-SHA384"));
        assert_eq!(cipher_suite_openssl_name(0xCCA9), Some("ECDHE-ECDSA-CHACHA20-POLY1305"));
        assert_eq!(cipher_suite_openssl_name(0xCCA8), Some("ECDHE-RSA-CHACHA20-POLY1305"));
        // TLS 1.2 DHE — the 0x009E regression guard (was mis-mapped to
        // ECDHE-RSA-AES256-GCM-SHA384 in the old servo-local table copy)
        assert_eq!(cipher_suite_openssl_name(0x009E), Some("DHE-RSA-AES128-GCM-SHA256"));
        assert_eq!(cipher_suite_openssl_name(0x009C), Some("DHE-RSA-AES256-GCM-SHA384"));
        assert_eq!(cipher_suite_openssl_name(0x0033), Some("DHE-RSA-AES128-SHA"));
        assert_eq!(cipher_suite_openssl_name(0x0067), Some("DHE-RSA-AES256-SHA256"));
        // TLS 1.2 ECDHE CBC
        assert_eq!(cipher_suite_openssl_name(0xC013), Some("ECDHE-RSA-AES128-SHA"));
        assert_eq!(cipher_suite_openssl_name(0xC009), Some("ECDHE-ECDSA-AES128-SHA"));
        assert_eq!(cipher_suite_openssl_name(0xC027), Some("ECDHE-RSA-AES128-SHA256"));
        assert_eq!(cipher_suite_openssl_name(0xC028), Some("ECDHE-RSA-AES256-SHA384"));
        // TLS 1.2 static RSA (were mis-mapped to ECDHE names in the old copy)
        assert_eq!(cipher_suite_openssl_name(0x002F), Some("AES128-SHA"));
        assert_eq!(cipher_suite_openssl_name(0x0035), Some("AES256-SHA"));
        // Unknown
        assert_eq!(cipher_suite_openssl_name(0xFFFF), None);
    }

    #[test]
    fn test_group_table_exhaustive_iana_truth() {
        assert_eq!(group_openssl_name(0x001D), Some("X25519"));
        assert_eq!(group_openssl_name(0x0017), Some("P-256"));
        assert_eq!(group_openssl_name(0x0018), Some("P-384"));
        assert_eq!(group_openssl_name(0x0019), Some("P-521"));
        assert_eq!(group_openssl_name(0x0100), Some("ffdhe2048"));
        assert_eq!(group_openssl_name(0x0101), Some("ffdhe3072"));
        assert_eq!(group_openssl_name(0xFFFF), None);
    }

    #[test]
    fn test_sigalg_table_exhaustive_iana_truth() {
        assert_eq!(sigalg_openssl_name(0x0403), Some("ecdsa_secp256r1_sha256"));
        assert_eq!(sigalg_openssl_name(0x0503), Some("ecdsa_secp384r1_sha384"));
        assert_eq!(sigalg_openssl_name(0x0603), Some("ecdsa_secp521r1_sha512"));
        assert_eq!(sigalg_openssl_name(0x0804), Some("rsa_pss_rsae_sha256"));
        assert_eq!(sigalg_openssl_name(0x0805), Some("rsa_pss_rsae_sha384"));
        assert_eq!(sigalg_openssl_name(0x0806), Some("rsa_pss_rsae_sha512"));
        assert_eq!(sigalg_openssl_name(0x0401), Some("rsa_pkcs1_sha256"));
        assert_eq!(sigalg_openssl_name(0x0501), Some("rsa_pkcs1_sha384"));
        assert_eq!(sigalg_openssl_name(0x0601), Some("rsa_pkcs1_sha512"));
        assert_eq!(sigalg_openssl_name(0x0203), Some("ecdsa_sha1"));
        assert_eq!(sigalg_openssl_name(0x0201), Some("rsa_pkcs1_sha1"));
        assert_eq!(sigalg_openssl_name(0xFFFF), None);
    }

    // ─── BoringSSL support predicates + filtered builders ─────────────

    #[test]
    fn test_cipher_boringssl_support_dhe_excluded() {
        for id in [0x009E, 0x009C, 0x0033, 0x0067] {
            assert!(
                !cipher_suite_boringssl_supported(id),
                "0x{id:04X} has no BoringSSL implementation"
            );
            // Truth table still knows the IANA name — engine limit, not gap.
            assert!(cipher_suite_openssl_name(id).is_some());
        }
        for id in [0x1301, 0xC02B, 0xC02F, 0xC02C, 0xC030, 0xCCA9, 0xCCA8, 0xC013, 0xC009] {
            assert!(cipher_suite_boringssl_supported(id), "0x{id:04X}");
        }
    }

    #[test]
    fn test_group_boringssl_support_ffdhe_excluded() {
        for id in [0x0100, 0x0101] {
            assert!(!group_boringssl_supported(id));
            assert!(group_openssl_name(id).is_some());
        }
        for id in [0x001D, 0x0017, 0x0018, 0x0019] {
            assert!(group_boringssl_supported(id));
        }
    }

    #[test]
    fn test_boringssl_cipher_list_filters_dhe_and_tls13() {
        let ff = TlsFingerprint::firefox();
        let list = boringssl_cipher_list_string(&ff.cipher_suites);
        // NB: match on entry prefix — the *substring* "DHE-" also occurs
        // inside "ECDHE-...".
        assert!(
            !list.split(':').any(|n| n.starts_with("DHE-")),
            "DHE names must not reach BoringSSL: {list}"
        );
        assert!(!list.contains("TLS_AES"), "TLS 1.3 is engine-builtin: {list}");
        assert_eq!(
            list,
            "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:\
             ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:\
             ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:\
             ECDHE-RSA-AES128-SHA:ECDHE-ECDSA-AES128-SHA"
        );
    }

    #[test]
    fn test_firefox_curves_list_excludes_ffdhe() {
        // Regression: SSL_set1_groups_list fails the WHOLE call on any
        // unknown name — an unfiltered Firefox list silently discarded the
        // entire groups fingerprint.
        let ff = TlsFingerprint::firefox();
        let list = ff.curves_list_string();
        assert_eq!(list, "X25519:P-256:P-384:P-521");
        assert!(!list.contains("ffdhe"), "FFDHE would poison the whole list: {list}");
    }

    #[test]
    fn test_chrome_curves_list_unchanged_by_filter() {
        let ch = TlsFingerprint::chrome();
        assert_eq!(ch.curves_list_string(), "X25519:P-256:P-384");
    }

    #[test]
    fn test_boringssl_sigalgs_list_full_profile() {
        let ff = TlsFingerprint::firefox();
        let list = boringssl_sigalgs_list_string(&ff.signature_algorithms);
        assert_eq!(
            list,
            "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:\
             ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:\
             rsa_pss_rsae_sha512:rsa_pkcs1_sha512:ecdsa_sha1:rsa_pkcs1_sha1"
        );
    }

    // ─── Profile completeness: no suite silently unmapped ─────────────

    #[test]
    fn test_all_profile_cipher_suites_have_names() {
        for (name, fp) in [
            ("firefox", TlsFingerprint::firefox()),
            ("chrome", TlsFingerprint::chrome()),
            ("chrome_120", TlsFingerprint::chrome_120()),
            ("chrome_latest", TlsFingerprint::chrome_latest()),
        ] {
            for &id in &fp.cipher_suites {
                assert!(
                    cipher_suite_openssl_name(id).is_some(),
                    "{name} suite 0x{id:04X} has no OpenSSL name — silent fingerprint loss"
                );
            }
            for &g in &fp.supported_groups {
                assert!(
                    group_openssl_name(g).is_some(),
                    "{name} group 0x{g:04X} has no name"
                );
            }
            for &s in &fp.signature_algorithms {
                assert!(
                    sigalg_openssl_name(s).is_some(),
                    "{name} sigalg 0x{s:04X} has no name"
                );
            }
        }
    }

    #[test]
    fn test_profile_suite_counts() {
        // Firefox: 15 suites (3×TLS1.3 + 12×TLS1.2, of which 4 DHE → 8 offered).
        let ff = TlsFingerprint::firefox();
        assert_eq!(ff.cipher_suites.len(), 15);
        assert_eq!(ff.tls12_cipher_list_string().matches(':').count() + 1, 8);
        // Chrome: 13 suites (3×TLS1.3 + 10×TLS1.2, of which 2 DHE → 8 offered).
        let ch = TlsFingerprint::chrome();
        assert_eq!(ch.cipher_suites.len(), 13);
        assert_eq!(ch.tls12_cipher_list_string().matches(':').count() + 1, 8);
    }

    #[test]
    fn test_boringssl_builders_preserve_order_and_dedupe_none() {
        // Order preservation: input order must survive the filter.
        let list = boringssl_cipher_list_string(&[0xC030, 0xC02B, 0x009E, 0x1301]);
        assert_eq!(list, "ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-AES128-GCM-SHA256");
        // Empty input → empty string (callers gate on is_empty before FFI).
        assert_eq!(boringssl_cipher_list_string(&[]), "");
        assert_eq!(boringssl_curves_list_string(&[]), "");
        assert_eq!(boringssl_sigalgs_list_string(&[]), "");
    }
}
