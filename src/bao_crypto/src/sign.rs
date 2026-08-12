use crate::CryptoError;
use bun_boringssl_sys::*;
use core::ffi::{c_long, c_void};
use core::ptr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsaHash {
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignAlgorithm {
    RsaPkcs1v15 { hash: RsaHash },
    RsaPss { hash: RsaHash },
    EcdsaP256,
    EcdsaP384,
    Ed25519,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureFormat {
    Der,
    Raw,
}

pub struct Signer {
    pkey: *mut EVP_PKEY,
    algo: SignAlgorithm,
}

impl Drop for Signer {
    fn drop(&mut self) {
        if !self.pkey.is_null() {
            unsafe { EVP_PKEY_free(self.pkey) };
        }
    }
}

unsafe impl Send for Signer {}

fn hash_md(hash: &RsaHash) -> *const EVP_MD {
    match hash {
        RsaHash::Sha256 => EVP_sha256(),
        RsaHash::Sha384 => EVP_sha384(),
        RsaHash::Sha512 => EVP_sha512(),
    }
}

fn algo_md(algo: &SignAlgorithm) -> *const EVP_MD {
    match algo {
        SignAlgorithm::RsaPkcs1v15 { hash } | SignAlgorithm::RsaPss { hash } => hash_md(hash),
        SignAlgorithm::EcdsaP256 => EVP_sha256(),
        SignAlgorithm::EcdsaP384 => EVP_sha384(),
        SignAlgorithm::Ed25519 => ptr::null(),
    }
}

fn parse_pem_to_pkey(pem: &str) -> Result<*mut EVP_PKEY, CryptoError> {
    unsafe {
        let bio = BIO_new_mem_buf(pem.as_ptr() as *const c_void, pem.len() as isize);
        if bio.is_null() {
            return Err(CryptoError::InvalidKey("BIO_new_mem_buf failed".into()));
        }
        let pkey = PEM_read_bio_PrivateKey(
            bio,
            ptr::null_mut(),
            None::<pem_password_cb>,
            ptr::null_mut(),
        );
        BIO_free(bio);
        if pkey.is_null() {
            return Err(CryptoError::InvalidKey(
                "PEM_read_bio_PrivateKey failed".into(),
            ));
        }
        Ok(pkey)
    }
}

fn parse_der_to_pkey(der: &[u8]) -> Result<*mut EVP_PKEY, CryptoError> {
    unsafe {
        let mut inp = der.as_ptr();
        let pkey = d2i_AutoPrivateKey(ptr::null_mut(), &mut inp, der.len() as c_long);
        if pkey.is_null() {
            return Err(CryptoError::InvalidKey("d2i_AutoPrivateKey failed".into()));
        }
        Ok(pkey)
    }
}

impl Signer {
    pub fn from_pkcs8_pem(algo: &SignAlgorithm, pem: &str) -> Result<Signer, CryptoError> {
        let pkey = parse_pem_to_pkey(pem)?;
        Ok(Signer {
            pkey,
            algo: algo.clone(),
        })
    }

    pub fn from_pkcs8_der(algo: &SignAlgorithm, der: &[u8]) -> Result<Signer, CryptoError> {
        let pkey = parse_der_to_pkey(der)?;
        Ok(Signer {
            pkey,
            algo: algo.clone(),
        })
    }

    pub fn from_pkey(pkey: *mut EVP_PKEY, algo: &SignAlgorithm) -> Result<Signer, CryptoError> {
        if pkey.is_null() {
            return Err(CryptoError::InvalidKey("null EVP_PKEY".into()));
        }
        Ok(Signer {
            pkey,
            algo: algo.clone(),
        })
    }

    pub fn sign(&self, data: &[u8], format: SignatureFormat) -> Result<Vec<u8>, CryptoError> {
        // Ed25519: one-shot EVP_PKEY_sign (streaming not supported by BoringSSL)
        if self.algo == SignAlgorithm::Ed25519 {
            return self.sign_ed25519(data);
        }

        unsafe {
            let mut md_ctx: EVP_MD_CTX = core::mem::zeroed();
            EVP_MD_CTX_init(&mut md_ctx);

            let md = algo_md(&self.algo);
            let mut pctx: *mut EVP_PKEY_CTX = ptr::null_mut();

            let init_result = if md.is_null() {
                EVP_DigestSignInit(
                    &mut md_ctx,
                    &mut pctx,
                    ptr::null(),
                    ptr::null_mut(),
                    self.pkey,
                )
            } else {
                EVP_DigestSignInit(&mut md_ctx, &mut pctx, md, ptr::null_mut(), self.pkey)
            };

            if init_result != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed("EVP_DigestSignInit failed".into()));
            }

            if let SignAlgorithm::RsaPss { .. } = self.algo {
                if !pctx.is_null() {
                    EVP_PKEY_CTX_set_rsa_padding(pctx, RSA_PKCS1_PSS_PADDING);
                    EVP_PKEY_CTX_set_rsa_pss_saltlen(pctx, RSA_PSS_SALTLEN_DIGEST);
                }
            }

            if EVP_DigestSignUpdate(&mut md_ctx, data.as_ptr() as *const c_void, data.len()) != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed(
                    "EVP_DigestSignUpdate failed".into(),
                ));
            }

            let mut sig_len: usize = 0;
            if EVP_DigestSignFinal(&mut md_ctx, ptr::null_mut(), &mut sig_len) != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed(
                    "EVP_DigestSignFinal (size query) failed".into(),
                ));
            }

            let mut sig = vec![0u8; sig_len];
            if EVP_DigestSignFinal(&mut md_ctx, sig.as_mut_ptr(), &mut sig_len) != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed("EVP_DigestSignFinal failed".into()));
            }

            EVP_MD_CTX_cleanup(&mut md_ctx);
            sig.truncate(sig_len);

            match self.algo {
                SignAlgorithm::EcdsaP256 | SignAlgorithm::EcdsaP384 => match format {
                    SignatureFormat::Der => Ok(sig),
                    SignatureFormat::Raw => der_ecdsa_sig_to_raw(&sig),
                },
                _ => Ok(sig),
            }
        }
    }

    fn sign_ed25519(&self, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        unsafe {
            let mut md_ctx: EVP_MD_CTX = core::mem::zeroed();
            EVP_MD_CTX_init(&mut md_ctx);

            if EVP_DigestSignInit(
                &mut md_ctx,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                self.pkey,
            ) != 1
            {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed("EVP_DigestSignInit failed".into()));
            }

            let mut sig_len: usize = 0;
            if EVP_DigestSign(
                &mut md_ctx,
                ptr::null_mut(),
                &mut sig_len,
                data.as_ptr(),
                data.len(),
            ) != 1
            {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed(
                    "EVP_DigestSign (size query) failed".into(),
                ));
            }

            let mut sig = vec![0u8; sig_len];
            if EVP_DigestSign(
                &mut md_ctx,
                sig.as_mut_ptr(),
                &mut sig_len,
                data.as_ptr(),
                data.len(),
            ) != 1
            {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::SignFailed("EVP_DigestSign failed".into()));
            }

            EVP_MD_CTX_cleanup(&mut md_ctx);
            sig.truncate(sig_len);
            Ok(sig)
        }
    }
}

