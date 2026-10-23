/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

use std::fmt::{self, Debug, Display};
use std::sync::{LazyLock, OnceLock};
use std::thread::{self, JoinHandle};

use bytes::Bytes;
use content_security_policy::{self as csp};
use cookie::Cookie;
use crossbeam_channel::{Receiver, Sender, unbounded};
use headers::{ContentType, HeaderMapExt, ReferrerPolicy as ReferrerPolicyHeader};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use hyper_serde::Serde;
use hyper_util::client::legacy::Error as HyperError;
use malloc_size_of::malloc_size_of_is_0;
use malloc_size_of_derive::MallocSizeOf;
use mime::Mime;
use parking_lot::RwLock;
use profile_traits::generic_callback::GenericCallback as ProfileGenericCallback;
use profile_traits::mem::ReportsChan;
use profile_traits::time::ProfilerChan;
use rand::{Rng, rng};
use request::RequestId;
use rustc_hash::FxHashMap;
use rustls::{CipherSuite, NamedGroup, ProtocolVersion};
use rustls_pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use serde_with::{FromInto, serde_as};
use servo_base::generic_channel::{
    self, CallbackSetter, GenericCallback, GenericOneshotSender, GenericSend, GenericSender,
    SendResult,
};
use servo_base::id::{CookieStoreId, HistoryStateId, PipelineId, WebViewId};
use servo_url::{ImmutableOrigin, ServoUrl};
use uuid::Uuid;

/// Identifies a pending asynchronous cookie operation initiated by the embedder.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CookieOperationId(pub u64);

use crate::fetch::headers::determine_nosniff;
use crate::filemanager_thread::FileManagerThreadMsg;
use crate::http_status::HttpStatus;
use crate::mime_classifier::{ApacheBugFlag, MimeClassifier};
use crate::request::{Destination, Request, RequestBuilder, RequestMode};
use crate::response::{Response, ResponseInit};

pub mod blob_url_store;
pub mod filemanager_thread;
pub mod http_status;
pub mod image_cache;
pub mod mime_classifier;
pub mod policy_container;
pub mod pub_domains;
pub mod quality;
pub mod request;
pub(crate) mod resource_fetch_timing;
pub mod response;
pub mod sfv;
pub use resource_fetch_timing::{
    RedirectEndValue, RedirectStartValue, ResourceAttribute, ResourceFetchTiming,
    ResourceFetchTimingContainer, ResourceTimeValue, ResourceTimingType,
};

/// <https://fetch.spec.whatwg.org/#document-accept-header-value>
pub const DOCUMENT_ACCEPT_HEADER_VALUE: HeaderValue =
    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");

/// An implementation of the [Fetch specification](https://fetch.spec.whatwg.org/)
pub mod fetch {
    pub mod headers;
}

/// A loading context, for context-specific sniffing, as defined in
/// <https://mimesniff.spec.whatwg.org/#context-specific-sniffing>
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum LoadContext {
    Browsing,
    Image,
    AudioVideo,
    Plugin,
    Style,
    Script,
    Font,
    TextTrack,
    CacheManifest,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct CustomResponse {
    #[serde(
        deserialize_with = "::hyper_serde::deserialize",
        serialize_with = "::hyper_serde::serialize"
    )]
    pub headers: HeaderMap,
    #[serde(
        deserialize_with = "::hyper_serde::deserialize",
        serialize_with = "::hyper_serde::serialize"
    )]
    pub raw_status: (StatusCode, String),
    pub body: Vec<u8>,
}

impl CustomResponse {
    pub fn new(
        headers: HeaderMap,
        raw_status: (StatusCode, String),
        body: Vec<u8>,
    ) -> CustomResponse {
        CustomResponse {
            headers,
            raw_status,
            body,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CustomResponseMediator {
    pub response_chan: GenericCallback<Option<CustomResponse>>,
    pub load_url: ServoUrl,
    /// <https://fetch.spec.whatwg.org/#concept-request-reload-navigation-flag>
    ///
    /// Bao vendor patch (wave2-B2 C): the mediated request is rebuilt in the
    /// SW realm from the URL alone, which reset both navigation flags to
    /// false — `FetchEvent.request.isReloadNavigation` never observed a
    /// reload. The net request itself cannot cross the thread boundary, so
    /// the flags travel with the mediator. (Producer face: the net-side SW
    /// interception — currently absent — stamps these.)
    pub reload_navigation: bool,
    /// <https://fetch.spec.whatwg.org/#concept-request-history-navigation-flag>
    pub history_navigation: bool,
    /// <https://fetch.spec.whatwg.org/#concept-request-destination>
    ///
    /// Bao vendor patch (e60): the mediated request observably carried
    /// `destination: ""` / `mode: "cors"` where the spec wants `"iframe"` /
    /// `"navigate"` — the Sec-Fetch-Dest/Mode face.
    pub destination: Destination,
    pub mode: RequestMode,
    /// Bao vendor patch (REQ-BRW-004 e112 client-scope mediation wave): the
    /// requesting client's pipeline, when the fetch was issued from a script
    /// realm. The manager routes non-navigation mediation by the CLIENT's
    /// controlled state (<https://w3c.github.io/ServiceWorker/#on-fetch-request-algorithm>
    /// step 4: the registration matching the request's client's creation URL,
    /// not the request URL), and the enrolled client set is keyed by pipeline
    /// — so the client identity travels with the mediator. `None` = an infra
    /// fetch with no script client, which keeps the legacy scope-only routing.
    pub client_pipeline: Option<PipelineId>,
}

/// [Policies](https://w3c.github.io/webappsec-referrer-policy/#referrer-policy-states)
/// for providing a referrer header for a request
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ReferrerPolicy {
    /// ""
    EmptyString,
    /// "no-referrer"
    NoReferrer,
    /// "no-referrer-when-downgrade"
    NoReferrerWhenDowngrade,
    /// "origin"
    Origin,
    /// "same-origin"
    SameOrigin,
    /// "origin-when-cross-origin"
    OriginWhenCrossOrigin,
    /// "unsafe-url"
    UnsafeUrl,
    /// "strict-origin"
    StrictOrigin,
    /// "strict-origin-when-cross-origin"
    #[default]
    StrictOriginWhenCrossOrigin,
}

impl ReferrerPolicy {
    /// <https://html.spec.whatwg.org/multipage/#meta-referrer>
    pub fn from_with_legacy(value: &str) -> Self {
        // Step 5. If value is one of the values given in the first column of the following table,
        // then set value to the value given in the second column:
        if value.eq_ignore_ascii_case("never") {
            ReferrerPolicy::NoReferrer
        } else if value.eq_ignore_ascii_case("default") {
            ReferrerPolicy::StrictOriginWhenCrossOrigin
        } else if value.eq_ignore_ascii_case("always") {
            ReferrerPolicy::UnsafeUrl
        } else if value.eq_ignore_ascii_case("origin-when-crossorigin") {
            ReferrerPolicy::OriginWhenCrossOrigin
        } else {
            ReferrerPolicy::from(value)
        }
    }

    /// <https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>
    pub fn parse_header_for_response(headers: &Option<Serde<HeaderMap>>) -> Self {
        // Step 4. Return policy.
        headers
            .as_ref()
            // Step 1. Let policy-tokens be the result of extracting header list values given `Referrer-Policy` and response’s header list.
            .and_then(|headers| headers.typed_get::<ReferrerPolicyHeader>())
            // Step 2-3.
            .into()
    }
}

impl From<&str> for ReferrerPolicy {
    /// <https://html.spec.whatwg.org/multipage/#referrer-policy-attribute>
    fn from(value: &str) -> Self {
        if value.eq_ignore_ascii_case("no-referrer") {
            ReferrerPolicy::NoReferrer
        } else if value.eq_ignore_ascii_case("no-referrer-when-downgrade") {
            ReferrerPolicy::NoReferrerWhenDowngrade
        } else if value.eq_ignore_ascii_case("origin") {
            ReferrerPolicy::Origin
        } else if value.eq_ignore_ascii_case("same-origin") {
            ReferrerPolicy::SameOrigin
        } else if value.eq_ignore_ascii_case("strict-origin") {
            ReferrerPolicy::StrictOrigin
        } else if value.eq_ignore_ascii_case("strict-origin-when-cross-origin") {
            ReferrerPolicy::StrictOriginWhenCrossOrigin
        } else if value.eq_ignore_ascii_case("origin-when-cross-origin") {
            ReferrerPolicy::OriginWhenCrossOrigin
        } else if value.eq_ignore_ascii_case("unsafe-url") {
            ReferrerPolicy::UnsafeUrl
        } else {
            ReferrerPolicy::EmptyString
        }
    }
}

impl Display for ReferrerPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let string = match self {
            ReferrerPolicy::EmptyString => "",
            ReferrerPolicy::NoReferrer => "no-referrer",
            ReferrerPolicy::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            ReferrerPolicy::Origin => "origin",
            ReferrerPolicy::SameOrigin => "same-origin",
            ReferrerPolicy::OriginWhenCrossOrigin => "origin-when-cross-origin",
            ReferrerPolicy::UnsafeUrl => "unsafe-url",
            ReferrerPolicy::StrictOrigin => "strict-origin",
            ReferrerPolicy::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
        };
        write!(formatter, "{string}")
    }
}

/// <https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>
impl From<Option<ReferrerPolicyHeader>> for ReferrerPolicy {
    fn from(header: Option<ReferrerPolicyHeader>) -> Self {
        // Step 2. Let policy be the empty string.
        // Step 3. For each token in policy-tokens, if token is a referrer policy and token is not the empty string, then set policy to token.
        header.map_or(ReferrerPolicy::EmptyString, |policy| match policy {
            ReferrerPolicyHeader::NO_REFERRER => ReferrerPolicy::NoReferrer,
            ReferrerPolicyHeader::NO_REFERRER_WHEN_DOWNGRADE => {
                ReferrerPolicy::NoReferrerWhenDowngrade
            },
            ReferrerPolicyHeader::SAME_ORIGIN => ReferrerPolicy::SameOrigin,
            ReferrerPolicyHeader::ORIGIN => ReferrerPolicy::Origin,
            ReferrerPolicyHeader::ORIGIN_WHEN_CROSS_ORIGIN => ReferrerPolicy::OriginWhenCrossOrigin,
            ReferrerPolicyHeader::UNSAFE_URL => ReferrerPolicy::UnsafeUrl,
            ReferrerPolicyHeader::STRICT_ORIGIN => ReferrerPolicy::StrictOrigin,
            ReferrerPolicyHeader::STRICT_ORIGIN_WHEN_CROSS_ORIGIN => {
                ReferrerPolicy::StrictOriginWhenCrossOrigin
            },
        })
    }
}

impl From<ReferrerPolicy> for ReferrerPolicyHeader {
    fn from(referrer_policy: ReferrerPolicy) -> Self {
        match referrer_policy {
            ReferrerPolicy::NoReferrer => ReferrerPolicyHeader::NO_REFERRER,
            ReferrerPolicy::NoReferrerWhenDowngrade => {
                ReferrerPolicyHeader::NO_REFERRER_WHEN_DOWNGRADE
            },
            ReferrerPolicy::SameOrigin => ReferrerPolicyHeader::SAME_ORIGIN,
            ReferrerPolicy::Origin => ReferrerPolicyHeader::ORIGIN,
            ReferrerPolicy::OriginWhenCrossOrigin => ReferrerPolicyHeader::ORIGIN_WHEN_CROSS_ORIGIN,
            ReferrerPolicy::UnsafeUrl => ReferrerPolicyHeader::UNSAFE_URL,
            ReferrerPolicy::StrictOrigin => ReferrerPolicyHeader::STRICT_ORIGIN,
            ReferrerPolicy::EmptyString | ReferrerPolicy::StrictOriginWhenCrossOrigin => {
                ReferrerPolicyHeader::STRICT_ORIGIN_WHEN_CROSS_ORIGIN
            },
        }
    }
}

// FIXME: https://github.com/servo/servo/issues/34591
#[expect(clippy::large_enum_variant)]
#[derive(Debug, Deserialize, Serialize)]
pub enum FetchResponseMsg {
    // todo: should have fields for transmitted/total bytes
    ProcessRequestBody(RequestId),
    // todo: send more info about the response (or perhaps the entire Response)
    ProcessResponse(RequestId, Result<FetchMetadata, NetworkError>),
    ProcessResponseChunk(RequestId, Bytes),
    ProcessResponseEOF(RequestId, Result<(), NetworkError>, ResourceFetchTiming),
    ProcessCspViolations(RequestId, Vec<csp::Violation>),
    ProcessContentLength(RequestId, usize),
}

impl FetchResponseMsg {
    pub fn request_id(&self) -> RequestId {
        match self {
            FetchResponseMsg::ProcessRequestBody(id) |
            FetchResponseMsg::ProcessResponse(id, ..) |
            FetchResponseMsg::ProcessResponseChunk(id, ..) |
            FetchResponseMsg::ProcessResponseEOF(id, ..) |
            FetchResponseMsg::ProcessCspViolations(id, ..) |
            FetchResponseMsg::ProcessContentLength(id, _) => *id,
        }
    }
}

pub trait FetchTaskTarget {
    /// <https://fetch.spec.whatwg.org/#process-request-body>
    ///
    /// Fired when a chunk of the request body is transmitted
    fn process_request_body(&mut self, request: &Request);

