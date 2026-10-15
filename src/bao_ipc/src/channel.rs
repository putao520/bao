//! @trace REQ-ARCH-3
//!
//! Channel constructors for the unified MPMC substrate. Contract semantics
//! live on the crate root docs; discriminating tests live in
//! `tests/suite/channel_tests.rs`.

use crate::{Receiver, Sender};

/// Creates a bounded MPMC channel with capacity `cap`.
///
/// - `cap > 0`: at most `cap` messages buffer up; [`Sender::send`] blocks
///   while the buffer is full (backpressure), [`Sender::try_send`] returns
///   [`crate::TrySendError::Full`]. The backpressure face is pinned by the
///   tests on the deterministic `try_send` path; blocking `send` behaviour is
///   crossbeam's own contract.
/// - `cap == 0`: rendezvous channel — no buffering, `send` hands a message
///   over only when a receiver is ready.
///
/// The channel closes once every [`Sender`] (including clones) is dropped:
/// [`Receiver::recv`] then drains what is buffered and returns
/// [`crate::RecvError`].
pub fn channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    crossbeam_channel::bounded(cap)
}

/// Creates an unbounded MPMC channel: `send` never blocks for lack of
/// capacity (backpressure-free), memory grows with the backlog.
///
/// Close semantics are identical to [`channel`]: once every [`Sender`] is
/// dropped, [`Receiver::recv`] drains what is buffered and then returns
/// [`crate::RecvError`].
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    crossbeam_channel::unbounded()
}
