//! Code coverage — Phase 1 no-op stub.

use std::sync::atomic::{AtomicBool, Ordering};

static IS_RUNNING: AtomicBool = AtomicBool::new(false);

pub struct CodeCoverage {
    _private: (),
}

impl CodeCoverage {
    pub fn start() -> bool {
        IS_RUNNING.store(true, Ordering::Relaxed);
        true
    }

    pub fn stop() -> bool {
        IS_RUNNING.store(false, Ordering::Relaxed);
        true
    }

    pub fn is_running() -> bool {
        IS_RUNNING.load(Ordering::Relaxed)
    }
}
