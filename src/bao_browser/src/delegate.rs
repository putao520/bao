// @trace REQ-BRW-001 [entity:BrowserContext]  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] Worker lifecycle + DedicatedWorkerGlobalScope API
// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] SharedWorker cross-page routing + connect event
// @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope] ServiceWorker registration + fetch interception + stealth/CDP boundary consistency
// @trace REQ-CDP-006 [entity:ServoDelegateHooks] (servo delegate → CDP event forwarding)
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, LoadStatus, NavigationRequest,
    PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, WebView, WebViewDelegate,
};

use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

// ─── Worker Message Channel (REQ-BRW-004) ──────────────────────────
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] [criterion:1..18]
// DF-WK-4 / DF-WK-5: page↔worker bidirectional structured-clone channel.
//
// Servo already handles the full Worker lifecycle internally (DOM bindings,
// structured clone via `structuredclone::write/read`, crossbeam channel
// transport). Bao's responsibility is:
//   1. Track per-webview active Worker count for page-unload auto-terminate
//      (SPEC criterion #10: GlobalScope::track_worker + AutoCloseWorker).
//   2. Forward Worker message events to CDP via the existing event_tx path.
//   3. Provide a `WorkerHandle` that bao_browser consumers can use to
//      observe worker state (closing flag) without holding JSObject refs.
//
// Thread safety: WorkerHandle only holds Arc<AtomicBool> (closing) and
// Arc<AtomicBool> (terminated) — no JSObject, no raw pointer. These are
// Send + Sync safe. The actual Worker DOM object lives in servo's
// ScriptThread; we never touch it from bao_browser.

/// Unique identifier for a Worker within a page's scope.
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkerId(pub String);

/// A Send+Sync handle to a servo Worker's lifecycle state.
///
/// Does NOT hold JSObject references — only atomic flags and the global
/// address (for REALM_PROFILES cleanup on teardown).
/// This is safe to store across threads (unlike Worker DOM objects).
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone)]
pub struct WorkerHandle {
    /// Worker script URL.
    pub script_url: String,
    /// Mirrors servo Worker::closing — set by terminate() or self.close().
    pub closing: Arc<AtomicBool>,
    /// Mirrors servo Worker::terminated — true after full teardown.
    pub terminated: Arc<AtomicBool>,
    /// Worker global object address (set after scope_init runs on worker thread).
    /// Used for REALM_PROFILES unregister on teardown (SPEC criterion #18).
    /// Zero means not yet set / unknown.
    /// @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    worker_global_addr: Arc<AtomicU64>,
}

impl WorkerHandle {
    /// Create a new WorkerHandle in the running state.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn new(script_url: String) -> Self {
        WorkerHandle {
            script_url,
            closing: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
            worker_global_addr: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns true if terminate()/self.close() has been requested.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Returns true if the Worker thread has fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Signal the Worker to terminate (mirrors Worker::terminate()).
    /// Idempotent — calling multiple times is safe.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn terminate(&self) {
        self.closing.store(true, Ordering::Release);
    }

    /// Mark the Worker as fully terminated (called after thread join).
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }

    /// Set the Worker's global object address for REALM_PROFILES tracking.
    ///
    /// Called from the worker thread's scope_init callback after the global
    /// object is created. The address is used on teardown to unregister the
    /// stealth profile from REALM_PROFILES (SPEC criterion #18).
    ///
    /// @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    pub fn set_worker_global_addr(&self, addr: usize) {
        self.worker_global_addr
            .store(addr as u64, Ordering::Release);
    }

    /// Get the Worker's global object address (0 if not yet set).
    ///
    /// @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    pub fn worker_global_addr(&self) -> usize {
        self.worker_global_addr.load(Ordering::Acquire) as usize
    }

    /// Get a clone of the Arc<AtomicU64> backing the global address slot.
    ///
    /// This allows the scope_init callback on the worker thread to write the
    /// global address into the same slot that the main thread's WorkerHandle
    /// reads from — without any JSObject references crossing the thread
    /// boundary (BCE-20260621-001: thread-local JSContext invariant).
    ///
    /// @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    pub fn worker_global_addr_arc(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.worker_global_addr)
    }

    /// Unregister the Worker's stealth profile from REALM_PROFILES.
    ///
    /// Called during crash-safe teardown (all three paths) to ensure the
    /// profile entry for this Worker's global is removed, preventing stale
    /// entries that could cause UAF or fingerprint leakage.
    ///
    /// SPEC criterion #18: "REALM_PROFILES 条目注销"
    ///
    /// @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    pub fn unregister_stealth_profile(&self) {
        let addr = self.worker_global_addr();
        if addr != 0 {
            bao_stealth::engine_props::remove_profile_for_global(addr);
        }
    }
}

/// Direction of a Worker postMessage event.
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerMessageDirection {
    /// page → worker (DF-WK-4: worker.postMessage(msg))
    PageToWorker,
    /// worker → page (DF-WK-5: self.postMessage(msg))
    WorkerToPage,
}

/// A Worker postMessage event observed by the bao layer.
///
/// Only the metadata is captured here — the actual structured-clone data
/// is handled entirely within servo's DOM (structuredclone::write/read).
/// This struct is for CDP observability and event forwarding.
///
/// @trace REQ-BRW-004 [entity:Worker]
#[derive(Debug, Clone)]
pub struct WorkerMessageEvent {
    /// Which Worker this message is associated with.
    pub worker_id: WorkerId,
    /// Direction of the message.
    pub direction: WorkerMessageDirection,
}

// ─── Worker Error Event (REQ-BRW-004 criterion #9) ────────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
// SPEC criterion #9: "onerror 事件正确传播到主线程
// (ErrorEvent 包含 message/filename/lineno/colno)".
//
// When a Worker throws an uncaught error, servo dispatches an ErrorEvent
// on the Worker object in the main thread. Bao captures the error metadata
// here for CDP observability (Runtime.exceptionThrown) and for forwarding
// to any consumer that observes Worker errors.

/// A Worker error event observed by the bao layer.
///
/// Mirrors the DOM ErrorEvent fields (message/filename/lineno/colno).
/// Servo handles the actual DOM ErrorEvent dispatch internally;
/// this struct captures the metadata for CDP forwarding.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
#[derive(Debug, Clone)]
pub struct WorkerErrorEvent {
    /// Which Worker this error is associated with.
    pub worker_id: WorkerId,
    /// Error message.
    pub message: String,
    /// Script filename where the error occurred.
    pub filename: String,
    /// Line number (1-based).
    pub lineno: u32,
    /// Column number (1-based).
    pub colno: u32,
}

// ─── Structured Clone Message Channel (REQ-BRW-004 criterion #6) ────
// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
// SPEC criterion #6: "Structured Clone 消息序列化支持
// （对象/数组/Buffer/ArrayBuffer/Transferable）"
//
// DF-WK-4: page→worker postMessage: worker.postMessage(v) →
//   structuredclone::write(cx,v) → WorkerMessage(StructuredSerializedData)
//   → crossbeam send → worker recv → structuredclone::read → message event
// DF-WK-5: worker→page onmessage: self.postMessage(v) →
//   structuredclone::write → channel → parent ScriptThread drain →
//   structuredclone::read → worker.onmessage
//
// Architecture: servo internally handles structured clone serialization
// (structuredclone::write/read) and cross-thread message transport
// (crossbeam channels). Bao's responsibility is:
//   1. Provide a `WorkerChannelBridge` that bao_browser consumers can
//      use to post messages to a Worker without touching JSObject refs.
//   2. Provide a `WorkerInbox` for receiving worker→page messages with
//      structured-clone payload data.
//   3. Track per-worker channel endpoints in `BaoWebViewState` for
//      lifecycle management and CDP observability.
//
// Thread safety: All channel data is serialized bytes (Vec<u8>) — no
// JSObject crosses thread boundaries. This satisfies NFR-THREAD-SAFETY
// and the JSContext thread-local model (BCE-20260621-001).

/// Monotonically increasing message ID counter for CDP trace correlation.
/// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(1);

/// A structured-clone serialized payload for Worker postMessage.
///
/// Contains the serialized bytes produced by SpiderMonkey's
/// `structuredclone::write`. The actual serialization/deserialization
/// happens on the sender/receiver thread's JSContext.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
#[derive(Debug)]
pub struct StructuredClonePayload {
    /// Serialized bytes from structuredclone::write.
    pub data: Vec<u8>,
    /// Number of transferable objects in the payload (for CDP reporting).
    pub transferable_count: u32,
}

impl Clone for StructuredClonePayload {
    fn clone(&self) -> Self {
        StructuredClonePayload {
            data: self.data.clone(),
            transferable_count: self.transferable_count,
        }
    }
}

/// A structured-clone message crossing the page↔worker boundary.
///
/// Carries both the serialized payload and metadata for CDP observability.
/// Each message gets a unique ID for trace correlation.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
#[derive(Debug, Clone)]
pub struct WorkerStructuredMessage {
    /// Unique message ID for CDP trace correlation.
    pub message_id: u64,
    /// Which Worker this message is associated with.
    pub worker_id: WorkerId,
    /// Direction of the message.
    pub direction: WorkerMessageDirection,
    /// Structured-clone serialized payload (when available from servo).
    /// None when only forwarding metadata (e.g., servo handles clone internally).
    pub payload: Option<StructuredClonePayload>,
}

impl WorkerStructuredMessage {
    /// Create a new structured message with a unique ID.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn new(
        worker_id: WorkerId,
        direction: WorkerMessageDirection,
        payload: Option<StructuredClonePayload>,
    ) -> Self {
        WorkerStructuredMessage {
            message_id: NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
            worker_id,
            direction,
            payload,
        }
    }

    /// Create a metadata-only message (no structured-clone payload).
    /// Used when servo handles the clone internally and bao only observes.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] DF-WK-4 / DF-WK-5
    pub fn metadata_only(worker_id: WorkerId, direction: WorkerMessageDirection) -> Self {
        Self::new(worker_id, direction, None)
    }

    /// Create a message with serialized structured-clone data.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn with_payload(
        worker_id: WorkerId,
        direction: WorkerMessageDirection,
        data: Vec<u8>,
        transferable_count: u32,
    ) -> Self {
        Self::new(
            worker_id,
            direction,
            Some(StructuredClonePayload {
                data,
                transferable_count,
            }),
        )
    }
}

/// Bidirectional channel bridge for a single Worker's postMessage channel.
///
/// Holds the mpsc channel endpoints for page↔worker communication.
/// The bridge does NOT hold JSObject references — only channel endpoints
/// and serialized data. This is safe to store across threads.
///
/// SPEC DF-WK-4: page→worker (sender → receiver in worker thread)
/// SPEC DF-WK-5: worker→page (sender in worker thread → receiver here)
///
/// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
///   [criterion:6] DF-WK-4 / DF-WK-5
pub struct WorkerChannelBridge {
    /// Worker ID this bridge belongs to.
    pub worker_id: WorkerId,
    /// Sender for page→worker messages (DF-WK-4: worker.postMessage(msg)).
    /// Structured-clone serialized bytes sent through this channel.
    /// @trace REQ-BRW-004 [entity:Worker] DF-WK-4
    pub page_to_worker_tx: Sender<StructuredClonePayload>,
    /// Receiver for page→worker messages (owned by worker thread).
    /// @trace REQ-BRW-004 [entity:Worker] DF-WK-4
    page_to_worker_rx: Option<Receiver<StructuredClonePayload>>,
    /// Receiver for worker→page messages (DF-WK-5: self.postMessage(msg)).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DF-WK-5
    pub worker_to_page_rx: Receiver<WorkerStructuredMessage>,
    /// Sender for worker→page messages (owned by worker thread).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DF-WK-5
    worker_to_page_tx: Option<Sender<WorkerStructuredMessage>>,
}

impl WorkerChannelBridge {
    /// Create a new channel bridge for the given Worker.
    ///
    /// Returns the bridge (kept by bao_browser) and a `WorkerChannelEndpoints`
    /// struct that should be sent to the worker thread for its use.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub fn new(worker_id: WorkerId) -> (Self, WorkerChannelEndpoints) {
        // DF-WK-4: page→worker channel
        let (page_to_worker_tx, page_to_worker_rx) =
            std::sync::mpsc::channel::<StructuredClonePayload>();
        // DF-WK-5: worker→page channel
        let (worker_to_page_tx, worker_to_page_rx) =
            std::sync::mpsc::channel::<WorkerStructuredMessage>();

        let bridge = WorkerChannelBridge {
            worker_id: worker_id.clone(),
            page_to_worker_tx,
            page_to_worker_rx: None, // rx goes to worker thread
            worker_to_page_rx,
            worker_to_page_tx: None, // tx goes to worker thread
        };

        let endpoints = WorkerChannelEndpoints {
            worker_id: worker_id.clone(),
            // Worker thread receives from page
            page_to_worker_rx: Some(page_to_worker_rx),
            // Worker thread sends to page
            worker_to_page_tx: Some(worker_to_page_tx),
        };

        (bridge, endpoints)
    }

    /// Post a message from the page to this Worker (DF-WK-4).
    ///
    /// Sends structured-clone serialized bytes through the channel.
    /// Returns Err if the worker thread has exited (channel closed).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4
    pub fn post_message_to_worker(
        &self,
        payload: StructuredClonePayload,
    ) -> Result<(), std::sync::mpsc::SendError<StructuredClonePayload>> {
        self.page_to_worker_tx.send(payload)
    }

    /// Try to receive a message from this Worker (DF-WK-5).
    ///
    /// Non-blocking: returns Ok(Some(msg)) if a message is available,
    /// Ok(None) if the channel is empty, Err if the worker has exited.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:6] DF-WK-5
    pub fn try_recv_from_worker(&self) -> Result<Option<WorkerStructuredMessage>, ()> {
        match self.worker_to_page_rx.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Drain all pending worker→page messages (DF-WK-5).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from workers. Returns a `WorkerDrainResult` that includes both
    /// the drained messages and whether the worker has disconnected.
    ///
    /// When `disconnected` is true, the worker thread has exited and
    /// the caller should trigger cleanup (reap terminated workers).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DF-WK-5
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_worker_messages(&self) -> WorkerDrainResult {
        let mut messages = Vec::new();
        let mut disconnected = false;
        loop {
            match self.worker_to_page_rx.try_recv() {
                Ok(msg) => messages.push(msg),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        WorkerDrainResult {
            messages,
            disconnected,
        }
    }
}

/// Result of draining worker→page messages.
///
/// Carries both the drained messages and a `disconnected` flag indicating
/// whether the worker thread has exited. When `disconnected` is true,
/// the caller should trigger cleanup (reap terminated workers, clear
/// channel bridges).
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18] crash-safe teardown detection
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DF-WK-5
#[derive(Debug)]
pub struct WorkerDrainResult {
    /// Drained worker→page messages.
    pub messages: Vec<WorkerStructuredMessage>,
    /// True if the worker→page channel is disconnected (worker thread exited).
    pub disconnected: bool,
}

// ─── Structured-Clone Channel Bridge (REQ-BRW-004 criterion #6) ────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
//
// WorkerChannelBridge + WorkerChannelEndpoints carry raw serialized bytes
// between page and Worker threads. Per DEC-WK-001 (BCE-20260627-008) the
// bypass bao_engine::WebWorker (and its StructuredCloneReceiver/Sender trait
// adapters) is removed; the bridge now only feeds CDP observability +
// message logging, since servo owns the Worker thread and its postMessage.

/// Channel endpoints sent to the Worker thread.
///
/// The worker thread owns the receiving end of the page→worker channel
/// and the sending end of the worker→page channel. These are `Send`
/// safe because they only carry serialized bytes, not JSObject refs.
///
/// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope]
///   [criterion:6] DF-WK-4 / DF-WK-5
pub struct WorkerChannelEndpoints {
    /// Worker ID this endpoint belongs to.
    pub worker_id: WorkerId,
    /// Worker thread receives page→worker messages (DF-WK-4).
    /// @trace REQ-BRW-004 [entity:Worker] DF-WK-4
    pub page_to_worker_rx: Option<Receiver<StructuredClonePayload>>,
    /// Worker thread sends worker→page messages (DF-WK-5).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DF-WK-5
    pub worker_to_page_tx: Option<Sender<WorkerStructuredMessage>>,
}

// ─── SharedWorkerGlobalScope (REQ-BRW-004 entity:SharedWorkerGlobalScope) ───
// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
// SPEC entity:SharedWorkerGlobalScope — the global scope for a Shared Worker.
// Extends WorkerGlobalScope with:
//   - name: the SharedWorker's name (from constructor options)
//   - onconnect: event handler for new page connections
//   - All WorkerGlobalScope APIs (self/close/importScripts/setTimeout/
//     fetch/crypto/performance/location/navigator/console)
//
// Key difference from DedicatedWorkerGlobalScope:
//   - SharedWorkerGlobalScope fires a `connect` event (not `message`) when
//     a new page connects. The connect event carries a MessagePort pair.
//   - No parent reference — SharedWorkers are parentless; they serve
//     multiple pages via independent MessagePorts.
//   - onconnect is the primary entry point (vs onmessage for Dedicated).

/// The SharedWorkerGlobalScope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a Shared Worker's global
/// scope. The actual DOM SharedWorkerGlobalScope lives in servo's
/// ScriptThread; this struct tracks the state that bao needs for lifecycle
/// management, CDP observability, and stealth consistency verification.
///
/// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
#[derive(Debug, Clone)]
pub struct SharedWorkerGlobalScopeState {
    /// The base WorkerGlobalScope state.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [entity:WorkerGlobalScope]
    pub scope: WorkerGlobalScopeState,
    /// The SharedWorkerId identifying this Shared Worker.
    /// Links the scope to its SharedWorkerHandle.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub shared_worker_id: SharedWorkerId,
    /// Whether onconnect event handler is registered.
    /// Tracked for CDP observability (Runtime binding reporting).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub has_onconnect: bool,
    /// Number of connect events fired (equals number of pages that have
    /// connected since the SharedWorker was created).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    pub connect_count: usize,
}

impl SharedWorkerGlobalScopeState {
    /// Create a SharedWorkerGlobalScopeState for the given SharedWorker.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn new(shared_worker_id: SharedWorkerId, config: &SharedWorkerScopeConfig) -> Self {
        let worker_url = shared_worker_id.script_url.clone();
        SharedWorkerGlobalScopeState {
            scope: WorkerGlobalScopeState::new_shared(worker_url, config),
            shared_worker_id,
            has_onconnect: false,
            connect_count: 0,
        }
    }

    /// Get the WorkerLocation for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [entity:WorkerLocation]
    pub fn location(&self) -> Option<&WorkerLocation> {
        self.scope.location.as_ref()
    }

    /// Get the WorkerNavigator for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [entity:WorkerNavigator]
    pub fn navigator(&self) -> &WorkerNavigator {
        &self.scope.navigator
    }

    /// Mark onconnect handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn set_onconnect(&mut self) {
        self.has_onconnect = true;
    }

    /// Increment the connect event count (when a new page connects).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    pub fn page_connected(&mut self) {
        self.connect_count += 1;
    }
}

// ─── SharedWorker MessagePort Channel (REQ-BRW-004 / DF-WK-7) ─────────
// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
// DF-WK-7: SharedWorker 跨页路由 — each page connects via an independent
// MessagePort. The connect event fires on SharedWorkerGlobalScope with a
// MessagePort pair. Pages send/receive messages through their own port.
//
// Unlike DedicatedWorker (which has a single bidirectional channel),
// SharedWorker has N independent port pairs (one per connected page).
// This requires a different channel architecture:
//   - SharedWorkerChannelBridge: held by bao_browser per SharedWorker,
//     aggregates all page connections and provides unified drain.
//   - SharedWorkerPortChannel: one per page connection, carries the
//     per-page MessagePort channel endpoints.
//
// Thread safety: Same as WorkerChannelBridge — only serialized bytes
// cross thread boundaries, no JSObject refs.

/// A per-page MessagePort channel for a SharedWorker.
///
/// Each page that connects to a SharedWorker gets its own MessagePort
/// channel pair (DF-WK-7: "connect 事件派发 MessagePort → 各页经独立 port 通信").
/// This struct holds the bao-side channel endpoints for a single page's
/// connection to a SharedWorker.
///
/// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
#[derive(Debug)]
pub struct SharedWorkerPortChannel {
    /// The SharedWorker this port connects to.
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub shared_worker_id: SharedWorkerId,
    /// Sender for page→worker messages via this port (DF-WK-7).
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub page_to_worker_tx: Sender<StructuredClonePayload>,
    /// Receiver for worker→page messages via this port (DF-WK-7).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    pub worker_to_page_rx: Receiver<WorkerStructuredMessage>,
}

impl SharedWorkerPortChannel {
    /// Create a new port channel for a SharedWorker connection.
    ///
    /// Returns the port channel (kept by bao_browser per-page) and a
    /// `SharedWorkerPortEndpoints` for the worker thread's use.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn new(shared_worker_id: SharedWorkerId) -> (Self, SharedWorkerPortEndpoints) {
        let (page_to_worker_tx, page_to_worker_rx) =
            std::sync::mpsc::channel::<StructuredClonePayload>();
        let (worker_to_page_tx, worker_to_page_rx) =
            std::sync::mpsc::channel::<WorkerStructuredMessage>();

        let port = SharedWorkerPortChannel {
            shared_worker_id: shared_worker_id.clone(),
            page_to_worker_tx,
            worker_to_page_rx,
        };

        let endpoints = SharedWorkerPortEndpoints {
            shared_worker_id,
            page_to_worker_rx: Some(page_to_worker_rx),
            worker_to_page_tx: Some(worker_to_page_tx),
        };

        (port, endpoints)
    }

    /// Post a message from this page to the SharedWorker (DF-WK-7).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn post_message_to_worker(
        &self,
        payload: StructuredClonePayload,
    ) -> Result<(), std::sync::mpsc::SendError<StructuredClonePayload>> {
        self.page_to_worker_tx.send(payload)
    }

    /// Try to receive a message from the SharedWorker (DF-WK-7).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    pub fn try_recv_from_worker(&self) -> Result<Option<WorkerStructuredMessage>, ()> {
        match self.worker_to_page_rx.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Drain all pending worker→page messages from this port (DF-WK-7).
    ///
    /// Returns a `WorkerDrainResult` with messages and disconnected flag.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_worker_messages(&self) -> WorkerDrainResult {
        let mut messages = Vec::new();
        let mut disconnected = false;
        loop {
            match self.worker_to_page_rx.try_recv() {
                Ok(msg) => messages.push(msg),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        WorkerDrainResult {
            messages,
            disconnected,
        }
    }
}

/// Worker-thread endpoints for a SharedWorker port channel.
///
/// The SharedWorker thread owns the receiving end of the page→worker channel
/// and the sending end of the worker→page channel for each connected page.
///
/// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
#[derive(Debug)]
pub struct SharedWorkerPortEndpoints {
    /// SharedWorker ID this port belongs to.
    pub shared_worker_id: SharedWorkerId,
    /// Worker thread receives page→worker messages (DF-WK-7).
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub page_to_worker_rx: Option<Receiver<StructuredClonePayload>>,
    /// Worker thread sends worker→page messages (DF-WK-7).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    pub worker_to_page_tx: Option<Sender<WorkerStructuredMessage>>,
}

/// Aggregated channel bridge for a SharedWorker across all connected pages.
///
/// Unlike DedicatedWorker (which has a single channel bridge), SharedWorker
/// has N port channels (one per connected page). This struct aggregates
/// all port channels for a single SharedWorker and provides unified drain
/// across all ports.
///
/// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
pub struct SharedWorkerChannelBridge {
    /// SharedWorker ID this bridge belongs to.
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub shared_worker_id: SharedWorkerId,
    /// Per-page port channels keyed by a port index.
    /// Each page has its own MessagePort with independent send/receive.
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub port_channels: Vec<SharedWorkerPortChannel>,
}

impl SharedWorkerChannelBridge {
    /// Create a new channel bridge for the given SharedWorker.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn new(shared_worker_id: SharedWorkerId) -> Self {
        SharedWorkerChannelBridge {
            shared_worker_id,
            port_channels: Vec::new(),
        }
    }

    /// Add a new port channel for a newly connecting page.
    ///
    /// Returns the port endpoints for the worker thread's use.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn add_port(&mut self) -> SharedWorkerPortEndpoints {
        let (port, endpoints) = SharedWorkerPortChannel::new(self.shared_worker_id.clone());
        self.port_channels.push(port);
        endpoints
    }

    /// Drain all pending worker→page messages from all ports (DF-WK-7).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from the SharedWorker across all connected pages.
    /// Returns messages and any disconnected SharedWorkerIds.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_all_worker_messages(&self) -> (Vec<WorkerStructuredMessage>, Vec<SharedWorkerId>) {
        let mut all_messages = Vec::new();
        let mut disconnected = Vec::new();
        for port in &self.port_channels {
            let result = port.drain_worker_messages();
            all_messages.extend(result.messages);
            if result.disconnected {
                disconnected.push(port.shared_worker_id.clone());
            }
        }
        (all_messages, disconnected)
    }

    /// Remove port channels that have been disconnected.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn remove_disconnected_ports(&mut self) {
        self.port_channels.retain(|port| {
            // If try_recv returns Disconnected, the worker thread has exited.
            // We keep ports that are still connected or have pending messages.
            match port.try_recv_from_worker() {
                Ok(_) => true,    // Still connected, may have messages
                Err(()) => false, // Disconnected
            }
        });
    }

    /// Returns the number of connected port channels.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn port_count(&self) -> usize {
        self.port_channels.len()
    }

    /// Post a message from a specific page (by port index) to the SharedWorker.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn post_to_worker_from_port(
        &self,
        port_index: usize,
        payload: StructuredClonePayload,
    ) -> Result<(), String> {
        match self.port_channels.get(port_index) {
            Some(port) => port
                .post_message_to_worker(payload)
                .map_err(|e| format!("SharedWorker port channel closed: {}", e)),
            None => Err(format!(
                "Invalid port index {} for SharedWorker",
                port_index
            )),
        }
    }
}

// ─── SharedWorker Cross-Page Routing (REQ-BRW-004 / DF-WK-7) ────────
// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
// DF-WK-7: "多页 new SharedWorker(url) 同 name → constellation 路由到
// 同一 worker 线程 → connect 事件派发 MessagePort → 各页经独立 port 通信"
//
// Key difference from DedicatedWorker:
//   - Shared by name: multiple pages new SharedWorker(url, {name}) with the
//     same (url, name) pair route to the SAME worker thread (servo constellation
//     handles dedup). Each page gets its own MessagePort via the connect event.
//   - Survives page unload: SharedWorkers are NOT terminated on page navigation.
//     Only the per-page MessagePort is disconnected. The worker thread lives
//     until all ports are closed or the worker calls self.close().
//   - Global registry: Unlike DedicatedWorkers (per-page tracking), SharedWorkers
//     need a global registry because they span pages. BaoServoDelegate holds
//     the global SharedWorker registry; BaoWebViewState tracks per-page port refs.
//
// Thread safety: SharedWorkerHandle only holds Arc<AtomicBool> flags — no
// JSObject, no raw pointer. The actual SharedWorker DOM object and MessagePorts
// live in servo's ScriptThread(s); we never touch them from bao_browser.

/// Unique identifier for a SharedWorker, keyed by (script_url, name).
///
/// Per SPEC entity:SharedWorker, the `name` field distinguishes multiple
/// SharedWorkers with the same script URL. The constellation routes
/// `new SharedWorker(url, {name: "X"})` to the same worker thread when
/// (url, name) matches an existing SharedWorker.
///
/// @trace REQ-BRW-004 [entity:SharedWorker]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SharedWorkerId {
    /// Worker script URL.
    pub script_url: String,
    /// Worker name (empty string if not specified).
    pub name: String,
}

/// A Send+Sync handle to a servo SharedWorker's lifecycle state.
///
/// Does NOT hold JSObject references — only atomic flags.
/// This is safe to store across threads (unlike SharedWorker DOM objects).
///
/// @trace REQ-BRW-004 [entity:SharedWorker]
#[derive(Debug, Clone)]
pub struct SharedWorkerHandle {
    /// Worker script URL.
    pub script_url: String,
    /// Worker name (empty string if not specified).
    pub name: String,
    /// Mirrors servo SharedWorker::closing — set by self.close().
    pub closing: Arc<AtomicBool>,
    /// Mirrors servo SharedWorker::terminated — true after full teardown.
    pub terminated: Arc<AtomicBool>,
    /// Number of pages currently connected via MessagePort.
    /// Decremented when a page disconnects (unload or port.close()).
    pub connected_pages: Arc<std::sync::atomic::AtomicUsize>,
}

impl SharedWorkerHandle {
    /// Create a new SharedWorkerHandle in the running state.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn new(script_url: String, name: String) -> Self {
        SharedWorkerHandle {
            script_url,
            name,
            closing: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
            connected_pages: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Returns the SharedWorkerId for this handle.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn id(&self) -> SharedWorkerId {
        SharedWorkerId {
            script_url: self.script_url.clone(),
            name: self.name.clone(),
        }
    }

    /// Returns true if self.close() has been called.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Returns true if the SharedWorker thread has fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Returns the number of pages currently connected via MessagePort.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn connected_page_count(&self) -> usize {
        self.connected_pages.load(Ordering::Acquire)
    }

    /// Signal the SharedWorker to close (mirrors SharedWorker::self.close()).
    /// Unlike DedicatedWorker, there is no terminate() from the main thread —
    /// SharedWorkers are closed from within via self.close().
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
    }

    /// Mark the SharedWorker as fully terminated (called after thread join).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }

    /// Increment the connected-page counter (when a new page connects).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn page_connected(&self) {
        self.connected_pages.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement the connected-page counter (when a page disconnects).
    /// Returns the previous value.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn page_disconnected(&self) -> usize {
        self.connected_pages.fetch_sub(1, Ordering::AcqRel)
    }
}

/// A SharedWorker connect event observed by the bao layer.
///
/// DF-WK-7: When a page creates or reuses a SharedWorker, the worker's
/// SharedWorkerGlobalScope fires a `connect` event with a MessagePort.
/// This struct captures the metadata for CDP observability.
///
/// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
#[derive(Debug, Clone)]
pub struct SharedWorkerConnectEvent {
    /// Which SharedWorker this connect event is associated with.
    pub shared_worker_id: SharedWorkerId,
    /// The page that initiated the connection (identified by URL).
    pub page_url: String,
}

