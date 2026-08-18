//! SM implementation of `__bun_jsc_generate_cached_bytecode`.
//!
//! The JSC version generates JSC bytecode cache (`.jsc` files) off the main
//! thread. The SM equivalent will use XDR encode to produce SpiderMonkey
//! bytecode cache.
//!
//! Phase 1: returns `None` (no bytecode cache).
//! Phase 2: SM `XDREncode` to produce XDR bytecode.

/// Force-link anchor — called by `BAO_BUNDLER_ANCHOR` in lib.rs.
// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
// BCE (no_mangle name collision): namespaced — the former generic
// `__force_link_entry` collided with bao_native_stubs' public entry.
#[unsafe(no_mangle)]
extern "C" fn __force_link_entry_bao_bundler() {}

// ══════════════════════════════════════════════════════════════════════════
// CYCLEBREAK §Symbol: `__bun_jsc_generate_cached_bytecode`
//
// Declared as `safe fn` in `bun_bundler::bundle_v2::dispatch` via
// `unsafe extern "Rust"`. We provide the `#[no_mangle]` body here.
// ══════════════════════════════════════════════════════════════════════════

// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
#[unsafe(no_mangle)]
fn __bun_jsc_generate_cached_bytecode(
    _format: bun_options_types::Format,
    _source: &[u8],
    _source_provider_url: &mut bun_core::String,
) -> Option<Box<[u8]>> {
    // Phase 1: no SM bytecode cache. Return None so bundler emits source text.
    None
}

/// Test-only entry point to verify the symbol is linked.
// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
#[cfg(test)]
pub fn generate_cached_bytecode_for_test() -> Option<Box<[u8]>> {
    let mut url = bun_core::String::empty();
    __bun_jsc_generate_cached_bytecode(bun_options_types::Format::Esm, b"let x = 1;", &mut url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytecode_returns_none_phase1() {
        let mut url = bun_core::String::empty();
        let result = __bun_jsc_generate_cached_bytecode(
            bun_options_types::Format::Esm,
            b"let x = 1;",
            &mut url,
        );
        assert!(result.is_none());
    }

    #[test]
    fn bytecode_handles_empty_source() {
        let mut url = bun_core::String::empty();
        let result =
            __bun_jsc_generate_cached_bytecode(bun_options_types::Format::Esm, b"", &mut url);
        assert!(result.is_none());
    }

    #[test]
    fn bytecode_handles_all_formats() {
        for (format_name, format) in [
            ("Esm", bun_options_types::Format::Esm),
            ("Iife", bun_options_types::Format::Iife),
            ("Cjs", bun_options_types::Format::Cjs),
        ] {
            let mut url = bun_core::String::empty();
            let result = __bun_jsc_generate_cached_bytecode(format, b"x", &mut url);
            assert!(
                result.is_none(),
                "Phase 1 should return None for {}",
                format_name
            );
        }
    }

    #[test]
    fn bytecode_for_test_returns_none() {
        assert!(generate_cached_bytecode_for_test().is_none());
    }

    #[test]
    fn bytecode_handles_large_source() {
        let large_source = "let x = 1;\n".repeat(10000);
        let mut url = bun_core::String::empty();
        let result = __bun_jsc_generate_cached_bytecode(
            bun_options_types::Format::Esm,
            large_source.as_bytes(),
            &mut url,
        );
        assert!(result.is_none());
    }

    #[test]
    fn bytecode_handles_unicode_source() {
        let unicode_source = "const greeting = \"你好世界\";";
        let mut url = bun_core::String::empty();
        let result = __bun_jsc_generate_cached_bytecode(
            bun_options_types::Format::Esm,
            unicode_source.as_bytes(),
            &mut url,
        );
        assert!(result.is_none());
    }

    #[test]
    fn bytecode_force_link_anchor_exists() {
        // Verify the namespaced force-link symbol exists (used by BAO_BUNDLER_ANCHOR)
        let ptr = __force_link_entry_bao_bundler as *const ();
        assert!(!ptr.is_null());
    }
}