/// Decode DER-encoded ECDSA signature (SEQUENCE { INTEGER r, INTEGER s })
/// into raw r||s concatenation padded to field size.
fn der_ecdsa_sig_to_raw(der: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if der.len() < 8 || der[0] != 0x30 {
        return Err(CryptoError::DecodingFailed(
            "Invalid DER ECDSA signature".into(),
        ));
    }

    let (seq_len, len_size) = read_der_len(der, 1)?;
    let mut pos = 1 + len_size;
    if pos + seq_len > der.len() {
        return Err(CryptoError::DecodingFailed(
            "DER SEQUENCE length overflow".into(),
        ));
    }

    if der[pos] != 0x02 {
        return Err(CryptoError::DecodingFailed(
            "Expected INTEGER tag for r".into(),
        ));
    }
    pos += 1;
    let (r_len, len_size) = read_der_len(der, pos)?;
    pos += len_size;
    let r_bytes = &der[pos..pos + r_len];
    pos += r_len;

    if pos >= der.len() || der[pos] != 0x02 {
        return Err(CryptoError::DecodingFailed(
            "Expected INTEGER tag for s".into(),
        ));
    }
    pos += 1;
    let (s_len, len_size) = read_der_len(der, pos)?;
    pos += len_size;
    let s_bytes = &der[pos..pos + s_len];

    let r_stripped = strip_leading_zero_padding(r_bytes);
    let s_stripped = strip_leading_zero_padding(s_bytes);

    // Determine field size: P-256 = 32 bytes, P-384 = 48 bytes
    let field_size = if r_stripped.len() <= 32 && s_stripped.len() <= 32 {
        32
    } else {
        48
    };

    let mut raw = vec![0u8; field_size * 2];
    let r_offset = field_size - r_stripped.len();
    let s_offset = field_size - s_stripped.len();
    raw[r_offset..field_size].copy_from_slice(r_stripped);
    raw[field_size + s_offset..].copy_from_slice(s_stripped);
    Ok(raw)
}

