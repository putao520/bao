/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

mod engines;

use std::borrow::ToOwned;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use log::{debug, error, warn};
use malloc_size_of::MallocSizeOf;
use malloc_size_of_derive::MallocSizeOf;
use profile_traits::generic_callback::GenericCallback;
use profile_traits::mem::{
    ProcessReports, ProfilerChan as MemProfilerChan, Report, ReportKind, perform_memory_report,
};
use profile_traits::path;
use rusqlite::Error as RusqliteError;
use rustc_hash::{FxHashMap, FxHashSet};
use servo_base::generic_channel::{self, GenericReceiver, GenericSender, ReceiveError};
use servo_base::threadpool::ThreadPool;
use servo_url::origin::ImmutableOrigin;
use storage_traits::client_storage::StorageProxyMap;
use storage_traits::indexeddb::{
    AsyncOperation, AsyncReadOnlyOperation, AsyncReadWriteOperation, AsyncSchemaOperation,
    BackendError, BackendResult, ConnectionMsg, CreateObjectResult, DatabaseInfo, DbResult,
    IndexedDBIndex, IndexedDBKeyRange, IndexedDBKeyType, IndexedDBObjectStore, IndexedDBRecord,
    IndexedDBThreadMsg, IndexedDBTxnMode, KeyPath, PutItemResult, SyncOperation, TxnCompleteMsg,
};
use uuid::Uuid;

use crate::indexeddb::engines::{KvsEngine, KvsOperation, KvsTransaction, SqliteEngine};
use crate::shared::is_sqlite_disk_full_error;

pub trait IndexedDBThreadFactory {
    fn new(mem_profiler_chan: MemProfilerChan, reporter_name: String) -> Self;
}

// BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): an upgrade
// transaction's record writes are buffered on the manager thread (the shadow
// write buffer below) instead of being applied to the engine as they arrive.
// Commit lands the whole buffer as one engine batch before the connection is
// handed back to script; abort discards the buffer with zero engine IO — the
// writes never reached the engine, so dropping them IS the rollback. Reads
// inside the upgrade are answered from the merged view (engine records with
// the buffered writes applied), so script observes its own writes.
//
// Honest boundaries (registered by design):
// - `deleteObjectStore` + recreate of a pre-existing store inside the upgrade
//   resets that store's records on abort: the engine-side delete is real and
//   the shadow buffer only knows post-recreate writes, so the pre-delete
//   records cannot be restored. Record-level restore for that shape needs
//   engine-level transactionality (E-idb-tx domain).
// - A flush that partially landed before failing cannot be un-landed (the
//   engine has no transaction support); the commit path reverts schema and
//   version and reports `DatabaseError` instead of a green upgrade.
/// Hard cap for one upgrade transaction's shadow write buffer. A pathological
/// `upgradeneeded` handler can otherwise buffer an unbounded number of writes
/// on the manager thread before commit; exceeding the cap fails the offending
/// request with `QuotaExceededError` (zero partial buffering) instead of
/// ballooning manager-thread memory.
const UPGRADE_BUFFER_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Upper bound for the manager's blocking wait on one engine round trip (a
/// merged read, a presence query, or the commit flush). The engine thread
/// delivers one result per request; if it panics mid-batch the remaining
/// callbacks never fire, and an unbounded wait would wedge the
/// single-threaded [`IndexedDBManager`] forever (the failure shape from the
/// servo drop-spin wedges). 30s is generous next to typical engine batches;
/// exceeding it is a loud degraded round trip, never a hang.
const ENGINE_BATCH_WAIT: Duration = Duration::from_secs(30);

impl IndexedDBThreadFactory for GenericSender<IndexedDBThreadMsg> {
    fn new(
        mem_profiler_chan: MemProfilerChan,
        reporter_name: String,
    ) -> GenericSender<IndexedDBThreadMsg> {
        let (chan, port) = generic_channel::channel().unwrap();
        let chan2 = chan.clone();

        let manager_sender = chan.clone();

        thread::Builder::new()
            .name("IndexedDBManager".to_owned())
            .spawn(move || {
                mem_profiler_chan.run_with_memory_reporting(
                    || IndexedDBManager::new(port, manager_sender).start(),
                    reporter_name,
                    chan2,
                    IndexedDBThreadMsg::CollectMemoryReport,
                );
            })
            .expect("Thread spawning failed");

        chan
    }
}

/// A key used to track databases.
#[derive(Clone, Debug, Eq, Hash, MallocSizeOf, PartialEq)]
pub struct IndexedDBDescription {
    pub origin: ImmutableOrigin,
    pub name: String,
}

#[derive(MallocSizeOf)]
struct KeyGeneratorSnapshot {
    // Mirrors IndexedDB's "key generator current number" for rollback on abort.
    // https://w3c.github.io/IndexedDB/#key-generator-current-number
    // Backed by i64 so we can represent the IndexedDB-mandated range.
    store_name: String,
    current_number: i64,
}

#[derive(MallocSizeOf)]
struct TxnScopeStore {
    name: String,
    key_generator_snapshot: Option<KeyGeneratorSnapshot>,
}

#[derive(MallocSizeOf)]
struct TxnInfo {
    created_seq: u64,
    mode: IndexedDBTxnMode,
    scope: Vec<TxnScopeStore>,
    live: bool,
}

#[derive(MallocSizeOf)]
struct IndexedDBEnvironment<E: KvsEngine> {
    engine: E,
    manager_sender: GenericSender<IndexedDBThreadMsg>,
    transactions: FxHashMap<u64, KvsTransaction>,
    next_created_seq: u64,
    txn_info: FxHashMap<u64, TxnInfo>,
    queued_readwrite: VecDeque<u64>,
    queued_readonly: VecDeque<u64>,
    // Fast membership checks + de-dup for queued_*.
    queued_readwrite_set: FxHashSet<u64>,
    queued_readonly_set: FxHashSet<u64>,
    running_readwrite: Option<u64>,
    running_readonly: HashSet<u64>,
    handled_next_unhandled_request_id: FxHashMap<u64, u64>,
    handled_pending: FxHashMap<u64, HashSet<u64>>,
    pending_commit_callbacks: FxHashMap<u64, Vec<GenericCallback<TxnCompleteMsg>>>,
    pending_abort_callbacks: FxHashMap<u64, Vec<GenericCallback<TxnCompleteMsg>>>,
    /// BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): the
    /// shadow write buffer of the currently-running upgrade transaction,
    /// keyed by its serial number (at most one upgrade transaction runs at a
    /// time). Entries are created by `register_transaction` for
    /// `Versionchange` transactions and removed by commit
    /// (`commit_upgrade_buffer`) or abort (`discard_upgrade_buffer`) — and as
    /// a leak safety net by `finish_transaction` / `abort_transaction`.
    pending_upgrade_writes: FxHashMap<u64, UpgradeWriteBuffer>,
}

impl<E: KvsEngine> IndexedDBEnvironment<E> {
    fn new(
        engine: E,
        manager_sender: GenericSender<IndexedDBThreadMsg>,
    ) -> IndexedDBEnvironment<E> {
        IndexedDBEnvironment {
            engine,
            manager_sender,
            transactions: FxHashMap::default(),
            next_created_seq: 0,
            txn_info: FxHashMap::default(),
            queued_readwrite: VecDeque::new(),
            queued_readonly: VecDeque::new(),
            queued_readwrite_set: FxHashSet::default(),
            queued_readonly_set: FxHashSet::default(),
            running_readwrite: None,
            running_readonly: HashSet::new(),
            handled_next_unhandled_request_id: FxHashMap::default(),
            handled_pending: FxHashMap::default(),
            pending_commit_callbacks: FxHashMap::default(),
            pending_abort_callbacks: FxHashMap::default(),
            pending_upgrade_writes: FxHashMap::default(),
        }
    }

