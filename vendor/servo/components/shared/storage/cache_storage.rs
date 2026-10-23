/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::ops::{Deref, DerefMut};

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};
use servo_base::generic_channel::{GenericCallback, GenericSender};
use servo_url::{ImmutableOrigin, ServoUrl};

use crate::client_storage::StorageProxyMap;

#[derive(Debug, Deserialize, Serialize)]
pub enum CacheStorageError<T> {
    Internal(T),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct CacheStorageThreadHandle {
    sender: GenericSender<CacheStorageThreadMessage>,
}

impl CacheStorageThreadHandle {
    pub fn new(sender: GenericSender<CacheStorageThreadMessage>) -> Self {
        CacheStorageThreadHandle { sender }
    }
}

impl From<CacheStorageThreadHandle> for GenericSender<CacheStorageThreadMessage> {
    fn from(handle: CacheStorageThreadHandle) -> Self {
        handle.sender
    }
}

impl From<GenericSender<CacheStorageThreadMessage>> for CacheStorageThreadHandle {
    fn from(sender: GenericSender<CacheStorageThreadMessage>) -> Self {
        CacheStorageThreadHandle::new(sender)
    }
}

impl Deref for CacheStorageThreadHandle {
    type Target = GenericSender<CacheStorageThreadMessage>;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}

impl DerefMut for CacheStorageThreadHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sender
    }
}

/// <https://w3c.github.io/ServiceWorker/#dictdef-cachequeryoptions>
///
/// The storage-side mirror of the generated `CacheQueryOptions` dictionary.
/// Kept free of DOM types so it can travel the storage thread channel.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub struct CacheQueryOptions {
    pub ignore_search: bool,
    pub ignore_method: bool,
    pub ignore_vary: bool,
}

/// <https://w3c.github.io/ServiceWorker/#dictdef-multicachequeryoptions>
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CacheStorageQueryOptions {
    pub ignore_search: bool,
    pub ignore_method: bool,
    pub ignore_vary: bool,
    /// `MultiCacheQueryOptions.cacheName`: when present, only the cache whose
    /// name matches exactly participates in the query.
    pub cache_name: Option<String>,
}

/// A [=request=] as stored in a cache's request response list.
///
/// Only the parts observable through the Cache API are retained: matching
/// happens on URL/method/headers and `keys()` reconstructs a `Request` from
/// them.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheRequest {
    pub url: ServoUrl,
    pub method: String,
    /// Header list as `(name, value)` pairs; names are already lowercased by
    /// the `http` crate and values keep their original bytes.
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
}

/// <https://fetch.spec.whatwg.org/#concept-response-type>
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CacheResponseType {
    Default,
    Basic,
    Cors,
    Error,
    Opaque,
    OpaqueRedirect,
}

/// A [=response=] as stored in a cache's request response list.
///
/// The body is fully buffered when the entry is written (spec: `Cache.put`
/// reads all bytes from the response body before the batch job runs), so a
/// cached entry can always be reconstructed independently of the network.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheResponse {
    pub status: u16,
    pub status_text: Vec<u8>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub url: Option<ServoUrl>,
    pub response_type: CacheResponseType,
    pub redirected: bool,
    pub body: Vec<u8>,
}

/// One entry of the [=request response list=].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheEntry {
    pub request: CacheRequest,
    pub response: CacheResponse,
}

/// Errors thrown by [=Batch Cache Operations=].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum CacheBatchError {
    /// An operation's request matches an item already added by this batch.
    InvalidState,
    Internal(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum CacheStorageThreadMessage {
    /// <https://w3c.github.io/ServiceWorker/#cache-storage-has>
    HasCache {
        cache_name: String,
        callback: GenericCallback<CacheStorageThreadResponse>,
        proxy: StorageProxyMap,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-storage-open>
    OpenCache {
        cache_name: String,
        callback: GenericCallback<CacheStorageThreadResponse>,
        proxy: StorageProxyMap,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-keys>
    Keys {
        cache_name: String,
        request: Option<CacheRequest>,
        options: CacheQueryOptions,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#batch-cache-operations>
    /// The batch form is shared by `Cache.put`, `Cache.add` and `Cache.addAll`
    /// so that a multi-entry add is applied atomically.
    Put {
        cache_name: String,
        entries: Vec<CacheEntry>,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-match>
    Match {
        cache_name: String,
        request: Option<CacheRequest>,
        options: CacheQueryOptions,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-matchall>
    MatchAll {
        cache_name: String,
        request: Option<CacheRequest>,
        options: CacheQueryOptions,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-delete>
    Delete {
        cache_name: String,
        request: CacheRequest,
        options: CacheQueryOptions,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-storage-keys>
    StorageKeys {
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#cache-storage-match>
    StorageMatch {
        request: CacheRequest,
        options: CacheStorageQueryOptions,
        callback: GenericCallback<CacheStorageThreadResponse>,
        origin: ImmutableOrigin,
    },
    /// <https://w3c.github.io/ServiceWorker/#dom-cachestorage-delete>
    DeleteCache {
        cache_name: String,
        callback: GenericCallback<CacheStorageThreadResponse>,
        proxy: StorageProxyMap,
        origin: ImmutableOrigin,
    },
    Exit(GenericSender<()>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum CacheStorageThreadResponse {
    HasCacheResult(Result<bool, String>),
    OpenCacheResult {
        result: Result<(), String>,
        cache_name: String,
    },
    KeysResult(Result<Vec<CacheRequest>, String>),
    MatchResult(Result<Option<CacheResponse>, String>),
    MatchAllResult(Result<Vec<CacheResponse>, String>),
    PutResult(Result<(), CacheBatchError>),
    DeleteEntriesResult(Result<bool, String>),
    DeleteCacheResult(Result<bool, String>),
    StorageKeysResult(Result<Vec<String>, String>),
    StorageMatchResult(Result<Option<CacheResponse>, String>),
}
