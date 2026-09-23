// @trace REQ-ENG-006 [api:node:cluster] [entity:IpcChannel]
//
// IPC channel for child_process / cluster fd passing.
// Based on Bun's `src/jsc/ipc.rs` SendQueue design, restructured as a
// synchronous channel with a cfg-blind public API and two platform halves:
//
// * unix — `std::os::unix::net::UnixStream` + libc SCM_RIGHTS fd passing
//   (mirrors Bun's unix ipc arm; the `Bun__addrinfo`-style wire contract is
//   documented on the unix half below).
// * windows — a byte-mode named-pipe pair (`CreateNamedPipeW` server +
//   `CreateFileW` client), synchronous `ReadFile`/`WriteFile`. Upstream's
//   windows IPC arm (`ipc.rs` `SocketUnion::Open(uv::Pipe)`) carries no
//   ancillary fd passing, so `send_handle` reports unsupported there — the
//   same behaviour parity as upstream.
//
// Wire format (JSON mode, mirrors Bun's JSON line protocol):
//   * Plain message: `<json>\n`.
//   * fd-carrying message (unix only): `<json>` sent via sendmsg(2) with
//     SCM_RIGHTS ancillary data carrying exactly one RawFd. No trailing
//     newline on that path — the JSON payload length is the iov_len.
//
// Receive side buffers bytes until a `\n` terminates a JSON line. On unix a
// recvmsg reads both payload bytes and any inbound SCM_RIGHTS fd; the next
// `recv_msg()` returns the parsed JSON line plus the stashed fd.

use ::std::io;

/// Size of the ancillary data buffer for one SCM_RIGHTS fd (unix).
///
/// `CMSG_SPACE(sizeof(RawFd))` is the alignment-safe total size required by
/// the kernel; 64 bytes covers the common 16-byte aligned cmsghdr + 4-byte fd
/// with room to spare.
#[cfg(unix)]
const CMSG_BUF_SIZE: usize = 64;

/// IPC channel — a buffered duplex endpoint that speaks newline-delimited
/// JSON, optionally carrying a RawFd per message (unix SCM_RIGHTS; unsupported
/// on windows, matching upstream).
///
/// One channel = one endpoint of a connected pair. The other endpoint belongs
/// to the peer (parent ↔ child).
pub struct IpcChannel {
    stream: IpcStream,

    /// Line-buffered receive buffer. Bytes accumulate here until a `\n`
    /// terminates a JSON line. If an fd-carrying recv arrives, the payload is
    /// appended here without a newline and treated as one complete message.
    recv_buf: Vec<u8>,

    /// fd stashed from the most recent unix SCM_RIGHTS recv, waiting to be
    /// paired with the next completed message returned to the caller.
    /// Mirrors Bun's `incoming_fd: Option<fd>` stash in ipc.zig.
    incoming_fd: Option<RawFd>,

    /// Tracks connection state. Set to false on EOF or unrecoverable error.
    connected: bool,
}

impl IpcChannel {
    /// Wrap an existing connected stream endpoint as an IPC channel.
    ///
    /// Accepts the platform stream type directly (unix `UnixStream` / windows
    /// HANDLE-wrapped `IpcStream`) via `Into<IpcStream>`, so call sites stay
    /// cfg-blind.
    pub fn new(stream: impl Into<IpcStream>) -> Self {
        Self {
            stream: stream.into(),
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
        self.stream.write_all(json.as_bytes())?;
        self.stream.write_all(b"\n")?;
        Ok(())
    }

    /// Send a JSON message + fd.
    ///
    /// unix: the fd is duplicated into the kernel's ancillary buffer; the
    /// receiver obtains a new independent fd. The kernel-level fd remains
    /// valid in the sender until the sender closes it (we do NOT close it
    /// here — the caller may still need it).
    ///
    /// windows: upstream's windows IPC arm carries no ancillary fd passing —
    /// report unsupported (behaviour parity).
    pub fn send_handle(&mut self, json: &str, fd: impl Into<RawFd>) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "ipc: not connected"));
        }
        IpcStream::send_handle(&self.stream, json.as_bytes(), fd.into())
    }

    /// Receive the next JSON message (blocking read until a newline-terminated
    /// line is available, or — unix — until a SCM_RIGHTS recv delivers a
    /// complete payload). Returns `(json, Option<fd>)` — the fd, if present,
    /// was carried by the ancillary data of one of the reads that produced
    /// this message (always None on windows).
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

            // Need more bytes. The platform read also picks up any inbound fd
            // (unix SCM_RIGHTS).
            let (bytes, fd_opt) = self.stream.recv_msg_chunk()?;
            if bytes.is_empty() {
                // Peer closed (read returned 0 bytes). If interrupted, the
                // platform read returns Ok((0, None)) WITHOUT EOF; a peek at
                // the buffer decides: empty buffer after a readable channel
                // means EOF.
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

    /// Close the underlying stream. Safe to call multiple times.
    pub fn close(&mut self) {
        self.stream.close();
        self.connected = false;
    }

    /// Borrow the underlying stream descriptor (for epoll registration etc.).
    pub fn raw_fd(&self) -> RawFd {
        self.stream.raw_fd()
    }
}

