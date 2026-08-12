use crate::CryptoError;
use bun_boringssl_sys::*;
use core::ffi::{c_int, c_void};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcCurve {
    P256,
    P384,
}

#[derive(Debug)]
pub enum KeyPairType {
    Rsa { bits: usize },
    Ec { curve: EcCurve },
    Ed25519,
    X25519,
}

pub struct KeyPairResult {
    pub public_key_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    pub public_key_pem: Option<String>,
    pub private_key_pem: Option<String>,
}

pub fn generate_key_pair(kp_type: &KeyPairType) -> Result<KeyPairResult, CryptoError> {
    match kp_type {
        KeyPairType::Rsa { bits } => generate_rsa(*bits),
        KeyPairType::Ec { curve } => generate_ec(*curve),
        KeyPairType::Ed25519 => generate_ed25519(),
        KeyPairType::X25519 => generate_x25519(),
    }
}

struct PkeyGuard(*mut EVP_PKEY);
impl Drop for PkeyGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { EVP_PKEY_free(self.0) };
        }
    }
}

struct RsaGuard(*mut RSA);
impl Drop for RsaGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { RSA_free(self.0) };
        }
    }
}

struct EcKeyGuard(*mut EC_KEY);
impl Drop for EcKeyGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { EC_KEY_free(self.0) };
        }
    }
}

struct BnGuard(*mut BIGNUM);
impl Drop for BnGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { BN_free(self.0) };
        }
    }
}

struct BioGuard(*mut BIO);
impl Drop for BioGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { BIO_free(self.0) };
        }
    }
}

fn bio_to_vec(bio: *mut BIO) -> Result<Vec<u8>, CryptoError> {
    let len = unsafe { BIO_ctrl(bio, 3, 0, std::ptr::null_mut()) } as usize; // BIO_CTRL_PENDING = 3
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len];
    let n = unsafe {
        BIO_read(
            bio,
            buf.as_mut_ptr() as *mut c_void,
            len as core::ffi::c_int,
        )
    };
    if n < 0 {
        return Err(CryptoError::EncodingFailed("BIO_read failed".into()));
    }
    buf.truncate(n as usize);
    Ok(buf)
}

fn bio_to_string(bio: *mut BIO) -> Result<String, CryptoError> {
    let bytes = bio_to_vec(bio)?;
    String::from_utf8(bytes).map_err(|e| CryptoError::EncodingFailed(format!("UTF-8: {}", e)))
}

fn serialize_pkey(pkey: *mut EVP_PKEY) -> Result<KeyPairResult, CryptoError> {
    // DER private key. Some BoringSSL builds reject i2d_PrivateKey for raw-seed
    // Ed25519/X25519 keys; treat DER as best-effort and let the PEM path be the
    // source of truth (Node consumers use the PEM form).
    let mut priv_out: *mut u8 = std::ptr::null_mut();
    let priv_len = unsafe { i2d_PrivateKey(pkey, &mut priv_out) };
    let private_key_der = if priv_len > 0 && !priv_out.is_null() {
        let bytes = unsafe { std::slice::from_raw_parts(priv_out, priv_len as usize) }.to_vec();
        unsafe { OPENSSL_free(priv_out as *mut c_void) };
        bytes
    } else {
        Vec::new()
    };

    // DER public key
    let mut pub_out: *mut u8 = std::ptr::null_mut();
    let pub_len = unsafe { i2d_PUBKEY(pkey, &mut pub_out) };
    if pub_len <= 0 || pub_out.is_null() {
        return Err(CryptoError::EncodingFailed("i2d_PUBKEY failed".into()));
    }
    let public_key_der = unsafe { std::slice::from_raw_parts(pub_out, pub_len as usize) }.to_vec();
    unsafe { OPENSSL_free(pub_out as *mut c_void) };

    // PEM private key
    let priv_bio = BioGuard(unsafe { BIO_new(BIO_s_mem()) });
    if priv_bio.0.is_null() {
        return Err(CryptoError::EncodingFailed("BIO_new failed".into()));
    }
    if unsafe {
        PEM_write_bio_PKCS8PrivateKey(
            priv_bio.0,
            pkey,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
            None,
            std::ptr::null_mut(),
        )
    } != 1
    {
        return Err(CryptoError::EncodingFailed(
            "PEM_write_bio_PKCS8PrivateKey failed".into(),
        ));
    }
    let private_key_pem = bio_to_string(priv_bio.0).ok();

    // PEM public key
    let pub_bio = BioGuard(unsafe { BIO_new(BIO_s_mem()) });
    if pub_bio.0.is_null() {
        return Err(CryptoError::EncodingFailed("BIO_new failed".into()));
    }
    if unsafe { PEM_write_bio_PUBKEY(pub_bio.0, pkey) } != 1 {
        return Err(CryptoError::EncodingFailed(
            "PEM_write_bio_PUBKEY failed".into(),
        ));
    }
    let public_key_pem = bio_to_string(pub_bio.0).ok();

    Ok(KeyPairResult {
        public_key_der,
        private_key_der,
        public_key_pem,
        private_key_pem,
    })
}

