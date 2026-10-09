// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:integration]
//
// SEMANTIC-PORT batch 2 (large items) — upstream settle-timing contracts ported
// to the bao Node.js/Bun compatibility face (REQ-ENG-007):
//
//   67e94f074b  streams: bytes()/arrayBuffer() settle on a direct stream at
//               end()/close(), not when pull() returns (#42950)
//   1e31339ae5  streams: settle a native sink's owner when a direct stream
//               closes, not when pull() returns (#42957)
//   1a6b0d99db  Bun.serve: send a response that JavaScript produced when it
//               completes (#42815)
//
// Verdicts (upstream carriers are JSC/C++ — the public behavior is what ports):
//
//   Item 1 = EQUIVALENT. Bao has no `type: "direct"` ReadableStream at all —
//     web_streams.js:165-173 accepts only `type === "bytes"` and
//     `type === undefined`, anything else throws RangeError, so the upstream
//     defect's precondition (a one-shot direct-sink consumer waiting on an
//     async pull that outlives its own end()/close()) is structurally absent.
//     Every whole-body consumer bao does have (`node:stream/consumers`
//     arrayBuffer/bytes/text/buffer — node_stream_consumers.rs:24-41,
//     async-iterator loops; Request.arrayBuffer via _bao_drain_stream —
//     web_fetch_classes.rs:607-621, reader pump) reads until the reader sees
//     done, i.e. it settles at close() by construction. The upstream
//     contract ("no consumer waits for pull() to return") is locked below
//     with the queued-model analog of the upstream hang shapes.
//
//   Item 2 = N-A-HOST. The upstream defect lives in readDirectStream's
//     native-sink owner wait (serve RequestContext holding the request until
//     pull() settles → server.stop() never resolves). Bao's serve pipeline
//     feeds no JS stream into any native sink: serve_write_response_object
//     (bun_api.rs:5721) reads only `_bodyText`/`_bodyBytes` (bun_api.rs:5987)
//     and ends the response synchronously (bun_api.rs:6003) — a ReadableStream
//     response body produces an empty body and the request lifetime ends in
//     the same onData callback. The other upstream sink owners are equally
//     absent: fetch() upload fails closed at the relay boundary
//     (fetch_api.rs:305), Bun.write fails closed for stream bodies
//     (bun_api.rs:8310), spawn stdin has no ReadableStream feed, and bao has
//     no HTMLRewriter global. There is no owner that can wait on pull().
//
//   Item 3 = EQUIVALENT. Bao's vendored bun-uws already carries the post-fix
//     C++ machinery (`sendWhenComplete` HttpResponse.h:143,
//     `HTTP_SEND_WHEN_COMPLETE` HttpResponseData.h:157,
//     `uncorkCompletedResponse` HttpResponse.h:129). Bao's dispatch shape
//     closes the regression window structurally: all handler JS (including
//     the Promise<Response> spin, bun_api.rs:5563) runs BEFORE the response
//     write, the write (`end(.., close_connection=true)`, bun_api.rs:6003) is
//     the terminal action of the onData callback, and because every serve
//     response marks connection-close the parser's CONNECTION_CLOSE gate
//     (HttpContext.h:437-449) ends the read instead of dispatching another
//     pipelined handler — no JS can run between response completion and the
//     onData-tail uncork, so the cork can never hold a completed JS response
//     behind trailing JavaScript. The lock below pins that contract with the
//     upstream test's own shape: a handler schedules 400ms of post-completion
//     JS and the client must already hold the full response.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bun_runtime::timers;
#[path = "common/mod.rs"]
mod common;

use common::drive_event_loop;

use common::eval_string_full as eval_string;


/// Drive until `source` evaluates to a string containing `needle`, or the
/// deadline passes (then the last value is returned for diagnostics). Each
/// pass is the full bun:test pump: microtasks (RunJobs) + `drain_one_pass`
/// (uWS tick + due wall-clock timers + pumps — the serve-spin driver).
fn drive_until_string(ctx: &mut JsContext, source: &str, needle: &str, max_ms: u64) -> String {
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    let mut last = String::new();
    while Instant::now() < deadline {
        let cx_raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
            timers::drain_one_pass(cx_raw);
        }
        std::thread::sleep(Duration::from_millis(1));
        last = eval_string(ctx, source);
        if last.contains(needle) {
            return last;
        }
    }
    last
}

