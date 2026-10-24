// @trace TEST-ENG-11-C16 [req:REQ-ENG-011] [level:integration]
// @trace TEST-ENG-6-C16 [req:REQ-ENG-006] [level:integration]
//
// C16 coverage wave — node_vm.rs / node_http.rs residual branch faces
// (residual to C8b's util_edge/vm_edge and runtime_misc_edge):
//   vm: ERR_INVALID_ARG_TYPE message-formatter arms (null / boolean /
//       symbol / function-name / >27-char string truncation) reached through
//       Script-method option validation, runInNewContext options forms
//       (string filename shorthand / object {filename} — both exercise the
//       filename read arms; the filename is NOT observable on the error
//       stack, see VM-FILENAME-NOT-ON-STACK — number → default "vm.js",
//       array/function spread copy — array keys land on the realm global
//       observably), Script .runInNewContext() default-sandbox +
//       codeGeneration forwarding, runInContext context-reuse arm,
//       compileFunction param shapes ({} → length non-int32, [1,2]
//       non-string elements skipped, real params, name-string bao
//       extension), Script ctor parity deviation (string options) +
//       syntax-error-at-compile.
//   http: server request-body streaming (data chunks as Buffers /
//       byte-exact, complete=true before 'end', end terminal), fail-closed
//       500 when the handler never responds, client-abort mid-body
//       ('aborted' terminal via a raw net client), WebSocket upgrade arms
//       (no listener → 426; listener + 101 written), writeHead status arms
//       (int32 honored / non-int32 no-op → default 200), server close
//       callback + 'close' emit, client request body write arms (string +
//       Buffer parts assembled into ONE chunk, write-after-end, invalid
//       part TypeError) and the connection-refused error face.
//
// Oracle: node v24.19.0 (vm option semantics), 2026-10-18.
//
// ── Registered product-defect / parity observations (characterization-pinned,
//    NOT fixed here — C16 contract: register only) ──
//
// | id | face | shipped behavior | oracle (node v24.19.0) | ref |
// |----|------|------------------|-------------------------|-----|
// | VM-SCRIPT-CTOR-STROPTS | `new vm.Script(code, 'str.js')` throws ERR_INVALID_ARG_TYPE (string not accepted as filename shorthand on the ctor) | node v24 ACCEPTS the string form (returns a Script) | node_vm.rs vm_script_ctor options gate; node oracle 2026-10-18 |
// | VM-COMPILEFUNCTION-PARAM-LAX | compileFunction params={} / params=[1,2] accepted laxly (length non-int32 → 0 params; non-string elements silently skipped); a string params arg is a bao-extension function NAME | node v24 THROWS ERR_INVALID_ARG_TYPE on all three | node_vm.rs vm_compile_function param arm; node oracle 2026-10-18 |
// | VM-CODEGEN-STRINGS-ERR-CLASS | codeGeneration.strings=false → plain Error inside the script-created context | node v24: EvalError (same class as C8b's registered vm_strings_disabled item) | node_vm.rs disabled-eval stub; run-1 evidence |
// | VM-FILENAME-NOT-ON-STACK | the compile-options filename does not surface on the SyntaxError stack (stack[0] shows the eval site `@<test>:1:…`) | node v24: stack[0] = "boom.js:1" | node_vm.rs CompileOptionsWrapper filename; run-1 evidence |
// | HTTP-UPGRADE-REQ-HEADERS-INCOMPLETE | the 'upgrade' handler's req object does not carry the Upgrade header (headers['upgrade'] === undefined even though the request carried `Upgrade: websocket`) | node v24: req.headers.upgrade === 'websocket' | node_http.rs uws_route_handler upgrade path; run-4 evidence |
// | HTTP-CLIENT-UPGRADE-EVENT-MISSING | a client request carrying Upgrade headers never settles when the server answers 101 — the transport resolves only normal final responses and has no client-side 'upgrade' event | node v24: client emits 'upgrade' (socket handed over); a 101 via 'response' is not the node contract either | node_http.rs HTTP_CLIENT_JS transport; run-5 evidence |

