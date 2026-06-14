//! mark_binding! macro — no-op in SM (SM uses explicit rooting).
//!
//! In JSC, `mark_binding!` is used to track bindings for GC mark-and-sweep.
//! In SpiderMonkey, we use explicit rooting (`rooted!`), so this is a no-op.

/// Mark a binding as active for GC safety. No-op in SM.
#[macro_export]
macro_rules! mark_binding {
    ($($arg:tt)*) => {
        // No-op: SM uses explicit rooting, not JSC's mark-and-sweep binding tracking
    };
}
