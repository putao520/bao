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

/// Declare an RAII free-guard for a raw BoringSSL pointer type.
///
/// Every BoringSSL object this crate owns needs the same teardown: free the
/// pointer on drop unless it is null. The resource type and its free function
/// vary per object, so the shared body lives here once instead of being
/// copy-pasted as a per-type `impl Drop`. Expands to a private tuple newtype
/// with a `Drop` impl that calls `$free` on the non-null inner pointer.
macro_rules! bssl_guard {
    ($name:ident, $ty:ty, $free:path) => {
        struct $name(*mut $ty);
        impl Drop for $name {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe { $free(self.0) };
                }
            }
        }
    };
}

/// Free a BoringSSL `EVP_PKEY` unless the pointer is null.
///
/// Shared teardown for the owning types whose Drop needs to release an
/// `EVP_PKEY` (`Signer` in [`sign`], `Verifier` in [`verify`]).
#[inline]
pub(crate) fn free_pkey_if_non_null(pkey: *mut bun_boringssl_sys::EVP_PKEY) {
    if !pkey.is_null() {
        unsafe { bun_boringssl_sys::EVP_PKEY_free(pkey) };
    }
}

pub mod certificate;
pub mod cipher;
pub mod dh;
pub mod kdf;
pub mod key_exchange;
pub mod keypair;
pub mod random;
pub mod sign;
pub mod verify;
