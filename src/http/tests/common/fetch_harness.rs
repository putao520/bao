//! Shared client-side fetch harness for the wire-level integration tests:
//! the `Delivery`/`Recorder` pair and `recorder_callback` for the
//! status/fail-shaped suites, the terminal-delivery reclamation shared by
//! every recorder variant, and the `run_h2_fetch` driver. The
//! body/tls/pause-shaped suites (`tls_info_and_streaming_tests`,
//! `transport_backpressure_tests`) keep their own `Delivery`/`Recorder` and
//! reuse only `reclaim_terminal_delivery`, so per-binary dead code here is
//! the module-inclusion mechanism at work, not rot.
#![allow(dead_code)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use bun_core::MutableString;
use bun_http::signals::Store;
use bun_http::{AsyncHTTP, FetchRedirect, HTTPClientResult, HTTPClientResultCallback, Method,
               async_http};

/// One delivery reduced to the status/fail shape the h2 suites assert on.
#[derive(Debug)]
pub(crate) struct Delivery {
    pub(crate) status: Option<u32>,
    pub(crate) fail: Option<bun_core::Error>,
    pub(crate) has_more: bool,
}

pub(crate) struct Recorder {
    pub(crate) tx: mpsc::Sender<Delivery>,
}

/// Terminal-delivery reclamation, shared by every recorder variant: reclaim
/// the caller-thread `AsyncHTTP` box via the `real` backref plus the
/// response buffer — sole dropper, mirroring `on_http_done` in
/// fetch_async.rs. The HTTP-thread clone is raw-deallocated by
/// `on_async_http_callback_raw`.
pub(crate) fn reclaim_terminal_delivery(async_http: *mut AsyncHTTP<'static>) {
    let real = unsafe { (*async_http).real };
    if let Some(r) = real {
        drop(unsafe { Box::from_raw(r.as_ptr()) });
    }
    let buf = unsafe { (*async_http).response_buffer };
    if !buf.is_null() {
        drop(unsafe { Box::from_raw(buf) });
    }
}

/// The `HTTPClientResultCallback`; runs on the HTTP thread.
pub(crate) fn recorder_callback(
    this: *mut Recorder,
    async_http: *mut AsyncHTTP<'static>,
    result: HTTPClientResult<'_>,
) {
    let rec: &Recorder = unsafe { &*this };
    let status = result.metadata.as_ref().map(|m| m.response.status_code);
    let fail = result.fail.clone();
    let has_more = result.has_more;

    if !has_more {
        reclaim_terminal_delivery(async_http);
    }

    let _ = rec.tx.send(Delivery {
        status,
        fail,
        has_more,
    });
}

/// Drive one GET through the real HTTPThread over ALPN-negotiated h2 and
/// collect deliveries until the terminal (`has_more == false`) one.
/// `configure` installs suite-specific options (e.g. the stealth-style
/// SETTINGS payload) on top of the common signals + self-signed-cert base.
pub(crate) fn run_h2_fetch(
    port: u16,
    configure: impl FnOnce(&mut async_http::Options),
) -> Vec<Delivery> {
    bao_native_stubs::force_link();
    bun_core::Output::init_test();
    bun_http::http_thread::init(&Default::default());

    let (tx, rx) = mpsc::channel();
    // Leaked on purpose: the Signals NonNulls point into this store for the
    // whole request lifetime; a stable heap address avoids any relocation.
    let store: &'static mut Store = Box::leak(Box::new(Store::default()));
    let recorder = Box::into_raw(Box::new(Recorder { tx }));

    let url = format!("https://127.0.0.1:{}/", port);
    let url_bytes: &'static [u8] = Box::leak(url.into_bytes().into_boxed_slice());
    let parsed_url = bun_url::URL::parse(url_bytes);

    let response_buffer = Box::into_raw(Box::new(MutableString::default()));
    let mut options = async_http::Options::default();
    options.signals = Some(store.to());
    // Self-signed fixture cert.
    options.reject_unauthorized = Some(false);
    configure(&mut options);

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

    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
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
