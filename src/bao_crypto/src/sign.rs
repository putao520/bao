//! Digital signature creation (RSA/ECDSA/Ed25519).
//!
//! Replaces the HMAC fallback in `bun_runtime/node_crypto.rs` createSign().

use rsa::RsaPrivateKey;
use ecdsa::SigningKey;
use ed25519_dalek::SigningKey as Ed25519SigningKey;
use p256::NistP256;
use p384::NistP384;
use pkcs8::DecodePrivateKey;
use signature::SignatureEncoding;

/// Supported signing algorithms.
#[derive(Debug, Clone)]
pub enum SignAlgorithm {
    /// RSA-PKCS1v15 with SHA-256/384/512
    RsaPkcs1v15 { hash: RsaHash },
    /// RSA-PSS with SHA-256/384/512
    RsaPss { hash: RsaHash },
    /// ECDSA P-256 SHA-256
    EcdsaP256,
    /// ECDSA P-384 SHA-384
    EcdsaP384,
    /// Ed25519 (pure EdDSA)
    Ed25519,
}

/// RSA hash variant.
#[derive(Debug, Clone, Copy)]
pub enum RsaHash {
    Sha256,
    Sha384,
    Sha512,
}

/// Signing key holder.
pub enum Signer {
    RsaPkcs1v15 {
        key: RsaPrivateKey,
        hash: RsaHash,
    },
    RsaPss {
        key: RsaPrivateKey,
        hash: RsaHash,
    },
    EcdsaP256(SigningKey<NistP256>),
    EcdsaP384(SigningKey<NistP384>),
    Ed25519(Ed25519SigningKey),
}

/// Signature output format.
#[derive(Debug, Clone, Copy)]
pub enum SignatureFormat {
    /// DER-encoded (default for Node.js RSA/ECDSA)
    Der,
    /// Raw IEEE P1363 format (r||s for ECDSA)
    Raw,
}

impl Signer {
    /// Create a signer from a PKCS#8 DER private key.
    pub fn from_pkcs8_der(algorithm: &SignAlgorithm, der: &[u8]) -> Result<Self, CryptoError> {
        match algorithm {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let key = RsaPrivateKey::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPkcs1v15 { key, hash: *hash })
            }
            SignAlgorithm::RsaPss { hash } => {
                let key = RsaPrivateKey::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPss { key, hash: *hash })
            }
            SignAlgorithm::EcdsaP256 => {
                let key = SigningKey::<NistP256>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP256(key))
            }
            SignAlgorithm::EcdsaP384 => {
                let key = SigningKey::<NistP384>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP384(key))
            }
            SignAlgorithm::Ed25519 => {
                let key = Ed25519SigningKey::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::Ed25519(key))
            }
        }
    }

    /// Create a signer from a PKCS#8 PEM private key.
    pub fn from_pkcs8_pem(algorithm: &SignAlgorithm, pem: &str) -> Result<Self, CryptoError> {
        match algorithm {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let key = RsaPrivateKey::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPkcs1v15 { key, hash: *hash })
            }
            SignAlgorithm::RsaPss { hash } => {
                let key = RsaPrivateKey::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPss { key, hash: *hash })
            }
            SignAlgorithm::EcdsaP256 => {
                let key = SigningKey::<NistP256>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP256(key))
            }
            SignAlgorithm::EcdsaP384 => {
                let key = SigningKey::<NistP384>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP384(key))
            }
            SignAlgorithm::Ed25519 => {
                let key = Ed25519SigningKey::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::Ed25519(key))
            }
        }
    }

    /// Sign data, returning the signature bytes.
    pub fn sign(&self, data: &[u8], format: SignatureFormat) -> Result<Vec<u8>, CryptoError> {
        use rsa::signature::Signer as RsaSigner;
        use rsa::signature::RandomizedSigner as RsaRandomizedSigner;

        match self {
            Self::RsaPkcs1v15 { key, hash } => {
                match hash {
                    RsaHash::Sha256 => {
                        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(key.clone());
                        let sig = signing_key.sign(data);
                        Ok(sig.to_vec())
                    }
                    RsaHash::Sha384 => {
                        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha384>::new(key.clone());
                        let sig = signing_key.sign(data);
                        Ok(sig.to_vec())
                    }
                    RsaHash::Sha512 => {
                        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new(key.clone());
                        let sig = signing_key.sign(data);
                        Ok(sig.to_vec())
                    }
                }
            }
            Self::RsaPss { key, hash } => {
                let mut rng = rand_core::OsRng;
                match hash {
                    RsaHash::Sha256 => {
                        let signing_key = rsa::pss::SigningKey::<sha2::Sha256>::new(key.clone());
                        let sig = signing_key.sign_with_rng(&mut rng, data);
                        Ok(sig.to_vec())
                    }
                    RsaHash::Sha384 => {
                        let signing_key = rsa::pss::SigningKey::<sha2::Sha384>::new(key.clone());
                        let sig = signing_key.sign_with_rng(&mut rng, data);
                        Ok(sig.to_vec())
                    }
                    RsaHash::Sha512 => {
                        let signing_key = rsa::pss::SigningKey::<sha2::Sha512>::new(key.clone());
                        let sig = signing_key.sign_with_rng(&mut rng, data);
                        Ok(sig.to_vec())
                    }
                }
            }
            Self::EcdsaP256(key) => {
                let sig: ecdsa::Signature<NistP256> = key.sign(data);
                match format {
                    SignatureFormat::Der => Ok(sig.to_der().as_bytes().to_vec()),
                    SignatureFormat::Raw => Ok(sig.to_bytes().to_vec()),
                }
            }
            Self::EcdsaP384(key) => {
                let sig: ecdsa::Signature<NistP384> = key.sign(data);
                match format {
                    SignatureFormat::Der => Ok(sig.to_der().as_bytes().to_vec()),
                    SignatureFormat::Raw => Ok(sig.to_bytes().to_vec()),
                }
            }
            Self::Ed25519(key) => {
                let sig = key.sign(data);
                Ok(sig.to_vec())
            }
        }
    }
}

/// Crypto error type.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key error: {0}")]
    KeyError(String),
    #[error("signature error: {0}")]
    SignatureError(String),
    #[error("verification error: {0}")]
    VerificationError(String),
    #[error("cipher error: {0}")]
    CipherError(String),
    #[error("encoding error: {0}")]
    EncodingError(String),
    #[error("invalid algorithm: {0}")]
    InvalidAlgorithm(String),
}
