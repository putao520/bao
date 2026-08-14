/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Bao vendor patch: this module implements the boringssl-backed stealth TLS
// connector (REQ-STL-001) and contains the unsafe FFI the rustls migration
// removed from the rest of the crate.
#![allow(unsafe_code)]

use std::collections::hash_map::HashMap;
use std::convert::TryFrom;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use std::{fmt, io};

use futures::task::{Context, Poll};
use futures::{Future, TryFutureExt};
use http::uri::{Authority, Uri as Destination};
use http_body_util::combinators::BoxBody;
use hyper::body::Bytes;
use hyper::rt::Executor;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{
    Connected, Connection, HttpConnector as HyperHttpConnector,
};
use hyper_util::rt::TokioIo;
use log::warn;
use parking_lot::Mutex;
use servo_config::pref;
use tokio::io::{AsyncRead as _, AsyncWrite as _};
use tokio::net::TcpStream;
use tower::Service;

use bao_boringssl_bridge::{TlsClient, TlsConnection, TlsProfile, TlsError};
use bao_stealth::{boringssl_cipher_list_string, boringssl_curves_list_string, boringssl_sigalgs_list_string};
use bao_boringssl_bridge::connection::TlsState;
use bun_boringssl_sys::boringssl::*;

use crate::async_runtime::spawn_task;
use crate::hosts::replace_host;

pub const BUF_SIZE: usize = 32768;

/// ALPN identifier for HTTP/2 (RFC 7540 §3.1).
pub const ALPN_H2: &str = "h2";

// ── Stealth TLS/HTTP2 wire configuration ──────────────────────────────
//
// Wire-level configuration for servo's TLS/HTTP2 stack, set by the embedder
// (Bao) during stealth profile initialization. Follows the same pattern as
// `servo_canvas::canvas_noise::set_global_canvas_noise()` — global static,
// set once at init time, read on every connection.
//
// Re-declared here (instead of importing from `bao_stealth`) to avoid a
// dependency on `bao_stealth` from servo's `net` crate. Fields must be kept
// in sync with `bao_stealth::StealthTlsWireConfig`.

/// Wire-level TLS/HTTP2 configuration for servo's network layer.
///
/// When set, `create_tls_config()` applies cipher suites, curves, signature
/// algorithms, and ALPN protocols to the BoringSSL `SSL_CTX`, and
/// `create_http_client()` applies HTTP/2 settings (window sizes, max
/// frame/header sizes) via hyper's builder.
///
/// BoringSSL supports full JA3/JA4 fingerprint configuration including
/// cipher suite reordering, curves/groups ordering, and signature algorithm
/// ordering — including cipher suite reordering, curves/groups ordering, and signature algorithm ordering.
#[derive(Debug, Clone)]
pub struct StealthTlsWireConfig {
    /// TLS 1.2 cipher suites as IANA u16 IDs (ordered as in profile).
    /// Applied via `SSL_CTX_set_cipher_list()` on the BoringSSL SSL_CTX.
    pub tls12_cipher_suites: Vec<u16>,
    /// TLS 1.3 cipher suites as IANA u16 IDs (ordered as in profile).
    /// Applied via `SSL_CTX_set_cipher_list()` on the BoringSSL SSL_CTX.
    pub tls13_cipher_suites: Vec<u16>,
    /// Signature algorithms as IANA u16 IDs.
    /// Applied via `SSL_CTX_set1_sigalgs_list()` on the BoringSSL SSL_CTX.
    pub signature_algorithms: Vec<u16>,
    /// Supported groups as IANA u16 IDs.
    /// Applied via `SSL_set1_curves_list()` per-connection on the BoringSSL SSL.
    pub supported_groups: Vec<u16>,
    /// ALPN protocols as raw bytes (e.g., `b"h2"`, `b"http/1.1"`).
    /// Applied via `SSL_CTX_set_alpn_protos()` on the BoringSSL SSL_CTX.
    pub alpn_protocols: Vec<Vec<u8>>,
    /// HTTP/2 SETTINGS payload in binary wire format (6 bytes per setting).
    /// Stored for potential future custom h2 wrapper.
    pub h2_settings_payload: Vec<u8>,
    /// HTTP/2 initial stream window size. Applied via hyper builder.
    pub h2_initial_stream_size: u32,
    /// HTTP/2 initial connection window size. Applied via hyper builder.
    pub h2_initial_connection_window_size: u32,
    /// HTTP/2 SETTINGS_MAX_FRAME_SIZE. Applied via hyper builder.
    pub h2_max_frame_size: u32,
    /// HTTP/2 SETTINGS_MAX_HEADER_LIST_SIZE. Applied via hyper builder.
    pub h2_max_header_list_size: u32,
}

/// Global stealth TLS/HTTP2 wire configuration set by the embedder (Bao).
/// When `Some`, `create_tls_config()` and `create_http_client()` use these
/// values to shape the TLS ClientHello and HTTP/2 SETTINGS frame.
static STEALTH_TLS_CONFIG: RwLock<Option<StealthTlsWireConfig>> = RwLock::new(None);

/// Set the global stealth TLS/HTTP2 configuration.
///
/// Called by Bao's runtime bridge during stealth profile initialization,
/// following the same pattern as `servo::set_canvas_noise_seed()`.
pub fn set_stealth_tls_config(config: Option<StealthTlsWireConfig>) {
    let mut guard = STEALTH_TLS_CONFIG.write().unwrap();
    *guard = config;
}

/// Read the current global stealth TLS/HTTP2 configuration.
fn get_stealth_tls_config() -> Option<StealthTlsWireConfig> {
    STEALTH_TLS_CONFIG.read().unwrap().clone()
}

