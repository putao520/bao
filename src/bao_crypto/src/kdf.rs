//! Key derivation functions (HKDF, PBKDF2).
//!
//! Implements `crypto.hkdf` / `crypto.pbkdf2` for Node.js.

use sha2::Sha256;
use sha1::Sha1;

use crate::sign::CryptoError;

/// HKDF extract-and-expand.
pub fn hkdf(
    hash: HkdfHash,
    salt: &[u8],
    ikm: &[u8],
    info: &[u8],
    okm_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    match hash {
        HkdfHash::Sha256 => {
            let h = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
            let mut okm = vec![0u8; okm_len];
            h.expand(info, &mut okm)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            Ok(okm)
        }
        HkdfHash::Sha1 => {
            let h = hkdf::Hkdf::<Sha1>::new(Some(salt), ikm);
            let mut okm = vec![0u8; okm_len];
            h.expand(info, &mut okm)
                .map_err(|e| CryptoError::CipherError(e.to_string()))?;
            Ok(okm)
        }
    }
}

/// HKDF hash variant.
#[derive(Debug, Clone, Copy)]
pub enum HkdfHash {
    Sha256,
    Sha1,
}

/// PBKDF2 key derivation using the `pbkdf2` crate.
pub fn pbkdf2(
    hash: Pbkdf2Hash,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    key_len: usize,
) -> Vec<u8> {
    let mut result = vec![0u8; key_len];
    match hash {
        Pbkdf2Hash::Sha256 => {
            pbkdf2::pbkdf2::<hmac::Hmac<Sha256>>(password, salt, iterations, &mut result)
                .expect("PBKDF2 output buffer is valid");
        }
        Pbkdf2Hash::Sha1 => {
            pbkdf2::pbkdf2::<hmac::Hmac<Sha1>>(password, salt, iterations, &mut result)
                .expect("PBKDF2 output buffer is valid");
        }
    }
    result
}

/// PBKDF2 hash variant.
#[derive(Debug, Clone, Copy)]
pub enum Pbkdf2Hash {
    Sha256,
    Sha1,
}
