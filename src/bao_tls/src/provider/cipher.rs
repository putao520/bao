//! AEAD cipher suite implementations for rustls CryptoProvider.
//!
//! Implements `Tls13AeadAlgorithm`, `Tls12AeadAlgorithm`, `MessageEncrypter`,
//! and `MessageDecrypter` traits using RustCrypto's `aes-gcm` and
//! `chacha20poly1305` crates.

use aes_gcm::{
    aead::{AeadCore, AeadInPlace, KeyInit},
    Aes128Gcm, Aes256Gcm,
};
use aes_gcm::aead::generic_array::GenericArray;
use chacha20poly1305::ChaCha20Poly1305;
use rustls::crypto::cipher::{
    AeadKey, InboundOpaqueMessage, InboundPlainMessage, Iv, KeyBlockShape, MessageDecrypter,
    MessageEncrypter, Nonce, OutboundOpaqueMessage, OutboundPlainMessage, PrefixedPayload,
    Tls12AeadAlgorithm, Tls13AeadAlgorithm, UnsupportedOperationError, make_tls12_aad,
    make_tls13_aad, NONCE_LEN,
};
use rustls::crypto::tls12::PrfUsingHmac;
use rustls::crypto::tls13::HkdfUsingHmac;
use rustls::crypto::KeyExchangeAlgorithm;
use rustls::{
    CipherSuite, CipherSuiteCommon, ConnectionTrafficSecrets, ContentType, Error,
    ProtocolVersion, SignatureScheme, SupportedCipherSuite, Tls12CipherSuite, Tls13CipherSuite,
};

use super::hash;
use super::hmac;

// ─── AEAD tag and overhead constants ───────────────────────────────────

/// Maximum TLS fragment length (2^14 = 16384 bytes, per RFC 5246 / RFC 8446).
///
/// This mirrors `rustls::msgs::fragmenter::MAX_FRAGMENT_LEN` which is `pub(crate)`.
const MAX_FRAGMENT_LEN: usize = 16384;

/// All supported AEAD algorithms (AES-GCM, ChaCha20-Poly1305) use a 16-byte tag.
const AEAD_TAG_LEN: usize = 16;

/// TLS 1.2 GCM explicit nonce length (bytes 4..12 of the 12-byte nonce).
const GCM_EXPLICIT_NONCE_LEN: usize = 8;

/// Total overhead for TLS 1.2 GCM: explicit nonce + auth tag.
const GCM_OVERHEAD: usize = GCM_EXPLICIT_NONCE_LEN + AEAD_TAG_LEN;

// ─── TLS 1.3 cipher suites ────────────────────────────────────────────
// HkdfUsingHmac and PrfUsingHmac take &'a dyn Hmac.  We use them inline
// in the cipher suite statics (like the ring provider), where the
// unsized coercion &HmacSha256 -> &dyn Hmac works in struct literal context.

/// TLS_AES_256_GCM_SHA384 (RFC 8446)
pub(crate) static TLS13_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_256_GCM_SHA384,
            hash_provider: hash::SHA384,
            // ref: <https://www.ietf.org/archive/id/draft-irtf-cfrg-aead-limits-08.html#section-5.1.1>
            confidentiality_limit: 1 << 24,
        },
        hkdf_provider: &HkdfUsingHmac(&hmac::HMAC_SHA384),
        aead_alg: &Aes256GcmAead,
        quic: None,
    });

/// TLS_AES_128_GCM_SHA256 (RFC 8446)
pub(crate) static TLS13_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_128_GCM_SHA256,
            hash_provider: hash::SHA256,
            confidentiality_limit: 1 << 24,
        },
        hkdf_provider: &HkdfUsingHmac(&hmac::HMAC_SHA256),
        aead_alg: &Aes128GcmAead,
        quic: None,
    });

/// TLS_CHACHA20_POLY1305_SHA256 (RFC 8446)
pub(crate) static TLS13_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
            hash_provider: hash::SHA256,
            // ref: <https://www.ietf.org/archive/id/draft-irtf-cfrg-aead-limits-08.html#section-5.2.1>
            confidentiality_limit: u64::MAX,
        },
        hkdf_provider: &HkdfUsingHmac(&hmac::HMAC_SHA256),
        aead_alg: &Chacha20Poly1305Aead,
        quic: None,
    });

// ─── TLS 1.2 cipher suites ────────────────────────────────────────────

/// TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384
pub(crate) static TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            hash_provider: hash::SHA384,
            confidentiality_limit: 1 << 24,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &Tls12Aes256Gcm,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA384),
    });

/// TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
pub(crate) static TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            hash_provider: hash::SHA256,
            confidentiality_limit: 1 << 24,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &Tls12Aes128Gcm,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA256),
    });

/// TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
pub(crate) static TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
            hash_provider: hash::SHA256,
            confidentiality_limit: u64::MAX,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &Tls12ChaCha20Poly1305,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA256),
    });

/// TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
pub(crate) static TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            hash_provider: hash::SHA384,
            confidentiality_limit: 1 << 24,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &Tls12Aes256Gcm,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA384),
    });

/// TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256
pub(crate) static TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            hash_provider: hash::SHA256,
            confidentiality_limit: 1 << 24,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &Tls12Aes128Gcm,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA256),
    });

/// TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
pub(crate) static TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
            hash_provider: hash::SHA256,
            confidentiality_limit: u64::MAX,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &Tls12ChaCha20Poly1305,
        prf_provider: &PrfUsingHmac(&hmac::HMAC_SHA256),
    });

// ─── TLS 1.2 signature scheme sets ────────────────────────────────────

static TLS12_ECDSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::ED25519,
    SignatureScheme::ECDSA_NISTP521_SHA512,
    SignatureScheme::ECDSA_NISTP384_SHA384,
    SignatureScheme::ECDSA_NISTP256_SHA256,
];

static TLS12_RSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PKCS1_SHA512,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA256,
];

// ═══════════════════════════════════════════════════════════════════════
// TLS 1.3 AEAD algorithm implementations
// ═══════════════════════════════════════════════════════════════════════

struct Aes256GcmAead;

impl Tls13AeadAlgorithm for Aes256GcmAead {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13AesGcmEncrypter {
            cipher: Aes256Gcm::new_from_slice(key.as_ref())
                .expect("AES-256-GCM key must be 32 bytes"),
            iv,
        })
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13AesGcmDecrypter {
            cipher: Aes256Gcm::new_from_slice(key.as_ref())
                .expect("AES-256-GCM key must be 32 bytes"),
            iv,
        })
    }

    fn key_len(&self) -> usize {
        32
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Aes256Gcm { key, iv })
    }

    fn fips(&self) -> bool {
        false // RustCrypto AES-GCM is not FIPS validated
    }
}

struct Aes128GcmAead;

impl Tls13AeadAlgorithm for Aes128GcmAead {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13AesGcmEncrypter {
            cipher: Aes128Gcm::new_from_slice(key.as_ref())
                .expect("AES-128-GCM key must be 16 bytes"),
            iv,
        })
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13AesGcmDecrypter {
            cipher: Aes128Gcm::new_from_slice(key.as_ref())
                .expect("AES-128-GCM key must be 16 bytes"),
            iv,
        })
    }

    fn key_len(&self) -> usize {
        16
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Aes128Gcm { key, iv })
    }

    fn fips(&self) -> bool {
        false
    }
}

struct Chacha20Poly1305Aead;

impl Tls13AeadAlgorithm for Chacha20Poly1305Aead {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Tls13ChaCha20Poly1305Encrypter {
            cipher: ChaCha20Poly1305::new_from_slice(key.as_ref())
                .expect("ChaCha20-Poly1305 key must be 32 bytes"),
            iv,
        })
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Tls13ChaCha20Poly1305Decrypter {
            cipher: ChaCha20Poly1305::new_from_slice(key.as_ref())
                .expect("ChaCha20-Poly1305 key must be 32 bytes"),
            iv,
        })
    }

    fn key_len(&self) -> usize {
        32
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv })
    }

    fn fips(&self) -> bool {
        false // ChaCha20-Poly1305 is not FIPS approved
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TLS 1.3 MessageEncrypter / MessageDecrypter
// ═══════════════════════════════════════════════════════════════════════

/// TLS 1.3 encrypter for AES-GCM (128 or 256 bit).
struct Tls13AesGcmEncrypter<C> {
    cipher: C,
    iv: Iv,
}

impl<C: AeadInPlace + AeadCore + Send + Sync> MessageEncrypter for Tls13AesGcmEncrypter<C> {
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_len = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls13_aad(total_len);

        // Copy plaintext + 1-byte content type (TLS 1.3 TLSInnerPlaintext)
        payload.extend_from_chunks(&msg.payload);
        payload.extend_from_slice(&msg.typ.to_array());

        // Encrypt in-place, then append the authentication tag
        let tag = self
            .cipher
            .encrypt_in_place_detached(
                aead_nonce::<C>(&nonce),
                &aad,
                &mut payload.as_mut()[..msg.payload.len() + 1],
            )
            .map_err(|_| Error::EncryptError)?;

        payload.extend_from_slice(tag.as_ref());

        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            // Note: all TLS 1.3 application data records use TLSv1_2 (0x0303)
            // as the legacy record protocol version, see RFC 8446 section 5.1
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + 1 + AEAD_TAG_LEN
    }
}

