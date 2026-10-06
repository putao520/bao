/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The service worker manager persists the descriptor of any registered service workers.
//! It also stores an active workers map, which holds descriptors of running service workers.
//! If an active service worker timeouts, then it removes the descriptor entry from its
//! active_workers map

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, select, unbounded};
use devtools_traits::{DevtoolsPageInfo, ScriptToDevtoolsControlMsg};
use fonts::FontContext;
use net_traits::{CoreResourceMsg, CustomResponseMediator};
use servo_base::generic_channel::{self, GenericCallback, GenericSender, RoutedReceiver};
use servo_base::id::{PipelineId, PipelineNamespace, ServiceWorkerId, ServiceWorkerRegistrationId};
use servo_config::pref;
use servo_constellation_traits::{
    DOMMessage, Job, JobError, JobResult, JobResultValue, JobType, SWManagerSenders, ScopeThings,
    ServiceWorkerAlgorithm, ServiceWorkerAlgorithmResult, ServiceWorkerManagerFactory,
    ServiceWorkerMsg, ServiceWorkerRegistrationInfo,
};
use servo_url::{ImmutableOrigin, ServoUrl};

use crate::dom::abstractworker::{MessageData, WorkerScriptMsg};
use crate::dom::serviceworkerglobalscope::{
    ServiceWorkerControlMsg, ServiceWorkerGlobalScope, ServiceWorkerScriptMsg,
};
use crate::dom::serviceworkerregistration::longest_prefix_match;
use crate::runtime::script_runtime::ThreadSafeJSContext;

enum Message {
    FromResource(CustomResponseMediator),
    FromConstellation(Box<ServiceWorkerMsg>),
}

/// <https://w3c.github.io/ServiceWorker/#dfn-service-worker>
#[derive(Clone)]
pub(crate) struct ServiceWorker {
    /// A unique identifer.
    pub(crate) id: ServiceWorkerId,
    /// <https://w3c.github.io/ServiceWorker/#dfn-script-url>
    pub(crate) script_url: ServoUrl,
    /// A sender to the running service worker scope.
    pub(crate) sender: Sender<ServiceWorkerScriptMsg>,
}

impl ServiceWorker {
    fn new(
        script_url: ServoUrl,
        sender: Sender<ServiceWorkerScriptMsg>,
        id: ServiceWorkerId,
    ) -> ServiceWorker {
        ServiceWorker {
            id,
            script_url,
            sender,
        }
    }

    /// Forward a DOM message to the running service worker scope.
    fn forward_dom_message(&self, msg: DOMMessage) {
        let DOMMessage {
            origin,
            data,
            pipeline_id,
        } = msg;
        let _ = self.sender.send(ServiceWorkerScriptMsg::CommonWorker(
            WorkerScriptMsg::DOMMessage(MessageData {
                origin,
                pipeline_id,
                data: Box::new(data),
            }),
        ));
    }

    /// Send a message to the running service worker scope.
    fn send_message(&self, msg: ServiceWorkerScriptMsg) {
        let _ = self.sender.send(msg);
    }
}

/// When updating a registration, which worker are we targetting?
enum RegistrationUpdateTarget {
    Installing,
    Waiting,
    Active,
}

impl Drop for ServiceWorkerRegistration {
    /// <https://html.spec.whatwg.org/multipage/#terminate-a-worker>
    fn drop(&mut self) {
        // Drop the channel to signal shutdown.
        if self
            .control_sender
            .take()
            .expect("No control sender to worker thread.")
            .send(ServiceWorkerControlMsg::Exit)
            .is_err()
        {
            warn!("Failed to send exit message to service worker scope.");
        }

        self.closing
            .take()
            .expect("No close flag for worker")
            .store(true, Ordering::SeqCst);
        self.context
            .take()
            .expect("No context to request interrupt.")
            .request_interrupt_callback();

        // TODO: Step 1, 2 and 3.
        if self
            .join_handle
            .take()
            .expect("No handle to join on worker.")
            .join()
            .is_err()
        {
            warn!("Failed to join on service worker thread.");
        }
    }
}

/// <https://w3c.github.io/ServiceWorker/#service-worker-registration-concept>
struct ServiceWorkerRegistration {
    /// A unique identifer.
    id: ServiceWorkerRegistrationId,
    /// <https://w3c.github.io/ServiceWorker/#dfn-active-worker>
    active_worker: Option<ServiceWorker>,
    /// <https://w3c.github.io/ServiceWorker/#dfn-waiting-worker>
    waiting_worker: Option<ServiceWorker>,
    /// <https://w3c.github.io/ServiceWorker/#dfn-installing-worker>
    installing_worker: Option<ServiceWorker>,
    /// A channel to send control message to the worker,
    /// currently only used to signal shutdown.
    control_sender: Option<Sender<ServiceWorkerControlMsg>>,
    /// A handle to join on the worker thread.
    join_handle: Option<JoinHandle<()>>,
    /// A context to request an interrupt.
    context: Option<ThreadSafeJSContext>,
    /// The closing flag for the worker.
    closing: Option<Arc<AtomicBool>>,
    /// <https://w3c.github.io/ServiceWorker/#serviceworkercontainer-service-worker-client>
    /// The client of the container to which this registration belongs.
    client: GenericCallback<ServiceWorkerAlgorithmResult>,
    /// BAO PATCH (REQ-BRW-004 e58 contract B): the registering client's
    /// creation URL (the register job's referrer) — the `clients.matchAll`
    /// data source on the SW side.
    client_url: ServoUrl,
}