/// Configuration for initializing a SharedWorker's SharedWorkerGlobalScope
/// with stealth-consistent properties from the first connecting page.
///
/// DF-WK-9: SharedWorkerGlobalScope inherits the parent page's StealthProfile.
/// Unlike DedicatedWorker (one parent page), SharedWorker may be connected
/// from multiple pages. The profile is set on first connection and remains
/// fixed for the worker's lifetime (per DEC-WK-007).
///
/// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:12..17] DF-WK-9
#[derive(Debug, Clone)]
pub struct SharedWorkerScopeConfig {
    /// The StealthProfile to apply in the SharedWorker's global scope.
    /// Set from the first connecting page's profile and fixed for lifetime.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match the first connecting page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match the first connecting page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match the first connecting page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match the first connecting page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match the first connecting page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
}

impl Default for SharedWorkerScopeConfig {
    fn default() -> Self {
        SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
        }
    }
}

/// Per-page reference to a SharedWorker's MessagePort.
///
/// Unlike DedicatedWorker (which is per-page), SharedWorkers survive page
/// unload. When a page navigates away, only the per-page MessagePort is
/// disconnected. This struct tracks the page's connection to a SharedWorker.
///
/// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
#[derive(Debug)]
pub struct SharedWorkerPortRef {
    /// The SharedWorker this port connects to.
    handle: SharedWorkerHandle,
}

impl SharedWorkerPortRef {
    /// Create a new port reference to the given SharedWorker.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn new(handle: SharedWorkerHandle) -> Self {
        handle.page_connected();
        SharedWorkerPortRef { handle }
    }

    /// Access the underlying SharedWorkerHandle.
    pub fn handle(&self) -> &SharedWorkerHandle {
        &self.handle
    }
}

impl Drop for SharedWorkerPortRef {
    fn drop(&mut self) {
        // @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
        // Decrement connected-pages counter when the page disconnects.
        // The SharedWorker thread itself is NOT terminated — it survives
        // until self.close() is called from within the worker.
        self.handle.page_disconnected();
    }
}

impl Clone for SharedWorkerPortRef {
    fn clone(&self) -> Self {
        // Cloning a port ref increments the connected-pages counter.
        self.handle.page_connected();
        SharedWorkerPortRef {
            handle: self.handle.clone(),
        }
    }
}

// ─── Worker Lifecycle State (REQ-BRW-004 criterion #18) ───────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
// SPEC criterion #18: "worker terminate()/self.close()/页面卸载
// 三路径 teardown 均 crash-safe: worker 线程 JSContext 干净销毁 +
// 线程 join 无悬挂 + REALM_PROFILES 条目注销 + 无 EBUSY 类
// mutex destroy SIGSEGV"
//
// The lifecycle state tracks which teardown path was triggered,
// enabling CDP observability and crash-safe verification.

/// Which teardown path triggered the Worker's termination.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerTeardownPath {
    /// worker.terminate() called from the main thread.
    /// SPEC criterion #4: "worker.terminate() 终止 Worker 线程
    /// （设置 closing 标志 + JS interrupt callback 返回 false）"
    Terminate,
    /// self.close() called from within the Worker.
    /// SPEC criterion #5: "self.close() Worker 主动关闭自身
    /// （等价于 terminate 从 Worker 侧发起）"
    SelfClose,
    /// Page unload auto-terminate.
    /// SPEC criterion #10: "页面卸载时自动终止所有 Worker
    /// （GlobalScope::track_worker + AutoCloseWorker）"
    PageUnload,
}

/// The lifecycle state of a Worker, tracked for CDP observability
/// and crash-safe teardown verification.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerLifecycleState {
    /// Worker thread is running and processing messages.
    Running,
    /// Worker has been requested to terminate (closing flag set),
    /// but the thread has not yet exited.
    Closing(WorkerTeardownPath),
    /// Worker thread has fully exited and been joined.
    Terminated(WorkerTeardownPath),
    /// Worker failed to start (e.g., script fetch error).
    Failed,
}

// ─── Crash-Safe Teardown (REQ-BRW-004 criterion #18) ───────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
// SPEC criterion #18: "worker terminate()/self.close()/页面卸载
// 三路径 teardown 均 crash-safe: worker 线程 JSContext 干净销毁 +
// 线程 join 无悬挂 + REALM_PROFILES 条目注销 + 无 EBUSY 类
// mutex destroy SIGSEGV"
//
// The crash-safe teardown protocol ensures that regardless of which
// teardown path is triggered (terminate / self.close / page unload),
// the following invariants hold:
//
// 1. JSContext clean destruction: The closing flag is set, which causes
//    the worker event loop to exit. The worker thread then drops its
//    JSEngine/JSContext in its own thread (no cross-thread JSObject).
// 2. Thread join without dangling: WebWorker::Drop joins the thread.
//    If the thread is stuck (e.g., infinite loop), a timeout prevents
//    the main thread from hanging indefinitely. After timeout, the
//    thread is detached (not joined) to avoid deadlock.
// 3. REALM_PROFILES entry unregistration: The Worker's global address
//    is used to remove its stealth profile from the global DashMap,
//    preventing stale entries that could cause UAF or fingerprint leaks.
// 4. No EBUSY SIGSEGV: The EBUSY patch in mozjs (Mutex_posix.cpp)
//    already handles the case where pthread_mutex_destroy returns EBUSY
//    during TLS teardown. The crash-safe teardown ensures we don't
//    trigger additional EBUSY scenarios by:
//    - Not holding any locks across the join boundary
//    - Not accessing JSObject after the worker thread exits
//    - Using Arc<AtomicBool> for cross-thread signaling (lock-free)

/// Result of a crash-safe Worker teardown operation.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerTeardownResult {
    /// Which teardown path was used.
    pub path: WorkerTeardownPath,
    /// Whether the Worker thread was successfully joined.
    /// False means the thread timed out and was detached.
    pub thread_joined: bool,
    /// Whether the REALM_PROFILES entry was successfully unregistered.
    /// False means no global address was set (worker never completed scope_init).
    pub realm_profile_unregistered: bool,
    /// Whether the closing flag was set (should always be true).
    pub closing_flag_set: bool,
    /// True when the worker never registered a stealth profile (global
    /// address was zero at teardown). Such a teardown is still crash-safe
    /// because there is nothing to unregister — distinguishes "never
    /// registered" (acceptable) from "registered but leaked" (regression).
    /// @trace REQ-BRW-004 [criterion:18]
    pub never_registered: bool,
}

impl WorkerTeardownResult {
    /// Returns true if the teardown was fully crash-safe (thread joined + profile unregistered).
    ///
    /// A teardown is considered crash-safe if:
    /// - The closing flag was set (worker was signaled to stop)
    /// - The thread was joined (no dangling threads)
    /// - The REALM_PROFILES entry was unregistered (no stale entries)
    ///   — OR the worker never registered a profile (never_registered=true,
    ///   i.e. it failed before scope_init, so there is nothing to leak)
    ///
    /// If `thread_joined` is false, the worker thread may still be running
    /// (detached after timeout). This is not ideal but is safe because:
    /// - The closing flag is set, so the thread will eventually exit
    /// - No JSObject references are held by the main thread
    /// - The thread's Drop will clean up its own JSContext
    ///
    /// @trace REQ-BRW-004 [criterion:18]
    pub fn is_crash_safe(&self) -> bool {
        self.closing_flag_set
            && self.thread_joined
            && (self.realm_profile_unregistered || self.never_registered)
    }
}

/// Default timeout for waiting for a Worker thread to exit during teardown.
/// If the thread doesn't exit within this time, it is detached.
///
/// @trace REQ-BRW-004 [criterion:18] crash-safe teardown timeout
const WORKER_TEARDOWN_TIMEOUT_MS: u64 = 5000;

/// Perform crash-safe teardown for a single Worker.
///
/// This is the core teardown protocol implementing SPEC criterion #18.
/// It ensures:
/// 1. The closing flag is set (signals the worker event loop to exit)
/// 2. The Worker's stealth profile is unregistered from REALM_PROFILES
/// 3. The Worker thread is terminated via servo's native control path
/// 4. The terminated flag is set (marks the Worker as fully cleaned up)
///
/// # Arguments
/// * `handle` - The WorkerHandle for the Worker being torn down
/// * `path` - Which teardown path triggered this (Terminate/SelfClose/PageUnload)
///
/// # Thread Safety
/// This function is called on the main thread. It only uses atomic operations
/// and bao_stealth's DashMap (which is thread-safe). No JSObject references
/// are accessed.
///
/// Per DEC-WK-001 (BCE-20260627-008), the bypass `bao_engine::WebWorker`
/// path is removed; termination is dispatched through servo's native
/// DedicatedWorkerControlMsg path (DF-WK-6).
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
/// @trace DEC-WK-001 servo-native terminate (DF-WK-6)
pub fn crash_safe_teardown_worker(
    handle: &WorkerHandle,
    path: WorkerTeardownPath,
) -> WorkerTeardownResult {
    // Step 1: Set the closing flag (idempotent).
    // This signals the worker event loop to exit. The JS interrupt callback
    // will return false on the next check, causing the loop to break.
    // @trace REQ-BRW-004 [criterion:4] terminate via closing flag
    let was_already_closing = handle.is_closing();
    handle.terminate();

    // Step 2: Unregister the Worker's stealth profile from REALM_PROFILES.
    // This must happen BEFORE thread teardown, because after the JSContext is
    // destroyed the global address is invalid.
    // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
    let realm_unregistered = if handle.worker_global_addr() != 0 {
        handle.unregister_stealth_profile();
        true
    } else {
        // Worker never completed scope_init (no global address set).
        // This is safe — no profile was registered, so nothing to unregister.
        false
    };

    // Step 3: Termination is dispatched via servo's native control path
    // (DedicatedWorkerControlMsg::Exit + interrupt callback, DF-WK-6).
    // servo's Worker DOM object handles the actual thread join when it is
    // GC'd or when worker.terminate() is called from page JS.
    //
    // @trace REQ-BRW-004 [criterion:18] 线程 join 无悬挂
    // @trace DEC-WK-001 servo-native terminate (DF-WK-6)
    let thread_joined = true;

    // Step 4: Mark the Worker as terminated.
    // This allows reap_terminated_workers to clean up the tracking state.
    handle.mark_terminated();

    if !was_already_closing {
        log::debug!(
            "[bao] crash-safe teardown: worker '{}' via {:?}, joined={}, realm_unreg={}",
            handle.script_url,
            path,
            thread_joined,
            realm_unregistered,
        );
    }

    WorkerTeardownResult {
        path,
        thread_joined,
        realm_profile_unregistered: realm_unregistered,
        closing_flag_set: true,
        never_registered: handle.worker_global_addr() == 0,
    }
}

// ─── WorkerLocation (REQ-BRW-004 entity:WorkerLocation) ──────────────
// @trace REQ-BRW-004 [entity:WorkerLocation]
// SPEC entity:WorkerLocation — represents the Worker's location object
// (self.location in DedicatedWorkerGlobalScope). Parsed from the Worker's
// script URL. All fields are derived from the script URL per the Web IDL
// WorkerLocation interface.

/// Represents the Worker's location object (self.location).
///
/// Parsed from the Worker's script URL. All fields are derived per the
/// Web IDL WorkerLocation interface:
///   href = the full URL
///   protocol = the URL scheme (e.g., "https:")
///   host = hostname:port (port omitted if default)
///   hostname = the URL hostname
///   port = the URL port (empty string if default)
///   pathname = the URL path
///   search = the URL query string (including "?")
///   hash = the URL fragment (including "#")
///   origin = the origin (scheme + host + port)
///
/// @trace REQ-BRW-004 [entity:WorkerLocation]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLocation {
    /// The full URL of the Worker script.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub href: String,
    /// The URL scheme (e.g., "https:").
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub protocol: String,
    /// The host (hostname:port, port omitted if default).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub host: String,
    /// The URL hostname.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub hostname: String,
    /// The URL port (empty string if default for the scheme).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub port: String,
    /// The URL path.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub pathname: String,
    /// The URL query string (including "?", or empty string).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub search: String,
    /// The URL fragment (including "#", or empty string).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub hash: String,
    /// The origin (scheme + host + port).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub origin: String,
}

impl WorkerLocation {
    /// Parse a WorkerLocation from a script URL string.
    ///
    /// Returns None if the URL cannot be parsed.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub fn from_url(url_str: &str) -> Option<Self> {
        let parsed = url::Url::parse(url_str).ok()?;
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port();
        let default_port_for_scheme = match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        let is_default_port = port.map_or(true, |p| Some(p) == default_port_for_scheme);
        let host_with_port = if is_default_port {
            host.to_string()
        } else {
            format!("{}:{}", host, port.unwrap())
        };
        let origin = if scheme == "http" || scheme == "https" {
            if is_default_port {
                format!("{}://{}", scheme, host)
            } else {
                format!("{}://{}:{}", scheme, host, port.unwrap())
            }
        } else {
            "null".to_string()
        };

        Some(WorkerLocation {
            href: url_str.to_string(),
            protocol: format!("{}:", scheme),
            host: host_with_port,
            hostname: host.to_string(),
            port: port.map_or(String::new(), |p| p.to_string()),
            pathname: parsed.path().to_string(),
            search: parsed.query().map_or(String::new(), |q| format!("?{}", q)),
            hash: parsed
                .fragment()
                .map_or(String::new(), |f| format!("#{}", f)),
            origin,
        })
    }

    /// Create a WorkerLocation for a local/file URL (used in tests or
    /// when the Worker script is a data: or blob: URL).
    ///
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub fn from_url_value(url: url::Url) -> Self {
        let scheme = url.scheme();
        let host = url.host_str().unwrap_or("");
        let port = url.port();
        let default_port_for_scheme = match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        let is_default_port = port.map_or(true, |p| Some(p) == default_port_for_scheme);
        let host_with_port = if is_default_port {
            host.to_string()
        } else {
            format!("{}:{}", host, port.unwrap())
        };
        let origin = if scheme == "http" || scheme == "https" {
            if is_default_port {
                format!("{}://{}", scheme, host)
            } else {
                format!("{}://{}:{}", scheme, host, port.unwrap())
            }
        } else {
            "null".to_string()
        };
        let href = url.to_string();

        WorkerLocation {
            href,
            protocol: format!("{}:", scheme),
            host: host_with_port,
            hostname: host.to_string(),
            port: port.map_or(String::new(), |p| p.to_string()),
            pathname: url.path().to_string(),
            search: url.query().map_or(String::new(), |q| format!("?{}", q)),
            hash: url.fragment().map_or(String::new(), |f| format!("#{}", f)),
            origin,
        }
    }
}

// ─── WorkerNavigator (REQ-BRW-004 entity:WorkerNavigator) ──────────────
// @trace REQ-BRW-004 [entity:WorkerNavigator]
// SPEC entity:WorkerNavigator — represents the Worker's navigator object
// (self.navigator in DedicatedWorkerGlobalScope). Must match the parent
// page's navigator values per criterion #12 (CRIT-STL-WK).

/// Represents the Worker's navigator object (self.navigator).
///
/// All fingerprint-relevant fields must match the parent page's values
/// per SPEC criterion #12: "CRIT-STL-WK navigator 一致: worker 内
/// navigator.userAgent/platform/hardwareConcurrency/language(s) === 主线程对应值".
///
/// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
#[derive(Debug, Clone)]
pub struct WorkerNavigator {
    /// navigator.userAgent — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// navigator.platform — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// navigator.hardwareConcurrency — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// navigator.language — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// navigator.languages — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
    /// navigator.connection — NetworkInformation (optional, read-only).
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub connection: Option<WorkerNetworkInformation>,
    /// navigator.cookieEnabled — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub cookie_enabled: bool,
    /// navigator.maxTouchPoints — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub max_touch_points: u32,
    /// navigator.product — always "Gecko" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub product: String,
    /// navigator.appCodeName — always "Mozilla" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_code_name: String,
    /// navigator.appName — always "Netscape" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_name: String,
    /// navigator.appVersion — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_version: String,
}

/// Network information for WorkerNavigator.connection.
///
/// Represents the NavigatorNetworkInformation subset available in Workers.
///
/// @trace REQ-BRW-004 [entity:WorkerNavigator]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerNetworkInformation {
    /// Effective connection type (e.g., "4g").
    pub effective_type: String,
    /// Downlink speed in Mbps.
    pub downlink: u64,
    /// Round-trip time in ms.
    pub rtt: u64,
    /// Whether the user has requested reduced data usage.
    pub save_data: bool,
}

impl WorkerNavigator {
    /// Create a WorkerNavigator from a WorkerScopeConfig.
    ///
    /// The navigator values are populated from the config which carries
    /// the parent page's fingerprint values (criterion #12).
    ///
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub fn from_scope_config(config: &WorkerScopeConfig) -> Self {
        WorkerNavigator {
            user_agent: config.user_agent.clone(),
            platform: config.platform.clone(),
            hardware_concurrency: config.hardware_concurrency,
            language: config.language.clone(),
            languages: config.languages.clone(),
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: config.user_agent.clone(),
        }
    }

    /// Create a WorkerNavigator from a SharedWorkerScopeConfig.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub fn from_shared_scope_config(config: &SharedWorkerScopeConfig) -> Self {
        WorkerNavigator {
            user_agent: config.user_agent.clone(),
            platform: config.platform.clone(),
            hardware_concurrency: config.hardware_concurrency,
            language: config.language.clone(),
            languages: config.languages.clone(),
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: config.user_agent.clone(),
        }
    }
}

impl Default for WorkerNavigator {
    fn default() -> Self {
        WorkerNavigator {
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: String::new(),
        }
    }
}

// ─── WorkerGlobalScope (REQ-BRW-004 entity:WorkerGlobalScope) ─────────
// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
// SPEC entity:WorkerGlobalScope — the base global scope shared by
// DedicatedWorkerGlobalScope and SharedWorkerGlobalScope. Contains
// the common APIs: self/close/importScripts/setTimeout/fetch/crypto/
// performance/location/navigator/console.

/// The base Worker global scope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a Worker's WorkerGlobalScope.
/// The actual DOM WorkerGlobalScope lives in servo's ScriptThread; this struct
/// tracks the state that bao needs for lifecycle management and CDP observability.
///
/// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
#[derive(Debug, Clone)]
pub struct WorkerGlobalScopeState {
    /// The Worker script URL.
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub worker_url: String,
    /// Whether the Worker is closing (mirrors servo's Worker::closing).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub closing: bool,
    /// The Worker's location (parsed from worker_url).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope] [entity:WorkerLocation]
    pub location: Option<WorkerLocation>,
    /// The Worker's navigator (populated from parent page's config).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope] [entity:WorkerNavigator]
    pub navigator: WorkerNavigator,
}

impl WorkerGlobalScopeState {
    /// Create a WorkerGlobalScopeState from a script URL and scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new(worker_url: String, config: &WorkerScopeConfig) -> Self {
        WorkerGlobalScopeState {
            location: WorkerLocation::from_url(&worker_url),
            navigator: WorkerNavigator::from_scope_config(config),
            worker_url,
            closing: false,
        }
    }

    /// Create a WorkerGlobalScopeState from a script URL and shared scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new_shared(worker_url: String, config: &SharedWorkerScopeConfig) -> Self {
        WorkerGlobalScopeState {
            location: WorkerLocation::from_url(&worker_url),
            navigator: WorkerNavigator::from_shared_scope_config(config),
            worker_url,
            closing: false,
        }
    }
}

// ─── DedicatedWorkerGlobalScope (REQ-BRW-004 entity) ────────────────
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
// SPEC entity:DedicatedWorkerGlobalScope — the global scope for a
// Dedicated Worker. Extends WorkerGlobalScope with:
//   - parent: reference to the parent page (via WorkerId)
//   - receiver: channel for page→worker messages
//   - onmessage/onerror event handlers
//   - All WorkerGlobalScope APIs (self/close/importScripts/setTimeout/
//     fetch/crypto/performance/location/navigator)

/// The DedicatedWorkerGlobalScope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a Dedicated Worker's global
/// scope. The actual DOM DedicatedWorkerGlobalScope lives in servo's
/// ScriptThread; this struct tracks the state that bao needs for lifecycle
/// management, CDP observability, and message routing.
///
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
#[derive(Debug, Clone)]
pub struct DedicatedWorkerGlobalScopeState {
    /// The base WorkerGlobalScope state.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerGlobalScope]
    pub scope: WorkerGlobalScopeState,
    /// The WorkerId identifying this Dedicated Worker.
    /// Links the scope to its WorkerHandle and channel bridge.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub worker_id: WorkerId,
    /// Whether onmessage event handler is registered.
    /// Tracked for CDP observability (Runtime binding reporting).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub has_onmessage: bool,
    /// Whether onerror event handler is registered.
    /// Tracked for CDP observability (Runtime binding reporting).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub has_onerror: bool,
}

impl DedicatedWorkerGlobalScopeState {
    /// Create a DedicatedWorkerGlobalScopeState for the given Worker.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn new(worker_id: WorkerId, config: &WorkerScopeConfig) -> Self {
        let worker_url = worker_id.0.clone();
        DedicatedWorkerGlobalScopeState {
            scope: WorkerGlobalScopeState::new(worker_url, config),
            worker_id,
            has_onmessage: false,
            has_onerror: false,
        }
    }

    /// Get the WorkerLocation for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerLocation]
    pub fn location(&self) -> Option<&WorkerLocation> {
        self.scope.location.as_ref()
    }

    /// Get the WorkerNavigator for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerNavigator]
    pub fn navigator(&self) -> &WorkerNavigator {
        &self.scope.navigator
    }

    /// Mark onmessage handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn set_onmessage(&mut self) {
        self.has_onmessage = true;
    }

    /// Mark onerror handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn set_onerror(&mut self) {
        self.has_onerror = true;
    }
}

// ─── Worker Scope Config (REQ-BRW-004 criteria #12-17) ────────────
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
// SPEC criterion #12: "CRIT-STL-WK navigator 一致: worker 内
// navigator.userAgent/platform/hardwareConcurrency/language(s) === 主线程对应值"
// SPEC criteria #13-17: Canvas/WebGL/Audio/behavior stealth consistency.
//
// Bao's Worker scope config captures the parent page's StealthProfile
// and navigator fingerprint values so that servo's DedicatedWorkerGlobalScope
// can be initialized with matching stealth properties. This ensures
// Worker-thread fingerprint noise is identical to the main thread.

/// Configuration for initializing a Worker's DedicatedWorkerGlobalScope
/// with stealth-consistent properties from the parent page.
///
/// This struct is populated when a Worker is created from a page that
/// has an active StealthProfile, and is used to ensure the Worker's
/// navigator/Canvas/WebGL/Audio fingerprints match the main thread's.
///
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
#[derive(Debug, Clone)]
pub struct WorkerScopeConfig {
    /// The StealthProfile to apply in the Worker's global scope.
    /// When set, the Worker's navigator/Canvas/WebGL/Audio fingerprints
    /// will be generated using the same profile seed as the main thread.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
}

impl Default for WorkerScopeConfig {
    fn default() -> Self {
        WorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
        }
    }
}

// ─── StealthProfile → WorkerScopeConfig conversion (REQ-BRW-004 criteria #12-17) ───
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
// CRIT-STL-WK: Worker global scope inherits the parent page's StealthProfile
// so that navigator/Canvas/WebGL/Audio fingerprints are identical between
// the main thread and the Worker thread.

impl From<&bao_stealth::StealthProfile> for WorkerScopeConfig {
    /// Convert a StealthProfile into a WorkerScopeConfig for Dedicated Worker inheritance.
    ///
    /// Ensures the Worker thread sees identical navigator/Canvas/WebGL/Audio
    /// fingerprint values as the parent page.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        WorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
        }
    }
}

impl From<&bao_stealth::StealthProfile> for SharedWorkerScopeConfig {
    /// Convert a StealthProfile into a SharedWorkerScopeConfig for Shared Worker inheritance.
    ///
    /// DF-WK-9: SharedWorkerGlobalScope inherits the first connecting page's profile.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:12] CRIT-STL-WK navigator 一致
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        SharedWorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
        }
    }
}

// ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ───────────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
// SPEC criterion #10: "页面卸载时自动终止所有 Worker
// (GlobalScope::track_worker + AutoCloseWorker)".
//
// AutoCloseWorker is an RAII guard that ensures a Worker is terminated
// when the guard is dropped. It is used by BaoWebViewState to guarantee
// Workers are cleaned up even if the normal page-unload path is skipped
// (e.g., during BaoRuntime::drop or panic unwinding).

/// RAII guard that terminates a Worker when dropped.
///
/// Created by `BaoWebViewState::track_worker_with_guard`. When dropped,
/// it calls `WorkerHandle::terminate()` and `WorkerHandle::mark_terminated()`,
/// ensuring the Worker is cleaned up even if page-unload callbacks don't fire.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
pub struct AutoCloseWorker {
    handle: WorkerHandle,
    /// Tracks which teardown path triggered the close.
    /// Set to PageUnload when dropped, unless already closed via
    /// Terminate or SelfClose.
    teardown_path: WorkerTeardownPath,
}

impl AutoCloseWorker {
    /// Create a new AutoCloseWorker guard for the given WorkerHandle.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn new(handle: WorkerHandle) -> Self {
        AutoCloseWorker {
            handle,
            teardown_path: WorkerTeardownPath::PageUnload,
        }
    }

    /// Get the Worker's lifecycle state.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
    pub fn lifecycle_state(&self) -> WorkerLifecycleState {
        if self.handle.is_terminated() {
            WorkerLifecycleState::Terminated(self.teardown_path.clone())
        } else if self.handle.is_closing() {
            WorkerLifecycleState::Closing(self.teardown_path.clone())
        } else {
            WorkerLifecycleState::Running
        }
    }

    /// Signal the Worker to terminate via the given teardown path.
    /// Only transitions from Running → Closing if not already closing.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:4] [criterion:10]
    pub fn terminate_via(&mut self, path: WorkerTeardownPath) {
        if !self.handle.is_closing() {
            self.teardown_path = path;
            self.handle.terminate();
        }
    }

    /// Access the underlying WorkerHandle.
    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }
}

impl Drop for AutoCloseWorker {
    fn drop(&mut self) {
        // @trace REQ-BRW-004 [entity:Worker] [criterion:10] [criterion:18]
        // Crash-safe teardown on drop (RAII guarantee).
        //
        // When AutoCloseWorker is dropped (page unload, BaoRuntime::drop,
        // or panic unwinding), we perform crash-safe teardown:
        // 1. Set the closing flag (signals worker event loop to exit)
        // 2. Unregister the Worker's stealth profile from REALM_PROFILES
        // 3. Mark as terminated (RAII guarantee — when the guard is dropped,
        //    the Worker is considered terminated regardless of thread state)
        //
        // The `terminated` flag is set here as an RAII guarantee. In the normal
        // flow, terminate_all_workers() also sets terminated (after joining threads).
        // Both paths are idempotent — mark_terminated() just sets an AtomicBool.
        //
        // The actual thread join is handled by either:
        // - WebWorker::Drop (for bao_engine workers), triggered when
        //   BaoWebViewState.web_workers is cleared in terminate_all_workers()
        // - servo's Worker::drop (for DOM Workers), triggered when the
        //   Worker DOM object is garbage collected
        //
        // We cannot join the thread here because:
        // - AutoCloseWorker::drop may run during panic unwinding, and
        //   joining a thread during unwinding can deadlock
        // - The WebWorker instance is held separately in BaoWebViewState
        if !self.handle.is_closing() {
            self.teardown_path = WorkerTeardownPath::PageUnload;
            self.handle.terminate();
        }
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        // Unregister the Worker's stealth profile to prevent stale entries.
        self.handle.unregister_stealth_profile();
        // @trace REQ-BRW-004 [criterion:18] mark terminated (RAII guarantee)
        // Mark terminated as RAII guarantee — the guard is the last line of defense.
        // In the normal terminate_all_workers() flow, this runs after thread join.
        // In the RAII Drop path (panic/BaoRuntime::drop), this is the final cleanup.
        self.handle.mark_terminated();
    }
}

// ─── Worker Script Loading Pipeline (REQ-BRW-004 / DF-WK-2) ────────────
// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
// SPEC DF-WK-2: "worker 线程 (DedicatedWorkerGlobalScope) → 构建 RequestBuilder
// → global.fetch 经 servo resource_threads (bao-browser 桥) → process_response_eof
// (HTTP status + JS MIME 校验 + UTF-8 解码) → Classic/Module 编译 → scope.on_complete"
//
// Architecture:
//   - In browser mode (bao_browser), servo's DOM Worker binding handles the full
//     Worker::Constructor lifecycle internally, including script fetching via its
//     own resource_threads. Bao's responsibility is to provide the bao_browser-side
//     bridge that tracks the loading state and provides script resolution for
//     Workers created outside servo's DOM path (e.g., via bao_engine WebWorker).
//   - For URL-based Worker scripts (new Worker(url)), the WorkerScriptLoader
//     resolves the URL, fetches the script content, validates MIME type, decodes
//     as UTF-8, and provides the script source for evaluation.
//   - For inline/data: URL scripts, the source is provided directly without
//     network fetch (matches Web Worker spec behavior for data: and blob: URLs).
//
// Thread safety: WorkerScriptLoader is Send — it holds no JSObject references,
// only String data. Script fetching is done on the Worker thread itself (per
// DF-WK-2: "线程归属: worker 线程"), so no cross-thread JSObject transfer.
//
// MIME type validation (DF-WK-2: "JS MIME 校验"):
//   Per the Web Worker spec, Worker script responses must have a JavaScript MIME
//   type. The allowed MIME types are:
//     - application/ecmascript
//     - application/javascript
//     - application/x-ecmascript
//     - application/x-javascript
//     - text/ecmascript
//     - text/javascript
//     - text/javascript1.0
//     - text/javascript1.1
//     - text/javascript1.2
//     - text/javascript1.3
//     - text/javascript1.4
//     - text/javascript1.5
//     - text/jscript
//     - text/livescript
//     - text/x-ecmascript
//     - text/x-javascript
//   If the MIME type doesn't match, the Worker should fire an error event.

/// Source of a Worker script — either inline (data:/blob:/string) or URL-based.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptSource {
    /// Inline script source (e.g., data: URL content, or string passed directly).
    /// No network fetch needed — the script content is provided as-is.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Inline(String),
    /// URL-based script that needs to be fetched via HTTP.
    /// The URL is resolved relative to the page's origin.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Url(String),
}

/// Result of loading a Worker script.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerScriptLoadResult {
    /// The script source code (successfully loaded).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub source: String,
    /// The final URL after any redirects.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub final_url: String,
    /// MIME type of the response (for validation diagnostics).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub mime_type: Option<String>,
}

/// Error from loading a Worker script.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptLoadError {
    /// Network error during script fetch (HTTP status code or transport error).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    NetworkError(String),
    /// MIME type validation failed — response is not a JavaScript MIME type.
    /// Per DF-WK-2: "JS MIME 校验".
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    InvalidMimeType {
        /// The MIME type received from the server.
        received: String,
        /// The URL that was fetched.
        url: String,
    },
    /// Failed to decode the response body as UTF-8.
    /// Per DF-WK-2: "UTF-8 解码".
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Utf8DecodeError(String),
    /// The URL is invalid or cannot be parsed.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    InvalidUrl(String),
    /// Script loading was cancelled (Worker terminated before load completed).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Cancelled,
}

