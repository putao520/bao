/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::Cell;
use std::sync::Arc;

use dom_struct::dom_struct;
use html5ever::{LocalName, Prefix, local_name, ns};
use js::context::JSContext;
use js::rust::HandleObject;
use net_traits::image_cache::{
    ImageCache, ImageCacheResult, ImageLoadListener, ImageOrMetadataAvailable, ImageResponse,
    PendingImageId, PendingImageResponse,
};
use net_traits::request::{CorsSettings, Destination, RequestId};
use net_traits::{FetchMetadata, FetchResponseMsg, NetworkError, ResourceFetchTiming};
use script_bindings::cell::DomRefCell;
use servo_url::ServoUrl;
use style::attr::AttrValue;

use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::csp::{GlobalCspReporting, Violation};
use crate::dom::document::Document;
use crate::dom::element::AttributeMutation;
use crate::dom::element::attributes::storage::AttrRef;
use crate::dom::element::{Element, cors_setting_for_element, referrer_policy_for_element};
use crate::dom::eventtarget::EventTarget;
use crate::dom::node::virtualmethods::VirtualMethods;
use crate::dom::node::{Node, NodeDamage, NodeTraits};
use crate::dom::performance::performanceresourcetiming::InitiatorType;
use crate::dom::svg::svggraphicselement::SVGGraphicsElement;
use crate::dom::window::Window;
use crate::event_loop::document_loader::{LoadBlocker, LoadType};
use crate::fetch::fetch::{RequestWithGlobalScope, create_a_potential_cors_request};
use crate::fetch::network_listener::{self, FetchResponseListener, ResourceTimingListener};

/// <https://svgwg.org/svg2-draft/embedded.html#Placement>
const DEFAULT_WIDTH: u32 = 300;
const DEFAULT_HEIGHT: u32 = 150;

#[dom_struct]
pub(crate) struct SVGImageElement {
    svggraphicselement: SVGGraphicsElement,
    /// Monotonic counter used to discard image responses that belong to a fetch
    /// started before the most recent `href` mutation.
    generation: Cell<u32>,
    /// Delays the document load event while an image resource is being fetched.
    load_blocker: DomRefCell<Option<LoadBlocker>>,
}

