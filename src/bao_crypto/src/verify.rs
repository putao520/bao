use crate::sign::{raw_ecdsa_sig_to_der, RsaHash, SignAlgorithm, SignatureFormat};
use crate::CryptoError;
use bun_boringssl_sys::*;
use core::ffi::{c_long, c_void};
use core::ptr;

pub struct Verifier {
    pkey: *mut EVP_PKEY,
    algo: SignAlgorithm,
}

impl Drop for Verifier {
    fn drop(&mut self) {
        if !self.pkey.is_null() {
            unsafe { EVP_PKEY_free(self.pkey) };
        }
    }
}

unsafe impl Send for Verifier {}

fn algo_md(algo: &SignAlgorithm) -> *const EVP_MD {
    match algo {
        SignAlgorithm::RsaPkcs1v15 { hash } | SignAlgorithm::RsaPss { hash } => match hash {
            RsaHash::Sha256 => EVP_sha256(),
            RsaHash::Sha384 => EVP_sha384(),
            RsaHash::Sha512 => EVP_sha512(),
        },
        SignAlgorithm::EcdsaP256 => EVP_sha256(),
        SignAlgorithm::EcdsaP384 => EVP_sha384(),
        SignAlgorithm::Ed25519 => ptr::null(),
    }
}

impl Verifier {
    pub fn from_pkcs8_pem(algo: &SignAlgorithm, pem: &str) -> Result<Verifier, CryptoError> {
        let pkey = unsafe {
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
                return Err(CryptoError::InvalidKey("PEM_read_bio_PrivateKey failed".into()));
            }
            pkey
        };
        Ok(Verifier { pkey, algo: algo.clone() })
    }

    pub fn from_pkcs8_der(algo: &SignAlgorithm, der: &[u8]) -> Result<Verifier, CryptoError> {
        let pkey = unsafe {
            let mut inp = der.as_ptr();
            let pkey = d2i_AutoPrivateKey(ptr::null_mut(), &mut inp, der.len() as c_long);
            if pkey.is_null() {
                return Err(CryptoError::InvalidKey("d2i_AutoPrivateKey failed".into()));
            }
            pkey
        };
        Ok(Verifier { pkey, algo: algo.clone() })
    }

    pub fn from_pkey(pkey: *mut EVP_PKEY, algo: &SignAlgorithm) -> Result<Verifier, CryptoError> {
        if pkey.is_null() {
            return Err(CryptoError::InvalidKey("null EVP_PKEY".into()));
        }
        Ok(Verifier { pkey, algo: algo.clone() })
    }

    pub fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        format: SignatureFormat,
    ) -> Result<bool, CryptoError> {
        // Ed25519: one-shot EVP_PKEY_verify (streaming not supported by BoringSSL)
        if self.algo == SignAlgorithm::Ed25519 {
            return self.verify_ed25519(data, signature);
        }

        unsafe {
            let mut md_ctx: EVP_MD_CTX = core::mem::zeroed();
            EVP_MD_CTX_init(&mut md_ctx);

            let md = algo_md(&self.algo);
            let mut pctx: *mut EVP_PKEY_CTX = ptr::null_mut();

            let init_result = if md.is_null() {
                EVP_DigestVerifyInit(
                    &mut md_ctx,
                    &mut pctx,
                    ptr::null(),
                    ptr::null_mut(),
                    self.pkey,
                )
            } else {
                EVP_DigestVerifyInit(&mut md_ctx, &mut pctx, md, ptr::null_mut(), self.pkey)
            };

            if init_result != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::VerifyFailed("EVP_DigestVerifyInit failed".into()));
            }

            if let SignAlgorithm::RsaPss { .. } = self.algo {
                if !pctx.is_null() {
                    EVP_PKEY_CTX_set_rsa_padding(pctx, RSA_PKCS1_PSS_PADDING);
                    EVP_PKEY_CTX_set_rsa_pss_saltlen(pctx, RSA_PSS_SALTLEN_DIGEST);
                }
            }

            if EVP_DigestVerifyUpdate(&mut md_ctx, data.as_ptr() as *const c_void, data.len()) != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::VerifyFailed("EVP_DigestVerifyUpdate failed".into()));
            }

            let sig_bytes = match self.algo {
                SignAlgorithm::EcdsaP256 | SignAlgorithm::EcdsaP384 => match format {
                    SignatureFormat::Der => signature.to_vec(),
                    SignatureFormat::Raw => raw_ecdsa_sig_to_der(signature)?,
                },
                _ => signature.to_vec(),
            };

            let result = EVP_DigestVerifyFinal(&mut md_ctx, sig_bytes.as_ptr(), sig_bytes.len());
            EVP_MD_CTX_cleanup(&mut md_ctx);

            Ok(result == 1)
        }
    }

    fn verify_ed25519(&self, data: &[u8], signature: &[u8]) -> Result<bool, CryptoError> {
        unsafe {
            let mut md_ctx: EVP_MD_CTX = core::mem::zeroed();
            EVP_MD_CTX_init(&mut md_ctx);

            if EVP_DigestVerifyInit(&mut md_ctx, ptr::null_mut(), ptr::null(), ptr::null_mut(), self.pkey) != 1 {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::VerifyFailed("EVP_DigestVerifyInit failed".into()));
            }

            let result = EVP_DigestVerify(&mut md_ctx, signature.as_ptr(), signature.len(), data.as_ptr(), data.len());
            EVP_MD_CTX_cleanup(&mut md_ctx);

            Ok(result == 1)
        }
    }
}
