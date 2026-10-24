// @trace TEST-ENG-6-WS-EDGE [req:REQ-ENG-006] [level:integration]
//
// Coverage-shortfall tests for web_api.rs WebSocket face (REQ-ENG-006): the
// real wire paths the shape-only probes never reach —
//   - WsConn::connect (plain ws://) + RFC 6455 client handshake against a real
//     in-process TcpListener server (compute_accept via bun_uws ws_handshake)
//   - send_text / read_message opcode arms: Text, Binary, masked server frame
//     (client-side unmask arm), Ping→pong echo, unsolicited Pong skip
//   (bare Continuation / reserved opcodes are protocol errors — the adjacent
//    `ws_reserved_opcode_tests` carrier pins that fail-the-connection face)
//   - the drain pump dispatch family: ws_connect_dispatch (open + close-
//     requested-while-connecting + connect-refused), ws_message_dispatch
//     (text + binary), ws_closed_dispatch (clean close handshake)
//   - constructor/ws_send/ws_close_fn error surfaces (no-arg, non-string,
//     send-while-CONNECTING InvalidStateError, send-after-closed, close-
//     while-CONNECTING), ws_has_pending liveness
//
// The server is REAL std TcpListener code — no socket mock. Frame encode/
// decode on the server side uses the same bun_uws::ws_codec primitives the
// product uses, so every byte on the loopback socket is RFC 6455 wire truth.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bun_uws::ws_codec::{FrameDecoder, FrameEncoder, Opcode};
use bun_uws::ws_handshake::compute_accept;

#[path = "common/mod.rs"]
mod common;

use common::eval_string;

/// Drive the JS thread until `probe` satisfies `check` (RunJobs + timer tick
/// + the WebSocket drain pump each pass). Mirrors the drive helper the
/// subtle string-algorithm suite uses, with an explicit ws pump for
/// determinism.
fn drive_until(
    ctx: &mut JsContext,
    probe: &str,
    check: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        let raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            bun_runtime::web_api::ws_pump_all(raw);
        }
        std::thread::sleep(Duration::from_millis(2));
        last = eval_string(ctx, probe);
        if check(&last) {
            return last;
        }
    }
    last
}

/// Complete the RFC 6455 server-side upgrade: read the client request and
/// answer 101 with the computed accept. Returns false if the peer vanished.
fn accept_ws_handshake(stream: &mut TcpStream) -> bool {
    // ── 1. read the client handshake request ──
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
        compute_accept(&key)
    );
    stream.write_all(response.as_bytes()).expect("write 101");
    true
}

/// One RFC 6455 server connection: handshake, then a fixed probe-frame
/// script (masked text / binary / ping / unsolicited pong), then an echo
/// loop until the client closes.
fn serve_connection(mut stream: TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .ok();
    if !accept_ws_handshake(&mut stream) {
        return;
    }

    // ── 2. probe-frame script (server→client) ──
    let mut enc = FrameEncoder::new();
    // Masked server frame: exercises the client decoder's mask arm
    // (take_mask + apply_mask) — servers never send masked frames per RFC,
    // but the product decoder accepts them, so the arm is wire-reachable.
    let masked = enc.encode_frame(Opcode::Text, b"masked-text", Some([0x12, 0x34, 0x56, 0x78]));
    stream.write_all(masked).expect("masked text");
    // Unmasked binary frame → onmessage with a Uint8Array.
    let bin = enc.encode_binary(&[0x01, 0xff, 0x00, 0x80]);
    stream.write_all(bin).expect("binary");
    // Ping → the client must echo a pong (read_message Ping arm) and keep
    // reading (the arm recurses into itself).
    let ping = enc.encode_frame(Opcode::Ping, b"pin", None);
    stream.write_all(ping).expect("ping");
    // Unsolicited pong → skipped silently (Pong arm).
    let pong = enc.encode_frame(Opcode::Pong, b"p", None);
    stream.write_all(pong).expect("pong");

    // ── 3. echo loop: text frames come back as "echo:<payload>" ──
    let mut dec = FrameDecoder::new();
    loop {
        let header = match dec.decode_frame(&mut stream) {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(_) => return,
        };
        let mut payload = if header.mask {
            // Mask key bytes sit before the payload on the wire — take_mask
            // MUST precede take_payload (same order as the product's
            // read_message).
            let key = dec.take_mask();
            let mut p = dec.take_payload(&header);
            bun_uws::ws_codec::apply_mask(&mut p, &key);
            p
        } else {
            dec.take_payload(&header)
        };
        match header.opcode {
            Opcode::Text => {
                let echoed = format!("echo:{}", String::from_utf8_lossy(&payload));
                let frame = enc.encode_text(&echoed);
                if stream.write_all(frame).is_err() {
                    return;
                }
            }
            Opcode::Ping => {
                let frame = enc.encode_frame(Opcode::Pong, &payload, None);
                if stream.write_all(frame).is_err() {
                    return;
                }
            }
            Opcode::Close => {
                let frame = enc.encode_close(1000, "");
                let _ = stream.write_all(frame);
                return;
            }
            _ => {}
        }
    }
}

