// @trace TEST-BAO-API-017-C12 [req:REQ-ENG-006] [req:REQ-BAO-API-017] [level:integration]
// C12 line-coverage wave for src/bao_runtime/src/bun_listen.rs — residual
// gap after C3 (fresh-lcov gap ≈ 857 lines; JS-reachable subset covered here).
//
// Branch map (source-anchored, complementing bun_listen_coverage_c3_tests.rs):
//   - build_http_server : server.ref()/unref() no-op faces (1025-1035),
//                         port double-parse arm (278-279)
//   - listen_route_handler throw arm (430-525): a throwing fetch handler
//                         answers 500, keeps serving, and routes the pending
//                         exception to process.on("uncaughtException") (R-A7
//                         class, both sides)
//   - write_response_object (3455-3520): plain-object fetch response arms —
//                         int status → `<code> OK` status line (reason phrase
//                         is ALWAYS "OK" — registered shape), string body arm,
//                         object body (toString) arm, empty-body arm
//   - bun_connect       : tls option arm (1374), hostname async path
//                         (ConnectResult::Connecting wait loop 1392-1407 +
//                         error arms), port double-parse arm (1196-1197),
//                         top-level open/end fallback extraction (1265-1266,
//                         socket sub-object present but handler missing)
//   - tcp_on_open       : accept-side identity bridge (remote_ip Some arm,
//                         2617-2625) → remoteAddress/remotePort/localPort
//   - connect_on_open   : refused-connect remote_port()<=0 guard (2908-2918)
//   - connect_on_data/end/close : identity arms (3015-3019, 3061-3065,
//                         3124-3133) via a full client→server→client round trip
//   - tcp_sock_write/end: argc==0 → -1 arm (2374), collect_byte_view None
//                         → -1 arm
//
// REGISTRY (unreachable-by-JS / divergent shapes — registered per the C12
// contract; each needs a product-side decision, NOT a test fix):
//   R-L1. bun_listen's own Bun.udpSocket face (1873-2311) + udp vtable
//         handlers (3200-3282): SHADOWED — bun_udp.rs installs its
//         Bun.udpSocket AFTER bun_listen::install and overwrites the binding
//         (same shadowing note as the C3 header). ~430 lines.
//   R-L2. bun_connect's own plain socket object methods socket_write /
//         socket_end / socket_destroy (1663-1825) + their only consumer
//         get_connect_ud (3702-3723): DEAD FACE — the object is built and
//         given methods but never exposed to JS (the promise resolves with
//         the build_tcp_socket_obj identity from the pointer-scoped
//         GcStore; the private object leaks). ~170 lines.
//   R-L3. write_response_object's non-int status arm (3495-3499): only
//         reachable if resolve_response_value admits an object whose status
//         is not an int32 — empirically it does not ({status:"nope"} is
//         rejected as not response-like → default 404 arm runs instead).
//   R-L4. listen_route_handler null-cx / no-realm / unresolvable-handler
//         500 arms (449-497), build_tcp_server/build_http_server
//         JS_NewPlainObject + get_loop failure arms (916-925, 1332-1336,
//         1445-1458): not forceable from JS without fault injection.
//   R-L5. tcp_on_long_timeout / tcp_on_timeout (2733-2751): uWS long-idle
//         timeout passthroughs — 10-minute idle horizon, impractical.
//   R-L6. tcp_on_handshake (2826-2837): requires a TLS listener
//         (Bun.listen tls face) — separate campaign.
//   R-L7. Hostname-DNS-failure connect face: environment-dependent (needs a
//         working resolver for fast NXDOMAIN); the refused/TLS faces cover
//         the same reject arms deterministically.
//   R-L8. [FIXED — oracle assertion] Accepted-socket remoteAddress is the
//         dotted-quad / colon-hex text Node/Bun surface ("127.0.0.1" for
//         loopback). build_tcp_socket_obj previously handed the raw sockaddr
//         bytes to JS_NewStringCopyZ, which stopped at the first NUL and
//         degenerated loopback to a single char (0x7F).

use bao_engine::context::JsContext;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
#[path = "common/mod.rs"]
mod common;