/// TLS 1.3 decrypter for AES-GCM (128 or 256 bit).
struct Tls13AesGcmDecrypter<C> {
    cipher: C,
    iv: Iv,
}

impl<C: AeadInPlace + AeadCore + Send + Sync> MessageDecrypter for Tls13AesGcmDecrypter<C> {
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut msg.payload;
        if payload.len() < AEAD_TAG_LEN {
            return Err(Error::DecryptError);
        }

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls13_aad(payload.len());

        // Split into ciphertext and tag
        let tag_pos = payload.len() - AEAD_TAG_LEN;
        let tag = aes_gcm::aead::Tag::<C>::clone_from_slice(&payload[tag_pos..]);
        payload.truncate(tag_pos);

        self.cipher
            .decrypt_in_place_detached(aead_nonce::<C>(&nonce), &aad, payload, &tag)
            .map_err(|_| Error::DecryptError)?;

        msg.into_tls13_unpadded_message()
    }
}

/// TLS 1.3 encrypter for ChaCha20-Poly1305.
struct Tls13ChaCha20Poly1305Encrypter {
    cipher: ChaCha20Poly1305,
    iv: Iv,
}

impl MessageEncrypter for Tls13ChaCha20Poly1305Encrypter {
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_len = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls13_aad(total_len);

        payload.extend_from_chunks(&msg.payload);
        payload.extend_from_slice(&msg.typ.to_array());

        let tag = self
            .cipher
            .encrypt_in_place_detached(
                chacha_nonce(&nonce),
                &aad,
                &mut payload.as_mut()[..msg.payload.len() + 1],
            )
            .map_err(|_| Error::EncryptError)?;

        payload.extend_from_slice(tag.as_ref());

        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + 1 + AEAD_TAG_LEN
    }
}

/// TLS 1.3 decrypter for ChaCha20-Poly1305.
struct Tls13ChaCha20Poly1305Decrypter {
    cipher: ChaCha20Poly1305,
    iv: Iv,
}

impl MessageDecrypter for Tls13ChaCha20Poly1305Decrypter {
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut msg.payload;
        if payload.len() < AEAD_TAG_LEN {
            return Err(Error::DecryptError);
        }

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls13_aad(payload.len());

        let tag_pos = payload.len() - AEAD_TAG_LEN;
        let tag = chacha20poly1305::aead::Tag::<ChaCha20Poly1305>::clone_from_slice(&payload[tag_pos..]);
        payload.truncate(tag_pos);

        self.cipher
            .decrypt_in_place_detached(chacha_nonce(&nonce), &aad, payload, &tag)
            .map_err(|_| Error::DecryptError)?;

        msg.into_tls13_unpadded_message()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TLS 1.2 AEAD algorithm implementations
// ═══════════════════════════════════════════════════════════════════════

struct Tls12Aes256Gcm;

impl Tls12AeadAlgorithm for Tls12Aes256Gcm {
    fn encrypter(
        &self,
        key: AeadKey,
        iv: &[u8],
        extra: &[u8],
    ) -> Box<dyn MessageEncrypter> {
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .expect("AES-256-GCM key must be 32 bytes");
        let enc_iv = gcm_iv(iv, extra);
        Box::new(Tls12GcmEncrypter { cipher, iv: enc_iv })
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .expect("AES-256-GCM key must be 32 bytes");
        let mut dec_salt = [0u8; 4];
        debug_assert_eq!(iv.len(), 4);
        dec_salt.copy_from_slice(iv);
        Box::new(Tls12GcmDecrypter { cipher, dec_salt })
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: 32,
            fixed_iv_len: 4,
            explicit_nonce_len: 8,
        }
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: &[u8],
        explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        let enc_iv = gcm_iv(iv, explicit);
        Ok(ConnectionTrafficSecrets::Aes256Gcm { key, iv: enc_iv })
    }

    fn fips(&self) -> bool {
        false
    }
}

struct Tls12Aes128Gcm;