/// Start the echo server; returns its port. Connections are served on
/// detached threads (nextest gives each test its own process, so the
/// listener dies with the test).
fn start_ws_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    std::thread::spawn(move || serve_connection(stream));
                }
                Err(_) => break,
            }
        }
    });
    port
}

/// Full wire path: connect → onopen → server probe frames (masked text,
/// binary, ping/pong/continuation silent) → client sends → echo replies →
/// close handshake → onclose. Every dispatched event is recorded by the page
/// and asserted byte-exactly against the wire script above.
#[test]
fn ws_echo_server_full_message_and_close_handshake() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let port = start_ws_echo_server();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
        globalThis.__wslog = [];
        globalThis.__wsdone = false;
        globalThis.__wsopen = false;
        globalThis.__ws = new WebSocket('ws://127.0.0.1:{port}/echo');
        __ws.onopen = function () {{
            __wsopen = true;
            this.send('hello');
            this.send('second');
        }};
        __ws.onmessage = function (e) {{
            var v = e.data;
            if (typeof v === 'string') {{
                __wslog.push('t:' + v);
            }} else {{
                var a = new Uint8Array(v);
                var s = '';
                for (var i = 0; i < a.length; i++) s += a[i].toString(16) + ',';
                __wslog.push('b:' + s);
            }}
            if (__wslog.length >= 4 && !__ws.__closing) {{
                __ws.__closing = true;
                this.close();
            }}
        }};
        __ws.onerror = function () {{ __wslog.push('unexpected-error'); }};
        __ws.onclose = function () {{ __wsdone = true; }};
        'setup-ok';
        "#,
            port = port
        ),
    );
    assert_eq!(setup, "setup-ok");

    // While the background connect is in flight the connection registers as
    // pending (liveness contract consumed by the event loop). The first pump
    // pass may already have completed the loopback connect, so the check is
    // "pending now OR already open" — both prove the registry is live.
    assert!(
        bun_runtime::web_api::ws_has_pending()
            || eval_string(&mut ctx, "String(__wsopen)") == "true",
        "a connecting WebSocket must register as pending"
    );

    let final_state = drive_until(
        &mut ctx,
        "__wsdone + '|' + __wsopen + '|' + __wslog.join(';')",
        |s| s.starts_with("true|"),
        Duration::from_secs(15),
    );

    // The full recorded wire history must match the server script exactly:
    // masked text (unmasked by the client), binary bytes, then the two
    // echoes, in order — and no error event. (The shared read loop consumes
    // ping/pong inside the read; a bare continuation would be a protocol
    // error — pinned by ws_reserved_opcode_tests.)
    assert_eq!(
        final_state,
        "true|true|t:masked-text;b:1,ff,0,80,;t:echo:hello;t:echo:second",
        "WebSocket wire transcript diverged"
    );

    // After the close handshake the connection must be fully deregistered.
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "closed WebSocket must not keep the event loop pending"
    );
    assert_eq!(eval_string(&mut ctx, "String(__ws.readyState)"), "3");
}

/// Constructor + lifecycle error surfaces: no-arg / non-string constructor
/// throws, send() while CONNECTING throws InvalidStateError, close() while
/// CONNECTING takes the close-requested path (never onopen), a refused
/// connect surfaces as onerror(data)+onclose with no onopen, and send()
/// after the failure throws InvalidStateError (closed).
#[test]
fn ws_constructor_and_lifecycle_error_paths() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            function record(label, fn) {
                try { out.push(label + '=' + fn()); }
                catch (e) { out.push(label + '=THREW:' + (e && e.message || e)); }
            }

            // ── constructor argument validation ──
            record('no-arg', function () { var w = new WebSocket(); return 'constructed'; });
            record('non-string', function () { var w = new WebSocket(42); return 'constructed'; });

            // ── refused connect (loopback port 1 — ECONNREFUSED, instant) ──
            globalThis.__errlog = [];
            globalThis.__faildone = false;
            globalThis.__failopen = false;
            var ws = new WebSocket('ws://127.0.0.1:1/refused');
            record('initial-ready-state', function () { return String(ws.readyState); });
            record('send-while-connecting', function () { ws.send('x'); return 'sent'; });
            record('close-while-connecting', function () { ws.close(); return String(ws.readyState); });
            ws.onerror = function (e) { __errlog.push('err:' + String(e && e.data || '').substring(0, 40)); };
            ws.onopen = function () { __failopen = true; };
            ws.onclose = function () { __faildone = true; };
            globalThis.__failws = ws;

            globalThis.__out = out;
            return out.join(';;');
        })();
        "#,
    );
    assert_eq!(
        report,
        "no-arg=THREW:WebSocket requires a URL argument;;\
         non-string=THREW:WebSocket URL must be a string;;\
         initial-ready-state=0;;\
         send-while-connecting=THREW:WebSocket send failed: InvalidStateError: WebSocket is still connecting;;\
         close-while-connecting=2",
        "constructor/lifecycle error surfaces diverged"
    );

    // The close-requested + refused connect must resolve without onopen.
    let final_state = drive_until(
        &mut ctx,
        "__faildone + '|' + __failopen + '|' + __errlog.join(';')",
        |s| s.starts_with("true|"),
        Duration::from_secs(15),
    );
    assert!(
        final_state.starts_with("true|false|err:"),
        "refused connect must fire onerror(data) + onclose, never onopen: {}",
        final_state
    );
    assert!(
        !final_state.contains("unexpected-error"),
        "the refused-connection socket must not leak error events: {}",
        final_state
    );
    assert_eq!(eval_string(&mut ctx, "String(__failws.readyState)"), "3");

    // Post-failure send(): the entry is dead — browser parity InvalidStateError.
    let late_send = eval_string(
        &mut ctx,
        r#"
        (function () {
            try { __failws.send('late'); return 'sent'; }
            catch (e) { return 'THREW:' + (e && e.message || e); }
        })();
        "#,
    );
    assert_eq!(
        late_send,
        "THREW:WebSocket send failed: InvalidStateError: WebSocket is already closed"
    );

    // send() with no argument: the arity check precedes the state check.
    // The socket is driven to its refused-close so the test ends with a
    // fully drained registry.
    let no_arg = eval_string(
        &mut ctx,
        r#"
        (function () {
            globalThis.__noargdone = false;
            var w = new WebSocket('ws://127.0.0.1:1/no-arg-send');
            w.onclose = function () { globalThis.__noargdone = true; };
            try { w.send(); w.close(); return 'sent'; }
            catch (e) { w.close(); return 'THREW:' + (e && e.message || e); }
        })();
        "#,
    );
    assert_eq!(no_arg, "THREW:WebSocket.send() requires a message argument");
    let drained = drive_until(
        &mut ctx,
        "String(__noargdone)",
        |s| s == "true",
        Duration::from_secs(15),
    );
    assert_eq!(drained, "true");
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "all sockets drained — registry must be empty"
    );
}