use common::{bounded_drain_hook, eval_str};

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
/// drain + MiniEventLoop tick + RunJobs, the C3 in-process server/client
/// accept/parse/dispatch discipline). A live-but-unsettled marker never
/// changes, so the `PumpStuck` tripwire fires at its 2s window instead of
/// silently burning the (longer) pass budget — an unbounded `passes` under
/// a stuck loop is the hang shape this delegate now diagnoses.
fn pump(ctx: &mut JsContext, passes: usize) {
    let budget = Duration::from_millis((passes as u64).saturating_mul(5));
    common::pump_until(ctx, "'pumping'", budget, common::eval_string_full, |_| {
        false
    });
}

/// Poll `cond` (a JS expression that must yield "y"/"n") until "y".
fn wait_until(ctx: &mut JsContext, js_condition: &str, iters: u32) -> bool {
    common::wait_until_with(ctx, js_condition, 60, iters, eval_str)
}

/// Grab a free TCP port for refused-connect probes (acceptance window tiny).
fn grab_free_tcp_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind port 0");
    l.local_addr().expect("local addr").port()
}

/// Send a raw HTTP request and collect the FULL response (read-until-EOF),
/// pulsing the loop between reads. Returns the raw response text.
fn raw_http(ctx: &mut JsContext, port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read timeout");
    stream.write_all(request.as_bytes()).expect("write request");

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

/// First real status line of a raw response, skipping interim
/// "HTTP/1.1 100 Continue" lines uWS may emit for deferred writes.
fn status_line_of(resp: &str) -> String {
    resp.lines()
        .find(|l| l.starts_with("HTTP/1.1") && !l.starts_with("HTTP/1.1 100"))
        .unwrap_or("")
        .to_string()
}

// ──────────────────── HTTP server faces ────────────────────

/// HTTP server .ref()/.unref() are no-op faces returning undefined
/// (server_ref/server_unref, 1025-1035), and the server KEEPS SERVING after
/// both calls — pinning the liveness contract, not just the return values.
#[test]
fn c12_http_server_ref_unref_keep_serving() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__seen = "";
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                globalThis.__seen = req.url;
                return { status: 200, body: "ref-ok" };
            },
        });
        var r1 = server.ref();
        var r2 = server.unref();
        globalThis.__refFaces = String(r1) + "|" + String(r2) + "|" + String(server.port > 0);
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok", "server setup");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__refFaces"),
        "undefined|undefined|true",
        "ref/unref return undefined; port is bound"
    );

    let port = eval_str(&mut ctx, "String(server.port)")
        .parse::<u16>()
        .expect("port");
    let resp = raw_http(
        &mut ctx,
        port,
        "GET /refaced HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        status_line_of(&resp),
        "HTTP/1.1 200 OK",
        "server must keep serving after ref+unref — got: {resp}"
    );
    assert!(
        resp.contains("ref-ok"),
        "body must round-trip — got: {resp}"
    );

    pump(&mut ctx, 20);
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__seen"),
        "/refaced",
        "fetch handler observed the request url"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 5);
}

/// The HTTP port option accepts a double and clamps it through the
/// `max(0) as u16` parse arm (278-279): port 0.5 binds an ephemeral port.
#[test]
fn c12_http_port_double_parse_arm() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        var server = Bun.listen({
            port: 0.5,
            hostname: "127.0.0.1",
            fetch: function () { return { status: 200, body: "double-port" }; },
        });
        String(server.port > 0)
        "#,
    );
    assert_eq!(setup, "true", "0.5 must clamp to 0 → ephemeral bind");

    let port = eval_str(&mut ctx, "String(server.port)")
        .parse::<u16>()
        .expect("port");
    let resp = raw_http(
        &mut ctx,
        port,
        "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        resp.contains("double-port"),
        "server on clamped port answers — got: {resp}"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 5);
}

