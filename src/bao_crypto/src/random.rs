use crate::CryptoError;
use bun_boringssl_sys as bssl;

/// Generate cryptographically secure random bytes using BoringSSL RAND_bytes.
pub fn random_bytes(len: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buf = vec![0u8; len];
    let rc = unsafe { bssl::RAND_bytes(buf.as_mut_ptr(), len) };
    if rc != 1 {
        return Err(CryptoError::RandomFailed("RAND_bytes failed".into()));
    }
    Ok(buf)
}

/// Fill a mutable byte slice with cryptographically secure random bytes.
/// This is the CSPRNG replacement for rand::thread_rng().fill_bytes().
pub fn rand_bytes(buf: &mut [u8]) -> Result<(), CryptoError> {
    let rc = unsafe { bssl::RAND_bytes(buf.as_mut_ptr(), buf.len()) };
    if rc != 1 {
        return Err(CryptoError::RandomFailed("RAND_bytes failed".into()));
    }
    Ok(())
}
