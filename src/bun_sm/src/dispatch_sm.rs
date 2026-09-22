// @trace REQ-ENG-001 [entity:BaoEventLoop]
//! SpiderMonkey `Jsc`/`Js` arm adapters for `bun_dispatch::link_interface!`.
//!
//! Bao replaces JavaScriptCore with SpiderMonkey. The dispatch variants
//! `Jsc` (in `bun_event_loop::JsEventLoop[Jsc]`, `bun_ast::TranspilerCacheImpl[Jsc]`)
//! and `Js` (in `bun_io::EventLoopCtx[Js, Mini]`) are upstream implemented in
//! `bun_jsc`; Bao's `bao_engine` provides the SpiderMonkey equivalents here.
//!
//! # Backing store design
//!
//! `BaoEventLoop` is a per-thread single instance wrapping a
//! `bun_event_loop::MiniEventLoop<'static>`. The MiniEventLoop contributes
//! the uSockets event loop, task queue, pipe-read buffer, and file-poll
//! store — SpiderMonkey adds the JS context on top.
//!
//! Reusing MiniEventLoop is sound because dispatch dispatches on the
//! **variant tag** (`Js` vs `Mini`), not on the backing struct identity.
//! `EventLoopCtx[Js]` dispatch through `BaoEventLoop` while
//! `EventLoopCtx[Mini]` dispatch through `MiniEventLoop` directly — the two
//! arms don't share an owner pointer.
//!
//! # Lazy initialization
//!
//! `MiniEventLoop::init()` is non-const, so `BaoEventLoop` stores
//! `Option<MiniEventLoop>` and lazily materializes the inner on first
//! access. This mirrors JSC's `VirtualMachine::get()` lazy thread-local
//! pattern.
//!
//! # Variant naming
//!
//! We reuse the upstream `Jsc` / `Js` variant identifiers — the variant
//! label is a link-time symbol token, not an engine identity claim. Keeping
//! the upstream name minimizes diff vs. Bun and avoids touching the
//! interface declarations in low-tier crates.
//!
//! # Wave 73 sub-wave map
//!
//! - 73-A: this file — framework + `BaoEventLoop` skeleton [COMPLETED]
//! - 73-D: `EventLoopCtx[Js]` arm (`bun_io::link_impl_EventLoopCtx!`) — 11 methods [IN PROGRESS]
//! - 73-E: `JsEventLoop[Jsc]` arm (`bun_event_loop::link_impl_JsEventLoop!`) + `__bun_js_event_loop_current`
//! - 73-G: integration — bao_runtime drops hand-written `TimerHeap`
//! - 73-B/C/F: CANCELLED — `ProcessExit`/`OutOfMemoryHandler`/`VmLoaderCtx` have no Jsc variant

use core::cell::{Cell, RefCell};

use bun_event_loop::MiniEventLoop::MiniEventLoop;

/// Per-thread Bao event loop backing the `Js`/`Jsc` arm of every dispatch
/// interface in this module.
///
/// Wraps a `MiniEventLoop<'static>` (which owns the uSockets loop, task
/// queue, pipe-read buffer, file-poll store). SpiderMonkey's `JSContext*`
/// is owned by `crate::context::JsContext` and borrowed via thread-local
/// registration; it is not stored here.
///
/// # Lifetimes
///
/// Stored in a `thread_local!`; intentionally leaked on thread exit
/// (SpiderMonkey insists on controlled shutdown order: roots drained →
/// runtime dropped → engine dropped). `std::mem::forget` is the right tool.
pub struct BaoEventLoop {
    inner: RefCell<Option<MiniEventLoop<'static>>>,
    /// Reentrancy counter for `enter()` / `exit()` (JSC parity).
    enter_depth: Cell<u32>,
    /// KeepAlive ref count. When > 0, the event loop has outstanding async work
    /// and should not exit. Mirrors JSC's `m_pendingRefCount`.
    pending_unref_count: Cell<u32>,
    /// Whether auto-tick is enabled. When true, the event loop automatically
    /// ticks after each JS callback (draining microtasks + polling I/O).
    auto_tick_enabled: Cell<bool>,
    /// Registered SpiderMonkey `JSContext*`. Set by `register_js_context()`
    /// when `JsContext` is created on this thread. Null before registration.
    js_context: Cell<*mut core::ffi::c_void>,
}

