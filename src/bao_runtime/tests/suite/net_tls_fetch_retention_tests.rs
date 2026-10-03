// @trace TEST-ENG-RETENTION [req:REQ-ENG-007] [level:e2e]
// GC over-retention locks for the upstream bun SEMANTIC-PORT batch:
//
// 1. bun 367d939d9a (node:net/tls: a handle whose socket dies before it
//    connects must not pin its wrapper): bao's transposed retention model
//    differs by construction — `net.connect` resolves synchronously inside
//    the native call and hands JS a numeric key (the native side roots no
//    JS wrapper at all), and `tls.connect` roots the early TLSSocket in
//    `TLS_CONNS` only while the native connection exists (the Close event
//    removes the entry and unroots). Both locks assert the property the
//    upstream fix established: after the wrapper is dropped by JS and a
//    full GC runs, nothing native holds it — the failed-connect rounds
//    collect to zero, a referenced control socket survives.
//
// 2. bun b4315fa551 (fetch, Bun.spawn: do not root the AbortSignal they
//    were given): bao's fetch abort wiring holds no JS state — the
//    listener is a closure-free native trampoline carrying an int id, and
//    ABORT_REGISTRY holds only Arc<AtomicBool>, removed when the fetch
//    settles. The lock is the upstream probe transposed: a Response
//    reachable only from an `abort` listener on its signal is collected
//    after the controller and response are dropped, while a mid-flight
//    abort (nothing but the signal alive, full GC run in between) still
//    reaches the in-flight fetch. (Bun.spawn has no signal option in bao —
//    no carrier to lock.)
//
// Wire-level: real loopback servers; per-test forced GCs via `Bun.gc`
// (SM NonIncrementalGC Shrink). Test 3 parks the HTTPThread — it
// dispatches through exit_isolation like the other fetch e2e files.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use mozjs::rooted;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Pump from abort_signal_timeout_tests: realm-entered timer drain +
/// MiniEventLoop tick + RunJobs, until `poll_js` stops carrying PENDING.
fn pump_until_settled(ctx: &mut JsContext, poll_js: &str, timeout: Duration) -> String {
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + timeout;
    let mut final_poll = String::new();
    while Instant::now() < deadline {
        {
            let mut cxm = ctx.cx();
            let global = bao_engine::context::thread_realm_global();
            if let Some(g) = global {
                rooted!(&in(cxm) let g_root = g);
                let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                timers::drain_and_check(realm_cx);
            } else {
                timers::drain_and_check(&mut cxm);
            }
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        std::thread::sleep(Duration::from_millis(2));
        final_poll = eval_string(ctx, poll_js);
        if !final_poll.contains("PENDING") {
            break;
        }
    }
    final_poll
}

fn new_ctx() -> JsContext {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ─── 1a. node:net — failed connects leave no retained wrappers ─────────────

/// 200 `net.connect()`s to a refused loopback port, dropped immediately
/// (the upstream reporter's per-round loop): after the error events drain
/// and a full GC, every wrapper must be collectible — the round must not
/// accumulate TCPSocket objects (upstream: 101, 201, 301 per round before
/// the fix). A referenced control socket survives the same GCs.
#[test]
fn net_connect_failed_wrappers_are_collectible_after_gc() {
    let mut ctx = new_ctx();
    assert_eq!(
        eval_string(
            &mut ctx,
            "String(typeof WeakRef === 'function' && typeof Bun.gc === 'function')"
        ),
        "true",
        "WeakRef + Bun.gc are the retention probe instruments; missing = harness broken"
    );

    let setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var net = require('net');
            var refs = [];
            (function () {
                for (var i = 0; i < 200; i++) {
                    var s = net.connect(1, '127.0.0.1'); // port 1: refused on loopback
                    refs.push(new WeakRef(s));
                    s.on('error', function () {}); // drain the deferred error event
                }
            })();
            // control: a referenced socket must survive the same GCs
            // (pinned on globalThis so the control itself stays reachable)
            var live = net.connect(1, '127.0.0.1');
            live.on('error', function () {});
            globalThis.__liveSocket = live;
            var liveRef = new WeakRef(live);
            globalThis.__pollNet = function () {
                var dead = 0;
                for (var i = 0; i < refs.length; i++) {
                    if (refs[i].deref() === undefined) dead++;
                }
                return 'dead=' + dead + '/' + refs.length +
                    '|liveSurvives=' + (liveRef.deref() !== undefined);
            };
            globalThis.__gcNet = function () {
                Bun.gc(true); Bun.gc(true);
                return 'gced';
            };
            return 'scheduled';
        })()
        "#,
    );
    assert_eq!(setup, "scheduled", "net retention setup failed");

    // the deferred error events fire and their closures die
    let drained = pump_until_settled(&mut ctx, "globalThis.__pollNet()", Duration::from_secs(15));
    assert!(
        drained.contains("liveSurvives=true"),
        "control socket must still be referenced after the pump: {}",
        drained
    );

    let gced = eval_string(&mut ctx, "globalThis.__gcNet()");
    assert_eq!(gced, "gced");
    let poll = eval_string(&mut ctx, "globalThis.__pollNet()");
    assert_eq!(
        poll,
        "dead=200/200|liveSurvives=true",
        "failed net.connect wrappers must collect to zero after full GC \
         (upstream 367d939d9a transposed: no native root pins an idle wrapper): {}",
        poll
    );
}