impl ServiceWorkerRegistration {
    pub(crate) fn new(
        client: GenericCallback<ServiceWorkerAlgorithmResult>,
        client_url: ServoUrl,
    ) -> ServiceWorkerRegistration {
        ServiceWorkerRegistration {
            id: ServiceWorkerRegistrationId::new(),
            active_worker: None,
            waiting_worker: None,
            installing_worker: None,
            join_handle: None,
            control_sender: None,
            context: None,
            closing: None,
            client,
            client_url,
        }
    }

    fn note_worker_thread(
        &mut self,
        join_handle: JoinHandle<()>,
        control_sender: Sender<ServiceWorkerControlMsg>,
        context: ThreadSafeJSContext,
        closing: Arc<AtomicBool>,
    ) {
        assert!(self.join_handle.is_none());
        self.join_handle = Some(join_handle);

        assert!(self.control_sender.is_none());
        self.control_sender = Some(control_sender);

        assert!(self.context.is_none());
        self.context = Some(context);

        assert!(self.closing.is_none());
        self.closing = Some(closing);
    }

    /// <https://w3c.github.io/ServiceWorker/#get-newest-worker>
    fn get_newest_worker(&self) -> Option<ServiceWorker> {
        if let Some(worker) = self.active_worker.as_ref() {
            return Some(worker.clone());
        }
        if let Some(worker) = self.waiting_worker.as_ref() {
            return Some(worker.clone());
        }
        if let Some(worker) = self.installing_worker.as_ref() {
            return Some(worker.clone());
        }
        None
    }

    /// <https://w3c.github.io/ServiceWorker/#update-registration-state>
    fn update_registration_state(
        &mut self,
        target: RegistrationUpdateTarget,
        worker: Option<ServiceWorker>,
    ) {
        match target {
            RegistrationUpdateTarget::Active => {
                self.active_worker = worker;
            },
            RegistrationUpdateTarget::Waiting => {
                self.waiting_worker = worker;
            },
            RegistrationUpdateTarget::Installing => {
                self.installing_worker = worker;
            },
        }
    }
}

/// A structure managing all registrations and workers for a given origin.
pub struct ServiceWorkerManager {
    /// <https://w3c.github.io/ServiceWorker/#dfn-scope-to-registration-map>
    registrations: HashMap<ServoUrl, ServiceWorkerRegistration>,
    /// BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling 2026-10-05):
    /// origin-wide set of enrolled client containers (upserted by client URL,
    /// latest callback wins). Populated by container-creation enrollment
    /// (`ServiceWorkerContainer::enroll_with_manager`); the delivery and
    /// `clients.matchAll` answer faces multicast across it.
    clients: Vec<ManagerClient>,
    // Round-robin cursor over `clients` (e70): one delivered message per client.
    next_client: usize,
    // own sender to send messages here
    own_sender: GenericSender<ServiceWorkerMsg>,
    // receiver to receive messages from constellation
    own_port: RoutedReceiver<ServiceWorkerMsg>,
    // to receive resource messages
    resource_receiver: RoutedReceiver<CustomResponseMediator>,
    /// A shared [`FontContext`] to use for all service workers spawned by this [`ServiceWorkerManager`].
    font_context: Arc<FontContext>,
}

/// BAO PATCH (REQ-BRW-004 e70 multi-client wave): one enrolled client
/// container — its creation URL (the matchAll/multicast identity), the
/// pipeline that enrolled it (the `ClientGone` removal identity, e75), and
/// the algorithm-result callback that doubles as its message-delivery
/// channel.
struct ManagerClient {
    client_url: ServoUrl,
    /// BAO PATCH (REQ-BRW-004 e75 unenroll teardown, user ruling
    /// 2026-10-05): the pipeline that enrolled this client — the removal
    /// identity for `ClientGone` (see `handle_client_gone`).
    client_pipeline: PipelineId,
    callback: GenericCallback<ServiceWorkerAlgorithmResult>,
}