    /// <https://fetch.spec.whatwg.org/#process-response>
    ///
    /// Fired when headers are received
    fn process_response(&mut self, request: &Request, response: &Response);

    /// Fired when a chunk of response content is received
    fn process_response_chunk(&mut self, request: &Request, chunk: bytes::Bytes);

    /// <https://fetch.spec.whatwg.org/#process-response-end-of-file>
    ///
    /// Fired when the response is fully fetched
    fn process_response_eof(&mut self, request: &Request, response: &Response);

    fn process_csp_violations(&mut self, request: &Request, violations: Vec<csp::Violation>);

    /// Tell the listener that have a hint of how long the content is. This will be sent at most once.
    fn process_response_length_hint(&mut self, request_id: &Request, length: usize);
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FilteredMetadata {
    Basic(Metadata),
    Cors(Metadata),
    Opaque,
    OpaqueRedirect(ServoUrl),
}

// FIXME: https://github.com/servo/servo/issues/34591
#[expect(clippy::large_enum_variant)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FetchMetadata {
    Unfiltered(Metadata),
    Filtered {
        filtered: FilteredMetadata,
        unsafe_: Metadata,
    },
}

impl FetchMetadata {
    pub fn metadata(&self) -> &Metadata {
        match self {
            Self::Unfiltered(metadata) => metadata,
            Self::Filtered { unsafe_, .. } => unsafe_,
        }
    }

    /// <https://html.spec.whatwg.org/multipage/#cors-cross-origin>
    pub fn is_cors_cross_origin(&self) -> bool {
        if let Self::Filtered { filtered, .. } = self {
            match filtered {
                FilteredMetadata::Basic(_) | FilteredMetadata::Cors(_) => false,
                FilteredMetadata::Opaque | FilteredMetadata::OpaqueRedirect(_) => true,
            }
        } else {
            false
        }
    }
}

impl FetchTaskTarget for GenericCallback<FetchResponseMsg> {
    fn process_request_body(&mut self, request: &Request) {
        let _ = self.send(FetchResponseMsg::ProcessRequestBody(request.id));
    }

    fn process_response(&mut self, request: &Request, response: &Response) {
        let _ = self.send(FetchResponseMsg::ProcessResponse(
            request.id,
            response.metadata(),
        ));
    }

    fn process_response_chunk(&mut self, request: &Request, chunk: bytes::Bytes) {
        let _ = self.send(FetchResponseMsg::ProcessResponseChunk(request.id, chunk));
    }

    fn process_response_eof(&mut self, request: &Request, response: &Response) {
        let result = response
            .get_network_error()
            .map_or_else(|| Ok(()), |network_error| Err(network_error.clone()));
        let timing = response.get_resource_timing().inner().clone();

        let _ = self.send(FetchResponseMsg::ProcessResponseEOF(
            request.id, result, timing,
        ));
    }

    fn process_csp_violations(&mut self, request: &Request, violations: Vec<csp::Violation>) {
        let _ = self.send(FetchResponseMsg::ProcessCspViolations(
            request.id, violations,
        ));
    }

    fn process_response_length_hint(&mut self, request: &Request, length: usize) {
        let _ = self.send(FetchResponseMsg::ProcessContentLength(request.id, length));
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsSecurityState {
    /// The connection used to fetch this resource was not secure.
    #[default]
    Insecure,
    /// This resource was transferred over a connection that used weak encryption.
    Weak,
    /// A security error prevented the resource from being loaded.
    Broken,
    /// The connection used to fetch this resource was secure.
    Secure,
}

impl Display for TlsSecurityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            TlsSecurityState::Insecure => "insecure",
            TlsSecurityState::Weak => "weak",
            TlsSecurityState::Broken => "broken",
            TlsSecurityState::Secure => "secure",
        };
        f.write_str(text)
    }
}

