// @trace TEST-BAO-API-017/018 [req:REQ-BAO-API-017,REQ-BAO-API-018] [level:integration]
// P0 surface regressions (v-surface audit wave):
//
// 1. Bun.$ — template AND direct-array invocations SIGSEGV'd in
//    ShellOutput::to_js_object: JS_NewStringCopyZ on a Rust String's
//    `as_ptr()` reads past the allocation for a NUL terminator, and for an
//    EMPTY output the Cow borrows the empty Vec's DANGLING pointer (0x1) —
//    strlen(0x1) → SIGSEGV in __strlen_avx2. Every command with empty
//    stdout OR empty stderr crashed (i.e. nearly all of them).
// 2. Bun.resolve — same JS_NewStringCopyZ class returned the resolved path
//    followed by heap bytes past the buffer ('/tmp/./x.js' + 0xDE×12).
//    Also asserts upstream lexical normalization ('./' and '..' removed).
// 3. Bun.listen/Bun.connect — client `open` fired with NO socket identity,
//    so `open(sock) { sock.write(..) }` threw a TypeError that
//    invoke_js_callback silently cleared: the client never sent a byte and
//    the server's data events never fired. The client identity bridge now
//    mirrors the listen-side #21 accept bridge (open/data/close/end all
//    carry the socket object).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;
use std::time::Duration;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

