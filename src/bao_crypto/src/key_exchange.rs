use crate::CryptoError;
use p256::elliptic_curve::rand_core::OsRng;
use p256::EncodedPoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcCurve {
    P256,
    P384,
    X25519,
}

pub fn parse_curve(name: &str) -> Result<EcCurve, CryptoError> {
    match name.to_lowercase().as_str() {
        "p256" | "prime256v1" | "secp256r1" => Ok(EcCurve::P256),
        "p384" | "secp384r1" => Ok(EcCurve::P384),
        "x25519" => Ok(EcCurve::X25519),
        _ => Err(CryptoError::InvalidCurve(format!(
            "Unsupported curve: {}",
            name
        ))),
    }
}

pub struct EcdhKeyPair {
    curve: EcCurve,
    private_bytes: Vec<u8>,
    public_bytes: Vec<u8>,
}

impl EcdhKeyPair {
    pub fn generate(curve: EcCurve) -> Result<EcdhKeyPair, CryptoError> {
        match curve {
            EcCurve::P256 => {
                let secret = p256::SecretKey::random(&mut OsRng);
                let public = secret.public_key();
                let private_bytes = secret.to_bytes().to_vec();
                let public_bytes = EncodedPoint::from(public).as_bytes().to_vec();
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes,
                    public_bytes,
                })
            }
            EcCurve::P384 => {
                let secret = p384::SecretKey::random(&mut OsRng);
                let public = secret.public_key();
                let private_bytes = secret.to_bytes().to_vec();
                let public_bytes = p384::EncodedPoint::from(public).as_bytes().to_vec();
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes,
                    public_bytes,
                })
            }
            EcCurve::X25519 => {
                let secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
                let public = x25519_dalek::PublicKey::from(&secret);
                let private_bytes = secret.to_bytes().to_vec();
                let public_bytes = public.to_bytes().to_vec();
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes,
                    public_bytes,
                })
            }
        }
    }

    pub fn reconstruct_keypair(
        curve: EcCurve,
        private_bytes: &[u8],
    ) -> Result<EcdhKeyPair, CryptoError> {
        match curve {
            EcCurve::P256 => {
                if private_bytes.len() != 32 {
                    return Err(CryptoError::InvalidKeyLength {
                        expected: 32,
                        got: private_bytes.len(),
                    });
                }
                let secret = p256::SecretKey::from_slice(private_bytes)
                    .map_err(|e| CryptoError::InvalidKey(format!("P256: {}", e)))?;
                let public = secret.public_key();
                let public_bytes = EncodedPoint::from(public).as_bytes().to_vec();
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes: private_bytes.to_vec(),
                    public_bytes,
                })
            }
            EcCurve::P384 => {
                if private_bytes.len() != 48 {
                    return Err(CryptoError::InvalidKeyLength {
                        expected: 48,
                        got: private_bytes.len(),
                    });
                }
                let secret = p384::SecretKey::from_slice(private_bytes)
                    .map_err(|e| CryptoError::InvalidKey(format!("P384: {}", e)))?;
                let public = secret.public_key();
                let public_bytes = p384::EncodedPoint::from(public).as_bytes().to_vec();
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes: private_bytes.to_vec(),
                    public_bytes,
                })
            }
            EcCurve::X25519 => {
                let bytes: [u8; 32] = private_bytes
                    .try_into()
                    .map_err(|_| CryptoError::InvalidKeyLength {
                        expected: 32,
                        got: private_bytes.len(),
                    })?;
                let secret = x25519_dalek::StaticSecret::from(bytes);
                let public = x25519_dalek::PublicKey::from(&secret);
                Ok(EcdhKeyPair {
                    curve,
                    private_bytes: private_bytes.to_vec(),
                    public_bytes: public.to_bytes().to_vec(),
                })
            }
        }
    }

    pub fn compute_shared_secret(&self, other_pub: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match self.curve {
            EcCurve::P256 => {
                let other_point = p256::PublicKey::from_sec1_bytes(other_pub)
                    .map_err(|e| CryptoError::InvalidKey(format!("P256 public: {}", e)))?;
                let secret = p256::SecretKey::from_slice(&self.private_bytes)
                    .map_err(|e| CryptoError::InvalidKey(format!("P256 private: {}", e)))?;
                let shared = p256::elliptic_curve::ecdh::diffie_hellman(
                    secret.to_nonzero_scalar(),
                    other_point.as_affine(),
                );
                Ok(shared.raw_secret_bytes().to_vec())
            }
            EcCurve::P384 => {
                let other_point = p384::PublicKey::from_sec1_bytes(other_pub)
                    .map_err(|e| CryptoError::InvalidKey(format!("P384 public: {}", e)))?;
                let secret = p384::SecretKey::from_slice(&self.private_bytes)
                    .map_err(|e| CryptoError::InvalidKey(format!("P384 private: {}", e)))?;
                let shared = p384::elliptic_curve::ecdh::diffie_hellman(
                    secret.to_nonzero_scalar(),
                    other_point.as_affine(),
                );
                Ok(shared.raw_secret_bytes().to_vec())
            }
            EcCurve::X25519 => {
                let priv_bytes: [u8; 32] = self.private_bytes.as_slice().try_into().map_err(
                    |_| CryptoError::InvalidKeyLength {
                        expected: 32,
                        got: self.private_bytes.len(),
                    },
                )?;
                let pub_bytes: [u8; 32] = other_pub.try_into().map_err(|_| {
                    CryptoError::InvalidKeyLength {
                        expected: 32,
                        got: other_pub.len(),
                    }
                })?;
                let secret = x25519_dalek::StaticSecret::from(priv_bytes);
                let other_public = x25519_dalek::PublicKey::from(pub_bytes);
                let shared = secret.diffie_hellman(&other_public);
                Ok(shared.as_bytes().to_vec())
            }
        }
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.public_bytes.clone()
    }

    pub fn private_key_bytes(&self) -> Vec<u8> {
        self.private_bytes.clone()
    }
}
