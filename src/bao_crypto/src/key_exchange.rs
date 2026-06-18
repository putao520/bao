use crate::CryptoError;
use bun_boringssl_sys::*;
use core::ffi::c_void;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcCurve {
    P256,
    P384,
    X25519,
}

pub fn parse_curve(name: &str) -> Result<EcCurve, CryptoError> {
    match name.to_lowercase().as_str() {
        "p256" | "prime256v1" | "secp256r1" => Ok(EcCurve::P256),
        "p384" | "secp384r1" => Ok(EcCurve::P384),
        "x25519" => Ok(EcCurve::X25519),
        _ => Err(CryptoError::InvalidCurve(format!(
            "Unsupported curve: {}",
            name
        ))),
    }
}

fn curve_nid(curve: EcCurve) -> core::ffi::c_int {
    match curve {
        EcCurve::P256 => NID_X9_62_prime256v1,
        EcCurve::P384 => NID_secp384r1,
        EcCurve::X25519 => NID_X25519,
    }
}

fn curve_priv_len(curve: EcCurve) -> usize {
    match curve {
        EcCurve::P256 => 32,
        EcCurve::P384 => 48,
        EcCurve::X25519 => 32,
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

struct BnCtxGuard(*mut BN_CTX);
impl Drop for BnCtxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { BN_CTX_free(self.0) };
        }
    }
}

struct EcPointGuard(*mut EC_POINT);
impl Drop for EcPointGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { EC_POINT_free(self.0) };
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

struct PkeyGuard(*mut EVP_PKEY);
impl Drop for PkeyGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { EVP_PKEY_free(self.0) };
        }
    }
}

struct PkeyCtxGuard(*mut EVP_PKEY_CTX);
impl Drop for PkeyCtxGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { EVP_PKEY_CTX_free(self.0) };
        }
    }
}

fn ec_extract_private_bytes(key: *mut EC_KEY, priv_len: usize) -> Result<Vec<u8>, CryptoError> {
    let priv_bn = unsafe { EC_KEY_get0_private_key(key) };
    if priv_bn.is_null() {
        return Err(CryptoError::KeyGenerationFailed("EC_KEY private key null".into()));
    }
    let num_bits = unsafe { BN_num_bits(priv_bn) } as usize;
    let bn_len = (num_bits + 7) / 8;
    let mut priv_buf = vec![0u8; priv_len];
    let mut bn_raw = vec![0u8; bn_len];
    unsafe { BN_bn2bin(priv_bn, bn_raw.as_mut_ptr()) };
    let offset = priv_len.saturating_sub(bn_len);
    priv_buf[offset..].copy_from_slice(&bn_raw);
    Ok(priv_buf)
}

fn ec_extract_public_bytes(
    key: *mut EC_KEY,
    ctx: *mut BN_CTX,
) -> Result<Vec<u8>, CryptoError> {
    let pub_point = unsafe { EC_KEY_get0_public_key(key) };
    let group = unsafe { EC_KEY_get0_group(key) };
    if pub_point.is_null() || group.is_null() {
        return Err(CryptoError::KeyGenerationFailed("EC_KEY public key/group null".into()));
    }
    let pub_len = unsafe {
        EC_POINT_point2oct(
            group,
            pub_point,
            POINT_CONVERSION_UNCOMPRESSED,
            std::ptr::null_mut(),
            0,
            ctx,
        )
    };
    if pub_len == 0 {
        return Err(CryptoError::KeyGenerationFailed("EC_POINT_point2oct size query failed".into()));
    }
    let mut pub_buf = vec![0u8; pub_len];
    let written = unsafe {
        EC_POINT_point2oct(
            group,
            pub_point,
            POINT_CONVERSION_UNCOMPRESSED,
            pub_buf.as_mut_ptr(),
            pub_len,
            ctx,
        )
    };
    if written == 0 {
        return Err(CryptoError::KeyGenerationFailed("EC_POINT_point2oct failed".into()));
    }
    Ok(pub_buf)
}

