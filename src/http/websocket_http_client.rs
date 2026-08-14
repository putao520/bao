// @trace REQ-STL-001 [api:WebSocket] — page WebSocket TLS on the bao stack
// (BoringSSL via bao_boringssl_bridge), removing servo-net's WS dependence on
// the hyper-ecosystem connector (`create_tls_config` machinery /
// `BoringsslTlsStream`). Ported from src/http/websocket_http_client.zig
// (upstream Bun: WebSocketHTTPClient / WebSocketClient split — here the HTTP
// upgrade and the WS client are one connector over the bao TLS stack).
//
// Two layers, one connector module:
//
// 1. `WsTlsStream` — async TLS stream (tokio AsyncRead/AsyncWrite) over a
//    `bao_boringssl_bridge::TlsConnection`, applying the stealth
//    per-connection fingerprint (sigalgs / ALPN / curves) and offering the
//    process-wide TLS session for the origin before the ClientHello is
//    serialized. Consumed by servo's `websocket_loader` through
//    `async_tungstenite::tokio::TokioAdapter` — the RFC 6455 protocol layer
//    and the DOM event plumbing stay servo-side (tungstenite); only the
//    TCP+TLS segment moved onto the bao stack. The poll-based stream needs
//    NO thread bridge: it drives the (memory-BIO) TlsConnection state
//    machine directly inside tokio's read/write readiness model.
//
// 2. `WsHttpClient` — a complete blocking WebSocket client (TCP + TLS +
//    RFC 6455 handshake + masked client framing), reusing `bun_uws`'s
//    codec/handshake primitives (`ws_codec` / `ws_handshake`). Ported from
//    the `WsConn` pattern in `bao_runtime::web_api` (which stays in place
//    until its domain reopens; both consume the same bun_uws primitives,
//    the same stealth application entry and the same session-cache salt
//    semantics and must not drift).
//
// Session-cache salting is stack-specific BY DESIGN — each stack shares
// sessions with its own fetch path and never across parameter sets
// (offering a session short-circuits parameter negotiation, which would
// corrupt the advertised TLS fingerprint):
//   - servo stack (`WsTlsStream`): `stealth_pc_salt` — a byte-identical
//     twin of servo connector.rs's private fn, so a wss connection resumes
//     a session cached by a servo fetch connection with the same parameter
//     set (same key scheme = "wss 同键").
//   - bun stack (`WsHttpClient`): `SSLConfig::content_hash()` / 0 — the
//     same salting as `WsConn` and the bun_http fetch `attach()` path.

use std::ffi::CString;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::pin::Pin;
use std::task::{Context, Poll};

use bao_boringssl_bridge::connection::{TlsConnection, TlsError, TlsState};
use bao_boringssl_bridge::{TlsClient, session_cache};

/// Ciphertext chunk size for TCP reads feeding the TLS engine.
const BUF_SIZE: usize = 16_384;

// ─────────────────────────────────────────────────────────────────────────
// Layer 1: async TLS stream for servo's websocket_loader
// ─────────────────────────────────────────────────────────────────────────

/// Per-connection TLS settings for [`WsTlsStream`], carried as explicit
/// data so this crate stays free of servo-net types (the servo caller
/// adapts its connector `TlsConfig`/`StealthPerConnection` into this
/// shape). Mirrors the fields servo's fetch connector applies per
/// connection because BoringSSL only exposes `SSL_set_*` variants for
/// them (no `SSL_CTX_set_*`).
#[derive(Clone, Debug, Default)]
pub struct WsTlsOptions {
    /// Signature algorithms as OpenSSL name strings (e.g.
    /// `"rsa_pss_rsae_sha256:rsa_pkcs1_sha256"`).
    pub sigalg_list: Option<String>,
    /// ALPN protocols in wire format (length-prefixed). `None` keeps the
    /// `TlsConnection::new_client` default (`h2` + `http/1.1`); the servo
    /// WS caller always passes `http/1.1` only — what a browser offers on
    /// a WebSocket TLS connection.
    pub alpn_wire: Option<Vec<u8>>,
    /// Supported groups as OpenSSL name strings (e.g. `"X25519:P-256:P-384"`).
    pub curves_list: Option<String>,
    /// Servo's `ignore_certificate_errors` (WPT): explicitly disable peer
    /// verification. NOTE parity with the servo fetch connector: neither
    /// path opts INTO verification — BoringSSL clients run
    /// `SSL_VERIFY_NONE` unless `set_verify_peer` is called, and the servo
    /// connector never calls it. This connector matches that posture
    /// instead of silently diverging; flipping the whole servo stack to
    /// verified TLS belongs to the U2 unification, not a WS rewiring.
    pub ignore_certificate_errors: bool,
}

