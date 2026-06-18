// @trace REQ-ENG-007 [entity:bao_crypto] [api:node:crypto createCipheriv/createDecipheriv]
// Real BoringSSL symmetric ciphers. AEAD (AES-GCM, ChaCha20-Poly1305) via the
// EVP_AEAD API exposed by bun_boringssl_sys; non-AEAD (AES-CBC/CTR, DES-EDE3-CBC)
// via EVP_CIPHER_CTX / EVP_aes_* / EVP_Cipher* FFI bound locally with extern "C"
// (bun_boringssl_sys does not surface these cipher symbols — bound here per the
// TASK-1-CRYPTO file-domain rule, never by editing bun_boringssl_sys source).
use crate::CryptoError;
use bun_boringssl_sys as bssl;
use core::ffi::c_int;
use core::mem::MaybeUninit;
use core::ptr;

// ── AEAD constants (BoringSSL layout) ────────────────────────────────────────
const AES_128_KEY_LEN: usize = 16;
const AES_192_KEY_LEN: usize = 24;
const AES_256_KEY_LEN: usize = 32;
const AES_GCM_NONCE_LEN: usize = 12;

// EVP_AEAD_DEFAULT_TAG_LENGTH in BoringSSL — pass 0 to use the AEAD's default.
const EVP_AEAD_DEFAULT_TAG_LENGTH: usize = 0;

// AES block size (CBC IV / block length).
const AES_BLOCK_SIZE: usize = 16;
// DES (EDE3) block size.
const DES_BLOCK_SIZE: usize = 8;

// BoringSSL: struct evp_aead_ctx_st { alignas(16) uint8_t opaque[580]; }
// 640 bytes with 16-byte alignment is safe across builds.
#[repr(C, align(16))]
struct AeadCtxStorage {
    data: [u8; 640],
}

// ── Local extern "C" bindings for non-AEAD EVP_CIPHER API ───────────────────
// bun_boringssl_sys deliberately omits EVP_CIPHER_CTX and the EVP_aes_*/des*
// cipher getters. Bao binds them here against the same libboringssl.a that
// bun_boringssl_sys already links (force_link propagates the native dep).
// Signatures mirror <openssl/evp.h> as shipped by BoringSSL.

#[repr(C)]
struct evp_cipher_ctx_st {
    opaque: [u8; 168],
}

unsafe extern "C" {
    fn EVP_CIPHER_CTX_new() -> *mut evp_cipher_ctx_st;
    fn EVP_CIPHER_CTX_free(ctx: *mut evp_cipher_ctx_st);

    fn EVP_aes_128_cbc() -> *const bssl::EVP_CIPHER;
    fn EVP_aes_192_cbc() -> *const bssl::EVP_CIPHER;
    fn EVP_aes_256_cbc() -> *const bssl::EVP_CIPHER;
    fn EVP_aes_128_ctr() -> *const bssl::EVP_CIPHER;
    fn EVP_aes_192_ctr() -> *const bssl::EVP_CIPHER;
    fn EVP_aes_256_ctr() -> *const bssl::EVP_CIPHER;
    fn EVP_des_ede3_cbc() -> *const bssl::EVP_CIPHER;

    fn EVP_CipherInit_ex(
        ctx: *mut evp_cipher_ctx_st,
        cipher: *const bssl::EVP_CIPHER,
        impl_: *mut core::ffi::c_void,
        key: *const u8,
        iv: *const u8,
        enc: c_int,
    ) -> c_int;
    fn EVP_CipherUpdate(
        ctx: *mut evp_cipher_ctx_st,
        out: *mut u8,
        outl: *mut c_int,
        input: *const u8,
        inl: c_int,
    ) -> c_int;
    fn EVP_CipherFinal_ex(
        ctx: *mut evp_cipher_ctx_st,
        out: *mut u8,
        outl: *mut c_int,
    ) -> c_int;
}

/// RAII EVP_CIPHER_CTX (non-AEAD). Reset on drop.
struct EvpCipherCtx {
    ctx: *mut evp_cipher_ctx_st,
}

