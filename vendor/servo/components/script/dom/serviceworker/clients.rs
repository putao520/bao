/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The ServiceWorker `Clients` surface.
//!
//! Bao vendor patch (REQ-BRW-004 e58 contract B, user ruling 2026-10-04,
//! Chromium-parity): upstream has no `Clients` interface at all. Minimal
//! subset per the ruling: `matchAll` resolves with the single registering
//! client the manager models — the manager records the registering client's
//! creation URL on the registration (the register job's referrer), and the
//! answer travels the existing `MatchServiceWorkerRegistration` pipeline.
//! `get` resolves with no match: the fork has no client registry.

use std::collections::VecDeque;

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use servo_base::generic_channel::{GenericCallback, GenericSender, GenericSend};
use servo_base::id::ServiceWorkerId;
use servo_constellation_traits::{
    ScriptToConstellationMessage, ServiceWorkerAlgorithm, ServiceWorkerAlgorithmResult,
    ServiceWorkerMsg, ServiceWorkerRegistrationInfo,
};
use servo_url::ServoUrl;

use crate::dom::bindings::codegen::Bindings::ClientBinding::FrameType;
use crate::dom::bindings::codegen::Bindings::ClientsBinding::{ClientQueryOptions, ClientsMethods};
use crate::dom::bindings::error::Error;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::client::Client;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::{Promise, RootedPromise, TracedPromise};

#[dom_struct]
pub(crate) struct Clients {
    reflector_: Reflector,

    /// Channel to the service worker manager, used to route
    /// `postMessage` back to the registering client.
    #[no_trace]
    swmanager_sender: GenericSender<ServiceWorkerMsg>,

    /// This service worker's registration scope.
    #[no_trace]
    scope_url: ServoUrl,

    /// This service worker's id — the `source` identity the manager uses
    /// when routing messages back to the client.
    #[no_trace]
    worker_id: ServiceWorkerId,

    /// `matchAll` promises in FIFO order: each send pairs with exactly one
    /// match answer.
    pending_match_all: DomRefCell<VecDeque<TracedPromise>>,

    /// Handler of match answers.
    #[no_trace]
    callback: DomRefCell<Option<GenericCallback<ServiceWorkerAlgorithmResult>>>,
}

impl Clients {
    fn new_inherited(
        swmanager_sender: GenericSender<ServiceWorkerMsg>,
        scope_url: ServoUrl,
        worker_id: ServiceWorkerId,
    ) -> Clients {
        Clients {
            reflector_: Reflector::new(),
            swmanager_sender,
            scope_url,
            worker_id,
            pending_match_all: Default::default(),
            callback: Default::default(),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        swmanager_sender: GenericSender<ServiceWorkerMsg>,
        scope_url: ServoUrl,
        worker_id: ServiceWorkerId,
    ) -> DomRoot<Clients> {
        reflect_dom_object(
            cx,
            Box::new(Clients::new_inherited(
                swmanager_sender,
                scope_url,
                worker_id,
            )),
            global,
        )
    }

    /// Continuation of the parallel steps from
    /// <https://w3c.github.io/ServiceWorker/#clients-matchall>: resolve with
    /// the client objects built from the matched registration. The
    /// single-registering-client model answers with at most one client, so
    /// `includeUncontrolled`/`type` filtering is a no-op (see the webidl).
    fn handle_match_result(
        &self,
        cx: &mut JSContext,
        registration_info: Option<ServiceWorkerRegistrationInfo>,
    ) {
        let promise = match self.pending_match_all.borrow_mut().pop_front() {
            Some(promise) => promise.root(cx),
            None => {
                debug_assert!(false, "No pending matchAll promise.");
                return;
            },
        };
        let clients: Vec<DomRoot<Client>> = match registration_info {
            Some(info) => {
                vec![Client::new(
                    cx,
                    &self.global(),
                    self.swmanager_sender.clone(),
                    info.client_url,
                    info.scope_url,
                    FrameType::Top_level,
                    self.worker_id,
                )]
            },
            None => Vec::new(),
        };
        promise.resolve_native(cx, &clients);
    }

    /// Setup the callback to the service worker manager, if this hasn't been
    /// done already (same shape as the container's algorithm channel).
    fn get_or_setup_callback(
        &self,
        promise: &RootedPromise,
    ) -> GenericCallback<ServiceWorkerAlgorithmResult> {
        self.pending_match_all
            .borrow_mut()
            .push_back(promise.to_traced());
        if let Some(cb) = self.callback.borrow_mut().as_ref() {
            return cb.clone();
        }

        let global = self.global();
        let response_listener = Trusted::new(self);

        let task_source = global
            .task_manager()
            .dom_manipulation_task_source()
            .to_sendable();
        let callback = GenericCallback::new(move |message| {
            let response_listener = response_listener.clone();
            let response = match message {
                Ok(inner) => inner,
                Err(err) => {
                    return error!(
                        "Error in service worker match result handling {:?}.",
                        err
                    );
                },
            };
            task_source.queue(task!(clients_match_all_result: move |cx| {
                let clients = response_listener.root();
                match response {
                    ServiceWorkerAlgorithmResult::MatchServiceWorkerRegistration(info) => {
                        clients.handle_match_result(cx, info)
                    },
                    _ => debug_assert!(false, "Unexpected result for clients.matchAll."),
                }
            }));
        })
        .expect("Could not create callback");

        *self.callback.borrow_mut() = Some(callback.clone());

        callback
    }
}

impl ClientsMethods<crate::DomTypeHolder> for Clients {
    /// <https://w3c.github.io/ServiceWorker/#clients-matchall>
    ///
    /// The fork's codegen passes promise-returning members no cx (the
    /// typeNeedsCx stub); take the script thread's active context
    /// (serviceworker/cache.rs precedent).
    #[allow(unsafe_code)]
    fn MatchAll(&self, _options: &ClientQueryOptions) -> RootedPromise {
        let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        let global = self.global();

        let promise = Promise::new(&mut cx, &global);

        let Some(storage_key) = global.obtain_storage_key() else {
            promise.reject_error(&mut cx, Error::Type(c"Failed to obtain a storage key".to_owned()));
            return promise;
        };

        let result_handler = self.get_or_setup_callback(&promise);

        if global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::MatchServiceWorkerRegistration {
                    storage_key,
                    client_url: self.scope_url.clone(),
                    result_handler,
                },
            ))
            .is_err()
        {
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_match_all.borrow_mut().pop_back();
            promise.reject_error(
                &mut cx,
                Error::Type(c"Failed to send MatchServiceWorkerRegistration algorithm".to_owned()),
            );
        }

        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#clients-get>
    ///
    /// Minimal subset: the fork keeps no client registry, so no id ever
    /// matches and the promise resolves with `undefined` — the spec's
    /// no-matching-client outcome.
    #[allow(unsafe_code)]
    fn Get(&self, _id: DOMString) -> RootedPromise {
        let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        let promise = Promise::new(&mut cx, &self.global());
        promise.resolve_native(&mut cx, &());
        promise
    }
}