/// One raw HTTP/1.1 GET, pumping the JS-thread MiniEventLoop between read
/// attempts (the uWS dispatch only runs when the loop ticks — a blocking
/// read on this thread would starve it and time out). Returns the full raw
/// response bytes, or None when the 5s deadline passed.
fn http_get(ctx: &mut JsContext, port: u16, path: &str) -> Option<Vec<u8>> {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(25))).ok();
    sock.set_nonblocking(false).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    sock.write_all(req.as_bytes()).ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        drive_event_loop(ctx, 3);
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
        // Complete responses end with connection close (end(…, close=true)).
        if let Some(sep) = out.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&out[..sep]).to_ascii_lowercase();
            if let Some(cl) = head.lines().find_map(|l| {
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            }) {
                if out.len() >= sep + 4 + cl {
                    break;
                }
            }
        }
    }
    Some(out)
}

/// Split a raw response into (status_line, headers_block, body_bytes).
fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let pos = raw
        .windows(sep.len())
        .position(|w| w == sep)
        .expect("response must contain header/body separator");
    let head = String::from_utf8_lossy(&raw[..pos]).into_owned();
    let body = raw[pos + sep.len()..].to_vec();
    (head, body)
}

// ────────────────────────────────────────────────────────────────────────────
// Item 1 — 67e94f074b: whole-body consumers settle at end()/close(), not at
// pull() return. Verdict: EQUIVALENT (direct-stream mechanism structurally
// absent; the consumers that exist settle at close by construction).
// ────────────────────────────────────────────────────────────────────────────

/// The mechanism's precondition is absent: bao's ReadableStream rejects any
/// underlying-source type other than "bytes"/undefined (web_streams.js:172),
/// so there is no direct stream, no one-shot direct sink, and no consumer
/// that could adopt a pull()-settle capability for one.
#[test]
fn semantic_port_stream_direct_type_is_structurally_rejected() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let result = eval_string(
        &mut ctx,
        r#"
        globalThis.__p1 = (function() {
          function tryType(t) {
            try {
              new ReadableStream({ type: t, pull: function(c) {} });
              return 'NO-THROW';
            } catch (e) {
              return (e && e.constructor && e.constructor.name) + ': ' + (e && e.message);
            }
          }
          return JSON.stringify({
            direct: tryType('direct'),
            bytes: tryType('bytes'),
            undef: tryType(undefined),
          });
        })();
        globalThis.__p1
    "#,
    );
    assert!(
        result.contains("\"direct\":\"RangeError"),
        "type:'direct' must be rejected (the upstream defect's precondition is \
         structurally absent in bao), got: {result}"
    );
    assert!(
        result.contains("\"bytes\":\"NO-THROW\"") && result.contains("\"undef\":\"NO-THROW\""),
        "bytes/default controllers must keep working, got: {result}"
    );
}