impl EvpCipherCtx {
    fn new() -> Result<Self, CryptoError> {
        let ctx = unsafe { EVP_CIPHER_CTX_new() };
        if ctx.is_null() {
            return Err(CryptoError::EncryptionFailed("EVP_CIPHER_CTX_new failed".into()));
        }
        Ok(EvpCipherCtx { ctx })
    }

    fn init(
        &mut self,
        cipher: *const bssl::EVP_CIPHER,
        key: &[u8],
        iv: &[u8],
        encrypt: bool,
    ) -> Result<(), CryptoError> {
        let enc: c_int = if encrypt { 1 } else { 0 };
        let rc = unsafe {
            EVP_CipherInit_ex(self.ctx, cipher, ptr::null_mut(), key.as_ptr(), iv.as_ptr(), enc)
        };
        if rc != 1 {
            return Err(CryptoError::EncryptionFailed("EVP_CipherInit_ex failed".into()));
        }
        Ok(())
    }

    fn update(&mut self, input: &[u8]) -> Result<Vec<u8>, CryptoError> {
        // Worst case: one full block of expansion per update.
        let max_out = input.len() + AES_BLOCK_SIZE;
        let mut out = vec![0u8; max_out];
        let mut outl: c_int = 0;
        let rc = unsafe {
            EVP_CipherUpdate(self.ctx, out.as_mut_ptr(), &mut outl, input.as_ptr(), input.len() as c_int)
        };
        if rc != 1 {
            return Err(CryptoError::EncryptionFailed("EVP_CipherUpdate failed".into()));
        }
        out.truncate(outl.max(0) as usize);
        Ok(out)
    }

    fn final_ex(&mut self) -> Result<Vec<u8>, CryptoError> {
        let mut out = vec![0u8; AES_BLOCK_SIZE];
        let mut outl: c_int = 0;
        let rc = unsafe { EVP_CipherFinal_ex(self.ctx, out.as_mut_ptr(), &mut outl) };
        if rc != 1 {
            return Err(CryptoError::DecryptionFailed("EVP_CipherFinal_ex failed".into()));
        }
        out.truncate(outl.max(0) as usize);
        Ok(out)
    }
}

impl Drop for EvpCipherCtx {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { EVP_CIPHER_CTX_free(self.ctx) };
        }
    }
}

// ── AEAD RAII wrapper ─────────────────────────────────────────────────────────
/// RAII wrapper for EVP_AEAD_CTX: init on creation, cleanup on drop.
///
/// The BoringSSL `EVP_AEAD_CTX` is a value type (`struct { alignas(16) uint8_t
/// opaque[580]; }`). We inline the storage here so the context never moves
/// independently of the pointer derived from it. `ctx_ptr()` recomputes the
/// pointer from `&mut storage` on every use to stay valid across `AeadCtx`
/// moves (a stored raw pointer would dangle after move).
struct AeadCtx {
    storage: MaybeUninit<AeadCtxStorage>,
    initialized: bool,
}

impl AeadCtx {
    fn new(aead: *const bssl::EVP_AEAD, key: &[u8]) -> Result<Self, CryptoError> {
        let mut storage = MaybeUninit::<AeadCtxStorage>::zeroed();
        let ctx = storage.as_mut_ptr() as *mut bssl::EVP_AEAD_CTX;
        let rc = unsafe {
            bssl::EVP_AEAD_CTX_init(
                ctx,
                aead,
                key.as_ptr(),
                key.len(),
                EVP_AEAD_DEFAULT_TAG_LENGTH,
                ptr::null_mut(),
            )
        };
        if rc != 1 {
            Err(CryptoError::EncryptionFailed("EVP_AEAD_CTX_init failed".into()))
        } else {
            Ok(Self { storage, initialized: true })
        }
    }

    fn ctx_ptr(&mut self) -> *mut bssl::EVP_AEAD_CTX {
        self.storage.as_mut_ptr() as *mut bssl::EVP_AEAD_CTX
    }
}

impl Drop for AeadCtx {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { bssl::EVP_AEAD_CTX_cleanup(self.ctx_ptr()) };
        }
    }
}