// ── IANA cipher/group/sigalg ID → OpenSSL name mapping ────────────────
//
// SINGLE SOURCE OF TRUTH: `bao_stealth` (src/bao_stealth/src/tls.rs),
// cross-verified against the vendored BoringSSL (`tls1.h` TLS1_TXT_* and
// the kCiphers/kNamedGroups/kSignatureAlgorithmNames tables). Local copies
// of this mapping are forbidden — the previous local table had nearly
// every entry wrong (0x009E→ECDHE-RSA-AES256-GCM-SHA384, 0xC02F/0xC030
// RSA/ECDSA swapped, 0x002F/0x0035 RSA suites mapped to ECDHE names) and
// dropped 9 of the 13 profile suites.
//
// The `boringssl_*_list_string` builders also encode engine limits:
//  - TLS 1.3 suite order is built into BoringSSL (ssl.h: "TLS 1.3 ciphers
//    do not participate in this mechanism");
//  - DHE_RSA ciphers and FFDHE groups have no BoringSSL implementation,
//    and an unrecognized GROUP name fails the whole set1_groups_list call.

// ── HTTP connector ────────────────────────────────────────────────────

/// DNS resolver sharing bao's process-wide per-host cache
/// (`bun_dns::cache`) with the usockets and `node:dns` paths — the fusion
/// point that makes every stack in one process resolve a host once per TTL
/// window (stealth parity with a real browser's single resolver).
///
/// Cache hit → immediate addresses; miss → blocking `getaddrinfo` on a
/// tokio worker (same fallback `GaiResolver` uses) with the result written
/// back into the shared cache. `getaddrinfo` returns no TTL, so entries use
/// the engine cap `BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS` (default 30 s, the
/// same lifetime upstream Bun applies to its DNS cache).
#[derive(Clone, Default)]
struct SharedCacheDnsResolver;

impl SharedCacheDnsResolver {
    fn to_std(ip: &bun_dns::cache::IpAddr) -> std::net::IpAddr {
        match ip {
            bun_dns::cache::IpAddr::V4(octets) => std::net::IpAddr::V4(
                std::net::Ipv4Addr::from(*octets),
            ),
            bun_dns::cache::IpAddr::V6(octets) => std::net::IpAddr::V6(
                std::net::Ipv6Addr::from(*octets),
            ),
        }
    }

    fn from_std(ip: &std::net::IpAddr) -> bun_dns::cache::IpAddr {
        match ip {
            std::net::IpAddr::V4(v4) => bun_dns::cache::IpAddr::V4(v4.octets()),
            std::net::IpAddr::V6(v6) => bun_dns::cache::IpAddr::V6(v6.octets()),
        }
    }
}

impl Service<hyper_util::client::legacy::connect::dns::Name> for SharedCacheDnsResolver {
    // Port 0 addresses: hyper-util's `set_port` applies the destination port
    // when the address port is 0 (same contract as `GaiResolver`).
    type Response = std::vec::IntoIter<std::net::SocketAddr>;
    type Error = io::Error;
    type Future = std::pin::Pin<
        Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(
        &mut self,
        name: hyper_util::client::legacy::connect::dns::Name,
    ) -> Self::Future {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if let Some(addrs) = bun_dns::cache::lookup(host.as_bytes()) {
                return Ok(addrs
                    .iter()
                    .map(|ip| std::net::SocketAddr::new(Self::to_std(ip), 0))
                    .collect::<Vec<_>>()
                    .into_iter());
            }
            let host_for_cache = host.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                // Same lookup GaiResolver performs: (host, 0) with the
                // system resolver, port applied later by hyper.
                use std::net::ToSocketAddrs;
                (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|it| it.map(|sa| sa.ip()).collect::<Vec<_>>())
            })
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))??;
            let ips: Vec<bun_dns::cache::IpAddr> =
                resolved.iter().map(Self::from_std).collect();
            bun_dns::cache::insert(host_for_cache.as_bytes(), ips, None);
            Ok(resolved
                .into_iter()
                .map(|ip| std::net::SocketAddr::new(ip, 0))
                .collect::<Vec<_>>()
                .into_iter())
        })
    }
}

#[derive(Clone)]
pub struct ServoHttpConnector {
    inner: HyperHttpConnector<SharedCacheDnsResolver>,
}

impl ServoHttpConnector {
    fn new() -> ServoHttpConnector {
        let mut inner = HyperHttpConnector::new_with_resolver(SharedCacheDnsResolver);
        inner.enforce_http(false);
        inner.set_happy_eyeballs_timeout(None);
        inner.set_connect_timeout(Some(Duration::from_secs(pref!(network_connection_timeout))));
        ServoHttpConnector { inner }
    }
}

impl Service<Destination> for ServoHttpConnector {
    type Response = TokioIo<TcpStream>;
    type Error = ConnectionError;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<TokioIo<TcpStream>, ConnectionError>> + Send>>;

