//! EnsureStillAlive — no-op guard for SM (SM uses explicit rooting).
//!
//! In JSC, `EnsureStillAlive` prevents collection during a scope.
//! In SM, we use explicit rooting (`rooted!`), so this is a no-op.

/// GC safety guard. In JSC, this prevents collection during a scope.
/// In SM, it's a no-op since SM uses explicit rooting.
pub struct EnsureStillAlive;

impl EnsureStillAlive {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnsureStillAlive {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EnsureStillAlive {
    fn drop(&mut self) {
        // No-op in SM
    }
}
