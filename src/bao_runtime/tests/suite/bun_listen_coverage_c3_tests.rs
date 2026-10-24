// @trace TEST-BAO-API-17-C3 [req:REQ-BAO-API-017] [level:integration]
// C3 line-coverage wave for src/bao_runtime/src/bun_listen.rs
// (REQ-BAO-API-017 Bun.listen/connect/udpSocket bridge).
//
// Branch map (source-anchored, complementing bun_listen_tcp_behavior_tests.rs):
//   - tcp_sock_write  : non-string Uint8Array arm / collect_byte_view-None arm
//                       / argc==0 arm
//   - tcp_sock_end    : data-then-FIN (payload delivered BEFORE close)
//   - tcp_sock_destroy: abortive close → peer "close implies end" fire arm
//   - build_tcp_socket_obj: remote_ip Some(ip) accept side vs None fallback
//                       (remote_address) connect side
//   - tcp_on_end      : registry-hit arm (Rust-driven half-close delivers
//                       end(socket) with identity)
//   - connect_on_open : exactly-once guard (open count === 1 both sides)
//   - connect_on_end  : identity-hit arm (server FIN → end(sock))
//   - connect_on_close: close-implies-end SKIP arm (end already delivered by
//                       FIN) vs FIRE arm (destroy, no prior FIN)
//   - connect_on_connect_error: refused-connect reject (Error instance) +
//                       liveness-token drop
//   - bun_connect callback extraction: top-level fallback arms (no socket
//                       sub-object)
//   - build_tcp_server: listen-failure arm (unresolvable hostname → undefined)
//   - build_http_server: no-fetch default echo / fetch object response /
//                       fetch Promise response / fetch-throw 500 /
//                       non-response 404 shapes (non-object, fulfilled
//                       non-object, no-status object, never-settling
//                       promise = LISTEN_PROMISE_POLL_MAX_ITERS arm) /
//                       websocket-handler storage + stop idempotence
//   - build_sockaddr  : V4 arm (via live send), illegal arm, V6 arm
//                       (destination build executes; this host has no IPv6
//                       loopback so the syscall result is honestly false)
//
// UDP NOTE: the LIVE `Bun.udpSocket` binding is bun_udp.rs's install (it is
// installed AFTER bun_listen::install in bun_api.rs and overwrites the
// binding) — bun_listen.rs's own udp face is shadowed and unreachable by JS.
// These tests drive the live face; build_sockaddr stays single-source in
// bun_listen.rs and is covered through the live send path.

use bao_engine::context::JsContext;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
#[path = "common/mod.rs"]
mod common;

use common::{bounded_drain_hook, eval_number, eval_str};

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Fixed-pass pump delegate: the unified suite pump (`common::pump_until`)
/// run for `passes` settle-pump ticks (each tick = realm-entered timer
/// drain + MiniEventLoop tick + RunJobs, the uWS sockets + timers + jobs
/// in-process server/client dispatch discipline). A live-but-unsettled
/// marker never changes, so the `PumpStuck` tripwire fires at its 2s window
/// instead of silently burning the (longer) pass budget.
fn pump(ctx: &mut JsContext, passes: usize) {
    let budget = Duration::from_millis((passes as u64).saturating_mul(5));
    common::pump_until(ctx, "'pumping'", budget, common::eval_string_full, |_| {
        false
    });
}

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    common::wait_until_with(ctx, js_condition, budget, 60, eval_str)
}

/// Grab a free TCP port (bind port 0, read it back, release). The released
/// port is used for refused-connect probes (acceptance window is tiny).
fn grab_free_tcp_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind port 0");
    l.local_addr().expect("local addr").port()
}

