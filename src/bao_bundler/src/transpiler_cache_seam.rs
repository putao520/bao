// @trace REQ-ENG-006 [api:Bun.build CYCLEBREAK transpiler-cache seam] [req:REQ-ENG-006] [level:library]
//! SM-bridge `Jsc` arm for `bun_ast::TranspilerCacheImpl` — the dispatch
//! interface `bun_js_parser`/`bun_ast` reference from the parse path
//! (`RuntimeTranspilerCache::get/put/is_disabled`).
//!
//! Upstream the arm is implemented by the JSC on-disk transpiler cache
//! (`bun_jsc`). Bao has no on-disk transpiler cache, and nothing in Bao
//! ever sets `RuntimeTranspilerCache::r#impl = Some(Jsc)` (the slot stays
//! `None` ⇒ "caching disabled", the documented default), so these bodies
//! are link-time-only today. They still implement the honest disabled
//! semantics — `is_disabled() == true`, `get() == false` (never a fake
//! cache hit), `put()` stores nothing — so flipping the slot on later
//! without a real cache backend would fail loud, not silent.

/// Backing type for the `Jsc` arm (no state: cache disabled).
pub struct BaoTranspilerCacheImpl;

// Provide the `Jsc` arm of the TranspilerCacheImpl dispatch interface.
bun_ast::link_impl_TranspilerCacheImpl! {
    Jsc for BaoTranspilerCacheImpl => |this| {
        get(_source, _parser_options, _used_jsx) => {
            let _ = this;
            // No cache backend: never report a hit.
            false
        },
        put(_output_code, _sourcemap, _esm_record) => {
            let _ = this;
            // No cache backend: store nothing.
        },
        is_disabled() => {
            let _ = this;
            true
        },
    }
}