fn boringssl_ec_generate(curve: EcCurve) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let nid = curve_nid(curve);
    let priv_len = curve_priv_len(curve);

    let key = EcKeyGuard(unsafe { EC_KEY_new_by_curve_name(nid) });
    if key.0.is_null() {
        return Err(CryptoError::KeyGenerationFailed("EC_KEY_new_by_curve_name failed".into()));
    }
    if unsafe { EC_KEY_generate_key(key.0) } != 1 {
        return Err(CryptoError::KeyGenerationFailed("EC_KEY_generate_key failed".into()));
    }

    let ctx = BnCtxGuard(unsafe { BN_CTX_new() });
    if ctx.0.is_null() {
        return Err(CryptoError::KeyGenerationFailed("BN_CTX_new failed".into()));
    }

    let priv_bytes = ec_extract_private_bytes(key.0, priv_len)?;
    let pub_bytes = ec_extract_public_bytes(key.0, ctx.0)?;
    Ok((priv_bytes, pub_bytes))
}

fn boringssl_ec_reconstruct(
    curve: EcCurve,
    private_bytes: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let nid = curve_nid(curve);
    let priv_len = curve_priv_len(curve);

    if private_bytes.len() != priv_len {
        return Err(CryptoError::InvalidKeyLength {
            expected: priv_len,
            got: private_bytes.len(),
        });
    }

    let key = EcKeyGuard(unsafe { EC_KEY_new_by_curve_name(nid) });
    if key.0.is_null() {
        return Err(CryptoError::InvalidKey("EC_KEY_new_by_curve_name failed".into()));
    }

    let priv_bn = BnGuard(unsafe {
        BN_bin2bn(
            private_bytes.as_ptr(),
            private_bytes.len(),
            std::ptr::null_mut(),
        )
    });
    if priv_bn.0.is_null() {
        return Err(CryptoError::InvalidKey("BN_bin2bn failed".into()));
    }

    if unsafe { EC_KEY_set_private_key(key.0, priv_bn.0) } != 1 {
        return Err(CryptoError::InvalidKey("EC_KEY_set_private_key failed".into()));
    }

    let group = unsafe { EC_KEY_get0_group(key.0) };
    let pub_point = EcPointGuard(unsafe { EC_POINT_new(group) });
    if pub_point.0.is_null() {
        return Err(CryptoError::InvalidKey("EC_POINT_new failed".into()));
    }

    let ctx = BnCtxGuard(unsafe { BN_CTX_new() });
    if ctx.0.is_null() {
        return Err(CryptoError::InvalidKey("BN_CTX_new failed".into()));
    }

    if unsafe {
        EC_POINT_mul(
            group,
            pub_point.0,
            priv_bn.0,
            std::ptr::null(),
            std::ptr::null(),
            ctx.0,
        )
    } != 1 {
        return Err(CryptoError::InvalidKey("EC_POINT_mul failed".into()));
    }

    if unsafe { EC_KEY_set_public_key(key.0, pub_point.0) } != 1 {
        return Err(CryptoError::InvalidKey("EC_KEY_set_public_key failed".into()));
    }

    ec_extract_public_bytes(key.0, ctx.0)
}

/// Generate X25519 keypair using BoringSSL EVP_PKEY API.
///
/// BoringSSL does not support X25519 via EVP_PKEY_CTX_new_id keygen; generate a
/// 32-byte seed, lift it into an EVP_PKEY via EVP_PKEY_from_raw_private_key,
/// then extract the raw private/public bytes.
fn x25519_generate() -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let mut seed = [0u8; 32];
    crate::random::rand_bytes(&mut seed)?;
    unsafe {
        let pkey = PkeyGuard(EVP_PKEY_from_raw_private_key(
            EVP_pkey_x25519(),
            seed.as_ptr(),
            seed.len(),
        ));
        if pkey.0.is_null() {
            return Err(CryptoError::KeyGenerationFailed("EVP_PKEY_from_raw_private_key (x25519) failed".into()));
        }

        let mut priv_len: usize = 0;
        if EVP_PKEY_get_raw_private_key(pkey.0, std::ptr::null_mut(), &mut priv_len) != 1 {
            return Err(CryptoError::KeyGenerationFailed("EVP_PKEY_get_raw_private_key size query failed".into()));
        }
        let mut priv_bytes = vec![0u8; priv_len];
        if EVP_PKEY_get_raw_private_key(pkey.0, priv_bytes.as_mut_ptr(), &mut priv_len) != 1 {
            return Err(CryptoError::KeyGenerationFailed("EVP_PKEY_get_raw_private_key failed".into()));
        }

        let mut pub_len: usize = 0;
        if EVP_PKEY_get_raw_public_key(pkey.0, std::ptr::null_mut(), &mut pub_len) != 1 {
            return Err(CryptoError::KeyGenerationFailed("EVP_PKEY_get_raw_public_key size query failed".into()));
        }
        let mut pub_bytes = vec![0u8; pub_len];
        if EVP_PKEY_get_raw_public_key(pkey.0, pub_bytes.as_mut_ptr(), &mut pub_len) != 1 {
            return Err(CryptoError::KeyGenerationFailed("EVP_PKEY_get_raw_public_key failed".into()));
        }

        Ok((priv_bytes, pub_bytes))
    }
}

