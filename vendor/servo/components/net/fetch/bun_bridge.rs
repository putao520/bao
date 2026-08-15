// Bao fusion (U2 page-network unification, phase 0 + stage 2): fetch driver
// that runs servo's page network through bun's `HTTPThread` (single epoll
// thread, usockets + boringssl stealth TLS) instead of hyper.
//
// Gate: `BAO_PAGE_NET_BUN` (default ON since U2 stage 3 — the page network
// rides the bridge; the hyper path in `http_loader.rs:obtain_response` is
// kept as the explicit escape hatch for one transition version).
// Unset routes every request destination through the bridge; `0`/`false`/
// `off`/`hyper` restores the hyper path for every destination; a comma list
// (`img,css`) routes only matching request destinations (phase 1 pilot).
//
// Threading model (mirrors `fetch_async.rs` FetchTasklet, rehosted on tokio):
//   - servo's fetch runs on a tokio net thread; this bridge schedules the
//     request on the HTTPThread via `HTTPThread::schedule` (the only
//     cross-thread entry point, lock-free MPSC + atomics — legal from here).
//   - the result callback (`on_http_done`) runs on the HTTPThread and is pure
//     Rust (no SM API, no JS state). It publishes the response head (status /
//     reason phrase / headers / TLS snapshot) into an `Arc<BridgeState>` slot
//     and streams body chunks through a tokio unbounded channel, waking the
//     awaiting future with `tokio::sync::Notify`.
//   - the response body is TRUE-streamed: `enable_response_body_streaming`
//     arms per-`on_data` delivery and every non-terminal callback drains the
//     shared body buffer + round-trips `schedule_response_body_drain`
//     (52634b89 contract). The tokio side re-exposes those bytes through a
//     `StreamBody` fed from the channel, so servo's `Decoder::detect(BoxBody,
//     ..)` consumption loop (`http_loader.rs` spawn_task/try_fold) is
//     unchanged — only the supply side became incremental (stage 2; phase 0
//     re-sliced a fully-buffered body into 16 KiB chunks).
//
// Semantics kept on the servo side (this bridge does NOT take over):
//   - redirects: `FetchRedirect::Manual` — bun returns the original 3xx and
//     servo's redirect loop (`http_loader.rs` http_fetch redirect handling)
//     stays the single source of truth.
//   - CORS / cache / HSTS / cookies: all servo-side, untouched.
//   - decompression: `disable_decompression = true` and servo's
//     Accept-Encoding header passes through verbatim; servo's Decoder does the
//     decoding, exactly as with hyper.
//
// Stage 2 (complete): devtools requestWillBeSent-equivalent message built at
// the same checkpoints with the same fields as the hyper path;
// ResponseStart timing + TLS handshake info (`BunTlsInfo` →
// `TlsHandshakeInfo` → servo's `TlsSecurityInfo` downstream); per-request CA
// override (servo `CACertificates::Default/Override` semantics, applied
// per-SSL via `SSLConfig::ca_certs_der`); non-canonical ReasonPhrase surfaced
// through an owned-bytes `hyper::ext::ReasonPhrase`; h2 fingerprint parity
// (SETTINGS payload + pseudo-header wire order + preface PRIORITY frames from
// the page profile's `Http2Fingerprint` snapshot). Request bodies stay
// buffered (servo's IPC body stream is drained before the request is
// scheduled; `http_loader.rs` owns the streaming-request-body semantics).
//
// Stage 3 (h2 coalescing): the per-request SSLConfig is interned through
// bun's `ssl_config::GlobalRegistry` — content-equal configs resolve to one
// pointer, which is what every bun_http pool key compares, so same-origin
// requests reuse one TLS/h2 connection (stream ids increment instead of a
// new connection per request).

use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc as StdArc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Once;
use std::time::Duration;

use bytes::Bytes;
use content_security_policy::percent_encoding::utf8_percent_encode;
use devtools_traits::ChromeToDevtoolsControlMsg;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use http_body_util::{BodyExt, StreamBody};
use hyper::Response as HyperResponse;
use hyper::body::Frame;
use hyper::ext::ReasonPhrase;
use ipc_channel::ipc::IpcSender;
use log::warn;
use net_traits::NetworkError;
use net_traits::ResourceAttribute;
use net_traits::request::{BodyChunkRequest, Destination};
use parking_lot::{Mutex, RwLock};
use servo_base::cross_process_instant::CrossProcessInstant;
use servo_base::id::{BrowsingContextId, PipelineId};
use servo_url::ServoUrl;
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::connector::StealthTlsWireConfig;
use crate::connector::TlsHandshakeInfo;
use crate::decoder::Decoder;
use crate::devtools::prepare_devtools_request;
use crate::fetch::methods::FetchContext;
use crate::http_loader::{FRAGMENT, BodyChunk, BodySink, obtain_response_setup_router_callback};

/// How often the awaiting future re-checks servo's cancellation listener while
/// waiting for the HTTPThread callback. The Notify wakes us the moment the
/// response head is ready; this tick only exists so an in-flight request can
/// be aborted promptly when servo cancels the fetch.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

// ──────────────────────────────────────────────────────────────────────────
// Flag (default ON — U2 stage 3)
//
// `BAO_PAGE_NET_BUN`:
//   - unset                      → every destination through the bridge
//                                  (stage 3 default; the page network is the
//                                  bun stack);
//   - `0` / `false` / `off` / `hyper`
//                                → hyper path for every destination (explicit
//                                  escape hatch, deprecated — one transition
//                                  version, then the hyper path is removed);
//   - `1` / `true` / `all`       → every destination through the bridge;
//   - comma list, e.g. `img,css` → only matching request destinations
//                                  through the bridge (phase 1 pilot).
//
// List tokens are fetch-spec destination names (`image`, `style`,
// `document`, …) plus the pilot aliases `img`, `css`, `js` and `xhr` (XHR
// requests carry `Destination::None`). Unknown tokens warn and are ignored;
// a list that parses to the empty set leaves the bridge off.
// ──────────────────────────────────────────────────────────────────────────

/// Effective bridge scope. `Destinations` is the phase 1 pilot shape
/// (`img,css`); `All` is the phase 2 end state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageNetBunMode {
    Off,
    All,
    Destinations(Vec<Destination>),
}

static PAGE_NET_BUN_MODE: RwLock<Option<PageNetBunMode>> = RwLock::new(None);
static PAGE_NET_BUN_ENV_READ: Once = Once::new();

/// Number of requests dispatched through the bridge (incremented at
/// `obtain_response_bun` entry, before any I/O). Diagnostics / e2e pilot
/// assertions: proves which requests actually took the bun path.
static PAGE_NET_BUN_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// Parse a comma-separated destination list. Aliases map to fetch-spec
/// names first, then `Destination::from_str` (the csp crate's closed table)
/// decides validity — no bespoke name matching to drift out of sync.
fn parse_destination_list(value: &str) -> Vec<Destination> {
    let mut destinations = Vec::new();
    for raw in value.split(',') {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        let canonical = match token.as_str() {
            "img" => "image",
            "css" => "style",
            "js" => "script",
            "xhr" => "",
            other => other,
        };
        match Destination::from_str(canonical) {
            Ok(destination) => {
                if !destinations.contains(&destination) {
                    destinations.push(destination);
                }
            },
            Err(_) => warn!("BAO_PAGE_NET_BUN: unknown destination token '{raw}' (ignored)"),
        }
    }
    destinations
}

/// Parse one `BAO_PAGE_NET_BUN` value (env string or runtime-override
/// spec): `0`/`false`/`off`/`hyper` → off (the deprecated hyper escape
/// hatch), `1`/`true`/`all` → every destination, anything else → a
/// destination list. Pure — no global state, directly unit-testable.
/// Note the *unset* default (bridge ON) lives in [`parse_env_mode`]; an
/// explicitly empty value still parses to off.
pub fn parse_page_net_bun_spec(value: &str) -> PageNetBunMode {
    let trimmed = value.trim();
    if trimmed.is_empty() ||
        trimmed.eq_ignore_ascii_case("0") ||
        trimmed.eq_ignore_ascii_case("false") ||
        trimmed.eq_ignore_ascii_case("off") ||
        trimmed.eq_ignore_ascii_case("hyper")
    {
        return PageNetBunMode::Off;
    }
    if trimmed == "1" || trimmed.eq_ignore_ascii_case("true") || trimmed.eq_ignore_ascii_case("all")
    {
        return PageNetBunMode::All;
    }
    let destinations = parse_destination_list(value);
    if destinations.is_empty() {
        warn!("BAO_PAGE_NET_BUN='{value}' parsed to an empty destination set — bridge stays off");
        return PageNetBunMode::Off;
    }
    PageNetBunMode::Destinations(destinations)
}

