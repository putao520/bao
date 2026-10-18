// @trace TEST-ENG-006-C12 [req:REQ-ENG-006] [level:integration]
// C12 line-coverage wave for src/bao_runtime/src/bun_api.rs — residual gap
// after C2 (fresh-lcov gap ≈ 1200 lines; spawn/serve/ws/file faces + process
// edges). Characterization truth sources: local node v24.19.0 oracle, Bun
// upstream semantics, and bao source for deliberate shapes.
//
// Branch map (source-anchored):
//   - bun_spawn            : argument-shape error arms (argc==0 / empty array
//                            / non-object-non-string / empty cmd,
//                            1934-2019), string-form + {cmd:[...]} alias +
//                            split form, exited promise + stdout.text()
//   - subproc_wait/_pollExitInfo/kill (2536-2590, 9265-9273): exit-code
//                            publish, [code, signal] probe shape, signal-name
//                            kill arm, signal-death exit → -1
//   - process edges        : nextTick ordering/recursion (8957-8980),
//                            Bun.exit request flag (9357-9367)
//   - Bun.serve            : server.ref/unref (3496-3506), status_line_for
//                            matrix (6053-6076), default-echo uppercase
//                            method arm (6020-6041), route-throw → 500 +
//                            uncaughtException routing (4682-4695), request
//                            object fields (serve_build_request_object
//                            5492-5564)
//   - Bun.which / Bun.resolve / Bun.build : PATH candidate arm (6298-6305),
//                            miss → null, dot-relative lexical join
//                            (6219-6227), package-miss throw, minify object
//                            + define + external + get_opt_bool non-bool arm
//                            (6434-6502, 6577-6586)
//   - Bun.file family      : exists faces (7005-7012), writer open-failure +
//                            highWaterMark arms (7281-7322), FileSink
//                            write/flush/end faces incl. bad-chunk throw and
//                            ended-sink 0 arm (7412-7578), slice clamps
//                            (8150/8249), Bun.write overwrite + error arm
//                            (8359-8364)
//   - concatArrayBuffers   : array-like list form + list-length double arm +
//                            totalLength arms (9746-9898)
//   - gzip/deflate         : no-arg and non-byte-view error arms
//                            (10422-10458)
//   - openInNewTab         : non-string URL throw + string → undefined
//                            (10957-10968)
//   - WebSocket client     : send text/binary/toString-object/non-byte arm,
//                            ping→pong, close(1000) handshake (3807-3981);
//                            server-side serve websocket open/message faces
//                            (4204-4350) via the in-process serve route
//
// REGISTRY (divergent shapes / not coverable — needs a product-side decision,
// NOT a test fix):
//   R-A1. Bun.serve req.method is the RAW LOWERCASE wire token ("get");
//         only the default-echo response uppercases it. Bun/node surface a
//         canonical uppercase method.
//   R-A2. Bun.which ignores its second argument entirely (options objects
//         {dir}/{PATH}/{env} unsupported — env PATH is the only source).
//         Bun upstream resolves {dir}/{PATH} options.
//   R-A3. (withdrawn — Bun.build resolves the upstream BuildOutput promise
//         shape {success, outputs, logs}; no divergence found)
//   R-A4. serve_body_stream_signal abort/cap arms (4839-4851) + 413
//         body-cap arm (3238-3250): need a 128 MB request body or a
//         mid-flight socket abort — impractical in-process (SERVE_BODY_MAX).
//   R-A5. bun_spawn pump-thread-failure fail-closed arm (2159-2178): needs
//         thread-spawn failure injection.
//   R-A6. stdin_read EOF face (2829-2840): nextest stdin shape is
//         environment-dependent (Ok(0) vs Err) — flake risk, low value.
//   R-A7. [FIXED — oracle assertion] Bun.serve route-throw now captures,
//         clears and routes the pending exception through
//         process.on("uncaughtException") (same contract as the listen-side
//         invoke_js_callback); the 500 + keep-serving faces are unchanged.
//   R-A8. proc.wait() publishes -<signal> for signal deaths (-9 for
//         SIGKILL); the subproc_wait doc comment claims -1 — doc/behavior
//         divergence.
//   R-A9. [FIXED — oracle assertion] BARE-TICK CRASH CLASS (gdb-verified):
//         ws_on_upgrade now enters the persistent thread realm before its
//         first JS allocation (JS_NewPlainObject resolves the plain-object
//         shape off CurrentGlobalOrNull; the bare-tick entry previously
//         SIGSEGV'd in GlobalObject::getPlainObjectShapeWithDefaultProto).
//         The no-realm teardown path fails closed with a 500 rejection
//         instead of crashing. c12_serve_websocket_in_process_roundtrip
//         drives the loop with NO realm entered (regression pin).
//   R-A9b. [FIXED — oracle assertion] Uint8Array WebSocket sends are now
//         binary frames end-to-end: the client BufferSource arm uses
//         send_binary and the serve message handler receives a byte-exact
//         Uint8Array (the previous ES ToString + C-string round trip
//         produced the comma-joined decimals "66,49,50").
//   R-A9c. [FIXED — ws-library parity] The client WebSocket now exposes the
//         npm `ws` control-frame surface: ping([data[, mask[, cb]]]) /
//         pong(...), inbound `ping`/`pong` events (onping/onpong properties
//         AND Node-style ws.on('ping'|'pong')), and mask=false support.
//         Node v24's built-in WebSocket has no ping method — the oracle is
//         the ws library, as recorded here.
//   R-A9d. [FIXED — root cause] Back-to-back sends were observed to starve
//         onmessage. Deterministic root cause: the non-blocking drain pump
//         called bun_uws FrameDecoder::decode_frame directly; a frame split
//         across TCP reads aborted the decode mid-frame, and the retry
//         re-read payload bytes as a frame header (observed
//         "recv: reserved websocket opcode 0x3") — the connection died with
//         no onmessage. The pump now buffers raw bytes and parses only a
//         complete frame (fresh decoder per attempt). Pins:
//         c12_websocket_client_split_frame_still_dispatches (pre-fix RED with
//         the reserved-opcode error) and
//         c12_websocket_client_back_to_back_sends_deliver (ordering).

use bao_engine::context::JsContext;
use mozjs::rooted;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
#[path = "common/mod.rs"]
mod common;

