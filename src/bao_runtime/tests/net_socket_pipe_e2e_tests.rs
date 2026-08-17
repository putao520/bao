// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:e2e]
// net.Socket Duplex face — .pipe() on server-accepted connection sockets.
// Rooted from the loud-gap audit: echo/write/on were REAL but pipe was
// missing, so the canonical TCP→stream bridge (socket.pipe(file sink),
// socket.pipe(transform), source.pipe(socket)) was undeclarable.
//
// The pipe implementation itself is REUSED from node:stream's
// Readable.prototype.pipe (node_net NET_JS grafts it onto Socket); these
// tests prove the full bridge over REAL TCP roundtrips:
//   1. file → createReadStream.pipe(client socket) → TCP → server
//      sock.pipe(decode Transform).pipe(fs createWriteStream) — BOTH pipe
//      directions on sockets plus stream composition, byte-compared.
//   2. sock.pipe(upper).pipe(sock) — in-connection transform echo.
//   3. pause()/resume() — REAL backpressure: native RX buffering while the
//      poll chain is halted, no byte loss across the pause window.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — the production pump path (same as
/// net_echo_e2e_tests): the tail loop must run inside the AutoRealm so
/// timer callbacks dispatch with the realm entered.
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

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..60 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

fn settle(ctx: &mut JsContext, budget: usize) {
    HOOK_BUDGET.with(|b| b.set(budget));
    let _ = eval_str(ctx, "'settle'");
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Both pipe directions over one TCP connection: the CLIENT socket is the
/// pipe DESTINATION (fs.createReadStream.pipe(client)) and the SERVER
/// socket is the pipe SOURCE (sock.pipe(decode).pipe(fs write stream)).
/// The decoded file must equal the source file byte-for-byte.
#[test]
fn net_socket_pipe_file_roundtrip_both_directions() {
    let mut ctx = setup_ctx();
    let dir = std::env::temp_dir().join(format!("bao-net-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let in_path = dir.join("in.txt");
    let out_path = dir.join("out.txt");

    // ASCII payload: byte-exact through BOTH pipe directions. (utf8 multi-
    // byte fidelity through the SOCKET pipe chain is pinned by the transform
    // echo test below; fs.readFileSync(path,'utf8') has an unrelated
    // pre-existing mojibake defect — JS_NewStringCopyZ in node_fs
    // return_string_content — reported separately, out of this task's file
    // scope.)
    let payload = "pipe-roundtrip-payload / second segment 0123456789 with tail markers <>[]{}&^%$#@! end";
    std::fs::write(&in_path, payload).unwrap();

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
        var net = require('net');
        var fs = require('fs');
        var stream = require('stream');
        var log = [];
        globalThis.__done = false;

        var server = net.createServer(function(sock) {{
          // objectMode decode Transform: 'data' delivers Buffers; the fs write
          // stream sink is string-oriented, so decode hands it utf8 strings
          // (objectMode keeps push(chunk) a string instead of re-encoding to
          // a Buffer).
          var decode = new stream.Transform({{
            objectMode: true,
            transform: function(c, e, cb) {{
              cb(null, Buffer.isBuffer(c) ? c.toString('utf8') : String(c));
            }}
          }});
          var ws = fs.createWriteStream({out_path:?});
          sock.pipe(decode).pipe(ws);
          ws.on('finish', function() {{
            log.push('ws_finished');
            sock.end();
            server.close(function() {{ globalThis.__done = true; }});
          }});
          ws.on('error', function(e) {{ log.push('ws_error:' + e.message); }});
        }});

        server.listen(0, '127.0.0.1', function() {{
          var client = net.connect(server.address().port, '127.0.0.1', function() {{
            log.push('connected');
          }});
          // Socket as pipe DESTINATION: readable file stream → socket.
          fs.createReadStream({in_path:?}, {{ encoding: 'utf8' }}).pipe(client);
          client.on('error', function(e) {{ log.push('client_error:' + e.message); }});
        }});
        globalThis.__log = function() {{ return log.join('|'); }};
        'setup-ok'
    "#
        ),
    );
    assert_eq!(setup, "setup-ok", "pipe file roundtrip wiring must eval cleanly");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let diag = eval_str(&mut ctx, "globalThis.__log ? globalThis.__log() : '(no log)'");
    assert!(done, "pipe file roundtrip must complete; log: {diag}");

    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(
        log.contains("connected") && log.contains("ws_finished"),
        "pipe flow must reach the sink, got: {log}"
    );
    assert!(
        !log.contains("error"),
        "no error events expected in the roundtrip, got: {log}"
    );

    let written = std::fs::read_to_string(&out_path)
        .unwrap_or_else(|e| panic!("output file must exist after ws finish: {e}"));
    assert_eq!(
        written, payload,
        "sink file must equal source file byte-for-byte through BOTH pipe directions"
    );

    settle(&mut ctx, 30);
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "teardown must drop the net liveness token"
    );
}

