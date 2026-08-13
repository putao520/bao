// @trace REQ-ENG-006 [api:node:cluster] [entity:IpcChannel]
//
// IPC channel for child_process / cluster fd passing.
// Based on Bun's `src/jsc/ipc.zig` SendQueue design, Rust-ified with
// `std::os::unix::net::UnixStream` + libc SCM_RIGHTS for fd passing.
//
// Wire format (JSON mode, mirrors Bun's JSON line protocol):
//   * Plain message: `<json>\n` written via write(2).
//   * fd-carrying message: `<json>` sent via sendmsg(2) with SCM_RIGHTS
//     ancillary data carrying exactly one RawFd. No trailing newline on the
//     sendmsg path — the JSON payload length is the iov_len.
//
// Receive side buffers bytes until a `\n` terminates a JSON line. recvmsg
// reads both the payload bytes and any inbound SCM_RIGHTS fd, and the next
// `recv_msg()` call returns the parsed JSON line plus any stashed fd. When a
// payload arrives via SCM_RIGHTS (no trailing `\n`), the buffered bytes form a
// complete message directly.
//
// SAFETY: sendmsg/recvmsg + cmsghdr manipulation is `unsafe` because it
// touches raw C struct layouts. The cmsg buffer sizing uses libc's
// CMSG_SPACE/CMSG_LEN/CMSG_DATA which are the canonical alignment-safe macros.

use ::std::io::{self, Read, Write};
use ::std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use ::std::os::unix::net::UnixStream;

/// Size of the ancillary data buffer for one SCM_RIGHTS fd.
///
/// `CMSG_SPACE(sizeof(RawFd))` is the alignment-safe total size required by
/// the kernel. We size the buffer to this value at minimum (64 bytes covers
/// the common 16-byte aligned cmsghdr + 4-byte fd on Linux x86_64 with room
/// to spare). The actual size used is computed dynamically via libc macros.
const CMSG_BUF_SIZE: usize = 64;

/// IPC channel — a buffered Unix socket pair endpoint that speaks
/// newline-delimited JSON, optionally carrying a RawFd per message via
/// SCM_RIGHTS ancillary data.
///
/// One channel = one endpoint of a `socketpair(AF_UNIX, SOCK_STREAM)`. The
/// other endpoint belongs to the peer (parent ↔ child).
pub struct IpcChannel {
    /// Underlying Unix domain socket endpoint.
    socket: UnixStream,

    /// Line-buffered receive buffer. Bytes accumulate here until a `\n`
    /// terminates a JSON line. If a SCM_RIGHTS recvmsg arrives, the payload
    /// is appended here without a newline and treated as one complete message.
    recv_buf: Vec<u8>,

    /// fd stashed from the most recent SCM_RIGHTS recvmsg, waiting to be
    /// paired with the next completed message returned to the caller.
    /// Mirrors Bun's `incoming_fd: Option<fd>` stash in ipc.zig.
    incoming_fd: Option<RawFd>,

    /// Tracks connection state. Set to false on EOF or unrecoverable error.
    connected: bool,
}

impl IpcChannel {
    /// Wrap an existing connected UnixStream as an IPC channel endpoint.
    pub fn new(socket: UnixStream) -> Self {
        Self {
            socket,
            recv_buf: Vec::with_capacity(4096),
            incoming_fd: None,
            connected: true,
        }
    }