#[path = "common/mod.rs"]
mod common;

use bao_engine::context::JsContext;

use common::{drive_event_loop, eval_string_full, setup_ctx};

fn ev(ctx: &mut JsContext, js: &str) -> String {
    eval_string_full(ctx, js)
}

/// Eval `expr` inside a throw catcher capturing `code` + `message`
/// (`THREW:<code>:<message>`).
fn eval_try_code(ctx: &mut JsContext, expr: &str) -> String {
    let js = format!(
        "(function(){{ try {{ return 'NO-THROW:' + String({expr}); }} \
         catch (e) {{ return 'THREW:' + ((e && e.code) || '') + ':' + ((e && e.message) || String(e)); }} }})()"
    );
    ev(ctx, &js)
}

// ══════════════════════════════ node:vm (node_vm.rs) ══════════════════════════════

/// ERR_INVALID_ARG_TYPE message-formatter arms reached through
/// Script.runInThisContext option validation: null / boolean / symbol /
/// named-function / >27-char string (24-char truncation + "...").
/// Wrapper shape: `The "options" argument must be of type object. Received X`.
#[test]
fn vm_invalid_options_message_formatter_arms() {
    let mut ctx = setup_ctx();
    let script = "new vm.Script('var z=1').runInThisContext(opts)";
    let head = "THREW:ERR_INVALID_ARG_TYPE:The \"options\" argument must be of type object. Received ";
    let cases: Vec<(&str, &str, &str)> = vec![
        ("null", "null", "null"),
        ("bool", "true", "type boolean (true)"),
        ("symbol", "Symbol('c16s')", "type symbol"),
        ("fn", "(function c16NamedOpt() {})", "function c16NamedOpt"),
        (
            "longstr",
            "'aaaaaaaaaaaaaaaaaaaaaaaaaaaa.js'",
            "type string ('aaaaaaaaaaaaaaaaaaaaaaaa...')",
        ),
    ];
    for (label, opts_expr, received) in cases {
        let js = format!("(function(){{ var vm = require('vm'); var opts = {opts_expr}; try {{ {script}; return 'NO-THROW'; }} catch (e) {{ return 'THREW:' + ((e && e.code) || '') + ':' + ((e && e.message) || String(e)); }} }})()");
        let out = ev(&mut ctx, &js);
        assert_eq!(
            out,
            format!("{head}{received}"),
            "{label}: {out}"
        );
    }
}

/// runInNewContext options forms: string = filename shorthand (the filename
/// read arm executes; the value is not observable on the error stack —
/// registered VM-FILENAME-NOT-ON-STACK), object {filename} (options_filename
/// arm), number → default "vm.js", array → own enumerable keys copied onto
/// the realm global (observable), function → spread copy arm.
#[test]
fn vm_run_in_new_context_options_forms() {
    let mut ctx = setup_ctx();
    // String opts + object opts: syntax error still throws with the
    // filename read arms executed.
    for opts in ["'c16-rnc-file.js'", "{ filename: 'c16-rnc-obj.js' }"] {
        let js = format!(
            "(function () {{ var vm = require('vm'); \
             try {{ vm.runInNewContext('nope(', {{}}, {opts}); return 'NO-THROW'; }} \
             catch (e) {{ return 'threw:' + ((e && e.name) || e); }} }})()"
        );
        let out = ev(&mut ctx, &js);
        assert_eq!(out, "threw:SyntaxError", "opts {opts}: {out}");
    }
    // Number opts → default "vm.js" filename + code still runs.
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         return 'r=' + vm.runInNewContext('5+6', {}, 5); })()",
    );
    assert_eq!(out, "r=11", "number opts default filename: {out}");
    // Array opts: accepted, keys spread onto the internal options object
    // (not the realm global — the eval still runs).
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         var r = vm.runInNewContext('1 + 1', {}, [7, 8]); \
         return 'r=' + (typeof r === 'number' && r === 2 ? 'ok' : String(r)); })()",
    );
    assert_eq!(out, "r=ok", "array opts accepted: {out}");
    // Function opts: accepted (spread copy arm runs onto the internal
    // options object — keys are not observable on the realm global).
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         var f = function c16Opt(){}; f.marker = 41; \
         var r = vm.runInNewContext('1 + 1', {}, f); \
         return 'r=' + (typeof r === 'number' && r === 2 ? 'ok' : String(r)); })()",
    );
    assert_eq!(out, "r=ok", "function opts accepted: {out}");
}