/// The upstream hang shapes, mapped to bao's queued model: a pull() that
/// enqueues + closes and then NEVER returns must not hold any whole-body
/// consumer — each one settles at close() with the enqueued content. The
/// close(error) analog (error after enqueue) must reject the consumer.
/// Carriers locked: node:stream/consumers arrayBuffer/text/bytes/buffer and
/// Request.arrayBuffer (the _bao_drain_stream reader-pump face).
#[test]
fn semantic_port_stream_whole_body_consumers_settle_at_close_not_pull_return() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    eval_string(
        &mut ctx,
        r#"
        globalThis.__p2 = (function() {
          var enc = new TextEncoder();
          var dec = new TextDecoder();
          function never() { return new Promise(function() {}); }
          // Upstream "async pull: (write,) end(), then never returns" shape —
          // queued-model analog: enqueue + close, then hang forever.
          function makeSettleAtClose() {
            return new ReadableStream({
              async pull(c) {
                c.enqueue(enc.encode('hello'));
                c.close();
                await never();
              },
            });
          }
          // Upstream "async pull: close(error), then never returns" shape —
          // queued-model analog: error after enqueue, then hang forever.
          function makeFailAfterClose() {
            return new ReadableStream({
              async pull(c) {
                c.enqueue(enc.encode('partial'));
                c.error(new Error('source failed'));
                await never();
              },
            });
          }
          function raced(p, tag) {
            return Promise.race([p, new Promise(function(_, rej) {
              setTimeout(function() { rej(new Error('TIMEOUT-STILL-PENDING ' + tag)); }, 500);
            })]);
          }
          var cm = null;
          try { cm = require('node:stream/consumers'); } catch (e) { cm = null; }
          function toText(x) {
            if (typeof x === 'string') return x;
            if (x instanceof Uint8Array) return dec.decode(x);
            if (x instanceof ArrayBuffer) return dec.decode(new Uint8Array(x));
            return String(x);
          }
          var lines = [];
          function job(name, p, wantText, wantErr) {
            return raced(p, name).then(
              function(v) {
                var got = toText(v);
                lines.push(name + '=' + (wantErr
                  ? 'UNEXPECTED-RESOLVE:' + got.slice(0, 30)
                  : ((got === wantText) ? 'OK' : 'MISMATCH:' + got.slice(0, 30))));
              },
              function(e) {
                var msg = (e && e.message) || String(e);
                lines.push(name + '=' + (wantErr
                  ? ((msg.indexOf(wantErr) !== -1) ? 'OK-REJ' : 'WRONG-REJ:' + msg.slice(0, 40))
                  : 'UNEXPECTED-REJ:' + msg.slice(0, 40)));
              }
            );
          }
          function reqWith(makeBody) {
            return new Request('http://bao.local/x', { method: 'POST', body: makeBody() }).arrayBuffer();
          }
          function cons(name, method, makeBody, wantText, wantErr) {
            return job(name, (cm && cm[method]) ? cm[method](makeBody()) : Promise.reject(new Error('module-missing')), wantText, wantErr);
          }
          var jobs = [
            cons('cons_arrayBuffer', 'arrayBuffer', makeSettleAtClose, 'hello', null),
            cons('cons_text', 'text', makeSettleAtClose, 'hello', null),
            cons('cons_bytes', 'bytes', makeSettleAtClose, 'hello', null),
            cons('cons_buffer', 'buffer', makeSettleAtClose, 'hello', null),
            job('req_arrayBuffer', reqWith(makeSettleAtClose), 'hello', null),
            cons('cons_arrayBuffer_err', 'arrayBuffer', makeFailAfterClose, null, 'source failed'),
            job('req_arrayBuffer_err', reqWith(makeFailAfterClose), null, 'source failed'),
          ];
          Promise.all(jobs).then(function() {
            globalThis.__p2r = lines.join('|') + '|DONE';
          });
          return 'started';
        })();
        'started'
    "#,
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__p2"),
        "started",
        "consumption jobs must be scheduled"
    );

    let result = drive_until_string(
        &mut ctx,
        "globalThis.__p2r === undefined ? 'pending' : globalThis.__p2r",
        "DONE",
        10_000,
    );
    assert!(
        result.contains("DONE"),
        "consumer matrix must finish within the deadline (a TIMEOUT-STILL-PENDING \
         entry means a consumer hung — the upstream defect shape), got: {result}"
    );
    let cells: Vec<&str> = result.split('|').filter(|c| !c.is_empty() && *c != "DONE").collect();
    assert_eq!(
        cells.len(),
        7,
        "all seven consumer cells must be judged, got: {result}"
    );
    for cell in &cells {
        assert!(
            cell.ends_with("=OK") || cell.ends_with("=OK-REJ"),
            "settle-at-close contract violated in cell: {cell} (full: {result})"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Item 2 — 1e31339ae5: a native sink's owner settles when the stream closes.
// Verdict: N-A-HOST — bao's serve pipeline feeds no JS stream into any native
// sink, so no owner exists that could wait for pull() to return.
// ────────────────────────────────────────────────────────────────────────────

/// Structural-absence lock: a serve handler returning `new
/// Response(readableStream)` completes the request synchronously (empty body,
/// no stream pump in the loop) — the request lifetime never extends into the
/// stream, which is exactly why the upstream defect (owner held until pull()
/// settles; server.stop() never resolves) is unreachable. When a future wave
/// wires streaming response bodies into serve_write_response_object, this
/// lock is the marker to revisit together with the settle-timing contract.
#[test]
fn semantic_port_serve_stream_response_body_completes_without_sink_owner_wait() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__srv3 = Bun.serve({
          port: 0,
          hostname: "127.0.0.1",
          fetch: function(req) {
            var p = new URL(req.url).pathname;
            if (p === "/stream-body") {
              return new Response(new ReadableStream({
                start: function(c) { c.enqueue(new TextEncoder().encode("never-on-the-wire")); }
              }));
            }
            if (p === "/never-pull") {
              // pull() returns a promise that NEVER settles — the upstream
              // hang source. An owner-wait design would hold this request.
              return new Response(new ReadableStream({
                pull: function(c) { return new Promise(function() {}); }
              }));
            }
            return new Response("plain", { status: 200 });
          },
        });
        '' + globalThis.__srv3.port
    "#,
    );
    let port: u16 = setup.trim().parse().expect("serve must report its port");
    assert!(port > 0, "serve must bind an ephemeral port");
    drive_event_loop(&mut ctx, 10);

    for path in ["/stream-body", "/never-pull"] {
        let started = Instant::now();
        let raw = http_get(&mut ctx, port, path)
            .unwrap_or_else(|| panic!("{path}: response must complete (bounded), not hang"));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{path}: the response must complete without waiting on the stream \
             (no sink-owner wait exists in bao's serve pipeline)"
        );
        let (head, body) = split_response(&raw);
        let status_line = head.lines().next().unwrap_or("");
        assert!(
            status_line.starts_with("HTTP/1.1 200"),
            "{path}: status must be 200, got: {status_line}"
        );
        assert!(
            body.is_empty(),
            "{path}: with no native sink wired, the stream body is not fed to the \
             wire (structural absence under test) — got {} bytes: {:?}",
            body.len(),
            body
        );
    }

    // The upstream symptom (server.stop() never resolving) has no carrier:
    // stop() is a synchronous native teardown and must return.
    let stopped = eval_string(&mut ctx, "globalThis.__srv3.stop(), 'stopped'");
    assert_eq!(stopped, "stopped", "server.stop() must return promptly");
    drive_event_loop(&mut ctx, 10);
}

