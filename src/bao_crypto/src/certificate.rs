//! X.509 certificate parsing and fingerprinting.
//!
//! Implements `new X509Certificate()` for Node.js crypto.

use sha2::{Digest, Sha256};
use sha1::Sha1;
use der::Decode;

use crate::sign::CryptoError;

/// Parsed X.509 certificate.
pub struct X509Certificate {
    raw_der: Vec<u8>,
    cert: x509_cert::Certificate,
}

impl X509Certificate {
    /// Parse a DER-encoded X.509 certificate.
    pub fn from_der(der: &[u8]) -> Result<Self, CryptoError> {
        let cert = x509_cert::Certificate::from_der(der)
            .map_err(|e| CryptoError::EncodingError(e.to_string()))?;
        Ok(Self {
            raw_der: der.to_vec(),
            cert,
        })
    }

    /// Parse a PEM-encoded X.509 certificate.
    pub fn from_pem(pem: &str) -> Result<Self, CryptoError> {
        let (label, der) = pem_rfc7468::decode_vec(pem.as_bytes())
            .map_err(|e| CryptoError::EncodingError(e.to_string()))?;
        if label != "CERTIFICATE" {
            return Err(CryptoError::EncodingError(format!(
                "expected CERTIFICATE label, got {label}"
            )));
        }
        Self::from_der(&der)
    }

    /// SHA-256 fingerprint (hex with colons, e.g., "AB:CD:EF:...").
    pub fn fingerprint_sha256(&self) -> String {
        let hash = Sha256::digest(&self.raw_der);
        hash.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// SHA-1 fingerprint (hex with colons).
    pub fn fingerprint_sha1(&self) -> String {
        let hash = Sha1::digest(&self.raw_der);
        hash.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Subject distinguished name (RFC 2253 format).
    pub fn subject(&self) -> String {
        format!("{}", self.cert.tbs_certificate.subject)
    }

    /// Issuer distinguished name (RFC 2253 format).
    pub fn issuer(&self) -> String {
        format!("{}", self.cert.tbs_certificate.issuer)
    }

    /// Serial number as hex string.
    pub fn serial_number(&self) -> String {
        let serial = &self.cert.tbs_certificate.serial_number;
        let bytes = serial.as_bytes();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Not Before (ISO 8601).
    pub fn valid_from(&self) -> String {
        format!("{}", self.cert.tbs_certificate.validity.not_before)
    }

    /// Not After (ISO 8601).
    pub fn valid_to(&self) -> String {
        format!("{}", self.cert.tbs_certificate.validity.not_after)
    }

    /// Raw DER bytes.
    pub fn raw_der(&self) -> &[u8] {
        &self.raw_der
    }
}