impl BaoEventLoop {
    /// Const-initializable empty shell; the real `MiniEventLoop` is
    /// materialized lazily on first dispatch.
    const fn new() -> Self {
        Self {
            inner: const { RefCell::new(None) },
            enter_depth: const { Cell::new(0) },
            pending_unref_count: const { Cell::new(0) },
            auto_tick_enabled: const { Cell::new(false) },
            js_context: const { Cell::new(core::ptr::null_mut()) },
        }
    }

    /// Materialize the inner `MiniEventLoop` if it doesn't yet exist.
    /// Returns a `RefMut` guard; callers dispatch through it.
    fn ensure_inner(&self) -> core::cell::RefMut<'_, MiniEventLoop<'static>> {
        let mut guard = self.inner.borrow_mut();
        if guard.is_none() {
            *guard = Some(MiniEventLoop::init());
        }
        core::cell::RefMut::map(guard, |opt| opt.as_mut().expect("just initialized"))
    }

    /// Register a SpiderMonkey `JSContext*` on this thread's event loop.
    /// Called by `JsContext::init_runtime()` / `JsContext::for_test()`.
    pub fn register_js_context(cx: *mut core::ffi::c_void) {
        let cell = Self::current();
        cell.js_context.set(cx);
    }

    /// Thread-local accessor matching JSC's `VirtualMachine::get()` semantics.
    ///
    /// # Panics
    ///
    /// Panics if called before `JsContext::for_test()` registers the context —
    /// mirrors JSC's panic when no VM is installed on the current thread.
    /// Wave 73-E will add the registration check.
    #[inline]
    pub fn current() -> &'static BaoEventLoop {
        BAO_EVENT_LOOP.with(|cell: &BaoEventLoop| -> &'static BaoEventLoop {
            // SAFETY: BaoEventLoop has no Drop; once initialized the
            // thread_local lives until thread exit. We hand out a 'static
            // reference matching the bun_dispatch owner contract.
            unsafe { &*(cell as *const BaoEventLoop) }
        })
    }
}

thread_local! {
    static BAO_EVENT_LOOP: BaoEventLoop = const { BaoEventLoop::new() };
}

// ──────────────────────────────────────────────────────────────────────────
// Wave 73-D: `EventLoopCtx[Js]` arm — 11 methods.
//
// All bodies route through the lazy-initialized inner `MiniEventLoop`,
// which owns the uSockets loop, pipe-read buffer, file-poll store, and
// after-callback slot. SpiderMonkey adds the JS context on top in 73-E.
// ──────────────────────────────────────────────────────────────────────────

bun_io::link_impl_EventLoopCtx! {
    Js for BaoEventLoop => |this| {
        platform_event_loop_ptr() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            let ptr = guard.loop_ptr();
            let _ = this;
            ptr
        },
        file_polls_ptr() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            let inner_ptr: *mut MiniEventLoop<'static> =
                (&mut *guard) as *mut MiniEventLoop<'static>;
            drop(guard);
            unsafe { MiniEventLoop::file_polls_raw(inner_ptr) }
        },
        increment_pending_unref_counter() => {
            let cell = BaoEventLoop::current();
            let count = cell.pending_unref_count.get();
            cell.pending_unref_count.set(count.saturating_add(1));
            let _ = this;
        },
        ref_concurrently() => {
            let cell = BaoEventLoop::current();
            let count = cell.pending_unref_count.get();
            cell.pending_unref_count.set(count.saturating_add(1));
            let _ = this;
        },
        unref_concurrently() => {
            let cell = BaoEventLoop::current();
            let count = cell.pending_unref_count.get();
            cell.pending_unref_count.set(count.saturating_sub(1));
            let _ = this;
        },
        after_event_loop_callback() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            guard.after_event_loop_callback
        },
        set_after_event_loop_callback(cb, ctx) => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            guard.after_event_loop_callback = cb;
            guard.after_event_loop_callback_ctx = ctx;
        },
        pipe_read_buffer() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            core::ptr::from_mut::<[u8]>(guard.pipe_read_buffer())
        },
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Wave 73-E: `JsEventLoop[Jsc]` arm — 17 methods.
//
// Routes to lazy-init `MiniEventLoop<'static>` for the bulk of methods.
// SpiderMonkey-specific bits (`global_object`, `bun_vm`, JS task dispatch)
// land in Wave 73-G when bao_runtime registers its `JsContext` and starts
// routing tasks through dispatch.
//
// `__bun_js_event_loop_current` is the `#[no_mangle]` Rust-ABI symbol
// `bun_event_loop::JsEventLoop::current()` calls (see event_loop/lib.rs:84).
// It returns the thread-local `*mut BaoEventLoop`.
// ──────────────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_js_event_loop_current() -> *mut () {
    BaoEventLoop::current() as *const BaoEventLoop as *mut ()
}

