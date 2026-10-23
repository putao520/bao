/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::thread;

use log::error;
use servo_base::generic_channel::{self, GenericReceiver, GenericSender};
use servo_url::ImmutableOrigin;
use storage_traits::cache_storage::{
    CacheBatchError, CacheEntry, CacheQueryOptions, CacheRequest, CacheResponse, CacheStorageError,
    CacheStorageQueryOptions, CacheStorageThreadHandle, CacheStorageThreadMessage,
    CacheStorageThreadResponse,
};
use storage_traits::client_storage::StorageProxyMap;

trait CacheStorageEngine {
    type Error: Debug;

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-has>
    fn has_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
    ) -> Result<bool, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-open>
    fn open_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: String,
        proxy_map: &StorageProxyMap,
    ) -> Result<(), CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#cache-keys>
    fn keys(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: Option<&CacheRequest>,
        options: &CacheQueryOptions,
    ) -> Result<Vec<CacheRequest>, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#cache-matchall>
    fn match_all(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: Option<&CacheRequest>,
        options: &CacheQueryOptions,
    ) -> Result<Vec<CacheResponse>, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#batch-cache-operations>
    fn put(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        entries: Vec<CacheEntry>,
    ) -> Result<(), CacheBatchError>;

    /// <https://w3c.github.io/ServiceWorker/#cache-delete>
    fn delete(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: &CacheRequest,
        options: &CacheQueryOptions,
    ) -> Result<bool, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-keys>
    fn storage_keys(
        &mut self,
        origin: ImmutableOrigin,
    ) -> Result<Vec<String>, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-match>
    fn storage_match(
        &mut self,
        origin: ImmutableOrigin,
        request: &CacheRequest,
        options: &CacheStorageQueryOptions,
    ) -> Result<Option<CacheResponse>, CacheStorageError<Self::Error>>;

    /// <https://w3c.github.io/ServiceWorker/#dom-cachestorage-delete>
    fn delete_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        proxy_map: &StorageProxyMap,
    ) -> Result<bool, CacheStorageError<Self::Error>>;
}

/// <https://w3c.github.io/ServiceWorker/#dfn-request-response-list>
#[derive(Default)]
pub struct RequestResponseList {
    /// The [=request response list=] entries, in insertion order.
    pub entries: Vec<CacheEntry>,
}

impl RequestResponseList {
    fn matching_indices(&self, query: &CacheRequest, options: &CacheQueryOptions) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                request_matches(query, &entry.request, Some(&entry.response), options)
            })
            .map(|(index, _)| index)
            .collect()
    }
}

pub struct MemCacheStorageEngine {
    /// <https://w3c.github.io/ServiceWorker/#dfn-name-to-cache-map>
    ///
    /// The name to cache map is an ordered map: `CacheStorage.keys()` must
    /// return names in insertion order, so each origin keeps a `Vec` of
    /// `(name, list)` pairs.
    name_to_cache_map: HashMap<ImmutableOrigin, Vec<(String, RequestResponseList)>>,
}

impl MemCacheStorageEngine {
    fn cache(&self, origin: &ImmutableOrigin, cache_name: &str) -> Option<&RequestResponseList> {
        self.name_to_cache_map
            .get(origin)?
            .iter()
            .find(|(name, _)| name == cache_name)
            .map(|(_, cache)| cache)
    }

    /// The relevant request response list for a cache. The DOM `Cache` objects
    /// survive `CacheStorage.delete()` (spec notes that existing DOM objects
    /// remain functional), so an operation on a deleted cache recreates an
    /// empty list instead of failing.
    fn ensure_cache(
        &mut self,
        origin: &ImmutableOrigin,
        cache_name: &str,
    ) -> &mut RequestResponseList {
        let caches = self.name_to_cache_map.entry(origin.clone()).or_default();
        let index = caches
            .iter()
            .position(|(name, _)| name == cache_name)
            .unwrap_or_else(|| {
                caches.push((cache_name.to_string(), RequestResponseList::default()));
                caches.len() - 1
            });
        &mut caches[index].1
    }
}

