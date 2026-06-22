// REQ-STL-001 / REQ-STL-002: Wire-level TLS/HTTP2 configuration for servo's network layer.
//
// servo uses BoringSSL + `hyper` for TLS/HTTP2. This type carries the
// `StealthProfile` fields that servo's connector consumes:
//   - Cipher suites (settable via `SSL_CTX_set_cipher_list()`)
//   - Curves/groups (settable via `SSL_set1_curves_list()`)
//   - Signature algorithms (settable via `SSL_CTX_set1_sigalgs_list()`)
//   - ALPN protocols (settable via `SSL_CTX_set_alpn_protos()`)
//   - HTTP/2 SETTINGS parameters (settable via hyper's Client builder)
//
// BoringSSL supports full JA3/JA4 fingerprint configuration including cipher
// suite reordering, curves/groups ordering, and signature algorithm ordering.
//
// This type is redeclared in servo's `net::connector` (no dependency on `bao_stealth`
// from servo's net crate) and kept in sync via field-matching.

/// Wire-level TLS/HTTP2 configuration for servo's network layer.
///
/// Derived from [`StealthProfile`](crate::StealthProfile) and stored as a global
/// that servo's connector reads during `create_tls_config()` and `create_http_client()`.
#[derive(Debug, Clone)]
pub struct StealthTlsWireConfig {
    /// TLS 1.2 cipher suites as IANA u16 IDs (ordered as in profile).
    ///
    /// Applied via `SSL_CTX_set_cipher_list()` on the BoringSSL SSL_CTX.
    pub tls12_cipher_suites: Vec<u16>,

    /// TLS 1.3 cipher suites as IANA u16 IDs (ordered as in profile).
    ///
    /// Applied via `SSL_CTX_set_cipher_list()` on the BoringSSL SSL_CTX.
    pub tls13_cipher_suites: Vec<u16>,

    /// Signature algorithms as IANA u16 IDs (ordered as in profile).
    ///
    /// Applied via `SSL_CTX_set1_sigalgs_list()` on the BoringSSL SSL_CTX.
    pub signature_algorithms: Vec<u16>,

    /// Supported groups as IANA u16 IDs (ordered as in profile).
    ///
    /// Applied via `SSL_set1_curves_list()` per-connection on the BoringSSL SSL.
    pub supported_groups: Vec<u16>,

    /// ALPN protocols as raw bytes (e.g., `b"h2"`, `b"http/1.1"`).
    ///
    /// Applied via `SSL_CTX_set_alpn_protos()` on the BoringSSL SSL_CTX.
    pub alpn_protocols: Vec<Vec<u8>>,

    /// HTTP/2 SETTINGS frame payload in binary wire format (6 bytes per setting:
    /// 2-byte setting ID big-endian + 4-byte value big-endian).
    ///
    /// This is used for HTTP/2 AKAMAI fingerprint matching. The individual settings
    /// that `hyper`'s builder supports are extracted and applied separately; this
    /// payload is stored for potential future use with a custom h2 connection wrapper.
    pub h2_settings_payload: Vec<u8>,

    /// HTTP/2 initial stream window size for the SETTINGS and WINDOW_UPDATE frames.
    ///
    /// Applied via `hyper`'s `http2_initial_stream_window_size()` builder method.
    pub h2_initial_stream_size: u32,

    /// HTTP/2 initial connection window size.
    ///
    /// Applied via `hyper`'s `http2_initial_connection_window_size()` builder method.
    pub h2_initial_connection_window_size: u32,

    /// HTTP/2 SETTINGS_MAX_FRAME_SIZE value.
    ///
    /// Applied via `hyper`'s `http2_max_frame_size()` builder method.
    pub h2_max_frame_size: u32,

    /// HTTP/2 SETTINGS_MAX_HEADER_LIST_SIZE value.
    ///
    /// Applied via `hyper`'s `http2_max_header_list_size()` builder method.
    pub h2_max_header_list_size: u32,
}

impl StealthTlsWireConfig {
    /// Derive a wire config from a [`StealthProfile`](crate::StealthProfile).
    ///
    /// Splits cipher suites into TLS 1.3 and TLS 1.2, and converts HTTP/2 SETTINGS
    /// into both binary wire format and the individual fields `hyper`'s builder accepts.
    pub fn from_profile(profile: &crate::StealthProfile) -> Self {
        let tls = &profile.tls;
        let http2 = &profile.http2;

        // Split cipher suites into TLS 1.3 (0x1301-0x1303) and TLS 1.2.
        let tls13_suites: Vec<u16> = tls
            .cipher_suites
            .iter()
            .copied()
            .filter(|s| (0x1301..=0x1303).contains(s))
            .collect();
        let tls12_suites: Vec<u16> = tls
            .cipher_suites
            .iter()
            .copied()
            .filter(|s| !(0x1301..=0x1303).contains(s))
            .collect();

        // HTTP/2 SETTINGS binary wire format: each setting is 2-byte ID + 4-byte value,
        // both big-endian (RFC 7540 §6.5.1).
        let h2_settings = http2.settings_frame_payload();
        let mut h2_wire = Vec::with_capacity(h2_settings.len() * 6);
        for (id, value) in &h2_settings {
            h2_wire.extend_from_slice(&id.to_be_bytes());
            h2_wire.extend_from_slice(&value.to_be_bytes());
        }

        // The connection-level window size is the same as the stream-level one in
        // Firefox's profile. hyper allows them to be set independently, but the
        // fingerprint data only provides one `initial_window_size` value.
        let h2_conn_window = http2.initial_window_size;

        StealthTlsWireConfig {
            tls12_cipher_suites: tls12_suites,
            tls13_cipher_suites: tls13_suites,
            signature_algorithms: tls.signature_algorithms.clone(),
            supported_groups: tls.supported_groups.clone(),
            alpn_protocols: tls.alpn_protocols.clone(),
            h2_settings_payload: h2_wire,
            h2_initial_stream_size: http2.initial_window_size,
            h2_initial_connection_window_size: h2_conn_window,
            h2_max_frame_size: http2.max_frame_size,
            h2_max_header_list_size: http2.max_header_list_size,
        }
    }

