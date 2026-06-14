//! SM-backed `DevServerHandle[Bake]` dispatch interface.
//!
//! In JSC/Bun, `DevServerHandle[Bake]` is backed by `bake::DevServer` — the
//! development server that serves baked bundles with HMR.
//!
//! Bao does not yet have a `bake` implementation. We provide stub
//! implementations using `link_impl_DevServerHandle!` because
//! `link_noop_DevServerHandle!` cannot auto-generate defaults for
//! `Result<(), Error>` return types.
//!
//! Phase 2: Implement `bake::DevServer` for Bao with SM-backed HMR.

/// Placeholder DevServer for Bao — not yet implemented.
///
/// This ZST exists solely to satisfy the `link_impl_DevServerHandle!` macro.
/// All dispatch methods return no-op / error values.
pub struct BaoDevServer;

// Provide the `Bake` arm of `DevServerHandle` dispatch interface.
// All methods return no-op defaults or Err(Error) where the interface
// requires Result<(), Error> returns (link_noop_ can't auto-generate these).
bun_bundler::link_impl_DevServerHandle! {
    Bake for BaoDevServer => |this| {
        barrel_needed_exports() => core::ptr::null_mut(),
        log_for_resolution_failures(abs_path, graph) => {
            let _ = (abs_path, graph);
            core::ptr::null_mut()
        },
        finalize_bundle(bv2, result) => {
            let _ = (bv2, result);
            Err(bun_core::Error::from(bun_core::AllocError))
        },
        handle_parse_task_failure(err, graph, abs_path, log, bv2) => {
            let _ = (err, graph, abs_path, log, bv2);
            Err(bun_core::Error::from(bun_core::AllocError))
        },
        put_or_overwrite_asset(path, contents, content_hash) => {
            let _ = (path, contents, content_hash);
            Err(bun_core::Error::from(bun_core::AllocError))
        },
        track_resolution_failure(import_source, specifier, renderer, loader) => {
            let _ = (import_source, specifier, renderer, loader);
            Err(bun_core::Error::from(bun_core::AllocError))
        },
        is_file_cached(abs_path, side) => {
            let _ = (abs_path, side);
            None
        },
        asset_hash(abs_path) => {
            let _ = abs_path;
            None
        },
        current_bundle_start_data() => core::ptr::null_mut(),
        register_barrel_with_deferrals(path) => {
            let _ = path;
            Err(bun_core::Error::from(bun_core::AllocError))
        },
        register_barrel_export(barrel_path, alias) => {
            let _ = (barrel_path, alias);
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_server_impl_compiles() {
        // Verify the link_impl_DevServerHandle! macro generated valid symbols.
        let _ = BaoDevServer;
    }

    #[test]
    fn dev_server_handle_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<bun_bundler::dispatch::DevServerHandle>();
    }

    #[test]
    fn dev_server_handle_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<bun_bundler::dispatch::DevServerHandle>();
    }
}