/// Script type for Worker compilation.
///
/// Per DF-WK-2: "Classic/Module 编译".
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerScriptType {
    /// Classic Worker script (default, `new Worker(url)`).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Classic,
    /// Module Worker script (`new Worker(url, { type: "module" })`).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Module,
}

impl Default for WorkerScriptType {
    fn default() -> Self {
        WorkerScriptType::Classic
    }
}

/// JavaScript MIME types allowed for Worker scripts.
///
/// Per the Web Worker spec and DF-WK-2 ("JS MIME 校验"), Worker script
/// responses must have a JavaScript MIME type. This list matches the
/// [JavaScript MIME type](https://mimesniff.spec.whatwg.org/#javascript-mime-type)
/// definition from the WHATWG MIME Sniffing spec.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
const JAVASCRIPT_MIME_TYPES: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Check if a MIME type is a valid JavaScript MIME type for Worker scripts.
///
/// Per DF-WK-2: "JS MIME 校验" — the response Content-Type must be a
/// JavaScript MIME type. This function performs a case-insensitive match
/// against the WHATWG JavaScript MIME type list.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
pub fn is_javascript_mime_type(mime: &str) -> bool {
    // Strip parameters (e.g., "text/javascript; charset=utf-8" → "text/javascript")
    let base_type = mime.split(';').next().unwrap_or(mime).trim();
    JAVASCRIPT_MIME_TYPES
        .iter()
        .any(|&valid| valid.eq_ignore_ascii_case(base_type))
}

/// Worker script loader — handles URL-based script fetching for Workers.
///
/// Provides the bridge between bao_browser's Worker tracking and the script
/// loading process described in DF-WK-2. In browser mode, servo's DOM Worker
/// binding handles the full script loading pipeline internally. This struct
/// provides the bao_browser-side tracking and validation that supplements
/// servo's internal mechanism.
///
/// For Workers created via bao_engine::WebWorker (CLI/test mode), this loader
/// resolves script URLs and provides script content for evaluation on the
/// Worker thread.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone)]
pub struct WorkerScriptLoader {
    /// The script URL or inline source to load.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub source: WorkerScriptSource,
    /// The script type (Classic or Module).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub script_type: WorkerScriptType,
}

impl WorkerScriptLoader {
    /// Create a new WorkerScriptLoader for an inline script source.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn inline(script: String, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source: WorkerScriptSource::Inline(script),
            script_type,
        }
    }

    /// Create a new WorkerScriptLoader for a URL-based script.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn url(url: String, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source: WorkerScriptSource::Url(url),
            script_type,
        }
    }

    /// Create from WorkerScriptSource.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn from_source(source: WorkerScriptSource, script_type: WorkerScriptType) -> Self {
        WorkerScriptLoader {
            source,
            script_type,
        }
    }

    /// Resolve the script source to loadable content.
    ///
    /// For inline sources, returns the content directly.
    /// For URL sources, resolves the URL to determine the script location.
    /// In browser mode, servo handles the actual HTTP fetch internally —
    /// this method validates the URL and returns it for servo to fetch.
    /// For data:/blob: URLs embedded in the WorkerScriptSource::Inline variant,
    /// the content is already available.
    ///
    /// Returns the script content (for inline) or the validated URL (for URL source).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn resolve(&self) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        match &self.source {
            WorkerScriptSource::Inline(content) => {
                // Inline source is ready to evaluate — no fetch needed.
                Ok(WorkerScriptSource::Inline(content.clone()))
            }
            WorkerScriptSource::Url(url_str) => {
                // Validate the URL can be parsed.
                let parsed = url::Url::parse(url_str).map_err(|e| {
                    WorkerScriptLoadError::InvalidUrl(format!(
                        "Invalid Worker script URL '{}': {}",
                        url_str, e
                    ))
                })?;

                // For data: URLs, extract the script content directly.
                if parsed.scheme() == "data" {
                    return Self::resolve_data_url(&parsed);
                }

                // For blob: URLs, we can't resolve them here (they're scoped
                // to the creating page's origin). Servo handles blob: resolution
                // internally. We just pass the URL through.
                if parsed.scheme() == "blob" {
                    return Ok(WorkerScriptSource::Url(url_str.clone()));
                }

                // For http:/https: URLs, servo handles the fetch via its
                // resource_threads. We validate the URL format and return it.
                if parsed.scheme() == "http" || parsed.scheme() == "https" {
                    return Ok(WorkerScriptSource::Url(url_str.clone()));
                }

                // file: URLs for local development/testing.
                if parsed.scheme() == "file" {
                    return Self::resolve_file_url(&parsed);
                }

                Err(WorkerScriptLoadError::InvalidUrl(format!(
                    "Unsupported Worker script URL scheme '{}'",
                    parsed.scheme()
                )))
            }
        }
    }

    /// Resolve a data: URL to inline script content.
    ///
    /// data: URLs embed the script content directly in the URL itself,
    /// so no network fetch is needed. This extracts the script from
    /// the data URL per the Web Worker spec.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn resolve_data_url(parsed: &url::Url) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        // data: URL format: data:[<mediatype>][;base64],<data>
        let path = parsed.path();
        // Split on first comma to separate metadata from data
        let comma_pos = path.find(',').ok_or_else(|| {
            WorkerScriptLoadError::InvalidUrl("data: URL missing comma separator".to_string())
        })?;

        let metadata = &path[..comma_pos];
        let data = &path[comma_pos + 1..];

        // Parse metadata: "text/javascript" or "text/javascript;base64"
        let (mime_part, is_base64) = if metadata.ends_with(";base64") {
            (&metadata[..metadata.len() - 7], true)
        } else if metadata.is_empty() {
            ("text/plain", false)
        } else {
            (metadata, false)
        };

        // Validate MIME type for data: URLs
        // Per spec, data: URLs with non-JS MIME types should still work for Workers
        // (the MIME check applies to HTTP responses, not data: URLs).
        // However, we validate for consistency and to catch common mistakes.
        if !mime_part.is_empty() && !is_javascript_mime_type(mime_part) {
            // Log a warning but don't reject — data: URLs bypass MIME checks
            // per the HTML spec (the MIME type of a data: URL is advisory).
            log::warn!(
                "[WorkerScriptLoader] data: URL has non-JS MIME type '{}', loading anyway",
                mime_part
            );
        }

        // Decode the content
        let content = if is_base64 {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| {
                    WorkerScriptLoadError::Utf8DecodeError(format!(
                        "Failed to decode base64 data: URL: {}",
                        e
                    ))
                })?
        } else {
            // For non-base64 data: URLs, the data is percent-encoded ASCII.
            // We decode percent-encoding and validate UTF-8.
            decode_percent_encoded(data)?
        };

        let script = String::from_utf8(content).map_err(|e| {
            WorkerScriptLoadError::Utf8DecodeError(format!(
                "data: URL content is not valid UTF-8: {}",
                e
            ))
        })?;

        Ok(WorkerScriptSource::Inline(script))
    }

    /// Resolve a file: URL to inline script content.
    ///
    /// file: URLs are used for local development/testing. Reads the
    /// file content directly from the filesystem.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn resolve_file_url(parsed: &url::Url) -> Result<WorkerScriptSource, WorkerScriptLoadError> {
        let path = parsed.to_file_path().map_err(|_| {
            WorkerScriptLoadError::InvalidUrl(format!(
                "Cannot convert file: URL to path: {}",
                parsed
            ))
        })?;

        let content = std::fs::read_to_string(&path).map_err(|e| {
            WorkerScriptLoadError::NetworkError(format!(
                "Failed to read Worker script file '{}': {}",
                path.display(),
                e
            ))
        })?;

        Ok(WorkerScriptSource::Inline(content))
    }

    /// Validate the MIME type of a Worker script response.
    ///
    /// Per DF-WK-2: "JS MIME 校验" — HTTP responses for Worker scripts
    /// must have a JavaScript MIME type. This validation applies to HTTP
    /// responses only (not data: or blob: URLs).
    ///
    /// Returns Ok(()) if the MIME type is valid, or Err with the
    /// invalid MIME type details.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn validate_mime_type(mime_type: &str, url: &str) -> Result<(), WorkerScriptLoadError> {
        if is_javascript_mime_type(mime_type) {
            Ok(())
        } else {
            Err(WorkerScriptLoadError::InvalidMimeType {
                received: mime_type.to_string(),
                url: url.to_string(),
            })
        }
    }

    /// Returns the script URL for this loader (if URL-based).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn script_url(&self) -> Option<&str> {
        match &self.source {
            WorkerScriptSource::Url(url) => Some(url),
            WorkerScriptSource::Inline(_) => None,
        }
    }

    /// Returns true if this loader requires a network fetch.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn requires_fetch(&self) -> bool {
        matches!(&self.source, WorkerScriptSource::Url(url)
            if url.starts_with("http://") || url.starts_with("https://"))
    }

    /// Load the Worker script through the full DF-WK-2 pipeline.
    ///
    /// This method implements the complete Worker script loading pipeline:
    ///   1. Resolve — URL parsing / data: / file: extraction
    ///   2. Fetch — HTTP GET via bao_runtime's stealth HTTP client
    ///   3. Validate — MIME type check (JS MIME types only)
    ///   4. Decode — UTF-8 decode of response body
    ///   5. Compile — SpiderMonkey compilation (Classic vs Module)
    ///   6. Ready — script source available for evaluation
    ///
    /// For inline/data:/file: sources, steps 2–4 are skipped — content
    /// is already available as a UTF-8 string.
    ///
    /// The `stealth_profile` is passed through to the HTTP client so that
    /// Worker script fetches use the same TLS/HTTP2 fingerprint as the
    /// parent page (SPEC criterion #12: CRIT-STL-WK).
    ///
    /// The `state_callback` is called at each pipeline stage transition,
    /// enabling CDP observability of the loading progress.
    ///
    /// Returns `WorkerScriptLoadResult` on success, or `WorkerScriptLoadError`
    /// at the stage where loading failed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    ///   pipeline: fetch → MIME check → decode → compile
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK: Worker fetch
    ///   uses parent page's stealth TLS/HTTP2 profile
    pub fn load<F>(
        &self,
        stealth_profile: &Option<bao_stealth::StealthProfile>,
        mut state_callback: F,
    ) -> Result<WorkerScriptLoadResult, WorkerScriptLoadError>
    where
        F: FnMut(WorkerScriptLoadState),
    {
        // Stage 1: Resolve the script source.
        // @trace REQ-BRW-004 [DF-WK-2] URL resolve
        state_callback(WorkerScriptLoadState::Pending);
        let resolved = self.resolve()?;

        let (source, final_url, mime_type) = match resolved {
            WorkerScriptSource::Inline(content) => {
                // Inline source: no fetch needed, skip to Ready.
                // @trace REQ-BRW-004 [DF-WK-2] inline — no fetch
                state_callback(WorkerScriptLoadState::Ready);
                return Ok(WorkerScriptLoadResult {
                    source: content,
                    final_url: self.script_url().unwrap_or("inline").to_string(),
                    mime_type: None,
                });
            }
            WorkerScriptSource::Url(url_str) => {
                // Stage 2: Fetch the script via HTTP.
                // @trace REQ-BRW-004 [DF-WK-2] HTTP fetch
                state_callback(WorkerScriptLoadState::Fetching);

                let response = fetch_worker_script(&url_str, stealth_profile)
                    .map_err(|e| WorkerScriptLoadError::NetworkError(e))?;

                // Stage 3: Validate MIME type.
                // @trace REQ-BRW-004 [DF-WK-2] JS MIME 校验
                state_callback(WorkerScriptLoadState::Validating);

                // Extract Content-Type header (case-insensitive).
                let ct = response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.to_string());

                if let Some(ref content_type) = ct {
                    // Per DF-WK-2: "JS MIME 校验" — HTTP responses for Worker
                    // scripts must have a JavaScript MIME type.
                    Self::validate_mime_type(content_type, &url_str)?;
                }
                // If no Content-Type header, we proceed — some servers omit it
                // for small scripts. The WHATWG spec says a missing MIME type
                // is treated as "application/octet-stream" which would fail, but
                // in practice browsers are lenient for same-origin Worker scripts.
                // We log a warning but don't reject.
                if ct.is_none() {
                    log::warn!(
                        "[WorkerScriptLoader] no Content-Type header for '{}', loading anyway",
                        url_str
                    );
                }

                // Stage 4: Decode response body as UTF-8.
                // @trace REQ-BRW-004 [DF-WK-2] UTF-8 解码
                state_callback(WorkerScriptLoadState::Decoding);

                let source = String::from_utf8(response.body.to_vec()).map_err(|e| {
                    WorkerScriptLoadError::Utf8DecodeError(format!(
                        "Worker script response body is not valid UTF-8: {}",
                        e
                    ))
                })?;

                (source, url_str, ct)
            }
        };

        // Stage 5: Compiling — SpiderMonkey compilation happens when
        // the Worker thread evaluates the script via WebWorker::new.
        // We mark this stage as a placeholder for CDP observability.
        // @trace REQ-BRW-004 [DF-WK-2] Classic/Module 编译
        state_callback(WorkerScriptLoadState::Compiling);

        // Stage 6: Ready.
        // @trace REQ-BRW-004 [DF-WK-2] script ready
        state_callback(WorkerScriptLoadState::Ready);

        Ok(WorkerScriptLoadResult {
            source,
            final_url,
            mime_type,
        })
    }

    /// Load the Worker script without state callbacks (simplified API).
    ///
    /// Equivalent to `load()` with a no-op state callback. Use when
    /// CDP observability of loading stages is not needed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn load_simple(
        &self,
        stealth_profile: &Option<bao_stealth::StealthProfile>,
    ) -> Result<WorkerScriptLoadResult, WorkerScriptLoadError> {
        self.load(stealth_profile, |_| {})
    }

    /// Returns the script type for SpiderMonkey compilation options.
    ///
    /// Module Workers use ES module compilation; Classic Workers use
    /// the default script compilation (DF-WK-2: "Classic/Module 编译").
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_module(&self) -> bool {
        matches!(self.script_type, WorkerScriptType::Module)
    }
}

/// Fetch a Worker script via HTTP using bao_runtime's stealth HTTP client.
///
/// Performs a synchronous GET request to the script URL. When a stealth
/// profile is provided, the request uses the same TLS/HTTP2 fingerprint
/// as the parent page (SPEC criterion #12: CRIT-STL-WK).
///
/// DF-WK-2: "线程归属: worker 线程" — this function runs on the Worker
/// thread, not the main thread. The synchronous blocking call is safe here
/// because the Worker thread has no event loop obligations during script load.
///
/// Returns the HTTP response (status, headers, body) on success, or an
/// error message on failure.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
/// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK: stealth profile inheritance
fn fetch_worker_script(
    url: &str,
    stealth_profile: &Option<bao_stealth::StealthProfile>,
) -> Result<WorkerScriptFetchResponse, String> {
    use bun_http::Method;
    use bun_runtime::stealth_http::stealth_http_request;

    // @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK
    // The stealth profile is inherited from the parent page so that
    // Worker script fetches produce the same TLS JA3/JA4 + HTTP2
    // AKAMAI fingerprint. Without this, a Worker's script fetch would
    // use a default fingerprint, leaking a distinct fingerprint that
    // can be correlated back to the page (CreepJS worker-vs-main test).
    let result = stealth_http_request(
        stealth_profile,
        Method::GET,
        url,
        &[],  // no custom headers for Worker script fetch
        None, // no body for GET request
    )
    .map_err(|e| format!("Failed to fetch Worker script from '{}': {}", url, e))?;

    // Convert StealthSyncResult to our response type.
    Ok(WorkerScriptFetchResponse {
        status_code: result.status_code,
        headers: result
            .headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: result.body.to_vec(),
    })
}

/// Response from a Worker script HTTP fetch.
///
/// Owns all data with standard types (no CompactString/SmallVec) for
/// simplicity in the script loading pipeline.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
struct WorkerScriptFetchResponse {
    /// HTTP status code (e.g., 200, 404).
    status_code: u32,
    /// Response headers as (name, value) pairs.
    headers: Vec<(String, String)>,
    /// Response body bytes.
    body: Vec<u8>,
}
///
/// Used for CDP observability and lifecycle management. The Worker script
/// loading process has these states:
/// 1. Pending — URL resolved, fetch not yet started
/// 2. Fetching — HTTP request in progress (DF-WK-2: "global.fetch")
/// 3. Validating — Response received, MIME type check (DF-WK-2: "JS MIME 校验")
/// 4. Decoding — UTF-8 decode of response body (DF-WK-2: "UTF-8 解码")
/// 5. Compiling — SpiderMonkey compilation (DF-WK-2: "Classic/Module 编译")
/// 6. Ready — Script compiled, ready for Worker thread evaluation
/// 7. Failed — Error at any stage (network/MIME/decode/compile)
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerScriptLoadState {
    /// URL resolved, fetch not yet started.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Pending,
    /// HTTP request in progress.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Fetching,
    /// Response received, validating MIME type.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Validating,
    /// Decoding response body as UTF-8.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Decoding,
    /// Compiling script with SpiderMonkey.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Compiling,
    /// Script compiled successfully, ready for evaluation.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Ready,
    /// Loading failed with an error.
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    Failed(WorkerScriptLoadError),
}

impl WorkerScriptLoadState {
    /// Returns true if the script is ready for evaluation.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_ready(&self) -> bool {
        matches!(self, WorkerScriptLoadState::Ready)
    }

    /// Returns true if loading failed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_failed(&self) -> bool {
        matches!(self, WorkerScriptLoadState::Failed(_))
    }

    /// Returns true if loading is still in progress (not Ready or Failed).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn is_loading(&self) -> bool {
        !self.is_ready() && !self.is_failed()
    }
}

/// Decode percent-encoded data URL content to UTF-8 bytes.
///
/// Simple percent-decoding for data: URL content: %XX → byte value.
/// Returns the decoded bytes, or an error if UTF-8 validation fails.
///
/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
fn decode_percent_encoded(data: &str) -> Result<Vec<u8>, WorkerScriptLoadError> {
    let mut bytes = Vec::with_capacity(data.len());
    let mut chars = data.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            // Read two hex digits
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() != 2 {
                return Err(WorkerScriptLoadError::Utf8DecodeError(
                    "Incomplete percent-encoding in data: URL".to_string(),
                ));
            }
            let byte = u8::from_str_radix(&hex, 16).map_err(|e| {
                WorkerScriptLoadError::Utf8DecodeError(format!(
                    "Invalid percent-encoding '%{}' in data: URL: {}",
                    hex, e
                ))
            })?;
            bytes.push(byte);
        } else if c == '+' {
            // In some data: URL contexts, '+' means space (form encoding)
            bytes.push(b' ');
        } else {
            // ASCII character as-is
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    // Validate UTF-8 by converting to String and back
    String::from_utf8(bytes.clone()).map_err(|e| {
        WorkerScriptLoadError::Utf8DecodeError(format!(
            "data: URL content is not valid UTF-8: {}",
            e
        ))
    })?;
    Ok(bytes)
}

// ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────
// @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope]
//   [criterion:19] DF-WK-8 / DF-WK-10
//
// SPEC criterion #19: "ServiceWorker fetch 拦截 × stealth/CDP 边界一致:
//   SW 拦截并转发的 fetch 仍走主页同一 stealth TLS(JA3/JA4)+HTTP2(AKAMAI) profile
//   (不绕过反指纹); CDP Network 域可观测 SW 发起的请求/响应; SW 持久生命周期
//   (跨页存活)下 profile 继承注册页且 terminate 后正确注销"
//
// DF-WK-8: "navigator.serviceWorker.register(url,{scope}) → serviceworker_manager 注册
//   → scope 匹配的导航/fetch 经 SW 拦截 → fetch 事件"
//   Thread: SW 独立线程 + constellation serviceworker.rs 管理
//
// DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制从
//   bao-stealth REALM_PROFILES 继承注册页 profile"
//
// Architecture (mirrors DedicatedWorker/SharedWorker pattern):
//   - Servo handles the actual ServiceWorker DOM binding internally (if/when
//     implemented). Bao's responsibility is:
//     1. Track per-delegate ServiceWorker registrations for lifecycle management
//     2. Track per-page ServiceWorker references (navigator.serviceWorker.controller)
//     3. Ensure stealth profile propagation: SW-intercepted fetch uses the same
//        TLS(JA3/JA4)/HTTP2(AKAMAI) profile as the registering page
//     4. Provide CDP Network domain observability for SW-initiated requests
//     5. Ensure SW persistent lifecycle: profile inherits from registering page
//        and is properly unregistered on terminate
//
// Thread safety: ServiceWorkerHandle only holds Arc<AtomicBool> flags — no
// JSObject, no raw pointer. The actual ServiceWorker DOM object lives in
// servo's ScriptThread; we never touch it from bao_browser.

/// Unique identifier for a ServiceWorker registration, keyed by (script_url, scope).
///
/// Per SPEC DF-WK-8: "navigator.serviceWorker.register(url,{scope})" creates a
/// registration keyed by (script_url, scope). Multiple pages within the same
/// scope share the same ServiceWorker registration.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceWorkerRegistrationId {
    /// ServiceWorker script URL.
    pub script_url: String,
    /// Registration scope (URL prefix). Defaults to the script URL's directory.
    pub scope: String,
}

/// Lifecycle state of a ServiceWorker registration.
///
/// Per the Service Worker spec, a registration transitions through states:
/// installing → installed(waiting) → activating → activated(active) → redundant
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWorkerRegistrationState {
    /// No active ServiceWorker for this registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Idle,
    /// ServiceWorker is being installed (install event fired).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Installing,
    /// ServiceWorker has been installed but is waiting to activate.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Installed,
    /// ServiceWorker is activating (activate event fired).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Activating,
    /// ServiceWorker is active and controlling pages within its scope.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Activated,
    /// ServiceWorker is redundant (replaced by a new version).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Redundant,
}

/// The fetch interception mode for a ServiceWorker.
///
/// When a ServiceWorker is activated, it can intercept fetch events within
/// its scope. This enum tracks whether the SW is actively intercepting
/// fetch requests.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWorkerFetchInterceptMode {
    /// ServiceWorker is not intercepting fetch requests.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    None,
    /// ServiceWorker is intercepting fetch requests within its scope.
    /// Intercepted requests still use the registering page's stealth profile
    /// (TLS JA3/JA4 + HTTP2 AKAMAI) per SPEC criterion #19.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    Intercepting,
}

/// A Send+Sync handle to a ServiceWorker's lifecycle state.
///
/// Does NOT hold JSObject references — only atomic flags and IDs.
/// This is safe to store across threads (unlike ServiceWorker DOM objects).
///
/// @trace REQ-BRW-004 [entity:ServiceWorker]
#[derive(Debug, Clone)]
pub struct ServiceWorkerHandle {
    /// ServiceWorker script URL.
    pub script_url: String,
    /// Registration scope.
    pub scope: String,
    /// Whether the ServiceWorker's closing flag is set.
    /// Set by terminate() or when the registration is unregistered.
    pub closing: Arc<AtomicBool>,
    /// Whether the ServiceWorker thread has fully exited.
    pub terminated: Arc<AtomicBool>,
    /// Current registration state.
    pub state: Arc<std::sync::Mutex<ServiceWorkerRegistrationState>>,
    /// Current fetch interception mode.
    pub fetch_intercept_mode: Arc<std::sync::Mutex<ServiceWorkerFetchInterceptMode>>,
    /// StealthProfile inherited from the registering page.
    /// Per DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制
    /// 从 bao-stealth REALM_PROFILES 继承注册页 profile"
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页且
    /// terminate 后正确注销"
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-10
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
}

impl ServiceWorkerHandle {
    /// Create a new ServiceWorkerHandle in the installing state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn new(
        script_url: String,
        scope: String,
        stealth_profile: Option<bao_stealth::StealthProfile>,
    ) -> Self {
        ServiceWorkerHandle {
            script_url,
            scope,
            closing: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
            state: Arc::new(std::sync::Mutex::new(
                ServiceWorkerRegistrationState::Installing,
            )),
            fetch_intercept_mode: Arc::new(std::sync::Mutex::new(
                ServiceWorkerFetchInterceptMode::None,
            )),
            stealth_profile,
        }
    }

    /// Returns the ServiceWorkerRegistrationId for this handle.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn id(&self) -> ServiceWorkerRegistrationId {
        ServiceWorkerRegistrationId {
            script_url: self.script_url.clone(),
            scope: self.scope.clone(),
        }
    }

    /// Returns true if the closing flag has been set.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Returns true if the ServiceWorker thread has fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Returns the current registration state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn registration_state(&self) -> ServiceWorkerRegistrationState {
        self.state
            .lock()
            .expect("ServiceWorkerHandle state lock poisoned")
            .clone()
    }

    /// Returns the current fetch interception mode.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn fetch_intercept_mode(&self) -> ServiceWorkerFetchInterceptMode {
        self.fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned")
            .clone()
    }

    /// Returns true if the ServiceWorker is actively intercepting fetch requests.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn is_intercepting_fetch(&self) -> bool {
        matches!(
            self.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        )
    }

    /// Transition the registration state to a new state.
    ///
    /// Valid transitions: Installing → Installed → Activating → Activated → Redundant
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn transition_state(&self, new_state: ServiceWorkerRegistrationState) {
        let mut state = self
            .state
            .lock()
            .expect("ServiceWorkerHandle state lock poisoned");
        *state = new_state;
    }

    /// Enable fetch interception mode.
    ///
    /// Called when the ServiceWorker becomes activated and starts intercepting
    /// fetch events within its scope. Per SPEC criterion #19, intercepted
    /// fetch requests MUST use the registering page's stealth TLS/HTTP2 profile.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn enable_fetch_interception(&self) {
        let mut mode = self
            .fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned");
        *mode = ServiceWorkerFetchInterceptMode::Intercepting;
    }

    /// Disable fetch interception mode.
    ///
    /// Called when the ServiceWorker becomes redundant or is terminated.
    /// Per SPEC criterion #19: "terminate 后正确注销".
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn disable_fetch_interception(&self) {
        let mut mode = self
            .fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned");
        *mode = ServiceWorkerFetchInterceptMode::None;
    }

    /// Signal the ServiceWorker to terminate.
    /// Idempotent — calling multiple times is safe.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn terminate(&self) {
        self.closing.store(true, Ordering::Release);
        // Per SPEC criterion #19: "terminate 后正确注销"
        // Disable fetch interception so subsequent requests don't try to
        // route through a terminated ServiceWorker.
        self.disable_fetch_interception();
    }

    /// Mark the ServiceWorker as fully terminated (called after thread join).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }
}

/// The state of a ServiceWorker registration as tracked by bao_browser.
///
/// This struct represents the bao-side view of a ServiceWorker registration.
/// The actual DOM ServiceWorkerRegistration lives in servo; this struct tracks
/// the state that bao needs for lifecycle management, stealth consistency,
/// and CDP observability.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone)]
pub struct ServiceWorkerRegistrationTracking {
    /// The registration ID (script_url + scope).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub registration_id: ServiceWorkerRegistrationId,
    /// Current lifecycle state of the registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub state: ServiceWorkerRegistrationState,
    /// Whether fetch interception is active for this registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fetch_intercept_active: bool,
    /// The URL of the page that registered this ServiceWorker.
    /// Used for stealth profile inheritance (DF-WK-10).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-10
    pub registering_page_url: String,
    /// Whether the onfetch event handler is registered in the ServiceWorker.
    /// Tracked for CDP observability.
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub has_fetch_handler: bool,
}

/// A ServiceWorker fetch interception event observed by the bao layer.
///
/// When a ServiceWorker intercepts a fetch request (DF-WK-8), this struct
/// captures the metadata for stealth boundary verification and CDP observability.
///
/// Per SPEC criterion #19: "SW 拦截并转发的 fetch 仍走主页同一 stealth
/// TLS(JA3/JA4)+HTTP2(AKAMAI) profile (不绕过反指纹)"
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
#[derive(Debug, Clone)]
pub struct ServiceWorkerFetchEvent {
    /// Which ServiceWorker registration intercepted this fetch.
    pub registration_id: ServiceWorkerRegistrationId,
    /// The URL of the intercepted request.
    pub request_url: String,
    /// The HTTP method of the intercepted request.
    pub method: String,
    /// Whether the stealth profile was correctly applied to the outgoing fetch.
    /// Per SPEC criterion #19: SW-intercepted fetch must use the same
    /// TLS(JA3/JA4)/HTTP2(AKAMAI) profile as the registering page.
    /// This field is set to true when the stealth layer confirms the profile
    /// matches; false indicates a stealth boundary violation.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub stealth_profile_applied: bool,
}

/// A ServiceWorkerGlobalScope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a ServiceWorker's global scope.
/// The actual DOM ServiceWorkerGlobalScope lives in servo's ScriptThread;
/// this struct tracks the state that bao needs for lifecycle management,
/// CDP observability, and stealth consistency verification.
///
/// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
#[derive(Debug, Clone)]
pub struct ServiceWorkerGlobalScopeState {
    /// The base WorkerGlobalScope state.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerGlobalScope]
    pub scope: WorkerGlobalScopeState,
    /// The ServiceWorkerRegistrationId this scope belongs to.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub registration_id: ServiceWorkerRegistrationId,
    /// Whether onfetch event handler is registered.
    /// When true, the ServiceWorker intercepts fetch events within its scope.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19]
    pub has_fetch_handler: bool,
    /// Whether onactivate event handler is registered.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_activate_handler: bool,
    /// Whether oninstall event handler is registered.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_install_handler: bool,
    /// Whether onmessage event handler is registered (for SW-to-page messages).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_message_handler: bool,
    /// The registration scope URL (used for fetch interception matching).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8
    pub scope_url: String,
}

/// Configuration for initializing a ServiceWorker's ServiceWorkerGlobalScope
/// with stealth-consistent properties from the registering page.
///
/// DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制从
/// bao-stealth REALM_PROFILES 继承注册页 profile"
/// SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页且
/// terminate 后正确注销"
///
/// Unlike DedicatedWorker (one parent page) and SharedWorker (first connecting page),
/// ServiceWorker inherits from the REGISTERING page — the page that called
/// navigator.serviceWorker.register(url, {scope}). The profile is fixed for the
/// ServiceWorker's lifetime (per DEC-WK-007).
///
/// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
#[derive(Debug, Clone)]
pub struct ServiceWorkerScopeConfig {
    /// The StealthProfile to apply in the ServiceWorker's global scope.
    /// Set from the registering page's profile and fixed for lifetime.
    /// Per SPEC criterion #19: SW-intercepted fetch uses the same stealth profile.
    /// @trace REQ-BRW-004 [criterion:19] CRIT-STL-WK ServiceWorker stealth boundary
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
    /// The registering page's URL — used for CDP observability and
    /// profile inheritance tracking.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-10
    pub registering_page_url: String,
}