    fn call(&mut self, dest: Destination) -> Self::Future {
        // Perform host replacement when making the actual TCP connection.
        let mut new_dest = dest.clone();
        let mut parts = dest.into_parts();

        if let Some(auth) = parts.authority {
            let host = auth.host();
            let host = replace_host(host);

            let authority = if let Some(port) = auth.port() {
                format!("{}:{}", host, port.as_str())
            } else {
                (*host).to_string()
            };

            if let Ok(authority) = Authority::from_maybe_shared(authority) {
                parts.authority = Some(authority);
                if let Ok(dest) = Destination::from_parts(parts) {
                    new_dest = dest
                }
            }
        }

        Box::pin(
            self.inner
                .call(new_dest)
                .map_err(|e| ConnectionError::HttpError(format!("{e}"))),
        )
    }

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Ok(()).into()
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ── BoringSSL-backed TLS stream ───────────────────────────────────────
//
// Wraps a TcpStream + TlsConnection to provide AsyncRead/AsyncWrite
// for hyper. The TlsConnection uses BIO pairs for non-blocking I/O:
// we feed incoming ciphertext from TCP into the TLS engine, and
// extract outgoing ciphertext from the TLS engine to write to TCP.

/// A TLS stream backed by BoringSSL, wrapping a TCP stream.
pub struct BoringsslTlsStream {
    tcp: TcpStream,
    tls: TlsConnection,
    /// Buffered outgoing TLS ciphertext that hasn't been written to TCP yet.
    outgoing: Vec<u8>,
}

impl BoringsslTlsStream {
    pub fn new(tcp: TcpStream, tls: TlsConnection) -> Self {
        Self {
            tcp,
            tls,
            outgoing: Vec::new(),
        }
    }

