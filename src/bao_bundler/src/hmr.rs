//! SM implementation of `__bun_jsc_enable_hot_module_reloading_for_bundler`.
//!
//! In JSC, this installs a `NewHotReloader<BundleV2, AnyEventLoop, true>`
//! watcher on the given `BundleV2`. The watcher monitors source files and
//! triggers re-bundling when changes are detected.
//!
//! Phase 1: No-op — HMR is not yet implemented for SM.
//! Phase 2: Use `bun_watcher` + SM module recompilation.

use core::ptr::NonNull;

#[unsafe(no_mangle)]
fn __bun_jsc_enable_hot_module_reloading_for_bundler(
    _bv2: NonNull<bun_bundler::BundleV2<'static>>,
) {
    // Phase 1: HMR not yet implemented for SM.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmr_symbol_is_callable() {
        let ptr = __bun_jsc_enable_hot_module_reloading_for_bundler as *const ();
        assert!(!ptr.is_null(), "HMR symbol must be linked");
    }

    #[test]
    fn hmr_symbol_has_correct_signature() {
        let _: fn(NonNull<bun_bundler::BundleV2<'static>>) =
            __bun_jsc_enable_hot_module_reloading_for_bundler;
    }
}