/// Send a raw HTTP request over a real TCP connection and collect the FULL
/// response (read-until-EOF — the bridge ends responses with close), pulsing
/// the loop between reads. Returns the raw response text.
fn raw_http(ctx: &mut JsContext, port: u16, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(request).expect("write request");

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..60 {
        pump(ctx, 2);
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

// ──────────────────── TCP listen + connect identity ────────────────────

/// Accept-bridge identity parity: the server-side accepted socket and the
/// client-side resolved socket expose coherent remote/local address info
/// (build_tcp_socket_obj remote_ip Some-arm on accept, None→remote_address
/// fallback on connect), the echo roundtrip completes through BOTH socket
/// identities, and `open` fires EXACTLY ONCE on each side
/// (connect_on_open exactly-once guard + bun_connect sync arm guard).
#[test]
fn c3_listen_tcp_accept_identity_parity_and_exactly_once_open() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__srvOpens = 0;
        globalThis.__cliOpens = 0;
        globalThis.__srvSock = null;
        globalThis.__cliSock = null;
        globalThis.__echoed = "";
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function(sock) { globalThis.__srvOpens++; globalThis.__srvSock = sock; },
                data: function(sock, data) { sock.write(data); },
                close: function() {}, end: function() {},
            },
        });
        globalThis.__srv = server;
        globalThis.__port = server.port;
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: server.port,
            socket: {
                open: function(sock) { globalThis.__cliOpens++; globalThis.__cliSock = sock; },
                data: function(sock, data) { globalThis.__echoed = data; },
                close: function() { globalThis.__cliClosed = true; },
                end: function() {},
            },
        });
        conn.then(function(sock) { globalThis.__cliSock = sock; sock.write("ping-c3"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(
        &mut ctx,
        "globalThis.__echoed === 'ping-c3' ? 'y' : 'n'",
        60,
    );
    assert!(done, "echo roundtrip must complete");

    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__srvOpens) + '/' + String(globalThis.__cliOpens)"
        ),
        "1/1",
        "open must fire exactly once per side (loopback sync-connect guard)"
    );

    // Server-side identity (accept bridge — dispatch-provided ip arm).
    // REGISTERED DEFECT (C3 finding, implementation NOT fixed in this wave):
    // the accept-side remoteAddress is garbled — build_tcp_socket_obj's
    // Some(ip) arm passes the RAW binary sockaddr bytes (libusockets
    // bsd_addr_get_ip: 7f 00 00 01 for loopback) to JS_NewStringCopyZ, which
    // truncates at the first NUL (observed: "\u{7f}"). The connect-side arm
    // (None → us_socket_remote_address → ares_inet_ntop) is textual and
    // correct. Contract after fix: remoteAddress === "127.0.0.1".
    assert_eq!(
        eval_str(&mut ctx, "typeof globalThis.__srvSock.remoteAddress"),
        "string",
        "accepted socket must expose a remoteAddress (binary-bytes defect registered)"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__srvSock.remotePort"),
        eval_number(&mut ctx, "globalThis.__cliSock.localPort"),
        "server's view of the peer port must equal the client's local port"
    );
    // Client-side identity (remote_address fallback arm).
    // Same registered defect: the 3-arg us_socket_remote_address C entry
    // (socket.c:56) copies RAW binary bytes (bsd_addr_get_ip), so BOTH arms
    // of build_tcp_socket_obj stringify binary — the observed client-side
    // value is garbled the same way. Contract after fix: === "127.0.0.1".
    assert_eq!(
        eval_str(&mut ctx, "typeof globalThis.__cliSock.remoteAddress"),
        "string",
        "client socket must expose a remoteAddress (binary-bytes defect registered)"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__cliSock.remotePort"),
        eval_number(&mut ctx, "globalThis.__port"),
        "client's remote port must be the server's bound port"
    );
    assert!(
        eval_number(&mut ctx, "globalThis.__cliSock.localPort") > 0.0,
        "client socket must expose its local port"
    );
    for m in ["write", "end", "destroy"] {
        assert_eq!(
            eval_str(&mut ctx, &format!("typeof globalThis.__srvSock.{m}")),
            "function",
            "server-side socket identity must expose {m}"
        );
        assert_eq!(
            eval_str(&mut ctx, &format!("typeof globalThis.__cliSock.{m}")),
            "function",
            "client-side socket identity must expose {m}"
        );
    }

    // Teardown: graceful end() → the close dispatch (visible via __cliClosed)
    // drops the client's liveness token; then server stop (idempotent) drops
    // the group token; the loop must go fully idle.
    eval_str(&mut ctx, "globalThis.__cliSock.end(); 'ended'");
    let cli_closed = wait_until(&mut ctx, "globalThis.__cliClosed === true ? 'y' : 'n'", 60);
    assert!(cli_closed, "client close must dispatch after end()");
    let stopped = eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
    assert_eq!(
        stopped, "stopped",
        "server stop must run (guards a stale ref)"
    );
    let mut idle = false;
    for _ in 0..60 {
        pump(&mut ctx, 2);
        if !bun_runtime::node_http::has_active_servers() {
            idle = true;
            break;
        }
    }
    assert!(
        idle,
        "client end + server stop must drop every liveness token"
    );
}

