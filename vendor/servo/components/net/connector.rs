/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::RefCell;
use std::collections::hash_map::HashMap;
use std::convert::TryFrom;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use std::{fmt, io};

use futures::task::{Context, Poll};
use futures::{Future, TryFutureExt};
use http::uri::{Authority, Uri as Destination};
use http_body_util::combinators::BoxBody;
use hyper::body::Bytes;
use hyper::rt::Executor;
use hyper_rustls::{HttpsConnector as HyperRustlsHttpsConnector, MaybeHttpsStream};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{
    Connected, Connection, HttpConnector as HyperHttpConnector,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use log::warn;
use parking_lot::Mutex;
use rustls::client::danger::ServerCertVerifier;
use rustls::client::{ClientConnection, EchStatus};
use rustls::crypto::{CryptoProvider, aws_lc_rs};
use rustls::{CipherSuite, ClientConfig, NamedGroup, ProtocolVersion};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use servo_base::id::WebViewId;
use servo_config::pref;
use tokio::net::TcpStream;
use tower::Service;

use bao_boringssl_bridge::TlsClient;
use bao_stealth::{
    boringssl_cipher_list_string, boringssl_curves_list_string, boringssl_sigalgs_list_string,
};
use bun_boringssl_sys::boringssl::*;

// Verification symbol compiled into the vendored BoringSSL library but not
// declared in the hand-rolled bindings (same pattern as
// bao_boringssl_bridge/src/client.rs). Ground truth: vendor/boringssl/include/openssl.
#[allow(unsafe_code)]
unsafe extern "C" {
    /// Load the system default trust paths (OPENSSLDIR bundle + hash dir)
    /// into the ctx's store.
    fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> core::ffi::c_int;
}

use crate::async_runtime::spawn_task;
use crate::hosts::replace_host;

// ── Stealth TLS/HTTP2 wire configuration ──────────────────────────────
//
// Bao vendor patch (REQ-STL-001). Wire-level configuration for servo's
// TLS/HTTP2 stack, set by the embedder (Bao) during stealth profile
// initialization. Follows the same pattern as
// `servo_canvas::canvas_noise::set_global_canvas_noise()` — global static,
// set once at init time, read on every connection.
//
// The struct is re-declared here (instead of importing from `bao_stealth`)
// to avoid widening the servo `net` crate's contract with the bao layer;
// fields must be kept in sync with `bao_stealth::StealthTlsWireConfig`.
// Consumers: the page-network bun bridge (`net/fetch/bun_bridge.rs`) shapes
// its `SSLConfig` from these values and the WebSocket loader's TLS config —
// the boringssl stealth connector face replays onto the same registry.

/// Wire-level TLS/HTTP2 configuration for servo's network layer.
///
/// When set, TLS consumers apply cipher suites, curves, signature
/// algorithms, and ALPN protocols to the BoringSSL `SSL_CTX` (WebSocket
/// loader; the page-network bridge reads the same config via
/// [`resolve_stealth_tls_config`] and shapes its `SSLConfig` from it).
///
/// BoringSSL supports full JA3/JA4 fingerprint configuration including
/// cipher suite reordering, curves/groups ordering, and signature algorithm
/// ordering.
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
    /// HTTP/2 initial stream window size.
    pub h2_initial_stream_size: u32,
    /// HTTP/2 initial connection window size.
    pub h2_initial_connection_window_size: u32,
    /// HTTP/2 SETTINGS_MAX_FRAME_SIZE.
    pub h2_max_frame_size: u32,
    /// HTTP/2 SETTINGS_MAX_HEADER_LIST_SIZE.
    pub h2_max_header_list_size: u32,
}

/// Global stealth TLS/HTTP2 wire configuration set by the embedder (Bao).
/// When `Some`, TLS consumers use these values to shape the TLS
/// ClientHello; the page-network bridge shapes its per-request
/// `bun_http::ssl_config::SSLConfig` from the same snapshot.
static STEALTH_TLS_CONFIG: std::sync::RwLock<Option<StealthTlsWireConfig>> =
    std::sync::RwLock::new(None);