impl ServiceWorkerManager {
    fn new(
        own_sender: GenericSender<ServiceWorkerMsg>,
        from_constellation_receiver: RoutedReceiver<ServiceWorkerMsg>,
        resource_port: RoutedReceiver<CustomResponseMediator>,
        font_context: Arc<FontContext>,
    ) -> ServiceWorkerManager {
        // Install a pipeline-namespace in the current thread.
        PipelineNamespace::auto_install();

        ServiceWorkerManager {
            registrations: HashMap::new(),
            clients: Vec::new(),
            next_client: 0,
            own_sender,
            own_port: from_constellation_receiver,
            resource_receiver: resource_port,
            font_context,
        }
    }

    pub(crate) fn get_matching_scope(&self, load_url: &ServoUrl) -> Option<ServoUrl> {
        for scope in self.registrations.keys() {
            if longest_prefix_match(scope, load_url) {
                return Some(scope.clone());
            }
        }
        None
    }

    fn handle_message(&mut self) {
        while let Ok(message) = self.receive_message() {
            let should_continue = match message {
                Message::FromConstellation(msg) => self.handle_message_from_constellation(*msg),
                Message::FromResource(msg) => self.handle_message_from_resource(msg),
            };
            if !should_continue {
                for registration in self.registrations.drain() {
                    // Signal shut-down, and join on the thread.
                    drop(registration);
                }
                break;
            }
        }
    }

    fn handle_message_from_resource(&mut self, mediator: CustomResponseMediator) -> bool {
        if serviceworker_enabled() &&
            let Some(scope) = self.get_matching_scope(&mediator.load_url) &&
            let Some(registration) = self.registrations.get(&scope) &&
            let Some(ref worker) = registration.active_worker
        {
            worker.send_message(ServiceWorkerScriptMsg::Response(mediator));
            return true;
        }
        let _ = mediator.response_chan.send(None);
        true
    }

    fn receive_message(&mut self) -> generic_channel::ReceiveResult<Message> {
        select! {
            recv(self.own_port) -> result_msg => generic_channel::to_receive_result::<ServiceWorkerMsg>(result_msg).map(|msg| Message::FromConstellation(Box::new(msg))),
            recv(self.resource_receiver) -> result_msg => generic_channel::to_receive_result::<CustomResponseMediator>(result_msg).map(Message::FromResource),
        }
    }

    fn handle_message_from_constellation(&mut self, msg: ServiceWorkerMsg) -> bool {
        match msg {
            ServiceWorkerMsg::Timeout(_scope) => {
                // TODO: https://w3c.github.io/ServiceWorker/#terminate-service-worker
            },
            ServiceWorkerMsg::ForwardDOMMessage(msg, scope_url) => {
                if let Some(registration) = self.registrations.get_mut(&scope_url) {
                    if let Some(ref worker) = registration.active_worker {
                        worker.forward_dom_message(msg);
                    } else if let Some(ref worker) = registration.waiting_worker {
                        worker.forward_dom_message(msg);
                    } else if let Some(ref worker) = registration.installing_worker {
                        worker.forward_dom_message(msg);
                    }
                }
            },
            ServiceWorkerMsg::ForwardWorkerMessage {
                data,
                url,
                source,
                origin,
                target,
            } => {
                let Some(registration) = self.registrations.get(&url) else {
                    warn!("No registration found for scope URL when forwarding message to worker.");
                    return true;
                };
                let script_url = if let Some(worker) = registration.active_worker.as_ref() {
                    worker.script_url.clone()
                } else if let Some(worker) = registration.waiting_worker.as_ref() {
                    worker.script_url.clone()
                } else if let Some(worker) = registration.installing_worker.as_ref() {
                    worker.script_url.clone()
                } else {
                    warn!("No worker found for scope URL when forwarding message to worker.");
                    return true;
                };
                // BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling
                // 2026-10-05): deliver across the origin-wide enrolled client
                // set instead of only the registering client — the old shape
                // could never reach iframe subdocument containers (e69
                // attribution: fetch-destination-worker TIMEOUT). Each
                // `Client.postMessage` call is a separately structured-cloned
                // message. BAO PATCH (REQ-BRW-004 e73 targeting): a message
                // posted on a `Client` object carries that client's creation
                // URL as `target` — deliver directly to the enrolled client
                // keyed by it (the set's identity, see `enroll_client`). A
                // target that matches nothing in the set — a stale/dead client
                // URL, or the `event.source` client the SW realm synthesizes
                // with the scope URL — falls through to the broadcast below:
                // delivery is no worse than the pre-targeting shape.
                let message = ServiceWorkerAlgorithmResult::MessageFromWorker {
                    message: data,
                    source,
                    scope_url: url,
                    script_url,
                    origin,
                };
                if let Some(target) = target
                    && let Some(client) = self.clients.iter().find(|c| c.client_url == target)
                {
                    let _ = client.callback.send(message);
                    return true;
                }
                if self.clients.is_empty() {
                    // Legacy fallback: no client has ever enrolled — deliver
                    // to the registering client exactly as before this patch.
                    let _ = registration.client.send(message);
                } else {
                    // Broadcast fallback (no identifier, or an identifier that
                    // matches nothing enrolled): one delivered message per
                    // client, rotating (round-robin) — a SW broadcasting to
                    // every `matchAll` result reaches every client exactly
                    // once, with no payload cloning
                    // (StructuredSerializedData is not Clone).
                    let index = self.next_client % self.clients.len();
                    self.next_client = self.next_client.wrapping_add(1);
                    let _ = self.clients[index].callback.send(message);
                }
            },
            ServiceWorkerMsg::HandleAlgorithm(algorithm) => match algorithm {
                ServiceWorkerAlgorithm::StartRegister(job) => {
                    self.handle_register_job(job);
                },
                ServiceWorkerAlgorithm::Unregister(job) => {
                    self.handle_unregister_job(job);
                },
                ServiceWorkerAlgorithm::MatchServiceWorkerRegistration {
                    storage_key,
                    client_url,
                    result_handler,
                    enroll_only,
                    client_pipeline,
                } => {
                    self.handle_match_registration(
                        storage_key,
                        client_url,
                        result_handler,
                        enroll_only,
                        client_pipeline,
                    );
                },
                // BAO PATCH (REQ-BRW-004 e75 unenroll teardown, user ruling
                // 2026-10-05): the document behind an enrolled client is gone
                // (its pipeline exited) — drop it from the origin-wide
                // enrolled set so delivery and `clients.matchAll` stop
                // answering with a dead container.
                ServiceWorkerAlgorithm::ClientGone {
                    client_url,
                    client_pipeline,
                    ..
                } => {
                    self.handle_client_gone(client_url, client_pipeline);
                },
            },
            // BAO PATCH (REQ-BRW-004 lifecycle wave, 2026-10-04): relay the
            // worker-thread-evidenced "script evaluated + activate dispatched"
            // report to the registering client as Update Worker State →
            // "activated". Reuses the algorithm-result channel (same as
            // MessageFromWorker); the client's pending job-promise queue is
            // not touched.
            ServiceWorkerMsg::WorkerEvaluated { worker_id } => {
                let owning_registration = self.registrations.values().find(|registration| {
                    registration
                        .get_newest_worker()
                        .is_some_and(|worker| worker.id == worker_id)
                });
                if let Some(registration) = owning_registration {
                    let _ = registration
                        .client
                        .send(ServiceWorkerAlgorithmResult::WorkerActivated { worker_id });
                } else {
                    warn!("No registration found for evaluated service worker.");
                }
            },
            ServiceWorkerMsg::Exit => return false,
        }
        true
    }

