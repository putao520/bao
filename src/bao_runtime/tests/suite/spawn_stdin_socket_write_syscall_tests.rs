// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
// Upstream regression: bun 118fdd203a "child_process: report a failed child
// stdin write with syscall write, as Node does (#40935)" (verified against
// ~/code/rust/bun git show). A "pipe" child stdio is a socketpair(2) on POSIX
// and the pipe writer sends to it with send(2) only to pass MSG_NOSIGNAL;
// the write error must still name syscall "write" (Node semantics), not
// "send". Fix landed in bun_io PipeWriter.rs as the write_to_socket wrapper.
//
// Real path only (no mock): a real AF_UNIX SOCK_STREAM socketpair created by
// the same bun_sys::socketpair_for_shell call spawn_process.rs uses for pipe
// stdio, a real /bin/sh child holding the peer end (reads exactly 1 byte —
// upstream's `readSync(0, Buffer.alloc(1))` — then exits, closing the socket),
// and the write driven through PosixPipeWriter::try_write's Socket arm — the
// shared entry both PosixBufferedWriter (StaticPipeWriter stdin) and
// PosixStreamingWriter use, which routes to write_to_socket → send_non_block.
// The upstream JS test races 16 MiB against the child exit; here the child is
// reaped first so the EPIPE is deterministic, then the same 16 MiB bulk write
// asserts errno=EPIPE and the relabeled syscall tag.

use std::os::fd::FromRawFd;
use std::process::{Command, Stdio};

use bun_io::pipes::{FileType, PollOrFd};
use bun_io::pipe_writer::{PosixPipeWriter, WriteResult, WriteStatus};
use bun_sys::{self as sys, Fd};

/// Minimal real-fd driver for the Socket arm of `PosixPipeWriter::try_write`.
/// The parent callbacks are inert because the assertion reads the returned
/// `WriteResult` directly (in production, StaticPipeWriter consumes the same
/// `Err(err)` via `on_error`, and `to_system_error` maps `err.syscall` onto
/// the JS error's `syscall` field).
struct SocketStdinWriter {
    fd: Fd,
    handle: PollOrFd,
}

impl PosixPipeWriter for SocketStdinWriter {
    fn get_fd(&self) -> Fd {
        self.fd
    }
    fn get_buffer(&self) -> &[u8] {
        &[]
    }
    fn on_write(&mut self, _written: usize, _status: WriteStatus) {}
    fn register_poll(&mut self) {}
    fn on_error(&mut self, _err: sys::Error) {}
    fn get_file_type(&self) -> FileType {
        FileType::Socket
    }
    fn get_force_sync(&self) -> bool {
        false
    }
    fn handle(&self) -> &PollOrFd {
        &self.handle
    }
}

#[test]
fn spawn_stdin_socket_write_epipe_names_syscall_write() {
    // Real socketpair — same construction as spawn_process.rs pipe stdio.
    let pair =
        sys::socketpair_for_shell(libc::AF_UNIX, libc::SOCK_STREAM, 0, false).expect("socketpair");
    let parent_fd = pair[0];
    let peer_fd = pair[1];

    // Real child: the peer end becomes its stdin; `head -c 1` reads exactly
    // one byte and exits, closing the socket from the child side.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("head -c 1 >/dev/null")
        .stdin(Stdio::from(unsafe {
            std::os::fd::OwnedFd::from_raw_fd(peer_fd.native())
        }))
        .spawn()
        .expect("spawn /bin/sh child");

    let writer = SocketStdinWriter {
        fd: parent_fd,
        handle: PollOrFd::Fd(parent_fd),
    };

    // Feed the byte `head` waits for so it completes its 1-byte read and
    // exits (one byte always fits the socket buffer: send succeeds).
    match writer.try_write(false, b"a") {
        WriteResult::Err(err) => panic!("first-byte write failed: {}", err),
        WriteResult::Pending(n) => panic!("first-byte write unexpectedly pending ({n})"),
        _ => {}
    }

    // Reap the child: its exit releases the peer socket, so every further
    // send deterministically fails EPIPE (MSG_NOSIGNAL suppresses SIGPIPE).
    let status = child.wait().expect("wait for child");
    assert!(
        status.success(),
        "child must exit 0 after its 1-byte read, got {status}"
    );

    // The 16 MiB bulk write from the upstream test — must fail EPIPE with
    // syscall named "write", not "send".
    let bulk = vec![0x61u8; 16 * 1024 * 1024];
    match writer.try_write(false, &bulk) {
        WriteResult::Err(err) => {
            assert_eq!(
                err.syscall,
                sys::Tag::write,
                "child stdin write error must name syscall write (Node semantics), got {:?} ({})",
                err.syscall,
                err.syscall.name()
            );
            assert_eq!(
                err.errno, libc::EPIPE as u16,
                "errno must be EPIPE, got {} ({})",
                err.errno,
                err.syscall.name()
            );
        }
        WriteResult::Wrote(n) => panic!("bulk write unexpectedly fully wrote {n} bytes"),
        WriteResult::Pending(n) => panic!("bulk write unexpectedly pending ({n} bytes in)"),
        WriteResult::Done(n) => panic!("bulk write unexpectedly hit EOF after {n} bytes"),
    }
}