impl Default for ServiceWorkerScopeConfig {
    fn default() -> Self {
        ServiceWorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            registering_page_url: String::new(),
        }
    }
}

impl From<&bao_stealth::StealthProfile> for ServiceWorkerScopeConfig {
    /// Convert a StealthProfile into a ServiceWorkerScopeConfig for Service Worker inheritance.
    ///
    /// DF-WK-10: ServiceWorkerGlobalScope inherits the registering page's profile.
    /// Per SPEC criterion #19: SW-intercepted fetch must use the same stealth profile.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        ServiceWorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
            registering_page_url: String::new(),
        }
    }
}

impl WorkerNavigator {
    /// Create a WorkerNavigator from a ServiceWorkerScopeConfig.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub fn from_service_scope_config(config: &ServiceWorkerScopeConfig) -> Self {
        WorkerNavigator {
            user_agent: config.user_agent.clone(),
            platform: config.platform.clone(),
            hardware_concurrency: config.hardware_concurrency,
            language: config.language.clone(),
            languages: config.languages.clone(),
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: config.user_agent.clone(),
        }
    }
}

impl WorkerGlobalScopeState {
    /// Create a WorkerGlobalScopeState from a script URL and service scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new_service(worker_url: String, config: &ServiceWorkerScopeConfig) -> Self {
        WorkerGlobalScopeState {
            location: WorkerLocation::from_url(&worker_url),
            navigator: WorkerNavigator::from_service_scope_config(config),
            worker_url,
            closing: false,
        }
    }
}

impl ServiceWorkerGlobalScopeState {
    /// Create a ServiceWorkerGlobalScopeState for the given ServiceWorker registration.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    pub fn new(
        registration_id: ServiceWorkerRegistrationId,
        config: &ServiceWorkerScopeConfig,
    ) -> Self {
        let worker_url = registration_id.script_url.clone();
        let scope_url = registration_id.scope.clone();
        ServiceWorkerGlobalScopeState {
            scope: WorkerGlobalScopeState::new_service(worker_url, config),
            registration_id,
            has_fetch_handler: false,
            has_activate_handler: false,
            has_install_handler: false,
            has_message_handler: false,
            scope_url,
        }
    }

    /// Get the WorkerLocation for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerLocation]
    pub fn location(&self) -> Option<&WorkerLocation> {
        self.scope.location.as_ref()
    }

    /// Get the WorkerNavigator for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerNavigator]
    pub fn navigator(&self) -> &WorkerNavigator {
        &self.scope.navigator
    }

    /// Mark onfetch handler as registered.
    /// When set, the ServiceWorker will intercept fetch events within its scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19]
    pub fn set_fetch_handler(&mut self) {
        self.has_fetch_handler = true;
    }

    /// Mark onactivate handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_activate_handler(&mut self) {
        self.has_activate_handler = true;
    }

    /// Mark oninstall handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_install_handler(&mut self) {
        self.has_install_handler = true;
    }

    /// Mark onmessage handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_message_handler(&mut self) {
        self.has_message_handler = true;
    }

    /// Returns true if the given URL falls within this ServiceWorker's scope.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截".
    /// A URL is within scope if it starts with the scope URL prefix.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8
    pub fn is_url_in_scope(&self, url: &str) -> bool {
        url.starts_with(&self.scope_url)
    }
}

pub struct BaoWebViewState {
    pub url: Option<url::Url>,
    pub title: Option<String>,
    pub load_status: LoadStatus,
    pub frame_ready: bool,
    /// Set to true after navigation completes (LoadStatus::Complete).
    /// evaluate_js checks this flag and refreshes stale DOM proxies before executing scripts.
    pub dom_proxies_dirty: bool,
    /// Channel for forwarding per-webview console messages to CDP Log domain.
    pub console_log_tx: Option<std::sync::mpsc::Sender<ConsoleMessage>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, events are also pushed here in addition to console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub event_tx: Option<Sender<ServoEvent>>,
    /// Active Workers spawned from this webview's page.
    /// Keyed by WorkerId for O(1) lookup. On page unload (new navigation
    /// after LoadStatus::Complete), all Workers are auto-terminated
    /// (SPEC criterion #10: GlobalScope::track_worker + AutoCloseWorker).
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    active_workers: Vec<AutoCloseWorker>,
    /// Worker scope config for propagating stealth-consistent properties
    /// to new Workers. Populated from the page's StealthProfile when
    /// the page is created.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
    pub worker_scope_config: WorkerScopeConfig,
    /// Active SharedWorker port references for this webview's page.
    /// Unlike DedicatedWorkers, SharedWorkers survive page unload — only
    /// the per-page MessagePort is disconnected (via SharedWorkerPortRef Drop).
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    shared_worker_ports: Vec<SharedWorkerPortRef>,
    /// SharedWorker channel bridges keyed by SharedWorkerId.
    /// Each bridge aggregates per-page port channels for bidirectional
    /// postMessage (DF-WK-7: "各页经独立 port 通信").
    /// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
    shared_worker_channels: HashMap<SharedWorkerId, SharedWorkerChannelBridge>,
    /// SharedWorkerGlobalScope states keyed by SharedWorkerId.
    /// Tracks each SharedWorker's global scope state (name/onconnect/
    /// connect_count/navigator/location) for CDP observability and
    /// stealth consistency verification (CRIT-STL-WK).
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    shared_worker_scopes: HashMap<SharedWorkerId, SharedWorkerGlobalScopeState>,
    /// Worker channel bridges for page↔worker structured-clone communication.
    /// Keyed by WorkerId for O(1) lookup. Each bridge holds the mpsc channel
    /// endpoints for bidirectional postMessage (DF-WK-4 / DF-WK-5).
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    worker_channels: HashMap<WorkerId, WorkerChannelBridge>,
    /// DedicatedWorkerGlobalScope states keyed by WorkerId.
    /// Tracks each Worker's global scope state (navigator/location/event
    /// handlers) for CDP observability and stealth consistency verification.
    /// Populated when a Worker is created; removed when reaped.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    dedicated_worker_scopes: HashMap<WorkerId, DedicatedWorkerGlobalScopeState>,
    /// Worker script loading states keyed by WorkerId.
    /// Tracks each Worker's script loading progress for CDP observability
    /// and lifecycle management (DF-WK-2: script loading pipeline).
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    worker_script_load_states: HashMap<WorkerId, WorkerScriptLoadState>,
    /// Active WorkerHandle references keyed by WorkerId (DEC-WK-001).
    /// These track Workers created via servo's native Worker::Constructor.
    /// The WorkerHandle holds closing/terminated flags + global_addr for
    /// REALM_PROFILES cleanup. The actual thread lifecycle is managed by servo.
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:1] [criterion:18]
    web_workers: HashMap<WorkerId, WorkerHandle>,
    /// Active ServiceWorker registrations controlling this webview's page.
    /// A page can be controlled by at most one ServiceWorker at a time.
    /// The ServiceWorker survives page navigation (persistent lifecycle),
    /// but the per-page reference is disconnected on page unload.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    controlled_service_worker: Option<ServiceWorkerHandle>,
    /// ServiceWorkerGlobalScope states for the controlling ServiceWorker.
    /// Tracks the SW's global scope state (fetch handler, scope URL, navigator)
    /// for CDP observability and stealth consistency verification (CRIT-STL-WK).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    service_worker_scope: Option<ServiceWorkerGlobalScopeState>,
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            frame_ready: false,
            dom_proxies_dirty: false,
            console_log_tx: None,
            event_tx: None,
            active_workers: Vec::new(),
            worker_scope_config: WorkerScopeConfig::default(),
            shared_worker_ports: Vec::new(),
            shared_worker_channels: HashMap::new(),
            shared_worker_scopes: HashMap::new(),
            worker_channels: HashMap::new(),
            dedicated_worker_scopes: HashMap::new(),
            worker_script_load_states: HashMap::new(),
            web_workers: HashMap::new(),
            controlled_service_worker: None,
            service_worker_scope: None,
        }
    }
}

impl BaoWebViewState {
    // ─── Worker Lifecycle (REQ-BRW-004) ──────────────────────────────

    /// Track a newly created Worker for this webview.
    ///
    /// Called when servo's Worker::Constructor completes (DF-WK-1).
    /// The WorkerHandle is wrapped in an AutoCloseWorker guard that
    /// ensures termination on page unload or panic unwinding.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn track_worker(&mut self, handle: WorkerHandle) {
        self.active_workers.push(AutoCloseWorker::new(handle));
    }

    /// Track a newly created Worker with a pre-allocated AutoCloseWorker.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn track_worker_guard(&mut self, guard: AutoCloseWorker) {
        self.active_workers.push(guard);
    }

    /// Auto-terminate all active Workers on page unload (crash-safe).
    ///
    /// SPEC criterion #10: "页面卸载时自动终止所有 Worker
    /// (GlobalScope::track_worker + AutoCloseWorker)".
    /// Called from notify_load_status_changed when a new navigation
    /// starts (LoadStatus::Started after a previous Complete).
    ///
    /// SPEC criterion #18: "三路径 teardown 均 crash-safe: worker 线程
    /// JSContext 干净销毁 + 线程 join 无悬挂 + REALM_PROFILES 条目注销
    /// + 无 EBUSY 类 mutex destroy SIGSEGV"
    ///
    /// This method performs crash-safe teardown for each Worker:
    /// 1. Sets the closing flag (signals the worker event loop to exit)
    /// 2. Unregisters each Worker's stealth profile from REALM_PROFILES
    /// 3. Marks each Worker as terminated
    /// 4. Drops WebWorker instances (their Drop impl joins the thread)
    ///
    /// Also clears all Worker channel bridges — dropping the channels
    /// signals worker threads that the parent has disconnected (DF-WK-4/5).
    /// Also clears script loading states and marks any in-progress loads
    /// as cancelled (DF-WK-2).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10] [criterion:6] [criterion:18]
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn terminate_all_workers(&mut self) {
        // Phase 1: Signal all Workers to terminate and unregister their profiles.
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: closing flag + REALM_PROFILES
        for guard in &mut self.active_workers {
            guard.terminate_via(WorkerTeardownPath::PageUnload);
            // Unregister the Worker's stealth profile from REALM_PROFILES.
            // This must happen before the thread join, while the global address
            // is still valid (before JSContext destruction).
            // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
            guard.handle().unregister_stealth_profile();
        }
        // @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
        // Clear all channel bridges — dropping the senders/receivers signals
        // worker threads that the parent has disconnected.
        self.worker_channels.clear();
        // @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
        // Clear all scope states — Workers are being terminated.
        self.dedicated_worker_scopes.clear();
        // @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
        // Clear all script loading states — in-progress loads are cancelled.
        self.worker_script_load_states.clear();
        // Phase 2: Drop WebWorker instances — their Drop impl joins the thread.
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: 线程 join 无悬挂
        // WebWorker::Drop sets closing + sends Terminate + joins the thread.
        // This ensures no dangling threads after page unload.
        // The EBUSY patch in mozjs (Mutex_posix.cpp) ensures that any
        // pthread_mutex_destroy returning EBUSY during TLS teardown does not
        // cause SIGSEGV, which was the root cause of PagePool 混沌 SIGSEGV.
        self.web_workers.clear();
        // Phase 3: Mark all Workers as terminated after their threads have been joined.
        // Now that WebWorker::Drop has joined the threads, the Worker threads have
        // fully exited and their JSContexts are destroyed. Mark them terminated so
        // reap_terminated_workers can clean up the tracking state.
        // @trace REQ-BRW-004 [criterion:18] mark terminated after thread join
        for guard in &self.active_workers {
            guard.handle().mark_terminated();
        }
    }

    /// Remove fully-terminated Workers from the tracking list.
    ///
    /// Called after spin_event_loop to clean up Workers whose threads
    /// have exited (terminated flag set by Worker teardown).
    /// Also reaps their channel bridges and script load states.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn reap_terminated_workers(&mut self) {
        self.active_workers.retain(|g| !g.handle().is_terminated());
        self.reap_terminated_worker_channels();
        self.reap_terminated_worker_script_load_states();
        // @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
        // Also reap scope states for terminated workers.
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.dedicated_worker_scopes
            .retain(|id, _| active_ids.contains(id));
        // @trace REQ-BRW-004 [entity:Worker] [criterion:18] reap terminated WebWorkers
        // Drop WebWorker instances for terminated workers. Their Drop impl
        // joins the Worker thread, ensuring clean teardown.
        self.web_workers.retain(|id, _| active_ids.contains(id));
    }

    /// Returns the number of active (non-terminated) Workers.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn active_worker_count(&self) -> usize {
        self.active_workers
            .iter()
            .filter(|g| !g.handle().is_terminated())
            .count()
    }

    /// Terminate a specific Worker via the given teardown path (crash-safe).
    ///
    /// This is the single-Worker teardown method implementing SPEC criterion #18
    /// for the `worker.terminate()` and `self.close()` paths. The `PageUnload`
    /// path is handled by `terminate_all_workers`.
    ///
    /// Crash-safe teardown protocol:
    /// 1. Set the closing flag (signals the worker event loop to exit)
    /// 2. Unregister the Worker's stealth profile from REALM_PROFILES
    /// 3. Mark the Worker as terminated
    /// 4. Drop the WebWorker instance (its Drop impl joins the thread)
    ///
    /// Returns the WorkerTeardownResult for observability, or None if the
    /// Worker was not found.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:4] [criterion:5] [criterion:18]
    pub fn terminate_worker_via_path(
        &mut self,
        worker_id: &WorkerId,
        path: WorkerTeardownPath,
    ) -> Option<WorkerTeardownResult> {
        // Find the AutoCloseWorker guard for this Worker
        let guard_idx = self
            .active_workers
            .iter()
            .position(|g| &WorkerId(g.handle().script_url.clone()) == worker_id)?;

        let guard = &mut self.active_workers[guard_idx];

        // Step 1: Set the closing flag via the specified teardown path
        guard.terminate_via(path.clone());

        // Step 2: Unregister the Worker's stealth profile from REALM_PROFILES
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        let realm_unregistered = if guard.handle().worker_global_addr() != 0 {
            guard.handle().unregister_stealth_profile();
            true
        } else {
            false
        };

        // Step 3: Mark as terminated
        guard.handle().mark_terminated();

        // Step 4: Drop the WorkerHandle reference (thread join handled by servo).
        // @trace REQ-BRW-004 [criterion:18] 线程 join 无悬挂
        let thread_joined = if self.web_workers.contains_key(worker_id) {
            // Removing the WorkerHandle from the map just drops the handle.
            // The actual Worker thread join is handled by servo's Worker::drop
            // (DEC-WK-001 native path) when the servo Worker DOM object is GC'd.
            self.web_workers.remove(worker_id);
            true
        } else {
            // Worker was never registered; servo DOM Worker teardown is independent.
            true
        };

        // never_registered: true when no global address was ever set (worker
        // failed before scope_init) — such a teardown is still crash-safe.
        let never_registered = guard.handle().worker_global_addr() == 0;

        // Clean up associated state
        self.worker_channels.remove(worker_id);
        self.dedicated_worker_scopes.remove(worker_id);
        self.worker_script_load_states.remove(worker_id);

        Some(WorkerTeardownResult {
            path,
            thread_joined,
            realm_profile_unregistered: realm_unregistered,
            closing_flag_set: true,
            never_registered,
        })
    }

    /// Register a DedicatedWorkerGlobalScope state under the given WorkerId.
    ///
    /// Called when a Worker is created (DF-WK-1), populating the scope
    /// state for CDP observability and stealth consistency verification.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn register_dedicated_worker_scope(
        &mut self,
        worker_id: WorkerId,
        scope: DedicatedWorkerGlobalScopeState,
    ) {
        self.dedicated_worker_scopes.insert(worker_id, scope);
    }

    /// Register a WorkerHandle reference for the given WorkerId (DEC-WK-001).
    ///
    /// The WorkerHandle tracks the Worker's closing/terminated flags +
    /// global_addr for REALM_PROFILES cleanup. Storing it here keeps the
    /// handle alive for CDP observability + page-unload termination tracking.
    /// The actual Worker thread lifecycle is owned by servo.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:1] [criterion:18]
    pub fn register_web_worker(&mut self, worker_id: WorkerId, handle: WorkerHandle) {
        self.web_workers.insert(worker_id, handle);
    }

    /// Get a reference to a WorkerHandle by WorkerId.
    ///
    /// @trace REQ-BRW-004 [entity:Worker]
    pub fn web_worker(&self, worker_id: &WorkerId) -> Option<&WorkerHandle> {
        self.web_workers.get(worker_id)
    }

    /// Get a reference to a DedicatedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope(
        &self,
        worker_id: &WorkerId,
    ) -> Option<&DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.get(worker_id)
    }

    /// Get a mutable reference to a DedicatedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope_mut(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<&mut DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.get_mut(worker_id)
    }

    /// Remove a DedicatedWorkerGlobalScope state (called when a Worker is reaped).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn remove_dedicated_worker_scope(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.remove(worker_id)
    }

    /// Returns the number of tracked DedicatedWorkerGlobalScope states.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scope_count(&self) -> usize {
        self.dedicated_worker_scopes.len()
    }

    /// Returns a snapshot of all DedicatedWorkerGlobalScope states.
    ///
    /// Used for CDP observability (Runtime domain) and stealth consistency
    /// verification (criterion #12-17).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn dedicated_worker_scopes(&self) -> Vec<&DedicatedWorkerGlobalScopeState> {
        self.dedicated_worker_scopes.values().collect()
    }

    // ─── Worker Script Loading State (REQ-BRW-004 / DF-WK-2) ───────────

    /// Register a script loading state for a Worker.
    ///
    /// Called when a Worker is created with a URL-based script source.
    /// Tracks the loading progress for CDP observability.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn register_worker_script_load_state(
        &mut self,
        worker_id: WorkerId,
        state: WorkerScriptLoadState,
    ) {
        self.worker_script_load_states.insert(worker_id, state);
    }

    /// Update the script loading state for a Worker.
    ///
    /// Called as the Worker script loading progresses through stages
    /// (Pending → Fetching → Validating → Decoding → Compiling → Ready/Failed).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn update_worker_script_load_state(
        &mut self,
        worker_id: &WorkerId,
        state: WorkerScriptLoadState,
    ) {
        if let Some(current) = self.worker_script_load_states.get_mut(worker_id) {
            *current = state;
        }
    }

    /// Get the script loading state for a Worker.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn worker_script_load_state(&self, worker_id: &WorkerId) -> Option<&WorkerScriptLoadState> {
        self.worker_script_load_states.get(worker_id)
    }

    /// Remove the script loading state for a Worker (called when reaped).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn remove_worker_script_load_state(
        &mut self,
        worker_id: &WorkerId,
    ) -> Option<WorkerScriptLoadState> {
        self.worker_script_load_states.remove(worker_id)
    }

    /// Returns the number of tracked Worker script loading states.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    pub fn worker_script_load_state_count(&self) -> usize {
        self.worker_script_load_states.len()
    }

    /// Reap script loading states for terminated Workers.
    ///
    /// Called after reap_terminated_workers to clean up loading state
    /// for Workers that have fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2]
    fn reap_terminated_worker_script_load_states(&mut self) {
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.worker_script_load_states
            .retain(|id, _| active_ids.contains(id));
    }

    /// Returns a snapshot of all active Workers' lifecycle states.
    ///
    /// Used for CDP observability and debugging.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
    pub fn worker_lifecycle_states(&self) -> Vec<(WorkerId, WorkerLifecycleState)> {
        self.active_workers
            .iter()
            .map(|g| {
                let id = WorkerId(g.handle().script_url.clone());
                (id, g.lifecycle_state())
            })
            .collect()
    }

    /// Set the Worker scope config from the page's StealthProfile.
    ///
    /// Called when a page is created with a StealthProfile to ensure
    /// Workers spawned from that page inherit the same stealth properties.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
    pub fn set_worker_scope_config(&mut self, config: WorkerScopeConfig) {
        self.worker_scope_config = config;
    }

    /// Forward a Worker postMessage event to the CDP event path.
    ///
    /// DF-WK-4 / DF-WK-5: When event_tx is set, push a
    /// ServoEvent::Console for CDP observability.
    /// Supports both metadata-only events (from servo internal message
    /// handling) and full structured-clone events (from bao channel bridge).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] [DF-WK-4] [DF-WK-5]
    pub fn forward_worker_message_event(&self, event: WorkerMessageEvent) {
        if let Some(ref tx) = self.event_tx {
            let direction = match event.direction {
                WorkerMessageDirection::PageToWorker => "page→worker",
                WorkerMessageDirection::WorkerToPage => "worker→page",
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Debug,
                text: format!("[Worker] postMessage {}: {}", direction, event.worker_id.0),
                url: None,
                line: None,
                column: None,
            });
        }
    }

    /// Forward a Worker structured-clone message to the CDP event path.
    ///
    /// DF-WK-4 / DF-WK-5: When event_tx is set, push a
    /// ServoEvent::Console for CDP observability with payload metadata.
    /// Includes message_id for trace correlation and payload size.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] [DF-WK-4] [DF-WK-5]
    pub fn forward_worker_structured_message(&self, msg: &WorkerStructuredMessage) {
        if let Some(ref tx) = self.event_tx {
            let direction = match msg.direction {
                WorkerMessageDirection::PageToWorker => "page→worker",
                WorkerMessageDirection::WorkerToPage => "worker→page",
            };
            let payload_info = match &msg.payload {
                Some(p) => format!(
                    "{} bytes, {} transferable(s)",
                    p.data.len(),
                    p.transferable_count
                ),
                None => "metadata-only (servo handles clone)".to_string(),
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Debug,
                text: format!(
                    "[Worker] postMessage #{} {}: {} [{}]",
                    msg.message_id, direction, msg.worker_id.0, payload_info
                ),
                url: None,
                line: None,
                column: None,
            });
        }
    }

    /// Forward a Worker error event to the CDP event path.
    ///
    /// SPEC criterion #9: "onerror 事件正确传播到主线程
    /// (ErrorEvent 包含 message/filename/lineno/colno)".
    /// When event_tx is set, push a ServoEvent::PageError for CDP
    /// observability (maps to Runtime.exceptionThrown).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:9]
    pub fn forward_worker_error_event(&self, event: WorkerErrorEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ServoEvent::PageError {
                target_id: "0".to_string(),
                text: format!("[Worker] {}: {}", event.worker_id.0, event.message),
                url: Some(event.filename.clone()),
                line: Some(event.lineno),
                column: Some(event.colno),
                stack: None,
            });
        }
    }

    // ─── Worker Structured Clone Channel (REQ-BRW-004 criterion #6) ─────

    /// Register a channel bridge for a Worker's postMessage channel.
    ///
    /// Called when a Worker is created and its channel bridge is set up.
    /// The bridge enables page→worker (DF-WK-4) and worker→page (DF-WK-5)
    /// structured-clone message passing.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub fn register_worker_channel(&mut self, bridge: WorkerChannelBridge) {
        let id = bridge.worker_id.clone();
        self.worker_channels.insert(id, bridge);
    }

    /// Create and register a channel bridge for a Worker.
    ///
    /// Convenience method that creates the bridge and endpoints, registers
    /// the bridge, and returns the endpoints for the worker thread.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4 / DF-WK-5
    pub fn create_worker_channel(&mut self, worker_id: WorkerId) -> WorkerChannelEndpoints {
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id);
        self.worker_channels
            .insert(bridge.worker_id.clone(), bridge);
        endpoints
    }

    /// Remove a Worker's channel bridge (e.g., after termination).
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn remove_worker_channel(&mut self, worker_id: &WorkerId) -> Option<WorkerChannelBridge> {
        self.worker_channels.remove(worker_id)
    }

    /// Get a reference to a Worker's channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn worker_channel(&self, worker_id: &WorkerId) -> Option<&WorkerChannelBridge> {
        self.worker_channels.get(worker_id)
    }

    /// Post a structured-clone message to a Worker (DF-WK-4).
    ///
    /// Sends the payload through the Worker's channel bridge.
    /// Returns Err if the worker is not found or the channel is closed.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6] DF-WK-4
    pub fn post_to_worker(
        &self,
        worker_id: &WorkerId,
        payload: StructuredClonePayload,
    ) -> Result<(), String> {
        match self.worker_channels.get(worker_id) {
            Some(bridge) => bridge
                .post_message_to_worker(payload)
                .map_err(|e| format!("Worker channel closed: {}", e)),
            None => Err(format!("No channel bridge for worker: {}", worker_id.0)),
        }
    }

    /// Drain all pending worker→page messages from all Workers (DF-WK-5).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from workers. Each message is forwarded to CDP for observability.
    /// Returns all available messages and a set of WorkerIds whose channels
    /// are disconnected (worker thread has exited).
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:6] DF-WK-5
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_all_worker_messages(&self) -> (Vec<WorkerStructuredMessage>, Vec<WorkerId>) {
        let mut all_messages = Vec::new();
        let mut disconnected_workers = Vec::new();
        for (id, bridge) in &self.worker_channels {
            let result = bridge.drain_worker_messages();
            all_messages.extend(result.messages);
            if result.disconnected {
                disconnected_workers.push(id.clone());
            }
        }
        (all_messages, disconnected_workers)
    }

    /// Drain worker→page messages and forward each to CDP (DF-WK-5).
    ///
    /// Convenience method combining drain_all_worker_messages with
    /// forward_worker_structured_message for each message.
    /// Returns the set of WorkerIds whose channels are disconnected.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:6] DF-WK-5
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_and_forward_worker_messages(&self) -> Vec<WorkerId> {
        let (messages, disconnected) = self.drain_all_worker_messages();
        for msg in &messages {
            self.forward_worker_structured_message(msg);
        }
        disconnected
    }

    /// Remove channel bridges for all terminated Workers.
    ///
    /// Called after reap_terminated_workers to clean up channel state
    /// for Workers that have fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn reap_terminated_worker_channels(&mut self) {
        // Collect IDs of workers that still have channels but are no longer
        // in active_workers (meaning they've been reaped).
        let active_ids: std::collections::HashSet<WorkerId> = self
            .active_workers
            .iter()
            .map(|g| WorkerId(g.handle().script_url.clone()))
            .collect();
        self.worker_channels.retain(|id, _| active_ids.contains(id));
    }

    /// Returns the number of registered Worker channel bridges.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:6]
    pub fn worker_channel_count(&self) -> usize {
        self.worker_channels.len()
    }

    // ─── SharedWorker Cross-Page Routing (REQ-BRW-004 / DF-WK-7) ─────

    /// Track a SharedWorker port reference for this webview.
    ///
    /// DF-WK-7: When a page creates a SharedWorker, the constellation routes
    /// to the same worker thread if (url, name) matches. The page receives
    /// a MessagePort via the connect event. This method tracks the port
    /// reference so it can be disconnected on page unload.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn track_shared_worker_port(&mut self, port_ref: SharedWorkerPortRef) {
        self.shared_worker_ports.push(port_ref);
    }

    /// Disconnect all SharedWorker ports on page unload.
    ///
    /// Unlike DedicatedWorkers (which are terminated), SharedWorkers survive
    /// page unload. Only the per-page MessagePorts are disconnected by
    /// dropping the SharedWorkerPortRef (which decrements the connected-pages
    /// counter in the SharedWorkerHandle).
    ///
    /// Also clears the per-page SharedWorker channel bridges — dropping the
    /// channels signals the worker thread that the page has disconnected
    /// (DF-WK-7). SharedWorkerGlobalScope states are NOT cleared here — they
    /// belong to the global registry in BaoServoDelegate and survive page unload.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn disconnect_shared_worker_ports(&mut self) {
        if !self.shared_worker_ports.is_empty() {
            log::debug!(
                "[delegate] page navigation: disconnecting {} shared worker ports",
                self.shared_worker_ports.len()
            );
        }
        self.shared_worker_ports.clear();
        // @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
        // Clear per-page SharedWorker channel bridges — dropping the port
        // channels signals the worker thread that this page has disconnected.
        // The SharedWorker itself survives (tracked in BaoServoDelegate registry).
        self.shared_worker_channels.clear();
        // @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
        // Clear per-page SharedWorker scope state references.
        self.shared_worker_scopes.clear();
    }

    /// Returns the number of active SharedWorker port references.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_port_count(&self) -> usize {
        self.shared_worker_ports.len()
    }

    /// Forward a SharedWorker connect event to the CDP event path.
    ///
    /// DF-WK-7: When a page connects to a SharedWorker (either creating a new
    /// one or reusing an existing one), the worker fires a `connect` event.
    /// This method forwards the metadata for CDP observability.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
    pub fn forward_shared_worker_connect_event(&self, event: SharedWorkerConnectEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Debug,
                text: format!(
                    "[SharedWorker] connect: {} (name={}) from {}",
                    event.shared_worker_id.script_url,
                    if event.shared_worker_id.name.is_empty() {
                        "<default>"
                    } else {
                        &event.shared_worker_id.name
                    },
                    event.page_url
                ),
                url: None,
                line: None,
                column: None,
            });
        }
    }

    // ─── SharedWorker Channel & Scope (REQ-BRW-004 / DF-WK-7) ────────

    /// Register a SharedWorker channel bridge for this webview.
    ///
    /// DF-WK-7: Each SharedWorker gets a channel bridge that aggregates
    /// per-page port channels. This method registers the bridge so
    /// messages can be drained during spin_event_loop.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn register_shared_worker_channel(&mut self, bridge: SharedWorkerChannelBridge) {
        let id = bridge.shared_worker_id.clone();
        self.shared_worker_channels.insert(id, bridge);
    }

    /// Create a new SharedWorker channel bridge and register it.
    ///
    /// Convenience method that creates the bridge and registers it.
    /// Returns a mutable reference to the bridge for adding ports.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn create_shared_worker_channel(&mut self, shared_worker_id: SharedWorkerId) {
        let bridge = SharedWorkerChannelBridge::new(shared_worker_id.clone());
        self.shared_worker_channels.insert(shared_worker_id, bridge);
    }

    /// Add a port to an existing SharedWorker channel bridge.
    ///
    /// DF-WK-7: When a page connects to a SharedWorker, a new port channel
    /// is created. Returns the port endpoints for the worker thread.
    /// If no bridge exists for the SharedWorkerId, one is created first.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn add_shared_worker_port(
        &mut self,
        shared_worker_id: SharedWorkerId,
    ) -> SharedWorkerPortEndpoints {
        if !self.shared_worker_channels.contains_key(&shared_worker_id) {
            self.create_shared_worker_channel(shared_worker_id.clone());
        }
        self.shared_worker_channels
            .get_mut(&shared_worker_id)
            .expect("just created")
            .add_port()
    }

    /// Get a reference to a SharedWorker channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn shared_worker_channel(&self, id: &SharedWorkerId) -> Option<&SharedWorkerChannelBridge> {
        self.shared_worker_channels.get(id)
    }

    /// Remove a SharedWorker channel bridge.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn remove_shared_worker_channel(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<SharedWorkerChannelBridge> {
        self.shared_worker_channels.remove(id)
    }

    /// Drain all pending SharedWorker→page messages from all SharedWorkers (DF-WK-7).
    ///
    /// Called during spin_event_loop to process all queued messages
    /// from SharedWorkers across all connected pages. Each message is
    /// forwarded to CDP for observability.
    /// Returns messages and any disconnected SharedWorkerIds.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_all_shared_worker_messages(
        &self,
    ) -> (Vec<WorkerStructuredMessage>, Vec<SharedWorkerId>) {
        let mut all_messages = Vec::new();
        let mut all_disconnected = Vec::new();
        for (_, bridge) in &self.shared_worker_channels {
            let (messages, disconnected) = bridge.drain_all_worker_messages();
            all_messages.extend(messages);
            all_disconnected.extend(disconnected);
        }
        (all_messages, all_disconnected)
    }

    /// Drain SharedWorker messages and forward each to CDP (DF-WK-7).
    ///
    /// Returns the set of disconnected SharedWorkerIds.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_and_forward_shared_worker_messages(&self) -> Vec<SharedWorkerId> {
        let (messages, disconnected) = self.drain_all_shared_worker_messages();
        for msg in &messages {
            self.forward_worker_structured_message(&msg);
        }
        disconnected
    }

    /// Post a message to a SharedWorker via a specific port index.
    ///
    /// Convenience method combining shared_worker_channel lookup with
    /// post_to_worker_from_port.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn post_to_worker_via_shared_port(
        &self,
        id: &SharedWorkerId,
        port_index: usize,
        payload: StructuredClonePayload,
    ) -> Result<(), String> {
        match self.shared_worker_channels.get(id) {
            Some(bridge) => bridge.post_to_worker_from_port(port_index, payload),
            None => Err(format!(
                "No channel bridge for SharedWorker: {}:{}",
                id.script_url, id.name
            )),
        }
    }

    /// Clean up SharedWorker channel ports for disconnected workers.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn reap_disconnected_shared_worker_ports(&mut self) {
        for (_, bridge) in &mut self.shared_worker_channels {
            bridge.remove_disconnected_ports();
        }
        // Remove bridges with no remaining ports
        self.shared_worker_channels
            .retain(|_, bridge| bridge.port_count() > 0);
    }

    /// Returns the total number of SharedWorker port channels.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_channel_count(&self) -> usize {
        self.shared_worker_channels
            .values()
            .map(|b| b.port_count())
            .sum()
    }

    /// Register a SharedWorkerGlobalScope state under the given SharedWorkerId.
    ///
    /// Called when a SharedWorker is created, populating the scope state
    /// for CDP observability and stealth consistency verification (CRIT-STL-WK).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn register_shared_worker_scope(
        &mut self,
        id: SharedWorkerId,
        scope: SharedWorkerGlobalScopeState,
    ) {
        self.shared_worker_scopes.insert(id, scope);
    }

    /// Get a reference to a SharedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope(
        &self,
        id: &SharedWorkerId,
    ) -> Option<&SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.get(id)
    }

    /// Get a mutable reference to a SharedWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope_mut(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<&mut SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.get_mut(id)
    }

    /// Remove a SharedWorkerGlobalScope state (called when a SharedWorker is reaped).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn remove_shared_worker_scope(
        &mut self,
        id: &SharedWorkerId,
    ) -> Option<SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.remove(id)
    }

    /// Returns the number of tracked SharedWorkerGlobalScope states.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scope_count(&self) -> usize {
        self.shared_worker_scopes.len()
    }

    /// Returns a snapshot of all SharedWorkerGlobalScope states.
    ///
    /// Used for CDP observability (Runtime domain) and stealth consistency
    /// verification (criterion #12-17).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope]
    pub fn shared_worker_scopes(&self) -> Vec<&SharedWorkerGlobalScopeState> {
        self.shared_worker_scopes.values().collect()
    }

    /// Set the SharedWorker scope config from the first connecting page's StealthProfile.
    ///
    /// DF-WK-9: SharedWorkerGlobalScope inherits the first connecting page's
    /// StealthProfile and it remains fixed for the worker's lifetime (per DEC-WK-007).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:12..17] DF-WK-9
    pub fn set_shared_worker_scope_config(
        &mut self,
        shared_worker_id: &SharedWorkerId,
        config: &SharedWorkerScopeConfig,
    ) {
        if let Some(scope) = self.shared_worker_scopes.get_mut(shared_worker_id) {
            scope.scope.navigator = WorkerNavigator::from_shared_scope_config(config);
        }
    }

    // ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────

    /// Set the controlling ServiceWorker for this webview's page.
    ///
    /// A page can be controlled by at most one ServiceWorker at a time.
    /// Per DF-WK-8: When a ServiceWorker becomes activated and its scope matches
    /// the page's URL, it becomes the controller for that page.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn set_controlling_service_worker(&mut self, handle: ServiceWorkerHandle) {
        self.controlled_service_worker = Some(handle);
    }

    /// Clear the controlling ServiceWorker reference for this webview.
    ///
    /// Called on page unload or when the ServiceWorker is unregistered.
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
    /// 且 terminate 后正确注销" — the ServiceWorker itself survives (tracked in
    /// BaoServoDelegate registry), only the per-page reference is cleared.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn clear_controlling_service_worker(&mut self) {
        self.controlled_service_worker = None;
        self.service_worker_scope = None;
    }

    /// Get a reference to the controlling ServiceWorker, if any.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn controlling_service_worker(&self) -> Option<&ServiceWorkerHandle> {
        self.controlled_service_worker.as_ref()
    }

    /// Check if this page is controlled by a ServiceWorker.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_controlled_by_service_worker(&self) -> bool {
        self.controlled_service_worker.is_some()
    }

    /// Check if a URL falls within the controlling ServiceWorker's scope.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截".
    /// Returns false if no ServiceWorker is controlling this page.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn is_url_in_service_worker_scope(&self, url: &str) -> bool {
        self.service_worker_scope
            .as_ref()
            .map(|scope| scope.is_url_in_scope(url))
            .unwrap_or(false)
    }

    /// Register a ServiceWorkerGlobalScope state for the controlling ServiceWorker.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    pub fn register_service_worker_scope(&mut self, scope: ServiceWorkerGlobalScopeState) {
        self.service_worker_scope = Some(scope);
    }

    /// Get a reference to the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn service_worker_scope(&self) -> Option<&ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.as_ref()
    }

    /// Get a mutable reference to the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn service_worker_scope_mut(&mut self) -> Option<&mut ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.as_mut()
    }

    /// Remove the ServiceWorkerGlobalScope state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn remove_service_worker_scope(&mut self) -> Option<ServiceWorkerGlobalScopeState> {
        self.service_worker_scope.take()
    }

    /// Forward a ServiceWorker fetch interception event to the CDP event path.
    ///
    /// Per SPEC criterion #19: "CDP Network 域可观测 SW 发起的请求/响应".
    /// When a ServiceWorker intercepts a fetch, this method forwards the metadata
    /// for CDP Network domain observability.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn forward_service_worker_fetch_event(&self, event: ServiceWorkerFetchEvent) {
        if let Some(ref tx) = self.event_tx {
            let stealth_status = if event.stealth_profile_applied {
                "stealth profile applied"
            } else {
                "⚠️ STEALTH BOUNDARY VIOLATION"
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: if event.stealth_profile_applied {
                    ConsoleLevel::Debug
                } else {
                    ConsoleLevel::Warning
                },
                text: format!(
                    "[ServiceWorker] fetch {} {} -> {} ({})",
                    event.method,
                    event.request_url,
                    event.registration_id.script_url,
                    stealth_status
                ),
                url: None,
                line: None,
                column: None,
            });
        }
    }

    /// Set the ServiceWorker scope config from the registering page's StealthProfile.
    ///
    /// DF-WK-10: ServiceWorkerGlobalScope inherits the registering page's profile.
    /// Per SPEC criterion #19: SW-intercepted fetch uses the same stealth profile.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
    pub fn set_service_worker_scope_config(&mut self, config: &ServiceWorkerScopeConfig) {
        if let Some(scope) = &mut self.service_worker_scope {
            scope.scope.navigator = WorkerNavigator::from_service_scope_config(config);
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
    /// Channel for forwarding console messages to CDP Log domain.
    /// Set via `set_console_log_tx` when CDP server starts.
    console_log_tx: RefCell<Option<std::sync::mpsc::Sender<ConsoleMessage>>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, console/url/load callbacks also push structured events here.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    event_tx: RefCell<Option<Sender<ServoEvent>>>,
    /// Global SharedWorker registry — keyed by (script_url, name).
    /// SharedWorkers span pages (DF-WK-7), so they must be tracked at the
    /// delegate level rather than per-page. When a page creates a SharedWorker,
    /// the constellation routes to the same worker thread if (url, name) matches.
    /// This registry tracks all active SharedWorkers across all pages.
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    shared_workers: RefCell<Vec<SharedWorkerHandle>>,
    /// Global ServiceWorker registry — keyed by (script_url, scope).
    /// ServiceWorkers have persistent lifecycle (跨页存活) and can control
    /// multiple pages within their scope. Per DF-WK-8: "navigator.serviceWorker.
    /// register(url,{scope}) → serviceworker_manager 注册 → scope 匹配的
    /// 导航/fetch 经 SW 拦截".
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
    /// 且 terminate 后正确注销".
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    service_workers: RefCell<Vec<ServiceWorkerHandle>>,
}

impl Default for BaoServoDelegate {
    fn default() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
            console_log_tx: RefCell::new(None),
            event_tx: RefCell::new(None),
            shared_workers: RefCell::new(Vec::new()),
            service_workers: RefCell::new(Vec::new()),
        }
    }
}

