//! Symmetric AEAD cipher operations (AES-GCM, ChaCha20-Poly1305).
//!
//! Implements `crypto.createCipheriv` / `crypto.createDecipheriv` for Node.js.

use aes_gcm::{Aes128Gcm, Aes256Gcm, KeyInit, Nonce, aead::Aead};
use chacha20poly1305::ChaCha20Poly1305;

use crate::sign::CryptoError;

/// Supported AEAD cipher algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlgorithm {
    Aes128Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

/// AEAD cipher result containing ciphertext + auth tag.
#[derive(Debug, Clone)]
pub struct CipherResult {
    pub ciphertext: Vec<u8>,
    pub auth_tag: Vec<u8>,
}

/// Encrypt data with an AEAD cipher.
pub fn encrypt(
    algorithm: CipherAlgorithm,
    key: &[u8],
    iv: &[u8],
    aad: Option<&[u8]>,
    plaintext: &[u8],
) -> Result<CipherResult, CryptoError> {
    match algorithm {
        CipherAlgorithm::Aes128Gcm => {
            let cipher = Aes128Gcm::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = Nonce::from_slice(iv);
            let mut payload = aes_gcm::aead::Payload::from(plaintext);
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            let ciphertext_with_tag = cipher
                .encrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            // AES-GCM appends 16-byte tag to ciphertext
            let (ct, tag) = ciphertext_with_tag.split_at(ciphertext_with_tag.len() - 16);
            Ok(CipherResult {
                ciphertext: ct.to_vec(),
                auth_tag: tag.to_vec(),
            })
        }
        CipherAlgorithm::Aes256Gcm => {
            let cipher = Aes256Gcm::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = Nonce::from_slice(iv);
            let mut payload = aes_gcm::aead::Payload::from(plaintext);
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            let ciphertext_with_tag = cipher
                .encrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let (ct, tag) = ciphertext_with_tag.split_at(ciphertext_with_tag.len() - 16);
            Ok(CipherResult {
                ciphertext: ct.to_vec(),
                auth_tag: tag.to_vec(),
            })
        }
        CipherAlgorithm::ChaCha20Poly1305 => {
            let cipher = ChaCha20Poly1305::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = chacha20poly1305::Nonce::from_slice(iv);
            let mut payload = chacha20poly1305::aead::Payload::from(plaintext);
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            let ciphertext_with_tag = cipher
                .encrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let (ct, tag) = ciphertext_with_tag.split_at(ciphertext_with_tag.len() - 16);
            Ok(CipherResult {
                ciphertext: ct.to_vec(),
                auth_tag: tag.to_vec(),
            })
        }
    }
}

/// Decrypt data with an AEAD cipher.
pub fn decrypt(
    algorithm: CipherAlgorithm,
    key: &[u8],
    iv: &[u8],
    aad: Option<&[u8]>,
    ciphertext: &[u8],
    auth_tag: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    // Concatenate ciphertext + tag for decryption API
    let mut ct_with_tag = ciphertext.to_vec();
    ct_with_tag.extend_from_slice(auth_tag);

    match algorithm {
        CipherAlgorithm::Aes128Gcm => {
            let cipher = Aes128Gcm::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = Nonce::from_slice(iv);
            let mut payload = aes_gcm::aead::Payload::from(ct_with_tag.as_slice());
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            cipher
                .decrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))
        }
        CipherAlgorithm::Aes256Gcm => {
            let cipher = Aes256Gcm::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = Nonce::from_slice(iv);
            let mut payload = aes_gcm::aead::Payload::from(ct_with_tag.as_slice());
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            cipher
                .decrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))
        }
        CipherAlgorithm::ChaCha20Poly1305 => {
            let cipher = ChaCha20Poly1305::new_from_slice(key)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            let nonce = chacha20poly1305::Nonce::from_slice(iv);
            let mut payload = chacha20poly1305::aead::Payload::from(ct_with_tag.as_slice());
            if let Some(aad_data) = aad {
                payload.aad = aad_data;
            }
            cipher
                .decrypt(nonce, payload)
                .map_err(|e| CryptoError::CipherError(e.to_string()))
        }
    }
}

/// Parse a cipher algorithm name from Node.js format (e.g., "aes-256-gcm").
pub fn parse_algorithm(name: &str) -> Result<CipherAlgorithm, CryptoError> {
    match name.to_ascii_lowercase().as_str() {
        "aes-128-gcm" => Ok(CipherAlgorithm::Aes128Gcm),
        "aes-256-gcm" => Ok(CipherAlgorithm::Aes256Gcm),
        "chacha20-poly1305" | "chacha20-poly1305-ietf" => Ok(CipherAlgorithm::ChaCha20Poly1305),
        other => Err(CryptoError::InvalidAlgorithm(other.to_string())),
    }
}
