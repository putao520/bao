// @trace TEST-BAO-API-17 [req:REQ-BAO-API-017] [level:integration]
// Bun.listen TCP-mode behavior, rooted from the IIFE/bridge sweep (bbe20a81):
//
// 1. error-on-success contract — us_internal_bind_and_listen writes
//    *error = LIBUS_ERR after a SUCCESSFUL listen() (bsd.c:921), so the old
//    `is_null() || err != 0` success test destroyed a group that still had
//    the listening socket linked → us_socket_group_deinit's
//    head_listen_sockets assert → SIGABRT on every Bun.listen TCP path.
//    The return value is the authority (NULL ⟺ failure).
// 2. listen(0) must expose the OS-assigned ephemeral port.
// 3. BCE-007 liveness registration: an idle script (no pending JS timers)
//    must still accept an inbound connection — drain_and_check only ticks
//    the uWS loop while node_http::has_active_servers() is true.
// 4. stop() is clean and idempotent, and drops the liveness token.

use bao_engine::context::JsContext;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
#[path = "common/mod.rs"]
mod common;
use common::bounded_drain_hook;

use common::eval_str;

/// Fixed-pass pump delegate: the unified suite pump (`common::pump_until`)
/// run for `passes` settle-pump ticks (each tick = realm-entered timer
/// drain + MiniEventLoop tick + RunJobs, the uWS sockets + timers + jobs
/// in-process server accept discipline). A live-but-unsettled marker never
/// changes, so the `PumpStuck` tripwire fires at its 2s window instead of
/// silently burning the (longer) pass budget.
fn pump(ctx: &mut JsContext, passes: usize) {
    let budget = Duration::from_millis((passes as u64).saturating_mul(5));
    common::pump_until(ctx, "'pumping'", budget, common::eval_string_full, |_| {
        false
    });
}

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    common::wait_until_with(ctx, js_condition, budget, 60, eval_str)
}

/// Full Bun.listen TCP lifecycle: real port, idle accept of a real inbound
/// TCP connection, clean double-stop, liveness token dropped.
#[test]
fn bun_listen_tcp_port0_idle_accept_and_clean_stop() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Before the root-cause this eval aborted the whole test process: the
    // success path was misjudged as failure and destroyed the live group.
    let out = eval_str(
        &mut ctx,
        r#"
        globalThis.__opens = 0;
        globalThis.__datas = 0;
        globalThis.__dataBytes = 0;
        globalThis.__server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            socket: {
                open: function() { globalThis.__opens++; },
                // Bun API: data(socket, data) — payload is the second arg.
                data: function(sock, data) {
                    globalThis.__datas++;
                    globalThis.__dataBytes += (data && data.length) || 0;
                },
                close: function() {},
                end: function() {},
            },
        });
        (globalThis.__server && typeof globalThis.__server.port === "number"
            && globalThis.__server.port > 0)
            ? "port=" + globalThis.__server.port
            : "FAIL:" + JSON.stringify(globalThis.__server)
    "#,
    );
    assert!(
        out.starts_with("port="),
        "Bun.listen TCP listen(0) must expose the real ephemeral port, got: {out}"
    );
    let port: u16 = out["port=".len()..]
        .trim()
        .parse()
        .expect("ephemeral port parse");

    // Real TCP roundtrip from the Rust side during JS-idle: with no pending
    // JS timer, only the liveness registration keeps drain_and_check ticking
    // the loop (BCE-007 class) — the accept and data callbacks must fire.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(b"probe-bytes").expect("write probe"); // 11 bytes

    let mut counters = String::new();
    for _ in 0..150 {
        pump(&mut ctx, 2);
        // Drain whatever the kernel delivered (no echo semantics asserted here
        // — the open/data dispatch counters are the contract under test).
        let mut chunk = [0u8; 256];
        let _ = stream.read(&mut chunk);
        counters = eval_str(
            &mut ctx,
            "globalThis.__opens + \"/\" + globalThis.__datas + \"/\" + globalThis.__dataBytes",
        );
        if counters == "1/1/11" {
            break;
        }
    }
    assert_eq!(
        counters, "1/1/11",
        "idle loop must accept the inbound connection and deliver all bytes \
         (open/data/byteCount counters)"
    );

    // Clean stop, then idempotent second stop (handles nullified).
    let stopped = eval_str(
        &mut ctx,
        "globalThis.__server.stop(); globalThis.__server.stop(); \"stopped\"",
    );
    assert_eq!(stopped, "stopped");

    // Liveness token dropped — the drain loop can go idle again.
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "server.stop() must unregister the BCE-007 liveness token"
    );
}

/// The listen port is genuinely bound: a second listener on the same explicit
/// port must fail (EADDRINUSE shape: null-ish server, not a live one), and
/// the failed path must not tear down the first server's group.
#[test]
fn bun_listen_tcp_addr_inuse_fails_cleanly() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let first = eval_str(
        &mut ctx,
        r#"
        globalThis.__first = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: { open: function(){}, data: function(){}, close: function(){}, end: function(){} },
        });
        "port=" + globalThis.__first.port
    "#,
    );
    assert!(first.starts_with("port="), "first listener: {first}");
    let port: u16 = first["port=".len()..].trim().parse().expect("port parse");

    // Same explicit port → us listen fails → NULL return → failure branch
    // (destroy is correct there: the group has NO live listen socket).
    let second = eval_str(
        &mut ctx,
        r#"
        var s2 = Bun.listen({
            port: PORT, hostname: "127.0.0.1",
            socket: { open: function(){}, data: function(){}, close: function(){}, end: function(){} },
        });
        (s2 === undefined || s2 === null || !s2) ? "rejected" : "leaked:" + s2.port
    "#.replace("PORT", &port.to_string()).as_str(),
    );
    assert_eq!(
        second, "rejected",
        "second listener on a taken port must fail"
    );

    // First server still alive and registered.
    assert!(bun_runtime::node_http::has_active_servers());
    eval_str(&mut ctx, "globalThis.__first.stop(); \"ok\"");
    assert!(!bun_runtime::node_http::has_active_servers());
}

