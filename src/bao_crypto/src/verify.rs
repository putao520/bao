use crate::CryptoError;
use crate::sign::{RsaHash, SignAlgorithm, SignatureFormat, raw_ecdsa_sig_to_der};
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
                return Err(CryptoError::InvalidKey(
                    "PEM_read_bio_PrivateKey failed".into(),
                ));
            }
            pkey
        };
        Ok(Verifier {
            pkey,
            algo: algo.clone(),
        })
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
        Ok(Verifier {
            pkey,
            algo: algo.clone(),
        })
    }

    pub fn from_pkey(pkey: *mut EVP_PKEY, algo: &SignAlgorithm) -> Result<Verifier, CryptoError> {
        if pkey.is_null() {
            return Err(CryptoError::InvalidKey("null EVP_PKEY".into()));
        }
        Ok(Verifier {
            pkey,
            algo: algo.clone(),
        })
    }

    /// Load from a PEM-encoded SubjectPublicKeyInfo (public key).
    pub fn from_public_pem(algo: &SignAlgorithm, pem: &str) -> Result<Verifier, CryptoError> {
        let pkey = unsafe {
            let bio = BIO_new_mem_buf(pem.as_ptr() as *const c_void, pem.len() as isize);
            if bio.is_null() {
                return Err(CryptoError::InvalidKey("BIO_new_mem_buf failed".into()));
            }
            let pkey = PEM_read_bio_PUBKEY(
                bio,
                ptr::null_mut(),
                None::<pem_password_cb>,
                ptr::null_mut(),
            );
            BIO_free(bio);
            if pkey.is_null() {
                return Err(CryptoError::InvalidKey("PEM_read_bio_PUBKEY failed".into()));
            }
            pkey
        };
        Ok(Verifier {
            pkey,
            algo: algo.clone(),
        })
    }

    /// Load from a DER-encoded SubjectPublicKeyInfo (public key).
    pub fn from_public_der(algo: &SignAlgorithm, der: &[u8]) -> Result<Verifier, CryptoError> {
        let pkey = unsafe {
            let mut inp = der.as_ptr();
            let pkey = d2i_PUBKEY(ptr::null_mut(), &mut inp, der.len() as c_long);
            if pkey.is_null() {
                return Err(CryptoError::InvalidKey("d2i_PUBKEY failed".into()));
            }
            pkey
        };
        Ok(Verifier {
            pkey,
            algo: algo.clone(),
        })
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
                return Err(CryptoError::VerifyFailed(
                    "EVP_DigestVerifyInit failed".into(),
                ));
            }

            if let SignAlgorithm::RsaPss { .. } = self.algo {
                if !pctx.is_null() {
                    EVP_PKEY_CTX_set_rsa_padding(pctx, RSA_PKCS1_PSS_PADDING);
                    EVP_PKEY_CTX_set_rsa_pss_saltlen(pctx, RSA_PSS_SALTLEN_DIGEST);
                }
            }

            if EVP_DigestVerifyUpdate(&mut md_ctx, data.as_ptr() as *const c_void, data.len()) != 1
            {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::VerifyFailed(
                    "EVP_DigestVerifyUpdate failed".into(),
                ));
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

            if EVP_DigestVerifyInit(
                &mut md_ctx,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                self.pkey,
            ) != 1
            {
                EVP_MD_CTX_cleanup(&mut md_ctx);
                return Err(CryptoError::VerifyFailed(
                    "EVP_DigestVerifyInit failed".into(),
                ));
            }

            let result = EVP_DigestVerify(
                &mut md_ctx,
                signature.as_ptr(),
                signature.len(),
                data.as_ptr(),
                data.len(),
            );
            EVP_MD_CTX_cleanup(&mut md_ctx);

            Ok(result == 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{RsaHash, SignAlgorithm, SignatureFormat, Signer};

    /// Generate one RSA-2048 keypair and return (private_pem, public_pem).
    fn rsa_keypair_pem() -> (String, String) {
        unsafe {
            let rsa = RSA_new();
            let bn = BN_new();
            BN_set_word(bn, 65537);
            assert_eq!(RSA_generate_key_ex(rsa, 2048, bn, ptr::null_mut()), 1);
            BN_free(bn);
            let pkey = EVP_PKEY_new();
            assert_eq!(EVP_PKEY_set1_RSA(pkey, rsa), 1);
            RSA_free(rsa);

            // Private PKCS8 PEM.
            let priv_bio = BIO_new(BIO_s_mem());
            assert_eq!(
                PEM_write_bio_PKCS8PrivateKey(
                    priv_bio,
                    pkey,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                    None::<pem_password_cb>,
                    ptr::null_mut(),
                ),
                1
            );
            let priv_pending = BIO_ctrl_pending(priv_bio);
            let mut priv_buf = vec![0u8; priv_pending];
            let priv_n = BIO_read(
                priv_bio,
                priv_buf.as_mut_ptr() as *mut core::ffi::c_void,
                priv_pending as core::ffi::c_int,
            );
            BIO_free(priv_bio);

            // Public SubjectPublicKeyInfo PEM.
            let pub_bio = BIO_new(BIO_s_mem());
            assert_eq!(PEM_write_bio_PUBKEY(pub_bio, pkey), 1);
            let pub_pending = BIO_ctrl_pending(pub_bio);
            let mut pub_buf = vec![0u8; pub_pending];
            let pub_n = BIO_read(
                pub_bio,
                pub_buf.as_mut_ptr() as *mut core::ffi::c_void,
                pub_pending as core::ffi::c_int,
            );
            BIO_free(pub_bio);

            EVP_PKEY_free(pkey);
            assert!(priv_n > 0 && pub_n > 0);
            (
                String::from_utf8(priv_buf[..priv_n as usize].to_vec()).unwrap(),
                String::from_utf8(pub_buf[..pub_n as usize].to_vec()).unwrap(),
            )
        }
    }

    #[test]
    fn verify_public_key_pem_roundtrip() {
        // Sign with the private key, verify with the matching public key PEM.
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let algo = SignAlgorithm::RsaPkcs1v15 {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &priv_pem).unwrap();
        let data = b"verify with public key pem";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();

        let verifier = Verifier::from_public_pem(&algo, &pub_pem).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn verify_public_key_pem_rejects_tampered() {
        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let algo = SignAlgorithm::RsaPkcs1v15 {
            hash: RsaHash::Sha256,
        };
        let signer = Signer::from_pkcs8_pem(&algo, &priv_pem).unwrap();
        let sig = signer.sign(b"original", SignatureFormat::Der).unwrap();

        let verifier = Verifier::from_public_pem(&algo, &pub_pem).unwrap();
        assert!(
            !verifier
                .verify(b"tampered", &sig, SignatureFormat::Der)
                .unwrap()
        );
    }
}