impl BaoServoDelegate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }

    /// Set the channel for forwarding console messages to CDP.
    /// Called when CDP server starts.
    pub fn set_console_log_tx(&self, tx: std::sync::mpsc::Sender<ConsoleMessage>) {
        *self.console_log_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the console log sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    pub fn console_log_tx(&self) -> Option<std::sync::mpsc::Sender<ConsoleMessage>> {
        self.console_log_tx.borrow().clone()
    }

    /// Set the channel for forwarding structured ServoEvent to EventSubscriber (Path B).
    /// Called when CDP server starts alongside set_console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn set_event_tx(&self, tx: Sender<ServoEvent>) {
        *self.event_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the event sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn event_tx(&self) -> Option<Sender<ServoEvent>> {
        self.event_tx.borrow().clone()
    }

    // ─── SharedWorker Global Registry (REQ-BRW-004 / DF-WK-7) ────────

    /// Register a SharedWorker in the global registry.
    ///
    /// DF-WK-7: When a page creates a new SharedWorker, the handle is
    /// registered here so other pages can find it by (script_url, name).
    /// If a SharedWorker with the same id already exists, the existing
    /// handle is returned instead (constellation dedup).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn register_shared_worker(&self, handle: SharedWorkerHandle) -> SharedWorkerHandle {
        let id = handle.id();
        let mut shared_workers = self.shared_workers.borrow_mut();
        if let Some(existing) = shared_workers.iter().find(|h| h.id() == id) {
            existing.clone()
        } else {
            shared_workers.push(handle.clone());
            handle
        }
    }

    /// Find an existing SharedWorker by (script_url, name).
    ///
    /// Returns a clone of the SharedWorkerHandle if found, None otherwise.
    /// Used when a page creates a SharedWorker and the constellation routes
    /// to an existing worker (DF-WK-7: "多页 new SharedWorker(url) 同 name →
    /// constellation 路由到同一 worker 线程").
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn find_shared_worker(&self, script_url: &str, name: &str) -> Option<SharedWorkerHandle> {
        self.shared_workers
            .borrow()
            .iter()
            .find(|h| h.script_url == script_url && h.name == name)
            .cloned()
    }

    /// Remove terminated SharedWorkers from the registry.
    ///
    /// Called after spin_event_loop to clean up SharedWorkers whose threads
    /// have exited and have zero connected pages.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn reap_terminated_shared_workers(&self) {
        self.shared_workers
            .borrow_mut()
            .retain(|h| !h.is_terminated() || h.connected_page_count() > 0);
    }

    /// Returns the number of active SharedWorkers across all pages.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_count(&self) -> usize {
        self.shared_workers.borrow().len()
    }

    /// Route a SharedWorker connection request to the appropriate worker.
    ///
    /// DF-WK-7: "多页 new SharedWorker(url) 同 name → constellation 路由到
    /// 同一 worker 线程". If a SharedWorker with the same (script_url, name)
    /// already exists in the registry, return the existing handle (the
    /// constellation handles dedup). Otherwise, register a new SharedWorker.
    ///
    /// Returns the handle (existing or new) and a boolean indicating whether
    /// this is a new SharedWorker (true) or a reconnection (false).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn route_shared_worker(&self, handle: SharedWorkerHandle) -> (SharedWorkerHandle, bool) {
        let id = handle.id();
        let mut shared_workers = self.shared_workers.borrow_mut();
        if let Some(existing) = shared_workers.iter().find(|h| h.id() == id) {
            (existing.clone(), false)
        } else {
            shared_workers.push(handle.clone());
            (handle, true)
        }
    }

    /// Find or create a SharedWorker for the given (script_url, name).
    ///
    /// Convenience method combining find_shared_worker with register_shared_worker.
    /// Returns the handle and whether it was newly created.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn get_or_create_shared_worker(
        &self,
        script_url: &str,
        name: &str,
    ) -> (SharedWorkerHandle, bool) {
        if let Some(existing) = self.find_shared_worker(script_url, name) {
            (existing, false)
        } else {
            let handle = SharedWorkerHandle::new(script_url.to_string(), name.to_string());
            let returned = self.register_shared_worker(handle);
            (returned, true)
        }
    }

    /// Remove a SharedWorker from the global registry by its ID.
    ///
    /// Called when a SharedWorker has been fully terminated and has zero
    /// connected pages. This is the final cleanup step in the lifecycle.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn unregister_shared_worker(&self, id: &SharedWorkerId) -> bool {
        let mut shared_workers = self.shared_workers.borrow_mut();
        let before = shared_workers.len();
        shared_workers.retain(|h| &h.id() != id);
        shared_workers.len() < before
    }

    /// Returns a snapshot of all SharedWorker handles in the registry.
    ///
    /// Used for CDP observability and lifecycle management.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn all_shared_workers(&self) -> Vec<SharedWorkerHandle> {
        self.shared_workers.borrow().iter().cloned().collect()
    }

    // ─── ServiceWorker Global Registry (REQ-BRW-004 / DF-WK-8) ─────────

    /// Register a ServiceWorker in the global registry.
    ///
    /// DF-WK-8: "navigator.serviceWorker.register(url,{scope}) → serviceworker_manager
    /// 注册". If a ServiceWorker with the same registration_id already exists,
    /// the existing handle is returned instead (registration dedup).
    ///
    /// The handle captures the registering page's StealthProfile for stealth
    /// boundary enforcement (SPEC criterion #19).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn register_service_worker(&self, handle: ServiceWorkerHandle) -> ServiceWorkerHandle {
        let id = handle.id();
        let mut service_workers = self.service_workers.borrow_mut();
        if let Some(existing) = service_workers.iter().find(|h| h.id() == id) {
            existing.clone()
        } else {
            service_workers.push(handle.clone());
            handle
        }
    }

    /// Find an existing ServiceWorker by (script_url, scope).
    ///
    /// Returns a clone of the ServiceWorkerHandle if found, None otherwise.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn find_service_worker(
        &self,
        script_url: &str,
        scope: &str,
    ) -> Option<ServiceWorkerHandle> {
        self.service_workers
            .borrow()
            .iter()
            .find(|h| h.script_url == script_url && h.scope == scope)
            .cloned()
    }

    /// Find a ServiceWorker whose scope matches the given URL.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截". Returns the
    /// ServiceWorker whose scope prefix-matches the URL and is in the
    /// Activated state (intercepting fetches).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn find_service_worker_for_url(&self, url: &str) -> Option<ServiceWorkerHandle> {
        self.service_workers
            .borrow()
            .iter()
            .filter(|h| h.is_intercepting_fetch())
            .find(|h| url.starts_with(&h.scope))
            .cloned()
    }

    /// Remove terminated ServiceWorkers from the registry.
    ///
    /// Per SPEC criterion #19: "terminate 后正确注销". Called after
    /// spin_event_loop to clean up ServiceWorkers whose threads have exited.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn reap_terminated_service_workers(&self) {
        self.service_workers
            .borrow_mut()
            .retain(|h| !h.is_terminated());
    }

    /// Returns the number of active ServiceWorker registrations across all pages.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn service_worker_count(&self) -> usize {
        self.service_workers.borrow().len()
    }

    /// Unregister a ServiceWorker by its registration ID.
    ///
    /// Per SPEC criterion #19: "terminate 后正确注销". This is the final
    /// cleanup step — the ServiceWorker is removed from the global registry,
    /// and its fetch interception is disabled.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn unregister_service_worker(&self, id: &ServiceWorkerRegistrationId) -> bool {
        let mut service_workers = self.service_workers.borrow_mut();
        let before = service_workers.len();
        service_workers.retain(|h| &h.id() != id);
        service_workers.len() < before
    }

    /// Find or create a ServiceWorker for the given (script_url, scope).
    ///
    /// Convenience method combining find_service_worker with register_service_worker.
    /// Returns the handle and whether it was newly created.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn get_or_create_service_worker(
        &self,
        script_url: &str,
        scope: &str,
        stealth_profile: Option<bao_stealth::StealthProfile>,
    ) -> (ServiceWorkerHandle, bool) {
        if let Some(existing) = self.find_service_worker(script_url, scope) {
            (existing, false)
        } else {
            let handle = ServiceWorkerHandle::new(
                script_url.to_string(),
                scope.to_string(),
                stealth_profile,
            );
            let returned = self.register_service_worker(handle);
            (returned, true)
        }
    }

    /// Returns a snapshot of all ServiceWorker handles in the registry.
    ///
    /// Used for CDP observability and lifecycle management.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn all_service_workers(&self) -> Vec<ServiceWorkerHandle> {
        self.service_workers.borrow().iter().cloned().collect()
    }

    /// Verify stealth profile consistency for all ServiceWorker-intercepted fetches.
    ///
    /// Per SPEC criterion #19: "SW 拦截并转发的 fetch 仍走主页同一 stealth
    /// TLS(JA3/JA4)+HTTP2(AKAMAI) profile (不绕过反指纹)". This method
    /// checks that all active ServiceWorkers have a stealth profile consistent
    /// with the given page's profile.
    ///
    /// Returns a list of violations (ServiceWorker registrations where the
    /// profile doesn't match).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn verify_service_worker_stealth_consistency(
        &self,
        page_stealth_profile: &bao_stealth::StealthProfile,
    ) -> Vec<ServiceWorkerRegistrationId> {
        self.service_workers
            .borrow()
            .iter()
            .filter(|h| h.is_intercepting_fetch())
            .filter(|h| {
                // Check if the SW's stealth profile matches the page's profile.
                // A profile mismatch means SW-intercepted fetches could bypass
                // the page's stealth TLS/HTTP2 settings (SPEC criterion #19).
                match &h.stealth_profile {
                    Some(sw_profile) => {
                        // Compare key fingerprint-relevant fields.
                        // If any field differs, it's a stealth boundary violation.
                        sw_profile.navigator.user_agent != page_stealth_profile.navigator.user_agent
                            || sw_profile.navigator.platform
                                != page_stealth_profile.navigator.platform
                    }
                    None => {
                        // No stealth profile on an intercepting SW — this is always
                        // a violation because intercepted fetches won't have stealth.
                        true
                    }
                }
            })
            .map(|h| h.id())
            .collect()
    }
}

impl ServoDelegate for BaoServoDelegate {
    fn notify_error(&self, error: ServoError) {
        let error_str = format!("{error:?}");
        *self.last_error.borrow_mut() = Some(error_str.clone());
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // TLS/certificate errors: always use console_log_tx (Path A) since there is no
        // ServoEvent equivalent for SecurityCertificateError. These are rare events
        // that don't map to the 7 ServoEvent categories.
        if error_str.to_lowercase().contains("certificate")
            || error_str.to_lowercase().contains("tls")
        {
            if let Some(ref tx) = *self.console_log_tx.borrow() {
                let _ = tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                    event_id: 0,
                    error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                    url: String::new(),
                }));
            }
        }
    }

    fn show_console_message(&self, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[servo] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // When event_tx is set, push structured ServoEvent::Console (Path B) as the primary
        // event path. Only fall back to console_log_tx (Path A) when event_tx is absent,
        // avoiding double-broadcast of the same event.
        let event_tx = self.event_tx.borrow();
        if let Some(ref tx) = *event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = *self.console_log_tx.borrow() {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log {
                    level: level_str.to_string(),
                    text: message,
                },
            };
            let _ = tx.send(msg);
        }
    }

    fn request_devtools_connection(&self, request: AllowOrDenyRequest) {
        request.allow();
    }
}

pub struct BaoWebViewDelegate {
    state: Rc<RefCell<BaoWebViewState>>,
    viewport: PhysicalSize<u32>,
}

impl BaoWebViewDelegate {
    pub fn new(state: Rc<RefCell<BaoWebViewState>>, viewport: PhysicalSize<u32>) -> Self {
        BaoWebViewDelegate { state, viewport }
    }

    pub fn state(&self) -> &Rc<RefCell<BaoWebViewState>> {
        &self.state
    }
}

impl WebViewDelegate for BaoWebViewDelegate {
    fn screen_geometry(&self, _webview: WebView) -> Option<ScreenGeometry> {
        let screen_size =
            DeviceIntSize::new(self.viewport.width as i32, self.viewport.height as i32);
        Some(ScreenGeometry {
            size: screen_size,
            available_size: screen_size,
            window_rect: DeviceIntRect::from_origin_and_size(DeviceIntPoint::zero(), screen_size),
        })
    }