/// Set the global stealth TLS/HTTP2 configuration.
///
/// Called by Bao's runtime bridge during stealth profile initialization,
/// following the same pattern as `servo::set_canvas_noise_seed()`.
pub fn set_stealth_tls_config(config: Option<StealthTlsWireConfig>) {
    let mut guard = STEALTH_TLS_CONFIG.write().unwrap();
    *guard = config;
}

/// Read the current global stealth TLS/HTTP2 configuration.
pub(crate) fn get_stealth_tls_config() -> Option<StealthTlsWireConfig> {
    STEALTH_TLS_CONFIG.read().unwrap().clone()
}

// ── Per-WebViewId stealth wire-config registry (R53-A net face) ─────────
//
// The process-global above is a LAST-WRITE-WINS snapshot: with more than
// one page carrying different stealth profiles, every page install
// overwrote the whole process's wire config, so ALL pages' subsequent
// connections rode the LAST-created page's fingerprint (silent
// cross-contamination inside a single runtime — BUN-EVOLUTION R53). The
// registries below key the SAME two faces (TLS wire config + the h2
// fingerprint snapshot the bun bridge reads) by the request's
// `target_webview_id`, which `net_traits::request::Request` already
// carries end-to-end:
//
//   keyed hit    → that entry is AUTHORITATIVE, including an explicit
//                  `None` (a stealth-free page must not inherit another
//                  page's profile through the fallback);
//   miss / None  → the process-global above (embedder-set default; the
//                  pre-R53 behavior, preserved for identity-less
//                  infrastructure fetches such as SW script updates).
//
// Dedicated/shared-worker egress carries the owning page's webview id
// natively (`GlobalScope::webview_id`); service-worker egress is stamped
// with the REGISTERING page's webview id by the script-side vendor patch
// (`GlobalScope::egress_webview_id`), so worker/SW realms resolve to
// their host page's profile.

static STEALTH_TLS_BY_WEBVIEW: LazyLock<
    std::sync::RwLock<HashMap<WebViewId, Option<StealthTlsWireConfig>>>,
> = LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

static STEALTH_H2_BY_WEBVIEW: LazyLock<
    std::sync::RwLock<HashMap<WebViewId, Option<bao_stealth::Http2Fingerprint>>>,
> = LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

/// Re-export so the `servo` facade can name the h2 fingerprint type in the
/// keyed setter without a direct `bao_stealth` dependency of its own (the
/// net crate already depends on it).
pub use bao_stealth::Http2Fingerprint;

/// Upsert one webview's stealth wire configuration (TLS + HTTP/2 faces
/// together — the embedder always installs both from a single profile).
pub fn set_stealth_wire_config_for_webview(
    webview_id: WebViewId,
    tls: Option<StealthTlsWireConfig>,
    h2: Option<Http2Fingerprint>,
) {
    STEALTH_TLS_BY_WEBVIEW
        .write()
        .unwrap()
        .insert(webview_id, tls);
    STEALTH_H2_BY_WEBVIEW
        .write()
        .unwrap()
        .insert(webview_id, h2);
}

/// Drop a webview's keyed entries (page close). Webview ids are not
/// reused within a process, but the entries pin profile memory otherwise.
pub fn clear_stealth_wire_config_for_webview(webview_id: WebViewId) {
    STEALTH_TLS_BY_WEBVIEW.write().unwrap().remove(&webview_id);
    STEALTH_H2_BY_WEBVIEW.write().unwrap().remove(&webview_id);
}

/// Resolve the stealth TLS wire config for a request's webview identity.
/// A keyed hit is authoritative (explicit `None` included — stealth-free
/// pages stay stealth-free); a miss or an identity-less request falls
/// back to the process-global default. Consumed by the page-network bun
/// bridge (`net/fetch/bun_bridge.rs`, module wiring owned by the net-face
/// replay slice).
#[allow(dead_code)]
pub(crate) fn resolve_stealth_tls_config(
    webview_id: Option<WebViewId>,
) -> Option<StealthTlsWireConfig> {
    match webview_id {
        Some(id) => STEALTH_TLS_BY_WEBVIEW
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(get_stealth_tls_config),
        None => get_stealth_tls_config(),
    }
}

