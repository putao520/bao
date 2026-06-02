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
    /// Wave 73-E reads it; 73-G integration may decrement on `exit()` to
    /// trigger tear-down when depth reaches zero.
    enter_depth: Cell<u32>,
}

impl BaoEventLoop {
    /// Const-initializable empty shell; the real `MiniEventLoop` is
    /// materialized lazily on first dispatch.
    const fn new() -> Self {
        Self {
            inner: const { RefCell::new(None) },
            enter_depth: const { Cell::new(0) },
        }
    }

    /// Materialize the inner `MiniEventLoop` if it doesn't yet exist.
    /// Returns a `RefMut` guard; callers dispatch through it.
    fn ensure_inner(
        &self,
    ) -> core::cell::RefMut<'_, MiniEventLoop<'static>> {
        let mut guard = self.inner.borrow_mut();
        if guard.is_none() {
            *guard = Some(MiniEventLoop::init());
        }
        core::cell::RefMut::map(guard, |opt| {
            opt.as_mut().expect("just initialized")
        })
    }

    /// Thread-local accessor matching JSC's `VirtualMachine::get()` semantics.
    ///
    /// # Panics
    ///
    /// Panics if called before `JsContext::new()` registers the context —
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
            let _ = this;
            panic!("increment_pending_unref_counter: SpiderMonkey KeepAlive not wired until Wave 73-G");
        },
        ref_concurrently() => {
            let _ = this;
            panic!("ref_concurrently: SpiderMonkey KeepAlive not wired until Wave 73-G");
        },
        unref_concurrently() => {
            let _ = this;
            panic!("unref_concurrently: SpiderMonkey KeepAlive not wired until Wave 73-G");
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
            let mut guard = cell.ensure_inner();
            // No SpiderMonkey-driven tick callback yet (73-G integration).
            // Tick the uSockets loop with a null context.
            guard.tick(core::ptr::null_mut(), |_| false);
        },
        auto_tick() => {
            let _ = this;
            // SpiderMonkey auto-tick not wired (73-G integration).
        },
        auto_tick_active() => {
            let _ = this;
            // No SpiderMonkey auto-tick wiring (73-G integration).
        },
        global_object() => {
            let _ = this;
            // SpiderMonkey global object pointer — registered in 73-G when
            // bao_runtime JsContext wires its global into BaoEventLoop.
            core::ptr::null_mut()
        },
        bun_vm() => {
            let _ = this;
            // SpiderMonkey VM wrapper — registered in 73-G.
            core::ptr::null_mut()
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
                    let map = unsafe { (*nn.as_ptr()).map.create_null_delimited_env_map() };
                    map
                },
                None => Err(bun_core::AllocError),
            }
        },
    }
}