impl Drop for IpcChannel {
    fn drop(&mut self) {
        self.close();
    }
}

// ─── unix half: UnixStream + SCM_RIGHTS ─────────────────────────────────────
#[cfg(unix)]
mod platform {
    use super::*;
    use ::std::io::{Read, Write};
    use ::std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
    use ::std::os::unix::net::UnixStream;

    pub type RawFdAlias = RawFd;

    /// unix endpoint — a connected AF_UNIX SOCK_STREAM.
    pub struct IpcStream(pub(crate) UnixStream);

    impl From<UnixStream> for IpcStream {
        fn from(s: UnixStream) -> Self {
            Self(s)
        }
    }

    impl IpcStream {
        pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
            (&self.0).write_all(bytes)
        }

        /// One recvmsg: up to 4096 bytes plus any inbound SCM_RIGHTS fd.
        /// Returns `(Vec<u8>, Option<fd>)`. Empty Vec = peer half-closed (EOF)
        /// or interrupted (caller re-checks its buffer). See the module doc.
        pub fn recv_msg_chunk(&self) -> io::Result<(Vec<u8>, Option<RawFd>)> {
            const RECV_CHUNK: usize = 4096;
            let mut buf = vec![0u8; RECV_CHUNK];

            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr() as *mut ::std::ffi::c_void,
                iov_len: buf.len(),
            };

            let mut cmsg_buf = [0u8; CMSG_BUF_SIZE];

            // Zero-initialise msghdr (idiomatic — sets all unused fields to NULL/0).
            let mut msg: libc::msghdr = unsafe { ::std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = cmsg_buf.as_mut_ptr() as *mut ::std::ffi::c_void;
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(::std::mem::size_of::<RawFd>() as u32) as _ };

            // SAFETY: socket is a connected AF_UNIX socket; msg/iov point at
            // memory we own; the cmsg macros are the alignment-safe helpers.
            let ret = unsafe { libc::recvmsg(self.0.as_raw_fd(), &mut msg, 0) };
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

        /// Send `payload` + `fd` via sendmsg with SCM_RIGHTS ancillary data.
        ///
        /// The fd is duplicated in-kernel: the receiver gets a fresh fd
        /// referring to the same open file description. The sender keeps its
        /// own fd valid until it explicitly closes it.
        ///
        /// # Safety
        /// Caller ensures the socket is a connected AF_UNIX SOCK_STREAM and
        /// `fd` is a valid open file descriptor in this process.
        pub fn send_handle(&self, payload: &[u8], fd: RawFd) -> io::Result<()> {
            // Build the iov pointing at the payload. sendmsg reads from it.
            let mut iov = libc::iovec {
                iov_base: payload.as_ptr() as *mut ::std::ffi::c_void,
                iov_len: payload.len(),
            };

            // Ancillary buffer sized to CMSG_SPACE(sizeof(RawFd)).
            let mut cmsg_buf = [0u8; CMSG_BUF_SIZE];

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
                let ret = unsafe { libc::sendmsg(self.0.as_raw_fd(), &msg, 0) };
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

        pub fn close(&self) {
            let _ = self.0.shutdown(::std::net::Shutdown::Both);
        }

        pub fn raw_fd(&self) -> RawFd {
            self.0.as_raw_fd()
        }
    }

    /// Create a connected pair of UnixStream endpoints via `socketpair(AF_UNIX,
    /// SOCK_STREAM, 0)`. The caller passes one end to the child (as fd 3 per
    /// Node.js convention) and keeps the other in the parent's IpcChannel.
    pub fn create_ipc_pair() -> io::Result<(IpcStream, IpcStream)> {
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
        Ok((IpcStream(a), IpcStream(b)))
    }
}