/// Resolve the HTTP/2 fingerprint snapshot for a webview identity (same
/// resolution semantics as [`resolve_stealth_tls_config`]; falls back to
/// `bao_stealth`'s process-global snapshot). Consumed by the page-network
/// bun bridge (see above).
#[allow(dead_code)]
pub(crate) fn resolve_http2_fingerprint(
    webview_id: Option<WebViewId>,
) -> Option<Http2Fingerprint> {
    match webview_id {
        Some(id) => STEALTH_H2_BY_WEBVIEW
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(bao_stealth::global_http2_fingerprint),
        None => bao_stealth::global_http2_fingerprint(),
    }
}

// ── BAO PATCH (REQ-STL-001, e113 WS per-page wire): WebSocket TLS leg ──
//
// The page `wss://` path rides the Bao BoringSSL stack (stealth
// per-connection fingerprint + process-wide session cache), NOT the
// upstream rustls connector above — same split as the HTTP leg, where the
// page network rides `fetch::bun_bridge::obtain_response_bun`. The
// per-page identity comes from `resolve_stealth_tls_config(webview_id)`:
// keyed hit is authoritative (explicit `None` = stealth-free page), miss
// falls back to the process global.
//
// Blueprint: the pre-snapshot-swap connector's `create_tls_config`
// 4-arg (webview_id) BoringSSL form; renamed here because the upstream
// rustls connector owns the `create_tls_config` name in this snapshot.

/// Per-connection settings from the stealth profile (applied on each new
/// TLS connection because BoringSSL only provides `SSL_set_*` variants,
/// not `SSL_CTX_set_*`).
pub struct StealthPerConnection {
    /// Signature algorithms as OpenSSL name strings (e.g.,
    /// "rsa_pss_rsae_sha256:rsa_pkcs1_sha256").
    pub sigalg_list: Option<String>,
    /// ALPN protocols in wire format (length-prefixed).
    pub alpn_wire: Option<Vec<u8>>,
    /// Supported groups as OpenSSL name strings (e.g., "X25519:P-256:P-384").
    pub curves_list: Option<String>,
}

/// TLS configuration for the WebSocket leg: the shared BoringSSL
/// `TlsClient` plus the stealth per-connection fingerprint fields the
/// `WsTlsStream` consumer applies per connection.
pub struct WsTlsConfig {
    pub client: TlsClient,
    pub ignore_certificate_errors: bool,
    pub stealth_per_connection: Option<StealthPerConnection>,
}

impl WsTlsConfig {
    /// Override the ALPN to only advertise HTTP/1.1 (for WebSocket
    /// connections that don't support HTTP/2).
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
            },
        }
    }
}

/// Create the WebSocket-leg [`WsTlsConfig`] for a request's webview
/// identity: the BoringSSL `TlsClient` with the per-page stealth cipher
/// list applied on the shared ctx, and the per-connection fingerprint
/// fields extracted for `WsTlsStream`.
#[allow(unsafe_code)]
pub fn create_ws_tls_config(
    webview_id: Option<WebViewId>,
    ca_certificates: CACertificates,
    ignore_certificate_errors: bool,
) -> WsTlsConfig {
    // Build the BoringSSL TlsClient
    let (client, stealth_per_connection) = match resolve_stealth_tls_config(webview_id) {
        Some(stealth) => {
            // Use stealth profile to configure cipher suites
            let client =
                TlsClient::new().expect("Failed to create BoringSSL TlsClient");
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
            let client =
                TlsClient::new().expect("Failed to create BoringSSL TlsClient");
            (client, None)
        }
    };

    // Trust store for peer verification. `Default` = system roots (what a
    // real browser trusts); an explicit override list (WPT / embedder-supplied
    // CAs) replaces it. Connections opt into verification at the consumer
    // (`SSL_VERIFY_PEER` per connection); the store must be populated here,
    // on the shared ctx.
    match ca_certificates {
        CACertificates::Default => {
            // SAFETY: client.ctx() is a live SSL_CTX; the call only mutates
            // its cert store. Errors leave the store as-is — verification
            // then fails closed against an empty store, never open.
            let ok = unsafe { SSL_CTX_set_default_verify_paths(client.ctx()) };
            if ok != 1 {
                warn!("BoringSSL: SSL_CTX_set_default_verify_paths failed — verification will fail closed");
            }
        },
        CACertificates::Override(certificates) => {
            for der in certificates {
                if !client.add_trusted_der(&der) {
                    warn!("BoringSSL: embedder CA certificate could not be parsed (DER) — skipped");
                }
            }
        },
    }

    WsTlsConfig {
        client,
        ignore_certificate_errors,
        stealth_per_connection,
    }
}