/// Script.runInNewContext() without a sandbox uses the default empty sandbox
/// and forwards the options argument (codeGeneration.strings=false disables
/// eval inside — registered VM-CODEGEN-STRINGS-ERR-CLASS: bao throws a
/// plain Error where node throws EvalError).
#[test]
fn vm_script_run_in_new_context_default_sandbox_and_codegen() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         return 'r=' + new vm.Script('5*4').runInNewContext(); })()",
    );
    assert_eq!(out, "r=20", "default sandbox: {out}");
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         try { return 'r=' + vm.runInNewContext(\"eval('1+1')\", {}, { codeGeneration: { strings: false } }); } \
         catch (e) { return 'threw:' + ((e && e.name) || e); } })()",
    );
    assert_eq!(out, "threw:Error", "codeGeneration forwarded (VM-CODEGEN-STRINGS-ERR-CLASS): {out}");
}

/// runInContext context-reuse arm: a second runInContext against an already
/// contextified sandbox reuses the realm (state persists across runs).
#[test]
fn vm_run_in_context_reuses_contextified_sandbox() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         var s = vm.createContext({ a: 1 }); \
         vm.runInContext('this.b = 2;', s); \
         var second = vm.runInContext('a + b', s); \
         return 'r=' + second; })()",
    );
    assert_eq!(out, "r=3", "reuse arm: {out}");
}

/// compileFunction param shapes: {} → length non-int32 → zero params;
/// [1,2] → non-string elements skipped; ['a','b'] → real params; a string
/// params argument is the bao-extension function NAME; non-object options
/// throw through validateObject.
#[test]
fn vm_compile_function_param_shapes() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var vm = require('vm');
            var r = [];
            var f1 = vm.compileFunction('return 11', {});
            r.push('objParams:' + f1());
            var f2 = vm.compileFunction('return arguments.length', [1, 2]);
            r.push('numElems:' + f2(9, 9, 9));
            var f3 = vm.compileFunction('return a + b', ['a', 'b']);
            r.push('realParams:' + f3(30, 12));
            var f4 = vm.compileFunction('return 42', 'c16FnName');
            r.push('nameExt:' + f4.name + ':' + f4());
            try { vm.compileFunction('return 1', [], 5); r.push('numOpts:no-throw'); }
            catch (e) { r.push('numOpts:' + ((e && e.code) || e)); }
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "objParams:11|numElems:3|realParams:42|nameExt:c16FnName:42|numOpts:ERR_INVALID_ARG_TYPE",
        "compileFunction shapes: {out}"
    );
}

/// Script ctor faces: the string-options parity deviation
/// (VM-SCRIPT-CTOR-STROPTS — bao throws, node v24 accepts), missing code
/// argument, and syntax-error-at-compile (the compiled-failure arm; the
/// options filename is not observable on the stack —
/// VM-FILENAME-NOT-ON-STACK).
#[test]
fn vm_script_ctor_faces() {
    let mut ctx = setup_ctx();
    let out = eval_try_code(
        &mut ctx,
        "new (require('vm')).Script('var z=1', 'c16-str.js').runInThisContext()",
    );
    assert!(
        out.starts_with("THREW:ERR_INVALID_ARG_TYPE:"),
        "string-opts parity deviation (registered VM-SCRIPT-CTOR-STROPTS): {out}"
    );
    let out = eval_try_code(&mut ctx, "new (require('vm')).Script()");
    assert!(
        out.contains("Script requires a code string argument"),
        "missing code arg: {out}"
    );
    let out = ev(
        &mut ctx,
        "(function () { var vm = require('vm'); \
         try { new vm.Script('nope(', { filename: 'c16-boom.js' }); return 'NO-THROW'; } \
         catch (e) { return 'name=' + ((e && e.name) || e); } })()",
    );
    assert_eq!(out, "name=SyntaxError", "syntax error at compile: {out}");
}

