// @trace REQ-ENG-007 [entity:TlsProfile]
//! BoringSSL TLS connection wrapper.
//!
//! Provides non-blocking TLS operations driven by BoringSSL's SSL API.
//! Uses two BIO pairs for buffer management:
//!
//! ```text
//!   SSL internal BIOs (owned by SSL, freed by SSL_free):
//!     - internal_rbio: SSL reads decrypted/handshake data from here
//!     - internal_wbio: SSL writes outgoing TLS records to here
//!
//!   Application-facing BIOs (peer side, freed by us):
//!     - network_read_bio:  Application BIO_write() to feed incoming ciphertext
//!     - network_write_bio: Application BIO_read() to extract outgoing ciphertext
//! ```

use std::ffi::{c_char, c_int, c_uint, c_void};

use bun_boringssl_sys::boringssl::*;

use crate::client::TlsClient;
use crate::server::TlsServer;

/// Maximum TLS record size (16 KiB + header overhead).
const TLS_RECORD_MAX: usize = 17_000;

// ─── SNI / async certificate-selection FFI surface ────────────────────
//
// These symbols are compiled into the vendored BoringSSL library (see
// `src/boringssl_sys/build.rs` — all of `ssl/*.cc` is in the build list)
// but are not yet declared in the hand-rolled `bun_boringssl_sys` bindings.
// They are declared here locally (same pattern as `node_net.rs`'s local
// `inet_ntop` declaration) until the bindgen pipeline replaces the bindings
// module wholesale.
//
// Ground truth: vendor/boringssl/include/openssl/ssl.h.

/// `#define SSL_ERROR_PENDING_CERTIFICATE 12` — the operation failed because
/// certificate selection (e.g. SNI-driven) is pending; retry later.
const SSL_ERROR_PENDING_CERTIFICATE: c_int = 12;

/// `enum ssl_select_cert_result_t` — return values for the
/// select-certificate callback (ssl.h). Consumed by the node:tls
/// SNICallback dispatch in `bun_runtime`.
pub const SSL_SELECT_CERT_SUCCESS: c_int = 1;
pub const SSL_SELECT_CERT_RETRY: c_int = 0;
pub const SSL_SELECT_CERT_ERROR: c_int = -1;

/// `#define TLSEXT_NAMETYPE_host_name 0` — the SNI server_name type.
const TLSEXT_NAMETYPE_HOST_NAME: c_int = 0;

/// `SSL_CLIENT_HELLO` — only the fields we consume. Layout ground truth:
/// `struct ssl_early_callback_ctx` in vendor/boringssl/include/openssl/ssl.h
/// (`SSL *ssl` is the first member).
#[repr(C)]
pub struct SslClientHello {
    pub ssl: *mut SSL,
}

// `TLS_method` is compiled into the vendored library but not yet declared
// in the hand-rolled bindings (only `TLS_with_buffers_method` is).
unsafe extern "C" {
    pub(crate) fn SSL_CTX_set_select_certificate_cb(
        ctx: *mut SSL_CTX,
        cb: Option<unsafe extern "C" fn(client_hello: *const SslClientHello) -> c_int>,
    );
    fn SSL_set_SSL_CTX(ssl: *mut SSL, ctx: *mut SSL_CTX) -> *mut SSL_CTX;
    fn SSL_get_peer_certificate(ssl: *const SSL) -> *mut X509;
    safe fn TLS_method() -> *const SSL_METHOD;
    // Session-info surface (protocol/cipher/peer-cert fields for the JS
    // `getProtocol`/`getCipher`/`getPeerCertificate`): compiled into the
    // vendored library but not yet in the hand-rolled bindings. Ground
    // truth: vendor/boringssl/include/openssl/{asn1.h,ssl.h}.
    fn ASN1_TIME_print(out: *mut BIO, a: *const ASN1_TIME) -> c_int;
    fn SSL_CIPHER_get_version(cipher: *const SSL_CIPHER) -> *const c_char;
    fn SSL_set1_host(ssl: *mut SSL, hostname: *const c_char) -> c_int;
}

