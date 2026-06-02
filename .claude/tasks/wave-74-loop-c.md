# Wave 74-LOOP-C — uSockets socket + poll dispatch on real epoll (unblocks P1-B)

> SSOT for the 74-C.x task breakdown. Persisted per W-2.3.2. Survives context compaction.

## 0. Problem statement

74-LOOP-A delivered the uSockets **loop core** on mio (`uws_get_loop`,
`us_loop_run_bun_tick`, `us_wakeup_loop`, defer/pre/post). But:

- `bao_uloop::run_mio_poll` (lib.rs:288-322) polls mio then **discards every
  event** — comment: "Wave 74-LOOP-C wires the registry side."
- Every `us_socket_*` / `us_socket_group_*` / `us_listen_socket_*` /
  `us_connecting_socket_*` symbol is a **no-op stub** in
  `bao_native_stubs/src/c_lib_stubs.rs`.
- Therefore `node_http` (P1-B) cannot move onto `bun_uws` sockets — there is no
  socket I/O dispatch.

## 1. THE load-bearing finding (gates everything)

`FilePoll::register_with_fd_impl` (`src/io/posix_event_loop.rs:654`):

```rust
let watcher_fd = loop_.fd;                       // reads PosixLoop.fd
...
linux::epoll_ctl(watcher_fd, op, fd.native(), &raw mut event);  // raw syscall
//   event.data.u64 = Pollable::init(self).ptr()  (tagged pointer)
```

- `loop_.fd` **MUST be a real epoll fd** (kqueue fd on macOS), or FilePoll
  registration silently targets fd `-1` and breaks.
- FilePoll cannot be redirected without editing upstream-mirrored `bun_io`
  (violates CLAUDE.md min-diff principle; FilePoll is used by fs.watch /
  child_process pipes / Bun.Socket / timerfd — huge blast radius).
- The loop's tick must `epoll_wait(loop_.fd, ready_polls, 1024, timeout)` and
  dispatch each ready event by its `data.u64` **tagged pointer**, routing to
  either a FilePoll (`FilePoll::on_epoll_event`) or a uSockets poll
  (socket / listen / callback).

### Backend options

| Opt | Approach | FilePoll change | Portability | Verdict |
|-----|----------|-----------------|-------------|---------|
| **A** | Raw `epoll_create1` → `loop_.fd`; `us_poll_*` = epoll_ctl wrappers; tick = raw `epoll_wait` + tagged dispatch; raw `eventfd` for wakeup. Faithful port of uSockets `epoll_kqueue.c`. | **none** | hand-written epoll+kqueue | **RECOMMENDED** |
| B | Keep mio, `loop_.fd = poll.as_raw_fd()`, mix raw epoll_ctl with mio-owned fd. | none | mio | rejected — mio assumes exclusive ownership of its registrations; fragile across versions; no real gain over A |
| C | `us_poll_*` registers via `mio::Registry`; **redirect FilePoll** to call `us_poll_*` instead of raw epoll_ctl. | invasive bun_io edits | mio | rejected — large blast radius, breaks min-diff |

**Recommendation: Option A.** Zero bun_io changes, FilePoll + us_socket share
one coherent epoll fd, total control over `ready_polls` / tagging /
edge-vs-level (dispatch correctness depends on this). Scope to **Linux epoll
first**; macOS kqueue is a tracked follow-up (74-C.9). Reworks ~200 LOC of the
74-LOOP-A mio skeleton — acceptable, it was always labelled "骨架".

> ⚠️ 74-C.0 is a GATE: this decision must be signed off (architect/user) before
> 74-C.1 starts. Picking B/C changes every downstream data structure.

## 2. Scope boundaries

IN scope (plain TCP socket dispatch for HTTP):
- Real epoll loop core + `us_poll_*` internal poll ABI + tagged dispatch
- `us_socket_t` concrete impl + `us_socket_*` ABI (~40 fns)
- `SocketGroup` (`#[repr(C)]`, layout already in `uws_sys/SocketGroup.rs`) +
  `us_socket_group_*` ABI
- `ListenSocket` + `ConnectingSocket` ABI + accept/connect loops
- Stub removal + force_link wiring
- bao_uloop socket-dispatch tests

