//! Signature verification algorithms for rustls CryptoProvider.
//!
//! Provides verification using RustCrypto backends: ECDSA P-256/P-384,
//! RSA PKCS1/PSS, and Ed25519.

use rustls::pki_types::{AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::SignatureScheme;

use ecdsa::signature::hazmat::PrehashVerifier;
use ecdsa::signature::Verifier as EcdsaVerifier;
use rsa::pkcs1::DecodeRsaPublicKey;
use sha2::{Digest, Sha256, Sha384, Sha512};

// ─── RustCryptoAlgorithm ────────────────────────────────────────────

/// A `SignatureVerificationAlgorithm` implemented using RustCrypto backends.
#[derive(Debug)]
struct RustCryptoAlgorithm {
    public_key_alg_id: AlgorithmIdentifier,
    signature_alg_id: AlgorithmIdentifier,
    verify_fn: fn(&[u8], &[u8], &[u8]) -> Result<(), InvalidSignature>,
}

impl SignatureVerificationAlgorithm for RustCryptoAlgorithm {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.public_key_alg_id
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature_alg_id
    }

    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        (self.verify_fn)(public_key, message, signature)
    }
}

// ─── ECDSA P-256 SHA-256 ────────────────────────────────────────────

fn verify_ecdsa_p256_sha256(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let verifying_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| InvalidSignature)?;
    let sig = ecdsa::Signature::<p256::NistP256>::from_der(signature)
        .map_err(|_| InvalidSignature)?;
    // Default digest for P-256 is SHA-256, so EcdsaVerifier::verify works directly.
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static ECDSA_P256_SHA256: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::ECDSA_P256,
    signature_alg_id: alg_id::ECDSA_SHA256,
    verify_fn: verify_ecdsa_p256_sha256,
};

// ─── ECDSA P-256 SHA-384 ────────────────────────────────────────────

fn verify_ecdsa_p256_sha384(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let verifying_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| InvalidSignature)?;
    let sig = ecdsa::Signature::<p256::NistP256>::from_der(signature)
        .map_err(|_| InvalidSignature)?;
    // P-256 default is SHA-256; for SHA-384 we hash then use verify_prehash.
    let digest = Sha384::digest(message);
    verifying_key
        .verify_prehash(&digest, &sig)
        .map_err(|_| InvalidSignature)
}

static ECDSA_P256_SHA384: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::ECDSA_P256,
    signature_alg_id: alg_id::ECDSA_SHA384,
    verify_fn: verify_ecdsa_p256_sha384,
};

// ─── ECDSA P-384 SHA-384 ────────────────────────────────────────────

fn verify_ecdsa_p384_sha384(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let verifying_key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| InvalidSignature)?;
    let sig = ecdsa::Signature::<p384::NistP384>::from_der(signature)
        .map_err(|_| InvalidSignature)?;
    // Default digest for P-384 is SHA-384, so EcdsaVerifier::verify works directly.
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static ECDSA_P384_SHA384: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::ECDSA_P384,
    signature_alg_id: alg_id::ECDSA_SHA384,
    verify_fn: verify_ecdsa_p384_sha384,
};

// ─── ECDSA P-384 SHA-256 ────────────────────────────────────────────

fn verify_ecdsa_p384_sha256(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let verifying_key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| InvalidSignature)?;
    let sig = ecdsa::Signature::<p384::NistP384>::from_der(signature)
        .map_err(|_| InvalidSignature)?;
    // P-384 default is SHA-384; for SHA-256 we hash then use verify_prehash.
    let digest = Sha256::digest(message);
    verifying_key
        .verify_prehash(&digest, &sig)
        .map_err(|_| InvalidSignature)
}

static ECDSA_P384_SHA256: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::ECDSA_P384,
    signature_alg_id: alg_id::ECDSA_SHA256,
    verify_fn: verify_ecdsa_p384_sha256,
};

// ─── Ed25519 ────────────────────────────────────────────────────────

fn verify_ed25519(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let pk_bytes: &[u8; 32] = public_key
        .try_into()
        .map_err(|_| InvalidSignature)?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(pk_bytes)
        .map_err(|_| InvalidSignature)?;
    let sig = ed25519_dalek::Signature::from_slice(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static ED25519: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::ED25519,
    signature_alg_id: alg_id::ED25519,
    verify_fn: verify_ed25519,
};

// ─── RSA PKCS1 SHA-256 ─────────────────────────────────────────────

fn verify_rsa_pkcs1_sha256(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pkcs1v15::VerifyingKey::<Sha256>::new(rsa_key);
    let sig = rsa::pkcs1v15::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PKCS1_2048_8192_SHA256: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PKCS1_SHA256,
    verify_fn: verify_rsa_pkcs1_sha256,
};

// ─── RSA PKCS1 SHA-384 ─────────────────────────────────────────────

fn verify_rsa_pkcs1_sha384(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pkcs1v15::VerifyingKey::<Sha384>::new(rsa_key);
    let sig = rsa::pkcs1v15::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PKCS1_2048_8192_SHA384: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PKCS1_SHA384,
    verify_fn: verify_rsa_pkcs1_sha384,
};

// ─── RSA PKCS1 SHA-512 ─────────────────────────────────────────────

fn verify_rsa_pkcs1_sha512(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pkcs1v15::VerifyingKey::<Sha512>::new(rsa_key);
    let sig = rsa::pkcs1v15::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PKCS1_2048_8192_SHA512: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PKCS1_SHA512,
    verify_fn: verify_rsa_pkcs1_sha512,
};

// ─── RSA PKCS1 SHA-256 ABSENT_PARAMS ───────────────────────────────

/// sha256WithRSAEncryption AlgorithmIdentifier with absent NULL parameters.
///
/// RFC 4055 Section 1: "When any of these four object identifiers appears within
/// an AlgorithmIdentifier, the parameters MUST be NULL. Implementations MUST
/// accept the parameters being absent as well as present."
const RSA_PKCS1_SHA256_ABSENT_PARAMS_DER: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b,
];

static RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: AlgorithmIdentifier::from_slice(RSA_PKCS1_SHA256_ABSENT_PARAMS_DER),
    verify_fn: verify_rsa_pkcs1_sha256,
};

// ─── RSA PKCS1 SHA-384 ABSENT_PARAMS ───────────────────────────────

const RSA_PKCS1_SHA384_ABSENT_PARAMS_DER: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c,
];

static RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: AlgorithmIdentifier::from_slice(RSA_PKCS1_SHA384_ABSENT_PARAMS_DER),
    verify_fn: verify_rsa_pkcs1_sha384,
};

// ─── RSA PKCS1 SHA-512 ABSENT_PARAMS ───────────────────────────────

const RSA_PKCS1_SHA512_ABSENT_PARAMS_DER: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d,
];

static RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: AlgorithmIdentifier::from_slice(RSA_PKCS1_SHA512_ABSENT_PARAMS_DER),
    verify_fn: verify_rsa_pkcs1_sha512,
};

// ─── RSA PSS SHA-256 ───────────────────────────────────────────────

fn verify_rsa_pss_sha256(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pss::VerifyingKey::<Sha256>::new(rsa_key);
    let sig = rsa::pss::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PSS_2048_8192_SHA256: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PSS_SHA256,
    verify_fn: verify_rsa_pss_sha256,
};

// ─── RSA PSS SHA-384 ───────────────────────────────────────────────

fn verify_rsa_pss_sha384(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pss::VerifyingKey::<Sha384>::new(rsa_key);
    let sig = rsa::pss::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PSS_2048_8192_SHA384: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PSS_SHA384,
    verify_fn: verify_rsa_pss_sha384,
};

// ─── RSA PSS SHA-512 ───────────────────────────────────────────────

fn verify_rsa_pss_sha512(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), InvalidSignature> {
    let rsa_key = rsa::RsaPublicKey::from_pkcs1_der(public_key)
        .map_err(|_| InvalidSignature)?;
    let verifying_key = rsa::pss::VerifyingKey::<Sha512>::new(rsa_key);
    let sig = rsa::pss::Signature::try_from(signature)
        .map_err(|_| InvalidSignature)?;
    verifying_key
        .verify(message, &sig)
        .map_err(|_| InvalidSignature)
}

static RSA_PSS_2048_8192_SHA512: RustCryptoAlgorithm = RustCryptoAlgorithm {
    public_key_alg_id: alg_id::RSA_ENCRYPTION,
    signature_alg_id: alg_id::RSA_PSS_SHA512,
    verify_fn: verify_rsa_pss_sha512,
};

// ─── ALGORITHMS ─────────────────────────────────────────────────────

/// All supported signature verification algorithms for the Bao CryptoProvider.
pub(super) static ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        &ECDSA_P256_SHA256,
        &ECDSA_P256_SHA384,
        &ECDSA_P384_SHA256,
        &ECDSA_P384_SHA384,
        &ED25519,
        &RSA_PKCS1_2048_8192_SHA256,
        &RSA_PKCS1_2048_8192_SHA384,
        &RSA_PKCS1_2048_8192_SHA512,
        &RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS,
        &RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS,
        &RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS,
        &RSA_PSS_2048_8192_SHA256,
        &RSA_PSS_2048_8192_SHA384,
        &RSA_PSS_2048_8192_SHA512,
    ],
    mapping: &[
        (
            SignatureScheme::ECDSA_NISTP256_SHA256,
            &[&ECDSA_P256_SHA256 as &dyn SignatureVerificationAlgorithm, &ECDSA_P384_SHA256],
        ),
        (
            SignatureScheme::ECDSA_NISTP384_SHA384,
            &[&ECDSA_P384_SHA384 as &dyn SignatureVerificationAlgorithm, &ECDSA_P256_SHA384],
        ),
        (SignatureScheme::ED25519, &[&ED25519]),
        (
            SignatureScheme::RSA_PKCS1_SHA256,
            &[&RSA_PKCS1_2048_8192_SHA256],
        ),
        (
            SignatureScheme::RSA_PKCS1_SHA384,
            &[&RSA_PKCS1_2048_8192_SHA384],
        ),
        (
            SignatureScheme::RSA_PKCS1_SHA512,
            &[&RSA_PKCS1_2048_8192_SHA512],
        ),
        (
            SignatureScheme::RSA_PSS_SHA256,
            &[&RSA_PSS_2048_8192_SHA256],
        ),
        (
            SignatureScheme::RSA_PSS_SHA384,
            &[&RSA_PSS_2048_8192_SHA384],
        ),
        (
            SignatureScheme::RSA_PSS_SHA512,
            &[&RSA_PSS_2048_8192_SHA512],
        ),
    ],
};
