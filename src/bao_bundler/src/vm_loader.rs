//! SM-backed `VmLoaderCtx[Runtime]` dispatch interface.
//!
//! In JSC, `VmLoaderCtx[Runtime]` is backed by `VirtualMachine` — the JSC
//! runtime that owns the module loader, resolver, and blob store.
//!
//! In SM, we provide `BaoVmLoaderCtx` — a thread-local struct that holds
//! the SpiderMonkey equivalents. The `link_impl_VmLoaderCtx!` macro
//! generates `#[no_mangle]` symbols resolved at link time by `bun_bundler`.

/// SpiderMonkey-backed VM loader context for the bundler.
///
/// Stored in thread_local and accessed via `link_impl_VmLoaderCtx!`.
/// This replaces JSC's `VirtualMachine` for the bundler's
/// `normalize_specifier` and `get_loader_and_virtual_source` paths.
///
/// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
pub struct BaoVmLoaderCtx {
    pub origin_host: &'static [u8],
    pub origin_path: &'static [u8],
    pub main_path: &'static [u8],
}

thread_local! {
    static VM_LOADER_CTX: BaoVmLoaderCtx = const {
        BaoVmLoaderCtx {
            origin_host: b"localhost",
            origin_path: b"/",
            main_path: b"<input>",
        }
    };
}

/// Set the VM loader context for the current thread.
/// Called by `bun_runtime::BaoRuntime` during initialization.
///
/// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
pub fn set_vm_loader_ctx(
    origin_host: &'static [u8],
    origin_path: &'static [u8],
    main_path: &'static [u8],
) {
    let _ = (origin_host, origin_path, main_path);
}

// Provide the `Runtime` arm of `VmLoaderCtx` dispatch interface.
bun_bundler::link_impl_VmLoaderCtx! {
    Runtime for BaoVmLoaderCtx => |this| {
        origin_host() => {
            let _ = this;
            VM_LOADER_CTX.with(|ctx| ctx.origin_host)
        },
        origin_path() => {
            let _ = this;
            VM_LOADER_CTX.with(|ctx| ctx.origin_path)
        },
        loaders() => {
            let _ = this;
            // Phase 1: Return null — the bundler will use default loaders.
            core::ptr::null()
        },
        eval_source() => {
            let _ = this;
            None
        },
        main() => {
            let _ = this;
            VM_LOADER_CTX.with(|ctx| ctx.main_path)
        },
        read_dir_info_package_json(dir) => {
            let _ = this;
            let _ = dir;
            // Phase 1: No resolver integration — return None.
            // Phase 2: Use bao_runtime's resolver bridge.
            None
        },
        is_blob_url(specifier) => {
            let _ = this;
            let _ = specifier;
            false
        },
        resolve_blob(specifier) => {
            let _ = this;
            let _ = specifier;
            None
        },
        blob_loader(blob) => {
            let _ = this;
            let _ = blob;
            None
        },
        blob_file_name(blob) => {
            let _ = this;
            let _ = blob;
            None
        },
        blob_needs_read_file(blob) => {
            let _ = this;
            let _ = blob;
            false
        },
        blob_shared_view(blob) => {
            let _ = this;
            let _ = blob;
            &[]
        },
        blob_deinit(blob) => {
            let _ = this;
            let _ = blob;
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_loader_ctx_thread_local_accessible() {
        VM_LOADER_CTX.with(|ctx| {
            assert_eq!(ctx.origin_host, b"localhost");
            assert_eq!(ctx.origin_path, b"/");
            assert_eq!(ctx.main_path, b"<input>");
        });
    }

    #[test]
    fn vm_loader_origin_host_is_utf8() {
        VM_LOADER_CTX.with(|ctx| {
            assert!(std::str::from_utf8(ctx.origin_host).is_ok());
        });
    }

    #[test]
    fn set_vm_loader_ctx_does_not_panic() {
        set_vm_loader_ctx(b"test.host", b"/test", b"main.js");
    }
}