    /// <https://w3c.github.io/ServiceWorker/#unregister>
    fn handle_unregister_job(&mut self, job: Job) {
        // Step 1: Let registration be the result of running Get Registration given job’s storage key and job’s scope url.
        if self.registrations.remove(&job.scope_url).is_none() {
            // Step 2: If registration is null, then:
            // Step 2.1: Invoke Resolve Job Promise with job and false.
            if job
                .client
                .send(ServiceWorkerAlgorithmResult::Job(
                    JobResult::ResolvePromise(JobResultValue::Unregister(false)),
                ))
                .is_err()
            {
                warn!("Failed to send unregister result to script.");
            }
            // Step 2.2: Invoke Finish Job with job and abort these steps.
            // TODO: Finish Job.
            return;
        };

        // Step 3: Remove registration map[(registration’s storage key, job’s scope url)].
        // Note: done by removing the registration from the map above.

        // Step 4: Invoke Resolve Job Promise with job and true.
        if job
            .client
            .send(ServiceWorkerAlgorithmResult::Job(
                JobResult::ResolvePromise(JobResultValue::Unregister(true)),
            ))
            .is_err()
        {
            warn!("Failed to send unregister result to script.");
        }

        // Step 5: Invoke Try Clear Registration with registration.
        // TODO: Try Clear Registration.

        // Step 6: Invoke Finish Job with job.
        // TODO: Finish Job.
    }

    /// BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling 2026-10-05):
    /// upsert a client into the origin-wide enrolled set. The creation URL is
    /// the matchAll/multicast identity; a re-enrollment replaces the stale
    /// callback AND the removal identity (pipeline) of a previous document
    /// with the same URL.
    fn enroll_client(
        &mut self,
        client_url: ServoUrl,
        client_pipeline: PipelineId,
        callback: GenericCallback<ServiceWorkerAlgorithmResult>,
    ) {
        if let Some(slot) = self.clients.iter_mut().find(|c| c.client_url == client_url) {
            slot.callback = callback;
            slot.client_pipeline = client_pipeline;
            return;
        }
        self.clients.push(ManagerClient {
            client_url,
            client_pipeline,
            callback,
        });
    }

