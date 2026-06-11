#[allow(unused_imports)]
use aes_gcm::{
    aead::{AeadInPlace, KeyInit, KeySizeUser}, Aes128Gcm, Aes256Gcm, Nonce,
};
#[allow(unused_imports)]
use chacha20poly1305::{ChaCha20Poly1305, KeyInit as ChaKeyInit, KeySizeUser as ChaKeySizeUser, Nonce as ChaNonce};

use crate::CryptoError;

const AES_128_KEY_LEN: usize = 16;
const AES_256_KEY_LEN: usize = 32;
const AES_GCM_NONCE_LEN: usize = 12;
const AES_GCM_TAG_LEN: usize = 16;

const CHACHA_KEY_LEN: usize = 32;
const CHACHA_NONCE_LEN: usize = 12;
const CHACHA_TAG_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlgorithm {
    Aes128Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

pub fn parse_algorithm(name: &str) -> Result<CipherAlgorithm, CryptoError> {
    match name.to_lowercase().as_str() {
        "aes-128-gcm" => Ok(CipherAlgorithm::Aes128Gcm),
        "aes-256-gcm" => Ok(CipherAlgorithm::Aes256Gcm),
        "chacha20-poly1305" | "chacha20poly1305" => Ok(CipherAlgorithm::ChaCha20Poly1305),
        _ => Err(CryptoError::UnsupportedAlgorithm(name.to_string())),
    }
}

fn key_len(algo: CipherAlgorithm) -> usize {
    match algo {
        CipherAlgorithm::Aes128Gcm => AES_128_KEY_LEN,
        CipherAlgorithm::Aes256Gcm => AES_256_KEY_LEN,
        CipherAlgorithm::ChaCha20Poly1305 => CHACHA_KEY_LEN,
    }
}

fn nonce_len(algo: CipherAlgorithm) -> usize {
    match algo {
        CipherAlgorithm::Aes128Gcm | CipherAlgorithm::Aes256Gcm => AES_GCM_NONCE_LEN,
        CipherAlgorithm::ChaCha20Poly1305 => CHACHA_NONCE_LEN,
    }
}

fn tag_len(algo: CipherAlgorithm) -> usize {
    match algo {
        CipherAlgorithm::Aes128Gcm | CipherAlgorithm::Aes256Gcm => AES_GCM_TAG_LEN,
        CipherAlgorithm::ChaCha20Poly1305 => CHACHA_TAG_LEN,
    }
}

pub struct EncryptResult {
    pub ciphertext: Vec<u8>,
    pub auth_tag: Vec<u8>,
}

pub fn encrypt(
    algo: CipherAlgorithm,
    key: &[u8],
    iv: &[u8],
    aad: Option<&[u8]>,
    plaintext: &[u8],
) -> Result<EncryptResult, CryptoError> {
    let expected_key = key_len(algo);
    if key.len() != expected_key {
        return Err(CryptoError::InvalidKeyLength {
            expected: expected_key,
            got: key.len(),
        });
    }

    let expected_nonce = nonce_len(algo);
    if iv.len() != expected_nonce {
        return Err(CryptoError::InvalidNonceLength {
            expected: expected_nonce,
            got: iv.len(),
        });
    }

    let aad_data = aad.unwrap_or(&[]);

    match algo {
        CipherAlgorithm::Aes128Gcm => {
            let k = aes_gcm::Key::<Aes128Gcm>::from_slice(key);
            let cipher = Aes128Gcm::new(&k);
            let nonce = Nonce::from_slice(iv);
            let mut buffer = plaintext.to_vec();
            let tag = cipher
                .encrypt_in_place_detached(nonce, aad_data, &mut buffer)
                .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
            Ok(EncryptResult {
                ciphertext: buffer,
                auth_tag: tag.to_vec(),
            })
        }
        CipherAlgorithm::Aes256Gcm => {
            let k = aes_gcm::Key::<Aes256Gcm>::from_slice(key);
            let cipher = Aes256Gcm::new(&k);
            let nonce = Nonce::from_slice(iv);
            let mut buffer = plaintext.to_vec();
            let tag = cipher
                .encrypt_in_place_detached(nonce, aad_data, &mut buffer)
                .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
            Ok(EncryptResult {
                ciphertext: buffer,
                auth_tag: tag.to_vec(),
            })
        }
        CipherAlgorithm::ChaCha20Poly1305 => {
            let k = chacha20poly1305::Key::from_slice(key);
            let cipher = ChaCha20Poly1305::new(&k);
            let nonce = ChaNonce::from_slice(iv);
            let mut buffer = plaintext.to_vec();
            let tag = cipher
                .encrypt_in_place_detached(nonce, aad_data, &mut buffer)
                .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
            Ok(EncryptResult {
                ciphertext: buffer,
                auth_tag: tag.to_vec(),
            })
        }
    }
}

pub fn decrypt(
    algo: CipherAlgorithm,
    key: &[u8],
    iv: &[u8],
    aad: Option<&[u8]>,
    ciphertext: &[u8],
    tag: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let expected_key = key_len(algo);
    if key.len() != expected_key {
        return Err(CryptoError::InvalidKeyLength {
            expected: expected_key,
            got: key.len(),
        });
    }

    let expected_nonce = nonce_len(algo);
    if iv.len() != expected_nonce {
        return Err(CryptoError::InvalidNonceLength {
            expected: expected_nonce,
            got: iv.len(),
        });
    }

    let expected_tag = tag_len(algo);
    if tag.len() != expected_tag {
        return Err(CryptoError::DecryptionFailed(format!(
            "invalid tag length: expected {expected_tag}, got {}",
            tag.len()
        )));
    }

    let aad_data = aad.unwrap_or(&[]);

    match algo {
        CipherAlgorithm::Aes128Gcm => {
            let k = aes_gcm::Key::<Aes128Gcm>::from_slice(key);
            let cipher = Aes128Gcm::new(&k);
            let nonce = Nonce::from_slice(iv);
            let tag_arr = aes_gcm::Tag::from_slice(tag);
            let mut buffer = ciphertext.to_vec();
            cipher
                .decrypt_in_place_detached(nonce, aad_data, &mut buffer, tag_arr)
                .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;
            Ok(buffer)
        }
        CipherAlgorithm::Aes256Gcm => {
            let k = aes_gcm::Key::<Aes256Gcm>::from_slice(key);
            let cipher = Aes256Gcm::new(&k);
            let nonce = Nonce::from_slice(iv);
            let tag_arr = aes_gcm::Tag::from_slice(tag);
            let mut buffer = ciphertext.to_vec();
            cipher
                .decrypt_in_place_detached(nonce, aad_data, &mut buffer, tag_arr)
                .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;
            Ok(buffer)
        }
        CipherAlgorithm::ChaCha20Poly1305 => {
            let k = chacha20poly1305::Key::from_slice(key);
            let cipher = ChaCha20Poly1305::new(&k);
            let nonce = ChaNonce::from_slice(iv);
            let tag_arr = chacha20poly1305::Tag::from_slice(tag);
            let mut buffer = ciphertext.to_vec();
            cipher
                .decrypt_in_place_detached(nonce, aad_data, &mut buffer, tag_arr)
                .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;
            Ok(buffer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_128_gcm_roundtrip() {
        let key = &[0u8; 16];
        let iv = &[1u8; 12];
        let plaintext = b"hello aes-128-gcm";
        let result = encrypt(CipherAlgorithm::Aes128Gcm, key, iv, None, plaintext).unwrap();
        let decrypted = decrypt(CipherAlgorithm::Aes128Gcm, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn aes_256_gcm_roundtrip() {
        let key = &[0u8; 32];
        let iv = &[1u8; 12];
        let plaintext = b"hello aes-256-gcm";
        let result = encrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, plaintext).unwrap();
        let decrypted = decrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn chacha20_poly1305_roundtrip() {
        let key = &[0u8; 32];
        let iv = &[1u8; 12];
        let plaintext = b"hello chacha20-poly1305";
        let result = encrypt(CipherAlgorithm::ChaCha20Poly1305, key, iv, None, plaintext).unwrap();
        let decrypted = decrypt(CipherAlgorithm::ChaCha20Poly1305, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn aes_256_gcm_with_aad() {
        let key = &[0u8; 32];
        let iv = &[1u8; 12];
        let aad = b"additional data";
        let plaintext = b"hello with aad";
        let result = encrypt(CipherAlgorithm::Aes256Gcm, key, iv, Some(aad), plaintext).unwrap();
        let decrypted = decrypt(CipherAlgorithm::Aes256Gcm, key, iv, Some(aad), &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key = &[0u8; 32];
        let wrong_key = &[1u8; 32];
        let iv = &[1u8; 12];
        let plaintext = b"secret message";
        let result = encrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, plaintext).unwrap();
        assert!(decrypt(CipherAlgorithm::Aes256Gcm, wrong_key, iv, None, &result.ciphertext, &result.auth_tag).is_err());
    }
}
