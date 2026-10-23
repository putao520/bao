/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! <https://w3c.github.io/ServiceWorker/#cache>
//!
//! Bao vendor patch (REQ-BRW-051): upstream only implemented `Cache.keys()`
//! (and only its no-argument form). This module implements the full Cache API
//! storage face — `put`/`add`/`addAll`/`match`/`matchAll`/`delete`/`keys` —
//! with the W3C algorithm shapes. Cache entries live in the storage thread
//! (`storage::cache_storage`); this module owns the DOM-facing algorithms
//! (validation, body buffering, `Request`/`Response` reconstruction).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use dom_struct::dom_struct;
use http::HeaderMap as HyperHeaders;
use js::context::JSContext;
use js::realm::CurrentRealm;
use js::rust::HandleValue;
use malloc_size_of::MallocSizeOf;
use net_traits::http_status::HttpStatus;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use script_bindings::root::DomRoot;
use script_bindings::str::{ByteString, USVString};
use servo_base::generic_channel::{GenericCallback, GenericSend};
use servo_url::{ImmutableOrigin, ServoUrl};
use storage_traits::cache_storage::{
    CacheBatchError, CacheEntry, CacheQueryOptions as StorageQueryOptions, CacheRequest,
    CacheResponse, CacheResponseType, CacheStorageThreadMessage, CacheStorageThreadResponse,
};

use crate::dom::RootedPromise;
use crate::dom::bindings::codegen::Bindings::CacheBinding::CacheMethods;
use crate::dom::bindings::codegen::Bindings::RequestBinding::{
    RequestInfo, RequestInit, RequestMethods,
};
use crate::dom::bindings::codegen::Bindings::ResponseBinding::{
    ResponseMethods, ResponseType as DOMResponseType,
};
use crate::dom::bindings::codegen::GenericBindings::CacheBinding::CacheQueryOptions;
use crate::dom::bindings::conversions::root_from_handlevalue;
use crate::dom::bindings::error::Error;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::str::DOMString;
use crate::dom::fetch::request::Request;
use crate::dom::fetch::response::Response;
use crate::dom::globalscope::GlobalScope;
use crate::dom::headers::Guard;
use crate::dom::promise::{Promise, TracedPromise};
use crate::dom::promisenativehandler::Callback;
use crate::dom::types::PromiseNativeHandler;
use crate::fetch::body::BodyMixin;
use crate::fetch::fetch::Fetch;
use crate::realms::enter_auto_realm;

/// The parts of a `Response` that are captured synchronously when an entry is
/// written to a cache. The body is buffered separately (asynchronously).
struct ResponseMetadata {
    status: u16,
    status_text: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    url: Option<ServoUrl>,
    response_type: CacheResponseType,
    redirected: bool,
}

impl ResponseMetadata {
    fn into_response(self, body: Vec<u8>) -> CacheResponse {
        CacheResponse {
            status: self.status,
            status_text: self.status_text,
            headers: self.headers,
            url: self.url,
            response_type: self.response_type,
            redirected: self.redirected,
            body,
        }
    }
}

/// <https://w3c.github.io/ServiceWorker/#cache>
#[dom_struct]
pub(crate) struct Cache {
    reflector_: Reflector,

    /// The name used to identify
    /// <https://w3c.github.io/ServiceWorker/#dfn-relevant-request-response-list>
    name: DOMString,

    #[no_trace]
    #[ignore_malloc_size_of = "GenericCallback"]
    callback: RefCell<Option<GenericCallback<CacheStorageThreadResponse>>>,

    // Dequeue of pending promises for backend operations, in the order the
    // corresponding messages were sent to the storage thread.
    pending_promises: RefCell<VecDeque<TracedPromise>>,
}

impl Cache {
    fn new_inherited(name: DOMString) -> Cache {
        Cache {
            reflector_: Reflector::new(),
            name,
            callback: Default::default(),
            pending_promises: Default::default(),
        }
    }

    pub(crate) fn new(cx: &mut JSContext, global: &GlobalScope, name: DOMString) -> DomRoot<Cache> {
        reflect_dom_object(cx, Box::new(Cache::new_inherited(name)), global)
    }

