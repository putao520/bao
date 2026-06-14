// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL-backed TLS 1.2/1.3 for Bao runtime.
//!
//! Replaces bao_tls (rustls) with BoringSSL C++ library, unifying the TLS
//! stack with Bun upstream. Provides the same public API as bao_tls so
//! downstream crates (bun_runtime, bao_browser) can switch transparently.
//!
//! # Architecture
//!
//! ```text
//! bun_runtime (node_tls.rs)
//!   └── bao_boringssl_bridge
//!         ├── bun_boringssl / bun_boringssl_sys (C++ BoringSSL)
//!         ├── bao_crypto (unified crypto re-exports)
//!         └── bun_uws_sys (Unbuffered zero-copy TLS)
//! ```

pub mod client;
pub mod connection;
pub mod profile;
pub mod server;
pub mod socket;

pub use client::TlsClient;
pub use connection::{TlsConnection, TlsError, ProcessResult, TlsState};
pub use profile::TlsProfile;
pub use server::TlsServer;

// ─── BoringSSL PEM parsing helpers ───────────────────────────────────
//
// These functions parse PEM-encoded certificates and private keys using
// BoringSSL's PEM_read_bio_X509 / PEM_read_bio_PrivateKey, returning
// DER-encoded bytes. Downstream crates (bun_runtime) use these instead
// of rustls_pemfile.

use core::ffi::c_void;
use bun_boringssl_sys::boringssl::*;

/// Parse PEM certificates and return DER bytes for each.
pub fn pem_parse_certs(pem: &str) -> Vec<Vec<u8>> {
    let bio = unsafe { BIO_new_mem_buf(pem.as_ptr() as *const c_void, pem.len() as isize) };
    if bio.is_null() {
        return Vec::new();
    }
    let mut ders = Vec::new();
    loop {
        let x509 = unsafe { PEM_read_bio_X509(bio, std::ptr::null_mut(), None, std::ptr::null_mut()) };
        if x509.is_null() {
            break;
        }
        let len = unsafe { i2d_X509(x509, std::ptr::null_mut()) };
        if len > 0 {
            let mut buf = vec![0u8; len as usize];
            let mut p = buf.as_mut_ptr();
            unsafe { i2d_X509(x509, &mut p) };
            ders.push(buf);
        }
        unsafe { X509_free(x509) };
    }
    unsafe { BIO_free(bio) };
    ders
}

/// Key format tag for private key DER bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFormat {
    Pkcs1,
    Pkcs8,
    Sec1,
}

/// Parse PEM private key and return (format, DER bytes).
///
/// Uses BoringSSL's `PEM_read_bio_PrivateKey` to validate the PEM is a
/// valid private key, then stores the raw PEM bytes tagged as Pkcs8.
/// `TlsServer::new(pem_certs, pem_key)` accepts PEM strings directly,
/// so DER conversion is unnecessary.
pub fn pem_parse_key(pem: &str) -> Option<(KeyFormat, Vec<u8>)> {
    let bio = unsafe { BIO_new_mem_buf(pem.as_ptr() as *const c_void, pem.len() as isize) };
    if bio.is_null() {
        return None;
    }
    let pkey = unsafe { PEM_read_bio_PrivateKey(bio, std::ptr::null_mut(), None, std::ptr::null_mut()) };
    unsafe { BIO_free(bio) };
    if pkey.is_null() {
        return None;
    }
    unsafe { EVP_PKEY_free(pkey) };
    // Store PEM bytes tagged as Pkcs8. TlsServer::new() accepts PEM directly.
    Some((KeyFormat::Pkcs8, pem.as_bytes().to_vec()))
}