pub const BUF_SIZE: usize = 32768;

/// ALPN identifier for HTTP/2 (RFC 7540 §3.1).
pub const ALPN_H2: &str = "h2";

#[derive(Clone)]
pub struct ServoHttpConnector {
    inner: HyperHttpConnector,
}

impl ServoHttpConnector {
    fn new() -> ServoHttpConnector {
        let mut inner = HyperHttpConnector::new();
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

#[derive(Clone)]
pub struct InstrumentedConnector<T> {
    inner: HyperRustlsHttpsConnector<T>,
}

impl<T> InstrumentedConnector<T> {
    fn new(inner: HyperRustlsHttpsConnector<T>) -> Self {
        Self { inner }
    }
}

impl<T> From<HyperRustlsHttpsConnector<T>> for InstrumentedConnector<T> {
    fn from(inner: HyperRustlsHttpsConnector<T>) -> Self {
        Self::new(inner)
    }
}

pub struct InstrumentedStream<T> {
    inner: MaybeHttpsStream<T>,
    tls_info: RefCell<Option<TlsHandshakeInfo>>,
}

impl<T: Unpin> Unpin for InstrumentedStream<T> {}

impl<T> fmt::Debug for InstrumentedStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstrumentedStream")
            .field("tls_info", &self.tls_info)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct TlsHandshakeInfo {
    pub protocol_version: Option<ProtocolVersion>,
    pub cipher_suite: Option<CipherSuite>,
    pub kea_group_name: Option<NamedGroup>,
    pub signature_scheme_name: Option<String>,
    pub alpn_protocol: Option<String>,
    pub certificate_chain_der: Vec<Vec<u8>>,
    pub used_ech: bool,
}

impl TlsHandshakeInfo {
    fn from_connection(conn: &ClientConnection) -> Self {
        let protocol_version = conn.protocol_version();
        let cipher_suite = conn.negotiated_cipher_suite().map(|suite| suite.suite());
        let kea_group_name = conn
            .negotiated_key_exchange_group()
            .map(|group| group.name());
        let certificate_chain_der = conn
            .peer_certificates()
            .map(|certs| certs.iter().map(|cert| cert.as_ref().to_vec()).collect())
            .unwrap_or_default();
        let alpn_protocol = conn
            .alpn_protocol()
            .map(|proto| String::from_utf8_lossy(proto).into_owned());
        let used_ech = matches!(conn.ech_status(), EchStatus::Accepted);

        Self {
            protocol_version,
            cipher_suite,
            kea_group_name,
            signature_scheme_name: None,
            alpn_protocol,
            certificate_chain_der,
            used_ech,
        }
    }
}

impl<T> InstrumentedStream<T>
where
    T: Connection + hyper::rt::Read + hyper::rt::Write + Unpin,
{
    fn from_maybe_https_stream(stream: MaybeHttpsStream<T>) -> Self {
        match stream {
            MaybeHttpsStream::Http(inner) => Self {
                inner: MaybeHttpsStream::Http(inner),
                tls_info: RefCell::new(None),
            },
            MaybeHttpsStream::Https(tls_stream) => {
                let (_tcp, tls) = tls_stream.inner().get_ref();
                let tls_info = TlsHandshakeInfo::from_connection(tls);

                Self {
                    inner: MaybeHttpsStream::Https(tls_stream),
                    tls_info: RefCell::new(Some(tls_info)),
                }
            },
        }
    }
}

impl<T> Connection for InstrumentedStream<T>
where
    T: Connection + hyper::rt::Read + hyper::rt::Write + Unpin,
{
    fn connected(&self) -> Connected {
        let connected = match &self.inner {
            MaybeHttpsStream::Http(stream) => stream.connected(),
            MaybeHttpsStream::Https(stream) => {
                let (tcp, tls) = stream.inner().get_ref();
                if tls.alpn_protocol() == Some(ALPN_H2.as_bytes()) {
                    tcp.inner().connected().negotiated_h2()
                } else {
                    tcp.inner().connected()
                }
            },
        };
        if let Some(info) = self.tls_info.borrow_mut().take() {
            connected.extra(info)
        } else {
            connected
        }
    }
}

impl<T> hyper::rt::Read for InstrumentedStream<T>
where
    T: Connection + hyper::rt::Read + hyper::rt::Write + Unpin,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<T> hyper::rt::Write for InstrumentedStream<T>
where
    T: Connection + hyper::rt::Read + hyper::rt::Write + Unpin,
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl<T> Service<Destination> for InstrumentedConnector<T>
where
    T: Service<Destination>,
    T::Response: Connection + hyper::rt::Read + hyper::rt::Write + Send + Unpin + 'static,
    T::Future: Send + 'static,
    T::Error: Into<BoxError>,
{
    type Response = InstrumentedStream<T::Response>;
    type Error = BoxError;
    type Future = std::pin::Pin<
        Box<dyn Future<Output = Result<InstrumentedStream<T::Response>, BoxError>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Destination) -> Self::Future {
        let future = self.inner.call(dst);
        Box::pin(async move {
            let stream = future.await.map_err(|error| -> BoxError { error })?;
            Ok(InstrumentedStream::from_maybe_https_stream(stream))
        })
    }
}

pub type Connector = InstrumentedConnector<ServoHttpConnector>;
pub type TlsConfig = ClientConfig;

#[derive(Clone, Debug, Default)]
struct CertificateErrorOverrideManagerInternal {
    /// A mapping of certificates and their hosts, which have seen certificate errors.
    /// This is used to later create an override in this [CertificateErrorOverrideManager].
    certificates_failing_to_verify: HashMap<ServerName<'static>, CertificateDer<'static>>,
    /// A list of certificates that should be accepted despite encountering verification
    /// errors.
    overrides: Vec<CertificateDer<'static>>,
}

/// This data structure is used to track certificate verification errors and overrides.
/// It tracks:
///  - A list of [Certificate]s with verification errors mapped by their [ServerName]
///  - A list of [Certificate]s for which to ignore verification errors.
#[derive(Clone, Debug, Default)]
pub struct CertificateErrorOverrideManager(Arc<Mutex<CertificateErrorOverrideManagerInternal>>);

impl CertificateErrorOverrideManager {
    pub fn new() -> Self {
        Self(Default::default())
    }

