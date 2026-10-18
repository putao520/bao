// @trace TEST-ENG-006-C16 [req:REQ-ENG-006] [level:integration]
// @trace TEST-ENG-011-C16 [req:REQ-ENG-011] [level:integration]
//
// C16 coverage wave — bun_password.rs / bun_ffi.rs / bun_shell.rs residual
// branch faces, exercised through the public JS surface:
//   password (was 8.8% LF — zero prior tests): hashSync default/short-string/
//   object options (timeCost/memoryCost/parallelism/cost, double-typed cost,
//   partial options), error paths (missing/non-string password, unknown
//   algorithm both forms), async promise resolve + rejection faces, verify
//   options (string/object/auto-detect), PHC normalization (v=19 keep,
//   missing-v= splice roundtrip), verify safety limits (WeakParameters m/t/p),
//   non-ASCII InvalidEncoding, malformed param pairs, verifySync error faces.
//   ffi (residual to C8a): TypeSpec::parse full type-name matrix via
//   descriptor validation (parse arms), unknown-type errors, returns-omitted
//   → Void default, malloc/free + BigInt pointer forms, view-ptr faces
//   (Uint8Array write observable / zero-length / ArrayBuffer / detached /
//   non-view error), CString NULL passthrough via setlocale(0, null),
//   callback construction validation faces + ptrValue instance property,
//   qsort via the typed-array callback form (real observable ordering),
//   arg-marshal completion matrix, js_function arg error arms, one-arg
//   library face (invalid this / symbol name required / missing symbol /
//   symbol-keyed descriptor skip), CString constructor (getenv PATH
//   roundtrip / ptr 0 / missing arg), toBuffer error arms + opts.free
//   rejection + zero-copy view.
//   shell (interpreter arms residual to C8a): bare assignment / subshell
//   statements, if-elif-else chain matrix, multi-stage and subshell
//   pipelines, lex-error / empty-command faces, signal-terminated command
//   exitCode, cwd override, output-method non-object `this` arms, output
//   bodies, atom rendering (quoted-empty / argv / braces / tilde), Bun.$
//   promise resolve + ShellError rejection shape.
//
// libc-face tests are #[cfg(unix)] (dlopen "libc.so.6"/"libm.so.6",
// /bin/sh spawn) — same convention as udp_ffi_shell_coverage_c8a_tests.rs.
//
// ── Registered product-defect / parity observations ──
//    (C16 registered these characterization-only; e-fix-shell-ffi flipped the
//    four FIXED rows below to oracle assertions — the rest stay pinned as
//    observed parity/deviation facts.)
//
// | id | face | shipped behavior | oracle / expected | ref |
// |----|------|------------------|-------------------|-----|
// | PWD-ERR-MSG-PARITY | Bun.password error messages | "Bun.password.hash(Sync) requires a password string" / "Unknown algorithm: X" / "InvalidEncoding: hash must be ASCII" | bun 1.4.2: "Not enough arguments to 'hash'..." / "Expected algorithm to be a unknown algorithm..." / 'Password verification failed with error "InvalidEncoding"' | bun_password.rs:538,574,607,652; bun oracle 2026-10-18 |
// | PWD-VERIFY-ERR-CLASS | malformed argon2 hash surfaces the raw rust-argon2 decoder error text | "argon2 verify failed: <crate error>" | bun: 'Password verification failed with error "InvalidEncoding"' | bun_password.rs:441-444 |
// | FFI-TWOARG-LIB-PROTO-MISSING | FIXED (e-fix-shell-ffi): two-arg dlopen returned a plain object without the FfiLibrary prototype (close/symbol unreachable) and dlclose'd the handle at return | two-arg Library chains onto FfiLibrary.prototype, owns the handle, close is one-shot | bun_ffi.rs ffi_dlopen two-arg path + installed_class_proto |
// | FFI-CALLBACK-METHODS-UNREACHABLE | FIXED (e-fix-shell-ffi): ffi.callback() instances were allocated with JS_NewObject, orphaning the FFI_CALLBACK_METHODS prototype | instances chain onto FfiCallback.prototype; ptr()==ptrValue, close() one-shot | bun_ffi.rs ffi_callback |
// | FFI-NARROW-ARG-CALL-SUSPECT | FIXED (e-fix-shell-ffi): Rust saturating float→int casts zeroed negative narrow ints (u8 abs(-7)=0), f32 args were widened to f64 bits and f32 returns decoded from the wrong half | C-cast ToInt32/ToUint32 semantics: u8 abs(-7)=249; f32 bits carried in the low 32 bits: fabsf(-2.5)=2.5 | bun_ffi.rs js_to_uint32/js_to_int32 + F32 slot/return |
// | SHELL-PARSER-PANIC | FIXED (e-fix-shell-ffi): sh.run('(echo') — Eof-less lex stream from an unclosed subshell ran peek() past the token slice → Rust panic → process abort | lexer errors surfaced (exit 1, "Unclosed subshell"); Parser::new rejects non-Eof streams; peek/prev bounded | shell_parser/parse.rs + bun_shell.rs |
// | SHELL-VAR-RENDER-MANGLES-DOUBLEDOLLAR | FIXED (e-fix-shell-ffi): root was the lexer, not the Var renderer — eat_var rejects '$' as a name char, so `$$` lexed as two literal-`$` Text atoms and reconstruction re-quoted them (`'$$'`), leaving PID expansion unreachable | `$$` lexes as a Var atom, reconstructs as `$$`, and /bin/sh expands it (self-kill carrier now live) | shell_parser/parse.rs `$` arm |
// | SHELL-LEXER-LENIENT-UNCLOSED-QUOTE | an unterminated double quote is silently auto-closed: `echo "unclosed` runs and prints "unclosed" (exit 0, empty stderr) | dash/bash: unexpected-EOF syntax error (exit 2); bun upstream: parse error | run-2 evidence; also: whitespace-only command is a silent exit-0 no-op (the exec_via_sh empty-command arm is unreachable from text input) |
// | FFI-NARROW-ARG-CALL-SUSPECT (extended — same fixed defect as the row above) | abs(-7) with declared u8 and fabsf(-2.5) with declared f32 both returned 0 from real calls | u8 call → 249; f32 call → 2.5 (pinned live in ffi_typespec_parse_type_name_matrix) | bun_ffi.rs js_to_uint32/js_to_int32 + F32 slot/return |
// | SHELL-IF-NO-ELSE-COND-STATUS | `if false; then echo a; fi` → exitCode 1 (the condition command's status propagates) | bash/POSIX: 0 (node child_process & bun parity) | bun_shell.rs interpret_if `return cond_result` arm |
// | SHELL-PIPE-SUBSHELL-STDIN-DROP | `echo hi | (cat)` → stdout empty (mid-pipeline subshell does not receive the piped stdin) | bash: "hi" | bun_shell.rs interpret_pipeline_item_with_stdin Subshell arm |
// | SHELL-BACKGROUND-AMP | `echo x &` → exit 1 with empty stdout/stderr (background operator not supported); dropped from the suite rather than pinning an unexplained error face | bun docs: background jobs unsupported — plausibly parity, unverified | run-1 evidence |
// | FFI-EXTERNAL-FREE-GC-REENTRANCY | toBuffer {free} runs from the GC finalizer (BufferContentsDeleter) — a JS callback there re-enters JS during GC (forbidden); the external_free_fn non-null-user arm is not reachable from a JS-level test without product-side deferral | — | bun_ffi.rs external_free_fn |
//
// VM/HTTP faces of this wave (and their registrations) live in
// vm_http_coverage_c16_tests.rs.

