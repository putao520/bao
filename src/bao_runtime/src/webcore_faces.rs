//! Link-time faces for `event_loop::MiniEventLoop`'s `unsafe extern "Rust"`
//! upward contracts (`__bun_stdio_blob_store_new` / `__bun_js_vm_get`).
//!
//! Zig has no crate split here — `MiniEventLoop` reached `webcore::Blob`'s
//! `Store` and `jsc.VirtualMachine.get()` directly. The crate split moves the
//! bodies into bun_runtime (which owns the runtime objects); the link resolves
//! the bare-name Rust-ABI symbols.
//!
//! ## `__bun_stdio_blob_store_new` — the console stdio Blob.Store
//!
//! Upstream (rare_data.zig:551) constructs a `Blob.Store` for each of
//! stdout/stderr/stdin with an intrusive refcount of **2**: one ref for the
//! event-loop slot that forwards writes, one for the runtime-side holder.
//! The minimal real store carries exactly what the console-output contract
//! consumes (the fd, its atty-ness and the posix mode bits) under that same
//! refcount discipline; destruction happens when the count reaches 0 via
//! [`StdioBlobStore::release`].
//!
//! ## `__bun_js_vm_get`
//!
//! SM's VM is the thread's `Runtime`/`JSContext` (TLS, via
//! `mozjs::rust::Runtime::get()`) — the accessor erases it to `*mut ()`.
//! Null = no Runtime on this thread (the caller's documented
//! wrong-thread/degraded case).

use core::cell::Cell;

use bun_sys::{Fd, Mode};

/// The console-output `Blob.Store` equivalent (see the module doc).
#[repr(C)]
pub struct StdioBlobStore {
    /// Intrusive refcount — constructed at 2 (upstream Blob.Store ctor
    /// contract: the event-loop slot + the runtime-side holder).
    ref_count: Cell<u32>,
    /// The stdio descriptor this store buffers for.
    fd: Fd,
    /// Whether the descriptor is a terminal.
    is_atty: bool,
    /// The posix mode bits captured at construction (fstat face).
    mode: Mode,
}

impl StdioBlobStore {
    /// Add one reference (upstream `Blob.Store::ref`).
    pub fn ref_(&self) {
        self.ref_count.set(self.ref_count.get() + 1);
    }

    /// Drop one reference; frees the store at 0 (upstream `Blob.Store::deref`).
    pub fn release(this: &Self) {
        let remaining = this.ref_count.get().saturating_sub(1);
        this.ref_count.set(remaining);
        if remaining == 0 {
            // SAFETY: the store is heap-allocated by [`__bun_stdio_blob_store_new`]
            // and this is the last reference.
            unsafe { drop(Box::from_raw(this as *const Self as *mut StdioBlobStore)) };
        }
    }
}

/// Constructs the stdio `Blob.Store` (intrusive refcount = 2).
///
/// # Safety (link contract)
/// By-value args; allocates fresh. The returned pointer is erased and only
/// forwarded — destruction goes through the runtime-side refcount.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_stdio_blob_store_new(fd: Fd, is_atty: bool, mode: Mode) -> *mut () {
    let store = Box::new(StdioBlobStore {
        ref_count: Cell::new(2),
        fd,
        is_atty,
        mode,
    });
    Box::into_raw(store) as *mut ()
}

/// Returns the thread's VM pointer — SM's TLS `Runtime`/`JSContext` erased
/// (`jsc.VirtualMachine.get()` face). Null = no Runtime on this thread.
///
/// # Safety (link contract)
/// Reads the thread-local; wrong-thread is a logic error, not UB.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_js_vm_get() -> *mut () {
    use mozjs::rust::Runtime;
    Runtime::get()
        .map_or(::std::ptr::null_mut(), |runtime| runtime.as_ptr() as *mut ())
}

/// Test-binary link anchor: references the two soft-link faces so lower-tier
/// lib-test binaries that consume them via `bun_event_loop` resolve them at
/// link time. GNU ld tolerates undefined symbols in test executables;
/// lld-link/COFF does not (twin pattern: `bao_bundler::force_link_test_seams`).
pub fn force_link_webcore_faces() {
    let vm_get: unsafe extern "Rust" fn() -> *mut () = __bun_js_vm_get;
    let store_new: unsafe extern "Rust" fn(Fd, bool, Mode) -> *mut () =
        __bun_stdio_blob_store_new;
    ::std::hint::black_box((vm_get, store_new));
}
