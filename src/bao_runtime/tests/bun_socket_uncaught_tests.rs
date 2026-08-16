// @trace TEST-BAO-API-017 [req:REQ-BAO-API-017] [level:integration]
//
// Socket-callback throws route through the unified uncaught-exception router
// (REQ-ENG-006 semantics). bun_listen/bun_udp invoke_js_callback previously
// JS_ClearPendingException'd the throw away — a throwing Bun.listen data
// handler or Bun.udpSocket data handler vanished with no report and no exit.
// The contract now mirrors timer throws and EventEmitter listener throws
// (uncaught_exception_tests paths 1-3):
//   - no uncaughtException handler → default report (stderr stack, captured
//     here via uncaught::begin_capture) + exit requested + exit code 1
//   - handler registered → handler receives the thrown Error; the process
//     keeps running (exit untouched, later socket events still dispatch)

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;
use std::io::Write;
use std::net::TcpStream;
use std::net::UdpSocket;
use std::time::Duration;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Pump the unified event loop (uWS sockets + timers + jobs) so socket
/// dispatch runs on this thread. Stops early once an exit was requested —
/// the router already did its job by then.
fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — the production CLI pump shape. The UDP
/// dispatch path (udp_on_data payload allocation + invoke_js_callback) runs
/// with the realm entered by the eval's AutoRealm; a bare-Rust pump leaves
/// no realm current and SIGSEGVs the allocation (same class the listen
/// tests root in net_echo_e2e_tests).
fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
    let exhausted = HOOK_BUDGET.with(|b| {
        let n = b.get();
        if n == 0 {
            return true;
        }
        b.set(n - 1);
        false
    });
    if exhausted {
        return false;
    }
    bun_runtime::timers::drain_and_check(cx)
}

/// Bun.listen TCP server on an ephemeral port whose data handler throws.
/// Returns the bound port.
fn listen_throwing_data_server(ctx: &mut JsContext, marker: &str) -> u16 {
    let out = eval_str(
        ctx,
        &format!(
            r#"
            globalThis.__server = Bun.listen({{
                port: 0,
                hostname: "127.0.0.1",
                socket: {{
                    data: function (sock, data) {{
                        globalThis.__datas = (globalThis.__datas || 0) + 1;
                        throw new Error("{}");
                    }},
                }},
            }});
            (globalThis.__server && typeof globalThis.__server.port === "number"
                && globalThis.__server.port > 0)
                ? String(globalThis.__server.port)
                : "FAIL:" + JSON.stringify(globalThis.__server)
            "#,
            marker,
        ),
    );
    assert!(
        !out.starts_with("FAIL") && !out.starts_with("ERROR"),
        "Bun.listen TCP must bind an ephemeral port, got: {out}"
    );
    out.trim()
        .parse::<u16>()
        .unwrap_or_else(|e| panic!("port parse from {out:?}: {e}"))
}

/// Path 4a: a throw inside a Bun.listen TCP data callback with no
/// uncaughtException handler — default report + exit 1 (NOT swallowed).
#[test]
fn listen_tcp_data_callback_throw_without_handler_prints_and_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let port = listen_throwing_data_server(&mut ctx, "boom-listen-data");

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream.write_all(b"probe").expect("write probe");

    bun_runtime::uncaught::begin_capture();
    for _ in 0..150 {
        pump(&mut ctx, 2);
        if bun_runtime::should_exit() {
            break;
        }
    }
    let cap = bun_runtime::uncaught::take_capture();
    assert!(
        cap.contains("boom-listen-data"),
        "default report must carry the callback's error message, got: {cap}"
    );
    assert!(
        cap.contains("Error:"),
        "default report must carry the Error framing, got: {cap}"
    );
    assert!(
        bun_runtime::should_exit(),
        "listen data-callback throw without handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "listen data-callback throw without handler must exit 1"
    );
}

/// Path 4b: with an uncaughtException handler registered, the handler
/// receives the thrown Error and the process keeps running — later data
/// events still dispatch (second write observed).
#[test]
fn listen_tcp_data_callback_throw_with_handler_receives_error_and_loop_continues() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        globalThis.got = null;
        process.on('uncaughtException', function (e) { globalThis.got = e.message; });
        'ok'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "handler registration must succeed: {:?}", r.err());
    let port = listen_throwing_data_server(&mut ctx, "handled-listen-boom");

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream.write_all(b"first").expect("write first");
    let mut datas = String::new();
    for _ in 0..150 {
        pump(&mut ctx, 2);
        datas = eval_str(&mut ctx, "String(globalThis.__datas || 0)");
        if datas == "1" {
            break;
        }
    }
    assert_eq!(datas, "1", "first data event must dispatch (and throw)");

    stream.write_all(b"second").expect("write second");
    for _ in 0..150 {
        pump(&mut ctx, 2);
        datas = eval_str(&mut ctx, "String(globalThis.__datas || 0)");
        if datas == "2" {
            break;
        }
    }
    assert_eq!(
        datas, "2",
        "after a handled throw the loop must keep dispatching later data events"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.got"),
        "handled-listen-boom",
        "uncaughtException handler must receive the thrown Error object"
    );
    assert!(
        !bun_runtime::should_exit(),
        "handled socket-callback throw must not request exit (handler decides)"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "handled socket-callback throw must leave the exit code untouched"
    );
}

/// Path 4c: a throw inside a Bun.udpSocket data callback with no handler —
/// default report + exit 1 (NOT swallowed).
///
/// Two environment notes:
/// - a keep-alive Bun.listen TCP server holds the BCE-007 liveness token:
///   the UDP bridge registers none, so without an active server
///   drain_and_check never ticks the uWS loop that dispatches UDP packets;
/// - the pump is the post-eval hook (realm-entered drain — production CLI
///   shape), because udp_on_data allocates with the caller's realm current.
#[test]
fn udp_data_callback_throw_without_handler_prints_and_exits_1() {
    let mut ctx = make_ctx();
    ctx.set_post_eval_hook(bounded_drain_hook);
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        globalThis.__keep = Bun.listen({ port: 0, hostname: "127.0.0.1", socket: {} });
        globalThis.__sock = null;
        Bun.udpSocket({
            hostname: "127.0.0.1",
            port: 0,
            socket: {
                data: function (data, port, address, flags) {
                    throw new Error("boom-udp-data");
                },
            },
        }).then(function (s) { globalThis.__sock = s; });
        'started'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());

    // The udpSocket promise resolves synchronously in bun_udp_socket and its
    // then-reaction runs in the eval tail's RunJobs — read the OS-assigned
    // ephemeral port directly.
    let port_str = eval_str(
        &mut ctx,
        "globalThis.__sock ? String(globalThis.__sock.address().port) : \"0\"",
    );
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        panic!("udpSocket promise must resolve with a bound socket, port got: {port_str}")
    });

    let sender = UdpSocket::bind("127.0.0.1:0").expect("udp bind sender");
    sender
        .send_to(b"probe-udp", ("127.0.0.1", port))
        .expect("udp send");

    bun_runtime::uncaught::begin_capture();
    let mut exited = false;
    for _ in 0..60 {
        HOOK_BUDGET.with(|b| b.set(4));
        let _ = eval_str(&mut ctx, "'tick'");
        if bun_runtime::should_exit() {
            exited = true;
            break;
        }
    }
    let cap = bun_runtime::uncaught::take_capture();
    assert!(
        cap.contains("boom-udp-data"),
        "default report must carry the UDP callback's error message, got: {cap}"
    );
    assert!(
        exited && bun_runtime::should_exit(),
        "udp data-callback throw without handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "udp data-callback throw without handler must exit 1"
    );
}