// ── Algorithm enumeration ─────────────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlgorithm {
    // Non-AEAD block/stream ciphers (EVP_Cipher*).
    Aes128Cbc,
    Aes192Cbc,
    Aes256Cbc,
    Aes128Ctr,
    Aes192Ctr,
    Aes256Ctr,
    DesEde3Cbc,
    // AEAD (EVP_AEAD).
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl CipherAlgorithm {
    pub fn is_aead(self) -> bool {
        matches!(
            self,
            CipherAlgorithm::Aes128Gcm
                | CipherAlgorithm::Aes192Gcm
                | CipherAlgorithm::Aes256Gcm
                | CipherAlgorithm::ChaCha20Poly1305
        )
    }

    pub fn key_len(self) -> usize {
        match self {
            CipherAlgorithm::Aes128Cbc | CipherAlgorithm::Aes128Ctr | CipherAlgorithm::Aes128Gcm => {
                AES_128_KEY_LEN
            }
            CipherAlgorithm::Aes192Cbc | CipherAlgorithm::Aes192Ctr | CipherAlgorithm::Aes192Gcm => {
                AES_192_KEY_LEN
            }
            CipherAlgorithm::Aes256Cbc
            | CipherAlgorithm::Aes256Ctr
            | CipherAlgorithm::Aes256Gcm
            | CipherAlgorithm::ChaCha20Poly1305 => AES_256_KEY_LEN,
            CipherAlgorithm::DesEde3Cbc => 24,
        }
    }

    fn nonce_len(self) -> usize {
        match self {
            CipherAlgorithm::Aes128Gcm
            | CipherAlgorithm::Aes192Gcm
            | CipherAlgorithm::Aes256Gcm
            | CipherAlgorithm::ChaCha20Poly1305 => AES_GCM_NONCE_LEN,
            CipherAlgorithm::Aes128Cbc
            | CipherAlgorithm::Aes192Cbc
            | CipherAlgorithm::Aes256Cbc => AES_BLOCK_SIZE,
            CipherAlgorithm::Aes128Ctr | CipherAlgorithm::Aes192Ctr | CipherAlgorithm::Aes256Ctr => {
                AES_BLOCK_SIZE
            }
            CipherAlgorithm::DesEde3Cbc => DES_BLOCK_SIZE,
        }
    }

    fn aead_cipher(self) -> Option<*const bssl::EVP_AEAD> {
        Some(match self {
            CipherAlgorithm::Aes128Gcm => bssl::EVP_aead_aes_128_gcm(),
            CipherAlgorithm::Aes256Gcm => bssl::EVP_aead_aes_256_gcm(),
            CipherAlgorithm::ChaCha20Poly1305 => bssl::EVP_aead_chacha20_poly1305(),
            // AES-192-GCM: BoringSSL ships no dedicated getter, but the 256-GCM
            // aead honors the actual key length passed to EVP_AEAD_CTX_init only
            // for the AES-*_GCM internal; 192 is unsupported by BoringSSL's AEAD.
            CipherAlgorithm::Aes192Gcm => return None,
            _ => return None,
        })
    }

    fn block_cipher(self) -> Option<*const bssl::EVP_CIPHER> {
        Some(unsafe {
            match self {
                CipherAlgorithm::Aes128Cbc => EVP_aes_128_cbc(),
                CipherAlgorithm::Aes192Cbc => EVP_aes_192_cbc(),
                CipherAlgorithm::Aes256Cbc => EVP_aes_256_cbc(),
                CipherAlgorithm::Aes128Ctr => EVP_aes_128_ctr(),
                CipherAlgorithm::Aes192Ctr => EVP_aes_192_ctr(),
                CipherAlgorithm::Aes256Ctr => EVP_aes_256_ctr(),
                CipherAlgorithm::DesEde3Cbc => EVP_des_ede3_cbc(),
                _ => return None,
            }
        })
    }
}

