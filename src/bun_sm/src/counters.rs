//! Runtime counters — performance and event-loop metrics.
//!
//! In Bun/JSC, `Counters` tracks per-VM metrics (tasks completed, I/O events,
//! GC cycles, etc.). In SM, equivalent metrics are available through
//! `js::GetGCStats()` and the event-loop tick counter.

use mozjs::jsapi::*;

/// Runtime performance counters.
///
/// These are read-only snapshots of internal metrics. The actual counters
/// live in the event loop and are atomically updated.
#[derive(Debug, Default)]
pub struct Counters {
    pub tasks_completed: u64,
    pub io_events: u64,
    pub gc_cycles: u64,
    pub modules_loaded: u64,
    pub http_requests: u64,
}

impl Counters {
    /// Create a zeroed counter set.
    pub fn new() -> Self {
        Counters::default()
    }

    /// Snapshot the current counters from the event loop.
    /// Phase 1: returns default (zero) counters.
    pub fn snapshot() -> Self {
        Counters::default()
    }
}

/// Create a JS object with counter properties on the global object.
///
/// This is the SM equivalent of `bun_jsc::counters::create_counters_object`.
/// Phase 1: creates an empty JS plain object. Phase 2 will add actual
/// counter properties (tasks_completed, io_events, etc.).
///
/// # Safety
/// `cx` must be a valid JSContext.
pub unsafe fn create_counters_object(cx: *mut JSContext) -> *mut JSObject {
    if cx.is_null() {
        return ::std::ptr::null_mut();
    }
    unsafe { JS_NewPlainObject(cx) }
}