/// fetch returning a PLAIN OBJECT goes through write_response_object
/// (3455-3520): int status → `<code> OK` status line (reason phrase is
/// ALWAYS "OK" — registered shape), string body arm, object body via
/// toString() arm, empty-body arm. An object whose status is not an int32 is
/// rejected as not-response-like by resolve_response_value → default 404
/// (R-L3).
#[test]
fn c12_http_fetch_plain_object_response_arms() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                if (req.url === "/coded") return { status: 404, body: "gone" };
                if (req.url === "/objbody")
                    return { status: 201, body: { toString: function () { return "obj-bytes"; } } };
                if (req.url === "/strstatus") return { status: "nope", body: "still-200" };
                if (req.url === "/emptybody") return { status: 204 };
                return { status: 200, body: "default" };
            },
        });
        String(server.port)
        "#,
    );
    let port: u16 = setup.parse().expect("bound port");

    let resp = raw_http(
        &mut ctx,
        port,
        "GET /coded HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        status_line_of(&resp),
        "HTTP/1.1 404 OK",
        "plain-object status → '<code> OK' line (always-OK reason, registered) — got: {resp}"
    );
    assert!(resp.contains("gone"), "string body arm — got: {resp}");

    let resp = raw_http(
        &mut ctx,
        port,
        "GET /objbody HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    let line = status_line_of(&resp);
    assert!(
        line == "HTTP/1.1 201 OK" && resp.contains("obj-bytes"),
        "object body goes through toString() arm — got: {resp}"
    );

    let resp = raw_http(
        &mut ctx,
        port,
        "GET /strstatus HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        status_line_of(&resp),
        "HTTP/1.1 404 Not Found",
        "non-int status object is rejected as not response-like → default 404 (R-L3) — got: {resp}"
    );

    let resp = raw_http(
        &mut ctx,
        port,
        "GET /emptybody HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(
        status_line_of(&resp),
        "HTTP/1.1 204 OK",
        "missing body → empty-body end arm — got: {resp}"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 5);
}

/// R-A7 class (listen side): a throwing fetch handler answers 500, the
/// server keeps serving, AND the pending exception reaches
/// process.on("uncaughtException") — the same routing contract as the
/// serve side (BCE same-class sweep, not a silent clear).
#[test]
fn c12_http_fetch_throw_routes_uncaught_and_keeps_serving() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__lcUncaught = "";
        process.on("uncaughtException", function (e) {
            globalThis.__lcUncaught = String((e && e.message) || e);
        });
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            fetch: function (req) {
                if (req.url === "/boom") throw new Error("c12-listen-boom");
                return { status: 200, body: "ok-after" };
            },
        });
        String(server.port)
        "#,
    );
    let port: u16 = setup.parse().expect("bound port");

    let resp = raw_http(
        &mut ctx,
        port,
        "GET /boom HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        status_line_of(&resp).starts_with("HTTP/1.1 500"),
        "throwing listen route → 500 — got: {resp}"
    );
    let routed = wait_until(
        &mut ctx,
        r#"globalThis.__lcUncaught !== "" ? "y" : "n""#,
        60,
    );
    assert!(
        routed,
        "listen fetch-handler throw must reach uncaughtException"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__lcUncaught"),
        "c12-listen-boom",
        "uncaughtException handler observes the original listen route error"
    );

    let after = raw_http(
        &mut ctx,
        port,
        "GET /ok HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    assert!(
        after.contains("ok-after"),
        "server survives a route throw — got: {after}"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 5);
}

// ──────────────────── Bun.connect faces ────────────────────