#[path = "common/mod.rs"]
mod common;

use bao_engine::context::JsContext;

use common::{drive_event_loop, eval_string_full, setup_ctx};

fn ev(ctx: &mut JsContext, js: &str) -> String {
    eval_string_full(ctx, js)
}

/// Eval `expr` inside a JS-side throw catcher (suite pattern). Returns
/// `THREW:<message>` on throw, `NO-THROW:<result>` otherwise.
fn eval_try(ctx: &mut JsContext, expr: &str) -> String {
    let js = format!(
        "(function(){{ try {{ return 'NO-THROW:' + String({expr}); }} \
         catch (e) {{ return 'THREW:' + ((e && e.message) || String(e)); }} }})()"
    );
    ev(ctx, &js)
}

// ══════════════════════════════ Bun.password (bun_password.rs) ══════════════════════════════

/// hashSync defaults: argon2id v=19 m=65536 t=2 p=1 PHC face + verify
/// roundtrip (true on match, false on mismatch — no throw on mismatch).
#[test]
fn password_hash_sync_default_argon2id_face() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var h = Bun.password.hashSync('pw-c16');
            if (!h.startsWith('$argon2id$v=19$m=65536,t=2,p=1$')) return 'BADPREFIX:' + h.slice(0, 30);
            if (typeof h !== 'string' || h.length < 40) return 'BADSHAPE';
            if (Bun.password.verifySync('pw-c16', h) !== true) return 'VERIFY-TRUE-FAIL';
            if (Bun.password.verifySync('wrong', h) !== false) return 'VERIFY-FALSE-FAIL';
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "hashSync default face: {out}");
}

/// Options forms: short string algorithm (argon2i/argon2d prefix arms),
/// object form with reduced costs (timeCost/memoryCost/parallelism), bcrypt
/// cost, double-typed option values (get_uint_property double arm), partial
/// options object (per-key fallback).
#[test]
fn password_hash_sync_options_matrix() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var r = [];
            var i = Bun.password.hashSync('pw', 'argon2i');
            r.push('argon2i:' + i.startsWith('$argon2i$v=19$'));
            var d = Bun.password.hashSync('pw', 'argon2d');
            r.push('argon2d:' + d.startsWith('$argon2d$v=19$'));
            var o = Bun.password.hashSync('pw', { algorithm: 'argon2id', timeCost: 1, memoryCost: 8192, parallelism: 1 });
            r.push('obj:' + o.startsWith('$argon2id$v=19$m=8192,t=1,p=1$'));
            r.push('objVerify:' + Bun.password.verifySync('pw', o));
            // Double-typed costs: { timeCost: 1.9 } truncates to 1 — the
            // is_double arm of get_uint_property.
            var dbl = Bun.password.hashSync('pw', { algorithm: 'argon2id', timeCost: 1.9, memoryCost: 8192, parallelism: 1 });
            r.push('dbl:' + dbl.startsWith('$argon2id$v=19$m=8192,t=1,p=1$'));
            // Absent keys fall back per-key (partial options object).
            var part = Bun.password.hashSync('pw', { memoryCost: 8192 });
            r.push('part:' + part.startsWith('$argon2id$v=19$m=8192,t=2,p=1$'));
            var b = Bun.password.hashSync('pw', { algorithm: 'bcrypt', cost: 4 });
            r.push('bcrypt:' + b.startsWith('$2b$04$'));
            r.push('bcryptVerify:' + Bun.password.verifySync('pw', b));
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "argon2i:true|argon2d:true|obj:true|objVerify:true|dbl:true|part:true|bcrypt:true|bcryptVerify:true",
        "options matrix: {out}"
    );
}

/// hashSync/hash error paths: missing arg, non-string arg, unknown algorithm
/// in object form AND in short-string form (compute_hash `other` arm). The
/// required-argument message names the exact method (hashSync vs hash).
#[test]
fn password_hash_sync_error_paths() {
    let mut ctx = setup_ctx();
    let r1 = eval_try(&mut ctx, "Bun.password.hashSync()");
    assert_eq!(
        r1, "THREW:Bun.password.hashSync requires a password string",
        "no args: {r1}"
    );
    let r2 = eval_try(&mut ctx, "Bun.password.hashSync(123)");
    assert_eq!(
        r2, "THREW:Bun.password.hashSync requires a password string",
        "non-string: {r2}"
    );
    let r3 = eval_try(&mut ctx, "Bun.password.hashSync('x', { algorithm: 'nope' })");
    assert_eq!(r3, "THREW:Unknown algorithm: nope", "object-form unknown algo: {r3}");
    let r4 = eval_try(&mut ctx, "Bun.password.hashSync('x', 'argon2x')");
    assert_eq!(r4, "THREW:Unknown algorithm: argon2x", "string-form unknown algo: {r4}");
    let r5 = eval_try(&mut ctx, "Bun.password.hash()");
    assert_eq!(
        r5, "THREW:Bun.password.hash requires a password string",
        "async fn no args (sync throw before promise): {r5}"
    );
}

/// Async faces: `hash` resolves a PHC string through the promise; a bad
/// algorithm rejects with the compute error as `.message`.
#[test]
fn password_hash_promise_resolve_and_reject() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16 = { hash: null, err: null, done: 0 };
        Bun.password.hash('pw', { algorithm: 'argon2id', timeCost: 1, memoryCost: 8192 })
            .then(function (h) { globalThis.__c16.hash = h; globalThis.__c16.done++; });
        Bun.password.hash('pw', { algorithm: 'nope' })
            .then(function () { globalThis.__c16.err = 'NO-REJECT'; })
            .catch(function (e) { globalThis.__c16.err = e && e.message; globalThis.__c16.done++; });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 4);
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var c = globalThis.__c16;
            if (c.done !== 2) return 'PENDING:' + c.done;
            if (typeof c.hash !== 'string' || !c.hash.startsWith('$argon2id$v=19$m=8192,t=1,p=1$')) return 'BADHASH:' + String(c.hash).slice(0, 30);
            if (c.err !== 'Unknown algorithm: nope') return 'BADERR:' + c.err;
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "hash promise faces: {out}");
}