fn generate_rsa(bits: usize) -> Result<KeyPairResult, CryptoError> {
    let rsa = RsaGuard(unsafe { RSA_new() });
    if rsa.0.is_null() {
        return Err(CryptoError::KeyPairError("RSA_new failed".into()));
    }

    let e = BnGuard(unsafe { BN_new() });
    if e.0.is_null() {
        return Err(CryptoError::KeyPairError("BN_new failed".into()));
    }
    if unsafe { BN_set_word(e.0, 0x10001) } != 1 {
        return Err(CryptoError::KeyPairError("BN_set_word failed".into()));
    }
    if unsafe { RSA_generate_key_ex(rsa.0, bits as c_int, e.0, std::ptr::null_mut()) } != 1 {
        return Err(CryptoError::KeyPairError(
            "RSA_generate_key_ex failed".into(),
        ));
    }

    let pkey = PkeyGuard(unsafe { EVP_PKEY_new() });
    if pkey.0.is_null() {
        return Err(CryptoError::KeyPairError("EVP_PKEY_new failed".into()));
    }
    if unsafe { EVP_PKEY_set1_RSA(pkey.0, rsa.0) } != 1 {
        return Err(CryptoError::KeyPairError("EVP_PKEY_set1_RSA failed".into()));
    }

    serialize_pkey(pkey.0)
}

fn generate_ec(curve: EcCurve) -> Result<KeyPairResult, CryptoError> {
    let nid = match curve {
        EcCurve::P256 => NID_X9_62_prime256v1,
        EcCurve::P384 => NID_secp384r1,
    };

    let ec_key = EcKeyGuard(unsafe { EC_KEY_new_by_curve_name(nid) });
    if ec_key.0.is_null() {
        return Err(CryptoError::KeyPairError(
            "EC_KEY_new_by_curve_name failed".into(),
        ));
    }
    if unsafe { EC_KEY_generate_key(ec_key.0) } != 1 {
        return Err(CryptoError::KeyPairError(
            "EC_KEY_generate_key failed".into(),
        ));
    }

    let pkey = PkeyGuard(unsafe { EVP_PKEY_new() });
    if pkey.0.is_null() {
        return Err(CryptoError::KeyPairError("EVP_PKEY_new failed".into()));
    }
    if unsafe { EVP_PKEY_set1_EC_KEY(pkey.0, ec_key.0) } != 1 {
        return Err(CryptoError::KeyPairError(
            "EVP_PKEY_set1_EC_KEY failed".into(),
        ));
    }

    serialize_pkey(pkey.0)
}

fn generate_ed25519() -> Result<KeyPairResult, CryptoError> {
    // BoringSSL does not support Ed25519 via EVP_PKEY_CTX_new_id keygen; generate
    // a 32-byte seed and lift it into an EVP_PKEY via EVP_PKEY_from_raw_private_key.
    let mut seed = [0u8; 32];
    crate::random::rand_bytes(&mut seed)?;
    let pkey = PkeyGuard(unsafe {
        EVP_PKEY_from_raw_private_key(EVP_pkey_ed25519(), seed.as_ptr(), seed.len())
    });
    if pkey.0.is_null() {
        return Err(CryptoError::KeyPairError(
            "EVP_PKEY_from_raw_private_key (ed25519) failed".into(),
        ));
    }
    serialize_pkey(pkey.0)
}

fn generate_x25519() -> Result<KeyPairResult, CryptoError> {
    // Same caveat as Ed25519: BoringSSL needs the raw-seed path for X25519 keygen.
    let mut seed = [0u8; 32];
    crate::random::rand_bytes(&mut seed)?;
    let pkey = PkeyGuard(unsafe {
        EVP_PKEY_from_raw_private_key(EVP_pkey_x25519(), seed.as_ptr(), seed.len())
    });
    if pkey.0.is_null() {
        return Err(CryptoError::KeyPairError(
            "EVP_PKEY_from_raw_private_key (x25519) failed".into(),
        ));
    }
    serialize_pkey(pkey.0)
}