bun_event_loop::link_impl_JsEventLoop! {
    Jsc for BaoEventLoop => |this| {
        iteration_number() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            // SAFETY: `loop_ptr` returns a live `*mut UwsLoop`; iteration_number
            // is a u64 counter on the loop struct.
            unsafe { (*guard.loop_ptr()).iteration_number() }
        },
        file_polls() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            let inner_ptr: *mut MiniEventLoop<'static> =
                (&mut *guard) as *mut MiniEventLoop<'static>;
            drop(guard);
            unsafe { MiniEventLoop::file_polls_raw(inner_ptr) }
        },
        put_file_poll(poll, was_ever_registered) => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            // SAFETY: `put_file_poll` requires a non-null `*mut FilePoll`;
            // the dispatch contract upholds this. `EventLoopCtx` is formed
            // from `&mut self` for the duration of the call.
            let store_ptr: *mut MiniEventLoop<'static> =
                (&mut *guard) as *mut MiniEventLoop<'static>;
            drop(guard);
            let store = unsafe { MiniEventLoop::file_polls_raw(store_ptr) };
            // SAFETY: dispatch contract — `poll` is a live hive-slot.
            let poll_nn = unsafe { core::ptr::NonNull::new_unchecked(poll) };
            // SAFETY: `EventLoopCtx::new` is unsafe; owner must be the live
            // thread-local BaoEventLoop pointer.
            let owner_ptr = BaoEventLoop::current() as *const BaoEventLoop as *mut ();
            let ctx = unsafe {
                bun_io::EventLoopCtx::new(bun_io::EventLoopCtxKind::Js, owner_ptr)
            };
            unsafe { (*store).put(poll_nn, ctx, was_ever_registered) };
        },
        uws_loop() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            guard.loop_ptr()
        },
        pipe_read_buffer() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            core::ptr::from_mut::<[u8]>(guard.pipe_read_buffer())
        },
        tick() => {
            let cell = BaoEventLoop::current();
            let cx = cell.js_context.get();
            let mut guard = cell.ensure_inner();
            guard.tick_once(cx);
            // Drain SpiderMonkey microtasks (Promise callbacks, queueMicrotask).
            if !cx.is_null() {
                unsafe {
                    mozjs::jsapi::js::RunJobs(cx as *mut mozjs::jsapi::JSContext);
                }
            }
        },
        auto_tick() => {
            let cell = BaoEventLoop::current();
            cell.auto_tick_enabled.set(true);
            let _ = this;
        },
        auto_tick_active() => {
            let cell = BaoEventLoop::current();
            cell.auto_tick_enabled.get();
            let _ = this;
        },
        global_object() => {
            let cell = BaoEventLoop::current();
            let cx = cell.js_context.get();
            if cx.is_null() {
                let _ = this;
                return core::ptr::null_mut();
            }
            unsafe {
                mozjs::jsapi::JS::CurrentGlobalOrNull(cx as *mut mozjs::jsapi::JSContext)
                    .cast()
            }
        },
        bun_vm() => {
            let cell = BaoEventLoop::current();
            cell.js_context.get().cast()
        },
        stdout() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            guard.stdout()
        },
        stderr() => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            guard.stderr()
        },
        enter() => {
            let cell = BaoEventLoop::current();
            let depth = cell.enter_depth.get();
            cell.enter_depth.set(depth.wrapping_add(1));
        },
        exit() => {
            let cell = BaoEventLoop::current();
            let depth = cell.enter_depth.get();
            if depth > 0 {
                cell.enter_depth.set(depth - 1);
            }
        },
        enqueue_task(task) => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            // `Task { tag, ptr }` is a tagged union of Taskable types. The
            // 73-G integration will dispatch on `task.tag` to route to the
            // appropriate bao_runtime callback. Until then, push the raw
            // pointer onto MiniEventLoop's task queue as an opaque work item.
            let task_ptr: *mut bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext =
                task.ptr.cast();
            // SAFETY: dispatch contract — `task.ptr` is a live Taskable
            // pointer valid until the next `tick` drains it.
            unsafe { guard.tasks.write_item(task_ptr).expect("task queue full") };
        },
        enqueue_task_concurrent(task) => {
            let cell = BaoEventLoop::current();
            let mut guard = cell.ensure_inner();
            // `task: NonNull<ConcurrentTask::ConcurrentTask>` — cast to the
            // underlying `AnyTaskWithExtraContext` (ConcurrentTask is a
            // wrapper type with an intrusive link at field offset 0).
            let any_task: core::ptr::NonNull<bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext> =
                task.cast();
            guard.enqueue_task_concurrent(any_task);
        },
        env() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            match guard.env {
                Some(nn) => nn.as_ptr(),
                None => core::ptr::null_mut(),
            }
        },
        top_level_dir() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            core::ptr::from_ref::<[u8]>(&*guard.top_level_dir)
        },
        create_null_delimited_env_map() => {
            let cell = BaoEventLoop::current();
            let guard = cell.ensure_inner();
            match guard.env {
                Some(nn) => {
                    // SAFETY: env loader is live for the dispatch call.
                    unsafe { (*nn.as_ptr()).map.create_null_delimited_env_map() }
                },
                None => Err(bun_core::AllocError),
            }
        },
    }
}

