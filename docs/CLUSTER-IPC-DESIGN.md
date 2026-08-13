# Bao cluster/IPC — Rust 实现设计

> 基于 Bun 上游 `src/jsc/ipc.zig` 的架构，Rust 化实现 child_process IPC channel + cluster fd 传递。
> 复用锚点：`bun_spawn` Process struct + `std::os::unix::net::UnixStream`（SCM_RIGHTS）+ servo `GenericChannel`。

## 上游架构参考（Bun ipc.zig）

Bun 的 IPC 核心设计（`src/jsc/ipc.zig`）：

```
SendQueue {
  socket: UnixSocket(fd),       // 传输层
  incoming: LineBuffer,         // 接收缓冲(行分隔 JSON 或二进制帧)
  incoming_fd: Option<fd>,      // SCM_RIGHTS 接收的 fd(stash for onFd)
  queue: Vec<SendHandle>,       // 发送队列
  mode: JSON | Binary,          // 消息格式
}
```

- **传输**：Unix domain socket（`socketpair(AF_UNIX, SOCK_STREAM)`）
- **消息格式**：JSON 模式（行分隔 `\n`，最简）或 Binary 模式（`type:u8 + len:u32_le + payload`）
- **fd 传递**：`sendmsg`/`recvmsg` + `SCM_RIGHTS` ancillary data
- **消息类型**：Version(握手) / SerializedMessage(JS 值) / InternalMessage(cluster 内部)

## Rust 实现方案

### 核心结构（新文件 `src/bao_runtime/src/ipc_channel.rs`）

```rust
/// IPC channel for child_process / cluster fd passing.
/// Based on Bun's ipc.zig SendQueue, Rust-ified with std::os::unix.
pub struct IpcChannel {
    /// Unix socket fd (write end for parent→child, read end for child→parent).
    /// Created via socketpair(AF_UNIX, SOCK_STREAM) in spawn.
    socket: std::os::unix::net::UnixStream,

    /// Line-buffered JSON receive buffer (mirrors Bun's JSONLineBuffer).
    recv_buf: Vec<u8>,

    /// Stashed fd from last SCM_RIGHTS recvmsg (consumed by on_fd callback).
    incoming_fd: Option<std::os::fd::RawFd>,

    /// Whether the channel is still connected.
    connected: bool,
}
```

### 消息格式（JSON 模式，最简先行）

```rust
/// One IPC message = one JSON line terminated by \n.
/// Mirrors Bun's JSON mode (ipc.zig:38-40).
///
/// For fd passing: the message JSON has `{ "type": "NODE_HANDLE", ... }`
/// and the actual fd travels via SCM_RIGHTS ancillary data on the same
/// sendmsg call.
fn send_json(&mut self, json: &str) -> io::Result<()> {
    // Write JSON + \n to socket
    self.socket.write_all(json.as_bytes())?;
    self.socket.write_all(b"\n")
}

fn send_handle(&mut self, json: &str, fd: RawFd) -> io::Result<()> {
    // sendmsg with SCM_RIGHTS ancillary data + JSON payload
    // Based on Bun's ipc.zig SCM_RIGHTS path (line 515-517)
    send_fd_via_scm_right(&self.socket, json.as_bytes(), fd)
}
```

### fd 传递（SCM_RIGHTS via libc sendmsg/recvmsg）

```rust
/// Send a JSON message + fd via SCM_RIGHTS.
/// Uses libc sendmsg (not std, because std doesn't expose ancillary data).
unsafe fn send_fd_via_scm_right(
    socket: &UnixStream,
    payload: &[u8],
    fd: RawFd,
) -> io::Result<()> {
    // Build cmsg buffer for SCM_RIGHTS
    let mut cmsg_buf = [0u8; 64]; // enough for one fd
    let iov = libc::iovec { iov_base: payload.as_ptr() as *mut _, iov_len: payload.len() };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &iov as *const _ as *mut _;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut _;
    msg.msg_controllen = cmsg_buf.len();

    // CMSG_FIRSTHDR + CMSG_DATA for fd
    let cmsg = libc::CMSG_FIRSTHDR(&msg);
    (*cmsg).cmsg_level = libc::SOL_SOCKET;
    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
    (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>()) as _;
    let data = libc::CMSG_DATA(cmsg) as *mut RawFd;
    *data = fd;

    let ret = libc::sendmsg(socket.as_raw_fd(), &msg, 0);
    // ... error handling
}

/// Receive a JSON message + optional fd via SCM_RIGHTS.
unsafe fn recv_msg(socket: &UnixStream) -> io::Result<(Vec<u8>, Option<RawFd>)> {
    // recvmsg, extract payload + cmsg fd
    // ...
}
```

### 集成点

**cp_spawn（node_child_process.rs）**：
- 当 `stdio[3] === 'ipc'`（Node.js IPC channel）：`socketpair(AF_UNIX)` 创建一对 socket
- 父端 fd 存进 `child` 对象的 `channel` 属性
- 子端 fd 传给 spawn 的 child（作为 fd 3，Node.js 约定）
- `child.send(msg)` → `IpcChannel::send_json(msg)`
- `child.send(msg, handle)` → `IpcChannel::send_handle(msg, handle_fd)`
- `child.on('message', cb)` → poll child socket fd，recv → emit

**node_cluster.rs**：
- `cluster.fork()` → 用 cp_spawn + IPC channel spawn worker
- `worker.send(msg)` → child.send
- `worker.on('message')` → child.on('message')
- round-robin fd handoff：master 收到连接 fd，通过 IPC `send_handle` 传给选中的 worker

### 复用清单（不手写）

| 需要的能力 | 复用什么 | 来源 |
|---|---|---|
| Unix socket pair | `libc::socketpair(AF_UNIX, SOCK_STREAM)` | libc 标准调用 |
| SCM_RIGHTS send/recv | `libc::sendmsg`/`recvmsg` + `SCM_RIGHTS` | libc 标准调用 |
| JSON 序列化 | `serde_json` (已有 workspace dep) | Cargo.toml |
| Process struct | `bun_spawn::Process`（已有 spawn） | src/bun_spawn |
| 事件循环 poll | `bao_uloop` epoll tick（已有）| src/bao_uloop |
| servo GenericChannel | 可选：IPC 消息用 servo 的 GenericChannel 路由 | vendor/servo |

### 实现顺序

1. **IpcChannel struct**（send_json/recv_msg/send_handle/recv_handle）
2. **cp_spawn 集成**（`stdio[3]='ipc'` → socketpair + 挂 channel 属性）
3. **child.send / child.on('message')** JS 绑定
4. **node_cluster.rs**（fork + round-robin fd handoff）
5. **测试**（child_process_deep_tests + node_cluster conformance）

### 不做的事

- 不重写 bun_spawn 的 Process/PosixStdio（pipe 已对）
- 不用 servo devtools 的 IPC（那是 Firefox actor 协议）
- 不做 Windows named pipe IPC（Linux x86_64 only，和项目范围一致）
- 不做 Binary 模式（先 JSON，后续按需加）