    /// <https://w3c.github.io/ServiceWorker/#match-service-worker-registration>
    ///
    /// BAO PATCH (REQ-BRW-004 e70 multi-client wave, user ruling 2026-10-05):
    /// the client-enrollment entry point (the `enroll_only` ping). Only
    /// enrollment pings mutate the set; real `clients.matchAll` queries
    /// never do (their scope-URL identity could collide with an in-scope
    /// document and hijack its delivery slot). The set is origin-wide —
    /// `matchAll` with `includeUncontrolled: true` answers with every
    /// enrolled same-origin client, which is how iframe subdocument
    /// containers become reachable.
    fn handle_match_registration(
        &mut self,
        storage_key: ImmutableOrigin,
        client_url: ServoUrl,
        result_handler: GenericCallback<ServiceWorkerAlgorithmResult>,
        enroll_only: bool,
        client_pipeline: PipelineId,
    ) {
        // BAO PATCH (REQ-BRW-004 e70 multi-client wave): enroll ONLY on the
        // dedicated enrollment ping. Real `clients.matchAll` queries must NOT
        // enroll: the SW realm's `Clients` query carries its scope URL, which
        // can equal an in-scope document's creation URL — the upsert would
        // then REPLACE that document's delivery callback with the SW's own
        // answer handler and the message would be silently swallowed there
        // (observed live: fetch-destination-worker's scope
        // `resources/dummy.html` collides with the iframe's creation URL).
        if enroll_only {
            // Upsert by (client URL, enrolling pipeline): a re-enrollment from
            // a newer document with the same creation URL replaces the stale
            // callback and the removal identity of the older document.
            self.enroll_client(client_url, client_pipeline, result_handler);
            return;
        }

        // Step 1: Run the following steps atomically.
        // Note: done using the channel from which this message was received.

        // Step 2: Let clientURLString be serialized clientURL.
        let client_url_string = client_url.as_str();

        // Step 3: Let matchingScopeString be the empty string.
        let mut matching_scope_string = String::new();

        // Step 4: Let scopeStringSet be an empty list.
        let mut scope_string_set = Vec::new();

        // Step 5: For each (entry storage key, entry scope) of registration map’s keys:
        for (entry_storage_key, entry_scope) in self.registrations.keys().map(|k| (k.origin(), k)) {
            // Step 5.1. If storage key equals entry storage key, then append entry scope to the end of scopeStringSet.
            if storage_key == entry_storage_key {
                scope_string_set.push(entry_scope.as_str());
            }
        }

        // Step 6: Set matchingScopeString to the longest value in scopeStringSet which the value of clientURLString starts with, if it exists.
        for scope in scope_string_set {
            if client_url_string.starts_with(scope) && scope.len() > matching_scope_string.len() {
                matching_scope_string = scope.to_owned();
            }
        }

        // Step 7: Let matchingScope be null.
        let mut matching_scope = None;

        // Step 8: If matchingScopeString is not the empty string, then:
        if !matching_scope_string.is_empty() {
            // Step 8.1. Set matchingScope to the result of parsing matchingScopeString.
            let Ok(parsed_matching_scope) = ServoUrl::parse(&matching_scope_string) else {
                error!("Failed to parse matching scope string as URL.");
                if result_handler
                    .send(ServiceWorkerAlgorithmResult::MatchServiceWorkerRegistration(None))
                    .is_err()
                {
                    warn!("Failed to send match registration result to script.");
                }
                return;
            };
            matching_scope = Some(parsed_matching_scope);

            // Step 8.2: Assert: matchingScope’s origin and clientURL’s origin are same origin.
            debug_assert_eq!(
                matching_scope.as_ref().unwrap().origin(),
                client_url.origin()
            );
        }

        let Some(matching_scope) = matching_scope else {
            if result_handler
                .send(ServiceWorkerAlgorithmResult::MatchServiceWorkerRegistration(None))
                .is_err()
            {
                warn!("Failed to send match registration result to script.");
            }
            return;
        };

        // Step 9: Return the result of running Get Registration given storage key and matchingScope.
        let registration = self.registrations.get(&matching_scope);
        let info = registration
            .as_ref()
            .map(|registration| ServiceWorkerRegistrationInfo {
                scope_url: matching_scope,
                script_url: registration
                    .get_newest_worker()
                    .expect("Registration should have a worker.")
                    .script_url,
                storage_key,
                id: registration.id,
                installing_worker: registration
                    .installing_worker
                    .as_ref()
                    .map(|worker| worker.id),
                waiting_worker: registration.waiting_worker.as_ref().map(|worker| worker.id),
                active_worker: registration.active_worker.as_ref().map(|worker| worker.id),
                // BAO PATCH (REQ-BRW-004 e70 multi-client wave): snapshot the
                // origin-wide enrolled set for `clients.matchAll`; legacy
                // fallback keeps the registering client when nothing has
                // enrolled yet.
                client_url: registration.client_url.clone(),
                client_urls: if self.clients.is_empty() {
                    vec![registration.client_url.clone()]
                } else {
                    self.clients.iter().map(|c| c.client_url.clone()).collect()
                },
            });
        if result_handler
            .send(ServiceWorkerAlgorithmResult::MatchServiceWorkerRegistration(info))
            .is_err()
        {
            warn!("Failed to send match registration result to script.");
        }
    }