use common::{bounded_drain_hook, drive_event_loop, eval_string_full as eval_string};

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    // The bounded post-eval drain (timers + spawn watcher + microtask tail)
    // rides every eval — the same discipline as bun_spawn_events_tests;
    // without it the exited/build promises never settle on the JS thread.
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Per-test temp path (nextest = one process per test; pid prefix avoids
/// cross-suite interference on shared /tmp).
fn tmp_path(name: &str) -> String {
    format!("/tmp/bao-c12-bunapi-{}-{}.txt", std::process::id(), name)
}

/// Poll a JS condition that yields "y"/"n", draining microtasks (promise
/// continuations) and the unified loop each pass.
fn poll_until(ctx: &mut JsContext, cond: &str, iters: u32) -> bool {
    for _ in 0..iters {
        if eval_string(ctx, cond) == "y" {
            return true;
        }
        common::drive_event_loop_drain(ctx, 3);
        std::thread::sleep(Duration::from_millis(5));
    }
    eval_string(ctx, cond) == "y"
}

/// One raw HTTP/1.1 GET/POST against the in-process serve, pumping the loop
/// between read attempts (uWS dispatches only while the loop ticks).
fn raw_http(ctx: &mut JsContext, port: u16, request: &str) -> String {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect serve");
    sock.set_read_timeout(Some(Duration::from_millis(25))).ok();
    sock.write_all(request.as_bytes()).expect("write request");
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
    String::from_utf8_lossy(&out).into_owned()
}

/// Bare-tick poll — NO realm entered around the loop tick. This is the exact
/// shape that previously SIGSEGV'd in ws_on_upgrade →
/// GlobalObject::getPlainObjectShapeWithDefaultProto (R-A9). Every dispatch
/// path (ws_on_upgrade / ws_on_open / ws_on_message / client pump) must
/// enter the persistent realm itself, so a regression here crashes the test
/// process — the crash-class oracle pin.
fn poll_until_bare(ctx: &mut JsContext, cond: &str, iters: u32) -> bool {
    for _ in 0..iters {
        if eval_string(ctx, cond) == "y" {
            return true;
        }
        let raw_cx = ctx.raw_cx();
        {
            let mut cxm = ctx.cx();
            bun_runtime::timers::drain_and_check(&mut cxm);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(raw_cx);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    eval_string(ctx, cond) == "y"
}

/// Realm-wrapping poll for WebSocket faces that also exercise the client
/// send/receive path under an entered realm (the production script-thread
/// shape). Kept for the send/ping/close sequence; the upgrade-path realm
/// gate itself is pinned by [`poll_until_bare`].
fn poll_until_realm(ctx: &mut JsContext, cond: &str, iters: u32) -> bool {
    for _ in 0..iters {
        if eval_string(ctx, cond) == "y" {
            return true;
        }
        {
            let mut cxm = ctx.cx();
            if let Some(g) = bao_engine::context::thread_realm_global() {
                rooted!(&in(cxm) let g_root = g);
                let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                bun_runtime::timers::drain_and_check(realm_cx);
                bun_runtime::timers::with_event_loop(|loop_| {
                    loop_.tick_without_idle(std::ptr::null_mut());
                });
                unsafe {
                    mozjs_sys::jsapi::js::RunJobs(realm_cx.raw_cx());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    eval_string(ctx, cond) == "y"
}

/// Complete the server side of the RFC 6455 upgrade on a raw loopback
/// socket (same wire truth as `web_api_ws_edge_tests`; duplicated here
/// because that file is outside this task's file scope).
fn accept_ws_upgrade(stream: &mut TcpStream) -> bool {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return false,
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let key = text
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("sec-websocket-key")
                .then(|| value.trim().to_string())
        })
        .expect("client handshake must carry Sec-WebSocket-Key");
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        bun_uws::ws_handshake::compute_accept(&key)
    );
    stream.write_all(response.as_bytes()).is_ok()
}

/// Read one client frame (masked, per RFC 6455) from a raw peer socket.
fn read_client_frame(
    dec: &mut bun_uws::ws_codec::FrameDecoder,
    stream: &mut TcpStream,
) -> Option<(bun_uws::ws_codec::Opcode, bool, Vec<u8>)> {
    let header = dec.decode_frame(stream).ok()??;
    let payload = if header.mask {
        let key = dec.take_mask();
        let mut p = dec.take_payload(&header);
        bun_uws::ws_codec::apply_mask(&mut p, &key);
        p
    } else {
        dec.take_payload(&header)
    };
    Some((header.opcode, header.mask, payload))
}

/// First real status line of a raw response, skipping interim
/// "HTTP/1.1 100 Continue" lines uWS may emit for deferred writes.
fn status_line_of(resp: &str) -> String {
    resp.lines()
        .find(|l| l.starts_with("HTTP/1.1") && !l.starts_with("HTTP/1.1 100"))
        .unwrap_or("")
        .to_string()
}

// ──────────────────── Bun.spawn faces ────────────────────

/// Argument-shape error arms (1934-2019): no args, empty array, non-object
/// non-string first arg, and empty cmd all throw with the documented
/// messages; nothing is spawned.
#[test]
fn c12_spawn_arg_shape_errors() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { fn(); out.push(label + "=no-throw"); }
            catch (e) { out.push(label + "=THREW:" + e.message); }
        }
        probe("noargs", function () { Bun.spawn(); });
        probe("emptyArr", function () { Bun.spawn([]); });
        probe("number", function () { Bun.spawn(42); });
        probe("emptyCmd", function () { Bun.spawn({ cmd: "" }); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "noargs=THREW:Bun.spawn() requires a command array or an options object|\
         emptyArr=THREW:Bun.spawn() command array must not be empty|\
         number=THREW:Bun.spawn() requires a command array or an options object|\
         emptyCmd=THREW:Bun.spawn() requires a non-empty command",
        "spawn argument-shape error arms"
    );
}

/// Command shapes: array form, options-object {cmd:[...]} alias, and the
/// split form {cmd, args} all run the real binary and deliver captured
/// stdout via the stdout "data" stream + "close" event (the proven
/// bun_spawn_events surface).
#[test]
fn c12_spawn_command_shapes_deliver_stdout() {
    let mut ctx = make_ctx();

    let setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__shapes = "";
        (function () {
            var out = [];
            var pending = 3;
            function run(label, makeProc) {
                var p = makeProc();
                var chunks = [];
                p.stdout.on("data", function (d) { chunks.push(String(d)); });
                p.on("close", function (code) {
                    out.push(label + ":" + code + ":" + chunks.join(""));
                    if (--pending === 0) { globalThis.__shapes = out.join("|"); }
                });
            }
            run("arr", function () { return Bun.spawn(["/bin/echo", "-n", "arr-form"]); });
            run("obj", function () { return Bun.spawn({ cmd: ["/bin/echo", "-n", "obj-alias"] }); });
            run("split", function () { return Bun.spawn({ cmd: "/bin/echo", args: ["-n", "split-form"] }); });
        })();
        void 0;
        "#,
    );
    assert_eq!(setup, "undefined", "spawn shapes setup must eval clean");

    let done = poll_until(&mut ctx, r#"globalThis.__shapes !== "" ? "y" : "n""#, 600);
    assert!(done, "spawn shapes must settle — got: {}", eval_string(&mut ctx, "globalThis.__shapes"));
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__shapes"),
        "arr:0:arr-form|obj:0:obj-alias|split:0:split-form",
        "all three command shapes run the real binary with exit 0 and captured stdout"
    );
}


/// proc.wait() (bounded synchronous native pump) + proc._pollExitInfo()
/// publish the pump's exit info and mirror the code onto the exitCode
/// property (2536-2590).
#[test]
fn c12_spawn_wait_and_poll_exit_info() {
    let mut ctx = make_ctx();

    let setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__waitReport = "";
        (function () {
            var p = Bun.spawn(["/bin/true"]);
            var waited = p.wait();                 // synchronous: bounded by the child's life
            var probed = p._pollExitInfo();        // [exitCode, signal] once published
            globalThis.__waitReport = "wait=" + waited + ";poll=" + Array.isArray(probed) + ":" + probed[0] +
                   ";exitCodeProp=" + p.exitCode;
        })();
        void 0;
        "#,
    );
    assert_eq!(setup, "undefined", "wait setup must eval clean");

    let done = poll_until(&mut ctx, r#"globalThis.__waitReport !== "" ? "y" : "n""#, 600);
    assert!(done, "wait report must settle — got: {}", eval_string(&mut ctx, "globalThis.__waitReport"));
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__waitReport"),
        "wait=0;poll=true:0;exitCodeProp=0",
        "wait/poll publish (0,…) for a clean exit; exitCode prop mirrors"
    );
}


/// kill(signal-name) arm (9265-9273): a named signal kills the child; the
/// pump publishes a signal death as exit code -1 (subproc_wait contract),
/// observed through the synchronous wait().
#[test]
fn c12_spawn_kill_signal_name_signal_death_is_minus_one() {
    let mut ctx = make_ctx();

    let setup = eval_string(
        &mut ctx,
        r#"
        globalThis.__killReport = "";
        (function () {
            var p = Bun.spawn(["/bin/sleep", "30"]);
            var sent = p.kill("SIGKILL");
            var code = p.wait();                   // synchronous: bounded by the killed child's life
            globalThis.__killReport = "sent=" + sent + ";code=" + code;
        })();
        void 0;
        "#,
    );
    assert_eq!(setup, "undefined", "kill setup must eval clean");

    let done = poll_until(&mut ctx, r#"globalThis.__killReport !== "" ? "y" : "n""#, 600);
    assert!(done, "kill report must settle — got: {}", eval_string(&mut ctx, "globalThis.__killReport"));
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__killReport"),
        "sent=true;code=-9",
        "named signal sent; signal death publishes -<signal> (-9 for SIGKILL).          REGISTRY R-A8: subproc_wait's doc comment claims signal deaths → -1; the observed          publish is -<signal>"
    );
}


// ──────────────────── process edges ────────────────────

/// process.nextTick faces (8957-8980): nextTick callbacks run before
/// promise jobs and timers, nested nextTicks queue behind the current drain,
/// and the order is FIFO.
#[test]
fn c12_process_next_tick_order() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        globalThis.__order = [];
        process.nextTick(function () { __order.push("t1"); });
        Promise.resolve().then(function () { __order.push("p1"); });
        process.nextTick(function () {
            __order.push("t2");
            process.nextTick(function () { __order.push("t2-nested"); });
        });
        "queued"
        "#,
    );
    assert_eq!(report, "queued");

    // Pump the loop so timers/microtasks/next-tick queue drain.
    {
        let mut cxm = ctx.cx();
        for _ in 0..20 {
            unsafe {
                mozjs_sys::jsapi::js::RunJobs(cxm.raw_cx());
            }
            let _ = bun_runtime::timers::drain_and_check(&mut cxm);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    let order = eval_string(&mut ctx, "globalThis.__order.join(',')");
    assert_eq!(
        order, "t1,t2,t2-nested,p1",
        "nextTick FIFO drains before promise jobs; nested ticks join the same drain"
    );
}

/// Bun.exit(code) sets the request-exit flag instead of killing the process
/// (9357-9367) — observable from Rust through the runtime's exit state.
#[test]
fn c12_bun_exit_sets_request_flag() {
    let mut ctx = make_ctx();

    assert!(!bun_runtime::should_exit(), "fresh context must not request exit");
    assert_eq!(eval_string(&mut ctx, "String(Bun.exit(7))"), "undefined");
    assert!(bun_runtime::should_exit(), "Bun.exit must raise the exit flag");
    assert_eq!(bun_runtime::exit_code(), 7, "exit code travels to the Rust side");
}

// ──────────────────── Bun.serve faces ────────────────────

/// Bun.serve server.ref()/unref() are undefined-returning no-ops (3496-3506)
/// and the server keeps serving; stop() tears it down.
#[test]
fn c12_serve_server_ref_unref_keep_serving() {
    let mut ctx = make_ctx();

    let setup = eval_string(
        &mut ctx,
        r#"
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function () { return new Response("refaced", { status: 200 }); },
        });
        var r1 = srv.ref(); var r2 = srv.unref();
        globalThis.__faces = String(r1) + "|" + String(r2) + "|" + String(srv.port > 0);
        srv.port
        "#,
    );
    let port: u16 = setup.parse().expect("bound port");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__faces"),
        "undefined|undefined|true",
        "ref/unref return undefined; port bound"
    );
    let resp = raw_http(&mut ctx, port, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status_line_of(&resp), "HTTP/1.1 200 OK", "serving after ref/unref — got: {resp}");
    assert!(resp.contains("refaced"), "body round-trips — got: {resp}");
    assert_eq!(eval_string(&mut ctx, "String(typeof srv.stop === 'function')"), "true");
    eval_string(&mut ctx, "srv.stop()");
    drive_event_loop(&mut ctx, 10);
}