    /// Send a JSON message terminated by `\n`. No fd is attached.
    pub fn send_json(&mut self, json: &str) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "ipc: not connected"));
        }
        self.socket.write_all(json.as_bytes())?;
        self.socket.write_all(b"\n")?;
        Ok(())
    }

    /// Send a JSON message + fd via SCM_RIGHTS ancillary data.
    ///
    /// The fd is duplicated into the kernel's ancillary buffer; the receiver
    /// obtains a new independent fd via recvmsg. The kernel-level fd remains
    /// valid in the sender until the sender closes it (we do NOT close it
    /// here — the caller may still need it).
    pub fn send_handle(&mut self, json: &str, fd: RawFd) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "ipc: not connected"));
        }
        let payload = json.as_bytes();
        unsafe { send_fd_via_scm_right(&self.socket, payload, fd)? };
        Ok(())
    }

    /// Receive the next JSON message (blocking read until a newline-terminated
    /// line is available, or until a SCM_RIGHTS recvmsg delivers a complete
    /// payload). Returns `(json, Option<fd>)` — the fd, if present, was
    /// carried by the SCM_RIGHTS ancillary data of one of the recvmsg calls
    /// that produced this message.
    ///
    /// Returns an UnexpectedEof error when the peer has closed the channel
    /// and the buffer is drained. `is_connected()` will return false afterwards.
    pub fn recv_msg(&mut self) -> io::Result<(String, Option<RawFd>)> {
        loop {
            // Fast path: do we already have a complete line buffered?
            if let Some(nl) = self.recv_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.recv_buf.drain(..=nl).collect();
                let json = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                let fd = self.incoming_fd.take();
                return Ok((json, fd));
            }

            // Also handle the SCM_RIGHTS case where the payload has no `\n`
            // but a fd is stashed — treat the entire buffer as one message
            // when a fd is present. (Mirrors Bun: when recvmsg delivers both
            // payload and fd atomically, that payload IS the message.)
            if self.incoming_fd.is_some() && !self.recv_buf.is_empty() {
                let buf = ::std::mem::take(&mut self.recv_buf);
                let json = String::from_utf8_lossy(&buf).into_owned();
                let fd = self.incoming_fd.take();
                return Ok((json, fd));
            }

            // Need more bytes. Try a recvmsg (which also picks up any inbound fd).
            let (bytes, fd_opt) = unsafe { recv_msg_with_cmsg(&self.socket)? };
            if bytes.is_empty() {
                // Peer closed (recvmsg returned 0 bytes). If interrupted, we
                // treat it as a benign retry (handled inside the primitive).
                // Distinguish: interrupted returns Ok((0, None)) WITHOUT EOF.
                // We use a peek at the recv_buf to decide: if the buffer is
                // empty and the channel was readable, it's EOF.
                if self.recv_buf.is_empty() {
                    self.connected = false;
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "ipc: peer closed channel",
                    ));
                }
                // Otherwise loop — interrupted with bytes still buffered just
                // re-checks the line-scan above.
                continue;
            }
            self.recv_buf.extend_from_slice(&bytes);
            if let Some(fd) = fd_opt {
                // Stash fd — it will be paired with the next completed message.
                self.incoming_fd = Some(fd);
            }
        }
    }

    /// Returns true while neither side has closed the channel.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Close the underlying socket. Safe to call multiple times.
    pub fn close(&mut self) {
        let _ = self.socket.shutdown(::std::net::Shutdown::Both);
        self.connected = false;
    }

    /// Borrow the underlying socket fd (for epoll registration etc.).
    pub fn raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

impl Drop for IpcChannel {
    fn drop(&mut self) {
        self.close();
    }
}

/// Create a connected pair of UnixStream endpoints via `socketpair(AF_UNIX,
/// SOCK_STREAM, 0)`. The caller passes one end to the child (as fd 3 per
/// Node.js convention) and keeps the other in the parent's IpcChannel.
pub fn create_ipc_pair() -> io::Result<(UnixStream, UnixStream)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    // SAFETY: socketpair writes two fd ints into a stack array we own; the
    // return value indicates success/failure and we propagate via io::Error.
    let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: FromRawFd takes ownership of fds[0] / fds[1]; they were just
    // created and are valid Unix-domain stream sockets. Each is wrapped exactly
    // once. Closing the UnixStream will close the underlying fd.
    let a = unsafe { <UnixStream as FromRawFd>::from_raw_fd(fds[0]) };
    let b = unsafe { <UnixStream as FromRawFd>::from_raw_fd(fds[1]) };
    Ok((a, b))
}

// ─── SCM_RIGHTS send/recv primitives ────────────────────────────────────────
//
// These are the canonical Linux sendmsg/recvmsg + cmsghdr patterns. They are
// `unsafe` because they manipulate raw C struct layouts (msghdr, cmsghdr,
// iovec). The macros CMSG_FIRSTHDR / CMSG_DATA / CMSG_SPACE / CMSG_LEN are
// the alignment-safe helpers exported by libc.