/// Stable in-process discriminator for a [`WsTlsOptions`] parameter set,
/// used to salt the TLS session-resumption origin key.
///
/// Byte-identical twin of servo connector.rs's private `stealth_pc_salt`:
/// connector hashes `Option<String>` / `Option<Vec<u8>>` fields, this fn
/// hashes `Option<&str>` / `Option<&[u8]>` — `Hash for String` delegates to
/// `str` and `Hash for Vec<u8>` to `[u8]`, so the derived values are
/// identical, which is what lets a wss connection offer a session cached
/// by a servo fetch connection under the same parameter set. `DefaultHasher`
/// values are not stable across compiler versions; the salt only needs to
/// be consistent within one process. The two twins must stay in sync.
pub fn stealth_pc_salt(
    sigalg_list: Option<&str>,
    alpn_wire: Option<&[u8]>,
    curves_list: Option<&str>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sigalg_list.hash(&mut h);
    alpn_wire.hash(&mut h);
    curves_list.hash(&mut h);
    h.finish()
}

/// Async TLS stream over the bao BoringSSL stack: a tokio `TcpStream` plus
/// a memory-BIO `TlsConnection`, exposing tokio `AsyncRead`/`AsyncWrite`
/// so `async_tungstenite::tokio::TokioAdapter` can run the RFC 6455
/// handshake and framing on top (hyper-free replacement for servo
/// connector's `BoringsslTlsStream`).
///
/// Thread contract: `Send` (via `TlsConnection`'s `unsafe impl Send`) —
/// lives on the tokio task spawned by servo's `run_ws_loop`; the SSL
/// object is created and driven on that same task, never shared.
pub struct WsTlsStream {
    tcp: tokio::net::TcpStream,
    tls: TlsConnection,
    /// Buffered outgoing TLS ciphertext that hasn't been written to TCP yet.
    outgoing: Vec<u8>,
}

impl WsTlsStream {
    /// Build the TLS connection on an already-connected TCP stream.
    ///
    /// Stealth per-connection settings, verification posture and the
    /// session-cache offer are all applied HERE — before the first
    /// `process()` call — so they land in the ClientHello (same
    /// precondition as the servo connector and `WsConn`).
    pub fn new(
        tcp: tokio::net::TcpStream,
        tls_client: &TlsClient,
        host: &str,
        port: u16,
        opts: &WsTlsOptions,
    ) -> Result<Self, TlsError> {
        let mut tls = TlsConnection::new_client(tls_client, host)?;
        let ssl = tls.ssl_ptr();

        // The SSL_set_* failures below are hard errors, not warnings: an
        // unapplied fingerprint parameter would silently degrade the
        // stealth TLS fingerprint this product exists to control. (The
        // servo connector's fetch path only warns there; the name strings
        // themselves are sanitized by bao_stealth's shared builder, so a
        // failure here is abnormal, not a data-quality event.)
        unsafe {
            if let Some(ref sigalg_str) = opts.sigalg_list {
                let sigalg_c = CString::new(sigalg_str.as_str()).map_err(|_| TlsError::BoringSSL("interior NUL in WS TLS sigalg list"))?;
                if bun_boringssl::c::SSL_set1_sigalgs_list(ssl, sigalg_c.as_ptr()) == 0 {
                    return Err(TlsError::BoringSSL("WS TLS: SSL_set1_sigalgs_list failed"));
                }
            }

            if let Some(ref alpn_wire) = opts.alpn_wire {
                if bun_boringssl::c::SSL_set_alpn_protos(ssl, alpn_wire.as_ptr(), alpn_wire.len())
                    != 0
                {
                    return Err(TlsError::BoringSSL("WS TLS: SSL_set_alpn_protos failed"));
                }
            }
        }

        if let Some(ref curves_str) = opts.curves_list {
            let curves_c = CString::new(curves_str.as_str()).map_err(|_| TlsError::BoringSSL("interior NUL in WS TLS curves list"))?;
            // Bridge method (safe; curves_c outlives the call).
            if tls.set_curves_list(curves_c.as_ptr()) == 0 {
                return Err(TlsError::BoringSSL("WS TLS: SSL_set1_curves_list failed"));
            }
        }

        if opts.ignore_certificate_errors {
            tls.set_verify_off();
        }

        // TLS session resumption ("wss 同键"): offer the cached session for
        // this origin before the handshake starts. The profile salt
        // segregates parameter sets — offering a session short-circuits
        // parameter negotiation, so connections under different stealth
        // parameters never resume each other's sessions.
        let profile_salt = stealth_pc_salt(
            opts.sigalg_list.as_deref(),
            opts.alpn_wire.as_deref(),
            opts.curves_list.as_deref(),
        );
        session_cache::offer_session(ssl, host, port, profile_salt);

        Ok(Self {
            tcp,
            tls,
            outgoing: Vec::new(),
        })
    }