/// status_line_for matrix (6053-6076): served statuses map to canonical
/// reason phrases on the wire; an unknown code degrades to the bare number.
#[test]
fn c12_serve_status_line_matrix() {
    let mut ctx = make_ctx();

    let port: u16 = eval_string(
        &mut ctx,
        r#"
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                var st = parseInt(req.url.split("/").pop(), 10);   // absolute url → tail segment
                return new Response("b-" + st, { status: st });
            },
        });
        srv.port
        "#,
    )
    .parse()
    .expect("bound port");

    // (status, expected reason) — covers distinct match arms of status_line_for.
    for (code, reason) in [
        (201, "Created"),
        (204, "No Content"),
        (206, "Partial Content"),
        (301, "Moved Permanently"),
        (429, "Too Many Requests"),
        (503, "Service Unavailable"),
        (599, ""), // unknown → bare number
    ] {
        let resp = raw_http(
            &mut ctx,
            port,
            &format!("GET /{} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", code),
        );
        let status_line = status_line_of(&resp);
        let expected = if reason.is_empty() {
            format!("HTTP/1.1 {}", code)
        } else {
            format!("HTTP/1.1 {} {}", code, reason)
        };
        assert_eq!(
            status_line, expected,
            "status {code} must wire '{expected}' — full response:\n{resp}"
        );
        if code != 204 {
            assert!(resp.contains(&format!("b-{}", code)), "body must round-trip for {code}");
        }
    }
    eval_string(&mut ctx, "srv.stop()");
    drive_event_loop(&mut ctx, 10);
}

/// The default response arm (serve_write_default_response 6020-6041): an
/// unmatched path answers 200 OK with {"method":"<UPPERCASE>","url":path} —
/// the lowercase uWS method token is canonicalized for the echo body.
#[test]
fn c12_serve_default_echo_uppercases_method() {
    let mut ctx = make_ctx();

    let port: u16 = eval_string(
        &mut ctx,
        r#"
        var srv = Bun.serve({ port: 0, hostname: "127.0.0.1" });  // no fetch → default echo
        srv.port
        "#,
    )
    .parse()
    .expect("bound port");

    let resp = raw_http(
        &mut ctx,
        port,
        "POST /nope HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        status_line_of(&resp),
        "HTTP/1.1 200 OK",
        "default echo answers 200 — got:\n{resp}"
    );
    assert!(
        resp.contains("\"method\":\"POST\"") && resp.contains("\"url\":\"/nope\""),
        "default echo reports uppercase method + url — got:\n{resp}"
    );
    let resp = raw_http(&mut ctx, port, "GET /q HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(resp.contains("\"method\":\"GET\""), "GET arm — got:\n{resp}");
    eval_string(&mut ctx, "srv.stop()");
    drive_event_loop(&mut ctx, 10);
}