    fn register_transaction(&mut self, txn: u64, mode: IndexedDBTxnMode, scope: Vec<String>) {
        if self.txn_info.contains_key(&txn) {
            return;
        }
        let scope: HashSet<String> = scope.into_iter().collect();
        let scope: Vec<TxnScopeStore> = scope
            .into_iter()
            .map(|store_name| {
                let key_generator_snapshot = if mode == IndexedDBTxnMode::Readwrite {
                    self.key_generator_current_number(&store_name)
                        .map(|current_number| KeyGeneratorSnapshot {
                            store_name: store_name.clone(),
                            current_number,
                        })
                } else {
                    None
                };
                TxnScopeStore {
                    name: store_name,
                    key_generator_snapshot,
                }
            })
            .collect();
        let created_seq = self.next_created_seq;
        self.next_created_seq += 1;
        self.txn_info.insert(
            txn,
            TxnInfo {
                created_seq,
                mode: mode.clone(),
                scope,
                live: true,
            },
        );
        self.transactions
            .entry(txn)
            .or_insert_with(|| KvsTransaction {
                requests: VecDeque::new(),
                mode: mode.clone(),
            });
        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): an
        // upgrade transaction's record writes are buffered instead of being
        // applied immediately; create its shadow write buffer here so every
        // intercepted request finds one.
        if mode == IndexedDBTxnMode::Versionchange {
            self.pending_upgrade_writes
                .entry(txn)
                .or_default();
        }
    }

    fn scopes_overlap(a: &TxnInfo, b: &TxnInfo) -> bool {
        // From <https://w3c.github.io/IndexedDB/#upgrade-transaction-construct>
        // > An upgrade transaction is exclusive. The steps to open a database connection
        // > ensure that only one connection to the database is open when an upgrade
        // > transaction is live.
        //
        // This mean that versionupgrade transactions are always overlapping with others.
        // They are defined to contain all of the object stores in the database.
        if a.mode == IndexedDBTxnMode::Versionchange || b.mode == IndexedDBTxnMode::Versionchange {
            return true;
        }

        a.scope
            .iter()
            .any(|store| b.scope.iter().any(|other| other.name == store.name))
    }

    fn earlier_overlapping_live_exists<F>(&self, transaction: u64, predicate: F) -> bool
    where
        F: Fn(&TxnInfo) -> bool,
    {
        let Some(current) = self.txn_info.get(&transaction) else {
            return false;
        };

        let comes_before = |other_transaction: &TxnInfo| {
            other_transaction.live && other_transaction.created_seq < current.created_seq
        };

        self.txn_info.iter().any(|(other_transaction, other)| {
            *other_transaction != transaction &&
                comes_before(other) &&
                Self::scopes_overlap(current, other) &&
                predicate(other)
        })
    }

    fn can_start_by_spec(&self, txn: u64) -> bool {
        let Some(info) = self.txn_info.get(&txn) else {
            return false;
        };
        match info.mode {
            IndexedDBTxnMode::Readonly => !self.earlier_overlapping_live_exists(txn, |other| {
                other.mode != IndexedDBTxnMode::Readonly
            }),
            IndexedDBTxnMode::Readwrite | IndexedDBTxnMode::Versionchange => {
                !self.earlier_overlapping_live_exists(txn, |_other| true)
            },
        }
    }

    fn enqueue_txn(&mut self, txn: u64, mode: &IndexedDBTxnMode) {
        match mode {
            IndexedDBTxnMode::Readonly => {
                if self.queued_readonly_set.insert(txn) {
                    self.queued_readonly.push_back(txn);
                }
            },
            _ => {
                if self.queued_readwrite_set.insert(txn) {
                    self.queued_readwrite.push_back(txn);
                }
            },
        }
    }

    fn queue_operation(
        &mut self,
        store_name: &str,
        serial_number: u64,
        mode: IndexedDBTxnMode,
        operation: AsyncOperation,
    ) {
        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): an
        // upgrade transaction's record requests never reach the engine queue
        // directly — writes land in the shadow write buffer, reads are
        // answered from the merged view, and schema operations pass through
        // unchanged.
        let operation = if self.txn_is_upgrade(serial_number) {
            match self.queue_upgrade_operation(store_name, serial_number, operation) {
                Some(operation) => operation,
                // The shadow write buffer answered or absorbed the request.
                None => return,
            }
        } else {
            operation
        };
        let mut enqueue_mode = None;
        match self.transactions.entry(serial_number) {
            Entry::Occupied(mut entry) => {
                let transaction = entry.get_mut();
                let transaction_mode = transaction.mode.clone();
                let was_empty = transaction.requests.is_empty();
                transaction.requests.push_back(KvsOperation {
                    operation,
                    store_name: String::from(store_name),
                });
                if was_empty {
                    // If the queue was empty and we just added work, we must enqueue the txn
                    // even if it is currently running a prior batch. The next batch will run
                    // after EngineTxnBatchComplete triggers scheduling again.
                    enqueue_mode = Some(transaction_mode);
                }
            },
            Entry::Vacant(entry) => {
                entry
                    .insert(KvsTransaction {
                        requests: VecDeque::new(),
                        mode: mode.clone(),
                    })
                    .requests
                    .push_back(KvsOperation {
                        operation,
                        store_name: String::from(store_name),
                    });
                enqueue_mode = Some(mode);
            },
        };
        if let Some(mode) = enqueue_mode {
            self.enqueue_txn(serial_number, &mode);
        }
    }

    fn txn_is_upgrade(&self, txn: u64) -> bool {
        // The upgrade transaction is the only `Versionchange` transaction, and
        // its `TxnInfo` entry is registered before any request can be queued
        // against it, so `txn_info` is the authoritative interception source.
        self.txn_info
            .get(&txn)
            .is_some_and(|info| info.mode == IndexedDBTxnMode::Versionchange)
    }

    /// Routes one queued operation through the upgrade shadow write buffer.
    ///
    /// Returns `Some(operation)` when the operation must continue through the
    /// normal engine queue (schema pass-through, or a read no buffered write
    /// touches); `None` when the buffer path handled it (answered or
    /// buffered).
    fn queue_upgrade_operation(
        &mut self,
        store_name: &str,
        txn: u64,
        operation: AsyncOperation,
    ) -> Option<AsyncOperation> {
        match operation {
            AsyncOperation::Schema(schema) => {
                match &schema {
                    // A store created during the upgrade is empty in the
                    // engine until commit (its schema write has not landed
                    // yet when subsequent requests are intercepted), so
                    // presence checks and merged reads must not query the
                    // engine for it. With `autoIncrement` it also starts a key
                    // generator at 1.
                    AsyncSchemaOperation::CreateObjectStore { auto_increment, .. } => {
                        let buffer = self.pending_upgrade_writes.entry(txn).or_default();
                        buffer.engine_empty_stores.insert(store_name.to_string());
                        if *auto_increment {
                            let store_writes =
                                buffer.stores.entry(store_name.to_string()).or_default();
                            // https://w3c.github.io/IndexedDB/#key-generator-construct
                            // The initial value of a key generator’s current
                            // number is 1, set when the associated object
                            // store is created.
                            if store_writes.generator_current.is_none() {
                                store_writes.generator_current = Some(1);
                            }
                        }
                    },
                    // A `deleteObjectStore` inside the upgrade deletes the
                    // engine rows for real; buffered shadow writes for the
                    // deleted store would target a missing store at commit
                    // and fail the flush, so drop them with the store.
                    AsyncSchemaOperation::DeleteObjectStore { .. } => {
                        let buffer = self.pending_upgrade_writes.entry(txn).or_default();
                        buffer.engine_empty_stores.remove(store_name);
                        if let Some(store_writes) = buffer.stores.remove(store_name) {
                            buffer.bytes = buffer.bytes.saturating_sub(store_writes.bytes);
                        }
                    },
                    _ => {},
                }
                Some(AsyncOperation::Schema(schema))
            },
            AsyncOperation::ReadWrite(write) => {
                self.buffer_upgrade_write(txn, store_name, write);
                None
            },
            AsyncOperation::ReadOnly(read) => {
                match self.answer_upgrade_read(txn, store_name, read) {
                    Some(read) => Some(AsyncOperation::ReadOnly(read)),
                    None => None,
                }
            },
        }
    }

    fn buffer_upgrade_write(
        &mut self,
        txn: u64,
        store_name: &str,
        operation: AsyncReadWriteOperation,
    ) {
        match operation {
            AsyncReadWriteOperation::PutItem {
                callback,
                key,
                value,
                should_overwrite,
                key_generator_current_number,
            } => {
                self.buffer_upgrade_put(
                    txn,
                    store_name,
                    callback,
                    key,
                    value,
                    should_overwrite,
                    key_generator_current_number,
                );
            },
            AsyncReadWriteOperation::RemoveItem {
                callback,
                key_range,
            } => {
                self.buffer_upgrade_delete(txn, store_name, callback, key_range);
            },
            AsyncReadWriteOperation::Clear(callback) => {
                self.buffer_upgrade_clear(txn, store_name, callback);
            },
        }
    }

    fn buffer_upgrade_put(
        &mut self,
        txn: u64,
        store_name: &str,
        callback: GenericCallback<BackendResult<PutItemResult>>,
        key: Option<IndexedDBKeyType>,
        value: Vec<u8>,
        should_overwrite: bool,
        key_generator_current_number: Option<i64>,
    ) {
        // The generator advance an explicit-key put carries is replayed
        // verbatim (script computes it — idbobjectstore.rs
        // `possibly_update_the_key_generator` — and the engine's `put_item`
        // applies it on success). A keyless put mints from the buffered key
        // generator instead, mirroring the engine's keyless-put arm
        // (engines/sqlite.rs:628-656) including its error parity.
        let (effective_key, key_generator_current_number) = match key {
            Some(key) => (key, key_generator_current_number),
            None => match self.next_upgrade_generator_key(txn, store_name) {
                Ok((key, next)) => (key, Some(next)),
                Err(error) => {
                    let _ = callback.send(Err(error));
                    return;
                },
            },
        };

        // `add` must not overwrite: consult the shadow view first, then the
        // engine for keys no buffered write touches. A failed add changes no
        // state, so nothing is buffered.
        if !should_overwrite {
            match self.upgrade_key_is_present(txn, store_name, &effective_key) {
                Ok(true) => {
                    let _ = callback.send(Ok(PutItemResult::CannotOverwrite));
                    return;
                },
                Ok(false) => {},
                Err(error) => {
                    let _ = callback.send(Err(error));
                    return;
                },
            }
        }

        let put_bytes = pending_key_bytes(&effective_key) + value.len() as u64;

        // Quota gate BEFORE any buffer mutation: zero partial buffering.
        let over_limit = {
            let buffer = self.pending_upgrade_writes.entry(txn).or_default();
            buffer.bytes.saturating_add(put_bytes) > UPGRADE_BUFFER_MAX_BYTES
        };
        if over_limit {
            let _ = callback.send(Err(BackendError::QuotaExceeded));
            return;
        }

        let previous_store_bytes = self
            .pending_upgrade_writes
            .get(&txn)
            .and_then(|buffer| buffer.stores.get(store_name))
            .map(|store_writes| store_writes.bytes)
            .unwrap_or(0);
        let new_store_bytes = {
            let store_writes = self.upgrade_buffer_store_entry(txn, store_name);
            let mut new_store_bytes = store_writes.bytes;
            let mut replaced = false;
            // A later put of the same key overwrites the earlier buffered put
            // in place. The scan stops at a Delete/Clear: below it, an older
            // put of this key is no longer visible, so the new write must be
            // appended instead.
            if should_overwrite {
                for write_slot in store_writes.ops.iter_mut().rev() {
                    match write_slot {
                        PendingWrite::Put {
                            key: put_key,
                            value: put_value,
                            ..
                        } if *put_key == effective_key => {
                            new_store_bytes = new_store_bytes
                                .saturating_sub(pending_key_bytes(put_key) + put_value.len() as u64)
                                .saturating_add(put_bytes);
                            *put_value = value.clone();
                            replaced = true;
                            break;
                        },
                        PendingWrite::Delete { .. } | PendingWrite::Clear => break,
                        PendingWrite::Put { .. } => {},
                    }
                }
            }
            if !replaced {
                store_writes.ops.push(PendingWrite::Put {
                    key: effective_key.clone(),
                    value,
                    key_generator_current_number,
                });
                new_store_bytes = new_store_bytes.saturating_add(put_bytes);
            }
            // An explicit-key put carrying a generator advance moves the
            // buffered generator exactly like the engine's `put_item` would
            // have when the op landed.
            if let Some(next) = key_generator_current_number {
                store_writes.generator_current = Some(next);
            }
            store_writes.bytes = new_store_bytes;
            new_store_bytes
        };
        let buffer = self
            .pending_upgrade_writes
            .get_mut(&txn)
            .expect("buffer entry exists after the store entry was created");
        buffer.bytes = buffer
            .bytes
            .saturating_sub(previous_store_bytes)
            .saturating_add(new_store_bytes);
        let _ = callback.send(Ok(PutItemResult::Key(effective_key)));
    }

    fn buffer_upgrade_delete(
        &mut self,
        txn: u64,
        store_name: &str,
        callback: GenericCallback<BackendResult<()>>,
        key_range: IndexedDBKeyRange,
    ) {
        let write = PendingWrite::Delete { key_range };
        let write_bytes = pending_write_bytes(&write);

        // Quota gate BEFORE any buffer mutation: zero partial buffering.
        let over_limit = {
            let buffer = self.pending_upgrade_writes.entry(txn).or_default();
            buffer.bytes.saturating_add(write_bytes) > UPGRADE_BUFFER_MAX_BYTES
        };
        if over_limit {
            let _ = callback.send(Err(BackendError::QuotaExceeded));
            return;
        }

        let store_writes = self.upgrade_buffer_store_entry(txn, store_name);
        store_writes.bytes = store_writes.bytes.saturating_add(write_bytes);
        store_writes.ops.push(write);
        let buffer = self
            .pending_upgrade_writes
            .get_mut(&txn)
            .expect("buffer entry exists after the store entry was created");
        buffer.bytes = buffer.bytes.saturating_add(write_bytes);
        let _ = callback.send(Ok(()));
    }

    fn buffer_upgrade_clear(
        &mut self,
        txn: u64,
        store_name: &str,
        callback: GenericCallback<BackendResult<()>>,
    ) {
        // Collapse: the Clear subsumes every write buffered for this store so
        // far. The key generator survives the collapse — spec `clear()` only
        // removes the records, it does not reset the key generator.
        let cleared_store_bytes = {
            let store_writes = self.upgrade_buffer_store_entry(txn, store_name);
            let cleared = store_writes.bytes;
            store_writes.ops.clear();
            store_writes.ops.push(PendingWrite::Clear);
            store_writes.bytes = 0;
            cleared
        };
        let buffer = self
            .pending_upgrade_writes
            .get_mut(&txn)
            .expect("buffer entry exists after the store entry was created");
        buffer.bytes = buffer.bytes.saturating_sub(cleared_store_bytes);
        let _ = callback.send(Ok(()));
    }

    /// Returns the buffered entry of `store_name`, creating it with the
    /// store's key generator state resolved when fresh. Once an entry exists,
    /// its `generator_current` is authoritative (`None` there = the store has
    /// no key generator): keyless puts, explicit-put advances and commit all
    /// read and move the same buffered value, mirroring what the engine would
    /// have applied per-op.
    fn upgrade_buffer_store_entry(
        &mut self,
        txn: u64,
        store_name: &str,
    ) -> &mut PendingStoreWrites {
        let needs_resolution = !self
            .pending_upgrade_writes
            .get(&txn)
            .is_some_and(|buffer| buffer.stores.contains_key(store_name));
        let resolved = if needs_resolution {
            let store_known_engine_empty = self
                .pending_upgrade_writes
                .get(&txn)
                .is_some_and(|buffer| buffer.engine_empty_stores.contains(store_name));
            if store_known_engine_empty {
                // Created during this upgrade without a key generator (the
                // auto-increment seeding in the CreateObjectStore pass-through
                // already made a resolved entry): never query the engine for a
                // store whose schema write has not landed yet.
                None
            } else {
                self.key_generator_current_number(store_name)
            }
        } else {
            None
        };
        let buffer = self.pending_upgrade_writes.entry(txn).or_default();
        let entry = buffer.stores.entry(store_name.to_string()).or_default();
        if needs_resolution {
            entry.generator_current = resolved;
        }
        entry
    }

    /// Resolves the key and the generator advance a keyless `put` must use
    /// inside an upgrade transaction. Error parity matches the engine's
    /// keyless-put arm (engines/sqlite.rs:628-656).
    fn next_upgrade_generator_key(
        &mut self,
        txn: u64,
        store_name: &str,
    ) -> BackendResult<(IndexedDBKeyType, i64)> {
        let generator_current = self
            .upgrade_buffer_store_entry(txn, store_name)
            .generator_current;
        let Some(current) = generator_current else {
            return Err(BackendError::DbErr(
                "Missing key for PutItem request".to_string(),
            ));
        };
        let Some(next) = current.checked_add(1) else {
            return Err(BackendError::DbErr("Key generator overflow".to_string()));
        };
        let entry = self.upgrade_buffer_store_entry(txn, store_name);
        entry.generator_current = Some(next);
        Ok((IndexedDBKeyType::Number(current as f64), next))
    }

    /// Buffered visibility of one key in an upgrade transaction's shadow
    /// writes. The scan walks the buffered ops backwards: the newest op that
    /// mentions the key decides, and ops below a Delete/Clear never matter.
    fn upgrade_lookup_key(
        &self,
        txn: u64,
        store_name: &str,
        key: &IndexedDBKeyType,
    ) -> BufferedKeyLookup {
        let Some(buffer) = self.pending_upgrade_writes.get(&txn) else {
            return BufferedKeyLookup::NotBuffered;
        };
        let Some(store_writes) = buffer.stores.get(store_name) else {
            return BufferedKeyLookup::NotBuffered;
        };
        for write in store_writes.ops.iter().rev() {
            match write {
                PendingWrite::Put {
                    key: put_key,
                    value,
                    ..
                } if put_key == key => return BufferedKeyLookup::Value(value.clone()),
                PendingWrite::Delete { key_range } if key_range.contains(key) => {
                    return BufferedKeyLookup::Missing;
                },
                PendingWrite::Clear => return BufferedKeyLookup::Missing,
                PendingWrite::Put { .. } | PendingWrite::Delete { .. } => {},
            }
        }
        BufferedKeyLookup::NotBuffered
    }

    /// Whether `key` is visible in `store_name` under the upgrade's shadow
    /// writes — used by `add`'s must-not-overwrite gate.
    fn upgrade_key_is_present(
        &self,
        txn: u64,
        store_name: &str,
        key: &IndexedDBKeyType,
    ) -> BackendResult<bool> {
        match self.upgrade_lookup_key(txn, store_name, key) {
            BufferedKeyLookup::Value(_) => Ok(true),
            BufferedKeyLookup::Missing => Ok(false),
            BufferedKeyLookup::NotBuffered => {
                // O(1) engine-empty cache: a store created during this
                // upgrade is empty in the engine until commit (its schema
                // write may not even have landed yet), so the single-point
                // engine query is skipped and a not-yet-materialized store
                // cannot yield a spurious StoreNotFound.
                let engine_store_known_empty = self
                    .pending_upgrade_writes
                    .get(&txn)
                    .is_some_and(|buffer| buffer.engine_empty_stores.contains(store_name));
                if engine_store_known_empty {
                    return Ok(false);
                }
                self.engine_key_present(store_name, key)
            },
        }
    }

    /// Single-point engine presence query (bounded wait), used by `add`'s
    /// presence gate when no buffered write touches the key.
    fn engine_key_present(
        &self,
        store_name: &str,
        key: &IndexedDBKeyType,
    ) -> BackendResult<bool> {
        let (result_sender, result_receiver) = mpsc::channel();
        let callback = GenericCallback::new(
            move |result: Result<BackendResult<Option<Vec<u8>>>, generic_channel::SendError>| {
                let outcome = result.unwrap_or_else(|send_error| {
                    Err(BackendError::DbErr(format!("{send_error:?}")))
                });
                let _ = result_sender.send(outcome);
            },
        )
        .map_err(|send_error| BackendError::DbErr(format!("{send_error:?}")))?;
        self.engine.process_transaction(
            KvsTransaction {
                mode: IndexedDBTxnMode::Readonly,
                requests: VecDeque::from(vec![KvsOperation {
                    store_name: store_name.to_string(),
                    operation: AsyncOperation::ReadOnly(AsyncReadOnlyOperation::GetItem {
                        callback,
                        key_range: IndexedDBKeyRange::only(key.clone()),
                    }),
                }]),
            },
            Box::new(|| ()),
        );
        match result_receiver.recv_timeout(ENGINE_BATCH_WAIT) {
            Ok(Ok(Some(_))) => Ok(true),
            Ok(Ok(None)) => Ok(false),
            Ok(Err(error)) => Err(error),
            Err(wait_error) => Err(BackendError::DbErr(format!(
                "engine presence query did not complete in time: {wait_error:?}"
            ))),
        }
    }

    /// Answers one read of the running upgrade transaction from the shadow
    /// write buffer's merged view.
    ///
    /// Returns `Some(operation)` when the buffer has no answer for it (a
    /// singleton read whose key no buffered write touches): the operation is
    /// then served by the engine exactly as it would have been without the
    /// buffer. Returns `None` when the callback was answered here.
    fn answer_upgrade_read(
        &self,
        txn: u64,
        store_name: &str,
        operation: AsyncReadOnlyOperation,
    ) -> Option<AsyncReadOnlyOperation> {
        match operation {
            AsyncReadOnlyOperation::GetItem { callback, key_range } => {
                if let Some(key) = key_range.as_singleton() {
                    match self.upgrade_lookup_key(txn, store_name, key) {
                        BufferedKeyLookup::Value(value) => {
                            let _ = callback.send(Ok(Some(value)));
                            return None;
                        },
                        BufferedKeyLookup::Missing => {
                            let _ = callback.send(Ok(None));
                            return None;
                        },
                        // Nothing buffered touches this key: the engine's
                        // view is authoritative.
                        BufferedKeyLookup::NotBuffered => {
                            return Some(AsyncReadOnlyOperation::GetItem {
                                callback,
                                key_range,
                            });
                        },
                    }
                }
                match self.merged_upgrade_records(txn, store_name, &key_range) {
                    Ok(records) => {
                        let _ = callback.send(Ok(
                            records.into_iter().next().map(|record| record.value)
                        ));
                    },
                    Err(error) => {
                        error!("Failed to read through the upgrade write buffer of object store \
                                '{store_name}': {error}");
                        let _ = callback.send(Err(error));
                    },
                }
                None
            },
            AsyncReadOnlyOperation::GetKey { callback, key_range } => {
                if let Some(key) = key_range.as_singleton() {
                    match self.upgrade_lookup_key(txn, store_name, key) {
                        BufferedKeyLookup::Value(_) => {
                            let _ = callback.send(Ok(Some(key.clone())));
                            return None;
                        },
                        BufferedKeyLookup::Missing => {
                            let _ = callback.send(Ok(None));
                            return None;
                        },
                        BufferedKeyLookup::NotBuffered => {
                            return Some(AsyncReadOnlyOperation::GetKey {
                                callback,
                                key_range,
                            });
                        },
                    }
                }
                match self.merged_upgrade_records(txn, store_name, &key_range) {
                    Ok(records) => {
                        let _ =
                            callback.send(Ok(records.into_iter().next().map(|record| record.key)));
                    },
                    Err(error) => {
                        error!("Failed to read through the upgrade write buffer of object store \
                                '{store_name}': {error}");
                        let _ = callback.send(Err(error));
                    },
                }
                None
            },
            AsyncReadOnlyOperation::GetAllKeys {
                callback,
                key_range,
                count,
            } => {
                let answer = self
                    .merged_upgrade_records(txn, store_name, &key_range)
                    .map(|mut records| {
                        if let Some(count) = count {
                            records.truncate(count as usize);
                        }
                        records.into_iter().map(|record| record.key).collect()
                    });
                if let Err(error) = &answer {
                    error!("Failed to read through the upgrade write buffer of object store \
                            '{store_name}': {error}");
                }
                let _ = callback.send(answer);
                None
            },
            AsyncReadOnlyOperation::GetAllItems {
                callback,
                key_range,
                count,
            } => {
                let answer = self
                    .merged_upgrade_records(txn, store_name, &key_range)
                    .map(|mut records| {
                        if let Some(count) = count {
                            records.truncate(count as usize);
                        }
                        records.into_iter().map(|record| record.value).collect()
                    });
                if let Err(error) = &answer {
                    error!("Failed to read through the upgrade write buffer of object store \
                            '{store_name}': {error}");
                }
                let _ = callback.send(answer);
                None
            },
            AsyncReadOnlyOperation::Count {
                callback,
                key_range,
            } => {
                let answer = self
                    .merged_upgrade_records(txn, store_name, &key_range)
                    .map(|records| records.len() as u64);
                if let Err(error) = &answer {
                    error!("Failed to read through the upgrade write buffer of object store \
                            '{store_name}': {error}");
                }
                let _ = callback.send(answer);
                None
            },
            AsyncReadOnlyOperation::Iterate {
                callback,
                key_range,
            } => {
                let answer = self.merged_upgrade_records(txn, store_name, &key_range);
                if let Err(error) = &answer {
                    error!("Failed to read through the upgrade write buffer of object store \
                            '{store_name}': {error}");
                }
                let _ = callback.send(answer);
                None
            },
        }
    }

    /// The merged view of `store_name` under the upgrade transaction's shadow
    /// writes: the engine's records with every buffered op applied in order.
    ///
    /// The engine read is skipped when it can contribute nothing (first
    /// buffered op is a Clear — everything below it was subsumed — or the
    /// store is known empty in the engine because the upgrade created it and
    /// its schema write has not landed yet).
    fn merged_upgrade_records(
        &self,
        txn: u64,
        store_name: &str,
        key_range: &IndexedDBKeyRange,
    ) -> BackendResult<Vec<IndexedDBRecord>> {
        let buffered = self
            .pending_upgrade_writes
            .get(&txn)
            .and_then(|buffer| buffer.stores.get(store_name));
        let engine_store_known_empty = self
            .pending_upgrade_writes
            .get(&txn)
            .is_some_and(|buffer| buffer.engine_empty_stores.contains(store_name));
        let read_engine = buffered
            .map(|store_writes| !matches!(store_writes.ops.first(), Some(PendingWrite::Clear)))
            .unwrap_or(true) &&
            !engine_store_known_empty;

        // The engine returns rows in insertion order, not key order; sort so
        // the apply pass below can keep `records` sorted for `partition_point`.
        let mut records = if read_engine {
            let mut records = self.engine_iterate(store_name, key_range)?;
            records.sort_by(|a, b| total_key_cmp(&a.key, &b.key));
            records
        } else {
            Vec::new()
        };
        let Some(store_writes) = buffered else {
            return Ok(records);
        };
        for write in &store_writes.ops {
            match write {
                PendingWrite::Clear => records.clear(),
                PendingWrite::Delete {
                    key_range: delete_range,
                } => records.retain(|record| !delete_range.contains(&record.key)),
                PendingWrite::Put { key, value, .. } => {
                    if !key_range.contains(key) {
                        // The read only observes records inside its range.
                        continue;
                    }
                    let index = records.partition_point(|record| {
                        total_key_cmp(&record.key, key) == Ordering::Less
                    });
                    match records.get_mut(index) {
                        Some(record) if total_key_cmp(&record.key, key) == Ordering::Equal => {
                            record.value = value.clone();
                        },
                        _ => records.insert(
                            index,
                            IndexedDBRecord {
                                key: key.clone(),
                                primary_key: key.clone(),
                                value: value.clone(),
                            },
                        ),
                    }
                },
            }
        }
        Ok(records)
    }

    /// Engine Iterate round trip (bounded wait), the data source of
    /// [`Self::merged_upgrade_records`].
    fn engine_iterate(
        &self,
        store_name: &str,
        key_range: &IndexedDBKeyRange,
    ) -> BackendResult<Vec<IndexedDBRecord>> {
        let (result_sender, result_receiver) = mpsc::channel();
        let callback = GenericCallback::new(
            move |result: Result<
                BackendResult<Vec<IndexedDBRecord>>,
                generic_channel::SendError,
            >| {
                let outcome = result.unwrap_or_else(|send_error| {
                    Err(BackendError::DbErr(format!("{send_error:?}")))
                });
                let _ = result_sender.send(outcome);
            },
        )
        .map_err(|send_error| BackendError::DbErr(format!("{send_error:?}")))?;
        self.engine.process_transaction(
            KvsTransaction {
                mode: IndexedDBTxnMode::Readonly,
                requests: VecDeque::from(vec![KvsOperation {
                    store_name: store_name.to_string(),
                    operation: AsyncOperation::ReadOnly(AsyncReadOnlyOperation::Iterate {
                        callback,
                        key_range: key_range.clone(),
                    }),
                }]),
            },
            Box::new(|| ()),
        );
        match result_receiver.recv_timeout(ENGINE_BATCH_WAIT) {
            Ok(Ok(records)) => Ok(records),
            Ok(Err(error)) => Err(error),
            Err(wait_error) => Err(BackendError::DbErr(format!(
                "engine iterate did not complete in time: {wait_error:?}"
            ))),
        }
    }

    /// Lands one upgrade transaction's shadow write buffer in the engine as a
    /// single Readwrite batch — the engine preserves in-batch FIFO order
    /// (engines/sqlite.rs:584), which is the buffered ops' order, which is
    /// script's request order (script/dom/indexeddb/idbtransaction.rs:552-676).
    /// Keyless puts replay their buffered generator advance per-put.
    ///
    /// The buffer entry is removed up front so a failure can never leave
    /// stale shadow state behind; the caller owns the fail-closed handling.
    fn commit_upgrade_buffer(&mut self, txn: u64) -> DbResult<()> {
        let Some(buffer) = self.pending_upgrade_writes.remove(&txn) else {
            return Ok(());
        };
        if buffer.stores.is_empty() {
            return Ok(());
        }

        let (result_sender, result_receiver) = mpsc::channel();
        let mut requests = VecDeque::new();
        for (store_name, store_writes) in &buffer.stores {
            for write in &store_writes.ops {
                let callback = upgrade_write_callback(store_name.clone(), result_sender.clone());
                let operation = match write {
                    PendingWrite::Clear => {
                        AsyncOperation::ReadWrite(AsyncReadWriteOperation::Clear(callback))
                    },
                    PendingWrite::Put {
                        key,
                        value,
                        key_generator_current_number,
                    } => AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                        callback: upgrade_put_item_callback(
                            store_name.clone(),
                            result_sender.clone(),
                        ),
                        key: Some(key.clone()),
                        value: value.clone(),
                        // Buffered writes were presence-checked at intercept
                        // time and same-key overwrites collapsed in place, so
                        // replay is put semantics.
                        should_overwrite: true,
                        key_generator_current_number: *key_generator_current_number,
                    }),
                    PendingWrite::Delete { key_range } => {
                        AsyncOperation::ReadWrite(AsyncReadWriteOperation::RemoveItem {
                            callback,
                            key_range: key_range.clone(),
                        })
                    },
                };
                requests.push_back(KvsOperation {
                    store_name: store_name.clone(),
                    operation,
                });
            }
        }
        drop(result_sender);

        let expected = requests.len();
        if expected == 0 {
            return Ok(());
        }

        self.engine.process_transaction(
            KvsTransaction {
                mode: IndexedDBTxnMode::Readwrite,
                requests,
            },
            Box::new(|| ()),
        );

        let mut first_error: Option<String> = None;
        for _ in 0..expected {
            let (store_name, outcome) = match result_receiver.recv_timeout(ENGINE_BATCH_WAIT) {
                Ok(outcome) => outcome,
                Err(wait_error) => {
                    return Err(format!(
                        "shadow write buffer commit did not complete in time: {wait_error:?}"
                    ));
                },
            };
            if let Err(write_error) = outcome {
                error!(
                    "Failed to land the buffered write of object store '{store_name}' at upgrade \
                     commit: {write_error}"
                );
                if first_error.is_none() {
                    first_error = Some(write_error);
                }
            }
        }

        // Reconcile the buffered key generators: the buffered value is the
        // desired post-commit current number. The batch's per-put advances
        // already land most of it; the explicit set also covers stores whose
        // surviving ops carry no advance (e.g. a same-key overwrite that
        // replaced the minted put). Fail-closed like the batch.
        for (store_name, store_writes) in &buffer.stores {
            let Some(current) = store_writes.generator_current else {
                continue;
            };
            if let Err(eng_error) =
                self.engine.set_key_generator_current_number(store_name, current)
            {
                let error = format!(
                    "failed to persist the key generator of object store '{store_name}' at \
                     upgrade commit: {eng_error:?}"
                );
                error!("{error}");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Drops one upgrade transaction's shadow write buffer. Abort is a
    /// zero-IO rollback: the buffered writes never reached the engine, so
    /// dropping them is the whole record-level rollback.
    fn discard_upgrade_buffer(&mut self, txn: u64) {
        self.pending_upgrade_writes.remove(&txn);
    }

    /// <https://w3c.github.io/IndexedDB/#transaction-scheduling>
    fn schedule_transactions(&mut self, origin: ImmutableOrigin, db_name: &str) {
        let readonly_len = self.queued_readonly.len();
        for _ in 0..readonly_len {
            let Some(txn) = self.queued_readonly.pop_front() else {
                break;
            };

            self.queued_readonly_set.remove(&txn);

            let Some(transaction) = self.transactions.get(&txn) else {
                continue;
            };
            if self.running_readonly.contains(&txn) {
                if !transaction.requests.is_empty() && self.queued_readonly_set.insert(txn) {
                    self.queued_readonly.push_back(txn);
                }
                continue;
            }
            if transaction.requests.is_empty() {
                continue;
            }
            if !self.can_start_by_spec(txn) {
                if self.queued_readonly_set.insert(txn) {
                    self.queued_readonly.push_back(txn);
                }
                continue;
            }

            // Mark running and start async.
            // DO NOT clear here; clear on EngineTxnBatchComplete.
            self.running_readonly.insert(txn);
            self.start_transaction(origin.clone(), db_name.to_string(), txn, None);
        }

        // Start at most one readwrite txn.
        if self.running_readwrite.is_some() {
            return;
        }
        let readwrite_len = self.queued_readwrite.len();
        for _ in 0..readwrite_len {
            let Some(txn) = self.queued_readwrite.pop_front() else {
                break;
            };

            self.queued_readwrite_set.remove(&txn);

            let Some(transaction) = self.transactions.get(&txn) else {
                continue;
            };

            if self.running_readwrite == Some(txn) {
                if !transaction.requests.is_empty() && self.queued_readwrite_set.insert(txn) {
                    self.queued_readwrite.push_back(txn);
                }
                continue;
            }

            if transaction.requests.is_empty() {
                continue;
            }
            if !self.can_start_by_spec(txn) {
                if self.queued_readwrite_set.insert(txn) {
                    self.queued_readwrite.push_back(txn);
                }
                continue;
            }

            // Mark running and start async.
            // DO NOT clear here; clear on EngineTxnBatchComplete.
            self.running_readwrite = Some(txn);
            self.start_transaction(origin, db_name.to_string(), txn, None);
            return;
        }
    }

    // Executes all requests for a transaction (without committing)
    fn start_transaction(
        &mut self,
        origin: ImmutableOrigin,
        db_name: String,
        txn: u64,
        sender: Option<GenericSender<BackendResult<()>>>,
    ) {
        // https://w3c.github.io/IndexedDB/#transaction-lifetime
        // Take the current queued batch of requests for this txn.
        // If more requests arrive while the engine is running, they’ll be queued into
        // transaction.requests and scheduled in a later batch once we receive EngineTxnBatchComplete.
        let (mode, requests) = match self.transactions.get_mut(&txn) {
            Some(transaction) => {
                let mode = transaction.mode.clone();
                let requests = std::mem::take(&mut transaction.requests);
                (mode, requests)
            },
            None => {
                // If a manual starter is waiting, treat as "nothing to do".
                if let Some(sender) = sender {
                    let _ = sender.send(Ok(()));
                }
                return;
            },
        };

        if requests.is_empty() {
            if let Some(sender) = sender {
                let _ = sender.send(Ok(()));
            }
            // Important: if there was no work, do NOT send EngineTxnBatchComplete,
            // otherwise we can create a pointless reschedule loop.
            return;
        }

        let manager_sender = self.manager_sender.clone();
        self.engine.process_transaction(
            KvsTransaction { mode, requests },
            Box::new(move || {
                // Notify the manager thread when the engine finishes so it can:
                // - clear running_readonly / running_readwrite
                // - re-run scheduling (maybe start next queued txn or next batch)
                if let Err(err) = manager_sender.send(IndexedDBThreadMsg::EngineTxnBatchComplete {
                    origin,
                    db_name,
                    txn,
                }) {
                    error!(
                        "Failed to send IndexedDBThreadMsg::EngineTxnBatchComplete: {:?}",
                        err
                    );
                    // TODO: Prevent backend/manager shutdown while a transaction batch is
                    // running so completion notification cannot be dropped in normal flow.
                }

                // We have a sender if the transaction is started manually, and they
                // probably want to know when it is finished.
                if let Some(sender) = sender {
                    let _ = sender.send(Ok(()));
                }
            }),
        );
    }

    fn mark_request_handled(&mut self, txn: u64, request_id: u64) {
        let current = self
            .handled_next_unhandled_request_id
            .get(&txn)
            .copied()
            .unwrap_or(0);
        if request_id == current {
            let mut next = current + 1;
            if let Some(pending) = self.handled_pending.get_mut(&txn) {
                while pending.remove(&next) {
                    next += 1;
                }
                if pending.is_empty() {
                    self.handled_pending.remove(&txn);
                }
            }
            self.handled_next_unhandled_request_id.insert(txn, next);
        } else if request_id > current {
            self.handled_pending
                .entry(txn)
                .or_default()
                .insert(request_id);
        }
    }

    fn can_notify_txn_maybe_commit(&self, txn: u64) -> bool {
        if self.running_readwrite == Some(txn) || self.running_readonly.contains(&txn) {
            return false;
        }
        // Avoid if the txn is still queued or has queued operations.
        if self.queued_readonly_set.contains(&txn) || self.queued_readwrite_set.contains(&txn) {
            return false;
        }
        match self.transactions.get(&txn) {
            Some(t) => t.requests.is_empty(),
            None => true,
        }
    }

    fn can_commit_now(&self, txn: u64) -> bool {
        self.can_start_by_spec(txn) && self.can_notify_txn_maybe_commit(txn)
    }

    fn queue_pending_commit_callback(
        &mut self,
        txn: u64,
        callback: GenericCallback<TxnCompleteMsg>,
    ) {
        self.pending_commit_callbacks
            .entry(txn)
            .or_default()
            .push(callback);
    }

    fn take_pending_commit_callbacks(&mut self, txn: u64) -> Vec<GenericCallback<TxnCompleteMsg>> {
        self.pending_commit_callbacks
            .remove(&txn)
            .unwrap_or_default()
    }

    fn maybe_commit_candidates(&self) -> Vec<u64> {
        let mut candidates: Vec<(u64, u64)> = self
            .txn_info
            .iter()
            .filter_map(|(txn, info)| {
                if !info.live || !self.can_commit_now(*txn) {
                    return None;
                }
                Some((*txn, info.created_seq))
            })
            .collect();
        candidates.sort_by_key(|(_, created_seq)| *created_seq);
        candidates.into_iter().map(|(txn, _)| txn).collect()
    }

    fn finish_transaction(&mut self, txn: u64) {
        if let Some(info) = self.txn_info.get_mut(&txn) {
            info.live = false;
        }
        self.txn_info.remove(&txn);
        self.transactions.remove(&txn);
        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): leak
        // safety net — an upgrade's buffer is normally consumed by
        // `commit_upgrade_buffer` before its TransactionFinished arrives.
        self.pending_upgrade_writes.remove(&txn);
        self.queued_readonly.retain(|queued| *queued != txn);
        self.queued_readwrite.retain(|queued| *queued != txn);
        self.queued_readonly_set.remove(&txn);
        self.queued_readwrite_set.remove(&txn);
        if self.running_readwrite == Some(txn) {
            self.running_readwrite = None;
        }
        self.running_readonly.remove(&txn);
        self.handled_next_unhandled_request_id.remove(&txn);
        self.handled_pending.remove(&txn);
        self.pending_commit_callbacks.remove(&txn);
    }

    fn abort_transaction(&mut self, txn: u64) {
        let key_generator_snapshots = self
            .txn_info
            .get(&txn)
            .filter(|info| info.mode == IndexedDBTxnMode::Readwrite)
            .map(|info| {
                info.scope
                    .iter()
                    .filter_map(|store| store.key_generator_snapshot.as_ref())
                    .map(|snapshot| KeyGeneratorSnapshot {
                        store_name: snapshot.store_name.clone(),
                        current_number: snapshot.current_number,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // https://w3c.github.io/IndexedDB/#key-generator-construct
        // Likewise, if a transaction is aborted, the current number of the
        // key generator for each object store in the transaction’s scope is
        // reverted to the value it had before the transaction was started.
        let res = self.restore_key_generators_after_abort(&key_generator_snapshots);
        debug_assert!(res.is_ok(), "Restoring key generators should not fail.");

        // Keep scheduling metadata until script reports TransactionFinished.
        // https://w3c.github.io/IndexedDB/#transaction-lifetime
        self.transactions.remove(&txn);
        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): an
        // aborted upgrade's shadow writes are discarded — they never reached
        // the engine, so dropping the buffer is the record-level rollback.
        self.pending_upgrade_writes.remove(&txn);
        self.queued_readonly.retain(|queued| *queued != txn);
        self.queued_readwrite.retain(|queued| *queued != txn);
        self.queued_readonly_set.remove(&txn);
        self.queued_readwrite_set.remove(&txn);
        if self.running_readwrite == Some(txn) {
            self.running_readwrite = None;
        }
        self.running_readonly.remove(&txn);
        self.handled_next_unhandled_request_id.remove(&txn);
        self.handled_pending.remove(&txn);
        self.pending_commit_callbacks.remove(&txn);
    }

    fn key_generator_current_number(&self, store_name: &str) -> Option<i64> {
        self.engine.key_generator_current_number(store_name)
    }

    fn set_key_generator_current_number(
        &self,
        store_name: &str,
        current_number: i64,
    ) -> DbResult<()> {
        self.engine
            .set_key_generator_current_number(store_name, current_number)
            .map_err(|err| format!("{err:?}"))
    }

    /// <https://w3c.github.io/IndexedDB/#key-generator-construct>
    fn restore_key_generators_after_abort(
        &self,
        key_generator_snapshots: &[KeyGeneratorSnapshot],
    ) -> DbResult<()> {
        // Likewise, if a transaction is aborted, the current number of the key
        // generator for each object store in the transaction’s scope is
        // reverted to the value it had before the transaction was started.
        for snapshot in key_generator_snapshots {
            self.set_key_generator_current_number(&snapshot.store_name, snapshot.current_number)?;
        }
        Ok(())
    }

    /// <https://w3c.github.io/IndexedDB/#key-generator-construct>
    fn object_store(&self, store_name: &str) -> DbResult<IndexedDBObjectStore> {
        // A key generator has a current number.
        let key_generator_current_number = self.key_generator_current_number(store_name);
        Ok(IndexedDBObjectStore {
            key_path: self.key_path(store_name),
            has_key_generator: key_generator_current_number.is_some(),
            key_generator_current_number,
            indexes: self.indexes(store_name)?,
            name: store_name.to_string(),
        })
    }

    fn object_stores(&self) -> DbResult<Vec<IndexedDBObjectStore>> {
        self.object_store_names()?
            .into_iter()
            .map(|store_name| self.object_store(&store_name))
            .collect()
    }

    fn key_path(&self, store_name: &str) -> Option<KeyPath> {
        self.engine.key_path(store_name)
    }

    fn object_store_names(&self) -> DbResult<Vec<String>> {
        self.engine
            .object_store_names()
            .map_err(|err| format!("{err:?}"))
    }

    fn indexes(&self, store_name: &str) -> DbResult<Vec<IndexedDBIndex>> {
        self.engine
            .indexes(store_name)
            .map_err(|err| format!("{err:?}"))
    }

    fn create_index(
        &self,
        store_name: &str,
        index_name: String,
        key_path: KeyPath,
        unique: bool,
        multi_entry: bool,
    ) -> DbResult<CreateObjectResult> {
        self.engine
            .create_index(store_name, index_name, key_path, unique, multi_entry)
            .map_err(|err| format!("{err:?}"))
    }

    fn delete_index(&self, store_name: &str, index_name: String) -> DbResult<()> {
        self.engine
            .delete_index(store_name, index_name)
            .map_err(|err| format!("{err:?}"))
    }

    fn create_object_store(
        &mut self,
        store_name: &str,
        key_path: Option<KeyPath>,
        auto_increment: bool,
    ) -> DbResult<CreateObjectResult> {
        self.engine
            .create_store(store_name, key_path, auto_increment)
            .map_err(|err| format!("{err:?}"))
    }

    fn delete_object_store(&mut self, store_name: &str) -> DbResult<()> {
        // https://www.w3.org/TR/IndexedDB-3/#dom-idbdatabase-deleteobjectstore
        // Step 7. Destroy store.
        let indexes = self
            .engine
            .indexes(store_name)
            .map_err(|err| format!("{err:?}"))?;
        for index in indexes {
            self.engine
                .delete_index(store_name, index.name)
                .map_err(|err| format!("{err:?}"))?;
        }
        self.engine
            .delete_store(store_name)
            .map_err(|err| format!("{err:?}"))
    }

    /// Restores the object-store metadata snapshot captured before an aborted
    /// upgrade: remove stores/indexes created during the aborted upgrade, create
    /// stores/indexes that existed before it, and reset key-generator current
    /// numbers for stores that use key generators.
    fn restore_object_stores(&mut self, object_stores: &[IndexedDBObjectStore]) -> DbResult<()> {
        let mut current_store_names: HashSet<String> =
            self.object_store_names()?.into_iter().collect();
        let expected_store_names: HashSet<String> = object_stores
            .iter()
            .map(|store| store.name.clone())
            .collect();

        for store_name in current_store_names.clone() {
            if !expected_store_names.contains(&store_name) {
                self.delete_object_store(&store_name)?;
                current_store_names.remove(&store_name);
            }
        }

        for store in object_stores {
            if !current_store_names.contains(&store.name) {
                // https://w3c.github.io/IndexedDB/#key-generator-construct
                // The initial value of a key generator’s current number is 1,
                // set when the associated object store is created.
                self.create_object_store(
                    &store.name,
                    store.key_path.clone(),
                    store.has_key_generator,
                )?;
                current_store_names.insert(store.name.clone());
            }

            if let Some(current_number) = store.key_generator_current_number {
                // https://w3c.github.io/IndexedDB/#key-generator-construct
                // Modifying a key generator’s current number is considered part
                // of a database operation. This means that if the operation
                // fails and the operation is reverted, the current number is
                // reverted to the value it had before the operation started.
                // Likewise, if a transaction is aborted, the current number of
                // the key generator for each object store in the transaction’s
                // scope is reverted to the value it had before the transaction
                // was started.
                self.set_key_generator_current_number(&store.name, current_number)?;
            }

            let mut current_index_names: HashSet<String> = self
                .indexes(&store.name)?
                .into_iter()
                .map(|index| index.name)
                .collect();
            let expected_index_names: HashSet<String> = store
                .indexes
                .iter()
                .map(|index| index.name.clone())
                .collect();

            for index_name in current_index_names.clone() {
                if !expected_index_names.contains(&index_name) {
                    self.delete_index(&store.name, index_name.clone())?;
                    current_index_names.remove(&index_name);
                }
            }

            for index in &store.indexes {
                if !current_index_names.contains(&index.name) {
                    self.create_index(
                        &store.name,
                        index.name.clone(),
                        index.key_path.clone(),
                        index.unique,
                        index.multi_entry,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn version(&self) -> Result<u64, E::Error> {
        self.engine.version()
    }

    fn set_version(&mut self, version: u64) -> DbResult<()> {
        self.engine
            .set_version(version)
            .map_err(|err| format!("{err:?}"))
    }

    /// <https://w3c.github.io/IndexedDB/#abort-a-transaction>
    ///
    /// > When a transaction is aborted the implementation must undo (roll back) any changes that
    /// > were made to the database during that transaction.
    ///
    /// This only aborts the transaction if one was previously queued by adding an abort
    /// callback to [`Self::pending_abort_callbacks`].
    ///
    /// TODO: implement the abort algorithm and rollback for the engine.
    fn abort(&mut self, origin: &ImmutableOrigin, database_name: &str, transaction: u64) -> bool {
        let message = || TxnCompleteMsg {
            origin: origin.clone(),
            db_name: database_name.into(),
            txn: transaction,
            result: Err(BackendError::Abort),
        };

        let Some(abort_callbacks) = self.pending_abort_callbacks.remove(&transaction) else {
            return false;
        };
        if abort_callbacks.is_empty() {
            return false;
        }

        for callback in self
            .take_pending_commit_callbacks(transaction)
            .into_iter()
            .chain(abort_callbacks)
        {
            if callback.send(message()).is_err() {
                error!(
                    "Failed to send deferred abort completion for \
                    database '{database_name}' transaction {transaction}.",
                );
            }
        }

        self.abort_transaction(transaction);
        self.schedule_transactions(origin.clone(), database_name);
        true
    }
}

fn backend_error_from_sqlite_error(err: RusqliteError) -> BackendError {
    if is_sqlite_disk_full_error(&err) {
        BackendError::QuotaExceeded
    } else {
        BackendError::DbErr(format!("{err:?}"))
    }
}

/// Creates the per-request completion callback used by
/// [`IndexedDBEnvironment::commit_upgrade_buffer`], reporting the store name
/// with the outcome so a flush failure names the affected store.
fn upgrade_put_item_callback(
    store_name: String,
    sender: mpsc::Sender<(String, Result<(), String>)>,
) -> GenericCallback<BackendResult<PutItemResult>> {
    let callback = GenericCallback::new(
        move |result: Result<
            BackendResult<PutItemResult>,
            servo_base::generic_channel::SendError,
        >| {
            let outcome = match result {
                // `CannotOverwrite` is unreachable here: replayed writes were
                // presence-checked at intercept time and always set
                // `should_overwrite: true`, so the engine cannot refuse.
                Ok(Ok(PutItemResult::Key(_))) => Ok(()),
                Ok(Ok(PutItemResult::CannotOverwrite)) => Err(
                    "put replay unexpectedly refused (cannot-overwrite)".to_owned(),
                ),
                Ok(Err(backend_error)) => Err(backend_error.to_string()),
                Err(send_error) => Err(format!("{send_error:?}")),
            };
            let _ = sender.send((store_name.clone(), outcome));
        },
    );
    match callback {
        Ok(callback) => callback,
        Err(error) => panic!("Failed to create IndexedDB upgrade put callback: {error:?}"),
    }
}

fn upgrade_write_callback(
    store_name: String,
    sender: mpsc::Sender<(String, Result<(), String>)>,
) -> GenericCallback<BackendResult<()>> {
    let callback = GenericCallback::new(
        move |result: Result<BackendResult<()>, servo_base::generic_channel::SendError>| {
            let outcome = match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(backend_error)) => Err(backend_error.to_string()),
                Err(send_error) => Err(format!("{send_error:?}")),
            };
            let _ = sender.send((store_name.clone(), outcome));
        },
    );
    match callback {
        Ok(callback) => callback,
        // Without a callback the engine's completion signal has nowhere to go
        // and the manager would block on the result channel forever; fail loud
        // instead of wedging the IndexedDB thread.
        Err(error) => panic!("Failed to create IndexedDB upgrade write callback: {error:?}"),
    }
}

/// Total order over IndexedDB keys for the merged view's sorted-record
/// invariants. Keys cannot be NaN (`convert a value to a key` rejects NaN
/// before a key exists — script/dom/indexeddb/key.rs:243), so `partial_cmp`
/// returning `None` is unreachable; folding it to `Equal` merely keeps the
/// comparator total for defensive robustness.
fn total_key_cmp(a: &IndexedDBKeyType, b: &IndexedDBKeyType) -> Ordering {
    a.partial_cmp(b).unwrap_or(Ordering::Equal)
}

/// Rough heap size of a buffered key. Only feeds the shadow buffer's byte
/// budget, where an upper bound is enough and exactness is not a goal.
fn pending_key_bytes(key: &IndexedDBKeyType) -> u64 {
    match key {
        IndexedDBKeyType::Number(_) | IndexedDBKeyType::Date(_) => 8,
        IndexedDBKeyType::String(string) => string.len() as u64 * 2, // UTF-16 upper bound
        IndexedDBKeyType::Binary(bytes) => bytes.len() as u64,
        IndexedDBKeyType::Array(keys) => keys.iter().map(pending_key_bytes).sum(),
    }
}

/// Rough heap size of one buffered write; see [`pending_key_bytes`].
fn pending_write_bytes(write: &PendingWrite) -> u64 {
    match write {
        PendingWrite::Clear => 0,
        PendingWrite::Put { key, value, .. } => pending_key_bytes(key) + value.len() as u64,
        PendingWrite::Delete { key_range } => {
            key_range
                .lower
                .as_ref()
                .map(pending_key_bytes)
                .unwrap_or(0) +
                key_range
                    .upper
                    .as_ref()
                    .map(pending_key_bytes)
                    .unwrap_or(0)
        },
    }
}

/// <https://w3c.github.io/IndexedDB/#request-open-request>
/// Used here to implement the
/// <https://w3c.github.io/IndexedDB/#connection-queue>
#[derive(MallocSizeOf)]
enum OpenRequest {
    Open {
        /// The callback used to send a result to script.
        sender: GenericCallback<ConnectionMsg>,

        /// The name of the database.
        db_name: String,

        /// Optionnaly, a requested db version.
        /// Note: when the open algorithm starts, this will be mutated and set to something as per the algo.
        version: Option<u64>,

        /// <https://w3c.github.io/IndexedDB/#request-processed-flag>
        processed: bool,

        /// Optionally, a version pending upgrade.
        pending_upgrade: Option<VersionUpgrade>,

        /// This request is pending on these connections to close.
        pending_close: HashSet<Uuid>,

        /// This request is pending on these connections to fire a versionchange event.
        /// Note: This starts as equal to `pending_close`, but when all events have fired,
        /// not all connections need to have closed, in which case the `blocked` event
        /// is fired on this request.
        pending_versionchange: HashSet<Uuid>,

        id: Uuid,

        /// <https://storage.spec.whatwg.org/#storage-proxy-map>
        proxy_map: StorageProxyMap,
    },
    Delete {
        /// The callback used to send a result to script.
        sender: GenericCallback<BackendResult<u64>>,

        _origin: ImmutableOrigin,

        /// The name of the database.
        /// Note: will be used when the full spec is implemented.
        db_name: String,

        /// <https://w3c.github.io/IndexedDB/#request-processed-flag>
        processed: bool,

        id: Uuid,

        /// <https://storage.spec.whatwg.org/#storage-proxy-map>
        proxy_map: StorageProxyMap,
    },
}

impl OpenRequest {
    fn get_id(&self) -> Uuid {
        let id = match self {
            OpenRequest::Open {
                sender: _,
                db_name: _,
                version: _,
                processed: _,
                pending_upgrade: _,
                pending_close: _,
                pending_versionchange: _,
                id,
                proxy_map: _,
            } => id,
            OpenRequest::Delete {
                sender: _,
                _origin: _,
                db_name: _,
                processed: _,
                proxy_map: _,
                id,
            } => id,
        };
        *id
    }

    fn is_open(&self) -> bool {
        match self {
            OpenRequest::Open {
                sender: _,
                db_name: _,
                version: _,
                processed: _,
                pending_upgrade: _,
                pending_close: _,
                pending_versionchange: _,
                id: _,
                proxy_map: _,
            } => true,
            OpenRequest::Delete {
                sender: _,
                _origin: _,
                db_name: _,
                processed: _,
                proxy_map: _,
                id: _,
            } => false,
        }
    }

    /// An open request remains pending until it has been processed,
    /// and while waiting on upgrade completion or other connections.
    fn is_pending(&self) -> bool {
        match self {
            OpenRequest::Open {
                sender: _,
                db_name: _,
                version: _,
                processed,
                pending_upgrade,
                pending_close,
                pending_versionchange,
                id: _,
                proxy_map: _,
            } => {
                !processed ||
                    pending_upgrade.is_some() ||
                    !pending_close.is_empty() ||
                    !pending_versionchange.is_empty()
            },
            OpenRequest::Delete {
                sender: _,
                _origin: _,
                db_name: _,
                processed,
                id: _,
                proxy_map: _,
            } => !processed,
        }
    }

    /// Abort the open request,
    /// optionally returning a version to revert to.
    ///
    /// BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): takes the
    /// pending upgrade out (`&mut self`) instead of cloning it — the shadow
    /// write buffer types held downstream made the clone a footgun, and the
    /// aborted request is dead afterwards anyway.
    fn abort(&mut self) -> Option<VersionUpgrade> {
        match self {
            OpenRequest::Open {
                sender,
                db_name,
                version: _,
                processed: _,
                pending_close: _,
                pending_versionchange: _,
                pending_upgrade,
                id,
                proxy_map: _,
            } => {
                if sender
                    .send(ConnectionMsg::AbortError {
                        name: db_name.clone(),
                        id: *id,
                    })
                    .is_err()
                {
                    error!("Failed to send ConnectionMsg::Connection to script.");
                };
                pending_upgrade.take()
            },
            OpenRequest::Delete {
                sender,
                _origin: _,
                db_name: _,
                processed: _,
                id: _,
                proxy_map: _,
            } => {
                if sender.send(Err(BackendError::DbNotFound)).is_err() {
                    error!("Failed to send result of database delete to script.");
                };
                None
            },
        }
    }
}

// BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): the shadow write
// buffer types. An upgrade transaction's record writes accumulate here on the
// manager thread; commit replays them into the engine as one batch, abort
// drops them with zero IO.
/// One buffered record write of a running upgrade transaction, in script's
/// request order.
#[derive(Clone, MallocSizeOf)]
enum PendingWrite {
    /// `clear()`: subsumes every write buffered before it for the store.
    Clear,
    Put {
        key: IndexedDBKeyType,
        value: Vec<u8>,
        /// Buffered generator advance replayed per-put at commit (`Some` only
        /// for keyless puts, mirroring the engine's `put_item` contract).
        key_generator_current_number: Option<i64>,
    },
    /// `delete(...)`: every visible record inside the range stops existing.
    Delete { key_range: IndexedDBKeyRange },
}

/// Buffered state of one object store inside a running upgrade transaction.
#[derive(Default, MallocSizeOf)]
struct PendingStoreWrites {
    /// Buffered writes in script's request order.
    ops: Vec<PendingWrite>,
    /// Mirrors the store's key generator current number as the upgrade sees
    /// it. `Some` is authoritative for keyless puts; `None` on a buffered
    /// entry means the store has no key generator. It survives a buffered
    /// `Clear` — spec `clear()` removes records only, it does not reset the
    /// key generator. <https://w3c.github.io/IndexedDB/#key-generator-construct>
    generator_current: Option<i64>,
    /// Rough heap size of `ops`; see [`pending_write_bytes`].
    bytes: u64,
}

/// The shadow write buffer of one running upgrade transaction.
#[derive(Default, MallocSizeOf)]
struct UpgradeWriteBuffer {
    stores: BTreeMap<String, PendingStoreWrites>,
    /// Stores this upgrade created (their schema write passes through to the
    /// engine queue asynchronously, so they are empty engine-side until
    /// commit and must not be queried there). Consumed by presence checks
    /// and merged reads as an O(1) empty answer.
    engine_empty_stores: BTreeSet<String>,
    /// Rough total heap size of all buffered writes; see [`pending_write_bytes`].
    bytes: u64,
}

/// Buffered visibility of one key under an upgrade transaction's shadow
/// writes.
enum BufferedKeyLookup {
    /// A buffered put makes the key visible with this value.
    Value(Vec<u8>),
    /// A buffered delete/clear makes the key invisible.
    Missing,
    /// No buffered write touches the key; the engine state decides.
    NotBuffered,
}

#[derive(MallocSizeOf)]
struct VersionUpgrade {
    old: u64,
    new: u64,
    transaction: u64,
    object_stores: Vec<IndexedDBObjectStore>,
}

/// <https://w3c.github.io/IndexedDB/#connection>
#[derive(MallocSizeOf)]
struct Connection {
    /// <https://w3c.github.io/IndexedDB/#connection-close-pending-flag>
    close_pending: bool,

    /// The callback used to send a result to script.
    sender: GenericCallback<ConnectionMsg>,
}

struct IndexedDBManager {
    port: GenericReceiver<IndexedDBThreadMsg>,
    manager_sender: GenericSender<IndexedDBThreadMsg>,
    databases: HashMap<IndexedDBDescription, IndexedDBEnvironment<SqliteEngine>>,
    thread_pool: Arc<ThreadPool>,

    /// A global counter to produce unique transaction ids.
    /// TODO: remove once db connections lifecyle is managed.
    /// A global counter is only necessary because of how deleting a db currently
    /// does not wait for connection to close and transactions to finish.
    serial_number_counter: u64,

    /// <https://w3c.github.io/IndexedDB/#connection-queue>
    connection_queues: HashMap<IndexedDBDescription, VecDeque<OpenRequest>>,

    /// <https://w3c.github.io/IndexedDB/#connection>
    connections: HashMap<IndexedDBDescription, HashMap<Uuid, Connection>>,
}

impl IndexedDBManager {
    fn new(
        port: GenericReceiver<IndexedDBThreadMsg>,
        manager_sender: GenericSender<IndexedDBThreadMsg>,
    ) -> IndexedDBManager {
        debug!("New indexedDBManager");

        IndexedDBManager {
            port,
            manager_sender,
            databases: HashMap::new(),
            thread_pool: ThreadPool::global(),
            serial_number_counter: 0,
            connection_queues: Default::default(),
            connections: Default::default(),
        }
    }
}

impl IndexedDBManager {
    fn start(&mut self) {
        loop {
            // FIXME:(arihant2math) No message *most likely* means that
            // the ipc sender has been dropped, so we break the look
            let message = match self.port.recv() {
                Ok(msg) => msg,
                Err(ReceiveError::Disconnected) => {
                    break;
                },
                Err(e) => {
                    warn!("Error in IndexedDB thread: {e:?}");
                    continue;
                },
            };
            match message {
                IndexedDBThreadMsg::Sync(SyncOperation::Exit(sender)) => {
                    let _ = sender.send(());
                    break;
                },
                IndexedDBThreadMsg::Sync(operation) => {
                    self.handle_sync_operation(operation);
                },
                IndexedDBThreadMsg::Async(
                    origin,
                    db_name,
                    store_name,
                    txn,
                    _request_id,
                    mode,
                    operation,
                ) => {
                    if let Some(db) = self.get_database_mut(origin.clone(), db_name.clone()) {
                        // Queues an operation for a transaction without starting it
                        db.queue_operation(&store_name, txn, mode, operation);
                        db.schedule_transactions(origin, &db_name);
                    }
                },
                IndexedDBThreadMsg::EngineTxnBatchComplete {
                    origin,
                    db_name,
                    txn,
                } => {
                    let should_notify = self
                        .get_database_mut(origin.clone(), db_name.clone())
                        .is_some_and(|db| {
                            // Decide which running flag to clear based on txn mode.
                            let mode = db.transactions.get(&txn).map(|t| t.mode.clone());

                            match mode {
                                Some(IndexedDBTxnMode::Readonly) => {
                                    db.running_readonly.remove(&txn);
                                },
                                Some(_) if db.running_readwrite == Some(txn) => {
                                    db.running_readwrite = None;
                                },
                                Some(_) => {},
                                None => {
                                    // txn might have been aborted/removed; nothing to clear
                                },
                            }

                            if db.abort(&origin, &db_name, txn) {
                                return false;
                            }

                            // If more requests were queued while this batch was running,
                            // schedule again now.
                            db.schedule_transactions(origin.clone(), &db_name);
                            db.can_notify_txn_maybe_commit(txn)
                        });

                    if should_notify {
                        self.handle_sync_operation(SyncOperation::TxnMaybeCommit {
                            origin,
                            db_name,
                            txn,
                        });
                    }
                },
                IndexedDBThreadMsg::CollectMemoryReport(sender) => {
                    let reports = self.collect_memory_reports();
                    sender.send(ProcessReports::new(reports));
                },
                IndexedDBThreadMsg::AsyncSchemaOperation {
                    origin,
                    database_name,
                    store_name,
                    operation,
                    transaction_serial_number,
                } => {
                    if let Some(database) =
                        self.get_database_mut(origin.clone(), database_name.clone())
                    {
                        // Queues an operation for a transaction without starting it
                        database.queue_operation(
                            &store_name,
                            transaction_serial_number,
                            IndexedDBTxnMode::Versionchange,
                            AsyncOperation::Schema(operation),
                        );
                        database.schedule_transactions(origin, &database_name);
                    } else {
                        operation.notify_error(BackendError::DbNotFound);
                    }
                },
            }
        }
    }

    fn dispatch_txn_maybe_commit(&self, origin: ImmutableOrigin, db_name: String, txn: u64) {
        let key = IndexedDBDescription {
            origin,
            name: db_name.clone(),
        };
        let Some(connections) = self.connections.get(&key) else {
            return;
        };
        for connection in connections.values() {
            if connection.close_pending {
                continue;
            }
            let _ = connection.sender.send(ConnectionMsg::TxnMaybeCommit {
                db_name: db_name.clone(),
                txn,
            });
        }
    }

    fn handle_txn_maybe_commit(&mut self, origin: ImmutableOrigin, db_name: String, txn: u64) {
        let key = IndexedDBDescription {
            origin: origin.clone(),
            name: db_name.clone(),
        };
        let callbacks = {
            let Some(db) = self.databases.get_mut(&key) else {
                return;
            };
            if !db.can_commit_now(txn) {
                return;
            }
            db.take_pending_commit_callbacks(txn)
        };

        if callbacks.is_empty() {
            self.dispatch_txn_maybe_commit(origin, db_name, txn);
            return;
        }

        for callback in callbacks {
            if callback
                .send(TxnCompleteMsg {
                    origin: origin.clone(),
                    db_name: db_name.clone(),
                    txn,
                    result: Ok(()),
                })
                .is_err()
            {
                error!(
                    "Failed to send deferred commit completion for db '{}' txn {}.",
                    db_name, txn
                );
            }
        }
    }

    /// Handle when an upgrade transaction finishes in script.
    fn handle_upgrade_transaction_finished(
        &mut self,
        name: String,
        origin: ImmutableOrigin,
        txn: u64,
        committed: bool,
    ) {
        let key = IndexedDBDescription {
            name: name.clone(),
            origin: origin.clone(),
        };

        if committed {
            let Some(queue) = self.connection_queues.get_mut(&key) else {
                return debug_assert!(false, "A connection queue should exist.");
            };
            let Some(front) = queue.front() else {
                return debug_assert!(false, "A pending open request should exist.");
            };
            let OpenRequest::Open {
                pending_upgrade: Some(pending_upgrade),
                ..
            } = front
            else {
                return;
            };
            if pending_upgrade.transaction != txn {
                return;
            }

            let Some(open_request) = queue.pop_front() else {
                return;
            };
            let OpenRequest::Open {
                sender,
                db_name,
                version: _,
                processed: _,
                pending_upgrade: Some(pending_upgrade),
                pending_close: _,
                pending_versionchange: _,
                id,
                proxy_map,
            } = open_request
            else {
                return;
            };

            // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): the
            // upgrade's record writes live in the shadow write buffer, so
            // they must land in the engine before the connection is handed
            // back to script. Fail-closed: a failed flush is reported as a
            // database error (after a best-effort revert), never as a green
            // upgrade.
            let flush_result = match self.databases.get_mut(&key) {
                Some(db) => db.commit_upgrade_buffer(txn),
                None => {
                    debug_assert!(false, "Db should have been opened.");
                    Err(String::from("database vanished before the upgrade commit"))
                },
            };
            if let Err(flush_error) = flush_result {
                error!(
                    "Failed to land the shadow write buffer of database '{}' at upgrade commit: \
                     {flush_error}",
                    key.name
                );
                self.revert_aborted_upgrade(&key, &pending_upgrade, &proxy_map);
                if sender
                    .send(ConnectionMsg::DatabaseError {
                        id,
                        name: db_name.clone(),
                        error: BackendError::DbErr(format!(
                            "upgrade transaction commit failed: {flush_error}"
                        )),
                    })
                    .is_err()
                {
                    error!("Failed to send ConnectionMsg::DatabaseError to script.");
                };
                self.remove_connection(&key, &id);
                self.advance_connection_queue(key);
                return;
            }

            let VersionUpgrade { new, .. } = pending_upgrade;
            let object_store_names = self
                .databases
                .get(&key)
                .and_then(|db| db.object_store_names().ok())
                .unwrap_or_default();
            if sender
                .send(ConnectionMsg::Connection {
                    id,
                    name: db_name,
                    version: new,
                    upgraded: true,
                    object_store_names,
                })
                .is_err()
            {
                error!("Failed to send ConnectionMsg::Connection to script.");
            };

            self.advance_connection_queue(key);
            return;
        }

        let (request_id, proxy_map) = {
            let Some(queue) = self.connection_queues.get_mut(&key) else {
                return debug_assert!(false, "A connection queue should exist.");
            };
            let Some(front) = queue.front() else {
                return debug_assert!(false, "A pending open request should exist.");
            };
            let OpenRequest::Open {
                pending_upgrade: Some(pending_upgrade),
                id,
                proxy_map,
                ..
            } = front
            else {
                return;
            };
            if pending_upgrade.transaction != txn {
                return;
            }
            (*id, (*proxy_map).clone())
        };

        self.abort_pending_upgrade(name, request_id, origin, &proxy_map);
    }

    /// Run the next open request in the queue.
    fn advance_connection_queue(&mut self, key: IndexedDBDescription) {
        loop {
            let is_open = {
                let Some(queue) = self.connection_queues.get_mut(&key) else {
                    return;
                };
                if queue.is_empty() {
                    return;
                }
                queue.front().expect("Queue is not empty.").is_open()
            };

            if is_open {
                self.open_database(key.clone());
            } else {
                self.delete_database(key.clone());
            }

            let was_pruned = self.maybe_remove_front_from_queue(&key);

            if !was_pruned {
                // Note: requests to delete a database are, at this point in the implementation,
                // done in one step; so we can continue on to the next request.
                // Request to open a connection consists of multiple async steps, so we must break if
                // it is still pending.
                break;
            }
        }
    }

    /// Remove the record at the front if it is not pending.
    fn maybe_remove_front_from_queue(&mut self, key: &IndexedDBDescription) -> bool {
        let (is_empty, was_pruned) = {
            let Some(queue) = self.connection_queues.get_mut(key) else {
                debug_assert!(false, "A connection queue should exist.");
                return false;
            };
            let mut pruned = false;
            let front_is_pending = queue.front().map(|record| record.is_pending());
            if let Some(is_pending) = front_is_pending &&
                !is_pending
            {
                queue.pop_front().expect("Queue has a non-pending item.");
                pruned = true
            }
            (queue.is_empty(), pruned)
        };
        if is_empty {
            self.connection_queues.remove(key);
        }
        was_pruned
    }

    fn remove_connection(&mut self, key: &IndexedDBDescription, id: &Uuid) {
        let is_empty = {
            let Some(connections) = self.connections.get_mut(key) else {
                return debug!("Connection already removed.");
            };
            connections.remove(id);
            connections.is_empty()
        };

        if is_empty {
            self.connections.remove(key);
        }
    }

    /// Revert the backing database state after aborting an upgrade transaction.
    ///
    /// <https://w3c.github.io/IndexedDB/#abort-an-upgrade-transaction>
    /// IndexedDB §5.8 step 3 restores the previous version, or `0` if the database
    /// was newly created. Step 4 restores the previous object store set, or the
    /// empty set if the database was newly created. Servo eagerly creates the
    /// backing database with version `0` and no stores during open, so aborting
    /// that first upgrade must roll back to the pre-creation state by deleting the
    /// placeholder backing store entirely.
    ///
    /// Related: <https://github.com/servo/servo/pull/42998>
    fn revert_aborted_upgrade(
        &mut self,
        key: &IndexedDBDescription,
        upgrade: &VersionUpgrade,
        proxy_map: &StorageProxyMap,
    ) {
        if upgrade.old == 0 {
            if let Some(db) = self.databases.remove(key) {
                // Note: ensure db is dropped before deleting directory,
                // to get around windows file locks.
                drop(db);
                let response = proxy_map
                    .handle
                    .delete_database(proxy_map.bottle_id, key.name.clone())
                    .recv();
                if response.is_err() {
                    error!("Failed to communicate with client storage.");
                    return;
                }
                if response.unwrap().is_err() {
                    error!("Failed to delete database {:?}", key.name);
                }
            }
            return;
        }

        let Some(db) = self.databases.get_mut(key) else {
            return debug_assert!(false, "Db should have been created");
        };
        let res = db.set_version(upgrade.old);
        debug_assert!(res.is_ok(), "Setting a db version should not fail.");

        // Step 4. Set connection’s object store set to the set of object stores
        // in database if database previously existed, or the empty set if
        // database was newly created.
        let res = db.restore_object_stores(&upgrade.object_stores);
        debug_assert!(res.is_ok(), "Restoring object stores should not fail.");

        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): the
        // record-level rollback is a zero-IO buffer discard — the upgrade's
        // writes never reached the engine, so dropping the shadow buffer is
        // the whole rollback. (The `old == 0` branch above drops the whole
        // database entry, buffer included.)
        db.discard_upgrade_buffer(upgrade.transaction);
    }

    /// Aborting the current upgrade for an origin.
    /// <https://w3c.github.io/IndexedDB/#abort-an-upgrade-transaction>
    fn abort_pending_upgrade(
        &mut self,
        name: String,
        id: Uuid,
        origin: ImmutableOrigin,
        proxy_map: &StorageProxyMap,
    ) {
        let key = IndexedDBDescription { name, origin };
        let upgrade = {
            let Some(queue) = self.connection_queues.get_mut(&key) else {
                return debug_assert!(
                    false,
                    "There should be a connection queue for the aborted upgrade."
                );
            };
            let Some(mut open_request) = queue.pop_front() else {
                return debug_assert!(false, "There should be an open request to upgrade.");
            };
            if open_request.get_id() != id {
                return debug_assert!(
                    false,
                    "Open request to abort should be at the head of the queue."
                );
            }
            open_request.abort()
        };
        if let Some(upgrade) = upgrade {
            self.revert_aborted_upgrade(&key, &upgrade, proxy_map);
        }

        self.remove_connection(&key, &id);

        self.advance_connection_queue(key);
    }

    /// Aborting all upgrades for an origin
    // https://w3c.github.io/IndexedDB/#abort-an-upgrade-transaction
    /// Note: this only reverts the version at this point.
    fn abort_pending_upgrades(
        &mut self,
        pending_upgrades: HashMap<String, HashSet<Uuid>>,
        origin: ImmutableOrigin,
        proxy_map: StorageProxyMap,
    ) {
        for (name, ids) in pending_upgrades.into_iter() {
            let mut upgrade_to_revert: Option<VersionUpgrade> = None;
            let key = IndexedDBDescription {
                name: name.clone(),
                origin: origin.clone(),
            };
            for id in ids.iter() {
                self.remove_connection(&key, id);
            }
            {
                let is_empty = {
                    let Some(queue) = self.connection_queues.get_mut(&key) else {
                        continue;
                    };
                    queue.retain_mut(|open_request| {
                        if ids.contains(&open_request.get_id()) {
                            let upgrade = open_request.abort();
                            if upgrade_to_revert.is_none() &&
                                let Some(upgrade) = upgrade
                            {
                                upgrade_to_revert = Some(upgrade);
                            }
                            false
                        } else {
                            true
                        }
                    });
                    queue.is_empty()
                };
                if is_empty {
                    self.connection_queues.remove(&key);
                }
            }
            if let Some(upgrade) = upgrade_to_revert {
                self.revert_aborted_upgrade(&key, &upgrade, &proxy_map);
            }
        }
    }

    /// <https://w3c.github.io/IndexedDB/#open-a-database-connection>
    fn open_a_database_connection(
        &mut self,
        sender: GenericCallback<ConnectionMsg>,
        origin: ImmutableOrigin,
        db_name: String,
        version: Option<u64>,
        id: Uuid,
        proxy_map: StorageProxyMap,
    ) {
        let key = IndexedDBDescription {
            name: db_name.clone(),
            origin,
        };
        let open_request = OpenRequest::Open {
            sender,
            db_name,
            version,
            processed: false,
            pending_close: Default::default(),
            pending_versionchange: Default::default(),
            pending_upgrade: None,
            id,
            proxy_map,
        };
        let should_continue = {
            // Step 1: Let queue be the connection queue for storageKey and name.
            let queue = self.connection_queues.entry(key.clone()).or_default();

            // Step 2: Add request to queue.
            queue.push_back(open_request);
            queue.len() == 1
        };

        // Step 3: Wait until all previous requests in queue have been processed.
        if should_continue {
            self.open_database(key.clone());
            self.maybe_remove_front_from_queue(&key);
        }
    }

    fn get_database(
        &self,
        origin: ImmutableOrigin,
        db_name: String,
    ) -> Option<&IndexedDBEnvironment<SqliteEngine>> {
        let idb_description = IndexedDBDescription {
            origin,
            name: db_name,
        };

        self.databases.get(&idb_description)
    }

    fn get_database_mut(
        &mut self,
        origin: ImmutableOrigin,
        db_name: String,
    ) -> Option<&mut IndexedDBEnvironment<SqliteEngine>> {
        let idb_description = IndexedDBDescription {
            origin,
            name: db_name,
        };

        self.databases.get_mut(&idb_description)
    }

    /// <https://w3c.github.io/IndexedDB/#upgrade-a-database>
    /// To upgrade a database with connection (a connection),
    /// a new version, and a request, run these steps:
    fn upgrade_database(&mut self, key: IndexedDBDescription, new_version: u64) {
        let Some(queue) = self.connection_queues.get_mut(&key) else {
            return debug_assert!(false, "A connection queue should exist.");
        };
        let Some(open_request) = queue.front_mut() else {
            return debug_assert!(false, "An open request should be in the queue.");
        };
        let OpenRequest::Open {
            sender,
            db_name,
            version: _,
            processed,
            id,
            pending_close: _,
            pending_versionchange: _,
            pending_upgrade,
            proxy_map: _,
        } = open_request
        else {
            return;
        };

        // Step 1: Let db be connection’s database.
        let db = self
            .databases
            .get_mut(&key)
            .expect("Db should have been opened.");

        // Step 2: Let transaction be a new upgrade transaction with connection used as connection.
        let transaction = self.serial_number_counter;
        self.serial_number_counter += 1;

        // Step 3: Set transaction’s scope to connection’s object store set.
        let scope = db
            .object_store_names()
            .expect("Fetching object store names should not fail.");

        // Step 4: Set db’s upgrade transaction to transaction.
        // Backend tracks the active upgrade transaction in `pending_upgrade` below.
        db.register_transaction(transaction, IndexedDBTxnMode::Versionchange, scope.clone());

        // Step 5: Set transaction’s state to inactive.
        // Step 6: Start transaction.
        // Backend transactions are started by the scheduler when requests are queued;
        // newly created upgrade transactions are therefore initially inactive.

        // Step 7: Let old version be db’s version.
        let old_version = db.version().expect("DB should have a version.");
        let object_stores = db
            .object_stores()
            .expect("Fetching object stores should not fail.");

        // Step 8: Set db’s version to version. This change is considered part of the
        // transaction, and so if the transaction is aborted, this change is reverted.
        db.set_version(new_version)
            .expect("Setting the version should not fail");

        // Step 9: Set request’s processed flag to true.
        *processed = true;
        // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer): no
        // pre-upgrade record snapshot is captured here anymore — the running
        // upgrade's writes accumulate in its shadow write buffer
        // (`pending_upgrade_writes`, created by `register_transaction`), and
        // commit/abort resolve the buffer.
        let _ = pending_upgrade.insert(VersionUpgrade {
            old: old_version,
            new: new_version,
            transaction,
            object_stores,
        });

        // Step 10: Queue a database task to run these steps.
        if sender
            .send(ConnectionMsg::Upgrade {
                id: *id,
                name: db_name.clone(),
                version: new_version,
                old_version,
                transaction,
                object_store_names: scope,
            })
            .is_err()
        {
            error!("Couldn't queue task for indexeddb upgrade event.");
        }

        // Step 11: Wait for transaction to finish.
        // Queue progression remains blocked while `pending_upgrade` is set.
    }

    /// <https://w3c.github.io/IndexedDB/#open-a-database-connection>
    fn handle_version_change_done(
        &mut self,
        name: String,
        from_id: Uuid,
        old_version: u64,
        origin: ImmutableOrigin,
    ) {
        let key = IndexedDBDescription {
            name: name.clone(),
            origin,
        };
        let (can_upgrade, version) = {
            let Some(queue) = self.connection_queues.get_mut(&key) else {
                return debug_assert!(false, "A connection queue should exist.");
            };
            let Some(open_request) = queue.front_mut() else {
                return debug_assert!(false, "An open request should be in the queue.");
            };
            let OpenRequest::Open {
                sender,
                db_name: _,
                version,
                id,
                pending_upgrade: _,
                processed: _,
                pending_versionchange,
                pending_close,
                proxy_map: _,
            } = open_request
            else {
                return debug_assert!(
                    false,
                    "An request to open a connection should be in the queue."
                );
            };
            debug_assert!(
                pending_versionchange.contains(&from_id),
                "The open request should be pending on the versionchange event for the connection sending the message."
            );

            pending_versionchange.remove(&from_id);

            // Step 10.3: Wait for all of the events to be fired.
            if !pending_versionchange.is_empty() {
                return;
            }

            let Some(version) = *version else {
                return debug_assert!(
                    false,
                    "An upgrade version should have been determined by now."
                );
            };

            // Step 10.4: If any of the connections in openConnections are still not closed,
            // queue a database task to fire a version change event named blocked
            // at request with db’s version and version.
            if !pending_close.is_empty() &&
                sender
                    .send(ConnectionMsg::Blocked {
                        name,
                        id: *id,
                        version,
                        old_version,
                    })
                    .is_err()
            {
                return debug!("Script exit during indexeddb database open");
            }

            (pending_close.is_empty(), version)
        };

        // Step 10.5: Wait until all connections in openConnections are closed.
        // Note: if we still need to wait, the algorithm will continue in the handling of the close message.
        if can_upgrade {
            // Step 10.6: Run upgrade a database using connection, version and request.
            self.upgrade_database(key.clone(), version);

            let was_pruned = self.maybe_remove_front_from_queue(&key);
            if was_pruned {
                self.advance_connection_queue(key);
            }
        }
    }

    /// <https://w3c.github.io/IndexedDB/#open-a-database-connection>
    /// The part where the open request is ready for processing.
    fn open_database(&mut self, key: IndexedDBDescription) {
        let Some(queue) = self.connection_queues.get_mut(&key) else {
            return debug_assert!(false, "A connection queue should exist.");
        };
        let Some(open_request) = queue.front_mut() else {
            return debug_assert!(false, "An open request should be in the queue.");
        };
        let OpenRequest::Open {
            sender,
            db_name,
            version,
            id,
            processed,
            pending_upgrade: _pending_upgrade,
            pending_close,
            pending_versionchange,
            proxy_map,
        } = open_request
        else {
            return debug_assert!(
                false,
                "An request to open a connection should be in the queue."
            );
        };

        let requested_version = *version;

        // Step 4: Let db be the database named name in origin, or null otherwise.
        let db_version = match self.databases.entry(key.clone()) {
            Entry::Vacant(e) => {
                // Step 5: If version is undefined, let version be 1 if db is null, or db’s version otherwise.
                // Note: done below with the zero as first tuple item.

                // https://www.w3.org/TR/IndexedDB/#open-a-database-connection
                // Step 6: If db is null, let db be a new database
                // with name name, version 0 (zero), and with no object stores.
                // If this fails for any reason, return an appropriate error
                // (e.g. a "QuotaExceededError" or "UnknownError" DOMException).
                let Ok(response) = proxy_map
                    .handle
                    .create_database(proxy_map.bottle_id, db_name.clone())
                    .recv()
                else {
                    if let Err(e) = sender.send(ConnectionMsg::DatabaseError {
                        id: *id,
                        name: db_name.clone(),
                        error: BackendError::DbErr(
                            "Failed to communicate with client storage.".to_string(),
                        ),
                    }) {
                        debug!("Script exit during indexeddb database open {:?}", e);
                    }
                    return;
                };
                let (path, created) = match response {
                    Ok((path, created)) => (path, created),
                    Err(err) => {
                        if let Err(e) = sender.send(ConnectionMsg::DatabaseError {
                            id: *id,
                            name: db_name.clone(),
                            error: BackendError::DbErr(format!("{err:?}")),
                        }) {
                            debug!("Script exit during indexeddb database open {:?}", e);
                        }
                        return;
                    },
                };
                let engine = match SqliteEngine::new(path, created, &key, self.thread_pool.clone())
                {
                    Ok(engine) => engine,
                    Err(err) => {
                        let error = backend_error_from_sqlite_error(err);
                        if let Err(e) = sender.send(ConnectionMsg::DatabaseError {
                            id: *id,
                            name: db_name.clone(),
                            error,
                        }) {
                            debug!("Script exit during indexeddb database open {:?}", e);
                        }
                        *processed = true;
                        return;
                    },
                };
                let created_db_path = engine.created_db_path();
                let db = IndexedDBEnvironment::new(engine, self.manager_sender.clone());
                let db_version = match db.version() {
                    Ok(version) => version,
                    Err(err) => {
                        let error = backend_error_from_sqlite_error(err);
                        if let Err(e) = sender.send(ConnectionMsg::DatabaseError {
                            id: *id,
                            name: db_name.clone(),
                            error,
                        }) {
                            debug!("Script exit during indexeddb database open {:?}", e);
                        }
                        *processed = true;
                        return;
                    },
                };

                *version = if created_db_path {
                    Some(requested_version.unwrap_or(1))
                } else {
                    Some(requested_version.unwrap_or(db_version))
                };

                e.insert(db);
                db_version
            },
            Entry::Occupied(db) => {
                let db_version = match db.get().version() {
                    Ok(version) => version,
                    Err(err) => {
                        let error = backend_error_from_sqlite_error(err);
                        if let Err(e) = sender.send(ConnectionMsg::DatabaseError {
                            id: *id,
                            name: db_name.clone(),
                            error,
                        }) {
                            debug!("Script exit during indexeddb database open {:?}", e);
                        }
                        *processed = true;
                        return;
                    },
                };
                // Step 5: If version is undefined, let version be 1 if db is null, or db’s version otherwise.
                *version = Some(requested_version.unwrap_or(db_version));
                db_version
            },
        };

        let Some(version) = *version else {
            return debug_assert!(
                false,
                "An upgrade version should have been determined by now."
            );
        };

        // Step 7: If db’s version is greater than version,
        // return a newly created "VersionError" DOMException
        // and abort these steps.
        if version < db_version {
            if sender
                .send(ConnectionMsg::VersionError {
                    name: db_name.clone(),
                    id: *id,
                })
                .is_err()
            {
                debug!("Script exit during indexeddb database open");
            }
            *processed = true;
            return;
        }

        // Step 8: Let connection be a new connection to db.
        // Step 9: Set connection’s version to version.
        let connection = Connection {
            close_pending: false,
            sender: sender.clone(),
        };
        let entry = self.connections.entry(key.clone()).or_default();
        entry.insert(*id, connection);

        // Step 10: If db’s version is less than version, then:
        if db_version < version {
            // Step 10.1: Let openConnections be the set of all connections,
            // except connection, associated with db.
            let open_connections = entry
                .iter_mut()
                .filter(|(other_id, conn)| !conn.close_pending && *other_id != id);
            for (id_to_close, conn) in open_connections {
                // Step 10.2: For each entry of openConnections
                // queue a database task to fire a version change event
                // named versionchange at entry with db’s version and version.
                if conn
                    .sender
                    .send(ConnectionMsg::VersionChange {
                        name: db_name.clone(),
                        id: *id_to_close,
                        version,
                        old_version: db_version,
                    })
                    .is_err()
                {
                    error!("Failed to send ConnectionMsg::Connection to script.");
                };
                pending_close.insert(*id_to_close);
                pending_versionchange.insert(*id_to_close);
            }
            if !pending_close.is_empty() {
                // Step 10.3: Wait for all of the events to be fired.
                return;
            }

            // Step 10.6: Run upgrade a database using connection, version and request.
            self.upgrade_database(key, version);
            return;
        }

        // Step 11:
        let object_store_names = self
            .databases
            .get(&key)
            .and_then(|db| db.object_store_names().ok())
            .unwrap_or_default();
        *processed = true;
        if sender
            .send(ConnectionMsg::Connection {
                name: db_name.clone(),
                id: *id,
                version: db_version,
                upgraded: false,
                object_store_names,
            })
            .is_err()
        {
            error!("Failed to send ConnectionMsg::Connection to script.");
        };
    }

    /// <https://www.w3.org/TR/IndexedDB/#delete-a-database>
    /// The part adding the request to the connection queue.
    fn start_delete_database(
        &mut self,
        key: IndexedDBDescription,
        id: Uuid,
        proxy_map: StorageProxyMap,
        sender: GenericCallback<BackendResult<u64>>,
    ) {
        let open_request = OpenRequest::Delete {
            sender,
            _origin: key.origin.clone(),
            db_name: key.name.clone(),
            processed: false,
            proxy_map,
            id,
        };

        let should_continue = {
            // Step 1: Let queue be the connection queue for storageKey and name.
            let queue = self.connection_queues.entry(key.clone()).or_default();

            // Step 2: Add request to queue.
            queue.push_back(open_request);
            queue.len() == 1
        };

        // Step 3: Wait until all previous requests in queue have been processed.
        if should_continue {
            self.delete_database(key.clone());
            self.maybe_remove_front_from_queue(&key);
        }
    }

    /// <https://www.w3.org/TR/IndexedDB/#delete-a-database>
    fn delete_database(&mut self, key: IndexedDBDescription) {
        let Some(queue) = self.connection_queues.get_mut(&key) else {
            return debug_assert!(false, "A connection queue should exist.");
        };
        let Some(open_request) = queue.front_mut() else {
            return debug_assert!(false, "An open request should be in the queue.");
        };
        let OpenRequest::Delete {
            sender,
            _origin: _,
            db_name,
            processed,
            id: _,
            proxy_map,
        } = open_request
        else {
            return debug_assert!(
                false,
                "An request to open a connection should be in the queue."
            );
        };

        // Step 4: Let db be the database named name in storageKey, if one exists. Otherwise, return 0 (zero).
        let version = if let Some(db) = self.databases.remove(&key) {
            // Step 5: Let openConnections be the set of all connections associated with db.
            // Step6: For each entry of openConnections that does not have its close pending flag set to true,
            // queue a database task to fire a version change event named versionchange
            // at entry with db’s version and null.
            // Step 7: Wait for all of the events to be fired.
            // Step 8: If any of the connections in openConnections are still not closed,
            // queue a database task to fire a version change event
            // named blocked at request with db’s version and null.
            // Step 9: Wait until all connections in openConnections are closed.
            // TODO: implement connections.

            // Step 10: Let version be db’s version.
            let res = db.version();
            let Ok(version) = res else {
                *processed = true;
                if sender
                    .send(BackendResult::Err(BackendError::DbErr(
                        res.unwrap_err().to_string(),
                    )))
                    .is_err()
                {
                    debug!("Script went away during pending database delete.");
                }
                return;
            };

            // Note: ensure db is dropped before deleting directory,
            // to get around windows file locks.
            drop(db);

            // Step 11: Delete db.
            // If this fails for any reason,
            // return an appropriate error (e.g. a QuotaExceededError, or an "UnknownError" DOMException).
            let Ok(response) = proxy_map
                .handle
                .delete_database(proxy_map.bottle_id, db_name.clone())
                .recv()
            else {
                if sender
                    .send(BackendResult::Err(BackendError::DbErr(
                        "Failed to communicate with client storage.".to_string(),
                    )))
                    .is_err()
                {
                    debug!("Script went away during pending database delete.");
                }
                return;
            };
            if let Err(err) = response {
                if sender
                    .send(BackendResult::Err(BackendError::DbErr(format!(
                        "Client storage error: {err:?}"
                    ))))
                    .is_err()
                {
                    debug!("Script went away during pending database delete.");
                }
                return;
            }
            version
        } else {
            0
        };

        // step 12: Return version.
        if sender.send(BackendResult::Ok(version)).is_err() {
            debug!("Script went away during pending database delete.");
        }

        *processed = true;
    }

    /// <https://w3c.github.io/IndexedDB/#closing-connection>
    fn close_database(&mut self, origin: ImmutableOrigin, id: Uuid, name: String) {
        // Step 1: Set connection’s close pending flag to true.
        // TODO: seems like a script only flag.

        // Step 2: If the forced flag is true,
        // then for each transaction created using connection
        // run abort a transaction with transaction and newly created "AbortError" DOMException.
        // Step 3: Wait for all transactions created using connection to complete.
        // Once they are complete, connection is closed.
        // TODO: transaction lifecycle.

        // Step 4: If the forced flag is true, then fire an event named close at connection.
        // TODO: implement, probably only on the script side of things.

        // Note: below we are continuing
        // <https://w3c.github.io/IndexedDB/#open-a-database-connection>
        // in the case that an open request is waiting for connections to close.
        let key = IndexedDBDescription { origin, name };
        let (can_upgrade, version) = {
            self.remove_connection(&key, &id);

            let Some(queue) = self.connection_queues.get_mut(&key) else {
                return;
            };
            let Some(open_request) = queue.front_mut() else {
                return;
            };
            if let OpenRequest::Open {
                sender: _,
                db_name: _,
                version,
                id: _,
                processed: _,
                pending_upgrade,
                pending_versionchange,
                pending_close,
                proxy_map: _,
            } = open_request
            {
                pending_close.remove(&id);
                (
                    // Note: need to exclude requests that have already started upgrading.
                    pending_close.is_empty() &&
                        pending_versionchange.is_empty() &&
                        !pending_upgrade.is_some(),
                    *version,
                )
            } else {
                (false, None)
            }
        };

        // <https://w3c.github.io/IndexedDB/#open-a-database-connection>
        // Step 10.3: Wait for all of the events to be fired.
        // Step 10.5: Wait until all connections in openConnections are closed.
        // Note: both conditions must be checked here,
        // because that is the condition enabling the upgrade to proceed.
        if can_upgrade {
            // Step 10.6: Run upgrade a database using connection, version and request.
            let Some(version) = version else {
                return debug_assert!(
                    false,
                    "An upgrade version should have been determined by now."
                );
            };
            self.upgrade_database(key.clone(), version);

            let was_pruned = self.maybe_remove_front_from_queue(&key);
            if was_pruned {
                self.advance_connection_queue(key);
            }
        }
    }

    fn handle_sync_operation(&mut self, operation: SyncOperation) {
        match operation {
            SyncOperation::GetDatabases(sender, origin) => {
                // The in-parallel steps of https://www.w3.org/TR/IndexedDB/#dom-idbfactory-databases

                // Step 4.1 Let databases be the set of databases in storageKey.
                // If this cannot be determined for any reason,
                // then queue a database task to reject p with an appropriate error
                // (e.g. an "UnknownError" DOMException) and terminate these steps.
                // TODO: separate database and connection concepts.
                // For now using `self.databases`, which track connections.

                // Step 4.2: Let result be a new list.
                let info_list: Vec<DatabaseInfo> = self
                    .databases
                    .iter()
                    .filter_map(|(description, info)| {
                        // Step 4.3: For each db of databases:
                        if let Ok(version) = info.version() {
                            // Step 4.3.4: If db’s version is 0, then continue.
                            if version == 0 {
                                None
                            } else {
                                // Step 4.3.5: Let info be a new IDBDatabaseInfo dictionary.
                                // Step 4.3.6: Set info’s name dictionary member to db’s name.
                                // Step 4.3.7: Set info’s version dictionary member to db’s version.
                                // Step 4.3.8: Append info to result.
                                if description.origin == origin {
                                    Some(DatabaseInfo {
                                        name: description.name.clone(),
                                        version,
                                    })
                                } else {
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    })
                    .collect();

                // IndexedDB `databases()` / "get a list of databases" returns the visible list;
                // filtering out non-user-visible entries must not turn success into an internal error.
                // https://w3c.github.io/IndexedDB/#dom-idbfactory-databases
                let result = Ok(info_list);

                // Step 4.4: Queue a database task to resolve p with result.
                if sender.send(result).is_err() {
                    debug!("Couldn't send SyncOperation::GetDatabases reply.");
                }
            },
            SyncOperation::CloseDatabase(origin, id, db_name) => {
                self.close_database(origin, id, db_name);
            },
            SyncOperation::OpenDatabase(sender, origin, db_name, version, id, proxy_map) => {
                self.open_a_database_connection(sender, origin, db_name, version, id, proxy_map);
            },
            SyncOperation::AbortPendingUpgrades {
                pending_upgrades,
                origin,
                proxy_map,
            } => {
                self.abort_pending_upgrades(pending_upgrades, origin, proxy_map);
            },
            SyncOperation::DeleteDatabase(callback, origin, db_name, proxy_map, id) => {
                let idb_description = IndexedDBDescription {
                    origin,
                    name: db_name,
                };
                self.start_delete_database(idb_description, id, proxy_map, callback);
            },
            SyncOperation::GetObjectStore(sender, origin, db_name, store_name) => {
                // FIXME:(arihant2math) Should we error out more aggressively here?
                let result = self.get_database(origin, db_name).map(|db| {
                    let key_generator_current_number = db.key_generator_current_number(&store_name);
                    IndexedDBObjectStore {
                        key_path: db.key_path(&store_name),
                        has_key_generator: key_generator_current_number.is_some(),
                        key_generator_current_number,
                        indexes: db.indexes(&store_name).unwrap_or_default(),
                        name: store_name,
                    }
                });
                let _ = sender.send(result.ok_or(BackendError::DbNotFound));
            },
            SyncOperation::Commit(callback, origin, db_name, txn) => {
                // https://w3c.github.io/IndexedDB/#commit-a-transaction
                // TODO: implement the commit algorithm and only reply after the backend has
                // transitioned the transaction to committed/aborted (should be atomic).
                if let Some(db) = self.get_database_mut(origin.clone(), db_name.clone()) {
                    if db.can_commit_now(txn) {
                        if callback
                            .send(TxnCompleteMsg {
                                origin: origin.clone(),
                                db_name: db_name.clone(),
                                txn,
                                result: Ok(()),
                            })
                            .is_err()
                        {
                            error!(
                                "Failed to send immediate commit completion for db '{}' txn {}.",
                                db_name, txn
                            );
                        }
                    } else {
                        db.queue_pending_commit_callback(txn, callback);
                    }
                    db.schedule_transactions(origin, &db_name);
                } else if callback
                    .send(TxnCompleteMsg {
                        origin,
                        db_name: db_name.clone(),
                        txn,
                        // If the database entry has already been removed, treat commit as a
                        // no-op success so script side completion does not spuriously abort.
                        result: Ok(()),
                    })
                    .is_err()
                {
                    error!(
                        "Failed to send commit completion for missing db '{}' txn {}.",
                        db_name, txn
                    );
                }
            },
            SyncOperation::Abort(abort_callback, origin, db_name, txn) => {
                self.handle_abort(abort_callback, origin, db_name, txn);
            },
            SyncOperation::UpgradeTransactionFinished {
                origin,
                db_name,
                txn,
                committed,
            } => {
                self.handle_upgrade_transaction_finished(db_name, origin, txn, committed);
            },
            SyncOperation::RequestHandled {
                origin,
                db_name,
                txn,
                request_id,
            } => {
                // https://w3c.github.io/IndexedDB/#transaction-lifecycl
                // The implementation must attempt to commit an inactive transaction
                // when all requests placed against the transaction have completed
                // and their returned results handled, no new requests have been
                // placed against the transaction, and the transaction has not been aborted

                let should_notify =
                    if let Some(db) = self.get_database_mut(origin.clone(), db_name.clone()) {
                        db.mark_request_handled(txn, request_id);
                        db.can_notify_txn_maybe_commit(txn)
                    } else {
                        false
                    };
                if should_notify {
                    self.handle_sync_operation(SyncOperation::TxnMaybeCommit {
                        origin,
                        db_name,
                        txn,
                    });
                }
            },
            SyncOperation::TxnMaybeCommit {
                origin,
                db_name,
                txn,
            } => {
                self.handle_txn_maybe_commit(origin, db_name, txn);
            },
            SyncOperation::TransactionFinished {
                origin,
                db_name,
                txn,
            } => {
                let maybe_commit_txns =
                    if let Some(db) = self.get_database_mut(origin.clone(), db_name.clone()) {
                        db.finish_transaction(txn);
                        db.schedule_transactions(origin.clone(), &db_name);
                        db.maybe_commit_candidates()
                    } else {
                        Vec::new()
                    };
                for candidate in maybe_commit_txns {
                    self.handle_txn_maybe_commit(origin.clone(), db_name.clone(), candidate);
                }
            },
            SyncOperation::CreateTransaction {
                sender,
                origin,
                db_name,
                mode,
                scope,
            } => {
                let key = IndexedDBDescription {
                    origin: origin.clone(),
                    name: db_name.clone(),
                };
                if let Some(db) = self.databases.get_mut(&key) {
                    let transaction_id = self.serial_number_counter;
                    self.serial_number_counter += 1;
                    db.register_transaction(transaction_id, mode, scope);
                    db.schedule_transactions(origin, &db_name);
                    let _ = sender.send(Ok(transaction_id));
                } else {
                    let _ = sender.send(Err(BackendError::DbNotFound));
                }
            },
            SyncOperation::UpgradeVersion(sender, origin, db_name, _txn, version) => {
                if let Some(db) = self.get_database_mut(origin, db_name) {
                    if version > db.version().unwrap_or(0) {
                        let _ = db.set_version(version);
                    }
                    // erroring out if the version is not upgraded can be and non-replicable
                    let _ = sender.send(db.version().map_err(backend_error_from_sqlite_error));
                } else {
                    let _ = sender.send(Err(BackendError::DbNotFound));
                }
            },
            SyncOperation::Version(sender, origin, db_name) => {
                if let Some(db) = self.get_database(origin, db_name) {
                    let _ = sender.send(db.version().map_err(backend_error_from_sqlite_error));
                } else {
                    let _ = sender.send(Err(BackendError::DbNotFound));
                }
            },
            SyncOperation::NotifyEndOfVersionChange {
                name,
                id,
                old_version,
                origin,
            } => {
                self.handle_version_change_done(name, id, old_version, origin);
            },
            SyncOperation::Exit(_) => {
                unreachable!("We must've already broken out of event loop.");
            },
        }
    }

    /// Handling for the `Abort` message which will call [`Self::abort`] if the transaction
    /// being aborted is not ongoing. If the transaction is in process, abort is delayed until
    /// the batch finishes.
    fn handle_abort(
        &mut self,
        abort_callback: GenericCallback<TxnCompleteMsg>,
        origin: ImmutableOrigin,
        database_name: String,
        transaction: u64,
    ) {
        let message = || TxnCompleteMsg {
            origin: origin.clone(),
            db_name: database_name.clone(),
            txn: transaction,
            result: Err(BackendError::Abort),
        };

        let Some(database) = self.get_database_mut(origin.clone(), database_name.clone()) else {
            // We didn't find the database, so just treat the transaction as aborted.
            if abort_callback.send(message()).is_err() {
                error!(
                    "Failed to send abort completion for database \
                    '{database_name}' transaction {transaction}.",
                );
            }
            return;
        };

        database
            .pending_abort_callbacks
            .entry(transaction)
            .or_default()
            .push(abort_callback);

        // If the transaction is running wait to abort until after it finishes to actually
        // abort.
        if database.running_readwrite == Some(transaction) ||
            database.running_readonly.contains(&transaction)
        {
            return;
        }

        database.abort(&origin, &database_name, transaction);
    }

    fn collect_memory_reports(&self) -> Vec<Report> {
        let mut reports = vec![];
        perform_memory_report(|ops| {
            reports.push(Report {
                path: path!["indexeddb"],
                kind: ReportKind::ExplicitJemallocHeapSize,
                size: self.connections.size_of(ops) +
                    self.databases.size_of(ops) +
                    self.connection_queues.size_of(ops),
            });
        });
        reports
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use profile_traits::generic_callback::GenericCallback;
    use serde::{Deserialize, Serialize};
    use servo_base::generic_channel;
    use servo_base::threadpool::ThreadPool;
    use servo_url::ImmutableOrigin;
    use storage_traits::indexeddb::{
        AsyncOperation, AsyncReadOnlyOperation, AsyncReadWriteOperation, AsyncSchemaOperation,
        BackendError, BackendResult, IndexedDBKeyRange, IndexedDBKeyType, IndexedDBRecord,
        IndexedDBTxnMode, KeyPath, PutItemResult,
    };
    use url::Host;

    use super::{IndexedDBDescription, IndexedDBEnvironment, UPGRADE_BUFFER_MAX_BYTES};
    use crate::indexeddb::engines::SqliteEngine;

    fn test_origin() -> ImmutableOrigin {
        ImmutableOrigin::Tuple(
            "test_origin".to_string(),
            Host::Domain("localhost".to_string()),
            80,
        )
    }

    #[test]
    fn test_restore_object_stores_removes_created_store_and_indexes() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let thread_pool = ThreadPool::global();
        let description = IndexedDBDescription {
            name: "test_db".to_string(),
            origin: test_origin(),
        };
        let engine = SqliteEngine::new(
            base_dir.path().to_path_buf(),
            true,
            &description,
            thread_pool,
        )
        .unwrap();
        let (sender, _receiver) = generic_channel::channel().unwrap();
        let mut env = IndexedDBEnvironment::new(engine, sender);

        let object_stores = env.object_stores().unwrap();
        assert!(object_stores.is_empty());

        env.create_object_store("not_books", None, false).unwrap();
        env.create_index(
            "not_books",
            "not_by_author".to_string(),
            KeyPath::String("author".to_string()),
            false,
            false,
        )
        .unwrap();

        env.restore_object_stores(&object_stores).unwrap();

        assert_eq!(env.object_store_names().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_abort_transaction_restores_readwrite_key_generator_current_number() {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let thread_pool = ThreadPool::global();
        let description = IndexedDBDescription {
            name: "test_db".to_string(),
            origin: test_origin(),
        };
        let engine = SqliteEngine::new(
            base_dir.path().to_path_buf(),
            true,
            &description,
            thread_pool,
        )
        .unwrap();
        let (sender, _receiver) = generic_channel::channel().unwrap();
        let mut env = IndexedDBEnvironment::new(engine, sender);

        env.create_object_store("books", None, true).unwrap();
        assert_eq!(env.key_generator_current_number("books"), Some(1));

        env.register_transaction(1, IndexedDBTxnMode::Readwrite, vec!["books".to_string()]);
        env.set_key_generator_current_number("books", 345680)
            .unwrap();

        env.abort_transaction(1);

        assert_eq!(env.key_generator_current_number("books"), Some(1));
    }

    /// Puts one record through the engine, blocking until the write landed.
    /// Mirrors the write shape an upgrade transaction issues.
    fn put_test_record(
        env: &IndexedDBEnvironment<SqliteEngine>,
        store_name: &str,
        key: IndexedDBKeyType,
        value: Vec<u8>,
    ) {
        use crate::indexeddb::engines::{KvsEngine, KvsOperation, KvsTransaction};
        use std::collections::VecDeque;
        use std::sync::mpsc;

        let (result_sender, result_receiver) = mpsc::channel();
        let callback = GenericCallback::new(move |result: Result<
            BackendResult<PutItemResult>,
            servo_base::generic_channel::SendError,
        >| {
            let _ = result_sender.send(result.map(|_| ()));
        })
        .unwrap();
        env.engine.process_transaction(
            KvsTransaction {
                mode: IndexedDBTxnMode::Readwrite,
                requests: VecDeque::from(vec![KvsOperation {
                    store_name: store_name.to_string(),
                    operation: AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                        callback,
                        key: Some(key),
                        value,
                        should_overwrite: true,
                        key_generator_current_number: None,
                    }),
                }]),
            },
            Box::new(|| ()),
        );
        let outcome = result_receiver.recv().unwrap().unwrap();
        assert!(outcome.is_ok(), "test put failed: {outcome:?}");
    }

    /// Creates an environment with one object store, plus its `TempDir` (which
    /// must outlive the environment).
    fn upgrade_test_env(
        db_name: &str,
        store_name: &str,
        auto_increment: bool,
    ) -> (tempfile::TempDir, IndexedDBEnvironment<SqliteEngine>) {
        let base_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let description = IndexedDBDescription {
            name: db_name.to_string(),
            origin: test_origin(),
        };
        let engine = SqliteEngine::new(
            base_dir.path().to_path_buf(),
            true,
            &description,
            ThreadPool::global(),
        )
        .unwrap();
        let (sender, _receiver) = generic_channel::channel().unwrap();
        let mut env = IndexedDBEnvironment::new(engine, sender);
        env.create_object_store(store_name, None, auto_increment)
            .unwrap();
        (base_dir, env)
    }

    /// Builds a `GenericCallback` that forwards the operation result into an
    /// mpsc channel for in-test assertion (same shape as `put_test_record`).
    fn collect_callback<R>(
        sender: mpsc::Sender<BackendResult<R>>,
    ) -> GenericCallback<BackendResult<R>>
    where
        R: for<'de> Deserialize<'de> + Serialize + Send + 'static,
    {
        GenericCallback::new(
            move |result: Result<BackendResult<R>, servo_base::generic_channel::SendError>| {
                let _ = sender.send(result.unwrap_or_else(|send_error| {
                    Err(BackendError::DbErr(format!("{send_error:?}")))
                }));
            },
        )
        .unwrap()
    }

    /// Reads a store's records straight from the engine (bypassing the shadow
    /// write buffer) so tests can assert committed engine state.
    fn engine_records(
        env: &IndexedDBEnvironment<SqliteEngine>,
        store_name: &str,
    ) -> Vec<IndexedDBRecord> {
        use crate::indexeddb::engines::{KvsEngine, KvsOperation, KvsTransaction};
        use std::collections::VecDeque;

        let (result_sender, result_receiver) = mpsc::channel();
        let callback = GenericCallback::new(
            move |result: Result<
                BackendResult<Vec<IndexedDBRecord>>,
                servo_base::generic_channel::SendError,
            >| {
                let _ = result_sender.send(result.unwrap_or_else(|send_error| {
                    Err(BackendError::DbErr(format!("{send_error:?}")))
                }));
            },
        )
        .unwrap();
        env.engine.process_transaction(
            KvsTransaction {
                mode: IndexedDBTxnMode::Readonly,
                requests: VecDeque::from(vec![KvsOperation {
                    store_name: store_name.to_string(),
                    operation: AsyncOperation::ReadOnly(AsyncReadOnlyOperation::Iterate {
                        callback,
                        key_range: IndexedDBKeyRange::default(),
                    }),
                }]),
            },
            Box::new(|| ()),
        );
        let mut records = result_receiver.recv().unwrap().unwrap();
        records.sort_by(|a, b| a.key.partial_cmp(&b.key).unwrap());
        records
    }

    // BAO PATCH (REQ-BRW-051, idb-upgrade-shadow-write-buffer) regression
    // tests: every test drives the real `queue_operation` entry point and
    // collects results over mpsc (same shape as `put_test_record`).

    #[test]
    fn test_upgrade_buffer_commit_lands_writes_in_engine() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        let (put_sender, put_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                callback: collect_callback(put_sender),
                key: Some(IndexedDBKeyType::Number(10.0)),
                value: b"buffered".to_vec(),
                should_overwrite: true,
                key_generator_current_number: None,
            }),
        );
        assert_eq!(
            put_receiver.recv().unwrap().unwrap(),
            PutItemResult::Key(IndexedDBKeyType::Number(10.0))
        );
        // Not committed yet: the engine must not have the write.
        assert!(engine_records(&env, "books").is_empty());

        env.commit_upgrade_buffer(1).unwrap();

        let records = engine_records(&env, "books");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, IndexedDBKeyType::Number(10.0));
        assert_eq!(records[0].value, b"buffered".to_vec());
    }

    #[test]
    fn test_upgrade_buffer_abort_discards_writes() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        let (put_sender, put_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                callback: collect_callback(put_sender),
                key: Some(IndexedDBKeyType::Number(11.0)),
                value: b"shadow".to_vec(),
                should_overwrite: true,
                key_generator_current_number: None,
            }),
        );
        assert_eq!(
            put_receiver.recv().unwrap().unwrap(),
            PutItemResult::Key(IndexedDBKeyType::Number(11.0))
        );

        // Abort: zero-IO rollback, the write never reached the engine.
        env.discard_upgrade_buffer(1);
        env.discard_upgrade_buffer(1); // idempotent safety net

        assert!(engine_records(&env, "books").is_empty());
    }

    #[test]
    fn test_upgrade_buffer_same_key_put_overwrites_in_place() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        for (name, value) in [("first", b"first".to_vec()), ("second", b"second".to_vec())] {
            let (put_sender, put_receiver) = mpsc::channel();
            env.queue_operation(
                "books",
                1,
                IndexedDBTxnMode::Versionchange,
                AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                    callback: collect_callback(put_sender),
                    key: Some(IndexedDBKeyType::Number(1.0)),
                    value,
                    should_overwrite: true,
                    key_generator_current_number: None,
                }),
            );
            assert_eq!(
                put_receiver.recv().unwrap().unwrap(),
                PutItemResult::Key(IndexedDBKeyType::Number(1.0)),
                "{name} put must be acknowledged"
            );
        }

        env.commit_upgrade_buffer(1).unwrap();

        let records = engine_records(&env, "books");
        assert_eq!(
            records.len(),
            1,
            "same-key puts must collapse to one buffered write"
        );
        assert_eq!(records[0].value, b"second".to_vec());
    }

    #[test]
    fn test_upgrade_buffer_clear_hides_engine_records_and_commits() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        put_test_record(
            &env,
            "books",
            IndexedDBKeyType::Number(1.0),
            b"joshua".to_vec(),
        );
        put_test_record(
            &env,
            "books",
            IndexedDBKeyType::Number(2.0),
            b"jonas".to_vec(),
        );
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        let (clear_sender, clear_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::Clear(collect_callback(
                clear_sender,
            ))),
        );
        assert_eq!(clear_receiver.recv().unwrap().unwrap(), ());

        // Read-own-write: the buffered Clear already hides the engine records.
        let (read_sender, read_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadOnly(AsyncReadOnlyOperation::GetAllItems {
                callback: collect_callback(read_sender),
                key_range: IndexedDBKeyRange::default(),
                count: None,
            }),
        );
        assert_eq!(
            read_receiver.recv().unwrap().unwrap(),
            Vec::<Vec<u8>>::new()
        );

        env.commit_upgrade_buffer(1).unwrap();

        assert!(engine_records(&env, "books").is_empty());
    }

    #[test]
    fn test_upgrade_buffer_iterate_merges_range_reads() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        put_test_record(
            &env,
            "books",
            IndexedDBKeyType::Number(1.0),
            b"one".to_vec(),
        );
        put_test_record(
            &env,
            "books",
            IndexedDBKeyType::Number(2.0),
            b"two".to_vec(),
        );
        put_test_record(
            &env,
            "books",
            IndexedDBKeyType::Number(3.0),
            b"three".to_vec(),
        );
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        let (delete_sender, delete_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::RemoveItem {
                callback: collect_callback(delete_sender),
                key_range: IndexedDBKeyRange::only(IndexedDBKeyType::Number(1.0)),
            }),
        );
        assert_eq!(delete_receiver.recv().unwrap().unwrap(), ());
        let (put_sender, put_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                callback: collect_callback(put_sender),
                key: Some(IndexedDBKeyType::Number(2.5)),
                value: b"inserted".to_vec(),
                should_overwrite: true,
                key_generator_current_number: None,
            }),
        );
        assert_eq!(
            put_receiver.recv().unwrap().unwrap(),
            PutItemResult::Key(IndexedDBKeyType::Number(2.5))
        );

        let (read_sender, read_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadOnly(AsyncReadOnlyOperation::Iterate {
                callback: collect_callback(read_sender),
                key_range: IndexedDBKeyRange::default(),
            }),
        );
        let records = read_receiver.recv().unwrap().unwrap();
        let keys: Vec<IndexedDBKeyType> = records.into_iter().map(|record| record.key).collect();
        assert_eq!(
            keys,
            vec![
                IndexedDBKeyType::Number(2.0),
                IndexedDBKeyType::Number(2.5),
                IndexedDBKeyType::Number(3.0),
            ]
        );
    }

    #[test]
    fn test_upgrade_buffer_keyless_put_mints_generator_keys() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "auto", true);
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["auto".to_string()]);

        // The real creation path: the schema op passes through queue_operation
        // and registers the store as engine-empty with a generator at 1.
        let (schema_sender, _schema_receiver) = mpsc::channel();
        let schema_callback = GenericCallback::new(
            move |result: Result<BackendError, servo_base::generic_channel::SendError>| {
                let _ = schema_sender.send(result);
            },
        )
        .unwrap();
        env.queue_operation(
            "auto",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::Schema(AsyncSchemaOperation::CreateObjectStore {
                callback: schema_callback,
                key_path: None,
                auto_increment: true,
            }),
        );

        for (name, value) in [("one", b"one".to_vec()), ("two", b"two".to_vec())] {
            let (put_sender, put_receiver) = mpsc::channel();
            env.queue_operation(
                "auto",
                1,
                IndexedDBTxnMode::Versionchange,
                AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                    callback: collect_callback(put_sender),
                    key: None,
                    value,
                    should_overwrite: false,
                    key_generator_current_number: None,
                }),
            );
            let key = put_receiver.recv().unwrap().unwrap();
            assert!(
                matches!(key, PutItemResult::Key(IndexedDBKeyType::Number(_))),
                "{name} keyless put must mint a generator key, got {key:?}"
            );
        }

        env.commit_upgrade_buffer(1).unwrap();

        let records = engine_records(&env, "auto");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].key, IndexedDBKeyType::Number(1.0));
        assert_eq!(records[0].value, b"one".to_vec());
        assert_eq!(records[1].key, IndexedDBKeyType::Number(2.0));
        assert_eq!(records[1].value, b"two".to_vec());
        assert_eq!(env.key_generator_current_number("auto"), Some(3));
    }

    #[test]
    fn test_upgrade_buffer_quota_exceeded_fails_closed() {
        let (_base_dir, mut env) = upgrade_test_env("test_db", "books", false);
        env.register_transaction(1, IndexedDBTxnMode::Versionchange, vec!["books".to_string()]);

        let (put_sender, put_receiver) = mpsc::channel();
        env.queue_operation(
            "books",
            1,
            IndexedDBTxnMode::Versionchange,
            AsyncOperation::ReadWrite(AsyncReadWriteOperation::PutItem {
                callback: collect_callback(put_sender),
                key: Some(IndexedDBKeyType::Number(1.0)),
                value: vec![0_u8; (UPGRADE_BUFFER_MAX_BYTES + 1) as usize],
                should_overwrite: true,
                key_generator_current_number: None,
            }),
        );
        assert_eq!(
            put_receiver.recv().unwrap().unwrap_err(),
            BackendError::QuotaExceeded
        );

        // Zero partial buffering: nothing was buffered, nothing reaches the
        // engine, and the buffer stays usable.
        env.commit_upgrade_buffer(1).unwrap();
        assert!(engine_records(&env, "books").is_empty());
    }
}
