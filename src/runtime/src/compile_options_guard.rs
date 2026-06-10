/// RAII guard for `mozjs::glue::NewCompileOptions` — calls `libc::free` on drop.
///
/// Every `NewCompileOptions` call must be paired with `libc::free(opts as *mut _)`.
/// This guard automates that, preventing leaks if early returns skip the free.
pub struct CompileOptionsGuard {
    ptr: *mut ::std::ffi::c_void,
}

impl CompileOptionsGuard {
    /// Create a guard from a raw CompileOptions pointer.
    /// Returns `None` if the pointer is null (NewCompileOptions failed).
    #[inline]
    pub fn new(ptr: *mut ::std::ffi::c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    /// Access the raw pointer for passing to `JS::Evaluate2` etc.
    #[inline]
    pub fn as_ptr(&self) -> *mut ::std::ffi::c_void {
        self.ptr
    }
}

impl Drop for CompileOptionsGuard {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { bun_sys::c::free(self.ptr); }
        }
    }
}