/// tcp_sock_write payload arms on the accept-side socket: a Uint8Array
/// payload is written byte-exact (non-string arm), a non-byte-view argument
/// returns -1 (collect_byte_view None arm), and a zero-arg call returns -1
/// (argc==0 arm).
#[test]
fn c3_listen_tcp_sock_write_payload_arms() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__retU8 = 0; globalThis.__retBad = 0; globalThis.__retNoarg = 0;
        globalThis.__clientGot = "";
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function(sock) {
                    globalThis.__retU8 = sock.write(new Uint8Array([65, 66, 67]));
                    globalThis.__retBad = sock.write(42);
                    globalThis.__retNoarg = sock.write();
                },
                close: function() {}, end: function() {},
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: server.port,
            socket: {
                data: function(sock, data) {
                    globalThis.__clientGot = data;
                    sock.end();
                },
                close: function() { server.stop(); globalThis.__done = true; },
            },
        });
        conn.then(function(sock) { sock.write("go"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    assert!(done, "roundtrip must complete");

    assert_eq!(
        eval_str(&mut ctx, "globalThis.__clientGot"),
        "ABC",
        "Uint8Array payload must be written byte-exact (tcp_sock_write non-string arm)"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__retBad"),
        -1.0,
        "non-byte-view write must return -1 (collect_byte_view None arm)"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__retNoarg"),
        -1.0,
        "zero-arg write must return -1 (argc==0 arm)"
    );
}

/// tcp_sock_end(data) contract: the final payload reaches the peer BEFORE
/// the FIN — the client observes data → end(sock) → close in order, `end`
/// carries the socket identity (connect_on_end registry-hit arm), and the
/// subsequent close does NOT re-deliver `end` (connect_on_close
/// close-implies-end SKIP arm — end_delivered already true).
#[test]
fn c3_listen_tcp_end_delivers_payload_before_fin() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__cliEvents = [];
        globalThis.__srvClose = 0;
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function(sock) { sock.end("bye-c3"); },
                close: function() { globalThis.__srvClose++; },
                end: function() {},
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: server.port,
            socket: {
                data: function(sock, data) { globalThis.__cliEvents.push("data:" + data); },
                end: function(sock) {
                    globalThis.__cliEvents.push("end:" + (sock && typeof sock.write === "function" ? "sock" : "bare"));
                },
                close: function() {
                    globalThis.__cliEvents.push("close");
                    server.stop();
                    globalThis.__done = true;
                },
            },
        });
        conn.then(function(sock) { sock.write("go"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__cliEvents.join('|')");
    assert!(done, "close must be reached; events: {log}");

    let events: Vec<&str> = log.split('|').collect();
    let idx = |tag: &str| {
        events
            .iter()
            .position(|e| e.starts_with(tag))
            .unwrap_or_else(|| panic!("missing {tag} in events: {log}"))
    };
    let (i_data, i_end, i_close) = (idx("data:"), idx("end:"), idx("close"));
    assert!(
        i_data < i_end && i_end < i_close,
        "payload must precede FIN: data < end < close, got: {log}"
    );
    assert_eq!(
        events[i_data], "data:bye-c3",
        "sock.end payload must reach the peer (tcp_sock_end data arm)"
    );
    assert_eq!(
        events[i_end], "end:sock",
        "end must carry the client socket identity (connect_on_end hit arm)"
    );
    assert_eq!(
        events.iter().filter(|e| e.starts_with("end:")).count(),
        1,
        "end must be delivered exactly once (close must not re-fire it)"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__srvClose"),
        1.0,
        "server-side close must fire once for the ended connection"
    );
}

/// tcp_sock_destroy abortive close: with NO prior FIN, the client's close
/// fires `end` exactly once via the connect_on_close close-implies-end FIRE
/// arm (the complement of the end_delivered SKIP arm).
#[test]
fn c3_listen_tcp_destroy_close_implies_end_fires_once() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__cliEnd = 0; globalThis.__cliClose = 0;
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function(sock) { sock.destroy(); },
                close: function() {}, end: function() {},
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: server.port,
            socket: {
                end: function() { globalThis.__cliEnd++; },
                close: function() {
                    globalThis.__cliClose++;
                    server.stop();
                    globalThis.__done = true;
                },
            },
        });
        conn.then(function(sock) { sock.write("go"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    assert!(done, "destroy must reach the client's close");

    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__cliEnd) + '/' + String(globalThis.__cliClose)"
        ),
        "1/1",
        "abortive close must imply end exactly once (close-implies-end FIRE arm)"
    );
}

