/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::RefCell;
use std::collections::VecDeque;

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use script_bindings::root::DomRoot;
use servo_base::generic_channel::{GenericCallback, GenericSend};
use servo_url::ImmutableOrigin;
use storage_traits::cache_storage::{
    CacheStorageQueryOptions, CacheStorageThreadMessage, CacheStorageThreadResponse,
};
use storage_traits::client_storage::{StorageIdentifier, StorageProxyMap, StorageType};

use crate::dom::bindings::codegen::Bindings::CacheStorageBinding::CacheStorageMethods;
use crate::dom::bindings::codegen::Bindings::RequestBinding::RequestInfo;
use crate::dom::bindings::codegen::GenericBindings::CacheStorageBinding::MultiCacheQueryOptions;
use crate::dom::bindings::error::Error;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;
use crate::dom::serviceworker::cache::{Cache, cache_request_from_info, response_from_cache};
use crate::dom::{Promise, RootedPromise, TracedPromise};

/// <https://w3c.github.io/ServiceWorker/#cachestorage>
#[dom_struct]
pub(crate) struct CacheStorage {
    reflector_: Reflector,

    #[no_trace]
    #[ignore_malloc_size_of = "GenericCallback"]
    callback: RefCell<Option<GenericCallback<CacheStorageThreadResponse>>>,

    // Dequeue of pending promises for backend operations.
    pending_promises: RefCell<VecDeque<TracedPromise>>,
}

impl CacheStorage {
    fn new_inherited() -> CacheStorage {
        CacheStorage {
            reflector_: Reflector::new(),
            callback: Default::default(),
            pending_promises: Default::default(),
        }
    }

    pub(crate) fn new(cx: &mut JSContext, global: &GlobalScope) -> DomRoot<CacheStorage> {
        reflect_dom_object(cx, Box::new(CacheStorage::new_inherited()), global)
    }

