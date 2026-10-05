/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::VecDeque;
use std::default::Default;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::jsval::UndefinedValue;
use js::realm::CurrentRealm;
use script_bindings::cell::DomRefCell;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object;
use servo_base::generic_channel::GenericCallback;
use servo_base::id::{ServiceWorkerId, ServiceWorkerRegistrationId};
use servo_constellation_traits::{
    Job, JobError, JobResult, JobResultValue, JobType, ScriptToConstellationMessage,
    ServiceWorkerAlgorithm, ServiceWorkerAlgorithmResult, ServiceWorkerRegistrationInfo,
};
use servo_url::{ImmutableOrigin, ServoUrl};
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::ServiceWorkerBinding::ServiceWorkerState;
use crate::dom::bindings::codegen::Bindings::ServiceWorkerContainerBinding::{
    RegistrationOptions, ServiceWorkerContainerMethods,
};
use crate::dom::bindings::error::Error;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{DomRoot, MutNullableDom};
use crate::dom::bindings::str::USVString;
use crate::dom::bindings::structuredclone;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::Promise;
use crate::dom::serviceworker::ServiceWorker;
use crate::dom::serviceworkerregistration::{ServiceWorkerRegistration, longest_prefix_match};
use crate::dom::types::MessageEvent;
use crate::dom::{RootedPromise, TracedPromise};

/// A promise parked on the container's FIFO algorithm-result queue. The
/// `Job` variant pairs with the register/unregister/getRegistration
/// answers; the `Ready` variant pairs with the match answer of `GetReady`,
/// whose continuation may park the promise on the ready list instead of
/// settling it (each send pairs with exactly one answer in FIFO order, so
/// the variant arriving at an arm is determined by the matching send).
#[derive(MallocSizeOf, JSTraceable)]
enum PendingAlgorithmResultPromise {
    Job(TracedPromise),
    Ready(TracedPromise),
}

/// A `ready` promise waiting for a matching registration to gain an active
/// worker. The `None` key resolves on the next activation of any
/// registration (the match found no registration to key on yet).
#[derive(MallocSizeOf, JSTraceable)]
struct PendingReadyPromise {
    #[no_trace]
    registration_id: Option<ServiceWorkerRegistrationId>,
    promise: TracedPromise,
}

#[dom_struct]
pub(crate) struct ServiceWorkerContainer {
    eventtarget: EventTarget,
    controller: MutNullableDom<ServiceWorker>,

    /// Pending results for
    /// <https://w3c.github.io/ServiceWorker/#algorithms>
    pending_algorithm_results: DomRefCell<VecDeque<PendingAlgorithmResultPromise>>,

    /// `ready` promises parked by `GetReady` until a matching registration
    /// gains an active worker
    /// (<https://w3c.github.io/ServiceWorker/#navigator-service-worker-ready>).
    pending_ready_promises: DomRefCell<Vec<PendingReadyPromise>>,

    /// Handler of algorithm results.
    #[no_trace]
    callback: DomRefCell<Option<GenericCallback<ServiceWorkerAlgorithmResult>>>,
}

impl ServiceWorkerContainer {
    fn new_inherited() -> ServiceWorkerContainer {
        ServiceWorkerContainer {
            eventtarget: EventTarget::new_inherited(),
            controller: Default::default(),
            pending_algorithm_results: Default::default(),
            pending_ready_promises: Default::default(),
            callback: Default::default(),
        }
    }

