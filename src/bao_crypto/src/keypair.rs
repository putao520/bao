use crate::CryptoError;
use p256::elliptic_curve::rand_core::OsRng;
use pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rand_core::RngCore;

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

fn generate_rsa(bits: usize) -> Result<KeyPairResult, CryptoError> {
    let mut rng = OsRng;
    let private = rsa::RsaPrivateKey::new(&mut rng, bits)
        .map_err(|e| CryptoError::KeyPairError(format!("RSA keygen: {}", e)))?;
    let public = private.to_public_key();

    let private_der = private
        .to_pkcs8_der()
        .map_err(|e| CryptoError::EncodingFailed(format!("RSA private PKCS8 DER: {}", e)))?
        .as_bytes()
        .to_vec();

    let public_der = public
        .to_public_key_der()
        .map_err(|e| CryptoError::EncodingFailed(format!("RSA public DER: {}", e)))?
        .as_bytes()
        .to_vec();

    let private_pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| CryptoError::EncodingFailed(format!("RSA private PEM: {}", e)))?
        .to_string();

    let public_pem = public
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| CryptoError::EncodingFailed(format!("RSA public PEM: {}", e)))?;

    Ok(KeyPairResult {
        public_key_der: public_der,
        private_key_der: private_der,
        public_key_pem: Some(public_pem),
        private_key_pem: Some(private_pem),
    })
}

fn generate_ec(curve: EcCurve) -> Result<KeyPairResult, CryptoError> {
    match curve {
        EcCurve::P256 => {
            let secret = p256::SecretKey::random(&mut OsRng);
            let public = secret.public_key();

            let private_der = secret
                .to_pkcs8_der()
                .map_err(|e| CryptoError::EncodingFailed(format!("P256 private PKCS8 DER: {}", e)))?
                .as_bytes()
                .to_vec();

            let public_der = public
                .to_public_key_der()
                .map_err(|e| CryptoError::EncodingFailed(format!("P256 public DER: {}", e)))?
                .as_bytes()
                .to_vec();

            let private_pem = secret
                .to_pkcs8_pem(LineEnding::LF)
                .map_err(|e| CryptoError::EncodingFailed(format!("P256 private PEM: {}", e)))?
                .to_string();

            let public_pem = public
                .to_public_key_pem(LineEnding::LF)
                .map_err(|e| CryptoError::EncodingFailed(format!("P256 public PEM: {}", e)))?;

            Ok(KeyPairResult {
                public_key_der: public_der,
                private_key_der: private_der,
                public_key_pem: Some(public_pem),
                private_key_pem: Some(private_pem),
            })
        }
        EcCurve::P384 => {
            let secret = p384::SecretKey::random(&mut OsRng);
            let public = secret.public_key();

            let private_der = secret
                .to_pkcs8_der()
                .map_err(|e| CryptoError::EncodingFailed(format!("P384 private PKCS8 DER: {}", e)))?
                .as_bytes()
                .to_vec();

            let public_der = public
                .to_public_key_der()
                .map_err(|e| CryptoError::EncodingFailed(format!("P384 public DER: {}", e)))?
                .as_bytes()
                .to_vec();

            let private_pem = secret
                .to_pkcs8_pem(LineEnding::LF)
                .map_err(|e| CryptoError::EncodingFailed(format!("P384 private PEM: {}", e)))?
                .to_string();

            let public_pem = public
                .to_public_key_pem(LineEnding::LF)
                .map_err(|e| CryptoError::EncodingFailed(format!("P384 public PEM: {}", e)))?;

            Ok(KeyPairResult {
                public_key_der: public_der,
                private_key_der: private_der,
                public_key_pem: Some(public_pem),
                private_key_pem: Some(private_pem),
            })
        }
    }
}

fn generate_ed25519() -> Result<KeyPairResult, CryptoError> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&bytes);
    let verifying_key = signing_key.verifying_key();

    let private_der = signing_key
        .to_pkcs8_der()
        .map_err(|e| CryptoError::EncodingFailed(format!("Ed25519 private PKCS8 DER: {}", e)))?
        .as_bytes()
        .to_vec();

    let public_der = verifying_key
        .to_public_key_der()
        .map_err(|e| CryptoError::EncodingFailed(format!("Ed25519 public DER: {}", e)))?
        .as_bytes()
        .to_vec();

    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| CryptoError::EncodingFailed(format!("Ed25519 private PEM: {}", e)))?
        .to_string();

    let public_pem = verifying_key
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| CryptoError::EncodingFailed(format!("Ed25519 public PEM: {}", e)))?;

    Ok(KeyPairResult {
        public_key_der: public_der,
        private_key_der: private_der,
        public_key_pem: Some(public_pem),
        private_key_pem: Some(private_pem),
    })
}