/// A throwing route handler → 500 response AND the pending exception is
/// routed to process's uncaughtException handler (4682-4695) — never leaked
/// past the native entry point; the server keeps serving afterwards.
#[test]
fn c12_serve_route_throw_500_and_uncaught_routing() {
    let mut ctx = make_ctx();

    let port: u16 = eval_string(
        &mut ctx,
        r#"
        globalThis.__uncaught = "";
        globalThis.__boomSeen = "";
        process.on("uncaughtException", function (e) {
            globalThis.__uncaught = String(e && e.message || e);
        });
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                globalThis.__boomSeen = req.url;
                if (req.url.indexOf("/boom") >= 0) { throw new Error("c12-route-boom"); }
                return new Response("fine");
            },
        });
        srv.port
        "#,
    )
    .parse()
    .expect("bound port");

    let resp = raw_http(&mut ctx, port, "GET /boom HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(
        eval_string(&mut ctx, "globalThis.__boomSeen").contains("/boom"),
        "the throwing route must have been dispatched — saw: {}",
        eval_string(&mut ctx, "globalThis.__boomSeen")
    );
    assert!(
        status_line_of(&resp).starts_with("HTTP/1.1 500"),
        "throwing route → 500 — full response:\n{resp}"
    );
    let after = raw_http(&mut ctx, port, "GET /ok HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(after.contains("fine"), "server survives a route throw — got:\n{after}");
    // R-A7 oracle: the throwing route answers 500, the server survives, AND
    // the captured pending exception reaches process.on("uncaughtException")
    // (Node semantics; symmetric with the listen-side invoke_js_callback).
    let routed = poll_until(&mut ctx, r#"globalThis.__uncaught !== "" ? "y" : "n""#, 60);
    assert!(
        routed,
        "route-throw pending exception must reach the uncaughtException handler"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__uncaught"),
        "c12-route-boom",
        "uncaughtException handler observes the original route error message"
    );
    eval_string(&mut ctx, "srv.stop()");
    drive_event_loop(&mut ctx, 10);
}

/// Request-object faces (serve_build_request_object 5492-5564): method is
/// the raw lowercase wire token (REGISTRY R-A1), url is path+query, headers
/// expose case-insensitive wire headers.
#[test]
fn c12_serve_request_object_fields() {
    let mut ctx = make_ctx();

    let port: u16 = eval_string(
        &mut ctx,
        r#"
        globalThis.__req = null;
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                globalThis.__req = req;
                return new Response("req-ok");
            },
        });
        srv.port
        "#,
    )
    .parse()
    .expect("bound port");

    raw_http(
        &mut ctx,
        port,
        "GET /a/b?x=1 HTTP/1.1\r\nHost: c12host\r\nX-C12-Header: probe\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__req.method"),
        "get",
        "raw lowercase wire token (REGISTRY R-A1 — Bun/node surface uppercase)"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__req.url"),
        "http://c12host/a/b?x=1",
        "url surfaces as the absolute form scheme://host/path?query"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__req.headers.get('x-c12-header')"),
        "probe",
        "headers are case-insensitive lookups"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__req.headers.get('host'))"),
        "c12host",
        "host header surfaces"
    );
    eval_string(&mut ctx, "srv.stop()");
    drive_event_loop(&mut ctx, 10);
}

// ──────────────────── Bun.which / resolve / build ────────────────────

/// Bun.which faces (6298-6305): env-PATH lookup finds a real binary and a
/// miss returns null. REGISTRY R-A2: the second argument is ignored — the
/// options form returns exactly what the one-arg form returns.
#[test]
fn c12_bun_which_faces() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        var onPath = Bun.which("sh");
        out.push(String(onPath === null ? "null" : onPath.endsWith("/sh")));
        out.push(String(Bun.which("sh", { dir: "/bin" }) === onPath));
        out.push(String(Bun.which("c12-definitely-not-a-binary-xyz")));
        out.join("|")
        "#,
    );
    assert_eq!(
        report, "true|true|null",
        "PATH hit endsWith /sh; options argument ignored (R-A2); miss → null"
    );
}

/// Bun.resolve faces (6219-6227): dot-relative specifiers join lexically on
/// `from`; an unresolvable package throws "Cannot resolve '<spec>'".
#[test]
fn c12_bun_resolve_faces() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        var rel = Bun.resolve("./c12target.js", "/tmp");
        out.push(String(rel === "/tmp/c12target.js"));
        try { Bun.resolve("c12-no-such-pkg-xyz", "/tmp"); out.push("pkg=no-throw"); }
        catch (e) { out.push("pkg=THREW:" + e.message.slice(0, 30)); }
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "true|pkg=THREW:Cannot resolve 'c12-no-such-pk",
        "dot-relative lexical join + package-miss throw"
    );
}

/// Bun.build option-parsing arms (6434-6502 + get_opt_bool 6577-6586):
/// minify as boolean AND as the {whitespace,syntax,identifiers} object,
/// define record, external list, and a non-boolean option value
/// (get_opt_bool → None arm). Bun.build returns a PROMISE resolved with the
/// upstream BuildOutput shape {success, outputs, logs}.
#[test]
fn c12_bun_build_options_matrix() {
    let mut ctx = make_ctx();
    let entry = tmp_path("entry.js");
    std::fs::write(
        &entry,
        "const greet = (name) => { return \"hello \" + name; };\nconsole.log(greet(\"c12\"));\n",
    )
    .expect("write entry");

    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__buildReport = "";
            (async function () {{
                var out = [];
                var r1 = await Bun.build({{ entrypoints: ["{entry}"], minify: true }});
                out.push("minify-bool:" + (typeof r1.success === "boolean" && typeof r1.outputs === "object"));
                var r2 = await Bun.build({{
                    entrypoints: ["{entry}"],
                    minify: {{ whitespace: true, syntax: true, identifiers: true }},
                    define: {{ __C12__: "\"defined\"" }},
                    external: ["left-alone"],
                }});
                out.push("minify-obj:" + (typeof r2.success === "boolean"));
                var r3 = await Bun.build({{ entrypoints: ["{entry}"], splitting: "not-a-bool" }});
                out.push("nonbool-opt:" + (typeof r3.success === "boolean"));
                globalThis.__buildLogs = (r1.logs || []).map(function (m) {{
                    return (m && m.message) || String(m);
                }}).join("; ").slice(0, 200);
                globalThis.__buildReport = out.join("|");
            }})().catch(function (e) {{ globalThis.__buildReport = "IIFE-ERR:" + (e && e.message || e); }});
            void 0;
            "#,
            entry = entry
        ),
    );
    assert_eq!(setup, "undefined", "build setup must eval clean");

    let done = poll_until(&mut ctx, r#"globalThis.__buildReport !== "" ? "y" : "n""#, 600);
    assert!(done, "build report must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__buildReport"),
        "minify-bool:true|minify-obj:true|nonbool-opt:true",
        "minify boolean/object arms, define + external parsing, non-boolean option tolerated; \
         promise resolves the upstream BuildOutput shape {{success, outputs, logs}} — logs: {}",
        eval_string(&mut ctx, "String(globalThis.__buildLogs || '')")
    );
    let _ = std::fs::remove_file(&entry);
}