impl CacheStorageEngine for MemCacheStorageEngine {
    type Error = ();

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-has>
    /// The parallel steps.
    fn has_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
    ) -> Result<bool, CacheStorageError<Self::Error>> {
        // Step 2.1: For each key → value of the relevant name to cache map:
        // Step 2.1.1: If cacheName matches key, resolve promise with true and abort these steps.
        // Step 2.2: Resolve promise with false.
        // Note: promise resolved in the callback in CacheStorage.
        Ok(self.cache(&origin, cache_name).is_some())
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-open>
    /// The parallel steps.
    fn open_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: String,
        proxy_map: &StorageProxyMap,
    ) -> Result<(), CacheStorageError<Self::Error>> {
        // Step 2.1: For each key → value of the relevant name to cache map:
        // Step 2.1.1: If cacheName matches key, then:
        // Resolve promise with a new Cache object that represents value.
        // Note: promise resolved in script.
        if self.cache(&origin, &cache_name).is_some() {
            // Step 2.1.2: Abort these steps.
            return Ok(());
        }

        // Step 2.2: Let cache be a new request response list.
        let cache = RequestResponseList::default();

        // Step 2.3: Set the relevant name to cache map[cacheName] to cache.
        // If this cache write operation failed due to exceeding the granted quota limit,
        // reject promise with a QuotaExceededError and abort these steps.
        // Note: there are no quota checks storage side at this point.
        let Ok(response) = proxy_map
            .handle
            .create_database(proxy_map.bottle_id, cache_name.clone())
            .recv()
        else {
            return Err(CacheStorageError::Internal(()));
        };
        if response.is_err() {
            return Err(CacheStorageError::Internal(()));
        }
        self.name_to_cache_map
            .entry(origin)
            .or_default()
            .push((cache_name, cache));

        // Step 2.4: Resolve promise with a new Cache object that represents cache.
        // Note: promise resolved in script.
        Ok(())
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-keys>
    /// The parallel steps.
    fn keys(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: Option<&CacheRequest>,
        options: &CacheQueryOptions,
    ) -> Result<Vec<CacheRequest>, CacheStorageError<Self::Error>> {
        // Step 5.1: Let requests be an empty list.
        // Step 5.2: If the optional argument request is omitted, then:
        // Step 5.2.1: For each requestResponse of the relevant request response list:
        // Step 5.2.2: Add requestResponse's request to requests.
        // Step 5.3: Else:
        // Step 5.3.1: Let requestResponses be the result of running Query Cache
        // with r and options.
        // Step 5.3.2: For each requestResponse of requestResponses:
        // Step 5.3.3: Add requestResponse's request to requests.
        let cache = self.ensure_cache(&origin, cache_name);
        let entries: Vec<&CacheEntry> = match request {
            None => cache.entries.iter().collect(),
            Some(query) => cache
                .entries
                .iter()
                .filter(|entry| {
                    request_matches(query, &entry.request, Some(&entry.response), options)
                })
                .collect(),
        };
        Ok(entries
            .into_iter()
            .map(|entry| entry.request.clone())
            .collect())
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-matchall>
    /// The parallel steps.
    fn match_all(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: Option<&CacheRequest>,
        options: &CacheQueryOptions,
    ) -> Result<Vec<CacheResponse>, CacheStorageError<Self::Error>> {
        let cache = self.ensure_cache(&origin, cache_name);
        let responses: Vec<CacheResponse> = match request {
            None => cache
                .entries
                .iter()
                .map(|entry| entry.response.clone())
                .collect(),
            Some(query) => cache
                .entries
                .iter()
                .filter(|entry| {
                    request_matches(query, &entry.request, Some(&entry.response), options)
                })
                .map(|entry| entry.response.clone())
                .collect(),
        };
        Ok(responses)
    }

    /// <https://w3c.github.io/ServiceWorker/#batch-cache-operations>
    fn put(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        entries: Vec<CacheEntry>,
    ) -> Result<(), CacheBatchError> {
        // Step 2: Let backupCache be a new request response list that is a copy of cache.
        let cache = self.ensure_cache(&origin, cache_name);
        let backup = cache.entries.clone();

        // Step 3: Let addedItems be an empty list.
        let mut added_items: Vec<CacheEntry> = Vec::new();

        let default_options = CacheQueryOptions::default();
        for entry in entries {
            // Step 4.2.3: If the result of running Query Cache with
            // operation's request, operation's options, and addedItems is not
            // empty, throw an InvalidStateError.
            if added_items.iter().any(|added| {
                request_matches(
                    &entry.request,
                    &added.request,
                    Some(&added.response),
                    &default_options,
                )
            }) {
                // Step 5: roll back any changes made during the batch job.
                cache.entries = backup;
                return Err(CacheBatchError::InvalidState);
            }

            // Step 4.2.5: Let requestResponses be the result of running Query Cache
            // with operation's request.
            let stale_indices = cache.matching_indices(&entry.request, &default_options);
            // Step 4.2.6: For each requestResponse of requestResponses:
            // Step 4.2.7: Remove the item whose value matches requestResponse from cache.
            for index in stale_indices.into_iter().rev() {
                cache.entries.remove(index);
            }
            // Step 4.2.8: Append operation's request/operation's response to cache.
            cache.entries.push(entry.clone());
            // Step 4.2.10: Append operation's request/operation's response to addedItems.
            added_items.push(entry);
        }

        Ok(())
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-delete>
    /// The "running the algorithm specified in has" part, and the parallel steps.
    fn delete(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        request: &CacheRequest,
        options: &CacheQueryOptions,
    ) -> Result<bool, CacheStorageError<Self::Error>> {
        // Step 2.3: Run the following substeps in parallel:
        // Step 2.3.1: Let requestResponses be the result of running Query Cache
        // with operation's request and operation's options.
        // Step 2.3.2: For each requestResponse in requestResponses:
        // Step 2.3.3: Remove the item whose value matches requestResponse from cache.
        let cache = self.ensure_cache(&origin, cache_name);
        let indices = cache.matching_indices(request, options);
        let deleted = !indices.is_empty();
        for index in indices.into_iter().rev() {
            cache.entries.remove(index);
        }
        Ok(deleted)
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-keys>
    fn storage_keys(
        &mut self,
        origin: ImmutableOrigin,
    ) -> Result<Vec<String>, CacheStorageError<Self::Error>> {
        Ok(self
            .name_to_cache_map
            .get(&origin)
            .map(|caches| caches.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default())
    }

    /// <https://w3c.github.io/ServiceWorker/#cache-storage-match>
    fn storage_match(
        &mut self,
        origin: ImmutableOrigin,
        request: &CacheRequest,
        options: &CacheStorageQueryOptions,
    ) -> Result<Option<CacheResponse>, CacheStorageError<Self::Error>> {
        let cache_options = CacheQueryOptions {
            ignore_search: options.ignore_search,
            ignore_method: options.ignore_method,
            ignore_vary: options.ignore_vary,
        };
        let Some(caches) = self.name_to_cache_map.get(&origin) else {
            return Ok(None);
        };
        // Step 1: If options["cacheName"] exists, then: for each cacheName →
        // cache of the relevant name to cache map: if it matches, resolve with
        // Cache.match(request, options) in that cache. Otherwise undefined.
        if let Some(cache_name) = &options.cache_name {
            let Some((_, cache)) = caches.iter().find(|(name, _)| name == cache_name) else {
                return Ok(None);
            };
            return Ok(cache
                .entries
                .iter()
                .find(|entry| {
                    request_matches(
                        request,
                        &entry.request,
                        Some(&entry.response),
                        &cache_options,
                    )
                })
                .map(|entry| entry.response.clone()));
        }
        // Step 2: Else, chain Cache.match(request, options) over every cache in
        // insertion order and resolve with the first non-undefined response.
        for (_, cache) in caches {
            if let Some(entry) = cache.entries.iter().find(|entry| {
                request_matches(
                    request,
                    &entry.request,
                    Some(&entry.response),
                    &cache_options,
                )
            }) {
                return Ok(Some(entry.response.clone()));
            }
        }
        Ok(None)
    }

    /// <https://w3c.github.io/ServiceWorker/#dom-cachestorage-delete>
    /// The "running the algorithm specified in has" part, and the parallel steps.
    fn delete_cache(
        &mut self,
        origin: ImmutableOrigin,
        cache_name: &str,
        proxy: &StorageProxyMap,
    ) -> Result<bool, CacheStorageError<Self::Error>> {
        // Step 1: Let promise be the result of running the algorithm specified in has(cacheName) method with cacheName.
        // Step 2: Return the result of reacting to promise with a fulfillment handler that,
        // when called with argument cacheExists,
        // performs the following substeps:
        // Note: skipping the promise here.

        // Step 2.3: Run the following substeps in parallel:
        // Step 2.3.1: Remove the relevant name to cache map[cacheName].
        let has = self
            .name_to_cache_map
            .get_mut(&origin)
            .and_then(|caches| {
                let index = caches.iter().position(|(name, _)| name == cache_name)?;
                caches.remove(index);
                Some(())
            })
            .is_some();

        if has {
            let Ok(response) = proxy
                .handle
                .delete_database(proxy.bottle_id, cache_name.to_string())
                .recv()
            else {
                return Err(CacheStorageError::Internal(()));
            };
            if response.is_err() {
                return Err(CacheStorageError::Internal(()));
            }
        }

        // Step 2.3.2: Resolve promise with true.
        // Note: promise resolved in script.
        Ok(has)
    }
}

/// <https://w3c.github.io/ServiceWorker/#request-matches-cached-item-algorithm>
///
/// The algorithm is asymmetric: the *cached response's* `Vary` header decides
/// which request headers are compared.
fn request_matches(
    request_query: &CacheRequest,
    request: &CacheRequest,
    response: Option<&CacheResponse>,
    options: &CacheQueryOptions,
) -> bool {
    // Step 1: If options["ignoreMethod"] is false and request's method is not
    // `GET`, return false.
    if !options.ignore_method && request.method != "GET" {
        return false;
    }

    // Steps 2-5: compare the query URL and cached URL, excluding fragments and
    // optionally clearing the query.
    let mut query_url = request_query.url.as_url().clone();
    let mut cached_url = request.url.as_url().clone();
    query_url.set_fragment(None);
    cached_url.set_fragment(None);
    if options.ignore_search {
        query_url.set_query(None);
        cached_url.set_query(None);
    }
    if query_url != cached_url {
        return false;
    }

    // Step 6: If response is null, options["ignoreVary"] is true, or response's
    // header list does not contain `Vary`, then return true.
    let Some(response) = response else {
        return true;
    };
    if options.ignore_vary {
        return true;
    }
    let Some(vary_values) = combined_header_value(&response.headers, b"vary") else {
        return true;
    };

    // Step 7: For each Vary field-value, compare the combined value of that
    // header in the query and cached requests.
    for field_value in String::from_utf8_lossy(&vary_values).split(',') {
        let field_value = field_value.trim();
        if field_value.is_empty() {
            continue;
        }
        // "If fieldValue matches `*` ... return false."
        if field_value == "*" {
            return false;
        }
        let name = field_value.as_bytes();
        if combined_header_value(&request.headers, name)
            != combined_header_value(&request_query.headers, name)
        {
            return false;
        }
    }
    true
}

/// <https://fetch.spec.whatwg.org/#concept-header-list-combine>
///
/// The combined value of all occurrences of `name` in `headers`, or `None`
/// when the header is absent.
fn combined_header_value(headers: &[(Vec<u8>, Vec<u8>)], name: &[u8]) -> Option<Vec<u8>> {
    let mut combined: Option<Vec<u8>> = None;
    for (header_name, value) in headers {
        if !header_name.eq_ignore_ascii_case(name) {
            continue;
        }
        match combined.as_mut() {
            Some(existing) => {
                existing.extend_from_slice(b", ");
                existing.extend_from_slice(value);
            },
            None => combined = Some(value.clone()),
        }
    }
    combined
}

pub trait CacheStorageThreadFactory {
    fn new(config_dir: Option<PathBuf>, temporary_storage: bool) -> Self;
}

impl CacheStorageThreadFactory for CacheStorageThreadHandle {
    fn new(config_dir: Option<PathBuf>, temporary_storage: bool) -> CacheStorageThreadHandle {
        let (generic_sender, generic_receiver) = generic_channel::channel().unwrap();
        let mut temp_dir: Option<tempfile::TempDir> = None;
        let base_dir = config_dir
            .unwrap_or_else(|| {
                let tmp_dir = tempfile::tempdir().unwrap();
                let path = tmp_dir.path().to_path_buf();
                temp_dir = Some(tmp_dir);
                path
            })
            .join("cachestorage");
        let storage_dir = if temporary_storage {
            let unique_id = uuid::Uuid::new_v4().to_string();
            base_dir.join("temporary").join(unique_id)
        } else {
            base_dir.join("default_v1")
        };
        std::fs::create_dir_all(&storage_dir)
            .expect("Failed to create CacheStorage storage directory");
        let sender_clone = generic_sender.clone();
        thread::Builder::new()
            .name("CacheStorageThread".to_owned())
            .spawn(move || {
                // Keep temp_dir alive while the thread runs.
                let _ = temp_dir;
                let engine = MemCacheStorageEngine {
                    name_to_cache_map: Default::default(),
                };
                let mut cache_storage_thread =
                    CacheStorageThread::new(sender_clone, generic_receiver, engine);
                cache_storage_thread.start();
            })
            .expect("Thread spawning failed");

        CacheStorageThreadHandle::new(generic_sender)
    }
}

struct CacheStorageThread<E: CacheStorageEngine> {
    receiver: GenericReceiver<CacheStorageThreadMessage>,
    // Note: a sender to self might be required later for the storage engine.
    _sender: GenericSender<CacheStorageThreadMessage>,
    engine: E,
}

impl<E> CacheStorageThread<E>
where
    E: CacheStorageEngine,
{
    pub fn new(
        _sender: GenericSender<CacheStorageThreadMessage>,
        receiver: GenericReceiver<CacheStorageThreadMessage>,
        engine: E,
    ) -> CacheStorageThread<E> {
        CacheStorageThread {
            _sender,
            receiver,
            engine,
        }
    }

    pub fn start(&mut self) {
        while let Ok(message) = self.receiver.recv() {
            match message {
                CacheStorageThreadMessage::HasCache {
                    cache_name,
                    callback,
                    proxy: _,
                    origin,
                } => {
                    let result = self.engine.has_cache(origin.clone(), &cache_name);
                    if callback
                        .send(CacheStorageThreadResponse::HasCacheResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for HasCache message.");
                    }
                },
                CacheStorageThreadMessage::OpenCache {
                    cache_name,
                    callback,
                    proxy,
                    origin,
                } => {
                    let result = self
                        .engine
                        .open_cache(origin.clone(), cache_name.clone(), &proxy);
                    if callback
                        .send(CacheStorageThreadResponse::OpenCacheResult {
                            result: result.map_err(|e| format!("{:?}", e)),
                            cache_name,
                        })
                        .is_err()
                    {
                        error!("Failed to send response to script for OpenCache message.");
                    }
                },
                CacheStorageThreadMessage::Keys {
                    cache_name,
                    request,
                    options,
                    callback,
                    origin,
                } => {
                    let result =
                        self.engine
                            .keys(origin.clone(), &cache_name, request.as_ref(), &options);
                    if callback
                        .send(CacheStorageThreadResponse::KeysResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for Keys message.");
                    }
                },
                CacheStorageThreadMessage::Put {
                    cache_name,
                    entries,
                    callback,
                    origin,
                } => {
                    let result = self.engine.put(origin.clone(), &cache_name, entries);
                    if callback
                        .send(CacheStorageThreadResponse::PutResult(result))
                        .is_err()
                    {
                        error!("Failed to send response to script for Put message.");
                    }
                },
                CacheStorageThreadMessage::Match {
                    cache_name,
                    request,
                    options,
                    callback,
                    origin,
                } => {
                    // `Cache.match` is `matchAll` + first element.
                    let result = self.engine.match_all(
                        origin.clone(),
                        &cache_name,
                        request.as_ref(),
                        &options,
                    );
                    let response = match result {
                        Ok(mut responses) => Ok(responses.drain(..).next()),
                        Err(e) => Err(format!("{:?}", e)),
                    };
                    if callback
                        .send(CacheStorageThreadResponse::MatchResult(response))
                        .is_err()
                    {
                        error!("Failed to send response to script for Match message.");
                    }
                },
                CacheStorageThreadMessage::MatchAll {
                    cache_name,
                    request,
                    options,
                    callback,
                    origin,
                } => {
                    let result = self.engine.match_all(
                        origin.clone(),
                        &cache_name,
                        request.as_ref(),
                        &options,
                    );
                    if callback
                        .send(CacheStorageThreadResponse::MatchAllResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for MatchAll message.");
                    }
                },
                CacheStorageThreadMessage::Delete {
                    cache_name,
                    request,
                    options,
                    callback,
                    origin,
                } => {
                    let result =
                        self.engine
                            .delete(origin.clone(), &cache_name, &request, &options);
                    if callback
                        .send(CacheStorageThreadResponse::DeleteEntriesResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for Delete message.");
                    }
                },
                CacheStorageThreadMessage::StorageKeys { callback, origin } => {
                    let result = self.engine.storage_keys(origin.clone());
                    if callback
                        .send(CacheStorageThreadResponse::StorageKeysResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for StorageKeys message.");
                    }
                },
                CacheStorageThreadMessage::StorageMatch {
                    request,
                    options,
                    callback,
                    origin,
                } => {
                    let result = self
                        .engine
                        .storage_match(origin.clone(), &request, &options);
                    if callback
                        .send(CacheStorageThreadResponse::StorageMatchResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for StorageMatch message.");
                    }
                },
                CacheStorageThreadMessage::DeleteCache {
                    cache_name,
                    callback,
                    proxy,
                    origin,
                } => {
                    let result = self
                        .engine
                        .delete_cache(origin.clone(), &cache_name, &proxy);
                    if callback
                        .send(CacheStorageThreadResponse::DeleteCacheResult(
                            result.map_err(|e| format!("{:?}", e)),
                        ))
                        .is_err()
                    {
                        error!("Failed to send response to script for DeleteCache message.");
                    }
                },
                CacheStorageThreadMessage::Exit(sender) => {
                    let _ = sender.send(());
                    break;
                },
            }
        }
    }
}