    /// Drive the TLS handshake to completion.
    ///
    /// Reads from TCP, feeds into TLS, processes, writes outgoing to TCP.
    /// Returns Ok(()) when handshake is complete, Err if it failed.
    pub async fn handshake(&mut self) -> io::Result<()> {
        loop {
            // Try to process the TLS state machine
            match self.tls.process() {
                Ok(result) => {
                    // Flush any outgoing data
                    self.flush_outgoing().await?;

                    match result.state {
                        TlsState::Active => return Ok(()),
                        TlsState::PeerClosed | TlsState::Closed => {
                            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "TLS peer closed during handshake"))
                        }
                        TlsState::Handshaking => {
                            // Need more data from the network
                            self.read_from_tcp().await?;
                        }
                        // BAO: SNI-driven certificate selection parked the
                        // handshake. The connector registers no
                        // select-certificate callback (that is node:tls
                        // server machinery), so this state is structurally
                        // unreachable here. Fail the handshake explicitly
                        // rather than waiting silently — the state only
                        // clears via an external credential resolver that
                        // this path does not have.
                        TlsState::PendingCertificate => {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "TLS certificate selection pending without a resolver",
                            ));
                        }
                    }
                }
                Err(TlsError::BoringSSL(msg)) => {
                    return Err(io::Error::new(io::ErrorKind::Other, msg));
                }
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
                }
            }
        }
    }

    /// Read ciphertext from TCP and feed it into the TLS engine.
    async fn read_from_tcp(&mut self) -> io::Result<()> {
        let mut buf = [0u8; BUF_SIZE];
        match self.tcp.readable().await {
            Ok(()) => {
                match self.tcp.try_read(&mut buf) {
                    Ok(0) => {
                        // EOF
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "TCP connection closed"));
                    }
                    Ok(n) => {
                        self.tls.feed(&buf[..n]);
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        // No data available yet, that's fine
                    }
                    Err(e) => return Err(e),
                }
            }
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
            match self.tcp.writable().await {
                Ok(()) => {
                    match self.tcp.try_write(&self.outgoing) {
                        Ok(n) => {
                            self.outgoing = self.outgoing[n..].to_vec();
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                            // TCP buffer full, try again later
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Get the ALPN protocol negotiated during the TLS handshake.
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        self.tls.alpn_protocol()
    }

    /// Get TLS handshake info from the BoringSSL connection.
    pub fn tls_info(&self) -> TlsHandshakeInfo {
        let ssl = self.tls.ssl_ptr();

        // SAFETY: SSL_get_version returns a pointer to a static string owned by BoringSSL.
        // The pointer is valid for the lifetime of the SSL object and we only read from it.
        let protocol_version = unsafe {
            let version = SSL_get_version(ssl);
            let version_str = std::ffi::CStr::from_ptr(version);
            Some(version_str.to_string_lossy().into_owned())
        };

        // SAFETY: SSL_get_current_cipher returns a pointer to an internal SSL_CIPHER struct
        // owned by BoringSSL. The pointer is valid for the lifetime of the SSL object.
        // SSL_CIPHER_get_name returns a static string. We only read from both pointers.
        let cipher_suite = unsafe {
            let cipher = SSL_get_current_cipher(ssl);
            if cipher.is_null() {
                None
            } else {
                let name = SSL_CIPHER_get_name(cipher);
                let name_str = std::ffi::CStr::from_ptr(name);
                Some(name_str.to_string_lossy().into_owned())
            }
        };

        // BoringSSL doesn't expose the KX group name directly via a simple API.
        // We leave kea_group_name as None for now; it can be extracted via
        // SSL_get_peer_cert_chain or custom extensions if needed.
        let kea_group_name: Option<String> = None;

        // Signature scheme name — not directly available from BoringSSL's public API.
        let signature_scheme_name: Option<String> = None;

        let alpn_protocol = self.tls.alpn_protocol()
            .map(|proto| String::from_utf8_lossy(proto).into_owned());

        // SAFETY: SSL_get0_peer_certificates returns a pointer to an internal stack of
        // CRYPTO_BUFFER owned by the SSL object. We only read from the buffers and copy
        // the data into owned Vec<u8>. The stack and buffers are valid for the SSL lifetime.
        let certificate_chain_der = unsafe {
            let mut chain: Vec<Vec<u8>> = Vec::new();
            let cert_stack = SSL_get0_peer_certificates(ssl);
            if !cert_stack.is_null() {
                let num = bun_boringssl_sys::sk_CRYPTO_BUFFER_num(cert_stack);
                for i in 0..num {
                    let buf_ptr = bun_boringssl_sys::sk_CRYPTO_BUFFER_value(cert_stack, i);
                    if !buf_ptr.is_null() {
                        let data = bun_boringssl_sys::CRYPTO_BUFFER_data(buf_ptr);
                        let len = bun_boringssl_sys::CRYPTO_BUFFER_len(buf_ptr);
                        if !data.is_null() && len > 0 {
                            let slice = std::slice::from_raw_parts(data, len);
                            chain.push(slice.to_vec());
                        }
                    }
                }
            }
            chain
        };

        TlsHandshakeInfo {
            protocol_version,
            cipher_suite,
            kea_group_name,
            signature_scheme_name,
            alpn_protocol,
            certificate_chain_der,
            used_ech: false, // ECH not supported in BoringSSL bridge yet
        }
    }
}

impl Connection for BoringsslTlsStream {
    fn connected(&self) -> Connected {
        let connected = self.tcp.connected();
        if self.alpn_protocol() == Some(ALPN_H2.as_bytes()) {
            connected.negotiated_h2()
        } else {
            connected
        }
    }
}

impl hyper::rt::Read for BoringsslTlsStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        let this = self.get_mut();

        // Try to read plaintext from the TLS engine
        loop {
            // First, try to process any pending TLS data
            match this.tls.process() {
                Ok(result) => {
                    // If there's outgoing data, schedule a flush (but don't block read on it)
                    if result.outgoing_bytes > 0 {
                        let outgoing = this.tls.take_outgoing();
                        this.outgoing.extend_from_slice(&outgoing);
                    }

                    // If we got plaintext data, copy it to the output buffer
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
                        TlsState::Active => {
                            // No plaintext available, need more ciphertext from TCP
                            break;
                        }
                        TlsState::PeerClosed | TlsState::Closed => {
                            return Poll::Ready(Ok(())); // EOF
                        }
                        TlsState::Handshaking => {
                            // Still handshaking, need more data
                            break;
                        }
                        // BAO: SNI-driven certificate selection parked the
                        // handshake. Unreachable on the connector path (no
                        // select-certificate callback is registered here —
                        // that is node:tls server machinery). Fail closed
                        // instead of waiting: the state only clears via an
                        // external credential resolver this path lacks.
                        TlsState::PendingCertificate => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::Other,
                                "TLS certificate selection pending without a resolver",
                            )));
                        }
                    }
                }
                Err(TlsError::NotReady) => break,
                Err(TlsError::BoringSSL(msg)) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, msg)));
                }
                Err(e) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string())));
                }
            }
        }

        // Need more data from TCP — read from TCP and feed into TLS
        let mut tcp_buf = [0u8; BUF_SIZE];
        let mut read_buf = tokio::io::ReadBuf::new(&mut tcp_buf);
        match std::pin::Pin::new(&mut this.tcp).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    // TCP EOF
                    return Poll::Ready(Ok(()));
                }
                this.tls.feed(read_buf.filled());
                // Try to process once more
                match this.tls.process() {
                    Ok(result) => {
                        // Flush outgoing
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
                        // No data yet, tell caller to poll again
                        Poll::Pending
                    }
                    Err(TlsError::NotReady) => Poll::Pending,
                    Err(e) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string()))),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl hyper::rt::Write for BoringsslTlsStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.get_mut();

        // Encrypt the plaintext via TLS
        match this.tls.write(buf) {
            Ok(n) => {
                // Get the encrypted outgoing data
                let outgoing = this.tls.take_outgoing();
                this.outgoing.extend_from_slice(&outgoing);

                // Try to write outgoing to TCP
                while !this.outgoing.is_empty() {
                    match std::pin::Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                        Poll::Ready(Ok(0)) => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "TCP write returned zero",
                            )));
                        }
                        Poll::Ready(Ok(written)) => {
                            this.outgoing = this.outgoing[written..].to_vec();
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                Poll::Ready(Ok(n))
            }
            Err(TlsError::NotReady) => Poll::Pending,
            Err(e) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string()))),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        let this = self.get_mut();

        // Flush any remaining outgoing data to TCP
        while !this.outgoing.is_empty() {
            match std::pin::Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "TCP write returned zero during flush",
                    )));
                }
                Poll::Ready(Ok(written)) => {
                    this.outgoing = this.outgoing[written..].to_vec();
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }

        std::pin::Pin::new(&mut this.tcp).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        let this = self.get_mut();

        // Send TLS close_notify
        let _ = this.tls.queue_close_notify();
        let outgoing = this.tls.take_outgoing();
        if !outgoing.is_empty() {
            this.outgoing.extend_from_slice(&outgoing);
        }

        // Flush remaining outgoing
        while !this.outgoing.is_empty() {
            match std::pin::Pin::new(&mut this.tcp).poll_write(cx, &this.outgoing) {
                Poll::Ready(Ok(0)) => break,
                Poll::Ready(Ok(written)) => {
                    this.outgoing = this.outgoing[written..].to_vec();
                }
                Poll::Ready(Err(_)) => break,
                Poll::Pending => return Poll::Pending,
            }
        }

        std::pin::Pin::new(&mut this.tcp).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.tcp.is_write_vectored()
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        // Flatten the vectored write into a single write for TLS
        let total: usize = bufs.iter().map(|b| b.len()).sum();
        if total == 0 {
            return Poll::Ready(Ok(0));
        }
        // For small writes, just concatenate and do a single TLS write
        if total <= BUF_SIZE {
            let mut flat = Vec::with_capacity(total);
            for b in bufs {
                flat.extend_from_slice(b);
            }
            <Self as hyper::rt::Write>::poll_write(self, cx, &flat)
        } else {
            // For large writes, just write the first buffer
            if bufs[0].is_empty() {
                <Self as hyper::rt::Write>::poll_write(self, cx, &bufs[1])
            } else {
                <Self as hyper::rt::Write>::poll_write(self, cx, &bufs[0])
            }
        }
    }
}

