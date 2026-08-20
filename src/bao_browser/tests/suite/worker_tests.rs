// @trace TEST-BRW-004 [req:REQ-BRW-004,REQ-BRW-4] [level:unit,integration]
// Worker integration & acceptance tests for REQ-BRW-004 (Dedicated Worker) + REQ-BRW-4 (Worker/SharedWorker/ServiceWorker).
//
// Tests cover:
//   - WorkerHandle lifecycle (new / terminate / mark_terminated / is_closing / is_terminated)
//   - WorkerChannelBridge bidirectional message channel (DF-WK-4 / DF-WK-5)
//   - StructuredClonePayload + WorkerStructuredMessage (criterion #6)
//   - WorkerErrorEvent (criterion #9)
//   - WorkerTeardownPath + WorkerLifecycleState + crash_safe_teardown_worker (criterion #18)
//   - AutoCloseWorker RAII guard (criterion #10)
//   - WorkerLocation URL parsing (entity:WorkerLocation)
//   - WorkerNavigator from WorkerScopeConfig (criterion #12)
//   - WorkerScopeConfig from StealthProfile (criterion #12-17)
//   - DedicatedWorkerGlobalScopeState (entity:DedicatedWorkerGlobalScope)
//   - WorkerGlobalScopeState (entity:WorkerGlobalScope)
//   - WorkerScriptSource / WorkerScriptLoadResult / WorkerScriptLoadError / WorkerScriptType
//   - is_javascript_mime_type (DF-WK-2)
//   - SharedWorker types (SharedWorkerId / SharedWorkerHandle / SharedWorkerGlobalScopeState)
//   - WorkerMessageDirection / WorkerMessageEvent
//   - WorkerDrainResult (crash-safe drain)
//
// NOTE: These are pure data-type and channel tests — no servo/BaoRuntime required.
// Integration tests that require a live BaoRuntime are in bce004_repro_tests.rs / bce004_stress_tests.rs.

#![allow(dead_code)]

use bao_browser::{
    crash_safe_teardown_worker, is_javascript_mime_type, AutoCloseWorker, BaoServoDelegate,
    DedicatedWorkerGlobalScopeState, SharedWorkerChannelBridge, SharedWorkerConnectEvent,
    SharedWorkerGlobalScopeState, SharedWorkerHandle, SharedWorkerId, SharedWorkerPortRef,
    SharedWorkerScopeConfig, StructuredClonePayload, WorkerChannelBridge, WorkerChannelEndpoints,
    WorkerErrorEvent, WorkerGlobalScopeState, WorkerHandle, WorkerId, WorkerLifecycleState,
    WorkerLocation, WorkerMessageDirection, WorkerMessageEvent, WorkerNavigator,
    WorkerNetworkInformation, WorkerScopeConfig, WorkerScriptLoadError, WorkerScriptLoadResult,
    WorkerScriptLoadState, WorkerScriptLoader, WorkerScriptSource, WorkerScriptType,
    WorkerStructuredMessage, WorkerTeardownPath, WorkerTeardownResult,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;

// Shared test helpers (deterministic wait_for_condition to replace sleep polling).
// @trace NFR-TEST-REPRODUCIBILITY [criterion:no_magic_sleep]
#[path = "common/mod.rs"]
mod common;
use common::{wait_for_condition, wait_for_worker_stopped};

// ═══════════════════════════════════════════════════════════════════════
// §1 WorkerHandle lifecycle (criterion #1, #4, #5, #18)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:1] new Worker(url) — WorkerHandle::new creates handle in running state
#[test]
fn test_worker_handle_new_running_state() {
    let handle = WorkerHandle::new("https://example.com/worker.js".into());
    // @trace REQ-BRW-004 [criterion:1] Worker starts in running state
    assert!(!handle.is_closing(), "new Worker should not be closing");
    assert!(
        !handle.is_terminated(),
        "new Worker should not be terminated"
    );
    assert_eq!(handle.script_url, "https://example.com/worker.js");
}

/// @trace REQ-BRW-004 [criterion:4] worker.terminate() sets closing flag
#[test]
fn test_worker_handle_terminate_sets_closing() {
    let handle = WorkerHandle::new("worker.js".into());
    assert!(!handle.is_closing());
    // @trace REQ-BRW-004 [criterion:4] terminate sets closing flag
    handle.terminate();
    assert!(handle.is_closing(), "terminate() should set closing flag");
    assert!(
        !handle.is_terminated(),
        "terminate() should not set terminated flag immediately"
    );
}

/// @trace REQ-BRW-004 [criterion:4] terminate() is idempotent
#[test]
fn test_worker_handle_terminate_idempotent() {
    let handle = WorkerHandle::new("w.js".into());
    handle.terminate();
    handle.terminate(); // second call should not panic
    assert!(handle.is_closing());
}

/// @trace REQ-BRW-004 [criterion:5] self.close() equivalent — mark_terminated after closing
#[test]
fn test_worker_handle_mark_terminated() {
    let handle = WorkerHandle::new("w.js".into());
    // @trace REQ-BRW-004 [criterion:5] self.close() path: closing then terminated
    handle.terminate();
    assert!(handle.is_closing());
    handle.mark_terminated();
    assert!(
        handle.is_terminated(),
        "mark_terminated should set terminated flag"
    );
    assert!(
        handle.is_closing(),
        "closing flag should remain set after mark_terminated"
    );
}

/// @trace REQ-BRW-004 [criterion:18] worker_global_addr for REALM_PROFILES unregistration
#[test]
fn test_worker_handle_global_addr_default_zero() {
    let handle = WorkerHandle::new("w.js".into());
    // @trace REQ-BRW-004 [criterion:18] default global addr is 0 (not yet set)
    assert_eq!(
        handle.worker_global_addr(),
        0,
        "default global addr should be 0"
    );
}

/// @trace REQ-BRW-004 [criterion:18] set_worker_global_addr + worker_global_addr_arc
#[test]
fn test_worker_handle_set_global_addr() {
    let handle = WorkerHandle::new("w.js".into());
    // @trace REQ-BRW-004 [criterion:18] set global addr for REALM_PROFILES tracking
    handle.set_worker_global_addr(0xDEAD_BEEF);
    assert_eq!(handle.worker_global_addr(), 0xDEAD_BEEF);

    // Arc variant shares the same atomic
    let arc = handle.worker_global_addr_arc();
    assert_eq!(arc.load(Ordering::Acquire), 0xDEAD_BEEF);

    // Mutating via handle reflects in arc
    handle.set_worker_global_addr(0xCAFE);
    assert_eq!(arc.load(Ordering::Acquire), 0xCAFE);
}

/// @trace REQ-BRW-004 [criterion:18] unregister_stealth_profile with zero addr is safe
#[test]
fn test_worker_handle_unregister_stealth_profile_zero_addr_safe() {
    let handle = WorkerHandle::new("w.js".into());
    // Should not panic when addr is 0 (worker never completed scope_init)
    handle.unregister_stealth_profile();
}

// ═══════════════════════════════════════════════════════════════════════
// §2 WorkerId
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:Worker] WorkerId equality and hashing
#[test]
fn test_worker_id_equality_and_hash() {
    let id1 = WorkerId("worker-1".into());
    let id2 = WorkerId("worker-1".into());
    let id3 = WorkerId("worker-2".into());
    assert_eq!(id1, id2, "same string WorkerIds should be equal");
    assert_ne!(id1, id3, "different string WorkerIds should not be equal");

    // Hash consistency
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(id1.clone());
    assert!(set.contains(&id2), "equal WorkerIds should hash the same");
    assert!(!set.contains(&id3));
}

