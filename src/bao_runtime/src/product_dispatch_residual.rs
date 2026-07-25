// @trace STUB-INVENTORY: product residual closed-set dispatch owners
//! Residual closed-set dispatch symbols for the **default product** path.
//!
//! These live in `bun_runtime` (not `bao_native_stubs`) so the product no longer
//! hard-deps or force-links the stubs crate. Variants already owned by
//! `bun_install` (`LifecycleScript` / `SecurityScan`) and CLI-linked real impls
//! are **excluded** (dual-def rule).
//!
//! Bodies come from `link_noop_*` until true type owners land full
//! `link_impl_*` / `impl_buffered_reader_parent!` (P0 backlog). Prefer real
//! owners over expanding this module — do not reintroduce a stubs hard-dep.

/// Ensure residual dispatch symbols are linked into product consumers.
/// Referenced from `lib.rs` via `#[used]` so the linker retains the unit.
#[inline(never)]
pub fn force_link_product_dispatch_residual() {
    // Symbol definitions are emitted by the macros below (no-op bodies).
    // This fn exists so a `#[used]` static can keep the compilation unit.
    let _ = force_link_product_dispatch_residual as *const () as usize;
}

// Product path does not link `bun_cli`; CLI variants stay residual here until
// either product always links CLI or closed-set splits (P1).
bun_io::link_noop_BufferedReaderParentLink!(
    SubprocessPipeReader,
    ShellPipeReader,
    ShellIoReader,
    FileReader,
    FileResponseStream,
    Terminal,
    CronRegister,
    CronRemove,
    FilterRunHandle,
    MultiRunPipeReader,
    TestParallelWorkerPipe
);

bun_spawn::link_noop_ProcessExit!(
    Subprocess,
    Shell,
    FilterRunHandle,
    MultiRunHandle,
    TestParallelWorker,
    CronRegister,
    CronRemove,
    ChromeProcess,
    HostProcess
);