    /// Add a certificate to this manager's list of certificates for which to ignore
    /// validation errors.
    pub fn add_override(&self, certificate: &CertificateDer<'static>) {
        self.0.lock().overrides.push(certificate.clone());
    }

    /// Given the a string representation of a sever host name, remove information about
    /// a [Certificate] with verification errors. If a certificate with
    /// verification errors was found, return it, otherwise None.
    pub(crate) fn remove_certificate_failing_verification(
        &self,
        host: &str,
    ) -> Option<CertificateDer<'static>> {
        let server_name = match ServerName::try_from(host) {
            Ok(name) => name.to_owned(),
            Err(error) => {
                warn!("Could not convert host string into RustTLS ServerName: {error:?}");
                return None;
            },
        };
        self.0
            .lock()
            .certificates_failing_to_verify
            .remove(&server_name)
    }

    // BAO PATCH (U2 bun_bridge, restored accessor): all user-accepted override
    // certificates — the bun bridge folds them into each request's per-SSL
    // trust store (hyper parity pass-through). Upstream's rustls verifier
    // reads `overrides` internally (BaoVerifier above) and never needed a
    // getter; the bridge consumes it from outside the verifier.
    pub(crate) fn override_certs(&self) -> Vec<CertificateDer<'static>> {
        self.0.lock().overrides.clone()
    }

    // BAO PATCH (U2 bun_bridge, restored accessor): record a failing-host
    // certificate obtained out-of-band (the bridge's bounded direct TLS
    // probe) so the immediately-following
    // `remove_certificate_failing_verification` returns it. Upstream writes
    // this map only from inside the rustls verifier; the bridge's error
    // path needs the external write.
    pub(crate) fn record_certificate_failing_verification(&self, host: &str, der: &[u8]) {
        let server_name = match ServerName::try_from(host) {
            Ok(name) => name.to_owned(),
            Err(error) => {
                warn!("Could not convert host string into RustTLS ServerName: {error:?}");
                return;
            },
        };
        self.0
            .lock()
            .certificates_failing_to_verify
            .insert(server_name, CertificateDer::from(der.to_vec()));
    }
}

