// @trace STUB-INVENTORY: product real BufferedReaderParentLink owners (no link_noop)
//! True `BufferedReaderParentLink` owners for product residual variants.
//!
//! Replaces `link_noop_BufferedReaderParentLink!` for the closed-set arms that
//! are not already owned by `bun_install` (`LifecycleScript` / `SecurityScan`).
//! Each owner is a thin, testable state machine: buffer / done / last error /
//! optional event-loop backrefs. Real domain types (Subprocess, Shell, Cron, …)
//! can later embed or replace these without changing the link_impl registration.
//!
//! ## Dual-def rule
//! Do **not** re-register `LifecycleScript` / `SecurityScan` (live in `bun_install`).
//! Product residual must not also `link_noop` the variants registered here.

use core::ptr;
use core::ptr::NonNull;

use bun_io::max_buf::MaxBuf;
use bun_io::pipe_reader::{BufferedReaderParent, Loop};
use bun_io::{BufferedReaderParentLinkKind, EventLoopHandle, ReadState};
use bun_sys::Error as SysError;

// ────────────────────────────────────────────────────────────────────────────
// Shared state machine (testable)
// ────────────────────────────────────────────────────────────────────────────

/// Thin parent state shared by all product residual BufferedReader variants.
///
/// Callbacks update only these fields (disjoint from any embedded
/// `BufferedReader` the domain type may hold later).
pub struct ProductReaderState {
    /// Accumulated chunks when `HAS_ON_READ_CHUNK` is true.
    pub buffer: Vec<u8>,
    /// Set by `on_reader_done` / `on_reader_error` / max-buffer overflow.
    pub done: bool,
    /// Last error from `on_reader_error` (moved in).
    pub last_error: Option<SysError>,
    /// Subprocess max-buffer budget tripped.
    pub max_buffer_overflowed: bool,
    /// Native loop backref (`null` until wired by the domain owner).
    pub loop_ptr: *mut Loop,
    /// By-value event-loop handle (`EventLoopCtx::default()` = Mini + null).
    pub event_loop: EventLoopHandle,
}

impl Default for ProductReaderState {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            done: false,
            last_error: None,
            max_buffer_overflowed: false,
            loop_ptr: ptr::null_mut(),
            event_loop: EventLoopHandle::default(),
        }
    }
}

impl ProductReaderState {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wire a real Mini/Js event loop before attaching a live `BufferedReader`.
    #[inline]
    pub fn with_event_loop(mut self, loop_ptr: *mut Loop, event_loop: EventLoopHandle) -> Self {
        self.loop_ptr = loop_ptr;
        self.event_loop = event_loop;
        self
    }

    #[inline]
    pub fn on_read_chunk(&mut self, chunk: &[u8], _has_more: ReadState) -> bool {
        self.buffer.extend_from_slice(chunk);
        true
    }

    #[inline]
    pub fn on_reader_done(&mut self) {
        self.done = true;
    }

    #[inline]
    pub fn on_reader_error(&mut self, err: SysError) {
        self.last_error = Some(err);
        self.done = true;
    }