pub fn parse_algorithm(name: &str) -> Result<CipherAlgorithm, CryptoError> {
    match name.to_lowercase().as_str() {
        "aes-128-cbc" => Ok(CipherAlgorithm::Aes128Cbc),
        "aes-192-cbc" => Ok(CipherAlgorithm::Aes192Cbc),
        "aes-256-cbc" => Ok(CipherAlgorithm::Aes256Cbc),
        "aes-128-ctr" => Ok(CipherAlgorithm::Aes128Ctr),
        "aes-192-ctr" => Ok(CipherAlgorithm::Aes192Ctr),
        "aes-256-ctr" => Ok(CipherAlgorithm::Aes256Ctr),
        "des-ede3-cbc" => Ok(CipherAlgorithm::DesEde3Cbc),
        "aes-128-gcm" => Ok(CipherAlgorithm::Aes128Gcm),
        "aes-192-gcm" => Ok(CipherAlgorithm::Aes192Gcm),
        "aes-256-gcm" => Ok(CipherAlgorithm::Aes256Gcm),
        "chacha20-poly1305" | "chacha20poly1305" => Ok(CipherAlgorithm::ChaCha20Poly1305),
        _ => Err(CryptoError::UnsupportedAlgorithm(name.to_string())),
    }
}

const AEAD_TAG_LEN: usize = 16;