#[serde_as]
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ServoProtocolVersion(#[serde_as(as = "FromInto<u16>")] pub ProtocolVersion);

impl malloc_size_of::MallocSizeOf for ServoProtocolVersion {
    fn size_of(&self, _: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        0
    }
}

impl std::fmt::Debug for ServoProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ProtocolVersion::SSLv2 => write!(f, "SSL 2.0"),
            ProtocolVersion::SSLv3 => write!(f, "SSL 3.0"),
            ProtocolVersion::TLSv1_0 => write!(f, "TLS 1.0"),
            ProtocolVersion::TLSv1_1 => write!(f, "TLS 1.1"),
            ProtocolVersion::TLSv1_2 => write!(f, "TLS 1.2"),
            ProtocolVersion::TLSv1_3 => write!(f, "TLS 1.3"),
            ProtocolVersion::DTLSv1_0 => write!(f, "DTLS 1.0"),
            ProtocolVersion::DTLSv1_2 => write!(f, "DTLS 1.2"),
            ProtocolVersion::DTLSv1_3 => write!(f, "DTLS 1.3"),
            ProtocolVersion::Unknown(value) => write!(f, "Unknown ({value})"),
            _ => write!(f, "Not yet implemented"),
        }
    }
}

#[serde_as]
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ServoCipherSuite(#[serde_as(as = "FromInto<u16>")] pub CipherSuite);

impl malloc_size_of::MallocSizeOf for ServoCipherSuite {
    fn size_of(&self, _: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        0
    }
}

impl std::fmt::Debug for ServoCipherSuite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[serde_as]
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ServoNamedGroup(#[serde_as(as = "FromInto<u16>")] pub NamedGroup);

impl malloc_size_of::MallocSizeOf for ServoNamedGroup {
    fn size_of(&self, _: &mut malloc_size_of::MallocSizeOfOps) -> usize {
        0
    }
}

impl std::fmt::Debug for ServoNamedGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct TlsSecurityInfo {
    // "insecure", "weak", "broken", "secure".
    #[serde(default)]
    pub state: TlsSecurityState,
    // Reasons explaining why the negotiated parameters are considered weak.
    pub weakness_reasons: Vec<String>,
    // Negotiated TLS protocol version (e.g. "TLS 1.3").
    pub protocol_version: Option<ServoProtocolVersion>,
    // Negotiated cipher suite identifier.
    pub cipher_suite: Option<ServoCipherSuite>,
    // Negotiated key exchange group.
    pub kea_group_name: Option<ServoNamedGroup>,
    // Signature scheme used for certificate verification.
    pub signature_scheme_name: Option<String>,
    // Negotiated ALPN protocol (e.g. "h2" for HTTP/2, "http/1.1" for HTTP/1.1).
    pub alpn_protocol: Option<String>,
    // Server certificate chain encoded as DER bytes, leaf first.
    pub certificate_chain_der: Vec<Vec<u8>>,
    // Certificate Transparency status, if provided.
    pub certificate_transparency: Option<String>,
    // HTTP Strict Transport Security flag.
    pub hsts: bool,
    // HTTP Public Key Pinning flag (always false, kept for parity).
    pub hpkp: bool,
    // Encrypted Client Hello usage flag.
    pub used_ech: bool,
    // Delegated credentials usage flag.
    pub used_delegated_credentials: bool,
    // OCSP stapling usage flag.
    pub used_ocsp: bool,
    // Private DNS usage flag.
    pub used_private_dns: bool,
}

impl FetchTaskTarget for ProfileGenericCallback<WebSocketNetworkEvent> {
    fn process_request_body(&mut self, _: &Request) {}
    fn process_response(&mut self, _: &Request, response: &Response) {
        if response.is_network_error() {
            let _ = self.send(WebSocketNetworkEvent::Fail);
        }
    }
    fn process_response_chunk(&mut self, _: &Request, _: bytes::Bytes) {}
    fn process_response_eof(&mut self, _: &Request, _: &Response) {}
    fn process_csp_violations(&mut self, _: &Request, violations: Vec<csp::Violation>) {
        let _ = self.send(WebSocketNetworkEvent::ReportCSPViolations(violations));
    }
    fn process_response_length_hint(&mut self, _: &Request, _: usize) {}
}

/// A fetch task that discards all data it's sent,
/// useful when speculatively prefetching data that we don't need right
/// now, but might need in the future.
pub struct DiscardFetch;

impl FetchTaskTarget for DiscardFetch {
    fn process_request_body(&mut self, _: &Request) {}
    fn process_response(&mut self, _: &Request, _: &Response) {}
    fn process_response_chunk(&mut self, _: &Request, _: bytes::Bytes) {}
    fn process_response_eof(&mut self, _: &Request, _: &Response) {}
    fn process_csp_violations(&mut self, _: &Request, _: Vec<csp::Violation>) {}
    fn process_response_length_hint(&mut self, _: &Request, _: usize) {}
}

/// Handle to an async runtime,
/// only used to shut it down for now.
pub trait AsyncRuntime: Send {
    fn shutdown(&mut self);
}

/// Handle to a resource thread
pub type CoreResourceThread = GenericSender<CoreResourceMsg>;

// FIXME: Originally we will construct an Arc<ResourceThread> from ResourceThread
// in script_thread to avoid some performance pitfall. Now we decide to deal with
// the "Arc" hack implicitly in future.
// See discussion: http://logs.glob.uno/?c=mozilla%23servo&s=16+May+2016&e=16+May+2016#c430412
// See also: https://github.com/servo/servo/blob/735480/components/script/script_thread.rs#L313
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResourceThreads {
    pub core_thread: CoreResourceThread,
    /// BAO PATCH (REQ-CDS-009 C6): when set, every message sent through this
    /// handle is wrapped in [`CoreResourceMsg::Partition`] carrying this
    /// webview identity. Page-attributed handles (Window globals, and the
    /// workers that inherit them) stamp; the bare channel pair
    /// (browser-level faces — NetworkManager / SiteDataManager / FetchThread)
    /// does not, so its messages keep the channel-implied public/private
    /// routing.
    pub partition: Option<WebViewId>,
}

impl ResourceThreads {
    pub fn new(core_thread: CoreResourceThread) -> ResourceThreads {
        ResourceThreads {
            core_thread,
            partition: None,
        }
    }

    /// BAO PATCH (REQ-CDS-009 C6): a page-attributed view of the channel —
    /// every send is stamped with the webview identity for partition
    /// resolution on the net router side.
    pub fn for_webview(core_thread: CoreResourceThread, webview_id: WebViewId) -> ResourceThreads {
        ResourceThreads {
            core_thread,
            partition: Some(webview_id),
        }
    }

    pub fn cache_entries(&self) -> Vec<CacheEntryDescriptor> {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self
            .core_thread
            .send(CoreResourceMsg::GetCacheEntries(sender));
        receiver.recv().unwrap()
    }

    pub fn clear_cache(&self) {
        // NOTE: Messages used in these methods are currently handled
        // synchronously on the backend without consulting other threads, so
        // waiting for the response here cannot deadlock. If the backend
        // handling ever becomes asynchronous or involves sending messages
        // back to the originating thread, this code will need to be revisited
        // to avoid potential deadlocks.
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self
            .core_thread
            .send(CoreResourceMsg::ClearCache(Some(sender)));
        let _ = receiver.recv();
    }

    pub fn cookies(&self) -> Vec<SiteDescriptor> {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::ListCookies(sender));
        receiver.recv().unwrap()
    }

    pub fn clear_cookies_for_sites(&self, sites: &[&str]) {
        let sites = sites.iter().map(|site| site.to_string()).collect();
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::DeleteCookiesForSites(sites, sender));
        let _ = receiver.recv();
    }

    pub fn clear_cookies(&self) {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::DeleteCookies(None, Some(sender)));
        let _ = receiver.recv();
    }

    pub fn cookies_for_url(&self, url: ServoUrl, source: CookieSource) -> Vec<Cookie<'static>> {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::GetCookiesForUrl(url, sender, source));
        receiver
            .recv()
            .unwrap()
            .into_iter()
            .map(|cookie| cookie.into_inner())
            .collect()
    }

    pub fn clear_session_cookies(&self) {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::DeleteSessionCookies(sender));
        let _ = receiver.recv();
    }

    pub fn set_cookie_for_url(&self, url: ServoUrl, cookie: Cookie<'static>, source: CookieSource) {
        let _ = self.send(CoreResourceMsg::SetCookieForUrl(
            url,
            Serde(cookie),
            source,
            None,
        ));
    }

    pub fn set_cookie_for_url_sync(
        &self,
        url: ServoUrl,
        cookie: Cookie<'static>,
        source: CookieSource,
    ) {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::SetCookieForUrl(
            url,
            Serde(cookie),
            source,
            Some(sender),
        ));
        let _ = receiver.recv();
    }

    pub fn cookies_for_url_async(
        &self,
        id: CookieOperationId,
        url: ServoUrl,
        source: CookieSource,
    ) {
        let _ = self
            .core_thread
            .send(CoreResourceMsg::EmbedderGetCookiesForUrl(id, url, source));
    }

    pub fn set_cookie_for_url_async(
        &self,
        id: CookieOperationId,
        url: ServoUrl,
        cookie: Cookie<'static>,
        source: CookieSource,
    ) {
        let _ = self
            .core_thread
            .send(CoreResourceMsg::EmbedderSetCookieForUrl(
                id,
                url,
                Serde(cookie),
                source,
            ));
    }

    pub fn clear_cookies_async(&self, id: CookieOperationId) {
        let _ = self
            .core_thread
            .send(CoreResourceMsg::EmbedderClearCookies(id));
    }

    pub fn clear_session_cookies_async(&self, id: CookieOperationId) {
        // BAO PATCH (REQ-ARCH-3 F4-W3): the embedder async face keeps the bare
        // channel (servo.rs `site_data_manager` response-chain domain, W1);
        // the page-attributed family senders below stamp through
        // [`GenericSend::send`].
        let _ = self
            .core_thread
            .send(CoreResourceMsg::EmbedderClearSessionCookies(id));
    }

    // Family senders for the `CoreResourceMsg` cookie family (REQ-ARCH-3
    // F4-W3), mirroring the consumer-side family handler in
    // `resource_thread.rs` (`handle_cookie_message`). Every family sender
    // forwards verbatim through `GenericSend::send`, so page-attributed
    // handles keep stamping these messages with their webview identity
    // (REQ-CDS-009 C6); payloads, ordering, and timing are unchanged.
    pub fn cookie_string_for_url(&self, url: ServoUrl, source: CookieSource) -> Option<String> {
        let (sender, receiver) = generic_channel::channel().unwrap();
        let _ = self.send(CoreResourceMsg::GetCookieStringForUrl(url, sender, source));
        receiver.recv().unwrap()
    }

    pub fn set_cookies_for_url(
        &self,
        url: ServoUrl,
        cookies: Vec<Cookie<'static>>,
        source: CookieSource,
    ) -> SendResult {
        self.send(CoreResourceMsg::SetCookiesForUrl(
            url,
            cookies.into_iter().map(Serde).collect(),
            source,
        ))
    }

    pub fn clear_cookies_for_url(&self, url: ServoUrl) -> SendResult {
        self.send(CoreResourceMsg::DeleteCookies(Some(url), None))
    }

    pub fn delete_cookie_for_url(&self, url: ServoUrl, name: String) -> SendResult {
        self.send(CoreResourceMsg::DeleteCookie(url, name))
    }

    pub fn cookie_store_register_listener(
        &self,
        id: CookieStoreId,
        callback: GenericCallback<CookieAsyncResponse>,
        url: ServoUrl,
    ) -> SendResult {
        self.send(CoreResourceMsg::NewCookieListener(id, callback, url))
    }

    pub fn cookie_store_unregister_listener(&self, id: CookieStoreId) -> SendResult {
        self.send(CoreResourceMsg::RemoveCookieListener(id))
    }

    pub fn cookie_store_get(
        &self,
        id: CookieStoreId,
        url: ServoUrl,
        name: Option<String>,
    ) -> SendResult {
        self.send(CoreResourceMsg::GetCookieDataForUrlAsync(id, url, name))
    }

    pub fn cookie_store_get_all(
        &self,
        id: CookieStoreId,
        url: ServoUrl,
        name: Option<String>,
    ) -> SendResult {
        self.send(CoreResourceMsg::GetAllCookieDataForUrlAsync(id, url, name))
    }

    pub fn cookie_store_set(
        &self,
        id: CookieStoreId,
        url: ServoUrl,
        cookie: Cookie<'static>,
        source: CookieSource,
    ) -> SendResult {
        self.send(CoreResourceMsg::SetCookieForUrlAsync(id, url, Serde(cookie), source))
    }

    pub fn cookie_store_delete(
        &self,
        id: CookieStoreId,
        url: ServoUrl,
        name: String,
    ) -> SendResult {
        self.send(CoreResourceMsg::DeleteCookieAsync(id, url, name))
    }

    // Family senders for the `CoreResourceMsg` history / cache-file / infra
    // families (REQ-ARCH-3 F4-W4), mirroring the consumer-side family
    // handlers in `resource_thread.rs` (`handle_history_message` /
    // `handle_cache_message` / `handle_file_manager_message` /
    // `handle_keep_alive_message` / `handle_exit_message`). Every family
    // sender forwards verbatim through `GenericSend::send`, so
    // page-attributed handles keep stamping these messages with their
    // webview identity (REQ-CDS-009 C6) — except
    // `total_size_of_in_flight_keep_alive_records`, which keeps the bare
    // channel of its pre-wave shape; this wave is parity-preserving
    // (message flow unchanged).

    /// Reads the structured-clone state stored for a history state id
    /// (History face). Blocks on the net thread's reply.
    pub fn history_state(
        &self,
        state_id: HistoryStateId,
        time_profile_chan: ProfilerChan,
    ) -> Option<Vec<u8>> {
        let (sender, receiver) =
            profile_traits::generic_channel::channel(time_profile_chan).unwrap();
        let _ = self.send(CoreResourceMsg::GetHistoryState(state_id, sender));
        receiver.recv().unwrap()
    }

    pub fn set_history_state(&self, state_id: HistoryStateId, serialized: Vec<u8>) -> SendResult {
        self.send(CoreResourceMsg::SetHistoryState(state_id, serialized))
    }

    pub fn remove_history_states(&self, states: Vec<HistoryStateId>) -> SendResult {
        self.send(CoreResourceMsg::RemoveHistoryStates(states))
    }

    /// Forwards a file-manager command opaquely (BlobFile face). The nested
    /// `FileManagerThreadMsg` is a black box at this seam (DEC-ARCH-001 ⑥).
    pub fn to_file_manager(&self, msg: FileManagerThreadMsg) -> SendResult {
        self.send(CoreResourceMsg::ToFileManager(msg))
    }

    /// Infra: total size of in-flight keep-alive records. Keeps the bare
    /// channel of the pre-wave shape (`GlobalScope::core_resource_thread`);
    /// send failures degrade to `u64::MAX` exactly as before, so a dead
    /// resource thread never blocks keep-alive requests.
    pub fn total_size_of_in_flight_keep_alive_records(&self, pipeline_id: PipelineId) -> u64 {
        let (sender, receiver) = generic_channel::channel().unwrap();
        if self
            .core_thread
            .send(CoreResourceMsg::TotalSizeOfInFlightKeepAliveRecords(
                pipeline_id, sender,
            ))
            .is_err()
        {
            return u64::MAX;
        }
        receiver.recv().unwrap_or(u64::MAX)
    }

    /// Asks the core resource thread to persist its state and exit. The
    /// oneshot sender is answered by the net thread's Exit arm before the
    /// thread stops.
    pub fn exit(&self, sender: GenericOneshotSender<()>) -> SendResult {
        self.send(CoreResourceMsg::Exit(sender))
    }

    // Family sender for the `CoreResourceMsg` fetch family (REQ-ARCH-3
    // F4-W5), mirroring the consumer-side family handler in
    // `resource_thread.rs` (`handle_fetch_message`). Forwards verbatim
    // through `GenericSend::send`, so page-attributed handles keep stamping
    // fetches with their webview identity (REQ-CDS-009 C6); the fetch
    // variant additionally self-attributes via
    // `RequestBuilder::target_webview_id` (`resolve_partition` pre-match).
    // Payloads and timing are unchanged. The bare-channel face of this
    // family sender lives in `script::fetch::fetch::send_fetch`.

    /// Sends a fetch to the core resource thread through this handle's
    /// registered send face (envelope-stamped for page-attributed handles).
    pub fn fetch(&self, request: RequestBuilder, channels: FetchChannels) -> SendResult {
        self.send(CoreResourceMsg::Fetch(request, channels))
    }
}