/// ws_send argument conversion (REQ-BRW-012 C3): node v24.19.0 oracle — a
/// non-string send() argument is stringified per ES ToString and transmitted
/// as a text frame (123 → "123", null → "null", {toString} → that result);
/// a throwing conversion (Symbol) propagates as a JS exception. The old
/// unconditional `msg_val.to_string()` bit-cast was UB for non-strings — this
/// carrier proves the value round-trips on the real wire instead of crashing.
#[test]
fn ws_send_non_string_arguments_convert_per_es_tostring() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_sink = seen.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut stream = match listener.incoming().next() {
            Some(Ok(s)) => s,
            _ => return,
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok();
        if !accept_ws_handshake(&mut stream) {
            return;
        }
        let mut dec = FrameDecoder::new();
        loop {
            let header = match dec.decode_frame(&mut stream) {
                Ok(Some(h)) => h,
                _ => return,
            };
            let payload = if header.mask {
                let key = dec.take_mask();
                let mut p = dec.take_payload(&header);
                bun_uws::ws_codec::apply_mask(&mut p, &key);
                p
            } else {
                dec.take_payload(&header)
            };
            match header.opcode {
                Opcode::Text => {
                    seen_sink
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&payload).into_owned());
                }
                Opcode::Close => {
                    let _ = stream.write_all(&[0x88, 0x02, 0x03, 0xE8]);
                    return;
                }
                _ => {}
            }
        }
    });

    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    let setup = eval_string(
        &mut ctx,
        &format!(
            r#"
        globalThis.__sendlog = [];
        globalThis.__senddone = false;
        var ws = new WebSocket('ws://127.0.0.1:{port}/tostring');
        ws.onopen = function () {{
            ws.send(123);
            ws.send(null);
            ws.send({{ toString: function () {{ return 'obj-str'; }} }});
            ws.send(true);
            // Symbol has no primitive string form: the ToString conversion
            // throws synchronously and nothing is sent.
            try {{ ws.send(Symbol('boom')); __sendlog.push('symbol=NO-THROW'); }}
            catch (e) {{ __sendlog.push('symbol=THREW:' + e.name); }}
            ws.close();
        }};
        ws.onclose = function () {{ __senddone = true; }};
        'setup-ok';
        "#,
            port = port
        ),
    );
    assert_eq!(setup, "setup-ok");

    let done = drive_until(
        &mut ctx,
        "String(__senddone)",
        |s| s == "true",
        Duration::from_secs(15),
    );
    assert_eq!(done, "true", "send-conversion socket must close cleanly");

    // ES ToString semantics on the wire, exactly as the node oracle measured.
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["123", "null", "obj-str", "true"],
        "non-string send() arguments must convert per ES ToString"
    );
    // A throwing conversion surfaces as a JS exception, not a crash and not
    // a silent drop.
    assert_eq!(
        eval_string(&mut ctx, "__sendlog.join('|')"),
        "symbol=THREW:TypeError",
        "Symbol send() must throw TypeError synchronously"
    );
    assert!(
        !bun_runtime::web_api::ws_has_pending(),
        "conversion socket must deregister after close"
    );
}
