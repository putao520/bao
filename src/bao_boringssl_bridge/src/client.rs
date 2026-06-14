// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL TLS client configuration.

use bun_boringssl_sys::boringssl::*;

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
    /// Uses the same cipher/kx setup as `bun_boringssl::init_client()`.
    pub fn new() -> Result<Self, TlsError> {
        // Ensure BoringSSL is initialized
        bun_boringssl::load();

        let ctx = unsafe { SSL_CTX_new(TLS_with_buffers_method()) };
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
}

impl Drop for TlsClient {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { SSL_CTX_free(self.ctx) };
        }
    }
}

// Safety: SSL_CTX is thread-safe in BoringSSL after creation.
unsafe impl Send for TlsClient {}
unsafe impl Sync for TlsClient {}

use crate::connection::TlsError;
