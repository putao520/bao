//! Signing key provider for rustls CryptoProvider.
//!
//! Implements `rustls::crypto::KeyProvider` and `rustls::crypto::signer::SigningKey`
//! using RustCrypto backends (RSA, ECDSA, Ed25519).

use std::fmt;
use std::sync::Arc;

use pkcs8::DecodePrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rustls::crypto::KeyProvider;
use rustls::pki_types::PrivateKeyDer;
use rustls::sign::{Signer, SigningKey};
use rustls::Error;

#[derive(Debug)]
pub(crate) struct BaoKeyProvider;

impl KeyProvider for BaoKeyProvider {
    fn load_private_key(
        &self,
        key_der: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn SigningKey>, Error> {
        match key_der {
            PrivateKeyDer::Pkcs8(item) => load_pkcs8(&item.secret_pkcs8_der()),
            PrivateKeyDer::Pkcs1(item) => load_pkcs1(&item.secret_pkcs1_der()),
            _ => Err(Error::General("unsupported private key format".into())),
        }
    }
}

fn load_pkcs8(der: &[u8]) -> Result<Arc<dyn SigningKey>, Error> {
    // Try Ed25519 first
    if let Ok(key) = ed25519_dalek::SigningKey::from_pkcs8_der(der) {
        return Ok(Arc::new(Ed25519Signer { key }));
    }
    // Try ECDSA P-256
    if let Ok(key) = ecdsa::SigningKey::<p256::NistP256>::from_pkcs8_der(der) {
        return Ok(Arc::new(EcdsaP256Signer { key }));
    }
    // Try ECDSA P-384
    if let Ok(key) = ecdsa::SigningKey::<p384::NistP384>::from_pkcs8_der(der) {
        return Ok(Arc::new(EcdsaP384Signer { key }));
    }
    // Try RSA
    if let Ok(key) = rsa::RsaPrivateKey::from_pkcs8_der(der) {
        return Ok(Arc::new(RsaSigner { key }));
    }
    Err(Error::General("no matching key type for PKCS#8 DER".into()))
}

fn load_pkcs1(der: &[u8]) -> Result<Arc<dyn SigningKey>, Error> {
    let key = rsa::RsaPrivateKey::from_pkcs1_der(der)
        .map_err(|e| Error::General(format!("RSA PKCS#1 parse error: {e}")))?;
    Ok(Arc::new(RsaSigner { key }))
}

// ─── RSA signer ─────────────────────────────────────────────────────

struct RsaSigner {
    key: rsa::RsaPrivateKey,
}

impl fmt::Debug for RsaSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RsaSigner").finish_non_exhaustive()
    }
}

