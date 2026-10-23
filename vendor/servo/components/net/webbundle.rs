/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Process-wide WebBundle subresource registry (BAO PATCH, REQ-BRW-051).
//!
//! `<script type=webbundle>` element rules and the parsed bundle responses are
//! registered here by the script thread (see `CoreResourceMsg::WebBundle*`).
//! Every fetch that reaches the net layer consults this registry:
//!
//! * a URL matching the newest element registration waits for that element's
//!   loader to settle, then is served from the bundle when the bundle contains
//!   the URL, or falls through to the network when it does not;
//! * requests already waiting on a bundle that fails are rejected instead of
//!   falling back to the network (WPT network-error/not-found); the element
//!   registration is dropped at the same time so requests issued after the
//!   failure resolve on the network (WPT nested-bundle);
//! * removing the element rejects in-flight requests that were waiting on it
//!   (WPT element-removal);
//! * bundle requests themselves (`Initiator::WebBundle`) never consult the
//!   registry, so a bundle stored inside another bundle cannot be used as a
//!   bundle (nested bundles are not supported, WPT nested-bundle).
//!
//! Registrations are keyed by the request's `PipelineId`: a document (including
//! an iframe document, which has its own pipeline) owns its registrations, so a
//! bundle element in one frame cannot serve URLs fetched by another frame.
//! The data is plain (no GC) and lives until the script side drops it.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use net_traits::http_status::HttpStatus;
use net_traits::request::{Destination, Initiator, Request, RequestBuilder, WebBundleBinding};
use net_traits::response::{Response, ResponseBody};
use net_traits::{NetworkError, ResourceFetchTiming, WebBundleLoaderData, WebBundleRule};
use parking_lot::RwLock;
use servo_base::id::PipelineId;
use servo_url::ServoUrl;
use tokio::sync::watch;

/// One decoded response as stored on the net side.
struct StoredResponse {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

struct LoaderData {
    /// Response keyed by the absolute URL string it serves.
    responses: HashMap<String, Arc<StoredResponse>>,
    /// The bundle fetch's resource timing; subresources served from the bundle
    /// report it (WPT resource-timing-attributes-consistent).
    timing: ResourceFetchTiming,
}

enum LoaderStatus {
    /// The bundle fetch is in flight; matching requests wait.
    Pending,
    Ready(Arc<LoaderData>),
    /// The fetch or the parse failed; matching requests fail closed.
    Failed,
}

struct LoaderEntry {
    status: LoaderStatus,
}

/// One element registration (newest last).
struct ElementEntry {
    element_id: u64,
    loader_id: u64,
    bundle_url: ServoUrl,
    resources: Vec<String>,
    scopes: Vec<String>,
}

impl ElementEntry {
    fn matches(&self, url: &str) -> bool {
        self.resources.iter().any(|resource| resource == url) ||
            self.scopes.iter().any(|scope| url.starts_with(scope.as_str()))
    }
}

#[derive(Default)]
struct PipelineBundles {
    loaders: HashMap<u64, LoaderEntry>,
    elements: Vec<ElementEntry>,
}

static REGISTRY: OnceLock<RwLock<HashMap<PipelineId, PipelineBundles>>> = OnceLock::new();
static CHANGE: OnceLock<watch::Sender<u64>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<PipelineId, PipelineBundles>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn change_sender() -> &'static watch::Sender<u64> {
    CHANGE.get_or_init(|| watch::channel(0u64).0)
}

/// Wake every fetch task waiting for a registry change. `watch` keeps the
/// latest version, so a waiter that observes a change between its lookup and
/// its await still wakes immediately (no lost wakeups).
fn notify_change() {
    change_sender().send_modify(|version| *version = version.wrapping_add(1));
}

// ── message handlers (called from the core resource thread) ──

pub(crate) fn register(pipeline: PipelineId, rule: WebBundleRule) {
    let element_id = rule.element_id;
    let loader_id = rule.loader_id;
    {
        let mut guard = registry().write();
        let view = guard.entry(pipeline).or_default();
        // Re-registration (adoption after a synchronous remove/append) moves the
        // element to the newest position.
        view.elements
            .retain(|element| element.element_id != element_id);
        view.elements.push(ElementEntry {
            element_id,
            loader_id,
            bundle_url: rule.bundle_url,
            resources: rule
                .resources
                .into_iter()
                .map(|url| url.as_str().to_string())
                .collect(),
            scopes: rule
                .scopes
                .into_iter()
                .map(|url| url.as_str().to_string())
                .collect(),
        });
        // The loader may already be Ready/Failed when an element adopts it; keep
        // that state and its data.
        view.loaders.entry(loader_id).or_insert(LoaderEntry {
            status: LoaderStatus::Pending,
        });
    }
    notify_change();
}