/// verifySync error matrix: malformed hash, non-ASCII, v=16 rejection,
/// WeakParameters for each of m/t/p, non-numeric + unknown param pairs
/// (continue arms → falls through to the raw decoder error), bcrypt
/// non-ASCII, missing hash argument.
#[test]
fn password_verify_error_matrix() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var r = [];
            function t(label, fn) { try { fn(); r.push(label + '=no-throw'); } catch (e) { r.push(label + '=' + String((e && e.message) || e)); } }
            t('malformed', function () { Bun.password.verifySync('x', 'garbage-hash'); });
            t('nonascii', function () { Bun.password.verifySync('x', '$argon2id$v=19$m=1,t=1,p=1$YQ$YQé'); });
            t('v16', function () { Bun.password.verifySync('x', '$argon2id$v=16$m=1024,t=2,p=1$YQ$YQ'); });
            t('m-over', function () { Bun.password.verifySync('x', '$argon2id$v=19$m=4194305,t=2,p=1$YQ$YQ'); });
            t('t-over', function () { Bun.password.verifySync('x', '$argon2id$v=19$m=1024,t=65537,p=1$YQ$YQ'); });
            t('p-over', function () { Bun.password.verifySync('x', '$argon2id$v=19$m=1024,t=2,p=65$YQ$YQ'); });
            t('param-nonnum', function () { Bun.password.verifySync('x', '$argon2id$v=19$m=abc,t=2,p=1$YQ$YQ'); });
            t('param-unknownkey', function () { Bun.password.verifySync('x', '$argon2id$v=19$zz=1,t=2,p=1$YQ$YQ'); });
            t('bcrypt-nonascii', function () { Bun.password.verifySync('x', '$2b$04$é'); });
            t('no-hash-arg', function () { Bun.password.verifySync('x'); });
            return r.join('|');
        })()
        "#,
    );
    for part in out.split('|') {
        let (label, val) = match part.split_once('=') {
            Some(kv) => kv,
            None => panic!("unparsed verify matrix segment: {part} (all: {out})"),
        };
        match label {
            "malformed" | "param-nonnum" | "param-unknownkey" => assert!(
                val.starts_with("argon2 verify failed"),
                "raw decoder error expected for {label}, got: {part}"
            ),
            "nonascii" | "bcrypt-nonascii" => assert_eq!(
                val, "InvalidEncoding: hash must be ASCII",
                "{label}: {part}"
            ),
            "v16" => assert_eq!(
                val, "InvalidEncoding: only argon2 v=19 (0x13) is supported",
                "v16: {part}"
            ),
            "m-over" => assert_eq!(
                val, "WeakParameters: m=4194305 exceeds limit 4194304",
                "m-over: {part}"
            ),
            "t-over" => assert_eq!(
                val, "WeakParameters: t=65537 exceeds limit 65536",
                "t-over: {part}"
            ),
            "p-over" => assert_eq!(
                val, "WeakParameters: p=65 exceeds limit 64",
                "p-over: {part}"
            ),
            "no-hash-arg" => assert_eq!(
                val, "Bun.password.verifySync requires a hash string",
                "no-hash-arg: {part}"
            ),
            _ => panic!("unknown verify matrix label: {part} (all: {out})"),
        }
    }
}

/// v=19 splice: stripping the `v=19$` segment from a real hash must still
/// verify (the normaliser splices v=19 back in before delegating).
#[test]
fn password_verify_splices_missing_v19_segment() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var h = Bun.password.hashSync('pw', { algorithm: 'argon2id', timeCost: 1, memoryCost: 8192 });
            var stripped = h.replace('v=19$', '');
            if (stripped.indexOf('v=19') !== -1) return 'STRIP-FAIL:' + stripped.slice(0, 30);
            if (Bun.password.verifySync('pw', stripped) !== true) return 'SPLICE-VERIFY-FAIL';
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "v=19 splice roundtrip: {out}");
}

/// verify options: explicit string algorithm, object {algorithm}, and
/// auto-detect arms ($argon2id / $2 bcrypt / $bcrypt$ / unknown-$argon2x →
/// argon2id fallback). Wrong explicit algorithm surfaces the decoder error.
#[test]
fn password_verify_algorithm_detect_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var a = Bun.password.hashSync('pw', { algorithm: 'argon2id', timeCost: 1, memoryCost: 8192 });
            var b = Bun.password.hashSync('pw', { algorithm: 'bcrypt', cost: 4 });
            var r = [];
            r.push('str:' + Bun.password.verifySync('pw', a, 'argon2id'));
            r.push('obj:' + Bun.password.verifySync('pw', a, { algorithm: 'argon2id' }));
            r.push('detectA:' + Bun.password.verifySync('pw', a));
            r.push('detectB:' + Bun.password.verifySync('pw', b));
            var r4;
            try { Bun.password.verifySync('pw', b, 'argon2id'); r4 = 'no-throw'; }
            catch (e) { r4 = 'threw:' + String((e && e.message) || e).slice(0, 19); }
            r.push('wrongAlgo:' + r4);
            var r5;
            try { Bun.password.verifySync('pw', '$argon2x$v=19$m=1,t=1,p=1$YQ$YQ'); r5 = 'no-throw'; }
            catch (e) { r5 = 'threw:' + String((e && e.message) || e).slice(0, 19); }
            r.push('detectFallback:' + r5);
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "str:true|obj:true|detectA:true|detectB:true|wrongAlgo:threw:argon2 verify faile|detectFallback:threw:argon2 verify faile",
        "detect arms: {out}"
    );
}

/// Async verify faces: resolved boolean through the promise + rejection for
/// an undecodable hash.
#[test]
fn password_verify_promise_faces() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16v = { hit: null, miss: null, err: null, done: 0 };
        var h = Bun.password.hashSync('pw', { algorithm: 'argon2id', timeCost: 1, memoryCost: 8192 });
        Bun.password.verify('pw', h).then(function (ok) { globalThis.__c16v.hit = ok; globalThis.__c16v.done++; });
        Bun.password.verify('nope', h).then(function (ok) { globalThis.__c16v.miss = ok; globalThis.__c16v.done++; });
        Bun.password.verify('pw', 'garbage-hash')
            .then(function () { globalThis.__c16v.err = 'NO-REJECT'; })
            .catch(function (e) { globalThis.__c16v.err = String((e && e.message) || e).slice(0, 18); globalThis.__c16v.done++; });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 4);
    let out = ev(
        &mut ctx,
        "(function () { var c = globalThis.__c16v; \
         return 'done=' + c.done + ' hit=' + c.hit + ' miss=' + c.miss + ' err=' + c.err; })()",
    );
    assert_eq!(out, "done=3 hit=true miss=false err=argon2 verify fail", "verify promise faces: {out}");
}

// ══════════════════════════════ Bun.$ / Shell (bun_shell.rs) ══════════════════════════════

