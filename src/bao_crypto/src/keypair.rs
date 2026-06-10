//! Key pair generation (RSA, EC P-256/P-384, Ed25519, X25519).
//!
//! Implements `crypto.generateKeyPair` / `crypto.generateKeyPairSync`.

use rsa::RsaPrivateKey;
use ecdsa::elliptic_curve::rand_core::OsRng;
use pkcs8::{EncodePrivateKey, EncodePublicKey};
use rand_core::RngCore;

use crate::sign::CryptoError;

/// Key pair type identifier.
#[derive(Debug, Clone)]
pub enum KeyPairType {
    Rsa { bits: usize },
    Ec { curve: EcCurve },
    Ed25519,
    X25519,
}

/// EC curve for key generation.
#[derive(Debug, Clone, Copy)]
pub enum EcCurve {
    P256,
    P384,
}

/// Generated key pair result.
pub struct GeneratedKeyPair {
    pub private_key_der: Vec<u8>,
    pub public_key_der: Vec<u8>,
    pub private_key_pem: Option<String>,
    pub public_key_pem: Option<String>,
}

/// Generate a key pair.
pub fn generate_key_pair(key_type: &KeyPairType) -> Result<GeneratedKeyPair, CryptoError> {
    match key_type {
        KeyPairType::Rsa { bits } => generate_rsa(*bits),
        KeyPairType::Ec { curve } => generate_ec(*curve),
        KeyPairType::Ed25519 => generate_ed25519(),
        KeyPairType::X25519 => generate_x25519(),
    }
}

fn generate_rsa(bits: usize) -> Result<GeneratedKeyPair, CryptoError> {
    let mut rng = OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, bits)
        .map_err(|e| CryptoError::KeyError(e.to_string()))?;
    let public_key = private_key.to_public_key();

    let private_der = private_key
        .to_pkcs8_der()
        .map_err(|e| CryptoError::EncodingError(e.to_string()))?
        .as_bytes()
        .to_vec();

    let public_der = public_key
        .to_public_key_der()
        .map_err(|e| CryptoError::EncodingError(e.to_string()))?
        .as_bytes()
        .to_vec();

    let private_pem = private_key
        .to_pkcs8_pem(pkcs8::LineEnding::LF)
        .map(|p| p.to_string())
        .ok();

    let public_pem = public_key
        .to_public_key_pem(pkcs8::LineEnding::LF)
        .ok();

    Ok(GeneratedKeyPair {
        private_key_der: private_der,
        public_key_der: public_der,
        private_key_pem: private_pem,
        public_key_pem: public_pem,
    })
}

fn generate_ec(curve: EcCurve) -> Result<GeneratedKeyPair, CryptoError> {
    match curve {
        EcCurve::P256 => {
            let private_key = p256::SecretKey::random(&mut OsRng);
            let public_key = private_key.public_key();
            let private_der = private_key
                .to_pkcs8_der()
                .map_err(|e| CryptoError::EncodingError(e.to_string()))?
                .as_bytes()
                .to_vec();
            let public_der = public_key
                .to_public_key_der()
                .map_err(|e| CryptoError::EncodingError(e.to_string()))?
                .as_bytes()
                .to_vec();
            Ok(GeneratedKeyPair {
                private_key_der: private_der,
                public_key_der: public_der,
                private_key_pem: None,
                public_key_pem: None,
            })
        }
        EcCurve::P384 => {
            let private_key = p384::SecretKey::random(&mut OsRng);
            let public_key = private_key.public_key();
            let private_der = private_key
                .to_pkcs8_der()
                .map_err(|e| CryptoError::EncodingError(e.to_string()))?
                .as_bytes()
                .to_vec();
            let public_der = public_key
                .to_public_key_der()
                .map_err(|e| CryptoError::EncodingError(e.to_string()))?
                .as_bytes()
                .to_vec();
            Ok(GeneratedKeyPair {
                private_key_der: private_der,
                public_key_der: public_der,
                private_key_pem: None,
                public_key_pem: None,
            })
        }
    }
}

fn generate_ed25519() -> Result<GeneratedKeyPair, CryptoError> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let private_key = ed25519_dalek::SigningKey::from_bytes(&bytes);
    let public_key = ed25519_dalek::VerifyingKey::from(&private_key);

    let private_der = private_key
        .to_pkcs8_der()
        .map_err(|e| CryptoError::EncodingError(e.to_string()))?
        .as_bytes()
        .to_vec();

    let public_der = public_key
        .to_public_key_der()
        .map_err(|e| CryptoError::EncodingError(e.to_string()))?
        .as_bytes()
        .to_vec();

    Ok(GeneratedKeyPair {
        private_key_der: private_der,
        public_key_der: public_der,
        private_key_pem: None,
        public_key_pem: None,
    })
}

fn generate_x25519() -> Result<GeneratedKeyPair, CryptoError> {
    let private_key = x25519_dalek::StaticSecret::random_from_rng(OsRng);
    let public_key = x25519_dalek::PublicKey::from(&private_key);

    // x25519-dalek doesn't implement pkcs8, encode raw bytes
    let private_der = private_key.to_bytes().to_vec();
    let public_der = public_key.as_bytes().to_vec();

    Ok(GeneratedKeyPair {
        private_key_der: private_der,
        public_key_der: public_der,
        private_key_pem: None,
        public_key_pem: None,
    })
}