// ─── Peer-certificate field extraction (Node getPeerCertificate shape) ──

// Standard X.509 NIDs for the RDN attributes surfaced as subject/issuer
// entries (vendor/boringssl/include/openssl/obj_mac.h). Only these are
// extracted; anything else is omitted rather than approximated.
const NID_COUNTRY_NAME: c_int = 14;
const NID_LOCALITY_NAME: c_int = 15;
const NID_STATE_OR_PROVINCE_NAME: c_int = 16;
const NID_ORGANIZATION_NAME: c_int = 17;
const NID_ORGANIZATIONAL_UNIT_NAME: c_int = 18;
const NID_PKCS9_EMAIL_ADDRESS: c_int = 48;

/// One RDN attribute of a certificate name: short key ("CN", "O", …) and
/// its UTF-8 value.
pub struct CertNameEntry {
    pub key: &'static str,
    pub value: String,
}

/// Parsed leaf-certificate fields for the Node `getPeerCertificate()`
/// surface. `None` fields are ones BoringSSL could not produce — the JS
/// layer surfaces those as `undefined`, never as a placeholder.
pub struct PeerCertInfo {
    pub subject: Vec<CertNameEntry>,
    pub issuer: Vec<CertNameEntry>,
    /// "Aug 14 12:00:00 2026 GMT" (ASN1_TIME_print format — the format
    /// Node reports).
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// Uppercase SHA-256 digest, colon-separated hex pairs.
    pub fingerprint256: Option<String>,
    /// Uppercase hex serial number.
    pub serial_number: Option<String>,
}

/// NUL-terminated C string → String (None for NULL / invalid UTF-8).
fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ptr is a NUL-terminated C string owned by the caller's source.
    let bytes = unsafe { std::ffi::CStr::from_ptr(ptr) };
    bytes.to_str().ok().map(|s| s.to_string())
}

/// Extract the known RDN attributes from an `X509_NAME` (subject/issuer).
fn cert_name_entries(name: *const X509_NAME) -> Vec<CertNameEntry> {
    const NIDS: &[(c_int, &str)] = &[
        (NID_commonName, "CN"),
        (NID_COUNTRY_NAME, "C"),
        (NID_STATE_OR_PROVINCE_NAME, "ST"),
        (NID_LOCALITY_NAME, "L"),
        (NID_ORGANIZATION_NAME, "O"),
        (NID_ORGANIZATIONAL_UNIT_NAME, "OU"),
        (NID_PKCS9_EMAIL_ADDRESS, "emailAddress"),
    ];
    let mut out = Vec::new();
    for &(nid, key) in NIDS {
        let mut loc: c_int = -1;
        loop {
            // SAFETY: name is a live X509_NAME owned by the certificate
            // being parsed.
            loc = unsafe { X509_NAME_get_index_by_NID(name, nid, loc) };
            if loc < 0 {
                break;
            }
            // SAFETY: loc was just returned by the index lookup on name.
            let entry = unsafe { X509_NAME_get_entry(name, loc) };
            if entry.is_null() {
                break;
            }
            // SAFETY: entry is a live X509_NAME_ENTRY owned by name.
            let data = unsafe { X509_NAME_ENTRY_get_data(entry) };
            if data.is_null() {
                continue;
            }
            // SAFETY: data is a live ASN1_STRING owned by the entry; the
            // two accessors are valid reads for its lifetime.
            let ptr = unsafe { ASN1_STRING_get0_data(data) };
            let len = unsafe { ASN1_STRING_length(data) };
            if !ptr.is_null() && len > 0 {
                // SAFETY: ptr/len delimit the string's raw bytes.
                let bytes = unsafe { core::slice::from_raw_parts(ptr, len as usize) };
                out.push(CertNameEntry {
                    key,
                    value: String::from_utf8_lossy(bytes).into_owned(),
                });
            }
        }
    }
    out
}