OUT of scope (stay stubbed / deferred):
- TLS (`us_ssl_ctx_from_options` stays null) → Phase 74-TLS
- QUIC (`us_quic_*`) → deferred
- `uws_*` C++ App layer → node_http uses `us_socket_*`, not the App
- macOS kqueue → 74-C.9 (optional follow-up; platform is Linux)
- node_http rewrite itself = P1-B (consumes this ABI, separate wave)

## 3. Data structures

### bao_uloop owns the concrete `us_socket_t` (opaque ZST to callers)

Callers see `us_socket_t` as `opaque_ffi!` ZST — bao fully controls layout.
FilePoll / Loop.rs never walk it; only bao's own dispatch + SocketGroup do.

```rust
// src/bao_uloop/src/socket.rs
#[repr(C)]
struct BaoSocket {
    poll: BaoPoll,              // MUST be first: us_poll_t-compatible head (fd+type+events)
    group: *mut SocketGroup,    // owning context (intrusive list membership)
    prev: *mut BaoSocket,       // SocketGroup.head_sockets intrusive list
    next: *mut BaoSocket,
    kind: u8,                   // SocketKind tag
    flags: SocketFlags,         // shut_down / closed / established / allow_half_open
    timeout_ticks: u16,         // for us_socket_timeout (sweep against global_tick)
    long_timeout_minutes: u8,
    write_buf: SocketStreamBuffer, // backpressure queue (mirrors us_socket_stream_buffer_t)
    ext: [u8; 0],               // trailing ext_size bytes (allocated via malloc(sizeof+ext_size))
}
```

### `BaoPoll` — the internal `us_poll_t` (no Rust extern exists upstream)

```rust
// src/bao_uloop/src/poll.rs
#[repr(C)]
struct BaoPoll {
    fd: i32,
    poll_type: PollType,   // tag stored in HIGH u15 bits of data.u64 (same scheme as FilePoll)
    events: u32,           // EPOLLIN | EPOLLOUT currently armed
}

// ⚠️ CORRECTED: FilePoll uses TaggedPtr with tag in HIGH bits (49-63), NOT low bits.
// FilePoll's FILE_POLL_TAG = 1024 (bit 49-63). BaoPoll uses disjoint tags 1-4.
// Dispatch: let tag = (data_u64 >> 49) as u16; match tag { 0=>Wakeup, 1024=>FilePoll(ptr), 1..=4=>BaoPoll(...) }
#[repr(u16)]
enum PollType {            // MUST be disjoint from FilePoll's tag (1024)
    Socket = 1,
    ListenSocket = 2,
    SocketShutdown = 3,    // half-closed
    Callback = 4,          // generic fd callback (timerfd etc.)
}
```

> ⚠️ R5 CORRECTED: FilePoll encodes tag in **high u15 bits (bit 49-63)** via TaggedPtr
> (see ptr/tagged_pointer.rs:26-35). FILE_POLL_TAG = 1024. BaoPoll uses disjoint
> tags {1,2,3,4}. 74-C.2 dispatch decodes: `(data.u64 >> 49) as u16` → match arm.
> Pre-task: read `posix_event_loop.rs` Pollable::init/ptr/tag + tagged_pointer.rs.

### Loop core rework (74-C.1)

```rust
// src/bao_uloop/src/lib.rs — BaoLoopState, mio fields replaced
struct BaoLoopState {
    loop_ptr: *mut PosixLoop,
    epfd: i32,                 // epoll_create1(EPOLL_CLOEXEC) — also stored in (*loop_ptr).fd
    // CORRECTED (architect audit): wakeup eventfd stored in InternalLoopData.wakeup_async
    // (loop-reachable) so us_wakeup_loop works from ANY thread holding *mut Loop.
    // BaoLoopState no longer owns wakeup_fd directly.
    pending_wakeups: AtomicU32,
    deferred: VecDeque<DeferredCall>,
    pre_handlers: Vec<HandlerSlot>,
    post_handlers: Vec<HandlerSlot>,
    closed_head: *mut BaoSocket, // deferred-close list (re-entrancy safety, mirrors PosixLoop.closed_head)
    wakeup_cb: Option<LoopCb>,
    pre_cb: Option<LoopCb>,
    post_cb: Option<LoopCb>,
}
const WAKEUP_TAG: u64 = 0; // data.u64 == 0 ⇒ wakeup eventfd, skip in dispatch
// us_wakeup_loop: write((*loop_).internal_loop_data.wakeup_async, &1u64, 8) — no thread_local match needed
```

