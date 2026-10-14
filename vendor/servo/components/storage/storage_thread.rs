/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::PathBuf;

use profile_traits::mem::ProfilerChan as MemProfilerChan;
use servo_base::generic_channel::GenericSender;
use servo_base::id::ContextPartitionId;
use storage_traits::StorageThreads;
use storage_traits::cache_storage::CacheStorageThreadHandle;
use storage_traits::client_storage::ClientStorageThreadHandle;
use storage_traits::indexeddb::IndexedDBThreadMsg;
use storage_traits::webstorage_thread::WebStorageThreadMsg;
use storage_traits::{
    register_partition_storage_threads, remove_partition_storage_threads as forget_partition,
};

use crate::{
    CacheStorageThreadFactory, ClientStorageThreadFactory, IndexedDBThreadFactory,
    WebStorageThreadFactory,
};

fn new_storage_thread_group(
    mem_profiler_chan: MemProfilerChan,
    config_dir: Option<PathBuf>,
    temporary_storage: bool,
    label: &str,
) -> StorageThreads {
    let client_storage: ClientStorageThreadHandle =
        ClientStorageThreadFactory::new(config_dir.clone(), temporary_storage);
    let idb: GenericSender<IndexedDBThreadMsg> = IndexedDBThreadFactory::new(
        mem_profiler_chan.clone(),
        format!("indexedDB-reporter-{label}"),
    );
    let web_storage: GenericSender<WebStorageThreadMsg> = WebStorageThreadFactory::new(
        config_dir.clone(),
        mem_profiler_chan,
        format!("storage-reporter-{label}"),
    );
    let cache_storage: CacheStorageThreadHandle =
        CacheStorageThreadFactory::new(config_dir, temporary_storage);

    StorageThreads::new(
        client_storage.into(),
        idb,
        web_storage,
        cache_storage.into(),
    )
}

pub fn new_storage_threads(
    mem_profiler_chan: MemProfilerChan,
    config_dir: Option<PathBuf>,
    temporary_storage: bool,
) -> (StorageThreads, StorageThreads) {
    let private_storage_threads = new_storage_thread_group(
        mem_profiler_chan.clone(),
        config_dir.clone(),
        temporary_storage,
        "private",
    );
    let public_storage_threads =
        new_storage_thread_group(mem_profiler_chan, config_dir, temporary_storage, "public");

    (private_storage_threads, public_storage_threads)
}

/// BAO PATCH (REQ-CDS-009 C6): create and register the storage thread group
/// of one browser-context partition. Ephemeral by construction — no
/// `config_dir`, so webstorage / IndexedDB / CacheStorage state stays
/// in-memory only and a disposed context leaves nothing on disk. Called by
/// the embedder-facing registration API (`Servo::create_context_partition`)
/// at `Target.createBrowserContext` time, before any page of the context can
/// exist.
pub fn create_partition_storage_threads(
    partition: ContextPartitionId,
    mem_profiler_chan: MemProfilerChan,
    temporary_storage: bool,
) {
    let threads = new_storage_thread_group(
        mem_profiler_chan,
        None,
        temporary_storage,
        &format!("context-{partition}"),
    );
    register_partition_storage_threads(partition, threads);
}

/// BAO PATCH (REQ-CDS-009 C6): drop a context partition's storage group
/// registry entry (context disposal — pages of the context are closed first).
pub fn remove_partition_storage_threads(partition: ContextPartitionId) {
    forget_partition(partition);
}
