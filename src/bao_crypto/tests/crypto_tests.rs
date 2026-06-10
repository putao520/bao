//! Tests for bao_crypto crate.

mod sign_tests {
    use bao_crypto::sign::{SignAlgorithm, Signer, SignatureFormat, RsaHash};
    use bao_crypto::verify::Verifier;
    use pkcs8::{EncodePrivateKey, EncodePublicKey};
    use rand_core::RngCore;

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        let mut bytes = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut bytes);
        let keypair = ed25519_dalek::SigningKey::from_bytes(&bytes);
        let private_der = keypair.to_pkcs8_der().unwrap();
        let public_der = keypair.verifying_key().to_public_key_der().unwrap();

        let signer = Signer::from_pkcs8_der(&SignAlgorithm::Ed25519, private_der.as_bytes()).unwrap();
        let verifier = Verifier::from_pkcs8_der(&SignAlgorithm::Ed25519, public_der.as_bytes()).unwrap();

        let data = b"hello bao crypto";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
        assert!(!verifier.verify(b"wrong data", &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn ecdsa_p256_sign_verify_roundtrip() {
        let private_key = p256::SecretKey::random(&mut rand_core::OsRng);
        let private_der = private_key.to_pkcs8_der().unwrap();
        let public_der = private_key.public_key().to_public_key_der().unwrap();

        let signer = Signer::from_pkcs8_der(&SignAlgorithm::EcdsaP256, private_der.as_bytes()).unwrap();
        let verifier = Verifier::from_pkcs8_der(&SignAlgorithm::EcdsaP256, public_der.as_bytes()).unwrap();

        let data = b"ecdsa p256 test";
        let sig_der = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(verifier.verify(data, &sig_der, SignatureFormat::Der).unwrap());

        let sig_raw = signer.sign(data, SignatureFormat::Raw).unwrap();
        assert!(verifier.verify(data, &sig_raw, SignatureFormat::Raw).unwrap());
    }

    #[test]
    fn ecdsa_p384_sign_verify_roundtrip() {
        let private_key = p384::SecretKey::random(&mut rand_core::OsRng);
        let private_der = private_key.to_pkcs8_der().unwrap();
        let public_der = private_key.public_key().to_public_key_der().unwrap();

        let signer = Signer::from_pkcs8_der(&SignAlgorithm::EcdsaP384, private_der.as_bytes()).unwrap();
        let verifier = Verifier::from_pkcs8_der(&SignAlgorithm::EcdsaP384, public_der.as_bytes()).unwrap();

        let data = b"ecdsa p384 test";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn rsa_pkcs1v15_sign_verify_roundtrip() {
        let mut rng = rand_core::OsRng;
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let private_der = private_key.to_pkcs8_der().unwrap();
        let public_der = private_key.to_public_key().to_public_key_der().unwrap();

        let algo = SignAlgorithm::RsaPkcs1v15 { hash: RsaHash::Sha256 };
        let signer = Signer::from_pkcs8_der(&algo, private_der.as_bytes()).unwrap();
        let verifier = Verifier::from_pkcs8_der(&algo, public_der.as_bytes()).unwrap();

        let data = b"rsa pkcs1v15 sha256 test";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }

    #[test]
    fn rsa_pss_sign_verify_roundtrip() {
        let mut rng = rand_core::OsRng;
        let private_key = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let private_der = private_key.to_pkcs8_der().unwrap();
        let public_der = private_key.to_public_key().to_public_key_der().unwrap();

        let algo = SignAlgorithm::RsaPss { hash: RsaHash::Sha256 };
        let signer = Signer::from_pkcs8_der(&algo, private_der.as_bytes()).unwrap();
        let verifier = Verifier::from_pkcs8_der(&algo, public_der.as_bytes()).unwrap();

        let data = b"rsa pss sha256 test";
        let sig = signer.sign(data, SignatureFormat::Der).unwrap();
        assert!(verifier.verify(data, &sig, SignatureFormat::Der).unwrap());
    }
}

mod cipher_tests {
    use bao_crypto::cipher::{self, CipherAlgorithm};

    #[test]
    fn aes_256_gcm_encrypt_decrypt() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello aes-256-gcm";
        let aad = b"additional data";

        let result = cipher::encrypt(CipherAlgorithm::Aes256Gcm, key, iv, Some(aad), plaintext).unwrap();
        assert!(!result.ciphertext.is_empty());
        assert_eq!(result.auth_tag.len(), 16);

        let decrypted = cipher::decrypt(CipherAlgorithm::Aes256Gcm, key, iv, Some(aad), &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_128_gcm_encrypt_decrypt() {
        let key = b"0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello aes-128-gcm";

        let result = cipher::encrypt(CipherAlgorithm::Aes128Gcm, key, iv, None, plaintext).unwrap();
        let decrypted = cipher::decrypt(CipherAlgorithm::Aes128Gcm, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn chacha20poly1305_encrypt_decrypt() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello chacha20";

        let result = cipher::encrypt(CipherAlgorithm::ChaCha20Poly1305, key, iv, None, plaintext).unwrap();
        let decrypted = cipher::decrypt(CipherAlgorithm::ChaCha20Poly1305, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_256_gcm_wrong_key_fails() {
        let key = b"0123456789abcdef0123456789abcdef";
        let wrong_key = b"fedcba9876543210fedcba9876543210";
        let iv = b"0123456789ab";
        let plaintext = b"secret message";

        let result = cipher::encrypt(CipherAlgorithm::Aes256Gcm, key, iv, None, plaintext).unwrap();
        assert!(cipher::decrypt(CipherAlgorithm::Aes256Gcm, wrong_key, iv, None, &result.ciphertext, &result.auth_tag).is_err());
    }

    #[test]
    fn parse_algorithm_names() {
        assert_eq!(cipher::parse_algorithm("aes-256-gcm").unwrap(), CipherAlgorithm::Aes256Gcm);
        assert_eq!(cipher::parse_algorithm("AES-128-GCM").unwrap(), CipherAlgorithm::Aes128Gcm);
        assert_eq!(cipher::parse_algorithm("chacha20-poly1305").unwrap(), CipherAlgorithm::ChaCha20Poly1305);
        assert!(cipher::parse_algorithm("unknown").is_err());
    }
}

mod key_exchange_tests {
    use bao_crypto::key_exchange::{self, EcdhCurve, EcdhKeyPair};

    #[test]
    fn ecdh_p256_shared_secret() {
        let alice = EcdhKeyPair::generate(EcdhCurve::P256).unwrap();
        let bob = EcdhKeyPair::generate(EcdhCurve::P256).unwrap();

        let shared_a = alice.compute_shared_secret(&bob.public_key_bytes()).unwrap();
        let shared_b = bob.compute_shared_secret(&alice.public_key_bytes()).unwrap();
        assert_eq!(shared_a, shared_b);
    }

    #[test]
    fn ecdh_p384_shared_secret() {
        let alice = EcdhKeyPair::generate(EcdhCurve::P384).unwrap();
        let bob = EcdhKeyPair::generate(EcdhCurve::P384).unwrap();

        let shared_a = alice.compute_shared_secret(&bob.public_key_bytes()).unwrap();
        let shared_b = bob.compute_shared_secret(&alice.public_key_bytes()).unwrap();
        assert_eq!(shared_a, shared_b);
    }

    #[test]
    fn ecdh_x25519_shared_secret() {
        let alice = EcdhKeyPair::generate(EcdhCurve::X25519).unwrap();
        let bob = EcdhKeyPair::generate(EcdhCurve::X25519).unwrap();

        let shared_a = alice.compute_shared_secret(&bob.public_key_bytes()).unwrap();
        let shared_b = bob.compute_shared_secret(&alice.public_key_bytes()).unwrap();
        assert_eq!(shared_a, shared_b);
    }

    #[test]
    fn parse_curve_names() {
        assert_eq!(key_exchange::parse_curve("P-256").unwrap(), EcdhCurve::P256);
        assert_eq!(key_exchange::parse_curve("prime256v1").unwrap(), EcdhCurve::P256);
        assert_eq!(key_exchange::parse_curve("P-384").unwrap(), EcdhCurve::P384);
        assert_eq!(key_exchange::parse_curve("X25519").unwrap(), EcdhCurve::X25519);
        assert!(key_exchange::parse_curve("unknown").is_err());
    }
}

mod keypair_tests {
    use bao_crypto::keypair::{self, KeyPairType, EcCurve};

    #[test]
    fn generate_rsa_2048() {
        let result = keypair::generate_key_pair(&KeyPairType::Rsa { bits: 2048 }).unwrap();
        assert!(!result.private_key_der.is_empty());
        assert!(!result.public_key_der.is_empty());
        assert!(result.private_key_pem.is_some());
        assert!(result.public_key_pem.is_some());
    }

    #[test]
    fn generate_ec_p256() {
        let result = keypair::generate_key_pair(&KeyPairType::Ec { curve: EcCurve::P256 }).unwrap();
        assert!(!result.private_key_der.is_empty());
        assert!(!result.public_key_der.is_empty());
    }

    #[test]
    fn generate_ed25519() {
        let result = keypair::generate_key_pair(&KeyPairType::Ed25519).unwrap();
        assert!(!result.private_key_der.is_empty());
        assert!(!result.public_key_der.is_empty());
    }

    #[test]
    fn generate_x25519() {
        let result = keypair::generate_key_pair(&KeyPairType::X25519).unwrap();
        assert_eq!(result.private_key_der.len(), 32);
        assert_eq!(result.public_key_der.len(), 32);
    }
}

mod kdf_tests {
    use bao_crypto::kdf::{self, HkdfHash, Pbkdf2Hash};

    #[test]
    fn hkdf_sha256() {
        let salt = b"salt";
        let ikm = b"input key material";
        let info = b"info";
        let okm = kdf::hkdf(HkdfHash::Sha256, salt, ikm, info, 32).unwrap();
        assert_eq!(okm.len(), 32);
    }

    #[test]
    fn pbkdf2_sha256_deterministic() {
        let password = b"password";
        let salt = b"salt";
        let key = kdf::pbkdf2(Pbkdf2Hash::Sha256, password, salt, 1000, 32);
        assert_eq!(key.len(), 32);
        let key2 = kdf::pbkdf2(Pbkdf2Hash::Sha256, password, salt, 1000, 32);
        assert_eq!(key, key2);
    }

    #[test]
    fn pbkdf2_sha1() {
        let password = b"password";
        let salt = b"salt";
        let key = kdf::pbkdf2(Pbkdf2Hash::Sha1, password, salt, 1000, 20);
        assert_eq!(key.len(), 20);
    }
}

mod random_tests {
    use bao_crypto::random;

    #[test]
    fn random_bytes_nonzero() {
        let mut buf = [0u8; 32];
        random::random_bytes(&mut buf).unwrap();
        assert!(!buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn random_vec_length() {
        let vec = random::random_vec(16).unwrap();
        assert_eq!(vec.len(), 16);
    }
}

mod certificate_tests {
    use bao_crypto::certificate::X509Certificate;

    #[test]
    fn parse_self_signed_cert() {
        let cert_der = include_bytes!("test_data/self_signed.der");
        let cert = X509Certificate::from_der(cert_der).unwrap();
        assert!(cert.subject().contains("bao-test"));
        assert!(!cert.fingerprint_sha256().is_empty());
        assert!(!cert.fingerprint_sha1().is_empty());
    }
}