fn generate_x25519() -> Result<KeyPairResult, CryptoError> {
    let secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
    let public = x25519_dalek::PublicKey::from(&secret);

    let private_bytes = secret.to_bytes();
    let public_bytes = public.to_bytes();

    let private_der = build_x25519_private_pkcs8_der(&private_bytes, &public_bytes);
    let public_der = build_x25519_public_pkcs8_der(&public_bytes);

    let private_pem = pem_rfc7468::encode_string("PRIVATE KEY", pem_rfc7468::LineEnding::LF, &private_der)
        .map_err(|e| CryptoError::EncodingFailed(format!("X25519 private PEM: {}", e)))?;

    let public_pem = pem_rfc7468::encode_string("PUBLIC KEY", pem_rfc7468::LineEnding::LF, &public_der)
        .map_err(|e| CryptoError::EncodingFailed(format!("X25519 public PEM: {}", e)))?;

    Ok(KeyPairResult {
        public_key_der: public_der,
        private_key_der: private_der,
        public_key_pem: Some(public_pem),
        private_key_pem: Some(private_pem),
    })
}

// X25519 OID: 1.3.101.110
const X25519_OID: [u8; 5] = [
    0x06, 0x03, 0x2B, 0x65, 0x6E, // OID 1.3.101.110
];

fn build_x25519_private_pkcs8_der(private_bytes: &[u8; 32], public_bytes: &[u8; 32]) -> Vec<u8> {
    // RFC 8410 OneAsymmetricKey:
    // SEQUENCE {
    //   INTEGER 0x01 (v2 for publicKey field)
    //   SEQUENCE { OID 1.3.101.110 }
    //   OCTET STRING { OCTET STRING { private_bytes } }
    //   [1] BIT STRING { 0x00, public_bytes }
    // }

    // Inner OCTET STRING wrapping private key (CurvePrivateKey)
    let mut inner_octet_string = vec![0x04, 0x20]; // OCTET STRING, length 32
    inner_octet_string.extend_from_slice(private_bytes);

    // AlgorithmIdentifier SEQUENCE
    let mut alg_seq = vec![0x30, 0x05]; // SEQUENCE, length 5
    alg_seq.extend_from_slice(&X25519_OID);

    // Public key [1] IMPLICIT BIT STRING
    let mut pub_key_cs = vec![0xA1, 0x23]; // context [1], length 35
    pub_key_cs.extend_from_slice(&[0x03, 0x21, 0x00]); // BIT STRING, length 33, 0 unused bits
    pub_key_cs.extend_from_slice(public_bytes);

    // Outer SEQUENCE
    let inner_len = 3 + alg_seq.len() + inner_octet_string.len() + pub_key_cs.len();
    let mut result = Vec::with_capacity(4 + inner_len);
    result.push(0x30); // SEQUENCE tag
    encode_length(&mut result, inner_len);
    // version = 1 (v2, includes publicKey)
    result.extend_from_slice(&[0x02, 0x01, 0x01]);
    result.extend_from_slice(&alg_seq);
    result.extend_from_slice(&inner_octet_string);
    result.extend_from_slice(&pub_key_cs);

    result
}

fn build_x25519_public_pkcs8_der(public_bytes: &[u8; 32]) -> Vec<u8> {
    // SubjectPublicKeyInfo:
    // SEQUENCE {
    //   SEQUENCE { OID 1.3.101.110 }
    //   BIT STRING { 0x00, public_bytes }
    // }

    let mut alg_seq = vec![0x30, 0x05];
    alg_seq.extend_from_slice(&X25519_OID);

    // BIT STRING: tag 0x03, length 33, 0 unused bits, 32 bytes
    let mut bit_string = vec![0x03, 0x21, 0x00];
    bit_string.extend_from_slice(public_bytes);

    let inner_len = alg_seq.len() + bit_string.len();
    let mut result = Vec::with_capacity(4 + inner_len);
    result.push(0x30);
    encode_length(&mut result, inner_len);
    result.extend_from_slice(&alg_seq);
    result.extend_from_slice(&bit_string);

    result
}

fn encode_length(buf: &mut Vec<u8>, len: usize) {
    if len < 128 {
        buf.push(len as u8);
    } else if len < 256 {
        buf.push(0x81);
        buf.push(len as u8);
    } else {
        buf.push(0x82);
        buf.push((len >> 8) as u8);
        buf.push((len & 0xFF) as u8);
    }
}