// ── tokio AsyncRead/AsyncWrite for BoringsslTlsStream ──────────────────
// Required for WebSocket integration via async_tungstenite::tokio::TokioAdapter.

impl tokio::io::AsyncRead for BoringsslTlsStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // Try to read plaintext from the TLS engine
        loop {
            // First, try to process any pending TLS data
            match this.tls.process() {
                Ok(result) => {
                    // If there's outgoing data, buffer it
                    if result.outgoing_bytes > 0 {
                        let outgoing = this.tls.take_outgoing();
                        this.outgoing.extend_from_slice(&outgoing);
                    }

                    // If we got plaintext data, copy it to the output buffer
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
                        TlsState::Active => {
                            // No plaintext available, need more ciphertext from TCP
                            break;
                        }
                        TlsState::PeerClosed | TlsState::Closed => {
                            return Poll::Ready(Ok(())); // EOF
                        }
                        TlsState::Handshaking => {
                            // Still handshaking, need more data
                            break;
                        }
                        // BAO: SNI-driven certificate selection parked the
                        // handshake. Unreachable on the connector path (no
                        // select-certificate callback is registered here —
                        // that is node:tls server machinery). Fail closed
                        // instead of waiting: the state only clears via an
                        // external credential resolver this path lacks.
                        TlsState::PendingCertificate => {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::Other,
                                "TLS certificate selection pending without a resolver",
                            )));
                        }
                    }
                }
                Err(TlsError::NotReady) => break,
                Err(TlsError::BoringSSL(msg)) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, msg)));
                }
                Err(e) => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string())));
                }
            }
        }

        // Need more data from TCP — read from TCP and feed into TLS
        let mut tcp_buf = [0u8; BUF_SIZE];
        let mut read_buf = tokio::io::ReadBuf::new(&mut tcp_buf);
        match std::pin::Pin::new(&mut this.tcp).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    // TCP EOF
                    return Poll::Ready(Ok(()));
                }
                this.tls.feed(read_buf.filled());
                // Try to process once more
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
                    }
                    Err(TlsError::NotReady) => Poll::Pending,
                    Err(e) => Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e.to_string()))),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl tokio::io::AsyncWrite for BoringsslTlsStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        // Delegate to the hyper Write implementation
        <Self as hyper::rt::Write>::poll_write(self, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        <Self as hyper::rt::Write>::poll_flush(self, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        <Self as hyper::rt::Write>::poll_shutdown(self, cx)
    }
}

// ── Instrumented connector and stream ─────────────────────────────────

/// A stream that wraps either a plain TCP stream or a BoringSSL TLS stream,
/// with optional TLS handshake info attached.
pub enum MaybeTlsStream {
    Http(TokioIo<TcpStream>),
    Https(BoringsslTlsStream),
}

pub struct InstrumentedStream {
    inner: MaybeTlsStream,
    tls_info: Option<TlsHandshakeInfo>,
}

impl Unpin for InstrumentedStream {}

impl fmt::Debug for InstrumentedStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstrumentedStream")
            .field("tls_info", &self.tls_info)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct TlsHandshakeInfo {
    pub protocol_version: Option<String>,
    pub cipher_suite: Option<String>,
    pub kea_group_name: Option<String>,
    pub signature_scheme_name: Option<String>,
    pub alpn_protocol: Option<String>,
    pub certificate_chain_der: Vec<Vec<u8>>,
    pub used_ech: bool,
}

impl Connection for InstrumentedStream {
    fn connected(&self) -> Connected {
        let connected = match &self.inner {
            MaybeTlsStream::Http(stream) => stream.connected(),
            MaybeTlsStream::Https(stream) => stream.connected(),
        };
        if let Some(info) = &self.tls_info {
            connected.extra(info.clone())
        } else {
            connected
        }
    }
}

impl hyper::rt::Read for InstrumentedStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match &mut self.get_mut().inner {
            MaybeTlsStream::Http(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Read>::poll_read(std::pin::Pin::new(stream), cx, buf)
            }
        }
    }
}

impl hyper::rt::Write for InstrumentedStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match &mut self.get_mut().inner {
            MaybeTlsStream::Http(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Write>::poll_write(std::pin::Pin::new(stream), cx, buf)
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match &mut self.get_mut().inner {
            MaybeTlsStream::Http(stream) => std::pin::Pin::new(stream).poll_flush(cx),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Write>::poll_flush(std::pin::Pin::new(stream), cx)
            }
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match &mut self.get_mut().inner {
            MaybeTlsStream::Http(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Write>::poll_shutdown(std::pin::Pin::new(stream), cx)
            }
        }
    }

    fn is_write_vectored(&self) -> bool {
        match &self.inner {
            MaybeTlsStream::Http(stream) => stream.is_write_vectored(),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Write>::is_write_vectored(stream)
            }
        }
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        match &mut self.get_mut().inner {
            MaybeTlsStream::Http(stream) => std::pin::Pin::new(stream).poll_write_vectored(cx, bufs),
            MaybeTlsStream::Https(stream) => {
                <BoringsslTlsStream as hyper::rt::Write>::poll_write_vectored(std::pin::Pin::new(stream), cx, bufs)
            }
        }
    }
}

// ── BoringSSL HTTPS connector ─────────────────────────────────────────

/// A connector that wraps TCP connections with BoringSSL TLS when the
/// scheme is "https", and passes through plain TCP for "http".
#[derive(Clone)]
pub struct BoringsslHttpsConnector {
    http: ServoHttpConnector,
    tls_client: TlsClient,
    ignore_certificate_errors: bool,
    stealth_per_connection: Option<StealthPerConnection>,
}