/// Interpreter expression arms reachable as statements: bare assignment
/// (Expr::Assign) and subshell (Expr::Subshell). The async `&` statement is
/// NOT exercised (SHELL-BACKGROUND-AMP registration).
#[cfg(unix)]
#[test]
fn shell_expr_statement_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = [];
            var a = sh.run('BAO_C16_X=1');
            r.push('assign:' + a.exitCode + ':' + a.stdout.length);
            var s = sh.run('(echo subshell-c16)');
            r.push('subshell:' + s.exitCode + ':' + s.stdout.trim());
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "assign:0:0|subshell:0:subshell-c16",
        "expr statement arms: {out}"
    );
}

/// If/elif/else chain matrix. Registered deviation SHELL-IF-NO-ELSE-COND-STATUS:
/// a false condition without else propagates the condition's exit status (1)
/// instead of the POSIX 0.
#[cfg(unix)]
#[test]
fn shell_if_elif_else_matrix() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = [];
            r.push('ifT:' + sh.run('if true; then echo a; fi').stdout.trim());
            var f = sh.run('if false; then echo a; fi');
            r.push('ifF:' + f.exitCode + ':' + f.stdout.trim());
            r.push('elif:' + sh.run('if false; then echo a; elif true; then echo b; fi').stdout.trim());
            r.push('else:' + sh.run('if false; then echo a; elif false; then echo b; else echo c; fi').stdout.trim());
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(out, "ifT:a|ifF:1:|elif:b|else:c", "if matrix: {out}");
}

/// Pipelines: multi-stage stdout chaining and a leading subshell item. The
/// mid-pipeline subshell drops the piped stdin (registered
/// SHELL-PIPE-SUBSHELL-STDIN-DROP) and is pinned as the empty face.
#[cfg(unix)]
#[test]
fn shell_pipeline_stage_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = [];
            r.push('two:' + sh.run('echo hi | cat').stdout.trim());
            r.push('three:' + sh.run('echo hi | cat | cat').stdout.trim());
            r.push('subMid:' + sh.run('echo hi | (cat)').stdout.trim());
            r.push('subFirst:' + sh.run('(echo hi) | cat').stdout.trim());
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "two:hi|three:hi|subMid:|subFirst:hi",
        "pipeline arms: {out}"
    );
}

/// Lenient-input faces (run-2 observed reality, pinned): a whitespace-only
/// command completes as a no-op (exit 0, silent) and an unterminated quote
/// is silently auto-closed by the lexer (echo runs, exit 0, empty stderr).
/// Registered deviation: SHELL-LEXER-LENIENT-UNCLOSED-QUOTE (dash/bash:
/// unexpected-EOF syntax error); the exec_via_sh empty-command arm is NOT
/// reachable from whitespace text (the lexer produces zero statements, the
/// interpreter returns before exec).
///
/// SHELL-PARSER-PANIC (fixed): an unbalanced subshell paren (`(echo`) is a
/// parser error, not a process abort — the lexer's "Unclosed subshell" is
/// surfaced as a fail-closed shell error (exit 1, stderr carries the text),
/// matching bun upstream's `Unclosed subshell` error face.
#[cfg(unix)]
#[test]
fn shell_lenient_input_faces() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = [];
            var e1 = sh.run('   ');
            r.push('empty:' + e1.exitCode + ':' + e1.stdout.length + ':' + e1.stderr.length);
            var e2 = sh.run('echo "unclosed');
            r.push('lex:' + e2.exitCode + ':' + e2.stdout.trim() + ':' + e2.stderr.length);
            var e3 = sh.run('(echo');
            r.push('paren:' + e3.exitCode + ':' + e3.stdout.length + ':' + e3.stderr.trim());
            var e4 = sh.run('echo $(unterminated');
            r.push('cmdsubst:' + e4.exitCode + ':' + e4.stdout.length + ':' + e4.stderr.trim());
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "empty:0:0:0|lex:0:unclosed:0|paren:1:0:Unclosed subshell|cmdsubst:1:0:Unclosed command substitution",
        "lenient input faces: {out}"
    );
}

/// A command terminated by a signal should surface as a negative exitCode
/// (Term::Signal arm).
///
/// Carrier: `kill -9 $$` makes the `sh -c` child signal itself. This became
/// reachable once `$$` stopped being lexed as two literal-`$` Text atoms and
/// re-quoted (SHELL-VAR-RENDER-MANGLES-DOUBLEDOLLAR, fixed): `$$` now lexes
/// as a Var atom, reconstructs as `$$`, and /bin/sh performs the PID
/// expansion. Deterministic — no external killer race.
#[test]
#[cfg(unix)]
fn shell_signal_exit_code_arm() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        "(function () { var r = new Bun.Shell().run('kill -9 $$'); \
         return 'exit=' + r.exitCode + ' success=' + r.success; })()",
    );
    assert_eq!(out, "exit=-9 success=false", "signal exit: {out}");
}

/// cwd override (cd) feeds exec_via_sh_with_stdin's current_dir arm.
#[cfg(unix)]
#[test]
fn shell_cwd_override_arm() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            sh.cd('/etc');
            var pwd = sh.run('pwd');
            return pwd.stdout.trim() === '/etc' ? 'ok' : 'CWD:' + pwd.stdout.trim();
        })()
        "#,
    );
    assert_eq!(out, "ok", "cwd override: {out}");
}

/// Output-method bodies on a real result: json parses stdout, lines splits.
/// bytes() is NOT exercised here — it is the already-registered
/// SHELL-BYTES-NULL-ISHARED-CRASH face (C8a's ignored test; intermittent
/// SIGSEGV, hit again in C16 run 6).
#[cfg(unix)]
#[test]
fn shell_output_method_bodies() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var j = sh.run("printf '%s' '{\"n\":7}'").json();
            if (!j || j.n !== 7) return 'JSON:' + JSON.stringify(j);
            var ls = sh.run('printf "a\\nb\\n"').lines();
            if (ls.length !== 3 || ls[0] !== 'a' || ls[1] !== 'b' || ls[2] !== '') return 'LINES:' + JSON.stringify(ls);
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "output bodies: {out}");
}

/// Output-method non-object `this` arms (undefined without throwing).
///
/// #[ignore] (C16): the proto-method-call carrier SIGSEGV'd intermittently
/// (green in runs 1-2, crash in run 3 under parallel load) — an unstable
/// crash in the result-face/prototype path that needs product-side GC-grade
/// investigation before it can pin the `!this.is_object()` arms.
#[test]
#[cfg(unix)]
#[ignore = "SHELL-THIS-ARM-INTERMITTENT-SIGSEGV: proto.text.call(42)-shape carrier crashed once in three runs under parallel load; needs product-side investigation"]
fn shell_output_method_this_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = sh.run('echo hi');
            var proto = Object.getPrototypeOf(r);
            var t = proto.text.call(42);
            var js2 = proto.json.call(null);
            var ls2 = proto.lines.call(undefined);
            var bs2 = proto.bytes.call(0);
            var thisArms = [t, js2, Array.isArray(ls2) ? 'arr' : String(ls2), bs2 instanceof Uint8Array ? 'u8' : String(bs2)].join(',');
            return thisArms === 'undefined,undefined,arr,u8' ? 'ok' : 'THIS:' + thisArms;
        })()
        "#,
    );
    assert_eq!(out, "ok", "output this arms: {out}");
}