    /// BAO PATCH (REQ-BRW-004 e75 unenroll teardown, user ruling 2026-10-05):
    /// drop the enrolled client that the exited pipeline enrolled. The pair
    /// (creation URL, enrolling pipeline) is the removal key: same-URL
    /// navigation enrolls the NEW document's container (upsert) at an
    /// arbitrary order relative to the OLD pipeline's exit, and only the
    /// pipeline stamp distinguishes a stale teardown from the live entry it
    /// must not remove. An unknown pair is a no-op (never enrolled, already
    /// gone, or a newer document re-enrolled the URL). The round-robin
    /// cursor is left alone: every delivery indexes through `% clients.len()`,
    /// so the shrink can at worst repeat one client once.
    fn handle_client_gone(&mut self, client_url: ServoUrl, client_pipeline: PipelineId) {
        let before = self.clients.len();
        self.clients.retain(|client| {
            client.client_url != client_url || client.client_pipeline != client_pipeline
        });
        if self.clients.len() != before {
            info!(
                "Service worker client gone (unenrolled): url={client_url} pipeline={client_pipeline:?}"
            );
        }
    }

    /// <https://w3c.github.io/ServiceWorker/#register-algorithm>
    fn handle_register_job(&mut self, mut job: Job) {
        // Step 1: If the result of running potentially trustworthy origin with the origin of job’s script url as the argument is Not Trusted, then:
        if !job.script_url.origin().is_potentially_trustworthy() {
            // Step 1.1: Invoke Reject Job Promise with job and "SecurityError" DOMException.
            if job
                .client
                .send(ServiceWorkerAlgorithmResult::Job(JobResult::RejectPromise(
                    JobError::SecurityError,
                )))
                .is_err()
            {
                warn!("Failed to send reject job promise result to script.");
            }

            // TODO Step 1.2: Invoke Finish Job with job and abort these steps.
            // TODO: Finish Job.
            return;
        }

        // Step 2: If job’s script url’s origin and job’s referrer’s origin are not same origin, then:
        // Step 3: If job’s scope url’s origin and job’s referrer’s origin are not same origin, then:
        // Note: both steps done in one conditional.
        if job.script_url.origin() != job.referrer.origin() ||
            job.scope_url.origin() != job.referrer.origin()
        {
            // Step 2.1: Invoke Reject Job Promise with job and "SecurityError" DOMException
            if job
                .client
                .send(ServiceWorkerAlgorithmResult::Job(JobResult::RejectPromise(
                    JobError::SecurityError,
                )))
                .is_err()
            {
                warn!("Failed to send reject job promise result to script.");
            }

            // TODO Step 2.2: Invoke Finish Job with job and abort these steps.
            return;
        }

        // Step 4: Let registration be the result of running Get Registration given job’s storage key and job’s scope url.
        if let Some(registration) = self.registrations.get(&job.scope_url) {
            // Step 5: If registration is not null, then:

            // Step 5.1: Let newestWorker be the result of running the Get Newest Worker algorithm passing registration as the argument.
            let newest_worker = registration.get_newest_worker();

            // step 5.2: If newestWorker is not null, job’s script url equals newestWorker’s script url, job’s worker type equals newestWorker’s type, and job’s update via cache mode’s value equals registration’s update via cache mode, then:
            if newest_worker.is_some() {
                // TODO: the various checks of job versus worker.

                // Step 5.2.1: Invoke Resolve Job Promise with job and registration.
                // Step 5.2.2: Invoke Finish Job with job and abort these steps.
                // TODO: Finish Job.
                let client = job.client.clone();
                let _ = client.send(ServiceWorkerAlgorithmResult::Job(
                    JobResult::ResolvePromise(JobResultValue::Register(
                        ServiceWorkerRegistrationInfo {
                            scope_url: job.scope_url.clone(),
                            script_url: job.script_url.clone(),
                            storage_key: job.storage_key.clone(),
                            id: registration.id,
                            installing_worker: registration
                                .installing_worker
                                .as_ref()
                                .map(|worker| worker.id),
                            waiting_worker: registration
                                .waiting_worker
                                .as_ref()
                                .map(|worker| worker.id),
                            active_worker: registration
                                .active_worker
                                .as_ref()
                                .map(|worker| worker.id),
                            client_url: registration.client_url.clone(),
                            client_urls: Vec::new(),
                        },
                    )),
                ));
            }
        } else {
            // Step 6: Else
            // Step 6.1: Invoke Set Registration algorithm with job’s storage key, job’s scope url, and job’s update via cache mode.
            let new_registration =
                ServiceWorkerRegistration::new(job.client.clone(), job.referrer.clone());
            self.registrations
                .insert(job.scope_url.clone(), new_registration);

            // Step 7: Invoke Update algorithm passing job as the argument.
            job.job_type = JobType::Update;
            self.handle_update_job(job);
        }
    }

