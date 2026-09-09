/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Service Worker `FetchEvent` pipeline.
//!
//! Bao vendor patch (user ruling 2026-09-09): upstream left the SW fetch
//! mediation as a TODO ("This will eventually use a FetchEvent interface",
//! serviceworkerglobalscope.rs) that fired a bare `Event` and always answered
//! the `CustomResponseMediator` channel with `None`. This module implements
//! the <https://w3c.github.io/ServiceWorker/#fetchevent-interface> surface:
//! the event carries the mediated `Request`, and the `respondWith` promise is
//! settled asynchronously on the worker's own event loop (via
//! `PromiseNativeHandler` reactions plus `read_all_bytes` success/failure
//! steps) — the mediator channel is answered exactly once:
//!
//! * `respondWith(promise)` fulfilled with a `Response` → status/headers/body
//!   are extracted in the SW realm and sent as `Some(CustomResponse)`;
//! * everything else (no `respondWith`, rejected promise, non-`Response`
//!   fulfillment, `Response.error()`, unreadable body) → `None`, which keeps
//!   upstream's pass-through semantics intact.

use std::cell::Cell;
use std::rc::Rc;

use dom_struct::dom_struct;
use http::StatusCode;
use ipc_channel::ipc::IpcSender;
use js::context::JSContext;
use js::realm::CurrentRealm;
use js::rust::{HandleObject, HandleValue};
use net_traits::{CustomResponse, CustomResponseMediator};
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::ExtendableEventBinding::ExtendableEvent_Binding::ExtendableEventMethods;
use crate::dom::bindings::codegen::Bindings::FetchEventBinding;
use crate::dom::bindings::codegen::Bindings::FetchEventBinding::FetchEventMethods;
use crate::dom::bindings::codegen::Bindings::RequestBinding::{RequestInfo, RequestInit};
use crate::dom::bindings::codegen::Bindings::ResponseBinding::ResponseMethods;
use crate::dom::bindings::conversions::root_from_handlevalue;
use crate::dom::bindings::error::{Error, ErrorResult, Fallible};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::root::{Dom, DomRoot, MutNullableDom};
use crate::dom::bindings::str::{DOMString, USVString};
use crate::dom::event::Event;
use crate::dom::eventtarget::EventTarget;
use crate::dom::extendableevent::ExtendableEvent;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::Promise;
use crate::dom::promisenativehandler::Callback;
use crate::dom::request::Request;
use crate::dom::response::Response;
use crate::dom::serviceworkerglobalscope::ServiceWorkerGlobalScope;
use crate::dom::types::PromiseNativeHandler;
use crate::fetch::body::BodyMixin;

// https://w3c.github.io/ServiceWorker/#fetchevent-interface
#[dom_struct]
pub(crate) struct FetchEvent {
    event: ExtendableEvent,
    /// <https://w3c.github.io/ServiceWorker/#fetch-event-request>
    request: Dom<Request>,
    /// The promise registered through `respondWith`, if it was called.
    respond_with_promise: MutNullableDom<Promise>,
    /// Whether `respondWith` was already called (spec: at most once).
    respond_with_entered: Cell<bool>,
    /// The mediator channel this event answers on. `None` for events
    /// constructed from script (the `FetchEvent` constructor), which answer
    /// to nobody.
    #[no_trace]
    #[ignore_malloc_size_of = "Ipc channel sender"]
    response_sender: Option<IpcSender<Option<CustomResponse>>>,
}

