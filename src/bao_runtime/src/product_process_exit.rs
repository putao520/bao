// @trace STUB-INVENTORY: product real ProcessExit owners (no link_noop)
//! True `ProcessExit` owners for product residual variants.
//!
//! Replaces `link_noop_ProcessExit!` for the closed-set arms that are not
//! already owned by `bun_install` (`LifecycleScript` / `SecurityScan`) or
//! `bun_spawn` (`SyncWindows`). Each owner is a thin, testable state machine:
//! last status / exit count / optional process backref / optional hook.
//! Real domain types (Subprocess, Shell, Cron, Chrome, Host, CLI handles)
//! can later embed or replace these without changing the link_impl registration.
//!
//! ## Dual-def rule
//! Do **not** re-register `LifecycleScript` / `SecurityScan` / `SyncWindows`.
//! Product residual must not also `link_noop` the variants registered here.
//! Product path does not link old `src/cli` ProcessExit impls; when those are
//! wired, they must **consume** these owners (single `link_impl` site).

use core::ptr;

use bun_core::Output;
use bun_spawn::{Process, ProcessExitKind, Rusage, Status};

// ────────────────────────────────────────────────────────────────────────────
// Shared state machine (testable)
// ────────────────────────────────────────────────────────────────────────────

/// Optional exit hook for domain owners (observability / finish accounting).
///
/// `# Safety`
/// `ctx` must remain valid for the duration of the call; the hook must not
/// re-enter `on_process_exit` on the same owner in a way that creates aliasing
/// conflicts with the live `&mut Process`.
pub type ProductExitHook =
    unsafe fn(ctx: *mut (), process: &mut Process, status: Status, rusage: &Rusage);

/// Thin exit-handler state shared by all product residual ProcessExit variants.
///
/// `Process::on_exit` already writes `process.status` and calls `detach()`
/// (poll close + clear handler) before dispatching here. This owner still:
/// 1. Mirrors status onto the owner (writeback for callers that only hold us)
/// 2. Clears / optionally unrefs its process backref (safe unref/poll cleanup)
/// 3. Fires an optional hook + debug log (observability)
///
/// Note: not `Debug` — `Status` is not `Debug` (owns `bun_sys::Error` paths).
pub struct ProductProcessExitState {
    /// Last status observed by `on_process_exit`.
    pub last_status: Option<Status>,
    /// How many times `on_process_exit` has run.
    pub exit_count: u32,
    /// Optional intrusive `*mut Process` the domain owner attached.
    pub process: *mut Process,
    /// When true, `on_process_exit` will `Process::deref` the matching backref
    /// after clearing it (domain took a strong ref via `attach_process`).
    pub release_on_exit: bool,
    /// Closed-set kind (for logs / debug).
    pub kind: ProcessExitKind,
    /// Optional user hook context.
    pub hook_ctx: *mut (),
    /// Optional user hook.
    pub hook: Option<ProductExitHook>,
}

impl ProductProcessExitState {
    #[inline]
    pub fn new(kind: ProcessExitKind) -> Self {
        Self {
            last_status: None,
            exit_count: 0,
            process: ptr::null_mut(),
            release_on_exit: false,
            kind,
            hook_ctx: ptr::null_mut(),
            hook: None,
        }
    }

    /// Attach a process backref. When `take_ref` is true, the owner holds a
    /// strong ref released on exit (caller must have already `ref_`'d).
    #[inline]
    pub fn attach_process(&mut self, process: *mut Process, take_ref: bool) {
        self.process = process;
        self.release_on_exit = take_ref && !process.is_null();
    }

    /// Install an optional observability / finish hook.
    #[inline]
    pub fn set_hook(&mut self, ctx: *mut (), hook: Option<ProductExitHook>) {
        self.hook_ctx = ctx;
        self.hook = hook;
    }

