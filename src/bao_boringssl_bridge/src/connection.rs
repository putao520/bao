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

use std::ffi::{c_int, c_void};

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

unsafe extern "C" {
    pub(crate) fn SSL_CTX_set_select_certificate_cb(
        ctx: *mut SSL_CTX,
        cb: Option<unsafe extern "C" fn(client_hello: *const SslClientHello) -> c_int>,
    );
    fn SSL_set_SSL_CTX(ssl: *mut SSL, ctx: *mut SSL_CTX) -> *mut SSL_CTX;
    fn SSL_get_peer_certificate(ssl: *const SSL) -> *mut X509;
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

// ─── TlsConnection ───────────────────────────────────────────────────

/// A TLS connection backed by BoringSSL.
///
/// Wraps an `SSL*` with BIO pairs for non-blocking I/O.
pub enum TlsConnection {
    Client(ClientConn),
    Server(ServerConn),
}

impl core::fmt::Debug for TlsConnection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Client(_) => f
                .debug_struct("TlsConnection::Client")
                .finish_non_exhaustive(),
            Self::Server(_) => f
                .debug_struct("TlsConnection::Server")
                .finish_non_exhaustive(),
        }
    }
}

/// Internal state for a client connection.
pub struct ClientConn {
    ssl: *mut SSL,
    /// Peer-side BIO for feeding incoming ciphertext.
    network_read_bio: *mut BIO,
    /// Peer-side BIO for extracting outgoing ciphertext.
    network_write_bio: *mut BIO,
    handshake_done: bool,
    saw_peer_closed: bool,
}

/// Internal state for a server connection.
pub struct ServerConn {
    ssl: *mut SSL,
    network_read_bio: *mut BIO,
    network_write_bio: *mut BIO,
    handshake_done: bool,
    saw_peer_closed: bool,
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
    /// Create a new client-side TLS connection.
    pub fn new_client(tls_client: &TlsClient, hostname: &str) -> Result<Self, TlsError> {
        let ssl = unsafe { SSL_new(tls_client.ctx()) };
        if ssl.is_null() {
            return Err(TlsError::BoringSSL("SSL_new failed"));
        }

        let bios = create_bio_pairs_v2()?;
        unsafe {
            SSL_set_bio(ssl, bios.internal_rbio, bios.internal_wbio);
            SSL_set_connect_state(ssl);

            let hostname_c = std::ffi::CString::new(hostname)
                .map_err(|_| TlsError::InvalidServerName(hostname.to_string()))?;
            SSL_set_tlsext_host_name(ssl, hostname_c.as_ptr());

            let alpn = b"\x02h2\x08http/1.1";
            SSL_set_alpn_protos(ssl, alpn.as_ptr(), alpn.len());
        }

        Ok(Self::Client(ClientConn {
            ssl,
            network_read_bio: bios.network_read_bio,
            network_write_bio: bios.network_write_bio,
            handshake_done: false,
            saw_peer_closed: false,
        }))
    }

    /// Create a new server-side TLS connection from BoringSSL TlsServer.
    pub fn new_server_boringssl(tls_server: &TlsServer) -> Result<Self, TlsError> {
        let ssl = unsafe { SSL_new(tls_server.ctx()) };
        if ssl.is_null() {
            return Err(TlsError::BoringSSL("SSL_new failed"));
        }

        let bios = create_bio_pairs_v2()?;
        unsafe {
            SSL_set_bio(ssl, bios.internal_rbio, bios.internal_wbio);
            SSL_set_accept_state(ssl);
        }

        Ok(Self::Server(ServerConn {
            ssl,
            network_read_bio: bios.network_read_bio,
            network_write_bio: bios.network_write_bio,
            handshake_done: false,
            saw_peer_closed: false,
        }))
    }

    /// Whether the TLS handshake has not yet completed.
    pub fn is_handshaking(&self) -> bool {
        match self {
            Self::Client(c) => !c.handshake_done,
            Self::Server(c) => !c.handshake_done,
        }
    }

    /// Feed raw TLS bytes received from the network.
    pub fn feed(&mut self, data: &[u8]) {
        let bio = match self {
            Self::Client(c) => c.network_read_bio,
            Self::Server(c) => c.network_read_bio,
        };
        unsafe {
            BIO_write(bio, data.as_ptr() as *const c_void, data.len() as c_int);
        }
    }

