//! C15 coverage wave — `bun_http::http_thread` faces (REQ-ENG-010).
//! Complements the C7 suites: the cross-thread `schedule_*_from_any_thread`
//! drains for unknown/sentinel ids (the request path must not wedge), and
//! the process-exit face — `defer_shutdown_reclaim` + `shutdown_for_exit`
//! (ack + reclaim application + idempotent second call).
//!
//! Own test binary on purpose: `shutdown_for_exit` latches the process-global
//! `SHUTDOWN_REQUESTED`; per-file process isolation (the C7 idle_timeout
//! precedent) keeps sibling test binaries unaffected under both nextest and
//! plain `cargo test`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::http_thread::{TransportPauseKind, WriteMessageType, request_body_send_buffer_capacity};
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};

// Link seam: bun_io's posix event loop dispatch (see the C7 suites).
#[unsafe(no_mangle)]
extern "Rust" fn __bun_run_file_poll(_poll: *mut bun_io::FilePoll, _size_or_offset: i64) {}

#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    eprintln!("bun: out of memory");
    std::process::abort()
}

// ─── offline faces (no HTTP thread) ────────────────────────────────────────

/// Request-head buffer sizing: < 32 KiB → small pool, ≥ 32 KiB → large pool.
#[test]
fn request_body_send_buffer_capacity_thresholds_c15() {
    const SMALL: usize = 32 * 1024;
    const LARGE: usize = 512 * 1024;
    assert_eq!(request_body_send_buffer_capacity(0), SMALL);
    assert_eq!(request_body_send_buffer_capacity(SMALL - 1), SMALL);
    assert_eq!(request_body_send_buffer_capacity(SMALL), LARGE);
    assert_eq!(request_body_send_buffer_capacity(usize::MAX), LARGE);
}

/// Message-type vocabulary (Zig enum(u2) ports): discriminants and Eq.
#[test]
fn message_types_discriminants_and_eq_c15() {
    assert_eq!(WriteMessageType::Data as u8, 0);
    assert_eq!(WriteMessageType::End as u8, 1);
    assert!(WriteMessageType::Data == WriteMessageType::Data);
    assert!(WriteMessageType::Data != WriteMessageType::End);

    assert_eq!(TransportPauseKind::Pause as u8, 0);
    assert_eq!(TransportPauseKind::Resume as u8, 1);
    assert!(TransportPauseKind::Pause != TransportPauseKind::Resume);
}

/// `InitOpts::default` — no CAs, no CA file, not the install face.
#[test]
fn init_opts_default_shape_c15() {
    let opts = bun_http::http_thread::InitOpts::default();
    assert!(opts.ca.is_empty());
    assert!(opts.abs_ca_file_name.is_empty());
    assert!(!opts.for_install);
}

/// `LibdeflateState` is a 512 KiB zeroable scratch (the decompressor's
/// intermediate-output shared buffer).
#[test]
fn libdeflate_state_shape_c15() {
    let state = bun_http::http_thread::LibdeflateState {
        shared_buffer: [0u8; 512 * 1024],
    };
    assert_eq!(state.shared_buffer.len(), 512 * 1024);
    assert!(state.shared_buffer.iter().all(|&b| b == 0));
}

// ─── real-thread faces ─────────────────────────────────────────────────────

static RECLAIMED: AtomicBool = AtomicBool::new(false);
unsafe fn reclaim_probe(_ctx: *mut std::ffi::c_void) {
    RECLAIMED.store(true, Ordering::Release);
}

#[derive(Debug)]
struct Delivery {
    bytes: Vec<u8>,
    has_more: bool,
    fail: Option<bun_core::Error>,
    status: Option<u32>,
}

struct Recorder {
    tx: mpsc::Sender<Delivery>,
}

fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let _ = rec.tx.send(Delivery {
        bytes: result
            .body
            .as_deref()
            .map(|b| b.list.as_slice().to_vec())
            .unwrap_or_default(),
        has_more: result.has_more,
        fail: result.fail.clone(),
        status: result.metadata.as_ref().map(|m| m.response.status_code),
    });
    if !result.has_more {
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

fn spawn_ok_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            let _ = stream.flush();
        }
    });
    port
}