/// Render an `ASN1_TIME` in the ASN1_TIME_print format (Node's
/// valid_from/valid_to format).
fn asn1_time_string(t: *const ASN1_TIME) -> Option<String> {
    if t.is_null() {
        return None;
    }
    // SAFETY: BIO_s_mem returns the process-static mem BIO method.
    let bio = unsafe { BIO_new(BIO_s_mem()) };
    if bio.is_null() {
        return None;
    }
    // SAFETY: bio is a live mem BIO; t is a live ASN1_TIME from the cert.
    if unsafe { ASN1_TIME_print(bio, t) } <= 0 {
        // SAFETY: bio was just created and is no longer needed.
        unsafe { BIO_free(bio) };
        return None;
    }
    // SAFETY: bio is the live mem BIO just written to.
    let pending = unsafe { BIO_ctrl_pending(bio) };
    let mut buf = vec![0u8; pending];
    // SAFETY: buf is a valid write buffer of `pending` bytes.
    let n = unsafe { BIO_read(bio, buf.as_mut_ptr().cast::<c_void>(), buf.len() as c_int) };
    // SAFETY: bio is done.
    unsafe { BIO_free(bio) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Uppercase colon-separated hex digest of a certificate (Node
/// fingerprint256 format).
fn cert_digest_hex(x509: *const X509, md: *const EVP_MD) -> Option<String> {
    let mut buf = vec![0u8; EVP_MAX_MD_SIZE as usize];
    let mut len: c_uint = 0;
    // SAFETY: x509 is a live certificate; buf is EVP_MAX_MD_SIZE bytes.
    if unsafe { X509_digest(x509, md, buf.as_mut_ptr(), &mut len) } != 1 {
        return None;
    }
    let digest = &buf[..len as usize];
    let mut out = String::with_capacity(digest.len() * 3);
    for (i, b) in digest.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{:02X}", b));
    }
    Some(out)
}

/// Uppercase hex serial number (Node serialNumber format).
fn cert_serial_hex(x509: *const X509) -> Option<String> {
    // SAFETY: serial is owned by the certificate (borrowed for this call).
    let serial = unsafe { X509_get_serialNumber(x509) };
    if serial.is_null() {
        return None;
    }
    // SAFETY: BN_new allocates a fresh BIGNUM (freed below).
    let bn = unsafe { BN_new() };
    if bn.is_null() {
        return None;
    }
    // SAFETY: serial is a live ASN1_INTEGER; bn is a fresh BIGNUM.
    let owned = unsafe { ASN1_INTEGER_to_BN(serial, bn) };
    if owned.is_null() {
        // SAFETY: bn was just allocated and conversion never took it.
        unsafe { BN_free(bn) };
        return None;
    }
    // SAFETY: owned is the BIGNUM holding the serial (bn or a new one).
    let hex = unsafe { BN_bn2hex(owned) };
    // SAFETY: owned was allocated by BN_new/ASN1_INTEGER_to_BN for us.
    unsafe { BN_free(owned) };
    if hex.is_null() {
        return None;
    }
    let s = cstr_to_string(hex).map(|s| s.to_uppercase());
    // SAFETY: hex is an OPENSSL_malloc'd string.
    unsafe { OPENSSL_free(hex.cast::<c_void>()) };
    s
}

/// The SNI servername for a raw `SSL*` (server side) — usable inside the
/// select-certificate callback. `TLSEXT_NAMETYPE_host_name` only.
pub fn ssl_servername(ssl: *const SSL) -> Option<String> {
    let name = unsafe { SSL_get_servername(ssl, TLSEXT_NAMETYPE_HOST_NAME) };
    if name.is_null() {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(name) };
    cstr.to_str().ok().map(|s| s.to_string())
}

// ─── Shared SSL_CTX prologue ──────────────────────────────────────────

/// Cipher list shared by client and server sides (same as Bun upstream).
const CIPHER_LIST: &core::ffi::CStr = c"TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305";

/// Create an `SSL_CTX` on the crypto-X509 method (`TLS_method`) with the
/// shared cipher list configured.
///
/// Both sides must use the X509 method: the trust-store / verify-param
/// APIs the client calls (`SSL_CTX_get_cert_store`, …) and the X509
/// certificate loaders the server calls (`SSL_CTX_use_certificate` /
/// `SSL_CTX_add1_chain_cert` / `SSL_CTX_use_PrivateKey`) all assert on a
/// buffers-method ctx (`check_ssl_ctx_x509_method`). Wire behavior is
/// identical to the buffers method; this only selects the certificate
/// representation inside BoringSSL.
pub(crate) fn new_tls_ctx() -> Result<*mut SSL_CTX, TlsError> {
    let ctx = unsafe { SSL_CTX_new(TLS_method()) };
    if ctx.is_null() {
        return Err(TlsError::BoringSSL("SSL_CTX_new failed"));
    }
    let ok = unsafe { SSL_CTX_set_cipher_list(ctx, CIPHER_LIST.as_ptr()) };
    if ok == 0 {
        unsafe { SSL_CTX_free(ctx) };
        return Err(TlsError::BoringSSL("SSL_CTX_set_cipher_list failed"));
    }
    Ok(ctx)
}

// ─── TlsConnection ───────────────────────────────────────────────────

/// Which side of the handshake this connection drives. The client and
/// server state machines share the entire I/O / handshake driver; the role
/// only gates the handshake error arms that can arise on one side (e.g.
/// pending certificate selection, which requires a server-side
/// select-certificate callback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Client,
    Server,
}

/// A TLS connection backed by BoringSSL.
///
/// Wraps an `SSL*` with BIO pairs for non-blocking I/O. One type serves
/// both sides; the role (client/server) is fixed at construction and only
/// varies the handshake setup and the certificate-selection error path.
pub struct TlsConnection {
    role: Role,
    ssl: *mut SSL,
    /// Peer-side BIO for feeding incoming ciphertext.
    network_read_bio: *mut BIO,
    /// Peer-side BIO for extracting outgoing ciphertext.
    network_write_bio: *mut BIO,
    handshake_done: bool,
    saw_peer_closed: bool,
}

impl core::fmt::Debug for TlsConnection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TlsConnection")
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// Result of create_bio_pairs.
struct BioPairResult {
    internal_rbio: *mut BIO,
    internal_wbio: *mut BIO,
    network_read_bio: *mut BIO,
    network_write_bio: *mut BIO,
}

fn create_bio_pairs_v2() -> Result<BioPairResult, TlsError> {
    let mut internal_rbio: *mut BIO = core::ptr::null_mut();
    let mut network_read_bio: *mut BIO = core::ptr::null_mut();
    let ok = unsafe { BIO_new_bio_pair(&mut internal_rbio, 0, &mut network_read_bio, 0) };
    if ok != 1 {
        return Err(TlsError::BoringSSL("BIO_new_bio_pair (read) failed"));
    }

    let mut internal_wbio: *mut BIO = core::ptr::null_mut();
    let mut network_write_bio: *mut BIO = core::ptr::null_mut();
    let ok = unsafe { BIO_new_bio_pair(&mut internal_wbio, 0, &mut network_write_bio, 0) };
    if ok != 1 {
        unsafe {
            BIO_free(internal_rbio);
            BIO_free(network_read_bio);
        }
        return Err(TlsError::BoringSSL("BIO_new_bio_pair (write) failed"));
    }

    Ok(BioPairResult {
        internal_rbio,
        internal_wbio,
        network_read_bio,
        network_write_bio,
    })
}

impl TlsConnection {
    /// Wire up the shared connection state: SSL object, BIO pairs, and the
    /// pending-plaintext bookkeeping. Callers then put the SSL into client
    /// or server state before returning the connection.
    fn new_raw(ctx: *mut SSL_CTX, role: Role) -> Result<Self, TlsError> {
        let ssl = unsafe { SSL_new(ctx) };
        if ssl.is_null() {
            return Err(TlsError::BoringSSL("SSL_new failed"));
        }

        let bios = create_bio_pairs_v2()?;
        unsafe { SSL_set_bio(ssl, bios.internal_rbio, bios.internal_wbio) };

        Ok(Self {
            role,
            ssl,
            network_read_bio: bios.network_read_bio,
            network_write_bio: bios.network_write_bio,
            handshake_done: false,
            saw_peer_closed: false,
        })
    }

    /// Create a new client-side TLS connection.
    pub fn new_client(tls_client: &TlsClient, hostname: &str) -> Result<Self, TlsError> {
        let conn = Self::new_raw(tls_client.ctx(), Role::Client)?;
        unsafe {
            SSL_set_connect_state(conn.ssl);

            let hostname_c = std::ffi::CString::new(hostname)
                .map_err(|_| TlsError::InvalidServerName(hostname.to_string()))?;
            SSL_set_tlsext_host_name(conn.ssl, hostname_c.as_ptr());

            let alpn = b"\x02h2\x08http/1.1";
            SSL_set_alpn_protos(conn.ssl, alpn.as_ptr(), alpn.len());
        }
        Ok(conn)
    }

    /// Create a new server-side TLS connection from BoringSSL TlsServer.
    pub fn new_server_boringssl(tls_server: &TlsServer) -> Result<Self, TlsError> {
        let conn = Self::new_raw(tls_server.ctx(), Role::Server)?;
        unsafe { SSL_set_accept_state(conn.ssl) };
        Ok(conn)
    }

    /// Whether the TLS handshake has not yet completed.
    pub fn is_handshaking(&self) -> bool {
        !self.handshake_done
    }

    /// Feed raw TLS bytes received from the network.
    pub fn feed(&mut self, data: &[u8]) {
        unsafe {
            BIO_write(
                self.network_read_bio,
                data.as_ptr() as *const c_void,
                data.len() as c_int,
            );
        }
    }

    /// Drive the TLS state machine.
    pub fn process(&mut self) -> Result<ProcessResult, TlsError> {
        let mut plaintext = Vec::new();
        let mut state = TlsState::Handshaking;

        if !self.handshake_done {
            let ret = unsafe { SSL_do_handshake(self.ssl) };
            if ret == 1 {
                self.handshake_done = true;
                state = TlsState::Active;
            } else {
                let err = unsafe { SSL_get_error(self.ssl, ret) };
                match err {
                    SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {}
                    SSL_ERROR_PENDING_CERTIFICATE if self.role == Role::Server => {
                        // Certificate selection (SNI) is pending: the
                        // select-certificate callback returned retry. The
                        // caller resolves the credential asynchronously and
                        // drives `process` again. (Client-side connections
                        // never register the select-certificate callback,
                        // so they fall through to the generic error path.)
                        state = TlsState::PendingCertificate;
                    }
                    SSL_ERROR_ZERO_RETURN => {
                        self.saw_peer_closed = true;
                        state = TlsState::PeerClosed;
                    }
                    SSL_ERROR_SSL => {
                        return Err(TlsError::BoringSSL("handshake failed (SSL_ERROR_SSL)"));
                    }
                    _ => {
                        return Err(TlsError::BoringSSL("handshake failed"));
                    }
                }
            }
        }

        if self.handshake_done {
            let mut buf = vec![0u8; TLS_RECORD_MAX];
            loop {
                let n = unsafe {
                    SSL_read(
                        self.ssl,
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len() as c_int,
                    )
                };
                if n > 0 {
                    plaintext.push(buf[..n as usize].to_vec());
                } else {
                    let err = unsafe { SSL_get_error(self.ssl, n) };
                    match err {
                        SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => break,
                        SSL_ERROR_ZERO_RETURN => {
                            self.saw_peer_closed = true;
                            state = TlsState::PeerClosed;
                            break;
                        }
                        _ => break,
                    }
                }
            }
            if !self.saw_peer_closed {
                state = TlsState::Active;
            }
        }

        let outgoing_bytes = unsafe { BIO_ctrl_pending(self.network_write_bio) };

        Ok(ProcessResult {
            plaintext,
            outgoing_bytes: outgoing_bytes as usize,
            state,
        })
    }

    /// Encrypt application data and queue it for sending.
    pub fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        if !self.handshake_done {
            return Err(TlsError::NotReady);
        }
        let n = unsafe {
            SSL_write(
                self.ssl,
                plaintext.as_ptr() as *const c_void,
                plaintext.len() as c_int,
            )
        };
        if n > 0 {
            Ok(n as usize)
        } else {
            let err = unsafe { SSL_get_error(self.ssl, n) };
            match err {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(TlsError::NotReady),
                _ => Err(TlsError::EncryptFailed),
            }
        }
    }

    /// Read decrypted application data (up to `buf.len()` bytes).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        if !self.handshake_done {
            return Err(TlsError::NotReady);
        }
        let n = unsafe {
            SSL_read(
                self.ssl,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as c_int,
            )
        };
        if n > 0 {
            Ok(n as usize)
        } else {
            let err = unsafe { SSL_get_error(self.ssl, n) };
            match err {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(TlsError::NotReady),
                SSL_ERROR_ZERO_RETURN => {
                    self.saw_peer_closed = true;
                    Err(TlsError::NotReady)
                }
                _ => Err(TlsError::BoringSSL("SSL_read failed")),
            }
        }
    }

    /// Take the outgoing ciphertext buffer for transmission.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        let bio = self.network_write_bio;
        let mut outgoing = Vec::new();
        let mut buf = [0u8; TLS_RECORD_MAX];
        loop {
            let n = unsafe { BIO_read(bio, buf.as_mut_ptr() as *mut c_void, buf.len() as c_int) };
            if n > 0 {
                outgoing.extend_from_slice(&buf[..n as usize]);
            } else {
                break;
            }
        }
        outgoing
    }

    /// Initiate a clean TLS shutdown.
    pub fn queue_close_notify(&mut self) -> Result<(), TlsError> {
        let ret = unsafe { SSL_shutdown(self.ssl) };
        if ret < 0 {
            let err = unsafe { SSL_get_error(self.ssl, ret) };
            match err {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Ok(()),
                _ => Err(TlsError::BoringSSL("SSL_shutdown failed")),
            }
        } else {
            if ret == 1 {
                self.saw_peer_closed = true;
            }
            Ok(())
        }
    }

    /// Whether the peer has closed their side.
    pub fn peer_closed(&self) -> bool {
        self.saw_peer_closed
    }

    /// ALPN protocol negotiated during handshake.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        let mut data: *const u8 = core::ptr::null();
        let mut len: u32 = 0;
        unsafe {
            SSL_get0_alpn_selected(self.ssl, &mut data, &mut len);
        }
        if data.is_null() || len == 0 {
            None
        } else {
            Some(unsafe { core::slice::from_raw_parts(data, len as usize) })
        }
    }

    /// The SNI servername the client sent (server side). Valid inside (and
    /// after) the select-certificate callback.
    pub fn servername(&self) -> Option<String> {
        ssl_servername(self.ssl)
    }

    /// Switch this connection's certificate configuration to `ctx` (the
    /// canonical SNI pattern). Must be called from inside the
    /// select-certificate callback (or while the handshake is paused from
    /// it) — see `SSL_set_SSL_CTX` in vendor/boringssl/include/openssl/ssl.h.
    ///
    /// # Safety
    ///
    /// `ctx` must be a live `SSL_CTX*` from a `TlsServer` (same method /
    /// x509_method). The caller keeps `ctx` alive for the connection's
    /// lifetime (SSL_set_SSL_CTX up-refs it internally, but the caller's
    /// original reference must not be released before the SSL is done if
    /// it is the only other one).
    pub unsafe fn switch_ssl_ctx(&mut self, ctx: *mut SSL_CTX) -> bool {
        let new_ctx = unsafe { SSL_set_SSL_CTX(self.ssl, ctx) };
        !new_ctx.is_null()
    }

    /// The peer's leaf certificate as DER bytes (after handshake).
    pub fn peer_certificate_der(&self) -> Option<Vec<u8>> {
        let x509 = unsafe { SSL_get_peer_certificate(self.ssl) };
        if x509.is_null() {
            return None;
        }
        let len = unsafe { i2d_X509(x509, core::ptr::null_mut()) };
        if len <= 0 {
            unsafe { X509_free(x509) };
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let mut p = buf.as_mut_ptr();
        unsafe {
            i2d_X509(x509, &mut p);
            X509_free(x509);
        }
        Some(buf)
    }

    /// Negotiated protocol version ("TLSv1.3", …) — available once the
    /// handshake has completed.
    pub fn protocol_version(&self) -> Option<String> {
        let ssl = self.ssl_ptr();
        // SAFETY: ssl is a live SSL owned by this connection.
        cstr_to_string(unsafe { SSL_get_version(ssl) })
    }

    /// Negotiated cipher name (e.g. "TLS_AES_256_GCM_SHA384") — available
    /// once the handshake has completed.
    pub fn cipher_name(&self) -> Option<String> {
        let ssl = self.ssl_ptr();
        // SAFETY: ssl is a live SSL owned by this connection.
        let cipher = unsafe { SSL_get_current_cipher(ssl) };
        if cipher.is_null() {
            return None;
        }
        // SAFETY: cipher was returned by SSL_get_current_cipher for this SSL.
        cstr_to_string(unsafe { SSL_CIPHER_get_name(cipher) })
    }

    /// The cipher's version string (BoringSSL's static "TLSv1/SSLv3").
    pub fn cipher_version(&self) -> Option<String> {
        let ssl = self.ssl_ptr();
        // SAFETY: ssl is a live SSL owned by this connection.
        let cipher = unsafe { SSL_get_current_cipher(ssl) };
        if cipher.is_null() {
            return None;
        }
        // SAFETY: cipher was returned by SSL_get_current_cipher for this SSL.
        cstr_to_string(unsafe { SSL_CIPHER_get_version(cipher) })
    }

    /// Disable peer-certificate verification (Node `rejectUnauthorized:
    /// false`). Must be called before the handshake starts.
    pub fn set_verify_off(&mut self) {
        // SAFETY: ssl is a live SSL owned by this connection, pre-handshake.
        unsafe { SSL_set_verify(self.ssl_ptr(), SSL_VERIFY_NONE, None) };
    }

    /// Enable peer-certificate verification against `hostname` (Node
    /// `rejectUnauthorized: true`, the `tls.connect` default). A BoringSSL
    /// client does NOT verify unless this is set — `SSL_VERIFY_PEER` turns
    /// on chain validation against the ctx trust store and `SSL_set1_host`
    /// adds the DNS-name check. Returns false when the hostname could not
    /// be installed (interior NUL) — callers must treat that as failure,
    /// not silently proceed unverified. Must be called before the handshake
    /// starts.
    pub fn set_verify_peer(&mut self, hostname: &str) -> bool {
        let Ok(name_c) = std::ffi::CString::new(hostname) else {
            return false;
        };
        // SAFETY: ssl is a live SSL owned by this connection, pre-handshake;
        // name_c outlives both calls.
        unsafe {
            SSL_set_verify(self.ssl_ptr(), SSL_VERIFY_PEER, None);
            SSL_set1_host(self.ssl_ptr(), name_c.as_ptr()) == 1
        }
    }

    /// The peer's leaf certificate parsed into the Node
    /// `getPeerCertificate()` field set (None when the peer presented no
    /// certificate). Only fields BoringSSL can hand back truthfully are
    /// filled; absent ones are `None` so the caller surfaces `undefined`
    /// instead of a placeholder.
    pub fn peer_cert_info(&self) -> Option<PeerCertInfo> {
        let ssl = self.ssl_ptr();
        // SAFETY: ssl is a live SSL owned by this connection; the returned
        // X509 is owned by us and freed below.
        let x509 = unsafe { SSL_get_peer_certificate(ssl) };
        if x509.is_null() {
            return None;
        }
        let info = PeerCertInfo {
            // SAFETY: x509 is a live certificate; both name accessors return
            // names owned by it (const access for the lifetime of this block).
            subject: cert_name_entries(unsafe { X509_get_subject_name(x509) }),
            issuer: cert_name_entries(unsafe { X509_get_issuer_name(x509) }),
            valid_from: asn1_time_string(unsafe { X509_get_notBefore(x509) }),
            valid_to: asn1_time_string(unsafe { X509_get_notAfter(x509) }),
            fingerprint256: cert_digest_hex(x509, EVP_sha256()),
            serial_number: cert_serial_hex(x509),
        };
        // SAFETY: x509 was returned by SSL_get_peer_certificate (owned).
        unsafe { X509_free(x509) };
        Some(info)
    }

    /// Set the curves list on the SSL connection (for profile-specific ordering).
    pub fn set_curves_list(&mut self, curves: *const i8) -> c_int {
        unsafe { SSL_set1_curves_list(self.ssl, curves) }
    }

    /// Get the raw SSL pointer (for advanced use).
    pub fn ssl_ptr(&self) -> *mut SSL {
        self.ssl
    }
}