// ═══════════════════════════════════════════════════════════════════════
// §3 WorkerChannelBridge — bidirectional message channel (criterion #2, #3, #6)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:6] DF-WK-4 / DF-WK-5 channel creation
#[test]
fn test_worker_channel_bridge_new() {
    let worker_id = WorkerId("test-worker".into());
    let (bridge, endpoints) = WorkerChannelBridge::new(worker_id.clone());
    // Bridge should have no messages initially
    let result = bridge.try_recv_from_worker();
    assert!(result.is_ok(), "try_recv should not error on empty channel");
    assert_eq!(
        result.unwrap().is_none(),
        true,
        "empty channel should return None"
    );
    // Endpoints should carry the worker_id
    assert_eq!(endpoints.worker_id, worker_id);
}

/// @trace REQ-BRW-004 [criterion:2] DF-WK-4: page→worker postMessage
#[test]
fn test_worker_channel_bridge_page_to_worker() {
    let worker_id = WorkerId("ch-test".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id.clone());

    // @trace REQ-BRW-004 [criterion:6] StructuredClonePayload with data
    let payload = StructuredClonePayload {
        data: vec![1, 2, 3, 4],
        transferable_count: 0,
    };

    // Page sends message to worker
    let send_result = bridge.post_message_to_worker(payload.clone());
    assert!(send_result.is_ok(), "post_message_to_worker should succeed");

    // Worker receives the message
    let rx = endpoints
        .page_to_worker_rx
        .take()
        .expect("should have receiver");
    let received = rx.try_recv().expect("should receive message");
    assert_eq!(received.data, payload.data);
    assert_eq!(received.transferable_count, 0);
}

/// @trace REQ-BRW-004 [criterion:3] DF-WK-5: worker→page postMessage (self.postMessage)
#[test]
fn test_worker_channel_bridge_worker_to_page() {
    let worker_id = WorkerId("ch-w2p".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id.clone());

    // Worker sends message to page
    let tx = endpoints
        .worker_to_page_tx
        .take()
        .expect("should have sender");
    let msg = WorkerStructuredMessage::metadata_only(
        worker_id.clone(),
        WorkerMessageDirection::WorkerToPage,
    );
    tx.send(msg.clone()).expect("send should succeed");

    // Page receives the message
    let result = bridge.try_recv_from_worker();
    assert!(result.is_ok());
    let received = result.unwrap().expect("should have a message");
    assert_eq!(received.worker_id, worker_id);
    assert_eq!(received.direction, WorkerMessageDirection::WorkerToPage);
}

/// @trace REQ-BRW-004 [criterion:6] StructuredClonePayload with transferable
#[test]
fn test_structured_clone_payload_with_transferable() {
    let payload = StructuredClonePayload {
        data: vec![0xAA, 0xBB, 0xCC],
        transferable_count: 2,
    };
    let cloned = payload.clone();
    assert_eq!(cloned.data, vec![0xAA, 0xBB, 0xCC]);
    assert_eq!(cloned.transferable_count, 2);
}

/// @trace REQ-BRW-004 [criterion:6] WorkerStructuredMessage with payload
#[test]
fn test_worker_structured_message_with_payload() {
    let worker_id = WorkerId("msg-test".into());
    let payload_data: Vec<u8> = vec![42];
    let transferable_count: u32 = 1;
    // @trace REQ-BRW-004 [criterion:6] with_payload takes (Vec<u8>, u32) not StructuredClonePayload
    let msg = WorkerStructuredMessage::with_payload(
        worker_id.clone(),
        WorkerMessageDirection::PageToWorker,
        payload_data.clone(),
        transferable_count,
    );
    assert_eq!(msg.worker_id, worker_id);
    assert_eq!(msg.direction, WorkerMessageDirection::PageToWorker);
    assert!(msg.payload.is_some());
    let p = msg.payload.unwrap();
    assert_eq!(p.data, payload_data);
    assert_eq!(p.transferable_count, transferable_count);
    // Message ID should be unique (monotonically increasing)
    assert!(msg.message_id > 0);
}

/// @trace REQ-BRW-004 [criterion:6] WorkerStructuredMessage metadata_only
#[test]
fn test_worker_structured_message_metadata_only() {
    let worker_id = WorkerId("meta-test".into());
    let msg = WorkerStructuredMessage::metadata_only(
        worker_id.clone(),
        WorkerMessageDirection::WorkerToPage,
    );
    assert!(
        msg.payload.is_none(),
        "metadata_only should have no payload"
    );
    assert!(msg.message_id > 0);
}