impl SVGImageElement {
    fn new_inherited(
        local_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
    ) -> SVGImageElement {
        SVGImageElement {
            svggraphicselement: SVGGraphicsElement::new_inherited(local_name, prefix, document),
            generation: Cell::new(0),
            load_blocker: DomRefCell::new(None),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        local_name: LocalName,
        prefix: Option<Prefix>,
        document: &Document,
        proto: Option<HandleObject>,
    ) -> DomRoot<SVGImageElement> {
        Node::reflect_node_with_proto(
            cx,
            Box::new(SVGImageElement::new_inherited(local_name, prefix, document)),
            document,
            proto,
        )
    }

    fn generation(&self) -> u32 {
        self.generation.get()
    }

    /// <https://svgwg.org/svg2-draft/linking.html#processingURL>
    fn fetch_image_resource(&self, cx: &mut JSContext) {
        LoadBlocker::terminate(&self.load_blocker, cx);
        self.generation.set(self.generation.get() + 1);
        let generation = self.generation.get();

        // <https://svgwg.org/svg2-draft/embedded.html#TermImageElement>:
        // the `href` attribute takes precedence over the legacy `xlink:href`.
        let element = self.upcast::<Element>();
        let href = element
            .get_attribute_string_value_with_namespace(&ns!(), &local_name!("href"))
            .or_else(|| {
                element.get_attribute_string_value_with_namespace(&ns!(xlink), &local_name!("href"))
            });

        let Some(href) = href.filter(|href| !href.is_empty()) else {
            return self.queue_error_event();
        };

        // As HTMLImageElement: parse the URL relative to the document's base URL.
        let document = self.owner_document();
        let Ok(image_url) = document.base_url().join(&href) else {
            return self.queue_error_event();
        };

        let window = self.owner_window();
        *self.load_blocker.borrow_mut() =
            Some(LoadBlocker::new(&document, LoadType::Image(image_url.clone())));

        let cache_result = window.image_cache().get_cached_image_status(
            image_url.clone(),
            window.origin().immutable().clone(),
            cors_setting_for_element(element),
        );

        match cache_result {
            ImageCacheResult::Available(ImageOrMetadataAvailable::ImageAvailable { image, url }) => {
                self.process_image_response(cx, ImageResponse::Loaded(image, url));
            },
            ImageCacheResult::Available(ImageOrMetadataAvailable::MetadataAvailable(_, id)) => {
                self.register_image_cache_callback(&window, id, generation);
            },
            ImageCacheResult::Pending(id) => {
                self.register_image_cache_callback(&window, id, generation);
            },
            ImageCacheResult::ReadyForRequest(id) => {
                self.fetch_request(&document, &window, &image_url, id);
                self.register_image_cache_callback(&window, id, generation);
            },
            ImageCacheResult::FailedToLoadOrDecode => {
                self.process_image_response(cx, ImageResponse::FailedToLoadOrDecode);
            },
        }
    }

    /// Start the network fetch that feeds the image cache, mirroring
    /// HTMLImageElement's background image request.
    fn fetch_request(
        &self,
        document: &Document,
        window: &Window,
        img_url: &ServoUrl,
        id: PendingImageId,
    ) {
        let global = document.global();
        let request = create_a_potential_cors_request(
            Some(window.webview_id()),
            img_url.clone(),
            Destination::Image,
            cors_setting_for_element(self.upcast()),
            None,
            global.get_referrer(),
        )
        .with_global_scope(&global)
        .referrer_policy(referrer_policy_for_element(self.upcast()));

        let context = SvgImageContext {
            image_cache: window.image_cache(),
            status: Ok(()),
            id,
            aborted: false,
            doc: Trusted::new(document),
            url: img_url.clone(),
            element: Trusted::new(self),
        };

        document.fetch_background(request, context);
    }

    fn register_image_cache_callback(&self, window: &Window, id: PendingImageId, generation: u32) {
        let trusted_node = Trusted::new(self);
        let callback = window.register_image_cache_listener(
            id,
            move |response: PendingImageResponse, cx: &mut JSContext| {
                let trusted_node = trusted_node.clone();
                let window = trusted_node.root().owner_window();

                window
                    .as_global_scope()
                    .task_manager()
                    .networking_task_source()
                    .queue(task!(process_image_response: move |cx| {
                        let element = trusted_node.root();

                        // Ignore any image response for a previous request that has been discarded.
                        if generation != element.generation() {
                            return;
                        }

                        element.process_image_response(cx, response.response);
                    }));
            },
        );

        window.image_cache().add_listener(ImageLoadListener::new(
            callback,
            window.pipeline_id(),
            id,
        ));
    }

    fn process_image_response(&self, cx: &mut JSContext, image: ImageResponse) {
        LoadBlocker::terminate(&self.load_blocker, cx);
        match image {
            // Note: the decoded image is not retained yet — SVG2 `<image>`
            // rendering (layout consumption of the image data) is a separate,
            // still-unimplemented surface; the fetch and load/error event
            // semantics are complete.
            ImageResponse::Loaded(..) => {
                self.upcast::<Node>().dirty(cx.no_gc(), NodeDamage::Other);
                self.upcast::<EventTarget>().fire_event(cx, atom!("load"));
            },
            ImageResponse::FailedToLoadOrDecode => {
                self.upcast::<EventTarget>().fire_event(cx, atom!("error"));
            },
            ImageResponse::MetadataLoaded(_) => {
                self.upcast::<Node>().dirty(cx.no_gc(), NodeDamage::Other);
            },
        }
    }

    fn queue_error_event(&self) {
        self.owner_global()
            .task_manager()
            .dom_manipulation_task_source()
            .queue_simple_event(self.upcast(), atom!("error"));
    }
}

/// The context required for asynchronously loading an external image, mirroring
/// HTMLImageElement's `ImageContext` (same image-cache feeding semantics).
struct SvgImageContext {
    /// Reference to the script thread image cache.
    image_cache: Arc<dyn ImageCache>,
    /// Indicates whether the request failed, and why
    status: Result<(), NetworkError>,
    /// The cache ID for this request.
    id: PendingImageId,
    /// Used to mark abort
    aborted: bool,
    /// The document associated with this request
    doc: Trusted<Document>,
    url: ServoUrl,
    element: Trusted<SVGImageElement>,
}

impl FetchResponseListener for SvgImageContext {
    fn should_invoke(&self) -> bool {
        !self.aborted
    }

    fn process_request_body(&mut self, _: RequestId) {}

