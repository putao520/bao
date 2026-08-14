// Bao fusion (U2 page-network unification, phase 0): fetch driver that runs
// servo's page network through bun's `HTTPThread` (single epoll thread,
// usockets + boringssl stealth TLS) instead of hyper.
//
// Gate: `BAO_PAGE_NET_BUN` (default OFF — the hyper path in
// `http_loader.rs:obtain_response` is untouched unless the flag is enabled).
// `1`/`true` routes every request destination through the bridge (phase 2
// posture); a comma list (`img,css`) routes only matching request
// destinations (phase 1 pilot) — everything else keeps the hyper path.
//
// Threading model (mirrors `fetch_async.rs` FetchTasklet, rehosted on tokio):
//   - servo's fetch runs on a tokio net thread; this bridge schedules the
//     request on the HTTPThread via `HTTPThread::schedule` (the only
//     cross-thread entry point, lock-free MPSC + atomics — legal from here).
//   - the result callback (`on_http_done`) runs on the HTTPThread and is pure
//     Rust (no SM API, no JS state). It writes the outcome into an
//     `Arc<BridgeState>` slot and wakes the tokio future with
//     `tokio::sync::Notify`.
//   - the response body is buffered by bun (phase 0) and re-sliced into a
//     16 KiB-chunk in-memory stream so servo's `Decoder::detect(BoxBody, ..)`
//     consumption loop (`http_loader.rs` spawn_task/try_fold) is unchanged.
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
// Phase 2 (annotated inline; everything below is complete for phase 0):
// devtools messages, real network timing attributes, TLS handshake info /
// TlsSecurityInfo, per-request CA plumbing, streaming (non-buffered) request
// and response bodies, stealth pseudo-header-order / priority-frame parity
// (needs the full StealthProfile, not just the wire config).

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
use ipc_channel::ipc::IpcSender;
use log::warn;
use net_traits::NetworkError;
use net_traits::ResourceAttribute;
use net_traits::request::{BodyChunkRequest, Destination};
use parking_lot::{Mutex, RwLock};
use servo_base::cross_process_instant::CrossProcessInstant;
use servo_url::ServoUrl;
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::connector::StealthTlsWireConfig;
use crate::decoder::Decoder;
use crate::fetch::methods::FetchContext;
use crate::http_loader::{FRAGMENT, BodyChunk, BodySink, obtain_response_setup_router_callback};

/// Chunk size used to re-slice the buffered response body into the in-memory
/// stream handed to servo's Decoder (16 KiB — the HTTP/2 max-frame default,
/// the same granularity bun's own streaming path uses).
const BODY_SLICE_SIZE: usize = 16 * 1024;

/// How often the awaiting future re-checks servo's cancellation listener while
/// waiting for the HTTPThread callback. The Notify wakes us the moment the
/// response is ready; this tick only exists so an in-flight request can be
/// aborted promptly when servo cancels the fetch.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

// ──────────────────────────────────────────────────────────────────────────
// Flag (default OFF)
//
// `BAO_PAGE_NET_BUN`:
//   - unset / `0` / `false`      → hyper path for every destination;
//   - `1` / `true` / `all`       → every destination through the bridge
//                                  (phase 2 posture);
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
/// spec): `0`/`false` → off, `1`/`true`/`all` → every destination, anything
/// else → a destination list. Pure — no global state, directly unit-testable.
pub fn parse_page_net_bun_spec(value: &str) -> PageNetBunMode {
    let trimmed = value.trim();
    if trimmed.is_empty() ||
        trimmed.eq_ignore_ascii_case("0") ||
        trimmed.eq_ignore_ascii_case("false")
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
        Err(_) => PageNetBunMode::Off,
    }
}

/// Resolve the effective mode: the runtime override wins; otherwise the env
/// value is read exactly once (first call). Default: [`PageNetBunMode::Off`]
/// (hyper path).
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

