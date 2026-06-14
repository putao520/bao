//! SM-backed event loop types.
//!
//! `EventLoop` is `BaoEventLoop` (from `crate::dispatch_sm`), which wraps
//! a `MiniEventLoop<'static>` and adds SpiderMonkey JS context registration,
//! enter/exit depth tracking, and keepalive ref counting.
//!
//! `EventLoopHandle` is a type alias for `bun_io::EventLoopCtx` — the
//! by-value `{kind, owner}` tagged pointer used throughout `bun_io` for
//! dispatch through `link_impl_EventLoopCtx!`.

/// Event loop — delegates to BaoEventLoop.
pub type EventLoop = crate::dispatch_sm::BaoEventLoop;

/// Event loop handle — type alias matching `bun_jsc`'s `EventLoopHandle`.
///
/// In JSC, `EventLoopHandle` is `EventLoopCtx` (a by-value tagged pointer
/// `{kind, owner}`). We reuse the same type from `bun_io` since the dispatch
/// mechanism is engine-agnostic — `BaoEventLoop` provides the `Js` arm.
pub type EventLoopHandle = bun_io::EventLoopCtx;