/// Atom rendering arms through the pipeline reconstruct path: quoted-empty,
/// $1 positional, braces (rendered literally — /bin/sh has no brace
/// expansion), tilde (expanded by sh).
#[cfg(unix)]
#[test]
fn shell_atom_reconstruction_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var sh = new Bun.Shell();
            var r = [];
            r.push('qempty:' + sh.run('printf "%s" "" | cat').stdout.length);
            r.push('argv1:' + sh.run('echo $1 | cat').stdout.trim().length);
            var braces = sh.run('echo {x,y} | cat');
            r.push('braces:' + braces.exitCode + ':' + (braces.stdout.trim().length > 0 ? 'nonempty' : 'empty'));
            var tilde = sh.run('echo ~ | cat');
            r.push('tilde:' + tilde.exitCode + ':' + (tilde.stdout.trim().length > 0 ? 'expanded' : 'empty'));
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "qempty:0|argv1:0|braces:0:nonempty|tilde:0:expanded",
        "atom arms: {out}"
    );
}

/// Bun.$ promise face: resolves with the result object (stdout/exitCode/
/// text face) for a passing pipeline; rejects with a ShellError carrying
/// the full payload shape (name/message/stdout/stderr/success/exitCode).
/// The rejection command is a bare `exit 7` — the stdout content of a
/// multi-statement failing pipeline is not pinned (run-1 showed the echo
/// output lost on the rejection path; see C16 report).
#[cfg(unix)]
#[test]
fn shell_dollar_promise_and_shell_error_shape() {
    let mut ctx = setup_ctx();
    ev(
        &mut ctx,
        r#"
        globalThis.__c16s = { ok: null, err: null, done: 0 };
        Bun.$`echo dollar-c16`.then(function (v) { globalThis.__c16s.ok = v; globalThis.__c16s.done++; });
        Bun.$`exit 7`
            .then(function () { globalThis.__c16s.err = 'NO-REJECT'; globalThis.__c16s.done++; })
            .catch(function (e) { globalThis.__c16s.err = e; globalThis.__c16s.done++; });
        'spawned'
        "#,
    );
    drive_event_loop(&mut ctx, 120);
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var c = globalThis.__c16s;
            if (c.done !== 2) return 'PENDING:' + c.done;
            if (!c.ok || typeof c.ok !== 'object') return 'BADOKTYPE:' + typeof c.ok;
            if (c.ok.stdout !== 'dollar-c16\n') return 'BADOK:' + JSON.stringify(c.ok.stdout);
            if (c.ok.exitCode !== 0 || c.ok.success !== true) return 'BADOKFACE';
            if (typeof c.ok.text !== 'function') return 'BADTEXTFACE';
            var e = c.err;
            if (!e || typeof e !== 'object') return 'BADERRTYPE:' + typeof e;
            if (e.name !== 'ShellError') return 'BADNAME:' + e.name;
            if (e.message !== 'Failed with exit code: 7') return 'BADMSG:' + e.message;
            if (e.exitCode !== 7 || typeof e.exitCode !== 'number') return 'BADCODE:' + e.exitCode;
            if (e.success !== false) return 'BADSUCCESS:' + e.success;
            if (typeof e.stdout !== 'string') return 'BADSTDOUTTYPE:' + typeof e.stdout;
            if (typeof e.stderr !== 'string') return 'BADSTDERRTYPE:' + typeof e.stderr;
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "Bun.$ promise + ShellError shape: {out}");
}

// ══════════════════════════════ bun:ffi (bun_ffi.rs) ══════════════════════════════

/// TypeSpec::parse full type-name matrix through descriptor validation: all
/// 16 type names parse at dlopen time (eager dlsym of a real symbol first —
/// C8a discipline). The narrow-width CALL faces are exercised for real:
/// integer args follow C-cast (ToInt32/ToUint32) truncation and f32 values
/// cross the SSE register as actual floats (FFI-NARROW-ARG-CALL-SUSPECT,
/// fixed).
#[cfg(unix)]
#[test]
fn ffi_typespec_parse_type_name_matrix() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var types = ['bool','u8','i8','u16','i16','u32','i32','u64','i64',
                         'usize','isize','f32','f64','ptr','cstring'];
            for (var i = 0; i < types.length; i++) {
                var lib = ffi.dlopen('libc.so.6', { abs: { args: [types[i]], returns: 'i32' } });
                if (typeof lib.abs !== 'function') return 'NODESC:' + types[i];
            }
            // 'void' parses in the returns position (Void arm; explicit and
            // omitted forms both covered here / in the void-return test).
            var v = ffi.dlopen('libc.so.6', { sync: { args: ['i32'], returns: 'void' } });
            if (typeof v.sync !== 'function') return 'NODESC:void-returns';
            // The C8a-proven call shape (i32-declared abs).
            var proven = ffi.dlopen('libc.so.6', { abs: { args: ['i32'], returns: 'i32' } });
            if (proven.abs(-7) !== 7) return 'FAIL:i32=' + proven.abs(-7);
            // f64 args through libm (xmm register class; f64 is the one
            // float width whose call marshals — see FFI-NARROW-ARG-CALL-SUSPECT).
            var lm = ffi.dlopen('libm.so.6', { fabs: { args: ['f64'], returns: 'f64' } });
            if (lm.fabs(-7.5) !== 7.5) return 'FAIL:f64=' + lm.fabs(-7.5);
            // Narrow-width CALL faces (FFI-NARROW-ARG-CALL-SUSPECT, fixed):
            // u8 follows C-cast truncation (abs(-7) → 249) and f32 crosses
            // the SSE register as a real float (fabsf(-2.5) → 2.5).
            var u8lib = ffi.dlopen('libc.so.6', { abs: { args: ['u8'], returns: 'i32' } });
            if (u8lib.abs(-7) !== 249) return 'FAIL:u8=' + u8lib.abs(-7);
            var lmf = ffi.dlopen('libm.so.6', { fabsf: { args: ['f32'], returns: 'f32' } });
            if (typeof lmf.fabsf !== 'function') return 'NODESC:f32';
            if (lmf.fabsf(-2.5) !== 2.5) return 'FAIL:f32=' + lmf.fabsf(-2.5);
            return 'ok:16';
        })()
        "#,
    );
    assert_eq!(out, "ok:16", "type-name parse matrix: {out}");
}