/// Run one real GET against the loopback server and return the terminal
/// delivery (the C7 recorder shape).
fn run_one_get(url: String) -> Delivery {
    let (tx, rx) = mpsc::channel();
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());

    let ah = AsyncHTTP::init(
        Method::GET,
        parsed_url,
        Default::default(),
        b"",
        response_buffer,
        b"",
        HTTPClientResultCallback::new(recorder, recorder_callback),
        FetchRedirect::Follow,
        options,
    );
    let ah_ptr = bun_core::heap::into_raw(Box::new(ah));
    let batch = bun_threading::thread_pool::Batch::from(unsafe {
        core::ptr::addr_of_mut!((*ah_ptr).task)
    });
    bun_http::HTTPThread::schedule(batch);

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            panic!("request must settle before the harness deadline");
        };
        let Ok(d) = rx.recv_timeout(remaining) else {
            panic!("request channel closed before terminal delivery");
        };
        if !d.has_more {
            return d;
        }
    }
}

/// The id-0 sentinel guards + unknown-id drains: every cross-thread schedule
/// with id 0 must early-return, and queues fed with an id that matches no
/// in-flight request must be drained by the HTTP thread's `drain_events`
/// without wedging a real request that runs afterwards.
#[test]
fn b_sentinel_and_unknown_id_schedules_do_not_wedge_requests_c15() {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    // id 0 = "no id" sentinel: every cross-thread schedule must no-op.
    bun_http::HTTPThread::schedule_shutdown_from_any_thread(0);
    bun_http::HTTPThread::schedule_request_write_from_any_thread(0, WriteMessageType::Data);
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(0, TransportPauseKind::Pause);
    bun_http::HTTPThread::schedule_response_body_drain_from_any_thread(0);

    // Unknown (never-issued) ids: pushed to the queues, drained, no effect —
    // no panic, no wedged queue, no leaked wakeup.
    bun_http::HTTPThread::schedule_response_body_drain_from_any_thread(0xDEAD_BEEF);
    bun_http::HTTPThread::schedule_shutdown_from_any_thread(0xDEAD_BEEF);
    bun_http::HTTPThread::schedule_request_write_from_any_thread(0xDEAD_BEEF, WriteMessageType::End);
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(0xDEAD_BEEF, TransportPauseKind::Pause);
    bun_http::HTTPThread::schedule_transport_pause_from_any_thread(0xDEAD_BEEF, TransportPauseKind::Resume);

    // The drains run on the HTTP thread's loop; a full request cycling the
    // loop afterwards proves the queue state is clean end to end.
    let port = spawn_ok_server();
    std::thread::sleep(Duration::from_millis(50));
    let terminal = run_one_get(format!("http://127.0.0.1:{port}/"));
    assert_eq!(terminal.fail, None, "request after bogus schedules: {terminal:?}");
    assert_eq!(terminal.status, Some(200));
    assert_eq!(terminal.bytes, b"ok");
}

/// Arm 1: HTTP thread never started — `shutdown_for_exit` must return
/// immediately (the `!HTTP_THREAD_INIT` early-return). Named `a_` so plain
/// `cargo test --test-threads=1` runs it first in this binary (fresh
/// process ⇒ the arm is actually taken); under nextest every test is its own
/// process, so ordering is irrelevant there.
#[test]
fn a_shutdown_for_exit_uninit_thread_returns_immediately_c15() {
    let start = Instant::now();
    bun_http::shutdown_for_exit();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "uninit arm must not wait on any shutdown handshake"
    );
}

/// Process-exit face on a live thread: `shutdown_for_exit` must ack, apply
/// deferred reclaims, and be idempotent on a second call.
#[test]
fn c_shutdown_for_exit_acks_applies_reclaims_and_is_idempotent_c15() {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    // Park a reclaim before exit: drop_fn must run on the JS-side thread
    // once the daemon acks (the ShutdownReclaim handoff contract).
    bun_http::defer_shutdown_reclaim(1 as *mut std::ffi::c_void, reclaim_probe);
    assert!(!RECLAIMED.load(Ordering::Acquire), "reclaim must wait for the ack");

    // Ensure has_awoken (a real request cycles the loop to completion).
    let port = spawn_ok_server();
    std::thread::sleep(Duration::from_millis(50));
    let terminal = run_one_get(format!("http://127.0.0.1:{port}/"));
    assert_eq!(terminal.status, Some(200), "pre-exit request must complete");

    bun_http::shutdown_for_exit();
    assert!(
        RECLAIMED.load(Ordering::Acquire),
        "acked shutdown must apply the deferred reclaim"
    );

    // Second call — already-shutdown state must be idempotent (no
    // double-apply, no hang: done-latch short-circuits the wait).
    bun_http::shutdown_for_exit();
    assert!(RECLAIMED.load(Ordering::Acquire));
}