/// A buffered response from bun's HTTP client. Headers/status/body are copied
/// out of the HTTPThread's buffers inside the callback, so this is plain
/// owned data and freely cross-thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BunHttpResponse {
    pub status_code: u16,
    /// Raw status text bytes (may be non-canonical; surfaced as ReasonPhrase).
    pub status_text: Vec<u8>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub body: Vec<u8>,
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
// Cancellation handle
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
    /// it; `AtomicU32` because it is written on the scheduling thread and read
    /// from whichever thread calls `abort()`).
    async_http_id: AtomicU32,
}

impl BunCancelHandle {
    pub fn new() -> Self {
        let signal_box = Box::new(SignalBox {
            store: bun_http::signals::Store::default(),
        });
        BunCancelHandle {
            signal_box: StdArc::from(signal_box),
            async_http_id: AtomicU32::new(0),
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
        self.signal_box
            .store
            .aborted
            .store(true, Ordering::Release);
        let id = self.async_http_id.load(Ordering::Acquire);
        if id != 0 {
            bun_http::HTTPThread::schedule_shutdown_from_any_thread(id);
        }
    }

    pub fn is_aborted(&self) -> bool {
        self.signal_box.store.aborted.load(Ordering::Acquire)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Bridge state (shared between the tokio future and the HTTPThread callback)
// ──────────────────────────────────────────────────────────────────────────

struct BridgeState {
    /// Written once by `on_http_done` (HTTPThread), consumed by the awaiting
    /// tokio future.
    outcome: StdMutex<Option<Result<BunHttpResponse, BridgeError>>>,
    /// Wakes the awaiting future after `outcome` is written.
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

// SAFETY: `outcome` (std Mutex), `notify` (tokio) and `signal_box` (atomics)
// are Sync. The three raw-pointer fields are set before the `Arc` is shared
// with the HTTPThread (before `HTTPThread::schedule`), are never mutated
// afterwards, and are dereferenced exactly once inside the single terminal
// callback — which the schedule() MPSC push happens-before. The pointers
// themselves are never dereferenced through `&BridgeState` on any other path.
unsafe impl Send for BridgeState {}
unsafe impl Sync for BridgeState {}

// ──────────────────────────────────────────────────────────────────────────
// Error mapping (bun_core::Error — name-interned — → servo NetworkError)
// ──────────────────────────────────────────────────────────────────────────

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
    match error.name() {
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
        // Certificate verification failure → refined by the wrapper via the
        // override manager (SslValidation with cert, or HttpError fallback),
        // exactly matching from_hyper_error's certificate parameter.
        "ERR_TLS_CERT_ALTNAME_INVALID" => BridgeError::CertificateFailure(error.to_string()),
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
// Stealth: servo's global wire config → bun_http SSLConfig
// ──────────────────────────────────────────────────────────────────────────

/// Convert servo's global stealth wire config (set by the embedder alongside
/// `set_stealth_tls_config`, same profile `window.fetch` uses) into a
/// `bun_http` `SSLConfig` so the bridge's ClientHello carries the same JA3
/// cipher/curves/sigalgs fingerprint and the same HTTP/2 SETTINGS payload.
///
/// IANA→OpenSSL name resolution goes through `bao_stealth` (single source of
/// truth). The ALPN offer is policy-driven via `Flags::is_page_egress`
/// (caller passes whether the profile's ALPN list contains `h2`) — the
/// page egress keeps hyper's `h2,http/1.1` offer. Phase 2: h2 pseudo-header
/// order / priority frames need the full `StealthProfile`, not just this
/// wire config.
fn stealth_wire_to_ssl_config(wire: &StealthTlsWireConfig) -> bun_http::ssl_config::SSLConfig {
    let mut config = bun_http::ssl_config::SSLConfig::default();
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
    config
}

// ──────────────────────────────────────────────────────────────────────────
// Core fetch (tokio future hosting the HTTPThread request)
// ──────────────────────────────────────────────────────────────────────────

/// Drive one HTTP exchange through bun's HTTPThread and await its outcome on
/// the current tokio thread.
///
/// This is the bridge's core, free of servo `FetchContext` concerns so it can
/// be exercised directly by unit tests. `url` must already be
/// fragment-percent-encoded (same `FRAGMENT` set the hyper path uses).
///
/// The ownership protocol mirrors `fetch_async.rs` (BUG-ENG-369 /
/// BCE-007-R5): URL / headers buffer / body are leaked to `&'static` because
/// the heap-allocated `AsyncHTTP` outlives this frame; the backing boxes are
/// reclaimed by `on_http_done`. The `AsyncHTTP` itself is heap-allocated via
/// `bun_core::heap::into_raw` and reclaimed through the `real` backref (see
/// the audit comment in `on_http_done`).
#[allow(clippy::too_many_arguments)]
pub async fn fetch_core(
    method: bun_http::Method,
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
    let state: StdArc<BridgeState> = {
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
        // slice).
        let (body_slice, body_owned): (&'static [u8], Option<*mut [u8]>) = match body {
            Some(b) if !b.is_empty() => {
                let leaked: &'static [u8] = Box::leak(b.into_boxed_slice());
                (leaked, Some(leaked as *const [u8] as *mut [u8]))
            },
            _ => (&[], None),
        };

        // Response buffer owned by the AsyncHTTP (raw *mut; freed by the
        // callback after the body bytes are copied out).
        let response_buffer = Box::into_raw(Box::new(bun_core::MutableString::default()));

        let state: StdArc<BridgeState> = StdArc::new(BridgeState {
            outcome: StdMutex::new(None),
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

        // Callback ctx: one Arc reference, consumed by `on_http_done`.
        let callback = bun_http::HTTPClientResultCallback::new(
            StdArc::into_raw(StdArc::clone(&state)) as *mut BridgeState,
            on_http_done,
        );

        let options = bun_http::async_http::Options {
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
        let async_http_box: *mut bun_http::AsyncHTTP<'static> =
            bun_core::heap::into_raw(Box::new(bun_http::AsyncHTTP::init(
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
            )));

        // Stash the assigned request id so `BunCancelHandle::abort` can
        // enqueue the HTTPThread shutdown. `AsyncHTTP::init` assigns a
        // non-zero id iff the aborted signal was wired — which
        // `BunCancelHandle::new` guarantees.
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

        state
    };

    // Phase B — await the outcome. `notify_one()` stores a permit when no
    // waiter is registered, so a callback that fires before we poll still
    // wakes us. The CANCEL_POLL_INTERVAL tick propagates servo-side
    // cancellation into the aborted signal (and the HTTPThread shutdown)
    // promptly.
    loop {
        if let Some(outcome) = state.outcome.lock().unwrap().take() {
            return outcome;
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
    }
}

/// HTTPThread result callback (pure Rust — no SM API, no JS state).
///
/// Converts the `HTTPClientResult`, publishes the outcome, wakes the tokio
/// future, and reclaims every leaked allocation (URL / headers / body
/// backing boxes, the response buffer, and the JS-thread-side `Box`
/// holding the original `AsyncHTTP`).
fn on_http_done(
    this: *mut BridgeState,
    async_http_box: *mut bun_http::AsyncHTTP<'static>,
    result: bun_http::HTTPClientResult<'_>,
) {
    // Snapshot terminality: the outcome block partially moves `result`.
    let result_is_terminal = !result.has_more;

    let outcome: Result<BunHttpResponse, BridgeError> = if let Some(fail) = result.fail {
        Err(map_bun_error(fail))
    } else {
        match result.metadata.as_ref() {
            Some(metadata) => {
                let status_code = match u16::try_from(metadata.response.status_code) {
                    Ok(code) => code,
                    Err(_) => {
                        // Unreachable for conformant servers; fail closed
                        // rather than truncating the status.
                        return publish(
                            this,
                            async_http_box,
                            result_is_terminal,
                            Err(BridgeError::Network(NetworkError::HttpError(format!(
                                "bun bridge: status code {} out of range",
                                metadata.response.status_code
                            )))),
                        );
                    },
                };
                Ok(BunHttpResponse {
                    status_code,
                    status_text: metadata.response.status.to_vec(),
                    headers: metadata
                        .response
                        .headers
                        .list
                        .iter()
                        .map(|header| (header.name().to_vec(), header.value().to_vec()))
                        .collect(),
                    body: result
                        .body
                        .as_deref()
                        .map(|buffer| buffer.list.as_slice().to_vec())
                        .unwrap_or_default(),
                })
            },
            None => Err(BridgeError::Network(NetworkError::HttpError(
                "bun bridge: response completed without metadata".into(),
            ))),
        }
    };

    publish(this, async_http_box, result_is_terminal, outcome)
}

/// Tail of [`on_http_done`]: publish + wake + reclaim. Split out so the
/// `?`-free early-return path above shares it.
fn publish(
    this: *mut BridgeState,
    async_http_box: *mut bun_http::AsyncHTTP<'static>,
    result_is_terminal: bool,
    outcome: Result<BunHttpResponse, BridgeError>,
) {
    // SAFETY: `this` came from `Arc::into_raw` in `fetch_core`; this callback
    // runs exactly once (terminal), consuming that reference.
    let state = unsafe { StdArc::from_raw(this) };

    if let Ok(mut guard) = state.outcome.lock() {
        *guard = Some(outcome);
    }
    state.notify.notify_one();

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
    // here once (the body bytes were copied into the outcome above).
    //
    // SAFETY: guarded on the terminal callback so a still-streaming clone's
    // shared fields cannot be freed underneath it.
    if !async_http_box.is_null() && result_is_terminal {
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
    drop(state);
}

// ──────────────────────────────────────────────────────────────────────────
// Response conversion (BunHttpResponse → servo HyperResponse<Decoder>)
// ──────────────────────────────────────────────────────────────────────────

/// Build the servo-shaped response: same `Decoder::detect(BoxBody, ..)` form
/// `obtain_response` produces, with the buffered body re-sliced into 16 KiB
/// chunks — so the streaming consumption loop in `http_loader.rs` is fed
/// exactly the shape it already consumes.
pub(crate) fn to_servo_response(
    response: BunHttpResponse,
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

    let chunks: Vec<Result<Frame<Bytes>, hyper::Error>> = response
        .body
        .chunks(BODY_SLICE_SIZE)
        .map(|chunk| Ok(Frame::data(Bytes::copy_from_slice(chunk))))
        .collect();
    let body = StreamBody::new(futures::stream::iter(chunks)).boxed();

    let http_response = HyperResponse::builder()
        .status(status)
        .version(Version::HTTP_11)
        .body(body)
        .map_err(|error| {
            NetworkError::HttpError(format!("bun bridge: response build failed: {error}"))
        })?;

    // Non-canonical status phrases: servo reads the `ReasonPhrase` extension
    // when present, else falls back to `status.canonical_reason()` — the
    // exact fallback the hyper path uses when hyper does not set one.
    // `ReasonPhrase::from_static` requires `&'static [u8]`, so surfacing the
    // origin's raw phrase bytes needs an owned-bytes carrier: phase 2.
    Ok(Decoder::detect(http_response, is_secure_scheme))
}

// ──────────────────────────────────────────────────────────────────────────
// Servo-side entry point (called from http_loader.rs behind the flag)
// ──────────────────────────────────────────────────────────────────────────

/// Flag-gated replacement for `obtain_response` (hyper). Same contract:
/// returns headers-wrapped response (+ devtools message, `None` in phase 0)
/// or a `NetworkError`. The redirect loop, CORS, cache, HSTS and cookies all
/// remain servo-side — this function only performs the single HTTP exchange.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn obtain_response_bun(
    url: &ServoUrl,
    method: &Method,
    request_headers: &mut HeaderMap,
    body_sender: Option<StdArc<Mutex<Option<IpcSender<BodyChunkRequest>>>>>,
    context: &FetchContext,
    fetch_terminated: UnboundedSender<bool>,
) -> Result<(HyperResponse<Decoder>, Option<ChromeToDevtoolsControlMsg>), NetworkError> {
    PAGE_NET_BUN_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Phase 2: devtools request/response messages (always None until then).

    // https://url.spec.whatwg.org/#percent-encoded-bytes — same encode set as
    // the hyper path.
    let encoded_url = utf8_percent_encode(url.as_str(), FRAGMENT).to_string();

    // Method: closed IANA registry enum is bun's wire contract (same policy
    // as window.fetch in fetch_api.rs); unknown tokens fail closed.
    let method_upper = method.as_str().to_uppercase();
    let bun_method = bun_http::Method::which(method_upper.as_bytes())
        .ok_or(NetworkError::InvalidMethod)?;

    // Headers pass through verbatim (Accept-Encoding included — set by servo
    // at http_loader.rs set_default_accept_encoding; bun decompression is
    // disabled so the Content-Encoding bytes reach servo's Decoder intact).
    let headers: Vec<(Vec<u8>, Vec<u8>)> = request_headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().as_bytes().to_vec(),
                value.as_bytes().to_vec(),
            )
        })
        .collect();

    // Request body: drain the IPC body stream into a buffer (phase 0). The
    // router callback is the hyper path's own plumbing — it forwards
    // `fetch_terminated(false/true)` on body Done/Error exactly as before.
    // Phase 2: true streaming request bodies (no Transfer-Encoding header is
    // set here; bun frames the buffered body with Content-Length).
    let body: Option<Vec<u8>> = if let Some(chunk_requester) = body_sender {
        let devtools_bytes = StdArc::new(Mutex::new(vec![]));
        let (sender, mut receiver) = unbounded_channel();
        obtain_response_setup_router_callback(
            devtools_bytes,
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

    // Timing attributes at the same checkpoints as the hyper path. Phase 2:
    // real per-phase timestamps from the HTTPThread (connect/secure/send).
    let connect_start = CrossProcessInstant::now();
    context.timing.set_attributes(&[
        ResourceAttribute::DomainLookupStart,
        ResourceAttribute::ConnectStart(connect_start),
    ]);
    if url.scheme() == "https" {
        context.timing.set_attribute(ResourceAttribute::SecureConnectionStart);
    }
    context
        .timing
        .set_attribute(ResourceAttribute::ConnectEnd(CrossProcessInstant::now()));
    context.timing.set_attribute(ResourceAttribute::RequestStart);

    // Stealth TLS: same global wire config the embedder derived from the page
    // profile (single source: connector's STEALTH_TLS_CONFIG, kept in sync
    // with the profile window.fetch uses). Plain default config when no
    // profile is active. The profile's ALPN list drives the h2 offer
    // (`is_page_egress`): the page egress migrated from hyper-h2 and must
    // keep offering `h2,http/1.1` — downgrading to h1 would change the
    // page's TLS fingerprint. Phase 2: per-request CA certificates from
    // context.ca_certificates.
    let (ssl_config, profile_offers_h2) = match crate::connector::get_stealth_tls_config() {
        Some(ref wire) => (
            stealth_wire_to_ssl_config(wire),
            wire.alpn_protocols
                .iter()
                .any(|proto| proto.as_slice() == b"h2".as_slice()),
        ),
        None => (bun_http::ssl_config::SSLConfig::default(), false),
    };
    let tls_props = Some(bun_http::ssl_config::SharedPtr::new(ssl_config));

    let cancel = BunCancelHandle::new();
    let cancellation_listener = StdArc::clone(&context.cancellation_listener);
    let should_cancel = move || cancellation_listener.cancelled();

    let host = url.host_str().unwrap_or("").to_owned();

    let outcome = fetch_core(
        bun_method,
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
            // one for this host, HttpError otherwise.
            let certificate = context
                .state
                .override_manager
                .remove_certificate_failing_verification(&host);
            return Err(match certificate {
                Some(certificate) => {
                    NetworkError::SslValidation(message, certificate.to_vec())
                },
                None => NetworkError::HttpError(message),
            });
        },
    };

    // Mirror the hyper path's post-response cancellation checkpoint
    // (http_loader.rs, before the body task is spawned).
    if context.cancellation_listener.cancelled() {
        return Err(NetworkError::LoadCancelled);
    }

    let response = to_servo_response(response, url.is_secure_scheme())?;
    Ok((response, None))
}
