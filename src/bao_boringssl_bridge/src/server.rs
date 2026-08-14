// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL TLS server configuration.
//!
//! Uses BoringSSL's `SSL_CTX_use_certificate` / `SSL_CTX_use_PrivateKey` API
//! for loading TLS credentials. PEM parsing is done via `PEM_read_bio_X509`
//! and `PEM_read_bio_PrivateKey`.

use bun_boringssl_sys::boringssl::*;
use core::ffi::{c_long, c_void};

use crate::connection::{
    SSL_CTX_set_select_certificate_cb as ffi_set_select_cert_cb, SslClientHello, TlsConnection,
    TlsError, new_tls_ctx,
};

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
    /// `PEM_read_bio_PrivateKey`, then loads them into the SSL_CTX. The
    /// ctx uses the crypto-X509 method — see [`new_tls_ctx`] for why the
    /// PEM loaders require it.
    pub fn new(pem_certs: &str, pem_key: &str) -> Result<Self, TlsError> {
        bun_boringssl::load();

        let ctx = new_tls_ctx()?;

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
                return Err(TlsError::InvalidCertKey(
                    "failed to load certificate".to_string(),
                ));
            }
        }
        unsafe { BIO_free(cert_bio) };

        // Load private key from PEM
        let key_bio =
            unsafe { BIO_new_mem_buf(pem_key.as_ptr() as *const c_void, pem_key.len() as isize) };
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
            return Err(TlsError::InvalidCertKey(
                "failed to parse private key".to_string(),
            ));
        }

        let ok = unsafe { SSL_CTX_use_PrivateKey(ctx, pkey) };
        unsafe { EVP_PKEY_free(pkey) };
        unsafe { BIO_free(key_bio) };

        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey(
                "failed to load private key".to_string(),
            ));
        }

        Ok(Self { ctx })
    }

    /// Create a new TLS server from DER-encoded certificate and private key.
    pub fn new_from_der(cert_der: &[u8], key_der: &[u8]) -> Result<Self, TlsError> {
        bun_boringssl::load();

        let ctx = new_tls_ctx()?;

        // Load certificate from DER
        let ok = unsafe { SSL_CTX_use_certificate_ASN1(ctx, cert_der.len(), cert_der.as_ptr()) };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey(
                "failed to load DER certificate".to_string(),
            ));
        }

        // Load private key from DER (EVP_PKEY_RSA = 6)
        let ok = unsafe {
            SSL_CTX_use_PrivateKey_ASN1(6, ctx, key_der.as_ptr(), key_der.len() as c_long)
        };
        if ok == 0 {
            unsafe { SSL_CTX_free(ctx) };
            return Err(TlsError::InvalidCertKey(
                "failed to load DER private key".to_string(),
            ));
        }

        Ok(Self { ctx })
    }

    /// Accept a TLS connection.
    pub fn accept(&self) -> Result<TlsConnection, TlsError> {
        TlsConnection::new_server_boringssl(self)
    }

    /// Register the BoringSSL select-certificate callback on this server's
    /// `SSL_CTX`. The callback fires early in the server handshake (before
    /// most ClientHello processing); it may inspect the SNI servername via
    /// [`TlsConnection::servername`], switch the connection's certificate
    /// configuration via [`TlsConnection::switch_ssl_ctx`], and defer the
    /// decision by returning retry (the handshake then suspends with
    /// `TlsState::PendingCertificate` until the caller resolves the
    /// credential and drives `process` again).
    ///
    /// This is the injection seam for node:tls `SNICallback` support.
    pub fn set_select_certificate_callback(
        &self,
        cb: Option<
            unsafe extern "C" fn(client_hello: *const SslClientHello) -> core::ffi::c_int,
        >,
    ) {
        unsafe { ffi_set_select_cert_cb(self.ctx, cb) }
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