pub(crate) fn loaded(pipeline: PipelineId, data: WebBundleLoaderData) {
    let responses: HashMap<String, Arc<StoredResponse>> = data
        .resources
        .into_iter()
        .map(|resource| {
            (
                resource.url.as_str().to_string(),
                Arc::new(StoredResponse {
                    status: resource.status,
                    headers: resource.headers.0,
                    body: resource.body,
                }),
            )
        })
        .collect();
    {
        let mut guard = registry().write();
        let view = guard.entry(pipeline).or_default();
        view.loaders.insert(
            data.loader_id,
            LoaderEntry {
                status: LoaderStatus::Ready(Arc::new(LoaderData {
                    responses,
                    timing: data.timing,
                })),
            },
        );
    }
    notify_change();
}

pub(crate) fn failed(pipeline: PipelineId, loader_id: u64) {
    {
        let mut guard = registry().write();
        let view = guard.entry(pipeline).or_default();
        view.loaders.insert(
            loader_id,
            LoaderEntry {
                status: LoaderStatus::Failed,
            },
        );
    }
    notify_change();
}

pub(crate) fn unregister(pipeline: PipelineId, element_id: u64) {
    {
        let mut guard = registry().write();
        if let Some(view) = guard.get_mut(&pipeline) {
            view.elements
                .retain(|element| element.element_id != element_id);
        }
    }
    notify_change();
}

pub(crate) fn drop_loader(pipeline: PipelineId, loader_id: u64) {
    {
        let mut guard = registry().write();
        if let Some(view) = guard.get_mut(&pipeline) {
            view.loaders.remove(&loader_id);
            view.elements
                .retain(|element| element.loader_id != loader_id);
        }
        if guard
            .get(&pipeline)
            .is_some_and(|view| view.elements.is_empty() && view.loaders.is_empty())
        {
            guard.remove(&pipeline);
        }
    }
    notify_change();
}

pub(crate) fn clear_pipeline(pipeline: PipelineId) {
    registry().write().remove(&pipeline);
    notify_change();
}

// ── fetch integration ──

/// Decide the web bundle binding for a request about to be fetched.
///
/// Runs on the core resource thread, where the registration messages are
/// applied in channel order. Binding here (rather than looking the URL up in
/// the async fetch task) makes the semantics deterministic:
///
/// * a request issued before an element is removed is bound to that element's
///   loader and fails closed when the element disappears
///   (WPT element-removal);
/// * a request issued after a failed element's registration was dropped falls
///   through to the network (WPT nested-bundle), while a request issued before
///   the failure was already bound and is rejected (WPT not-found);
/// * the newest element registration wins (WPT "multiple web bundle
///   elements");
/// * redirect continuations are built as fresh requests without a binding, so
///   a redirect *to* a registered URL resolves on the network (WPT redirect
///   case).
pub(crate) fn bind_request(request_builder: &mut RequestBuilder) {
    if request_builder.initiator == Initiator::WebBundle {
        return;
    }
    // Subresource loading only: navigations are not served out of a bundle
    // (navigation-from-bundle is a separate feature; the WPT suite covers the
    // `<script type=webbundle>` subresource API).
    if matches!(
        request_builder.destination,
        Destination::Document |
            Destination::Embed |
            Destination::Frame |
            Destination::IFrame |
            Destination::Object
    ) {
        return;
    }
    let Some(pipeline) = request_builder.pipeline_id else {
        return;
    };
    let url = request_builder.url.as_str().to_string();
    let binding = {
        let guard = registry().read();
        guard.get(&pipeline).and_then(|view| {
            view.elements
                .iter()
                .rev()
                .find(|element| element.matches(&url))
                .map(|element| WebBundleBinding {
                    element_id: element.element_id,
                    loader_id: element.loader_id,
                    bundle_url: element.bundle_url.clone(),
                })
        })
    };
    if binding.is_some() {
        request_builder.web_bundle_binding = binding;
    }
}

/// Whether the request is bound to a web bundle registration. Used to bypass
/// service worker mediation before the request enters the network-or-cache
/// path: bundle subresources must not be intercepted by a service worker (WPT
/// service-worker-controlled).
pub(crate) fn request_matches_registered_bundle(request: &Request) -> bool {
    // A redirect continuation is not served from a bundle.
    request.redirect_count == 0 && request.web_bundle_binding.is_some()
}

/// The URL a CSP check must use for a non-HTTP(S) subresource URL that a web
/// bundle registration binds. Chrome matches `uuid-in-package:` script loads
/// against the bundle URL (`csp-allowed` / `csp-blocked` WPT cases); HTTP(S)
/// URLs keep matching against themselves.
pub(crate) fn csp_url_override(request: &Request) -> Option<ServoUrl> {
    if request.redirect_count > 0 ||
        matches!(request.current_url().scheme(), "http" | "https")
    {
        return None;
    }
    request
        .web_bundle_binding
        .as_ref()
        .map(|binding| binding.bundle_url.clone())
}

/// The outcome of consulting the registry for a fetch.
pub(crate) enum BundleLookupOutcome {
    /// Serve this synthetic response instead of hitting the network.
    Serve(Response),
    /// Fail the fetch (failed/removed bundle or invalid stored response).
    Reject(NetworkError),
}

