// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL TLS server configuration.
//!
//! Uses BoringSSL's `SSL_CTX_use_certificate` / `SSL_CTX_use_PrivateKey` API
//! for loading TLS credentials. PEM parsing is done via `PEM_read_bio_X509`
//! and `PEM_read_bio_PrivateKey`.

use core::ffi::{c_long, c_void};
use bun_boringssl_sys::boringssl::*;

use crate::connection::{TlsConnection, TlsError};

/// BoringSSL-backed TLS server.
///
/// Wraps an `SSL_CTX` configured for server-side TLS connections.
pub struct TlsServer {
    ctx: *mut SSL_CTX,
}

impl TlsServer {
    /// Create a new TLS server with PEM-encoded certificate and private key.
    ///
    /// Parses the PEM data using BoringSSL's `PEM_read_bio_X509` and
    /// `PEM_read_bio_PrivateKey`, then loads them into the SSL_CTX.
    pub fn new(pem_certs: &str, pem_key: &str) -> Result<Self, TlsError> {
        bun_boringssl::load();

        let ctx = unsafe { SSL_CTX_new(TLS_with_buffers_method()) };
        if ctx.is_null() {
            return Err(TlsError::BoringSSL("SSL_CTX_new failed"));
        }

        // Configure cipher list
        let ok = unsafe {
            SSL_CTX_set_cipher_list(
                ctx,
                c"TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305".as_ptr(),
            )
        };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::BoringSSL("SSL_CTX_set_cipher_list failed"));
        }

        // Load certificate chain from PEM
        let cert_bio = unsafe {
            BIO_new_mem_buf(
                pem_certs.as_ptr() as *const c_void,
                pem_certs.len() as isize,
            )
        };
        if cert_bio.is_null() {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::BoringSSL("BIO_new_mem_buf failed for cert"));
        }

        let mut is_first = true;
        loop {
            let x509 = unsafe {
                PEM_read_bio_X509(cert_bio, core::ptr::null_mut(), None, core::ptr::null_mut())
            };
            if x509.is_null() {
                break;
            }

            let ok = if is_first {
                is_first = false;
                unsafe { SSL_CTX_use_certificate(ctx, x509) }
            } else {
                unsafe { SSL_CTX_add1_chain_cert(ctx, x509) }
            };

            unsafe { X509_free(x509) };

            if ok == 0 {
                unsafe {
                    BIO_free(cert_bio);
                    SSL_CTX_free(ctx);
                }
                return Err(TlsError::InvalidCertKey("failed to load certificate".to_string()));
            }
        }
        unsafe { BIO_free(cert_bio) };

        // Load private key from PEM
        let key_bio = unsafe {
            BIO_new_mem_buf(
                pem_key.as_ptr() as *const c_void,
                pem_key.len() as isize,
            )
        };
        if key_bio.is_null() {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::BoringSSL("BIO_new_mem_buf failed for key"));
        }

        let pkey = unsafe {
            PEM_read_bio_PrivateKey(key_bio, core::ptr::null_mut(), None, core::ptr::null_mut())
        };
        if pkey.is_null() {
            unsafe {
                BIO_free(key_bio);
                SSL_CTX_free(ctx);
            }
            return Err(TlsError::InvalidCertKey("failed to parse private key".to_string()));
        }

        let ok = unsafe { SSL_CTX_use_PrivateKey(ctx, pkey) };
        unsafe { EVP_PKEY_free(pkey) };
        unsafe { BIO_free(key_bio) };

        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey("failed to load private key".to_string()));
        }

        Ok(Self { ctx })
    }

    /// Create a new TLS server from DER-encoded certificate and private key.
    pub fn new_from_der(cert_der: &[u8], key_der: &[u8]) -> Result<Self, TlsError> {
        bun_boringssl::load();

        let ctx = unsafe { SSL_CTX_new(TLS_with_buffers_method()) };
        if ctx.is_null() {
            return Err(TlsError::BoringSSL("SSL_CTX_new failed"));
        }

        // Configure cipher list
        let ok = unsafe {
            SSL_CTX_set_cipher_list(
                ctx,
                c"TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305".as_ptr(),
            )
        };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::BoringSSL("SSL_CTX_set_cipher_list failed"));
        }

        // Load certificate from DER
        let ok = unsafe {
            SSL_CTX_use_certificate_ASN1(ctx, cert_der.len(), cert_der.as_ptr())
        };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey("failed to load DER certificate".to_string()));
        }

        // Load private key from DER (EVP_PKEY_RSA = 6)
        let ok = unsafe {
            SSL_CTX_use_PrivateKey_ASN1(6, ctx, key_der.as_ptr(), key_der.len() as c_long)
        };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey("failed to load DER private key".to_string()));
        }

        Ok(Self { ctx })
    }

    /// Accept a TLS connection.
    pub fn accept(&self) -> Result<TlsConnection, TlsError> {
        TlsConnection::new_server_boringssl(self)
    }

    /// Get the underlying `SSL_CTX` pointer.
    pub fn ctx(&self) -> *mut SSL_CTX {
        self.ctx
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { SSL_CTX_free(self.ctx) };
        }
    }
}

// Safety: SSL_CTX is thread-safe in BoringSSL after creation.
unsafe impl Send for TlsServer {}
unsafe impl Sync for TlsServer {}
