// @trace TEST-ENG-006-RUNTIME-CLEANUP [req:REQ-ENG-006] [level:integration]
//
// B1 slice 2 段一(用户裁决 2026-09-17 A「drop 时未 close 资源必须 close」):
// BaoRuntime::drop must terminate every dgram UDP socket the runtime owns
// (fd closed + port released), without touching sockets owned by other live
// runtimes (per-token isolation).
//
// B1 slice 2 段二(same ruling): BaoRuntime::drop must also terminate every
// worker_threads Worker the runtime owns (OS thread joined — a leaked worker
// thread pins its own JSContext + stack for the life of the process),
// without touching workers owned by other live runtimes.
//
// Assertions are fully behavioral — the token/registry internals are
// pub(crate), invisible to this harness:
//   T1: while the runtime is alive the fd is a real socket and its port is
//       occupied; after `drop` the fd link is gone and the port is
//       immediately re-bindable.
//   T2: two runtimes (A first, B second — B parasitizes A's JSContext per
//       JsContext::init_runtime) each bind a socket; dropping B closes only
//       B's socket (A's port stays occupied, A's fd stays open, A can still
//       send/receive); dropping A then closes A's socket too.
//   T3: two workers spawned inside one runtime are alive while it lives and
//       their OS threads are gone after `drop`.
//   T4: two runtimes each spawn a worker; dropping B joins only B's worker
//       (A's worker thread stays alive and still accepts postMessage);
//       dropping A then joins A's worker too.

use std::net::UdpSocket;
use std::time::Duration;

/// Reserve a free ephemeral port and release it immediately. The caller
/// binds the dgram socket to it right away (small TOCTOU window; a collision
/// surfaces as a loud bind failure in the test, never a silent pass).
fn free_ephemeral_port() -> u16 {
    let probe = UdpSocket::bind("127.0.0.1:0").expect("probe bind 127.0.0.1:0");
    probe.local_addr().expect("probe local_addr").port()
}

fn eval_number(rt: &mut bun_runtime::BaoRuntime, src: &str) -> f64 {
    match rt.eval(src, "<runtime-cleanup-test>").expect("eval must succeed") {
        bao_engine::value::JsValue::Number(n) => n,
        _ => panic!("eval returned a non-number: {}", src),
    }
}

/// Bind a fresh dgram socket to 127.0.0.1:`port` inside `rt`'s realm via the
/// real user path (`require('node:dgram')` + `createSocket` + `bind`), stop
/// the 1ms recv-poll interval the JS wrapper starts (a live interval keeps
/// the post-eval event-loop drain in `BaoRuntime::eval` non-empty forever),
/// and return `(fd, actual bound port)`.
fn bind_dgram(rt: &mut bun_runtime::BaoRuntime, port: u16, slot: &str) -> (i32, u16) {
    rt.eval(
        &format!(
            r#"
            var dgram = require('node:dgram');
            globalThis.{slot} = dgram.createSocket('udp4');
            globalThis.{slot}.bind({port}, '127.0.0.1');
            clearInterval(globalThis.{slot}._recvTimer);
            "#,
            slot = slot,
            port = port
        ),
        "<runtime-cleanup-test>",
    )
    .expect("dgram createSocket+bind must succeed");
    let fd = eval_number(rt, &format!("globalThis.{slot}._fd")) as i32;
    let bound = eval_number(rt, &format!("globalThis.{slot}.address().port")) as u16;
    assert!(fd >= 0, "bind must produce a valid fd, got {}", fd);
    (fd, bound)
}