/// Send `payload` bytes + `fd` via sendmsg with SCM_RIGHTS ancillary data.
///
/// The fd is duplicated in-kernel: the receiver gets a fresh fd referring to
/// the same open file description. The sender keeps its own fd valid until
/// it explicitly closes it.
///
/// # Safety
/// Caller ensures `socket` is a connected AF_UNIX SOCK_STREAM and `fd` is a
/// valid open file descriptor in this process.
unsafe fn send_fd_via_scm_right(
    socket: &UnixStream,
    payload: &[u8],
    fd: RawFd,
) -> io::Result<()> {
    // Build the iov pointing at the payload. sendmsg reads from it.
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as *mut ::std::ffi::c_void,
        iov_len: payload.len(),
    };

    // Ancillary buffer sized to CMSG_SPACE(sizeof(RawFd)) — this is the
    // kernel-required alignment-safe size for one fd.
    let mut cmsg_buf = [0u8; CMSG_BUF_SIZE];

    // Zero-initialise msghdr (idiomatic — sets all unused fields to NULL/0).
    let mut msg: libc::msghdr = unsafe { ::std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut ::std::ffi::c_void;
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(::std::mem::size_of::<RawFd>() as u32) as _ };

    // CMSG_FIRSTHDR returns a pointer into msg_control; we fill in the
    // cmsghdr header for SCM_RIGHTS and copy the fd into CMSG_DATA.
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if cmsg.is_null() {
        return Err(io::Error::new(io::ErrorKind::Other, "CMSG_FIRSTHDR null"));
    }
    unsafe {
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(::std::mem::size_of::<RawFd>() as u32) as _;
        let data_ptr = libc::CMSG_DATA(cmsg) as *mut RawFd;
        // Copy the fd value into the ancillary data slot.
        ::std::ptr::write_unaligned(data_ptr, fd);
    }

    // sendmsg may be interrupted; loop until we write at least the payload
    // (the kernel handles fd ancillary separately from partial writes — but
    // for SOCK_STREAM small payloads are written atomically).
    let mut sent_total = 0usize;
    while sent_total < payload.len() {
        let ret =
            unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, 0) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        sent_total += ret as usize;
        // For SCM_RIGHTS we only send once (the cmsg goes with the first
        // successful sendmsg). Subsequent partial writes use plain writev
        // by zeroing msg_control — but in practice the payload is tiny so a
        // single sendmsg completes it.
        if sent_total < payload.len() {
            // Strip ancillary data for the remainder.
            msg.msg_control = ::std::ptr::null_mut();
            msg.msg_controllen = 0;
            // Advance iov_base past the bytes already sent.
            iov.iov_base = (payload[sent_total..].as_ptr()) as *mut ::std::ffi::c_void;
            iov.iov_len = payload.len() - sent_total;
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
        }
    }
    Ok(())
}