    #[inline]
    pub fn on_max_buffer_overflow(&mut self, _maxbuf: NonNull<MaxBuf>) {
        self.max_buffer_overflowed = true;
        self.done = true;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Per-variant thin owner types + link_impl registration
// ────────────────────────────────────────────────────────────────────────────

/// Owner type + common constructors for a product residual parent.
macro_rules! define_parent_type {
    ($ty:ident) => {
        pub struct $ty {
            pub state: ProductReaderState,
        }

        impl Default for $ty {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $ty {
            #[inline]
            pub fn new() -> Self {
                Self {
                    state: ProductReaderState::new(),
                }
            }

            #[inline]
            pub fn with_event_loop(
                mut self,
                loop_ptr: *mut Loop,
                event_loop: EventLoopHandle,
            ) -> Self {
                self.state = self.state.with_event_loop(loop_ptr, event_loop);
                self
            }

            #[inline]
            pub fn state(&self) -> &ProductReaderState {
                &self.state
            }

            #[inline]
            pub fn state_mut(&mut self) -> &mut ProductReaderState {
                &mut self.state
            }
        }
    };
}

// ── Streaming parents (Zig declares onReadChunk) ───────────────────────────

define_parent_type!(SubprocessPipeReaderParent);
bun_io::impl_buffered_reader_parent! {
    SubprocessPipeReader for SubprocessPipeReaderParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
    on_max_buffer_overflow = |this, maxbuf| {
        (*this).state.on_max_buffer_overflow(maxbuf);
    };
}

define_parent_type!(ShellPipeReaderParent);
bun_io::impl_buffered_reader_parent! {
    ShellPipeReader for ShellPipeReaderParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(ShellIoReaderParent);
bun_io::impl_buffered_reader_parent! {
    ShellIoReader for ShellIoReaderParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(FileReaderParent);
bun_io::impl_buffered_reader_parent! {
    FileReader for FileReaderParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(FileResponseStreamParent);
bun_io::impl_buffered_reader_parent! {
    FileResponseStream for FileResponseStreamParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

// Arms for handles from the deleted `src/cli` port (canonical parents kept).
define_parent_type!(FilterRunHandleParent);
bun_io::impl_buffered_reader_parent! {
    FilterRunHandle for FilterRunHandleParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(MultiRunPipeReaderParent);
bun_io::impl_buffered_reader_parent! {
    MultiRunPipeReader for MultiRunPipeReaderParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(TestParallelWorkerPipeParent);
bun_io::impl_buffered_reader_parent! {
    TestParallelWorkerPipe for TestParallelWorkerPipeParent;
    has_on_read_chunk = true;
    on_read_chunk   = |this, chunk, has_more| (*this).state.on_read_chunk(chunk, has_more);
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

// ── Done/error only (no onReadChunk in Zig parents) ────────────────────────

define_parent_type!(TerminalParent);
bun_io::impl_buffered_reader_parent! {
    Terminal for TerminalParent;
    has_on_read_chunk = false;
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(CronRegisterParent);
bun_io::impl_buffered_reader_parent! {
    CronRegister for CronRegisterParent;
    has_on_read_chunk = false;
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

define_parent_type!(CronRemoveParent);
bun_io::impl_buffered_reader_parent! {
    CronRemove for CronRemoveParent;
    has_on_read_chunk = false;
    on_reader_done  = |this| (*this).state.on_reader_done();
    on_reader_error = |this, err| (*this).state.on_reader_error(err);
    loop_           = |this| (*this).state.loop_ptr;
    event_loop      = |this| (*this).state.event_loop;
}

/// Ensure product BufferedReaderParentLink link_impl units stay live.
/// Referenced from `lib.rs` via `force_link_native_c_libs`.
#[inline(never)]
pub fn force_link_product_buffered_reader() {
    let _ = force_link_product_buffered_reader as *const () as usize;
    // Touch KIND constants so trait impls are considered used by thin LTO.
    let _ = <SubprocessPipeReaderParent as BufferedReaderParent>::KIND;
    let _ = <ShellPipeReaderParent as BufferedReaderParent>::KIND;
    let _ = <ShellIoReaderParent as BufferedReaderParent>::KIND;
    let _ = <FileReaderParent as BufferedReaderParent>::KIND;
    let _ = <FileResponseStreamParent as BufferedReaderParent>::KIND;
    let _ = <TerminalParent as BufferedReaderParent>::KIND;
    let _ = <CronRegisterParent as BufferedReaderParent>::KIND;
    let _ = <CronRemoveParent as BufferedReaderParent>::KIND;
    let _ = <FilterRunHandleParent as BufferedReaderParent>::KIND;
    let _ = <MultiRunPipeReaderParent as BufferedReaderParent>::KIND;
    let _ = <TestParallelWorkerPipeParent as BufferedReaderParent>::KIND;
    let _ = BufferedReaderParentLinkKind::SubprocessPipeReader;
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests — done/error/chunk state machine
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bun_sys::{E, Tag};

    fn sample_err() -> SysError {
        SysError::from_code(E::EIO, Tag::read)
    }

    #[test]
    fn subprocess_done_sets_flag() {
        let mut p = SubprocessPipeReaderParent::new();
        assert!(!p.state.done);
        unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::on_reader_done(&mut p as *mut _);
        }
        assert!(p.state.done);
        assert!(p.state.last_error.is_none());
    }

    #[test]
    fn subprocess_error_sets_error_and_done() {
        let mut p = SubprocessPipeReaderParent::new();
        let err = sample_err();
        let errno = err.errno;
        unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::on_reader_error(
                &mut p as *mut _,
                err,
            );
        }
        assert!(p.state.done);
        assert_eq!(p.state.last_error.as_ref().map(|e| e.errno), Some(errno));
    }

    #[test]
    fn subprocess_read_chunk_buffers() {
        let mut p = SubprocessPipeReaderParent::new();
        assert!(SubprocessPipeReaderParent::HAS_ON_READ_CHUNK);
        let cont = unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::on_read_chunk(
                &mut p as *mut _,
                b"hello",
                ReadState::Progress,
            )
        };
        assert!(cont);
        assert_eq!(p.state.buffer, b"hello");
        let cont = unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::on_read_chunk(
                &mut p as *mut _,
                b" world",
                ReadState::Eof,
            )
        };
        assert!(cont);
        assert_eq!(p.state.buffer, b"hello world");
    }

    #[test]
    fn subprocess_max_buffer_overflow_flags() {
        let mut p = SubprocessPipeReaderParent::new();
        // NonNull with dangling is OK — callback only sets flags, never derefs.
        let dangling = NonNull::dangling();
        unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::on_max_buffer_overflow(
                &mut p as *mut _,
                dangling,
            );
        }
        assert!(p.state.max_buffer_overflowed);
        assert!(p.state.done);
    }

    #[test]
    fn terminal_done_error_no_chunk() {
        assert!(!TerminalParent::HAS_ON_READ_CHUNK);
        let mut p = TerminalParent::new();
        unsafe {
            <TerminalParent as BufferedReaderParent>::on_reader_done(&mut p as *mut _);
        }
        assert!(p.state.done);

        let mut p = TerminalParent::new();
        unsafe {
            <TerminalParent as BufferedReaderParent>::on_reader_error(
                &mut p as *mut _,
                sample_err(),
            );
        }
        assert!(p.state.done);
        assert!(p.state.last_error.is_some());
    }

    #[test]
    fn cron_register_remove_state() {
        let mut reg = CronRegisterParent::new();
        let mut rem = CronRemoveParent::new();
        unsafe {
            <CronRegisterParent as BufferedReaderParent>::on_reader_done(&mut reg as *mut _);
            <CronRemoveParent as BufferedReaderParent>::on_reader_error(
                &mut rem as *mut _,
                sample_err(),
            );
        }
        assert!(reg.state.done);
        assert!(rem.state.done);
        assert!(rem.state.last_error.is_some());
        assert!(!CronRegisterParent::HAS_ON_READ_CHUNK);
        assert!(!CronRemoveParent::HAS_ON_READ_CHUNK);
    }

    #[test]
    fn shell_and_file_stream_chunks() {
        let mut shell = ShellPipeReaderParent::new();
        let mut shell_io = ShellIoReaderParent::new();
        let mut file = FileReaderParent::new();
        let mut fres = FileResponseStreamParent::new();
        unsafe {
            <ShellPipeReaderParent as BufferedReaderParent>::on_read_chunk(
                &mut shell as *mut _,
                b"a",
                ReadState::Progress,
            );
            <ShellIoReaderParent as BufferedReaderParent>::on_read_chunk(
                &mut shell_io as *mut _,
                b"b",
                ReadState::Progress,
            );
            <FileReaderParent as BufferedReaderParent>::on_read_chunk(
                &mut file as *mut _,
                b"c",
                ReadState::Progress,
            );
            <FileResponseStreamParent as BufferedReaderParent>::on_read_chunk(
                &mut fres as *mut _,
                b"d",
                ReadState::Eof,
            );
            <ShellPipeReaderParent as BufferedReaderParent>::on_reader_done(&mut shell as *mut _);
        }
        assert_eq!(shell.state.buffer, b"a");
        assert_eq!(shell_io.state.buffer, b"b");
        assert_eq!(file.state.buffer, b"c");
        assert_eq!(fres.state.buffer, b"d");
        assert!(shell.state.done);
    }

    #[test]
    fn cli_residual_variants_state() {
        let mut filter = FilterRunHandleParent::new();
        let mut multi = MultiRunPipeReaderParent::new();
        let mut worker = TestParallelWorkerPipeParent::new();
        unsafe {
            <FilterRunHandleParent as BufferedReaderParent>::on_read_chunk(
                &mut filter as *mut _,
                b"f",
                ReadState::Progress,
            );
            <MultiRunPipeReaderParent as BufferedReaderParent>::on_reader_done(
                &mut multi as *mut _,
            );
            <TestParallelWorkerPipeParent as BufferedReaderParent>::on_reader_error(
                &mut worker as *mut _,
                sample_err(),
            );
        }
        assert_eq!(filter.state.buffer, b"f");
        assert!(multi.state.done);
        assert!(worker.state.done);
        assert!(worker.state.last_error.is_some());
    }

    #[test]
    fn kind_constants_match_variants() {
        assert_eq!(
            SubprocessPipeReaderParent::KIND,
            BufferedReaderParentLinkKind::SubprocessPipeReader
        );
        assert_eq!(
            ShellPipeReaderParent::KIND,
            BufferedReaderParentLinkKind::ShellPipeReader
        );
        assert_eq!(TerminalParent::KIND, BufferedReaderParentLinkKind::Terminal);
        assert_eq!(
            MultiRunPipeReaderParent::KIND,
            BufferedReaderParentLinkKind::MultiRunPipeReader
        );
    }

    #[test]
    fn loop_and_event_loop_passthrough() {
        let mut p = SubprocessPipeReaderParent::new();
        // Default: null loop + default EventLoopCtx.
        let lp = unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::loop_(&mut p as *mut _)
        };
        assert!(lp.is_null());
        let _ev = unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::event_loop(&mut p as *mut _)
        };

        // Wire a non-null sentinel (not dereferenced by the parent trait).
        let sentinel = 0x1 as *mut Loop;
        p.state.loop_ptr = sentinel;
        let lp2 = unsafe {
            <SubprocessPipeReaderParent as BufferedReaderParent>::loop_(&mut p as *mut _)
        };
        assert_eq!(lp2, sentinel);
    }
}