/// Encode raw r||s ECDSA signature into DER format.
pub fn raw_ecdsa_sig_to_der(raw: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let field_size = raw.len() / 2;
    if field_size == 0 {
        return Err(CryptoError::DecodingFailed("Empty raw signature".into()));
    }
    let r_raw = &raw[..field_size];
    let s_raw = &raw[field_size..];

    let r_enc = der_encode_integer(r_raw);
    let s_enc = der_encode_integer(s_raw);

    let inner_len = r_enc.len() + s_enc.len();
    let mut der = Vec::with_capacity(2 + inner_len);
    der.push(0x30);
    append_der_len(&mut der, inner_len);
    der.extend_from_slice(&r_enc);
    der.extend_from_slice(&s_enc);
    Ok(der)
}

fn read_der_len(buf: &[u8], offset: usize) -> Result<(usize, usize), CryptoError> {
    if offset >= buf.len() {
        return Err(CryptoError::DecodingFailed("DER length overflow".into()));
    }
    let first = buf[offset];
    if first & 0x80 == 0 {
        Ok((first as usize, 1))
    } else {
        let num_bytes = (first & 0x7f) as usize;
        if num_bytes > 4 || offset + 1 + num_bytes > buf.len() {
            return Err(CryptoError::DecodingFailed("Invalid DER length".into()));
        }
        let mut len: usize = 0;
        for i in 0..num_bytes {
            len = (len << 8) | buf[offset + 1 + i] as usize;
        }
        Ok((len, 1 + num_bytes))
    }
}

/// Strip DER INTEGER leading-zero padding (added when high bit set).
fn strip_leading_zero_padding(bytes: &[u8]) -> &[u8] {
    if bytes.len() > 1 && bytes[0] == 0 && bytes[1] & 0x80 != 0 {
        &bytes[1..]
    } else {
        bytes
    }
}

fn der_encode_integer(val: &[u8]) -> Vec<u8> {
    let mut start = 0;
    while start < val.len() - 1 && val[start] == 0 {
        start += 1;
    }
    let stripped = &val[start..];

    let mut enc = Vec::with_capacity(stripped.len() + 3);
    enc.push(0x02);
    if stripped[0] & 0x80 != 0 {
        append_der_len(&mut enc, stripped.len() + 1);
        enc.push(0x00);
    } else {
        append_der_len(&mut enc, stripped.len());
    }
    enc.extend_from_slice(stripped);
    enc
}