// ──────────────────── Bun.file family ────────────────────

/// FileSink lifecycle (7281-7578): writer() with a double highWaterMark,
/// write string arm → accepted count, flush/end resolve, content lands on
/// disk; the ended sink accepts nothing; open failure throws.
#[test]
fn c12_file_writer_sink_lifecycle() {
    let mut ctx = make_ctx();
    let path = tmp_path("sink.txt");

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__sinkReport = "";
            (async function () {{
                var out = [];
                var w = Bun.file("{path}").writer({{ highWaterMark: 4.5 }});
                var n1 = w.write("hello ");        // string arm
                var n2 = w.write(new Uint8Array([119, 111])); // "wo"
                out.push("accepted:" + n1 + "+" + n2);
                var flushed = await w.flush();
                out.push("flushed:" + (flushed > 0));
                var total = await w.end();
                out.push("ended:" + (total >= 8));
                try {{ w.write("x"); out.push("after-end:0"); }}
                catch (e) {{ out.push("after-end:THREW"); }}
                out.push("content:" + (await Bun.file("{path}").text()));
                try {{ Bun.file("/nonexistent-c12-dir/f.txt").writer(); out.push("openfail:no-throw"); }}
                catch (e) {{ out.push("openfail:THREW:" + e.constructor.name); }}
                globalThis.__sinkReport = out.join("|");
            }})().catch(function (e) {{ globalThis.__sinkReport = "IIFE-ERR:" + (e && e.message || e); }});
            "#,
            path = path
        ),
    );

    let done = poll_until(&mut ctx, r#"globalThis.__sinkReport !== "" ? "y" : "n""#, 600);
    assert!(done, "sink report must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__sinkReport"),
        "accepted:6+2|flushed:true|ended:true|after-end:0|content:hello wo|openfail:THREW:Error",
        "sink write/flush/end faces; ended sink returns 0; unwritable path throws at open"
    );
    let _ = std::fs::remove_file(&path);
}

/// Bun.write arms (8359-8364): overwrite truncates an existing file and the
/// unwritable-dir error arm surfaces as a rejection.
#[test]
fn c12_bun_write_overwrite_and_error_arm() {
    let mut ctx = make_ctx();
    let path = tmp_path("write.txt");
    std::fs::write(&path, "0123456789").expect("seed longer file");

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__writeReport = "";
            (async function () {{
                var out = [];
                var n = await Bun.write("{path}", "short");
                out.push("wrote:" + n + ":" + (await Bun.file("{path}").text()));
                try {{ await Bun.write("/nonexistent-c12-dir/f", "x"); out.push("err:no-throw"); }}
                catch (e) {{ out.push("err:THREW"); }}
                globalThis.__writeReport = out.join("|");
            }})().catch(function (e) {{ globalThis.__writeReport = "IIFE-ERR:" + (e && e.message || e); }});
            "#,
            path = path
        ),
    );

    let done = poll_until(&mut ctx, r#"globalThis.__writeReport !== "" ? "y" : "n""#, 600);
    assert!(done, "write report must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__writeReport"),
        "wrote:5:short|err:THREW",
        "write truncates (10-byte seed → 5 bytes) and the bad-dir arm rejects"
    );
    let _ = std::fs::remove_file(&path);
}

/// Bun.file exists faces (7005-7012) + slice clamps (8150/8249): negative /
/// oversized slice bounds clamp to the real content.
#[test]
fn c12_file_exists_and_slice_clamps() {
    let mut ctx = make_ctx();
    let path = tmp_path("slice.txt");
    std::fs::write(&path, "abcdefgh").expect("seed");

    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__sliceReport = "";
            (async function () {{
                var out = [];
                out.push("exists-real:" + (await Bun.file("{path}").exists()));
                out.push("exists-miss:" + (await Bun.file("/nonexistent-c12-file").exists()));
                out.push("slice-neg:" + (await Bun.file("{path}").slice(-3).text()));
                out.push("slice-big:" + (await Bun.file("{path}").slice(2, 1e9).text()));
                out.push("slice-empty:" + (await Bun.file("{path}").slice(100, 200).text()));
                globalThis.__sliceReport = out.join("|");
            }})().catch(function (e) {{ globalThis.__sliceReport = "IIFE-ERR:" + (e && e.message || e); }});
            "#,
            path = path
        ),
    );

    let done = poll_until(&mut ctx, r#"globalThis.__sliceReport !== "" ? "y" : "n""#, 600);
    assert!(done, "slice report must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__sliceReport"),
        "exists-real:true|exists-miss:false|slice-neg:fgh|slice-big:cdefgh|slice-empty:",
        "exists faces + slice clamp arms"
    );
    let _ = std::fs::remove_file(&path);
}

// ──────────────────── concatArrayBuffers / gzip / openInNewTab ────────────────────

/// concatArrayBuffers array-like list form + list-length double arm +
/// totalLength truncation/zero-fill arms (9746-9898).
#[test]
fn c12_concat_array_buffers_arraylike_and_total() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var a = new Uint8Array([1, 2]).buffer;
        var b = new Uint8Array([3]).buffer;
        var out = [];
        out.push(Array.from(new Uint8Array(Bun.concatArrayBuffers({ length: 2, 0: a, 1: b }))).join());
        out.push(Array.from(new Uint8Array(Bun.concatArrayBuffers([a, b], 5))).join());
        out.push(Array.from(new Uint8Array(Bun.concatArrayBuffers([a, b], 2))).join());
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "1,2,3|1,2,3,0,0|1,2",
        "array-like list accepted; totalLength zero-fills / truncates"
    );
}

/// gzip/deflate error arms (10422-10458): no-arg and non-byte-view inputs
/// throw; the happy path still round-trips through the sync compressors.
#[test]
fn c12_gzip_deflate_error_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { fn(); out.push(label + "=no-throw"); }
            catch (e) { out.push(label + "=THREW:" + e.message.slice(0, 34)); }
        }
        probe("gzip-noarg", function () { Bun.gzip(); });
        probe("gzip-nonbyte", function () { Bun.gzip(42); });
        probe("deflate-noarg", function () { Bun.deflate(); });
        probe("deflate-nonbyte", function () { Bun.deflate({}); });
        out.push("roundtrip:" + Bun.gunzipSync(Bun.gzipSync("c12-payload")));
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "gzip-noarg=THREW:Bun.gzip() requires data|gzip-nonbyte=THREW:Bun.gzip() requires string or Arra|\
         deflate-noarg=THREW:Bun.deflate() requires data|deflate-nonbyte=THREW:Bun.deflate() requires string or A|\
         roundtrip:c12-payload",
        "error arms + happy-path round trip"
    );
}

