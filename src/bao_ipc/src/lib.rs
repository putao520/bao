//! @trace REQ-ARCH-3
//!
//! bao_ipc — Bao's unified MPMC message substrate (REQ-ARCH-3, wave F1).
//!
//! Single import point for in-process multi-producer / multi-consumer message
//! passing: F2 (bridge migration), F3 (channel hub) and F5 (PROCESS object)
//! import `bao_ipc::{channel, unbounded, Sender, Receiver, ...}` and never
//! touch `crossbeam_channel` directly, so the substrate stays swappable at
//! exactly one place.
//!
//! # Facade, not wrapper (design note)
//!
//! `Sender` / `Receiver` and the error types are a **transparent re-export**
//! of the in-tree `crossbeam-channel` 0.5 (reuse ruling 2026-10-15: the reuse
//! ladder forbids hand-rolling a lock-free MPMC queue when a mature one is
//! already in the lock graph). A newtype wrapper was rejected: it would
//! forward every method one-by-one (pure boilerplate, DRY violation), freeze
//! the usable API surface (`Select`, iteration, timeouts would each need
//! manual plumbing) and add zero semantic value — the "unified" part of this
//! crate is the constructor entry point (`channel` / `unbounded`), the
//! capacity / close / clone contract pinned by `tests/suite/channel_tests.rs`,
//! and the single import point, not an isolating layer.
//!
//! # Contract
//!
//! - **Bounded capacity**: `channel` caps the buffer at `cap`; `send` blocks
//!   while full (backpressure), `try_send` returns
//!   [`TrySendError::Full`]. `cap == 0` is a rendezvous channel (no
//!   buffering).
//! - **Close**: dropping every `Sender` (including clones) closes the
//!   channel; `recv` drains the remaining messages, then returns
//!   [`RecvError`]. Dropping every `Receiver` makes `send` return
//!   [`SendError`].
//! - **Clones**: `Sender` / `Receiver` are `Clone + Send + Sync`; clones
//!   share one queue and each message is delivered to exactly one receiver.

#![forbid(unsafe_code)]

mod channel;

pub use channel::{channel, unbounded};
// `Select` + `SelectedOperation` + `SelectTimeoutError`: the substrate's
// multi-channel wait face (REQ-ARCH-3 wave F2c pump tail — wake on whichever
// of two queues lands first). The facade doc above already cites `Select` as
// part of the intended usable surface; this completes the re-export.
pub use crossbeam_channel::{
    Receiver, RecvError, RecvTimeoutError, Select, SelectTimeoutError, SelectedOperation,
    SendError, SendTimeoutError, Sender, TryRecvError, TrySendError,
};