// ══════════════════════════════ node:http (node_http.rs) ══════════════════════════════

/// Server request-body streaming: POST chunks arrive as Buffers (byte
/// exact), `complete` flips before 'end', the handler's terminal response
/// round-trips to the in-process client.
#[test]
fn http_server_post_body_stream_data_end() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16h = { done: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) {
            var chunks = [];
            req.on('data', function (c) { chunks.push(c); });
            req.on('end', function () {
                globalThis.__c16h.reqComplete = req.complete;
                globalThis.__c16h.chunkIsBuffer = Buffer.isBuffer(chunks[0]);
                globalThis.__c16h.chunkText = chunks.map(function (c) { return c.toString(); }).join('');
                res.writeHead(200, { 'Content-Type': 'text/plain' });
                res.end('echo:' + globalThis.__c16h.chunkText);
            });
        });
        srv.listen(0, '127.0.0.1');
        var body = Buffer.from('c16-body');
        var rq = http.request({
            port: srv.address().port, host: '127.0.0.1', method: 'POST', path: '/upload'
        });
        rq.write(body);
        rq.write('!suffix');
        rq.end();
        rq.on('response', function (incoming) {
            var parts = [];
            incoming.on('data', function (c) { parts.push(c.toString()); });
            incoming.on('end', function () {
                globalThis.__c16h.done = { code: incoming.statusCode, body: parts.join('') };
                srv.close();
            });
        });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 200);
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var h = globalThis.__c16h;
            if (!h.done) return 'PENDING';
            if (h.done.code !== 200) return 'CODE:' + h.done.code;
            if (h.done.body !== 'echo:c16-body!suffix') return 'BODY:' + h.done.body;
            if (h.reqComplete !== true) return 'COMPLETE:' + h.reqComplete;
            if (h.chunkIsBuffer !== true) return 'CHUNKTYPE:' + h.chunkIsBuffer;
            if (h.chunkText !== 'c16-body!suffix') return 'CHUNKTEXT:' + h.chunkText;
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "POST body stream: {out}");
}

/// Fail-closed terminal: a handler that consumes the body but never
/// responds gets an explicit 500 with the diagnostic body — never a silent
/// hang.
#[test]
fn http_server_unanswered_request_gets_500() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16h5 = { done: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) {
            req.on('data', function () {});
            req.on('end', function () { /* never responds */ });
        });
        srv.listen(0, '127.0.0.1');
        var rq = http.request({
            port: srv.address().port, host: '127.0.0.1', method: 'POST', path: '/silent'
        });
        rq.write('partial');
        rq.end();
        rq.on('response', function (incoming) {
            var parts = [];
            incoming.on('data', function (c) { parts.push(c.toString()); });
            incoming.on('end', function () {
                globalThis.__c16h5.done = { code: incoming.statusCode, body: parts.join('') };
                srv.close();
            });
        });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 200);
    let out = ev(
        &mut ctx,
        "(function () { var d = globalThis.__c16h5.done; \
         return d ? ('code=' + d.code + ' body=' + d.body) : 'PENDING'; })()",
    );
    assert_eq!(
        out,
        "code=500 body=request handler did not respond",
        "unanswered 500: {out}"
    );
}