impl GenericSend<CoreResourceMsg> for ResourceThreads {
    fn send(&self, document_msg: CoreResourceMsg) -> SendResult {
        // REQ-CDS-009 C6: page-attributed handles stamp every message so the
        // net router can resolve the sending page's context partition.
        match self.partition {
            Some(webview_id) => self
                .core_thread
                .send(CoreResourceMsg::Partition(webview_id, Box::new(document_msg))),
            None => self.core_thread.send(document_msg),
        }
    }

    fn sender(&self) -> GenericSender<CoreResourceMsg> {
        self.core_thread.clone()
    }
}

// Ignore the sub-fields
malloc_size_of_is_0!(ResourceThreads);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum IncludeSubdomains {
    Included,
    NotIncluded,
}

#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum MessageData {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Deserialize, Serialize, MallocSizeOf)]
pub enum WebSocketDomAction {
    SendMessage(MessageData),
    Close(Option<u16>, Option<String>),
    /// REQ-BRW-051: grant the network loop one read credit. The
    /// `WebSocketStream` readable half sends this from its pull algorithm, so
    /// data is only read from the socket when the consumer asks for it.
    RequestRead,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum WebSocketNetworkEvent {
    ReportCSPViolations(Vec<csp::Violation>),
    ConnectionEstablished { protocol_in_use: Option<String> },
    MessageReceived(MessageData),
    /// REQ-BRW-051: one `SendMessage` action has been fully written to the
    /// socket. `WebSocketStream` uses this to resolve write promises (and so
    /// to propagate backpressure); the classic `WebSocket` API ignores it.
    SendComplete,
    Close(Option<u16>, String),
    Fail,
}

#[derive(Debug, Deserialize, Serialize)]
/// IPC channels to communicate with the script thread about network or DOM events.
pub enum FetchChannels {
    ResponseMsg(GenericCallback<FetchResponseMsg>),
    WebSocket {
        event_sender: ProfileGenericCallback<WebSocketNetworkEvent>,
        action_receiver: CallbackSetter<WebSocketDomAction>,
        /// REQ-BRW-051: `true` for `WebSocketStream`, whose readable half is
        /// demand-driven (one `RequestRead` credit per message delivered).
        /// `false` for the classic `WebSocket` API, which reads eagerly.
        flow_control: bool,
    },
    /// If the fetch is just being done to populate the cache,
    /// not because the data is needed now.
    Prefetch,
}

// ── BAO PATCH (REQ-BRW-051): WebBundle subresource-loading wire types ──
//
// `<script type=webbundle>` lives in the script thread (it owns the JSON rule,
// the element lifetime, and the bundle fetch), while every subresource fetch
// settles in the net thread. The registrations therefore cross the core
// resource channel as these messages. The script side resolves rule URLs and
// parses the bundle; the net side only registers/serves the already-decoded
// responses (it does not depend on the `webbundle` parser crate).
//
// `loader_id` identifies the shared script-side loader (one per
// `(bundle URL, credentials mode)` pair, adoptable by a new element during a
// synchronous remove/append), `element_id` identifies one element registration.
// Several elements may reference one loader; the newest registration wins.

/// One element registration: the resolved `resources`/`scopes` of its rule.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebBundleRule {
    pub element_id: u64,
    pub loader_id: u64,
    pub bundle_url: ServoUrl,
    /// Absolute URLs listed in the rule's `resources` member (already filtered
    /// to same-origin-with-bundle and path-restricted entries, plus
    /// `uuid-in-package:` URLs).
    pub resources: Vec<ServoUrl>,
    /// Absolute scope prefixes from the rule's `scopes` member.
    pub scopes: Vec<ServoUrl>,
}

