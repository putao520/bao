//! TLS security-information snapshot for the HTTP client result path.
//!
//! Servo-bridge / JS consumers need the post-handshake TLS facts of the
//! connection that produced a response (the equivalent of hyper-rustls's
//! `TlsHandshakeInfo` surface: protocol, cipher, peer certificate). The
//! `SSL*` only lives on the HTTP thread while its socket is alive, so the
//! facts are snapshotted into this plain-data struct at handshake
//! completion (or, for pooled-socket adoption, at first result delivery)
//! and ride `HTTPClientResult::tls_info` out to the consumer — the same
//! driver-snapshot pattern as `TlsSessionInfo` in `node_tls.rs`.
//!
//! Extraction delegates to `bao_boringssl_bridge`'s standalone SSL
//! accessors (the `PeerCertInfo` FFI surface) so there is exactly one
//! BoringSSL field-extraction implementation in the tree.

use bao_boringssl_bridge::{PeerCertInfo, ssl_cipher_bits, ssl_cipher_name, ssl_cipher_version,
                           ssl_peer_cert_info, ssl_peer_certificates_der, ssl_protocol_version};
use bun_boringssl_sys::SSL;

/// One TLS connection's negotiated security facts, snapshotted on the HTTP
/// thread while the `SSL` is live. All plain data (`Send`), no SSL pointers.
///
/// `None` fields mean BoringSSL could not produce the value — never a
/// placeholder. In particular `mac` is always `None` for the suites
/// BoringSSL negotiates: they are AEAD ciphers where authentication is
/// integral to the cipher (`cipher_bits` carries the encryption strength).
#[derive(Clone, Debug, Default)]
pub struct BunTlsInfo {
    /// `SSL_get_version` — e.g. "TLSv1.3".
    pub protocol_version: Option<String>,
    /// `SSL_CIPHER_get_name` — e.g. "TLS_AES_256_GCM_SHA384".
    pub cipher_suite: Option<String>,
    /// `SSL_CIPHER_get_version` — BoringSSL's static "TLSv1/SSLv3".
    pub cipher_version: Option<String>,
    /// Effective symmetric-key strength of the negotiated cipher.
    pub cipher_bits: Option<i32>,
    /// Maximum strength of the cipher's algorithm (differs for export
    /// ciphers; equals `cipher_bits` otherwise).
    pub cipher_alg_bits: Option<i32>,
    /// Separate MAC algorithm. AEAD suites (all BoringSSL negotiates)
    /// authenticate within the cipher, so this is `None` — the field exists
    /// so consumers can distinguish "integral AEAD MAC" from "absent".
    pub mac: Option<String>,
    /// ALPN protocol negotiated on this connection (e.g. "h2").
    pub alpn: Option<Vec<u8>>,
    /// Parsed leaf certificate (Node `getPeerCertificate` field set).
    /// `None` when the peer presented no certificate.
    pub peer_certificate: Option<PeerCertInfo>,
    /// Full peer certificate chain as DER, leaf first. Empty when the peer
    /// presented no chain.
    pub peer_certificates_der: Vec<Vec<u8>>,
}

impl BunTlsInfo {
    /// Snapshot the negotiated facts from a live `SSL`.
    ///
    /// # Safety
    /// `ssl` must be a live `*const SSL` on the calling thread for the
    /// duration of this call (the HTTP-thread socket's native handle), with
    /// the handshake completed — the values read are only meaningful then.
    pub unsafe fn from_ssl(ssl: *const SSL) -> Self {
        let (cipher_bits, cipher_alg_bits) = match ssl_cipher_bits(ssl) {
            Some((bits, alg_bits)) => (Some(bits), Some(alg_bits)),
            None => (None, None),
        };
        let mut alpn_len: core::ffi::c_uint = 0;
        let mut alpn_ptr: *const u8 = core::ptr::null();
        // SAFETY: caller contract — ssl is live; out-params are valid stack
        // locals. `alpn_ptr[0..alpn_len]` is the slice ALPN wrote, borrowed
        // from the session and copied out below.
        unsafe { bun_boringssl_sys::SSL_get0_alpn_selected(ssl, &raw mut alpn_ptr, &raw mut alpn_len) };
        let alpn = if alpn_ptr.is_null() || alpn_len == 0 {
            None
        } else {
            // SAFETY: alpn_ptr/alpn_len delimit the negotiated protocol bytes.
            Some(unsafe { core::slice::from_raw_parts(alpn_ptr, alpn_len as usize) }.to_vec())
        };
        Self {
            protocol_version: ssl_protocol_version(ssl),
            cipher_suite: ssl_cipher_name(ssl),
            cipher_version: ssl_cipher_version(ssl),
            cipher_bits,
            cipher_alg_bits,
            mac: None,
            alpn,
            peer_certificate: ssl_peer_cert_info(ssl),
            peer_certificates_der: ssl_peer_certificates_der(ssl),
        }
    }
}

// ported from: (new) — hyper-rustls TlsHandshakeInfo-equivalent surface