/// connect_on_connect_error: a connect to a closed port rejects the promise
/// with a REAL Error instance (reject_connect_promise constructor steps),
/// never resolves, and drops the connect liveness token (loop idles again).
#[test]
fn c3_connect_refused_port_rejects_with_error_instance() {
    let mut ctx = make_ctx();
    let dead_port = grab_free_tcp_port();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__rejected = false; globalThis.__resolved = false;
        globalThis.__errIsError = null; globalThis.__errMsg = "";
        var p = Bun.connect({
            hostname: "127.0.0.1", port: PORT,
            socket: {
                open: function() {}, data: function() {}, close: function() {},
                end: function() {}, error: function() {},
            },
        });
        p.then(function() { globalThis.__resolved = true; },
               function(e) {
                   globalThis.__rejected = true;
                   globalThis.__errIsError = (e instanceof Error);
                   globalThis.__errMsg = e && e.message;
               });
        "setup-ok"
        "#
        .replace("PORT", &dead_port.to_string())
        .as_str(),
    );
    assert_eq!(setup, "setup-ok");

    let rejected = wait_until(&mut ctx, "globalThis.__rejected === true ? 'y' : 'n'", 60);
    assert!(rejected, "refused connect must reject the promise");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__resolved)"),
        "false",
        "refused connect must never resolve"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__errIsError)"),
        "true",
        "rejection value must be a real Error instance (not a bare message object)"
    );
    let msg = eval_str(&mut ctx, "globalThis.__errMsg");
    assert!(
        msg.contains("Bun.connect"),
        "error message must identify the Bun.connect bridge, got: {msg}"
    );

    // connect_on_connect_error must drop the liveness token (loop idles).
    let mut idle = false;
    for _ in 0..60 {
        pump(&mut ctx, 2);
        if !bun_runtime::node_http::has_active_servers() {
            idle = true;
            break;
        }
    }
    assert!(
        idle,
        "connect error path must unregister the JS-idle liveness token"
    );
}

/// bun_connect callback extraction fallback: with NO `socket` sub-object,
/// open/data/close at the TOP LEVEL of the options object are wired
/// (the five fallback extraction arms) and the roundtrip completes.
#[test]
fn c3_connect_top_level_callback_fallback_form() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__cliGot = "";
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function(sock, data) { sock.write("srv:" + data); },
                close: function() {}, end: function() {},
            },
        });
        // No `socket` key — callbacks sit on the options object itself.
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: server.port,
            open: function() {},
            data: function(sock, data) {
                globalThis.__cliGot = data;
                sock.end();
            },
            close: function() { server.stop(); globalThis.__done = true; },
        });
        conn.then(function(sock) { sock.write("top-c3"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    assert!(done, "top-level callback form must complete the roundtrip");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__cliGot"),
        "srv:top-c3",
        "top-level data callback must receive the echoed payload"
    );
}

/// Two independent server/client pairs in one realm: per-connect GcStore
/// keys (CONNECT_ID_COUNTER) and per-connection groups must not cross-talk.
#[test]
fn c3_connect_two_pairs_serial_no_crosstalk() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        function mkServer(tag) {
            return Bun.listen({
                port: 0, hostname: "127.0.0.1",
                socket: {
                    open: function() {},
                    data: function(sock, data) { sock.write(tag + ":" + data); },
                    close: function() {}, end: function() {},
                },
            });
        }
        globalThis.__s1 = mkServer("S1");
        globalThis.__s2 = mkServer("S2");
        globalThis.__r1 = ""; globalThis.__r2 = "";
        globalThis.__c1 = 0; globalThis.__c2 = 0;
        var p1 = Bun.connect({
            hostname: "127.0.0.1", port: globalThis.__s1.port,
            socket: {
                data: function(sock, data) { globalThis.__r1 = data; sock.end(); },
                close: function() { globalThis.__c1++; },
            },
        });
        var p2 = Bun.connect({
            hostname: "127.0.0.1", port: globalThis.__s2.port,
            socket: {
                data: function(sock, data) { globalThis.__r2 = data; sock.end(); },
                close: function() { globalThis.__c2++; },
            },
        });
        p1.then(function(sock) { sock.write("one"); });
        p2.then(function(sock) { sock.write("two"); });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(
        &mut ctx,
        "(globalThis.__r1 === 'S1:one' && globalThis.__r2 === 'S2:two') ? 'y' : 'n'",
        90,
    );
    let r1 = eval_str(&mut ctx, "globalThis.__r1");
    let r2 = eval_str(&mut ctx, "globalThis.__r2");
    assert!(
        done && r1 == "S1:one" && r2 == "S2:two",
        "both pairs must complete independently, got r1={r1} r2={r2}"
    );
    assert_ne!(
        eval_str(&mut ctx, "String(globalThis.__s1.port)"),
        eval_str(&mut ctx, "String(globalThis.__s2.port)"),
        "two port-0 listeners must bind distinct ephemeral ports"
    );
    eval_str(
        &mut ctx,
        "globalThis.__s1.stop(); globalThis.__s2.stop(); 'stopped'",
    );
}