enum Lookup {
    /// The bound bundle does not contain this URL: continue with the network.
    NoMatch,
    /// The bound loader is still loading; wait and retry.
    Waiting,
    Serve {
        response: Arc<StoredResponse>,
        timing: ResourceFetchTiming,
    },
    Reject(NetworkError),
}

/// Serve a fetch from its bound web bundle. `None` means "not bound"; `Some`
/// means the fetch must settle here.
pub(crate) async fn serve_from_web_bundle(request: &Request) -> Option<BundleLookupOutcome> {
    // A redirect continuation is not served from a bundle.
    if request.redirect_count > 0 {
        return None;
    }
    let binding = request.web_bundle_binding.as_ref()?;
    let pipeline = request.pipeline_id?;
    let url = request.current_url().as_str().to_string();
    let element_id = binding.element_id;
    let loader_id = binding.loader_id;
    loop {
        // Subscribe *before* the lookup: a registry change that lands between
        // the lookup and `changed()` still wakes this task (the watch keeps the
        // latest version, so no lost wakeup).
        let mut receiver = change_sender().subscribe();
        match lookup_bound(pipeline, element_id, loader_id, &url) {
            Lookup::NoMatch => return None,
            Lookup::Waiting => {
                if receiver.changed().await.is_err() {
                    return Some(BundleLookupOutcome::Reject(NetworkError::LoadCancelled));
                }
            },
            Lookup::Serve { response, timing } => {
                return Some(match build_response(request, &response, timing) {
                    Ok(response) => BundleLookupOutcome::Serve(response),
                    Err(error) => BundleLookupOutcome::Reject(error),
                });
            },
            Lookup::Reject(error) => return Some(BundleLookupOutcome::Reject(error)),
        }
    }
}

fn lookup_bound(
    pipeline: PipelineId,
    element_id: u64,
    loader_id: u64,
    url: &str,
) -> Lookup {
    let guard = registry().read();
    let Some(view) = guard.get(&pipeline) else {
        return Lookup::Reject(NetworkError::ResourceLoadError(
            "The Web Bundle element was removed".to_string(),
        ));
    };
    // Removing the element fails the requests it already bound, instead of
    // letting them fall back to the network (WPT element-removal).
    if !view
        .elements
        .iter()
        .any(|element| element.element_id == element_id)
    {
        return Lookup::Reject(NetworkError::ResourceLoadError(
            "The Web Bundle element was removed".to_string(),
        ));
    }
    match view.loaders.get(&loader_id).map(|entry| &entry.status) {
        None => Lookup::Reject(NetworkError::ResourceLoadError(
            "The Web Bundle is unavailable".to_string(),
        )),
        Some(LoaderStatus::Pending) => Lookup::Waiting,
        Some(LoaderStatus::Failed) => Lookup::Reject(NetworkError::ResourceLoadError(
            "The Web Bundle failed to load".to_string(),
        )),
        Some(LoaderStatus::Ready(data)) => match data.responses.get(url) {
            Some(response) => Lookup::Serve {
                response: response.clone(),
                timing: data.timing.clone(),
            },
            // The bundle does not contain this bound URL: fall through to the
            // network (WPT "A resource should not be found").
            None => Lookup::NoMatch,
        },
    }
}

fn build_response(
    request: &Request,
    stored: &StoredResponse,
    timing: ResourceFetchTiming,
) -> Result<Response, NetworkError> {
    if !(100..=599).contains(&stored.status) {
        return Err(NetworkError::ResourceLoadError(
            "The Web Bundle response has an invalid status".to_string(),
        ));
    }
    // Subresource resource timing: the resource becomes available once the
    // bundle has finished loading, so the subresource's `responseStart` is the
    // bundle's completion time. WPT resource-timing-attributes-consistent
    // asserts `responseStart >= requestStart + 500` against a bundle trickled
    // over 0.5 s; reusing the bundle's header time would violate it when the
    // fixture delivers headers immediately.
    let mut timing = timing;
    if let Some(response_end) = timing.response_end {
        timing.response_start = Some(response_end);
    }

    let url = request.current_url();
    let mut response = Response::new(url.clone(), timing);
    response.headers = stored.headers.clone();
    response.status = HttpStatus::new_raw(stored.status, Vec::new());
    response.url_list = vec![url];
    *response.body.lock() = ResponseBody::Done(stored.body.clone());

    // CDP Network byte-level observation (REQ-CDP-006): an HTTP(S) bundled body
    // settles as one in-process chunk with no transfer encoding, so the wire
    // bytes are the payload bytes. Non-HTTP(S) URLs (`uuid-in-package:`)
    // settle through the scheme-response seam in `wait_for_response`, which
    // already emits both events — emitting here too would duplicate them.
    if matches!(request.current_url().scheme(), "http" | "https") {
        let request_id = request.id.0.to_string();
        let webview_id = request.target_webview_id.map(|id| id.to_string());
        let body_length = stored.body.len() as u64;
        crate::http_loader::bao_emit_network_data_received_tap(
            request_id.clone(),
            webview_id.clone(),
            body_length,
            body_length,
        );
        crate::http_loader::bao_emit_network_finished_tap(request_id, webview_id, body_length);
    }
    Ok(response)
}