fn pump(ctx: &mut JsContext, passes: usize) {
    for _ in 0..passes {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

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

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ──────────────────── Item 1: Bun.$ ────────────────────

/// Both invocation forms (tagged template + direct array), nested quote
/// escaping, interleaved expressions, and the two empty-output paths that
/// used to SIGSEGV (empty stderr on success, empty stdout on failure).
#[test]
fn bun_dollar_template_array_and_escaping() {
    let mut ctx = fresh_ctx();

    // Tagged template — canonical form; echo emits EMPTY stderr, the exact
    // dangling-pointer arm that crashed before the fix.
    let out = eval_str(
        &mut ctx,
        r#"
        var out = Bun.$`echo dollar-template-ok`;
        (out.success === true && out.exitCode === 0
            && out.text() === "dollar-template-ok\n"
            && out.stderr === "")
            ? "tpl-ok" : "tpl-FAIL:" + out.exitCode + "/" + JSON.stringify(out.text())
    "#,
    );
    assert_eq!(out, "tpl-ok", "tagged-template Bun.$ echo");

    // Direct array call — elements are separate shell words.
    let arr = eval_str(
        &mut ctx,
        r#"
        var a = Bun.$(["echo", "array-form-ok"]);
        (a.success === true && a.text() === "array-form-ok\n")
            ? "arr-ok" : "arr-FAIL:" + a.exitCode + "/" + JSON.stringify(a.text())
    "#,
    );
    assert_eq!(arr, "arr-ok", "direct-array Bun.$ word semantics");

    // Nested quote escaping through the shell parser.
    let esc = eval_str(
        &mut ctx,
        r#"
        var e = Bun.$`echo "he said 'hi' twice"`;
        (e.success === true && e.text() === "he said 'hi' twice\n")
            ? "esc-ok" : "esc-FAIL:" + JSON.stringify(e.text())
    "#,
    );
    assert_eq!(esc, "esc-ok", "nested single/double quote escaping");

    // Interleaved ${expr} values.
    let expr = eval_str(
        &mut ctx,
        r#"
        var x = "expr";
        var i = Bun.$`echo ${x}-value`;
        (i.success === true && i.text() === "expr-value\n")
            ? "expr-ok" : "expr-FAIL:" + JSON.stringify(i.text())
    "#,
    );
    assert_eq!(expr, "expr-ok", "interleaved template expression");

    // Failing command — non-empty stderr AND EMPTY stdout (the other crash
    // arm). Exit code is nonzero; both strings are exact.
    let fail = eval_str(
        &mut ctx,
        r#"
        var f = Bun.$(["definitely-not-a-command-xyz-42"]);
        (f.success === false && f.exitCode !== 0 && f.stdout === ""
            && typeof f.stderr === "string" && f.stderr.length > 0)
            ? "fail-ok" : "fail-FAIL:" + f.exitCode + "/" + JSON.stringify(f.stdout)
    "#,
    );
    assert_eq!(fail, "fail-ok", "failing command: empty stdout + real stderr");

    // lines() over multi-line stdout (per-line strings were also built from
    // non-NUL-terminated &str slices). \\n stays a literal backslash-n in the
    // command so printf's own escape produces the real newlines.
    let lines = eval_str(
        &mut ctx,
        r#"
        var l = Bun.$`printf 'one\\ntwo\\nthree'`;
        var ls = l.lines();
        (l.success === true && ls.length === 3 && ls[0] === "one" && ls[2] === "three")
            ? "lines-ok" : "lines-FAIL:" + JSON.stringify(ls)
    "#,
    );
    assert_eq!(lines, "lines-ok", "ShellOutput.lines() byte-exact per line");

    // Metacharacters inside quoted literals must survive reconstruction
    // (unquoted re-assembly used to re-expose '|' as a shell pipe → 127).
    let meta = eval_str(
        &mut ctx,
        r#"
        var m = Bun.$(['printf', '%s|', "a|b;c"]);
        (m.success === true && m.exitCode === 0 && m.text() === "a|b;c|")
            ? "meta-ok" : "meta-FAIL:" + m.exitCode + "/" + JSON.stringify(m.text())
    "#,
    );
    assert_eq!(meta, "meta-ok", "quoted metacharacters survive sh re-execution");
}

// ──────────────────── Item 2: Bun.resolve ────────────────────

/// Returned strings must be byte-exact: exact equality plus a printable-ASCII
/// guard so any post-buffer heap bytes (the 0xDE×12 signature) fail loudly.
#[test]
fn bun_resolve_relative_paths_byte_exact() {
    let mut ctx = fresh_ctx();

    let out = eval_str(
        &mut ctx,
        r#"
        var results = [];
        function check(label, got, want) {
            var ok = got === want && got.length === want.length;
            for (var i = 0; i < got.length; i++) {
                var c = got.charCodeAt(i);
                if (c < 0x20 && c !== 0x0a) { ok = false; }
            }
            results.push(label + "=" + ok + (ok ? "" : " got:" + JSON.stringify(got) + " want:" + JSON.stringify(want)));
        }
        // Relative './' against an explicit nonexistent target — the exact
        // v-surface repro (used to return '/tmp/./x.js' + 12 dirty bytes).
        check("dot-rel", Bun.resolve("./x.js", "/tmp"), "/tmp/x.js");
        // '..' is lexically resolved.
        check("dotdot", Bun.resolve("./sub/../y.js", "/tmp/bun_rs_probe"), "/tmp/bun_rs_probe/y.js");
        // Absolute passthrough.
        check("abs", Bun.resolve("/etc"), "/etc");
        // Deep nesting.
        check("deep", Bun.resolve("./a/./b/../c.js", "/tmp/bun_rs_probe"),
              "/tmp/bun_rs_probe/a/c.js");
        results.join(";")
    "#,
    );
    assert!(
        out.split(";").all(|p| p.ends_with("=true")),
        "Bun.resolve must return byte-exact normalized paths, got: {out}"
    );
}

// ──────────────────── Item 3: Bun.listen/connect data flow ────────────────────

/// The exact v-surface repro shape: the CLIENT writes from its `open`
/// callback (previously received NO socket → TypeError silently cleared →
/// zero bytes ever sent → server data never fired). Full loop: client
/// open→write, server data→echo, client data(socket, data)→end, both closes.
#[test]
fn bun_connect_open_identity_drives_data_flow() {
    let mut ctx = fresh_ctx();
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
                open: function(sock) { log.push("srv_open=" + (sock && typeof sock.write === "function")); },
                data: function(sock, data) {
                    log.push("srv_data=" + data);
                    sock.write(data); // echo back
                },
                close: function(sock) { log.push("srv_close=" + (sock !== undefined)); },
                end: function() {},
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1",
            port: server.port,
            socket: {
                open: function(sock) {
                    log.push("cli_open=" + (sock && typeof sock.write === "function"
                        && typeof sock.end === "function"));
                    sock.write("ping-from-open"); // THE repro: write from open
                },
                data: function(sock, data) {
                    log.push("cli_data=" + data + "/sock=" + (sock && typeof sock.write === "function"));
                    sock.end();
                },
                close: function(sock) {
                    log.push("cli_close=" + (sock !== undefined));
                    server.stop();
                    globalThis.__done = true;
                },
                end: function() {},
            },
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok port=" + server.port
    "#,
    );
    assert!(setup.starts_with("setup-ok"), "listen/connect setup: {setup}");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(
        done,
        "open→write→echo→data→close flow must complete; log: {log}"
    );

    for part in [
        "srv_open=true",
        "cli_open=true",
        "srv_data=ping-from-open",
        "cli_data=ping-from-open/sock=true",
        "cli_close=true",
        "srv_close=true",
    ] {
        assert!(
            log.split('|').any(|entry| entry == part),
            "data-flow log must contain '{part}', got: {log}"
        );
    }
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "server.stop() must drop the liveness token after the roundtrip"
    );
}

