//! Cryptographically secure random number generation.
//!
//! Wraps `getrandom` for `crypto.randomBytes` / `crypto.randomFillSync`.

use crate::sign::CryptoError;

/// Fill a buffer with cryptographically secure random bytes.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(buf).map_err(|e| CryptoError::CipherError(e.to_string()))
}

/// Generate a Vec of cryptographically secure random bytes.
pub fn random_vec(len: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buf = vec![0u8; len];
    random_bytes(&mut buf)?;
    Ok(buf)
}

/// Generate a random integer in [0, max).
pub fn random_int(max: u64) -> Result<u64, CryptoError> {
    let mut bytes = [0u8; 8];
    random_bytes(&mut bytes)?;
    let val = u64::from_ne_bytes(bytes);
    Ok(val % max)
}
