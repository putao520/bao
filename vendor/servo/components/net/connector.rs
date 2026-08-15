/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// Bao vendor patch: this module carries the boringssl-backed stealth TLS
// configuration face (REQ-STL-001) shared by the page-network bun bridge
// and the WebSocket loader. The hyper client/connector machinery that used
// to live here was removed with the hyper escape hatch (U2 terminal): the
// bun bridge is the only page-network path.
#![allow(unsafe_code)]

use std::collections::hash_map::HashMap;
use std::sync::{Arc, RwLock};

use http_body_util::combinators::BoxBody;
use hyper::body::Bytes;
use log::warn;
use parking_lot::Mutex;

use bao_boringssl_bridge::TlsClient;
use bao_stealth::{boringssl_cipher_list_string, boringssl_curves_list_string, boringssl_sigalgs_list_string};
use bun_boringssl_sys::boringssl::*;

// Verification symbol compiled into the vendored BoringSSL library but not
// declared in the hand-rolled bindings (same pattern as
// bao_boringssl_bridge/src/client.rs). Ground truth: vendor/boringssl/include/openssl.
unsafe extern "C" {
    /// Load the system default trust paths (OPENSSLDIR bundle + hash dir)
    /// into the ctx's store.
    fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> core::ffi::c_int;
}

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
/// algorithms, and ALPN protocols to the BoringSSL `SSL_CTX` (consumed by
/// the WebSocket loader; the page-network bridge reads the same global via
/// `get_stealth_tls_config()` and shapes its `SSLConfig` from it).
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
/// When `Some`, `create_tls_config()` uses these values to shape the TLS
/// ClientHello; the page-network bridge shapes its per-request
/// `bun_http::ssl_config::SSLConfig` from the same snapshot.
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
pub(crate) fn get_stealth_tls_config() -> Option<StealthTlsWireConfig> {
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

/// TLS handshake snapshot surfaced to servo (`TlsSecurityInfo` downstream).
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

    /// Record a certificate that failed verification, keyed by hostname, so
    /// the fetch error path can surface it as `SslValidation` instead of a
    /// generic connection error.
    pub(crate) fn record_certificate_failing_verification(&self, host: &str, der: &[u8]) {
        self.0
            .lock()
            .certificates_failing_to_verify
            .insert(host.to_string(), der.to_vec());
    }

    /// Whether `der` was explicitly overridden (user accepted this exact
    /// certificate despite the verification failure).
    pub(crate) fn has_override(&self, der: &[u8]) -> bool {
        self.0.lock().overrides.iter().any(|o| o == der)
    }

    /// The explicitly-accepted override certificates (DER), cloned out.
    /// U2 bridge consumer: the bun-bridge fetch driver trusts these in its
    /// per-request verify store so an accepted certificate bypasses chain
    /// verification exactly as the hyper-era connector's per-connection
    /// `has_override(leaf)` check did. Read-only accessor — no behavior
    /// change in this manager.
    pub(crate) fn override_certs(&self) -> Vec<Vec<u8>> {
        self.0.lock().overrides.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub enum CACertificates {
    #[default]
    Default,
    Override(Vec<Vec<u8>>),
}

// ── TLS config ────────────────────────────────────────────────────────

/// BoringSSL TLS configuration for servo's TLS consumers.
///
/// Wraps a `TlsClient` (which owns an `SSL_CTX`) along with certificate
/// verification settings. Consumed by the WebSocket loader
/// (`start_websocket`); the page-network bridge reads the global stealth
/// wire config directly instead.
#[derive(Clone)]
pub struct TlsConfig {
    pub client: TlsClient,
    pub ignore_certificate_errors: bool,
    /// Per-connection settings from stealth profile (applied on each new TLS connection
    /// because BoringSSL only provides SSL_set_* variants, not SSL_CTX_set_*).
    pub stealth_per_connection: Option<StealthPerConnection>,
    /// Certificate-override bookkeeping shared with the fetch error path:
    /// failing certificates are recorded per host (→ `SslValidation` errors)
    /// and previously accepted certificates bypass verification.
    pub override_manager: CertificateErrorOverrideManager,
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
    ca_certificates: CACertificates,
    ignore_certificate_errors: bool,
    override_manager: CertificateErrorOverrideManager,
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
                warn!("BoringSSL: SSL_CTX_set_default_verify_paths failed — HTTPS verification will fail closed");
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

    TlsConfig {
        client,
        ignore_certificate_errors,
        stealth_per_connection,
        override_manager,
    }
}

// ── Error types ──────────────────────────────────────────────────────

pub type BoxedBody = BoxBody<Bytes, hyper::Error>;