// ─── Drop ────────────────────────────────────────────────────────────

impl Drop for TlsConnection {
    fn drop(&mut self) {
        // SSL_free frees the internal BIOs set via SSL_set_bio.
        // We only need to free the network-side (peer) BIOs.
        unsafe {
            SSL_free(self.ssl);
            BIO_free(self.network_read_bio);
            BIO_free(self.network_write_bio);
        }
    }
}

unsafe impl Send for TlsConnection {}

// ─── ProcessResult ───────────────────────────────────────────────────

/// Result of driving the TLS state machine.
#[derive(Debug)]
pub struct ProcessResult {
    /// Decrypted application data records.
    pub plaintext: Vec<Vec<u8>>,
    /// Number of outgoing ciphertext bytes ready to send.
    pub outgoing_bytes: usize,
    /// The TLS connection state after processing.
    pub state: TlsState,
}

/// Summarized TLS connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsState {
    /// Handshake in progress.
    Handshaking,
    /// Handshake paused: certificate selection (SNI) is pending. The
    /// select-certificate callback returned `ssl_select_cert_retry`; the
    /// caller must resolve the credential out-of-band and drive `process`
    /// again.
    PendingCertificate,
    /// Handshake complete, ready for application data.
    Active,
    /// Peer sent close_notify.
    PeerClosed,
    /// Both sides closed.
    Closed,
}

// ─── TlsError ────────────────────────────────────────────────────────

/// Errors that can occur during TLS operations.
#[derive(Debug)]
pub enum TlsError {
    /// BoringSSL returned an error.
    BoringSSL(&'static str),
    /// Connection not ready for application data.
    NotReady,
    /// TLS encryption failed.
    EncryptFailed,
    /// TLS encoding failed.
    EncodeFailed,
    /// Invalid server name.
    InvalidServerName(String),
    /// Invalid certificate/key.
    InvalidCertKey(String),
}

impl core::fmt::Display for TlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BoringSSL(msg) => write!(f, "BoringSSL error: {msg}"),
            Self::NotReady => write!(f, "connection not ready for application data"),
            Self::EncryptFailed => write!(f, "TLS encryption failed"),
            Self::EncodeFailed => write!(f, "TLS encoding failed"),
            Self::InvalidServerName(name) => write!(f, "invalid server name: {name}"),
            Self::InvalidCertKey(msg) => write!(f, "invalid certificate/key: {msg}"),
        }
    }
}

impl std::error::Error for TlsError {}