// ─── 1b. node:tls — failed tls.connect releases its registry root ──────────

/// tls.connect to a closed port: ClientError + Close remove the TLS_CONNS
/// entry (the early TLSSocket's RawValueRootGuard drops with it); after
/// the JS reference is dropped and a full GC runs, the wrapper must be
/// collectible. This is the face where a leak would be visible — the
/// native side DOES root the wrapper while the connection exists, so the
/// Close-path release is the load-bearing line.
#[test]
fn tls_connect_failed_wrapper_is_collectible_after_gc() {
    let mut ctx = new_ctx();

    let setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            var tls = require('tls');
            var out = { closed: "PENDING", alive: "PENDING" };
            var sock = tls.connect({
                host: '127.0.0.1',
                port: 1, // closed: TCP connect fails, ClientError + Close
                rejectUnauthorized: false,
            });
            sock.on('error', function () {});
            sock.on('close', function () { out.closed = 'closed'; });
            var ref = new WeakRef(sock);
            globalThis.__pollTls = function () {
                return out.closed + '|' + String(ref.deref() !== undefined);
            };
            globalThis.__dropAndGcTls = function () {
                sock = null;
                Bun.gc(true); Bun.gc(true);
                out.alive = String(ref.deref() !== undefined);
                return out.closed + '|aliveAfterGc=' + out.alive;
            };
            return 'scheduled';
        })()
        "#,
    );
    assert_eq!(setup, "scheduled", "tls retention setup failed");

    let polled = pump_until_settled(&mut ctx, "globalThis.__pollTls()", Duration::from_secs(15));
    assert!(
        polled.starts_with("closed|"),
        "tls.connect to a closed port must reach Close within the pump window: {}",
        polled
    );

    let dropped = eval_string(&mut ctx, "globalThis.__dropAndGcTls()");
    assert_eq!(
        dropped,
        "closed|aliveAfterGc=false",
        "the failed tls.connect wrapper must be collectible once Close removed \
         the TLS_CONNS root (upstream 367d939d9a terminal-teardown contract): {}",
        dropped
    );
}

// ─── 2. fetch — the abort listener does not root the Response ──────────────

fn start_retention_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let slow = loop {
                        match stream.read(&mut buf) {
                            Ok(0) => break false,
                            Ok(_) => {
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break buf.windows(7).any(|w| w == b"/r-slow");
                                }
                            }
                            Err(_) => break false,
                        }
                    };
                    if slow {
                        std::thread::sleep(Duration::from_millis(800));
                    }
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    stream.write_all(resp.as_bytes()).ok();
                    let _ = stream.flush();
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    port
}

/// The upstream leak probe (b4315fa551) transposed to bao:
///   A. a Response reachable ONLY from an `abort` listener on its own
///      fetch's signal collects once the controller and response are
///      dropped (bao's trampoline listener carries an int id and holds no
///      JS state — the signal → listener → Response cycle cannot form);
///   B. a mid-flight abort still reaches the in-flight fetch after a full
///      GC ran while the signal was the only thing keeping the wiring.
#[test]
fn fetch_abort_listener_does_not_root_the_response() {
    crate::exit_isolation::dispatch(
        "net_tls_fetch_retention_tests::fetch_abort_listener_does_not_root_the_response",
        fetch_abort_retention_body,
    );
}