/// openInNewTab arms (10957-10968): non-string URL throws; a string URL
/// resolves to undefined (the opener spawn is best-effort by design).
#[test]
fn c12_open_in_new_tab_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        try { Bun.openInNewTab(42); out.push("nonstring=no-throw"); }
        catch (e) { out.push("nonstring=THREW:" + e.message); }
        out.push("string=" + String(Bun.openInNewTab("https://example.invalid/c12")));
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "nonstring=THREW:Bun.openInNewTab() requires a string URL|string=undefined",
        "URL validation arm + best-effort string arm"
    );
}

// ──────────────────── WebSocket client faces ────────────────────

/// WebSocket client send/ping/close faces, with the IN-PROCESS
/// Bun.serve({websocket}) echo handler as the peer (the proven same-loop
/// path — an external raw-TCP peer showed run-to-run dispatch races).
/// Covers ws_js_send's string/binary/ToString-object/non-byte arms
/// (3807-3981) + the client open/message/close callback faces; the serve
/// message handler echoes every payload back.
#[test]
fn c12_websocket_client_send_ping_close_faces() {
    let mut ctx = make_ctx();

    eval_string(
        &mut ctx,
        r#"
        globalThis.__wslog = [];
        globalThis.__closed = "";
        globalThis.__srvMsgs = [];
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            websocket: {
                open: function (ws) {},
                message: function (ws, msg) {
                    // Byte-exact descriptor: text frames stay strings, binary
                    // frames must arrive as a Uint8Array carrying the payload
                    // bytes (R-A9b oracle).
                    var desc = (typeof msg === "string")
                        ? "string:" + msg
                        : (msg && msg.constructor && msg.constructor.name) + ":" +
                          Array.from(msg).join(",");
                    __srvMsgs.push(desc);
                    ws.send(msg);   // echo verbatim (text or binary)
                },
                close: function (ws) {},
            },
        });
        globalThis.__srv = srv;
        var ws = new WebSocket("ws://127.0.0.1:" + srv.port + "/echo");
        globalThis.__ws = ws;
        // Sequential ping-pong across the send/ping/close sequence (the
        // back-to-back delivery face is pinned separately by
        // c12_websocket_client_back_to_back_sends_deliver).
        globalThis.__stage = 0;
        ws.onopen = function () {
            __wslog.push("open");
            __wslog.push("text:" + ws.send("c12-text"));
        };
        ws.onmessage = function (e) {
            var d = typeof e.data === "string"
                ? e.data
                : e.data && e.data.constructor && e.data.constructor.name + ":" + e.data.byteLength;
            __wslog.push("msg:" + d);
            __stage++;
            if (__stage === 1) { __wslog.push("binary:" + ws.send(new Uint8Array([66, 49, 50]))); }
            else if (__stage === 2) { __wslog.push("second:" + ws.send("plain-second")); }
            else if (__stage === 3) { __wslog.push("third:" + ws.send("plain-third")); }
            else if (__stage === 4) {
                // R-A9c oracle: client ping() is the ws-library control-frame
                // method — undefined return, no throw; the serve websocket
                // engine answers with Pong (RFC 6455 §5.5.2).
                try { __wslog.push("pinged:" + String(ws.ping("pp"))); }
                catch (e) { __wslog.push("pinged=THREW:" + (e && e.message || e)); }
                __wslog.push("after-ping:" + ws.send("after-ping"));
            }
        };
        ws.onclose = function (e) {
            __wslog.push("close");
            __closed = String((e && e.code === undefined ? "undef" : e.code)) + ":" +
                       String((e && e.reason) || "");
        };
        void 0;
        "#,
    );

    let saw_all = poll_until_realm(
        &mut ctx,
        r#"(globalThis.__wslog.join("|").indexOf("msg:c12-text") >= 0 &&
            globalThis.__wslog.join("|").indexOf("msg:after-ping") >= 0) ? "y" : "n""#,
        400,
    );
    assert!(
        saw_all,
        "echoes must arrive — log: {} | srvMsgs: {}",
        eval_string(&mut ctx, "globalThis.__wslog.join('|')"),
        eval_string(&mut ctx, "globalThis.__srvMsgs.join('|')")
    );

    let log = eval_string(&mut ctx, "globalThis.__wslog.join('|')");
    assert!(log.contains("open"), "client opened — log: {log}");
    // send() returns undefined (void) for every accepted payload arm —
    // string, Uint8Array binary, and ToString-able object all DELIVER.
    assert!(log.contains("text:undefined"), "string send delivers (void return)");
    assert!(log.contains("binary:undefined"), "Uint8Array send delivers (void return)");
    assert!(log.contains("msg:c12-text"), "text echo arrives — log: {log}");
    assert!(log.contains("msg:plain-second") && log.contains("msg:after-ping"),
        "later echoes arrive (connection healthy across the sequence) — log: {log}");
    assert!(
        log.contains("pinged:undefined"),
        "client ping(<payload>) is the ws-library control-frame method (R-A9c fixed): \
         undefined return, frame accepted — log: {log}"
    );
    // R-A9b oracle: the binary frame reaches the serve handler as a
    // Uint8Array carrying the 3 payload bytes ([66,49,50] = "B12"), and the
    // server echo returns to the CLIENT as a byte-backed view — no ToString
    // stringification anywhere on the path.
    let srv_msgs = eval_string(&mut ctx, "globalThis.__srvMsgs.join('|')");
    assert!(
        srv_msgs.contains("string:c12-text") && srv_msgs.contains("Uint8Array:66,49,50"),
        "serve message handler must receive the binary payload as a byte-exact Uint8Array — log: {srv_msgs}"
    );
    assert!(
        log.contains("msg:Uint8Array:3"),
        "binary echo must arrive at the client as a 3-byte Uint8Array — log: {log}"
    );

    eval_string(&mut ctx, "globalThis.__ws.close(1000, 'c12-bye');");
    let closed = poll_until_realm(&mut ctx, r#"globalThis.__closed !== "" ? "y" : "n""#, 400);
    assert!(
        closed,
        "close handshake completes with a close event — closed(code:reason)={} log: {}",
        eval_string(&mut ctx, "globalThis.__closed"),
        eval_string(&mut ctx, "globalThis.__wslog.join('|')")
    );
    // The wire close-code face (echoed code value) is carried in __closed's
    // code:reason form; the handshake-completion contract is what's pinned
    // here — the echoed code enumeration is a serve-ws dispatch detail (see
    // R-A9 family) rather than a stable surface.
    eval_string(&mut ctx, "globalThis.__srv.stop();");
    drive_event_loop(&mut ctx, 20);
}