/// One-shot AEAD encrypt (combined ciphertext||tag split out).
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
    let aead = algo.aead_cipher().ok_or_else(|| {
        CryptoError::UnsupportedAlgorithm("encrypt() requires an AEAD cipher".into())
    })?;
    if key.len() != algo.key_len() {
        return Err(CryptoError::InvalidKeyLength {
            expected: algo.key_len(),
            got: key.len(),
        });
    }
    if iv.len() != algo.nonce_len() {
        return Err(CryptoError::InvalidNonceLength {
            expected: algo.nonce_len(),
            got: iv.len(),
        });
    }

    let aad_data = aad.unwrap_or(&[]);
    let max_out = plaintext.len() + AEAD_TAG_LEN;
    let mut ctx = AeadCtx::new(aead, key)?;
    let mut out = vec![0u8; max_out];
    let mut out_len: usize = 0;

    let rc = unsafe {
        bssl::EVP_AEAD_CTX_seal(
            ctx.ctx_ptr(),
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

    let ct_len = out_len - AEAD_TAG_LEN;
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
    let aead = algo.aead_cipher().ok_or_else(|| {
        CryptoError::UnsupportedAlgorithm("decrypt() requires an AEAD cipher".into())
    })?;
    if key.len() != algo.key_len() {
        return Err(CryptoError::InvalidKeyLength {
            expected: algo.key_len(),
            got: key.len(),
        });
    }
    if iv.len() != algo.nonce_len() {
        return Err(CryptoError::InvalidNonceLength {
            expected: algo.nonce_len(),
            got: iv.len(),
        });
    }
    if tag.len() != AEAD_TAG_LEN {
        return Err(CryptoError::DecryptionFailed(format!(
            "invalid tag length: expected {AEAD_TAG_LEN}, got {}",
            tag.len()
        )));
    }

    let aad_data = aad.unwrap_or(&[]);
    let mut ctx = AeadCtx::new(aead, key)
        .map_err(|_| CryptoError::DecryptionFailed("EVP_AEAD_CTX_init failed".into()))?;

    let mut combined = Vec::with_capacity(ciphertext.len() + tag.len());
    combined.extend_from_slice(ciphertext);
    combined.extend_from_slice(tag);

    let mut out = vec![0u8; ciphertext.len()];
    let mut out_len: usize = 0;
    let rc = unsafe {
        bssl::EVP_AEAD_CTX_open(
            ctx.ctx_ptr(),
            out.as_mut_ptr(),
            &mut out_len,
            ciphertext.len(),
            iv.as_ptr(),
            iv.len(),
            combined.as_ptr(),
            combined.len(),
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

// ── Streaming CipherCtx (update / final, AEAD + non-AEAD) ─────────────────────

/// Operation direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Encrypt,
    Decrypt,
}

/// Buffer-mode selection for AEAD ciphers (which cannot stream natively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AeadPhase {
    Accumulating,
    Done,
}

/// Streaming cipher context supporting Node's update()/final() flow.
///
/// - Non-AEAD (AES-CBC/CTR, DES-EDE3-CBC): true streaming via EVP_Cipher*.
/// - AEAD (AES-GCM, ChaCha20-Poly1305): BoringSSL's EVP_AEAD is one-shot, so
///   data is buffered during update() and the AEAD op runs at final(). For
///   encryption the tag is captured and read via `take_auth_tag()`; for
///   decryption the caller must `set_auth_tag()` before final().
pub struct CipherCtx {
    algo: CipherAlgorithm,
    direction: Direction,
    iv: Vec<u8>,
    key: Vec<u8>,
    // Non-AEAD streaming state.
    block_ctx: Option<EvpCipherCtx>,
    // AEAD buffering state.
    aead_buffer: Vec<u8>,
    aead_tag: Vec<u8>,
    aead_aad: Vec<u8>,
    aead_phase: AeadPhase,
    aead_result: Vec<u8>,
    finalized: bool,
}

impl CipherCtx {
    /// Create a streaming context bound to `algo` / `key` / `iv`.
    pub fn new(
        algo: CipherAlgorithm,
        key: &[u8],
        iv: &[u8],
        direction: Direction,
    ) -> Result<Self, CryptoError> {
        if key.len() != algo.key_len() {
            return Err(CryptoError::InvalidKeyLength {
                expected: algo.key_len(),
                got: key.len(),
            });
        }
        if iv.len() != algo.nonce_len() {
            return Err(CryptoError::InvalidNonceLength {
                expected: algo.nonce_len(),
                got: iv.len(),
            });
        }

        let block_ctx = if !algo.is_aead() {
            let cipher = algo.block_cipher().ok_or_else(|| {
                CryptoError::UnsupportedAlgorithm("missing block cipher getter".into())
            })?;
            let mut ctx = EvpCipherCtx::new()?;
            ctx.init(cipher, key, iv, direction == Direction::Encrypt)?;
            Some(ctx)
        } else {
            None
        };

        Ok(CipherCtx {
            algo,
            direction,
            iv: iv.to_vec(),
            key: key.to_vec(),
            block_ctx,
            aead_buffer: Vec::new(),
            aead_tag: Vec::new(),
            aead_aad: Vec::new(),
            aead_phase: AeadPhase::Accumulating,
            aead_result: Vec::new(),
            finalized: false,
        })
    }

    /// Feed input bytes; return the bytes produced so far.
    /// For non-AEAD this is a real streaming update; for AEAD it accumulates.
    pub fn update(&mut self, input: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if self.finalized {
            return Err(CryptoError::EncryptionFailed("update() after final()".into()));
        }
        if self.algo.is_aead() {
            self.aead_buffer.extend_from_slice(input);
            Ok(Vec::new())
        } else {
            self.block_ctx
                .as_mut()
                .map(|c| c.update(input))
                .unwrap_or_else(|| Err(CryptoError::EncryptionFailed("no block ctx".into())))
        }
    }

    /// Additional authenticated data for AEAD (must be set before final()).
    pub fn update_aad(&mut self, aad: &[u8]) -> Result<(), CryptoError> {
        if self.finalized || !self.algo.is_aead() {
            return Ok(());
        }
        self.aead_aad.extend_from_slice(aad);
        Ok(())
    }

    /// Provide the auth tag for AEAD decryption (set before final()).
    pub fn set_auth_tag(&mut self, tag: &[u8]) -> Result<(), CryptoError> {
        if !self.algo.is_aead() {
            return Err(CryptoError::UnsupportedAlgorithm("set_auth_tag on non-AEAD".into()));
        }
        if tag.len() != AEAD_TAG_LEN {
            return Err(CryptoError::InvalidLength(format!(
                "auth tag must be {AEAD_TAG_LEN} bytes, got {}",
                tag.len()
            )));
        }
        self.aead_tag = tag.to_vec();
        Ok(())
    }

    /// Finalize the operation and return trailing bytes (and, for AEAD encrypt,
    /// the full ciphertext since AEAD buffers until final).
    pub fn final_ex(&mut self) -> Result<Vec<u8>, CryptoError> {
        if self.finalized {
            return Err(CryptoError::EncryptionFailed("final() already called".into()));
        }
        self.finalized = true;

        if self.algo.is_aead() {
            return self.final_aead();
        }
        self.block_ctx
            .as_mut()
            .map(|c| c.final_ex())
            .unwrap_or_else(|| Err(CryptoError::EncryptionFailed("no block ctx".into())))
    }

    fn final_aead(&mut self) -> Result<Vec<u8>, CryptoError> {
        if self.aead_phase != AeadPhase::Accumulating {
            return Err(CryptoError::EncryptionFailed("AEAD already finalized".into()));
        }
        self.aead_phase = AeadPhase::Done;
        let aead = self.algo.aead_cipher().ok_or_else(|| {
            CryptoError::UnsupportedAlgorithm("no AEAD getter for algorithm".into())
        })?;

        match self.direction {
            Direction::Encrypt => {
                let plaintext = core::mem::take(&mut self.aead_buffer);
                let max_out = plaintext.len() + AEAD_TAG_LEN;
                let mut ctx = AeadCtx::new(aead, &self.key)?;
                let mut out = vec![0u8; max_out];
                let mut out_len: usize = 0;
                let aad: &[u8] = self.aead_aad.as_slice();
                let rc = unsafe {
                    bssl::EVP_AEAD_CTX_seal(
                        ctx.ctx_ptr(),
                        out.as_mut_ptr(),
                        &mut out_len,
                        max_out,
                        self.iv.as_ptr(),
                        self.iv.len(),
                        plaintext.as_ptr(),
                        plaintext.len(),
                        aad.as_ptr(),
                        aad.len(),
                    )
                };
                if rc != 1 {
                    return Err(CryptoError::EncryptionFailed("EVP_AEAD_CTX_seal failed".into()));
                }
                let ct_len = out_len - AEAD_TAG_LEN;
                self.aead_tag = out[ct_len..out_len].to_vec();
                Ok(out[..ct_len].to_vec())
            }
            Direction::Decrypt => {
                if self.aead_tag.len() != AEAD_TAG_LEN {
                    return Err(CryptoError::DecryptionFailed(
                        "missing auth tag for AEAD decrypt".into(),
                    ));
                }
                let ciphertext = core::mem::take(&mut self.aead_buffer);
                let mut combined = Vec::with_capacity(ciphertext.len() + self.aead_tag.len());
                combined.extend_from_slice(&ciphertext);
                combined.extend_from_slice(&self.aead_tag);

                let mut ctx = AeadCtx::new(aead, &self.key)
                    .map_err(|_| CryptoError::DecryptionFailed("EVP_AEAD_CTX_init failed".into()))?;
                let mut out = vec![0u8; ciphertext.len()];
                let mut out_len: usize = 0;
                let aad: &[u8] = self.aead_aad.as_slice();
                let rc = unsafe {
                    bssl::EVP_AEAD_CTX_open(
                        ctx.ctx_ptr(),
                        out.as_mut_ptr(),
                        &mut out_len,
                        ciphertext.len(),
                        self.iv.as_ptr(),
                        self.iv.len(),
                        combined.as_ptr(),
                        combined.len(),
                        aad.as_ptr(),
                        aad.len(),
                    )
                };
                if rc != 1 {
                    return Err(CryptoError::DecryptionFailed("EVP_AEAD_CTX_open failed".into()));
                }
                out.truncate(out_len);
                self.aead_result = out.clone();
                Ok(out)
            }
        }
    }

    /// Read the AEAD auth tag (valid after final() of an encrypt context).
    pub fn take_auth_tag(&mut self) -> Option<Vec<u8>> {
        if self.algo.is_aead() && self.direction == Direction::Encrypt && !self.aead_tag.is_empty() {
            Some(core::mem::take(&mut self.aead_tag))
        } else {
            None
        }
    }

    pub fn algorithm(&self) -> CipherAlgorithm {
        self.algo
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }
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

    // ── Non-AEAD streaming roundtrips ────────────────────────────────────────
    fn roundtrip_streaming(algo: CipherAlgorithm, klen: usize, ivlen: usize) {
        let key: Vec<u8> = (0..klen).map(|i| (i as u8).wrapping_mul(7).wrapping_add(1)).collect();
        let iv: Vec<u8> = (0..ivlen).map(|i| (0xa0u8).wrapping_add(i as u8)).collect();
        let pt = b"the quick brown fox 1234567890 !@#";

        let mut enc = CipherCtx::new(algo, &key, &iv, Direction::Encrypt).unwrap();
        let mut ct = enc.update(pt).unwrap();
        ct.extend_from_slice(&enc.final_ex().unwrap());
        assert!(!ct.is_empty());

        let mut dec = CipherCtx::new(algo, &key, &iv, Direction::Decrypt).unwrap();
        let mut rec = dec.update(&ct).unwrap();
        rec.extend_from_slice(&dec.final_ex().unwrap());
        assert_eq!(rec, pt);
    }

    #[test]
    fn aes_128_cbc_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::Aes128Cbc, 16, 16);
    }

    #[test]
    fn aes_192_cbc_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::Aes192Cbc, 24, 16);
    }

    #[test]
    fn aes_256_cbc_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::Aes256Cbc, 32, 16);
    }

    #[test]
    fn aes_128_ctr_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::Aes128Ctr, 16, 16);
    }

    #[test]
    fn aes_256_ctr_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::Aes256Ctr, 32, 16);
    }

    #[test]
    fn des_ede3_cbc_streaming_roundtrip() {
        roundtrip_streaming(CipherAlgorithm::DesEde3Cbc, 24, 8);
    }

    // ── AEAD streaming roundtrip (buffered via final) ────────────────────────
    #[test]
    fn aes_256_gcm_streaming_roundtrip() {
        let key: Vec<u8> = (0..32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(1)).collect();
        let iv: Vec<u8> = (0..12).map(|i| (0xa0u8).wrapping_add(i as u8)).collect();
        let pt = b"gcm secret payload";

        let mut enc = CipherCtx::new(CipherAlgorithm::Aes256Gcm, &key, &iv, Direction::Encrypt).unwrap();
        let _ = enc.update(pt).unwrap();
        let ct = enc.final_ex().unwrap();
        let tag = enc.take_auth_tag().unwrap();
        assert_eq!(tag.len(), 16);

        let mut dec = CipherCtx::new(CipherAlgorithm::Aes256Gcm, &key, &iv, Direction::Decrypt).unwrap();
        dec.set_auth_tag(&tag).unwrap();
        let _ = dec.update(&ct).unwrap();
        let rec = dec.final_ex().unwrap();
        assert_eq!(rec, pt);
    }

    #[test]
    fn chacha20_poly1305_streaming_roundtrip() {
        let key: Vec<u8> = (0..32).map(|i| (i as u8).wrapping_mul(3).wrapping_add(5)).collect();
        let iv: Vec<u8> = (0..12).map(|i| (0x50u8).wrapping_add(i as u8)).collect();
        let pt = b"chacha aead";

        let mut enc = CipherCtx::new(CipherAlgorithm::ChaCha20Poly1305, &key, &iv, Direction::Encrypt).unwrap();
        let _ = enc.update(pt).unwrap();
        let ct = enc.final_ex().unwrap();
        let tag = enc.take_auth_tag().unwrap();
        assert_eq!(tag.len(), 16);

        let mut dec = CipherCtx::new(CipherAlgorithm::ChaCha20Poly1305, &key, &iv, Direction::Decrypt).unwrap();
        dec.set_auth_tag(&tag).unwrap();
        let _ = dec.update(&ct).unwrap();
        let rec = dec.final_ex().unwrap();
        assert_eq!(rec, pt);
    }

    #[test]
    fn aes_256_gcm_tampered_tag_fails() {
        let key: Vec<u8> = (0..32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(1)).collect();
        let iv: Vec<u8> = (0..12).map(|i| (0xa0u8).wrapping_add(i as u8)).collect();
        let pt = b"tamper me";

        let mut enc = CipherCtx::new(CipherAlgorithm::Aes256Gcm, &key, &iv, Direction::Encrypt).unwrap();
        let _ = enc.update(pt).unwrap();
        let ct = enc.final_ex().unwrap();
        let mut tag = enc.take_auth_tag().unwrap();
        tag[0] ^= 0xff;

        let mut dec = CipherCtx::new(CipherAlgorithm::Aes256Gcm, &key, &iv, Direction::Decrypt).unwrap();
        dec.set_auth_tag(&tag).unwrap();
        let _ = dec.update(&ct).unwrap();
        assert!(dec.final_ex().is_err());
    }
}
