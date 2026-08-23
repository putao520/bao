use bun_alloc::AllocError;

// upstream 066ae19465 — the `HandleOom` trait was ceremony: only the
// `Result<T, AllocError>` arm ever had callers (js_parser scan_imports), so it
// collapses to a plain function.
/// Unwraps a `Result<T, AllocError>`, converting OOM into the controlled
/// `bun.outOfMemory` crash.
pub fn handle_oom<T>(result: Result<T, AllocError>) -> T {
    match result {
        Ok(success) => success,
        Err(AllocError) => crate::out_of_memory(),
    }
}

// ported from: src/crash_handler/handle_oom.zig
