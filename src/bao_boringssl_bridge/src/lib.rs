// @trace REQ-ENG-007 [entity:TlsProfile]
// @trace REQ-PURE-001 [level:library] [entity:TlsProfile,TlsConnection]
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
pub mod session_cache;
pub mod socket;

pub use client::TlsClient;
pub use connection::{
    ProcessResult, SslClientHello, TlsConnection, TlsError, TlsState, SSL_SELECT_CERT_ERROR,
    SSL_SELECT_CERT_RETRY, SSL_SELECT_CERT_SUCCESS, ssl_servername,
};
pub use profile::TlsProfile;
pub use server::TlsServer;
pub use session_cache::{ClientSessionCache, SslSession, offer_session, session_reused};

// ─── BoringSSL PEM parsing helpers ───────────────────────────────────
//
// These functions parse PEM-encoded certificates and private keys using
// BoringSSL's PEM_read_bio_X509 / PEM_read_bio_PrivateKey, returning
// DER-encoded bytes. Downstream crates (bun_runtime) use these instead
// of rustls_pemfile.

use bun_boringssl_sys::boringssl::*;
use core::ffi::c_void;

/// Parse PEM certificates and return DER bytes for each.
pub fn pem_parse_certs(pem: &str) -> Vec<Vec<u8>> {
    let bio = unsafe { BIO_new_mem_buf(pem.as_ptr() as *const c_void, pem.len() as isize) };
    if bio.is_null() {
        return Vec::new();
    }
    let mut ders = Vec::new();
    loop {
        let x509 =
            unsafe { PEM_read_bio_X509(bio, std::ptr::null_mut(), None, std::ptr::null_mut()) };
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
    let pkey =
        unsafe { PEM_read_bio_PrivateKey(bio, std::ptr::null_mut(), None, std::ptr::null_mut()) };
    unsafe { BIO_free(bio) };
    if pkey.is_null() {
        return None;
    }
    unsafe { EVP_PKEY_free(pkey) };
    // Store PEM bytes tagged as Pkcs8. TlsServer::new() accepts PEM directly.
    Some((KeyFormat::Pkcs8, pem.as_bytes().to_vec()))
}

// ─── Self-signed certificate generation (X509 creation FFI) ──────────
//
// BoringSSL exposes the minimal X509-creation surface needed to build
// self-signed certificates (the same primitives OpenSSL-based dev servers
// use for localhost certificates). The symbols are compiled into the
// vendored library but not declared in the hand-rolled bindings yet —
// declared locally, same as the SNI surface in connection.rs.
//
// Ground truth: vendor/boringssl/include/openssl/{x509.h,rsa.h,evp.h}.

use core::ffi::{c_char, c_int, c_long};

unsafe extern "C" {
    // Only the symbols missing from `bun_boringssl_sys` are declared here;
    // BN_*, RSA_new/free, EVP_PKEY_new, X509_get_serialNumber,
    // X509_get_subject_name, EVP_sha256, BIO_* come from the bindings.
    fn EVP_PKEY_assign_RSA(pkey: *mut EVP_PKEY, key: *mut RSA) -> c_int;
    fn X509_new() -> *mut X509;
    fn X509_set_version(x509: *mut X509, version: c_long) -> c_int;
    fn ASN1_INTEGER_set(a: *mut ASN1_INTEGER, v: c_long) -> c_int;
    fn X509_gmtime_adj(s: *mut ASN1_TIME, offset_sec: c_long) -> *mut ASN1_TIME;
    fn X509_getm_notBefore(x509: *mut X509) -> *mut ASN1_TIME;
    fn X509_getm_notAfter(x509: *mut X509) -> *mut ASN1_TIME;
    fn X509_set_subject_name(x509: *mut X509, name: *mut X509_NAME) -> c_int;
    fn X509_set_issuer_name(x509: *mut X509, name: *mut X509_NAME) -> c_int;
    fn X509_NAME_add_entry_by_txt(
        name: *mut X509_NAME,
        field: *const c_char,
        ty: c_int,
        bytes: *const u8,
        len: c_int,
        loc: c_int,
        set: c_int,
    ) -> c_int;
    fn X509_set_pubkey(x509: *mut X509, key: *mut EVP_PKEY) -> c_int;
    fn X509_sign(x509: *mut X509, key: *mut EVP_PKEY, md: *const EVP_MD) -> c_int;
    fn PEM_write_bio_X509(bio: *mut BIO, x509: *const X509) -> c_int;
}

/// MBSTRING_ASC flag for X509_NAME_add_entry_by_txt (openssl/asn1.h).
const MBSTRING_ASC: c_int = 0x1000 | 1;

/// Generate a fresh RSA-2048 self-signed certificate for `cn`, valid from
/// now for `days`, signed with SHA-256. Returns `(cert_pem, key_pem)` where
/// the key is emitted as PKCS#8 PEM (parseable by `pem_parse_key` and
/// loadable by `TlsServer::new`).
///
/// This is a real certificate (proper X509v3 structure, real RSA key), not
/// a fixture blob — used by tests and available for dev-server style
/// localhost certificates.
pub fn generate_self_signed_pem(cn: &str, days: c_long) -> Result<(String, String), TlsError> {
    bun_boringssl::load();

    // ── key: RSA 2048, e = 65537 ──────────────────────────────────────────
    let rsa = unsafe { RSA_new() };
    if rsa.is_null() {
        return Err(TlsError::BoringSSL("RSA_new failed"));
    }
    let e = unsafe { BN_new() };
    if e.is_null() || unsafe { BN_set_word(e, 65537) } != 1 {
        unsafe {
            RSA_free(rsa);
            if !e.is_null() {
                BN_free(e);
            }
        }
        return Err(TlsError::BoringSSL("BN_new/BN_set_word failed"));
    }
    let gen_ok = unsafe { RSA_generate_key_ex(rsa, 2048, e, core::ptr::null_mut()) };
    unsafe { BN_free(e) };
    if gen_ok != 1 {
        unsafe { RSA_free(rsa) };
        return Err(TlsError::BoringSSL("RSA_generate_key_ex failed"));
    }

    let pkey = unsafe { EVP_PKEY_new() };
    if pkey.is_null() {
        unsafe { RSA_free(rsa) };
        return Err(TlsError::BoringSSL("EVP_PKEY_new failed"));
    }
    // On success ownership of `rsa` transfers to `pkey`.
    if unsafe { EVP_PKEY_assign_RSA(pkey, rsa) } != 1 {
        unsafe {
            RSA_free(rsa);
            EVP_PKEY_free(pkey);
        }
        return Err(TlsError::BoringSSL("EVP_PKEY_assign_RSA failed"));
    }

    // ── certificate ───────────────────────────────────────────────────────
    let x509 = unsafe { X509_new() };
    if x509.is_null() {
        unsafe { EVP_PKEY_free(pkey) };
        return Err(TlsError::BoringSSL("X509_new failed"));
    }
    let build_ok = unsafe {
        X509_set_version(x509, 2) == 1 // v3
            && ASN1_INTEGER_set(X509_get_serialNumber(x509), 1) == 1
            && !X509_gmtime_adj(X509_getm_notBefore(x509), 0).is_null()
            && !X509_gmtime_adj(X509_getm_notAfter(x509), days * 24 * 60 * 60).is_null()
            && X509_NAME_add_entry_by_txt(
                X509_get_subject_name(x509),
                c"CN".as_ptr(),
                MBSTRING_ASC,
                cn.as_ptr(),
                cn.len() as c_int,
                -1,
                0,
            ) == 1
            && X509_set_subject_name(x509, X509_get_subject_name(x509)) == 1
            && X509_set_issuer_name(x509, X509_get_subject_name(x509)) == 1
            && X509_set_pubkey(x509, pkey) == 1
            && X509_sign(x509, pkey, EVP_sha256()) != 0
    };
    if !build_ok {
        unsafe {
            X509_free(x509);
            EVP_PKEY_free(pkey);
        }
        return Err(TlsError::InvalidCertKey("self-signed cert build failed".to_string()));
    }

    // ── serialize: cert PEM + PKCS#8 key PEM ─────────────────────────────
    let cert_pem = read_bio_pem(|bio| unsafe { PEM_write_bio_X509(bio, x509) });
    let key_pem = read_bio_pem(|bio| unsafe {
        PEM_write_bio_PKCS8PrivateKey(
            bio,
            pkey,
            core::ptr::null(),
            core::ptr::null_mut(),
            0,
            None,
            core::ptr::null_mut(),
        )
    });
    unsafe {
        X509_free(x509);
        EVP_PKEY_free(pkey);
    }
    match (cert_pem, key_pem) {
        (Some(c), Some(k)) => Ok((c, k)),
        _ => Err(TlsError::InvalidCertKey(
            "self-signed PEM serialization failed".to_string(),
        )),
    }
}

/// Write `write_fn` output to a memory BIO and read it back as a String.
fn read_bio_pem<F: Fn(*mut BIO) -> c_int>(write_fn: F) -> Option<String> {
    let bio = unsafe { BIO_new(BIO_s_mem()) };
    if bio.is_null() {
        return None;
    }
    let ok = write_fn(bio);
    let out = if ok == 1 {
        let pending = unsafe { BIO_ctrl_pending(bio) };
        if pending > 0 {
            let mut buf = vec![0u8; pending];
            let n = unsafe { BIO_read(bio, buf.as_mut_ptr() as *mut c_void, pending as c_int) };
            if n > 0 {
                buf.truncate(n as usize);
                String::from_utf8(buf).ok()
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    unsafe { BIO_free(bio) };
    out
}