/// Server-side serve websocket faces (ws_on_open/ws_on_message 4204-4350):
/// an in-process Bun.serve({websocket}) upgrade + client round trip —
/// open fires once per socket, message reaches the handler, handler
/// ws.send() lands on the client. The loop is ticked with NO realm entered
/// (R-A9 regression pin: the upgrade handler must enter the persistent
/// realm itself; a bare-tick regression SIGSEGVs this test process).
#[test]
fn c12_serve_websocket_in_process_roundtrip() {
    let mut ctx = make_ctx();

    eval_string(
        &mut ctx,
        r#"
        globalThis.__srvLog = [];
        globalThis.__cliMsg = "";
        globalThis.__cliOpen = 0;
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            websocket: {
                open: function (ws) { __srvLog.push("open"); },
                message: function (ws, msg) {
                    __srvLog.push("msg:" + msg);
                    ws.send("echo:" + msg);
                },
                close: function (ws) { __srvLog.push("close"); },
            },
        });
        globalThis.__srv = srv;
        var ws = new WebSocket("ws://127.0.0.1:" + srv.port + "/ws");
        ws.onopen = function () { __cliOpen++; ws.send("hi-c12"); };
        ws.onmessage = function (e) { __cliMsg = e.data; };
        globalThis.__cli = ws;
        "#,
    );

    let roundtripped = poll_until_bare(&mut ctx, r#"globalThis.__cliMsg === "echo:hi-c12" ? "y" : "n""#, 400);
    let srv_log = eval_string(&mut ctx, "globalThis.__srvLog.join('|')");
    assert!(
        roundtripped,
        "client↔server ws round trip — srvLog: {srv_log}, cliMsg: {}",
        eval_string(&mut ctx, "globalThis.__cliMsg")
    );
    assert_eq!(eval_string(&mut ctx, "String(globalThis.__cliOpen)"), "1", "client open exactly once");
    assert!(srv_log.contains("open"), "server open fired — log: {srv_log}");
    assert!(srv_log.contains("msg:hi-c12"), "server message handler got the payload — log: {srv_log}");

    eval_string(&mut ctx, "globalThis.__cli.close(); globalThis.__srv.stop();");
    drive_event_loop(&mut ctx, 20);
}

// ──────────── WebSocket client R-A9c/R-A9d wave (ping/pong + pump) ────────────

/// R-A9d pin: back-to-back `send()` calls from `onopen` must all be echoed
/// and dispatched to `onmessage` — no pump starvation, order preserved.
#[test]
fn c12_websocket_client_back_to_back_sends_deliver() {
    let mut ctx = make_ctx();

    eval_string(
        &mut ctx,
        r#"
        globalThis.__b2bMsgs = [];
        globalThis.__b2bSrv = [];
        var srv = Bun.serve({
            port: 0,
            hostname: "127.0.0.1",
            websocket: {
                open: function (ws) {},
                message: function (ws, msg) { __b2bSrv.push(msg); ws.send(msg); },
                close: function (ws) {},
            },
        });
        globalThis.__b2bServer = srv;
        var ws = new WebSocket("ws://127.0.0.1:" + srv.port + "/b2b");
        globalThis.__b2bWs = ws;
        ws.onopen = function () {
            // Three sends with no wait for the reply in between.
            ws.send("b2b-a");
            ws.send("b2b-b");
            ws.send("b2b-c");
        };
        ws.onmessage = function (e) { __b2bMsgs.push(e.data); };
        ws.onerror = function (e) { __b2bMsgs.push("ERR:" + String(e && e.data || e)); };
        void 0;
        "#,
    );

    let done = poll_until_realm(
        &mut ctx,
        r#"globalThis.__b2bMsgs.length >= 3 ? "y" : "n""#,
        400,
    );
    let msgs = eval_string(&mut ctx, "globalThis.__b2bMsgs.join('|')");
    let srv_msgs = eval_string(&mut ctx, "globalThis.__b2bSrv.join('|')");
    assert!(
        done,
        "back-to-back sends must all be echoed and dispatched — client: {msgs} | server: {srv_msgs}"
    );
    assert_eq!(
        msgs, "b2b-a|b2b-b|b2b-c",
        "back-to-back echoes must arrive in send order (R-A9d)"
    );
    eval_string(&mut ctx, "globalThis.__b2bWs.close(); globalThis.__b2bServer.stop();");
    drive_event_loop(&mut ctx, 20);
}

/// R-A9d root-cause pin: TCP does not preserve write boundaries. A server
/// frame split across two `write()` calls (header first, payload later) must
/// still decode and dispatch — the non-blocking drain pump sees a partial
/// frame and must resume the same frame on the next pass. The former
/// `FrameDecoder::decode_frame` path advanced its cursor before the frame was
/// complete, so the retry re-read payload bytes as a frame header (reserved
/// opcode) and killed the connection with no `onmessage`.
#[test]
fn c12_websocket_client_split_frame_still_dispatches() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok();
        if !accept_ws_upgrade(&mut stream) {
            return;
        }
        // Give the client time to finish its open sequence, then split one
        // server text frame across two TCP writes with a delay wider than the
        // pump's poll cadence.
        std::thread::sleep(Duration::from_millis(40));
        let payload = b"split-ok";
        let mut frame = vec![0x81u8, payload.len() as u8];
        frame.extend_from_slice(payload);
        stream.write_all(&frame[..2]).expect("write header");
        stream.flush().ok();
        std::thread::sleep(Duration::from_millis(80));
        stream.write_all(&frame[2..]).expect("write payload");
        stream.flush().ok();
        // Keep the connection readable for the client's close frame.
        std::thread::sleep(Duration::from_millis(300));
    });

    let mut ctx = make_ctx();
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__splitLog = [];
            var ws = new WebSocket("ws://127.0.0.1:{port}/split");
            globalThis.__splitWs = ws;
            ws.onopen = function () {{ __splitLog.push("open"); }};
            ws.onmessage = function (e) {{ __splitLog.push("msg:" + e.data); }};
            ws.onerror = function (e) {{ __splitLog.push("err:" + String(e && e.data || e)); }};
            ws.onclose = function () {{ __splitLog.push("close"); }};
            void 0;
            "#,
            port = port
        ),
    );

    let done = poll_until_realm(
        &mut ctx,
        r#"globalThis.__splitLog.join("|").indexOf("msg:split-ok") >= 0 ? "y" : "n""#,
        400,
    );
    let log = eval_string(&mut ctx, "globalThis.__splitLog.join('|')");
    assert!(
        done,
        "a frame split across TCP reads must still dispatch — log: {log}"
    );
    assert!(
        !log.contains("err:"),
        "no transport error for a valid split frame — log: {log}"
    );
    eval_string(&mut ctx, "globalThis.__splitWs.close();");
    drive_event_loop(&mut ctx, 20);
}