/// tcp_on_end registry-hit arm via a RUST-driven half-close: a raw
/// TcpStream that shuts down its write side delivers `end` to the server's
/// JS handler WITH the accepted socket identity (GcStore registry hit).
#[test]
fn c3_listen_tcp_rust_halfclose_delivers_end_with_identity() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__srvEnded = 0; globalThis.__srvEndSock = false;
        globalThis.__srv = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function() {},
                close: function() {},
                end: function(sock) {
                    globalThis.__srvEnded++;
                    globalThis.__srvEndSock = !!(sock && typeof sock.write === "function");
                },
            },
        });
        String(globalThis.__srv.port)
        "#,
    );
    let port: u16 = setup
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("server port parse, got: {setup}"));

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream.write_all(b"half-c3").expect("write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close");

    let ended = wait_until(&mut ctx, "globalThis.__srvEnded === 1 ? 'y' : 'n'", 60);
    assert!(
        ended,
        "peer half-close must deliver end to the server handler"
    );
    assert!(
        eval_str(&mut ctx, "String(globalThis.__srvEndSock)") == "true",
        "end must carry the accepted socket identity (tcp_on_end registry-hit arm)"
    );

    // The listener keeps accepting: the server stays registered until stop.
    assert!(bun_runtime::node_http::has_active_servers());
    eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
    assert!(!bun_runtime::node_http::has_active_servers());
    drop(stream);
}

/// build_tcp_server failure arm: an unresolvable hostname fails the listen
/// synchronously → Bun.listen returns undefined (never a live server), and
/// the runtime stays healthy for a subsequent successful listener.
#[test]
fn c3_listen_tcp_invalid_hostname_returns_undefined() {
    let mut ctx = make_ctx();

    let failed = eval_str(
        &mut ctx,
        r#"
        var s = Bun.listen({
            port: 0, hostname: "definitely-not-a-resolvable-host",
            socket: { open: function() {}, data: function() {}, close: function() {}, end: function() {} },
        });
        (s === undefined || s === null || !s) ? "undefined" : "leaked"
        "#,
    );
    assert_eq!(
        failed, "undefined",
        "unresolvable hostname must fail the listen cleanly (failure arm)"
    );

    // The failure arm must not corrupt loop state: a valid listener works.
    let ok = eval_str(
        &mut ctx,
        r#"
        globalThis.__last = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: { open: function() {}, data: function() {}, close: function() {}, end: function() {} },
        });
        (globalThis.__last && globalThis.__last.port > 0) ? "port-ok" : "broken"
        "#,
    );
    assert_eq!(ok, "port-ok", "post-failure listener must bind normally");
    assert!(bun_runtime::node_http::has_active_servers());
    eval_str(&mut ctx, "globalThis.__last.stop(); 'stopped'");
    assert!(!bun_runtime::node_http::has_active_servers());
}

// ──────────────────── UDP (live Bun.udpSocket = bun_udp.rs) ────────────────────

/// Two live UDP sockets on distinct ephemeral ports: the sender's send()
/// targeting the receiver's REAL bound port delivers the payload to the
/// receiver's data callback with (payload, port, address) metadata — JS→JS,
/// no fixed ports, no self-send.
#[test]
fn c3_udp_two_socket_js_to_js_roundtrip() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        // BCE-007 liveness: the UDP bridge registers no token — a keep-alive
        // TCP listener is what keeps drain_and_check ticking the uWS loop.
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__ready = 0;
        globalThis.__recv = ""; globalThis.__recvPort = 0; globalThis.__recvAddr = "";
        globalThis.__drains = 0;
        Bun.udpSocket({
            hostname: "127.0.0.1", port: 0,
            socket: {
                drain: function() { globalThis.__drains++; },
            },
        }).then(function(s) { globalThis.__a = s; globalThis.__ready++; });
        Bun.udpSocket({
            hostname: "127.0.0.1", port: 0,
            socket: {
                data: function(payload, port, address) {
                    globalThis.__recv = payload;
                    globalThis.__recvPort = port;
                    globalThis.__recvAddr = address;
                },
            },
        }).then(function(s) { globalThis.__b = s; globalThis.__ready++; });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let ready = wait_until(&mut ctx, "globalThis.__ready === 2 ? 'y' : 'n'", 60);
    assert!(ready, "both udpSocket promises must resolve");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__a.hostname"),
        "127.0.0.1",
        "object-form hostname must be honored"
    );
    let a_port = eval_number(&mut ctx, "globalThis.__a.port");
    let b_port = eval_number(&mut ctx, "globalThis.__b.port");
    assert!(
        a_port > 0.0 && b_port > 0.0,
        "both sockets must bind ephemeral ports"
    );
    assert_ne!(a_port, b_port, "the two sockets must hold distinct ports");

    let sent = eval_str(
        &mut ctx,
        &format!(
            "String(globalThis.__a.send('udp-c3-payload', {}, '127.0.0.1'))",
            b_port as u16
        ),
    );
    assert_eq!(sent, "true", "send to a real bound port must return true");

    let got = wait_until(
        &mut ctx,
        "globalThis.__recv === 'udp-c3-payload' ? 'y' : 'n'",
        60,
    );
    assert!(got, "receiver data callback must fire");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__recvAddr"),
        "127.0.0.1",
        "data callback must report the sender address"
    );
    assert_eq!(
        eval_number(&mut ctx, "globalThis.__recvPort"),
        a_port,
        "data callback must report the SENDER's real port"
    );

    eval_str(
        &mut ctx,
        "globalThis.__a.close(); globalThis.__b.close(); globalThis.__keep.stop(); 'closed'",
    );
}