/// Bun.listen TCP accept identity + Bun.connect full roundtrip — both sides
/// JS over a real TCP connection. Covers the two #21 sweep follow-up gaps:
///   1. Bun.listen's accept callbacks previously fired with NO socket
///      identity (open/data/close/end) — the accepted connection could not
///      be addressed from JS.
///   2. Bun.connect resolved its Promise with a bare {_socketPtr} that had
///      no write/end methods (and the synchronous loopback path never
///      settled the promise at all).
#[test]
fn bun_listen_accept_identity_and_bun_connect_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;

        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            socket: {
                open: function(sock) {
                    log.push("open_sock=" + (sock && typeof sock.write === "function"
                        && typeof sock.remotePort === "number" && sock.remotePort > 0));
                },
                data: function(sock, data) {
                    log.push("data_sock=" + (sock && typeof sock.write === "function"));
                    sock.write(data); // echo back through the SAME identity
                },
                close: function(sock) { log.push("srv_close=" + (sock !== undefined)); },
                end: function(sock) { if (sock) sock.end(); },
            },
        });
        log.push("port=" + (server.port > 0));

        var conn = Bun.connect({
            hostname: "127.0.0.1",
            port: server.port,
            socket: {
                // Bun API parity with the listen side: data(socket, data) —
                // the client socket identity is the first argument.
                data: function(sock, data) {
                    log.push("cli_sock=" + (sock && typeof sock.write === "function"));
                    log.push("cli_data=" + data);
                    setTimeout(function() { sock.end(); }, 0);
                },
                close: function() {
                    log.push("cli_close");
                    server.stop();
                    globalThis.__done = true;
                },
            },
        });
        conn.then(function(sock) {
            globalThis.__cliSock = sock;
            log.push("resolved=" + (sock && typeof sock.write === "function"
                && typeof sock.end === "function" && typeof sock.remoteAddress === "string"));
            sock.write("ping");
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "Bun TCP roundtrip must complete; log: {log}");

    for part in [
        "port=true",
        "open_sock=true",
        "data_sock=true",
        "resolved=true",
        "cli_sock=true",
        "cli_data=ping",
        "cli_close",
        "srv_close=true",
    ] {
        assert!(
            log.split('|').any(|entry| entry == part),
            "roundtrip log must contain '{part}', got: {log}"
        );
    }
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "server.stop() must drop the liveness token"
    );
}

/// e208 face ① (e206 adjacency): ConnectUserData carried the pending
/// Bun.connect Promise as a bare `*mut JSObject`. The box is reachable from
/// the uWS VTable callbacks across loop ticks — connect_on_open reads the
/// stored pointer AFTER the user's `open` callback returns — so any moving
/// GC inside that callback (nursery churn / `Bun.gc(true)` compaction, which
/// relocates survivors) leaves the stored pointer at the pre-move address
/// and the resolve arm dereferences a moved-from cell.
///
/// Carrier: a real loopback connect whose `open` callback first churns the
/// nursery (forces an evacuation) and then runs `Bun.gc(true)` (compacting
/// Shrink GC — moves survivors) before returning. Post-fix the VTable arm
/// re-reads the raw-rooted slot, so the promise still resolves with a live
/// socket identity and the roundtrip completes.
#[test]
fn bun_connect_promise_survives_gc_in_open_callback() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__log = [];
        globalThis.__done = false;
        globalThis.__resolved = false;
        globalThis.__sockOk = false;
        var server = Bun.listen({
            port: 0, hostname: "127.0.0.1",
            socket: {
                open: function() {},
                data: function(sock, data) { sock.write(data); },
                close: function() {},
                end: function(sock) { sock.end(); },
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1",
            port: server.port,
            socket: {
                open: function(sock) {
                    // Moving-GC storm inside the open callback: the VTable
                    // resolve arm reads the stored promise AFTER this
                    // returns. 30k nursery objects force an evacuation;
                    // Bun.gc(true) is the compacting Shrink GC on top.
                    for (var i = 0; i < 30000; i++) { ({ n: i }); }
                    Bun.gc(true);
                    globalThis.__log.push("open");
                },
                data: function(sock, data) {
                    globalThis.__log.push("data");
                    setTimeout(function() { sock.end(); }, 0);
                },
                close: function() {
                    globalThis.__log.push("close");
                    globalThis.__done = true;
                },
            },
        });
        conn.then(function(sock) {
            globalThis.__resolved = true;
            globalThis.__sockOk = !!(sock && typeof sock.write === "function"
                && typeof sock.end === "function");
            sock.write("ping");
        }, function() {
            globalThis.__log.push("unexpected-reject");
            globalThis.__done = true;
        });
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 120);
    let log = eval_str(&mut ctx, "globalThis.__log.join('|')");
    assert!(done, "connect roundtrip must complete; log: {log}");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__resolved ? 'y' : 'n'"),
        "y",
        "Bun.connect promise must resolve after the open-callback GC storm; log: {log}"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__sockOk ? 'y' : 'n'"),
        "y",
        "resolved value must carry a live socket identity; log: {log}"
    );
}