impl FetchEvent {
    fn new_inherited(
        request: &Request,
        response_sender: Option<IpcSender<Option<CustomResponse>>>,
    ) -> FetchEvent {
        FetchEvent {
            event: ExtendableEvent::new_inherited(),
            request: Dom::from_ref(request),
            respond_with_promise: Default::default(),
            respond_with_entered: Cell::new(false),
            response_sender,
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        type_: Atom,
        bubbles: bool,
        cancelable: bool,
        request: &Request,
        response_sender: Option<IpcSender<Option<CustomResponse>>>,
    ) -> DomRoot<FetchEvent> {
        Self::new_with_proto(
            cx,
            global,
            None,
            type_,
            bubbles,
            cancelable,
            request,
            response_sender,
        )
    }

    fn new_with_proto(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: bool,
        cancelable: bool,
        request: &Request,
        response_sender: Option<IpcSender<Option<CustomResponse>>>,
    ) -> DomRoot<FetchEvent> {
        let event = reflect_dom_object_with_proto(
            cx,
            Box::new(FetchEvent::new_inherited(request, response_sender)),
            global,
            proto,
        );
        {
            let event = event.upcast::<Event>();
            event.init_event(type_, bubbles, cancelable);
        }
        event
    }

    /// Run the fetch mediation for one `CustomResponseMediator`: build the
    /// mediated `Request`, dispatch a trusted `fetch` `FetchEvent` on the
    /// service worker global, and settle the mediator channel from
    /// `respondWith`. Runs inside the service worker realm.
    pub(crate) fn handle_mediator(
        cx: &mut CurrentRealm,
        scope: &ServiceWorkerGlobalScope,
        mediator: CustomResponseMediator,
    ) {
        let global = scope.upcast::<GlobalScope>();

        // Build the Request the event carries: a plain GET for the mediated
        // URL. The URL is absolute, so the constructor's base-URL join is a
        // no-op. On failure (should not happen for a well-formed mediator
        // URL) fall through to the pass-through signal.
        let request_init = RequestInit::empty();
        let request_info = RequestInfo::USVString(USVString(mediator.load_url.to_string()));
        let request = match Request::constructor(cx, global, None, request_info, &request_init) {
            Ok(request) => request,
            Err(_) => {
                let _ = mediator.response_chan.send(None);
                return;
            },
        };

        // <https://w3c.github.io/ServiceWorker/#service-worker-global-scope-fetch-event>
        // The event is dispatched on the service worker's event target with
        // type "fetch" and cancelable=true; the trusted flag is set by
        // `Event::fire` because the engine (not script) dispatches it.
        let event = FetchEvent::new(
            cx,
            global,
            atom!("fetch"),
            false,
            true,
            &request,
            Some(mediator.response_chan.clone()),
        );
        let target = scope.upcast::<EventTarget>();
        event.upcast::<Event>().fire(cx, target);

        match event.respond_with_promise.get() {
            // No `respondWith` call: keep upstream pass-through semantics.
            None => {
                let _ = mediator.response_chan.send(None);
            },
            Some(promise) => {
                // The reactions run on this worker thread's event loop when
                // the promise settles — no blocking wait is involved.
                let handler = PromiseNativeHandler::new(
                    cx,
                    global,
                    Some(Box::new(FetchResponseResolveHandler {
                        response_sender: Some(mediator.response_chan.clone()),
                    })),
                    Some(Box::new(FetchResponseRejectHandler {
                        response_sender: Some(mediator.response_chan),
                    })),
                );
                promise.append_native_handler(cx, &handler);
            },
        }
    }
}

/// Fulfillment steps of the `respondWith` promise: extract the `Response` and
/// answer the mediator channel.
#[derive(MallocSizeOf, JSTraceable)]
struct FetchResponseResolveHandler {
    #[no_trace]
    #[ignore_malloc_size_of = "Ipc channel sender"]
    response_sender: Option<IpcSender<Option<CustomResponse>>>,
}

/// Rejection steps of the `respondWith` promise: pass-through.
#[derive(MallocSizeOf, JSTraceable)]
struct FetchResponseRejectHandler {
    #[no_trace]
    #[ignore_malloc_size_of = "Ipc channel sender"]
    response_sender: Option<IpcSender<Option<CustomResponse>>>,
}

impl Callback for FetchResponseResolveHandler {
    fn callback(&self, cx: &mut CurrentRealm, value: HandleValue) {
        let Some(sender) = self.response_sender.clone() else {
            return;
        };
        // The promise must fulfil with a Response; anything else is a
        // network-error equivalent, for which the channel only knows the
        // pass-through signal.
        let response = match root_from_handlevalue::<Response>(cx, value) {
            Ok(response) => response,
            Err(_) => {
                let _ = sender.send(None);
                return;
            },
        };
        // Extract status and headers while the event realm is on the stack;
        // `Response.error()` has no real status, and its network-error
        // equivalent is the pass-through signal.
        let Some(raw_status) = response_raw_status(&response) else {
            let _ = sender.send(None);
            return;
        };
        let headers = response.Headers(cx).get_headers_list();

        // A null body answers immediately with an empty byte sequence;
        // otherwise the body stream must be fully read before the response
        // can travel the channel.
        match response.body() {
            None => {
                let _ = sender.send(Some(CustomResponse::new(
                    headers,
                    raw_status,
                    Vec::with_capacity(0),
                )));
            },
            Some(stream) => {
                if response.is_unusable() || stream.is_errored() {
                    let _ = sender.send(None);
                    return;
                }
                let reader = match stream.acquire_default_reader(cx) {
                    Ok(reader) => reader,
                    Err(_) => {
                        let _ = sender.send(None);
                        return;
                    },
                };
                // The `read_all_bytes` steps hold plain data only (no JS
                // references), and the channel is answered once.
                let sender_success = self.response_sender.clone();
                let sender_failure = self.response_sender.clone();
                reader.read_all_bytes(
                    cx,
                    Rc::new(move |_cx, bytes| {
                        if let Some(ref sender) = sender_success {
                            let _ = sender.send(Some(CustomResponse::new(
                                headers.clone(),
                                raw_status.clone(),
                                bytes.to_vec(),
                            )));
                        }
                    }),
                    Rc::new(move |_cx, _error| {
                        if let Some(ref sender) = sender_failure {
                            let _ = sender.send(None);
                        }
                    }),
                );
            },
        }
    }
}

impl Callback for FetchResponseRejectHandler {
    fn callback(&self, _cx: &mut CurrentRealm, _value: HandleValue) {
        if let Some(ref sender) = self.response_sender {
            let _ = sender.send(None);
        }
    }
}

/// Extract `(status, reason)` from a `Response` the way
/// `CustomResponse::raw_status` wants, or `None` when the response carries no
/// real status (`Response.error()`).
fn response_raw_status(response: &Response) -> Option<(StatusCode, String)> {
    let status_code = StatusCode::from_u16(response.Status()).ok()?;
    let reason = String::from_utf8_lossy(&response.StatusText()).into_owned();
    Some((status_code, reason))
}

impl FetchEventMethods<crate::DomTypeHolder> for FetchEvent {
    /// <https://w3c.github.io/ServiceWorker/#dom-fetchevent-fetchevent>
    fn Constructor(
        cx: &mut JSContext,
        worker: &ServiceWorkerGlobalScope,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &FetchEventBinding::FetchEventInit,
    ) -> Fallible<DomRoot<FetchEvent>> {
        Ok(FetchEvent::new_with_proto(
            cx,
            worker.upcast::<GlobalScope>(),
            proto,
            Atom::from(type_),
            init.parent.parent.bubbles,
            init.parent.parent.cancelable,
            &init.request,
            None,
        ))
    }

    /// <https://w3c.github.io/ServiceWorker/#fetch-event-request>
    fn Request(&self) -> DomRoot<Request> {
        DomRoot::from_ref(&*self.request)
    }

    /// <https://w3c.github.io/ServiceWorker/#fetch-event-respondwith>
    fn RespondWith(&self, p: &Promise) -> ErrorResult {
        // Step: at most one respondWith per event.
        if self.respond_with_entered.replace(true) {
            return Err(Error::InvalidState(None));
        }
        // Steps: the promise's settlement decides the response; the reactions
        // are installed by the dispatcher once the event finished running
        // (see `FetchEvent::handle_mediator`).
        self.respond_with_promise.set(Some(p));
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }
}