    /// Setup the callback to the backend service, if this hasn't been done already.
    pub(crate) fn get_or_setup_callback(&self) -> GenericCallback<CacheStorageThreadResponse> {
        if let Some(cb) = self.callback.borrow().as_ref() {
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
                Ok(inner) => Some(inner),
                Err(err) => {
                    error!("Error in Cache callback {:?}.", err);
                    None
                },
            };
            task_source.queue(task!(set_request_result_to_database: move |cx| {
                let cache = response_listener.root();
                cache.handle_response(cx, response)
            }));
        })
        .expect("Could not create Cache callback");

        *self.callback.borrow_mut() = Some(callback.clone());

        callback
    }

    /// Queue a storage request and record its promise. The storage thread
    /// answers FIFO, so sends and the pending queue must stay in lockstep.
    fn send_and_track(
        &self,
        cx: &mut JSContext,
        promise: &RootedPromise,
        message: CacheStorageThreadMessage,
    ) {
        let global = self.global();
        if global.storage_threads().send(message).is_err() {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return;
        }
        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());
    }

    fn pop_pending(&self, cx: &mut JSContext, context: &str) -> Option<RootedPromise> {
        let promise = self
            .pending_promises
            .borrow_mut()
            .pop_front()
            .map(|promise| promise.root(cx));
        if promise.is_none() {
            error!("No pending promise for Cache {context} response.");
        }
        promise
    }

    fn handle_response(&self, cx: &mut JSContext, response: Option<CacheStorageThreadResponse>) {
        let response = match response {
            Some(response) => response,
            None => {
                let Some(promise) = self.pop_pending(cx, "error") else {
                    return;
                };
                promise.reject_error(
                    cx,
                    Error::Operation(Some("No response from Cache backend.".to_string())),
                );
                return;
            },
        };
        match response {
            // <https://w3c.github.io/ServiceWorker/#cache-keys>
            CacheStorageThreadResponse::KeysResult(result) => {
                let Some(promise) = self.pop_pending(cx, "KeysResult") else {
                    return;
                };
                let requests = match result {
                    Ok(requests) => requests,
                    Err(error) => {
                        promise.reject_error(cx, Error::Operation(Some(error)));
                        return;
                    },
                };
                // Step 5.4.1: Let requestList be a list.
                match requests
                    .iter()
                    .map(|request| request_from_cache(cx, &self.global(), request))
                    .collect::<Result<Vec<_>, _>>()
                {
                    // Step 5.4.3: Resolve promise with a frozen array created
                    // from requestList, in realm.
                    Ok(request_list) => promise.resolve_native(cx, &request_list),
                    Err(error) => promise.reject_error(cx, error),
                }
            },
            // <https://w3c.github.io/ServiceWorker/#cache-match>
            CacheStorageThreadResponse::MatchResult(result) => {
                let Some(promise) = self.pop_pending(cx, "MatchResult") else {
                    return;
                };
                match result {
                    Ok(Some(response)) => match self.response_from_cache(cx, response) {
                        Ok(response) => promise.resolve_native(cx, &response),
                        Err(error) => promise.reject_error(cx, error),
                    },
                    Ok(None) => promise.resolve_native(cx, &()),
                    Err(error) => promise.reject_error(cx, Error::Operation(Some(error))),
                }
            },
            // <https://w3c.github.io/ServiceWorker/#cache-matchall>
            CacheStorageThreadResponse::MatchAllResult(result) => {
                let Some(promise) = self.pop_pending(cx, "MatchAllResult") else {
                    return;
                };
                let responses = match result {
                    Ok(responses) => responses,
                    Err(error) => {
                        promise.reject_error(cx, Error::Operation(Some(error)));
                        return;
                    },
                };
                match responses
                    .into_iter()
                    .map(|response| self.response_from_cache(cx, response))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(response_list) => promise.resolve_native(cx, &response_list),
                    Err(error) => promise.reject_error(cx, error),
                }
            },
            // <https://w3c.github.io/ServiceWorker/#cache-put>
            CacheStorageThreadResponse::PutResult(result) => {
                let Some(promise) = self.pop_pending(cx, "PutResult") else {
                    return;
                };
                match result {
                    Ok(()) => promise.resolve_native(cx, &()),
                    Err(CacheBatchError::InvalidState) => {
                        promise.reject_error(cx, Error::InvalidState(None))
                    },
                    Err(CacheBatchError::Internal(error)) => {
                        promise.reject_error(cx, Error::Operation(Some(error)))
                    },
                }
            },
            // <https://w3c.github.io/ServiceWorker/#cache-delete>
            CacheStorageThreadResponse::DeleteEntriesResult(result) => {
                let Some(promise) = self.pop_pending(cx, "DeleteEntriesResult") else {
                    return;
                };
                match result {
                    Ok(deleted) => promise.resolve_native(cx, &deleted),
                    Err(error) => promise.reject_error(cx, Error::Operation(Some(error))),
                }
            },
            CacheStorageThreadResponse::HasCacheResult(_) => debug_assert!(
                false,
                "Unexpected HasCacheResult response in Cache handle_response."
            ),
            CacheStorageThreadResponse::DeleteCacheResult(_) => debug_assert!(
                false,
                "Unexpected DeleteCacheResult response in Cache handle_response."
            ),
            CacheStorageThreadResponse::OpenCacheResult { .. } => debug_assert!(
                false,
                "Unexpected OpenCacheResult response in Cache handle_response."
            ),
            CacheStorageThreadResponse::StorageKeysResult(_) => debug_assert!(
                false,
                "Unexpected StorageKeysResult response in Cache handle_response."
            ),
            CacheStorageThreadResponse::StorageMatchResult(_) => debug_assert!(
                false,
                "Unexpected StorageMatchResult response in Cache handle_response."
            ),
        }
    }

    /// Reconstruct a `Response` for a cache hit.
    fn response_from_cache(
        &self,
        cx: &mut JSContext,
        response: CacheResponse,
    ) -> Result<DomRoot<Response>, Error> {
        response_from_cache(cx, &self.global(), response)
    }
}