impl BoringsslHttpsConnector {
    fn new(tls_client: TlsClient, ignore_certificate_errors: bool, stealth_per_connection: Option<StealthPerConnection>) -> Self {
        Self {
            http: ServoHttpConnector::new(),
            tls_client,
            ignore_certificate_errors,
            stealth_per_connection,
        }
    }
}

impl Service<Destination> for BoringsslHttpsConnector {
    type Response = InstrumentedStream;
    type Error = BoxError;
    type Future = std::pin::Pin<
        Box<dyn Future<Output = Result<InstrumentedStream, BoxError>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.http.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Destination) -> Self::Future {
        let is_https = dst.scheme_str() == Some("https");
        let tls_client = self.tls_client.clone();
        let ignore_cert_errors = self.ignore_certificate_errors;
        let stealth_pc = self.stealth_per_connection.clone();

        if is_https {
            let host = dst
                .host()
                .map(|h| h.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            // TLS endpoint port for the session-resumption origin key.
            let port = dst.port_u16().unwrap_or(443);

            let future = self.http.call(dst);
            Box::pin(async move {
                let tcp_stream = future.await.map_err(|e| BoxError::from(e.to_string()))?;
                // tcp_stream is TokioIo<TcpStream>, extract the inner TcpStream
                let tcp = tcp_stream.into_inner();

                // Create a BoringSSL TLS connection
                let mut tls_conn = TlsConnection::new_client(&tls_client, &host)
                    .map_err(|e| BoxError::from(io::Error::new(io::ErrorKind::Other, e.to_string())))?;

                // Apply per-connection stealth settings via SSL_set_* functions
                if let Some(ref pc) = stealth_pc {
                    let ssl = tls_conn.ssl_ptr();

                    // Set signature algorithms
                    if let Some(ref sigalg_str) = pc.sigalg_list {
                        let sigalg_c = std::ffi::CString::new(sigalg_str.as_str())
                            .map_err(|e| BoxError::from(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
                        // SAFETY: SSL_set1_sigalgs_list sets the signature algorithms on the SSL
                        // object. The CString is valid for the duration of this call. The ssl
                        // pointer is valid because we just created the TlsConnection.
                        let ok = unsafe { SSL_set1_sigalgs_list(ssl, sigalg_c.as_ptr()) };
                        if ok == 0 {
                            warn!("BoringSSL: SSL_set1_sigalgs_list failed");
                        }
                    }

                    // Set ALPN protocols
                    if let Some(ref alpn_wire) = pc.alpn_wire {
                        // SAFETY: SSL_set_alpn_protos sets ALPN on the SSL object. The alpn_wire
                        // buffer is valid for the duration of this call. The ssl pointer is valid
                        // because we just created the TlsConnection.
                        let ok = unsafe {
                            SSL_set_alpn_protos(ssl, alpn_wire.as_ptr(), alpn_wire.len())
                        };
                        if ok != 0 {
                            warn!("BoringSSL: SSL_set_alpn_protos failed");
                        }
                    }

                    // Set curves/groups
                    if let Some(ref curves_str) = pc.curves_list {
                        let curves_c = std::ffi::CString::new(curves_str.as_str())
                            .map_err(|e| BoxError::from(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
                        let _ = tls_conn.set_curves_list(curves_c.as_ptr());
                    }
                }

                // If ignoring certificate errors, disable verification per-connection
                if ignore_cert_errors {
                    // SAFETY: SSL_set_verify disables certificate verification on the SSL
                    // object. The ssl pointer is valid because we just created the TlsConnection.
                    unsafe {
                        SSL_set_verify(tls_conn.ssl_ptr(), 0, None);
                    }
                }

                // TLS session resumption: offer the cached session for this
                // origin (shared with the bun_http stack via
                // bao_boringssl_bridge::session_cache) before the handshake
                // starts. The profile salt segregates stealth-profile
                // sessions from default-profile ones — offering a session
                // short-circuits parameter negotiation, so a stealth
                // connection must not resume a default-profile session
                // (and vice versa).
                let profile_salt = stealth_pc.as_ref().map(stealth_pc_salt).unwrap_or(0);
                bao_boringssl_bridge::session_cache::offer_session(
                    tls_conn.ssl_ptr(),
                    &host,
                    port,
                    profile_salt,
                );

                let mut stream = BoringsslTlsStream::new(tcp, tls_conn);

                // Drive the TLS handshake
                stream.handshake().await
                    .map_err(|e| -> BoxError { BoxError::from(e) })?;

                let tls_info = stream.tls_info();
                Ok(InstrumentedStream {
                    inner: MaybeTlsStream::Https(stream),
                    tls_info: Some(tls_info),
                })
            })
        } else {
            let future = self.http.call(dst);
            Box::pin(async move {
                let tcp_stream = future.await.map_err(|e| BoxError::from(e.to_string()))?;
                Ok(InstrumentedStream {
                    inner: MaybeTlsStream::Http(tcp_stream),
                    tls_info: None,
                })
            })
        }
    }
}

// ── Certificate error override management ─────────────────────────────

#[derive(Clone, Debug, Default)]
struct CertificateErrorOverrideManagerInternal {
    /// Certificates that have seen verification errors, keyed by hostname.
    certificates_failing_to_verify: HashMap<String, Vec<u8>>,
    /// Certificates that should be accepted despite verification errors.
    overrides: Vec<Vec<u8>>,
}

/// This data structure is used to track certificate verification errors and overrides.
/// It tracks:
///  - A list of certificate DER bytes with verification errors mapped by hostname
///  - A list of certificate DER bytes for which to ignore verification errors.
#[derive(Clone, Debug, Default)]
pub struct CertificateErrorOverrideManager(Arc<Mutex<CertificateErrorOverrideManagerInternal>>);

impl CertificateErrorOverrideManager {
    pub fn new() -> Self {
        Self(Default::default())
    }

    /// Add a certificate to this manager's list of certificates for which to ignore
    /// validation errors.
    pub fn add_override(&self, certificate: &[u8]) {
        self.0.lock().overrides.push(certificate.to_vec());
    }

    /// Given a server host name, remove information about a certificate with
    /// verification errors. If a certificate with verification errors was found,
    /// return it, otherwise None.
    pub(crate) fn remove_certificate_failing_verification(
        &self,
        host: &str,
    ) -> Option<Vec<u8>> {
        self.0
            .lock()
            .certificates_failing_to_verify
            .remove(host)
    }
}

#[derive(Clone, Debug, Default)]
pub enum CACertificates {
    #[default]
    Default,
    Override(Vec<Vec<u8>>),
}

// ── TLS config ────────────────────────────────────────────────────────

/// BoringSSL TLS configuration for servo's HTTP client.
///
/// Wraps a `TlsClient` (which owns an `SSL_CTX`) along with certificate
/// verification settings.
#[derive(Clone)]
pub struct TlsConfig {
    pub client: TlsClient,
    pub ignore_certificate_errors: bool,
    /// Per-connection settings from stealth profile (applied on each new TLS connection
    /// because BoringSSL only provides SSL_set_* variants, not SSL_CTX_set_*).
    pub stealth_per_connection: Option<StealthPerConnection>,
}

impl TlsConfig {
    /// Override the ALPN to only advertise HTTP/1.1 (for WebSocket connections
    /// that don't support HTTP/2).
    pub fn set_alpn_http1_only(&mut self) {
        let alpn_wire = vec![0x08, b'h', b't', b't', b'p', b'/', b'1', b'.', b'1'];
        match &mut self.stealth_per_connection {
            Some(pc) => pc.alpn_wire = Some(alpn_wire),
            None => {
                self.stealth_per_connection = Some(StealthPerConnection {
                    sigalg_list: None,
                    alpn_wire: Some(alpn_wire),
                    curves_list: None,
                });
            }
        }
    }
}

/// Per-connection TLS settings that must be applied via SSL_set_* functions
/// (BoringSSL does not expose SSL_CTX_set_* for these).
#[derive(Clone, Debug)]
pub struct StealthPerConnection {
    /// Signature algorithms as OpenSSL name strings (e.g., "rsa_pss_rsae_sha256:rsa_pkcs1_sha256").
    pub sigalg_list: Option<String>,
    /// ALPN protocols in wire format (length-prefixed).
    pub alpn_wire: Option<Vec<u8>>,
    /// Supported groups as OpenSSL name strings (e.g., "X25519:P-256:P-384").
    pub curves_list: Option<String>,
}

/// Stable in-process discriminator for a [`StealthPerConnection`]: used to
/// salt the TLS session-resumption origin key so sessions established under
/// one TLS parameter set are never offered under another (offering a
/// session short-circuits parameter negotiation — see
/// bao_boringssl_bridge::session_cache). `DefaultHasher` values are not
/// stable across compiler versions, which is fine: the salt only needs to be
/// consistent within one process.
fn stealth_pc_salt(pc: &StealthPerConnection) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pc.sigalg_list.hash(&mut h);
    pc.alpn_wire.hash(&mut h);
    pc.curves_list.hash(&mut h);
    h.finish()
}

/// Create a [`TlsConfig`] to use for managing an HTTP connection.
///
/// Builds a BoringSSL `SSL_CTX` with the appropriate cipher suites,
/// curves, signature algorithms, and ALPN from the stealth configuration.
/// Certificate verification uses BoringSSL's built-in system root CA store
/// by default (same as Chrome).
///
/// FIXME: The `ignore_certificate_errors` argument ignores all certificate errors. This
/// is used when running the WPT tests, because BoringSSL currently rejects the WPT certificate.
#[servo_tracing::instrument(skip_all)]
pub fn create_tls_config(
    _ca_certificates: CACertificates,
    ignore_certificate_errors: bool,
    _override_manager: CertificateErrorOverrideManager,
) -> TlsConfig {
    // Build the BoringSSL TlsClient
    let (client, stealth_per_connection) = match get_stealth_tls_config() {
        Some(stealth) => {
            // Use stealth profile to configure cipher suites
            let client = TlsClient::new()
                .expect("Failed to create BoringSSL TlsClient");
            let ctx = client.ctx();

            // Build the TLS 1.2 cipher list string from the stealth config.
            // SSL_CTX_set_cipher_list sets the default for all connections.
            // TLS 1.3 suites are omitted: their order is built into BoringSSL
            // (no set_ciphersuites API in this build).
            let cipher_str = boringssl_cipher_list_string(&stealth.tls12_cipher_suites);

            if !cipher_str.is_empty() {
                let cipher_c = std::ffi::CString::new(cipher_str)
                    .expect("invalid cipher string");
                // SAFETY: SSL_CTX_set_cipher_list sets the cipher list on the SSL_CTX.
                // The CString is valid for the duration of this call. The ctx pointer is
                // valid because we just obtained it from the TlsClient.
                let ok = unsafe { SSL_CTX_set_cipher_list(ctx, cipher_c.as_ptr()) };
                if ok == 0 {
                    warn!("BoringSSL: SSL_CTX_set_cipher_list failed for stealth config");
                }
            }

            // Prepare per-connection settings (BoringSSL only has SSL_set_* for these)
            let sigalg_list = if !stealth.signature_algorithms.is_empty() {
                let sigalgs = boringssl_sigalgs_list_string(&stealth.signature_algorithms);
                if !sigalgs.is_empty() {
                    Some(sigalgs)
                } else {
                    None
                }
            } else {
                None
            };

            let alpn_wire = if !stealth.alpn_protocols.is_empty() {
                let mut wire: Vec<u8> = Vec::new();
                for proto in &stealth.alpn_protocols {
                    wire.push(proto.len() as u8);
                    wire.extend_from_slice(proto);
                }
                Some(wire)
            } else {
                None
            };

            // FFDHE groups are filtered by the shared builder: a single
            // unrecognized group name makes SSL_set1_curves_list fail the
            // whole call, silently discarding the groups fingerprint.
            let curves_list = if !stealth.supported_groups.is_empty() {
                let curves = boringssl_curves_list_string(&stealth.supported_groups);
                if !curves.is_empty() {
                    Some(curves)
                } else {
                    None
                }
            } else {
                None
            };

            (client, Some(StealthPerConnection {
                sigalg_list,
                alpn_wire,
                curves_list,
            }))
        }
        None => {
            let client = TlsClient::new()
                .expect("Failed to create BoringSSL TlsClient");
            (client, None)
        }
    };

    TlsConfig {
        client,
        ignore_certificate_errors,
        stealth_per_connection,
    }
}

// ── Tokio executor ────────────────────────────────────────────────────

#[derive(Clone)]
struct TokioExecutor {}

impl<F> Executor<F> for TokioExecutor
where
    F: Future<Output = ()> + 'static + std::marker::Send,
{
    fn execute(&self, fut: F) {
        spawn_task(fut);
    }
}

/// Prewarm the TLS stack to speed up the first connection.
///
/// Currently, this initializes BoringSSL via `bun_boringssl::load()`,
/// which on some systems can take a few milliseconds.
#[inline]
pub fn prewarm_tls() {
    #[servo_tracing::instrument]
    fn prewarm_tls_impl() {
        bun_boringssl::load();
    }

    if let Err(error) = std::thread::Builder::new()
        .name("Net-TLS-prewarm".into())
        .spawn(prewarm_tls_impl)
    {
        warn!("Failed to spawn thread to prewarm TLS: {error:?}");
    }
}

// ── Error types ──────────────────────────────────────────────────────

pub type BoxedBody = BoxBody<Bytes, hyper::Error>;

#[derive(Debug)]
/// The error type for the MaybeProxyConnector
pub enum ConnectionError {
    HttpError(String),
    // It looks like currently the type is not exported.
    ProxyError(String),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ConnectionError {}

// ── Proxy connector ──────────────────────────────────────────────────

#[derive(Clone)]
/// A proxy connector. This will automatically open a proxy connection if the uri matches the proxy uri.
/// Also respects 'no_proxy'.
pub struct ProxyConnector {
    /// A client without proxy for `no_proxy` matches.
    client: ServoHttpConnector,
    /// Matcher to see if we should forward to the proxy or not.
    matcher: std::sync::Arc<hyper_util::client::proxy::matcher::Matcher>,
}

impl ProxyConnector {
    fn new() -> Self {
        let matcher_builder = hyper_util::client::proxy::matcher::Matcher::builder()
            .http(servo_config::pref!(network_http_proxy_uri))
            .https(servo_config::pref!(network_https_proxy_uri))
            .no(servo_config::pref!(network_http_no_proxy));
        ProxyConnector {
            client: ServoHttpConnector::new(),
            matcher: std::sync::Arc::new(matcher_builder.build()),
        }
    }
}

// Just forward everything to the inner type except that we modify the errors returned.
impl Service<Destination> for ProxyConnector {
    type Response = TokioIo<TcpStream>;
    type Error = ConnectionError;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<TokioIo<TcpStream>, ConnectionError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.client
            .poll_ready(cx)
            .map_err(|e| ConnectionError::ProxyError(format!("{e}")))
    }

    fn call(&mut self, req: Destination) -> Self::Future {
        match self.matcher.intercept(&req) {
            Some(intercept) => Box::pin(
                Tunnel::new(intercept.uri().clone(), self.client.clone())
                    .call(req)
                    .map_err(|e| ConnectionError::ProxyError(format!("{e}"))),
            ),
            None => Box::pin(
                self.client
                    .call(req)
                    .map_err(|e| ConnectionError::ProxyError(format!("{e}"))),
            ),
        }
    }
}

pub type ServoClient = Client<BoringsslHttpsConnector, BoxedBody>;

pub fn create_http_client(tls_config: TlsConfig) -> ServoClient {
    let stealth = get_stealth_tls_config();

    let connector = BoringsslHttpsConnector::new(
        tls_config.client,
        tls_config.ignore_certificate_errors,
        tls_config.stealth_per_connection,
    );

    let mut builder = Client::builder(TokioExecutor {});
    builder.http1_title_case_headers(true);

    // Apply stealth HTTP/2 fingerprint settings if configured.
    //
    // hyper's builder supports window sizes, max frame size, and max header list
    // size. Other SETTINGS parameters (header_table_size, max_concurrent_streams)
    // are NOT configurable via hyper's builder API and would require a custom h2
    // connection wrapper for full AKAMAI fingerprint matching.
    if let Some(s) = stealth {
        builder
            .http2_initial_stream_window_size(s.h2_initial_stream_size)
            .http2_initial_connection_window_size(s.h2_initial_connection_window_size)
            .http2_max_frame_size(s.h2_max_frame_size)
            .http2_max_header_list_size(s.h2_max_header_list_size);
    }

    builder.build(connector)
}