#[derive(Clone, Debug, Default)]
pub enum CACertificates<'de> {
    #[default]
    Default,
    Override(Vec<CertificateDer<'de>>),
}

/// Create a [TlsConfig] to use for managing a HTTP connection. This currently creates
/// a rustls [ClientConfig].
///
/// FIXME: The `ignore_certificate_errors` argument ignores all certificate errors. This
/// is used when running the WPT tests, because rustls currently rejects the WPT certificiate.
/// See <https://github.com/servo/servo/issues/30080>
#[servo_tracing::instrument(skip_all)]
pub fn create_tls_config(
    ca_certificates: CACertificates<'static>,
    ignore_certificate_errors: bool,
    override_manager: CertificateErrorOverrideManager,
) -> TlsConfig {
    let verifier = CertificateVerificationOverrideVerifier::new(
        ca_certificates,
        ignore_certificate_errors,
        override_manager,
    );
    // TODO: After <https://github.com/rustls/rustls-platform-verifier/pull/204> is merged,
    // `dangerous` can be removed.
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth()
}

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

static CRYPTO_PROVIDER_CACHE: LazyLock<Arc<CryptoProvider>> = LazyLock::new(|| {
    CryptoProvider::get_default()
        .cloned()
        // The embedder should have initialized the default crypto provider before
        // initializing servo, so this should never fail.
        .unwrap_or_else(|| {
            warn!("Default crypto provider not initialized before first access in connector.");
            Arc::new(aws_lc_rs::default_provider())
        })
});

/// A cache for the default rustls platform verifier.
///
/// Instantiating a new verifier can be expensive, since it can read through all certificates:
/// <https://github.com/rustls/rustls-platform-verifier/blob/996b1c903491641b17b3c9afb65d1352f6fc6b76/rustls-platform-verifier/src/verification/others.rs#L92>
static RUSTLS_PLATFORM_VERIFIER_CACHE: LazyLock<Arc<rustls_platform_verifier::Verifier>> =
    LazyLock::new(|| {
        Arc::new(
            rustls_platform_verifier::Verifier::new(CRYPTO_PROVIDER_CACHE.clone())
                .expect("Could not initialize platform certificate verifier"),
        )
    });

/// Prewarm the TLS stack to speed up the first connection
///
/// Currently, this force-seeds the crypto provider (from aws_lc_rs),
/// which on my system takes around 30-50ms according to samply, spent in
/// `tree_jitter_initialize_once`. If we don't call this function, then
/// the initialization will happen much later, on a tokio runtime thread.
#[inline]
pub fn prewarm_tls() {
    #[servo_tracing::instrument]
    fn prewarm_tls_impl() {
        let mut sink = [0u8; 32];
        // The first access can be slow, if the provider needs to gather entropy.
        let _ = CRYPTO_PROVIDER_CACHE.secure_random.fill(&mut sink);
        // Note: We don't need to explicitly force initialize RUSTLS_PLATFORM_VERIFIER_CACHE,
        // since the resource manager thread will do that during startup.
    }

    if let Err(error) = std::thread::Builder::new()
        .name("Net-TLS-prewarm".into())
        .spawn(prewarm_tls_impl)
    {
        warn!("Failed to spawn thread to prewarm TLS: {error:?}");
    }
}

#[derive(Debug)]
struct CertificateVerificationOverrideVerifier {
    main_verifier: Arc<dyn ServerCertVerifier>,
    ignore_certificate_errors: bool,
    override_manager: CertificateErrorOverrideManager,
}