/// Reconstruct a `Response` from stored cache data (shared with
/// `CacheStorage.match`).
pub(crate) fn response_from_cache(
    cx: &mut JSContext,
    global: &GlobalScope,
    response: CacheResponse,
) -> Result<DomRoot<Response>, Error> {
    let status = match response.response_type {
        CacheResponseType::Error
        | CacheResponseType::Opaque
        | CacheResponseType::OpaqueRedirect => HttpStatus::new_error(),
        _ => HttpStatus::new_raw(response.status, response.status_text),
    };
    let response_type = match response.response_type {
        CacheResponseType::Default => DOMResponseType::Default,
        CacheResponseType::Basic => DOMResponseType::Basic,
        CacheResponseType::Cors => DOMResponseType::Cors,
        CacheResponseType::Error => DOMResponseType::Error,
        CacheResponseType::Opaque => DOMResponseType::Opaque,
        CacheResponseType::OpaqueRedirect => DOMResponseType::Opaqueredirect,
    };
    let redirected = response.redirected;
    let response = Response::new_from_parts(
        cx,
        global,
        status,
        headers_from_vec(&response.headers),
        response_type,
        response.url,
        response.body,
    )?;
    response.set_redirected(redirected);
    Ok(response)
}

/// The generated trait passes no `cx` to these methods; acquire the script
/// thread's context locally (same shape as `ImageDecoder::thread_cx`).
#[allow(unsafe_code)]
fn thread_cx() -> JSContext {
    unsafe { JSContext::get_from_thread() }.expect("no active JS context")
}