/// `/proc/self/fd/<fd>` link target, if the fd still exists.
fn fd_link_target(fd: i32) -> Option<String> {
    std::fs::read_link(format!("/proc/self/fd/{}", fd))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// T1: a socket bound inside a runtime is released when the runtime drops.
#[test]
fn runtime_drop_closes_dgram_sockets_fd_and_port_released() {
    let port = free_ephemeral_port();
    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    let (fd, bound) = bind_dgram(&mut rt, port, "__sock_t1");
    assert_eq!(bound, port, "dgram socket must bind the requested port");

    // Alive: the fd is a real socket and the port is occupied.
    let target = fd_link_target(fd)
        .unwrap_or_else(|| panic!("/proc/self/fd/{} must exist while the runtime is alive", fd));
    assert!(
        target.starts_with("socket:"),
        "fd {} should be a socket, link target = {}",
        fd,
        target
    );
    assert!(
        UdpSocket::bind(("127.0.0.1", port)).is_err(),
        "port {} must be occupied while the runtime is alive",
        port
    );

    // Drop: the runtime-owned resource must be terminated (fd closed, port
    // freed) — 用户裁决 2026-09-17 A.
    drop(rt);
    let after = fd_link_target(fd);
    assert!(
        after.is_none() || after.as_deref() != Some(target.as_str()),
        "fd {} must be closed after runtime drop (still {:?})",
        fd,
        after
    );
    UdpSocket::bind(("127.0.0.1", port))
        .expect("port must be immediately re-bindable after runtime drop");
}

/// T2: per-token isolation — dropping one runtime must not terminate another
/// live runtime's sockets, and the survivor stays functional.
#[test]
fn runtime_drop_is_per_token_other_runtimes_sockets_untouched() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("receiver bind");
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("receiver read timeout");
    let recv_port = receiver.local_addr().expect("receiver local_addr").port();

    let port_a = free_ephemeral_port();
    let mut rt_a = bun_runtime::BaoRuntime::new().expect("runtime A");
    let (fd_a, bound_a) = bind_dgram(&mut rt_a, port_a, "__sock_t2a");
    assert_eq!(bound_a, port_a);

    let port_b = free_ephemeral_port();
    // Second runtime on this thread parasitizes the live JSContext
    // (`JsContext::init_runtime` hands back a context without a new
    // SmRuntimeGuard) — exactly the multi-runtime shape the token slot must
    // keep isolated.
    let mut rt_b = bun_runtime::BaoRuntime::new().expect("runtime B");
    let (fd_b, bound_b) = bind_dgram(&mut rt_b, port_b, "__sock_t2b");
    assert_eq!(bound_b, port_b);

    // Both alive: both ports occupied, both fds real sockets.
    let target_a = fd_link_target(fd_a)
        .unwrap_or_else(|| panic!("/proc/self/fd/{} (A) must exist", fd_a));
    let target_b = fd_link_target(fd_b)
        .unwrap_or_else(|| panic!("/proc/self/fd/{} (B) must exist", fd_b));
    assert!(target_a.starts_with("socket:") && target_b.starts_with("socket:"));
    assert!(
        UdpSocket::bind(("127.0.0.1", port_a)).is_err(),
        "A's port must be occupied while A is alive"
    );
    assert!(
        UdpSocket::bind(("127.0.0.1", port_b)).is_err(),
        "B's port must be occupied while B is alive"
    );

    // Drop B (the most recent runtime): ONLY B's socket may be terminated.
    drop(rt_b);
    let b_after = fd_link_target(fd_b);
    assert!(
        b_after.is_none() || b_after.as_deref() != Some(target_b.as_str()),
        "B's fd {} must be closed when B drops (still {:?})",
        fd_b,
        b_after
    );
    UdpSocket::bind(("127.0.0.1", port_b)).expect("B's port must be released when B drops");
    assert!(
        fd_link_target(fd_a)
            .unwrap_or_else(|| panic!("A's fd must stay open after B dropped"))
            .starts_with("socket:"),
        "A's fd must still be a live socket after B dropped"
    );
    assert!(
        UdpSocket::bind(("127.0.0.1", port_a)).is_err(),
        "A's port must stay occupied after B dropped (per-token isolation)"
    );

    // A is still fully functional: a datagram sent through A's surviving
    // socket (the same registry entry A stamped at bind time) arrives on a
    // plain std socket.
    let sent = eval_number(
        &mut rt_a,
        &format!(
            "__dgram_send_buf(globalThis.__sock_t2a._fd, [66, 65, 79], {recv_port}, '127.0.0.1')"
        ),
    ) as usize;
    assert_eq!(sent, 3, "A's surviving socket must send the full datagram");
    let mut buf = [0u8; 16];
    let (n, _) = receiver
        .recv_from(&mut buf)
        .expect("receiver must receive the datagram from A's surviving socket");
    assert_eq!(
        &buf[..n],
        b"BAO",
        "datagram from A's surviving socket must arrive intact"
    );

    // Drop A: its socket is terminated too.
    drop(rt_a);
    let a_after = fd_link_target(fd_a);
    assert!(
        a_after.is_none() || a_after.as_deref() != Some(target_a.as_str()),
        "A's fd {} must be closed when A drops (still {:?})",
        fd_a,
        a_after
    );
    UdpSocket::bind(("127.0.0.1", port_a)).expect("A's port must be released after A drops");
}

// ── B1 slice 2 段二: WORKER_REGISTRY drop sweep (row 24) ──────────────────

fn eval_string(rt: &mut bun_runtime::BaoRuntime, src: &str) -> String {
    match rt.eval(src, "<runtime-cleanup-test>").expect("eval must succeed") {
        bao_engine::value::JsValue::String(s) => s,
        _ => panic!("eval returned a non-string: {}", src),
    }
}