/// One decoded response stored in a bundle.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebBundleResource {
    pub url: ServoUrl,
    pub status: u16,
    pub headers: Serde<HeaderMap>,
    pub body: Vec<u8>,
}

/// The successful load of one bundle: every response plus the bundle fetch's
/// resource timing. Subresources served from this bundle report the bundle's
/// timing (resource timing contract for web bundle subresources).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebBundleLoaderData {
    pub loader_id: u64,
    pub resources: Vec<WebBundleResource>,
    pub timing: ResourceFetchTiming,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum CoreResourceMsg {
    Fetch(RequestBuilder, FetchChannels),
    Cancel(Vec<RequestId>),
    /// BAO PATCH (REQ-CDS-009 C6): partition envelope. A page created inside
    /// an explicit browser context stamps every message its global scope
    /// sends with its own webview identity; the net router unwraps the
    /// envelope, resolves the webview's [`servo_base::id::ContextPartitionId`]
    /// and serves partition-relevant faces (the cookie store, HTTP cache,
    /// HSTS, auth cache) from that context's `HttpState`. Only
    /// page-attributed senders (`ResourceThreads::for_webview`) produce this
    /// variant — the channel-implied public/private routing is untouched for
    /// everything else, so un-partitioned pages keep the pre-C6 behavior.
    Partition(WebViewId, Box<CoreResourceMsg>),
    /// BAO PATCH (REQ-BRW-051): register a `<script type=webbundle>` element's
    /// rule. The loader starts out pending; later subresource requests whose
    /// URL matches `resources` or `scopes` wait for the loader to settle
    /// instead of racing the network. Registering an element id that is
    /// already present replaces it and moves it to the newest position (the
    /// "last added element must be referred" rule).
    WebBundleRegister(PipelineId, WebBundleRule),
    /// BAO PATCH (REQ-BRW-051): the bundle body parsed successfully; every
    /// subresource served from it uses these responses and this timing.
    WebBundleLoaded(PipelineId, WebBundleLoaderData),
    /// BAO PATCH (REQ-BRW-051): the bundle fetch/parse failed. Requests that
    /// were already waiting on the pending loader fail closed (they never fall
    /// back to the network); the element registration is dropped by the script
    /// side at the same time, so requests issued after the failure fall through
    /// to the network (WPT nested-bundle).
    WebBundleFailed(PipelineId, u64),
    /// BAO PATCH (REQ-BRW-051): drop one element registration. Matching
    /// in-flight requests that were waiting on this registration fail
    /// immediately; the shared loader stays cached for a synchronous
    /// remove/append.
    WebBundleUnregister(PipelineId, u64),
    /// BAO PATCH (REQ-BRW-051): the script-side loader was destroyed (orphan
    /// cleanup or document teardown); free its net-side data.
    WebBundleDropLoader(PipelineId, u64),
    /// BAO PATCH (REQ-BRW-051): the document was destroyed; drop every
    /// registration and loader belonging to this pipeline (one document).
    WebBundleClearPipeline(PipelineId),
    /// Initiate a fetch in response to processing a redirection
    FetchRedirect(
        RequestBuilder,
        ResponseInit,
        GenericCallback<FetchResponseMsg>,
    ),
    /// Store a cookie for a given originating URL.
    /// If a sender is provided, the caller will block until the cookie is stored.
    SetCookieForUrl(
        ServoUrl,
        Serde<Cookie<'static>>,
        CookieSource,
        Option<GenericSender<()>>,
    ),
    /// Store a set of cookies for a given originating URL
    SetCookiesForUrl(ServoUrl, Vec<Serde<Cookie<'static>>>, CookieSource),
    SetCookieForUrlAsync(
        CookieStoreId,
        ServoUrl,
        Serde<Cookie<'static>>,
        CookieSource,
    ),
    /// Retrieve the stored cookies as a header string for a given URL.
    GetCookieStringForUrl(ServoUrl, GenericSender<Option<String>>, CookieSource),
    /// Retrieve the stored cookies as a vector for the given URL.
    /// The response is sent via the provided sender.
    GetCookiesForUrl(
        ServoUrl,
        GenericSender<Vec<Serde<Cookie<'static>>>>,
        CookieSource,
    ),
    /// Retrieve cookies for a URL for embedder. The response is
    /// sent via [`NetToEmbedderMsg::EmbedderGetCookiesForUrlResponse`].
    EmbedderGetCookiesForUrl(CookieOperationId, ServoUrl, CookieSource),
    /// Set a cookie for a URL on behalf of the embedder. The response is
    /// sent via [`NetToEmbedderMsg::EmbedderSetCookieForUrlResponse`].
    EmbedderSetCookieForUrl(
        CookieOperationId,
        ServoUrl,
        Serde<Cookie<'static>>,
        CookieSource,
    ),
    /// Clear all cookies on behalf of the embedder. The response is sent via NetToEmbedderMsg.
    EmbedderClearCookies(CookieOperationId),
    /// Clear session cookies on behalf of the embedder. The response is sent via NetToEmbedderMsg.
    EmbedderClearSessionCookies(CookieOperationId),
    GetCookieDataForUrlAsync(CookieStoreId, ServoUrl, Option<String>),
    GetAllCookieDataForUrlAsync(CookieStoreId, ServoUrl, Option<String>),
    DeleteCookiesForSites(Vec<String>, GenericSender<()>),
    /// This currently is used by unit tests and WebDriver only.
    /// When url is `None`, this clears cookies across all origins.
    DeleteCookies(Option<ServoUrl>, Option<GenericSender<()>>),
    /// Delete all session cookies (cookies without an expiry or max-age).
    DeleteSessionCookies(GenericSender<()>),
    DeleteCookie(ServoUrl, String),
    DeleteCookieAsync(CookieStoreId, ServoUrl, String),
    NewCookieListener(
        CookieStoreId,
        GenericCallback<CookieAsyncResponse>,
        ServoUrl,
    ),
    RemoveCookieListener(CookieStoreId),
    ListCookies(GenericSender<Vec<SiteDescriptor>>),
    /// Get a history state by a given history state id
    GetHistoryState(HistoryStateId, GenericSender<Option<Vec<u8>>>),
    /// Set a history state for a given history state id
    SetHistoryState(HistoryStateId, Vec<u8>),
    /// Removes history states for the given ids
    RemoveHistoryStates(Vec<HistoryStateId>),
    /// Gets a list of origin descriptors derived from entries in the cache
    GetCacheEntries(GenericSender<Vec<CacheEntryDescriptor>>),
    /// Clear the network cache.
    ClearCache(Option<GenericSender<()>>),
    /// Send the service worker network mediator for an origin to CoreResourceThread
    NetworkMediator(GenericSender<CustomResponseMediator>, ImmutableOrigin),
    /// Message forwarded to file manager's handler
    ToFileManager(FileManagerThreadMsg),
    TotalSizeOfInFlightKeepAliveRecords(PipelineId, GenericSender<u64>),
    /// Break the load handler loop, send a reply when done cleaning up local resources
    /// and exit
    Exit(GenericOneshotSender<()>),
    CollectMemoryReport(ReportsChan),
    RevokeTokenForFile(BlobTokenRevocationRequest),
    RefreshTokenForFile(BlobTokenRefreshRequest),
    // BAO PATCH (REQ-BRW-051): the `navigator.deviceBoundSession` query
    // face — does the session store of `url`'s site (specifically: the
    // calling page's cookie partition) hold at least one session?
    DbscHasSession(ServoUrl, GenericCallback<bool>),
    // BAO PATCH (REQ-BRW-051): the `navigator.deviceBoundSession.clearSession`
    // face — end the identified session, or (identifier `None`) every
    // session of `url`'s origin.
    DbscClearSession(ServoUrl, Option<String>, GenericCallback<Result<(), String>>),
}

#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct BlobTokenRevocationRequest {
    pub blob_id: Uuid,
    pub token: Uuid,
}

