// @trace REQ-ENG-006 [api:Bun.build CYCLEBREAK macro seam] [req:REQ-ENG-006] [level:library]
//! SM-bridge CYCLEBREAK definitions for `bun_js_parser::Macro` — the macro
//! seam `bun_bundler`'s pipeline references at link time and calls on every
//! `Transpiler::init` (`transpiler.rs:417`, upstream `transpiler.zig:311`).
//!
//! Upstream the `#[no_mangle]` bodies live in `bun_js_parser_jsc` (the JSC
//! macro VM). Bao has no macro VM yet, so this bridge provides the honest
//! **no-macro state**: `init` returns a null-`data` `MacroContext` (the
//! caller's own null-checks then short-circuit remap lookups to `None`),
//! `call` fails closed with a build error (a macro import resolves to an
//! explicit error + log, never a crash or a silent passthrough), and the
//! garbage-collect hook is a no-op (no VM state to sweep). When a macro VM
//! lands, this module is replaced by the real runner — same symbols.

use bun_ast::{Expr, Log, Range, Source};
use bun_core::Error;
use bun_js_parser::Macro::{MacroContext, MacroJSCtx, MacroRemapEntry};

/// `MacroContext::init` — no macro runner in Bao: encode the null state.
///
/// The `transpiler` pointer is not dereferenced (nothing to read without a
/// macro VM); the returned context's `data == null` is the contract the
/// parser's own null-checks (`get_remap`) key off.
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_macro_context_init(_transpiler: *mut core::ffi::c_void) -> MacroContext {
    MacroContext {
        javascript_object: MacroJSCtx::ZERO,
        data: core::ptr::null_mut(),
    }
}

/// `MacroContext::deinit` — null `data` (the only state our `init` produces)
/// is a documented no-op.
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_macro_context_deinit(data: *mut core::ffi::c_void) {
    if data.is_null() {
        return;
    }
    // A non-null `data` can only come from a different definer — never ours.
    unreachable!("Bao macro bridge: deinit on a foreign MacroContext");
}

/// `MacroContext::call` — fail closed: no macro VM is linked, so a macro
/// invocation surfaces as an explicit build error (log + success:false),
/// never a crash nor a silent literal passthrough.
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_macro_context_call(
    _ctx: &mut MacroContext,
    _import_record_path: &[u8],
    _source_dir: &[u8],
    log: &mut Log,
    _source: &Source,
    _import_range: Range,
    _caller: Expr,
    _function_name: &[u8],
) -> Result<Expr, Error> {
    log.add_error(
        None,
        bun_ast::Loc::EMPTY,
        b"Macros are not supported in this Bao build",
    );
    Err(bun_core::err!("MacroNotSupported"))
}

/// `MacroContext::get_remap` — no macro remap table without a macro VM.
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_macro_context_get_remap(
    data: *mut core::ffi::c_void,
    _path: &[u8],
) -> Option<&'static MacroRemapEntry> {
    if data.is_null() {
        return None;
    }
    unreachable!("Bao macro bridge: remap lookup on a foreign MacroContext");
}

/// `Macro::collect_vm_garbage` — no macro VM state to sweep.
#[unsafe(no_mangle)]
pub extern "Rust" fn __bun_macro_collect_vm_garbage() {}
