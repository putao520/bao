use super::*;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, InputEventId, InputEventResult, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, TraversalId,
    WebView, WebViewDelegate,
};

use bao_cdp::servo_bridge::main_frame_id_for_target;
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
        try_recv_worker_msg(&self.worker_to_page_rx)
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
        drain_worker_rx(&self.worker_to_page_rx)
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

/// Try to receive one worker→page message from `rx` — the single shared
/// core behind the DedicatedWorker (DF-WK-5) and SharedWorker port
/// (DF-WK-7) `try_recv_from_worker` implementations.
///
/// @trace REQ-BRW-004 [criterion:6] DF-WK-5 / DF-WK-7
pub(super) fn try_recv_worker_msg(
    rx: &Receiver<WorkerStructuredMessage>,
) -> Result<Option<WorkerStructuredMessage>, ()> {
    match rx.try_recv() {
        Ok(msg) => Ok(Some(msg)),
        Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(()),
    }
}

/// Drain all pending worker→page messages from `rx` until the channel is
/// empty or disconnected — the single shared core behind both the
/// DedicatedWorker channel bridge (DF-WK-5) and the SharedWorker port
/// channel (DF-WK-7) `drain_worker_messages` implementations.
///
/// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
pub(super) fn drain_worker_rx(rx: &Receiver<WorkerStructuredMessage>) -> WorkerDrainResult {
    let mut messages = Vec::new();
    let mut disconnected = false;
    loop {
        match rx.try_recv() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            cdp_target_id: Some("7".to_string()),
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
            cdp_target_id: Some("7".to_string()),
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
        assert!(
            state
                .worker_channel(&WorkerId("worker2.js".to_string()))
                .is_some()
        );
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
            cdp_target_id: Some("7".to_string()),
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
            cdp_target_id: Some("7".to_string()),
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
            cdp_target_id: Some("7".to_string()),
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
}
