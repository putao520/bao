use crate::CryptoError;
use ecdsa::signature::Signer as SignerTrait;
use ecdsa::SigningKey;
use pkcs8::DecodePrivateKey;
use rsa::pkcs1v15;
use rsa::pss;
use rsa::RsaPrivateKey;
use sha2::{Sha256, Sha384, Sha512};
use signature::RandomizedSigner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsaHash {
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignAlgorithm {
    RsaPkcs1v15 { hash: RsaHash },
    RsaPss { hash: RsaHash },
    EcdsaP256,
    EcdsaP384,
    Ed25519,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureFormat {
    Der,
    Raw,
}

enum SignerInner {
    RsaPkcs1v15Sha256(pkcs1v15::SigningKey<Sha256>),
    RsaPkcs1v15Sha384(pkcs1v15::SigningKey<Sha384>),
    RsaPkcs1v15Sha512(pkcs1v15::SigningKey<Sha512>),
    RsaPssSha256(pss::SigningKey<Sha256>),
    RsaPssSha384(pss::SigningKey<Sha384>),
    RsaPssSha512(pss::SigningKey<Sha512>),
    EcdsaP256(SigningKey<p256::NistP256>),
    EcdsaP384(SigningKey<p384::NistP384>),
    Ed25519(ed25519_dalek::SigningKey),
}

pub struct Signer {
    inner: SignerInner,
}

fn parse_rsa_private_key_pem(pem: &str) -> Result<RsaPrivateKey, CryptoError> {
    RsaPrivateKey::from_pkcs8_pem(pem)
        .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse RSA PEM key: {}", e)))
}

fn parse_rsa_private_key_der(der: &[u8]) -> Result<RsaPrivateKey, CryptoError> {
    RsaPrivateKey::from_pkcs8_der(der)
        .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse RSA DER key: {}", e)))
}

impl Signer {
    pub fn from_pkcs8_pem(algo: &SignAlgorithm, pem: &str) -> Result<Signer, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { .. } | SignAlgorithm::RsaPss { .. } => {
                let rsa_key = parse_rsa_private_key_pem(pem)?;
                Signer::from_rsa_key(algo, &rsa_key)
            }
            SignAlgorithm::EcdsaP256 => {
                let sk = SigningKey::<p256::NistP256>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-256 PEM key: {}", e)))?;
                Ok(Signer { inner: SignerInner::EcdsaP256(sk) })
            }
            SignAlgorithm::EcdsaP384 => {
                let sk = SigningKey::<p384::NistP384>::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-384 PEM key: {}", e)))?;
                Ok(Signer { inner: SignerInner::EcdsaP384(sk) })
            }
            SignAlgorithm::Ed25519 => {
                let sk = ed25519_dalek::SigningKey::from_pkcs8_pem(pem)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse Ed25519 PEM key: {}", e)))?;
                Ok(Signer { inner: SignerInner::Ed25519(sk) })
            }
        }
    }

    pub fn from_pkcs8_der(algo: &SignAlgorithm, der: &[u8]) -> Result<Signer, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { .. } | SignAlgorithm::RsaPss { .. } => {
                let rsa_key = parse_rsa_private_key_der(der)?;
                Signer::from_rsa_key(algo, &rsa_key)
            }
            SignAlgorithm::EcdsaP256 => {
                let sk = SigningKey::<p256::NistP256>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-256 DER key: {}", e)))?;
                Ok(Signer { inner: SignerInner::EcdsaP256(sk) })
            }
            SignAlgorithm::EcdsaP384 => {
                let sk = SigningKey::<p384::NistP384>::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse ECDSA P-384 DER key: {}", e)))?;
                Ok(Signer { inner: SignerInner::EcdsaP384(sk) })
            }
            SignAlgorithm::Ed25519 => {
                let sk = ed25519_dalek::SigningKey::from_pkcs8_der(der)
                    .map_err(|e| CryptoError::InvalidKey(format!("Failed to parse Ed25519 DER key: {}", e)))?;
                Ok(Signer { inner: SignerInner::Ed25519(sk) })
            }
        }
    }

    fn from_rsa_key(algo: &SignAlgorithm, rsa_key: &RsaPrivateKey) -> Result<Signer, CryptoError> {
        match algo {
            SignAlgorithm::RsaPkcs1v15 { hash } => {
                let inner = match hash {
                    RsaHash::Sha256 => SignerInner::RsaPkcs1v15Sha256(pkcs1v15::SigningKey::<Sha256>::new(rsa_key.clone())),
                    RsaHash::Sha384 => SignerInner::RsaPkcs1v15Sha384(pkcs1v15::SigningKey::<Sha384>::new(rsa_key.clone())),
                    RsaHash::Sha512 => SignerInner::RsaPkcs1v15Sha512(pkcs1v15::SigningKey::<Sha512>::new(rsa_key.clone())),
                };
                Ok(Signer { inner })
            }
            SignAlgorithm::RsaPss { hash } => {
                let inner = match hash {
                    RsaHash::Sha256 => SignerInner::RsaPssSha256(pss::SigningKey::<Sha256>::new(rsa_key.clone())),
                    RsaHash::Sha384 => SignerInner::RsaPssSha384(pss::SigningKey::<Sha384>::new(rsa_key.clone())),
                    RsaHash::Sha512 => SignerInner::RsaPssSha512(pss::SigningKey::<Sha512>::new(rsa_key.clone())),
                };
                Ok(Signer { inner })
            }
            _ => Err(CryptoError::UnsupportedAlgorithm(format!("{:?}", algo))),
        }
    }

    pub fn sign(&self, data: &[u8], format: SignatureFormat) -> Result<Vec<u8>, CryptoError> {
        use rand_core::OsRng;
        match &self.inner {
            SignerInner::RsaPkcs1v15Sha256(sk) => {
                let sig: rsa::pkcs1v15::Signature = sk.sign(data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::RsaPkcs1v15Sha384(sk) => {
                let sig: rsa::pkcs1v15::Signature = sk.sign(data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::RsaPkcs1v15Sha512(sk) => {
                let sig: rsa::pkcs1v15::Signature = sk.sign(data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::RsaPssSha256(sk) => {
                let sig: rsa::pss::Signature = sk.sign_with_rng(&mut OsRng, data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::RsaPssSha384(sk) => {
                let sig: rsa::pss::Signature = sk.sign_with_rng(&mut OsRng, data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::RsaPssSha512(sk) => {
                let sig: rsa::pss::Signature = sk.sign_with_rng(&mut OsRng, data);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.to_vec())
            }
            SignerInner::EcdsaP256(sk) => {
                let sig: ecdsa::Signature<p256::NistP256> = sk.sign(data);
                Ok(encode_ecdsa_signature(EcdsaSignature::P256(&sig), format))
            }
            SignerInner::EcdsaP384(sk) => {
                let sig: ecdsa::Signature<p384::NistP384> = sk.sign(data);
                Ok(encode_ecdsa_signature(EcdsaSignature::P384(&sig), format))
            }
            SignerInner::Ed25519(sk) => {
                let sig = sk.sign(data);
                Ok(sig.to_bytes().to_vec())
            }
        }
    }
}

enum EcdsaSignature<'a> {
    P256(&'a ecdsa::Signature<p256::NistP256>),
    P384(&'a ecdsa::Signature<p384::NistP384>),
}

fn encode_ecdsa_signature(sig: EcdsaSignature<'_>, format: SignatureFormat) -> Vec<u8> {
    match format {
        SignatureFormat::Der => match sig {
            EcdsaSignature::P256(s) => s.to_der().as_bytes().to_vec(),
            EcdsaSignature::P384(s) => s.to_der().as_bytes().to_vec(),
        },
        SignatureFormat::Raw => match sig {
            EcdsaSignature::P256(s) => s.to_bytes().to_vec(),
            EcdsaSignature::P384(s) => s.to_bytes().to_vec(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::EncodePrivateKey;

    fn generate_rsa_key() -> RsaPrivateKey {
        use rand_core::OsRng;
        RsaPrivateKey::new(&mut OsRng, 2048).unwrap()
    }

    #[test]
    fn rsa_pkcs1v15_sha256_sign_verify_roundtrip() {
        let rsa_key = generate_rsa_key();
        let pem = rsa_key.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::RsaPkcs1v15 { hash: RsaHash::Sha256 };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let data = b"hello rsa-pkcs1v15-sha256";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn rsa_pss_sha256_sign_verify_roundtrip() {
        let rsa_key = generate_rsa_key();
        let pem = rsa_key.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::RsaPss { hash: RsaHash::Sha256 };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let data = b"hello rsa-pss-sha256";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn ecdsa_p256_sign_verify_roundtrip() {
        use p256::ecdsa::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::random(&mut OsRng);
        let pem = sk.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::EcdsaP256;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ecdsa-p256";
        let sig_der = signer.sign(data, SignatureFormat::Der).unwrap();
        let sig_raw = signer.sign(data, SignatureFormat::Raw).unwrap();
        assert!(!sig_der.is_empty());
        assert!(!sig_raw.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig_der, SignatureFormat::Der).unwrap());
        assert!(verifier.verify(data, &sig_raw, SignatureFormat::Raw).unwrap());
    }

    #[test]
    fn ecdsa_p384_sign_verify_roundtrip() {
        use p384::ecdsa::SigningKey;
        use rand_core::OsRng;
        let sk = SigningKey::random(&mut OsRng);
        let pem = sk.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::EcdsaP384;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ecdsa-p384";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        use ed25519_dalek::SigningKey;
        use rand_core::{OsRng, RngCore};
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let sk = SigningKey::from_bytes(&bytes);
        let pem = sk.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::Ed25519;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ed25519";
        let sig = signer.sign(data, SignatureFormat::Raw).unwrap();
        assert_eq!(sig.len(), 64);

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Raw).unwrap());
    }

    #[test]
    fn sign_wrong_data_fails_verification() {
        let rsa_key = generate_rsa_key();
        let pem = rsa_key.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
        let algo = SignAlgorithm::RsaPkcs1v15 { hash: RsaHash::Sha256 };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let sig = signer.sign(b"original data", SignatureFormat::Der).unwrap();

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(!verifier.verify(b"tampered data", &sig, SignatureFormat::Der).unwrap());
    }
}