    /// Drive the TLS handshake to completion.
    ///
    /// Reads from TCP, feeds into TLS, processes, writes outgoing to TCP.
    /// Returns Ok(()) when the handshake is complete, Err if it failed.
    /// BCE-20260814-WS-TLS ordering (shared with the WsConn path): every
    /// flight `process()` produces is flushed to TCP BEFORE blocking on a
    /// read, so the ClientHello is never stranded in the write BIO while
    /// both sides wait.
    pub async fn handshake(&mut self) -> io::Result<()> {
        loop {
            match self.tls.process() {
                Ok(result) => {
                    self.flush_outgoing().await?;
                    match result.state {
                        TlsState::Active => return Ok(()),
                        TlsState::PeerClosed | TlsState::Closed => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "TLS peer closed during handshake",
                            ));
                        },
                        TlsState::Handshaking => {
                            self.read_from_tcp().await?;
                        },
                        // SNI-driven certificate selection parked the
                        // handshake; no select-certificate callback exists
                        // on the client path, so this is structurally
                        // unreachable — fail closed rather than wait.
                        TlsState::PendingCertificate => {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "TLS certificate selection pending without a resolver",
                            ));
                        },
                    }
                },
                Err(TlsError::BoringSSL(msg)) => {
                    return Err(io::Error::new(io::ErrorKind::Other, msg));
                },
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
                },
            }
        }
    }

    /// Read ciphertext from TCP and feed it into the TLS engine.
    async fn read_from_tcp(&mut self) -> io::Result<()> {
        let mut buf = [0u8; BUF_SIZE];
        self.tcp.readable().await?;
        match self.tcp.try_read(&mut buf) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "TCP connection closed",
                ));
            },
            Ok(n) => {
                self.tls.feed(&buf[..n]);
            },
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {},
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Write buffered outgoing TLS ciphertext to the TCP stream.
    async fn flush_outgoing(&mut self) -> io::Result<()> {
        if self.outgoing.is_empty() {
            self.outgoing = self.tls.take_outgoing();
        }
        while !self.outgoing.is_empty() {
            self.tcp.writable().await?;
            match self.tcp.try_write(&self.outgoing) {
                Ok(n) => {
                    self.outgoing = self.outgoing[n..].to_vec();
                },
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                },
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Negotiated ALPN protocol (e.g. `b"http/1.1"`), once handshaken.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.tls.alpn_protocol()
    }
}


impl tokio::io::AsyncRead for WsTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // Try to read plaintext from the TLS engine
        loop {
            match this.tls.process() {
                Ok(result) => {
                    if result.outgoing_bytes > 0 {
                        let outgoing = this.tls.take_outgoing();
                        this.outgoing.extend_from_slice(&outgoing);
                    }

                    if !result.plaintext.is_empty() {
                        for chunk in &result.plaintext {
                            let remaining = buf.remaining();
                            let to_copy = remaining.min(chunk.len());
                            if to_copy > 0 {
                                buf.put_slice(&chunk[..to_copy]);
                            }
                        }
                        return Poll::Ready(Ok(()));
                    }

                    match result.state {
                        TlsState::Active | TlsState::Handshaking => {
                            // No plaintext available, need more ciphertext
                            // from TCP.
                            break;
                        },
                        TlsState::PeerClosed | TlsState::Closed => {
                            return Poll::Ready(Ok(())); // EOF
                        },
                        // Structurally unreachable on the client path (no
                        // select-certificate callback) — fail closed.
                        TlsState::PendingCertificate => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::Other,
                                "TLS certificate selection pending without a resolver",
                            )));
                        },
                    }
                },
                Err(TlsError::NotReady) => break,
                Err(TlsError::BoringSSL(msg)) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, msg)));
                },
                Err(e) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::Other,
                        e.to_string(),
                    )));
                },
            }
        }

        // Need more data from TCP — read from TCP and feed into TLS
        let mut tcp_buf = [0u8; BUF_SIZE];
        let mut read_buf = tokio::io::ReadBuf::new(&mut tcp_buf);
        match Pin::new(&mut this.tcp).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    // TCP EOF
                    return Poll::Ready(Ok(()));
                }
                this.tls.feed(read_buf.filled());
                match this.tls.process() {
                    Ok(result) => {
                        if result.outgoing_bytes > 0 {
                            let outgoing = this.tls.take_outgoing();
                            this.outgoing.extend_from_slice(&outgoing);
                        }

                        if !result.plaintext.is_empty() {
                            for chunk in &result.plaintext {
                                let remaining = buf.remaining();
                                let to_copy = remaining.min(chunk.len());
                                if to_copy > 0 {
                                    buf.put_slice(&chunk[..to_copy]);
                                }
                            }
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Pending
                    },
                    Err(TlsError::NotReady) => Poll::Pending,
                    Err(e) => Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::Other,
                        e.to_string(),
                    ))),
                }
            },
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl tokio::io::AsyncWrite for WsTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.get_mut();

        // Encrypt the plaintext via TLS
        match this.tls.write(buf) {
            Ok(n) => {
                let outgoing = this.tls.take_outgoing();
                this.outgoing.extend_from_slice(&outgoing);

                // Try to write outgoing to TCP
                while !this.outgoing.is_empty() {
                    match Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                        Poll::Ready(Ok(0)) => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "TCP write returned zero",
                            )));
                        },
                        Poll::Ready(Ok(written)) => {
                            this.outgoing = this.outgoing[written..].to_vec();
                        },
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                Poll::Ready(Ok(n))
            },
            Err(TlsError::NotReady) => Poll::Pending,
            Err(e) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string()))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = self.get_mut();

        // Flush any remaining outgoing data to TCP
        while !this.outgoing.is_empty() {
            match Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "TCP write returned zero during flush",
                    )));
                },
                Poll::Ready(Ok(written)) => {
                    this.outgoing = this.outgoing[written..].to_vec();
                },
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        Pin::new(&mut this.tcp).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let this = self.get_mut();

        // Send TLS close_notify
        let _ = this.tls.queue_close_notify();
        let outgoing = this.tls.take_outgoing();
        if !outgoing.is_empty() {
            this.outgoing.extend_from_slice(&outgoing);
        }

        // Flush remaining outgoing
        while !this.outgoing.is_empty() {
            match Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                Poll::Ready(Ok(0)) => break,
                Poll::Ready(Ok(written)) => {
                    this.outgoing = this.outgoing[written..].to_vec();
                },
                Poll::Ready(Err(_)) => break,
                Poll::Pending => return Poll::Pending,
            }
        }

        Pin::new(&mut this.tcp).poll_shutdown(cx)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Layer 2: blocking full-stack RFC 6455 client (WsConn pattern)