    /// Drive the TLS state machine.
    pub fn process(&mut self) -> Result<ProcessResult, TlsError> {
        match self {
            Self::Client(c) => c.process(),
            Self::Server(c) => c.process(),
        }
    }

    /// Encrypt application data and queue it for sending.
    pub fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        match self {
            Self::Client(c) => c.write(plaintext),
            Self::Server(c) => c.write(plaintext),
        }
    }

    /// Read decrypted application data (up to `buf.len()` bytes).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        match self {
            Self::Client(c) => c.read(buf),
            Self::Server(c) => c.read(buf),
        }
    }

    /// Take the outgoing ciphertext buffer for transmission.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        let bio = match self {
            Self::Client(c) => c.network_write_bio,
            Self::Server(c) => c.network_write_bio,
        };
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
        let (ssl, saw_peer_closed) = match self {
            Self::Client(c) => (c.ssl, &mut c.saw_peer_closed),
            Self::Server(c) => (c.ssl, &mut c.saw_peer_closed),
        };
        let ret = unsafe { SSL_shutdown(ssl) };
        if ret < 0 {
            let err = unsafe { SSL_get_error(ssl, ret) };
            match err {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Ok(()),
                _ => Err(TlsError::BoringSSL("SSL_shutdown failed")),
            }
        } else {
            if ret == 1 {
                *saw_peer_closed = true;
            }
            Ok(())
        }
    }

    /// Whether the peer has closed their side.
    pub fn peer_closed(&self) -> bool {
        match self {
            Self::Client(c) => c.saw_peer_closed,
            Self::Server(c) => c.saw_peer_closed,
        }
    }

    /// ALPN protocol negotiated during handshake.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        let ssl = match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        };
        let mut data: *const u8 = core::ptr::null();
        let mut len: u32 = 0;
        unsafe {
            SSL_get0_alpn_selected(ssl, &mut data, &mut len);
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
        let ssl = match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        };
        ssl_servername(ssl)
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
        let ssl = match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        };
        let new_ctx = unsafe { SSL_set_SSL_CTX(ssl, ctx) };
        !new_ctx.is_null()
    }

    /// The peer's leaf certificate as DER bytes (after handshake).
    pub fn peer_certificate_der(&self) -> Option<Vec<u8>> {
        let ssl = match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        };
        let x509 = unsafe { SSL_get_peer_certificate(ssl) };
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

    /// Set the curves list on the SSL connection (for profile-specific ordering).
    pub fn set_curves_list(&mut self, curves: *const i8) -> c_int {
        let ssl = match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        };
        unsafe { SSL_set1_curves_list(ssl, curves) }
    }

    /// Get the raw SSL pointer (for advanced use).
    pub fn ssl_ptr(&self) -> *mut SSL {
        match self {
            Self::Client(c) => c.ssl,
            Self::Server(c) => c.ssl,
        }
    }
}

// ─── ClientConn ──────────────────────────────────────────────────────

impl ClientConn {
    fn process(&mut self) -> Result<ProcessResult, TlsError> {
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

    fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
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

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
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
}

// ─── ServerConn ──────────────────────────────────────────────────────

impl ServerConn {
    fn process(&mut self) -> Result<ProcessResult, TlsError> {
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
                    SSL_ERROR_PENDING_CERTIFICATE => {
                        // Certificate selection (SNI) is pending: the
                        // select-certificate callback returned retry. The
                        // caller resolves the credential asynchronously and
                        // drives `process` again.
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

    fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
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

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
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
}

// ─── Drop ────────────────────────────────────────────────────────────

impl Drop for TlsConnection {
    fn drop(&mut self) {
        // SSL_free frees the internal BIOs set via SSL_set_bio.
        // We only need to free the network-side (peer) BIOs.
        let (ssl, network_read_bio, network_write_bio) = match self {
            Self::Client(c) => (c.ssl, c.network_read_bio, c.network_write_bio),
            Self::Server(c) => (c.ssl, c.network_read_bio, c.network_write_bio),
        };
        unsafe {
            SSL_free(ssl);
            BIO_free(network_read_bio);
            BIO_free(network_write_bio);
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
