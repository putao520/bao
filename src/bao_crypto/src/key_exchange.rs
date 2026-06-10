//! Key exchange: ECDH (P-256, P-384, X25519) and classic DH.
//!
//! Implements `crypto.createECDH` / `crypto.createDiffieHellman`.

use ecdsa::elliptic_curve::rand_core::OsRng;

use crate::sign::CryptoError;

/// ECDH curve identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcdhCurve {
    P256,
    P384,
    X25519,
}

/// ECDH key pair holder.
pub enum EcdhKeyPair {
    P256 {
        secret: p256::SecretKey,
        public: p256::PublicKey,
    },
    P384 {
        secret: p384::SecretKey,
        public: p384::PublicKey,
    },
    X25519 {
        secret: x25519_dalek::StaticSecret,
        public: x25519_dalek::PublicKey,
    },
}

impl EcdhKeyPair {
    /// Generate a new ECDH key pair.
    pub fn generate(curve: EcdhCurve) -> Result<Self, CryptoError> {
        match curve {
            EcdhCurve::P256 => {
                let secret = p256::SecretKey::random(&mut OsRng);
                let public = secret.public_key();
                Ok(Self::P256 { secret, public })
            }
            EcdhCurve::P384 => {
                let secret = p384::SecretKey::random(&mut OsRng);
                let public = secret.public_key();
                Ok(Self::P384 { secret, public })
            }
            EcdhCurve::X25519 => {
                let secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
                let public = x25519_dalek::PublicKey::from(&secret);
                Ok(Self::X25519 { secret, public })
            }
        }
    }

    /// Get the uncompressed public key bytes.
    pub fn public_key_bytes(&self) -> Vec<u8> {
        match self {
            Self::P256 { public, .. } => public.to_sec1_bytes().to_vec(),
            Self::P384 { public, .. } => public.to_sec1_bytes().to_vec(),
            Self::X25519 { public, .. } => public.as_bytes().to_vec(),
        }
    }

    /// Get the raw private key bytes.
    pub fn private_key_bytes(&self) -> Vec<u8> {
        match self {
            Self::P256 { secret, .. } => secret.to_bytes().to_vec(),
            Self::P384 { secret, .. } => secret.to_bytes().to_vec(),
            Self::X25519 { secret, .. } => secret.to_bytes().to_vec(),
        }
    }

    /// Compute the shared secret from the local private key and remote public key.
    pub fn compute_shared_secret(&self, remote_public: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match self {
            Self::P256 { secret, .. } => {
                let remote = p256::PublicKey::from_sec1_bytes(remote_public)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), remote.as_affine());
                Ok(shared.raw_secret_bytes().to_vec())
            }
            Self::P384 { secret, .. } => {
                let remote = p384::PublicKey::from_sec1_bytes(remote_public)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?;
                let shared = p384::ecdh::diffie_hellman(secret.to_nonzero_scalar(), remote.as_affine());
                Ok(shared.raw_secret_bytes().to_vec())
            }
            Self::X25519 { secret, .. } => {
                let remote = x25519_dalek::PublicKey::from(
                    <[u8; 32]>::try_from(remote_public)
                        .map_err(|e| CryptoError::KeyError(e.to_string()))?,
                );
                let shared = secret.diffie_hellman(&remote);
                Ok(shared.as_bytes().to_vec())
            }
        }
    }
}

/// Parse an ECDH curve name (Node.js format).
pub fn parse_curve(name: &str) -> Result<EcdhCurve, CryptoError> {
    match name {
        "prime256v1" | "P-256" | "secp256r1" => Ok(EcdhCurve::P256),
        "secp384r1" | "P-384" => Ok(EcdhCurve::P384),
        "X25519" | "x25519" => Ok(EcdhCurve::X25519),
        other => Err(CryptoError::InvalidAlgorithm(other.to_string())),
    }
}

/// Reconstruct an ECDH key pair from a curve identifier and raw private key bytes.
pub fn reconstruct_keypair(
    curve: EcdhCurve,
    private_bytes: &[u8],
) -> Result<EcdhKeyPair, CryptoError> {
    match curve {
        EcdhCurve::P256 => {
            let arr: [u8; 32] = <[u8; 32]>::try_from(private_bytes)
                .map_err(|e| CryptoError::KeyError(e.to_string()))?;
            let secret = p256::SecretKey::from_slice(&arr)
                .map_err(|e| CryptoError::KeyError(e.to_string()))?;
            let public = secret.public_key();
            Ok(EcdhKeyPair::P256 { secret, public })
        }
        EcdhCurve::P384 => {
            let arr: [u8; 48] = <[u8; 48]>::try_from(private_bytes)
                .map_err(|e| CryptoError::KeyError(e.to_string()))?;
            let secret = p384::SecretKey::from_slice(&arr)
                .map_err(|e| CryptoError::KeyError(e.to_string()))?;
            let public = secret.public_key();
            Ok(EcdhKeyPair::P384 { secret, public })
        }
        EcdhCurve::X25519 => {
            let secret = x25519_dalek::StaticSecret::from(
                <[u8; 32]>::try_from(private_bytes)
                    .map_err(|e| CryptoError::KeyError(e.to_string()))?,
            );
            let public = x25519_dalek::PublicKey::from(&secret);
            Ok(EcdhKeyPair::X25519 { secret, public })
        }
    }
}