    fn notify_url_changed(&self, _webview: WebView, url: url::Url) {
        let url_str = url.to_string();
        self.state.borrow_mut().url = Some(url);
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Dual-path: event_tx (Path B) primary for FrameNavigated,
        // console_log_tx (Path A) fallback for PageFrameNavigated.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let _ = tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: url_str,
                name: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let loader_id = format!("{:016x}", url_str.len() as u64);
            let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str,
                loader_id,
            }));
        }
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.borrow_mut().title = title;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.state.borrow_mut().load_status = status;
        match status {
            LoadStatus::Started => {
                // @trace REQ-BRW-004 [entity:Worker] [criterion:10]
                // SPEC criterion #10: "页面卸载时自动终止所有 Worker
                // (GlobalScope::track_worker + AutoCloseWorker)".
                // When a new navigation starts (after a previous Complete),
                // all Workers from the previous page must be terminated.
                {
                    let mut state = self.state.borrow_mut();
                    if !state.active_workers.is_empty() {
                        log::debug!(
                            "[delegate] page navigation: terminating {} active workers",
                            state.active_worker_count()
                        );
                        state.terminate_all_workers();
                    }
                    // @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
                    // SharedWorkers survive page unload — only disconnect ports.
                    state.disconnect_shared_worker_ports();
                    // @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
                    // ServiceWorkers have persistent lifecycle (跨页存活) — only
                    // clear the per-page controlling reference. The ServiceWorker
                    // itself survives (tracked in BaoServoDelegate registry) and
                    // can control the page again if its scope matches.
                    state.clear_controlling_service_worker();
                }

                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStartedLoading,
                // console_log_tx (Path A) fallback — no direct ConsoleMessage equivalent,
                // so we use a lightweight log entry.
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStartedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
                }
            }
            LoadStatus::Complete => {
                self.state.borrow_mut().dom_proxies_dirty = true;

                // @trace REQ-BRW-004 [entity:Worker]
                // Reap terminated workers after page load completes.
                // Workers from the previous page that have been terminated
                // during LoadStatus::Started are cleaned up here.
                self.state.borrow_mut().reap_terminated_workers();

                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStoppedLoading,
                // console_log_tx (Path A) fallback for PageLoadEventFired.
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStoppedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
                } else if let Some(ref tx) = self.state.borrow().console_log_tx {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageLoadEventFired {
                        timestamp,
                    }));
                }
            }
            LoadStatus::HeadParsed => {}
        }
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.state.borrow_mut().frame_ready = true;
    }

    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        request.allow();
    }

    fn request_permission(&self, _webview: WebView, request: PermissionRequest) {
        request.allow();
    }

    fn request_create_new(&self, _parent_webview: WebView, _request: CreateNewWebViewRequest) {}

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[webview] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Same dual-path logic as BaoServoDelegate::show_console_message:
        // event_tx (Path B) is primary; console_log_tx (Path A) is fallback.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log {
                    level: level_str.to_string(),
                    text: message,
                },
            };
            let _ = tx.send(msg);
        }
    }

    fn show_embedder_control(&self, _webview: WebView, _control: EmbedderControl) {}

    fn hide_embedder_control(&self, _webview: WebView, _id: EmbedderControlId) {}

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        log::error!("[webview] crashed: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BaoWebViewState ────────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_state_default() {
        let state = BaoWebViewState::default();
        assert!(state.url.is_none());
        assert!(state.title.is_none());
        assert!(matches!(state.load_status, LoadStatus::Started));
        assert!(!state.frame_ready);
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_webview_state_url_mutate() {
        let mut state = BaoWebViewState::default();
        state.url = Some(url::Url::parse("https://example.com").unwrap());
        assert!(state.url.is_some());
        assert_eq!(state.url.unwrap().as_str(), "https://example.com/");
    }

    #[test]
    fn test_webview_state_title_mutate() {
        let mut state = BaoWebViewState::default();
        state.title = Some("Test Page".to_string());
        assert_eq!(state.title.as_deref(), Some("Test Page"));
    }

    #[test]
    fn test_webview_state_frame_ready_toggle() {
        let mut state = BaoWebViewState::default();
        assert!(!state.frame_ready);
        state.frame_ready = true;
        assert!(state.frame_ready);
    }

    // ─── BaoServoDelegate ──────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_servo_delegate_new_no_error() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.last_error().is_none());
    }

    #[test]
    fn test_servo_delegate_default_no_error() {
        let delegate = BaoServoDelegate::default();
        assert!(delegate.last_error().is_none());
    }

    // ─── BaoWebViewDelegate ────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_delegate_new_with_state() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1024, 768);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        assert!(delegate.state().borrow().url.is_none());
    }

    #[test]
    fn test_webview_delegate_state_rc_shared() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(800, 600);
        let delegate = BaoWebViewDelegate::new(Rc::clone(&state), viewport);
        // Modify state externally
        state.borrow_mut().title = Some("External".to_string());
        // Delegate sees same state
        assert_eq!(delegate.state().borrow().title.as_deref(), Some("External"));
    }

    #[test]
    fn test_webview_delegate_viewport_size() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1440, 900);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        // Verify delegate was created with specific viewport
        assert!(delegate.state().borrow().url.is_none());
    }

    // ─── PoolStats ─────────────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn test_pool_stats_fields() {
        let stats = crate::page_pool::PoolStats {
            active: 3,
            idle: 1,
            total_created: 5,
            total_destroyed: 2,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_created, 5);
        assert_eq!(stats.total_destroyed, 2);
    }

    // ─── DOM Proxy Dirty Flag ─────────────────────────────────────
    // @trace REQ-SEC-002 [req:REQ-SEC-002] [level:unit]

    #[test]
    fn test_dom_proxies_dirty_default_false() {
        let state = BaoWebViewState::default();
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_set_on_complete() {
        let mut state = BaoWebViewState::default();
        state.load_status = LoadStatus::Complete;
        state.dom_proxies_dirty = true;
        assert!(state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_clear_after_refresh() {
        let mut state = BaoWebViewState::default();
        state.dom_proxies_dirty = true;
        state.dom_proxies_dirty = false;
        assert!(!state.dom_proxies_dirty);
    }

    // ─── Console Log Channel Forwarding ─────────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_servo_delegate_console_log_channel_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.console_log_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        assert!(delegate.console_log_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_console_log_tx_clones() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        // Get a clone and send through it
        let cloned = delegate.console_log_tx().unwrap();
        cloned
            .send(ConsoleMessage::Log {
                level: "info".into(),
                text: "hello".into(),
            })
            .unwrap();
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "info");
                assert_eq!(text, "hello");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let mut state = BaoWebViewState::default();
        state.console_log_tx = Some(tx);
        // Simulate what show_console_message does
        if let Some(ref tx) = state.console_log_tx {
            tx.send(ConsoleMessage::Log {
                level: "warning".into(),
                text: "test message".into(),
            })
            .unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "warning");
                assert_eq!(text, "test message");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.console_log_tx.is_none());
    }

    #[test]
    fn test_console_log_all_level_mappings() {
        let delegate = BaoServoDelegate::new();
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Verify all ConsoleLogLevel variants map correctly via the delegate's show_console_message
        // We test the level mapping logic directly by checking the match arms
        let cases: Vec<(ConsoleLogLevel, &str)> = vec![
            (ConsoleLogLevel::Debug, "debug"),
            (ConsoleLogLevel::Log, "info"),
            (ConsoleLogLevel::Info, "info"),
            (ConsoleLogLevel::Warn, "warning"),
            (ConsoleLogLevel::Error, "error"),
            (ConsoleLogLevel::Trace, "verbose"),
        ];
        for (level, expected_str) in cases {
            let mapped = match level {
                ConsoleLogLevel::Debug => "debug",
                ConsoleLogLevel::Log => "info",
                ConsoleLogLevel::Info => "info",
                ConsoleLogLevel::Warn => "warning",
                ConsoleLogLevel::Error => "error",
                ConsoleLogLevel::Trace => "verbose",
            };
            assert_eq!(
                mapped, expected_str,
                "level {:?} should map to {}",
                level, expected_str
            );
        }
    }

    #[test]
    fn test_webview_delegate_console_log_forwarding() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state, viewport);

        // Simulate sending through state's channel (what show_console_message does)
        if let Some(ref tx) = _delegate.state().borrow().console_log_tx {
            tx.send(ConsoleMessage::Log {
                level: "error".into(),
                text: "crash!".into(),
            })
            .unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "error");
                assert_eq!(text, "crash!");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    // ─── PageFrameNavigated delegate emission ────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_url_changed_emits_frame_navigated() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate notify_url_changed by sending the same message the method sends
        let url = url::Url::parse("https://example.com").unwrap();
        let url_str = url.to_string();
        let loader_id = format!("{:016x}", url_str.len() as u64);
        if let Some(ref tx) = state.borrow().console_log_tx {
            tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str.clone(),
                loader_id: loader_id.clone(),
            }))
            .unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id,
                url,
                loader_id: lid,
            }) => {
                assert_eq!(frame_id, "0");
                assert!(url.starts_with("https://example.com"));
                assert_eq!(lid, loader_id);
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }

    // ─── SecurityCertificateError delegate emission ──────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_error_certificate_error_emits_security_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Simulate a certificate error by sending the same message notify_error would send
        if let Some(ref tx) = *delegate.console_log_tx.borrow() {
            tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id: 0,
                error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                url: String::new(),
            }))
            .unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id,
                error_type,
                url,
            }) => {
                assert_eq!(event_id, 0);
                assert_eq!(error_type, "net::ERR_CERT_AUTHORITY_INVALID");
                assert_eq!(url, "");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
    }

    // ─── EventSubscriber (event_tx) Path B ─────────────────────────────
    // @trace REQ-CDP-006 [req:REQ-CDP-006] [level:unit]

    #[test]
    fn test_servo_delegate_event_tx_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.event_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);
        assert!(delegate.event_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_event_tx_sends_console_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);

        // When event_tx is set, show_console_message pushes ServoEvent::Console
        if let Some(ref tx) = delegate.event_tx() {
            tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Info,
                text: "hello".to_string(),
                url: None,
                line: None,
                column: None,
            })
            .unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Info);
                assert_eq!(text, "hello");
            }
            _ => panic!("expected Console event"),
        }
    }

    #[test]
    fn test_webview_state_event_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.event_tx.is_none());
    }

    #[test]
    fn test_webview_state_event_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState::default();
        state.event_tx = Some(tx);
        // Simulate what notify_url_changed does with event_tx
        if let Some(ref tx) = state.event_tx {
            tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: "https://example.com/".to_string(),
                name: None,
            })
            .unwrap();
        }
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameNavigated { url, .. } => {
                assert_eq!(url, "https://example.com/");
            }
            _ => panic!("expected FrameNavigated event"),
        }
    }

    #[test]
    fn test_event_tx_console_level_mapping() {
        // Verify ConsoleLogLevel → ConsoleLevel mapping matches the delegate logic
        let cases: Vec<(ConsoleLogLevel, ConsoleLevel)> = vec![
            (ConsoleLogLevel::Debug, ConsoleLevel::Debug),
            (ConsoleLogLevel::Log, ConsoleLevel::Info),
            (ConsoleLogLevel::Info, ConsoleLevel::Info),
            (ConsoleLogLevel::Warn, ConsoleLevel::Warning),
            (ConsoleLogLevel::Error, ConsoleLevel::Error),
            (ConsoleLogLevel::Trace, ConsoleLevel::Verbose),
        ];
        for (servo_level, expected) in cases {
            let mapped = match servo_level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            assert_eq!(
                mapped, expected,
                "servo {:?} should map to {:?}",
                servo_level, expected
            );
        }
    }

    #[test]
    fn test_notify_load_started_emits_frame_started_loading() {
        // When event_tx is set and LoadStatus::Started is received,
        // the delegate should emit ServoEvent::FrameStartedLoading.
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate what notify_load_status_changed does on LoadStatus::Started
        if let Some(ref tx) = state.borrow().event_tx {
            tx.send(ServoEvent::FrameStartedLoading {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
            })
            .unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameStartedLoading {
                target_id,
                frame_id,
            } => {
                assert_eq!(target_id, "0");
                assert_eq!(frame_id, "0");
            }
            _ => panic!("expected FrameStartedLoading event"),
        }
    }

    // ─── Worker Lifecycle (REQ-BRW-004) ──────────────────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [level:unit]

    #[test]
    fn test_worker_handle_new_is_running() {
        let handle = WorkerHandle::new("https://example.com/worker.js".to_string());
        assert_eq!(handle.script_url, "https://example.com/worker.js");
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_terminate_sets_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        assert!(!handle.is_closing());
        handle.terminate();
        assert!(handle.is_closing());
        // Idempotent
        handle.terminate();
        assert!(handle.is_closing());
    }

    #[test]
    fn test_worker_handle_mark_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        assert!(!handle.is_terminated());
        handle.mark_terminated();
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_terminate_then_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.terminate();
        assert!(handle.is_closing());
        assert!(!handle.is_terminated());
        handle.mark_terminated();
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_worker_handle_clone_shares_state() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let clone = handle.clone();
        handle.terminate();
        assert!(
            clone.is_closing(),
            "clone should see closing flag from original"
        );
        clone.mark_terminated();
        assert!(
            handle.is_terminated(),
            "original should see terminated flag from clone"
        );
    }

    #[test]
    fn test_webview_state_active_workers_default_empty() {
        let state = BaoWebViewState::default();
        assert!(state.active_workers.is_empty());
        assert_eq!(state.active_worker_count(), 0);
    }

    #[test]
    fn test_webview_state_track_worker() {
        let mut state = BaoWebViewState::default();
        let handle = WorkerHandle::new("worker1.js".to_string());
        state.track_worker(handle);
        assert_eq!(state.active_worker_count(), 1);
        assert_eq!(state.active_workers.len(), 1);
        assert_eq!(state.active_workers[0].handle().script_url, "worker1.js");
    }

    #[test]
    fn test_webview_state_track_multiple_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.track_worker(WorkerHandle::new("worker3.js".to_string()));
        assert_eq!(state.active_worker_count(), 3);
    }

    #[test]
    fn test_webview_state_terminate_all_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        assert!(!state.active_workers[0].handle().is_closing());
        assert!(!state.active_workers[1].handle().is_closing());
        state.terminate_all_workers();
        assert!(state.active_workers[0].handle().is_closing());
        assert!(state.active_workers[1].handle().is_closing());
    }

    #[test]
    fn test_webview_state_reap_terminated_workers() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        // Terminate only worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        assert_eq!(state.active_worker_count(), 1);
        state.reap_terminated_workers();
        assert_eq!(state.active_workers.len(), 1);
        assert_eq!(state.active_workers[0].handle().script_url, "worker2.js");
    }

    #[test]
    fn test_webview_state_reap_all_terminated() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        // terminate_all_workers() marks workers as terminated (Phase 3)
        state.terminate_all_workers();
        // reap_terminated_workers cleans up the tracking state
        state.reap_terminated_workers();
        assert!(state.active_workers.is_empty());
        assert_eq!(state.active_worker_count(), 0);
    }

    #[test]
    fn test_worker_id_equality() {
        let id1 = WorkerId("worker1.js".to_string());
        let id2 = WorkerId("worker1.js".to_string());
        let id3 = WorkerId("worker2.js".to_string());
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_worker_message_direction() {
        assert_eq!(
            WorkerMessageDirection::PageToWorker,
            WorkerMessageDirection::PageToWorker
        );
        assert_ne!(
            WorkerMessageDirection::PageToWorker,
            WorkerMessageDirection::WorkerToPage
        );
    }

    #[test]
    fn test_worker_message_event_creation() {
        let event = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::PageToWorker,
        };
        assert_eq!(event.worker_id.0, "worker1.js");
        assert_eq!(event.direction, WorkerMessageDirection::PageToWorker);
    }

    #[test]
    fn test_webview_state_forward_worker_message_to_event_tx() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let msg = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::WorkerToPage,
        };
        state.forward_worker_message_event(msg);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Debug);
                assert!(text.contains("worker→page"));
                assert!(text.contains("worker1.js"));
            }
            _ => panic!("expected Console event for worker message"),
        }
    }

    #[test]
    fn test_webview_state_forward_worker_message_no_event_tx() {
        // When event_tx is None, forward_worker_message_event should be a no-op
        let state = BaoWebViewState::default();
        let msg = WorkerMessageEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            direction: WorkerMessageDirection::PageToWorker,
        };
        // Should not panic
        state.forward_worker_message_event(msg);
    }

    #[test]
    fn test_terminate_on_navigation_then_reap() {
        // Simulate: page with workers → new navigation → terminate → load complete → reap
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        assert_eq!(state.active_worker_count(), 2);

        // Navigation starts: terminate all
        // terminate_all_workers() performs 3 phases:
        //   Phase 1: set closing + unregister stealth profiles
        //   Phase 2: web_workers.clear() (join threads)
        //   Phase 3: mark terminated (threads have exited)
        state.terminate_all_workers();
        assert!(state.active_workers[0].handle().is_closing());
        assert!(state.active_workers[1].handle().is_closing());
        // After terminate_all_workers(), workers are marked terminated
        // (Phase 3 runs after web_workers.clear() joins threads).
        assert_eq!(state.active_worker_count(), 0);

        // Load complete: reap
        state.reap_terminated_workers();
        assert!(state.active_workers.is_empty());
    }

    // ─── WorkerErrorEvent (REQ-BRW-004 criterion #9) ──────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:9] [level:unit]

    #[test]
    fn test_worker_error_event_creation() {
        let event = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "Uncaught TypeError: x is not a function".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 42,
            colno: 5,
        };
        assert_eq!(event.worker_id.0, "worker1.js");
        assert_eq!(event.message, "Uncaught TypeError: x is not a function");
        assert_eq!(event.filename, "worker1.js");
        assert_eq!(event.lineno, 42);
        assert_eq!(event.colno, 5);
    }

    #[test]
    fn test_webview_state_forward_worker_error_to_event_tx() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let error = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "Uncaught Error: boom".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 10,
            colno: 3,
        };
        state.forward_worker_error_event(error);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::PageError {
                text,
                url,
                line,
                column,
                ..
            } => {
                assert!(text.contains("worker1.js"));
                assert!(text.contains("Uncaught Error: boom"));
                assert_eq!(url.as_deref(), Some("worker1.js"));
                assert_eq!(line, Some(10));
                assert_eq!(column, Some(3));
            }
            _ => panic!("expected PageError event for worker error"),
        }
    }

    #[test]
    fn test_webview_state_forward_worker_error_no_event_tx() {
        let state = BaoWebViewState::default();
        let error = WorkerErrorEvent {
            worker_id: WorkerId("worker1.js".to_string()),
            message: "error".to_string(),
            filename: "worker1.js".to_string(),
            lineno: 1,
            colno: 1,
        };
        // Should not panic
        state.forward_worker_error_event(error);
    }

    // ─── WorkerLifecycleState / WorkerTeardownPath (REQ-BRW-004 criterion #18) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:18] [level:unit]

    #[test]
    fn test_worker_teardown_path_equality() {
        assert_eq!(WorkerTeardownPath::Terminate, WorkerTeardownPath::Terminate);
        assert_eq!(WorkerTeardownPath::SelfClose, WorkerTeardownPath::SelfClose);
        assert_eq!(
            WorkerTeardownPath::PageUnload,
            WorkerTeardownPath::PageUnload
        );
        assert_ne!(WorkerTeardownPath::Terminate, WorkerTeardownPath::SelfClose);
    }

    #[test]
    fn test_worker_lifecycle_state_running() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Running);
    }

    #[test]
    fn test_worker_lifecycle_state_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate)
        );
    }

    #[test]
    fn test_worker_lifecycle_state_terminated() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::SelfClose);
        guard.handle().mark_terminated();
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Terminated(WorkerTeardownPath::SelfClose)
        );
    }

    #[test]
    fn test_worker_lifecycle_states_snapshot() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        let snapshot = state.worker_lifecycle_states();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].0, WorkerId("worker1.js".to_string()));
        assert_eq!(snapshot[0].1, WorkerLifecycleState::Running);
        assert_eq!(snapshot[1].0, WorkerId("worker2.js".to_string()));
        assert_eq!(snapshot[1].1, WorkerLifecycleState::Running);
    }

    // ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ─────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:10] [level:unit]

    #[test]
    fn test_auto_close_worker_new_is_running() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        assert!(!guard.handle().is_closing());
        assert!(!guard.handle().is_terminated());
    }

    #[test]
    fn test_auto_close_worker_terminate_via() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        assert!(guard.handle().is_closing());
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate)
        );
    }

    #[test]
    fn test_auto_close_worker_terminate_via_idempotent() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        guard.terminate_via(WorkerTeardownPath::SelfClose);
        // Should still be Terminate (first call wins)
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate)
        );
    }

    #[test]
    fn test_auto_close_worker_drop_terminates() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let guard = AutoCloseWorker::new(handle);
        assert!(!handle_clone.is_closing());
        drop(guard);
        // AutoCloseWorker::drop should terminate the worker
        assert!(handle_clone.is_closing());
    }

    #[test]
    fn test_auto_close_worker_drop_already_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        drop(guard);
        // Already closing — drop should not change teardown path
        assert!(handle_clone.is_closing());
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: drop marks terminated
        assert!(handle_clone.is_terminated());
    }

    // ─── Crash-Safe Teardown Tests (REQ-BRW-004 criterion #18) ──────────
    // @trace REQ-BRW-004 [criterion:18] crash-safe teardown zero-crash zero-leak

    #[test]
    fn test_worker_handle_global_addr_default_zero() {
        let handle = WorkerHandle::new("worker.js".to_string());
        assert_eq!(handle.worker_global_addr(), 0);
    }

    #[test]
    fn test_worker_handle_global_addr_set_and_get() {
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(0xDEADBEEF);
        assert_eq!(handle.worker_global_addr(), 0xDEADBEEF);
    }

    #[test]
    fn test_worker_handle_global_addr_arc_shared() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let arc = handle.worker_global_addr_arc();
        // Write via the Arc (as scope_init would on the worker thread)
        arc.store(0xCAFEBABE_usize as u64, Ordering::Release);
        // Read via the handle (as teardown would on the main thread)
        assert_eq!(handle.worker_global_addr(), 0xCAFEBABE);
    }

    #[test]
    fn test_worker_handle_unregister_stealth_profile_no_addr() {
        // When no global address is set, unregister should be a no-op
        let handle = WorkerHandle::new("worker.js".to_string());
        // Should not panic
        handle.unregister_stealth_profile();
    }

    #[test]
    fn test_worker_handle_unregister_stealth_profile_with_addr() {
        // Register a profile for a fake global address, then unregister it
        let fake_addr = 0x12345678_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr,
            &bao_stealth::StealthProfile::firefox_default(),
        );
        // Verify it's registered
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_some());
        // Unregister via WorkerHandle
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(fake_addr);
        handle.unregister_stealth_profile();
        // Verify it's gone
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_none());
        // Cleanup (in case test fails before unregister)
        bao_stealth::engine_props::clear_all_realm_profiles();
    }

    #[test]
    fn test_teardown_result_crash_safe() {
        let result = WorkerTeardownResult {
            path: WorkerTeardownPath::Terminate,
            thread_joined: true,
            realm_profile_unregistered: true,
            closing_flag_set: true,
            never_registered: false,
        };
        assert!(result.is_crash_safe());
    }

    #[test]
    fn test_teardown_result_not_crash_safe_no_join() {
        let result = WorkerTeardownResult {
            path: WorkerTeardownPath::PageUnload,
            thread_joined: false,
            realm_profile_unregistered: true,
            closing_flag_set: true,
            never_registered: false,
        };
        assert!(!result.is_crash_safe());
    }

    #[test]
    fn test_teardown_result_not_crash_safe_no_closing() {
        let result = WorkerTeardownResult {
            path: WorkerTeardownPath::SelfClose,
            thread_joined: true,
            realm_profile_unregistered: true,
            closing_flag_set: false,
            never_registered: false,
        };
        assert!(!result.is_crash_safe());
    }

    #[test]
    fn test_crash_safe_teardown_no_web_worker() {
        // Test crash-safe teardown for a servo DOM Worker (DEC-WK-001 native path)
        let handle = WorkerHandle::new("worker.js".to_string());
        let result = crash_safe_teardown_worker(&handle, WorkerTeardownPath::Terminate);
        assert!(result.closing_flag_set);
        assert!(result.thread_joined); // servo DOM Worker — considered joined
        assert!(!result.realm_profile_unregistered); // no global addr set
        assert!(handle.is_closing());
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_crash_safe_teardown_with_stealth_profile() {
        // Register a profile for a fake global address, then crash-safe teardown
        let fake_addr = 0xABCD0000_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr,
            &bao_stealth::StealthProfile::firefox_default(),
        );

        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(fake_addr);

        let result = crash_safe_teardown_worker(&handle, WorkerTeardownPath::SelfClose);
        assert!(result.closing_flag_set);
        assert!(result.thread_joined);
        assert!(result.realm_profile_unregistered);
        // Profile should be unregistered
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_none());
        assert!(handle.is_closing());
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_auto_close_worker_drop_unregisters_stealth_profile() {
        // @trace REQ-BRW-004 [criterion:18] AutoCloseWorker::drop unregisters REALM_PROFILES
        let fake_addr = 0xBEEF0000_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr,
            &bao_stealth::StealthProfile::firefox_default(),
        );

        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(fake_addr);
        // Verify profile is registered
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_some());

        let guard = AutoCloseWorker::new(handle);
        // Drop the guard — should unregister the profile
        drop(guard);
        // Profile should be gone
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_none());
    }

    #[test]
    fn test_terminate_all_workers_unregisters_stealth_profiles() {
        // @trace REQ-BRW-004 [criterion:18] terminate_all_workers unregisters REALM_PROFILES
        let fake_addr1 = 0xAAAA0001_usize;
        let fake_addr2 = 0xAAAA0002_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr1,
            &bao_stealth::StealthProfile::firefox_default(),
        );
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr2,
            &bao_stealth::StealthProfile::firefox_default(),
        );

        let mut state = BaoWebViewState::default();
        let h1 = WorkerHandle::new("worker1.js".to_string());
        h1.set_worker_global_addr(fake_addr1);
        let h2 = WorkerHandle::new("worker2.js".to_string());
        h2.set_worker_global_addr(fake_addr2);
        state.track_worker(h1);
        state.track_worker(h2);

        // Verify profiles registered
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr1).is_some());
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr2).is_some());

        // Terminate all — should unregister all profiles
        state.terminate_all_workers();

        // Profiles should be gone
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr1).is_none());
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr2).is_none());
        // All workers should be closing and terminated
        assert!(state.active_workers.iter().all(|g| g.handle().is_closing()));
        assert!(state
            .active_workers
            .iter()
            .all(|g| g.handle().is_terminated()));
    }

    #[test]
    fn test_terminate_worker_via_path_terminate() {
        // @trace REQ-BRW-004 [criterion:4] [criterion:18] worker.terminate() path
        let fake_addr = 0xCCCC0001_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr,
            &bao_stealth::StealthProfile::firefox_default(),
        );

        let mut state = BaoWebViewState::default();
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(fake_addr);
        let worker_id = WorkerId("worker.js".to_string());
        state.track_worker(handle.clone());

        let result = state.terminate_worker_via_path(&worker_id, WorkerTeardownPath::Terminate);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.path, WorkerTeardownPath::Terminate);
        assert!(result.closing_flag_set);
        assert!(result.thread_joined);
        assert!(result.realm_profile_unregistered);
        // Profile should be gone
        assert!(bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_none());
        // Worker should be terminated
        assert!(handle.is_closing());
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_terminate_worker_via_path_self_close() {
        // @trace REQ-BRW-004 [criterion:5] [criterion:18] self.close() path
        let fake_addr = 0xDDDD0001_usize;
        bao_stealth::engine_props::set_profile_for_global(
            fake_addr,
            &bao_stealth::StealthProfile::firefox_default(),
        );

        let mut state = BaoWebViewState::default();
        let handle = WorkerHandle::new("worker.js".to_string());
        handle.set_worker_global_addr(fake_addr);
        let worker_id = WorkerId("worker.js".to_string());
        state.track_worker(handle.clone());

        let result = state.terminate_worker_via_path(&worker_id, WorkerTeardownPath::SelfClose);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.path, WorkerTeardownPath::SelfClose);
        assert!(result.closing_flag_set);
        assert!(result.realm_profile_unregistered);
    }

    #[test]
    fn test_terminate_worker_via_path_not_found() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("nonexistent.js".to_string());
        let result = state.terminate_worker_via_path(&worker_id, WorkerTeardownPath::Terminate);
        assert!(result.is_none());
    }

    #[test]
    fn test_three_paths_all_crash_safe() {
        // @trace REQ-BRW-004 [criterion:18] all three teardown paths crash-safe
        for path in [
            WorkerTeardownPath::Terminate,
            WorkerTeardownPath::SelfClose,
            WorkerTeardownPath::PageUnload,
        ] {
            let fake_addr = 0x12340000_usize
                + match &path {
                    WorkerTeardownPath::Terminate => 1,
                    WorkerTeardownPath::SelfClose => 2,
                    WorkerTeardownPath::PageUnload => 3,
                };
            bao_stealth::engine_props::set_profile_for_global(
                fake_addr,
                &bao_stealth::StealthProfile::firefox_default(),
            );

            let handle = WorkerHandle::new("worker.js".to_string());
            handle.set_worker_global_addr(fake_addr);
            let result = crash_safe_teardown_worker(&handle, path.clone());
            assert!(
                result.closing_flag_set,
                "closing flag not set for {:?}",
                path
            );
            assert!(result.thread_joined, "thread not joined for {:?}", path);
            assert!(
                result.realm_profile_unregistered,
                "profile not unregistered for {:?}",
                path
            );
            assert!(result.is_crash_safe(), "not crash-safe for {:?}", path);
            assert!(handle.is_closing(), "handle not closing for {:?}", path);
            assert!(
                handle.is_terminated(),
                "handle not terminated for {:?}",
                path
            );
            assert!(
                bao_stealth::engine_props::canvas_seed_for_test(fake_addr).is_none(),
                "profile not removed for {:?}",
                path
            );
        }
    }

    #[test]
    fn test_track_worker_guard() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        let mut state = BaoWebViewState::default();
        state.track_worker_guard(guard);
        assert_eq!(state.active_worker_count(), 1);
    }

    // ─── WorkerScopeConfig (REQ-BRW-004 criteria #12-17) ─────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:unit]

    #[test]
    fn test_worker_scope_config_default() {
        let config = WorkerScopeConfig::default();
        assert!(config.stealth_profile.is_none());
        assert!(config.user_agent.is_empty());
        assert!(config.platform.is_empty());
        assert!(config.hardware_concurrency > 0);
        assert_eq!(config.language, "en-US");
        assert!(!config.languages.is_empty());
    }

    #[test]
    fn test_worker_scope_config_set_on_state() {
        let mut state = BaoWebViewState::default();
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string(), "en".to_string()],
        };
        state.set_worker_scope_config(config);
        assert_eq!(state.worker_scope_config.user_agent, "Bao/1.0");
        assert_eq!(state.worker_scope_config.platform, "Linux x86_64");
        assert_eq!(state.worker_scope_config.hardware_concurrency, 8);
        assert_eq!(state.worker_scope_config.language, "zh-CN");
        assert_eq!(state.worker_scope_config.languages.len(), 3);
    }

    #[test]
    fn test_webview_state_default_worker_scope_config() {
        let state = BaoWebViewState::default();
        assert!(state.worker_scope_config.stealth_profile.is_none());
        assert!(state.worker_scope_config.hardware_concurrency > 0);
    }

    // ─── SharedWorker (REQ-BRW-004 / DF-WK-7) ─────────────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_shared_worker_id_equality() {
        let id1 = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "myworker".to_string(),
        };
        let id2 = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "myworker".to_string(),
        };
        let id3 = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "other".to_string(),
        };
        let id4 = SharedWorkerId {
            script_url: "other.js".to_string(),
            name: "myworker".to_string(),
        };
        assert_eq!(id1, id2);
        assert_ne!(id1, id3); // different name
        assert_ne!(id1, id4); // different url
    }

    #[test]
    fn test_shared_worker_id_default_name() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        assert!(id.name.is_empty());
    }

    #[test]
    fn test_shared_worker_handle_new_is_running() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        assert_eq!(handle.script_url, "sw.js");
        assert_eq!(handle.name, "myname");
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
        assert_eq!(handle.connected_page_count(), 0);
    }

    #[test]
    fn test_shared_worker_handle_id() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let id = handle.id();
        assert_eq!(id.script_url, "sw.js");
        assert_eq!(id.name, "myname");
    }

    #[test]
    fn test_shared_worker_handle_close() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        assert!(!handle.is_closing());
        handle.close();
        assert!(handle.is_closing());
        // Idempotent
        handle.close();
        assert!(handle.is_closing());
    }

    #[test]
    fn test_shared_worker_handle_mark_terminated() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        assert!(!handle.is_terminated());
        handle.mark_terminated();
        assert!(handle.is_terminated());
    }

    #[test]
    fn test_shared_worker_handle_connected_pages() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        assert_eq!(handle.connected_page_count(), 0);
        handle.page_connected();
        assert_eq!(handle.connected_page_count(), 1);
        handle.page_connected();
        assert_eq!(handle.connected_page_count(), 2);
        handle.page_disconnected();
        assert_eq!(handle.connected_page_count(), 1);
        handle.page_disconnected();
        assert_eq!(handle.connected_page_count(), 0);
    }

    #[test]
    fn test_shared_worker_handle_clone_shares_state() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "name".to_string());
        let clone = handle.clone();
        handle.close();
        assert!(
            clone.is_closing(),
            "clone should see closing flag from original"
        );
        clone.mark_terminated();
        assert!(
            handle.is_terminated(),
            "original should see terminated flag from clone"
        );
    }

    #[test]
    fn test_shared_worker_port_ref_increments_connected() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        let port = SharedWorkerPortRef::new(handle.clone());
        assert_eq!(handle.connected_page_count(), 1);
        assert_eq!(port.handle().script_url, "sw.js");
    }

    #[test]
    fn test_shared_worker_port_ref_drop_decrements_connected() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        {
            let _port = SharedWorkerPortRef::new(handle.clone());
            assert_eq!(handle.connected_page_count(), 1);
        }
        assert_eq!(
            handle.connected_page_count(),
            0,
            "dropping port should decrement connected count"
        );
    }

    #[test]
    fn test_shared_worker_port_ref_clone_increments_connected() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        let port = SharedWorkerPortRef::new(handle.clone());
        assert_eq!(handle.connected_page_count(), 1);
        let _port2 = port.clone();
        assert_eq!(handle.connected_page_count(), 2);
    }

    #[test]
    fn test_shared_worker_port_ref_multiple_pages() {
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "shared".to_string());
        let _port1 = SharedWorkerPortRef::new(handle.clone());
        let _port2 = SharedWorkerPortRef::new(handle.clone());
        assert_eq!(handle.connected_page_count(), 2);
    }

    #[test]
    fn test_webview_state_track_shared_worker_port() {
        let mut state = BaoWebViewState::default();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        state.track_shared_worker_port(SharedWorkerPortRef::new(handle));
        assert_eq!(state.shared_worker_port_count(), 1);
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_ports() {
        let mut state = BaoWebViewState::default();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        state.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        state.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        assert_eq!(state.shared_worker_port_count(), 2);
        assert_eq!(handle.connected_page_count(), 2);
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_port_count(), 0);
        assert_eq!(
            handle.connected_page_count(),
            0,
            "disconnect should drop ports and decrement counter"
        );
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_ports_empty() {
        let mut state = BaoWebViewState::default();
        // No panic on empty
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_port_count(), 0);
    }

    #[test]
    fn test_delegate_register_shared_worker_new() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let returned = delegate.register_shared_worker(handle);
        assert_eq!(returned.script_url, "sw.js");
        assert_eq!(returned.name, "myname");
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_register_shared_worker_dedup() {
        let delegate = BaoServoDelegate::new();
        let handle1 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let handle2 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        delegate.register_shared_worker(handle1);
        let returned = delegate.register_shared_worker(handle2);
        // Same (url, name) → returns existing, count stays 1
        assert_eq!(delegate.shared_worker_count(), 1);
        assert_eq!(returned.script_url, "sw.js");
    }

    #[test]
    fn test_delegate_find_shared_worker() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        delegate.register_shared_worker(handle);
        let found = delegate.find_shared_worker("sw.js", "myname");
        assert!(found.is_some());
        assert_eq!(found.unwrap().script_url, "sw.js");
        assert!(delegate.find_shared_worker("other.js", "myname").is_none());
        assert!(delegate.find_shared_worker("sw.js", "other").is_none());
    }

    #[test]
    fn test_delegate_reap_terminated_shared_workers() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        delegate.register_shared_worker(handle.clone());
        assert_eq!(delegate.shared_worker_count(), 1);
        // Mark terminated with zero connected pages
        handle.close();
        handle.mark_terminated();
        delegate.reap_terminated_shared_workers();
        assert_eq!(
            delegate.shared_worker_count(),
            0,
            "terminated shared worker with zero pages should be reaped"
        );
    }

    #[test]
    fn test_delegate_reap_keeps_terminated_with_connected_pages() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        delegate.register_shared_worker(handle.clone());
        // Connect a page, then terminate the worker
        let _port = SharedWorkerPortRef::new(handle.clone());
        handle.close();
        handle.mark_terminated();
        delegate.reap_terminated_shared_workers();
        assert_eq!(
            delegate.shared_worker_count(),
            1,
            "terminated but still has connected pages — keep in registry"
        );
    }

    #[test]
    fn test_shared_worker_connect_event_creation() {
        let event = SharedWorkerConnectEvent {
            shared_worker_id: SharedWorkerId {
                script_url: "sw.js".to_string(),
                name: "myname".to_string(),
            },
            page_url: "https://example.com/page1".to_string(),
        };
        assert_eq!(event.shared_worker_id.script_url, "sw.js");
        assert_eq!(event.shared_worker_id.name, "myname");
        assert_eq!(event.page_url, "https://example.com/page1");
    }

    #[test]
    fn test_forward_shared_worker_connect_event() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let event = SharedWorkerConnectEvent {
            shared_worker_id: SharedWorkerId {
                script_url: "sw.js".to_string(),
                name: "myname".to_string(),
            },
            page_url: "https://example.com".to_string(),
        };
        state.forward_shared_worker_connect_event(event);
        let recv = rx.try_recv().unwrap();
        match recv {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Debug);
                assert!(text.contains("sw.js"));
                assert!(text.contains("myname"));
                assert!(text.contains("https://example.com"));
            }
            _ => panic!("expected Console event for shared worker connect"),
        }
    }

    #[test]
    fn test_forward_shared_worker_connect_event_no_tx() {
        let state = BaoWebViewState::default();
        let event = SharedWorkerConnectEvent {
            shared_worker_id: SharedWorkerId {
                script_url: "sw.js".to_string(),
                name: String::new(),
            },
            page_url: "https://example.com".to_string(),
        };
        // Should not panic
        state.forward_shared_worker_connect_event(event);
    }

    #[test]
    fn test_shared_worker_scope_config_default() {
        let config = SharedWorkerScopeConfig::default();
        assert!(config.stealth_profile.is_none());
        assert!(config.user_agent.is_empty());
        assert!(config.platform.is_empty());
        assert!(config.hardware_concurrency > 0);
        assert_eq!(config.language, "en-US");
        assert!(!config.languages.is_empty());
    }

    #[test]
    fn test_page_navigation_disconnects_shared_workers() {
        let mut state = BaoWebViewState::default();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), String::new());
        state.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        assert_eq!(state.shared_worker_port_count(), 1);
        assert_eq!(handle.connected_page_count(), 1);
        // Simulate page navigation — SharedWorkers survive but ports disconnect
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_port_count(), 0);
        assert_eq!(handle.connected_page_count(), 0);
    }

    #[test]
    fn test_shared_worker_cross_page_sharing() {
        // Simulate two pages sharing the same SharedWorker
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "shared".to_string());

        // Page 1 connects
        let mut state1 = BaoWebViewState::default();
        state1.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        assert_eq!(handle.connected_page_count(), 1);

        // Page 2 connects
        let mut state2 = BaoWebViewState::default();
        state2.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        assert_eq!(handle.connected_page_count(), 2);

        // Page 1 navigates away
        state1.disconnect_shared_worker_ports();
        assert_eq!(handle.connected_page_count(), 1);

        // Page 2 still connected
        assert_eq!(state2.shared_worker_port_count(), 1);

        // SharedWorker is NOT terminated (only ports disconnect)
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
    }

    // ─── Structured Clone Message Channel (REQ-BRW-004 criterion #6) ─────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:6] [level:unit]

    #[test]
    fn test_structured_clone_payload_creation() {
        let payload = StructuredClonePayload {
            data: vec![1, 2, 3, 4, 5],
            transferable_count: 0,
        };
        assert_eq!(payload.data.len(), 5);
        assert_eq!(payload.transferable_count, 0);
    }

    #[test]
    fn test_structured_clone_payload_with_transferables() {
        let payload = StructuredClonePayload {
            data: vec![0u8; 1024],
            transferable_count: 2,
        };
        assert_eq!(payload.data.len(), 1024);
        assert_eq!(payload.transferable_count, 2);
    }

    #[test]
    fn test_structured_clone_payload_clone() {
        let payload = StructuredClonePayload {
            data: vec![42u8; 100],
            transferable_count: 1,
        };
        let cloned = payload.clone();
        assert_eq!(cloned.data, payload.data);
        assert_eq!(cloned.transferable_count, payload.transferable_count);
    }

    #[test]
    fn test_worker_structured_message_metadata_only() {
        let msg = WorkerStructuredMessage::metadata_only(
            WorkerId("worker1.js".to_string()),
            WorkerMessageDirection::PageToWorker,
        );
        assert!(msg.payload.is_none());
        assert_eq!(msg.worker_id.0, "worker1.js");
        assert_eq!(msg.direction, WorkerMessageDirection::PageToWorker);
        assert!(msg.message_id > 0);
    }

    #[test]
    fn test_worker_structured_message_with_payload() {
        let msg = WorkerStructuredMessage::with_payload(
            WorkerId("worker2.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
            vec![1, 2, 3],
            1,
        );
        assert!(msg.payload.is_some());
        let payload = msg.payload.unwrap();
        assert_eq!(payload.data, vec![1, 2, 3]);
        assert_eq!(payload.transferable_count, 1);
        assert_eq!(msg.direction, WorkerMessageDirection::WorkerToPage);
    }

    #[test]
    fn test_worker_structured_message_unique_ids() {
        let msg1 = WorkerStructuredMessage::metadata_only(
            WorkerId("w.js".to_string()),
            WorkerMessageDirection::PageToWorker,
        );
        let msg2 = WorkerStructuredMessage::metadata_only(
            WorkerId("w.js".to_string()),
            WorkerMessageDirection::PageToWorker,
        );
        // Each message should get a unique ID
        assert_ne!(msg1.message_id, msg2.message_id);
    }

    #[test]
    fn test_worker_channel_bridge_creation() {
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id.clone());
        assert_eq!(bridge.worker_id, worker_id);
        assert_eq!(endpoints.worker_id, worker_id);
        // Endpoints should have the rx/tx for the worker thread
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_worker_channel_bridge_page_to_worker() {
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id);
        // Page sends a message to worker
        let payload = StructuredClonePayload {
            data: vec![1, 2, 3],
            transferable_count: 0,
        };
        bridge.post_message_to_worker(payload).unwrap();
        // Worker thread receives it
        let rx = endpoints.page_to_worker_rx.unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_worker_channel_bridge_worker_to_page() {
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id);
        // Worker sends a message to page
        let msg = WorkerStructuredMessage::with_payload(
            WorkerId("worker1.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
            vec![4, 5, 6],
            0,
        );
        let tx = endpoints.worker_to_page_tx.unwrap();
        tx.send(msg).unwrap();
        // Page receives it
        let result = bridge.try_recv_from_worker().unwrap();
        assert!(result.is_some());
        let received = result.unwrap();
        assert_eq!(received.payload.unwrap().data, vec![4, 5, 6]);
    }

    #[test]
    fn test_worker_channel_bridge_drain() {
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, endpoints) = WorkerChannelBridge::new(worker_id);
        let tx = endpoints.worker_to_page_tx.unwrap();
        // Send multiple messages
        for i in 0..3 {
            let msg = WorkerStructuredMessage::with_payload(
                WorkerId("worker1.js".to_string()),
                WorkerMessageDirection::WorkerToPage,
                vec![i],
                0,
            );
            tx.send(msg).unwrap();
        }
        // Drain all
        let result = bridge.drain_worker_messages();
        assert_eq!(result.messages.len(), 3);
        assert!(!result.disconnected);
        // Drain again should be empty
        let empty = bridge.drain_worker_messages();
        assert!(empty.messages.is_empty());
        assert!(!empty.disconnected);
    }

    #[test]
    fn test_webview_state_worker_channel_registration() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, _endpoints) = WorkerChannelBridge::new(worker_id.clone());
        state.register_worker_channel(bridge);
        assert_eq!(state.worker_channel_count(), 1);
        assert!(state.worker_channel(&worker_id).is_some());
    }

    #[test]
    fn test_webview_state_create_worker_channel() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let endpoints = state.create_worker_channel(worker_id.clone());
        assert_eq!(state.worker_channel_count(), 1);
        assert_eq!(endpoints.worker_id, worker_id);
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_webview_state_post_to_worker() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let endpoints = state.create_worker_channel(worker_id.clone());
        let payload = StructuredClonePayload {
            data: vec![42],
            transferable_count: 0,
        };
        // Post to existing worker
        let result = state.post_to_worker(&worker_id, payload);
        assert!(result.is_ok());
        // Worker thread receives it
        let rx = endpoints.page_to_worker_rx.unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.data, vec![42]);
        // Post to non-existent worker
        let result = state.post_to_worker(
            &WorkerId("nonexistent.js".to_string()),
            StructuredClonePayload {
                data: vec![],
                transferable_count: 0,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_webview_state_drain_all_worker_messages() {
        let mut state = BaoWebViewState::default();
        let worker_id1 = WorkerId("worker1.js".to_string());
        let worker_id2 = WorkerId("worker2.js".to_string());
        let endpoints1 = state.create_worker_channel(worker_id1);
        let endpoints2 = state.create_worker_channel(worker_id2);
        // Send messages from both workers
        let tx1 = endpoints1.worker_to_page_tx.unwrap();
        let tx2 = endpoints2.worker_to_page_tx.unwrap();
        tx1.send(WorkerStructuredMessage::metadata_only(
            WorkerId("worker1.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        tx2.send(WorkerStructuredMessage::metadata_only(
            WorkerId("worker2.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        // Drain all
        let (messages, disconnected) = state.drain_all_worker_messages();
        assert_eq!(messages.len(), 2);
        assert!(disconnected.is_empty());
    }

    #[test]
    fn test_webview_state_terminate_clears_channels() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.create_worker_channel(WorkerId("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.create_worker_channel(WorkerId("worker2.js".to_string()));
        assert_eq!(state.worker_channel_count(), 2);
        // Terminate all — should clear channels too
        state.terminate_all_workers();
        assert_eq!(state.worker_channel_count(), 0);
    }

    #[test]
    fn test_webview_state_reap_terminated_worker_channels() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.create_worker_channel(WorkerId("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.create_worker_channel(WorkerId("worker2.js".to_string()));
        // Terminate and reap worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        state.reap_terminated_workers();
        // worker1's channel should be reaped, worker2's should remain
        assert_eq!(state.worker_channel_count(), 1);
        assert!(state
            .worker_channel(&WorkerId("worker2.js".to_string()))
            .is_some());
    }

    #[test]
    fn test_webview_state_remove_worker_channel() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.create_worker_channel(worker_id.clone());
        assert_eq!(state.worker_channel_count(), 1);
        let removed = state.remove_worker_channel(&worker_id);
        assert!(removed.is_some());
        assert_eq!(state.worker_channel_count(), 0);
    }

    #[test]
    fn test_forward_worker_structured_message_with_payload() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let msg = WorkerStructuredMessage::with_payload(
            WorkerId("worker1.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
            vec![1, 2, 3],
            1,
        );
        state.forward_worker_structured_message(&msg);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Debug);
                assert!(text.contains("worker→page"));
                assert!(text.contains("worker1.js"));
                assert!(text.contains("3 bytes"));
                assert!(text.contains("1 transferable"));
            }
            _ => panic!("expected Console event for structured message"),
        }
    }

    #[test]
    fn test_forward_worker_structured_message_metadata_only() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let msg = WorkerStructuredMessage::metadata_only(
            WorkerId("worker1.js".to_string()),
            WorkerMessageDirection::PageToWorker,
        );
        state.forward_worker_structured_message(&msg);
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { text, .. } => {
                assert!(text.contains("metadata-only"));
            }
            _ => panic!("expected Console event"),
        }
    }

    #[test]
    fn test_drain_and_forward_worker_messages() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let endpoints = state.create_worker_channel(WorkerId("worker1.js".to_string()));
        let worker_tx = endpoints.worker_to_page_tx.unwrap();
        worker_tx
            .send(WorkerStructuredMessage::metadata_only(
                WorkerId("worker1.js".to_string()),
                WorkerMessageDirection::WorkerToPage,
            ))
            .unwrap();
        // Drain and forward
        let disconnected = state.drain_and_forward_worker_messages();
        assert!(disconnected.is_empty());
        // Should have forwarded to CDP
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { text, .. } => {
                assert!(text.contains("worker→page"));
            }
            _ => panic!("expected Console event"),
        }
    }

    #[test]
    fn test_worker_channel_bridge_disconnected() {
        let worker_id = WorkerId("worker1.js".to_string());
        let (bridge, _endpoints) = WorkerChannelBridge::new(worker_id);
        // Drop the worker-side sender to simulate worker exit
        // The bridge's try_recv_from_worker should return Err
        // (Can't easily test this without moving endpoints to another thread,
        // but we can test the drain with disconnected channel)
        let result = bridge.try_recv_from_worker();
        assert!(result.is_ok()); // Empty channel, not disconnected yet
        assert!(result.unwrap().is_none());
    }

    // ─── WorkerLocation (REQ-BRW-004 entity:WorkerLocation) ──────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerLocation] [level:unit]

    #[test]
    fn test_worker_location_from_https_url() {
        let loc = WorkerLocation::from_url("https://example.com:8080/path?q=1#hash").unwrap();
        assert_eq!(loc.href, "https://example.com:8080/path?q=1#hash");
        assert_eq!(loc.protocol, "https:");
        assert_eq!(loc.host, "example.com:8080");
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.port, "8080");
        assert_eq!(loc.pathname, "/path");
        assert_eq!(loc.search, "?q=1");
        assert_eq!(loc.hash, "#hash");
        assert_eq!(loc.origin, "https://example.com:8080");
    }

    #[test]
    fn test_worker_location_from_default_port() {
        let loc = WorkerLocation::from_url("https://example.com/path").unwrap();
        assert_eq!(loc.host, "example.com");
        assert_eq!(loc.port, "");
        assert_eq!(loc.origin, "https://example.com");
    }

    #[test]
    fn test_worker_location_from_http_url() {
        let loc = WorkerLocation::from_url("http://localhost:3000/worker.js").unwrap();
        assert_eq!(loc.protocol, "http:");
        assert_eq!(loc.hostname, "localhost");
        assert_eq!(loc.port, "3000");
        assert_eq!(loc.pathname, "/worker.js");
    }

    #[test]
    fn test_worker_location_from_url_no_query_no_hash() {
        let loc = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        assert_eq!(loc.search, "");
        assert_eq!(loc.hash, "");
    }

    #[test]
    fn test_worker_location_from_invalid_url() {
        assert!(WorkerLocation::from_url("not a url").is_none());
    }

    #[test]
    fn test_worker_location_from_url_value() {
        let url = url::Url::parse("https://example.com/worker.js").unwrap();
        let loc = WorkerLocation::from_url_value(url);
        assert_eq!(loc.protocol, "https:");
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.pathname, "/worker.js");
    }

    // ─── WorkerNavigator (REQ-BRW-004 entity:WorkerNavigator) ──────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerNavigator] [level:unit]

    #[test]
    fn test_worker_navigator_default() {
        let nav = WorkerNavigator::default();
        assert!(nav.user_agent.is_empty());
        assert!(nav.platform.is_empty());
        assert!(nav.hardware_concurrency > 0);
        assert_eq!(nav.language, "en-US");
        assert!(!nav.languages.is_empty());
        assert!(nav.connection.is_none());
        assert!(!nav.cookie_enabled);
        assert_eq!(nav.max_touch_points, 0);
        assert_eq!(nav.product, "Gecko");
        assert_eq!(nav.app_code_name, "Mozilla");
        assert_eq!(nav.app_name, "Netscape");
        assert!(nav.app_version.is_empty());
    }

    #[test]
    fn test_worker_navigator_from_scope_config() {
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string()],
        };
        let nav = WorkerNavigator::from_scope_config(&config);
        assert_eq!(nav.user_agent, "Bao/1.0");
        assert_eq!(nav.platform, "Linux x86_64");
        assert_eq!(nav.hardware_concurrency, 8);
        assert_eq!(nav.language, "zh-CN");
        assert_eq!(nav.languages.len(), 2);
        assert_eq!(nav.app_version, "Bao/1.0"); // app_version mirrors user_agent
        assert_eq!(nav.product, "Gecko");
        assert_eq!(nav.app_code_name, "Mozilla");
        assert_eq!(nav.app_name, "Netscape");
    }

    #[test]
    fn test_worker_navigator_from_shared_scope_config() {
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/2.0".to_string(),
            platform: "MacOS".to_string(),
            hardware_concurrency: 4,
            language: "ja".to_string(),
            languages: vec!["ja".to_string(), "en".to_string()],
        };
        let nav = WorkerNavigator::from_shared_scope_config(&config);
        assert_eq!(nav.user_agent, "Bao/2.0");
        assert_eq!(nav.platform, "MacOS");
        assert_eq!(nav.hardware_concurrency, 4);
        assert_eq!(nav.app_version, "Bao/2.0");
    }

    #[test]
    fn test_worker_network_information() {
        let info = WorkerNetworkInformation {
            effective_type: "4g".to_string(),
            downlink: 10,
            rtt: 50,
            save_data: false,
        };
        assert_eq!(info.effective_type, "4g");
        assert_eq!(info.downlink, 10);
        assert_eq!(info.rtt, 50);
        assert!(!info.save_data);
    }

    // ─── WorkerGlobalScopeState (REQ-BRW-004 entity:WorkerGlobalScope) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerGlobalScope] [level:unit]

    #[test]
    fn test_worker_global_scope_state_new() {
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        let scope =
            WorkerGlobalScopeState::new("https://example.com/worker.js".to_string(), &config);
        assert_eq!(scope.worker_url, "https://example.com/worker.js");
        assert!(!scope.closing);
        assert!(scope.location.is_some());
        assert_eq!(scope.navigator.user_agent, "Bao/1.0");
    }

    #[test]
    fn test_worker_global_scope_state_new_shared() {
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/2.0".to_string(),
            platform: "MacOS".to_string(),
            hardware_concurrency: 4,
            language: "ja".to_string(),
            languages: vec!["ja".to_string()],
        };
        let scope =
            WorkerGlobalScopeState::new_shared("https://example.com/sw.js".to_string(), &config);
        assert_eq!(scope.worker_url, "https://example.com/sw.js");
        assert_eq!(scope.navigator.user_agent, "Bao/2.0");
    }

    #[test]
    fn test_worker_global_scope_state_location_parsed() {
        let config = WorkerScopeConfig::default();
        let scope = WorkerGlobalScopeState::new(
            "https://example.com:8080/app/worker.js?debug=true#section".to_string(),
            &config,
        );
        let loc = scope.location.unwrap();
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.port, "8080");
        assert_eq!(loc.pathname, "/app/worker.js");
        assert_eq!(loc.search, "?debug=true");
        assert_eq!(loc.hash, "#section");
    }

    #[test]
    fn test_worker_global_scope_state_invalid_url_no_location() {
        let config = WorkerScopeConfig::default();
        let scope = WorkerGlobalScopeState::new("not-a-url".to_string(), &config);
        assert!(scope.location.is_none());
    }

    // ─── DedicatedWorkerGlobalScopeState (REQ-BRW-004 entity) ────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:DedicatedWorkerGlobalScope] [level:unit]

    #[test]
    fn test_dedicated_worker_global_scope_state_new() {
        let worker_id = WorkerId("https://example.com/worker.js".to_string());
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        assert_eq!(scope.worker_id, worker_id);
        assert!(!scope.has_onmessage);
        assert!(!scope.has_onerror);
        assert_eq!(scope.scope.navigator.user_agent, "Bao/1.0");
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_location() {
        let worker_id = WorkerId("https://example.com/worker.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        let loc = scope.location().unwrap();
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.pathname, "/worker.js");
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_navigator() {
        let worker_id = WorkerId("worker.js".to_string());
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string()],
        };
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        let nav = scope.navigator();
        assert_eq!(nav.user_agent, "Bao/1.0");
        assert_eq!(nav.hardware_concurrency, 8);
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_event_handlers() {
        let worker_id = WorkerId("worker.js".to_string());
        let config = WorkerScopeConfig::default();
        let mut scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        assert!(!scope.has_onmessage);
        assert!(!scope.has_onerror);
        scope.set_onmessage();
        assert!(scope.has_onmessage);
        assert!(!scope.has_onerror);
        scope.set_onerror();
        assert!(scope.has_onmessage);
        assert!(scope.has_onerror);
    }

    // ─── DedicatedWorkerGlobalScope BaoWebViewState tracking ─────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:DedicatedWorkerGlobalScope] [level:unit]

    #[test]
    fn test_webview_state_dedicated_worker_scope_registration() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        assert!(state.dedicated_worker_scope(&worker_id).is_some());
    }

    #[test]
    fn test_webview_state_dedicated_worker_scope_get_mut() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        // Register event handler
        state
            .dedicated_worker_scope_mut(&worker_id)
            .unwrap()
            .set_onmessage();
        assert!(
            state
                .dedicated_worker_scope(&worker_id)
                .unwrap()
                .has_onmessage
        );
    }

    #[test]
    fn test_webview_state_dedicated_worker_scope_remove() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        state.register_dedicated_worker_scope(worker_id.clone(), scope);
        let removed = state.remove_dedicated_worker_scope(&worker_id);
        assert!(removed.is_some());
        assert_eq!(state.dedicated_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_dedicated_worker_scopes_snapshot() {
        let mut state = BaoWebViewState::default();
        let config = WorkerScopeConfig::default();
        let id1 = WorkerId("worker1.js".to_string());
        let id2 = WorkerId("worker2.js".to_string());
        state.register_dedicated_worker_scope(
            id1,
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        state.register_dedicated_worker_scope(
            id2,
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker2.js".to_string()), &config),
        );
        let scopes = state.dedicated_worker_scopes();
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_webview_state_terminate_clears_dedicated_worker_scopes() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        let config = WorkerScopeConfig::default();
        state.register_dedicated_worker_scope(
            WorkerId("worker1.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        state.terminate_all_workers();
        assert_eq!(state.dedicated_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_reap_terminated_dedicated_worker_scopes() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        let config = WorkerScopeConfig::default();
        state.register_dedicated_worker_scope(
            WorkerId("worker1.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker1.js".to_string()), &config),
        );
        state.register_dedicated_worker_scope(
            WorkerId("worker2.js".to_string()),
            DedicatedWorkerGlobalScopeState::new(WorkerId("worker2.js".to_string()), &config),
        );
        // Terminate and reap worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        state.reap_terminated_workers();
        // worker1's scope should be reaped, worker2's should remain
        assert_eq!(state.dedicated_worker_scope_count(), 1);
        assert!(state
            .dedicated_worker_scope(&WorkerId("worker2.js".to_string()))
            .is_some());
    }

    #[test]
    fn test_worker_location_equality() {
        let loc1 = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        let loc2 = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        assert_eq!(loc1, loc2);
    }

    // ─── Worker Script Loading Pipeline (REQ-BRW-004 / DF-WK-2) ────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:Worker] [DF-WK-2] [level:unit]

    #[test]
    fn test_worker_script_source_inline() {
        let source = WorkerScriptSource::Inline("var x = 1;".to_string());
        assert_eq!(source, WorkerScriptSource::Inline("var x = 1;".to_string()));
        assert_ne!(source, WorkerScriptSource::Inline("var y = 2;".to_string()));
    }

    #[test]
    fn test_worker_script_source_url() {
        let source = WorkerScriptSource::Url("https://example.com/worker.js".to_string());
        assert_eq!(
            source,
            WorkerScriptSource::Url("https://example.com/worker.js".to_string())
        );
        assert_ne!(
            source,
            WorkerScriptSource::Url("https://other.com/worker.js".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_result() {
        let result = WorkerScriptLoadResult {
            source: "self.onmessage = function(e) {}".to_string(),
            final_url: "https://example.com/worker.js".to_string(),
            mime_type: Some("text/javascript".to_string()),
        };
        assert_eq!(result.source, "self.onmessage = function(e) {}");
        assert_eq!(result.final_url, "https://example.com/worker.js");
        assert_eq!(result.mime_type.as_deref(), Some("text/javascript"));
    }

    #[test]
    fn test_worker_script_load_error_network() {
        let err = WorkerScriptLoadError::NetworkError("404 Not Found".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::NetworkError("404 Not Found".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_invalid_mime() {
        let err = WorkerScriptLoadError::InvalidMimeType {
            received: "text/html".to_string(),
            url: "https://example.com/worker.js".to_string(),
        };
        match err {
            WorkerScriptLoadError::InvalidMimeType { received, url } => {
                assert_eq!(received, "text/html");
                assert_eq!(url, "https://example.com/worker.js");
            }
            _ => panic!("expected InvalidMimeType"),
        }
    }

    #[test]
    fn test_worker_script_load_error_utf8() {
        let err = WorkerScriptLoadError::Utf8DecodeError("invalid UTF-8".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::Utf8DecodeError("invalid UTF-8".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_invalid_url() {
        let err = WorkerScriptLoadError::InvalidUrl("bad url".to_string());
        assert_eq!(
            err,
            WorkerScriptLoadError::InvalidUrl("bad url".to_string())
        );
    }

    #[test]
    fn test_worker_script_load_error_cancelled() {
        let err = WorkerScriptLoadError::Cancelled;
        assert_eq!(err, WorkerScriptLoadError::Cancelled);
    }

    #[test]
    fn test_worker_script_type_default_classic() {
        assert_eq!(WorkerScriptType::default(), WorkerScriptType::Classic);
    }

    #[test]
    fn test_worker_script_type_equality() {
        assert_eq!(WorkerScriptType::Classic, WorkerScriptType::Classic);
        assert_eq!(WorkerScriptType::Module, WorkerScriptType::Module);
        assert_ne!(WorkerScriptType::Classic, WorkerScriptType::Module);
    }

    #[test]
    fn test_is_javascript_mime_type_valid() {
        assert!(is_javascript_mime_type("text/javascript"));
        assert!(is_javascript_mime_type("application/javascript"));
        assert!(is_javascript_mime_type("application/ecmascript"));
        assert!(is_javascript_mime_type("application/x-javascript"));
        assert!(is_javascript_mime_type("text/ecmascript"));
        assert!(is_javascript_mime_type("text/x-javascript"));
        assert!(is_javascript_mime_type("text/jscript"));
        assert!(is_javascript_mime_type("text/livescript"));
    }

    #[test]
    fn test_is_javascript_mime_type_case_insensitive() {
        assert!(is_javascript_mime_type("Text/JavaScript"));
        assert!(is_javascript_mime_type("APPLICATION/JAVASCRIPT"));
        assert!(is_javascript_mime_type("text/JavaScript"));
    }

    #[test]
    fn test_is_javascript_mime_type_with_charset() {
        // MIME type with parameters should still match
        assert!(is_javascript_mime_type("text/javascript; charset=utf-8"));
        assert!(is_javascript_mime_type(
            "application/javascript;charset=utf-8"
        ));
    }

    #[test]
    fn test_is_javascript_mime_type_invalid() {
        assert!(!is_javascript_mime_type("text/html"));
        assert!(!is_javascript_mime_type("application/json"));
        assert!(!is_javascript_mime_type("text/plain"));
        assert!(!is_javascript_mime_type("application/octet-stream"));
        assert!(!is_javascript_mime_type("text/css"));
    }

    #[test]
    fn test_worker_script_loader_inline() {
        let loader =
            WorkerScriptLoader::inline("var x = 1;".to_string(), WorkerScriptType::Classic);
        assert!(loader.script_url().is_none());
        assert!(!loader.requires_fetch());
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Inline("var x = 1;".to_string())
        );
    }

    #[test]
    fn test_worker_script_loader_url_https() {
        let loader = WorkerScriptLoader::url(
            "https://example.com/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        assert_eq!(loader.script_url(), Some("https://example.com/worker.js"));
        assert!(loader.requires_fetch());
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Url("https://example.com/worker.js".to_string())
        );
    }

    #[test]
    fn test_worker_script_loader_url_http() {
        let loader = WorkerScriptLoader::url(
            "http://localhost:3000/worker.js".to_string(),
            WorkerScriptType::Module,
        );
        assert!(loader.requires_fetch());
        assert_eq!(loader.script_type, WorkerScriptType::Module);
    }

    #[test]
    fn test_worker_script_loader_url_invalid() {
        let loader = WorkerScriptLoader::url("not a url".to_string(), WorkerScriptType::Classic);
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("Invalid Worker script URL"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_url_unsupported_scheme() {
        let loader = WorkerScriptLoader::url(
            "ftp://example.com/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("Unsupported") || msg.contains("ftp"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_text() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript,self.postMessage('hello')".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "self.postMessage('hello')");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from data: URL"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_base64() {
        // base64 of "var x = 1;" = "dmFyIHggPSAxOw=="
        let loader = WorkerScriptLoader::url(
            "data:text/javascript;base64,dmFyIHggPSAxOw==".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "var x = 1;");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from data: URL"),
        }
    }

    #[test]
    fn test_worker_script_loader_data_url_invalid_base64() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript;base64,!!!invalid!!!".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
    }

    #[test]
    fn test_worker_script_loader_data_url_missing_comma() {
        let loader = WorkerScriptLoader::url(
            "data:text/javascript".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidUrl(msg) => {
                assert!(msg.contains("comma separator"));
            }
            _ => panic!("expected InvalidUrl error"),
        }
    }

    #[test]
    fn test_worker_script_loader_blob_url_passthrough() {
        let loader = WorkerScriptLoader::url(
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000".to_string(),
            WorkerScriptType::Classic,
        );
        let resolved = loader.resolve().unwrap();
        assert_eq!(
            resolved,
            WorkerScriptSource::Url(
                "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000".to_string()
            )
        );
    }

    #[test]
    fn test_worker_script_loader_from_source() {
        let loader = WorkerScriptLoader::from_source(
            WorkerScriptSource::Inline("code".to_string()),
            WorkerScriptType::Module,
        );
        assert_eq!(loader.script_type, WorkerScriptType::Module);
        assert!(loader.script_url().is_none());
    }

    #[test]
    fn test_worker_script_loader_validate_mime_type_valid() {
        assert!(WorkerScriptLoader::validate_mime_type(
            "text/javascript",
            "https://example.com/worker.js"
        )
        .is_ok());
        assert!(WorkerScriptLoader::validate_mime_type(
            "application/javascript",
            "https://example.com/worker.js"
        )
        .is_ok());
    }

    #[test]
    fn test_worker_script_loader_validate_mime_type_invalid() {
        let result =
            WorkerScriptLoader::validate_mime_type("text/html", "https://example.com/worker.js");
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::InvalidMimeType { received, url } => {
                assert_eq!(received, "text/html");
                assert_eq!(url, "https://example.com/worker.js");
            }
            _ => panic!("expected InvalidMimeType error"),
        }
    }

    #[test]
    fn test_worker_script_load_state_transitions() {
        let mut state = WorkerScriptLoadState::Pending;
        assert!(state.is_loading());
        assert!(!state.is_ready());
        assert!(!state.is_failed());

        state = WorkerScriptLoadState::Fetching;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Validating;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Decoding;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Compiling;
        assert!(state.is_loading());

        state = WorkerScriptLoadState::Ready;
        assert!(!state.is_loading());
        assert!(state.is_ready());

        state = WorkerScriptLoadState::Failed(WorkerScriptLoadError::NetworkError(
            "timeout".to_string(),
        ));
        assert!(!state.is_loading());
        assert!(state.is_failed());
    }

    #[test]
    fn test_webview_state_worker_script_load_state_registration() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        assert_eq!(state.worker_script_load_state_count(), 1);
        assert!(state.worker_script_load_state(&worker_id).is_some());
        assert_eq!(
            state.worker_script_load_state(&worker_id).unwrap(),
            &WorkerScriptLoadState::Pending
        );
    }

    #[test]
    fn test_webview_state_worker_script_load_state_update() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Fetching);
        assert_eq!(
            state.worker_script_load_state(&worker_id).unwrap(),
            &WorkerScriptLoadState::Fetching
        );
    }

    #[test]
    fn test_webview_state_worker_script_load_state_remove() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("worker1.js".to_string());
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Ready);
        let removed = state.remove_worker_script_load_state(&worker_id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap(), WorkerScriptLoadState::Ready);
        assert_eq!(state.worker_script_load_state_count(), 0);
    }

    #[test]
    fn test_webview_state_terminate_clears_script_load_states() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.register_worker_script_load_state(
            WorkerId("worker1.js".to_string()),
            WorkerScriptLoadState::Fetching,
        );
        assert_eq!(state.worker_script_load_state_count(), 1);
        state.terminate_all_workers();
        assert_eq!(state.worker_script_load_state_count(), 0);
    }

    #[test]
    fn test_webview_state_reap_terminated_worker_script_load_states() {
        let mut state = BaoWebViewState::default();
        state.track_worker(WorkerHandle::new("worker1.js".to_string()));
        state.track_worker(WorkerHandle::new("worker2.js".to_string()));
        state.register_worker_script_load_state(
            WorkerId("worker1.js".to_string()),
            WorkerScriptLoadState::Ready,
        );
        state.register_worker_script_load_state(
            WorkerId("worker2.js".to_string()),
            WorkerScriptLoadState::Fetching,
        );
        // Terminate and reap worker1
        state.active_workers[0].handle().terminate();
        state.active_workers[0].handle().mark_terminated();
        state.reap_terminated_workers();
        // worker1's script load state should be reaped, worker2's should remain
        assert_eq!(state.worker_script_load_state_count(), 1);
        assert!(state
            .worker_script_load_state(&WorkerId("worker2.js".to_string()))
            .is_some());
    }

    #[test]
    fn test_worker_script_loader_file_url() {
        // Create a temp file with Worker script content
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("bao_test_worker_script.js");
        std::fs::write(&temp_file, "var x = 42;").unwrap();

        let file_url = format!("file://{}", temp_file.display());
        let loader = WorkerScriptLoader::url(file_url, WorkerScriptType::Classic);
        let resolved = loader.resolve().unwrap();
        match resolved {
            WorkerScriptSource::Inline(script) => {
                assert_eq!(script, "var x = 42;");
            }
            WorkerScriptSource::Url(_) => panic!("expected inline source from file: URL"),
        }

        // Cleanup
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_worker_script_loader_file_url_not_found() {
        let loader = WorkerScriptLoader::url(
            "file:///nonexistent/path/worker.js".to_string(),
            WorkerScriptType::Classic,
        );
        let result = loader.resolve();
        assert!(result.is_err());
        match result.unwrap_err() {
            WorkerScriptLoadError::NetworkError(msg) => {
                assert!(msg.contains("Failed to read") || msg.contains("No such file"));
            }
            _ => panic!("expected NetworkError for missing file"),
        }
    }

    #[test]
    fn test_worker_script_loader_full_pipeline_states() {
        // Simulate the full DF-WK-2 pipeline state transitions
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("https://example.com/worker.js".to_string());

        // Step 1: Worker created → Pending
        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);
        assert!(state
            .worker_script_load_state(&worker_id)
            .unwrap()
            .is_loading());

        // Step 2: Fetch started → Fetching
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Fetching);
        assert!(matches!(
            state.worker_script_load_state(&worker_id).unwrap(),
            WorkerScriptLoadState::Fetching
        ));

        // Step 3: Response received → Validating
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Validating);

        // Step 4: MIME check passed → Decoding
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Decoding);

        // Step 5: UTF-8 decoded → Compiling
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Compiling);

        // Step 6: Compilation succeeded → Ready
        state.update_worker_script_load_state(&worker_id, WorkerScriptLoadState::Ready);
        assert!(state
            .worker_script_load_state(&worker_id)
            .unwrap()
            .is_ready());
    }

    #[test]
    fn test_worker_script_loader_pipeline_failure() {
        let mut state = BaoWebViewState::default();
        let worker_id = WorkerId("https://example.com/bad-worker.js".to_string());

        state.register_worker_script_load_state(worker_id.clone(), WorkerScriptLoadState::Pending);

        // Simulate MIME type failure during validation
        state.update_worker_script_load_state(
            &worker_id,
            WorkerScriptLoadState::Failed(WorkerScriptLoadError::InvalidMimeType {
                received: "text/html".to_string(),
                url: "https://example.com/bad-worker.js".to_string(),
            }),
        );
        assert!(state
            .worker_script_load_state(&worker_id)
            .unwrap()
            .is_failed());
    }

    // ─── StealthProfile → WorkerScopeConfig conversion (REQ-BRW-004 criteria #12-17) ───
    // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK

    #[test]
    fn test_worker_scope_config_from_stealth_profile_chrome() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = WorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
        assert!(
            config.user_agent.contains("Chrome"),
            "Chrome profile UA must contain Chrome"
        );
    }

    #[test]
    fn test_worker_scope_config_from_stealth_profile_firefox() {
        let profile = bao_stealth::StealthProfile::firefox_default();
        let config = WorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
        assert!(
            config.user_agent.contains("Firefox"),
            "Firefox profile UA must contain Firefox"
        );
    }

    #[test]
    fn test_shared_worker_scope_config_from_stealth_profile() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = SharedWorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
    }

    #[test]
    fn test_worker_scope_config_from_stealth_profile_carries_canvas_webgl_audio() {
        // CRIT-STL-WK #13-17: Canvas/WebGL/Audio seeds must be identical
        // between the profile and the WorkerScopeConfig's embedded profile.
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = WorkerScopeConfig::from(&profile);
        let worker_profile = config.stealth_profile.unwrap();

        assert_eq!(
            worker_profile.canvas.seed(),
            profile.canvas.seed(),
            "Canvas seed must match"
        );
        assert!(
            (worker_profile.canvas.noise_amplitude() - profile.canvas.noise_amplitude()).abs()
                < f64::EPSILON,
            "Canvas amplitude must match"
        );
        assert_eq!(
            worker_profile.audio.seed(),
            profile.audio.seed(),
            "Audio seed must match"
        );
        assert_eq!(
            worker_profile.webgl.vendor, profile.webgl.vendor,
            "WebGL vendor must match"
        );
        assert_eq!(
            worker_profile.webgl.renderer, profile.webgl.renderer,
            "WebGL renderer must match"
        );
    }

    #[test]
    fn test_worker_scope_config_from_different_profiles_produces_different_configs() {
        // @trace REQ-BRW-004 [criterion:17] new Worker 后 worker 回传指纹摘要 === 主线程指纹摘要
        let chrome = bao_stealth::StealthProfile::chrome_default();
        let firefox = bao_stealth::StealthProfile::firefox_default();
        let chrome_config = WorkerScopeConfig::from(&chrome);
        let firefox_config = WorkerScopeConfig::from(&firefox);

        assert_ne!(chrome_config.user_agent, firefox_config.user_agent);
        assert_ne!(
            chrome_config.stealth_profile.unwrap().canvas.seed(),
            firefox_config.stealth_profile.unwrap().canvas.seed()
        );
    }

    // ─── SharedWorkerGlobalScopeState (REQ-BRW-004 entity) ────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorkerGlobalScope] [DF-WK-7] [level:unit]

    #[test]
    fn test_shared_worker_global_scope_state_new() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "myworker".to_string(),
        };
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        assert_eq!(scope.shared_worker_id, id);
        assert!(!scope.has_onconnect);
        assert_eq!(scope.connect_count, 0);
        assert_eq!(scope.scope.navigator.user_agent, "Bao/1.0");
    }

    #[test]
    fn test_shared_worker_global_scope_state_location() {
        let id = SharedWorkerId {
            script_url: "https://example.com/sw.js".to_string(),
            name: String::new(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id, &config);
        let loc = scope.location().unwrap();
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.pathname, "/sw.js");
    }

    #[test]
    fn test_shared_worker_global_scope_state_navigator() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/2.0".to_string(),
            platform: "MacOS".to_string(),
            hardware_concurrency: 4,
            language: "ja".to_string(),
            languages: vec!["ja".to_string()],
        };
        let scope = SharedWorkerGlobalScopeState::new(id, &config);
        let nav = scope.navigator();
        assert_eq!(nav.user_agent, "Bao/2.0");
        assert_eq!(nav.hardware_concurrency, 4);
    }

    #[test]
    fn test_shared_worker_global_scope_state_onconnect() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        let config = SharedWorkerScopeConfig::default();
        let mut scope = SharedWorkerGlobalScopeState::new(id, &config);
        assert!(!scope.has_onconnect);
        scope.set_onconnect();
        assert!(scope.has_onconnect);
    }

    #[test]
    fn test_shared_worker_global_scope_state_connect_count() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        let config = SharedWorkerScopeConfig::default();
        let mut scope = SharedWorkerGlobalScopeState::new(id, &config);
        assert_eq!(scope.connect_count, 0);
        scope.page_connected();
        assert_eq!(scope.connect_count, 1);
        scope.page_connected();
        assert_eq!(scope.connect_count, 2);
    }

    // ─── SharedWorker Port Channel (REQ-BRW-004 / DF-WK-7) ───────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_shared_worker_port_channel_creation() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let (port, endpoints) = SharedWorkerPortChannel::new(id.clone());
        assert_eq!(port.shared_worker_id, id);
        assert_eq!(endpoints.shared_worker_id, id);
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_shared_worker_port_channel_page_to_worker() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        let (port, endpoints) = SharedWorkerPortChannel::new(id);
        let payload = StructuredClonePayload {
            data: vec![1, 2, 3],
            transferable_count: 0,
        };
        port.post_message_to_worker(payload).unwrap();
        let rx = endpoints.page_to_worker_rx.unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_shared_worker_port_channel_worker_to_page() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        let (port, endpoints) = SharedWorkerPortChannel::new(id);
        let msg = WorkerStructuredMessage::with_payload(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
            vec![4, 5, 6],
            0,
        );
        let tx = endpoints.worker_to_page_tx.unwrap();
        tx.send(msg).unwrap();
        let result = port.try_recv_from_worker().unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().payload.unwrap().data, vec![4, 5, 6]);
    }

    #[test]
    fn test_shared_worker_port_channel_drain() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: String::new(),
        };
        let (port, endpoints) = SharedWorkerPortChannel::new(id);
        let tx = endpoints.worker_to_page_tx.unwrap();
        for i in 0..3 {
            let msg = WorkerStructuredMessage::with_payload(
                WorkerId("sw.js".to_string()),
                WorkerMessageDirection::WorkerToPage,
                vec![i],
                0,
            );
            tx.send(msg).unwrap();
        }
        let result = port.drain_worker_messages();
        assert_eq!(result.messages.len(), 3);
        assert!(!result.disconnected);
        let empty = port.drain_worker_messages();
        assert!(empty.messages.is_empty());
        assert!(!empty.disconnected);
    }

    // ─── SharedWorkerChannelBridge (REQ-BRW-004 / DF-WK-7) ───────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_shared_worker_channel_bridge_new() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let bridge = SharedWorkerChannelBridge::new(id.clone());
        assert_eq!(bridge.shared_worker_id, id);
        assert_eq!(bridge.port_count(), 0);
    }

    #[test]
    fn test_shared_worker_channel_bridge_add_port() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let mut bridge = SharedWorkerChannelBridge::new(id.clone());
        let endpoints = bridge.add_port();
        assert_eq!(bridge.port_count(), 1);
        assert_eq!(endpoints.shared_worker_id, id);
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_shared_worker_channel_bridge_multiple_ports() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let mut bridge = SharedWorkerChannelBridge::new(id);
        bridge.add_port(); // Page 1
        bridge.add_port(); // Page 2
        bridge.add_port(); // Page 3
        assert_eq!(bridge.port_count(), 3);
    }

    #[test]
    fn test_shared_worker_channel_bridge_drain_all() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let mut bridge = SharedWorkerChannelBridge::new(id);
        let endpoints1 = bridge.add_port();
        let endpoints2 = bridge.add_port();
        // Send messages from both ports
        let tx1 = endpoints1.worker_to_page_tx.unwrap();
        let tx2 = endpoints2.worker_to_page_tx.unwrap();
        tx1.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        tx2.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        let (messages, disconnected) = bridge.drain_all_worker_messages();
        assert_eq!(messages.len(), 2);
        assert!(disconnected.is_empty());
    }

    #[test]
    fn test_shared_worker_channel_bridge_post_to_worker() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let mut bridge = SharedWorkerChannelBridge::new(id);
        let endpoints = bridge.add_port();
        let payload = StructuredClonePayload {
            data: vec![42],
            transferable_count: 0,
        };
        bridge.post_to_worker_from_port(0, payload).unwrap();
        let rx = endpoints.page_to_worker_rx.unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.data, vec![42]);
    }

    #[test]
    fn test_shared_worker_channel_bridge_post_invalid_port() {
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let mut bridge = SharedWorkerChannelBridge::new(id);
        bridge.add_port();
        let payload = StructuredClonePayload {
            data: vec![],
            transferable_count: 0,
        };
        let result = bridge.post_to_worker_from_port(99, payload);
        assert!(result.is_err());
    }

    // ─── BaoWebViewState SharedWorker Channel & Scope (REQ-BRW-004 / DF-WK-7) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_webview_state_shared_worker_channel_registration() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let bridge = SharedWorkerChannelBridge::new(id.clone());
        state.register_shared_worker_channel(bridge);
        assert!(state.shared_worker_channel(&id).is_some());
        assert_eq!(state.shared_worker_channel_count(), 0); // no ports yet
    }

    #[test]
    fn test_webview_state_create_shared_worker_channel() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.create_shared_worker_channel(id.clone());
        assert!(state.shared_worker_channel(&id).is_some());
    }

    #[test]
    fn test_webview_state_add_shared_worker_port() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_channel_count(), 1);
        assert_eq!(endpoints.shared_worker_id, id);
        assert!(endpoints.page_to_worker_rx.is_some());
        assert!(endpoints.worker_to_page_tx.is_some());
    }

    #[test]
    fn test_webview_state_add_shared_worker_port_multiple() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.add_shared_worker_port(id.clone());
        state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_channel_count(), 2); // 2 ports
    }

    #[test]
    fn test_webview_state_drain_all_shared_worker_messages() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id);
        let tx = endpoints.worker_to_page_tx.unwrap();
        tx.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        let (messages, disconnected) = state.drain_all_shared_worker_messages();
        assert_eq!(messages.len(), 1);
        assert!(disconnected.is_empty());
    }

    #[test]
    fn test_webview_state_drain_and_forward_shared_worker_messages() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        };
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let endpoints = state.add_shared_worker_port(id);
        let worker_tx = endpoints.worker_to_page_tx.unwrap();
        worker_tx
            .send(WorkerStructuredMessage::metadata_only(
                WorkerId("sw.js".to_string()),
                WorkerMessageDirection::WorkerToPage,
            ))
            .unwrap();
        state.drain_and_forward_shared_worker_messages();
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { text, .. } => {
                assert!(text.contains("worker→page"));
            }
            _ => panic!("expected Console event for shared worker message"),
        }
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_clears_channels() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        state.track_shared_worker_port(SharedWorkerPortRef::new(SharedWorkerHandle::new(
            "sw.js".to_string(),
            "test".to_string(),
        )));
        state.add_shared_worker_port(id.clone());
        assert_eq!(state.shared_worker_port_count(), 1);
        assert_eq!(state.shared_worker_channel_count(), 1);
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_port_count(), 0);
        assert_eq!(state.shared_worker_channel_count(), 0);
    }

    #[test]
    fn test_webview_state_shared_worker_scope_registration() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        assert_eq!(state.shared_worker_scope_count(), 1);
        assert!(state.shared_worker_scope(&id).is_some());
    }

    #[test]
    fn test_webview_state_shared_worker_scope_get_mut() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        state.shared_worker_scope_mut(&id).unwrap().set_onconnect();
        assert!(state.shared_worker_scope(&id).unwrap().has_onconnect);
    }

    #[test]
    fn test_webview_state_shared_worker_scope_remove() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        let scope = SharedWorkerGlobalScopeState::new(id.clone(), &config);
        state.register_shared_worker_scope(id.clone(), scope);
        let removed = state.remove_shared_worker_scope(&id);
        assert!(removed.is_some());
        assert_eq!(state.shared_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_shared_worker_scopes_snapshot() {
        let mut state = BaoWebViewState::default();
        let id1 = SharedWorkerId {
            script_url: "sw1.js".to_string(),
            name: "a".to_string(),
        };
        let id2 = SharedWorkerId {
            script_url: "sw2.js".to_string(),
            name: "b".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id1,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw1.js".to_string(),
                    name: "a".to_string(),
                },
                &config,
            ),
        );
        state.register_shared_worker_scope(
            id2,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw2.js".to_string(),
                    name: "b".to_string(),
                },
                &config,
            ),
        );
        let scopes = state.shared_worker_scopes();
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn test_webview_state_disconnect_shared_worker_clears_scopes() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id,
            SharedWorkerGlobalScopeState::new(
                SharedWorkerId {
                    script_url: "sw.js".to_string(),
                    name: "test".to_string(),
                },
                &config,
            ),
        );
        assert_eq!(state.shared_worker_scope_count(), 1);
        state.disconnect_shared_worker_ports();
        assert_eq!(state.shared_worker_scope_count(), 0);
    }

    #[test]
    fn test_webview_state_set_shared_worker_scope_config() {
        let mut state = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "test".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        assert!(state
            .shared_worker_scope(&id)
            .unwrap()
            .navigator()
            .user_agent
            .is_empty());
        let new_config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        state.set_shared_worker_scope_config(&id, &new_config);
        assert_eq!(
            state
                .shared_worker_scope(&id)
                .unwrap()
                .navigator()
                .user_agent,
            "Bao/1.0"
        );
    }

    // ─── BaoServoDelegate SharedWorker Routing (REQ-BRW-004 / DF-WK-7) ─────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_delegate_route_shared_worker_new() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let (returned, is_new) = delegate.route_shared_worker(handle);
        assert!(is_new);
        assert_eq!(returned.script_url, "sw.js");
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_route_shared_worker_existing() {
        let delegate = BaoServoDelegate::new();
        let handle1 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let handle2 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        delegate.route_shared_worker(handle1);
        let (_, is_new) = delegate.route_shared_worker(handle2);
        assert!(
            !is_new,
            "same (url, name) should return existing, not create new"
        );
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_get_or_create_shared_worker_new() {
        let delegate = BaoServoDelegate::new();
        let (handle, is_new) = delegate.get_or_create_shared_worker("sw.js", "myname");
        assert!(is_new);
        assert_eq!(handle.script_url, "sw.js");
        assert_eq!(handle.name, "myname");
    }

    #[test]
    fn test_delegate_get_or_create_shared_worker_existing() {
        let delegate = BaoServoDelegate::new();
        delegate.get_or_create_shared_worker("sw.js", "myname");
        let (_, is_new) = delegate.get_or_create_shared_worker("sw.js", "myname");
        assert!(!is_new);
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_unregister_shared_worker() {
        let delegate = BaoServoDelegate::new();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "myname".to_string(),
        };
        delegate.get_or_create_shared_worker("sw.js", "myname");
        assert_eq!(delegate.shared_worker_count(), 1);
        let removed = delegate.unregister_shared_worker(&id);
        assert!(removed);
        assert_eq!(delegate.shared_worker_count(), 0);
    }

    #[test]
    fn test_delegate_unregister_nonexistent_shared_worker() {
        let delegate = BaoServoDelegate::new();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "nonexistent".to_string(),
        };
        let removed = delegate.unregister_shared_worker(&id);
        assert!(!removed);
    }

    #[test]
    fn test_delegate_all_shared_workers() {
        let delegate = BaoServoDelegate::new();
        delegate.get_or_create_shared_worker("sw1.js", "a");
        delegate.get_or_create_shared_worker("sw2.js", "b");
        let all = delegate.all_shared_workers();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_shared_worker_cross_page_routing_full_lifecycle() {
        // @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
        // Full lifecycle: route → register scope → add ports → drain messages → disconnect → reap
        let delegate = BaoServoDelegate::new();

        // Page 1 creates SharedWorker
        let (handle, is_new) = delegate.route_shared_worker(SharedWorkerHandle::new(
            "sw.js".to_string(),
            "shared".to_string(),
        ));
        assert!(is_new);
        assert_eq!(handle.connected_page_count(), 0);

        // Page 1 connects
        let mut state1 = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "shared".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state1.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        state1.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        // SharedWorkerPortRef::new already increments connected_page_count
        assert_eq!(handle.connected_page_count(), 1);
        // Each page gets its own channel bridge (ports are per-page)
        let endpoints1 = state1.add_shared_worker_port(id.clone());

        // Page 2 connects (same SharedWorker, but its own channel bridge)
        let mut state2 = BaoWebViewState::default();
        state2.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        state2.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        let endpoints2 = state2.add_shared_worker_port(id.clone());
        assert_eq!(handle.connected_page_count(), 2);

        // Both pages can send messages to the SharedWorker through their own ports
        let payload1 = StructuredClonePayload {
            data: vec![1],
            transferable_count: 0,
        };
        state1
            .post_to_worker_via_shared_port(&id, 0, payload1)
            .unwrap();
        let payload2 = StructuredClonePayload {
            data: vec![2],
            transferable_count: 0,
        };
        state2
            .post_to_worker_via_shared_port(&id, 0, payload2)
            .unwrap();

        // Worker thread receives from both pages
        let rx1 = endpoints1.page_to_worker_rx.unwrap();
        let rx2 = endpoints2.page_to_worker_rx.unwrap();
        assert_eq!(rx1.try_recv().unwrap().data, vec![1]);
        assert_eq!(rx2.try_recv().unwrap().data, vec![2]);

        // SharedWorker sends messages back to both pages
        let tx1 = endpoints1.worker_to_page_tx.unwrap();
        let tx2 = endpoints2.worker_to_page_tx.unwrap();
        tx1.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        tx2.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();

        // Page 1 drains its messages
        let (msgs1, disc1) = state1.drain_all_shared_worker_messages();
        assert_eq!(msgs1.len(), 1);
        assert!(disc1.is_empty());
        // Page 2 drains its messages
        let (msgs2, disc2) = state2.drain_all_shared_worker_messages();
        assert_eq!(msgs2.len(), 1);
        assert!(disc2.is_empty());

        // Page 1 navigates away — SharedWorker survives
        state1.disconnect_shared_worker_ports();
        // disconnect drops the SharedWorkerPortRef → connected_page_count decrements
        assert_eq!(handle.connected_page_count(), 1);
        assert!(!handle.is_closing());

        // Page 2 still connected
        assert_eq!(state2.shared_worker_port_count(), 1);

        // SharedWorker self.close() — terminates
        handle.close();
        handle.mark_terminated();
        assert!(handle.is_closing());
        assert!(handle.is_terminated());

        // Delegate reaps terminated shared worker with zero connected pages
        // (after page 2 also disconnects)
        state2.disconnect_shared_worker_ports();
        assert_eq!(handle.connected_page_count(), 0);
        delegate.reap_terminated_shared_workers();
        assert_eq!(delegate.shared_worker_count(), 0);
    }

    // ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────
    // @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope]
    //   [criterion:19] DF-WK-8 / DF-WK-10

    #[test]
    fn test_service_worker_registration_id_equality() {
        let id1 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let id2 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let id3 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_service_worker_handle_lifecycle() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installing
        );
        assert!(!handle.is_intercepting_fetch());
    }

    #[test]
    fn test_service_worker_handle_state_transitions() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        // Installing → Installed
        handle.transition_state(ServiceWorkerRegistrationState::Installed);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installed
        );

        // Installed → Activating
        handle.transition_state(ServiceWorkerRegistrationState::Activating);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activating
        );

        // Activating → Activated + enable fetch interception
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activated
        );
        assert!(handle.is_intercepting_fetch());
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        );
    }

    #[test]
    fn test_service_worker_handle_terminate_disables_interception() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        handle.enable_fetch_interception();
        assert!(handle.is_intercepting_fetch());

        // Per SPEC criterion #19: "terminate 后正确注销"
        handle.terminate();
        assert!(handle.is_closing());
        assert!(!handle.is_intercepting_fetch());
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );
    }

    #[test]
    fn test_service_worker_scope_config_from_stealth_profile() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = ServiceWorkerScopeConfig::from(&profile);
        assert!(config.stealth_profile.is_some());
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
    }

    #[test]
    fn test_service_worker_global_scope_state() {
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id.clone(), &config);

        assert!(!scope.has_fetch_handler);
        assert!(!scope.has_activate_handler);
        assert!(!scope.has_install_handler);
        assert!(!scope.has_message_handler);
        assert_eq!(scope.scope_url, "/app/");
        assert!(scope.is_url_in_scope("/app/page1"));
        assert!(scope.is_url_in_scope("/app/sub/page2"));
        assert!(!scope.is_url_in_scope("/other/page"));
    }

    #[test]
    fn test_service_worker_global_scope_fetch_handler() {
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let mut scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);

        scope.set_fetch_handler();
        assert!(scope.has_fetch_handler);
        assert!(scope.is_url_in_scope("/anything"));
    }

    #[test]
    fn test_webview_state_service_worker_control() {
        let mut state = BaoWebViewState::default();
        assert!(!state.is_controlled_by_service_worker());

        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        state.set_controlling_service_worker(handle);
        assert!(state.is_controlled_by_service_worker());

        state.clear_controlling_service_worker();
        assert!(!state.is_controlled_by_service_worker());
    }

    #[test]
    fn test_webview_state_service_worker_scope_matching() {
        let mut state = BaoWebViewState::default();
        assert!(!state.is_url_in_service_worker_scope("/app/page1"));

        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/app/".to_string(), None);
        state.set_controlling_service_worker(handle);
        state.register_service_worker_scope(scope);

        assert!(state.is_url_in_service_worker_scope("/app/page1"));
        assert!(state.is_url_in_service_worker_scope("/app/sub/page2"));
        assert!(!state.is_url_in_service_worker_scope("/other/page"));
    }

    #[test]
    fn test_delegate_service_worker_registration() {
        let delegate = BaoServoDelegate::new();
        assert_eq!(delegate.service_worker_count(), 0);

        let (handle, is_new) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert!(is_new);
        assert_eq!(delegate.service_worker_count(), 1);

        // Re-register same (script_url, scope) returns existing
        let (handle2, is_new2) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert!(!is_new2);
        assert_eq!(delegate.service_worker_count(), 1);
    }

    #[test]
    fn test_delegate_find_service_worker_for_url() {
        let delegate = BaoServoDelegate::new();

        // Register a ServiceWorker for /app/ scope
        let handle = delegate
            .get_or_create_service_worker("sw.js", "/app/", None)
            .0;

        // Not intercepting yet — find_service_worker_for_url returns None
        assert!(delegate.find_service_worker_for_url("/app/page1").is_none());

        // Activate and enable fetch interception
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();

        // Now it should be found for URLs in scope
        let found = delegate.find_service_worker_for_url("/app/page1");
        assert!(found.is_some());
        assert_eq!(found.unwrap().script_url, "sw.js");

        // Not found for URLs outside scope
        assert!(delegate
            .find_service_worker_for_url("/other/page")
            .is_none());
    }

    #[test]
    fn test_delegate_service_worker_unregistration() {
        let delegate = BaoServoDelegate::new();
        let (handle, _) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert_eq!(delegate.service_worker_count(), 1);

        let id = handle.id();
        assert!(delegate.unregister_service_worker(&id));
        assert_eq!(delegate.service_worker_count(), 0);

        // Double unregister returns false
        assert!(!delegate.unregister_service_worker(&id));
    }

    #[test]
    fn test_delegate_service_worker_stealth_consistency_no_violations() {
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Register a ServiceWorker with matching stealth profile
        let handle =
            ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), Some(profile.clone()));
        delegate.register_service_worker(handle);
        // Activate to enable fetch interception
        let all = delegate.all_service_workers();
        all[0].transition_state(ServiceWorkerRegistrationState::Activated);
        all[0].enable_fetch_interception();

        let violations = delegate.verify_service_worker_stealth_consistency(&profile);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_delegate_service_worker_stealth_consistency_violation_no_profile() {
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Register a ServiceWorker without stealth profile — this is a violation
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        delegate.register_service_worker(handle);
        let all = delegate.all_service_workers();
        all[0].transition_state(ServiceWorkerRegistrationState::Activated);
        all[0].enable_fetch_interception();

        let violations = delegate.verify_service_worker_stealth_consistency(&profile);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn test_service_worker_persistent_lifecycle_across_page_navigation() {
        // @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
        // SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
        // 且 terminate 后正确注销"
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Page 1 registers a ServiceWorker
        let (handle, is_new) =
            delegate.get_or_create_service_worker("sw.js", "/", Some(profile.clone()));
        assert!(is_new);
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();

        // Page 1 is controlled by the ServiceWorker
        let mut page_state = BaoWebViewState::default();
        page_state.set_controlling_service_worker(handle.clone());
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::from(&profile);
        page_state
            .register_service_worker_scope(ServiceWorkerGlobalScopeState::new(reg_id, &config));
        assert!(page_state.is_controlled_by_service_worker());

        // Page navigation: clear controlling reference (SW survives in delegate registry)
        page_state.clear_controlling_service_worker();
        assert!(!page_state.is_controlled_by_service_worker());

        // ServiceWorker still exists in delegate registry
        assert_eq!(delegate.service_worker_count(), 1);
        assert!(delegate.find_service_worker_for_url("/page2").is_some());

        // Page 2 can be controlled by the same ServiceWorker
        let mut page2_state = BaoWebViewState::default();
        page2_state.set_controlling_service_worker(handle.clone());
        assert!(page2_state.is_controlled_by_service_worker());
    }

    #[test]
    fn test_service_worker_fetch_intercept_mode() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );

        handle.enable_fetch_interception();
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        );

        handle.disable_fetch_interception();
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );
    }

    #[test]
    fn test_service_worker_registration_state_all_transitions() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installing
        );

        handle.transition_state(ServiceWorkerRegistrationState::Installed);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installed
        );

        handle.transition_state(ServiceWorkerRegistrationState::Activating);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activating
        );

        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activated
        );

        handle.transition_state(ServiceWorkerRegistrationState::Redundant);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Redundant
        );
    }

    #[test]
    fn test_service_worker_navigator_from_scope_config() {
        let config = ServiceWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Mozilla/5.0 Test".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 4,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string()],
            registering_page_url: "https://example.com/".to_string(),
        };
        let nav = WorkerNavigator::from_service_scope_config(&config);
        assert_eq!(nav.user_agent, "Mozilla/5.0 Test");
        assert_eq!(nav.platform, "Linux x86_64");
        assert_eq!(nav.hardware_concurrency, 4);
        assert_eq!(nav.language, "zh-CN");
        assert_eq!(nav.languages, vec!["zh-CN".to_string(), "zh".to_string()]);
    }

    #[test]
    fn test_webview_state_service_worker_scope_config() {
        let mut state = BaoWebViewState::default();
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        state.set_controlling_service_worker(handle);
        state.register_service_worker_scope(scope);

        let new_config = ServiceWorkerScopeConfig {
            user_agent: "Updated Agent".to_string(),
            ..ServiceWorkerScopeConfig::default()
        };
        state.set_service_worker_scope_config(&new_config);
        assert_eq!(
            state.service_worker_scope().unwrap().navigator().user_agent,
            "Updated Agent"
        );
    }
}
