/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use content_security_policy::Destination;
use embedder_traits::{GenericEmbedderProxy, WebResourceRequest, WebResourceResponseMsg};
use log::error;
use net_traits::NetworkError;
use net_traits::http_status::HttpStatus;
use net_traits::request::Request;
use net_traits::response::{Response, ResponseBody};

use crate::embedder::NetToEmbedderMsg;
use crate::fetch::methods::FetchContext;

// ── BAO PATCH (BCE-20260910-002): webview-less WebResourceRequested local verdict ──
//
// Upstream servo assumes the embedder runs a resident main loop that keeps
// draining the net→embedder channel (`Servo::spin_event_loop`), so every
// `WebResourceRequested` round-trip gets answered. Bao's embedder is a lazy
// pump (PageHandle interaction APIs only) and `Servo(Rc<ServoInner>)` is
// !Send/!Sync, so no resident thread may spin it; a fetch with
// `target_webview_id == None` (SW/worker realms carry no webview) could
// therefore park forever in the embedder wait while the owning page sat
// idle (the fetchevent 25s stall).
//
// Fix: for webview-less requests, consult a process-global handler
// installed by the embedder (bao_browser, at `BaoRuntime::new` — same
// global-setter pattern as the C19-② network tap in http_loader.rs).
// `PassThrough` answers the interceptor locally with the exact default the
// embedder path would have produced for bao today (no-op
// `ServoDelegate::load_web_resource` → `WebResourceLoad` drop →
// `IpcResponder` default `DoNotIntercept`, webview_delegate.rs). With no
// handler installed (pure servo embedder), the upstream embedder
// round-trip is preserved unchanged; webview-owned requests always keep
// the full round-trip either way.

/// The embedder's verdict for a webview-less (`target_webview_id == None`)
/// resource request, answered locally on the fetch thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaoWebviewlessResourceVerdict {
    /// Answer `DoNotIntercept` without an embedder round-trip —
    /// byte-equivalent to a default no-op
    /// `ServoDelegate::load_web_resource` for this request.
    PassThrough,
}

/// The embedder-installed handler. Called on fetch worker threads; must be
/// cheap and must not block.
pub type BaoWebviewlessResourceHandler =
    std::sync::Arc<dyn Fn(&WebResourceRequest) -> BaoWebviewlessResourceVerdict + Send + Sync>;

static BAO_WEBVIEWLESS_RESOURCE_HANDLER: parking_lot::RwLock<Option<BaoWebviewlessResourceHandler>> =
    parking_lot::RwLock::new(None);

/// Install or remove the process-wide webview-less resource handler
/// (embedder API face is `servo::set_webviewless_resource_handler`).
pub fn set_webviewless_resource_handler(handler: Option<BaoWebviewlessResourceHandler>) {
    *BAO_WEBVIEWLESS_RESOURCE_HANDLER.write() = handler;
}

#[derive(Clone)]
pub struct RequestInterceptor {
    embedder_proxy: GenericEmbedderProxy<NetToEmbedderMsg>,
}

impl RequestInterceptor {
    pub fn new(embedder_proxy: GenericEmbedderProxy<NetToEmbedderMsg>) -> RequestInterceptor {
        RequestInterceptor { embedder_proxy }
    }

    pub async fn intercept_request(
        &self,
        request: &mut Request,
        response: &mut Option<Response>,
        context: &FetchContext,
    ) {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let is_for_main_frame = matches!(request.destination, Destination::Document);
        let web_resource_request = WebResourceRequest {
            method: request.method.clone(),
            url: request.url().into_url(),
            headers: request.headers.clone(),
            destination: request.destination,
            referrer_url: request.referrer.to_url().map(|url| url.as_url().clone()),
            is_for_main_frame,
            is_redirect: request.redirect_count > 0,
        };

        // BAO PATCH (BCE-20260910-002): a webview-less request (SW/worker
        // realm — no owning webview) consults the embedder-installed handler
        // locally instead of round-tripping through the net→embedder
        // channel, which only a pumping embedder drains.
        if request.target_webview_id.is_none() {
            let handler = BAO_WEBVIEWLESS_RESOURCE_HANDLER.read().clone();
            if let Some(handler) = handler {
                match handler(&web_resource_request) {
                    BaoWebviewlessResourceVerdict::PassThrough => return,
                }
            }
        }

        self.embedder_proxy
            .send(NetToEmbedderMsg::WebResourceRequested(
                request.target_webview_id,
                web_resource_request,
                sender,
            ));

        // TODO: use done_chan and run in CoreResourceThreadPool.
        let mut accumulated_body = Vec::new();
        while let Some(message) = receiver.recv().await {
            match message {
                WebResourceResponseMsg::Start(webresource_response) => {
                    let timing = context.timing.inner().clone();
                    let mut response_override =
                        Response::new(webresource_response.url.into(), timing);
                    response_override.headers = webresource_response.headers;
                    response_override.status = HttpStatus::new(
                        webresource_response.status_code,
                        webresource_response.status_message,
                    );
                    *response = Some(response_override);
                },
                WebResourceResponseMsg::SendBodyData(data) => {
                    accumulated_body.push(data);
                },
                WebResourceResponseMsg::FinishLoad => {
                    if accumulated_body.is_empty() {
                        break;
                    }
                    let Some(response) = response.as_mut() else {
                        error!("Received unexpected FinishLoad message");
                        break;
                    };
                    *response.body.lock() =
                        ResponseBody::Done(accumulated_body.into_iter().flatten().collect());
                    break;
                },
                WebResourceResponseMsg::CancelLoad => {
                    *response = Some(Response::network_error(NetworkError::LoadCancelled));
                    break;
                },
                WebResourceResponseMsg::DoNotIntercept => break,
            }
        }
    }
}