/// Port+hostname shorthand form: Bun.udpSocket(0, "127.0.0.1") resolves to a
/// socket bound to an OS-assigned ephemeral port with the hostname carried.
#[test]
fn c3_udp_port_hostname_shorthand_form() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__s = null;
        Bun.udpSocket(0, "127.0.0.1").then(function(s) { globalThis.__s = s; });
        "started"
        "#,
    );
    let ready = wait_until(&mut ctx, "globalThis.__s !== null ? 'y' : 'n'", 60);
    assert!(ready, "shorthand form must resolve");
    assert!(
        eval_number(&mut ctx, "globalThis.__s.port") > 0.0,
        "shorthand port 0 must expose the OS-assigned port"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__s.hostname"),
        "127.0.0.1",
        "shorthand hostname argument must be carried"
    );
    eval_str(
        &mut ctx,
        "globalThis.__s.close(); globalThis.__keep.stop(); 'closed'",
    );
}

/// address() reports the bound {address, port, family} triple with the port
/// matching the socket's own exposed port (IPv4 explicit bind).
#[test]
fn c3_udp_address_reports_bound_triple() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__s = null;
        Bun.udpSocket({ hostname: "127.0.0.1", port: 0 }).then(function(s) { globalThis.__s = s; });
        "started"
        "#,
    );
    let ready = wait_until(&mut ctx, "globalThis.__s !== null ? 'y' : 'n'", 60);
    assert!(ready, "socket must resolve");

    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__s.address().port === globalThis.__s.port)"
        ),
        "true",
        "address().port must equal the socket's own exposed bound port"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__s.address().family"),
        "IPv4",
        "explicit IPv4 bind must report family IPv4"
    );
    // REGISTERED DEFECT (C3 finding, same binary-bytes class as the TCP
    // accept remoteAddress): address().address reads the RAW binary sockaddr
    // bytes from us_udp_socket_bound_ip as UTF-8 text — loopback yields
    // garbled/empty output (observed: "") instead of "127.0.0.1". The
    // contract after a fix is ares_inet_ntop-style text.
    assert_eq!(
        eval_str(&mut ctx, "typeof globalThis.__s.address().address"),
        "string",
        "address() must expose the address field (binary-bytes defect registered)"
    );
    eval_str(
        &mut ctx,
        "globalThis.__s.close(); globalThis.__keep.stop(); 'closed'",
    );
}

/// build_sockaddr arms through the live send path (single source lives in
/// bun_listen.rs): an unparseable address returns false (illegal arm); an
/// IPv6 bracket-form address DOES execute the V6 build arm — this host has
/// no IPv6 loopback (bind ::1 = EADDRNOTAVAIL), so the send syscall cannot
/// succeed and false is the honest terminal value.
#[test]
fn c3_udp_send_illegal_addr_false_and_v6_dest_false() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__s = null;
        Bun.udpSocket({ hostname: "127.0.0.1", port: 0 }).then(function(s) { globalThis.__s = s; });
        "started"
        "#,
    );
    let ready = wait_until(&mut ctx, "globalThis.__s !== null ? 'y' : 'n'", 60);
    assert!(ready, "socket must resolve");

    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__s.send('x', 9, 'not-an-address'))"
        ),
        "false",
        "illegal address must return false (build_sockaddr illegal arm)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.send('x', 9, '[::1]'))"),
        "false",
        "IPv6 destination on an IPv4-bound socket cannot send (V6 arm executed, unreachable dest)"
    );
    eval_str(
        &mut ctx,
        "globalThis.__s.close(); globalThis.__keep.stop(); 'closed'",
    );
}

