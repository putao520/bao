// @trace REQ-ENG-007 [entity:bao_crypto] [api:node:crypto pbkdf2Sync/scryptSync/hkdf]
// Unified KDF routing (DEC-ENG-003): the sha_hmac::pbkdf2 module was removed;
// pbkdf2Sync, scryptSync, and hkdf all route through here. scrypt.rs in
// sha_hmac is retained per the DEC-ENG-003 carve-out.
use crate::CryptoError;
use bun_boringssl_sys as bssl;

/// HKDF hash selection.
pub enum HkdfHash {
    Sha256,
    Sha1,
}

/// PBKDF2 digest selection (Node accepts sha1/sha256/sha512).
pub enum Pbkdf2Hash {
    Sha1,
    Sha256,
    Sha512,
}

impl Pbkdf2Hash {
    fn md(self) -> *const bssl::EVP_MD {
        match self {
            Pbkdf2Hash::Sha1 => bssl::EVP_sha1(),
            Pbkdf2Hash::Sha256 => bssl::EVP_sha256(),
            Pbkdf2Hash::Sha512 => bssl::EVP_sha512(),
        }
    }
}

pub fn parse_pbkdf2_hash(name: &str) -> Result<Pbkdf2Hash, CryptoError> {
    match name.to_lowercase().as_str() {
        "sha1" => Ok(Pbkdf2Hash::Sha1),
        "sha256" => Ok(Pbkdf2Hash::Sha256),
        "sha512" => Ok(Pbkdf2Hash::Sha512),
        other => Err(CryptoError::UnsupportedAlgorithm(other.to_string())),
    }
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

/// PBKDF2-HMAC key derivation (PKCS5_PBKDF2_HMAC via BoringSSL).
pub fn pbkdf2(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    hash: Pbkdf2Hash,
    key_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    let md = hash.md();
    let mut out = vec![0u8; key_len];
    let pw_ptr = if password.is_empty() {
        core::ptr::null()
    } else {
        password.as_ptr()
    };
    let rc = unsafe {
        bssl::PKCS5_PBKDF2_HMAC(
            pw_ptr,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_sha256_determinism_and_length() {
        let a = pbkdf2(b"password", b"salt", 1000, Pbkdf2Hash::Sha256, 32).unwrap();
        let b = pbkdf2(b"password", b"salt", 1000, Pbkdf2Hash::Sha256, 32).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(a, b);
    }

    #[test]
    fn pbkdf2_sha1_length() {
        let a = pbkdf2(b"password", b"salt", 1, Pbkdf2Hash::Sha1, 20).unwrap();
        assert_eq!(a.len(), 20);
    }

    #[test]
    fn pbkdf2_sha512_length() {
        let a = pbkdf2(b"password", b"salt", 10, Pbkdf2Hash::Sha512, 64).unwrap();
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn pbkdf2_empty_password_ok() {
        // Empty password must not deref null; PKCS5 accepts len=0 with NULL ptr.
        let a = pbkdf2(&[], b"salt", 5, Pbkdf2Hash::Sha256, 16).unwrap();
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn scrypt_determinism_and_length() {
        let a = scrypt(b"password", b"NaCl", 1 << 5, 8, 1, 32).unwrap();
        let b = scrypt(b"password", b"NaCl", 1 << 5, 8, 1, 32).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(a, b);
    }

    #[test]
    fn hkdf_sha256_length() {
        let a = hkdf(HkdfHash::Sha256, b"salt", b"ikm", b"info", 32).unwrap();
        assert_eq!(a.len(), 32);
    }
}