### SocketGroup / VTable — ALREADY defined, do not redefine

`src/uws_sys/SocketGroup.rs` has `#[repr(C)] SocketGroup` (9 ptr + u32 + u16 +
3×u8, static_assert'd) and `VTable` (11 `Option<extern fn>`). bao_uloop only
*implements* the `us_socket_group_*` functions over these; layout is fixed.

## 4. Task breakdown (TaskCreate-ready)

---
Task ID: 74-C.0
Title: GATE — sign off poll backend architecture (Option A recommended)
Files: .claude/tasks/wave-74-loop-c.md (this file)
LOC estimate: 0 (decision)
Depends on: —
Description: Confirm Option A (raw epoll, drop mio, Linux-first). Run
`architect(task_type=consult)` to validate the FilePoll-shares-loop.fd finding
and the raw-epoll port plan. No code until signed off — picking B/C rewrites all
downstream structs.

---
Task ID: 74-C.1
Title: Rework bao_uloop loop core to raw epoll + eventfd (replace mio)
Files: src/bao_uloop/src/lib.rs (rework ~200), src/bao_uloop/src/poll.rs (new, BaoPoll + tag decode helpers ~120)
LOC estimate: ~320
Depends on: 74-C.0
Description: Replace mio::Poll with epoll_create1 fd stored in (*loop_ptr).fd.
Replace mio::Waker with raw eventfd (register into epfd with WAKEUP_TAG). Rework
run_mio_poll → run_epoll: epoll_wait(epfd, (*loop_).ready_polls.as_mut_ptr(),
1024, timeout) → set num_ready_polls/current_ready_poll → drain wakeup eventfd
when WAKEUP_TAG fires. Keep the 7-phase tick (defer/pre_cb/pre_handlers/POLL/
post_handlers/post_cb/bump). Preserve all 6 existing 74-LOOP-A unit tests green.
Add tag-decode helper that, given a data.u64, returns enum {Wakeup, FilePoll(ptr),
BaoPoll(ptr)} — REQUIRES reading posix_event_loop.rs Pollable tag bits first.
Add #[trace] REQ-ENG-008.

---
Task ID: 74-C.2
Title: us_poll_* internal poll ABI + tagged ready-event dispatch
Files: src/bao_uloop/src/poll.rs (extend ~280)
LOC estimate: ~280
Depends on: 74-C.1
Description: Implement the internal poll layer uSockets keeps in C (no Rust
extern exists). Functions: poll_start(epoll_ctl ADD), poll_change(MOD),
poll_stop(DEL), poll_fd, poll_events, poll_set_events — all wrapping
epoll_ctl(loop_.fd,...) with event.data.u64 = (BaoPoll ptr | PollType tag).
Implement dispatch_ready_poll(poll, events): match decoded tag →
  Wakeup: drain eventfd;
  FilePoll(ptr): call FilePoll::on_epoll_event (via bun_io fn — verify symbol);
  BaoPoll(Socket): route to socket on_data/on_writable/on_close (74-C.3 hooks);
  BaoPoll(ListenSocket): accept loop (74-C.5).
Match FilePoll's epoll flag convention (level-triggered, no EPOLLET unless
oneshot) — R3. Re-entrancy: never close a socket inside the ready_polls loop;
push to closed_head, sweep after the loop (R4).

---
Task ID: 74-C.3
Title: BaoSocket concrete impl + us_socket_* ABI (~40 fns)
Files: src/bao_uloop/src/socket.rs (new ~520)
LOC estimate: ~520
Depends on: 74-C.1, 74-C.2
Description: Define #[repr(C)] BaoSocket (see §3) with trailing ext bytes
(malloc(size_of::<BaoSocket>()+ext_size)). Implement all us_socket_* per the
canonical ABI in src/uws_sys/us_socket_t.rs (NOT the lossy c_void stubs):
write/write2/raw_write (i32 len, no msg_more), flush, close(CloseCode,reason),
shutdown, shutdown_read, is_closed/is_shut_down/is_established, timeout/
long_timeout (setters, c_uint), keepalive(enable,delay), nodelay, ext,
group, kind/set_kind, get_fd/get_native_handle, local/remote_port,
local/remote_address, get_error, pause/resume, open, adopt, sendfile_needs_more,
free_stream_buffer. write() buffers on EAGAIN + arms EPOLLOUT (backpressure).
on_writable flushes write_buf. Wire callbacks through group.vtable. ext_size
discipline: allocate trailing bytes, return &ext via us_socket_ext.
PARALLELIZABLE: split the ~40 fn bodies across workers (worker_dispatch mode=pro)
once the struct + write/close/dispatch spine lands.