    pub(crate) fn new(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<ServiceWorkerContainer> {
        let container = reflect_dom_object(
            cx,
            Box::new(ServiceWorkerContainer::new_inherited()),
            global,
        );
        // BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling
        // 2026-10-05): enroll this container with the origin's service worker
        // manager as a matchable client the moment it exists. Every document
        // that touches `navigator.serviceWorker` (register / getRegistration /
        // onmessage / matchAll) creates its container first, so this is the
        // single choke point that makes iframe subdocument containers visible
        // to the manager's client set — without it a SW's
        // `clients.matchAll({includeUncontrolled: true})` could never reach
        // them and worker→client messages time out (e69 attribution).
        container.enroll_with_manager();
        container
    }

    /// BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling 2026-10-05):
    /// fire-and-forget enrollment ping. Reuses the
    /// `MatchServiceWorkerRegistration` algorithm shape (its `enroll_only`
    /// flag makes the manager answer nothing), so no new constellation
    /// routing arm is required. The container's algorithm-result callback
    /// doubles as the message-delivery channel the manager multicasts
    /// `MessageFromWorker` through.
    fn enroll_with_manager(&self) {
        let global = self.global();
        let Some(storage_key) = global.obtain_storage_key() else {
            return;
        };
        let result_handler = self.ensure_callback();
        let _ = global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::MatchServiceWorkerRegistration {
                    storage_key,
                    client_url: global.creation_url(),
                    result_handler,
                    enroll_only: true,
                    client_pipeline: global.pipeline_id(),
                },
            ));
    }

    /// BAO PATCH (REQ-BRW-004 e70 multi-client wave): create the container's
    /// algorithm-result callback if it doesn't exist yet. Split out of
    /// `get_or_setup_callback_with` so enrollment can set up the delivery
    /// channel eagerly at container creation.
    fn ensure_callback(&self) -> GenericCallback<ServiceWorkerAlgorithmResult> {
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
                        "Error in Service worker algorithm result handlings {:?}.",
                        err
                    );
                },
            };
            task_source.queue(task!(set_request_result_to_database: move |cx| {
                let container = response_listener.root();
                container.handle_algorithm_result(cx, response)
            }));
        })
        .expect("Could not create callback");

        *self.callback.borrow_mut() = Some(callback.clone());

        callback
    }

    /// <https://w3c.github.io/ServiceWorker/#reject-job-promise>
    /// <https://w3c.github.io/ServiceWorker/#resolve-job-promise>
    fn handle_job_result(&self, cx: &mut JSContext, result: JobResult, promise: &RootedPromise) {
        let global = self.global();
        match result {
            // <https://w3c.github.io/ServiceWorker/#reject-job-promise>
            // Step 2.2: Queue a task, on equivalentJob’s client’s responsible event loop
            // using the DOM manipulation task source,
            // to reject equivalentJob’s job promise with a new exception with errorData,
            // in equivalentJob’s client’s Realm.
            // Note: we are in the task already.
            JobResult::RejectPromise(error) => match error {
                JobError::TypeError => {
                    promise.reject_error(
                        cx,
                        Error::Type(c"Failed to register a ServiceWorker".to_owned()),
                    );
                },
                JobError::SecurityError => {
                    promise.reject_error(cx, Error::Security(None));
                },
            },
            // <https://w3c.github.io/ServiceWorker/#resolve-job-promise>
            JobResult::ResolvePromise(value) => {
                match value {
                    JobResultValue::Unregister(success) => {
                        promise.resolve_native(cx, &success);
                    },
                    JobResultValue::Register(value) => {
                        let ServiceWorkerRegistrationInfo {
                            id,
                            installing_worker,
                            waiting_worker,
                            active_worker,
                            storage_key: _,
                            scope_url,
                            script_url,
                            client_url: _,
                            client_urls: _,
                        } = value;
                        // BAO PATCH (REQ-BRW-004 C19 controller wave): the
                        // manager resolves this job after the waiting→active
                        // transitions, so `active_worker` below reflects the
                        // post-activation state — assign the container's
                        // controller in the same task that settles the
                        // register() promise (see refresh_controller).
                        self.refresh_controller(cx, &script_url, &scope_url, active_worker);
                        // Step 2.2: If equivalentJob’s job type is either register or update,
                        // set convertedValue to the result of getting the service worker registration object
                        // that represents value in equivalentJob’s client.
                        let registration = global.get_serviceworker_registration(
                            cx,
                            &script_url,
                            &scope_url,
                            id,
                            installing_worker,
                            waiting_worker,
                            active_worker,
                        );

                        // TODO Step 2.3: Else, set convertedValue to value, in equivalentJob’s client’s Realm.

                        // Step 2.4: Resolve equivalentJob’s job promise with convertedValue.
                        promise.resolve_native(cx, &*registration);

                        // BAO PATCH (REQ-BRW-004 e57 contract A, user ruling
                        // 2026-10-04): activation notify — the manager
                        // resolves the register job only after the
                        // waiting→active transitions, so this is where a
                        // `ready` promise parked on this registration
                        // settles.
                        if active_worker.is_some() {
                            self.resolve_pending_ready(cx, id, &registration);
                        }
                    },
                }
            },
        }
    }

    /// BAO PATCH (REQ-BRW-004 C19 controller wave, user ruling 2026-09-09):
    /// upstream never assigns the container's `controller` field (the getter
    /// hardcoded `None`), so page JS had no way to observe that the document
    /// sits in a registered scope with an active worker. Minimal
    /// activated-assignment chain: whenever a registration answer reaching
    /// this container (the register-job resolution, or a getRegistration
    /// match) carries an active worker whose scope prefixes this global's
    /// URL, store the page-side ServiceWorker object for that worker as this
    /// container's controller.
    /// Boundary (deliberately minimal — spec claim()/clients territory):
    /// only the container that registered or queried is refreshed, since the
    /// manager keeps a single client callback per registration; a page that
    /// never touches the SW API keeps `controller === null` (navigation
    /// SW-ification is not implemented upstream); the attribute is never
    /// cleared — unregister leaves the stale object in place while
    /// interception itself stops at the manager (which drops the
    /// registration).
    fn refresh_controller(
        &self,
        cx: &mut JSContext,
        script_url: &ServoUrl,
        scope_url: &ServoUrl,
        active_worker: Option<ServiceWorkerId>,
    ) {
        let Some(worker_id) = active_worker else {
            return;
        };
        let global = self.global();
        if !longest_prefix_match(scope_url, &global.get_url()) {
            return;
        }
        let worker = global.get_serviceworker(cx, script_url, scope_url, worker_id);
        // BAO PATCH (REQ-BRW-004 e57 contract A, user ruling 2026-10-04):
        // fire "controllerchange" when this swap actually changes the
        // controller. The global's worker map hands out one DOM object per
        // worker id, so pointer equality is worker identity here — a
        // re-refresh with the same active worker (register resolution
        // followed by a getRegistration match) must not re-fire.
        let changed = match self.controller.get() {
            None => true,
            Some(current) => !std::ptr::eq::<ServiceWorker>(&*current, &*worker),
        };
        self.controller.set(Some(&*worker));
        if changed {
            self.upcast::<EventTarget>()
                .fire_event(cx, atom!("controllerchange"));
        }
    }

    /// Continuation of the parallel steps from
    /// <https://w3c.github.io/ServiceWorker/#dom-serviceworkercontainer-getregistration>
    fn handle_match_registration_result(
        &self,
        cx: &mut JSContext,
        registration_info: Option<ServiceWorkerRegistrationInfo>,
        promise: &RootedPromise,
    ) {
        // Step 8.1 Let registration be the result of running Match Service Worker Registration given storage key and clientURL.
        // Note: the `registration_info` argument is the result from the parallel algorithm run.

        // Step 8.2: If registration is null, resolve promise with undefined and abort these steps.
        let Some(info) = registration_info else {
            promise.resolve_native(cx, &());
            return;
        };

        // Step 8.3: Resolve promise with the result of getting the service worker registration object
        // that represents registration in promise’s relevant settings object.
        // BAO PATCH (REQ-BRW-004 C19 controller wave): pull-path refresh — a
        // page that queries getRegistration() gets its controller assigned
        // from the matched registration's active worker (see
        // refresh_controller).
        self.refresh_controller(cx, &info.script_url, &info.scope_url, info.active_worker);
        let registration = self.global().get_serviceworker_registration(
            cx,
            &info.script_url,
            &info.scope_url,
            info.id,
            info.installing_worker,
            info.waiting_worker,
            info.active_worker,
        );
        promise.resolve_native(cx, &*registration);
    }

    /// Continuation of the parallel steps from
    /// <https://w3c.github.io/ServiceWorker/#navigator-service-worker-ready>.
    /// BAO PATCH (REQ-BRW-004 e57 contract A, user ruling 2026-10-04,
    /// Chromium-parity): spec shape — a match with an active worker resolves
    /// immediately; otherwise the promise parks until a matching
    /// registration gains an active worker (see `resolve_pending_ready`).
    /// Boundary (deliberately minimal, same per-client scope as
    /// `refresh_controller`): the parked promise is settled by this
    /// container's own register-job resolutions — the manager keeps a
    /// single client callback per registration, so activations driven by
    /// *other* pages do not reach this container.
    fn handle_ready_match_result(
        &self,
        cx: &mut JSContext,
        registration_info: Option<ServiceWorkerRegistrationInfo>,
        promise: &RootedPromise,
    ) {
        let Some(info) = registration_info else {
            // No matching registration yet: park with the match-less key —
            // the next activation of any registration settles it.
            self.pending_ready_promises
                .borrow_mut()
                .push(PendingReadyPromise {
                    registration_id: None,
                    promise: promise.to_traced(),
                });
            return;
        };
        let ServiceWorkerRegistrationInfo {
            id,
            installing_worker,
            waiting_worker,
            active_worker,
            storage_key: _,
            scope_url,
            script_url,
            client_url: _,
            client_urls: _,
        } = info;
        if let Some(active_worker) = active_worker {
            let registration = self.global().get_serviceworker_registration(
                cx,
                &script_url,
                &scope_url,
                id,
                installing_worker,
                waiting_worker,
                Some(active_worker),
            );
            promise.resolve_native(cx, &*registration);
            return;
        }
        self.pending_ready_promises
            .borrow_mut()
            .push(PendingReadyPromise {
                registration_id: Some(id),
                promise: promise.to_traced(),
            });
    }

    /// Settle every parked `ready` promise waiting on this registration (or
    /// parked key-less) with `registration`. Runs inside the register-job
    /// resolution task.
    fn resolve_pending_ready(
        &self,
        cx: &mut JSContext,
        registration_id: ServiceWorkerRegistrationId,
        registration: &ServiceWorkerRegistration,
    ) {
        let matched: Vec<PendingReadyPromise> = {
            let mut pending = self.pending_ready_promises.borrow_mut();
            let (matched, remaining): (Vec<_>, Vec<_>) = pending.drain(..).partition(|entry| {
                entry.registration_id.is_none() ||
                    entry.registration_id == Some(registration_id)
            });
            *pending = remaining;
            matched
        };
        for entry in matched {
            let promise = entry.promise.root(cx);
            promise.resolve_native(cx, registration);
        }
    }

    fn handle_algorithm_result(&self, cx: &mut JSContext, result: ServiceWorkerAlgorithmResult) {
        match result {
            // BAO PATCH (REQ-BRW-004 lifecycle wave, 2026-10-04): Update
            // Registration State relay — the manager set the installing worker
            // on this registration. Fire "updatefound" on the client's
            // ServiceWorkerRegistration object. Delivered as its own FIFO
            // message AFTER the register promise's resolve message, so the
            // registering page's promise handler (which attaches the
            // updatefound listener) has already run. Must not consume a
            // pending job promise.
            ServiceWorkerAlgorithmResult::UpdateFound { registration_id } => {
                if let Some(registration) = self
                    .global()
                    .get_serviceworker_registration_by_id(registration_id)
                {
                    // BAO PATCH (REQ-DEPLOY-1, 2026-10-05): literal Atom
                    // instead of atom!() — registry stylo_atoms 0.22.0's
                    // static_atoms.txt has no "updatefound" entry, so the
                    // macro form breaks publish-face compilation. Atom::from
                    // interns dynamically; identical runtime behavior.
                    registration
                        .upcast()
                        .fire_event(cx, Atom::from("updatefound"));
                }
            },
            // BAO PATCH (REQ-BRW-004 lifecycle wave, 2026-10-04): Update
            // Worker State relay — the worker thread reported script
            // evaluation + activate dispatch complete. Transition the DOM
            // ServiceWorker object to "activated" and fire "statechange"
            // (workers were observably stuck at "installing" forever before
            // this relay existed). Must not consume a pending job promise.
            ServiceWorkerAlgorithmResult::WorkerActivated { worker_id } => {
                if let Some(worker) = self.global().get_serviceworker_by_id(worker_id) {
                    worker.update_state(cx, ServiceWorkerState::Activated);
                }
            },
            ServiceWorkerAlgorithmResult::Job(job_result) => {
                let promise = match self.pending_algorithm_results.borrow_mut().pop_front() {
                    Some(PendingAlgorithmResultPromise::Job(promise)) => Some(promise.root(cx)),
                    // FIFO pairing: a Job answer always meets a Job send.
                    Some(PendingAlgorithmResultPromise::Ready(_)) |
                    None => None,
                };
                let Some(promise) = promise else {
                    debug_assert!(false, "No pending algorithm result.");
                    return;
                };
                self.handle_job_result(cx, job_result, &promise);
            },
            ServiceWorkerAlgorithmResult::MatchServiceWorkerRegistration(registration_info) => {
                let popped = self.pending_algorithm_results.borrow_mut().pop_front();
                let Some(pending) = popped else {
                    debug_assert!(false, "No pending algorithm result.");
                    return;
                };
                match pending {
                    PendingAlgorithmResultPromise::Job(promise) => {
                        let promise = promise.root(cx);
                        self.handle_match_registration_result(cx, registration_info, &promise);
                    },
                    PendingAlgorithmResultPromise::Ready(promise) => {
                        let promise = promise.root(cx);
                        self.handle_ready_match_result(cx, registration_info, &promise);
                    },
                }
            },
            ServiceWorkerAlgorithmResult::MessageFromWorker {
                message,
                source,
                scope_url,
                script_url,
                origin,
            } => {
                // <https://w3c.github.io/ServiceWorker/#dom-client-postmessage-message-options>
                // Add a task that runs the following steps to destination’s client message queue:
                // Note: we are in the task.
                // Step 4.5.2: Let source be the result of getting the service worker object
                // that represents contextObject’s relevant global object’s service worker in targetClient.
                let global = self.global();

                // Note: spec uses a MesssageEvent, so it's unclear what to do with source.
                // Perhaps an ExtendableMessageEvent should be used instead.
                // See https://github.com/w3c/ServiceWorker/issues/1823
                let _source = global.get_serviceworker(cx, &script_url, &scope_url, source);

                // Step 4.5.4: Let messageClone be deserializeRecord.[[Deserialized]].
                // Step 4.5.5: Let newPorts be a new frozen array consisting of all MessagePort objects
                // in deserializeRecord.[[TransferredValues]], if any.
                rooted!(&in(cx) let mut message_val = UndefinedValue());
                if let Ok(ports) =
                    structuredclone::read(cx, &global, message, message_val.handle_mut())
                {
                    // Step 4.5.6: Dispatch an event named message at destination, using MessageEvent, with its origin initialized to origin,
                    // the source attribute initialized to source,
                    // the data attribute initialized to messageClone, and the ports attribute initialized to newPorts.
                    MessageEvent::dispatch_jsval(
                        cx,
                        self.upcast(),
                        &global,
                        message_val.handle(),
                        Some(origin.ascii_serialization().as_ref()),
                        None,
                        ports,
                    );
                } else {
                    error!("Failed to deserialize message ports in message from service worker.");
                }
            },
        }
    }

    /// Setup the callback to the backend service, if this hasn't been done already.
    fn get_or_setup_callback(
        &self,
        promise: &RootedPromise,
    ) -> GenericCallback<ServiceWorkerAlgorithmResult> {
        self.get_or_setup_callback_with(PendingAlgorithmResultPromise::Job(promise.to_traced()))
    }

    /// Same as `get_or_setup_callback`, for a `GetReady` promise: its answer
    /// (a registration match) must not be mistaken for a job-promise answer.
    fn get_or_setup_ready_callback(
        &self,
        promise: &RootedPromise,
    ) -> GenericCallback<ServiceWorkerAlgorithmResult> {
        self.get_or_setup_callback_with(PendingAlgorithmResultPromise::Ready(promise.to_traced()))
    }

    fn get_or_setup_callback_with(
        &self,
        pending: PendingAlgorithmResultPromise,
    ) -> GenericCallback<ServiceWorkerAlgorithmResult> {
        self.pending_algorithm_results.borrow_mut().push_back(pending);
        self.ensure_callback()
    }

    /// Continuation for
    /// <https://w3c.github.io/ServiceWorker/#dom-serviceworkerregistration-unregister>
    pub(crate) fn create_and_schedule_unregister_job(
        &self,
        cx: &mut JSContext,
        storage_key: ImmutableOrigin,
        scope: ServoUrl,
        script_url: ServoUrl,
        promise: &RootedPromise,
    ) {
        let global = self.global();
        let result_handler = self.get_or_setup_callback(promise);

        // Step 3: Let job be the result of running Create Job with unregister,
        // registration’s storage key, registration’s scope url, null, promise,
        // and this’s relevant settings object.
        let job = Job::create_job(
            JobType::Unregister,
            scope,
            script_url,
            result_handler,
            global.creation_url(),
            None,
            storage_key,
        );

        // Step 4: Invoke Schedule Job with job.
        if global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::Unregister(job),
            ))
            .is_err()
        {
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_algorithm_results.borrow_mut().pop_back();

            debug_assert!(
                false,
                "Failed to send Unregister algorithm message to the constellation."
            );
            self.handle_algorithm_result(
                cx,
                ServiceWorkerAlgorithmResult::Job(JobResult::RejectPromise(JobError::TypeError)),
            );
        }
    }
}