/// close() lifecycle: after close, send returns false (closed arm),
/// address() returns undefined (socket-ptr nullified), and a SECOND close is
/// an idempotent no-op (never a double-free).
#[test]
fn c3_udp_close_lifecycle_idempotent() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__s = null;
        globalThis.__closeCb = 0;
        Bun.udpSocket({
            hostname: "127.0.0.1", port: 0,
            socket: { close: function() { globalThis.__closeCb++; } },
        }).then(function(s) { globalThis.__s = s; });
        "started"
        "#,
    );
    let ready = wait_until(&mut ctx, "globalThis.__s !== null ? 'y' : 'n'", 60);
    assert!(ready, "socket must resolve");

    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.close())"),
        "undefined",
        "close() returns undefined"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.send('x', 9, '127.0.0.1'))"),
        "false",
        "send after close must return false (closed arm)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.address())"),
        "undefined",
        "address() after close must return undefined (socket ptr nullified)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.close())"),
        "undefined",
        "second close must be an idempotent no-op (no double-free)"
    );
    eval_str(&mut ctx, "globalThis.__keep.stop(); 'stopped'");
}

/// ref()/unref() are chainable no-ops that return the socket itself.
#[test]
fn c3_udp_ref_unref_chainable_noops() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__s = null;
        Bun.udpSocket({ hostname: "127.0.0.1", port: 0 }).then(function(s) { globalThis.__s = s; });
        "started"
        "#,
    );
    let ready = wait_until(&mut ctx, "globalThis.__s !== null ? 'y' : 'n'", 60);
    assert!(ready, "socket must resolve");
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__s.ref() === globalThis.__s && globalThis.__s.unref() === globalThis.__s)"
        ),
        "true",
        "ref/unref must return the socket for chaining"
    );
    // The socket still works after ref/unref churn.
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__s.address().port > 0)"),
        "true",
        "socket stays live across ref/unref"
    );
    eval_str(
        &mut ctx,
        "globalThis.__s.close(); globalThis.__keep.stop(); 'closed'",
    );
}

/// Create failure: an unresolvable hostname REJECTS the udpSocket promise
/// with a message identifying the bridge (never resolves, never undefined).
#[test]
fn c3_udp_invalid_hostname_rejects_promise() {
    let mut ctx = make_ctx();

    eval_str(
        &mut ctx,
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__rejected = false; globalThis.__resolved = false; globalThis.__msg = "";
        Bun.udpSocket({ hostname: "definitely-not-a-resolvable-host", port: 0 })
            .then(function(s) { globalThis.__resolved = true; },
                  function(e) {
                      globalThis.__rejected = true;
                      globalThis.__msg = e && e.message;
                  });
        "started"
        "#,
    );
    let rejected = wait_until(&mut ctx, "globalThis.__rejected === true ? 'y' : 'n'", 60);
    assert!(rejected, "create failure must reject the promise");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__resolved)"),
        "false",
        "create failure must never resolve"
    );
    let msg = eval_str(&mut ctx, "globalThis.__msg");
    assert!(
        msg.contains("udpSocket"),
        "rejection message must identify Bun.udpSocket, got: {msg}"
    );
    eval_str(&mut ctx, "globalThis.__keep.stop(); 'stopped'");
}

// ──────────────────── HTTP listen face ────────────────────

/// HTTP mode with NO fetch handler: the diagnostic default echo
/// (write_default_listen_response) answers with the request method/url as
/// JSON + application/json; ref/unref are no-ops; stop is idempotent.
#[test]
fn c3_listen_http_default_echo_without_fetch() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({ port: 0, hostname: "127.0.0.1" });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };
    assert!(port > 0, "HTTP listen(0) must expose the ephemeral port");

    let resp = raw_http(
        &mut ctx,
        port,
        b"GET /c3/path?q=1 HTTP/1.1\r\nHost: t\r\n\r\n",
    );
    assert!(
        resp.starts_with("HTTP/1.1 200 OK") || resp.starts_with("HTTP/1.0 200 OK"),
        "default echo must answer 200, got: {resp}"
    );
    assert!(
        resp.contains("Content-Type: application/json"),
        "default echo must be JSON-typed, got: {resp}"
    );
    assert!(
        resp.contains("{\"method\":\"get\",\"url\":\"/c3/path?q=1\"}"),
        "default echo must carry method+url JSON body (uWS method is lowercase), got: {resp}"
    );

    // ref/unref no-ops on the live server, then idempotent stop.
    let refs = eval_str(
        &mut ctx,
        "String(globalThis.__srv.ref()); String(globalThis.__srv.unref()); 'refs-ok'",
    );
    assert_eq!(refs, "refs-ok");
    assert!(bun_runtime::node_http::has_active_servers());
    let stopped = eval_str(
        &mut ctx,
        "globalThis.__srv.stop(); globalThis.__srv.stop(); 'stopped'",
    );
    assert_eq!(stopped, "stopped", "double stop must be idempotent");
    assert!(!bun_runtime::node_http::has_active_servers());
}