/// Refused loopback connect: the promise REJECTS with a real Error, the
/// `error` callback fires, and `open` never fires. Exercises the
/// refused-dispatch family (tcp_on_connect_error 2775-2788 and/or the
/// Connecting→error arms 1392-1419 depending on kernel timing — both are
/// characterization-equivalent at the JS surface).
#[test]
fn c12_connect_refused_promise_rejects() {
    let mut ctx = make_ctx();
    let refused = grab_free_tcp_port();

    eval_str(
        &mut ctx,
        &format!(
            r#"
            globalThis.__opened = 0;
            globalThis.__errored = 0;
            globalThis.__rejected = "";
            Bun.connect({{
                hostname: "127.0.0.1",
                port: {port},
                socket: {{
                    open: function () {{ globalThis.__opened++; }},
                    error: function () {{ globalThis.__errored++; }},
                    close: function () {{}},
                }},
            }}).then(
                function () {{ globalThis.__rejected = "resolved"; }},
                function (e) {{ globalThis.__rejected = (e instanceof Error) ? "rejected-error" : "rejected-other"; }}
            );
            "#,
            port = refused
        ),
    );

    let settled = wait_until(&mut ctx, r#"globalThis.__rejected !== "" ? "y" : "n""#, 400);
    assert!(settled, "promise must settle on refused connect");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__rejected"),
        "rejected-error",
        "refused connect rejects with an Error instance"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__opened)"),
        "0",
        "open must not fire"
    );
    pump(&mut ctx, 10);
}

/// tls:true selects SocketKind::BunSocketTls (bun_connect 1374 arm); against
/// a REFUSED port the connect still fails fast at the TCP layer and the
/// promise rejects — the TLS selection itself must not crash or hang.
#[test]
fn c12_connect_tls_arm_refused_fails_fast() {
    let mut ctx = make_ctx();
    let refused = grab_free_tcp_port();

    eval_str(
        &mut ctx,
        &format!(
            r#"
            globalThis.__tlsState = "pending";
            Bun.connect({{
                hostname: "127.0.0.1",
                port: {port},
                tls: true,
                socket: {{
                    error: function () {{}},
                    close: function () {{}},
                }},
            }}).then(
                function () {{ globalThis.__tlsState = "resolved"; }},
                function (e) {{ globalThis.__tlsState = (e instanceof Error) ? "rejected" : "other"; }}
            );
            "#,
            port = refused
        ),
    );

    let settled = wait_until(
        &mut ctx,
        r#"globalThis.__tlsState !== "pending" ? "y" : "n""#,
        400,
    );
    assert!(settled, "tls connect to a refused port must settle fast");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__tlsState"),
        "rejected",
        "tls connect failure rejects with an Error"
    );
    pump(&mut ctx, 10);
}

/// Full client lifecycle over the hostname async path ("localhost" requires
/// name resolution → ConnectResult::Connecting wait loop): open fires with a
/// socket identity, client write arrives as server data with identity,
/// server half-close delivers end(sock) BEFORE close, and server close
/// delivers close(sock, code). Covers the connect_on_* identity arms.
#[test]
fn c12_connect_localhost_async_full_lifecycle() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__log = [];
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            socket: {
                open: function (sock) {
                    globalThis.__srvSock = sock;
                    globalThis.__log.push("srv-open:" + (sock.remotePort > 0));
                },
                data: function (sock, data) {
                    globalThis.__log.push("srv-data:" + data);
                    sock.write("pong:" + data);
                    sock.end();           // half-close AFTER the reply
                },
                close: function (sock, code) {
                    globalThis.__log.push("srv-close:" + code);
                },
            },
        });
        globalThis.__server = server;
        var conn = Bun.connect({
            hostname: "localhost",
            port: server.port,
            socket: {
                open: function (sock) {
                    globalThis.__cliSock = sock;
                    globalThis.__log.push("cli-open:" + (typeof sock.write === "function"));
                    sock.write("ping");
                },
                data: function (sock, data) { globalThis.__log.push("cli-data:" + data); },
                end: function (sock) { globalThis.__log.push("cli-end"); sock.close(); },
                close: function (sock, code) { globalThis.__log.push("cli-close:" + code); },
            },
        });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok", "listen+connect setup");

    let done = wait_until(
        &mut ctx,
        r#"globalThis.__log.join(",").indexOf("cli-close") >= 0 ? "y" : "n""#,
        400,
    );
    let log = eval_str(&mut ctx, "globalThis.__log.join('|')");
    assert!(done, "lifecycle must complete — log: {log}");

    assert!(
        log.contains("cli-open:true"),
        "client open carries a socket identity — log: {log}"
    );
    assert!(
        log.contains("srv-data:ping"),
        "server data callback got the client payload — log: {log}"
    );
    assert!(
        log.contains("cli-data:pong:ping"),
        "client received the server reply — log: {log}"
    );
    // end BEFORE close on the client side (half-close ordering contract)
    let end_idx = log.find("cli-end").expect("cli-end in log");
    let close_idx = log.find("cli-close").expect("cli-close in log");
    assert!(
        end_idx < close_idx,
        "end must fire before close — log: {log}"
    );
    assert!(
        log.contains("srv-close:"),
        "server close fires with a code — log: {log}"
    );

    // The accept-side identity bridge (tcp_on_open 2617-2625). R-L8 oracle:
    // remoteAddress is the node v24 dotted-quad text for the loopback peer
    // (previously a single 0x7F char because raw sockaddr bytes were treated
    // as a NUL-terminated C string).
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__srvSock.remoteAddress"),
        "127.0.0.1",
        "remoteAddress must be the dotted-quad loopback (node oracle; R-L8)"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__srvSock.remotePort > 0 && globalThis.__srvSock.localPort === globalThis.__server.port)"
        ),
        "true",
        "accepted socket remotePort/localPort faces"
    );
    eval_str(&mut ctx, "globalThis.__server.stop(); true");
    pump(&mut ctx, 10);
}

