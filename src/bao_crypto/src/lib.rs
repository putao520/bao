use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid key length: expected {expected}, got {got}")]
    InvalidKeyLength { expected: usize, got: usize },
    #[error("Invalid nonce length: expected {expected}, got {got}")]
    InvalidNonceLength { expected: usize, got: usize },
    #[error("Invalid key: {0}")]
    InvalidKey(String),
    #[error("Invalid curve: {0}")]
    InvalidCurve(String),
    #[error("Invalid certificate: {0}")]
    InvalidCertificate(String),
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),
    #[error("Invalid length: {0}")]
    InvalidLength(String),
    #[error("Invalid format: {0}")]
    InvalidFormat(String),
    #[error("Unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Signing failed: {0}")]
    SignFailed(String),
    #[error("Verification failed: {0}")]
    VerifyFailed(String),
    #[error("Encoding failed: {0}")]
    EncodingFailed(String),
    #[error("Decoding failed: {0}")]
    DecodingFailed(String),
    #[error("Certificate error: {0}")]
    CertificateError(String),
    #[error("KDF error: {0}")]
    KdfError(String),
    #[error("Key exchange error: {0}")]
    KeyExchangeError(String),
    #[error("Key pair generation error: {0}")]
    KeyPairError(String),
    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),
    #[error("Shared secret computation failed: {0}")]
    SharedSecretFailed(String),
    #[error("Random generation failed: {0}")]
    RandomFailed(String),
    #[error("Operation failed: {0}")]
    OperationFailed(String),
}

pub mod sign;
pub mod verify;
pub mod cipher;
pub mod key_exchange;
pub mod keypair;
pub mod certificate;
pub mod kdf;
pub mod random;
pub mod dh;
