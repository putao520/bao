/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Fetch listener for the `<script type=webbundle>` bundle request (BAO PATCH,
//! REQ-BRW-051).
//!
//! The listener accumulates the bundle body, validates the HTTP status and the
//! `application/webbundle` MIME type, submits the bundle's resource timing
//! entry (WPT resource-timing-attributes-consistent expects one entry named by
//! the bundle URL) and then hands the parsed responses to
//! `crate::dom::webbundle::fetch_completed`, which registers them with the net
//! registry and fires the element's load/error event.

use bytes::{Bytes, BytesMut};
use http::header::CONTENT_TYPE;
use hyper_serde::Serde;
use js::context::JSContext;
use net_traits::request::RequestId;
use net_traits::{FetchMetadata, NetworkError, ResourceFetchTiming, WebBundleResource};
use servo_url::ServoUrl;
use webbundle as wbn;

use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::csp::{GlobalCspReporting, Violation};
use crate::dom::document::Document;
use crate::dom::globalscope::GlobalScope;
use crate::dom::performance::performanceresourcetiming::InitiatorType;
use crate::fetch::network_listener::{self, FetchResponseListener, ResourceTimingListener};

pub(crate) struct WebBundleFetchListener {
    /// The document that owns the element(s) using this loader.
    document: Trusted<Document>,
    loader_id: u64,
    /// The bundle URL (with query) used both for parsing relative keys and as
    /// the resource timing entry name.
    bundle_url: ServoUrl,
    data: BytesMut,
    /// HTTP-level failure (status/MIME). The network-level result arrives at EOF.
    status: Result<(), NetworkError>,
}

impl WebBundleFetchListener {
    pub(crate) fn new(
        document: Trusted<Document>,
        loader_id: u64,
        bundle_url: ServoUrl,
    ) -> WebBundleFetchListener {
        WebBundleFetchListener {
            document,
            loader_id,
            bundle_url,
            data: BytesMut::new(),
            status: Ok(()),
        }
    }
}

impl FetchResponseListener for WebBundleFetchListener {
    fn process_request_body(&mut self, _: RequestId) {}

    fn process_response(
        &mut self,
        _: &mut JSContext,
        _: RequestId,
        metadata: Result<FetchMetadata, NetworkError>,
    ) {
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                self.status = Err(error);
                return;
            },
        };
        // Use the unsafe (unfiltered) metadata even for an opaque response: the
        // status/headers of a CORS-cross-origin bundle are what decide whether
        // a bundle can be used, exactly like the script loader's own use.
        let metadata = match &metadata {
            FetchMetadata::Unfiltered(metadata) => metadata,
            FetchMetadata::Filtered { unsafe_, .. } => unsafe_,
        };

        if !metadata.status.is_success() {
            self.status = Err(NetworkError::ResourceLoadError(format!(
                "The Web Bundle request failed with status {}",
                metadata.status.code()
            )));
            return;
        }

        let mime_is_bundle = metadata
            .headers
            .as_ref()
            .and_then(|headers| headers.0.get(CONTENT_TYPE))
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case(wbn::MIME_TYPE)
            });
        if !mime_is_bundle {
            self.status = Err(NetworkError::ResourceLoadError(
                "The Web Bundle response is not an application/webbundle resource".to_string(),
            ));
        }
    }

    fn process_response_chunk(&mut self, _: &mut JSContext, _: RequestId, chunk: Bytes) {
        if self.status.is_ok() {
            self.data.extend_from_slice(&chunk);
        }
    }

    fn process_response_eof(
        self,
        cx: &mut JSContext,
        _: RequestId,
        response: Result<(), NetworkError>,
        timing: ResourceFetchTiming,
    ) {
        // Resource timing is expected to be available before load/error fires.
        network_listener::submit_timing(cx, &self, &response, &timing);

        let document = self.document.root();
        let result = if response.is_ok() && self.status.is_ok() {
            match wbn::parse(&self.data, &self.bundle_url) {
                Ok(bundle) => Ok(bundle
                    .responses
                    .into_iter()
                    .map(to_wire_resource)
                    .collect::<Vec<_>>()),
                Err(error) => Err(format!("{error}")),
            }
        } else {
            Err(match &response {
                Err(error) => format!("{error:?}"),
                Ok(()) => match &self.status {
                    Err(error) => format!("{error:?}"),
                    Ok(()) => "the Web Bundle request was cancelled".to_string(),
                },
            })
        };

        crate::dom::webbundle::fetch_completed(cx, &document, self.loader_id, result, timing);
    }

    fn process_csp_violations(
        &mut self,
        cx: &mut JSContext,
        _: RequestId,
        violations: Vec<Violation>,
    ) {
        self.resource_timing_global()
            .report_csp_violations(cx, violations, None, None);
    }

    fn process_content_length(&mut self, _: RequestId, size: usize) {
        self.data.reserve(size.saturating_sub(self.data.len()));
    }
}

impl ResourceTimingListener for WebBundleFetchListener {
    fn resource_timing_information(&self) -> (InitiatorType, ServoUrl) {
        (
            InitiatorType::LocalName("script".to_string()),
            self.bundle_url.clone(),
        )
    }

    fn resource_timing_global(&self) -> DomRoot<GlobalScope> {
        self.document.root().global()
    }
}

fn to_wire_resource(response: wbn::BundleResponse) -> WebBundleResource {
    WebBundleResource {
        url: response.url,
        status: response.status,
        headers: Serde(response.headers),
        body: response.body,
    }
}