impl CertificateVerificationOverrideVerifier {
    fn new(
        ca_certficates: CACertificates<'static>,
        ignore_certificate_errors: bool,
        override_manager: CertificateErrorOverrideManager,
    ) -> Self {
        // From <https://github.com/rustls/rustls-platform-verifier/blob/main/README.md>:
        // > Some manual setup is required, outside of cargo, to use this crate on
        // > Android. In order to use Android's certificate verifier, the crate needs to
        // > call into the JVM. A small Kotlin component must be included in your app's
        // > build to support rustls-platform-verifier.
        //
        // Since we cannot count on embedders to do this setup, just stick with webpki roots
        // on Android.
        let use_webpki_roots = cfg!(target_os = "android") || pref!(network_use_webpki_roots);
        let main_verifier = if !use_webpki_roots {
            let verifier = match ca_certficates {
                CACertificates::Default => RUSTLS_PLATFORM_VERIFIER_CACHE.clone(),
                // Android doesn't support `Verifier::new_with_extra_roots`, but currently Android
                // never uses the platform verifier at all.
                CACertificates::Override(_certificates) => {
                    #[cfg(target_os = "android")]
                    unreachable!("Android should always use the WebPKI verifier.");
                    #[cfg(not(target_os = "android"))]
                    {
                        let verifier = rustls_platform_verifier::Verifier::new_with_extra_roots(
                            _certificates,
                            CRYPTO_PROVIDER_CACHE.clone(),
                        )
                        .expect("Could not initialize platform certificate verifier");
                        Arc::new(verifier)
                    }
                },
            };
            verifier as Arc<dyn ServerCertVerifier>
        } else {
            let mut root_store =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            match ca_certficates {
                CACertificates::Default => {},
                CACertificates::Override(certificates) => {
                    for certificate in certificates {
                        if root_store.add(certificate).is_err() {
                            log::error!("Could not add an override certificate.");
                        }
                    }
                },
            }
            rustls::client::WebPkiServerVerifier::builder(root_store.into())
                .build()
                .expect("Could not initialize platform certificate verifier.")
                as Arc<dyn ServerCertVerifier>
        };

        Self {
            main_verifier,
            ignore_certificate_errors,
            override_manager,
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for CertificateVerificationOverrideVerifier {
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.main_verifier
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.main_verifier
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.main_verifier.supported_verify_schemes()
    }

    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let error = match self.main_verifier.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(result) => return Ok(result),
            Err(error) => error,
        };

        if self.ignore_certificate_errors {
            warn!("Ignoring certficate error: {error:?}");
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }

        // If there's an override for this certificate, just accept it.
        for cert_with_exception in &*self.override_manager.0.lock().overrides {
            if *end_entity == *cert_with_exception {
                return Ok(rustls::client::danger::ServerCertVerified::assertion());
            }
        }
        self.override_manager
            .0
            .lock()
            .certificates_failing_to_verify
            .insert(server_name.to_owned(), end_entity.clone().into_owned());
        Err(error)
    }
}

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
            Some(intercept) => {
                let mut tunnel = Tunnel::new(intercept.uri().clone(), self.client.clone());
                let final_tunnel = if let Some(auth) = intercept.basic_auth() {
                    tunnel.with_auth(auth.clone())
                } else {
                    tunnel
                }
                .call(req)
                .map_err(|e| ConnectionError::ProxyError(format!("{e}")));
                Box::pin(final_tunnel)
            },
            None => Box::pin(
                self.client
                    .call(req)
                    .map_err(|e| ConnectionError::ProxyError(format!("{e}"))),
            ),
        }
    }
}

pub type ServoClient = Client<InstrumentedConnector<ProxyConnector>, BoxedBody>;

pub fn create_http_client(tls_config: TlsConfig) -> ServoClient {
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(ProxyConnector::new());

    Client::builder(TokioExecutor {})
        .http1_title_case_headers(true)
        // This is necessary for hyper to reap unused idle keep-alive connections.
        .pool_timer(TokioTimer::new())
        // Other browsers control the maximum connections per host, tyipcally set to 6, but hyper
        // does not allow controlling this directly. Until we have code to manually do that, this
        // limit controls the maximum idle connections per host, which we still don't want to grow
        // without bound.
        .pool_max_idle_per_host(6)
        .build(InstrumentedConnector::from(connector))
}