/// Reconstruct X25519 public key from private key using BoringSSL EVP_PKEY API.
fn x25519_reconstruct(private_bytes: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if private_bytes.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            got: private_bytes.len(),
        });
    }
    unsafe {
        let pkey = PkeyGuard(EVP_PKEY_from_raw_private_key(
            EVP_pkey_x25519(),
            private_bytes.as_ptr(),
            private_bytes.len(),
        ));
        if pkey.0.is_null() {
            return Err(CryptoError::InvalidKey("EVP_PKEY_from_raw_private_key failed".into()));
        }

        let mut pub_len: usize = 0;
        if EVP_PKEY_get_raw_public_key(pkey.0, std::ptr::null_mut(), &mut pub_len) != 1 {
            return Err(CryptoError::InvalidKey("EVP_PKEY_get_raw_public_key size query failed".into()));
        }
        let mut pub_bytes = vec![0u8; pub_len];
        if EVP_PKEY_get_raw_public_key(pkey.0, pub_bytes.as_mut_ptr(), &mut pub_len) != 1 {
            return Err(CryptoError::InvalidKey("EVP_PKEY_get_raw_public_key failed".into()));
        }

        Ok(pub_bytes)
    }
}

/// Compute X25519 shared secret using BoringSSL EVP_PKEY_derive API.
fn x25519_derive(private_bytes: &[u8], peer_public: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if private_bytes.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            got: private_bytes.len(),
        });
    }
    if peer_public.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            got: peer_public.len(),
        });
    }
    unsafe {
        let our_pkey = PkeyGuard(EVP_PKEY_from_raw_private_key(
            EVP_pkey_x25519(),
            private_bytes.as_ptr(),
            private_bytes.len(),
        ));
        if our_pkey.0.is_null() {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_from_raw_private_key failed".into()));
        }

        let peer_pkey = PkeyGuard(EVP_PKEY_from_raw_public_key(
            EVP_pkey_x25519(),
            peer_public.as_ptr(),
            peer_public.len(),
        ));
        if peer_pkey.0.is_null() {
            return Err(CryptoError::InvalidKey("EVP_PKEY_from_raw_public_key for peer failed".into()));
        }

        let ctx = PkeyCtxGuard(EVP_PKEY_CTX_new(our_pkey.0, std::ptr::null_mut()));
        if ctx.0.is_null() {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_CTX_new failed".into()));
        }

        if EVP_PKEY_derive_init(ctx.0) != 1 {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_derive_init failed".into()));
        }
        if EVP_PKEY_derive_set_peer(ctx.0, peer_pkey.0) != 1 {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_derive_set_peer failed".into()));
        }

        let mut key_len: usize = 0;
        if EVP_PKEY_derive(ctx.0, std::ptr::null_mut(), &mut key_len) != 1 {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_derive size query failed".into()));
        }

        let mut shared = vec![0u8; key_len];
        if EVP_PKEY_derive(ctx.0, shared.as_mut_ptr(), &mut key_len) != 1 {
            return Err(CryptoError::SharedSecretFailed("EVP_PKEY_derive failed".into()));
        }

        Ok(shared)
    }
}

pub struct EcdhKeyPair {
    curve: EcCurve,
    private_bytes: Vec<u8>,
    public_bytes: Vec<u8>,
}

