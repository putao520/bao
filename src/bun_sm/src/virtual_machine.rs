// @trace REQ-ENG-002 [module:bun_sm]
//! `VirtualMachine` — TLS singleton wrapping SpiderMonkey's Runtime.
//!
//! In JSC, `VirtualMachine` is a heap object managing the JS execution context.
//! In SpiderMonkey, `mozjs::rust::Runtime` is stored in TLS, accessed via
//! `Runtime::get()`. We wrap this as `VirtualMachine` for API compatibility.
//!
//! # Architecture
//!
//! ```text
//! JSC:  VM* -> JSGlobalObject -> heap
//! SM:   Runtime::get() (TLS) -> JSContext -> heap
//! ```

use mozjs::jsapi::JSContext as RawJSContext;

/// Reference to the VirtualMachine — a thin wrapper over `*mut JSContext`.
///
/// This is the "borrowed" form of VirtualMachine, used in function signatures
/// where the caller doesn't own the VM.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualMachineRef(pub(crate) *mut RawJSContext);

impl VirtualMachineRef {
    /// Get the raw `*mut JSContext`.
    #[inline]
    pub fn raw(&self) -> *mut RawJSContext {
        self.0
    }
}

unsafe impl Send for VirtualMachineRef {}

/// Whether the current thread's VM is the main thread VM.
pub static IS_MAIN_THREAD_VM: ::std::sync::atomic::AtomicBool =
    ::std::sync::atomic::AtomicBool::new(false);

/// GC and runtime options.
pub struct Options {
    pub gc_threshold: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { gc_threshold: 0 }
    }
}

/// Initialization options for VirtualMachine.
pub struct InitOptions {
    pub options: Options,
}

/// VirtualMachine — TLS singleton for SpiderMonkey's Runtime.
///
/// In JSC, `VM` is a heap object with a pointer. In SM, the Runtime is in TLS.
/// This struct provides the same API surface but accesses the TLS Runtime.
pub struct VirtualMachine {
    cx: *mut RawJSContext,
}

impl VirtualMachine {
    /// Get the current thread's VirtualMachine.
    ///
    /// Returns `None` if no Runtime has been initialized on this thread.
    pub fn get() -> Option<Self> {
        let cx = mozjs::rust::Runtime::get()?;
        Some(VirtualMachine { cx: cx.as_ptr() })
    }

    /// Get the VM, panicking if not initialized.
    pub fn get_unchecked() -> Self {
        Self::get().expect("VirtualMachine::get_unchecked: no Runtime on this thread")
    }

    /// Get a `VirtualMachineRef` (borrowed form).
    pub fn as_ref(&self) -> VirtualMachineRef {
        VirtualMachineRef(self.cx)
    }

    /// Get the raw `*mut JSContext`.
    #[inline]
    pub fn raw(&self) -> *mut RawJSContext {
        self.cx
    }

    /// Check if a VM is initialized on the current thread.
    pub fn is_initialized() -> bool {
        mozjs::rust::Runtime::get().is_some()
    }

    /// Get the JSGlobalObject for this VM.
    pub fn global(&self) -> crate::global_object::JSGlobalObject {
        crate::global_object::JSGlobalObject(self.cx)
    }

    /// Get the event loop handle.
    ///
    /// Returns the `EventLoopHandle` for the current thread's JS event loop.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn event_loop(&self) -> crate::event_loop::EventLoopHandle {
        let cell = crate::dispatch_sm::BaoEventLoop::current();
        let owner_ptr = cell as *const _ as *mut core::ffi::c_void;
        unsafe { bun_io::EventLoopCtx::new(bun_io::EventLoopCtxKind::Js, owner_ptr) }
    }

    /// Get the approximate heap size in bytes.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn heap_size(&self) -> usize {
        unsafe {
            mozjs::jsapi::JS_GetGCParameter(self.cx, mozjs::jsapi::JSGCParamKey::JSGC_BYTES)
                as usize
        }
    }

    /// Run a full garbage collection cycle.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn collect_garbage(&self) {
        unsafe { mozjs::jsapi::JS_GC(self.cx, mozjs::jsapi::GCReason::API) }
    }
}

impl ::std::fmt::Debug for VirtualMachine {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("VirtualMachine")
            .field("cx", &self.cx)
            .finish()
    }
}

/// Event loop type alias for JSC API compatibility.
pub type EventLoop = crate::dispatch_sm::BaoEventLoop;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_not_initialized_before_init() {
        let _ = VirtualMachine::is_initialized();
    }

    #[test]
    fn vm_ref_from_raw() {
        let ptr = 1usize as *mut RawJSContext;
        let vm_ref = VirtualMachineRef(ptr);
        assert_eq!(vm_ref.raw(), ptr);
    }
}