// ─────────────────────────────────────────────────────────────────────────

/// A TLS-over-TCP adapter implementing `std::io::{Read, Write}` — the same
/// shape as `WsConn`'s adapter in `bao_runtime::web_api`, so
/// `bun_uws::ws_handshake` and `ws_codec` consume it directly and the wss
/// path reuses the exact same RFC 6455 code as the ws:// path. Works for
/// both client and server `TlsConnection` roles (`drive_handshake` just
/// pumps the state machine), which the loopback tests rely on.
pub struct TlsIoStream {
    tcp: TcpStream,
    tls: TlsConnection,
    /// Decrypted plaintext not yet handed to the reader. `Read` callers
    /// (the WS handshake reads byte-at-a-time) may take less than one TLS
    /// record per read(); the surplus must survive across calls
    /// (BCE-20260814-WS-TLS: dropping it corrupted the handshake).
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl TlsIoStream {
    /// Wrap an accepted/connected TCP socket with an (unhandshaken) TLS
    /// connection.
    pub fn new(tcp: TcpStream, tls: TlsConnection) -> Self {
        Self {
            tcp,
            tls,
            pending_plain: Vec::new(),
            pending_off: 0,
        }
    }

    /// Pump the TLS state machine: flush any pending outgoing ciphertext
    /// to the socket, then process inbound records until the TLS layer has
    /// decrypted data ready (or the socket would block). Returns the
    /// decrypted plaintext bytes.
    fn pump_inbound(&mut self) -> io::Result<Vec<u8>> {
        loop {
            // Drain any ciphertext BoringSSL wants to send first so a
            // mid-handshake flight isn't stranded in the write BIO.
            self.flush_outgoing()?;
            let res = self
                .tls
                .process()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(joined);
            }
            // No decrypted data yet — read more ciphertext from the socket.
            let mut buf = [0u8; BUF_SIZE];
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ));
                },
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                },
                Err(e) => return Err(e),
            }
        }
    }

    /// Write any pending ciphertext from BoringSSL's write BIO to the socket.
    fn flush_outgoing(&mut self) -> io::Result<()> {
        let outgoing = self.tls.take_outgoing();
        if outgoing.is_empty() {
            return Ok(());
        }
        self.tcp.write_all(&outgoing)
    }

    /// Drive the (blocking) TLS handshake to Active. The flight produced
    /// by `process()` is flushed BEFORE blocking on read
    /// (BCE-20260814-WS-TLS) — the reverse order strands the ClientHello
    /// while waiting for a ServerHello that can never arrive.
    pub fn drive_handshake(&mut self) -> io::Result<()> {
        loop {
            let res = self
                .tls
                .process()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            loop {
                let outgoing = self.tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                self.tcp.write_all(&outgoing)?;
            }
            match res.state {
                TlsState::Active | TlsState::PeerClosed => return Ok(()),
                TlsState::Handshaking => {
                    let mut buf = [0u8; BUF_SIZE];
                    match self.tcp.read(&mut buf) {
                        Ok(n) if n > 0 => self.tls.feed(&buf[..n]),
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "tls handshake stalled",
                            ));
                        },
                    }
                },
                TlsState::Closed => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "tls closed during handshake",
                    ));
                },
                TlsState::PendingCertificate => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "tls certificate selection pending without a resolver",
                    ));
                },
            }
        }
    }

    /// Negotiated ALPN protocol, once handshaken.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.tls.alpn_protocol()
    }
}

