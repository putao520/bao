use crate::CryptoError;
use sha1::Sha1;
use sha2::Sha256;

pub enum HkdfHash {
    Sha256,
    Sha1,
}

pub fn hkdf(
    hash: HkdfHash,
    salt: &[u8],
    ikm: &[u8],
    info: &[u8],
    key_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    match hash {
        HkdfHash::Sha256 => {
            let h = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
            let mut okm = vec![0u8; key_len];
            h.expand(info, &mut okm)
                .map_err(|e| CryptoError::KdfError(format!("HKDF-SHA256 expand: {}", e)))?;
            Ok(okm)
        }
        HkdfHash::Sha1 => {
            let h = hkdf::Hkdf::<Sha1>::new(Some(salt), ikm);
            let mut okm = vec![0u8; key_len];
            h.expand(info, &mut okm)
                .map_err(|e| CryptoError::KdfError(format!("HKDF-SHA1 expand: {}", e)))?;
            Ok(okm)
        }
    }
}
