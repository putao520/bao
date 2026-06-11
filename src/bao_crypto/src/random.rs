use crate::CryptoError;

pub fn random_bytes(len: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buf = vec![0u8; len];
    getrandom::fill(&mut buf).map_err(|e| CryptoError::RandomFailed(e.to_string()))?;
    Ok(buf)
}