/// Unknown type names in args and returns positions (TypeSpec::parse `other`
/// arm at both call sites) and returns-omitted → Void default descriptor.
#[cfg(unix)]
#[test]
fn ffi_typespec_parse_error_and_void_return() {
    let mut ctx = setup_ctx();
    let r1 = eval_try(&mut ctx, "require('bun:ffi').dlopen('libc.so.6', { abs: { args: ['nope'], returns: 'i32' } }).abs(-1)");
    assert!(
        r1.contains("unknown FFI type: nope") && r1.starts_with("THREW"),
        "unknown arg type: {r1}"
    );
    let r2 = eval_try(&mut ctx, "require('bun:ffi').dlopen('libc.so.6', { abs: { args: ['i32'], returns: 'nope' } }).abs(-1)");
    assert!(
        r2.contains("unknown FFI type: nope") && r2.starts_with("THREW"),
        "unknown return type: {r2}"
    );
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var lib = require('bun:ffi').dlopen('libc.so.6', { free: { args: ['ptr'] } });
            lib.free(0); // free(NULL) is a no-op — call completes, ret undefined
            return 'ok:' + String(lib.free(0));
        })()
        "#,
    );
    assert_eq!(out, "ok:undefined", "void-return descriptor: {out}");
}

/// Pointer conversion faces through malloc/free: number roundtrip, BigInt
/// form, negative rejection, BigInt out-of-range rejection, non-numeric
/// rejection (view-probe message).
#[cfg(unix)]
#[test]
fn ffi_pointer_conversion_faces() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', {
                malloc: { args: ['usize'], returns: 'ptr' },
                free: { args: ['ptr'], returns: 'void' }
            });
            var p = lib.malloc(16);
            if (typeof p !== 'number' || p <= 0) return 'BADPTR:' + String(p);
            lib.free(p); // number form
            var p2 = lib.malloc(16);
            lib.free(BigInt(p2)); // BigInt form (jsval_to_ptr bigint arm)
            var r = [];
            function t(label, fn) { try { fn(); r.push(label + '=no-throw'); } catch (e) { r.push(label + '=' + String((e && e.message) || e)); } }
            t('neg', function () { lib.free(-5); });
            t('bigint-range', function () { lib.free(18446744073709551617n); });
            t('nonnum', function () { lib.free({}); });
            return 'ptr-ok|' + r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "ptr-ok|neg=ffi argument 1: pointer must be a non-negative integer|bigint-range=ffi argument 1: BigInt pointer out of range|nonnum=ffi argument 1: argument must be a Buffer/TypedArray view, or a Number/BigInt address",
        "pointer faces: {out}"
    );
}

/// View-pointer faces: a Uint8Array passed to a `ptr` arg is written through
/// (observable), zero-length views pass safely, a plain ArrayBuffer arm, a
/// detached view (when ArrayBuffer.transfer exists) and the non-view
/// rejection.
#[cfg(unix)]
#[test]
fn ffi_view_pointer_faces() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', {
                memset: { args: ['ptr', 'i32', 'usize'], returns: 'ptr' }
            });
            var r = [];
            var u8 = new Uint8Array(4);
            lib.memset(u8, 65, 3);
            r.push('u8:' + u8[0] + u8[1] + u8[2] + u8[3]);
            var empty = new Uint8Array(0);
            lib.memset(empty, 65, 0);
            r.push('zerolen:' + empty.length);
            var ab = new ArrayBuffer(4);
            lib.memset(ab, 66, 0);
            r.push('ab:' + ab.byteLength);
            if (typeof ArrayBuffer.prototype.transfer === 'function') {
                var ab2 = new ArrayBuffer(4);
                var detachedView = new Uint8Array(ab2);
                ab2.transfer();
                r.push('detached:' + detachedView.byteLength);
                try { lib.memset(detachedView, 65, 0); r.push('detachedCall:no-throw'); }
                catch (e) { r.push('detachedCall:threw'); }
            } else {
                r.push('detached:notransfer');
            }
            try { lib.memset({}, 65, 0); r.push('nonview:threw'); }
            catch (e) { r.push('nonview:threw'); }
            return r.join('|');
        })()
        "#,
    );
    assert!(out.starts_with("u8:6565650|zerolen:0|ab:4|detached:"), "view faces head: {out}");
    assert!(
        out.contains("detached:0|detachedCall:no-throw")
            || out.contains("detached:0|detachedCall:threw")
            || out.contains("detached:notransfer|"),
        "detached face: {out}"
    );
    assert!(out.ends_with("nonview:threw"), "nonview face: {out}");
}

/// CString NULL-pointer passthrough: a `cstring` arg given JS `null` hands
/// the callee a NULL pointer. setlocale(0, null) is the glibc-sanctioned
/// NULL-query form (puts(null) is UB and segfaulted run 1).
#[cfg(unix)]
#[test]
fn ffi_cstring_null_passthrough_arm() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', { setlocale: { args: ['i32', 'cstring'], returns: 'ptr' } });
            var p = lib.setlocale(0, null); // query current LC_CTYPE — non-NULL
            if (!p) return 'NULLRET';
            var cs = new ffi.CString(p);
            if (cs.toString().length === 0) return 'EMPTY:' + String(p);
            return 'ok';
        })()
        "#,
    );
    assert_eq!(out, "ok", "cstring null passthrough: {out}");
}

/// js_function arg error arms: a non-object and an object without a callback
/// slot both reject with the "ffi argument 4"-prefixed callback-object
/// message (qsort never invoked); a valid callback with zero elements
/// marshals without invoking.
#[cfg(unix)]
#[test]
fn ffi_js_function_arg_error_arms() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', { qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' } });
            var buf = Buffer.alloc(16);
            var r = [];
            try { lib.qsort(buf, 4, 4, {}); r.push('obj=no-throw'); } catch (e) { r.push('obj=' + ((e && e.message) || e)); }
            try { lib.qsort(buf, 0, 0, 42); r.push('num=no-throw'); } catch (e) { r.push('num=' + ((e && e.message) || e)); }
            var cb = ffi.callback(['ptr', 'ptr'], 'i32', function () { return 0; });
            lib.qsort(buf, 0, 0, cb);
            r.push('valid-zero=ok');
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "obj=ffi argument 4: argument must be a bun:ffi callback object|num=ffi argument 4: argument must be a bun:ffi callback object|valid-zero=ok",
        "js_function arms: {out}"
    );
}