    fn process_response(
        &mut self,
        _: &mut JSContext,
        request_id: RequestId,
        metadata: Result<FetchMetadata, NetworkError>,
    ) {
        self.image_cache.notify_pending_response(
            self.id,
            FetchResponseMsg::ProcessResponse(request_id, metadata.clone()),
        );

        let metadata = metadata.ok().map(|meta| match meta {
            FetchMetadata::Unfiltered(m) => m,
            FetchMetadata::Filtered { unsafe_, .. } => unsafe_,
        });

        if let Some(metadata) = metadata.as_ref() &&
            let Some(ref content_type) = metadata.content_type
        {
            let mime: mime::Mime = content_type.clone().into_inner().into();
            if mime.type_() == mime::MULTIPART && mime.subtype().as_str() == "x-mixed-replace" {
                self.aborted = true;
            }
        }

        // The HTTP status code is ignored here. Ok NetworkError is treated
        // as real error
        self.status = match metadata.as_ref().map(|m| m.status.clone()) {
            None => Err(NetworkError::ResourceLoadError(
                "No http status code received".to_owned(),
            )),
            Some(_) => Ok(()),
        };
    }

    fn process_response_chunk(&mut self, _: &mut JSContext, request_id: RequestId, payload: bytes::Bytes) {
        if self.status.is_ok() {
            self.image_cache.notify_pending_response(
                self.id,
                FetchResponseMsg::ProcessResponseChunk(request_id, payload.into()),
            );
        }
    }

    fn process_response_eof(
        self,
        cx: &mut JSContext,
        request_id: RequestId,
        response: Result<(), NetworkError>,
        timing: ResourceFetchTiming,
    ) {
        self.image_cache.notify_pending_response(
            self.id,
            FetchResponseMsg::ProcessResponseEOF(request_id, response.clone(), timing.clone()),
        );
        network_listener::submit_timing(cx, &self, &response, &timing);
    }

    fn process_csp_violations(
        &mut self,
        cx: &mut JSContext,
        _request_id: RequestId,
        violations: Vec<Violation>,
    ) {
        // SVGImageElement does not track a source line number; report without
        // a source position.
        let global = &self.resource_timing_global();
        global.report_csp_violations(cx, violations, None, None);
    }

    fn process_content_length(&mut self, request_id: RequestId, size: usize) {
        self.image_cache.notify_pending_response(
            self.id,
            FetchResponseMsg::ProcessContentLength(request_id, size),
        );
    }
}

impl ResourceTimingListener for SvgImageContext {
    fn resource_timing_information(&self) -> (InitiatorType, ServoUrl) {
        (
            InitiatorType::LocalName("image".to_string()),
            self.url.clone(),
        )
    }

    fn resource_timing_global(&self) -> DomRoot<crate::dom::globalscope::GlobalScope> {
        self.doc.root().global()
    }
}

impl VirtualMethods for SVGImageElement {
    fn super_type(&self) -> Option<&dyn VirtualMethods> {
        Some(self.upcast::<SVGGraphicsElement>() as &dyn VirtualMethods)
    }

    fn attribute_mutated(
        &self,
        cx: &mut JSContext,
        attr: AttrRef<'_>,
        mutation: AttributeMutation,
    ) {
        self.super_type()
            .unwrap()
            .attribute_mutated(cx, attr, mutation);
        if attr.local_name() == &local_name!("href") &&
            matches!(attr.namespace(), &ns!() | &ns!(xlink)) &&
            let AttributeMutation::Set(_) = mutation
        {
            self.fetch_image_resource(cx);
        }
    }

    fn unbind_from_tree(&self, cx: &mut JSContext, context: &crate::dom::node::UnbindContext) {
        self.super_type().unwrap().unbind_from_tree(cx, context);
        // Stop delaying the document load event when the element is removed
        // while a fetch is in flight.
        LoadBlocker::terminate(&self.load_blocker, cx);
        self.generation.set(self.generation.get() + 1);
    }

    fn attribute_affects_presentational_hints(&self, attr: AttrRef<'_>) -> bool {
        match attr.local_name() {
            &local_name!("width") | &local_name!("height") => true,
            _ => self
                .super_type()
                .unwrap()
                .attribute_affects_presentational_hints(attr),
        }
    }

    fn parse_plain_attribute(&self, name: &LocalName, value: DOMString) -> AttrValue {
        match *name {
            local_name!("width") => AttrValue::from_u32(value.into(), DEFAULT_WIDTH),
            local_name!("height") => AttrValue::from_u32(value.into(), DEFAULT_HEIGHT),
            _ => self
                .super_type()
                .unwrap()
                .parse_plain_attribute(name, value),
        }
    }
}