// ──────────────────────────────────────────────────────────────────────────
// W7 layer-3: `spawn_sync` event-loop face — the `__bun_spawn_sync_*` family.
//
// `bun_event_loop::SpawnSyncEventLoop` declares the family as `extern "Rust"`
// (link-time resolved; upstream bodies live in `bun_jsc`). Bao's SpiderMonkey
// owner provides them here, on BOTH targets: neither side had a provider (the
// declarations were orphaned until the spawn_sync wiring consumes them).
//
// Bao shape (js_current form — same as the child_process windows wiring):
// spawnSync binds to the per-thread `BaoEventLoop` singleton instead of
// heap-creating an isolated jsc::EventLoop, so:
//  - `destroy_event_loop` is an ownership no-op by design: the singleton is
//    thread_local and intentionally leaked on thread exit (see the
//    BaoEventLoop lifetime notes) — freeing it here would break every later
//    dispatch on the thread;
//  - `vm_swap_suppress_microtask_drain` maintains REAL per-thread state,
//    consumed by the microtask checkpoint face via
//    [`spawn_sync_microtask_drain_suppressed`].
// ──────────────────────────────────────────────────────────────────────────

thread_local! {
    /// spawnSync sets this for its tick scope so the microtask checkpoint
    /// holds the drain while the isolated spawn_sync loop runs.
    static SPAWN_SYNC_SUPPRESS_MICROTASK_DRAIN: Cell<bool> = const { Cell::new(false) };
}