impl ServiceWorkerContainerMethods<crate::DomTypeHolder> for ServiceWorkerContainer {
    /// <https://w3c.github.io/ServiceWorker/#service-worker-container-controller-attribute>
    /// BAO PATCH (REQ-BRW-004 C19 controller wave, user ruling 2026-09-09):
    /// upstream hardcoded `None` here while the `controller` field sat
    /// unassigned tree-wide. Return the field, which refresh_controller
    /// populates from registration answers carrying an active worker.
    fn GetController(&self) -> Option<DomRoot<ServiceWorker>> {
        self.controller.get()
    }

    // <https://w3c.github.io/ServiceWorker/#dom-serviceworkercontainer-oncontrollerchange>
    // BAO PATCH (REQ-BRW-004 e57 contract A, user ruling 2026-10-04,
    // Chromium-parity): fired by refresh_controller when the controller
    // swap changes the active worker.
    event_handler!(controllerchange, GetOncontrollerchange, SetOncontrollerchange);

    // <https://w3c.github.io/ServiceWorker/#dom-serviceworkercontainer-onmessage>
    // BAO PATCH: the container already dispatches worker→client "message"
    // events (the MessageFromWorker arm); this only exposes the handler.
    event_handler!(message, GetOnmessage, SetOnmessage);

