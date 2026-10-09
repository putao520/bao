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
        try_recv_worker_msg(&self.worker_to_page_rx)
    }

    /// Drain all pending worker→page messages from this port (DF-WK-7).
    ///
    /// Returns a `WorkerDrainResult` with messages and disconnected flag.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] DF-WK-7
    /// @trace REQ-BRW-004 [criterion:18] crash-safe teardown detection
    pub fn drain_worker_messages(&self) -> WorkerDrainResult {
        drain_worker_rx(&self.worker_to_page_rx)
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

#[cfg(test)]
mod tests {
    use super::*;

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
            cdp_target_id: Some("7".to_string()),
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
}