    /// <https://www.w3.org/TR/service-workers/#install>
    fn install(&mut self, job: Job, new_worker: ServiceWorker) {
        let Some(registration) = self.registrations.get_mut(&job.scope_url) else {
            error!("Registration should exist when installing a worker.");
            return;
        };

        // Step 4: Run the Update Registration State algorithm passing registration, "installing" and worker as the arguments.
        registration.update_registration_state(
            RegistrationUpdateTarget::Installing,
            Some(new_worker.clone()),
        );

        // Step 17: Run the Update Registration State algorithm passing registration,
        // "waiting" and registration’s installing worker as the arguments.
        registration.update_registration_state(RegistrationUpdateTarget::Waiting, Some(new_worker));

        // Step 18: Run the Update Registration State algorithm passing registration, "installing" and null as the arguments.
        // TODO: registration.update_registration_state(RegistrationUpdateTarget::Installing, None);
        // Note: commenting out for now, because it causes errors in tests,
        // probably because we are still lacking functionality elsewhere.

        // BAO PATCH (REQ-BRW-004 C19, user ruling 2026-09-09): the spec's
        // "Try Activate" step (install finishes by activating the waiting
        // worker when the registration has no active worker —
        // <https://w3c.github.io/ServiceWorkers/#activation-algorithm>: set
        // the active worker to the waiting worker and clear waiting). Upstream
        // never passes `RegistrationUpdateTarget::Active` anywhere, so
        // `active_worker` stays null forever and `handle_message_from_resource`
        // answers every fetch mediation with pass-through — a service worker
        // could never intercept anything. An existing active worker keeps
        // control (the waiting worker stays waiting, browser update
        // semantics).
        if registration.active_worker.is_none()
            && let Some(waiting) = registration.waiting_worker.clone()
        {
            registration.update_registration_state(RegistrationUpdateTarget::Active, Some(waiting));
            registration.update_registration_state(RegistrationUpdateTarget::Waiting, None);
        }

        // Step 7: Invoke Resolve Job Promise with job and registration.
        // BAO PATCH (REQ-BRW-004 C19 controller wave, user ruling 2026-09-09):
        // upstream sent this resolution at Step 7, before the waiting→active
        // transitions, so the resolved ServiceWorkerRegistrationInfo always
        // carried pre-activation state (active_worker: None on first
        // registration). Moved after the activation transitions: this resolve
        // is the only activation notification the registering client ever
        // gets, and the page-side container needs `active_worker` populated
        // here to assign `navigator.serviceWorker.controller` in the same
        // task that settles the register() promise.
        let client = job.client.clone();
        if client
            .send(ServiceWorkerAlgorithmResult::Job(
                JobResult::ResolvePromise(JobResultValue::Register(
                    ServiceWorkerRegistrationInfo {
                        scope_url: job.scope_url.clone(),
                        storage_key: job.storage_key.clone(),
                        script_url: job.script_url.clone(),
                        id: registration.id,
                        installing_worker: registration
                            .installing_worker
                            .as_ref()
                            .map(|worker| worker.id),
                        waiting_worker: registration
                            .waiting_worker
                            .as_ref()
                            .map(|worker| worker.id),
                        active_worker: registration.active_worker.as_ref().map(|worker| worker.id),
                        client_url: registration.client_url.clone(),
                        // BAO PATCH (REQ-BRW-004 e70): the Register job answer carries no
                        // client set; consumers ignore it (matchAll is the set consumer).
                        client_urls: Vec::new(),
                    },
                )),
            ))
            .is_err()
        {
            warn!("Failed to send resolve job promise result to script.");
        }

        // BAO PATCH (REQ-BRW-004 lifecycle wave, 2026-10-04): Update
        // Registration State set the installing worker above (null → worker);
        // the spec queues an "updatefound" event on the registration's
        // ServiceWorkerRegistration objects at that point. Sent AFTER the
        // resolve so the registering page's promise handler — which attaches
        // its updatefound listener (wait_for_update in the WPT SW test
        // helpers) — runs in the resolve task's microtasks first; the two
        // callback deliveries are FIFO, so the event lands in a later task.
        let _ = client.send(ServiceWorkerAlgorithmResult::UpdateFound {
            registration_id: registration.id,
        });

        // Step 21: Wait for all the tasks queued by Update Worker State invoked in this algorithm to have executed.
        // TODO: queue tasks above and wait for them to execute.
    }