impl io::Read for TlsIoStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Serve buffered plaintext first; only pump the TLS state machine
        // when the buffer is drained (records can exceed the caller's buf).
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self.pump_inbound()?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl io::Write for TlsIoStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self
            .tls
            .write(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        self.flush_outgoing()?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

/// An inbound WebSocket message (RFC 6455 §5.6, assembled by
/// [`WsHttpClient::read_message`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    /// Peer-initiated close: status code (parsed from the close-frame
    /// payload when present) and UTF-8 reason.
    Close(Option<(u16, String)>),
    /// Peer ping — the caller must reply via [`WsHttpClient::send_pong`].
    Ping(Vec<u8>),
}

/// Errors from the blocking client's connect/send/read paths.
#[derive(Debug, thiserror::Error)]
pub enum WsClientError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("tls: {0}")]
    Tls(#[from] TlsError),
    /// RFC 6455 opening-handshake failure (`HandshakeError` implements no
    /// `Display`, so the payload is only reachable via `Debug`).
    #[error("ws handshake failed")]
    Handshake(bun_uws::ws_handshake::HandshakeError),
    #[error("ws connect: {0}")]
    Connect(String),
}

/// A complete blocking WebSocket client over the bao TLS stack: TCP +
/// BoringSSL TLS (stealth fingerprint + session-cache offer, same
/// application path as the bun fetch stack) + RFC 6455 handshake and
/// masked client framing via `bun_uws` primitives.
///
/// Thread contract: created and driven on one thread (the caller's —
/// e.g. a background worker); the SSL handle never crosses threads.
pub struct WsHttpClient {
    stream: TlsIoStream,
    decoder: bun_uws::ws_codec::FrameDecoder,
    encoder: bun_uws::ws_codec::FrameEncoder,
    closed: bool,
}