fn parse_env_mode() -> PageNetBunMode {
    match std::env::var("BAO_PAGE_NET_BUN") {
        Ok(value) => parse_page_net_bun_spec(&value),
        // U2 stage 3: unset → the page network rides the bridge (hyper is
        // the explicit `off` escape hatch, deprecated).
        Err(_) => PageNetBunMode::All,
    }
}

/// Resolve the effective mode: the runtime override wins; otherwise the env
/// value is read exactly once (first call). Default (unset):
/// [`PageNetBunMode::All`] — the bridge; `BAO_PAGE_NET_BUN=off` restores
/// the deprecated hyper path.
fn effective_mode() -> PageNetBunMode {
    PAGE_NET_BUN_ENV_READ.call_once(|| {
        let mut guard = PAGE_NET_BUN_MODE.write();
        if guard.is_none() {
            *guard = Some(parse_env_mode());
        }
    });
    PAGE_NET_BUN_MODE
        .read()
        .clone()
        .unwrap_or(PageNetBunMode::Off)
}

/// Dispatch predicate for the `http_network_fetch` cut point: does this
/// request destination go through the bun bridge?
pub fn page_net_bun_enabled_for(destination: Destination) -> bool {
    match effective_mode() {
        PageNetBunMode::Off => false,
        PageNetBunMode::All => true,
        PageNetBunMode::Destinations(destinations) => destinations.contains(&destination),
    }
}

/// The current mode (diagnostics).
pub fn page_net_bun_mode() -> PageNetBunMode {
    effective_mode()
}

/// Override the bridge flag at runtime (embedder / tests): `true` = every
/// destination, `false` = off. Does not affect already-in-flight requests.
pub fn set_page_net_bun_enabled(enabled: bool) {
    *PAGE_NET_BUN_MODE.write() = Some(if enabled {
        PageNetBunMode::All
    } else {
        PageNetBunMode::Off
    });
}

/// Override the bridge flag with a destination list at runtime (embedder /
/// tests) — same token syntax as the env value (e.g. `"img,css"`; `1`/`true`
/// also accepted). An empty/invalid parse leaves the bridge off.
pub fn set_page_net_bun_destinations(spec: &str) {
    *PAGE_NET_BUN_MODE.write() = Some(parse_page_net_bun_spec(spec));
}

/// How many requests have been dispatched through the bridge (see
/// [`PAGE_NET_BUN_REQUESTS`]).
pub fn page_net_bun_request_count() -> u64 {
    PAGE_NET_BUN_REQUESTS.load(Ordering::Relaxed)
}

// ──────────────────────────────────────────────────────────────────────────
// Outcome types
// ──────────────────────────────────────────────────────────────────────────

/// One streamed body frame crossing the HTTPThread → tokio boundary.
pub type BodyFrame = Result<Vec<u8>, BridgeError>;

/// The response head + incremental body handle returned by [`fetch_core`].
/// Head fields are copied out of the HTTPThread's buffers inside the first
/// delivery callback (plain owned data, freely cross-thread); body bytes
/// arrive through the channel as the HTTPThread delivers them.
pub struct BunHttpResponse {
    pub status_code: u16,
    /// Raw status text bytes (may be non-canonical; surfaced as ReasonPhrase).
    pub status_text: Vec<u8>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    /// Whether ALPN selected HTTP/2 for this exchange (drives the response
    /// `Version` surfaced to servo).
    pub is_http2: bool,
    /// TLS security snapshot of the connection that produced this head
    /// (protocol/cipher/ALPN/peer-cert chain); `None` for plaintext.
    pub tls_info: Option<bun_http::BunTlsInfo>,
    body_rx: UnboundedReceiver<BodyFrame>,
    /// Carried alongside the body; consumed by [`to_servo_response`].
    abort_guard: Option<StreamAbortGuard>,
}

impl BunHttpResponse {
    /// Await the next incremental body frame: `Ok(bytes)` per delivery,
    /// `Err` on mid-stream failure, `None` at clean stream end.
    pub async fn next_chunk(&mut self) -> Option<BodyFrame> {
        self.body_rx.recv().await
    }

    /// Drain the body to completion (test / buffered-consumer helper).
    pub async fn collect_body(&mut self) -> Result<Vec<u8>, BridgeError> {
        let mut body = Vec::new();
        while let Some(frame) = self.next_chunk().await {
            body.extend_from_slice(&frame?);
        }
        Ok(body)
    }
}

/// Bridge-level error. `CertificateFailure` is refined by the servo-side
/// wrapper (it needs `CertificateErrorOverrideManager`, which the pure-Rust
/// HTTPThread callback must not touch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    Network(NetworkError),
    CertificateFailure(String),
}

// ──────────────────────────────────────────────────────────────────────────
// Cancellation handles
// ──────────────────────────────────────────────────────────────────────────

struct SignalBox {
    store: bun_http::signals::Store,
}

/// Abort handle for one in-flight bridge request.
///
/// Mirrors the `src/http/Signals.rs` `Store` / `Field::Aborted` pattern: the
/// `aborted` atomic is wired into the `AsyncHTTP` signals, and `abort()` also
/// enqueues an HTTPThread shutdown for the request's `async_http_id`, which
/// force-closes the abort-tracker-registered socket via `close_and_abort` →
/// `err!(Aborted)` → mapped to [`NetworkError::LoadCancelled`].
pub struct BunCancelHandle {
    signal_box: StdArc<SignalBox>,
    /// `async_http_id` of the scheduled request (0 until `fetch_core` stashes
    /// it). Shared as an `Arc<AtomicU32>` so the response-body stream's
    /// abort-on-drop guard can trigger the same shutdown when servo cancels
    /// mid-body (written on the scheduling thread, read from whichever thread
    /// calls `abort()`).
    async_http_id: StdArc<AtomicU32>,
}

impl BunCancelHandle {
    pub fn new() -> Self {
        let signal_box = Box::new(SignalBox {
            store: bun_http::signals::Store::default(),
        });
        BunCancelHandle {
            signal_box: StdArc::from(signal_box),
            async_http_id: StdArc::new(AtomicU32::new(0)),
        }
    }

    /// The `Signals` view to wire into `AsyncHTTP::init` options.
    ///
    /// Equivalent to `Store::to(&mut)` — every slot points at this handle's
    /// atomics — but works from a shared reference (the atomics are
    /// interior-mutable, exactly the `Signals` BACKREF contract).
    fn signals(&self) -> bun_http::signals::Signals {
        let store = &self.signal_box.store;
        bun_http::signals::Signals {
            header_progress: Some(::std::ptr::NonNull::from(&store.header_progress)),
            response_body_streaming: Some(::std::ptr::NonNull::from(
                &store.response_body_streaming,
            )),
            aborted: Some(::std::ptr::NonNull::from(&store.aborted)),
            cert_errors: Some(::std::ptr::NonNull::from(&store.cert_errors)),
            upgraded: Some(::std::ptr::NonNull::from(&store.upgraded)),
        }
    }

    /// Abort the in-flight request: sets the aborted signal and enqueues the
    /// HTTPThread shutdown for this request's socket.
    pub fn abort(&self) {
        abort_signal(&self.signal_box, &self.async_http_id);
    }

    pub fn is_aborted(&self) -> bool {
        self.signal_box.store.aborted.load(Ordering::Acquire)
    }
}

/// The abort primitive shared by [`BunCancelHandle::abort`] and the
/// stream-guard's [`Drop`]: set the Signals `aborted` atomic, then enqueue the
/// HTTPThread shutdown for the request's `async_http_id` (0 = not yet
/// scheduled → nothing to shut down).
fn abort_signal(signal_box: &SignalBox, async_http_id: &AtomicU32) {
    signal_box.store.aborted.store(true, Ordering::Release);
    let id = async_http_id.load(Ordering::Acquire);
    if id != 0 {
        bun_http::HTTPThread::schedule_shutdown_from_any_thread(id);
    }
}

/// Abort-on-drop guard carried by the streamed response body: when servo
/// drops the body future mid-stream (its consumption loop does exactly that
/// on cancellation), the HTTPThread request is shut down so its socket and
/// task are reclaimed promptly — the bridge equivalent of dropping a hyper
/// body mid-read. Disarmed at natural stream end (terminal delivery already
/// reclaimed everything; a shutdown for a finished id is only noise).
pub struct StreamAbortGuard {
    signal_box: StdArc<SignalBox>,
    async_http_id: StdArc<AtomicU32>,
    armed: bool,
}

