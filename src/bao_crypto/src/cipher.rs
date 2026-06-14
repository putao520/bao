use crate::CryptoError;
use bun_boringssl_sys::*;
use core::mem::MaybeUninit;
const AES_128_KEY_LEN: usize = 16;
const AES_256_KEY_LEN: usize = 32;
const AES_GCM_NONCE_LEN: usize = 12;
const AES_GCM_TAG_LEN: usize = 16;

const CHACHA_KEY_LEN: usize = 32;
const CHACHA_NONCE_LEN: usize = 12;
const CHACHA_TAG_LEN: usize = 16;

// EVP_AEAD_DEFAULT_TAG_LENGTH in BoringSSL — pass 0 to use the AEAD's default.
const EVP_AEAD_DEFAULT_TAG_LENGTH: usize = 0;

// BoringSSL: struct evp_aead_ctx_st { alignas(16) uint8_t opaque[580]; }
// 640 bytes with 16-byte alignment is safe across builds.
#[repr(C, align(16))]
struct AeadCtxStorage {
    data: [u8; 640],
}

/// RAII wrapper: init on creation, cleanup on drop.
struct AeadCtx {
    #[allow(dead_code)]
    storage: MaybeUninit<AeadCtxStorage>,
    ctx: *mut EVP_AEAD_CTX,
    initialized: bool,
}

impl AeadCtx {
    fn new(algo: CipherAlgorithm, key: &[u8]) -> Result<Self, CryptoError> {
        let mut storage = MaybeUninit::<AeadCtxStorage>::zeroed();
        let ctx = storage.as_mut_ptr() as *mut EVP_AEAD_CTX;
        let aead = get_aead(algo);
        let rc = unsafe {
            EVP_AEAD_CTX_init(
                ctx,
                aead,
                key.as_ptr(),
                key.len(),
                EVP_AEAD_DEFAULT_TAG_LENGTH,
                core::ptr::null_mut(),
            )
        };
        if rc != 1 {
            Err(CryptoError::EncryptionFailed("EVP_AEAD_CTX_init failed".into()))
        } else {
            Ok(Self { storage, ctx, initialized: true })
        }
    }

    fn as_ptr(&mut self) -> *mut EVP_AEAD_CTX {
        self.ctx
    }
}

impl Drop for AeadCtx {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { EVP_AEAD_CTX_cleanup(self.ctx) };
        }
    }
}

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

fn get_aead(algo: CipherAlgorithm) -> *const EVP_AEAD {
    match algo {
        CipherAlgorithm::Aes128Gcm => EVP_aead_aes_128_gcm(),
        CipherAlgorithm::Aes256Gcm => EVP_aead_aes_256_gcm(),
        CipherAlgorithm::ChaCha20Poly1305 => EVP_aead_chacha20_poly1305(),
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
    let tag_size = tag_len(algo);
    let max_out = plaintext.len() + tag_size;

    let mut ctx = AeadCtx::new(algo, key)?;

    // EVP_AEAD_CTX_seal outputs ciphertext || tag (combined format).
    let mut out = vec![0u8; max_out];
    let mut out_len: usize = 0;

    let rc = unsafe {
        EVP_AEAD_CTX_seal(
            ctx.as_ptr(),
            out.as_mut_ptr(),
            &mut out_len,
            max_out,
            iv.as_ptr(),
            iv.len(),
            plaintext.as_ptr(),
            plaintext.len(),
            aad_data.as_ptr(),
            aad_data.len(),
        )
    };

    if rc != 1 {
        return Err(CryptoError::EncryptionFailed("EVP_AEAD_CTX_seal failed".into()));
    }

    // Split combined output into detached ciphertext + tag.
    let ct_len = out_len - tag_size;
    let ciphertext = out[..ct_len].to_vec();
    let auth_tag = out[ct_len..out_len].to_vec();

    Ok(EncryptResult { ciphertext, auth_tag })
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

    let mut ctx = AeadCtx::new(algo, key).map_err(|_| {
        CryptoError::DecryptionFailed("EVP_AEAD_CTX_init failed".into())
    })?;

    // EVP_AEAD_CTX_open expects ciphertext || tag (combined format).
    let combined_len = ciphertext.len() + tag.len();
    let mut combined = Vec::with_capacity(combined_len);
    combined.extend_from_slice(ciphertext);
    combined.extend_from_slice(tag);

    let mut out = vec![0u8; ciphertext.len()];
    let mut out_len: usize = 0;

    let rc = unsafe {
        EVP_AEAD_CTX_open(
            ctx.as_ptr(),
            out.as_mut_ptr(),
            &mut out_len,
            ciphertext.len(),
            iv.as_ptr(),
            iv.len(),
            combined.as_ptr(),
            combined_len,
            aad_data.as_ptr(),
            aad_data.len(),
        )
    };

    if rc != 1 {
        return Err(CryptoError::DecryptionFailed("EVP_AEAD_CTX_open failed".into()));
    }

    debug_assert_eq!(out_len, ciphertext.len());
    out.truncate(out_len);
    Ok(out)
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
        assert_eq!(result.auth_tag.len(), 16);
        let decrypted = decrypt(CipherAlgorithm::Aes128Gcm, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn aes_256_gcm_roundtrip() {
        let key = &[0u8; 32];
        let iv = &[1u8; 12];
        let plaintext = b"hello aes-256-gcm";
        let result = encrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, plaintext).unwrap();
        assert_eq!(result.auth_tag.len(), 16);
        let decrypted = decrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn chacha20_poly1305_roundtrip() {
        let key = &[0u8; 32];
        let iv = &[1u8; 12];
        let plaintext = b"hello chacha20-poly1305";
        let result = encrypt(CipherAlgorithm::ChaCha20Poly1305, key, iv, None, plaintext).unwrap();
        assert_eq!(result.auth_tag.len(), 16);
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