impl SigningKey for RsaSigner {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn Signer>> {
        use rustls::SignatureScheme::*;
        for scheme in offered {
            match scheme {
                RSA_PKCS1_SHA256 => {
                    return Some(Box::new(RsaPkcs1Signer {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                RSA_PKCS1_SHA384 => {
                    return Some(Box::new(RsaPkcs1Signer {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                RSA_PKCS1_SHA512 => {
                    return Some(Box::new(RsaPkcs1Signer {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                RSA_PSS_SHA256 => {
                    return Some(Box::new(RsaPssSigner {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                RSA_PSS_SHA384 => {
                    return Some(Box::new(RsaPssSigner {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                RSA_PSS_SHA512 => {
                    return Some(Box::new(RsaPssSigner {
                        key: self.key.clone(),
                        scheme: *scheme,
                    }));
                }
                _ => continue,
            }
        }
        None
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::RSA
    }
}

struct RsaPkcs1Signer {
    key: rsa::RsaPrivateKey,
    scheme: rustls::SignatureScheme,
}

impl fmt::Debug for RsaPkcs1Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RsaPkcs1Signer")
            .field("scheme", &self.scheme)
            .finish_non_exhaustive()
    }
}

impl Signer for RsaPkcs1Signer {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        use rsa::signature::RandomizedSigner;
        let mut rng = rand_core::OsRng;
        match self.scheme {
            rustls::SignatureScheme::RSA_PKCS1_SHA256 => {
                let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            rustls::SignatureScheme::RSA_PKCS1_SHA384 => {
                let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha384>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            rustls::SignatureScheme::RSA_PKCS1_SHA512 => {
                let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            _ => Err(Error::General("unsupported RSA PKCS1 scheme".into())),
        }
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        self.scheme
    }
}

struct RsaPssSigner {
    key: rsa::RsaPrivateKey,
    scheme: rustls::SignatureScheme,
}

impl fmt::Debug for RsaPssSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RsaPssSigner")
            .field("scheme", &self.scheme)
            .finish_non_exhaustive()
    }
}

impl Signer for RsaPssSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        use rsa::signature::RandomizedSigner;
        let mut rng = rand_core::OsRng;
        match self.scheme {
            rustls::SignatureScheme::RSA_PSS_SHA256 => {
                let signing_key = rsa::pss::SigningKey::<sha2::Sha256>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            rustls::SignatureScheme::RSA_PSS_SHA384 => {
                let signing_key = rsa::pss::SigningKey::<sha2::Sha384>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            rustls::SignatureScheme::RSA_PSS_SHA512 => {
                let signing_key = rsa::pss::SigningKey::<sha2::Sha512>::new(self.key.clone());
                let sig = signing_key.sign_with_rng(&mut rng, message);
                let bytes: Box<[u8]> = sig.into();
                Ok(bytes.into_vec())
            }
            _ => Err(Error::General("unsupported RSA PSS scheme".into())),
        }
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        self.scheme
    }
}

// ─── ECDSA P-256 signer ─────────────────────────────────────────────

struct EcdsaP256Signer {
    key: ecdsa::SigningKey<p256::NistP256>,
}

impl fmt::Debug for EcdsaP256Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EcdsaP256Signer").finish_non_exhaustive()
    }
}

impl SigningKey for EcdsaP256Signer {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn Signer>> {
        if offered.contains(&rustls::SignatureScheme::ECDSA_NISTP256_SHA256) {
            Some(Box::new(EcdsaP256SignerInner { key: self.key.clone() }))
        } else {
            None
        }
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ECDSA
    }
}

struct EcdsaP256SignerInner {
    key: ecdsa::SigningKey<p256::NistP256>,
}

impl fmt::Debug for EcdsaP256SignerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EcdsaP256SignerInner").finish_non_exhaustive()
    }
}

impl Signer for EcdsaP256SignerInner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let sig: ecdsa::Signature<p256::NistP256> = ecdsa::signature::Signer::sign(&self.key, message);
        Ok(sig.to_der().as_bytes().to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ECDSA_NISTP256_SHA256
    }
}

// ─── ECDSA P-384 signer ─────────────────────────────────────────────

struct EcdsaP384Signer {
    key: ecdsa::SigningKey<p384::NistP384>,
}

impl fmt::Debug for EcdsaP384Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EcdsaP384Signer").finish_non_exhaustive()
    }
}

impl SigningKey for EcdsaP384Signer {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn Signer>> {
        if offered.contains(&rustls::SignatureScheme::ECDSA_NISTP384_SHA384) {
            Some(Box::new(EcdsaP384SignerInner { key: self.key.clone() }))
        } else {
            None
        }
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ECDSA
    }
}

struct EcdsaP384SignerInner {
    key: ecdsa::SigningKey<p384::NistP384>,
}

impl fmt::Debug for EcdsaP384SignerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EcdsaP384SignerInner").finish_non_exhaustive()
    }
}

impl Signer for EcdsaP384SignerInner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let sig: ecdsa::Signature<p384::NistP384> = ecdsa::signature::Signer::sign(&self.key, message);
        Ok(sig.to_der().as_bytes().to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ECDSA_NISTP384_SHA384
    }
}

// ─── Ed25519 signer ─────────────────────────────────────────────────

struct Ed25519Signer {
    key: ed25519_dalek::SigningKey,
}

impl fmt::Debug for Ed25519Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ed25519Signer").finish_non_exhaustive()
    }
}

impl SigningKey for Ed25519Signer {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn Signer>> {
        if offered.contains(&rustls::SignatureScheme::ED25519) {
            Some(Box::new(Ed25519SignerInner { key: self.key.clone() }))
        } else {
            None
        }
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ED25519
    }
}

struct Ed25519SignerInner {
    key: ed25519_dalek::SigningKey,
}

impl fmt::Debug for Ed25519SignerInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ed25519SignerInner").finish_non_exhaustive()
    }
}

impl Signer for Ed25519SignerInner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let sig = ed25519_dalek::Signer::sign(&self.key, message);
        Ok(sig.to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ED25519
    }
}
