//! Connection deadline semantics under CPU oversubscription — both
//! directions of "the deadline must not be punched through by scheduling":
//!
//! 1. **Healthy connections are never mis-killed by load.** The uSockets
//!    idle timer is *tick-counted*, not wall-clock-deadline-checked:
//!    `us_socket_timeout` (packages/bun-usockets/src/socket.c:104) arms
//!    `group->timestamp + ceil(seconds/4)`, and only
//!    `us_internal_timer_sweep` (loop.c:169) advances `group->timestamp` —
//!    exactly one tick per sweep-timerfd period (4s wall, loop.c:55-61).
//!    A starved loop thread therefore *lags* the tick counter; firing can
//!    only be delayed, never advanced. This test drives concurrent
//!    requests through the real HTTPThread while spinner threads
//!    oversubscribe every core (the in-process analogue of the observed
//!    single-core-pinned 20-parallel-tests flake) and asserts nobody is
//!    misjudged `error.Timeout`. It fails if the single-clock-domain
//!    invariant is ever rewritten — e.g. a wall-clock `now > deadline`
//!    check at wake, or "catch-up" tick advancement for missed timerfd
//!    periods, both of which mix time domains and fire early under load.
//! 2. **Genuinely stalled connections still time out.** A server that
//!    accepts and never responds must still produce `error.Timeout` once
//!    the (shortened) idle deadline passes — load delays the firing but
//!    never removes it.
//!
//! Both phases share one `#[test]` so the `IDLE_TIMEOUT_SECONDS` override
//! in phase 2 cannot race a parallel phase 1 (libtest runs test fns on
//! separate threads).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};

// Link seam: bun_io's posix event loop dispatches through
// `__bun_run_file_poll`, owned by `bun_runtime::dispatch` in product
// binaries (bun_runtime is higher-tier than bun_http and cannot be
// dev-depped from here). No FilePoll sources are registered anywhere in
// these tests, so a no-op satisfies the link-time reference.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

// Link seam for `__bun_crash_handler_out_of_memory`: bun_alloc resolves it
// at link time against bun_crash_handler (higher-tier than this crate's
// test binary). OOM aborts the process either way — a faithful test stub.
#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

/// One delivery, reduced to what the deadline assertions need.
#[derive(Debug, Clone)]
struct Outcome {
    status: Option<u32>,
    failed: bool,
    timed_out: bool,
    has_more: bool,
}

struct Recorder {
    tx: std::sync::mpsc::Sender<Outcome>,
}

/// The `HTTPClientResultCallback`; runs on the HTTP thread.
fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let _ = rec.tx.send(Outcome {
        status: result.metadata.as_ref().map(|m| m.response.status_code),
        failed: result.fail.is_some(),
        timed_out: result.is_timeout(),
        has_more: result.has_more,
    });

    if !result.has_more {
        // Terminal delivery: reclaim the caller-thread AsyncHTTP box via
        // the `real` backref plus the response buffer — sole dropper,
        // mirroring the contract in tls_info_and_streaming_tests.
        let real = unsafe { (*async_http).real };
        if let Some(r) = real {
            drop(unsafe { Box::from_raw(r.as_ptr()) });
        }
        let buf = unsafe { (*async_http).response_buffer };
        if !buf.is_null() {
            drop(unsafe { Box::from_raw(buf) });
        }
    }
}

/// Schedule one GET on the real HTTPThread; deliveries arrive on the
/// returned receiver until the terminal (`has_more == false`) outcome.
fn spawn_request(url: String) -> std::sync::mpsc::Receiver<Outcome> {
    let (tx, rx) = std::sync::mpsc::channel();
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let ah = AsyncHTTP::init(
        Method::GET,
        parsed_url,
        Default::default(),
        b"",
        response_buffer,
        b"",
        HTTPClientResultCallback::new(recorder, recorder_callback),
        FetchRedirect::Follow,
        async_http::Options::default(),
    );
    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);
    rx
}

/// Collect deliveries until the terminal one, bounded by `bound`.
fn collect_bounded(rx: &std::sync::mpsc::Receiver<Outcome>, bound: Duration) -> Vec<Outcome> {
    let deadline = Instant::now() + bound;
    let mut out = Vec::new();
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let Ok(d) = rx.recv_timeout(remaining) else {
            break;
        };
        let terminal = !d.has_more;
        out.push(d);
        if terminal {
            break;
        }
    }
    out
}