impl Tls12AeadAlgorithm for Tls12Aes128Gcm {
    fn encrypter(
        &self,
        key: AeadKey,
        iv: &[u8],
        extra: &[u8],
    ) -> Box<dyn MessageEncrypter> {
        let cipher = Aes128Gcm::new_from_slice(key.as_ref())
            .expect("AES-128-GCM key must be 16 bytes");
        let enc_iv = gcm_iv(iv, extra);
        Box::new(Tls12GcmEncrypter { cipher, iv: enc_iv })
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        let cipher = Aes128Gcm::new_from_slice(key.as_ref())
            .expect("AES-128-GCM key must be 16 bytes");
        let mut dec_salt = [0u8; 4];
        debug_assert_eq!(iv.len(), 4);
        dec_salt.copy_from_slice(iv);
        Box::new(Tls12GcmDecrypter { cipher, dec_salt })
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: 16,
            fixed_iv_len: 4,
            explicit_nonce_len: 8,
        }
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: &[u8],
        explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        let enc_iv = gcm_iv(iv, explicit);
        Ok(ConnectionTrafficSecrets::Aes128Gcm { key, iv: enc_iv })
    }

    fn fips(&self) -> bool {
        false
    }
}

struct Tls12ChaCha20Poly1305;

impl Tls12AeadAlgorithm for Tls12ChaCha20Poly1305 {
    fn encrypter(
        &self,
        key: AeadKey,
        iv: &[u8],
        _extra: &[u8],
    ) -> Box<dyn MessageEncrypter> {
        let cipher = ChaCha20Poly1305::new_from_slice(key.as_ref())
            .expect("ChaCha20-Poly1305 key must be 32 bytes");
        Box::new(Tls12ChaCha20Poly1305Encrypter {
            cipher,
            iv: Iv::copy(iv),
        })
    }

    fn decrypter(&self, key: AeadKey, iv: &[u8]) -> Box<dyn MessageDecrypter> {
        let cipher = ChaCha20Poly1305::new_from_slice(key.as_ref())
            .expect("ChaCha20-Poly1305 key must be 32 bytes");
        Box::new(Tls12ChaCha20Poly1305Decrypter {
            cipher,
            iv: Iv::copy(iv),
        })
    }

    fn key_block_shape(&self) -> KeyBlockShape {
        KeyBlockShape {
            enc_key_len: 32,
            fixed_iv_len: 12,
            explicit_nonce_len: 0,
        }
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: &[u8],
        _explicit: &[u8],
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(ConnectionTrafficSecrets::Chacha20Poly1305 {
            key,
            iv: Iv::copy(iv),
        })
    }

    fn fips(&self) -> bool {
        false
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TLS 1.2 MessageEncrypter / MessageDecrypter
// ═══════════════════════════════════════════════════════════════════════

/// TLS 1.2 AES-GCM encrypter (128 or 256 bit).
struct Tls12GcmEncrypter<C> {
    cipher: C,
    iv: Iv,
}

impl<C: AeadInPlace + AeadCore + Send + Sync> MessageEncrypter for Tls12GcmEncrypter<C> {
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_len = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls12_aad(seq, msg.typ, msg.version, msg.payload.len());

        // Prepend the 8-byte explicit nonce (bytes 4..12 of the full nonce)
        payload.extend_from_slice(&nonce.0[4..]);
        payload.extend_from_chunks(&msg.payload);

        // Encrypt in-place after the explicit nonce prefix, then append tag
        let tag = self
            .cipher
            .encrypt_in_place_detached(
                aead_nonce::<C>(&nonce),
                &aad,
                &mut payload.as_mut()[GCM_EXPLICIT_NONCE_LEN..],
            )
            .map_err(|_| Error::EncryptError)?;

        payload.extend_from_slice(tag.as_ref());

        Ok(OutboundOpaqueMessage::new(msg.typ, msg.version, payload))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + GCM_OVERHEAD
    }
}

/// TLS 1.2 AES-GCM decrypter (128 or 256 bit).
struct Tls12GcmDecrypter<C> {
    cipher: C,
    dec_salt: [u8; 4],
}

impl<C: AeadInPlace + AeadCore + Send + Sync> MessageDecrypter for Tls12GcmDecrypter<C> {
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &msg.payload;
        if payload.len() < GCM_OVERHEAD {
            return Err(Error::DecryptError);
        }

        // Reconstruct the 12-byte nonce from 4-byte salt + 8-byte explicit nonce
        let nonce = {
            let mut nonce_bytes = [0u8; NONCE_LEN];
            nonce_bytes[..4].copy_from_slice(&self.dec_salt);
            nonce_bytes[4..].copy_from_slice(&payload[..GCM_EXPLICIT_NONCE_LEN]);
            Nonce(nonce_bytes)
        };