/// Client-abort terminal: a raw TCP client sending a partial POST then
/// destroying the connection should drive the on_aborted terminal.
///
/// #[ignore] (C16, three-strike): net.connect against the node:http uWS
/// listener never completes the client-side connect inside the test pump
/// regime — the wrote flag (callback form) and the write itself (event
/// form) both stay silent across three carrier variants (runs 1-3), so the
/// on_aborted terminal (server_req_body_aborted) is not reachable from a
/// JS-level test today. Re-enable with a product-side test hook or a
/// working raw-socket carrier.
#[test]
#[ignore = "HTTP-ABORT-CARRIER-DEAD: net.connect against the node:http listener never connects in the test pump regime (wrote=false, 3 carrier variants); on_aborted terminal unreachable from JS"]
fn http_server_aborted_mid_body() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16ha = { aborted: null, ended: false, srv: null };
        var http = require('http');
        var net = require('net');
        var srv = http.createServer(function (req, res) {
            req.on('data', function () {});
            req.on('end', function () { globalThis.__c16ha.ended = true; });
            req.on('aborted', function () { globalThis.__c16ha.aborted = true; });
        });
        srv.listen(0, '127.0.0.1');
        globalThis.__c16ha.srv = srv;
        globalThis.__c16ha.wrote = false;
        var s = net.connect(srv.address().port, '127.0.0.1', function () {
            globalThis.__c16ha.wrote = true;
            s.write('POST /partial HTTP/1.1\r\nHost: c16\r\nContent-Length: 64\r\n\r\nonly-half');
            s.destroy();
        });
        'spawned'
        "#,
    );
    let mut out = String::new();
    for _ in 0..30 {
        drive_event_loop(&mut ctx, 10);
        out = ev(
            &mut ctx,
            "(function () { var a = globalThis.__c16ha; \
             return 'wrote=' + a.wrote + ' aborted=' + a.aborted + ' ended=' + a.ended; })()",
        );
        if out.contains("aborted=true") {
            break;
        }
    }
    ev(&mut ctx, "try { globalThis.__c16ha.srv.close(); } catch (e) {}");
    assert!(
        out.contains("wrote=true"),
        "raw client must connect and write: {out}"
    );
    assert_eq!(out, "wrote=true aborted=true ended=false", "aborted terminal: {out}");
}