/// In-connection transform echo: sock.pipe(upper).pipe(sock) — the canonical
/// self-duplex server shape. The transform's stdout flows back through the
/// SAME socket the input arrived on. The payload mixes ASCII with multi-byte
/// UTF-8 (你好世界) — the socket pipe chain (net_write string encode →
/// buffer_to_string utf8 decode) must round-trip the multi-byte characters
/// byte-faithfully.
#[test]
fn net_socket_pipe_transform_echo_same_socket() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        var stream = require('stream');
        var log = [];
        globalThis.__done = false;

        var server = net.createServer(function(sock) {
          var upper = new stream.Transform({
            transform: function(c, e, cb) {
              cb(null, (Buffer.isBuffer(c) ? c.toString('utf8') : String(c)).toUpperCase());
            }
          });
          sock.pipe(upper).pipe(sock);
        });

        server.listen(0, '127.0.0.1', function() {
          var client = net.connect(server.address().port, '127.0.0.1', function() {
            client.write('hello pipe duplex face 你好世界');
          });
          client.on('data', function(d) {
            log.push('echo=' + (Buffer.isBuffer(d) ? d.toString('utf8') : String(d)));
            client.end();
          });
          client.on('close', function() {
            server.close(function() { globalThis.__done = true; });
          });
        });
        globalThis.__log = function() { return log.join('|'); };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok", "transform echo wiring must eval cleanly");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let diag = eval_str(&mut ctx, "globalThis.__log ? globalThis.__log() : '(no log)'");
    assert!(done, "transform echo must complete; log: {diag}");

    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert_eq!(
        log,
        "echo=HELLO PIPE DUPLEX FACE \u{4f60}\u{597d}\u{4e16}\u{754c}",
        "transform echo must uppercase ASCII and preserve utf8 multi-byte through sock.pipe(upper).pipe(sock), got: {log}"
    );

    settle(&mut ctx, 30);
    assert!(!bun_runtime::node_http::has_active_servers());
}

/// REAL backpressure: pause() halts the poll chain so RX bytes buffer
/// natively; a payload landing while paused is NOT delivered until
/// resume() — and nothing is lost across the pause window.
#[test]
fn net_socket_pause_resume_no_loss() {
    let mut ctx = setup_ctx();
    let setup = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        var log = [];
        var received = '';
        globalThis.__done = false;

        var server = net.createServer(function(sock) {
          var pausedOnce = false;
          sock.on('data', function(d) {
            received += Buffer.isBuffer(d) ? d.toString('utf8') : String(d);
            log.push('got:' + (Buffer.isBuffer(d) ? d.toString('utf8') : String(d)));
            if (!pausedOnce) {
              pausedOnce = true;
              sock.pause();
              log.push('paused;isPaused=' + sock.isPaused());
              // Handshake: part2 is only written after the client sees this
              // ack — i.e. strictly AFTER the server paused — so part2 must
              // sit in the native RX buffer across the pause window (no race
              // with the server's own poll tick).
              sock.write('ack1');
              setTimeout(function() {
                sock.resume();
                log.push('resumed;isPaused=' + sock.isPaused());
              }, 60);
            }
          });
          sock.on('end', function() { sock.end(); });
        });

        server.listen(0, '127.0.0.1', function() {
          var client = net.connect(server.address().port, '127.0.0.1', function() {
            client.write('part-one-');
          });
          client.on('data', function() {
            // Fires on the ack — write part2 into the paused server.
            setTimeout(function() { client.write('part-two-'); }, 5);
            setTimeout(function() { client.end(); }, 120);
          });
        });

        var watcher = setInterval(function() {
          if (received === 'part-one-part-two-') {
            clearInterval(watcher);
            server.close(function() { globalThis.__done = true; });
          }
        }, 0);
        globalThis.__log = function() { return log.join('|') + '#received=' + received; };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok", "pause/resume wiring must eval cleanly");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 80);
    let diag = eval_str(&mut ctx, "globalThis.__log ? globalThis.__log() : '(no log)'");
    assert!(done, "pause/resume roundtrip must complete; log: {diag}");

    let log = eval_str(&mut ctx, "globalThis.__log()");
    // Part2 (written at +5ms) must not be delivered before resume (+30ms).
    let part2_idx = log.find("part-two-").unwrap_or_else(|| panic!("part-two must be received, log: {log}"));
    let resume_idx = log.find("resumed;isPaused=false").unwrap_or_else(|| panic!("resume must be logged, log: {log}"));
    assert!(
        resume_idx < part2_idx,
        "part-two must be held in the native buffer until resume(): resume@{resume_idx} must precede part-two@{part2_idx}, log: {log}"
    );
    assert!(
        log.contains("paused;isPaused=true"),
        "isPaused() must report true after pause(), log: {log}"
    );
    assert!(
        log.ends_with("#received=part-one-part-two-"),
        "no bytes may be lost across the pause window, log: {log}"
    );

    settle(&mut ctx, 30);
    assert!(!bun_runtime::node_http::has_active_servers());
}