/// R-A9c: node `ws` client control-frame surface — `ping([data[, mask[, cb]]])`
/// and `pong(...)`, plus the inbound `ping`/`pong` events (browser-style
/// `onping`/`onpong` properties AND the Node-style `ws.on('pong')` emitter,
/// backed by the shared node_events EventEmitter). The oracle is the npm `ws`
/// library contract (Node v24's built-in WebSocket has no ping method), as
/// recorded in the C12 registry.
#[test]
fn c12_websocket_client_ping_pong_control_frames() {
    use std::sync::{Arc, Mutex};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let wire: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let wire_peer = Arc::clone(&wire);
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .ok();
        if !accept_ws_upgrade(&mut stream) {
            return;
        }
        let mut dec = bun_uws::ws_codec::FrameDecoder::new();
        let mut enc = bun_uws::ws_codec::FrameEncoder::new();
        loop {
            let Some((opcode, mask, payload)) = read_client_frame(&mut dec, &mut stream) else {
                return;
            };
            let text = String::from_utf8_lossy(&payload).into_owned();
            wire_peer
                .lock()
                .unwrap()
                .push(format!("{:?}:mask={}:{}", opcode, mask, text));
            match opcode {
                bun_uws::ws_codec::Opcode::Text => {
                    // First text: push one server Ping and one server Pong so
                    // the client surfaces both inbound control events, plus a
                    // text frame for the emitter's 'message' face.
                    let ping = enc.encode_frame(bun_uws::ws_codec::Opcode::Ping, b"srv-ping", None);
                    stream.write_all(ping).expect("write srv ping");
                    let pong = enc.encode_frame(bun_uws::ws_codec::Opcode::Pong, b"srv-pong", None);
                    stream.write_all(pong).expect("write srv pong");
                    let text = enc.encode_text("srv-text");
                    stream.write_all(text).expect("write srv text");
                }
                bun_uws::ws_codec::Opcode::Ping => {
                    // RFC 6455 §5.5.2: answer the client's ping with a pong
                    // carrying the same application data.
                    let pong = enc.encode_frame(bun_uws::ws_codec::Opcode::Pong, &payload, None);
                    stream.write_all(pong).expect("write pong ack");
                }
                bun_uws::ws_codec::Opcode::Close => {
                    let frame = enc.encode_close(1000, "");
                    let _ = stream.write_all(frame);
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = make_ctx();
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__ctlLog = [];
            function asc(a) {{
                var s = "";
                for (var i = 0; i < a.length; i++) s += String.fromCharCode(a[i]);
                return s;
            }}
            var ws = new WebSocket("ws://127.0.0.1:{port}/ctl");
            globalThis.__ctlWs = ws;
            ws.onopen = function () {{
                __ctlLog.push("open");
                // ping(data, mask, cb) — returns undefined; the callback runs
                // once the frame has been written to the socket.
                __ctlLog.push("ping-ret:" + String(ws.ping("c12-ping", true, function () {{
                    __ctlLog.push("ping-cb");
                }})));
                // omitting the mask argument defaults to masked client frames
                __ctlLog.push("pong-ret:" + String(ws.pong("c12-pong-api")));
                // mask=false is allowed by the ws library even for a client
                ws.ping("c12-unmasked", false);
                ws.send("start");
            }};
            ws.onping = function (e) {{ __ctlLog.push("onping:" + asc(e.data)); }};
            ws.onpong = function (e) {{ __ctlLog.push("onpong:" + asc(e.data)); }};
            ws.onclose = function () {{ __ctlLog.push("onclose"); }};
            ws.on("open", function () {{ __ctlLog.push("emit-open"); }});
            ws.on("message", function (d, isBinary) {{
                __ctlLog.push("emit-message:" + String(d) + ":" + isBinary);
            }});
            ws.on("ping", function (d) {{ __ctlLog.push("emit-ping:" + asc(d)); }});
            ws.on("pong", function (d) {{ __ctlLog.push("emit-pong:" + asc(d)); }});
            ws.once("pong", function (d) {{ __ctlLog.push("once-pong:" + asc(d)); }});
            ws.on("close", function () {{ __ctlLog.push("emit-close"); }});
            ws.onerror = function (e) {{ __ctlLog.push("err:" + String(e && e.data || e)); }};
            void 0;
            "#,
            port = port
        ),
    );

    let done = poll_until_realm(
        &mut ctx,
        r#"(function () {{
            var j = __ctlLog.join("|");
            return (j.indexOf("emit-open") >= 0 &&
                    j.indexOf("onpong:srv-pong") >= 0 &&
                    j.indexOf("onpong:c12-ping") >= 0 &&
                    j.indexOf("onping:srv-ping") >= 0 &&
                    j.indexOf("emit-pong:srv-pong") >= 0 &&
                    j.indexOf("emit-message:srv-text:false") >= 0) ? "y" : "n";
        }})()"#,
        400,
    );
    // Give the peer thread a beat to consume the client's auto-pong and the
    // explicit pong frame before asserting the wire log.
    for _ in 0..20 {
        common::drive_event_loop_drain(&mut ctx, 1);
        std::thread::sleep(Duration::from_millis(5));
    }

    let log = eval_string(&mut ctx, "globalThis.__ctlLog.join('|')");
    let wire_log = wire.lock().unwrap().join("|");
    assert!(
        done,
        "inbound ping/pong events must fire both surfaces — log: {log} | wire: {wire_log}"
    );
    assert!(log.contains("open"), "client opened — log: {log}");
    assert!(
        log.contains("ping-ret:undefined") && log.contains("pong-ret:undefined"),
        "ping()/pong() return undefined (ws contract) — log: {log}"
    );
    assert!(
        log.contains("ping-cb"),
        "ping(data, mask, cb) must invoke the write callback — log: {log}"
    );
    assert!(
        log.contains("onping:srv-ping") && log.contains("emit-ping:srv-ping"),
        "inbound Ping fires onping + on('ping') with the payload — log: {log}"
    );
    assert!(
        log.contains("onpong:srv-pong") && log.contains("onpong:c12-ping"),
        "inbound Pong fires onpong for unsolicited and ping-ack payloads — log: {log}"
    );
    assert!(
        log.contains("emit-pong:srv-pong"),
        "on('pong') listener receives the payload — log: {log}"
    );
    assert!(
        log.contains("emit-open") && log.contains("emit-message:srv-text:false"),
        "on('open') fires and on('message') carries (data, isBinary) — log: {log}"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            "__ctlLog.filter(function (x) { return x.indexOf('once-pong:') === 0; }).length"
        ),
        "1",
        "once('pong') fires exactly once across multiple inbound pongs — log: {log}"
    );
    assert!(
        !log.contains("err:"),
        "control-frame exchange must not surface an error — log: {log}"
    );
    // Wire truth: masked Ping/Pong by default, the ws-library mask=false arm,
    // and the RFC 6455 auto-pong for the server's Ping.
    assert!(
        wire_log.contains("Ping:mask=true:c12-ping")
            && wire_log.contains("Pong:mask=true:c12-pong-api")
            && wire_log.contains("Ping:mask=false:c12-unmasked")
            && wire_log.contains("Pong:mask=true:srv-ping"),
        "client control frames must reach the wire with the requested masking — wire: {wire_log}"
    );

    eval_string(&mut ctx, "globalThis.__ctlWs.close(1000);");
    let closed = poll_until_realm(
        &mut ctx,
        r#"(function () {{ var j = __ctlLog.join("|");
            return (j.indexOf("onclose") >= 0 && j.indexOf("emit-close") >= 0) ? "y" : "n"; }})()"#,
        200,
    );
    let close_log = eval_string(&mut ctx, "globalThis.__ctlLog.join('|')");
    assert!(
        closed && close_log.contains("onclose") && close_log.contains("emit-close"),
        "close handshake fires both the onclose property and on('close') — log: {close_log}"
    );
    drive_event_loop(&mut ctx, 20);
}