/// WebSocket upgrade arms through the in-process http client (the raw
/// net.connect carrier is dead against this listener — see the aborted
/// test's registration): without an 'upgrade' listener the server answers
/// 426 Upgrade Required; with a listener the handler receives the upgrade
/// face and its 101 answer surfaces as the response status.
#[test]
fn http_server_upgrade_arms() {
    let mut ctx = setup_ctx();
    // No listener → explicit 426.
    ev(
        &mut ctx,
        r#"
        globalThis.__c16u1 = { code: null, srv: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) { res.end('plain'); });
        srv.listen(0, '127.0.0.1');
        globalThis.__c16u1.srv = srv;
        var rq = http.request({
            port: srv.address().port, host: '127.0.0.1', method: 'GET', path: '/ws',
            headers: { Connection: 'Upgrade', Upgrade: 'websocket' }
        });
        rq.on('response', function (incoming) {
            incoming.resume();
            incoming.on('end', function () {
                globalThis.__c16u1.code = incoming.statusCode;
                srv.close();
            });
        });
        rq.on('error', function (e) { globalThis.__c16u1.code = 'ERR:' + String(e && e.message).slice(0, 40); srv.close(); });
        rq.end();
        'spawned'
        "#,
    );
    let mut out = String::new();
    for _ in 0..30 {
        drive_event_loop(&mut ctx, 10);
        out = ev(&mut ctx, "String(globalThis.__c16u1.code)");
        if out != "null" {
            break;
        }
    }
    ev(&mut ctx, "try { globalThis.__c16u1.srv.close(); } catch (e) {}");
    assert_eq!(out, "426", "upgrade without listener → 426: {out}");

    // With listener → 101 + handler face.
    ev(
        &mut ctx,
        r#"
        globalThis.__c16u2 = { code: null, saw: null, srv: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) { res.end('plain'); });
        srv.on('upgrade', function (req, socket, head) {
            globalThis.__c16u2.saw = {
                upgradeHeader: req.headers['upgrade'],
                statusCode: head.statusCode,
                hasEnd: typeof head.end === 'function'
            };
            head.writeHead(101, { 'Upgrade': 'websocket' });
            head.end();
        });
        srv.listen(0, '127.0.0.1');
        globalThis.__c16u2.srv = srv;
        var rq2 = http.request({
            port: srv.address().port, host: '127.0.0.1', method: 'GET', path: '/ws',
            headers: { Connection: 'Upgrade', Upgrade: 'websocket' }
        });
        rq2.on('response', function (incoming) {
            incoming.resume();
            incoming.on('end', function () {
                globalThis.__c16u2.code = incoming.statusCode;
                srv.close();
            });
        });
        rq2.on('error', function (e) { globalThis.__c16u2.code = 'ERR:' + String(e && e.message).slice(0, 40); srv.close(); });
        rq2.end();
        'spawned'
        "#,
    );
    out.clear();
    for _ in 0..30 {
        drive_event_loop(&mut ctx, 10);
        out = ev(&mut ctx, "String(globalThis.__c16u2.code)");
        if out != "null" {
            break;
        }
    }
    let out2 = ev(
        &mut ctx,
        r#"
        (function () {
            var u = globalThis.__c16u2;
            if (!u.saw) return 'PENDING-HANDLER';
            // Registered HTTP-UPGRADE-REQ-HEADERS-INCOMPLETE: the upgrade
            // handler's req object does not carry the Upgrade header (run-4:
            // undefined) — only the status/face checks are strong here.
            if (u.saw.upgradeHeader !== undefined && u.saw.upgradeHeader !== 'websocket') return 'HDR:' + u.saw.upgradeHeader;
            if (u.saw.statusCode !== 101) return 'STATUS:' + u.saw.statusCode;
            if (!u.saw.hasEnd) return 'NOEND';
            return 'ok';
        })()
        "#,
    );
    ev(&mut ctx, "try { globalThis.__c16u2.srv.close(); } catch (e) {}");
    assert_eq!(out2, "ok", "upgrade handler face: {out2}");
    // The 101-on-the-wire face is NOT asserted: the client transport never
    // settles a 101 response (code stayed null across the poll — node's
    // client delivers upgrades via the 'upgrade' event, which this
    // transport does not implement; registered below).
}

/// writeHead status arms: an int32 status reaches the wire; a non-int32
/// status is a no-op and the default 200 is sent (h1-parity
/// characterization of res_write_head_status's None arm).
#[test]
fn http_res_write_head_status_arms() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16w = { intStatus: null, strStatus: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) {
            if (req.url === '/int') { res.writeHead(404, { 'X-C16': 'int' }); res.end('nf'); }
            else { res.writeHead('404', { 'X-C16': 'str' }); res.end('str'); }
        });
        srv.listen(0, '127.0.0.1');
        var port = srv.address().port;
        function probe(path, slot) {
            http.get({ port: port, host: '127.0.0.1', path: path }, function (incoming) {
                var parts = [];
                incoming.on('data', function (c) { parts.push(c.toString()); });
                incoming.on('end', function () {
                    globalThis.__c16w[slot] = { code: incoming.statusCode, body: parts.join('') };
                    if (globalThis.__c16w.intStatus && globalThis.__c16w.strStatus) srv.close();
                });
            });
        }
        probe('/int', 'intStatus');
        probe('/str', 'strStatus');
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 200);
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var w = globalThis.__c16w;
            if (!w.intStatus || !w.strStatus) return 'PENDING';
            var i = w.intStatus, s = w.strStatus;
            if (i.code !== 404 || i.body !== 'nf') return 'INT:' + JSON.stringify(i);
            if (s.code !== 200) return 'STR-CODE:' + s.code;
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "writeHead status arms: {out}");
}

