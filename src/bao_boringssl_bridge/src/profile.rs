// @trace REQ-STL-001 [entity:TlsProfile]
//! TLS profile configuration for stealth fingerprinting.
//!
//! Provides preset TLS profiles that mimic browser fingerprints
//! for anti-detection purposes. Each profile reorders cipher suites
//! and key exchange groups to match the target browser's
//! ClientHello fingerprint (JA3/JA4).

use bun_boringssl_sys::boringssl::*;

use crate::client::TlsClient;
use crate::connection::TlsError;

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
    /// Build a TlsClient with this profile's cipher suite ordering.
    pub fn build_client(&self) -> Result<TlsClient, TlsError> {
        match self {
            TlsProfile::Default => TlsClient::new(),
            TlsProfile::Chrome => Self::build_with_profile(Self::chrome_ciphers(), Self::chrome_curves()),
            TlsProfile::Firefox => Self::build_with_profile(Self::firefox_ciphers(), Self::firefox_curves()),
            TlsProfile::Safari => Self::build_with_profile(Self::safari_ciphers(), Self::safari_curves()),
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
    // TLS 1.2: ECDSA+128, RSA+128, ECDSA+256, RSA+256, CHACHA20
    // Curves: X25519, P-256, P-384

    fn chrome_ciphers() -> &'static str {
        "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305"
    }

    fn chrome_curves() -> &'static str {
        "X25519:P-256:P-384"
    }

    // ─── Firefox 120+ ───────────────────────────────────────────────
    // TLS 1.3: AES_128, CHACHA20, AES_256
    // TLS 1.2: ECDSA+128, RSA+128, ECDSA+256, RSA+256, CHACHA20
    // Curves: X25519, P-256, P-384

    fn firefox_ciphers() -> &'static str {
        "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305"
    }

    fn firefox_curves() -> &'static str {
        "X25519:P-256:P-384"
    }

    // ─── Safari 17+ ─────────────────────────────────────────────────
    // TLS 1.3: AES_128, AES_256, CHACHA20
    // TLS 1.2: ECDSA+256, ECDSA+128, RSA+256, RSA+128
    // Curves: P-256, X25519, P-384

    fn safari_ciphers() -> &'static str {
        "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-RSA-AES128-GCM-SHA256"
    }

    fn safari_curves() -> &'static str {
        "P-256:X25519:P-384"
    }

    // ─── Common builder ─────────────────────────────────────────────

    fn build_with_profile(ciphers: &'static str, curves: &'static str) -> Result<TlsClient, TlsError> {
        let client = TlsClient::new()?;
        let ctx = client.ctx();

        // Override cipher list with profile-specific ordering
        let ciphers_c = std::ffi::CString::new(ciphers)
            .map_err(|_| TlsError::BoringSSL("invalid cipher string"))?;
        let ok = unsafe { SSL_CTX_set_cipher_list(ctx, ciphers_c.as_ptr()) };
        if ok == 0 {
            return Err(TlsError::BoringSSL("SSL_CTX_set_cipher_list failed for profile"));
        }

        // Set curves on the SSL_CTX via an intermediate SSL object.
        // BoringSSL only has SSL_set1_curves_list (per-connection), not SSL_CTX_set1_curves_list.
        // We create a temporary SSL to apply curves, but since TlsClient owns the SSL_CTX
        // and connections are created from it, we instead store the curves string for
        // TlsConnection to apply when it creates the SSL.
        // For now, curves will be applied per-connection in TlsConnection::new_client().
        let _ = curves; // Applied at connection time

        Ok(client)
    }

    /// Get the curves string for this profile (applied per-connection).
    pub fn curves(&self) -> Option<&'static str> {
        match self {
            TlsProfile::Default => None,
            TlsProfile::Chrome => Some(Self::chrome_curves()),
            TlsProfile::Firefox => Some(Self::firefox_curves()),
            TlsProfile::Safari => Some(Self::safari_curves()),
        }
    }
}

impl Default for TlsProfile {
    fn default() -> Self {
        TlsProfile::Default
    }
}
