// Regression pair for the upstream fixes absorbed into src/io/PipeReader.rs:
//
// - ada2a67ef  "HTMLRewriter: don't read a streamed input ahead of its reader"
//   → PipeReader half: `READ_SCRATCH_IN_USE` scratch-claim — a nested read
//   frame (e.g. `FileReader.on_pull` re-entering `read()` from inside the
//   outer frame's `on_read_chunk` dispatch) must not refill the per-loop
//   scratch under the outer one; it falls to its `_buffer` branch.
// - 0b041cba1  "PipeReader: don't re-deliver streamed bytes after a
//   re-entrant read" — the `_buffer` branch dispatches only the newest slice,
//   so the reinstall must keep only what re-entry buffered (clear first) and
//   the scratch/`_buffer` choice must key on `is_empty()` so one nested pull
//   doesn't permanently demote the reader to 16 KB buffered reads. Pre-fix,
//   the delivered slice stayed installed and the final HUP drain handed the
//   retained bytes to `on_reader_done` a second time.
//
// The dispatch externs for `TestParallelWorkerPipe` / `Mini` are registered
// here: no other registration is linked into this test binary (the product
// ones live in `bao_runtime`/`bun_install`), and the dispatcher's `match kind`
// arms resolve the `no_mangle` symbols this file defines.
#[cfg(unix)]
mod unix_tests {
    use bun_io::pipe_reader::{BufferedReaderParent, Loop, PosixBufferedReader, PosixFlags};
    use bun_io::pipes::PollOrFd;
    use bun_io::{EventLoopCtx, EventLoopCtxKind};
    use bun_sys::Fd;

    // `bun_core::dump_current_stack_trace` routes through this link-time
    // symbol, normally provided by `bun_crash_handler` in product binaries.
    // This test binary doesn't link that crate — stub it the same way
    // bun_core's own cfg(test) build does.
    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_crash_handler_dump_stack_trace(
        _first_address: Option<usize>,
        _limits: bun_core::DumpStackTraceOptions,
    ) {
    }

    // Same story for `bun_io::get_vm_ctx`'s link-time hook (normally installed
    // by `bun_runtime::init()`). Off the tested path; hand back a Mini ctx
    // over a leaked TestLoop so any stray caller still gets a live owner.
    #[unsafe(no_mangle)]
    extern "Rust" fn __bun_get_vm_ctx(_kind: bun_io::AllocatorType) -> bun_io::EventLoopCtx {
        let owner: &'static mut TestLoop = Box::leak(Box::new(TestLoop));
        // SAFETY: `owner` is a leaked, never-freed TestLoop — the ctx's
        // dispatch contract (owner live for every dispatch) always holds.
        unsafe { EventLoopCtx::new(EventLoopCtxKind::Mini, core::ptr::from_mut(owner)) }
    }

    // ── minimal EventLoopCtx owner ─────────────────────────────────────────
    //
    // Only `pipe_read_buffer` is reachable on the code paths below; the poll
    // paths that would touch `platform_event_loop_ptr`/`file_polls_ptr` are
    // never entered (the frames end at EOF or a consumer pause, never at
    // `register_poll`).
    static mut TEST_SCRATCH: [u8; 8 * 1024] = [0; 8 * 1024];

    struct TestLoop;

    bun_io::link_impl_EventLoopCtx! {
        Mini for TestLoop => |this| {
            platform_event_loop_ptr() => core::ptr::null_mut(),
            file_polls_ptr() => core::ptr::null_mut(),
            increment_pending_unref_counter() => {},
            ref_concurrently() => {},
            unref_concurrently() => {},
            after_event_loop_callback() => None,
            set_after_event_loop_callback(_cb, _ctx) => {},
            pipe_read_buffer() => core::ptr::addr_of_mut!(TEST_SCRATCH) as *mut [u8],
        }
    }

    // ── re-entrant parent ──────────────────────────────────────────────────
    //
    // Mirrors the upstream repro shape: the first `on_read_chunk` dispatch
    // re-enters `on_poll` on the same reader (a nested `FileReader.on_pull`
    // frame), and the nested frame's consumer pauses after one chunk. At
    // `on_reader_done` the parent receives whatever `_buffer` still retains —
    // the exact path where pre-0b041cba1 re-delivered already-delivered bytes.
    // `link_noop_EventLoopCtx!` can't cover `pipe_read_buffer() -> *mut [u8]`
    // (fat pointer has no null), so `Js` gets a real (never-called) stub.
    struct JsLoopStub;

    bun_io::link_impl_EventLoopCtx! {
        Js for JsLoopStub => |_this| {
            platform_event_loop_ptr() => core::ptr::null_mut(),
            file_polls_ptr() => core::ptr::null_mut(),
            increment_pending_unref_counter() => {},
            ref_concurrently() => {},
            unref_concurrently() => {},
            after_event_loop_callback() => None,
            set_after_event_loop_callback(_cb, _ctx) => {},
            pipe_read_buffer() => core::ptr::slice_from_raw_parts_mut(
                core::ptr::null_mut(),
                0,
            ),
        }
    }