/// `bun_event_loop::SpawnSyncEventLoop` destroy face. `el` is the erased
/// per-thread `BaoEventLoop` (js_current form) — thread_local-owned,
/// intentionally leaked on thread exit; there is nothing to reclaim and
/// freeing the singleton would strand every later dispatch on the thread.
/// Ownership no-op by design (NOT a silencing stub: the singleton leak is
/// BaoEventLoop's documented lifetime model).
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_destroy_event_loop(el: *mut ()) {
    // Validation only (debug): the pointer must be the live thread singleton,
    // never a foreign allocation. Only checked when non-null so a degenerate
    // call does not materialize the singleton as a side effect.
    debug_assert!(el.is_null() || el == BaoEventLoop::current() as *const BaoEventLoop as *mut ());
}

/// Swap the thread's spawnSync microtask-drain suppression, returning the
/// previous value (drives `SuppressMicrotaskDrain`'s RAII in
/// `SpawnSyncEventLoop`). The `vm` operand is the erased per-thread VM —
/// Bao's model is one VM per JS thread, so the suppression state is
/// thread-scoped (equivalent keying).
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_vm_swap_suppress_microtask_drain(
    _vm: *mut (),
    v: bool,
) -> bool {
    SPAWN_SYNC_SUPPRESS_MICROTASK_DRAIN.with(|flag| {
        let prev = flag.get();
        flag.set(v);
        prev
    })
}

/// Read face for the microtask checkpoint: `true` while a spawnSync scope is
/// suppressing the microtask drain on this thread (the spawn_sync wiring
/// consumes this to hold the drain while its isolated loop ticks).
pub fn spawn_sync_microtask_drain_suppressed() -> bool {
    SPAWN_SYNC_SUPPRESS_MICROTASK_DRAIN.with(Cell::get)
}


// ── spawn_sync event-loop faces (issue #18 W8) ─────────────────────────────
// The erased `*mut ()` is a heap `SpawnSyncEventLoopState` binding the VM to
// its isolated uWS loop (the spawnSync wait-loop contract: each wait
// iteration ticks the loop's tasks once; the VM's active event-loop handle
// is swapped to the isolated loop for the duration and restored after).
// The `vm_get/vm_set_event_loop_handle` slot is keyed by the erased VM
// pointer (the handle is `Option<NonNull<Loop>>` erased to `*mut ()`).

// `*mut ()` is not `Send`; the registry stores the pointer as `usize`
// (same bits — the handle is only dereferenced by the owning thread).
static SPAWN_SYNC_VM_EVENT_LOOP_HANDLES: ::std::sync::LazyLock<
    ::std::sync::Mutex<::std::collections::HashMap<usize, usize>>,
> = ::std::sync::LazyLock::new(|| ::std::sync::Mutex::new(::std::collections::HashMap::new()));

pub(crate) struct SpawnSyncState {
    pub vm: *mut (),
    pub uws_loop: *mut bun_uws::Loop,
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_create_event_loop(
    vm: *mut (),
    uws_loop: *mut bun_uws::Loop,
) -> *mut () {
    Box::into_raw(Box::new(SpawnSyncState { vm, uws_loop })) as *mut ()
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_event_loop_set_vm(el: *mut (), vm: *mut ()) {
    // SAFETY: el is the live heap state from create_event_loop.
    let state = unsafe { &mut *(el as *mut SpawnSyncState) };
    state.vm = vm;
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_event_loop_tick_tasks_only(el: *mut ()) {
    // Drain the isolated loop's pending tasks once: a single non-blocking
    // pump pass over the uWS loop (no I/O wait — the spawn-sync caller owns
    // the wait/timeout cadence).
    let state = unsafe { &*(el as *mut SpawnSyncState) };
    // SAFETY: uws_loop is the live loop handle from us_create_loop.
    unsafe {
        us_loop_pump(state.uws_loop);
    }
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_vm_get_event_loop_handle(vm: *mut ()) -> *mut () {
    SPAWN_SYNC_VM_EVENT_LOOP_HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(vm as usize))
        .copied()
        .unwrap_or(0) as *mut ()
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_spawn_sync_vm_set_event_loop_handle(vm: *mut (), h: *mut ()) {
    SPAWN_SYNC_VM_EVENT_LOOP_HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(vm as usize, h as usize);
}

unsafe extern "Rust" {
    fn us_loop_pump(loop_: *mut bun_uws::Loop);
}