// ────────────────────────────────────────────────────────────────────────────
// Item 3 — 1a6b0d99db: a JS-produced Bun.serve response is sent when it
// completes. Verdict: EQUIVALENT — all handler JS runs before the write, the
// write is the terminal onData action, and connection-close framing ends the
// read instead of dispatching another pipelined handler.
// ────────────────────────────────────────────────────────────────────────────

/// Upstream test shape ("sends a response before the JavaScript that runs
/// after it"): the handler returns a 202 and schedules 400ms of blocking
/// post-completion JS. The client must already hold the full response while
/// that work is still pending/running — post-completion JS cannot delay the
/// send. The spin really running (span >= 350ms) proves the lock is not
/// vacuous.
#[test]
fn semantic_port_serve_response_sent_before_trailing_js() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let port: u16 = eval_string(
        &mut ctx,
        r#"
        globalThis.__t4 = { t0: 0, spinEnd: 0, spinRan: false };
        globalThis.__srv4 = Bun.serve({
          port: 0,
          hostname: "127.0.0.1",
          fetch: function(req) {
            var p = new URL(req.url).pathname;
            if (p === "/fast") {
              globalThis.__t4.t0 = Date.now();
              setTimeout(function() {
                var end = Date.now() + 400;
                while (Date.now() < end) {
                  // post-completion JS: must not hold the completed response
                }
                globalThis.__t4.spinEnd = Date.now();
                globalThis.__t4.spinRan = true;
              }, 10);
              return new Response("sent-now", { status: 202 });
            }
            return new Response("other", { status: 200 });
          },
        });
        '' + globalThis.__srv4.port
    "#,
    )
    .trim()
    .parse()
    .expect("serve must report its port");
    drive_event_loop(&mut ctx, 10);

    let started = Instant::now();
    let raw = http_get(&mut ctx, port, "/fast").expect("/fast must get a response");
    let arrival = started.elapsed();
    let (head, body) = split_response(&raw);
    let status_line = head.lines().next().unwrap_or("");
    assert!(
        status_line.starts_with("HTTP/1.1 202"),
        "status must be 202, got: {status_line}"
    );
    assert_eq!(body, b"sent-now", "response body must roundtrip exactly");

    // The contract under lock: the completed JS response is not held behind
    // the JS that the handler left behind. A held-cork design would deliver
    // the bytes only after the 400ms spin (arrival >= 400ms).
    assert!(
        arrival < Duration::from_millis(200),
        "response must be sent when it completes, not after the trailing JS it \
         left behind — arrival took {:?} (upstream #42815 contract)",
        arrival
    );

    // Non-vacuity: the trailing spin actually ran, spanning the window.
    let t4 = drive_until_string(&mut ctx, "JSON.stringify(globalThis.__t4)", "\"spinRan\":true", 8_000);
    assert!(
        t4.contains("\"spinRan\":true"),
        "the trailing spin must have run (lock must not be vacuous), got: {t4}"
    );
    let t0: u64 = t4
        .split("\"t0\":")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.parse().ok())
        .expect("t0 timestamp present");
    let spin_end: u64 = t4
        .split("\"spinEnd\":")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.parse().ok())
        .expect("spinEnd timestamp present");
    assert!(
        spin_end.saturating_sub(t0) >= 350,
        "trailing work must span >= 350ms to prove the response beat it \
         (t0={t0}, spinEnd={spin_end})"
    );

    let stopped = eval_string(&mut ctx, "globalThis.__srv4.stop(), 'stopped'");
    assert_eq!(stopped, "stopped");
    drive_event_loop(&mut ctx, 10);
}