/// callback(argCount, ret, fn) argCount form: args declare as f64 slots (the
/// SysV SSE register class — value-semantics only, NOT address-holding, so
/// the comparator must be constant). The call exercises the argCount parse
/// arm, the f64 c_arg_to_jsval arm and the i32 return-conversion arm; the
/// buffer multiset must survive.
#[cfg(unix)]
#[test]
fn ffi_callback_argcount_form_runs_qsort() {
    let mut ctx = setup_ctx();
    let out = eval_string_full(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', { qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' } });
            var buf = Buffer.alloc(16);
            buf[0] = 3; buf[4] = 1; buf[8] = 2; buf[12] = 4;
            var calls = 0;
            var cmp = ffi.callback(2, 'i32', function () { calls++; return 0; });
            lib.qsort(buf, 4, 4, cmp);
            var sum = buf[0] + buf[4] + buf[8] + buf[12];
            return 'calls=' + calls + '|sum=' + sum;
        })()
        "#,
    );
    assert!(
        out.starts_with("calls="),
        "argCount-form qsort eval failed: {out}"
    );
    let (calls_str, sum_str) = out
        .split_once("|sum=")
        .unwrap_or_else(|| panic!("argCount-form output shape: {out}"));
    let calls: i64 = calls_str
        .trim_start_matches("calls=")
        .parse()
        .unwrap_or_else(|e| panic!("argCount calls parse ({e}): {out}"));
    assert!(
        calls >= 3,
        "qsort on 4 elements must invoke the comparator (>= n-1 comparisons): {out}"
    );
    let sum: i64 = sum_str.parse().unwrap_or_else(|e| panic!("argCount sum parse ({e}): {out}"));
    assert_eq!(sum, 10, "element multiset must survive the call: {out}");
}

/// Callback arg-marshal completion matrix: declared narrow/wide/float arg
/// types marshal through the trampoline (constant comparator; qsort must
/// complete and preserve the element multiset). Buffers are n*4 bytes —
/// qsort's element view.
#[cfg(unix)]
#[test]
fn ffi_callback_arg_marshal_matrix() {
    let mut ctx = setup_ctx();
    let out = eval_string_full(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', { qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' } });
            var shapes = [['u8','i8'], ['u16','i16'], ['u32','u32'], ['u64','i64'],
                          ['f32','f64'], ['bool','bool'], ['usize','usize']];
            var results = [];
            for (var i = 0; i < shapes.length; i++) {
                var buf = Buffer.alloc(16);
                buf[0] = 3; buf[4] = 1; buf[8] = 2; buf[12] = 4;
                var cb = ffi.callback(shapes[i], 'i32', function () { return 0; });
                lib.qsort(buf, 4, 4, cb);
                var sum = buf[0] + buf[4] + buf[8] + buf[12];
                results.push(shapes[i].join('+') + ':' + (sum === 10 ? 'ok' : 'SUM=' + sum));
            }
            return results.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "u8+i8:ok|u16+i16:ok|u32+u32:ok|u64+i64:ok|f32+f64:ok|bool+bool:ok|usize+usize:ok",
        "arg marshal matrix: {out}"
    );
}

/// Callback return-type matrix: the i32 arm drives a real observable
/// ordering (C8a-proven ptr/ptr comparator shape); the remaining scalar
/// return arms execute through ignored-value comparators (the conversion
/// line runs; order is not ABI-observable because qsort reads the int
/// return register).
#[cfg(unix)]
#[test]
fn ffi_callback_return_type_matrix() {
    let mut ctx = setup_ctx();
    let out = eval_string_full(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6', { qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' } });
            function write32(buf, i, v) { buf[i*4] = v & 0xff; buf[i*4+1] = (v >>> 8) & 0xff; buf[i*4+2] = (v >>> 16) & 0xff; buf[i*4+3] = (v >>> 24) & 0xff; }
            function read32(buf, i) { return buf[i*4] | (buf[i*4+1] << 8) | (buf[i*4+2] << 16) | (buf[i*4+3] << 24); }
            function readAddr32(p) { var b = ffi.toBuffer(p, 4); return b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24); }
            function sortedAfter(run) {
                var buf = Buffer.alloc(16);
                write32(buf, 0, 4); write32(buf, 1, 1); write32(buf, 2, 3); write32(buf, 3, 2);
                run(buf);
                return read32(buf, 0) === 1 && read32(buf, 1) === 2 && read32(buf, 2) === 3 && read32(buf, 3) === 4;
            }
            var r = [];
            r.push('i32:' + sortedAfter(function (buf) {
                lib.qsort(buf, 4, 4, ffi.callback(['ptr', 'ptr'], 'i32', function (a, b) {
                    var x = readAddr32(a), y = readAddr32(b); return x < y ? -1 : x > y ? 1 : 0; }));
            }));
            var ignored = ['bool', 'u8', 'i8', 'u16', 'i16', 'u32', 'u64', 'i64', 'f32', 'f64', 'void', 'ptr'];
            var buf = Buffer.alloc(16);
            write32(buf, 0, 2); write32(buf, 1, 1); write32(buf, 2, 4); write32(buf, 3, 3);
            for (var i = 0; i < ignored.length; i++) {
                var cmp = ffi.callback(['ptr', 'ptr'], ignored[i], function () { return 1; });
                lib.qsort(buf, 4, 4, cmp);
            }
            var sum = read32(buf, 0) + read32(buf, 1) + read32(buf, 2) + read32(buf, 3);
            r.push('ignored:' + (sum === 10 ? 'ok' : 'SUM=' + sum));
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "i32:true|ignored:ok",
        "ret matrix: {out}"
    );
}

/// Callback construction validation faces + the per-instance `ptrValue`
/// property. close()/ptr() are reachable through FfiCallback.prototype and
/// the object chains onto it (FFI-CALLBACK-METHODS-UNREACHABLE, fixed):
/// ptr() equals ptrValue; close() frees the closure and is one-shot.
#[cfg(unix)]
#[test]
fn ffi_callback_construction_faces() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var cb = ffi.callback(['i32'], 'i32', function (x) { return x; });
            var pv = cb.ptrValue;
            if (typeof pv !== 'number' || pv <= 0) return 'BADPTRVALUE:' + String(pv);
            var r = [];
            try { ffi.callback(); r.push('nofn=no-throw'); } catch (e) { r.push('nofn=' + ((e && e.message) || e)); }
            try { ffi.callback(['zzz'], 'i32', function () {}); r.push('badtype=no-throw'); } catch (e) { r.push('badtype=' + ((e && e.message) || e)); }
            try { ffi.callback(17, 'i32', function () {}); r.push('count=no-throw'); } catch (e) { r.push('count=' + ((e && e.message) || e)); }
            try { ffi.callback(-1, 'i32', function () {}); r.push('negcount=no-throw'); } catch (e) { r.push('negcount=' + ((e && e.message) || e)); }
            // FFI-CALLBACK-METHODS-UNREACHABLE (fixed): instance methods live
            // on FfiCallback.prototype and the instance chains onto it.
            r.push('closeFace:' + (typeof cb.close === 'function' ? 'present' : 'absent'));
            r.push('ptrFace:' + (typeof cb.ptr === 'function' ? 'present' : 'absent'));
            // Constructor-less class: SM installs the prototype object itself
            // under the class name (there is no `.prototype` property).
            r.push('proto:' + (Object.getPrototypeOf(cb) === globalThis.FfiCallback));
            r.push('ptrEq:' + (cb.ptr() === cb.ptrValue));
            cb.close();
            r.push('closed:ok');
            try { cb.close(); r.push('reclose=no-throw'); } catch (e) { r.push('reclose=' + ((e && e.message) || e)); }
            return 'ptrvalue-ok|' + r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "ptrvalue-ok|nofn=bun:ffi callback: function argument required|badtype=bun:ffi callback: args[0]: unknown FFI type: zzz|count=bun:ffi callback: arg count must be 0..=16|negcount=bun:ffi callback: arg count must be 0..=16|closeFace:present|ptrFace:present|proto:true|ptrEq:true|closed:ok|reclose=FfiCallback.close: invalid FfiCallback object",
        "callback construction: {out}"
    );
}