impl StreamAbortGuard {
    fn new(signal_box: StdArc<SignalBox>, async_http_id: StdArc<AtomicU32>) -> Self {
        StreamAbortGuard {
            signal_box,
            async_http_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StreamAbortGuard {
    fn drop(&mut self) {
        if self.armed {
            abort_signal(&self.signal_box, &self.async_http_id);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Bridge state (shared between the tokio future and the HTTPThread callback)
// ──────────────────────────────────────────────────────────────────────────

/// The response head as published by the HTTPThread callback (plain owned
/// data — see [`BunHttpResponse`] for the field semantics).
struct BridgeHead {
    status_code: u16,
    status_text: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    is_http2: bool,
    tls_info: Option<bun_http::BunTlsInfo>,
}

struct BridgeState {
    /// Response head (or the terminal error, when the exchange failed before
    /// headers). Written once by `on_http_done` (first delivery carrying
    /// metadata, or any failure), consumed by the awaiting tokio future.
    head: StdMutex<Option<Result<BridgeHead, BridgeError>>>,
    /// Incremental body frames: HTTPThread callback → tokio consumer. The
    /// sender lives in this state (dropped with it after the terminal
    /// callback, closing the channel at clean stream end).
    body_tx: UnboundedSender<BodyFrame>,
    /// Wakes the awaiting future after `head` is written.
    notify: Notify,
    /// Keeps the `Store` that the AsyncHTTP signals point into alive for as
    /// long as either the future or the callback holds this state.
    signal_box: StdArc<SignalBox>,
    /// Backing `Box<[u8]>` for the leaked `&'static` URL (reclaimed by the
    /// callback). `None` for the empty static slice.
    url_owned: Option<*mut [u8]>,
    /// Backing `Box<[u8]>` for the leaked `&'static` request body.
    body_owned: Option<*mut [u8]>,
    /// Backing `Box<[u8]>` for the leaked `&'static` headers buffer.
    headers_owned: Option<*mut [u8]>,
}

// SAFETY: `head` (std Mutex), `body_tx` (tokio), `notify` (tokio) and
// `signal_box` (atomics) are Sync. The three raw-pointer fields are set
// before the `Arc` is shared with the HTTPThread (before
// `HTTPThread::schedule`), are never mutated afterwards, and are
// dereferenced exactly once inside the terminal callback — which the
// schedule() MPSC push happens-before. The pointers themselves are never
// dereferenced through `&BridgeState` on any other path.
unsafe impl Send for BridgeState {}
unsafe impl Sync for BridgeState {}

// ──────────────────────────────────────────────────────────────────────────
// Error mapping (bun_core::Error — name-interned — → servo NetworkError)
// ──────────────────────────────────────────────────────────────────────────

/// BoringSSL X509 verify-failure names (`get_cert_error_from_no`'s Zig
/// error-set tags, delivered by `on_handshake` → `close_and_fail`) that are
/// peer-certificate verification failures. Mapped to
/// [`BridgeError::CertificateFailure`] so the servo-side wrapper can refine
/// them through the override manager exactly like the hyper era's
/// `from_hyper_error(&error, failing_cert)` classification.
const CERT_VERIFY_FAILURES: &[&str] = &[
    "ERR_TLS_CERT_ALTNAME_INVALID",
    "UNABLE_TO_GET_ISSUER_CERT",
    "UNABLE_TO_DECRYPT_CERT_SIGNATURE",
    "UNABLE_TO_DECODE_ISSUER_PUBLIC_KEY",
    "CERT_SIGNATURE_FAILURE",
    "CERT_NOT_YET_VALID",
    "CERT_HAS_EXPIRED",
    "ERROR_IN_CERT_NOT_BEFORE_FIELD",
    "ERROR_IN_CERT_NOT_AFTER_FIELD",
    "DEPTH_ZERO_SELF_SIGNED_CERT",
    "SELF_SIGNED_CERT_IN_CHAIN",
    "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
    "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
    "CERT_CHAIN_TOO_LONG",
    "CERT_REVOKED",
    "INVALID_CA",
    "PATH_LENGTH_EXCEEDED",
    "INVALID_PURPOSE",
    "CERT_UNTRUSTED",
    "CERT_REJECTED",
    "SUBJECT_ISSUER_MISMATCH",
    "AKID_SKID_MISMATCH",
    "AKID_ISSUER_SERIAL_MISMATCH",
    "KEYUSAGE_NO_CERTSIGN",
    "UNHANDLED_CRITICAL_EXTENSION",
    "INVALID_EXTENSION",
    "HOSTNAME_MISMATCH",
    "EMAIL_MISMATCH",
    "IP_ADDRESS_MISMATCH",
    "NAME_CONSTRAINTS_WITHOUT_SANS",
    "STORE_LOOKUP",
];

/// Map a `bun_core::Error` (Zig `anyerror` port: identity is the interned
/// name) to a servo [`NetworkError`].
///
/// Semantics are cross-checked against the hyper era
/// (`http_loader.rs:from_hyper_error`): everything except certificate
/// verification failures surfaced as `HttpError(message)`; certificate
/// failures became `SslValidation(message, certificate)` when the override
/// manager had a failing-verification certificate for the host. This table
/// makes the transport-level classes servo already had dedicated variants for
/// precise, and keeps `HttpError` as the default — identical to hyper-era
/// behaviour for everything else.
pub fn map_bun_error(error: bun_core::Error) -> BridgeError {
    let name = error.name();
    if CERT_VERIFY_FAILURES.contains(&name) {
        // Refined by the wrapper via the override manager (SslValidation
        // with cert, or HttpError fallback) — exactly matching
        // from_hyper_error's certificate parameter.
        return BridgeError::CertificateFailure(error.to_string());
    }
    match name {
        // Abort family (Signals::Field::Aborted / ClientAborted on socket
        // lifecycle) → the fetch was cancelled.
        "Aborted" | "AbortedBeforeConnecting" | "ClientAborted" => {
            BridgeError::Network(NetworkError::LoadCancelled)
        },
        // Connect-phase transport failures → servo's generic connection
        // failure (the variant the hyper path used for a failed request-body
        // stream and socket-level errors).
        "ConnectionRefused" | "ECONNREFUSED" | "FailedToOpenSocket" => {
            BridgeError::Network(NetworkError::ConnectionFailure)
        },
        // Mid-response transport failures. Hyper era: surfaced inside
        // `HttpError("... connection closed ...")`; servo's dedicated
        // ConnectionFailure variant is the precise form of the same class.
        "ConnectionClosed" | "ECONNRESET" | "EPIPE" | "ECONNABORTED" => {
            BridgeError::Network(NetworkError::ConnectionFailure)
        },
        // DNS: hyper-era servo had no dedicated variant — the resolver error
        // travelled inside the HttpError message; keep that shape.
        "DNSResolutionFailed" | "EAI_AGAIN" => {
            BridgeError::Network(NetworkError::HttpError(error.to_string()))
        },
        // Idle-timeout family: hyper-era default client had no total-request
        // timeout, so a timeout was just a transport error message.
        "Timeout" | "ETIMEDOUT" => {
            BridgeError::Network(NetworkError::HttpError(error.to_string()))
        },
        // Redirects (only reachable if a future caller switches off Manual —
        // kept mapped so the table is total).
        "TooManyRedirects" => BridgeError::Network(NetworkError::TooManyRedirects),
        "RedirectURLTooLong" | "RedirectURLInvalid" | "InvalidRedirectURL" |
        "UnsupportedRedirectProtocol" | "UnexpectedRedirect" => {
            BridgeError::Network(NetworkError::RedirectError)
        },
        "InvalidMethod" => BridgeError::Network(NetworkError::InvalidMethod),
        // bun never decompresses for us (disable_decompression), but the
        // error name is mapped for table totality.
        "DecompressionNotImplemented" => BridgeError::Network(NetworkError::DecompressionError),
        "OutOfMemory" => BridgeError::Network(NetworkError::Crash(error.to_string())),
        // Wire-protocol parse failures and the HTTP/2 / HTTP/3 protocol-error
        // families: hyper-era parity (message inside HttpError).
        _ => BridgeError::Network(NetworkError::HttpError(error.to_string())),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Stealth: wire config + h2 fingerprint + CA override → bun_http SSLConfig
// ──────────────────────────────────────────────────────────────────────────

/// Build the bridge's `bun_http` `SSLConfig` from the three servo-side
/// inputs, so the bridge's ClientHello carries the same JA3/JA4 fingerprint,
/// its h2 sessions carry the same SETTINGS payload / pseudo-header wire
/// order / preface PRIORITY frames as the page profile, and its connections
/// verify against the same trust list as the hyper path.
///
/// - `wire`: servo's global stealth wire config (set by the embedder alongside
///   `set_stealth_tls_config`; IANA→OpenSSL name resolution goes through
///   `bao_stealth` — single source of truth). The ALPN offer stays
///   policy-driven via `Flags::is_page_egress` (caller passes whether the
///   profile's ALPN list contains `h2`).
/// - `h2_fingerprint`: the active page profile's `Http2Fingerprint`
///   (`bao_stealth::global_http2_fingerprint` — the wire config does not
///   carry pseudo-header order / PRIORITY frames, stage 2 reads them from
///   this snapshot). Same source window.fetch shapes its SSLConfig from.
/// - `ca_override`: servo `CACertificates::Override` DER list, or `None` for
///   `CACertificates::Default` (system roots — the usockets context default).
///
/// Plain default config when no profile is active.
pub fn build_ssl_config(
    wire: Option<&StealthTlsWireConfig>,
    h2_fingerprint: Option<&bao_stealth::Http2Fingerprint>,
    ca_override: Option<&[Vec<u8>]>,
) -> bun_http::ssl_config::SSLConfig {
    let mut config = bun_http::ssl_config::SSLConfig::default();
    let Some(wire) = wire else {
        if let Some(ca) = ca_override {
            // Profile-less embedder can still pin a trust list.
            config.ca_certs_der = Some(ca.iter().map(|der| der.clone().into()).collect());
        }
        return config;
    };
    config.tls12_cipher_list = bun_core::dupe_z(
        bao_stealth::boringssl_cipher_list_string(&wire.tls12_cipher_suites).as_bytes(),
    );
    // TLS 1.3 suite names are not part of boringssl_cipher_list_string
    // (TLS 1.3 ciphers have a built-in order); resolve them directly.
    let tls13 = wire
        .tls13_cipher_suites
        .iter()
        .copied()
        .filter(|id| (0x1301..=0x1303).contains(id))
        .filter_map(bao_stealth::cipher_suite_openssl_name)
        .collect::<Vec<_>>()
        .join(":");
    config.tls13_cipher_suites = bun_core::dupe_z(tls13.as_bytes());
    config.tls_curves_list = bun_core::dupe_z(
        bao_stealth::boringssl_curves_list_string(&wire.supported_groups).as_bytes(),
    );
    config.tls_sigalgs_list = bun_core::dupe_z(
        bao_stealth::boringssl_sigalgs_list_string(&wire.signature_algorithms).as_bytes(),
    );
    config.h2_settings_payload = Some(wire.h2_settings_payload.clone().into_boxed_slice());
    config.h2_initial_window_size = wire.h2_initial_stream_size;
    if let Some(fingerprint) = h2_fingerprint {
        // REQ-STL-002: pseudo-header wire order (Firefox/Chrome differ) and
        // REQ-STL-002-C3: connection-setup PRIORITY frames (Firefox-only) —
        // same field mapping `stealth_http::stealth_profile_to_ssl_config`
        // uses for window.fetch (the wire order is the HPACK emission order,
        // a client-visible fingerprint).
        config.h2_pseudo_header_order = Some(
            fingerprint
                .pseudo_header_order
                .iter()
                .map(|name| name.to_string().into_boxed_str())
                .collect(),
        );
        config.h2_priority_frames = Some(
            fingerprint
                .priority_frames
                .iter()
                .map(|frame| bun_http::ssl_config::H2PriorityFrame {
                    stream_id: frame.stream_id,
                    stream_dependency: frame.stream_dependency,
                    exclusive: frame.exclusive,
                    weight: frame.weight,
                })
                .collect(),
        );
    }
    if let Some(ca) = ca_override {
        config.ca_certs_der = Some(ca.iter().map(|der| der.clone().into()).collect());
    }
    config
}

// ──────────────────────────────────────────────────────────────────────────
// Core fetch (tokio future hosting the HTTPThread request)
// ──────────────────────────────────────────────────────────────────────────

/// Drive one HTTP exchange through bun's HTTPThread and await its response
/// head (plus the streamed-body handle) on the current tokio thread.
///
/// This is the bridge's core, free of servo `FetchContext` concerns so it can
/// be exercised directly by unit tests. `url` must already be
/// fragment-percent-encoded (same `FRAGMENT` set the hyper path uses).
/// `extension_method` carries the interned wire token when `method` is
/// `Method::EXTENSION` (verbs outside bun's closed registry — hyper parity).
///
/// The ownership protocol mirrors `fetch_async.rs` (BUG-ENG-369 /
/// BCE-007-R5): URL / headers buffer / body are leaked to `&'static` because
/// the heap-allocated `AsyncHTTP` outlives this frame; the backing boxes are
/// reclaimed by the terminal `on_http_done`. The `AsyncHTTP` itself is
/// heap-allocated via `bun_core::heap::into_raw` and reclaimed through the
/// `real` backref (see the audit comment in `on_http_done`).
///
/// Response streaming: the `ResponseBodyStreaming` signal is armed before
/// scheduling, so the callback fires per `on_data` — every non-terminal
/// delivery drains the shared body buffer into the channel and round-trips
/// `schedule_response_body_drain` (the 52634b89 contract). The returned
/// future resolves as soon as the head (status line + headers + TLS
/// snapshot) is available; the body keeps streaming into
/// `BunHttpResponse::next_chunk` afterwards.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_core(
    method: bun_http::Method,
    extension_method: Option<&'static [u8]>,
    url: String,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Option<Vec<u8>>,
    cancel: &BunCancelHandle,
    should_cancel: Option<impl Fn() -> bool>,
    tls_props: Option<bun_http::ssl_config::SharedPtr>,
    page_egress_h2: bool,
    reject_unauthorized: bool,
) -> Result<BunHttpResponse, BridgeError> {
    // Phase A — build, schedule. Every non-Send local (raw pointers, the
    // HeaderBuilder, the Signals view) is created and fully consumed inside
    // this block, so the awaiting Phase B below stays Send (servo boxes the
    // http_fetch future as `dyn Future + Send`).
    let (state, body_rx, abort_guard) = {
        // URL: leak to 'static for the heap AsyncHTTP to borrow.
        let (url_static, url_owned) = if url.is_empty() {
            (&[][..], None)
        } else {
            let leaked: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
            (leaked, Some(leaked as *const [u8] as *mut [u8]))
        };
        let parsed_url = bun_url::URL::parse(url_static);

        // Headers: HeaderBuilder → entries + packed buffer (same pattern as
        // fetch_async.rs / http_client.rs).
        let mut hb = bun_http::HeaderBuilder::default();
        for (name, value) in &headers {
            hb.count(name, value);
        }
        if hb.allocate().is_err() {
            return Err(BridgeError::Network(NetworkError::Crash(
                "bun bridge: header allocation failed".into(),
            )));
        }
        for (name, value) in &headers {
            hb.append(name, value);
        }
        let content_len = hb.content.len;
        let headers_cap: Box<[u8]> = hb.content.move_to_slice();
        let headers_ptr = Box::into_raw(headers_cap);
        // SAFETY: the first `content_len` bytes were initialized by the
        // appends above; the full-capacity backing box is reclaimed by
        // `on_http_done`.
        let headers_buf: &'static [u8] = if content_len > 0 {
            unsafe { ::std::slice::from_raw_parts((*headers_ptr).as_ptr(), content_len) }
        } else {
            // SAFETY: allocated just above, never handed to anyone.
            unsafe { drop(Box::from_raw(headers_ptr)) };
            &[]
        };
        let entry_list = hb.entries;

        // Request body: leak to 'static (empty body shares the static empty
        // slice). Buffered — see the module header (stage 2 keeps servo-side
        // request-body semantics; no Transfer-Encoding header is set here,
        // bun frames the buffered body with Content-Length).
        let (body_slice, body_owned): (&'static [u8], Option<*mut [u8]>) = match body {
            Some(b) if !b.is_empty() => {
                let leaked: &'static [u8] = Box::leak(b.into_boxed_slice());
                (leaked, Some(leaked as *const [u8] as *mut [u8]))
            },
            _ => (&[], None),
        };

        // Response buffer owned by the AsyncHTTP (raw *mut; freed by the
        // callback after the terminal delivery).
        let response_buffer = Box::into_raw(Box::new(bun_core::MutableString::default()));

        let (body_tx, body_rx) = unbounded_channel::<BodyFrame>();
        let state: StdArc<BridgeState> = StdArc::new(BridgeState {
            head: StdMutex::new(None),
            body_tx,
            notify: Notify::new(),
            signal_box: StdArc::clone(&cancel.signal_box),
            url_owned,
            body_owned,
            headers_owned: if content_len > 0 {
                Some(headers_ptr)
            } else {
                None
            },
        });
        // The state's `signal_box` keeps the atomics the AsyncHTTP signals
        // point into alive for as long as either the future or the callback
        // holds the state — assert it is the same store we wired.
        debug_assert!(StdArc::ptr_eq(&state.signal_box, &cancel.signal_box));

        // Callback ctx: one Arc reference, consumed by the TERMINAL
        // `on_http_done` delivery (intermediate deliveries only borrow it).
        let callback = bun_http::HTTPClientResultCallback::new(
            StdArc::into_raw(StdArc::clone(&state)) as *mut BridgeState,
            on_http_done,
        );

        let options = bun_http::async_http::Options {
            extension_method,
            // Hyper wire parity: no `Connection: keep-alive` on h1 (the
            // replaced hyper client omitted it; connection persistence is
            // the HTTP/1.1 default).
            omit_connection_header: Some(true),
            tls_props,
            signals: Some(cancel.signals()),
            // Servo's Decoder owns decompression; Accept-Encoding passes
            // through.
            disable_decompression: Some(true),
            reject_unauthorized: Some(reject_unauthorized),
            // Page egress keeps hyper's h2 ALPN offer (stealth-profile
            // driven; see `Flags::is_page_egress` in bun_http).
            is_page_egress: Some(page_egress_h2),
            ..Default::default()
        };

        // SAFETY (heap allocation): the AsyncHTTP's intrusive `task` is
        // linked into the HTTPThread queue and dereferenced after this frame
        // returns, so it must live at a stable heap address (BUG-ENG-369 /
        // BCE-007-R5 — mirrors fetch_async.rs and the upstream Preconnect
        // pattern).
        let mut async_http_boxed = Box::new(bun_http::AsyncHTTP::init(
            method,
            parsed_url,
            entry_list,
            headers_buf,
            response_buffer,
            body_slice,
            callback,
            // Redirect loop belongs to servo (http_loader.rs); bun
            // returns the original 3xx untouched.
            bun_http::FetchRedirect::Manual,
            options,
        ));
        // True streaming (stage 2): per-on_data delivery instead of a single
        // buffered terminal body — the same signal the JS fetch path arms.
        async_http_boxed.enable_response_body_streaming();
        let async_http_box: *mut bun_http::AsyncHTTP<'static> =
            bun_core::heap::into_raw(async_http_boxed);

        // Stash the assigned request id so `BunCancelHandle::abort` and the
        // body-stream guard can enqueue the HTTPThread shutdown.
        // `AsyncHTTP::init` assigns a non-zero id iff the aborted signal was
        // wired — which `BunCancelHandle::new` guarantees.
        cancel
            .async_http_id
            .store(unsafe { (*async_http_box).async_http_id }, Ordering::Release);
        // If the abort already happened before we got here, still enqueue the
        // shutdown so the request cannot run to completion.
        if cancel.is_aborted() {
            cancel.abort();
        }

        // Idempotent process-global init (Once) — mirrors fetch_async.rs:466.
        bun_http::http_thread::init(&Default::default());

        // SAFETY: heap allocation live until `on_http_done` reclaims it via
        // the `real` backref; the task pointer is therefore valid for the
        // scheduler.
        let batch = bun_threading::thread_pool::Batch::from(unsafe {
            ::std::ptr::addr_of_mut!((*async_http_box).task)
        });
        bun_http::HTTPThread::schedule(batch);

        // The abort-on-drop guard shares the cancel handle's signal store and
        // id: dropping the streamed body mid-consumption aborts the request.
        let abort_guard = StreamAbortGuard::new(
            StdArc::clone(&cancel.signal_box),
            StdArc::clone(&cancel.async_http_id),
        );

        (state, body_rx, abort_guard)
    };

    // Phase B — await the head. `notify_one()` stores a permit when no
    // waiter is registered, so a callback that fires before we poll still
    // wakes us. The CANCEL_POLL_INTERVAL tick propagates servo-side
    // cancellation into the aborted signal (and the HTTPThread shutdown)
    // promptly.
    let head = loop {
        if let Some(head) = state.head.lock().unwrap().take() {
            break head;
        }
        let notified = pin!(state.notify.notified());
        ::tokio::select! {
            _ = notified => {},
            _ = ::tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                if let Some(should_cancel) = &should_cancel && should_cancel() {
                    cancel.abort();
                }
            },
        }
    };

    let mut abort_guard = abort_guard;
    match head {
        Ok(head) => Ok(BunHttpResponse {
            status_code: head.status_code,
            status_text: head.status_text,
            headers: head.headers,
            is_http2: head.is_http2,
            tls_info: head.tls_info,
            body_rx,
            abort_guard: Some(abort_guard),
        }),
        Err(error) => {
            // The exchange failed before headers: nothing will consume the
            // body channel — disarm the guard so returning the error (and
            // dropping `body_rx` + guard) cannot abort an already-terminal
            // request.
            abort_guard.disarm();
            Err(error)
        },
    }
}

/// HTTPThread result callback (pure Rust — no SM API, no JS state).
///
/// Streaming shape (stage 2): fires per `on_data` while `has_more`, terminal
/// once at the end. The first delivery carrying metadata publishes the head
/// and wakes the tokio future; every delivery forwards its body bytes into
/// the channel and (non-terminal) drains the shared body buffer +
/// round-trips `schedule_response_body_drain` — the 52634b89 contract. The
/// terminal delivery closes the exchange: it reclaims every leaked
/// allocation (URL / headers / body backing boxes, the response buffer, the
/// JS-thread-side `Box` holding the original `AsyncHTTP`) and consumes the
/// callback's `Arc<BridgeState>` reference.
fn on_http_done(
    this: *mut BridgeState,
    async_http_box: *mut bun_http::AsyncHTTP<'static>,
    mut result: bun_http::HTTPClientResult<'_>,
) {
    let result_is_terminal = !result.has_more;

    // Borrow the state for this delivery (the callback ctx holds one Arc
    // reference that only the terminal delivery consumes below).
    // SAFETY: `this` came from `Arc::into_raw` in `fetch_core` and is alive
    // until the terminal delivery consumes it; intermediate deliveries hold
    // only this shared borrow.
    let state: &BridgeState = unsafe { &*this };

    if let Some(fail) = result.fail {
        let error = map_bun_error(fail);
        publish_failure(state, error);
    } else {
        // Success delivery: publish the head from the first metadata-carrying
        // delivery (headers arrive before any body bytes).
        let head_unpublished = state.head.lock().unwrap().is_none();
        if head_unpublished {
            match result.metadata.as_ref() {
                Some(metadata) => {
                    let head = build_head(metadata, result.is_http2, result.tls_info.clone());
                    match head {
                        Ok(head) => {
                            *state.head.lock().unwrap() = Some(Ok(head));
                            state.notify.notify_one();
                        },
                        Err(error) => publish_failure(state, error),
                    }
                },
                None => {
                    // Defensive: a headers-less body delivery before any
                    // metadata. Nothing to publish yet — the bytes still
                    // stream; the head publishes with the next delivery.
                },
            }
        }

        // Forward this delivery's bytes (any delivery may carry body bytes;
        // the terminal one usually carries none in streaming mode).
        let bytes = result
            .body
            .as_deref()
            .map(|buffer| buffer.list.as_slice().to_vec())
            .unwrap_or_default();
        if !bytes.is_empty() && state.body_tx.send(Ok(bytes)).is_err() {
            // Consumer gone mid-stream (servo dropped the body future): abort
            // so the HTTPThread request terminates instead of streaming into
            // a dead channel until the idle timeout.
            let id = unsafe { (*async_http_box).async_http_id };
            state.signal_box.store.aborted.store(true, Ordering::Release);
            bun_http::HTTPThread::schedule_shutdown_from_any_thread(id);
        }

        // Streaming drain round-trip (mirrors the 52634b89 consumer
        // contract): hand the buffer back empty and schedule the drain so the
        // HTTPThread keeps reading. Terminal deliveries need no drain — the
        // request is complete.
        if !result_is_terminal {
            if let Some(buffer) = result.body.as_deref_mut() {
                buffer.list.clear();
            }
            let id = unsafe { (*async_http_box).async_http_id };
            // SAFETY/THREADING: this callback runs on the HTTPThread, which
            // owns the HTTP_THREAD cell (same-thread idiom as ProxyTunnel's
            // schedule_proxy_deref call sites).
            bun_http::http_thread_mut().schedule_response_body_drain(id);
        }
    }

    // Defensive completeness: a terminal success with the head still
    // unpublished (no metadata ever arrived) must not leave the future
    // waiting forever.
    if result_is_terminal && result.fail.is_none() && state.head.lock().unwrap().is_none() {
        publish_failure(
            state,
            BridgeError::Network(NetworkError::HttpError(
                "bun bridge: response completed without metadata".into(),
            )),
        );
    }

    if result_is_terminal {
        terminal_cleanup(this, async_http_box, state);
    }
}

/// Publish `error` into the head slot if nothing was published yet (fails
/// before headers), else forward it as a mid-stream body frame.
fn publish_failure(state: &BridgeState, error: BridgeError) {
    let mut head = state.head.lock().unwrap();
    if head.is_none() {
        *head = Some(Err(error));
        drop(head);
        state.notify.notify_one();
    } else {
        drop(head);
        // Mid-stream failure: the terminal delivery below drops the state
        // (and with it the sender), so the consumer observes the error frame
        // followed by channel close.
        let _ = state.body_tx.send(Err(error));
    }
}

/// Snapshot the delivery's metadata into the owned [`BridgeHead`].
fn build_head(
    metadata: &bun_http::HTTPResponseMetadata,
    is_http2: bool,
    tls_info: Option<bun_http::BunTlsInfo>,
) -> Result<BridgeHead, BridgeError> {
    let status_code = u16::try_from(metadata.response.status_code).map_err(|_| {
        // Unreachable for conformant servers; fail closed rather than
        // truncating the status.
        BridgeError::Network(NetworkError::HttpError(format!(
            "bun bridge: status code {} out of range",
            metadata.response.status_code
        )))
    })?;
    Ok(BridgeHead {
        status_code,
        status_text: metadata.response.status.to_vec(),
        headers: metadata
            .response
            .headers
            .list
            .iter()
            .map(|header| (header.name().to_vec(), header.value().to_vec()))
            .collect(),
        is_http2,
        tls_info,
    })
}

/// Terminal tail of [`on_http_done`]: publish-side cleanup + reclaim.
/// Consumes the callback's `Arc<BridgeState>` reference (the sole remaining
/// reference is whoever still holds the body receiver — dropping the state
/// closes the channel at clean stream end).
fn terminal_cleanup(
    this: *mut BridgeState,
    async_http_box: *mut bun_http::AsyncHTTP<'static>,
    state: &BridgeState,
) {
    // Reclaim the leaked URL / body / headers backing boxes (each leaked at
    // most once, reclaimed exactly once here).
    //
    // SAFETY: the pointers were produced by `Box::leak`/`Box::into_raw` in
    // `fetch_core` before the state was shared; the terminal callback is the
    // sole consumer.
    unsafe {
        for owned in [
            state.url_owned,
            state.body_owned,
            state.headers_owned,
        ]
        .into_iter()
        .flatten()
        {
            drop(Box::from_raw(owned));
        }
    }

    // Reclaim the scheduling-thread `Box<AsyncHTTP>` via the `real` backref.
    //
    // OWNERSHIP (mirrors the fetch_async.rs on_http_done audit, BCE-007-R6):
    // there are two boxes — (a) the one `fetch_core` heap-allocated, and
    // (b) the HTTPThread's bitwise clone. `on_async_http_callback_raw`
    // drops the clone-owned fields and raw-deallocates box (b); box (a) is
    // the sole dropper of the bitwise-shared fields and is reclaimed here
    // through the `real` backref. The shared raw `response_buffer` is freed
    // here once.
    //
    // SAFETY: guarded on the terminal callback so a still-streaming clone's
    // shared fields cannot be freed underneath it.
    if !async_http_box.is_null() {
        // SAFETY: `async_http_box` is the live HTTPThread clone; `real` was
        // set by `start_queued_task` back to box (a). `take` claims sole
        // ownership of the backref.
        let real = unsafe { (*async_http_box).real.take() };
        if let Some(real_ptr) = real {
            let original_ptr = real_ptr.as_ptr();
            // SAFETY: `Box::into_raw` from `fetch_core`; freed once here.
            let response_buffer = unsafe { (*original_ptr).response_buffer };
            if !response_buffer.is_null() {
                drop(unsafe { Box::from_raw(response_buffer) });
            }
            // SAFETY: sole reclaiming site for box (a).
            drop(unsafe { Box::from_raw(original_ptr) });
        }
    }

    // Consumes the callback's Arc reference.
    // SAFETY: `this` came from `Arc::into_raw` in `fetch_core`; this runs
    // exactly once (terminal), consuming that reference.
    drop(unsafe { StdArc::from_raw(this) });
}

// ──────────────────────────────────────────────────────────────────────────
// Failing-certificate probe (error path only)
// ──────────────────────────────────────────────────────────────────────────

/// Retrieve `host:port`'s leaf certificate (DER) via a bounded direct TLS
/// handshake (`bao_boringssl_bridge::TlsClient` — the same standalone client
/// the connector tests use). Used ONLY on the certificate-failure path to
/// give the override manager the failing certificate (record →
/// `SslValidation` refinement → optional user override), replacing the hyper
/// connector's in-callback recording. Deadline-bounded; plain blocking I/O —
/// acceptable on this error path.
fn probe_failing_certificate(host: &str, port: u16) -> Option<Vec<u8>> {
    use std::io::{Read, Write};

    let client = bao_boringssl_bridge::TlsClient::new().ok()?;
    let mut conn = bao_boringssl_bridge::TlsConnection::new_client(&client, host).ok()?;
    let mut stream = std::net::TcpStream::connect((host, port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let result = conn.process().ok()?;
        loop {
            let outgoing = conn.take_outgoing();
            if outgoing.is_empty() {
                break;
            }
            stream.write_all(&outgoing).ok()?;
        }
        if !conn.is_handshaking() {
            return conn.peer_certificate_der();
        }
        let _ = result;
        if std::time::Instant::now() > deadline {
            return None;
        }
        let mut buf = [0u8; 16_384];
        match stream.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => conn.feed(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock ||
                    e.kind() == std::io::ErrorKind::TimedOut => {},
            Err(_) => return None,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Response conversion (BunHttpResponse → servo HyperResponse<Decoder>)
// ──────────────────────────────────────────────────────────────────────────

/// Convert a `BunTlsInfo` (bun HTTPThread's post-handshake snapshot) into
/// servo's `TlsHandshakeInfo` — the extension the hyper path attaches via
/// `Connected::extra()` and `http_fetch` reads to build `TlsSecurityInfo`
/// (`build_tls_security_info`). Field parity with the connector's own
/// `BoringsslTlsStream::tls_info()` (b946b713): `kea_group_name` /
/// `signature_scheme_name` stay `None` (BoringSSL exposes no public API for
/// them — same documented limitation on the hyper path), `mac` is integral
/// to the AEAD suites BoringSSL negotiates (see `BunTlsInfo::mac`'s doc).
pub fn bun_tls_info_to_handshake(info: &bun_http::BunTlsInfo) -> TlsHandshakeInfo {
    TlsHandshakeInfo {
        protocol_version: info.protocol_version.clone(),
        cipher_suite: info.cipher_suite.clone(),
        // BoringSSL doesn't expose the KX group name via a simple API —
        // connector.rs parity (b946b713 documents the same limitation).
        kea_group_name: None,
        // Signature scheme name — not available from BoringSSL's public API
        // (connector.rs parity).
        signature_scheme_name: None,
        alpn_protocol: info
            .alpn
            .as_deref()
            .map(|alpn| String::from_utf8_lossy(alpn).into_owned()),
        certificate_chain_der: info.peer_certificates_der.clone(),
        used_ech: false, // ECH not negotiated by the bridge stack
    }
}

/// Build the servo-shaped response: same `Decoder::detect(BoxBody, ..)` form
/// `obtain_response` produces, with the body fed INCREMENTALLY from the
/// HTTPThread's streamed deliveries (a `StreamBody` over the bridge's body
/// channel — servo's consumption loop in `http_loader.rs` is fed exactly the
/// shape it already consumes, only the supply became live).
///
/// Extensions carried for downstream servo semantics:
/// - `ReasonPhrase`: the origin's raw (possibly non-canonical) status text,
///   surfaced exactly when it differs from the canonical reason — hyper's own
///   client sets the extension under the same condition. Owned bytes via
///   `TryFrom<Vec<u8>>` (no `&'static` leak); invalid phrase bytes fall back
///   to the canonical reason, matching hyper's client behaviour.
/// - `TlsHandshakeInfo`: `BunTlsInfo` snapshot for https exchanges;
///   `http_fetch` reads it to populate `response.tls_security_info` and the
///   devtools SecurityInfo update.
pub fn to_servo_response(
    mut response: BunHttpResponse,
    is_secure_scheme: bool,
) -> Result<HyperResponse<Decoder>, NetworkError> {
    let status = StatusCode::from_u16(response.status_code).map_err(|error| {
        NetworkError::HttpError(format!("bun bridge: invalid status code: {error}"))
    })?;

    let mut header_map = HeaderMap::new();
    for (name, value) in response.headers {
        let name = HeaderName::from_bytes(&name).map_err(|error| {
            NetworkError::HttpError(format!("bun bridge: invalid header name: {error}"))
        })?;
        let value = HeaderValue::from_bytes(&value).map_err(|error| {
            NetworkError::HttpError(format!("bun bridge: invalid header value: {error}"))
        })?;
        header_map.append(name, value);
    }

    // Mid-stream failures end the stream: hyper 1.x exposes no public
    // constructor for a `hyper::Error` carrying an `io::Error`, and the
    // hyper-era visible behaviour on a transport failure mid-body was
    // precisely "body marked Done, Data::Done" (try_fold's error arm) —
    // ending the stream yields the same page-visible semantics.
    let body_stream = futures::stream::unfold(
        (response.body_rx, response.abort_guard.take()),
        |(mut body_rx, mut guard)| async move {
            match body_rx.recv().await {
                Some(Ok(bytes)) => Some((Ok(Frame::data(Bytes::from(bytes))), (body_rx, guard))),
                Some(Err(error)) => {
                    log::debug!("bun bridge: response body failed mid-stream: {error:?}");
                    if let Some(guard) = guard.as_mut() {
                        guard.disarm();
                    }
                    None // terminal fail delivery already reclaimed everything
                },
                None => {
                    if let Some(guard) = guard.as_mut() {
                        guard.disarm();
                    }
                    None // clean stream end
                },
            }
        },
    );
    let body = StreamBody::new(body_stream).boxed();

    let mut builder = HyperResponse::builder()
        .status(status)
        .version(if response.is_http2 {
            Version::HTTP_2
        } else {
            Version::HTTP_11
        });
    if let Some(tls_info) = &response.tls_info {
        builder = builder.extension(bun_tls_info_to_handshake(tls_info));
    }
    if Some(response.status_text.as_slice()) != status.canonical_reason().map(str::as_bytes) {
        if let Ok(phrase) = ReasonPhrase::try_from(std::mem::take(&mut response.status_text)) {
            builder = builder.extension(phrase);
        }
    }

    let mut http_response = builder.body(body).map_err(|error| {
        NetworkError::HttpError(format!("bun bridge: response build failed: {error}"))
    })?;
    // Stage-2 fix: the header map MUST land on the response — servo's
    // downstream semantics all read it (CORS check / ACAO, MIME + nosniff,
    // HSTS, cookies, cache). The phase-0 bridge built the map but never
    // attached it, which no bridge-level assertion caught (only the
    // full servo pipeline observes response headers).
    *http_response.headers_mut() = header_map;

    Ok(Decoder::detect(http_response, is_secure_scheme))
}

// ──────────────────────────────────────────────────────────────────────────
// Devtools (requestWillBeSent-equivalent — hyper-path field parity)
// ──────────────────────────────────────────────────────────────────────────

/// Build the devtools `NetworkEvent::HttpRequest` message — the same message
/// the hyper path builds at the same checkpoints (`obtain_response`
/// and_then), field for field: url/method/headers/body(published bytes),
/// connect_time (head build span), send_time (wire span), destination,
/// is_xhr, browsing context. `None` when devtools identification is
/// incomplete (no request_id / pipeline_id / browsing_context_id) — the
/// hyper path's own triple-Option gate.
#[allow(clippy::too_many_arguments)]
pub fn build_devtools_request_msg(
    request_id: Option<&str>,
    url: &ServoUrl,
    method: &Method,
    headers: &HeaderMap,
    body_bytes: Vec<u8>,
    pipeline_id: Option<PipelineId>,
    connect_time: std::time::Duration,
    send_time: std::time::Duration,
    destination: Destination,
    is_xhr: bool,
    browsing_context_id: Option<BrowsingContextId>,
) -> Option<ChromeToDevtoolsControlMsg> {
    let (request_id, pipeline_id, browsing_context_id) =
        (request_id?.to_owned(), pipeline_id?, browsing_context_id?);
    let body = Some(body_bytes);
    Some(prepare_devtools_request(
        request_id,
        url.clone(),
        method.clone(),
        headers.clone(),
        body,
        pipeline_id,
        connect_time,
        send_time,
        destination,
        is_xhr,
        browsing_context_id,
    ))
}

// ──────────────────────────────────────────────────────────────────────────
// Servo-side entry point (called from http_loader.rs behind the flag)
// ──────────────────────────────────────────────────────────────────────────

/// Flag-gated replacement for `obtain_response` (hyper). Same contract:
/// returns headers-wrapped response + devtools message (stage 2: built, not
/// `None`) or a `NetworkError`. The redirect loop, CORS, cache, HSTS and
/// cookies all remain servo-side — this function only performs the single
/// HTTP exchange. The downstream `responseReceived`-equivalent devtools
/// messages are emitted by `http_fetch`'s shared body-consumption loop,
/// exactly as for the hyper path.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn obtain_response_bun(
    url: &ServoUrl,
    method: &Method,
    request_headers: &mut HeaderMap,
    body_sender: Option<StdArc<Mutex<Option<IpcSender<BodyChunkRequest>>>>>,
    pipeline_id: &Option<PipelineId>,
    request_id: Option<&str>,
    destination: Destination,
    is_xhr: bool,
    browsing_context_id: Option<BrowsingContextId>,
    context: &FetchContext,
    fetch_terminated: UnboundedSender<bool>,
) -> Result<(HyperResponse<Decoder>, Option<ChromeToDevtoolsControlMsg>), NetworkError> {
        PAGE_NET_BUN_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // https://url.spec.whatwg.org/#percent-encoded-bytes — same encode set as
    // the hyper path.
    let mut encoded_url = utf8_percent_encode(url.as_str(), FRAGMENT).to_string();

    // Host-table parity (hyper connector behavior): `Connector::connect`
    // resolves the CONNECT destination through `hosts::replace_host`
    // (HOST_FILE / opts.host_file / the test-util mock table) while the Host
    // header keeps the original name. The bridge rewrites the URL host the
    // same way and pins the original authority as an explicit Host header —
    // bun's builder then uses it verbatim (override_host_header) instead of
    // deriving `Host` from the replaced URL.
    let mut wire_headers = request_headers.clone();
    let original_host = url.host_str().unwrap_or("").to_owned();
    let replaced_host = crate::hosts::replace_host(&original_host).into_owned();
    if replaced_host != original_host {
        let port_suffix = match url.port() {
            Some(port) => format!(":{port}"),
            None => String::new(),
        };
        let authority = format!("{original_host}{port_suffix}");
        encoded_url = encoded_url.replacen(
            &format!("://{original_host}"),
            &format!("://{replaced_host}"),
            1,
        );
        if let Ok(value) = HeaderValue::from_str(&authority) {
            wire_headers.insert(http::header::HOST, value);
        }
    }

    // Timing + devtools checkpoints at the same points as the hyper path
    // (obtain_response): connect_start before the request is assembled,
    // connect_end once it is fully built, send_start right before the wire,
    // send_end at response-head arrival.
    let connect_start = CrossProcessInstant::now();
    context.timing.set_attributes(&[
        ResourceAttribute::DomainLookupStart,
        ResourceAttribute::ConnectStart(connect_start),
    ]);
    // TODO-parity(#21261-shape): the hyper path also approximates
    // secure_connection_start here (no handshake-completion signal on a
    // pooled/opening connection); the bridge keeps the same checkpoint.
    if url.scheme() == "https" {
        context.timing.set_attribute(ResourceAttribute::SecureConnectionStart);
    }

    // Method: bun's wire contract is a closed IANA registry enum (same
    // policy as window.fetch in fetch_api.rs) — but hyper, the path being
    // replaced, accepts ANY token verb. Stage 3 (hyper parity): unknown
    // methods ride as `Method::EXTENSION` with the interned token
    // (`intern_extension_method`), reaching the wire verbatim instead of
    // failing closed (servo-net test_fetch_redirect_updates_method drives a
    // literal `FOO` method through the pipeline).
    let method_upper = method.as_str().to_uppercase();
    let (bun_method, extension_method) = match bun_http::Method::which(method_upper.as_bytes())
    {
        Some(known) => (known, None),
        None => (
            bun_http::Method::EXTENSION,
            Some(bun_http::intern_extension_method(method_upper.as_bytes())),
        ),
    };

    // Headers pass through verbatim (Accept-Encoding included — set by servo
    // at http_loader.rs set_default_accept_encoding; bun decompression is
    // disabled so the Content-Encoding bytes reach servo's Decoder intact).
    // `wire_headers` is the local copy (possibly carrying the pinned Host
    // header from the host-table rewrite above); the devtools message below
    // keeps reporting the original servo-side header list.
    let headers: Vec<(Vec<u8>, Vec<u8>)> = wire_headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().as_bytes().to_vec(),
                value.as_bytes().to_vec(),
            )
        })
        .collect();

    // Request body: drain the IPC body stream into a buffer. The router
    // callback is the hyper path's own plumbing — it forwards
    // `fetch_terminated(false/true)` on body Done/Error exactly as before,
    // and accumulates the published bytes for the devtools message.
    // (Request bodies stay buffered — servo's
    // streaming-request-body semantics belong to http_loader's hyper path;
    // bun frames the buffered body with Content-Length.)
    let devtools_bytes = StdArc::new(Mutex::new(vec![]));
    let body: Option<Vec<u8>> = if let Some(chunk_requester) = body_sender {
        let (sender, mut receiver) = unbounded_channel();
        obtain_response_setup_router_callback(
            StdArc::clone(&devtools_bytes),
            chunk_requester,
            BodySink::Buffered(sender),
            fetch_terminated,
        )?;
        let mut buffered = vec![];
        loop {
            match receiver.recv().await {
                Some(BodyChunk::Chunk(bytes)) => buffered.extend_from_slice(&bytes),
                Some(BodyChunk::Done) => break,
                None => {
                    log::warn!("Failed to read all chunks from request body.");
                    break;
                },
            }
        }
        Some(buffered)
    } else {
        None
    };

    let connect_end = CrossProcessInstant::now();
    context
        .timing
        .set_attribute(ResourceAttribute::ConnectEnd(connect_end));
    let send_start = CrossProcessInstant::now();
    // Generally a persistent connection is used (hyper-path comment); the
    // request-start stamp sits right before the wire write.
    context
        .timing
        .set_attribute(ResourceAttribute::RequestStart);

    // Stealth TLS + h2 fingerprint + per-request CA (stage 2):
    // - the global wire config the embedder derived from the page profile
    //   (single source: connector's STEALTH_TLS_CONFIG, kept in sync with
    //   the profile window.fetch uses); its ALPN list drives the h2 offer
    //   (`is_page_egress`): the page egress migrated from hyper-h2 and must
    //   keep offering `h2,http/1.1` — downgrading to h1 would change the
    //   page's TLS fingerprint.
    // - the profile's full `Http2Fingerprint` snapshot (pseudo-header order
    //   + preface PRIORITY frames — REQ-STL-002 / REQ-STL-002-C3).
    // - `context.ca_certificates`: `Override(list)` replaces the trust store
    //   for this request's connections (connector `create_tls_config`
    //   semantics); `Default` keeps the system roots.
    let (wire_config, profile_offers_h2) = match crate::connector::get_stealth_tls_config() {
        Some(ref wire) => (
            Some(wire.clone()),
            wire.alpn_protocols
                .iter()
                .any(|proto| proto.as_slice() == b"h2".as_slice()),
        ),
        None => (None, false),
    };
    let ca_override: Option<Vec<Vec<u8>>> = match &context.ca_certificates {
        crate::connector::CACertificates::Override(certificates) => Some(certificates.clone()),
        crate::connector::CACertificates::Default => None,
    };
    // Certificate-override pass-through (hyper parity): certificates the user
    // explicitly accepted (`CertificateErrorOverrideManager::add_override`)
    // bypass chain verification — the hyper connector honors them per
    // connection via its verify callback (`has_override(leaf)`), so the
    // bridge adds them to this request's per-SSL trust store. Hostname
    // checking still applies downstream (`check_server_identity`).
    let mut ca_override = ca_override;
    let overrides = context.state.override_manager.override_certs();
    if !overrides.is_empty() {
        let trust = ca_override.get_or_insert_with(Vec::new);
        for der in overrides {
            if !trust.contains(&der) {
                trust.push(der);
            }
        }
    }
    let ssl_config = build_ssl_config(
        wire_config.as_ref(),
        bao_stealth::global_http2_fingerprint().as_ref(),
        ca_override.as_deref(),
    );
    // Stage 3 (h2 coalescing): intern the config through bun's global
    // registry instead of a fresh `SharedPtr::new` per request. Every pool
    // key in bun_http — the keep-alive pool (`PooledSocket.ssl_config`) and
    // the h2 session/pending-connect matchers (`ClientSession::matches` /
    // `PendingConnect::matches`) — compares `*const SSLConfig`, so
    // content-equal configs must resolve to ONE pointer for a second request
    // to land on the first's connection. The registry weak-upgrades while the
    // originating session/pooled socket still holds a strong ref, so the
    // interned pointer stays stable across sequential requests to the same
    // origin; a profile/CA change produces different content → a distinct
    // interned pointer → its own connection bucket (correct isolation).
    let tls_props = Some(bun_http::ssl_config::GlobalRegistry::intern(ssl_config));

    let cancel = BunCancelHandle::new();
    let cancellation_listener = StdArc::clone(&context.cancellation_listener);
    let should_cancel = move || cancellation_listener.cancelled();

    let host = url.host_str().unwrap_or("").to_owned();

        let outcome = fetch_core(
        bun_method,
        extension_method,
        encoded_url,
        headers,
        body,
        &cancel,
        Some(should_cancel),
        tls_props,
        profile_offers_h2,
        !context.ignore_certificate_errors,
    )
    .await;

    let response = match outcome {
        Ok(response) => response,
        Err(BridgeError::Network(network_error)) => return Err(network_error),
        Err(BridgeError::CertificateFailure(message)) => {
            // Same shape as from_hyper_error: SslValidation with the
            // failing-verification certificate when the override manager has
            // one for this host, HttpError otherwise. The hyper connector
            // populated the manager during its verify callback; the bridge's
            // chain failures close inside the HTTPThread before any servo
            // code sees the certificate, so when the manager has no entry
            // the wrapper retrieves the failing host's leaf certificate via
            // a bounded direct TLS probe (error path only) and records it —
            // the same record→refine→(override manager) flow.
            let certificate = context
                .state
                .override_manager
                .remove_certificate_failing_verification(&host)
                .or_else(|| {
                    if url.scheme() != "https" {
                        return None;
                    }
                    let port = url.port_or_known_default()?;
                    let der = probe_failing_certificate(&host, port)?;
                    context
                        .state
                        .override_manager
                        .record_certificate_failing_verification(&host, &der);
                    context
                        .state
                        .override_manager
                        .remove_certificate_failing_verification(&host)
                });
            return Err(match certificate {
                Some(certificate) => NetworkError::SslValidation(message, certificate.to_vec()),
                None => NetworkError::HttpError(message),
            });
        },
    };

    let send_end = CrossProcessInstant::now();
    // Response head arrived (headers + status on the wire — the hyper path
    // left this a TODO(#21271); the bridge's first delivery is exactly the
    // first-byte-of-response checkpoint).
    context
        .timing
        .set_attribute(ResourceAttribute::ResponseStart);

    // Devtools requestWillBeSent-equivalent (stage 2): same fields, same
    // checkpoints as the hyper path's and_then arm.
    let msg = build_devtools_request_msg(
        request_id,
        url,
        method,
        request_headers,
        devtools_bytes.lock().clone(),
        *pipeline_id,
        (connect_end - connect_start).unsigned_abs(),
        (send_end - send_start).unsigned_abs(),
        destination,
        is_xhr,
        browsing_context_id,
    );

    // Mirror the hyper path's post-response cancellation checkpoint
    // (http_loader.rs, before the body task is spawned).
    if context.cancellation_listener.cancelled() {
        return Err(NetworkError::LoadCancelled);
    }

    let response = to_servo_response(response, url.is_secure_scheme())?;
    Ok((response, msg))
}