        let aad = make_tls12_aad(
            seq,
            msg.typ,
            msg.version,
            payload.len() - GCM_OVERHEAD,
        );

        let payload = &mut msg.payload;
        let tag_pos = payload.len() - AEAD_TAG_LEN;
        let tag = aes_gcm::aead::Tag::<C>::clone_from_slice(&payload[tag_pos..]);

        // Decrypt in-place the region after the explicit nonce, before the tag
        self.cipher
            .decrypt_in_place_detached(
                aead_nonce::<C>(&nonce),
                &aad,
                &mut payload[GCM_EXPLICIT_NONCE_LEN..tag_pos],
                &tag,
            )
            .map_err(|_| Error::DecryptError)?;

        let plain_len = tag_pos - GCM_EXPLICIT_NONCE_LEN;
        if plain_len > MAX_FRAGMENT_LEN {
            return Err(Error::PeerSentOversizedRecord);
        }

        payload.truncate(plain_len);
        Ok(msg.into_plain_message())
    }
}

/// TLS 1.2 ChaCha20-Poly1305 encrypter.
struct Tls12ChaCha20Poly1305Encrypter {
    cipher: ChaCha20Poly1305,
    iv: Iv,
}

impl MessageEncrypter for Tls12ChaCha20Poly1305Encrypter {
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_len = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls12_aad(seq, msg.typ, msg.version, msg.payload.len());

        payload.extend_from_chunks(&msg.payload);

        let tag = self
            .cipher
            .encrypt_in_place_detached(
                chacha_nonce(&nonce),
                &aad,
                &mut payload.as_mut()[..msg.payload.len()],
            )
            .map_err(|_| Error::EncryptError)?;

        payload.extend_from_slice(tag.as_ref());

        Ok(OutboundOpaqueMessage::new(msg.typ, msg.version, payload))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + AEAD_TAG_LEN
    }
}

/// TLS 1.2 ChaCha20-Poly1305 decrypter.
struct Tls12ChaCha20Poly1305Decrypter {
    cipher: ChaCha20Poly1305,
    iv: Iv,
}

impl MessageDecrypter for Tls12ChaCha20Poly1305Decrypter {
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut msg.payload;
        if payload.len() < AEAD_TAG_LEN {
            return Err(Error::DecryptError);
        }

        let nonce = Nonce::new(&self.iv, seq);
        let aad = make_tls12_aad(
            seq,
            msg.typ,
            msg.version,
            payload.len() - AEAD_TAG_LEN,
        );

        let tag_pos = payload.len() - AEAD_TAG_LEN;
        let tag = chacha20poly1305::aead::Tag::<ChaCha20Poly1305>::clone_from_slice(&payload[tag_pos..]);
        payload.truncate(tag_pos);

        self.cipher
            .decrypt_in_place_detached(chacha_nonce(&nonce), &aad, payload, &tag)
            .map_err(|_| Error::DecryptError)?;

        if payload.len() > MAX_FRAGMENT_LEN {
            return Err(Error::PeerSentOversizedRecord);
        }

        Ok(msg.into_plain_message())
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Utility functions
// ═══════════════════════════════════════════════════════════════════════

/// Convert a rustls `Nonce` to an AES-GCM `GenericArray` nonce reference.
fn aead_nonce<C: AeadInPlace + AeadCore>(nonce: &Nonce) -> &GenericArray<u8, C::NonceSize> {
    GenericArray::from_slice(&nonce.0)
}

/// Convert a rustls `Nonce` to a ChaCha20-Poly1305 `GenericArray` nonce reference.
fn chacha_nonce(nonce: &Nonce) -> &GenericArray<u8, <ChaCha20Poly1305 as AeadCore>::NonceSize> {
    GenericArray::from_slice(&nonce.0)
}

/// Construct a GCM IV from a 4-byte write_iv and 8-byte explicit nonce.
///
/// The GCM nonce is constructed from a 32-bit salt derived from the key
/// block, and a 64-bit explicit part.  We use the same construction as
/// TLS 1.3 / ChaCha20-Poly1305: a starting point extracted from the key
/// block, XORed with the sequence number.
fn gcm_iv(write_iv: &[u8], explicit: &[u8]) -> Iv {
    debug_assert_eq!(write_iv.len(), 4);
    debug_assert_eq!(explicit.len(), 8);

    let mut iv = [0u8; NONCE_LEN];
    iv[..4].copy_from_slice(write_iv);
    iv[4..].copy_from_slice(explicit);
    Iv::new(iv)
}