fn append_der_len(buf: &mut Vec<u8>, len: usize) {
    if len < 128 {
        buf.push(len as u8);
    } else if len < 256 {
        buf.push(0x81);
        buf.push(len as u8);
    } else {
        buf.push(0x82);
        buf.push((len >> 8) as u8);
        buf.push((len & 0xff) as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_rsa_pkey() -> *mut EVP_PKEY {
        unsafe {
            let rsa = RSA_new();
            let bn = BN_new();
            BN_set_word(bn, 65537);
            assert_eq!(RSA_generate_key_ex(rsa, 2048, bn, ptr::null_mut()), 1);
            BN_free(bn);
            let pkey = EVP_PKEY_new();
            assert_eq!(EVP_PKEY_set1_RSA(pkey, rsa), 1);
            RSA_free(rsa);
            pkey
        }
    }

    fn generate_ec_pkey(nid: i32) -> *mut EVP_PKEY {
        unsafe {
            let ec_key = EC_KEY_new_by_curve_name(nid);
            assert_eq!(EC_KEY_generate_key(ec_key), 1);
            let pkey = EVP_PKEY_new();
            assert_eq!(EVP_PKEY_set1_EC_KEY(pkey, ec_key), 1);
            EC_KEY_free(ec_key);
            pkey
        }
    }

    fn generate_ed25519_pkey() -> *mut EVP_PKEY {
        unsafe {
            let mut seed = [0u8; 32];
            RAND_bytes(seed.as_mut_ptr(), 32);
            let pkey = EVP_PKEY_from_raw_private_key(EVP_pkey_ed25519(), seed.as_ptr(), 32);
            assert!(!pkey.is_null(), "EVP_PKEY_from_raw_private_key failed");
            pkey
        }
    }

    fn pkey_to_pkcs8_pem(pkey: *mut EVP_PKEY) -> String {
        unsafe {
            let bio = BIO_new(BIO_s_mem());
            assert!(!bio.is_null());
            assert_eq!(
                PEM_write_bio_PKCS8PrivateKey(
                    bio,
                    pkey,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                    None::<pem_password_cb>,
                    ptr::null_mut(),
                ),
                1
            );
            let pending = BIO_ctrl_pending(bio);
            let mut buf = vec![0u8; pending];
            let read_len = BIO_read(bio, buf.as_mut_ptr() as *mut c_void, pending as i32);
            BIO_free(bio);
            assert!(read_len > 0);
            String::from_utf8(buf[..read_len as usize].to_vec()).unwrap()
        }
    }

    fn pkey_to_pkcs8_der(pkey: *mut EVP_PKEY) -> Vec<u8> {
        unsafe {
            let len = i2d_PrivateKey(pkey, ptr::null_mut());
            assert!(len > 0);
            let mut buf = vec![0u8; len as usize];
            let mut out_ptr = buf.as_mut_ptr();
            i2d_PrivateKey(pkey, &mut out_ptr);
            buf
        }
    }

    #[test]
    fn rsa_pkcs1v15_sha256_sign_verify_roundtrip() {
        let pkey = generate_rsa_pkey();
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::RsaPkcs1v15 {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let data = b"hello rsa-pkcs1v15-sha256";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn rsa_pkcs1v15_sha256_der_input() {
        let pkey = generate_rsa_pkey();
        let der = pkey_to_pkcs8_der(pkey);
        let algo = SignAlgorithm::RsaPkcs1v15 {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_der(&algo, &der).unwrap();
        let data = b"hello rsa-der";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();

        let verifier = crate::verify::Verifier::from_pkcs8_der(&algo, &der).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn rsa_pss_sha256_sign_verify_roundtrip() {
        let pkey = generate_rsa_pkey();
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::RsaPss {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let data = b"hello rsa-pss-sha256";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn rsa_pss_sha384_sign_verify_roundtrip() {
        let pkey = generate_rsa_pkey();
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::RsaPss {
            hash: RsaHash::Sha384,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let data = b"hello rsa-pss-sha384";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn ecdsa_p256_sign_verify_roundtrip() {
        let pkey = generate_ec_pkey(NID_X9_62_prime256v1);
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::EcdsaP256;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ecdsa-p256";
        let sig_der = signer.sign(data, SignatureFormat::Der).unwrap();
        let sig_raw = signer.sign(data, SignatureFormat::Raw).unwrap();
        assert!(!sig_der.is_empty());
        assert!(!sig_raw.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(
            verifier
                .verify(data, &sig_der, SignatureFormat::Der)
                .unwrap()
        );
        assert!(
            verifier
                .verify(data, &sig_raw, SignatureFormat::Raw)
                .unwrap()
        );
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn ecdsa_p384_sign_verify_roundtrip() {
        let pkey = generate_ec_pkey(NID_secp384r1);
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::EcdsaP384;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ecdsa-p384";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(!sig.is_empty());

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        let pkey = generate_ed25519_pkey();
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::Ed25519;
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();

        let data = b"hello ed25519";
        let sig = signer.sign(data, SignatureFormat::Raw).unwrap();
        assert_eq!(sig.len(), 64);

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Raw).unwrap());
        unsafe { EVP_PKEY_free(pkey) };
    }

    #[test]
    fn sign_wrong_data_fails_verification() {
        let pkey = generate_rsa_pkey();
        let pem = pkey_to_pkcs8_pem(pkey);
        let algo = SignAlgorithm::RsaPkcs1v15 {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &pem).unwrap();
        let sig = signer.sign(b"original data", SignatureFormat::Der).unwrap();

        let verifier = crate::verify::Verifier::from_pkcs8_pem(&algo, &pem).unwrap();
        assert!(
            !verifier
                .verify(b"tampered data", &sig, SignatureFormat::Der)
                .unwrap()
        );
        unsafe { EVP_PKEY_free(pkey) };
    }
}