    /// Convenience constructor using the default Firefox profile.
    pub fn firefox() -> Self {
        Self::from_profile(&crate::StealthProfile::firefox_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_profile_firefox_nonempty() {
        let profile = crate::StealthProfile::firefox_default();
        let config = StealthTlsWireConfig::from_profile(&profile);
        assert!(!config.tls12_cipher_suites.is_empty());
        assert!(!config.tls13_cipher_suites.is_empty());
        assert!(!config.signature_algorithms.is_empty());
        assert!(!config.supported_groups.is_empty());
        assert!(!config.alpn_protocols.is_empty());
        assert!(!config.h2_settings_payload.is_empty());
    }

    #[test]
    fn from_profile_chrome_nonempty() {
        let profile = crate::StealthProfile::chrome_default();
        let config = StealthTlsWireConfig::from_profile(&profile);
        assert!(!config.tls12_cipher_suites.is_empty());
        assert!(!config.alpn_protocols.is_empty());
    }

    #[test]
    fn firefox_convenience_matches_from_profile() {
        let from_convenience = StealthTlsWireConfig::firefox();
        let from_explicit =
            StealthTlsWireConfig::from_profile(&crate::StealthProfile::firefox_default());
        assert_eq!(
            from_convenience.tls12_cipher_suites, from_explicit.tls12_cipher_suites,
        );
        assert_eq!(
            from_convenience.tls13_cipher_suites, from_explicit.tls13_cipher_suites,
        );
        assert_eq!(from_convenience.alpn_protocols, from_explicit.alpn_protocols);
        assert_eq!(
            from_convenience.h2_initial_stream_size,
            from_explicit.h2_initial_stream_size,
        );
    }

    #[test]
    fn tls13_suites_contain_standard_three() {
        let config = StealthTlsWireConfig::firefox();
        assert!(config.tls13_cipher_suites.contains(&0x1301)); // AES_128_GCM
        assert!(config.tls13_cipher_suites.contains(&0x1302)); // AES_256_GCM
        assert!(config.tls13_cipher_suites.contains(&0x1303)); // CHACHA20
    }

    #[test]
    fn tls12_suites_do_not_contain_tls13_range() {
        let config = StealthTlsWireConfig::firefox();
        for suite in &config.tls12_cipher_suites {
            assert!(
                !(0x1301..=0x1303).contains(suite),
                "TLS 1.2 suite list should not contain TLS 1.3 suite 0x{:04x}",
                suite
            );
        }
    }

    #[test]
    fn suite_partition_completeness() {
        let config = StealthTlsWireConfig::firefox();
        let profile = crate::StealthProfile::firefox_default();
        let total = profile.tls.cipher_suites.len();
        let tls13 = config.tls13_cipher_suites.len();
        let tls12 = config.tls12_cipher_suites.len();
        assert_eq!(total, tls13 + tls12);
    }

    #[test]
    fn h2_settings_payload_wire_format() {
        let config = StealthTlsWireConfig::firefox();
        // Each setting is 6 bytes (2 ID + 4 value). Firefox has 6 settings.
        assert_eq!(config.h2_settings_payload.len(), 36);
        // First setting ID should be 0x0001 (SETTINGS_HEADER_TABLE_SIZE)
        assert_eq!(&config.h2_settings_payload[0..2], &[0x00, 0x01]);
    }

    #[test]
    fn h2_initial_stream_size_firefox() {
        let config = StealthTlsWireConfig::firefox();
        assert_eq!(config.h2_initial_stream_size, 131072);
    }

    #[test]
    fn h2_max_frame_size_firefox() {
        let config = StealthTlsWireConfig::firefox();
        assert_eq!(config.h2_max_frame_size, 16384);
    }

    #[test]
    fn h2_max_header_list_size_firefox() {
        let config = StealthTlsWireConfig::firefox();
        assert_eq!(config.h2_max_header_list_size, 262144);
    }

    #[test]
    fn alpn_protocols_contain_h2() {
        let config = StealthTlsWireConfig::firefox();
        assert!(config.alpn_protocols.iter().any(|p| p == b"h2"));
        assert!(config
            .alpn_protocols
            .iter()
            .any(|p| p == b"http/1.1"));
    }

    #[test]
    fn firefox_and_chrome_different() {
        let ff = StealthTlsWireConfig::firefox();
        let ch = StealthTlsWireConfig::from_profile(&crate::StealthProfile::chrome_default());
        // Chrome has fewer cipher suites than Firefox
        assert_ne!(
            ff.tls12_cipher_suites.len(),
            ch.tls12_cipher_suites.len()
        );
        // Different HTTP/2 window sizes
        assert_ne!(ff.h2_initial_stream_size, ch.h2_initial_stream_size);
    }

    #[test]
    fn clone_preserves_all_fields() {
        let config = StealthTlsWireConfig::firefox();
        let cloned = config.clone();
        assert_eq!(config.tls12_cipher_suites, cloned.tls12_cipher_suites);
        assert_eq!(config.tls13_cipher_suites, cloned.tls13_cipher_suites);
        assert_eq!(config.alpn_protocols, cloned.alpn_protocols);
        assert_eq!(config.h2_settings_payload, cloned.h2_settings_payload);
        assert_eq!(config.h2_initial_stream_size, cloned.h2_initial_stream_size);
    }
}
