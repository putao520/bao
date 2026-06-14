use crate::CryptoError;
use bun_boringssl_sys as bssl;

pub fn random_bytes(len: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buf = vec![0u8; len];
    let rc = unsafe { bssl::RAND_bytes(buf.as_mut_ptr(), len) };
    if rc != 1 {
        return Err(CryptoError::RandomFailed("RAND_bytes failed".into()));
    }
    Ok(buf)
}