    // The dispatcher's `match kind` arms reference every variant's extern
    // symbols, so the closed set must be fully defined in this binary:
    // `TestParallelWorkerPipe` above (real impl), everything else no-op'd.
    // (Dual-def rule: never list a variant that has a `link_impl_*!` here.)
    bun_io::link_noop_BufferedReaderParentLink!(
        SubprocessPipeReader,
        ShellPipeReader,
        ShellIoReader,
        FileReader,
        FileResponseStream,
        Terminal,
        CronRegister,
        CronRemove,
        FilterRunHandle,
        MultiRunPipeReader,
        LifecycleScript,
        SecurityScan
    );

    struct ReentrantParent {
        reader: *mut PosixBufferedReader,
        loop_owner: *mut TestLoop,
        delivered: Vec<Vec<u8>>,
        reentered: bool,
        done_count: usize,
        last_error: Option<i32>,
    }

    bun_io::impl_buffered_reader_parent! {
        TestParallelWorkerPipe for ReentrantParent;
        has_on_read_chunk = true;
        on_read_chunk = |this, chunk, _has_more| {
            let p = &mut *this;
            p.delivered.push(chunk.to_vec());
            if !p.reentered {
                p.reentered = true;
                // Nested read frame while the outer one is inside this
                // dispatch: outer holds the scratch claim, so this frame must
                // fall to its `_buffer` branch.
                PosixBufferedReader::on_poll(&mut *p.reader, 0, true);
                // Outer consumer keeps going so the outer frame drains to EOF.
                true
            } else {
                // Nested consumer pauses after its first chunk.
                false
            }
        };
        on_reader_done = |this| {
            let p = &mut *this;
            p.done_count += 1;
            let retained = (*p.reader)._buffer.clone();
            if !retained.is_empty() {
                p.delivered.push(retained);
            }
        };
        on_reader_error = |this, err| {
            (*this).last_error = Some(err.errno.into());
        };
        loop_ = |_this| core::ptr::null_mut::<Loop>();
        event_loop = |this| EventLoopCtx::new(
            EventLoopCtxKind::Mini,
            (*this).loop_owner,
        );
    }

    #[test]
    fn reentrant_read_delivers_each_byte_exactly_once() {
        const PAYLOAD_LEN: usize = 3 * 8 * 1024; // scratch(8K) + nested(≥16K) + tail
        let payload: Vec<u8> = (0..PAYLOAD_LEN)
            .map(|i| ((i * 7 + 13) % 256) as u8)
            .collect();

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (rfd, wfd) = (fds[0], fds[1]);
        let mut written = 0usize;
        while written < PAYLOAD_LEN {
            let n = unsafe {
                libc::write(
                    wfd,
                    payload[written..].as_ptr().cast(),
                    payload.len() - written,
                )
            };
            assert!(n >= 0, "pipe write failed");
            written += n as usize;
        }
        // Close the writer so the reader sees HUP → drains to EOF in-frame
        // (no `register_poll`, which would need real poll infrastructure).
        assert_eq!(unsafe { libc::close(wfd) }, 0);

        let loop_owner: &'static mut TestLoop = Box::leak(Box::new(TestLoop));

        let mut reader = PosixBufferedReader::init::<ReentrantParent>();
        reader.handle = PollOrFd::Fd(Fd::from_native(rfd));
        reader.flags.insert(PosixFlags::POLLABLE); // FileType::Pipe

        let mut parent = ReentrantParent {
            reader: core::ptr::from_mut(&mut reader),
            loop_owner: core::ptr::from_mut(loop_owner),
            delivered: Vec::new(),
            reentered: false,
            done_count: 0,
            last_error: None,
        };
        reader.set_parent(core::ptr::from_mut(&mut parent).cast());

        // Outer frame: HUP snapshot (writer closed), data pending.
        PosixBufferedReader::on_poll(&mut reader, 0, true);

        assert!(parent.reentered, "nested read frame must have run");
        assert_eq!(parent.done_count, 1, "on_reader_done fires exactly once");
        assert!(
            parent.last_error.is_none(),
            "no syscall error on the fixed path: {:?}",
            parent.last_error
        );

        let total: Vec<u8> = parent.delivered.concat();
        assert_eq!(
            total, payload,
            "delivered bytes must be exactly the payload, once each \
             (double-delivery regression: delivered {} bytes for a {} byte payload)",
            total.len(),
            payload.len()
        );

        // No re-deliverable residue may survive the final HUP drain.
        assert!(
            reader._buffer.is_empty(),
            "no re-deliverable residue may survive the final HUP drain"
        );
        // Evidence the nested frame took the `_buffer` branch (it could not
        // claim the 8 KiB scratch): its chunk came from a ≥16 KiB reserved
        // buffer slice, so it can exceed the scratch size — the outer
        // frame's chunks never can.
        assert!(
            parent.delivered.iter().any(|c| c.len() > 8 * 1024),
            "nested frame must read through its own buffer, not the scratch: \
             chunk sizes {:?}",
            parent.delivered.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }

    // Silence the "macro never used" path when the link registration is
    // compiled but the trait impl is only reached through the dispatch table.
    const _: fn() = || {
        fn _assert_parent_impl_present(p: &ReentrantParent) {
            let _ = <ReentrantParent as BufferedReaderParent>::HAS_ON_READ_CHUNK;
            let _ = &p.delivered;
        }
    };
}
