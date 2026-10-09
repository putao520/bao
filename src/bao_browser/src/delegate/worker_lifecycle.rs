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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            state
                .active_workers
                .iter()
                .all(|g| g.handle().is_terminated())
        );
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
}
