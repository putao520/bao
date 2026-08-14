// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL TLS client configuration.

use bun_boringssl_sys::boringssl::*;

// Trust-store symbols compiled into the vendored library but not yet in
// the hand-rolled bindings (ground truth: vendor/boringssl/include/openssl).
unsafe extern "C" {
    fn SSL_CTX_get_cert_store(ctx: *mut SSL_CTX) -> *mut X509_STORE;
    fn X509_STORE_add_cert(store: *mut X509_STORE, x509: *mut X509) -> core::ffi::c_int;
    safe fn TLS_method() -> *const SSL_METHOD;
}

/// BoringSSL-backed TLS client.
///
/// Wraps an `SSL_CTX` configured for client-side TLS connections.
/// Compatible with `bao_tls::TlsClient` API but backed by BoringSSL.
pub struct TlsClient {
    ctx: *mut SSL_CTX,
}

impl TlsClient {
    /// Create a new TLS client with default BoringSSL configuration.
    ///
    /// Uses the crypto-X509 method (`TLS_method`): the trust-store /
    /// verify-param APIs (`SSL_CTX_get_cert_store`, …) assert on a
    /// buffers-method ctx (`check_ssl_ctx_x509_method`), and the client
    /// verifies peer certificates by default. Wire behavior is identical
    /// to the buffers method; this only selects the certificate
    /// representation inside BoringSSL.
    pub fn new() -> Result<Self, TlsError> {
        // Ensure BoringSSL is initialized
        bun_boringssl::load();

        let ctx = unsafe { SSL_CTX_new(TLS_method()) };
        if ctx.is_null() {
            return Err(TlsError::BoringSSL("SSL_CTX_new failed"));
        }

        // Configure cipher list (same as Bun upstream)
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

        Ok(Self { ctx })
    }

    /// Get the underlying `SSL_CTX` pointer.
    pub fn ctx(&self) -> *mut SSL_CTX {
        self.ctx
    }

    /// Trust an additional DER-encoded certificate (CA or self-signed leaf)
    /// for peer verification — the BoringSSL client verifies by default, so
    /// private-CA / self-signed servers must be anchored here (Node's `ca`
    /// option equivalent).
    pub fn add_trusted_der(&self, der: &[u8]) -> bool {
        let mut p = der.as_ptr();
        // SAFETY: d2i_X509 reads from the DER slice; the returned X509 is
        // owned by us and freed after adding to the store (X509_STORE_add_cert
        // up-refs internally).
        let x509 = unsafe { d2i_X509(core::ptr::null_mut(), &mut p, der.len() as core::ffi::c_long) };
        if x509.is_null() {
            return false;
        }
        // SAFETY: self.ctx is a live SSL_CTX; its cert store outlives the ctx.
        let ok = unsafe {
            let store = SSL_CTX_get_cert_store(self.ctx);
            let r = if store.is_null() {
                0
            } else {
                X509_STORE_add_cert(store, x509)
            };
            X509_free(x509);
            r
        };
        ok == 1
    }
}

impl Drop for TlsClient {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { SSL_CTX_free(self.ctx) };
        }
    }
}

impl Clone for TlsClient {
    fn clone(&self) -> Self {
        unsafe { SSL_CTX_up_ref(self.ctx) };
        Self { ctx: self.ctx }
    }
}

// Safety: SSL_CTX is thread-safe in BoringSSL after creation.
unsafe impl Send for TlsClient {}
unsafe impl Sync for TlsClient {}

use crate::connection::TlsError;