#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct BlobTokenRefreshRequest {
    pub blob_id: Uuid,
    pub new_token_sender: GenericSender<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SiteDescriptor {
    pub name: String,
}

impl SiteDescriptor {
    pub fn new(name: String) -> Self {
        SiteDescriptor { name }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheEntryDescriptor {
    pub key: String,
}

impl CacheEntryDescriptor {
    pub fn new(key: String) -> Self {
        Self { key }
    }
}

// FIXME: https://github.com/servo/servo/issues/34591
#[expect(clippy::large_enum_variant)]
enum ToFetchThreadMessage {
    Cancel(Vec<RequestId>, CoreResourceThread),
    StartFetch(
        /* request_builder */ RequestBuilder,
        /* response_init */ Option<ResponseInit>,
        /* callback  */ BoxedFetchCallback,
        /* core resource thread channel */ CoreResourceThread,
    ),
    FetchResponse(FetchResponseMsg),
    /// BAO PATCH (REQ-BRW-051): a `<script type=webbundle>` registration
    /// message. It rides the same FIFO as this thread's fetches so the
    /// dispatch-time web bundle binding in the resource thread observes the
    /// registrations in exactly the order the script issued them: a fetch
    /// issued before an element removal is bound to that element, while a
    /// fetch issued after the removal is not.
    WebBundle(CoreResourceThread, CoreResourceMsg),
    /// Stop the background thread.
    Exit,
}

pub type BoxedFetchCallback = Box<dyn FnMut(FetchResponseMsg) + Send + 'static>;

/// A thread to handle fetches in a Servo process. This thread is responsible for
/// listening for new fetch requests as well as updates on those operations and forwarding
/// them to crossbeam channels.
pub struct FetchThread {
    /// A list of active fetches. A fetch is no longer active once the
    /// [`FetchResponseMsg::ProcessResponseEOF`] is received.
    active_fetches: FxHashMap<RequestId, BoxedFetchCallback>,
    /// A crossbeam receiver attached to the router proxy which converts incoming fetch
    /// updates from IPC messages to crossbeam messages as well as another sender which
    /// handles requests from clients wanting to do fetches.
    receiver: Receiver<ToFetchThreadMessage>,
    /// An [`IpcSender`] that's sent with every fetch request and leads back to our
    /// router proxy.
    to_fetch_sender: GenericCallback<FetchResponseMsg>,
}

impl FetchThread {
    fn spawn() -> FetchThreadHandle {
        let (sender, receiver) = unbounded();

        let sender_clone = sender.clone();
        let to_fetch_sender = GenericCallback::new(move |message| {
            let message: FetchResponseMsg = message.unwrap();
            let _ = sender_clone.send(ToFetchThreadMessage::FetchResponse(message));
        })
        .expect("Couldn't create fetch callback");
        let join_handle = thread::Builder::new()
            .name("FetchThread".to_owned())
            .spawn(move || {
                let mut fetch_thread = FetchThread {
                    active_fetches: FxHashMap::default(),
                    receiver,
                    to_fetch_sender,
                };
                fetch_thread.run();
            })
            .expect("Thread spawning failed");
        FetchThreadHandle {
            sender,
            join_handle: RwLock::new(Some(join_handle)),
        }
    }

    fn run(&mut self) {
        loop {
            match self.receiver.recv() {
                Ok(ToFetchThreadMessage::StartFetch(
                    request_builder,
                    response_init,
                    callback,
                    core_resource_thread,
                )) => {
                    let request_builder_id = request_builder.id;

                    // Only redirects have a `response_init` field.
                    let message = match response_init {
                        Some(response_init) => CoreResourceMsg::FetchRedirect(
                            request_builder,
                            response_init,
                            self.to_fetch_sender.clone(),
                        ),
                        None => CoreResourceMsg::Fetch(
                            request_builder,
                            FetchChannels::ResponseMsg(self.to_fetch_sender.clone()),
                        ),
                    };

                    if core_resource_thread.send(message).is_err() {
                        // In this case the connection with the resource threads has been
                        // broken, so just assume that we are shutting down as any further
                        // messaging is likely to be unreliable.
                        break;
                    }

                    let preexisting_fetch =
                        self.active_fetches.insert(request_builder_id, callback);
                    // When we terminate a fetch group, all deferred fetches are processed.
                    // In case we were already processing a deferred fetch, we should not
                    // process the second call. This should be handled by [`DeferredFetchRecord::process`]
                    assert!(preexisting_fetch.is_none());
                },
                Ok(ToFetchThreadMessage::WebBundle(core_resource_thread, message)) => {
                    if core_resource_thread.send(message).is_err() {
                        // Same shutdown assumption as `StartFetch` above.
                        break;
                    }
                },
                Ok(ToFetchThreadMessage::FetchResponse(fetch_response_msg)) => {
                    let request_id = fetch_response_msg.request_id();
                    let fetch_finished =
                        matches!(fetch_response_msg, FetchResponseMsg::ProcessResponseEOF(..));

                    self.active_fetches
                        .get_mut(&request_id)
                        .expect("Got fetch response for unknown fetch")(
                        fetch_response_msg
                    );

                    if fetch_finished {
                        self.active_fetches.remove(&request_id);
                    }
                },
                Ok(ToFetchThreadMessage::Cancel(request_ids, core_resource_thread)) => {
                    // Errors are ignored here, because Servo sends many cancellation requests when shutting down.
                    // At this point the networking task might be shut down completely, so just ignore errors
                    // during this time.
                    let _ = core_resource_thread.send(CoreResourceMsg::Cancel(request_ids));
                },
                Ok(ToFetchThreadMessage::Exit) | Err(_) => break,
            }
        }
    }

    fn fetch_async(
        core_resource_thread: &CoreResourceThread,
        request: RequestBuilder,
        response_init: Option<ResponseInit>,
        callback: BoxedFetchCallback,
    ) {
        let mut message = ToFetchThreadMessage::StartFetch(
            request,
            response_init,
            callback,
            core_resource_thread.clone(),
        );
        let mut fetch_thread = FETCH_THREAD.lock().unwrap();
        loop {
            let Some(handle) = fetch_thread.as_ref() else {
                *fetch_thread = Some(FetchThread::spawn());
                continue;
            };
            match handle.sender.send(message) {
                Ok(()) => return,
                Err(send_error) => {
                    // The multiplexer thread is gone (a previous instance's
                    // shutdown exited it, or its run loop broke on a dead
                    // resource thread). `SendError` hands the undelivered
                    // message back, so the request is not lost: respawn the
                    // thread and retry on the fresh handle.
                    message = send_error.0;
                    *fetch_thread = Some(FetchThread::spawn());
                },
            }
        }
    }

    fn cancel_async_fetch(request_ids: Vec<RequestId>, core_resource_thread: &CoreResourceThread) {
        if let Some(fetch_thread) = FETCH_THREAD.lock().unwrap().as_ref() {
            let _ = fetch_thread.sender.send(ToFetchThreadMessage::Cancel(
                request_ids,
                core_resource_thread.clone(),
            ));
        }
    }

    /// BAO PATCH (REQ-BRW-051): enqueue a web bundle registration message on
    /// the fetch thread's FIFO, respawning the multiplexer if it has exited
    /// (mirrors `fetch_async`'s re-arm handling).
    fn send_web_bundle_message(
        core_resource_thread: &CoreResourceThread,
        message: CoreResourceMsg,
    ) {
        let mut message = ToFetchThreadMessage::WebBundle(
            core_resource_thread.clone(),
            message,
        );
        let mut fetch_thread = FETCH_THREAD.lock().unwrap();
        loop {
            let Some(handle) = fetch_thread.as_ref() else {
                *fetch_thread = Some(FetchThread::spawn());
                continue;
            };
            match handle.sender.send(message) {
                Ok(()) => return,
                Err(send_error) => {
                    message = send_error.0;
                    *fetch_thread = Some(FetchThread::spawn());
                },
            }
        }
    }

    /// If the `FetchThread` is running, send the exit message and wait for it to exit.
    ///
    /// BAO NOTE (e134): this clears the slot, so a subsequent `fetch_async`
    /// respawns the thread instead of reusing the dead handle (process-exit
    /// callers only; instance shutdown no longer calls this — see
    /// `Constellation::run`).
    pub fn exit() {
        let Some(fetch_thread) = FETCH_THREAD.lock().unwrap().take() else {
            return;
        };
        let _ = fetch_thread.sender.send(ToFetchThreadMessage::Exit);
        if let Some(join_handle) = fetch_thread.join_handle.write().take() {
            join_handle
                .join()
                .expect("Failed to join on the FetchThread join handle.");
        }
    }
}

struct FetchThreadHandle {
    sender: Sender<ToFetchThreadMessage>,
    join_handle: RwLock<Option<JoinHandle<()>>>,
}

// BAO PATCH (e134, REQ-BRW-002 multi-runtime): the FetchThread is a
// PROCESS-wide async-fetch multiplexer — every request carries its own
// `core_resource_thread`, so the thread itself is shared by all Servo
// instances in the process. Upstream stored the handle in a `OnceLock` and
// `Constellation`'s instance shutdown called `FetchThread::exit()`, which
// permanently killed the thread while leaving the dead handle cached: a
// SECOND Servo instance in the same process (embedder multi-runtime shape)
// then had every `fetch_async` call silently drop its request
// (`sender.send` on the dead handle fails into `let _ =`), so all of that
// instance's network loads — initial page loads included — never started.
// The slot is re-armable: `fetch_async` respawns the thread when the
// previous one is gone (instance shutdown, or the run loop's break on a
// dead resource thread).
static FETCH_THREAD: std::sync::Mutex<Option<FetchThreadHandle>> =
    std::sync::Mutex::new(None);

/// Instruct the fetch thread to start a new asynchronous fetch request.
pub fn fetch_async(
    core_resource_thread: &CoreResourceThread,
    request: RequestBuilder,
    response_init: Option<ResponseInit>,
    callback: BoxedFetchCallback,
) {
    FetchThread::fetch_async(core_resource_thread, request, response_init, callback);
}

/// BAO PATCH (REQ-BRW-051): deliver a web bundle registration message through
/// the fetch thread's FIFO, so it is ordered with the fetches this script
/// thread issues (see [`ToFetchThreadMessage::WebBundle`]).
pub fn send_web_bundle_message(
    core_resource_thread: &CoreResourceThread,
    message: CoreResourceMsg,
) {
    FetchThread::send_web_bundle_message(core_resource_thread, message);
}

/// Instruct the resource thread to cancel an existing request. Does nothing if the
/// request has already completed or has not been fetched yet.
pub fn cancel_async_fetch(request_ids: Vec<RequestId>, core_resource_thread: &CoreResourceThread) {
    FetchThread::cancel_async_fetch(request_ids, core_resource_thread);
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ResourceCorsData {
    /// CORS Preflight flag
    pub preflight: bool,
    /// Origin of CORS Request
    pub origin: ServoUrl,
}

/// Metadata about a loaded resource, such as is obtained from HTTP headers.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct Metadata {
    /// Final URL after redirects.
    pub final_url: ServoUrl,

    /// Location URL from the response headers.
    pub location_url: Option<Result<ServoUrl, String>>,

    #[ignore_malloc_size_of = "Defined in hyper"]
    /// MIME type / subtype.
    pub content_type: Option<Serde<ContentType>>,

    /// Character set.
    pub charset: Option<String>,

    #[ignore_malloc_size_of = "Defined in hyper"]
    /// Headers
    pub headers: Option<Serde<HeaderMap>>,

    /// HTTP Status
    pub status: HttpStatus,

    /// Referrer Url
    pub referrer: Option<ServoUrl>,

    /// Referrer Policy of the Request used to obtain Response
    pub referrer_policy: ReferrerPolicy,
    /// Performance information for navigation events
    pub timing: Option<ResourceFetchTiming>,
    /// True if the request comes from a redirection
    pub redirected: bool,
    /// Detailed TLS metadata associated with the response, if any.
    pub tls_security_info: Option<TlsSecurityInfo>,
}

impl Metadata {
    /// Metadata with defaults for everything optional.
    pub fn default(url: ServoUrl) -> Self {
        Metadata {
            final_url: url,
            location_url: None,
            content_type: None,
            charset: None,
            headers: None,
            status: HttpStatus::default(),
            referrer: None,
            referrer_policy: ReferrerPolicy::EmptyString,
            timing: None,
            redirected: false,
            tls_security_info: None,
        }
    }

    /// Extract the parts of a Mime that we care about.
    pub fn set_content_type(&mut self, content_type: Option<&Mime>) {
        if self.headers.is_none() {
            self.headers = Some(Serde(HeaderMap::new()));
        }

        if let Some(mime) = content_type {
            self.headers
                .as_mut()
                .unwrap()
                .typed_insert(ContentType::from(mime.clone()));
            if let Some(charset) = mime.get_param(mime::CHARSET) {
                self.charset = Some(charset.to_string());
            }
            self.content_type = Some(Serde(ContentType::from(mime.clone())));
        }
    }

    /// Set the referrer policy associated with the loaded resource.
    pub fn set_referrer_policy(&mut self, referrer_policy: ReferrerPolicy) {
        if referrer_policy == ReferrerPolicy::EmptyString {
            return;
        }

        if self.headers.is_none() {
            self.headers = Some(Serde(HeaderMap::new()));
        }

        self.referrer_policy = referrer_policy;

        self.headers
            .as_mut()
            .unwrap()
            .typed_insert::<ReferrerPolicyHeader>(referrer_policy.into());
    }

    /// <https://html.spec.whatwg.org/multipage/#content-type>
    pub fn resource_content_type_metadata(&self, load_context: LoadContext, data: &[u8]) -> Mime {
        // The Content-Type metadata of a resource must be obtained and interpreted in a manner consistent with the requirements of MIME Sniffing. [MIMESNIFF]
        let no_sniff = self
            .headers
            .as_deref()
            .is_some_and(determine_nosniff)
            .into();
        let mime = self
            .content_type
            .clone()
            .map(|content_type| content_type.into_inner().into());
        MimeClassifier::default().classify(
            load_context,
            no_sniff,
            ApacheBugFlag::from_content_type(mime.as_ref()),
            &mime,
            data,
        )
    }
}

/// The creator of a given cookie
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum CookieSource {
    /// An HTTP API
    HTTP,
    /// A non-HTTP API
    NonHTTP,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CookieChange {
    changed: Vec<Serde<Cookie<'static>>>,
    deleted: Vec<Serde<Cookie<'static>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum CookieData {
    Change(CookieChange),
    Get(Option<Serde<Cookie<'static>>>),
    GetAll(Vec<Serde<Cookie<'static>>>),
    Set(Result<(), ()>),
    Delete(Result<(), ()>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CookieAsyncResponse {
    pub data: CookieData,
}

/// Network errors that have to be exported out of the loaders
#[derive(Clone, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub enum NetworkError {
    LoadCancelled,
    /// SSL validation error, to be converted to Resource::BadCertHTML in the HTML parser.
    SslValidation(String, Vec<u8>),
    /// Crash error, to be converted to Resource::Crash in the HTML parser.
    Crash(String),
    UnsupportedScheme,
    CorsGeneral,
    CrossOriginResponse,
    CorsCredentials,
    CorsAllowMethods,
    CorsAllowHeaders,
    CorsMethod,
    CorsAuthorization,
    CorsHeaders,
    ConnectionFailure,
    RedirectError,
    TooManyRedirects,
    TooManyInFlightKeepAliveRequests,
    InvalidMethod,
    ResourceLoadError(String),
    ContentSecurityPolicy,
    Nosniff,
    MimeType(String),
    SubresourceIntegrity,
    MixedContent,
    CacheError,
    InvalidPort,
    WebsocketConnectionFailure(String),
    LocalDirectoryError,
    PartialResponseToNonRangeRequestError,
    ProtocolHandlerSubstitutionError,
    BlobURLStoreError(String),
    HttpError(String),
    DecompressionError,
}

impl fmt::Debug for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkError::UnsupportedScheme => write!(f, "Unsupported scheme"),
            NetworkError::CorsGeneral => write!(f, "CORS check failed"),
            NetworkError::CrossOriginResponse => write!(f, "Cross-origin response"),
            NetworkError::CorsCredentials => write!(f, "Cross-origin credentials check failed"),
            NetworkError::CorsAllowMethods => write!(f, "CORS ACAM check failed"),
            NetworkError::CorsAllowHeaders => write!(f, "CORS ACAH check failed"),
            NetworkError::CorsMethod => write!(f, "CORS method check failed"),
            NetworkError::CorsAuthorization => write!(f, "CORS authorization check failed"),
            NetworkError::CorsHeaders => write!(f, "CORS headers check failed"),
            NetworkError::ConnectionFailure => write!(f, "Request failed"),
            NetworkError::RedirectError => write!(f, "Redirect failed"),
            NetworkError::TooManyRedirects => write!(f, "Too many redirects"),
            NetworkError::TooManyInFlightKeepAliveRequests => {
                write!(f, "Too many in flight keep-alive requests")
            },
            NetworkError::InvalidMethod => write!(f, "Unexpected method"),
            NetworkError::ResourceLoadError(s) => write!(f, "{}", s),
            NetworkError::ContentSecurityPolicy => write!(f, "Blocked by Content-Security-Policy"),
            NetworkError::Nosniff => write!(f, "Blocked by nosniff"),
            NetworkError::MimeType(s) => write!(f, "{}", s),
            NetworkError::SubresourceIntegrity => {
                write!(f, "Subresource integrity validation failed")
            },
            NetworkError::MixedContent => write!(f, "Blocked as mixed content"),
            NetworkError::CacheError => write!(f, "Couldn't find response in cache"),
            NetworkError::InvalidPort => write!(f, "Request attempted on bad port"),
            NetworkError::LocalDirectoryError => write!(f, "Local directory access failed"),
            NetworkError::LoadCancelled => write!(f, "Load cancelled"),
            NetworkError::SslValidation(s, _) => write!(f, "SSL validation error: {}", s),
            NetworkError::Crash(s) => write!(f, "Crash: {}", s),
            NetworkError::PartialResponseToNonRangeRequestError => write!(
                f,
                "Refusing to provide partial response from earlier ranged request to API that did not make a range request"
            ),
            NetworkError::ProtocolHandlerSubstitutionError => {
                write!(f, "Failed to parse substituted protocol handler url")
            },
            NetworkError::BlobURLStoreError(s) => write!(f, "Blob URL store error: {}", s),
            NetworkError::WebsocketConnectionFailure(s) => {
                write!(f, "Websocket connection failure: {}", s)
            },
            NetworkError::HttpError(s) => write!(f, "HTTP failure: {}", s),
            NetworkError::DecompressionError => write!(f, "Decompression error"),
        }
    }
}

impl NetworkError {
    pub fn is_permanent_failure(&self) -> bool {
        matches!(
            self,
            NetworkError::ContentSecurityPolicy |
                NetworkError::MixedContent |
                NetworkError::SubresourceIntegrity |
                NetworkError::Nosniff |
                NetworkError::InvalidPort |
                NetworkError::CorsGeneral |
                NetworkError::CrossOriginResponse |
                NetworkError::CorsCredentials |
                NetworkError::CorsAllowMethods |
                NetworkError::CorsAllowHeaders |
                NetworkError::CorsMethod |
                NetworkError::CorsAuthorization |
                NetworkError::CorsHeaders |
                NetworkError::UnsupportedScheme
        )
    }

    pub fn from_hyper_error(error: &HyperError, certificate: Option<CertificateDer>) -> Self {
        let error_string = error.to_string();
        match certificate {
            Some(certificate) => NetworkError::SslValidation(error_string, certificate.to_vec()),
            _ => NetworkError::HttpError(error_string),
        }
    }
}

/// Normalize `slice`, as defined by
/// [the Fetch Spec](https://fetch.spec.whatwg.org/#concept-header-value-normalize).
pub fn trim_http_whitespace(mut slice: &[u8]) -> &[u8] {
    const HTTP_WS_BYTES: &[u8] = b"\x09\x0A\x0D\x20";

    loop {
        match slice.split_first() {
            Some((first, remainder)) if HTTP_WS_BYTES.contains(first) => slice = remainder,
            _ => break,
        }
    }

    loop {
        match slice.split_last() {
            Some((last, remainder)) if HTTP_WS_BYTES.contains(last) => slice = remainder,
            _ => break,
        }
    }

    slice
}

/// Returns true if a given string has a given suffix with case-insensitive match.
pub fn ends_with_ignore_ascii_case(string: &str, suffix: &str) -> bool {
    string.len() >= suffix.len() &&
        string.as_bytes()[string.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Returns the cached current system locale, or en-US by default.
pub fn get_current_locale() -> &'static (String, HeaderValue) {
    static CURRENT_LOCALE: OnceLock<(String, HeaderValue)> = OnceLock::new();

    CURRENT_LOCALE.get_or_init(|| {
        let locale_override = servo_config::pref!(intl_locale_override);
        let locale = if locale_override.is_empty() {
            sys_locale::get_locale().unwrap_or_else(|| "en-US".into())
        } else {
            locale_override
        };
        let header_value = HeaderValue::from_str(&locale)
            .ok()
            .unwrap_or_else(|| HeaderValue::from_static("en-US"));
        (locale, header_value)
    })
}

/// Step 12 of <https://fetch.spec.whatwg.org/#concept-fetch>
pub fn set_default_accept_language(headers: &mut HeaderMap) {
    // If request’s header list does not contain `Accept-Language`,
    // then user agents should append (`Accept-Language, an appropriate header value) to request’s header list.
    if headers.contains_key(header::ACCEPT_LANGUAGE) {
        return;
    }

    // To reduce fingerprinting we set only a single language.
    headers.insert(header::ACCEPT_LANGUAGE, get_current_locale().1.clone());
}

pub static PRIVILEGED_SECRET: LazyLock<u32> = LazyLock::new(|| rng().next_u32());
