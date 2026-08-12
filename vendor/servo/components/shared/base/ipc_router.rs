/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Thread-local RouterProxy infrastructure for Bao's per-instance IPC routing.
//!
//! The global `ipc_channel::router::ROUTER` is a process-wide singleton and
//! cannot be safely shut down and re-initialized. This module provides a
//! thread-local alternative so each BaoRuntime can own its own router instance.
//!
//! # Design Overview
//!
//! - Each `BaoRuntime` creates a per-instance `RouterProxy` in its `Constellation`
//! - The `Constellation` sets the thread-local router via `set_thread_router()`
//! - All `ROUTER.add_typed_route()` calls are migrated to use `router().add_typed_route()`
//! - If a thread never calls `set_thread_router()`, it falls back to the global `ROUTER`
//!
//! # Safety Guarantee (BCE-20260628-002根治)
//!
//! The thread-local stores a **raw pointer**, NOT an Arc. The Constellation is the
//! sole owner of the RouterProxy via its Arc. The thread_local is a lookup shortcut
//! with NO ownership, so thread exit (Drop RefCell<Option<*const>>) does NOT call
//! RouterProxy::drop, which would trigger router.shutdown and SIGSEGV if callbacks
//! are still in-flight. The OS reclaims memory on process exit; the leaked Arc
//! from `into_raw` is harmless because the Constellation's Arc is the real owner.

#![allow(unsafe_code)]

use ipc_channel::router::RouterProxy;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// Thread-local raw pointer to the per-instance RouterProxy.
    /// **NO OWNERSHIP** — just a lookup shortcut. The Constellation owns the RouterProxy.
    static THREAD_ROUTER: RefCell<Option<*const RouterProxy>> = const { RefCell::new(None) };
}

/// Set the current thread's per-instance RouterProxy.
///
/// Uses `Arc::into_raw` to leak the Arc (intentional — the thread_local has NO ownership).
/// The Constellation's Arc is the sole owner; thread exit does NOT call RouterProxy::drop.
pub fn set_thread_router(router: Arc<RouterProxy>) {
    // Convert Arc to raw pointer (increments strong count, does NOT drop).
    // We intentionally leak this Arc — the Constellation's Arc is the owner.
    let ptr: *const RouterProxy = Arc::into_raw(router);
    THREAD_ROUTER.with(|r| {
        *r.borrow_mut() = Some(ptr);
    });
}

/// Canonical router accessor: thread-local per-instance RouterProxy if set,
/// otherwise falls back to the process-global `ipc_channel::router::ROUTER`.
///
/// # Safety
///
/// Returns `'static` reference because:
/// 1. The RouterProxy is heap-allocated and owned by Constellation (Arc).
/// 2. The thread-local pointer is just a lookup shortcut, no ownership.
/// 3. The Constellation outlives all routes it registers (BaoRuntime lifetime).
pub fn router() -> &'static RouterProxy {
    THREAD_ROUTER.with(|r| {
        let ptr = *r.borrow();
        if let Some(p) = ptr {
            // Safety: p points to a heap-allocated RouterProxy owned by Constellation.
            // The Constellation lives for BaoRuntime's lifetime, which exceeds any
            // route registration or IPC callback. The pointer is valid until the
            // Constellation drops its Arc, which happens at BaoRuntime shutdown.
            return unsafe { &*p };
        }
        &ipc_channel::router::ROUTER
    })
}