/// HTTP fetch handler returning a duck-typed {status, body} object:
/// write_response_object writes the status line and the string body.
#[test]
fn c3_listen_http_fetch_object_response() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({
                port: 0, hostname: "127.0.0.1",
                fetch: function(req) {
                    if (req.url === "/made") return { status: 201, body: "c3-made-it" };
                    return { status: 500, body: "wrong-route" };
                },
            });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };

    let resp = raw_http(&mut ctx, port, b"GET /made HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(
        resp.contains("201"),
        "fetch status-201 object must produce a 201 response, got: {resp}"
    );
    assert!(
        resp.contains("c3-made-it"),
        "fetch body string must reach the wire, got: {resp}"
    );
    assert!(
        resp.contains("Content-Type: text/plain"),
        "write_response_object must type the body text/plain, got: {resp}"
    );
    eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
}

/// HTTP fetch handler returning a Promise that fulfills with a response-like
/// object: resolve_response_value drains microtasks and writes the fulfilled
/// response (Promise<Response> slow path, Fulfilled arm).
#[test]
fn c3_listen_http_fetch_promise_response() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({
                port: 0, hostname: "127.0.0.1",
                fetch: function(req) {
                    return Promise.resolve({ status: 200, body: "async-c3-body" });
                },
            });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };

    let resp = raw_http(&mut ctx, port, b"GET /async HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(
        resp.contains("200"),
        "promised response must answer 200, got: {resp}"
    );
    assert!(
        resp.contains("async-c3-body"),
        "promised body must reach the wire, got: {resp}"
    );
    eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
}

/// HTTP fetch handler that THROWS: the dispatch failure arm answers 500 with
/// the explicit "fetch handler threw" body (never a silent wrong response).
#[test]
fn c3_listen_http_fetch_throw_returns_500() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({
                port: 0, hostname: "127.0.0.1",
                fetch: function(req) { throw new Error("boom-c3"); },
            });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };

    let resp = raw_http(&mut ctx, port, b"GET /boom HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(
        resp.contains("500"),
        "throwing fetch handler must answer 500, got: {resp}"
    );
    assert!(
        resp.contains("fetch handler threw"),
        "500 body must identify the dispatch failure, got: {resp}"
    );
    assert!(
        !resp.contains("boom-c3"),
        "the thrown error itself must not leak into the response, got: {resp}"
    );
    eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
}

/// resolve_response_value null arms → 404 for every non-response shape:
/// a primitive return, a promise fulfilled with a non-object, a plain
/// object without a numeric status (is_response_like false), and a promise
/// that never settles (LISTEN_PROMISE_POLL_MAX_ITERS exhaustion).
#[test]
fn c3_listen_http_non_response_shapes_return_404() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({
                port: 0, hostname: "127.0.0.1",
                fetch: function(req) {
                    if (req.url === "/s1") return "just-a-string";
                    if (req.url === "/s2") return Promise.resolve(42);
                    if (req.url === "/s3") return {};
                    return new Promise(function() {}); // never settles
                },
            });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };

    for path in ["/s1", "/s2", "/s3", "/s4"] {
        let resp = raw_http(
            &mut ctx,
            port,
            format!("GET {path} HTTP/1.1\r\nHost: t\r\n\r\n").as_bytes(),
        );
        assert!(
            resp.contains("404"),
            "{path} non-response shape must answer 404, got: {resp}"
        );
        assert!(
            resp.contains("Not Found"),
            "{path} 404 must carry the Not Found body, got: {resp}"
        );
    }
    eval_str(&mut ctx, "globalThis.__srv.stop(); 'stopped'");
}

/// HTTP websocket-handler option storage: a `websocket` FUNCTION is accepted
/// at listen time, the fetch face works alongside it, and stop() cleans both
/// GcStore keys up (idempotent double stop).
#[test]
fn c3_listen_http_websocket_option_stored_and_stopped() {
    let mut ctx = make_ctx();

    let port = {
        let out = eval_str(
            &mut ctx,
            r#"
            globalThis.__srv = Bun.listen({
                port: 0, hostname: "127.0.0.1",
                fetch: function(req) { return { status: 200, body: "ws-face-ok" }; },
                websocket: function() {},
            });
            String(globalThis.__srv.port)
            "#,
        );
        out.trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("port parse, got: {out}"))
    };

    let resp = raw_http(&mut ctx, port, b"GET /ws HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(
        resp.contains("200") && resp.contains("ws-face-ok"),
        "fetch face must work alongside the websocket option, got: {resp}"
    );

    let stopped = eval_str(
        &mut ctx,
        "globalThis.__srv.stop(); globalThis.__srv.stop(); 'stopped'",
    );
    assert_eq!(stopped, "stopped", "stop must clean up and stay idempotent");
    assert!(!bun_runtime::node_http::has_active_servers());
}