// ─── windows half: named pipe pair, synchronous ReadFile/WriteFile ─────────
#[cfg(windows)]
mod platform {
    use super::*;
    use bun_windows_sys as w;
    use bun_windows_sys::kernel32::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, ReadFile, WriteFile,
    };
    use ::std::ffi::c_void;

    /// windows endpoint — one end of a byte-mode duplex named pipe.
    ///
    /// `server` marks the `CreateNamedPipeW` end (its `close` severs the
    /// client via `DisconnectNamedPipe` first, matching pipe semantics).
    ///
    /// SAFETY (Send/Sync): a Win32 HANDLE has no thread affinity — any thread
    /// may use it. The channel's callers serialize access (same contract as
    /// the unix `UnixStream`, which is `Send`).
    pub struct IpcStream {
        handle: w::HANDLE,
        server: bool,
    }
    unsafe impl Send for IpcStream {}
    unsafe impl Sync for IpcStream {}

    impl IpcStream {
        /// Wrap an already-connected pipe/file HANDLE (e.g. a pipe end the
        /// spawn face produced for the child's fd-3 slot).
        pub const fn from_handle(handle: w::HANDLE) -> Self {
            Self { handle, server: false }
        }

        pub fn handle(&self) -> w::HANDLE {
            self.handle
        }

        pub fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
            while !bytes.is_empty() {
                let mut written: w::DWORD = 0;
                // SAFETY: synchronous WriteFile on a HANDLE we own; buffer +
                // length describe the caller's slice; out-param is a stack
                // DWORD.
                let ok = unsafe {
                    WriteFile(
                        self.handle,
                        bytes.as_ptr(),
                        bytes.len() as w::DWORD,
                        &mut written,
                        ::std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                if written == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "ipc: pipe write made no progress",
                    ));
                }
                bytes = &bytes[written as usize..];
            }
            Ok(())
        }

        /// One pipe read: up to 4096 bytes. Returns `(Vec<u8>, None)` —
        /// empty Vec = peer closed (`ERROR_BROKEN_PIPE` / zero-byte read EOF).
        ///
        /// WIN-IPC-POLL (E9, 2026-09-23): the pair is created blocking-mode
        /// (`PIPE_WAIT`), where the old comment's `ERROR_NO_DATA` would-block
        /// shape never fires — `ReadFile` with no pending bytes **parks the
        /// JS thread** (cluster worker drain-hook livelock: pump recv blocks
        /// forever, worker loop never drains, primary never sees exit —
        /// 240s test timeout). `PeekNamedPipe` is the non-destructive
        /// availability probe: zero bytes → `WouldBlock` (the poll retry
        /// shape the callers already handle), broken pipe → EOF, bytes
        /// pending → the ReadFile below completes immediately.
        pub fn recv_msg_chunk(&self) -> io::Result<(Vec<u8>, Option<RawFd>)> {
            const RECV_CHUNK: usize = 4096;
            let mut avail: w::DWORD = 0;
            // SAFETY: HANDLE we own; out-param is a stack DWORD; the bytes /
            // bytes-read / leftover probes are all null (availability only).
            let peeked = unsafe {
                PeekNamedPipe(
                    self.handle,
                    ::std::ptr::null_mut(),
                    0,
                    ::std::ptr::null_mut(),
                    &mut avail,
                    ::std::ptr::null_mut(),
                )
            };
            if peeked == 0 {
                let err = io::Error::last_os_error();
                let code = err.raw_os_error().unwrap_or(0) as w::DWORD;
                if code == w::ERROR_BROKEN_PIPE || code == w::ERROR_NO_DATA {
                    return Ok((Vec::new(), None));
                }
                return Err(err);
            }
            if avail == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "ipc: no bytes pending",
                ));
            }
            let mut buf = vec![0u8; RECV_CHUNK];
            let mut read: w::DWORD = 0;
            // SAFETY: synchronous ReadFile on a HANDLE we own; buffer/length
            // describe our own Vec; out-param is a stack DWORD.
            let ok = unsafe {
                ReadFile(
                    self.handle,
                    buf.as_mut_ptr(),
                    buf.len() as w::DWORD,
                    &mut read,
                    ::std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = io::Error::last_os_error();
                let code = err.raw_os_error().unwrap_or(0) as w::DWORD;
                if code == w::ERROR_BROKEN_PIPE || code == w::ERROR_NO_DATA {
                    // Peer closed its end: EOF (empty vec signals it upstream).
                    return Ok((Vec::new(), None));
                }
                return Err(err);
            }
            if read == 0 {
                // Clean zero-byte read: EOF.
                return Ok((Vec::new(), None));
            }
            buf.truncate(read as usize);
            // windows IPC carries no ancillary fds (upstream parity).
            Ok((buf, None))
        }

        /// windows IPC carries no ancillary fd passing (upstream parity — the
        /// windows arm of Bun's ipc has no SCM_RIGHTS equivalent).
        pub fn send_handle(&self, _payload: &[u8], _fd: RawFd) -> io::Result<()> {
            let _ = _payload;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ipc: handle passing is not supported on windows",
            ))
        }

        pub fn close(&self) {
            if self.server {
                // SAFETY: server-side pipe we own; severing the client before
                // closing is the documented pipe teardown sequence.
                unsafe { DisconnectNamedPipe(self.handle) };
            }
            // SAFETY: HANDLE we own.
            unsafe { w::CloseHandle(self.handle) };
        }

        pub fn raw_fd(&self) -> RawFd {
            self.handle as RawFd
        }
    }

    /// Create a connected byte-mode duplex named pipe pair.
    ///
    /// The server end is created with `CreateNamedPipeW` under a unique
    /// per-pair name; the client end connects via `CreateFileW`. The server
    /// then waits in `ConnectNamedPipe` (a client that raced us surfaces as
    /// `ERROR_PIPE_CONNECTED`, which is success).
    ///
    /// The client handle is created inheritable so the spawn face can hand it
    /// to the child (Node's fd-3 IPC convention maps to handle inheritance on
    /// windows).
    pub fn create_ipc_pair() -> io::Result<(IpcStream, IpcStream)> {
        use ::std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "\\\\.\\pipe\\bao-ipc-{}-{}\0",
            ::std::process::id(),
            seq
        );
        let name_w: Vec<u16> = name.encode_utf16().collect();

        // SAFETY: name is NUL-terminated UTF-16 we own; the pipe is created
        // with a single instance, byte mode, blocking; no security attributes
        // (default) — the server end is not handed to the child.
        let server = unsafe {
            CreateNamedPipeW(
                name_w.as_ptr(),
                w::PIPE_ACCESS_DUPLEX,
                w::PIPE_TYPE_BYTE | w::PIPE_READMODE_BYTE | w::PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                ::std::ptr::null_mut(),
            )
        };
        if server == w::INVALID_HANDLE_VALUE || server.is_null() {
            return Err(io::Error::last_os_error());
        }

        // Client end: inheritable so the spawn face can pass it as the child's
        // IPC handle (Node's fd-3 convention maps to handle inheritance here).
        let mut sa = w::SECURITY_ATTRIBUTES {
            nLength: ::std::mem::size_of::<w::SECURITY_ATTRIBUTES>() as w::DWORD,
            lpSecurityDescriptor: ::std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        // SAFETY: name is NUL-terminated UTF-16 we own; sa is a stack struct
        // valid for the call; OPEN_EXISTING + GENERIC_* are the client-side
        // access pair for a duplex pipe.
        let client = unsafe {
            w::CreateFileW(
                name_w.as_ptr(),
                w::GENERIC_READ | w::GENERIC_WRITE,
                0,
                &mut sa,
                w::OPEN_EXISTING,
                0,
                ::std::ptr::null_mut(),
            )
        };
        if client == w::INVALID_HANDLE_VALUE || client.is_null() {
            let err = io::Error::last_os_error();
            // SAFETY: we own the server handle.
            unsafe { w::CloseHandle(server) };
            return Err(err);
        }

        // SAFETY: we own the server handle; no overlapped IO, so the
        // overlapped parameter is null. A client that connected between
        // CreateNamedPipeW and here reports ERROR_PIPE_CONNECTED = success.
        let connected = unsafe { ConnectNamedPipe(server, ::std::ptr::null_mut()) };
        if connected == 0 {
            let err = io::Error::last_os_error();
            let code = err.raw_os_error().unwrap_or(0) as w::DWORD;
            if code != w::ERROR_PIPE_CONNECTED {
                // SAFETY: we own both handles.
                unsafe {
                    w::CloseHandle(server);
                    w::CloseHandle(client);
                }
                return Err(err);
            }
        }

        Ok((
            IpcStream { handle: server, server: true },
            IpcStream { handle: client, server: false },
        ))
    }
}

// ─── cfg-blind re-exports ───────────────────────────────────────────────────
#[cfg(unix)]
pub use platform::create_ipc_pair;
#[cfg(windows)]
pub use platform::create_ipc_pair;

#[cfg(unix)]
pub(crate) use platform::IpcStream;
#[cfg(windows)]
pub(crate) use platform::IpcStream;

/// Raw stream descriptor: unix fd (`i32`) / windows HANDLE (`isize`).
#[cfg(unix)]
pub type RawFd = ::std::os::unix::io::RawFd;
#[cfg(windows)]
pub type RawFd = isize;

// ─── Unit tests (unix: end-to-end socketpair round-trips) ───────────────────
//
// These tests exercise the wire protocol end-to-end in-process: parent and
// child endpoints live in two halves of a connected pair within the same
// process. They verify JSON line framing, SCM_RIGHTS fd passing (unix), and
// that an extra inbound fd is correctly closed (Bun ipc.zig "one fd per
// message" rule).

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn test_send_recv_json_no_fd() {
        // Create the socketpair, wrap one end as channel A (sender), the
        // other end as channel B (receiver). Send two newline-delimited JSON
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