/// Client-side socket.write byte-view payload: the resolved connect socket
/// (build_tcp_socket_obj → tcp_sock_write) writes a Uint8Array byte-exact to
/// the server, and no-arg / non-byte-view calls return -1 (2374 arm). Writes
/// happen after the promise resolves (the C3-proven shape).
#[test]
fn c12_connect_sock_write_byte_and_error_arms() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__srvGot = "";
        globalThis.__retNoarg = 0;
        globalThis.__retBad = 0;
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            socket: {
                open: function () {},
                data: function (sock, data) { globalThis.__srvGot = data; },
                close: function () {},
            },
        });
        Bun.connect({
            hostname: "127.0.0.1",
            port: server.port,
            socket: {
                open: function () {},
                close: function () {},
            },
        }).then(function (sock) {
            globalThis.__retNoarg = sock.write();
            globalThis.__retBad = sock.write(42);
            sock.write(new Uint8Array([67, 49, 50])); // "C12"
        });
        "setup-ok"
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let got = wait_until(
        &mut ctx,
        r#"globalThis.__srvGot.length > 0 ? "y" : "n""#,
        400,
    );
    assert!(got, "server must receive the client payload");
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__srvGot"),
        "C12",
        "binary payload arrives byte-exact"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "globalThis.__retNoarg + '/' + globalThis.__retBad"
        ),
        "-1/-1",
        "no-arg and non-byte-view writes return -1"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 10);
}

/// Bun.connect options accept a double port (1196-1197 clamp arm) and fall
/// back to TOP-LEVEL open/end callbacks when the socket sub-object exists
/// but lacks those handlers (1265-1266 fallback extraction arms).
#[test]
fn c12_connect_port_double_and_top_level_callback_fallback() {
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__topOpen = 0;
        globalThis.__topData = "";
        var server = Bun.listen({
            port: 0,
            hostname: "127.0.0.1",
            socket: {
                open: function () {},
                data: function (sock, data) { globalThis.__topData = data; },
                close: function () {},
            },
        });
        Bun.connect({
            hostname: "127.0.0.1",
            port: server.port + 0.5,   // double port arm: clamps to server.port
            socket: { close: function () {} },   // no open here → top-level fallback
            open: function (sock) { globalThis.__topOpen++; sock.write("top-level"); },
        });
        String(server.port)
        "#,
    );
    let bound: u16 = setup.parse().expect("bound port");
    assert!(bound > 0, "server bound");

    let got = wait_until(
        &mut ctx,
        r#"globalThis.__topData.length > 0 ? "y" : "n""#,
        400,
    );
    assert!(got, "top-level open fallback must have fired and written");
    assert_eq!(eval_str(&mut ctx, "globalThis.__topData"), "top-level");
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__topOpen)"),
        "1",
        "open fired exactly once"
    );
    eval_str(&mut ctx, "server.stop(); true");
    pump(&mut ctx, 10);
}