fn fetch_abort_retention_body() {
    let port = start_retention_server();
    std::thread::sleep(Duration::from_millis(50));
    let mut ctx = new_ctx();

    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
        (function() {{
            var out = {{ settled: "PENDING", gcDone: "PENDING" }};
            // Phase A: response + signal held only by the abort-listener
            // shape; nobody ever aborts this controller.
            var refs = [];
            var probe = {{}};
            (function () {{
                var controller = new AbortController();
                var chain = fetch('http://127.0.0.1:{port}/r-fast', {{ signal: controller.signal }});
                probe.chain = new WeakRef(chain);
                chain
                    .then(function (response) {{
                        // upstream leak shape: a listener closes over the response.
                        // Nobody ever aborts this controller: the listener only
                        // proves the wiring exists; its lifetime is the signal
                        // and both collect together.
                        controller.signal.addEventListener('abort', function () {{
                            out.leaked = 'listener fired: response was rooted';
                        }});
                        probe.response = new WeakRef(response);
                        probe.signal = new WeakRef(controller.signal);
                        // the body IS read: an unread streamed body legitimately
                        // parks the transport reference — this probe measures
                        // the abort wiring, not that backpressure
                        return response.text();
                    }})
                    .then(function () {{
                        // the closure returns: controller, chain, response all
                        // unreachable from here
                    }})
.catch(function (e) {{ out.settled = 'ERROR:' + e; }});
            }})();
            // Phase P (diagnostic, unasserted): a bare fetch whose body is
            // never read. Evidence recorded 2026-10-03: an unread streamed
            // body parks the streaming transport reference, and the native
            // promise root then prevents the stream's finalize-cancel — the
            // settled promise + response stay rooted (plain=true) while a
            // bare `Promise.resolve` collects (bare=false). That per-fetch
            // leak is abort-independent and is reported separately; this
            // test reads the bodies it probes so it measures only the abort
            // wiring.
            (function () {{
                var plain = fetch('http://127.0.0.1:{port}/r-fast');
                probe.plain = new WeakRef(plain);
                plain.then(function () {{ /* drained */ }});
                // discriminator: a bare resolved promise, no fetch involved
                var bare = Promise.resolve(1);
                bare.then(function () {{}});
                probe.bare = new WeakRef(bare);
            }})();
            // Phase B: in-flight abort with nothing alive but the signal
            var slowController = new AbortController();
            fetch('http://127.0.0.1:{port}/r-slow', {{ signal: slowController.signal }})
                .then(function (r) {{
                    out.settled = 'SLOW-RESOLVED:' + r.status; // must NOT happen
                }})
                .catch(function (e) {{
                    out.settled = 'rejected:' + (e && e.name);
                }});
            setTimeout(function () {{
                slowController.abort();
            }}, 120);
            // post-settle GC runs AFTER both fetches are fully settled, so
            // any late reclaim tick has drained before the probes are read
            globalThis.__gcFetchRetention = function () {{
                Bun.gc(true); Bun.gc(true);
                out.gcDone = 'gced';
                return 'gced';
            }};

            globalThis.__pollFetchRetention = function () {{
                return 'chain=' + String(probe.chain.deref() !== undefined) +
                    '|resp=' + (probe.response ? String(probe.response.deref() !== undefined) : 'pending') +
                    '|sig=' + (probe.signal ? String(probe.signal.deref() !== undefined) : 'pending') +
                    '|plain=' + (probe.plain ? String(probe.plain.deref() !== undefined) : 'pending') +
                    '|bare=' + (probe.bare ? String(probe.bare.deref() !== undefined) : 'pending') +
                    '|settled=' + out.settled +
                    '|gcDone=' + out.gcDone +
                    '|leaked=' + (out.leaked || 'no');
            }};
            return 'scheduled';
        }})()
        "#
        ),
    );
    assert_eq!(setup, "scheduled", "fetch retention setup failed");

    let poll = pump_until_settled(
        &mut ctx,
        "globalThis.__pollFetchRetention()",
        Duration::from_secs(15),
    );
    // extra drain rounds: the transport-side tasklet handoff may land on a
    // ConcurrentTask after the settle marker the pump stopped at
    let _ = pump_until_settled(&mut ctx, "globalThis.__pollFetchRetention()", Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(300));
    let _ = pump_until_settled(&mut ctx, "globalThis.__pollFetchRetention()", Duration::from_secs(3));
    let gced = eval_string(&mut ctx, "globalThis.__gcFetchRetention()");
    assert_eq!(gced, "gced");
    let poll = eval_string(&mut ctx, "globalThis.__pollFetchRetention()");
    let parts: Vec<&str> = poll.split('|').collect();
    assert_eq!(
        parts[0], "chain=false",
        "the fetch chain promise must be collectible after settle: {}", poll
    );
    assert_eq!(
        parts[1], "resp=false",
        "the response held only by the abort-listener shape must collect after \
         the fetch settled and full GCs ran (upstream b4315fa551 transposed: no \
         GC root rides the fetch's signal wiring): {}",
        poll
    );
    assert_eq!(
        parts[2], "sig=false",
        "the signal must collect with its listener: {}", poll
    );
    assert_eq!(
        parts[5], "settled=rejected:AbortError",
        "the mid-flight abort must still reach the in-flight fetch after the \
         GC pressure window: {}",
        poll
    );
    assert_eq!(
        parts[6], "gcDone=gced",
        "the GC-pressure window must have run before the mid-flight abort: {}",
        poll
    );
    assert_eq!(
        parts[7], "leaked=no",
        "the never-aborted listener must stay inert — firing it would mean \
         the response was still rooted: {}",
        poll
    );

    eprintln!(
        "[PASS] TEST-ENG-RETENTION fetch/abort: response-only-via-listener collects, \
         in-flight abort survives GC pressure"
    );

    // Exit strategy of every fetch e2e file: the parked HTTPThread is a
    // non-daemon thread; force-exit inside the exit-isolated process.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
