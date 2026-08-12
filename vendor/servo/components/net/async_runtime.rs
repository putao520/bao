/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use futures::Future;
use net_traits::AsyncRuntime;
use tokio::runtime::{Builder, Handle, Runtime};

/// The actual runtime,
/// to be used as part of shut-down.
pub struct AsyncRuntimeHolder {
    runtime: Option<Runtime>,
}

impl AsyncRuntimeHolder {
    pub(crate) fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Some(runtime),
        }
    }

    /// BAO PATCH (BCE-20260627-009): Create an empty holder (no owned runtime).
    /// Used when the runtime is process-global and leaked (never dropped) — the
    /// empty holder's shutdown is a no-op, so dropping it doesn't kill the
    /// process-global async runtime.
    pub(crate) fn new_empty() -> Self {
        Self { runtime: None }
    }
}

impl AsyncRuntime for AsyncRuntimeHolder {
    fn shutdown(&mut self) {
        // BAO PATCH (BCE-20260627-009): Handle empty holder (no runtime to shut down).
        // The process-global runtime is leaked and never drops; empty holders have no
        // owned runtime. The original expect() would panic on the second BaoRuntime
        // shutdown path (where the holder is empty because the runtime was leaked).
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_millis(100));
        }
    }
}

/// A shared handle to the runtime,
/// to be initialized on start-up.
static ASYNC_RUNTIME_HANDLE: OnceLock<Handle> = OnceLock::new();

pub fn init_async_runtime() -> Box<dyn AsyncRuntime> {
    // Initialize a tokio runtime.
    let runtime = Builder::new_multi_thread()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::Relaxed);
            format!("tokio-runtime-{}", id)
        })
        .worker_threads(
            thread::available_parallelism()
                .map(|i| i.get())
                .unwrap_or(servo_config::pref!(thread_pool_fallback_workers) as usize)
                .min(servo_config::pref!(thread_pool_async_runtime_workers_max).max(1) as usize),
        )
        .enable_io()
        .enable_time()
        .build()
        .expect("Unable to build tokio-runtime runtime");

    // Make the runtime available to users inside this crate.
    // BAO PATCH (BCE-20260627-009, root-caused BCE-20260628-001): Idempotent init —
    // supports multiple BaoRuntime instances (production multi-tenant + concurrent
    // integration tests) AND keeps the first runtime alive process-wide.
    //
    // Original servo: `ASYNC_RUNTIME_HANDLE.set(handle).expect("Runtime handle
    // should be initialized once on start-up")` panics on ANY re-init. servo is a
    // single-instance architecture: every `Servo::new` spawns a resource thread
    // (`resource_thread.rs:90`) that calls `init_async_runtime()`. Multiple
    // BaoRuntime instances (concurrent integration tests, multi-tenant production)
    // therefore collide on this OnceLock and panic.
    //
    // Idempotent semantics (matches the opts.rs BCE-20260627-009 fix):
    //   - First `init_async_runtime()` wins: installs the handle AND leaks the
    //     runtime so it lives for the process lifetime (matches the JSEngine
    //     forget strategy). The leaked runtime's worker pool keeps polling tasks
    //     spawned via `spawn_task`/`spawn_blocking_task`.
    //   - Subsequent calls: the OnceLock is already full → the NEW runtime is
    //     dropped (its threads shut down cleanly, no leak), and an empty holder
    //     is returned. `spawn_task`/`spawn_blocking_task` keep using the
    //     first-installed (leaked, still-live) handle.
    //
    // Root-cause note (BCE-20260628-001): a prior version of this function
    // returned early (`if ASYNC_RUNTIME_HANDLE.get().is_some() { return ... }`)
    // BEFORE calling `std::mem::forget(runtime)`. That made the `forget` dead
    // code on the first init, so the first runtime was dropped when this
    // function returned — leaving `ASYNC_RUNTIME_HANDLE` holding a Handle into a
    // DEAD runtime. `handle.spawn(task)` then silently succeeded but the task was
    // never polled (no worker threads alive), which broke the servo fetch path
    // (data: URL workers' fetch tasks never executed). The fix is to decide
    // first-vs-subsequent via the `set()` return value and leak the runtime ONLY
    // on the first (winning) init.
    let is_first_init = ASYNC_RUNTIME_HANDLE.set(runtime.handle().clone()).is_ok();

    if is_first_init {
        // First init: leak the runtime so it never drops (process-global lifetime).
        // The handle in ASYNC_RUNTIME_HANDLE keeps a strong reference; spawn_task
        // uses it. The worker pool stays alive polling tasks until process exit,
        // at which point the OS reclaims everything.
        std::mem::forget(runtime);
        log::debug!(
            "async_runtime handle installed (first init) — runtime leaked process-wide"
        );
    } else {
        // Already initialized by a prior BaoRuntime/Servo::new — idempotent skip.
        // Drop the new `runtime` here: its threads shut down cleanly (no leak),
        // and spawn_task/etc. keep using the first-installed (leaked, live) handle.
        log::debug!(
            "async_runtime handle already initialized — idempotent skip (BaoRuntime multi-instance)"
        );
    }

    // Always return an empty holder. The process-global runtime's lifetime is
    // managed by the leak above (first init) or by the drop here (subsequent
    // inits); the holder's Drop must NOT touch the process-global runtime (it is
    // owned by the OnceLock handle + leak, not by the holder).
    Box::new(AsyncRuntimeHolder::new_empty())
}

pub fn async_runtime_initialized() -> bool {
    ASYNC_RUNTIME_HANDLE.get().is_some()
}

/// Spawn a task using the handle to the runtime.
///
/// BAO PATCH (BCE-20260627-009): No-op if no async runtime is available.
/// In bao's multi-BaoRuntime model, a second BaoRuntime may have the handle
/// cleared (if the first one shut down). This is a transient state during
/// tear-down/re-init; logging the skip is sufficient. The caller must tolerate
/// the task not being spawned (for data: URL fetches, the task is the fetch
/// itself — if the runtime is down, the fetch silently falls through).
/// This prevents panics when a worker's fetch task arrives during the window
/// between the first BaoRuntime dropping its runtime and the second
/// BaoRuntime initializing its new one.
pub fn spawn_task<F>(task: F)
where
    F: Future + 'static + std::marker::Send,
    F::Output: Send + 'static,
{
    if let Some(handle) = ASYNC_RUNTIME_HANDLE.get() {
        handle.spawn(task);
    } else {
        log::warn!("async_runtime not available — task dropped (BaoRuntime multi-instance transient)");
    }
}

/// Spawn a blocking task using the handle to the runtime.
pub fn spawn_blocking_task<F, R>(task: F) -> F::Output
where
    F: Future,
{
    ASYNC_RUNTIME_HANDLE
        .get()
        .expect("Runtime handle should be initialized on start-up")
        .block_on(task)
}