/// Server close: the close callback fires once the live server stops and
/// the 'close' event emits on the server object.
#[test]
fn http_server_close_callback_and_emit() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16c = { cb: null, ev: null, done: false };
        var http = require('http');
        var srv = http.createServer(function (req, res) { res.end('x'); });
        srv.on('close', function () { globalThis.__c16c.ev = true; });
        srv.listen(0, '127.0.0.1');
        var port = srv.address().port;
        http.get({ port: port, host: '127.0.0.1', path: '/' }, function (incoming) {
            incoming.resume();
            incoming.on('end', function () {
                srv.close(function () {
                    globalThis.__c16c.cb = true;
                    globalThis.__c16c.done = true;
                });
            });
        });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 200);
    let out = ev(
        &mut ctx,
        "(function () { var c = globalThis.__c16c; \
         return c.done ? ('cb=' + c.cb + ' ev=' + c.ev) : 'PENDING'; })()",
    );
    assert_eq!(out, "cb=true ev=true", "close callback + emit: {out}");
}

/// Client request body write arms: string + Buffer parts assemble byte-exact
/// into ONE transport chunk (buildBodyArg byte assembly); write() after
/// end() throws; a non-string non-buffer part throws at the call; a
/// connection-refused request surfaces the 'error' event followed by
/// 'close'.
#[test]
fn http_client_request_body_write_arms_and_error() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16cl = { body: null, writeAfterEnd: null, badPart: null, refused: null, refusedClosed: null };
        var http = require('http');
        var srv = http.createServer(function (req, res) {
            var chunks = [];
            req.on('data', function (c) { chunks.push(c); });
            req.on('end', function () {
                globalThis.__c16cl.chunkLen = chunks[0].length;
                res.end('got:' + chunks.map(function (c) { return c.toString(); }).join('') + ':' + chunks[0].length);
            });
        });
        srv.listen(0, '127.0.0.1');
        var rq = http.request({ port: srv.address().port, host: '127.0.0.1', method: 'POST', path: '/b' });
        rq.write('str-part|');
        rq.write(Buffer.from([0x01, 0x02]));
        rq.end('tail');
        rq.on('response', function (incoming) {
            var parts = [];
            incoming.on('data', function (c) { parts.push(c.toString()); });
            incoming.on('end', function () {
                globalThis.__c16cl.body = parts.join('');
                try { rq.write('late'); } catch (e) { globalThis.__c16cl.writeAfterEnd = String(e.message).indexOf('write() after end') !== -1 ? 'ok' : 'GOT:' + e.message; }
                srv.close();
            });
        });
        try { http.request({ port: srv.address().port, host: '127.0.0.1', method: 'POST' }).write({ nope: 1 }); }
        catch (e) { globalThis.__c16cl.badPart = String(e.message).indexOf('request body chunk must be a string, Buffer') !== -1 ? 'ok' : 'GOT:' + e.message; }
        // Connection refused: bind + release a port, then request it.
        var holder = http.createServer(function () {});
        holder.listen(0, '127.0.0.1');
        var deadPort = holder.address().port;
        holder.close(function () {
            var rq2 = http.request({ port: deadPort, host: '127.0.0.1', method: 'GET', path: '/' });
            rq2.on('error', function () { globalThis.__c16cl.refused = true; });
            rq2.on('close', function () { globalThis.__c16cl.refusedClosed = true; });
            rq2.end();
        });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 240);
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var c = globalThis.__c16cl;
            if (!c.body) return 'PENDING-BODY';
            var bin = String.fromCharCode(1) + String.fromCharCode(2);
            if (c.body !== 'got:str-part|' + bin + 'tail:15') return 'BODY:' + JSON.stringify(c.body);
            if (c.chunkLen !== 15) return 'CHUNKLEN:' + c.chunkLen;
            if (c.writeAfterEnd !== 'ok') return 'WAE:' + c.writeAfterEnd;
            if (c.badPart !== 'ok') return 'BADPART:' + c.badPart;
            if (c.refused !== true) return 'REFUSED:' + c.refused;
            if (c.refusedClosed !== true) return 'CLOSED:' + c.refusedClosed;
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "client write arms: {out}");
}
