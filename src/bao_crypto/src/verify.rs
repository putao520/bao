use crate::CryptoError;
use ecdsa::signature::Verifier as VerifierTrait;
use ecdsa::VerifyingKey;
use pkcs8::DecodePrivateKey;
use rsa::pkcs1v15;
use rsa::pss;
use rsa::RsaPrivateKey;
use sha2::{Sha256, Sha384, Sha512};

use crate::sign::{RsaHash, SignAlgorithm, SignatureFormat};

enum VerifierInner {
    RsaPkcs1v15Sha256(pkcs1v15::VerifyingKey<Sha256>),
    RsaPkcs1v15Sha384(pkcs1v15::VerifyingKey<Sha384>),
    RsaPkcs1v15Sha512(pkcs1v15::VerifyingKey<Sha512>),
    RsaPssSha256(pss::VerifyingKey<Sha256>),
    RsaPssSha384(pss::VerifyingKey<Sha384>),
    RsaPssSha512(pss::VerifyingKey<Sha512>),
    EcdsaP256(VerifyingKey<p256::NistP256>),
    EcdsaP384(VerifyingKey<p384::NistP384>),
    Ed25519(ed25519_dalek::VerifyingKey),
}

pub struct Verifier {
    inner: VerifierInner,
}

fn parse_rsa_private_key_pem(pem: &str) -> Result<RsaPrivateKey, CryptoError> {
    RsaPrivateKey::from_pkcs8_pem(pem)
        .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse RSA PEM key: {}", e)))
}

fn parse_rsa_private_key_der(der: &[u8]) -> Result<RsaPrivateKey, CryptoError> {
    RsaPrivateKey::from_pkcs8_der(der)
        .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse RSA DER key: {}", e)))
}