impl WsHttpClient {
    /// Connect to `wss://host:port<path>`: resolve, TCP connect, TLS
    /// handshake (stealth via `tls_props` — the same
    /// `configure_http_client_with_alpn` entry the fetch stack uses —
    /// plus session-cache offer with the bun-stack salting), then the
    /// RFC 6455 opening handshake.
    pub fn connect(
        host: &str,
        port: u16,
        path: &str,
        tls_props: Option<&crate::ssl_config::SSLConfig>,
    ) -> Result<Self, WsClientError> {
        let addr = format!("{}:{}", host, port);
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| WsClientError::Connect(format!("invalid address: {}", e)))?
            .next()
            .ok_or_else(|| WsClientError::Connect(format!("no address for {}", addr)))?;
        let tcp = TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_secs(10))
            .map_err(|e| WsClientError::Connect(format!("connect failed: {}", e)))?;
        tcp.set_nonblocking(false)
            .map_err(|e| WsClientError::Connect(format!("tcp setup: {}", e)))?;
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .map_err(|e| WsClientError::Connect(format!("tcp setup: {}", e)))?;

        let tls_client = TlsClient::new()?;
        let tls = TlsConnection::new_client(&tls_client, host)?;

        // STEALTH (REQ-STL-001) + session resumption — same application
        // path as WsConn / bun fetch: SNI (null for IP literals, RFC 6066)
        // + ALPN(http/1.1) + fingerprint fields, then offer the cached
        // session. All BEFORE the first `process()` so the config lands in
        // the ClientHello. Salt: no stealth props → 0 (default-profile
        // pool shared across stacks); props → the SSLConfig content hash,
        // so sessions that short-circuit parameter negotiation never cross
        // profiles.
        let host_z: *const core::ffi::c_char = if host.parse::<std::net::IpAddr>().is_err() {
            let host_c = CString::new(host)
                .map_err(|_| WsClientError::Connect(format!("invalid host: {}", host)))?;
            host_c.into_raw()
        } else {
            core::ptr::null()
        };
        // SAFETY: `tls.ssl_ptr()` is the live SSL handle of this
        // connection; `configure_http_client_with_alpn` only issues
        // SSL_set_* configuration calls on it. host_z (when non-null) is
        // leaked into the `CString::into_raw` allocation above so the
        // pointer outlives this call the same way the fetch path's
        // NUL-terminated buffer does.
        unsafe {
            crate::configure_http_client_with_alpn(
                &mut *tls.ssl_ptr(),
                host_z,
                crate::AlpnOffer::H1,
                tls_props,
            );
        }
        let profile_salt = tls_props.map(|props| props.content_hash()).unwrap_or(0);
        session_cache::offer_session(tls.ssl_ptr(), host, port, profile_salt);

        let mut stream = TlsIoStream::new(tcp, tls);
        stream
            .drive_handshake()
            .map_err(|e| WsClientError::Connect(format!("tls handshake: {}", e)))?;

        // RFC 6455 client handshake over the TLS stream (bun_uws-owned).
        bun_uws::ws_handshake::client_handshake(&mut stream, host, path)
            .map_err(WsClientError::Handshake)?;

        Ok(Self {
            stream,
            decoder: bun_uws::ws_codec::FrameDecoder::new(),
            encoder: bun_uws::ws_codec::FrameEncoder::new(),
            closed: false,
        })
    }

    /// Negotiated ALPN protocol, once handshaken.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.stream.alpn_protocol()
    }

    /// Send a masked text frame (client→server frames are always masked,
    /// RFC 6455 §5.1; `FrameEncoder::encode_frame` with a generated key
    /// appends the masked payload in one piece — the BCE-20260814-WS-TLS
    /// header-only-send class cannot recur here).
    pub fn send_text(&mut self, text: &str) -> io::Result<()> {
        let key = bun_uws::ws_codec::gen_mask_key();
        let frame = self
            .encoder
            .encode_frame(bun_uws::ws_codec::Opcode::Text, text.as_bytes(), Some(key));
        let owned = frame.to_vec();
        self.stream.write_all(&owned)
    }

    /// Send a masked binary frame.
    pub fn send_binary(&mut self, payload: &[u8]) -> io::Result<()> {
        let key = bun_uws::ws_codec::gen_mask_key();
        let frame = self
            .encoder
            .encode_frame(bun_uws::ws_codec::Opcode::Binary, payload, Some(key));
        let owned = frame.to_vec();
        self.stream.write_all(&owned)
    }

    /// Reply to a [`WsMessage::Ping`] with a masked pong carrying the
    /// ping payload back (RFC 6455 §5.5.3).
    pub fn send_pong(&mut self, payload: &[u8]) -> io::Result<()> {
        let key = bun_uws::ws_codec::gen_mask_key();
        let frame = self
            .encoder
            .encode_frame(bun_uws::ws_codec::Opcode::Pong, payload, Some(key));
        let owned = frame.to_vec();
        self.stream.write_all(&owned)
    }

    /// Send a masked close frame with status code and reason, then mark
    /// the client closed (RFC 6455 §5.5.1).
    pub fn close(&mut self, code: u16, reason: &str) -> io::Result<()> {
        let key = bun_uws::ws_codec::gen_mask_key();
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        let frame = self
            .encoder
            .encode_frame(bun_uws::ws_codec::Opcode::Close, &payload, Some(key));
        let owned = frame.to_vec();
        self.closed = true;
        self.stream.write_all(&owned)
    }

    /// Read the next assembled message. Control frames surface as
    /// [`WsMessage::Ping`] / [`WsMessage::Close`]; text/binary data frames
    /// are unmasked and returned. EOF from the peer maps to
    /// [`WsMessage::Close`] (idempotent once `closed`).
    pub fn read_message(&mut self) -> io::Result<WsMessage> {
        if self.closed {
            return Ok(WsMessage::Close(None));
        }
        let header = match self.decoder.decode_frame(&mut self.stream) {
            Ok(Some(h)) => h,
            Ok(None) => {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            },
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            },
            Err(ref e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.closed = true;
                return Ok(WsMessage::Close(None));
            },
            Err(e) => return Err(e),
        };
        let payload = if header.mask {
            let mask_key = self.decoder.take_mask();
            let mut p = self.decoder.take_payload(&header);
            bun_uws::ws_codec::apply_mask(&mut p, &mask_key);
            p
        } else {
            self.decoder.take_payload(&header)
        };
        match header.opcode {
            bun_uws::ws_codec::Opcode::Text => {
                // A non-UTF-8 text frame is a protocol error (RFC 6455
                // §8.1) — surface it rather than silently lossy-converting.
                let text = String::from_utf8(payload).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "ws text frame not UTF-8")
                })?;
                Ok(WsMessage::Text(text))
            },
            bun_uws::ws_codec::Opcode::Binary => Ok(WsMessage::Binary(payload)),
            bun_uws::ws_codec::Opcode::Close => {
                self.closed = true;
                let frame = if payload.len() >= 2 {
                    let code = u16::from_be_bytes([payload[0], payload[1]]);
                    let reason = String::from_utf8_lossy(&payload[2..]).into_owned();
                    Some((code, reason))
                } else {
                    None
                };
                Ok(WsMessage::Close(frame))
            },
            bun_uws::ws_codec::Opcode::Ping => Ok(WsMessage::Ping(payload)),
            bun_uws::ws_codec::Opcode::Pong => {
                // Unsolicited pong (reply to our implicit pings) carries no
                // message content — read on for the next data frame.
                let _ = payload;
                self.read_message()
            },
            // Fragmented messages: per-frame decoding (WsConn parity) does
            // not reassemble continuation fragments — deliver the fragment
            // payload as binary so the caller still sees the bytes.
            bun_uws::ws_codec::Opcode::Continuation => Ok(WsMessage::Binary(payload)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Link seams for the lib-test binary (same contract as
    // tests/tls_info_and_streaming_tests.rs): bun_io's posix event loop
    // dispatches through `__bun_run_file_poll` (owned by
    // bun_runtime::dispatch in product binaries; no FilePoll sources are
    // registered in these tests, so a no-op satisfies the reference), and
    // bun_alloc resolves `__bun_crash_handler_out_of_memory` against
    // bun_crash_handler at link time (OOM aborts either way).
    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
        eprintln!("bun: out of memory");
        std::process::abort()
    }

    #[test]
    fn stealth_pc_salt_is_deterministic_and_sensitive() {
        let a = stealth_pc_salt(Some("a:b"), Some(&[8, b'h']), Some("X25519"));
        let b = stealth_pc_salt(Some("a:b"), Some(&[8, b'h']), Some("X25519"));
        assert_eq!(a, b, "same parameter set must salt identically");

        // Each field contributes to the salt so parameter sets never share
        // a session pool.
        assert_ne!(a, stealth_pc_salt(None, Some(&[8, b'h']), Some("X25519")));
        assert_ne!(a, stealth_pc_salt(Some("a:b"), None, Some("X25519")));
        assert_ne!(a, stealth_pc_salt(Some("a:b"), Some(&[8, b'h']), None));
        // ALPN is the fetch-vs-wss segregator (h2+h1 vs h1).
        assert_ne!(
            stealth_pc_salt(None, Some(&[2, b'h', b'2', 8, b'h']), None),
            stealth_pc_salt(None, Some(&[8, b'h']), None)
        );
    }

    /// Loopback TLS WS roundtrip: a blocking server (TlsServer +
    /// server_handshake + unmasked FrameEncoder) echoes text frames;
    /// WsHttpClient connects, sends, and receives. Exercises Layer 2
    /// end-to-end including the TLS handshake pump and RFC 6455
    /// handshake/masking on both sides.
    #[test]
    fn ws_http_client_tls_roundtrip() {
        use bun_uws::ws_codec::{FrameDecoder, Opcode};
        use std::net::TcpListener;

        // C-seam link leg (addrinfo/StackCheck natives) for the test binary.
        bao_native_stubs::force_link();

        let (cert_pem, key_pem) =
            bao_boringssl_bridge::generate_self_signed_pem("localhost", 1)
                .expect("generate self-signed cert");
        let tls_server =
            bao_boringssl_bridge::TlsServer::new(&cert_pem, &key_pem).expect("TlsServer::new");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().expect("accept");
            let tls_conn = tls_server.accept().expect("tls accept");
            let mut io = TlsIoStream::new(tcp, tls_conn);
            io.drive_handshake().expect("server tls handshake");

            bun_uws::ws_handshake::server_handshake(&mut io).expect("server ws handshake");

            let mut decoder = FrameDecoder::new();
            let mut encoder = bun_uws::ws_codec::FrameEncoder::new();
            // echo loop: one masked text frame in, unmasked text frame out
            let header = decoder.decode_frame(&mut io).expect("decode").expect("frame");
            assert_eq!(header.opcode, Opcode::Text);
            let payload = if header.mask {
                let key = decoder.take_mask();
                let mut p = decoder.take_payload(&header);
                bun_uws::ws_codec::apply_mask(&mut p, &key);
                p
            } else {
                decoder.take_payload(&header)
            };
            let text = String::from_utf8(payload).expect("utf8");
            let reply = encoder.encode_text(&format!("echo:{}", text)).to_vec();
            io.write_all(&reply).expect("server write");
        });

        let mut client =
            WsHttpClient::connect("127.0.0.1", port, "/", None).expect("client connect");
        // No ALPN assertion: negotiation requires the SERVER to run a
        // select callback, which bao_boringssl_bridge::TlsServer does not
        // expose — a server that ignores ALPN yields no negotiated
        // protocol on the client. The client-side advertisement
        // (http/1.1 only) is structural: configure_http_client_with_alpn
        // sets exactly that wire list.
        client.send_text("ping").expect("client send");
        match client.read_message().expect("client read") {
            WsMessage::Text(t) => assert_eq!(t, "echo:ping"),
            other => panic!("expected text, got {:?}", other),
        }
        server.join().expect("server thread");
    }

    /// Async Layer-1 roundtrip on a real tokio runtime — the exact segment
    /// servo's websocket_loader now uses: `WsTlsStream::new` (stealth opts
    /// applied, session offered) + `handshake().await` + the RFC 6455
    /// upgrade handshake and one masked text frame driven through the
    /// tokio AsyncRead/AsyncWrite impls. The wire actions are hand-rolled
    /// (tungstenite stays servo-side) but exercise the same bytes the
    /// servo path produces.
    #[test]
    fn ws_tls_stream_async_roundtrip() {
        use std::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        bao_native_stubs::force_link();

        let (cert_pem, key_pem) =
            bao_boringssl_bridge::generate_self_signed_pem("localhost", 1)
                .expect("generate self-signed cert");
        let tls_server =
            bao_boringssl_bridge::TlsServer::new(&cert_pem, &key_pem).expect("TlsServer::new");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().expect("accept");
            let tls_conn = tls_server.accept().expect("tls accept");
            let mut io = TlsIoStream::new(tcp, tls_conn);
            io.drive_handshake().expect("server tls handshake");
            bun_uws::ws_handshake::server_handshake(&mut io).expect("server ws handshake");
            let mut decoder = bun_uws::ws_codec::FrameDecoder::new();
            let mut encoder = bun_uws::ws_codec::FrameEncoder::new();
            let header = decoder.decode_frame(&mut io).expect("decode").expect("frame");
            let payload = if header.mask {
                let key = decoder.take_mask();
                let mut p = decoder.take_payload(&header);
                bun_uws::ws_codec::apply_mask(&mut p, &key);
                p
            } else {
                decoder.take_payload(&header)
            };
            let reply = encoder
                .encode_text(&String::from_utf8_lossy(&payload))
                .to_vec();
            io.write_all(&reply).expect("server write");
        });

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(async move {
                let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .expect("tcp connect");
                let opts = WsTlsOptions {
                    // Same ALPN the servo caller passes (http/1.1 only).
                    alpn_wire: Some(vec![8, b'h', b't', b't', b'p', b'/', b'1', b'.', b'1']),
                    ..Default::default()
                };
                let tls_client = TlsClient::new().expect("TlsClient");
                let mut stream = WsTlsStream::new(tcp, &tls_client, "127.0.0.1", port, &opts)
                    .expect("WsTlsStream::new");
                stream.handshake().await.expect("client tls handshake");

                // RFC 6455 opening handshake (what tungstenite sends on
                // behalf of servo's websocket_loader).
                let key = bun_uws::ws_handshake::generate_sec_websocket_key();
                let request = format!(
                    "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\n\
                     Connection: Upgrade\r\nSec-WebSocket-Key: {}\r\n\
                     Sec-WebSocket-Version: 13\r\n\r\n",
                    key
                );
                stream.write_all(request.as_bytes()).await.expect("ws upgrade send");

                // Read the 101 response headers (terminate on \r\n\r\n).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 512];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).await.expect("read 101");
                    assert!(n > 0, "EOF during upgrade response");
                    buf.extend_from_slice(&chunk[..n]);
                }
                let headers = String::from_utf8_lossy(&buf).to_string();
                assert!(
                    headers.starts_with("HTTP/1.1 101"),
                    "unexpected upgrade response: {}",
                    headers
                );
                let expected_accept = bun_uws::ws_handshake::compute_accept(&key);
                assert!(
                    headers.contains(&expected_accept),
                    "missing Sec-WebSocket-Accept in: {}",
                    headers
                );

                // One masked text frame out, unmasked echo back.
                let mut encoder = bun_uws::ws_codec::FrameEncoder::new();
                let frame = encoder
                    .encode_frame(
                        bun_uws::ws_codec::Opcode::Text,
                        b"ping",
                        Some(bun_uws::ws_codec::gen_mask_key()),
                    )
                    .to_vec();
                stream.write_all(&frame).await.expect("frame send");

                let mut reply = Vec::new();
                while reply.len() < 2 + 4 {
                    let n = stream.read(&mut chunk).await.expect("read echo");
                    assert!(n > 0, "EOF waiting for echo");
                    reply.extend_from_slice(&chunk[..n]);
                }
                // Minimal frame parse: fin|opcode=0x81, mask=0, len=4,
                // then the echoed payload "ping" (server-side frames are
                // unmasked).
                assert_eq!(&reply[..2], &[0x81, 0x04], "echo frame header");
                assert_eq!(&reply[2..6], b"ping");
            });

        server.join().expect("server thread");
    }
}
