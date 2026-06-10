//! TLS profile configuration for stealth fingerprinting.
//!
//! Provides preset TLS profiles that mimic browser fingerprints
//! for anti-detection purposes. Each profile reorders cipher suites,
//! key exchange groups, and ALPN protocols to match the target browser's
//! ClientHello fingerprint (JA3/JA4).

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::SupportedCipherSuite;

use crate::client::TlsClient;
use crate::provider::cipher;
use crate::provider::kx;
use crate::provider::bao_crypto_provider_with_order;

/// Browser TLS profile presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsProfile {
    /// Chrome desktop TLS fingerprint (Chrome 120+).
    Chrome,
    /// Firefox desktop TLS fingerprint (Firefox 120+).
    Firefox,
    /// Safari desktop TLS fingerprint (Safari 17+).
    Safari,
    /// Default profile (no fingerprint manipulation).
    Default,
}

impl TlsProfile {
    /// Build a ClientConfig with this profile's cipher suite ordering.
    pub fn build_client_config(&self) -> Arc<ClientConfig> {
        match self {
            TlsProfile::Default => TlsClient::new().build(),
            TlsProfile::Chrome => Self::build_with_profile(
                Self::chrome_suites(),
                Self::chrome_kx(),
                vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ),
            TlsProfile::Firefox => Self::build_with_profile(
                Self::firefox_suites(),
                Self::firefox_kx(),
                vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ),
            TlsProfile::Safari => Self::build_with_profile(
                Self::safari_suites(),
                Self::safari_kx(),
                vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ),
        }
    }

    /// Get the profile name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            TlsProfile::Chrome => "chrome",
            TlsProfile::Firefox => "firefox",
            TlsProfile::Safari => "safari",
            TlsProfile::Default => "default",
        }
    }

    // ─── Chrome 120+ ────────────────────────────────────────────────
    // TLS 1.3: AES_128, AES_256, CHACHA20
    // TLS 1.2: ECDSA+128, RSA+128, ECDSA+256, RSA+256
    // KX: X25519, P-256, P-384

    fn chrome_suites() -> Vec<SupportedCipherSuite> {
        vec![
            cipher::TLS13_AES_128_GCM_SHA256,
            cipher::TLS13_AES_256_GCM_SHA384,
            cipher::TLS13_CHACHA20_POLY1305_SHA256,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            cipher::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            cipher::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        ]
    }

    fn chrome_kx() -> Vec<&'static dyn rustls::crypto::SupportedKxGroup> {
        vec![kx::X25519, kx::SECP256R1, kx::SECP384R1]
    }

    // ─── Firefox 120+ ───────────────────────────────────────────────
    // TLS 1.3: AES_128, CHACHA20, AES_256
    // TLS 1.2: ECDSA+128, RSA+128, ECDSA+256, RSA+256
    // KX: X25519, P-256, P-384

    fn firefox_suites() -> Vec<SupportedCipherSuite> {
        vec![
            cipher::TLS13_AES_128_GCM_SHA256,
            cipher::TLS13_CHACHA20_POLY1305_SHA256,
            cipher::TLS13_AES_256_GCM_SHA384,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            cipher::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            cipher::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        ]
    }

    fn firefox_kx() -> Vec<&'static dyn rustls::crypto::SupportedKxGroup> {
        vec![kx::X25519, kx::SECP256R1, kx::SECP384R1]
    }

    // ─── Safari 17+ ─────────────────────────────────────────────────
    // TLS 1.3: AES_128, AES_256, CHACHA20
    // TLS 1.2: ECDSA+256, ECDSA+128, RSA+256, RSA+128
    // KX: P-256, X25519, P-384

    fn safari_suites() -> Vec<SupportedCipherSuite> {
        vec![
            cipher::TLS13_AES_128_GCM_SHA256,
            cipher::TLS13_AES_256_GCM_SHA384,
            cipher::TLS13_CHACHA20_POLY1305_SHA256,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            cipher::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            cipher::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            cipher::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        ]
    }

    fn safari_kx() -> Vec<&'static dyn rustls::crypto::SupportedKxGroup> {
        vec![kx::SECP256R1, kx::X25519, kx::SECP384R1]
    }

    // ─── Common builder ─────────────────────────────────────────────

    fn build_with_profile(
        cipher_suites: Vec<SupportedCipherSuite>,
        kx_groups: Vec<&'static dyn rustls::crypto::SupportedKxGroup>,
        alpn_protocols: Vec<Vec<u8>>,
    ) -> Arc<ClientConfig> {
        let provider = bao_crypto_provider_with_order(cipher_suites, kx_groups);

        let mut root_store = rustls::RootCertStore::empty();
        let native_certs = rustls_native_certs::load_native_certs();
        for cert in native_certs.certs {
            let _ = root_store.add(cert);
        }
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let mut config = config;
        config.alpn_protocols = alpn_protocols;
        Arc::new(config)
    }
}

impl Default for TlsProfile {
    fn default() -> Self {
        TlsProfile::Default
    }
}