impl CacheMethods<crate::DomTypeHolder> for Cache {
    /// <https://w3c.github.io/ServiceWorker/#cache-match>
    fn Match(&self, request: RequestInfo, options: &CacheQueryOptions) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);
        let request = match cache_request_from_info(cx, &global, request) {
            Ok(request) => request,
            Err(error) => {
                promise.reject_error(cx, error);
                return promise;
            },
        };
        let options = storage_query_options(options);
        // matchAll Step 2.1.2: a non-GET query without ignoreMethod resolves
        // with an empty array, so match resolves with undefined.
        if request.method != "GET" && !options.ignore_method {
            promise.resolve_native(cx, &());
            return promise;
        }
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        self.send_and_track(
            cx,
            &promise,
            CacheStorageThreadMessage::Match {
                cache_name: self.name.to_string(),
                request: Some(request),
                options,
                callback,
                origin,
            },
        );
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-matchall>
    fn MatchAll(&self, request: Option<RequestInfo>, options: &CacheQueryOptions) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);
        let query = match request {
            Some(request) => match cache_request_from_info(cx, &global, request) {
                Ok(request) => Some(request),
                Err(error) => {
                    promise.reject_error(cx, error);
                    return promise;
                },
            },
            None => None,
        };
        let options = storage_query_options(options);
        if query
            .as_ref()
            .is_some_and(|request| request.method != "GET" && !options.ignore_method)
        {
            promise.resolve_native(cx, &Vec::<DomRoot<Response>>::new());
            return promise;
        }
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        self.send_and_track(
            cx,
            &promise,
            CacheStorageThreadMessage::MatchAll {
                cache_name: self.name.to_string(),
                request: query,
                options,
                callback,
                origin,
            },
        );
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-add>
    fn Add(&self, request: RequestInfo) -> RootedPromise {
        // Step 1: Let requests be an array containing only request.
        // Step 2-3: Return the result of reacting to addAll's result.
        self.AddAll(vec![request])
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-addall>
    fn AddAll(&self, requests: Vec<RequestInfo>) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);

        // Steps 3-4: validate every request (scheme + GET) and construct the
        // `Request` objects that will be fetched.
        let mut cache_requests = Vec::with_capacity(requests.len());
        let mut dom_requests = Vec::with_capacity(requests.len());
        for info in requests {
            let request = match Request::constructor(cx, &global, None, info, &RequestInit::empty())
            {
                Ok(request) => request,
                Err(error) => {
                    promise.reject_error(cx, error);
                    return promise;
                },
            };
            {
                let net_request = request.request();
                if !matches!(net_request.url().scheme(), "http" | "https")
                    || net_request.method != http::Method::GET
                {
                    promise.reject_error(
                        cx,
                        Error::Type(
                            c"Cache.add/addAll only supports http(s) GET requests".to_owned(),
                        ),
                    );
                    return promise;
                }
            }
            cache_requests.push(cache_request_from_request(&request));
            dom_requests.push(request);
        }

        if dom_requests.is_empty() {
            // No operations: the batch is a no-op that resolves with undefined.
            promise.resolve_native(cx, &());
            return promise;
        }

        // Step 4: fetch every request. Each response body is buffered by the
        // per-fetch handlers; when the last one completes the batch put runs.
        let shared = Rc::new(AddAllShared {
            requests: cache_requests,
            entries: RefCell::new(vec![None; dom_requests.len()]),
            remaining: Cell::new(dom_requests.len()),
            failed: Cell::new(false),
            cache: Trusted::new(self),
            promise: promise.to_traced(),
            cache_name: self.name.to_string(),
            origin: global.origin().immutable().clone(),
        });

        let request_init = RequestInit::empty();
        for (index, request) in dom_requests.into_iter().enumerate() {
            let fetch_promise = {
                let mut realm = enter_auto_realm(cx, &*global);
                Fetch(
                    &global,
                    RequestInfo::Request(request),
                    &request_init,
                    &mut realm.current_realm(),
                )
            };
            let handler = PromiseNativeHandler::new(
                cx,
                &global,
                Some(Box::new(AddAllFetchHandler {
                    shared: shared.clone(),
                    index,
                })),
                Some(Box::new(AddAllFetchErrorHandler {
                    shared: shared.clone(),
                })),
            );
            let mut realm = enter_auto_realm(cx, &*global);
            fetch_promise.append_native_handler(&mut realm.current_realm(), &handler);
        }

        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-put>
    fn Put(&self, request: RequestInfo, response: &Response) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);

        // Step 1-2: Let innerRequest be request's request, or the result of
        // invoking Request's constructor with request as its argument.
        let inner_request = match cache_request_from_info(cx, &global, request) {
            Ok(request) => request,
            Err(error) => {
                promise.reject_error(cx, error);
                return promise;
            },
        };

        // Step 3: If innerRequest's URL's scheme is not one of http and https,
        // or its method is not `GET`, reject with a TypeError.
        if !matches!(inner_request.url.scheme(), "http" | "https") || inner_request.method != "GET"
        {
            promise.reject_error(
                cx,
                Error::Type(c"Cache.put only supports http(s) GET requests".to_owned()),
            );
            return promise;
        }

        // Step 4-5: If innerResponse's status is 206, reject with a TypeError.
        if response.Status() == 206 {
            promise.reject_error(
                cx,
                Error::Type(c"Cache.put does not support 206".to_owned()),
            );
            return promise;
        }

        // Step 6: If innerResponse's header list contains `Vary: *`, reject
        // with a TypeError.
        if response_has_vary_star(response.Headers(cx).get_headers_list()) {
            promise.reject_error(
                cx,
                Error::Type(c"Cache.put does not support Vary: *".to_owned()),
            );
            return promise;
        }

        // Step 7: If innerResponse's body is disturbed or locked, reject with
        // a TypeError.
        if response.is_disturbed() || response.is_locked() {
            promise.reject_error(
                cx,
                Error::Type(c"Cache.put does not support a used body".to_owned()),
            );
            return promise;
        }

        // Steps 8-9: Capture the response's observable state before reading
        // the body, mirroring the spec's clone step (later header mutations by
        // the caller must not leak into the stored entry).
        let metadata_slot = RefCell::new(Some(response_metadata(cx, response)));

        // `Rc`-shared so the body-reader success callback can invoke it from
        // an `Fn` closure (a bare closure capture would move out on call).
        let send_entry = Rc::new({
            let promise = promise.to_traced();
            let trusted_cache = Trusted::new(self);
            let cache_name = self.name.to_string();
            let origin = global.origin().immutable().clone();
            move |cx: &mut JSContext, body: Vec<u8>| {
                let metadata = metadata_slot
                    .borrow_mut()
                    .take()
                    .expect("Cache.put reads the body at most once");
                let cache = trusted_cache.root();
                let entry = CacheEntry {
                    request: inner_request.clone(),
                    response: metadata.into_response(body),
                };
                let callback = cache.get_or_setup_callback();
                let message = CacheStorageThreadMessage::Put {
                    cache_name: cache_name.clone(),
                    entries: vec![entry],
                    callback,
                    origin: origin.clone(),
                };
                if cache.global().storage_threads().send(message).is_err() {
                    let promise = promise.root(cx);
                    promise.reject_error(
                        cx,
                        Error::Operation(Some("Could not run the parallel steps.".to_string())),
                    );
                } else {
                    cache
                        .pending_promises
                        .borrow_mut()
                        .push_back(promise.clone());
                }
            }
        });

        // Steps 10-11: Read all bytes from the response body. An absent body
        // reads as an empty byte sequence. The response's own stream is read
        // (not a tee branch), so the body is consumed by the put.
        match response.body() {
            None => send_entry(cx, Vec::new()),
            Some(stream) => {
                if stream.is_errored() {
                    promise.reject_error(cx, Error::Type(c"The body stream is errored".to_owned()));
                    return promise;
                }
                let reader = match stream.acquire_default_reader(cx) {
                    Ok(reader) => reader,
                    Err(error) => {
                        promise.reject_error(cx, error);
                        return promise;
                    },
                };
                let failure_promise = promise.to_traced();
                reader.read_all_bytes(
                    cx,
                    Rc::new(move |cx: &mut JSContext, bytes: &[u8]| {
                        send_entry(cx, bytes.to_vec());
                    }),
                    Rc::new(move |cx: &mut JSContext, _error| {
                        let promise = failure_promise.root(cx);
                        promise.reject_error(
                            cx,
                            Error::Type(c"Could not read the response body".to_owned()),
                        );
                    }),
                );
            },
        }

        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-delete>
    fn Delete(&self, request: RequestInfo, options: &CacheQueryOptions) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);
        let request = match cache_request_from_info(cx, &global, request) {
            Ok(request) => request,
            Err(error) => {
                promise.reject_error(cx, error);
                return promise;
            },
        };
        let options = storage_query_options(options);

        // Step 2.1.2: If r's method is not `GET` and options.ignoreMethod is
        // false, return a promise resolved with false.
        if request.method != "GET" && !options.ignore_method {
            promise.resolve_native(cx, &false);
            return promise;
        }

        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        self.send_and_track(
            cx,
            &promise,
            CacheStorageThreadMessage::Delete {
                cache_name: self.name.to_string(),
                request,
                options,
                callback,
                origin,
            },
        );
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-keys>
    fn Keys(
        &self,
        cx: &mut JSContext,
        request: Option<RequestInfo>,
        options: &CacheQueryOptions,
    ) -> RootedPromise {
        let global = self.global();
        let promise = Promise::new(cx, &global);
        let query = match request {
            Some(request) => match cache_request_from_info(cx, &global, request) {
                Ok(request) => Some(request),
                Err(error) => {
                    promise.reject_error(cx, error);
                    return promise;
                },
            },
            None => None,
        };
        let options = storage_query_options(options);
        // Step 2.1.2: If r's method is not `GET` and options.ignoreMethod is
        // false, return a promise resolved with an empty array.
        if query
            .as_ref()
            .is_some_and(|request| request.method != "GET" && !options.ignore_method)
        {
            promise.resolve_native(cx, &Vec::<DomRoot<Request>>::new());
            return promise;
        }
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        self.send_and_track(
            cx,
            &promise,
            CacheStorageThreadMessage::Keys {
                cache_name: self.name.to_string(),
                request: query,
                options,
                callback,
                origin,
            },
        );
        promise
    }
}

