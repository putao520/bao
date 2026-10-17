//! Idle-timeout firing coverage (`HTTPClient::on_timeout` — REQ-ENG-010).
//!
//! Own test binary on purpose: `BUN_CONFIG_HTTP_IDLE_TIMEOUT` is read once
//! in the HTTP thread's `on_start`, so the env must be set before the
//! process's first `http_thread::init`. A separate top-level test target
//! guarantees process isolation under both nextest and plain `cargo test`
//! (one binary = one process), regardless of intra-crate test ordering.
//!
//! Budget: env=1s normalizes to 1+LIBUS_TIMEOUT_GRANULARITY(4)=5s at
//! `on_start`; a server that accepts but never answers must fail the
//! request with the `Timeout` error name right around that budget.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::signals::Store;
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

#[derive(Debug)]
struct Delivery {
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

/// A server that accepts, drains the request head, and never answers —
/// holds the connection open so the client's idle timer is the only way
/// out.
fn spawn_silent_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
            std::thread::spawn(move || {
                // Drain the request head, then hold the socket silently.
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                std::thread::sleep(Duration::from_secs(30));
            });
        }
    });
    port
}

/// A silent peer must fail the request with `Timeout` once the normalized
/// idle budget elapses — never a hang, never a fake success.
#[test]
fn silent_server_times_out_with_timeout_error() {
    // Must precede http_thread::init: on_start reads this once.
    // SAFETY: single-threaded test bring-up (no other thread reads env), the
    // established test idiom for pre-runtime env wiring in this repo.
    unsafe { std::env::set_var("BUN_CONFIG_HTTP_IDLE_TIMEOUT", "1") };

    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    // The accessor reflects the normalized value: 1s + 4s short-wheel pad.
    let normalized = u64::from(bun_http::idle_timeout_seconds());
    assert_eq!(
        normalized, 5,
        "env=1 must normalize to 1 + LIBUS_TIMEOUT_GRANULARITY(4)"
    );

    let port = spawn_silent_server();
    std::thread::sleep(Duration::from_millis(50));

    let (tx, rx) = mpsc::channel();
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url = format!("http://127.0.0.1:{port}/");
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
    let scheduled_at = Instant::now();
    bun_http::HTTPThread::schedule(batch);

    // Wait past the 5s budget + uSockets wheel slack, but well under the
    // server's 30s hold.
    let deadline = scheduled_at + Duration::from_secs(20);
    let mut terminal = None;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(d) if !d.has_more => {
                terminal = Some(d);
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let elapsed = scheduled_at.elapsed();
    let term = terminal.expect("silent-server request must settle via timeout");

    assert_eq!(
        term.fail.as_ref().map(|e| e.name()),
        Some("Timeout"),
        "idle timeout must surface as the Timeout error: {term:?}"
    );
    assert_eq!(term.status, None, "no response ever arrived");
    assert!(
        elapsed >= Duration::from_secs(4),
        "timeout must not fire before the budget: elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "timeout must fire near the budget, not at the harness deadline: elapsed={elapsed:?}"
    );
}