    /// <https://w3c.github.io/ServiceWorker/#update>
    fn handle_update_job(&mut self, job: Job) {
        // Step 1: Get registation
        let (job, new_worker) =
            if let Some(registration) = self.registrations.get_mut(&job.scope_url) {
                // Step 3.
                let newest_worker = registration.get_newest_worker();

                // Step 4.
                if let Some(worker) = newest_worker &&
                    worker.script_url != job.script_url
                {
                    let _ = job.client.send(ServiceWorkerAlgorithmResult::Job(
                        JobResult::RejectPromise(JobError::TypeError),
                    ));
                    return;
                }

                let scope_things = job
                    .scope_things
                    .clone()
                    .expect("Update job should have scope things.");

                // Very roughly steps 5 to 18.
                // TODO: implement all steps precisely.
                let (new_worker, join_handle, control_sender, context, closing) =
                    update_serviceworker(
                        self.own_sender.clone(),
                        job.scope_url.clone(),
                        scope_things,
                        self.font_context.clone(),
                    );

                // Since we've just started the worker thread, ensure we can shut it down later.
                registration.note_worker_thread(join_handle, control_sender, context, closing);

                (job, new_worker)
            } else {
                // Step 2
                let _ =
                    job.client
                        .send(ServiceWorkerAlgorithmResult::Job(JobResult::RejectPromise(
                            JobError::TypeError,
                        )));
                return;
            };
        // Step 17: Else, invoke Install algorithm with job, worker, and registration as its arguments.
        self.install(job, new_worker);
    }
}

/// <https://w3c.github.io/ServiceWorker/#update-algorithm>
fn update_serviceworker(
    own_sender: GenericSender<ServiceWorkerMsg>,
    scope_url: ServoUrl,
    mut scope_things: ScopeThings,
    font_context: Arc<FontContext>,
) -> (
    ServiceWorker,
    JoinHandle<()>,
    Sender<ServiceWorkerControlMsg>,
    ThreadSafeJSContext,
    Arc<AtomicBool>,
) {
    let (sender, receiver) = unbounded();
    let (devtools_sender, devtools_receiver) = generic_channel::channel().unwrap();
    scope_things.init.from_devtools_sender = Some(devtools_sender);

    if let Some(ref chan) = scope_things.devtools_chan &&
        let Some(ref sender) = scope_things.init.from_devtools_sender
    {
        let page_info = DevtoolsPageInfo {
            title: format!("Service Worker for {}", scope_things.script_url),
            url: scope_things.script_url.clone(),
            is_top_level_global: false,
            is_service_worker: true,
        };
        let _ = chan.send(ScriptToDevtoolsControlMsg::NewGlobal(
            (
                scope_things.browsing_context_id,
                scope_things.init.pipeline_id,
                Some(scope_things.worker_id),
                scope_things.webview_id,
            ),
            sender.clone(),
            page_info,
        ));
    }

    let worker_id = ServiceWorkerId::new();

    let (control_sender, control_receiver) = unbounded();
    let (context_sender, context_receiver) = unbounded();
    let closing = Arc::new(AtomicBool::new(false));

    let join_handle = ServiceWorkerGlobalScope::run_serviceworker_scope(
        scope_things.clone(),
        sender.clone(),
        receiver,
        devtools_receiver,
        own_sender,
        scope_url,
        control_receiver,
        context_sender,
        closing.clone(),
        font_context,
        worker_id,
    );

    let context = context_receiver
        .recv()
        .expect("Couldn't receive a context for worker.");

    (
        ServiceWorker::new(scope_things.script_url, sender, worker_id),
        join_handle,
        control_sender,
        context,
        closing,
    )
}

impl ServiceWorkerManagerFactory for ServiceWorkerManager {
    fn create(sw_senders: SWManagerSenders, origin: ImmutableOrigin) {
        let (resource_chan, resource_port) = generic_channel::channel().unwrap();

        let SWManagerSenders {
            resource_threads,
            own_sender,
            receiver,
            system_font_service_sender,
            paint_api,
        } = sw_senders;

        let from_constellation = receiver.route_preserving_errors();
        let resource_port = resource_port.route_preserving_errors();
        let _ = resource_threads
            .core_thread
            .send(CoreResourceMsg::NetworkMediator(resource_chan, origin));

        let font_context = Arc::new(FontContext::new(
            Arc::new(system_font_service_sender.to_proxy()),
            paint_api,
            resource_threads,
        ));

        let swmanager_thread = move || {
            ServiceWorkerManager::new(own_sender, from_constellation, resource_port, font_context)
                .handle_message()
        };
        if thread::Builder::new()
            .name("SvcWorkerManager".to_owned())
            .spawn(swmanager_thread)
            .is_err()
        {
            warn!("ServiceWorkerManager thread spawning failed");
        }
    }
}

pub(crate) fn serviceworker_enabled() -> bool {
    pref!(dom_serviceworker_enabled)
}