---
Task ID: 74-C.4
Title: SocketGroup impl + us_socket_group_* ABI (listen/connect/init)
Files: src/bao_uloop/src/socket_group.rs (new ~460)
LOC estimate: ~460
Depends on: 74-C.3
Description: Implement over the existing #[repr(C)] SocketGroup (uws_sys).
us_socket_group_init(group,loop,vt,ext): zero head lists, link into loop.
deinit/close_all: walk head_sockets + head_listen_sockets, close each.
us_socket_group_listen(group,kind,ssl_ctx,host,port,options,ext_size,err):
socket()+setsockopt(REUSE_PORT/ADDR per LIBUS_LISTEN_* flags)+bind()+listen()
→ alloc ListenSocket, poll_start(EPOLLIN), link head_listen_sockets, return.
listen_unix: AF_UNIX variant. connect(group,kind,ssl_ctx,host,port,options,
ext_size,is_connecting): getaddrinfo (sync OK for now) → socket(NONBLOCK) →
connect(); if EINPROGRESS → alloc ConnectingSocket, *is_connecting=1, arm
EPOLLOUT; else → BaoSocket, *is_connecting=0. connect_unix, from_fd, pair.
ssl_ctx!=null ⇒ return error (TLS deferred). Intrusive list insert/remove on
head_sockets via BaoSocket.prev/next.

---
Task ID: 74-C.5
Title: ListenSocket + ConnectingSocket ABI + accept/connect completion
Files: src/bao_uloop/src/listen.rs (new ~200), src/bao_uloop/src/connecting.rs (new ~210)
LOC estimate: ~410
Depends on: 74-C.3, 74-C.4
Description:
listen.rs — ListenSocket as opaque ZST over a BaoPoll(ListenSocket)+group+ext;
us_listen_socket_close/ext/get_fd/group; servername fns return no-op/0 (TLS
deferred); accept handler (called from 74-C.2 dispatch): accept4(NONBLOCK|CLOEXEC)
in a loop until EAGAIN → for each fd alloc BaoSocket, link group, poll_start
(EPOLLIN), invoke vtable.on_open.
connecting.rs — ConnectingSocket ZST; us_connecting_socket_* full ABI
(close/shutdown/shutdown_read/ext/get_error/get_native_handle/is_closed/
is_shut_down/timeout/long_timeout/group/kind/get_loop) — note stubs were MISSING
shutdown_read/group/kind/get_loop. EPOLLOUT-ready handler: getsockopt(SO_ERROR);
0 → promote to BaoSocket + vtable.on_open; err → vtable.on_connecting_error.

---
Task ID: 74-C.6
Title: Remove socket stubs from bao_native_stubs; fix force_link
Files: src/bao_native_stubs/src/c_lib_stubs.rs (delete ~150), src/bao_uloop/src/lib.rs (force_link extend), src/bao_uloop/Cargo.toml
LOC estimate: ~30 net (−150 +force_link refs)
Depends on: 74-C.3, 74-C.4, 74-C.5
Description: Delete the us_socket_* / us_socket_group_* / us_listen_socket_* /
us_connecting_socket_* stubs now provided by bao_uloop (1a/1b/1d in ABI catalog).
KEEP: us_ssl_* (2, TLS deferred), us_quic_* (QUIC deferred), uws_* App stubs.
Extend bao_uloop::force_link() to reference every new #[no_mangle] socket symbol
so linker GC doesn't drop them. Update force_c_lib_stubs keep-alive. WATCH the
Wave 74-A lesson: a stub that looks orphaned is reached via FFI extern blocks —
verify cargo test links before declaring a stub deletable.