    /// Setup the callback to the backend service, if this hasn't been done already.
    fn get_or_setup_callback(&self) -> GenericCallback<CacheStorageThreadResponse> {
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
                    error!("Error in CacheStorage callback {:?}.", err);
                    None
                },
            };
            task_source.queue(task!(set_request_result_to_database: move |cx| {
                let cache_storage = response_listener.root();
                cache_storage.handle_response(cx, response)
            }));
        })
        .expect("Could not create CacheStorage callback");

        *self.callback.borrow_mut() = Some(callback.clone());

        callback
    }

    fn handle_response(&self, cx: &mut JSContext, response: Option<CacheStorageThreadResponse>) {
        let response = match response {
            Some(response) => response,
            None => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    error!("No pending promise for CacheStorage response.");
                    return;
                };
                promise.reject_error(
                    cx,
                    Error::Operation(Some("No response from CacheStorage backend.".to_string())),
                );
                return;
            },
        };
        match response {
            // <https://w3c.github.io/ServiceWorker/#cache-storage-has>
            // the steps resolving the promise with the result.
            // Note: spec forgets to queue a task see <https://github.com/w3c/ServiceWorker/issues/1831>
            CacheStorageThreadResponse::HasCacheResult(result) => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    debug_assert!(false, "No pending promise for HasCacheResult response.");
                    return;
                };
                let Ok(has_cache) = result else {
                    promise.reject_error(
                        cx,
                        Error::Operation(Some(
                            result
                                .err()
                                .unwrap_or_else(|| "HasCacheResult error".to_string()),
                        )),
                    );
                    return;
                };
                // Step 2.1:For each key → value of the relevant name to cache map:
                // Step 2.1.1: If cacheName matches key, resolve promise with true and abort these steps.
                // Step 2.2: Resolve promise with false.
                // Note: promise resolved with the result obtained in parallel.
                promise.resolve_native(cx, &has_cache);
            },
            // <https://w3c.github.io/ServiceWorker/#cache-storage-open>
            // the steps resolving the promise with the result.
            CacheStorageThreadResponse::OpenCacheResult { result, cache_name } => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    debug_assert!(false, "No pending promise for OpenCacheResult response.");
                    return;
                };
                if result.is_err() {
                    promise.reject_error(
                        cx,
                        Error::Operation(Some(
                            result
                                .err()
                                .unwrap_or_else(|| "OpenCacheResult error".to_string()),
                        )),
                    );
                    return;
                };
                // Resolve promise with a new Cache object that represents value.
                let cache = Cache::new(cx, &self.global(), DOMString::from(cache_name));
                promise.resolve_native(cx, &cache);
            },
            // <https://w3c.github.io/ServiceWorker/#dom-cachestorage-delete>
            CacheStorageThreadResponse::DeleteCacheResult(result) => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    debug_assert!(false, "No pending promise for DeleteCacheResult response.");
                    return;
                };
                let Ok(deleted) = result else {
                    promise.reject_error(
                        cx,
                        Error::Operation(Some(
                            result
                                .err()
                                .unwrap_or_else(|| "DeleteCacheResult error".to_string()),
                        )),
                    );
                    return;
                };
                promise.resolve_native(cx, &deleted);
            },
            // <https://w3c.github.io/ServiceWorker/#cache-storage-keys>
            CacheStorageThreadResponse::StorageKeysResult(result) => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    debug_assert!(false, "No pending promise for StorageKeysResult response.");
                    return;
                };
                match result {
                    Ok(keys) => {
                        let keys: Vec<DOMString> = keys.into_iter().map(DOMString::from).collect();
                        promise.resolve_native(cx, &keys);
                    },
                    Err(error) => promise.reject_error(cx, Error::Operation(Some(error))),
                }
            },
            // <https://w3c.github.io/ServiceWorker/#cache-storage-match>
            CacheStorageThreadResponse::StorageMatchResult(result) => {
                let Some(promise) = self
                    .pending_promises
                    .borrow_mut()
                    .pop_front()
                    .map(|promise| promise.root(cx))
                else {
                    debug_assert!(false, "No pending promise for StorageMatchResult response.");
                    return;
                };
                match result {
                    Ok(Some(response)) => match response_from_cache(cx, &self.global(), response) {
                        Ok(response) => promise.resolve_native(cx, &response),
                        Err(error) => promise.reject_error(cx, error),
                    },
                    Ok(None) => promise.resolve_native(cx, &()),
                    Err(error) => promise.reject_error(cx, Error::Operation(Some(error))),
                }
            },
            CacheStorageThreadResponse::KeysResult(_) => debug_assert!(
                false,
                "Unexpected KeysResult response in CacheStorage handle_response."
            ),
            CacheStorageThreadResponse::MatchResult(_) => debug_assert!(
                false,
                "Unexpected MatchResult response in CacheStorage handle_response."
            ),
            CacheStorageThreadResponse::MatchAllResult(_) => debug_assert!(
                false,
                "Unexpected MatchAllResult response in CacheStorage handle_response."
            ),
            CacheStorageThreadResponse::PutResult(_) => debug_assert!(
                false,
                "Unexpected PutResult response in CacheStorage handle_response."
            ),
            CacheStorageThreadResponse::DeleteEntriesResult(_) => debug_assert!(
                false,
                "Unexpected DeleteEntriesResult response in CacheStorage handle_response."
            ),
        }
    }
}

/// <https://w3c.github.io/ServiceWorker/#relevant-name-to-cache-map>
fn relevant_name_to_cache_map(
    global: &GlobalScope,
    origin: ImmutableOrigin,
) -> Result<StorageProxyMap, Error> {
    // The relevant name to cache map for a CacheStorage object
    // is the name to cache map associated with the result of
    // running obtain a local storage bottle map with
    // the object’s relevant settings object and "caches".
    let handle = global.storage_threads().client_storage_handle();
    let message = handle
        .obtain_a_storage_bottle_map(
            StorageType::Local,
            global.webview_id(),
            StorageIdentifier::Caches,
            origin,
        )
        .recv();
    let Ok(response) = message else {
        return Err(Error::Operation(Some(
            "Could not obtain a local storage bottle map.".to_string(),
        )));
    };
    let Ok(proxy_map) = response else {
        return Err(Error::Operation(Some(
            "Could not obtain a local storage bottle map.".to_string(),
        )));
    };
    Ok(proxy_map)
}

/// The generated trait passes no `cx` to `match`/`keys`; acquire the script
/// thread's context locally (same shape as `ImageDecoder::thread_cx`).
#[allow(unsafe_code)]
fn thread_cx() -> JSContext {
    unsafe { JSContext::get_from_thread() }.expect("no active JS context")
}