/// Write a worker script to a unique temp file, return its path.
fn write_worker_file(tag: &str, body: &str) -> String {
    let path = ::std::env::temp_dir().join(format!(
        "bao-worker-cleanup-{}-{}.js",
        tag,
        ::std::process::id()
    ));
    ::std::fs::write(&path, body).expect("write worker file");
    path.to_string_lossy().into_owned()
}

/// Whether an OS thread named exactly `name` is alive in this process
/// (Linux `/proc/self/task/<tid>/comm`; worker threads are named
/// `bao-worker-<threadId>` at spawn).
fn worker_thread_alive(name: &str) -> bool {
    let tasks = match ::std::fs::read_dir("/proc/self/task") {
        Ok(d) => d,
        Err(_) => return false,
    };
    tasks.flatten().any(|entry| {
        ::std::fs::read_to_string(entry.path().join("comm"))
            .map(|n| n.trim_end() == name)
            .unwrap_or(false)
    })
}

/// Poll `pred` until it holds (workers are spawned asynchronously, so both
/// the appearance and the disappearance of their threads are asynchronous).
fn wait_until(pred: impl Fn() -> bool, what: &str) {
    let deadline = ::std::time::Instant::now() + ::std::time::Duration::from_secs(10);
    while ::std::time::Instant::now() < deadline {
        if pred() {
            return;
        }
        ::std::thread::sleep(::std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for: {}", what);
}

/// Spawn an idle worker inside `rt` via the real user path
/// (`require('worker_threads')` + `new Worker`), wait until its OS thread is
/// observable, and return the thread's name. The idle script keeps the
/// worker in its message receive loop — nothing for the drop sweep to race.
fn spawn_idle_worker(rt: &mut bun_runtime::BaoRuntime, slot: &str, tag: &str) -> String {
    let worker_path = write_worker_file(tag, "self.onmessage = function() {};");
    let tid = eval_string(
        rt,
        &format!(
            r#"
(function() {{
  var wt = require('worker_threads');
  globalThis.{slot} = new wt.Worker({worker_path:?});
  return String(globalThis.{slot}.threadId);
}})()
"#
        ),
    );
    let thread_name = format!("bao-worker-{}", tid);
    wait_until(
        || worker_thread_alive(&thread_name),
        &format!("worker thread {} alive after spawn", thread_name),
    );
    thread_name
}

/// T3: workers spawned inside a runtime are terminated (their OS threads
/// exit) when the runtime drops — a leaked worker thread would pin its own
/// JSContext + stack for the life of the process.
#[test]
fn runtime_drop_joins_owned_worker_threads() {
    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    let w1 = spawn_idle_worker(&mut rt, "__wT3a", "t3a");
    let w2 = spawn_idle_worker(&mut rt, "__wT3b", "t3b");
    assert_ne!(w1, w2, "the two workers must be distinct OS threads");

    drop(rt);
    // Idle workers observe Terminate on their next recv — the bounded join
    // in cleanup_for_token returns well within the 5s deadline.
    wait_until(
        || !worker_thread_alive(&w1) && !worker_thread_alive(&w2),
        "both worker threads to exit after runtime drop",
    );
}

/// T4: per-token isolation — dropping one runtime must not terminate another
/// live runtime's workers, and the survivor keeps working.
#[test]
fn runtime_drop_is_per_token_other_runtimes_workers_untouched() {
    let mut rt_a = bun_runtime::BaoRuntime::new().expect("runtime A");
    let wa = spawn_idle_worker(&mut rt_a, "__wT4a", "t4a");
    // Second runtime on this thread parasitizes the live JSContext — the
    // same multi-runtime shape the token slot must keep isolated.
    let mut rt_b = bun_runtime::BaoRuntime::new().expect("runtime B");
    let wb = spawn_idle_worker(&mut rt_b, "__wT4b", "t4b");
    assert_ne!(wa, wb);

    // Drop B: ONLY B's worker may be terminated.
    drop(rt_b);
    wait_until(
        || !worker_thread_alive(&wb),
        "B's worker thread to exit when B drops",
    );
    assert!(
        worker_thread_alive(&wa),
        "A's worker thread must survive B's drop (per-token isolation)"
    );

    // A is still fully functional: its surviving worker accepts messages
    // through the unchanged JS API surface.
    rt_a
        .eval(
            "globalThis.__wT4a.postMessage('still-alive');",
            "<runtime-cleanup-test>",
        )
        .expect("postMessage into A's surviving worker must succeed");
    assert!(
        worker_thread_alive(&wa),
        "A's worker thread must still be alive after postMessage"
    );

    // Drop A: its worker is terminated too.
    drop(rt_a);
    wait_until(
        || !worker_thread_alive(&wa),
        "A's worker thread to exit when A drops",
    );
}