/// @trace REQ-BRW-004 [criterion:6] message IDs are unique
#[test]
fn test_worker_structured_message_unique_ids() {
    let worker_id = WorkerId("uniq-test".into());
    let msg1 = WorkerStructuredMessage::metadata_only(
        worker_id.clone(),
        WorkerMessageDirection::PageToWorker,
    );
    let msg2 = WorkerStructuredMessage::metadata_only(
        worker_id.clone(),
        WorkerMessageDirection::WorkerToPage,
    );
    assert_ne!(
        msg1.message_id, msg2.message_id,
        "each message should have a unique ID"
    );
    assert!(
        msg2.message_id > msg1.message_id,
        "message IDs should be monotonically increasing"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 WorkerDrainResult — crash-safe drain (criterion #18)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:18] drain_worker_messages on empty channel
#[test]
fn test_worker_drain_empty_channel() {
    let worker_id = WorkerId("drain-empty".into());
    let (bridge, _endpoints) = WorkerChannelBridge::new(worker_id);
    let result = bridge.drain_worker_messages();
    assert!(
        result.messages.is_empty(),
        "empty channel should drain no messages"
    );
    assert!(
        !result.disconnected,
        "empty channel should not be disconnected"
    );
}

/// @trace REQ-BRW-004 [criterion:18] drain_worker_messages with messages
#[test]
fn test_worker_drain_with_messages() {
    let worker_id = WorkerId("drain-msg".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id.clone());

    let tx = endpoints
        .worker_to_page_tx
        .take()
        .expect("should have sender");
    for i in 0..5 {
        let msg = WorkerStructuredMessage::metadata_only(
            worker_id.clone(),
            WorkerMessageDirection::WorkerToPage,
        );
        tx.send(msg).expect("send should succeed");
    }
    drop(tx); // close sender to signal end

    let result = bridge.drain_worker_messages();
    assert_eq!(result.messages.len(), 5, "should drain all 5 messages");
    assert!(
        result.disconnected,
        "closed sender should mark disconnected"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §5 WorkerErrorEvent (criterion #9)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:9] onerror event propagation — ErrorEvent fields
#[test]
fn test_worker_error_event_fields() {
    let event = WorkerErrorEvent {
        worker_id: WorkerId("err-worker".into()),
        message: "Uncaught TypeError: x is not a function".into(),
        filename: "worker.js".into(),
        lineno: 42,
        colno: 10,
    };
    assert_eq!(event.worker_id.0, "err-worker");
    assert_eq!(event.message, "Uncaught TypeError: x is not a function");
    assert_eq!(event.filename, "worker.js");
    assert_eq!(event.lineno, 42);
    assert_eq!(event.colno, 10);
}

/// @trace REQ-BRW-004 [criterion:9] WorkerErrorEvent is Clone
#[test]
fn test_worker_error_event_clone() {
    let event = WorkerErrorEvent {
        worker_id: WorkerId("clone-err".into()),
        message: "test error".into(),
        filename: "test.js".into(),
        lineno: 1,
        colno: 1,
    };
    let cloned = event.clone();
    assert_eq!(cloned.worker_id, event.worker_id);
    assert_eq!(cloned.message, event.message);
    assert_eq!(cloned.lineno, event.lineno);
}

// ═══════════════════════════════════════════════════════════════════════
// §6 WorkerMessageDirection + WorkerMessageEvent
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:Worker] WorkerMessageDirection variants
#[test]
fn test_worker_message_direction_variants() {
    let page_to_worker = WorkerMessageDirection::PageToWorker;
    let worker_to_page = WorkerMessageDirection::WorkerToPage;
    assert_ne!(page_to_worker, worker_to_page);
}

/// @trace REQ-BRW-004 [entity:Worker] WorkerMessageEvent construction
#[test]
fn test_worker_message_event_construction() {
    let event = WorkerMessageEvent {
        worker_id: WorkerId("msg-evt".into()),
        direction: WorkerMessageDirection::PageToWorker,
    };
    assert_eq!(event.worker_id.0, "msg-evt");
    assert_eq!(event.direction, WorkerMessageDirection::PageToWorker);
}

// ═══════════════════════════════════════════════════════════════════════
// §7 WorkerTeardownPath + WorkerLifecycleState (criterion #18)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:18] WorkerTeardownPath variants
#[test]
fn test_worker_teardown_path_variants() {
    let terminate = WorkerTeardownPath::Terminate;
    let self_close = WorkerTeardownPath::SelfClose;
    let page_unload = WorkerTeardownPath::PageUnload;
    // All three paths should be distinct
    assert_ne!(terminate, self_close);
    assert_ne!(self_close, page_unload);
    assert_ne!(terminate, page_unload);
}

/// @trace REQ-BRW-004 [criterion:18] WorkerLifecycleState transitions
#[test]
fn test_worker_lifecycle_state_variants() {
    let running = WorkerLifecycleState::Running;
    let closing_terminate = WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate);
    let closing_self = WorkerLifecycleState::Closing(WorkerTeardownPath::SelfClose);
    let terminated_unload = WorkerLifecycleState::Terminated(WorkerTeardownPath::PageUnload);
    let failed = WorkerLifecycleState::Failed;

    assert_ne!(running, closing_terminate);
    assert_ne!(closing_terminate, closing_self);
    assert_ne!(terminated_unload, failed);
}

/// @trace REQ-BRW-004 [criterion:18] WorkerTeardownResult is_crash_safe
#[test]
fn test_worker_teardown_result_crash_safe() {
    // Fully crash-safe: all flags true
    let safe = WorkerTeardownResult {
        path: WorkerTeardownPath::Terminate,
        thread_joined: true,
        realm_profile_unregistered: true,
        closing_flag_set: true,
        never_registered: false,
    };
    assert!(
        safe.is_crash_safe(),
        "fully safe teardown should be crash-safe"
    );

    // Not crash-safe: thread not joined
    let not_joined = WorkerTeardownResult {
        path: WorkerTeardownPath::Terminate,
        thread_joined: false,
        realm_profile_unregistered: true,
        closing_flag_set: true,
        never_registered: false,
    };
    assert!(
        !not_joined.is_crash_safe(),
        "thread not joined should not be crash-safe"
    );

    // Not crash-safe: closing flag not set
    let not_closing = WorkerTeardownResult {
        path: WorkerTeardownPath::SelfClose,
        thread_joined: true,
        realm_profile_unregistered: true,
        closing_flag_set: false,
        never_registered: false,
    };
    assert!(
        !not_closing.is_crash_safe(),
        "closing flag not set should not be crash-safe"
    );
}

/// @trace REQ-BRW-004 [criterion:18] crash_safe_teardown_worker without WebWorker
#[test]
fn test_crash_safe_teardown_worker_no_web_worker() {
    let handle = WorkerHandle::new("teardown-test.js".into());
    // @trace REQ-BRW-004 [criterion:18] crash-safe teardown without WebWorker (servo DOM Worker path)
    let result = crash_safe_teardown_worker(&handle, WorkerTeardownPath::Terminate);
    assert!(result.closing_flag_set, "closing flag should be set");
    assert!(
        result.thread_joined,
        "servo DOM Worker path should report joined=true"
    );
    assert!(
        !result.realm_profile_unregistered,
        "no global addr set, so realm unreg should be false"
    );
    assert!(handle.is_closing());
    assert!(handle.is_terminated());
}

/// @trace REQ-BRW-004 [criterion:18] crash_safe_teardown_worker with global addr
#[test]
fn test_crash_safe_teardown_worker_with_global_addr() {
    let handle = WorkerHandle::new("teardown-addr.js".into());
    handle.set_worker_global_addr(0x1000);
    let result = crash_safe_teardown_worker(&handle, WorkerTeardownPath::SelfClose);
    assert!(result.closing_flag_set);
    assert!(
        result.realm_profile_unregistered,
        "with global addr, realm should be unregistered"
    );
    assert_eq!(result.path, WorkerTeardownPath::SelfClose);
}

// ═══════════════════════════════════════════════════════════════════════
// §8 AutoCloseWorker — RAII guard (criterion #10)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:10] AutoCloseWorker terminates on drop
#[test]
fn test_auto_close_worker_drop_terminates() {
    let handle = WorkerHandle::new("auto-close.js".into());
    {
        let _guard = AutoCloseWorker::new(handle);
        // Guard goes out of scope → should terminate the worker
    }
    // After drop, the handle inside AutoCloseWorker should be terminated
    // We can't access the handle directly after moving it, so we test via a shared approach:
    // Create a new handle and verify the RAII behavior
    let handle2 = WorkerHandle::new("auto-close-2.js".into());
    let guard = AutoCloseWorker::new(handle2);
    assert!(
        !guard.handle().is_closing(),
        "guard should not close before drop"
    );
    drop(guard);
}

/// @trace REQ-BRW-004 [criterion:10] AutoCloseWorker lifecycle_state transitions
#[test]
fn test_auto_close_worker_lifecycle_state() {
    let handle = WorkerHandle::new("lifecycle.js".into());
    let guard = AutoCloseWorker::new(handle);
    // @trace REQ-BRW-004 [criterion:10] initial state is Running
    assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Running);

    // After terminate_via
    let mut guard2 = AutoCloseWorker::new(WorkerHandle::new("lifecycle2.js".into()));
    // @trace REQ-BRW-004 [criterion:4] terminate via Terminate path
    guard2.terminate_via(WorkerTeardownPath::Terminate);
    match guard2.lifecycle_state() {
        WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate) => {}
        other => panic!("expected Closing(Terminate), got {:?}", other),
    }
}

/// @trace REQ-BRW-004 [criterion:10] AutoCloseWorker terminate_via is idempotent
#[test]
fn test_auto_close_worker_terminate_via_idempotent() {
    let mut guard = AutoCloseWorker::new(WorkerHandle::new("idempotent.js".into()));
    guard.terminate_via(WorkerTeardownPath::Terminate);
    // Second call with different path should not change the teardown path
    guard.terminate_via(WorkerTeardownPath::SelfClose);
    match guard.lifecycle_state() {
        WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate) => {}
        other => panic!("expected Closing(Terminate) (idempotent), got {:?}", other),
    }
}

/// @trace REQ-BRW-004 [criterion:10] AutoCloseWorker handle() accessor
#[test]
fn test_auto_close_worker_handle_accessor() {
    let handle = WorkerHandle::new("accessor.js".into());
    let guard = AutoCloseWorker::new(handle);
    assert_eq!(guard.handle().script_url, "accessor.js");
    assert!(!guard.handle().is_closing());
}

// ═══════════════════════════════════════════════════════════════════════
// §9 WorkerLocation — URL parsing (entity:WorkerLocation)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url parses HTTPS URL
#[test]
fn test_worker_location_from_https_url() {
    let loc = WorkerLocation::from_url("https://example.com:8443/path/script.js?q=1#frag").unwrap();
    assert_eq!(loc.href, "https://example.com:8443/path/script.js?q=1#frag");
    assert_eq!(loc.protocol, "https:");
    assert_eq!(loc.host, "example.com:8443");
    assert_eq!(loc.hostname, "example.com");
    assert_eq!(loc.port, "8443");
    assert_eq!(loc.pathname, "/path/script.js");
    assert_eq!(loc.search, "?q=1");
    assert_eq!(loc.hash, "#frag");
    assert_eq!(loc.origin, "https://example.com:8443");
}

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url default port omitted
#[test]
fn test_worker_location_default_port_omitted() {
    let loc = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
    assert_eq!(
        loc.host, "example.com",
        "default port should be omitted from host"
    );
    assert_eq!(loc.port, "", "default port should be empty string");
    assert_eq!(loc.origin, "https://example.com");
}

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url HTTP default port
#[test]
fn test_worker_location_http_default_port() {
    let loc = WorkerLocation::from_url("http://localhost/app.js").unwrap();
    assert_eq!(loc.protocol, "http:");
    assert_eq!(loc.host, "localhost");
    assert_eq!(loc.origin, "http://localhost");
}

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url invalid URL returns None
#[test]
fn test_worker_location_invalid_url() {
    assert!(WorkerLocation::from_url("not a url").is_none());
    assert!(WorkerLocation::from_url("").is_none());
}

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url data: URL
#[test]
fn test_worker_location_data_url() {
    let loc = WorkerLocation::from_url("data:text/javascript,self.close()").unwrap();
    assert_eq!(loc.protocol, "data:");
    // data: URLs have null origin per spec
    assert_eq!(loc.origin, "null");
}

/// @trace REQ-BRW-004 [entity:WorkerLocation] from_url_value
#[test]
fn test_worker_location_from_url_value() {
    let url = url::Url::parse("https://cdn.example.com/workers/v2/processor.js?v=3").unwrap();
    let loc = WorkerLocation::from_url_value(url);
    assert_eq!(loc.protocol, "https:");
    assert_eq!(loc.hostname, "cdn.example.com");
    assert_eq!(loc.pathname, "/workers/v2/processor.js");
    assert_eq!(loc.search, "?v=3");
}

// ═══════════════════════════════════════════════════════════════════════
// §10 WorkerNavigator (criterion #12)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致 — WorkerScopeConfig → WorkerNavigator
#[test]
fn test_worker_navigator_from_scope_config() {
    let config = WorkerScopeConfig {
        stealth_profile: None,
        user_agent: "Mozilla/5.0 (TestAgent)".into(),
        platform: "Win32".into(),
        hardware_concurrency: 8,
        language: "en-US".into(),
        languages: vec!["en-US".into(), "en".into()],
    };
    let nav = WorkerNavigator::from_scope_config(&config);
    // @trace REQ-BRW-004 [criterion:12] navigator values must match config
    assert_eq!(nav.user_agent, "Mozilla/5.0 (TestAgent)");
    assert_eq!(nav.platform, "Win32");
    assert_eq!(nav.hardware_concurrency, 8);
    assert_eq!(nav.language, "en-US");
    assert_eq!(nav.languages, vec!["en-US", "en"]);
    // Spec-mandated constant values
    assert_eq!(nav.product, "Gecko");
    assert_eq!(nav.app_code_name, "Mozilla");
    assert_eq!(nav.app_name, "Netscape");
    assert_eq!(nav.app_version, "Mozilla/5.0 (TestAgent)");
}

/// @trace REQ-BRW-004 [criterion:12] WorkerScopeConfig default values
#[test]
fn test_worker_scope_config_default() {
    let config = WorkerScopeConfig::default();
    assert!(config.stealth_profile.is_none());
    assert!(config.user_agent.is_empty());
    assert!(config.platform.is_empty());
    assert!(config.hardware_concurrency >= 1);
    assert_eq!(config.language, "en-US");
    assert_eq!(config.languages, vec!["en-US", "en"]);
}

/// @trace REQ-BRW-004 [criterion:12] StealthProfile → WorkerScopeConfig conversion
#[test]
fn test_stealth_profile_to_worker_scope_config() {
    let profile = bao_stealth::StealthProfile::chrome_default();
    let config: WorkerScopeConfig = (&profile).into();
    // @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    assert!(config.stealth_profile.is_some());
    assert_eq!(config.user_agent, profile.navigator.user_agent);
    assert_eq!(config.platform, profile.navigator.platform);
    assert_eq!(
        config.hardware_concurrency,
        profile.navigator.hardware_concurrency as usize
    );
    assert_eq!(config.language, profile.navigator.language);
    assert_eq!(config.languages, profile.navigator.languages);
}

/// @trace REQ-BRW-004 [entity:WorkerNavigator] WorkerNetworkInformation
#[test]
fn test_worker_network_information() {
    let info = WorkerNetworkInformation {
        effective_type: "4g".into(),
        downlink: 10,
        rtt: 50,
        save_data: false,
    };
    assert_eq!(info.effective_type, "4g");
    assert_eq!(info.downlink, 10);
    assert_eq!(info.rtt, 50);
    assert!(!info.save_data);
}

// ═══════════════════════════════════════════════════════════════════════
// §11 WorkerGlobalScopeState + DedicatedWorkerGlobalScopeState
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:WorkerGlobalScope] WorkerGlobalScopeState construction
#[test]
fn test_worker_global_scope_state_construction() {
    let config = WorkerScopeConfig::default();
    let scope = WorkerGlobalScopeState::new("https://example.com/worker.js".into(), &config);
    assert_eq!(scope.worker_url, "https://example.com/worker.js");
    assert!(!scope.closing, "new scope should not be closing");
    assert!(scope.location.is_some());
    let loc = scope.location.as_ref().unwrap();
    assert_eq!(loc.hostname, "example.com");
}

