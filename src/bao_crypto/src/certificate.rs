use crate::CryptoError;
use x509_cert::der::Decode;
use sha1::Sha1;
use sha2::{Digest, Sha256};

pub struct X509Certificate {
    der_bytes: Vec<u8>,
    inner: x509_cert::Certificate,
}

impl X509Certificate {
    pub fn from_pem(pem: &str) -> Result<X509Certificate, CryptoError> {
        let (label, der) = pem_rfc7468::decode_vec(pem.as_bytes())
            .map_err(|e| CryptoError::InvalidCertificate(format!("PEM decode: {}", e)))?;

        if label != "CERTIFICATE" {
            return Err(CryptoError::InvalidCertificate(format!(
                "Expected CERTIFICATE PEM block, got: {}",
                label
            )));
        }

        Self::from_der(&der)
    }

    pub fn from_der(der: &[u8]) -> Result<X509Certificate, CryptoError> {
        let inner = x509_cert::Certificate::from_der(der)
            .map_err(|e| CryptoError::InvalidCertificate(format!("DER parse: {}", e)))?;
        Ok(X509Certificate {
            der_bytes: der.to_vec(),
            inner,
        })
    }

    pub fn subject(&self) -> String {
        format!("{}", self.inner.tbs_certificate().subject())
    }

    pub fn issuer(&self) -> String {
        format!("{}", self.inner.tbs_certificate().issuer())
    }

    pub fn fingerprint_sha256(&self) -> String {
        let hash = Sha256::digest(&self.der_bytes);
        hex_colon(&hash)
    }

    pub fn fingerprint_sha1(&self) -> String {
        let hash = Sha1::digest(&self.der_bytes);
        hex_colon(&hash)
    }

    pub fn valid_from(&self) -> String {
        format!("{}", self.inner.tbs_certificate().validity().not_before)
    }

    pub fn valid_to(&self) -> String {
        format!("{}", self.inner.tbs_certificate().validity().not_after)
    }

    pub fn serial_number(&self) -> String {
        let serial = self.inner.tbs_certificate().serial_number();
        let bytes = serial.as_bytes();
        hex_no_colon(bytes)
    }
}

fn hex_colon(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}

fn hex_no_colon(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect()
}
