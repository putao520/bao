//! Digital signature verification (RSA/ECDSA/Ed25519).
//!
//! Replaces the HMAC comparison in `bun_runtime/node_crypto.rs` createVerify().

use rsa::RsaPublicKey;
use ecdsa::VerifyingKey;
use ed25519_dalek::VerifyingKey as Ed25519VerifyingKey;
use pkcs8::DecodePublicKey;
use p256::NistP256;
use p384::NistP384;

use crate::sign::{CryptoError, RsaHash, SignAlgorithm, SignatureFormat};

/// Verification key holder.
pub enum Verifier {
    RsaPkcs1v15 {
        key: RsaPublicKey,
        hash: RsaHash,
    },
    RsaPss {
        key: RsaPublicKey,
        hash: RsaHash,
    },
    EcdsaP256(VerifyingKey<NistP256>),
    EcdsaP384(VerifyingKey<NistP384>),
    Ed25519(Ed25519VerifyingKey),
}

impl Verifier {
    /// Create a verifier from a PKCS#8 DER public key.
    pub fn from_pkcs8_der(algorithm: &SignAlgorithm, der: &[u8]) -> Result<Self, CryptoError> {
        match algorithm {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let key = RsaPublicKey::from_public_key_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPkcs1v15 { key, hash: *hash })
            }
            SignAlgorithm::RsaPss { hash } => {
                let key = RsaPublicKey::from_public_key_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPss { key, hash: *hash })
            }
            SignAlgorithm::EcdsaP256 => {
                let key = VerifyingKey::<NistP256>::from_public_key_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP256(key))
            }
            SignAlgorithm::EcdsaP384 => {
                let key = VerifyingKey::<NistP384>::from_public_key_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP384(key))
            }
            SignAlgorithm::Ed25519 => {
                let key = Ed25519VerifyingKey::from_public_key_der(der)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::Ed25519(key))
            }
        }
    }

    /// Create a verifier from a PKCS#8 PEM public key.
    pub fn from_pkcs8_pem(algorithm: &SignAlgorithm, pem: &str) -> Result<Self, CryptoError> {
        match algorithm {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let key = RsaPublicKey::from_public_key_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPkcs1v15 { key, hash: *hash })
            }
            SignAlgorithm::RsaPss { hash } => {
                let key = RsaPublicKey::from_public_key_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::RsaPss { key, hash: *hash })
            }
            SignAlgorithm::EcdsaP256 => {
                let key = VerifyingKey::<NistP256>::from_public_key_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP256(key))
            }
            SignAlgorithm::EcdsaP384 => {
                let key = VerifyingKey::<NistP384>::from_public_key_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::EcdsaP384(key))
            }
            SignAlgorithm::Ed25519 => {
                let key = Ed25519VerifyingKey::from_public_key_pem(pem)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                Ok(Self::Ed25519(key))
            }
        }
    }

    /// Verify a signature against data.
    pub fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        format: SignatureFormat,
    ) -> Result<bool, CryptoError> {
        use rsa::signature::Verifier as RsaVerifier;

        match self {
            Self::RsaPkcs1v15 { key, hash } => {
                match hash {
                    RsaHash::Sha256 => {
                        let verifying_key =
                            rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key.clone());
                        let sig = rsa::pkcs1v15::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                    RsaHash::Sha384 => {
                        let verifying_key =
                            rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key.clone());
                        let sig = rsa::pkcs1v15::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                    RsaHash::Sha512 => {
                        let verifying_key =
                            rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key.clone());
                        let sig = rsa::pkcs1v15::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                }
            }
            Self::RsaPss { key, hash } => {
                match hash {
                    RsaHash::Sha256 => {
                        let verifying_key =
                            rsa::pss::VerifyingKey::<sha2::Sha256>::new(key.clone());
                        let sig = rsa::pss::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                    RsaHash::Sha384 => {
                        let verifying_key =
                            rsa::pss::VerifyingKey::<sha2::Sha384>::new(key.clone());
                        let sig = rsa::pss::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                    RsaHash::Sha512 => {
                        let verifying_key =
                            rsa::pss::VerifyingKey::<sha2::Sha512>::new(key.clone());
                        let sig = rsa::pss::Signature::try_from(signature)
                            .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                        Ok(verifying_key.verify(data, &sig).is_ok())
                    }
                }
            }
            Self::EcdsaP256(key) => {
                let sig = parse_p256_signature(signature, format)?;
                Ok(key.verify(data, &sig).is_ok())
            }
            Self::EcdsaP384(key) => {
                let sig = parse_p384_signature(signature, format)?;
                Ok(key.verify(data, &sig).is_ok())
            }
            Self::Ed25519(key) => {
                let sig = ed25519_dalek::Signature::from_slice(signature)
                    .map_err(|e| CryptoError::VerificationError(e.to_string()))?;
                Ok(key.verify(data, &sig).is_ok())
            }
        }
    }
}

fn parse_p256_signature(
    signature: &[u8],
    format: SignatureFormat,
) -> Result<ecdsa::Signature<NistP256>, CryptoError> {
    match format {
        SignatureFormat::Der => ecdsa::Signature::<NistP256>::from_der(signature)
            .map_err(|e| CryptoError::VerificationError(e.to_string())),
        SignatureFormat::Raw => ecdsa::Signature::<NistP256>::from_slice(signature)
            .map_err(|e| CryptoError::VerificationError(e.to_string())),
    }
}

fn parse_p384_signature(
    signature: &[u8],
    format: SignatureFormat,
) -> Result<ecdsa::Signature<NistP384>, CryptoError> {
    match format {
        SignatureFormat::Der => ecdsa::Signature::<NistP384>::from_der(signature)
            .map_err(|e| CryptoError::VerificationError(e.to_string())),
        SignatureFormat::Raw => ecdsa::Signature::<NistP384>::from_slice(signature)
            .map_err(|e| CryptoError::VerificationError(e.to_string())),
    }
}