/// The resolved-promise socket and the open-callback socket must expose the
/// SAME identity (single GcStore registration), so state attached in open()
/// is visible on the resolved object.
#[test]
fn bun_connect_promise_and_open_share_identity() {
    let mut ctx = fresh_ctx();
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
                data: function(sock, data) { sock.end(); },
                close: function() {},
                end: function() {},
                open: function() {},
            },
        });
        var conn = Bun.connect({
            hostname: "127.0.0.1",
            port: server.port,
            socket: {
                open: function(sock) {
                    sock.__tagged = "from-open";
                    sock.write("x");
                },
                data: function(sock, data) {
                    log.push("tag-on-data=" + sock.__tagged);
                },
                close: function(sock) {
                    server.stop();
                    globalThis.__done = true;
                },
                end: function() {},
            },
        });
        conn.then(function(sock) {
            log.push("tag-on-promise=" + sock.__tagged);
        });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "identity-sharing roundtrip must complete; log: {log}");
    assert!(
        log.split('|').any(|e| e == "tag-on-promise=from-open"),
        "resolved promise socket must be the SAME object open() tagged, got: {log}"
    );
}

/// Refused connect (ECONNREFUSED): error callback fires, the promise rejects,
/// and the never-opened socket is TERMINATED in the on_connect_error handler
/// (uSockets hands close responsibility there for the single-address connect
/// fast path — leaving it registered is a level-triggered EPOLLERR that
/// re-fires every loop pass, spinning the CPU; same class as HTTPContext's
/// on_connect_error fix). The test settling with an idle loop is the
/// anti-spin proof.
#[test]
fn bun_connect_refused_rejects_and_terminates_socket() {
    // Port 1 on loopback: nothing listens (binding it needs root), the
    // kernel answers SYN with RST immediately — a DETERMINISTIC ECONNREFUSED.
    // (A bind-then-drop ephemeral port is racy on this box: the fleet's
    // parallel test binaries reuse the released port within the window and
    // the connect unexpectedly SUCCEEDS.)
    let port: u16 = 1;

    let mut ctx = fresh_ctx();
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__done = false;
        var conn = Bun.connect({
            hostname: "127.0.0.1", port: PORT,
            socket: {
                error: function(code) { log.push("error=" + (code !== undefined)); },
                open: function() { log.push("UNEXPECTED-OPEN"); },
                close: function() {},
            },
        });
        conn.then(function(s) { log.push("UNEXPECTED-RESOLVE"); },
                  function(e) { log.push("rejected=" + (e instanceof Error)); globalThis.__done = true; });
        globalThis.__log = function() { return log.join("|"); };
        "setup-ok"
    "#
    .replace("PORT", &port.to_string())
    .as_str(),
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 60);
    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(done, "refused connect must reject the promise; log: {log}");
    assert!(
        log.split('|').any(|e| e == "rejected=true"),
        "promise must reject with an Error, got: {log}"
    );
    assert!(
        log.split('|').any(|e| e == "error=true"),
        "error callback must fire with a code, got: {log}"
    );
    assert!(
        !log.contains("UNEXPECTED"),
        "no open/resolve may fire on a refused connect, got: {log}"
    );
    // Anti-spin: the refused socket was terminated, no liveness token leaks,
    // the drain loop can go idle again.
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "refused-connect socket must not keep the loop alive"
    );
}