impl EcdhKeyPair {
    pub fn generate(curve: EcCurve) -> Result<EcdhKeyPair, CryptoError> {
        let (priv_bytes, pub_bytes) = match curve {
            EcCurve::P256 | EcCurve::P384 => boringssl_ec_generate(curve)?,
            EcCurve::X25519 => x25519_generate()?,
        };
        Ok(EcdhKeyPair {
            curve,
            private_bytes: priv_bytes,
            public_bytes: pub_bytes,
        })
    }

    pub fn reconstruct_keypair(
        curve: EcCurve,
        private_bytes: &[u8],
    ) -> Result<EcdhKeyPair, CryptoError> {
        let pub_bytes = match curve {
            EcCurve::P256 | EcCurve::P384 => boringssl_ec_reconstruct(curve, private_bytes)?,
            EcCurve::X25519 => x25519_reconstruct(private_bytes)?,
        };
        Ok(EcdhKeyPair {
            curve,
            private_bytes: private_bytes.to_vec(),
            public_bytes: pub_bytes,
        })
    }

    pub fn compute_shared_secret(&self, other_pub: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match self.curve {
            EcCurve::P256 | EcCurve::P384 => {
                let nid = curve_nid(self.curve);

                let key = EcKeyGuard(unsafe { EC_KEY_new_by_curve_name(nid) });
                if key.0.is_null() {
                    return Err(CryptoError::SharedSecretFailed("EC_KEY_new_by_curve_name failed".into()));
                }

                let priv_bn = BnGuard(unsafe {
                    BN_bin2bn(
                        self.private_bytes.as_ptr(),
                        self.private_bytes.len(),
                        std::ptr::null_mut(),
                    )
                });
                if priv_bn.0.is_null() {
                    return Err(CryptoError::SharedSecretFailed("BN_bin2bn failed".into()));
                }
                if unsafe { EC_KEY_set_private_key(key.0, priv_bn.0) } != 1 {
                    return Err(CryptoError::SharedSecretFailed("EC_KEY_set_private_key failed".into()));
                }

                let group = unsafe { EC_KEY_get0_group(key.0) };
                let pub_point = EcPointGuard(unsafe { EC_POINT_new(group) });
                if pub_point.0.is_null() {
                    return Err(CryptoError::SharedSecretFailed("EC_POINT_new failed".into()));
                }

                let ctx = BnCtxGuard(unsafe { BN_CTX_new() });
                if ctx.0.is_null() {
                    return Err(CryptoError::SharedSecretFailed("BN_CTX_new failed".into()));
                }

                if unsafe {
                    EC_POINT_mul(
                        group,
                        pub_point.0,
                        priv_bn.0,
                        std::ptr::null(),
                        std::ptr::null(),
                        ctx.0,
                    )
                } != 1 {
                    return Err(CryptoError::SharedSecretFailed("EC_POINT_mul failed".into()));
                }
                if unsafe { EC_KEY_set_public_key(key.0, pub_point.0) } != 1 {
                    return Err(CryptoError::SharedSecretFailed("EC_KEY_set_public_key failed".into()));
                }

                let peer_point = EcPointGuard(unsafe { EC_POINT_new(group) });
                if peer_point.0.is_null() {
                    return Err(CryptoError::SharedSecretFailed("EC_POINT_new for peer failed".into()));
                }
                if unsafe {
                    EC_POINT_oct2point(
                        group,
                        peer_point.0,
                        other_pub.as_ptr(),
                        other_pub.len(),
                        ctx.0,
                    )
                } != 1 {
                    return Err(CryptoError::InvalidKey("Failed to parse peer public key".into()));
                }

                let secret_len = curve_priv_len(self.curve);
                let mut shared = vec![0u8; secret_len];
                let ret = unsafe {
                    ECDH_compute_key(
                        shared.as_mut_ptr() as *mut c_void,
                        secret_len,
                        peer_point.0,
                        key.0,
                        None,
                    )
                };
                if ret <= 0 {
                    return Err(CryptoError::SharedSecretFailed("ECDH_compute_key failed".into()));
                }
                shared.truncate(ret as usize);
                Ok(shared)
            }
            EcCurve::X25519 => x25519_derive(&self.private_bytes, other_pub),
        }
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.public_bytes.clone()
    }

    pub fn private_key_bytes(&self) -> Vec<u8> {
        self.private_bytes.clone()
    }
}