/// Spinner threads that oversubscribe every core for the duration of a
/// phase — the in-process analogue of pinning the whole test fleet to one
/// core. Dropping the guard stops and joins them.
struct CpuHogs {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl CpuHogs {
    fn spawn(multiplier: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let handles = (0..cores.saturating_mul(multiplier))
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self { stop, handles }
    }
}

impl Drop for CpuHogs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

/// Accept loop that serves every accepted stream on a dedicated
/// per-connection thread until `deadline_secs` passes.
fn spawn_accept_server<F>(deadline_secs: u64, serve: F) -> u16
where
    F: Fn(std::net::TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let serve = Arc::new(serve);
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(deadline_secs);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let serve = Arc::clone(&serve);
                    std::thread::spawn(move || serve(stream));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    port
}

fn read_request_head(stream: &mut std::net::TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 4096];
    let mut seen = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

#[test]
fn connection_deadline_both_directions_under_oversubscription() {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    // ── Phase 1: healthy + immediate server under full oversubscription ──
    //
    // With the default 300s idle timeout the short timer needs ≥74 sweep
    // ticks ⇒ ≥292s wall before it can fire; CPU starvation only pushes
    // firing further out. Every request must therefore complete cleanly —
    // any `error.Timeout` here is a mis-fire caused by a broken deadline
    // clock domain, not by the configured deadline.
    let prev_idle = bun_http::IDLE_TIMEOUT_SECONDS.load(Ordering::Relaxed);
    bun_http::IDLE_TIMEOUT_SECONDS.store(300, Ordering::Relaxed);

    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_sink = Arc::clone(&accepted);
    let port = spawn_accept_server(60, move |mut stream| {
        accepted_sink.fetch_add(1, Ordering::Relaxed);
        read_request_head(&mut stream);
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    });
    std::thread::sleep(Duration::from_millis(20));

    const CONCURRENT: usize = 8;
    // Schedule every request first, then collect: all 8 are in flight on
    // the HTTPThread at once while the hogs starve the machine.
    let rxs: Vec<_> = (0..CONCURRENT)
        .map(|i| spawn_request(format!("http://127.0.0.1:{port}/load-{i}")))
        .collect();
    let hogs = CpuHogs::spawn(2);
    let start = Instant::now();
    let results: Vec<Vec<Outcome>> = rxs
        .iter()
        .map(|rx| collect_bounded(rx, Duration::from_secs(30)))
        .collect();
    let elapsed = start.elapsed();
    drop(hogs);

    assert_eq!(
        accepted.load(Ordering::Relaxed),
        CONCURRENT,
        "healthy server must have served every request"
    );
    for (i, deliveries) in results.iter().enumerate() {
        let terminal = deliveries
            .last()
            .unwrap_or_else(|| panic!("request {i}: no delivery at all (hung?)"));
        assert!(
            !terminal.has_more,
            "request {i}: last delivery must be terminal, got {terminal:?}"
        );
        assert!(
            !terminal.failed,
            "request {i} mis-killed under load: {terminal:?} (all: {deliveries:?})"
        );
        assert_eq!(
            terminal.status,
            Some(200),
            "request {i} status (all: {deliveries:?})"
        );
        assert!(
            !terminal.timed_out,
            "request {i} was misjudged error.Timeout under load — deadline clock domain broken"
        );
    }
    assert!(
        elapsed < Duration::from_secs(25),
        "phase 1 took {elapsed:?} — oversubscription degraded into starvation"
    );

    // ── Phase 2: stalled server still produces error.Timeout ─────────────
    //
    // 4s idle timeout = 1 sweep tick: on_open arms the short timer and the
    // next tick (≤4s wall + starvation lag) fires on_timeout → error.Timeout.
    // The hogs prove the direction-2 property under the same load that
    // phase 1 survived: firing is delayed, never removed.
    bun_http::IDLE_TIMEOUT_SECONDS.store(4, Ordering::Relaxed);

    let stall_port = spawn_accept_server(60, move |mut stream| {
        // Accept and read the request, then hold the connection open
        // without ever responding — a genuinely stalled peer.
        read_request_head(&mut stream);
        let park = Instant::now() + Duration::from_secs(45);
        while Instant::now() < park {
            std::thread::sleep(Duration::from_millis(100));
            let mut probe = [0u8; 16];
            match stream.read(&mut probe) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });
    std::thread::sleep(Duration::from_millis(20));

    let stall_rx = spawn_request(format!("http://127.0.0.1:{stall_port}/stall"));
    let hogs = CpuHogs::spawn(2);
    let start = Instant::now();
    let deliveries = collect_bounded(&stall_rx, Duration::from_secs(30));
    let elapsed = start.elapsed();
    drop(hogs);

    bun_http::IDLE_TIMEOUT_SECONDS.store(prev_idle, Ordering::Relaxed);

    let terminal = deliveries
        .last()
        .expect("stalled request must terminate (idle deadline), got no delivery");
    assert!(
        terminal.timed_out,
        "stalled server must fail with error.Timeout, got {terminal:?} (all: {deliveries:?})"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "timeout firing delayed to {elapsed:?} — tick lag grew unbounded"
    );

    // Exit strategy mirrors tls_info_and_streaming_tests: the parked
    // HTTPThread and the servers' accept-loop threads are non-daemon;
    // force-exit sidesteps waiting out their deadlines.
    bun_http::http_thread::shutdown_for_exit();
    std::process::exit(0);
}
