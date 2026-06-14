use crate::CryptoError;
use bun_boringssl_sys as bssl;

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
    let md = match hash {
        HkdfHash::Sha256 => bssl::EVP_sha256(),
        HkdfHash::Sha1 => bssl::EVP_sha1(),
    };
    let mut out = vec![0u8; key_len];
    let rc = unsafe {
        bssl::HKDF(
            out.as_mut_ptr(),
            key_len,
            md,
            salt.as_ptr(),
            salt.len(),
            ikm.as_ptr(),
            ikm.len(),
            info.as_ptr(),
            info.len(),
        )
    };
    if rc != 1 {
        return Err(CryptoError::KdfError("HKDF expand failed".into()));
    }
    Ok(out)
}

pub fn pbkdf2(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    hash: HkdfHash,
    key_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    let md = match hash {
        HkdfHash::Sha256 => bssl::EVP_sha256(),
        HkdfHash::Sha1 => bssl::EVP_sha1(),
    };
    let mut out = vec![0u8; key_len];
    let rc = unsafe {
        bssl::PKCS5_PBKDF2_HMAC(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            iterations,
            md,
            key_len,
            out.as_mut_ptr(),
        )
    };
    if rc != 1 {
        return Err(CryptoError::KdfError("PBKDF2 failed".into()));
    }
    Ok(out)
}

pub fn scrypt(
    password: &[u8],
    salt: &[u8],
    n: u64,
    r: u64,
    p: u64,
    key_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    let mut out = vec![0u8; key_len];
    let rc = unsafe {
        bssl::EVP_PBE_scrypt(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            n,
            r,
            p,
            0,
            out.as_mut_ptr(),
            key_len,
        )
    };
    if rc != 1 {
        return Err(CryptoError::KdfError("scrypt failed".into()));
    }
    Ok(out)
}
