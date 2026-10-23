/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Issuing navigational prefetch requests for speculation candidates.
//!
//! BAO PATCH (REQ-BRW-051 C2, GOLDEN B): this is the `net` seam used by the
//! speculation rules implementation. A prefetch is a navigation-shaped fetch
//! (<https://wicg.github.io/nav-speculation/prefetch.html#create-a-navigation-request>):
//!
//! * mode `navigate`, destination `document`, redirect mode `follow`,
//! * `Sec-Purpose: prefetch` (added by `http_loader` for
//!   `Initiator::Prefetch`, or set here with the `anonymous-client-ip`
//!   parameter when the rule asked for it),
//! * `Sec-Speculation-Tags` carrying the collected tags,
//! * same-origin candidates use the document's credentials; cross-origin
//!   candidates are sent without credentials.
//!
//! The response is never surfaced to content: the listener discards the body.
//!
//! Known gap (deliberate, tracked by the Wave C plan): Bao does not keep a
//! prefetch cache, so a later navigation re-fetches the URL instead of reusing
//! the prefetched response. That affects `deliveryType`, `No-Vary-Search`
//! matching and the "navigation request carries `Sec-Purpose`" assertions in
//! WPT; the wire-level prefetch request itself is emitted correctly.

use bytes::Bytes;
use content_security_policy::Violation;
use http::HeaderValue;
use http::header::HeaderMap;
use js::context::JSContext;
use net_traits::blob_url_store::UrlWithBlobClaim;
use net_traits::request::{
    CacheMode, CredentialsMode, Destination, Initiator, RequestBuilder, RequestId, RequestMode,
};
use net_traits::{FetchMetadata, NetworkError, ResourceFetchTiming};
use servo_url::ServoUrl;

use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::csp::GlobalCspReporting;
use crate::dom::document::Document;
use crate::dom::globalscope::GlobalScope;
use crate::dom::speculation::rules::serialize_sec_speculation_tags;
use crate::fetch::network_listener::FetchResponseListener;

/// Start a referrer-initiated navigational prefetch for `url`.
///
/// Returns the [`RequestId`] of the request so that a rule set that goes away
/// can cancel it.
pub(crate) fn start_prefetch(
    document: &Document,
    url: ServoUrl,
    referrer_policy: net_traits::ReferrerPolicy,
    tags: Vec<Option<String>>,
    anonymous_client_ip: bool,
) -> RequestId {
    let global = document.global();
    let origin = document.origin().immutable().clone();
    let same_origin = origin == url.origin();

    // Cross-site speculative loads are always credentialless
    // (<https://html.spec.whatwg.org/multipage/speculative-loading.html#partitioning>).
    let credentials_mode = if same_origin {
        CredentialsMode::Include
    } else {
        CredentialsMode::Omit
    };

    let mut headers = HeaderMap::new();
    if anonymous_client_ip && !same_origin {
        // The only defined requirement token is
        // `anonymous-client-ip-when-cross-origin`.
        headers.insert(
            "Sec-Purpose",
            HeaderValue::from_static("prefetch;anonymous-client-ip"),
        );
    }
    if let Ok(value) = HeaderValue::from_str(&serialize_sec_speculation_tags(&tags)) {
        headers.insert("Sec-Speculation-Tags", value);
    }

    // Navigation requests carry their starting URL in the URL list; this also
    // makes `Sec-Fetch-Site` report cross-site correctly for cross-origin
    // candidates.
    let url_list = vec![url.clone()];
    let request = RequestBuilder::new(
        Some(document.webview_id()),
        UrlWithBlobClaim::new(url, None),
        global.get_referrer(),
    )
    .url_list(url_list)
    .destination(Destination::Document)
    .mode(RequestMode::Navigate)
    .credentials_mode(credentials_mode)
    // Prefetch responses must not pollute the HTTP cache used by navigations
    // (`no-http-cache-interference.https.html`).
    .cache_mode(CacheMode::NoStore)
    .initiator(Initiator::Prefetch)
    .origin(origin)
    .referrer_policy(referrer_policy)
    .policy_container((*document.policy_container()).clone())
    .client(global.request_client(None))
    .pipeline_id(Some(global.pipeline_id()))
    .headers(headers);

    let request_id = request.id;
    document.fetch_background(
        request,
        PrefetchListener {
            global: Trusted::new(&*global),
        },
    );
    request_id
}

/// A listener that throws prefetch responses away.
///
/// Speculative loads are not exposed to content: they never create
/// `PerformanceResourceTiming` entries and their bodies are not used by this
/// backend (see the module-level note about the missing prefetch cache).
pub(crate) struct PrefetchListener {
    global: Trusted<GlobalScope>,
}

impl FetchResponseListener for PrefetchListener {
    fn process_request_body(&mut self, _request_id: RequestId) {}

    fn process_response(
        &mut self,
        _cx: &mut JSContext,
        _request_id: RequestId,
        _metadata: Result<FetchMetadata, NetworkError>,
    ) {
    }

    fn process_response_chunk(&mut self, _cx: &mut JSContext, _request_id: RequestId, _: Bytes) {}

    fn process_response_eof(
        self,
        _cx: &mut JSContext,
        _request_id: RequestId,
        _response: Result<(), NetworkError>,
        _timing: ResourceFetchTiming,
    ) {
    }

    fn process_csp_violations(
        &mut self,
        cx: &mut JSContext,
        _request_id: RequestId,
        violations: Vec<Violation>,
    ) {
        let global = self.global.root();
        global.report_csp_violations(cx, violations, None, None);
    }
}

/// Cancel in-flight prefetch requests, e.g. when their rule set is removed.
pub(crate) fn cancel_prefetches(document: &Document, request_ids: &[RequestId]) {
    if request_ids.is_empty() {
        return;
    }
    let global = document.global();
    net_traits::cancel_async_fetch(request_ids.to_vec(), &global.core_resource_thread());
}