/// Shared state for one `Cache.addAll` batch: the request list (in input
/// order), the fetched responses, and the outer promise.
#[derive(JSTraceable, MallocSizeOf)]
struct AddAllShared {
    #[no_trace]
    #[ignore_malloc_size_of = "plain cache data"]
    requests: Vec<CacheRequest>,
    #[no_trace]
    #[ignore_malloc_size_of = "plain cache data"]
    entries: RefCell<Vec<Option<CacheEntry>>>,
    remaining: Cell<usize>,
    failed: Cell<bool>,
    cache: Trusted<Cache>,
    promise: TracedPromise,
    cache_name: String,
    #[no_trace]
    origin: ImmutableOrigin,
}

impl AddAllShared {
    fn fail(&self, cx: &mut JSContext) {
        if self.failed.replace(true) {
            return;
        }
        let promise = self.promise.root(cx);
        promise.reject_error(
            cx,
            Error::Type(c"Cache.add/addAll failed to fetch a request".to_owned()),
        );
    }

    fn finish(&self, cx: &mut JSContext, index: usize, response: CacheResponse) {
        if self.failed.get() {
            return;
        }
        let request = self
            .requests
            .get(index)
            .cloned()
            .expect("The request list is filled in input order");
        if let Some(slot) = self.entries.borrow_mut().get_mut(index) {
            *slot = Some(CacheEntry { request, response });
        }
        let remaining = self.remaining.get().saturating_sub(1);
        self.remaining.set(remaining);
        if remaining > 0 {
            return;
        }
        // Every response body is buffered: run Batch Cache Operations.
        let entries: Option<Vec<CacheEntry>> = self.entries.borrow().iter().cloned().collect();
        let Some(entries) = entries else {
            self.fail(cx);
            return;
        };
        let cache = self.cache.root();
        let callback = cache.get_or_setup_callback();
        let message = CacheStorageThreadMessage::Put {
            cache_name: self.cache_name.clone(),
            entries,
            callback,
            origin: self.origin.clone(),
        };
        if cache.global().storage_threads().send(message).is_err() {
            self.fail(cx);
            return;
        }
        cache
            .pending_promises
            .borrow_mut()
            .push_back(self.promise.clone());
    }
}