---
Task ID: 74-C.7
Title: bao_uloop socket-dispatch integration tests
Files: src/bao_uloop/tests/socket_dispatch_tests.rs (new ~420)
LOC estimate: ~420
Depends on: 74-C.6
Description: AAA, no mocks. (1) echo server: init group with vtable, listen on
127.0.0.1:0, std::net::TcpStream client connects, on_data echoes, assert client
reads back payload — drive via us_loop_run_bun_tick ticks. (2) backpressure:
large write returns partial, EPOLLOUT flushes remainder. (3) close path:
us_socket_close fires vtable.on_close, fd removed from epoll. (4) connect path:
us_socket_group_connect to a local listener, on_open fires. (5) cross-thread
us_wakeup_loop unblocks a blocking tick with a live socket registered.
(6) FilePoll coexistence: register a pipe FilePoll + a socket on the same loop,
assert both dispatch. Pull bao_native_stubs dev-dep + NATIVE_STUBS anchor.

---
Task ID: 74-C.8  [optional / follow-up]
Title: macOS kqueue backend for the poll layer
Files: src/bao_uloop/src/poll.rs (cfg(macos) arm ~250)
LOC estimate: ~250
Depends on: 74-C.7
Description: kqueue()/kevent64 mirror of the epoll path; EventType already
cfg-aliased to kevent64_s in PosixLoop. Independent of Linux path — can run in
parallel with any later wave. Skip if Linux-only ship is acceptable.

## 5. Parallelization

The honest shape: this is ONE coherent subsystem (poll → socket → group →
listen/connect), so the spine is a dependency chain, not a fan-out.

```
74-C.0 (gate)
   └─ 74-C.1 ── 74-C.2 ── 74-C.3 ──┬── 74-C.4 ── 74-C.5 ── 74-C.6 ── 74-C.7
                                    │                                    └─ unblocks P1-B
                                    └─ (within .3: split ~40 us_socket_* bodies via worker_dispatch pro)
   74-C.8 (kqueue) ‖ independent, any time after .7
```

Concurrency opportunities (limited, be honest):
- Inside 74-C.3: once struct + write/close/dispatch spine exists, the remaining
  ~30 trivial accessor bodies (timeout/kind/ext/ports/addresses/error) split
  cleanly across workers (worker_dispatch, mode=pro, ~5 concurrency).
- 74-C.7 test scaffolding can be drafted in parallel with .4/.5.
- 74-C.8 (kqueue) is fully independent.
Everything else is sequential — do not pretend otherwise.

## 6. Total LOC

~320 + 280 + 520 + 460 + 410 + 30 + 420 = **~2,440 LOC** (matches the P1-B
blocker note's "1500-2500 LOC Phase 级变更"). +250 if kqueue (74-C.8).

## 7. Risk assessment

| ID | Risk | Severity | Mitigation |
|----|------|----------|-----------|
| R1 | loop_.fd must be a real epoll fd or FilePoll silently breaks (epoll_ctl(-1)) | HIGH | Option A puts epoll_create1 fd in loop_.fd; 74-C.7 test (6) proves coexistence |
| R5 | dispatch can't tell FilePoll Pollable ptr from BaoPoll ptr in shared epoll set | HIGH | 74-C.1/.2 pre-task: read Pollable tag scheme; reserve disjoint PollType tag bits; verify with FilePoll coexistence test |
| R4 | re-entrant close during ready_polls iteration → use-after-free | HIGH | deferred-close via closed_head, sweep after loop (mirrors uSockets); never free mid-iteration |
| R3 | edge vs level triggering mismatch → busy-loop or missed events | MED | match FilePoll's level-triggered convention exactly; assert in echo test |
| R2 | BaoSocket layout / ext_size / intrusive-list consistency | MED | bao owns layout; static_assert offsets; close_all walks via prev/next |
| R6 | Wave 74-A trap: deleting a "no-op" stub breaks FFI link | MED | 74-C.6 verifies cargo test links before deleting each stub |
| R7 | SIGSEGV on test teardown (known SpiderMonkey dtor issue) masks real failures | LOW | run bao_uloop tests without SM context; assert before teardown |
| R8 | getaddrinfo sync blocking in connect() | LOW | acceptable for P1-B (HTTP server side dominant); async DNS = later wave |

## 8. Verification (W-2.5)

Per-task: write code → align to ABI in us_socket_t.rs/SocketGroup.rs → first
compile → unit/integration test. Final: `cargo test -p bao_uloop`,
`cargo test -p bao_runtime` (zero regression), `cargo build --workspace`,
`cargo clippy -p bao_uloop -- -D warnings`. Then P1-B can start.