impl CacheStorageMethods<crate::DomTypeHolder> for CacheStorage {
    /// <https://w3c.github.io/ServiceWorker/#cache-storage-has>
    fn Has(&self, cx: &mut JSContext, cache_name: DOMString) -> RootedPromise {
        let global = self.global();

        // Step 1: Let promise be a new promise.
        let promise = Promise::new(cx, &global);

        // Step 2: Run the following substeps in parallel:
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        let proxy_map = match relevant_name_to_cache_map(&global, origin.clone()) {
            Ok(proxy_map) => proxy_map,
            Err(err) => {
                promise.reject_error(cx, err);
                return promise;
            },
        };
        if global
            .storage_threads()
            .send(CacheStorageThreadMessage::HasCache {
                cache_name: cache_name.to_string(),
                callback,
                proxy: proxy_map,
                origin,
            })
            .is_err()
        {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return promise;
        }

        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());

        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#dom-cachestorage-open>
    fn Open(&self, cx: &mut JSContext, cache_name: DOMString) -> RootedPromise {
        // Step 1: Let promise be a new promise.
        let global = self.global();
        let promise = Promise::new(cx, &global);

        // Step 2: Run the following substeps in parallel:
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        let proxy_map = match relevant_name_to_cache_map(&global, origin.clone()) {
            Ok(proxy_map) => proxy_map,
            Err(err) => {
                promise.reject_error(cx, err);
                return promise;
            },
        };
        if global
            .storage_threads()
            .send(CacheStorageThreadMessage::OpenCache {
                cache_name: cache_name.to_string(),
                callback,
                proxy: proxy_map,
                origin,
            })
            .is_err()
        {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return promise;
        }

        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());

        // Step 3: Return promise.
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#dom-cachestorage-delete>
    fn Delete(&self, cx: &mut JSContext, cache_name: DOMString) -> RootedPromise {
        // Step 1: Let promise be the result of running the algorithm specified in has(cacheName) method with cacheName.
        // Step 2: Return the result of reacting to promise with a fulfillment handler that,
        // when called with argument cacheExists, performs the following substeps:
        // Step 2.1: If cacheExists is false, then
        // Step 2.1.1: Return false.
        // Note: we skip the promise, and will run the equivalent steps directly in the backend.

        // Step 2.2: Let cacheJobPromise be a new promise.
        let global = self.global();
        let promise = Promise::new(cx, &global);

        // Step 3: Run the following substeps in parallel:
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        let proxy_map = match relevant_name_to_cache_map(&global, origin.clone()) {
            Ok(proxy_map) => proxy_map,
            Err(err) => {
                promise.reject_error(cx, err);
                return promise;
            },
        };
        if global
            .storage_threads()
            .send(CacheStorageThreadMessage::DeleteCache {
                cache_name: cache_name.to_string(),
                callback,
                proxy: proxy_map,
                origin,
            })
            .is_err()
        {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return promise;
        }

        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());

        // Step 4: Return cacheJobPromise.
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-match>
    fn Match(&self, request: RequestInfo, options: &MultiCacheQueryOptions) -> RootedPromise {
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
        let options = CacheStorageQueryOptions {
            ignore_search: options.parent.ignoreSearch,
            ignore_method: options.parent.ignoreMethod,
            ignore_vary: options.parent.ignoreVary,
            cache_name: options.cacheName.as_ref().map(|name| name.to_string()),
        };
        // `Cache.match` performs this check; a non-GET query with
        // ignoreMethod=false resolves to undefined.
        if request.method != "GET" && !options.ignore_method {
            promise.resolve_native(cx, &());
            return promise;
        }
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        if global
            .storage_threads()
            .send(CacheStorageThreadMessage::StorageMatch {
                request,
                options,
                callback,
                origin,
            })
            .is_err()
        {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return promise;
        }
        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());
        promise
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-keys>
    fn Keys(&self) -> RootedPromise {
        let cx = &mut thread_cx();
        let global = self.global();
        let promise = Promise::new(cx, &global);
        let callback = self.get_or_setup_callback();
        let origin = global.origin().immutable().clone();
        if global
            .storage_threads()
            .send(CacheStorageThreadMessage::StorageKeys { callback, origin })
            .is_err()
        {
            promise.reject_error(
                cx,
                Error::Operation(Some("Could not run the parallel steps.".to_string())),
            );
            return promise;
        }
        self.pending_promises
            .borrow_mut()
            .push_back(promise.to_traced());
        promise
    }
}