/// Fulfillment steps of one `Cache.addAll` fetch promise: validate the
/// response and buffer its body.
#[derive(JSTraceable, MallocSizeOf)]
struct AddAllFetchHandler {
    #[conditional_malloc_size_of]
    shared: Rc<AddAllShared>,
    index: usize,
}

impl Callback for AddAllFetchHandler {
    fn callback(&self, cx: &mut CurrentRealm, value: HandleValue) {
        let Ok(response) = root_from_handlevalue::<Response>(cx, value) else {
            self.shared.fail(cx);
            return;
        };
        // Step 4.2.1-4.2.2: error type, non-ok status or 206 rejects.
        if response.Type() == DOMResponseType::Error
            || response.Status() == 206
            || !(200..=299).contains(&response.Status())
        {
            self.shared.fail(cx);
            return;
        }
        // Step 4.2.3: `Vary: *` rejects.
        if response_has_vary_star(response.Headers(cx).get_headers_list()) {
            self.shared.fail(cx);
            return;
        }
        let metadata = RefCell::new(Some(response_metadata(cx, &response)));
        let shared = self.shared.clone();
        let index = self.index;
        let finish = Rc::new(move |cx: &mut JSContext, body: Vec<u8>| {
            let Some(metadata) = metadata.borrow_mut().take() else {
                return;
            };
            shared.finish(cx, index, metadata.into_response(body));
        });

        match response.body() {
            None => finish(cx, Vec::new()),
            Some(stream) => {
                if response.is_unusable() || stream.is_errored() {
                    self.shared.fail(cx);
                    return;
                }
                let reader = match stream.acquire_default_reader(cx) {
                    Ok(reader) => reader,
                    Err(_) => {
                        self.shared.fail(cx);
                        return;
                    },
                };
                let failure_shared = self.shared.clone();
                reader.read_all_bytes(
                    cx,
                    Rc::new(move |cx: &mut JSContext, bytes: &[u8]| {
                        finish(cx, bytes.to_vec());
                    }),
                    Rc::new(move |cx: &mut JSContext, _error| {
                        failure_shared.fail(cx);
                    }),
                );
            },
        }
    }
}

