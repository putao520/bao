//! Shared server-side TLS fixture for the wire-level integration tests
//! (historically a verbatim copy in `tls_info_and_streaming_tests`,
//! `h2_continuation_cap_tests`, `hpack_table_size_update_tests`, and
//! `transport_backpressure_tests` — extracted after the same piggyback bug
//! had to be fixed four times in four copies).
//!
//! `ServerTlsIo` drives the server-side BoringSSL state machine over the
//! raw TCP socket (shape originally mirroring the runtime test helper in
//! `web_socket_async_tests.rs`). `handshake` pumps
//! `process`/`take_outgoing`/`feed` until the connection reaches
//! Active/PeerClosed and returns any application data that piggybacked on
//! the final handshake record; the `Read`/`Write` impls then speak
//! plaintext to the TLS peer, replaying that piggybacked plaintext first.

pub(crate) mod fetch_harness;
pub(crate) mod h2_framing;

use std::io::{Read, Write};
use std::net::TcpStream;

use bao_boringssl_bridge::{TlsConnection, TlsServer, TlsState};

/// TLS stream adapter driving the server-side BoringSSL state machine over
/// the raw TCP socket.
pub(crate) struct ServerTlsIo {
    /// Public for fixture call sites that need the raw socket (e.g. the
    /// tls_info test's post-response `shutdown(Write)`).
    pub(crate) tcp: TcpStream,
    tls: TlsConnection,
    pending_plain: Vec<u8>,
    pending_off: usize,
}

impl ServerTlsIo {
    pub(crate) fn handshake(
        tcp: &mut TcpStream,
        tls: &mut TlsConnection,
    ) -> std::io::Result<Vec<u8>> {
        loop {
            let res = tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            loop {
                let outgoing = tls.take_outgoing();
                if outgoing.is_empty() {
                    break;
                }
                tcp.write_all(&outgoing)?;
            }
            if res.state == TlsState::Active || res.state == TlsState::PeerClosed {
                // The handshake-completing process() may have decrypted
                // application data that piggybacked on the final handshake
                // record (e.g. the client's Finished + first h2 record read
                // as one segment). It must be delivered, not discarded, or
                // the server waits forever for bytes it already consumed.
                let mut piggybacked = Vec::new();
                for chunk in res.plaintext {
                    piggybacked.extend_from_slice(&chunk);
                }
                return Ok(piggybacked);
            }
            let mut buf = [0u8; 16_384];
            match tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "peer closed during tls handshake",
                    ))
                }
                Ok(n) => tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }

    /// Wrap a handshaken connection. `piggybacked` is the plaintext
    /// [`ServerTlsIo::handshake`] returned (empty when nothing piggybacked);
    /// the first `read` replays it before touching the socket.
    pub(crate) fn new(tcp: TcpStream, tls: TlsConnection, piggybacked: Vec<u8>) -> Self {
        Self {
            tcp,
            tls,
            pending_plain: piggybacked,
            pending_off: 0,
        }
    }

    fn read_plaintext(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            let outgoing = self.tls.take_outgoing();
            if !outgoing.is_empty() {
                self.tcp.write_all(&outgoing)?;
            }
            let res = self
                .tls
                .process()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            if !res.plaintext.is_empty() {
                let mut joined = Vec::new();
                for chunk in res.plaintext {
                    joined.extend_from_slice(&chunk);
                }
                return Ok(joined);
            }
            let mut buf = [0u8; 16_384];
            match self.tcp.read(&mut buf) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "tls peer closed",
                    ))
                }
                Ok(n) => self.tls.feed(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Read for ServerTlsIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending_off >= self.pending_plain.len() {
            self.pending_plain = self.read_plaintext()?;
            self.pending_off = 0;
        }
        let avail = &self.pending_plain[self.pending_off..];
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.pending_off += n;
        Ok(n)
    }
}

impl Write for ServerTlsIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self
            .tls
            .write(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let outgoing = self.tls.take_outgoing();
        if !outgoing.is_empty() {
            self.tcp.write_all(&outgoing)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

// ─── ALPN-h2 selection ──────────────────────────────────────────────────────

// Used by the three h2-shaped binaries; `tls_info_and_streaming_tests`
// selects no ALPN, so these are dead code in its test binary.
#[allow(dead_code)]
/// Length-prefixed ALPN wire entry for "h2".
const ALPN_H2: &[u8] = b"\x02h2";

#[allow(dead_code)]
unsafe extern "C" fn alpn_select_h2(
    _ssl: *mut bun_boringssl_sys::SSL,
    out: *mut *const u8,
    out_len: *mut u8,
    in_: *const u8,
    in_len: core::ffi::c_uint,
    _arg: *mut core::ffi::c_void,
) -> core::ffi::c_int {
    let list = unsafe { std::slice::from_raw_parts(in_, in_len as usize) };
    let mut offset = 0usize;
    while offset < list.len() {
        let len = list[offset] as usize;
        offset += 1;
        if offset + len > list.len() {
            break;
        }
        if &list[offset..offset + len] == b"h2" {
            unsafe {
                *out = ALPN_H2.as_ptr().add(1); // past the length byte
                *out_len = 2;
            }
            return bun_boringssl_sys::SSL_TLSEXT_ERR_OK;
        }
        offset += len;
    }
    bun_boringssl_sys::SSL_TLSEXT_ERR_NOACK
}

#[allow(dead_code)]
/// Install the h2-only ALPN selector on the fixture server's SSL_CTX.
///
/// SAFETY: TlsServer::ctx returns its live SSL_CTX; installing the ALPN
/// select callback before any accept is thread-free.
pub(crate) fn install_alpn_h2(server: &TlsServer) {
    unsafe {
        bun_boringssl_sys::SSL_CTX_set_alpn_select_cb(
            server.ctx(),
            Some(alpn_select_h2),
            core::ptr::null_mut(),
        );
    }
}