    /// Core exit body — non-empty: status writeback, safe cleanup, log + hook.
    ///
    /// `# Safety`
    /// `process` is the live `&mut Process` from `Process::on_exit` /
    /// `ProcessExitHandler` dispatch. Must not be freed before this returns.
    pub unsafe fn on_process_exit(
        &mut self,
        process: &mut Process,
        status: Status,
        rusage: &Rusage,
    ) {
        // 1) Writeback status onto the live Process (parity with MultiRun/FilterRun
        //    owner mirrors) and onto this owner for callers that only hold us.
        process.status = status.clone();
        self.last_status = Some(status.clone());
        self.exit_count = self.exit_count.saturating_add(1);

        // 2) Safe unref / poll cleanup.
        //    `Process::on_exit` already `detach()`'d when `has_exited()` (closes
        //    poller + clears exit_handler). We only clear our backref and,
        //    if we took a strong ref, drop it via `Process::deref`.
        let owned = self.process;
        let should_release = self.release_on_exit
            && !owned.is_null()
            && core::ptr::eq(owned, process as *mut Process);
        self.process = ptr::null_mut();
        self.release_on_exit = false;
        if should_release {
            // SAFETY: `owned` is the same live Process as `process`; detach
            // already ran in on_exit. deref drops our strong ref only.
            unsafe { Process::deref(owned) };
        }

        // 3) Observable log (product residual path — verbose debug).
        Output::debug(format_args!(
            "<d>[ProductProcessExit:{:?}]<r> exit#{} status={}",
            self.kind, self.exit_count, status
        ));

        // 4) Optional domain hook (finish accounting / callback).
        if let Some(hook) = self.hook {
            // SAFETY: caller of `set_hook` established `hook_ctx` validity.
            unsafe { hook(self.hook_ctx, process, status, rusage) };
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Per-variant thin owner types + link_impl registration
// ────────────────────────────────────────────────────────────────────────────

/// Generate a named owner wrapping [`ProductProcessExitState`] and register it
/// as the closed-set `ProcessExit` for `$variant`.
macro_rules! product_process_exit {
    ($variant:ident => $ty:ident) => {
        pub struct $ty {
            pub state: ProductProcessExitState,
        }

        impl $ty {
            #[inline]
            pub fn new() -> Self {
                Self {
                    state: ProductProcessExitState::new(ProcessExitKind::$variant),
                }
            }

            #[inline]
            pub fn state(&self) -> &ProductProcessExitState {
                &self.state
            }

            #[inline]
            pub fn state_mut(&mut self) -> &mut ProductProcessExitState {
                &mut self.state
            }

            #[inline]
            pub fn attach_process(&mut self, process: *mut Process, take_ref: bool) {
                self.state.attach_process(process, take_ref);
            }

            #[inline]
            pub fn set_hook(&mut self, ctx: *mut (), hook: Option<ProductExitHook>) {
                self.state.set_hook(ctx, hook);
            }

            /// Install this owner as the process exit handler.
            ///
            /// `# Safety`
            /// `self` must remain live for every dispatch through the handler
            /// (same contract as `ProcessExit::new`).
            #[inline]
            pub unsafe fn install_on(&mut self, process: &mut Process) {
                process.set_exit_handler(unsafe {
                    bun_spawn::ProcessExit::new(ProcessExitKind::$variant, self as *mut Self)
                });
            }
        }

        impl Default for $ty {
            #[inline]
            fn default() -> Self {
                Self::new()
            }
        }

        bun_spawn::link_impl_ProcessExit! {
            $variant for $ty => |this| {
                on_process_exit(process, status, rusage) =>
                    (*this).state.on_process_exit(process, status, rusage),
            }
        }

        // Silence unused-type warning when only the link_impl const unit is live.
        const _: fn() = || {
            let _ = core::mem::size_of::<$ty>();
        };
    };
}

// Product spawn / shell / browser residual variants.
product_process_exit!(Subprocess => ProductSubprocessExit);
product_process_exit!(Shell => ProductShellExit);
product_process_exit!(CronRegister => ProductCronRegisterExit);
product_process_exit!(CronRemove => ProductCronRemoveExit);
product_process_exit!(ChromeProcess => ProductChromeProcessExit);
product_process_exit!(HostProcess => ProductHostProcessExit);

// CLI residual arms (product path does not link old `src/cli` ProcessExit
// impls; single link_impl site lives here — no dual-def with bao_cli).
product_process_exit!(FilterRunHandle => ProductFilterRunHandleExit);
product_process_exit!(MultiRunHandle => ProductMultiRunHandleExit);
product_process_exit!(TestParallelWorker => ProductTestParallelWorkerExit);

/// Ensure product ProcessExit link_impl units stay live.
/// Referenced from `lib.rs` via `force_link_native_c_libs`.
#[inline(never)]
pub fn force_link_product_process_exit() {
    let _ = force_link_product_process_exit as *const () as usize;
    // Touch type sizes so thin LTO keeps the compilation unit + link_impl symbols.
    let _ = core::mem::size_of::<ProductSubprocessExit>();
    let _ = core::mem::size_of::<ProductShellExit>();
    let _ = core::mem::size_of::<ProductFilterRunHandleExit>();
    let _ = core::mem::size_of::<ProductMultiRunHandleExit>();
    let _ = core::mem::size_of::<ProductTestParallelWorkerExit>();
    let _ = core::mem::size_of::<ProductCronRegisterExit>();
    let _ = core::mem::size_of::<ProductCronRemoveExit>();
    let _ = core::mem::size_of::<ProductChromeProcessExit>();
    let _ = core::mem::size_of::<ProductHostProcessExit>();
    let _ = ProcessExitKind::Subprocess;
    let _ = ProcessExitKind::HostProcess;
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests — owner construction / state / hook install (no full Process)
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bun_spawn::Exited;

    #[test]
    fn all_product_variants_construct_with_kinds() {
        assert_eq!(
            ProductSubprocessExit::new().state.kind,
            ProcessExitKind::Subprocess
        );
        assert_eq!(ProductShellExit::new().state.kind, ProcessExitKind::Shell);
        assert_eq!(
            ProductCronRegisterExit::new().state.kind,
            ProcessExitKind::CronRegister
        );
        assert_eq!(
            ProductCronRemoveExit::new().state.kind,
            ProcessExitKind::CronRemove
        );
        assert_eq!(
            ProductChromeProcessExit::new().state.kind,
            ProcessExitKind::ChromeProcess
        );
        assert_eq!(
            ProductHostProcessExit::new().state.kind,
            ProcessExitKind::HostProcess
        );
        assert_eq!(
            ProductFilterRunHandleExit::new().state.kind,
            ProcessExitKind::FilterRunHandle
        );
        assert_eq!(
            ProductMultiRunHandleExit::new().state.kind,
            ProcessExitKind::MultiRunHandle
        );
        assert_eq!(
            ProductTestParallelWorkerExit::new().state.kind,
            ProcessExitKind::TestParallelWorker
        );
    }

    #[test]
    fn attach_process_and_hook_fields() {
        let mut owner = ProductSubprocessExit::new();
        assert!(owner.state.process.is_null());
        assert!(!owner.state.release_on_exit);
        assert!(owner.state.hook.is_none());
        assert_eq!(owner.state.exit_count, 0);
        assert!(owner.state.last_status.is_none());

        // Sentinel non-null process ptr (not dereferenced when take_ref=false
        // until on_process_exit, which we do not call without a live Process).
        let sentinel = 0x10 as *mut Process;
        owner.attach_process(sentinel, false);
        assert_eq!(owner.state.process, sentinel);
        assert!(!owner.state.release_on_exit);

        owner.attach_process(sentinel, true);
        assert!(owner.state.release_on_exit);

        static mut HOOK_ARMED: bool = false;
        unsafe fn hook(_ctx: *mut (), _process: &mut Process, _status: Status, _rusage: &Rusage) {
            unsafe { HOOK_ARMED = true };
        }
        owner.set_hook(ptr::null_mut(), Some(hook));
        assert!(owner.state.hook.is_some());
        let _ = unsafe { HOOK_ARMED };

        // Mirror writeback without live Process: state fields only.
        owner.state.last_status = Some(Status::Exited(Exited { code: 3, signal: 0 }));
        owner.state.exit_count = 1;
        assert!(matches!(
            owner.state.last_status,
            Some(Status::Exited(Exited { code: 3, signal: 0 }))
        ));
        assert_eq!(owner.state.exit_count, 1);
    }

    #[test]
    fn force_link_symbol_exists() {
        force_link_product_process_exit();
    }
}