/// Rejection steps of one `Cache.addAll` fetch promise.
#[derive(JSTraceable, MallocSizeOf)]
struct AddAllFetchErrorHandler {
    #[conditional_malloc_size_of]
    shared: Rc<AddAllShared>,
}

impl Callback for AddAllFetchErrorHandler {
    fn callback(&self, cx: &mut CurrentRealm, _value: HandleValue) {
        self.shared.fail(cx);
    }
}

/// Convert a Cache API request argument into the storage-side request shape.
pub(crate) fn cache_request_from_info(
    cx: &mut JSContext,
    global: &GlobalScope,
    info: RequestInfo,
) -> Result<CacheRequest, Error> {
    let request = match info {
        RequestInfo::Request(request) => request,
        RequestInfo::USVString(url) => Request::constructor(
            cx,
            global,
            None,
            RequestInfo::USVString(url),
            &RequestInit::empty(),
        )?,
    };
    Ok(cache_request_from_request(&request))
}

fn cache_request_from_request(request: &Request) -> CacheRequest {
    let net_request = request.request();
    CacheRequest {
        url: net_request.url(),
        method: net_request.method.as_str().to_string(),
        headers: headers_to_vec(&net_request.headers),
    }
}

fn request_from_cache(
    cx: &mut JSContext,
    global: &GlobalScope,
    cached: &CacheRequest,
) -> Result<DomRoot<Request>, Error> {
    // Stored requests are always the request that wrote the entry. Rebuild it
    // through the constructor so URL parsing/base rules are the standard ones.
    let mut request_init = RequestInit::empty();
    request_init.method = Some(ByteString::new(cached.method.as_bytes().to_vec()));
    let info = RequestInfo::USVString(USVString(cached.url.as_str().to_string()));
    let request = Request::constructor(cx, global, None, info, &request_init)?;
    let headers = request.Headers(cx);
    headers.set_headers(headers_from_vec(&cached.headers));
    // <https://w3c.github.io/ServiceWorker/#cache-keys> Step 5.4.2.1:
    // a new Headers object whose guard is "immutable".
    headers.set_guard(Guard::Immutable);
    Ok(request)
}

fn storage_query_options(options: &CacheQueryOptions) -> StorageQueryOptions {
    StorageQueryOptions {
        ignore_search: options.ignoreSearch,
        ignore_method: options.ignoreMethod,
        ignore_vary: options.ignoreVary,
    }
}

fn response_metadata(cx: &mut JSContext, response: &Response) -> ResponseMetadata {
    let url = response.Url();
    ResponseMetadata {
        status: response.Status(),
        status_text: response.StatusText().to_vec(),
        headers: headers_to_vec(&response.Headers(cx).get_headers_list()),
        url: if url.0.is_empty() {
            None
        } else {
            ServoUrl::parse(&url.0).ok()
        },
        response_type: match response.Type() {
            DOMResponseType::Default => CacheResponseType::Default,
            DOMResponseType::Basic => CacheResponseType::Basic,
            DOMResponseType::Cors => CacheResponseType::Cors,
            DOMResponseType::Error => CacheResponseType::Error,
            DOMResponseType::Opaque => CacheResponseType::Opaque,
            DOMResponseType::Opaqueredirect => CacheResponseType::OpaqueRedirect,
        },
        redirected: response.Redirected(),
    }
}

/// <https://fetch.spec.whatwg.org/#http-vary>
fn response_has_vary_star(headers: HyperHeaders) -> bool {
    headers.get_all(http::header::VARY).iter().any(|value| {
        value
            .to_str()
            .map(|value| value.split(',').any(|field| field.trim() == "*"))
            .unwrap_or(false)
    })
}

fn headers_to_vec(headers: &HyperHeaders) -> Vec<(Vec<u8>, Vec<u8>)> {
    headers
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect()
}

fn headers_from_vec(headers: &[(Vec<u8>, Vec<u8>)]) -> HyperHeaders {
    let mut map = HyperHeaders::new();
    for (name, value) in headers {
        let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name),
            http::HeaderValue::from_bytes(value),
        ) else {
            continue;
        };
        map.append(name, value);
    }
    map
}