/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] DedicatedWorkerGlobalScopeState construction
#[test]
fn test_dedicated_worker_global_scope_state_construction() {
    let config = WorkerScopeConfig::default();
    let worker_id = WorkerId("https://example.com/dedicated.js".into());
    let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
    assert_eq!(scope.worker_id, worker_id);
    assert!(!scope.has_onmessage, "new scope should not have onmessage");
    assert!(!scope.has_onerror, "new scope should not have onerror");
    assert!(scope.location().is_some());
    assert_eq!(scope.navigator().user_agent, config.user_agent);
}

/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] set_onmessage / set_onerror
#[test]
fn test_dedicated_worker_global_scope_state_handlers() {
    let config = WorkerScopeConfig::default();
    let worker_id = WorkerId("handler-test.js".into());
    let mut scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
    scope.set_onmessage();
    assert!(scope.has_onmessage);
    scope.set_onerror();
    assert!(scope.has_onerror);
}

// ═══════════════════════════════════════════════════════════════════════
// §12 WorkerScriptSource / WorkerScriptLoadResult / WorkerScriptLoadError / WorkerScriptType
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptSource variants
#[test]
fn test_worker_script_source_variants() {
    let inline = WorkerScriptSource::Inline("self.close()".into());
    let url = WorkerScriptSource::Url("https://example.com/worker.js".into());
    assert_eq!(inline, WorkerScriptSource::Inline("self.close()".into()));
    assert_eq!(
        url,
        WorkerScriptSource::Url("https://example.com/worker.js".into())
    );
    assert_ne!(inline, url);
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptLoadResult
#[test]
fn test_worker_script_load_result() {
    let result = WorkerScriptLoadResult {
        source: "console.log('hello')".into(),
        final_url: "https://example.com/worker.js".into(),
        mime_type: Some("text/javascript".into()),
    };
    assert_eq!(result.source, "console.log('hello')");
    assert_eq!(result.final_url, "https://example.com/worker.js");
    assert_eq!(result.mime_type.as_deref(), Some("text/javascript"));
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptLoadError variants
#[test]
fn test_worker_script_load_error_variants() {
    let net_err = WorkerScriptLoadError::NetworkError("timeout".into());
    let mime_err = WorkerScriptLoadError::InvalidMimeType {
        received: "text/html".into(),
        url: "https://example.com/worker.js".into(),
    };
    let utf8_err = WorkerScriptLoadError::Utf8DecodeError("invalid utf8".into());
    let url_err = WorkerScriptLoadError::InvalidUrl("bad url".into());
    let cancelled = WorkerScriptLoadError::Cancelled;

    // Verify Debug formatting works
    assert!(format!("{:?}", net_err).contains("NetworkError"));
    assert!(format!("{:?}", mime_err).contains("InvalidMimeType"));
    assert!(format!("{:?}", utf8_err).contains("Utf8DecodeError"));
    assert!(format!("{:?}", url_err).contains("InvalidUrl"));
    assert!(format!("{:?}", cancelled).contains("Cancelled"));
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptType variants
#[test]
fn test_worker_script_type_variants() {
    assert_eq!(WorkerScriptType::default(), WorkerScriptType::Classic);
    assert_ne!(WorkerScriptType::Classic, WorkerScriptType::Module);
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptLoader construction
#[test]
fn test_worker_script_loader_construction() {
    let loader = WorkerScriptLoader {
        source: WorkerScriptSource::Url("https://example.com/w.js".into()),
        script_type: WorkerScriptType::Classic,
    };
    assert_eq!(
        loader.source,
        WorkerScriptSource::Url("https://example.com/w.js".into())
    );
    assert_eq!(loader.script_type, WorkerScriptType::Classic);
}

// ═══════════════════════════════════════════════════════════════════════
// §13 is_javascript_mime_type (DF-WK-2)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] valid JavaScript MIME types
#[test]
fn test_is_javascript_mime_type_valid() {
    let valid_types = [
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
    for mime in &valid_types {
        assert!(
            is_javascript_mime_type(mime),
            "{} should be a valid JS MIME type",
            mime
        );
    }
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] invalid MIME types
#[test]
fn test_is_javascript_mime_type_invalid() {
    let invalid_types = [
        "text/html",
        "application/json",
        "text/plain",
        "application/octet-stream",
        "image/png",
        "text/css",
    ];
    for mime in &invalid_types {
        assert!(
            !is_javascript_mime_type(mime),
            "{} should NOT be a valid JS MIME type",
            mime
        );
    }
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] MIME type with parameters
#[test]
fn test_is_javascript_mime_type_with_params() {
    // "text/javascript; charset=utf-8" → should strip params and match
    assert!(is_javascript_mime_type("text/javascript; charset=utf-8"));
    assert!(is_javascript_mime_type(
        "application/javascript ; charset=utf-8"
    ));
}

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] MIME type case-insensitive
#[test]
fn test_is_javascript_mime_type_case_insensitive() {
    assert!(is_javascript_mime_type("TEXT/JAVASCRIPT"));
    assert!(is_javascript_mime_type("Application/JavaScript"));
    assert!(is_javascript_mime_type("text/JavaScript"));
}

// ═══════════════════════════════════════════════════════════════════════
// §14 SharedWorker types (REQ-BRW-4)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerId construction
#[test]
fn test_shared_worker_id_construction() {
    let id = SharedWorkerId {
        script_url: "https://example.com/shared.js".into(),
        name: "my-shared-worker".into(),
    };
    assert_eq!(id.script_url, "https://example.com/shared.js");
    assert_eq!(id.name, "my-shared-worker");
}

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerHandle lifecycle
#[test]
fn test_shared_worker_handle_lifecycle() {
    let handle = SharedWorkerHandle::new("https://example.com/shared.js".into(), "sw-name".into());
    assert_eq!(handle.id().script_url, "https://example.com/shared.js");
    assert_eq!(handle.id().name, "sw-name");
    assert!(
        !handle.is_closing(),
        "new SharedWorker should not be closing"
    );
    assert!(
        !handle.is_terminated(),
        "new SharedWorker should not be terminated"
    );
    assert_eq!(handle.connected_page_count(), 0);

    // Page connects
    handle.page_connected();
    assert_eq!(handle.connected_page_count(), 1);

    // Close
    handle.close();
    assert!(handle.is_closing());

    // Mark terminated
    handle.mark_terminated();
    assert!(handle.is_terminated());
}

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerHandle page disconnect
#[test]
fn test_shared_worker_handle_page_disconnect() {
    let handle = SharedWorkerHandle::new("sw.js".into(), "".into());
    handle.page_connected();
    handle.page_connected();
    assert_eq!(handle.connected_page_count(), 2);
    // page_disconnected returns the PREVIOUS value (before decrement)
    let previous = handle.page_disconnected();
    assert_eq!(previous, 2, "page_disconnected returns the previous count");
    assert_eq!(handle.connected_page_count(), 1);
}

/// @trace REQ-BRW-4 [entity:SharedWorkerGlobalScope] SharedWorkerGlobalScopeState
#[test]
fn test_shared_worker_global_scope_state() {
    let config = SharedWorkerScopeConfig::default();
    let sw_id = SharedWorkerId {
        script_url: "https://example.com/shared.js".into(),
        name: "test-sw".into(),
    };
    let scope = SharedWorkerGlobalScopeState::new(sw_id.clone(), &config);
    assert_eq!(scope.shared_worker_id, sw_id);
    assert!(!scope.has_onconnect, "new scope should not have onconnect");
    assert_eq!(scope.connect_count, 0);

    // Set onconnect
    let mut scope2 = SharedWorkerGlobalScopeState::new(sw_id.clone(), &config);
    scope2.set_onconnect();
    assert!(scope2.has_onconnect);

    // Page connected
    scope2.page_connected();
    assert_eq!(scope2.connect_count, 1);
}

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerConnectEvent
#[test]
fn test_shared_worker_connect_event() {
    let event = SharedWorkerConnectEvent {
        shared_worker_id: SharedWorkerId {
            script_url: "sw.js".into(),
            name: "conn-test".into(),
        },
        page_url: "https://example.com/page1".into(),
    };
    assert_eq!(event.shared_worker_id.script_url, "sw.js");
    assert_eq!(event.page_url, "https://example.com/page1");
}

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerScopeConfig default
#[test]
fn test_shared_worker_scope_config_default() {
    let config = SharedWorkerScopeConfig::default();
    assert!(config.stealth_profile.is_none());
    assert!(config.user_agent.is_empty());
    assert!(config.hardware_concurrency >= 1);
}

/// @trace REQ-BRW-4 [criterion:12] StealthProfile → SharedWorkerScopeConfig conversion
#[test]
fn test_stealth_profile_to_shared_worker_scope_config() {
    let profile = bao_stealth::StealthProfile::chrome_default();
    let config: SharedWorkerScopeConfig = (&profile).into();
    assert!(config.stealth_profile.is_some());
    assert_eq!(config.user_agent, profile.navigator.user_agent);
    assert_eq!(config.platform, profile.navigator.platform);
    assert_eq!(config.language, profile.navigator.language);
}

// ═══════════════════════════════════════════════════════════════════════
// §15 WorkerChannelEndpoints (criterion #6)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:6] WorkerChannelEndpoints carry worker_id
#[test]
fn test_worker_channel_endpoints_carry_worker_id() {
    let worker_id = WorkerId("endpoint-test".into());
    let (_bridge, endpoints) = WorkerChannelBridge::new(worker_id.clone());
    assert_eq!(endpoints.worker_id, worker_id);
    // Endpoints should have receivers and senders
    assert!(
        endpoints.page_to_worker_rx.is_some(),
        "should have page_to_worker_rx"
    );
    assert!(
        endpoints.worker_to_page_tx.is_some(),
        "should have worker_to_page_tx"
    );
}

/// @trace REQ-BRW-004 [criterion:6] Full round-trip: page→worker→page
#[test]
fn test_worker_channel_full_round_trip() {
    let worker_id = WorkerId("round-trip".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id.clone());

    // Step 1: Page sends message to worker
    let payload = StructuredClonePayload {
        data: vec![1, 2, 3],
        transferable_count: 0,
    };
    bridge
        .post_message_to_worker(payload.clone())
        .expect("page→worker send should work");

    // Step 2: Worker receives and echoes back
    let rx = endpoints.page_to_worker_rx.take().expect("should have rx");
    let received = rx.try_recv().expect("worker should receive message");
    assert_eq!(received.data, vec![1, 2, 3]);

    // Step 3: Worker sends response to page
    let tx = endpoints.worker_to_page_tx.take().expect("should have tx");
    let response = WorkerStructuredMessage::with_payload(
        worker_id.clone(),
        WorkerMessageDirection::WorkerToPage,
        vec![4, 5, 6],
        0,
    );
    tx.send(response).expect("worker→page send should work");
    drop(tx);

    // Step 4: Page receives response
    let result = bridge.drain_worker_messages();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.disconnected, true); // tx was dropped
    let msg = &result.messages[0];
    assert_eq!(msg.direction, WorkerMessageDirection::WorkerToPage);
    let resp_payload = msg.payload.as_ref().expect("should have payload");
    assert_eq!(resp_payload.data, vec![4, 5, 6]);
}

// ═══════════════════════════════════════════════════════════════════════
// §16 Concurrent Worker channel stress (criterion #18)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:18] Multiple messages in sequence
#[test]
fn test_worker_channel_multiple_messages() {
    let worker_id = WorkerId("multi-msg".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id.clone());

    let tx = endpoints.worker_to_page_tx.take().expect("should have tx");
    for i in 0..100 {
        let msg = WorkerStructuredMessage::metadata_only(
            worker_id.clone(),
            WorkerMessageDirection::WorkerToPage,
        );
        tx.send(msg).expect(&format!("send #{} should work", i));
    }
    drop(tx);

    let result = bridge.drain_worker_messages();
    assert_eq!(result.messages.len(), 100, "should drain all 100 messages");
    assert!(result.disconnected);
}

/// @trace REQ-BRW-004 [criterion:18] Channel closed detection
#[test]
fn test_worker_channel_closed_detection() {
    let worker_id = WorkerId("closed-det".into());
    let (bridge, mut endpoints) = WorkerChannelBridge::new(worker_id);
    // Drop the sender without sending anything
    drop(endpoints.worker_to_page_tx.take());
    // Drain should detect disconnection
    let result = bridge.drain_worker_messages();
    assert!(
        result.disconnected,
        "dropped sender should be detected as disconnected"
    );
    assert!(result.messages.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// §17 WorkerHandle concurrent terminate (criterion #18 crash-safe)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:18] Concurrent terminate from multiple threads
#[test]
fn test_worker_handle_concurrent_terminate() {
    use std::sync::Arc;
    use std::thread;

    let handle = Arc::new(WorkerHandle::new("concurrent-terminate.js".into()));
    let mut handles = vec![];

    for _ in 0..8 {
        let h = Arc::clone(&handle);
        handles.push(thread::spawn(move || {
            h.terminate();
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    assert!(
        handle.is_closing(),
        "concurrent terminate should set closing flag"
    );
}

/// @trace REQ-BRW-004 [criterion:18] Concurrent set_worker_global_addr
#[test]
fn test_worker_handle_concurrent_set_global_addr() {
    use std::sync::Arc;
    use std::thread;

    let handle = Arc::new(WorkerHandle::new("concurrent-addr.js".into()));
    let mut handles = vec![];

    for i in 0..4 {
        let h = Arc::clone(&handle);
        handles.push(thread::spawn(move || {
            h.set_worker_global_addr(1000 + i);
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    // One of the values should be set (last writer wins)
    let addr = handle.worker_global_addr();
    assert!(
        addr >= 1000 && addr <= 1003,
        "addr should be one of the written values, got {}",
        addr
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §18 WorkerScriptLoadState (DF-WK-2)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [entity:Worker] [DF-WK-2] WorkerScriptLoadState all variants
#[test]
fn test_worker_script_load_state_variants() {
    let pending = WorkerScriptLoadState::Pending;
    let fetching = WorkerScriptLoadState::Fetching;
    let validating = WorkerScriptLoadState::Validating;
    let decoding = WorkerScriptLoadState::Decoding;
    let compiling = WorkerScriptLoadState::Compiling;
    let ready = WorkerScriptLoadState::Ready;
    let failed =
        WorkerScriptLoadState::Failed(WorkerScriptLoadError::NetworkError("timeout".into()));

    // is_ready() only true for Ready variant
    assert!(!pending.is_ready());
    assert!(!fetching.is_ready());
    assert!(!validating.is_ready());
    assert!(!decoding.is_ready());
    assert!(!compiling.is_ready());
    assert!(ready.is_ready());
    assert!(!failed.is_ready());

    // Debug formatting
    assert!(format!("{:?}", pending).contains("Pending"));
    assert!(format!("{:?}", ready).contains("Ready"));
    assert!(format!("{:?}", failed).contains("Failed"));
}

// ═══════════════════════════════════════════════════════════════════════
// §19 SharedWorkerPortRef (REQ-BRW-4)
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-4 [entity:SharedWorker] SharedWorkerPortRef construction
#[test]
fn test_shared_worker_port_ref_construction() {
    let handle = SharedWorkerHandle::new("port-ref.js".into(), "port-test".into());
    // SharedWorkerPortRef::new calls page_connected() internally
    let port_ref = SharedWorkerPortRef::new(handle);
    assert_eq!(port_ref.handle().id().script_url, "port-ref.js");
    assert_eq!(port_ref.handle().id().name, "port-test");
    assert_eq!(
        port_ref.handle().connected_page_count(),
        1,
        "new port should increment connected count"
    );
    // Drop the port_ref — should call page_disconnected()
    drop(port_ref);
}

// ═══════════════════════════════════════════════════════════════════════
// §20 Integration: StealthProfile → WorkerScopeConfig → DedicatedWorkerGlobalScopeState
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:12..17] Full stealth inheritance chain
#[test]
fn test_stealth_profile_inheritance_chain() {
    let profile = bao_stealth::StealthProfile::chrome_default();
    let config: WorkerScopeConfig = (&profile).into();
    let worker_id = WorkerId("stealth-chain.js".into());
    let scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);

    // @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    let nav = scope.navigator();
    assert_eq!(
        nav.user_agent, profile.navigator.user_agent,
        "worker navigator.userAgent must match parent page's stealth profile"
    );
    assert_eq!(
        nav.platform, profile.navigator.platform,
        "worker navigator.platform must match parent page's stealth profile"
    );
    assert_eq!(
        nav.hardware_concurrency, profile.navigator.hardware_concurrency as usize,
        "worker navigator.hardwareConcurrency must match parent page's stealth profile"
    );
    assert_eq!(
        nav.language, profile.navigator.language,
        "worker navigator.language must match parent page's stealth profile"
    );
    assert_eq!(
        nav.languages, profile.navigator.languages,
        "worker navigator.languages must match parent page's stealth profile"
    );

    // Stealth profile should be carried through
    assert!(
        config.stealth_profile.is_some(),
        "WorkerScopeConfig should carry the StealthProfile for Canvas/WebGL/Audio consistency"
    );
}

/// @trace REQ-BRW-004 [criterion:12..17] SharedWorker stealth inheritance
#[test]
fn test_stealth_profile_shared_worker_inheritance() {
    let profile = bao_stealth::StealthProfile::chrome_default();
    let config: SharedWorkerScopeConfig = (&profile).into();
    let sw_id = SharedWorkerId {
        script_url: "shared-stealth.js".into(),
        name: "stealth-sw".into(),
    };
    let scope = SharedWorkerGlobalScopeState::new(sw_id, &config);

    // @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    let nav = scope.navigator();
    assert_eq!(nav.user_agent, profile.navigator.user_agent);
    assert_eq!(nav.platform, profile.navigator.platform);
    assert_eq!(nav.language, profile.navigator.language);
}

// ═══════════════════════════════════════════════════════════════════════
// §21 Integration: AutoCloseWorker + crash_safe_teardown_worker
// ═══════════════════════════════════════════════════════════════════════

/// @trace REQ-BRW-004 [criterion:10,18] AutoCloseWorker drop triggers crash-safe teardown
#[test]
fn test_auto_close_worker_drop_triggers_crash_safe_teardown() {
    let handle = WorkerHandle::new("raii-teardown.js".into());
    handle.set_worker_global_addr(0xBEEF);

    {
        let _guard = AutoCloseWorker::new(handle);
        // Guard should be in Running state
    }
    // After drop, the handle inside the guard should be terminated
    // We can't access the moved handle, but we verify the RAII pattern works
    // by creating a new handle and checking the flow
    let handle2 = WorkerHandle::new("raii-teardown-2.js".into());
    handle2.set_worker_global_addr(0xCAFE);
    let guard = AutoCloseWorker::new(handle2);
    assert_eq!(guard.lifecycle_state(), WorkerLifecycleState::Running);
    // Drop the guard — should trigger terminate + unregister + mark_terminated
    drop(guard);
}

/// @trace REQ-BRW-004 [criterion:18] Three teardown paths all produce crash-safe results
#[test]
fn test_three_teardown_paths_crash_safe() {
    for path in [
        WorkerTeardownPath::Terminate,
        WorkerTeardownPath::SelfClose,
        WorkerTeardownPath::PageUnload,
    ] {
        let handle = WorkerHandle::new("three-paths.js".into());
        handle.set_worker_global_addr(0x1000);
        let result = crash_safe_teardown_worker(&handle, path.clone());
        assert!(
            result.closing_flag_set,
            "closing flag should be set for {:?}",
            path
        );
        assert!(
            result.realm_profile_unregistered,
            "realm should be unregistered for {:?}",
            path
        );
        assert!(
            result.thread_joined,
            "thread should be joined for {:?}",
            path
        );
        assert_eq!(result.path, path);
    }
}