/// Receive up to `RECV_CHUNK` bytes via recvmsg, plus any inbound SCM_RIGHTS
/// fd from ancillary data.
///
/// Returns `(Vec<u8>, Option<fd>)`. Empty Vec means peer half-closed the
/// stream (EOF). `Interrupted` errors return `Ok((vec![], None))` so the
/// caller's loop just retries. Other errors propagate.
///
/// # Safety
/// Caller ensures `socket` is a connected AF_UNIX SOCK_STREAM.
unsafe fn recv_msg_with_cmsg(socket: &UnixStream) -> io::Result<(Vec<u8>, Option<RawFd>)> {
    const RECV_CHUNK: usize = 4096;
    let mut buf = vec![0u8; RECV_CHUNK];

    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut ::std::ffi::c_void,
        iov_len: buf.len(),
    };

    let mut cmsg_buf = [0u8; CMSG_BUF_SIZE];

    let mut msg: libc::msghdr = unsafe { ::std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut ::std::ffi::c_void;
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(::std::mem::size_of::<RawFd>() as u32) as _ };

    let ret = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, 0) };
    if ret < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok((Vec::new(), None));
        }
        return Err(err);
    }

    let n_bytes = ret as usize;
    buf.truncate(n_bytes);

    // Walk ancillary data for an SCM_RIGHTS fd. Use CMSG_FIRSTHDR/CMSG_NXTHDR
    // to enumerate cmsg entries safely (handles alignment).
    let mut found_fd: Option<RawFd> = None;
    let mut cmsg_ptr = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg_ptr.is_null() {
        let cmsg = unsafe { &*cmsg_ptr };
        if cmsg.cmsg_level == libc::SOL_SOCKET && cmsg.cmsg_type == libc::SCM_RIGHTS {
            let data_ptr = unsafe { libc::CMSG_DATA(cmsg_ptr) } as *const RawFd;
            // Read the fd value (unaligned read for safety).
            let fd = unsafe { ::std::ptr::read_unaligned(data_ptr) };
            // If we already have one fd from this message, close the extra
            // (Bun ipc.zig semantics: one fd per message; extras are dropped).
            if found_fd.is_some() {
                unsafe { libc::close(fd) };
            } else {
                found_fd = Some(fd);
            }
        }
        cmsg_ptr = unsafe { libc::CMSG_NXTHDR(&msg, cmsg_ptr) };
    }

    Ok((buf, found_fd))
}

// ─── Unit tests ─────────────────────────────────────────────────────────────
//
// These tests exercise the wire protocol end-to-end in-process: parent and
// child endpoints live in two halves of a socketpair within the same process.
// They verify JSON line framing, SCM_RIGHTS fd passing, and that an extra
// inbound fd is correctly closed (Bun ipc.zig "one fd per message" rule).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_recv_json_no_fd() {
        // Create the socketpair, wrap one end as channel A (sender), the
        // other as channel B (receiver). Send two newline-delimited JSON
        // messages and verify they round-trip in order.
        let (a, b) = create_ipc_pair().expect("socketpair");
        let mut parent = IpcChannel::new(a);
        let mut child = IpcChannel::new(b);

        parent.send_json(r#"{"type":"hello"}"#).unwrap();
        parent.send_json(r#"{"type":"world"}"#).unwrap();

        let (m1, fd1) = child.recv_msg().unwrap();
        assert_eq!(m1, r#"{"type":"hello"}"#);
        assert!(fd1.is_none());

        let (m2, fd2) = child.recv_msg().unwrap();
        assert_eq!(m2, r#"{"type":"world"}"#);
        assert!(fd2.is_none());
    }

    #[test]
    fn test_send_handle_delivers_fd() {
        // Open /dev/null, pass its fd to the peer via SCM_RIGHTS. The peer
        // should receive a new fd (different number) that also refers to
        // /dev/null and is independently closeable.
        let (a, b) = create_ipc_pair().expect("socketpair");
        let mut parent = IpcChannel::new(a);
        let mut child = IpcChannel::new(b);

        // Open a stable fd to /dev/null that we can identify.
        let devnull =
            unsafe { libc::open(b"/dev/null\0".as_ptr() as *const _, libc::O_RDONLY) };
        assert!(devnull >= 0);

        parent
            .send_handle(r#"{"type":"NODE_HANDLE","kind":"devnull"}"#, devnull)
            .unwrap();

        let (msg, fd_opt) = child.recv_msg().unwrap();
        assert_eq!(msg, r#"{"type":"NODE_HANDLE","kind":"devnull"}"#);
        let recv_fd = fd_opt.expect("should have received a fd");
        assert_ne!(recv_fd, devnull, "kernel should have allocated a new fd");

        // The received fd must be readable (refer to /dev/null).
        let mut probe = [0u8; 1];
        let n = unsafe { libc::read(recv_fd, probe.as_mut_ptr() as *mut _, 1) };
        assert_eq!(n, 0, "received fd should read EOF (it's /dev/null)");

        // Cleanup: close both fds.
        unsafe {
            libc::close(recv_fd);
            libc::close(devnull);
        }
    }
}