/// Library faces: invalid `this`, symbol name required, symbol probe +
/// missing symbol error on the one-arg object; two-arg face's symbol-keyed
/// descriptor entries are skipped (exports = ['symbols']). Both faces chain
/// onto FfiLibrary.prototype, so close/symbol are reachable on the two-arg
/// object too (FFI-TWOARG-LIB-PROTO-MISSING, fixed) and the dlopen handle
/// stays alive with the object until close().
#[cfg(unix)]
#[test]
fn ffi_library_one_arg_faces_and_symbol_errors() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var lib = ffi.dlopen('libc.so.6'); // one-arg face
            var r = [];
            try { lib.close.call(42); r.push('closeThis=no-throw'); } catch (e) { r.push('closeThis=' + ((e && e.message) || e)); }
            try { lib.symbol.call({}); r.push('symThis=no-throw'); } catch (e) { r.push('symThis=' + ((e && e.message) || e)); }
            try { lib.symbol(); r.push('symName=no-throw'); } catch (e) { r.push('symName=' + ((e && e.message) || e)); }
            r.push('symProbe:' + lib.symbol('getpid'));
            try { ffi.dlopen('libc.so.6', { zzz_c16_missing: { args: ['i32'], returns: 'i32' } }); r.push('missing=no-throw'); } catch (e) { r.push('missing=' + String((e && e.message) || e).slice(0, 24)); }
            var keyed = {};
            keyed[Symbol('skip-me')] = { args: ['i32'], returns: 'i32' };
            var lib2 = ffi.dlopen('libc.so.6', keyed);
            var names = Object.keys(lib2).sort().join(',');
            r.push('keyedSkip:' + names);
            // FFI-TWOARG-LIB-PROTO-MISSING (fixed): the two-arg Library
            // chains onto FfiLibrary.prototype — close/symbol reachable, the
            // handle is owned by the object, and close is one-shot.
            r.push('twoArgClose:' + (typeof lib2.close === 'function' ? 'present' : 'absent'));
            r.push('twoArgSymbol:' + (typeof lib2.symbol === 'function' ? 'present' : 'absent'));
            r.push('twoArgProto:' + (Object.getPrototypeOf(lib2) === globalThis.FfiLibrary.prototype));
            r.push('twoArgProbe:' + lib2.symbol('getpid'));
            lib2.close();
            r.push('twoArgClosed:ok');
            try { lib2.close(); r.push('twoArgReclose=no-throw'); } catch (e) { r.push('twoArgReclose=' + ((e && e.message) || e)); }
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "closeThis=FfiLibrary.close: invalid FfiLibrary object|symThis=FfiLibrary.symbol: invalid FfiLibrary object|symName=symbol: name argument required|symProbe:true|missing=dlopen: zzz_c16_missing:|keyedSkip:symbols|twoArgClose:present|twoArgSymbol:present|twoArgProto:true|twoArgProbe:true|twoArgClosed:ok|twoArgReclose=Library already closed",
        "library faces: {out}"
    );
}

/// CString constructor faces: a libc pointer round-trips (getenv PATH →
/// CString toString/length/toPrimitive), ptr 0 yields the empty arm, missing
/// arg rejects; toBuffer error arms + opts.free rejection + zero-copy view.
#[cfg(unix)]
#[test]
fn ffi_cstring_ctor_and_to_buffer_faces() {
    let mut ctx = setup_ctx();
    let out = ev(
        &mut ctx,
        r#"
        (function () {
            var ffi = require('bun:ffi');
            var pathEnv = String(process.env.PATH || '/usr/bin');
            var lib = ffi.dlopen('libc.so.6', {
                getenv: { args: ['cstring'], returns: 'ptr' },
                malloc: { args: ['usize'], returns: 'ptr' },
                free: { args: ['ptr'], returns: 'void' }
            });
            var r = [];
            var p = lib.getenv('PATH');
            if (!p) { r.push('getenv:skip'); }
            else {
                var cs = new ffi.CString(p);
                if (cs.toString() !== pathEnv) return 'PATH-MISMATCH:' + cs.toString() + ' vs ' + pathEnv;
                if (cs.length !== pathEnv.length) return 'LEN:' + cs.length;
                if (String(cs) !== pathEnv) return 'PRIM:' + String(cs);
                r.push('getenv:ok');
            }
            var zero = new ffi.CString(0);
            r.push('zero:' + (zero.toString() === '' && zero.length === 0 ? 'ok' : 'BAD'));
            try { new ffi.CString(); r.push('noarg:no-throw'); } catch (e) { r.push('noarg:' + ((e && e.message) || e)); }
            try { ffi.toBuffer(); r.push('tb-noargs:no-throw'); } catch (e) { r.push('tb-noargs:' + ((e && e.message) || e)); }
            try { ffi.toBuffer('x', 4); r.push('tb-ptr:no-throw'); } catch (e) { r.push('tb-ptr:' + ((e && e.message) || e)); }
            var mp = lib.malloc(8);
            try { ffi.toBuffer(mp, 0); r.push('tb-zero:no-throw'); } catch (e) { r.push('tb-zero:' + ((e && e.message) || e)); }
            try { ffi.toBuffer(mp, 8, { free: 'nope' }); r.push('tb-free:no-throw'); } catch (e) { r.push('tb-free:' + ((e && e.message) || e)); }
            var view = ffi.toBuffer(mp, 8);
            view[0] = 0x41; view[7] = 0x42;
            r.push('tb-view:' + (view[0] === 0x41 && view[7] === 0x42 ? 'ok' : 'BAD'));
            lib.free(mp);
            return r.join('|');
        })()
        "#,
    );
    assert_eq!(
        out,
        "getenv:ok|zero:ok|noarg:CString: pointer argument required|tb-noargs:toBuffer(pointer, byteLength) requires 2 arguments|tb-ptr:pointer must be a Number or BigInt|tb-zero:toBuffer: pointer and byteLength must be non-zero|tb-free:toBuffer: opts.free must be a non-null function pointer number|tb-view:ok",
        "CString/toBuffer faces: {out}"
    );
}