    // <https://w3c.github.io/ServiceWorker/#dom-serviceworkercontainer-onmessageerror>
    // BAO PATCH: exposure only — no dispatch site exists yet (Chrome-parity
    // surface; the error event is raised on malformed deserialization,
    // which the MessageFromWorker arm currently logs).
    event_handler!(messageerror, GetOnmessageerror, SetOnmessageerror);

    /// <https://w3c.github.io/ServiceWorker/#dom-serviceworkercontainer-register> - A
    /// and <https://w3c.github.io/ServiceWorker/#start-register> - B
    fn Register(
        &self,
        realm: &mut CurrentRealm,
        script_url: USVString,
        options: &RegistrationOptions,
    ) -> RootedPromise {
        // A: Step 2.
        let global = self.global();

        // A: Step 1
        let promise = Promise::new_in_realm(realm);
        let USVString(ref script_url) = script_url;

        // A: Step 3
        let api_base_url = global.api_base_url();
        let script_url = match api_base_url.join(script_url) {
            Ok(url) => url,
            Err(_) => {
                // B: Step 1
                promise.reject_error(realm, Error::Type(c"Invalid script URL".to_owned()));
                return promise;
            },
        };

        // A: Step 4-5
        let scope = match options.scope {
            Some(ref scope) => {
                let USVString(inner_scope) = scope;
                match api_base_url.join(inner_scope) {
                    Ok(url) => url,
                    Err(_) => {
                        promise.reject_error(realm, Error::Type(c"Invalid scope URL".to_owned()));
                        return promise;
                    },
                }
            },
            None => script_url.join("./").unwrap(),
        };

        // A: Step 6 -> invoke B.

        // B: Step 3
        match script_url.scheme() {
            "https" | "http" => {},
            _ => {
                promise.reject_error(
                    realm,
                    Error::Type(c"Only secure origins are allowed".to_owned()),
                );
                return promise;
            },
        }
        // B: Step 4
        if script_url.path().to_ascii_lowercase().contains("%2f") ||
            script_url.path().to_ascii_lowercase().contains("%5c")
        {
            promise.reject_error(
                realm,
                Error::Type(c"Script URL contains forbidden characters".to_owned()),
            );
            return promise;
        }

        // B: Step 6
        match scope.scheme() {
            "https" | "http" => {},
            _ => {
                promise.reject_error(
                    realm,
                    Error::Type(c"Only secure origins are allowed".to_owned()),
                );
                return promise;
            },
        }
        // B: Step 7
        if scope.path().to_ascii_lowercase().contains("%2f") ||
            scope.path().to_ascii_lowercase().contains("%5c")
        {
            promise.reject_error(
                realm,
                Error::Type(c"Scope URL contains forbidden characters".to_owned()),
            );
            return promise;
        }

        let result_handler = self.get_or_setup_callback(&promise);

        let scope_things =
            ServiceWorkerRegistration::create_scope_things(&global, script_url.clone());

        // B: Step 8 - 13

        // Step 10: Let storage key be the result of running obtain a storage key given client.
        let Some(storage_key) = global.obtain_storage_key() else {
            promise.reject_error(
                realm,
                Error::Type(c"Failed to obtain a storage key".to_owned()),
            );
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_algorithm_results.borrow_mut().pop_back();
            return promise;
        };

        let job = Job::create_job(
            JobType::Register,
            scope,
            script_url,
            result_handler,
            global.creation_url(),
            Some(scope_things),
            storage_key,
        );

        // B: Step 14: schedule job.
        if global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::StartRegister(job),
            ))
            .is_err()
        {
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_algorithm_results.borrow_mut().pop_back();
            debug_assert!(
                false,
                "Failed to send StartRegister algorithm message to the constellation."
            );
            promise.reject_error(
                realm,
                Error::Type(c"Failed to register a ServiceWorker".to_owned()),
            );
        }

        // A: Step 7
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#navigator-service-worker-getRegistration>
    fn GetRegistration(&self, realm: &mut CurrentRealm, client_url: USVString) -> RootedPromise {
        // Step 1: Let client be this’s service worker client.
        let global = self.global();

        // Step 7: Let promise be a new promise.
        // Note: done here so it can be used to handle failure of the below steps.
        let promise = Promise::new_in_realm(realm);

        // Step 2: Let client storage key be the result of running obtain a storage key given client.
        let Some(storage_key) = global.obtain_storage_key() else {
            promise.reject_error(
                realm,
                Error::Type(c"Failed to obtain a storage key".to_owned()),
            );
            return promise;
        };

        // Step 3: Let clientURL be the result of parsing clientURL with this’s relevant settings object’s API base URL.
        let mut client_url = match global.api_base_url().join(&client_url.0) {
            Ok(url) => url,
            Err(_) => {
                // Step 4: If clientURL is failure, return a promise rejected with a TypeError.
                promise.reject_error(realm, Error::Type(c"Failed to parse clientURL".to_owned()));
                return promise;
            },
        };

        // Step 5: Set clientURL’s fragment to null.
        client_url.set_fragment(None);

        // Step 6: If the origin of clientURL is not client’s origin, return a promise rejected with a "SecurityError" DOMException.
        if &client_url.origin() != global.origin().immutable() {
            promise.reject_error(realm, Error::Security(None));
            return promise;
        }

        let result_handler = self.get_or_setup_callback(&promise);

        // Step 8: Run the following substeps in parallel:
        // Note: continues in parallel in the service worker manager,
        // by way of the constellation.
        if global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::MatchServiceWorkerRegistration {
                    enroll_only: false,
                    client_url,
                    storage_key,
                    result_handler,
                    client_pipeline: global.pipeline_id(),
                },
            ))
            .is_err()
        {
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_algorithm_results.borrow_mut().pop_back();
            promise.reject_error(
                realm,
                Error::Type(c"Failed to send MatchServiceWorkerRegistration algorithm".to_owned()),
            );
        }

        // Step 9: Return promise.
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#navigator-service-worker-ready>
    ///
    /// BAO PATCH (REQ-BRW-004 e57 contract A, user ruling 2026-10-04,
    /// Chromium-parity): steps 1-3 run the match against this client's
    /// creation URL (`handle_ready_match_result` settles or parks the
    /// promise); step 4's activation notify is
    /// `resolve_pending_ready`, reached from the register-job resolution.
    /// The fork's codegen hands promise-valued getters no cx (the
    /// typeNeedsCx stub — see CLAUDE.md fork-codegen notes); take the script
    /// thread's active context instead, same shape as `serviceworker/cache.rs`.
    #[allow(unsafe_code)]
    fn Ready(&self) -> RootedPromise {
        let mut cx = unsafe { JSContext::get_from_thread().expect("no active JS context") };
        // Step 1: Let client be this’s service worker client.
        let global = self.global();

        // Step 2: Let promise be a new promise.
        let promise = Promise::new(&mut cx, &global);

        // Step 3: Let client storage key be the result of running obtain a storage key given client.
        let Some(storage_key) = global.obtain_storage_key() else {
            promise.reject_error(&mut cx, Error::Type(c"Failed to obtain a storage key".to_owned()));
            return promise;
        };

        let result_handler = self.get_or_setup_ready_callback(&promise);

        // Step 3 (parallel): run match service worker registration given the
        // storage key and the creation URL.
        if global
            .script_to_constellation_chan()
            .send(ScriptToConstellationMessage::ServiceWorkerAlgorithm(
                ServiceWorkerAlgorithm::MatchServiceWorkerRegistration {
                    enroll_only: false,
                    client_url: global.creation_url(),
                    storage_key,
                    result_handler,
                    client_pipeline: global.pipeline_id(),
                },
            ))
            .is_err()
        {
            // Note: pop the promise we just pushed, since we will not get a result back to handle it.
            self.pending_algorithm_results.borrow_mut().pop_back();
            promise.reject_error(
                &mut cx,
                Error::Type(c"Failed to send MatchServiceWorkerRegistration algorithm".to_owned()),
            );
        }

        // Step 4: Return promise.
        promise
    }
}
