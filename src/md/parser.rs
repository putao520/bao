/// Parser error types.
///
/// In the original hand-written parser these corresponded to allocation failures
/// and stack overflow during recursive inline processing. With pulldown-cmark
/// as the engine, only `OutOfMemory` and `StackOverflow` are realistically
/// reachable (via renderer callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserError {
    OutOfMemory,
    JSError,
    JSTerminated,
    StackOverflow,
}

bun_core::oom_from_alloc!(ParserError);

impl From<ParserError> for bun_core::Error {
    fn from(_e: ParserError) -> Self {
        bun_core::err!(ParserError)
    }
}