impl Verifier {
    pub fn from_pkcs8_pem(algo: &SignAlgorithm, pem: &str) -> Result<Verifier, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { .. } | SignAlgorithm::RsaPss { .. } => {
                let rsa_key = parse_rsa_private_key_pem(pem)?;
                Verifier::from_rsa_public_key(algo, &rsa_key.to_public_key())
            }
            SignAlgorithm::EcdsaP256 => {
                let sk = ecdsa::SigningKey::<p256::NistP256>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-256 PEM key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::EcdsaP256(sk.verifying_key().clone()) })
            }
            SignAlgorithm::EcdsaP384 => {
                let sk = ecdsa::SigningKey::<p384::NistP384>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-384 PEM key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::EcdsaP384(sk.verifying_key().clone()) })
            }
            SignAlgorithm::Ed25519 => {
                let sk = ed25519_dalek::SigningKey::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse Ed25519 PEM key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::Ed25519(sk.verifying_key()) })
            }
        }
    }

    pub fn from_pkcs8_der(algo: &SignAlgorithm, der: &[u8]) -> Result<Verifier, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { .. } | SignAlgorithm::RsaPss { .. } => {
                let rsa_key = parse_rsa_private_key_der(der)?;
                Verifier::from_rsa_public_key(algo, &rsa_key.to_public_key())
            }
            SignAlgorithm::EcdsaP256 => {
                let sk = ecdsa::SigningKey::<p256::NistP256>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-256 DER key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::EcdsaP256(sk.verifying_key().clone()) })
            }
            SignAlgorithm::EcdsaP384 => {
                let sk = ecdsa::SigningKey::<p384::NistP384>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-384 DER key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::EcdsaP384(sk.verifying_key().clone()) })
            }
            SignAlgorithm::Ed25519 => {
                let sk = ed25519_dalek::SigningKey::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse Ed25519 DER key: {}", e)))?;
                Ok(Verifier { inner: VerifierInner::Ed25519(sk.verifying_key()) })
            }
        }
    }

    fn from_rsa_public_key(algo: &SignAlgorithm, pub_key: &rsa::RsaPublicKey) -> Result<Verifier, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let inner = match hash {
                    RsaHash::Sha256 => VerifierInner::RsaPkcs1v15Sha256(pkcs1v15::VerifyingKey::<Sha256>::new(pub_key.clone())),
                    RsaHash::Sha384 => VerifierInner::RsaPkcs1v15Sha384(pkcs1v15::VerifyingKey::<Sha384>::new(pub_key.clone())),
                    RsaHash::Sha512 => VerifierInner::RsaPkcs1v15Sha512(pkcs1v15::VerifyingKey::<Sha512>::new(pub_key.clone())),
                };
                Ok(Verifier { inner })
            }
            SignAlgorithm::RsaPss { hash } => {
                let inner = match hash {
                    RsaHash::Sha256 => VerifierInner::RsaPssSha256(pss::VerifyingKey::<Sha256>::new(pub_key.clone())),
                    RsaHash::Sha384 => VerifierInner::RsaPssSha384(pss::VerifyingKey::<Sha384>::new(pub_key.clone())),
                    RsaHash::Sha512 => VerifierInner::RsaPssSha512(pss::VerifyingKey::<Sha512>::new(pub_key.clone())),
                };
                Ok(Verifier { inner })
            }
            _ => Err(CryptoError::UnsupportedAlgorithm(format!("{:?}", algo))),
        }
    }

    pub fn verify(&self, data: &[u8], signature: &[u8], format: SignatureFormat) -> Result<bool, CryptoError> {
        match &self.inner {
            VerifierInner::RsaPkcs1v15Sha256(vk) => {
                let sig = rsa::pkcs1v15::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA PKCS1v15 signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::RsaPkcs1v15Sha384(vk) => {
                let sig = rsa::pkcs1v15::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA PKCS1v15 signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::RsaPkcs1v15Sha512(vk) => {
                let sig = rsa::pkcs1v15::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA PKCS1v15 signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::RsaPssSha256(vk) => {
                let sig = rsa::pss::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA-PSS signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::RsaPssSha384(vk) => {
                let sig = rsa::pss::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA-PSS signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::RsaPssSha512(vk) => {
                let sig = rsa::pss::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid RSA-PSS signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::EcdsaP256(vk) => {
                let sig = decode_p256_signature(signature, format)?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::EcdsaP384(vk) => {
                let sig = decode_p384_signature(signature, format)?;
                Ok(vk.verify(data, &sig).is_ok())
            }
            VerifierInner::Ed25519(vk) => {
                let sig = ed25519_dalek::Signature::try_from(signature)
                    .map_err(|e| CryptoError::DecodingFailed(format!("Invalid Ed25519 signature: {}", e)))?;
                Ok(vk.verify(data, &sig).is_ok())
            }
        }
    }
}

fn decode_p256_signature(
    bytes: &[u8],
    format: SignatureFormat,
) -> Result<ecdsa::Signature<p256::NistP256>, CryptoError> {
    match format {
        SignatureFormat::Der => {
            ecdsa::Signature::<p256::NistP256>::from_der(bytes)
                .map_err(|e| CryptoError::DecodingFailed(format!("Invalid DER ECDSA P256 signature: {}", e)))
        }
        SignatureFormat::Raw => {
            ecdsa::Signature::<p256::NistP256>::from_slice(bytes)
                .map_err(|e| CryptoError::DecodingFailed(format!("Invalid raw ECDSA P256 signature: {}", e)))
        }
    }
}

fn decode_p384_signature(
    bytes: &[u8],
    format: SignatureFormat,
) -> Result<ecdsa::Signature<p384::NistP384>, CryptoError> {
    match format {
        SignatureFormat::Der => {
            ecdsa::Signature::<p384::NistP384>::from_der(bytes)
                .map_err(|e| CryptoError::DecodingFailed(format!("Invalid DER ECDSA P384 signature: {}", e)))
        }
        SignatureFormat::Raw => {
            ecdsa::Signature::<p384::NistP384>::from_slice(bytes)
                .map_err(|e| CryptoError::DecodingFailed(format!("Invalid raw ECDSA P384 signature: {}", e)))
        }
    }
}
