//! Self-built CryptoProvider backed by RustCrypto primitives.
//!
//! Implements all rustls crypto traits without aws-lc-rs or ring:
//! - Cipher suites: AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305
//! - Key exchange: X25519, P-256, P-384
//! - Signature verification: ECDSA P-256/P-384, RSA PKCS1/PSS, Ed25519
//! - Secure random: getrandom

use std::sync::Arc;

use rustls::crypto::{CryptoProvider, SecureRandom};

// ─── Public API ──────────────────────────────────────────────────────

/// Build the Bao CryptoProvider using RustCrypto backends.
pub fn bao_crypto_provider() -> Arc<CryptoProvider> {
    bao_crypto_provider_with_order(
        DEFAULT_CIPHER_SUITES.to_vec(),
        DEFAULT_KX_GROUPS.to_vec(),
    )
}

/// Build the Bao CryptoProvider with custom cipher suite and kx group ordering.
///
/// Used by `profile.rs` to create browser-specific fingerprints.
pub(crate) fn bao_crypto_provider_with_order(
    cipher_suites: Vec<rustls::SupportedCipherSuite>,
    kx_groups: Vec<&'static dyn rustls::crypto::SupportedKxGroup>,
) -> Arc<CryptoProvider> {
    Arc::new(CryptoProvider {
        cipher_suites,
        kx_groups,
        signature_verification_algorithms: verify::ALGORITHMS,
        secure_random: &BaoSecureRandom,
        key_provider: &BaoKeyProvider,
    })
}

/// Default cipher suite order (security-first: AES-256 before AES-128).
const DEFAULT_CIPHER_SUITES: &[rustls::SupportedCipherSuite] = &[
    cipher::TLS13_AES_256_GCM_SHA384,
    cipher::TLS13_AES_128_GCM_SHA256,
    cipher::TLS13_CHACHA20_POLY1305_SHA256,
    cipher::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    cipher::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    cipher::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    cipher::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    cipher::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    cipher::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
];

/// Default key exchange group order (X25519 first for performance).
const DEFAULT_KX_GROUPS: &[&'static dyn rustls::crypto::SupportedKxGroup] = &[
    kx::X25519,
    kx::SECP256R1,
    kx::SECP384R1,
];

// ─── SecureRandom ────────────────────────────────────────────────────

#[derive(Debug)]
struct BaoSecureRandom;

impl SecureRandom for BaoSecureRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), rustls::crypto::GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| rustls::crypto::GetRandomFailed)
    }
}

// ─── Sub-modules ─────────────────────────────────────────────────────

pub(crate) mod cipher;
mod hash;
mod hmac;
pub(crate) mod kx;
mod sign;
mod verify;

// Re-export sign for key_provider
use sign::BaoKeyProvider;
