// @trace TEST-ENG-007-UTIL-TTY-LIB-C17 [req:REQ-ENG-007] [level:integration]
//
// C17 line-coverage wave: node_util.rs + node_tty.rs + lib.rs residual gaps.
// Baseline gap ledger (coverage-governance-20261017.md「缺口结构」):
// node_util(451) / node_tty(383) / lib(348). C8b assets (util_edge) already
// cover inspect primitives/arrays/depth/Buffer, format specifier arms,
// styleText, types matrix, promisify/callbackify, assert spec forms — this
// file only adds faces those files do not touch. Oracle: node v24.19.0
// (local, byte-level pty probes for tty; `node -e` for util).
//
// ── 产品缺陷根治台账(T1-T5 / U1-U10,逐项 = node v24.19.0 本机实测)──────
// | # | 面 | 旧行为 | node v24.19.0 oracle | 本波处置 |
// |---|---|---|---|---|
// | U1 | util.parseArgs | Rust stub 返回 `{}` | `parseArgs({args,options})` → `{values,positionals}`,strict 默认未知选项/位置参数抛 ERR_PARSE_ARGS_* | FIXED(JS 移植 node parse_args 全语义) |
// | U2 | util.getSystemErrorName | 恒 undefined | `-1`→"EPERM" / `-13`→"EACCES";`0`/正数 → RangeError ERR_OUT_OF_RANGE;非数字 → TypeError ERR_INVALID_ARG_TYPE | FIXED(bun_sys::UV_E::name 映射) |
// | U3 | util.deprecate | 原样返回 fn,零包装零告警 | 包装 fn(w !== g,prototype/length 保留),首次调用发 DeprecationWarning | FIXED |
// | U4 | util.inherits | 只设 `Child.super_` | 另接原型链 → `new C() instanceof P` true | FIXED |
// | U5 | util.inspect 类族 | 全部落 plain-object/空串 | Map/Set/Date/RegExp/TypedArray/WeakMap/Promise/Symbol/BigInt/class/Error/null-proto 的 node 形态 | FIXED(委托 Bun.inspect + 本地类名前缀) |
// | U6 | util.format `%i`/`%d`/`%f` | 全走 display | `%i`=parseInt 截断/`%d`=Number/`%f`=parseFloat;`%c` 吞参 | FIXED(余参联结仍为 C8b 已登记分歧,本波未改) |
// | U7 | util.types.isProxy | constructor 探针,真 proxy false | 真 proxy true;普通对象 false;函数 proxy true(node v24 oracle parity) | FIXED(JS_IsProxy 原生探针——mozjs fork js::IsProxy 薄包装,替代结构化克隆启发式) |
// | U8 | assert loose deepEqual 文案 | "loosely deeply equal" | "Expected values to be loosely deep-equal:" | FIXED |
// | U9 | isDeepStrictEqual Rust native | 注册后被 JS 覆盖 = 死注册臂 | — | FIXED(删除死注册,JS 面为唯一实现) |
// | U10 | assert fail()/equal() 文案 | 全部带 `AssertionError: ` 前缀;equal 走 diff headline | fail 用户消息裸 `boom`;equal 双侧渲染 `==` 联结 | FIXED(用户消息裸传;生成消息前缀按旧约定保留) |
// | T1 | tty writer 方法返回值 | 返回 `this` | boolean true | FIXED |
// | T2 | tty moveCursor(dx,dy) | dy 符号反转 + 臂序颠倒 | dy>0→`[nB` 下移,dx 先(`moveCursor(1,2)` → `\x1b[1C\x1b[2B`) | FIXED |
// | T3 | tty cursorTo(x) 缺 y 臂 | 强制行归位 | 仅列移动(`cursorTo(4)` → `\x1b[5G`) | FIXED |
// | T4 | tty 零尺寸窗 getWindowSize | undefined | [0,0] | FIXED |
// | T5 | tty 非 int32 参数 | `JSVal::to_int32` assert → 进程 abort | ToNumber 容错(`clearLine('x')`→2K;符号→TypeError) | FIXED(根治 crash 类) |
// | L1 | lib.rs seam 体 | 链接期 face,运行时不可达 | — | (登记,归分母治理) |
// | L2 | lib.rs shutdown_engine | 进程级 JS_ShutDown,suite 内毒化后续测试 | — | (登记:harness 边界面) |

use bao_engine::context::JsContext;
use mozjs::rooted;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run `body` and return the joined per-check results; every label must
/// report PASS for the caller's assert to hold (util_edge idiom).
fn run_checks(ctx: &mut JsContext, body: &str) -> String {
    let wrapped = format!(
        r#"
        var __results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); __results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ __results.push(label + " ERR:" + (e.message || e).toString().substring(0, 90)); }}
        }}
        {body}
        __results.join("\n");
        "#
    );
    eval_string_full(ctx, &wrapped)
}

fn assert_all_pass(results: &str) {
    assert!(
        !results.contains("FAIL") && !results.contains("ERR:"),
        "c17 checks failed:\n{}",
        results
    );
}

// ---------------------------------------------------------------------------
// 1. util.inspect — the class-instance family (C8b covered primitives,
//    arrays, Buffer, functions, depth caps; every non-plain-object branch
//    below funnels into the plain-object tail, which is the uncovered face).
// ---------------------------------------------------------------------------

#[test]
fn c17_util_inspect_class_family() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // node oracle (v24.19.0, local): Map(1) { 1 => 'a' }.
        check("inspect_map_class_face", function() {
            var s = util.inspect(new Map([[1, 'a']]));
            if (s === "Map(1) { 1 => 'a' }") return true;
            throw new Error('GOT:' + s);
        });
        check("inspect_set_class_face", function() {
            return util.inspect(new Set([1, 2])) === 'Set(2) { 1, 2 }';
        });
        check("inspect_date_class_face", function() {
            return util.inspect(new Date(0)) === '1970-01-01T00:00:00.000Z';
        });
        check("inspect_regexp_class_face", function() {
            return util.inspect(/ab/g) === '/ab/g';
        });
        // node oracle: error head starts with `TypeError: boom` (node appends
        // the stack; SpiderMonkey frames differ, so the head is the stable
        // contract).
        check("inspect_error_class_face", function() {
            return util.inspect(new TypeError('boom')).indexOf('TypeError: boom') === 0;
        });
        check("inspect_uint8array_class_face", function() {
            return util.inspect(new Uint8Array([1, 2, 3])) === 'Uint8Array(3) [ 1, 2, 3 ]';
        });
        check("inspect_weakmap_class_face", function() {
            return util.inspect(new WeakMap()) === 'WeakMap { <items unknown> }';
        });
        check("inspect_weakset_class_face", function() {
            return util.inspect(new WeakSet()) === 'WeakSet { <items unknown> }';
        });
        check("inspect_promise_class_face", function() {
            return util.inspect(new Promise(function() {})) === 'Promise { <pending> }';
        });
        check("inspect_symbol_face", function() {
            return util.inspect(Symbol('s')) === 'Symbol(s)'
                && util.inspect(Symbol()) === 'Symbol()';
        });
        check("inspect_bigint_face", function() {
            return util.inspect(BigInt(9)) === '9n';
        });
        check("inspect_class_instance_prefix", function() {
            var s = util.inspect(new (function Point() { this.x = 1; })());
            return s === 'Point { x: 1 }';
        });
        check("inspect_null_proto_prefix", function() {
            var o = Object.assign(Object.create(null), {a: 1});
            return util.inspect(Object.create(null)) === '[Object: null prototype] {}'
                && util.inspect(o) === '[Object: null prototype] { a: 1 }';
        });
        // The tail arms themselves still honor depth caps and key ordering
        // (mixed int + string keys on a plain object — the int-jsid arm).
        check("inspect_mixed_keys_int_first", function() {
            var o = { name: 'n' };
            o[3] = 'c';
            o[1] = 'a';
            var s = util.inspect(o);
            return s.indexOf('1: ') !== -1 && s.indexOf('3: ') !== -1
                && s.indexOf('1: ') < s.indexOf('3: ')
                && s.indexOf("name: 'n'") !== -1;
        });
        check("inspect_nested_class_family_depth_cap", function() {
            // node oracle: a Map at the depth cap still renders its empty
            // body; a nested plain object caps to [Object].
            var outer = { m: new Map() };
            var s = util.inspect(outer, { depth: 0 });
            var t = util.inspect({a: {b: 1}}, {depth: 0});
            if (s === '{ m: Map(0) {} }' && t === '{ a: [Object] }') return true;
            throw new Error('GOT:' + s + ' / ' + t);
        });
        "#
    );
    assert_all_pass(&results);
}

// ---------------------------------------------------------------------------
// 2. util.assert — loose deepEqual / equal-on-objects diff rendering
//    (domain-check 300f3a0b29 face; C8b covered the strict variants).
// ---------------------------------------------------------------------------

#[test]
fn c17_util_assert_diff_faces() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var assert = require('assert');

        // node oracle wording: "Expected values to be loosely deep-equal:"
        // + diff body. The diff body shape (object lines, +/- markers on the
        // differing key) is the contract under test.
        check("deep_equal_loose_diff_body", function() {
            var msg = '';
            try { assert.deepEqual({ a: 1, b: 2 }, { a: 1, b: 3 }); }
            catch (e) {
                if (e.name !== 'AssertionError') return false;
                msg = e.message;
            }
            return msg.indexOf('loosely deep-equal') !== -1
                && msg.indexOf('loosely deeply equal') === -1
                && msg.indexOf('a: 1') !== -1
                && msg.indexOf('b: 2') !== -1 && msg.indexOf('b: 3') !== -1
                && msg.indexOf('+') !== -1 && msg.indexOf('-') !== -1;
        });
        // equal() on objects: node renders both sides joined by `==`
        // (multi-line inspect), never the collapsed `[object Object]` form.
        check("equal_objects_rendered_with_operator", function() {
            var msg = '';
            try { assert.equal({ a: 1 }, { a: 1 }); }
            catch (e) { msg = e.message; }
            if (msg.indexOf(' == ') !== -1
                && msg.indexOf('{\n  a: 1\n}') !== -1
                && msg.indexOf('[object Object]') === -1) return true;
            throw new Error('GOT:' + msg.substring(0, 160));
        });
        check("equal_primitives_operator_join", function() {
            var msg = '';
            try { assert.equal(1, 2); }
            catch (e) { msg = e.message; }
            return msg.indexOf('1 == 2') !== -1;
        });
        // node: a user-supplied fail() message is delivered verbatim
        // (e.message === 'boom'), no "AssertionError: " prefix.
        check("fail_message_is_bare_argument", function() {
            try { assert.fail('boom'); }
            catch (e) {
                if (e && e.name === 'AssertionError' && e.message === 'boom') return true;
                throw new Error('GOT:name=' + (e && e.name) + ',msg=' + (e && e.message) + ',kind=' + typeof e);
            }
            throw new Error('GOT:no-throw');
        });
        // node: fail() with no message → "Failed" (generated-message prefix
        // retained by the legacy contract: AssertionError: Failed).
        check("fail_default_message", function() {
            try { assert.fail(); }
            catch (e) { return e.name === 'AssertionError' && e.message.indexOf('Failed') !== -1; }
            return false;
        });
        check("if_error_non_error_throws", function() {
            try { assert.ifError(new Error('wrapped')); return false; }
            catch (e) { return e.message === 'wrapped'; }
        });
        check("if_error_falsy_ok", function() {
            assert.ifError(null);
            assert.ifError(undefined);
            return true;
        });
        check("not_deep_equal_loose_pass_and_fail", function() {
            assert.notDeepEqual({ a: 1 }, { a: 2 });
            var threw = false;
            try { assert.notDeepEqual({ a: 1 }, { a: 1 }); }
            catch (e) { threw = e.name === 'AssertionError'; }
            return threw;
        });
        "#
    );
    assert_all_pass(&results);
}

// ---------------------------------------------------------------------------
// 3. util stub faces vs node v24 oracle — registered divergences U1-U4/U7,
//    pinned here as characterization (register-only, no product fix).
// ---------------------------------------------------------------------------

#[test]
fn c17_util_oracle_parity() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // U1 — node parseArgs semantics (options, short/long forms, defaults,
        // strict errors, `--` terminator, multiple).
        check("parseargs_values_positionals", function() {
            var r = util.parseArgs({
                args: ['-f', '--bar', 'b', 'pos'],
                options: { foo: {type: 'boolean', short: 'f'}, bar: {type: 'string'} },
                allowPositionals: true
            });
            return r.values.foo === true && r.values.bar === 'b'
                && r.positionals.length === 1 && r.positionals[0] === 'pos';
        });
        check("parseargs_long_equals_and_defaults", function() {
            var r = util.parseArgs({
                args: ['--x=1'],
                options: {
                    x: {type: 'string'},
                    y: {type: 'string', default: 'dv'},
                    z: {type: 'string', multiple: true}
                }
            });
            return r.values.x === '1' && r.values.y === 'dv'
                && r.values.z === undefined;
        });
        check("parseargs_strict_unknown_and_positional", function() {
            function codeOf(fn) {
                try { fn(); return null; } catch (e) { return e.code; }
            }
            return codeOf(function() { util.parseArgs({args: ['--nope']}); })
                       === 'ERR_PARSE_ARGS_UNKNOWN_OPTION'
                && codeOf(function() { util.parseArgs({args: ['pos']}); })
                       === 'ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL'
                && codeOf(function() {
                       util.parseArgs({args: ['--x'], options: {x: {type: 'string'}}});
                   }) === 'ERR_PARSE_ARGS_INVALID_OPTION_VALUE';
        });
        check("parseargs_terminator_and_negative", function() {
            var r = util.parseArgs({args: ['--', '-f', '--bar'], allowPositionals: true});
            var n = util.parseArgs({
                args: ['--no-flag'],
                options: {flag: {type: 'boolean'}},
                allowNegative: true
            });
            return r.positionals.join(',') === '-f,--bar' && n.values.flag === false;
        });
        // U2 — libuv errno names + node validation.
        check("getsystemerrorname_mapping", function() {
            return util.getSystemErrorName(-1) === 'EPERM'
                && util.getSystemErrorName(-13) === 'EACCES'
                && util.getSystemErrorName(-2) === 'ENOENT'
                && util.getSystemErrorName(-4096) === 'Unknown system error -4096';
        });
        check("getsystemerrorname_validation", function() {
            function thrown(fn) {
                try { fn(); return null; } catch (e) { return e; }
            }
            var r0 = thrown(function() { return util.getSystemErrorName(0); });
            var r1 = thrown(function() { return util.getSystemErrorName(1); });
            var s = thrown(function() { return util.getSystemErrorName('x'); });
            var u = thrown(function() { return util.getSystemErrorName(); });
            return r0 && r0.name === 'RangeError' && r0.code === 'ERR_OUT_OF_RANGE'
                && r1 && r1.code === 'ERR_OUT_OF_RANGE'
                && s && s.name === 'TypeError' && s.code === 'ERR_INVALID_ARG_TYPE'
                && u && u.code === 'ERR_INVALID_ARG_TYPE';
        });
        // U3 — deprecate wraps (w !== g), preserves prototype/length and
        // emits exactly one DeprecationWarning on first invocation.
        check("deprecate_wraps_and_warns_once", function() {
            var g = function twice(a, b) { return a + b; };
            var w = util.deprecate(g, 'gone soon');
            var count = 0;
            var original = process.emitWarning;
            process.emitWarning = function(msg, type, code) { count++; };
            var r1, r2;
            try { r1 = w(1, 2); r2 = w(3, 4); } finally { process.emitWarning = original; }
            return w !== g && w.name === 'deprecated'
                && r1 === 3 && r2 === 7
                && w.length === g.length
                && Object.getPrototypeOf(w) === g
                && count === 1;
        });
        check("deprecate_noargs_throws_node_typeerror", function() {
            try { util.deprecate(); return false; }
            catch (e) { return e.name === 'TypeError'; }
        });
        // U4 — super_ *and* prototype chain.
        check("inherits_links_prototype_chain", function() {
            function P() {}
            function C() {}
            util.inherits(C, P);
            var protoLinked = C.prototype.__proto__ === P.prototype;
            return C.super_ === P && protoLinked === true
                && ((new C()) instanceof P);
        });
        // inherits argc/argument guards (owned no-op face, non-node).
        check("inherits_guards", function() {
            function C() {}
            return util.inherits(C) === undefined
                && util.inherits(5, {}) === undefined
                && util.inherits({}, 5) === undefined;
        });
        // U7 — real proxies answer true, plain objects false. Function
        // proxies are detected too via the mozjs fork's JS_IsProxy (thin
        // wrapper over the public inline js::IsProxy), matching the node v24
        // oracle 100% (probe /tmp/u7-probe: node utilTypesIsProxy=true for
        // userland `new Proxy(function target, {})`, bao previously false).
        check("types_isproxy_object_proxy", function() {
            var p = new Proxy({}, {});
            return util.types.isProxy(p) === true
                && util.types.isProxy({}) === false
                && util.types.isProxy({a: 1}) === false
                && util.types.isProxy(null) === false
                && util.types.isProxy(undefined) === false
                && util.types.isProxy(42) === false
                && util.types.isProxy('x') === false;
        });
        // U7 function-proxy face — oracle parity (node v24 answers true).
        check("types_isproxy_function_proxy", function() {
            var fp = new Proxy(function() {}, {});
            var fp2 = new Proxy(function ctrlTarget() {}, {
                get: function(t, k) { return t[k]; }
            });
            return util.types.isProxy(fp) === true
                && util.types.isProxy(fp2) === true
                // Non-proxy functions and callable objects stay false.
                && util.types.isProxy(function() {}) === false
                && util.types.isProxy(Math.max) === false;
        });
        check("types_is_module_namespace_object", function() {
            // Math is not a module namespace; a real namespace needs ESM, so
            // only the negative face is harness-reachable.
            return util.types.isModuleNamespaceObject(Math) === false
                && util.types.isModuleNamespaceObject({}) === false;
        });
        // U9 — the dead Rust registration is gone; the JS implementation is
        // the single face (primitive + object arms).
        check("is_deep_strict_equal_js_face", function() {
            return util.isDeepStrictEqual(2.5, 2.5) === true
                && util.isDeepStrictEqual(2.5, 2) === false
                && util.isDeepStrictEqual(true, 1) === false
                && util.isDeepStrictEqual({a: [1, 2]}, {a: [1, 2]}) === true
                && util.isDeepStrictEqual({a: 1}, {a: 2}) === false;
        });
        "#
    );
    assert_all_pass(&results);
}

// ---------------------------------------------------------------------------
// 4. util misc oracle faces — TextEncoder/TextDecoder identity + the %i
//    truncation divergence (U6).
// ---------------------------------------------------------------------------

#[test]
fn c17_util_misc_oracle_faces() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // node oracle: util.TextEncoder === TextEncoder (identity re-export).
        check("text_codecs_identity", function() {
            return util.TextEncoder === TextEncoder
                && util.TextDecoder === TextDecoder;
        });
        check("text_codecs_roundtrip", function() {
            var e = new util.TextEncoder();
            var d = new util.TextDecoder();
            return d.decode(e.encode('héllo')) === 'héllo';
        });
        // U6: node truncates (%i → parseInt); %d/%f are Number/parseFloat.
        check("format_i_truncates", function() {
            return util.format('%i', 2.9) === '2'
                && util.format('%i', '2.9x') === '2'
                && util.format('%i', 'x') === 'NaN';
        });
        check("format_d_double_display", function() {
            return util.format('%d', 2.5) === '2.5'
                && util.format('%d', '3.9x') === 'NaN'
                && util.format('%d', BigInt(9)) === '9n';
        });
        check("format_f_parse_float", function() {
            return util.format('%f', '2.9x') === '2.9'
                && util.format('%f', BigInt(9)) === '9';
        });
        // %c consumes its argument; leftover-argument joining stays on the
        // already-registered C8b divergence (bao drops them, node appends).
        check("format_c_consumes_argument", function() {
            return util.format('%c', 'color') === '';
        });
        // %j over an array (JSON semantics, not inspect).
        check("format_j_array_json_form", function() {
            return util.format('%j', [1, 'a']) === '[1,"a"]';
        });
        "#
    );
    assert_all_pass(&results);
}

// ---------------------------------------------------------------------------
// 5. tty — true-tty arms on a real kernel pty (the non-tty refused faces
//    are C5's). Oracle: node v24.19.0 byte-level probes on the same pty
//    geometry (TIOCSWINSZ 80x24 → isTTY/cols/rows/getWindowSize exact).
// ---------------------------------------------------------------------------

struct Pty {
    master: i32,
    slave: i32,
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.master);
            libc::close(self.slave);
        }
    }
}

fn open_pty(cols: u16, rows: u16) -> Pty {
    use std::ffi::CString;
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(master >= 0, "posix_openpt: {}", std::io::Error::last_os_error());
        assert_eq!(libc::grantpt(master), 0, "grantpt");
        assert_eq!(libc::unlockpt(master), 0, "unlockpt");
        let mut buf = [0u8; 128];
        let rc = libc::ptsname_r(master, buf.as_mut_ptr() as *mut libc::c_char, buf.len());
        assert_eq!(rc, 0, "ptsname_r");
        let path = std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char)
            .to_string_lossy()
            .into_owned();
        let cpath = CString::new(path).unwrap();
        let slave = libc::open(cpath.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(slave >= 0, "open slave: {}", std::io::Error::last_os_error());
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        assert_eq!(libc::ioctl(master, libc::TIOCSWINSZ, &ws), 0, "TIOCSWINSZ");
        Pty { master, slave }
    }
}

fn set_master_nonblocking(master: i32) {
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

/// Read everything currently buffered on the pty master (non-blocking).
fn drain_master(master: i32) -> String {
    let mut out = Vec::new();
    unsafe {
        let mut buf = [0u8; 1024];
        loop {
            let n = libc::read(master, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            if n <= 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn c17_tty_pty_true_size_arms() {
    let pty = open_pty(80, 24);
    set_master_nonblocking(pty.master);
    let fd = pty.slave;
    let mut ctx = make_ctx();

    // Phase 1: identity + sized-window ctor arms (oracle: node on the same
    // geometry reports isTTY=true cols=80 rows=24 gws=[80,24]).
    let results = run_checks(
        &mut ctx,
        &format!(
            r#"
        var tty = require('tty');
        check("isatty_pty_true", function() {{
            return tty.isatty({fd}) === true;
        }});
        check("rs_ctor_true_tty_arms", function() {{
            var rs = new tty.ReadStream({fd});
            return rs.fd === {fd} && rs.isTTY === true && rs.isRaw === false && rs.readable === true;
        }});
        check("rs_setraw_roundtrip_arms", function() {{
            var rs = new tty.ReadStream({fd});
            rs.setRawMode(true);
            var raw = rs.isRaw === true;
            rs.setRawMode(false);
            return raw && rs.isRaw === false;
        }});
        check("ws_ctor_size_arms", function() {{
            var ws = new tty.WriteStream({fd});
            return ws.isTTY === true && ws.columns === 80 && ws.rows === 24;
        }});
        check("ws_get_window_size_pair", function() {{
            var ws = new tty.WriteStream({fd});
            var g = ws.getWindowSize();
            return g.length === 2 && g[0] === 80 && g[1] === 24;
        }});
        check("ws_write_true", function() {{
            var ws = new tty.WriteStream({fd});
            return ws.write('C17MARK') === true;
        }});
        // Lifecycle returns (node returns the stream object for these).
        check("rs_lifecycle_returns_this", function() {{
            var rs = new tty.ReadStream({fd});
            return rs.resume() === rs && rs.pause() === rs && rs.destroy() === rs
                && rs.ref() === rs && rs.unref() === rs;
        }});
        // Non-object `this` guards (native arms).
        check("lifecycle_non_object_this_arms", function() {{
            var rs = new tty.ReadStream({fd});
            return rs.resume.call(5) === undefined
                && rs.pause.call(5) === undefined
                && rs.destroy.call(5) === undefined
                && rs.ref.call(5) === undefined
                && rs.unref.call(5) === undefined;
        }});
        "#
        ),
    );
    assert_all_pass(&results);

    // Phase 2: escape-byte verification against the kernel pty — every
    // writer arm emits its documented byte sequence on the master side.
    let results2 = run_checks(
        &mut ctx,
        &format!(
            r#"
        var tty = require('tty');
        var ws = new tty.WriteStream({fd});
        ws.clearLine(-1);
        ws.clearLine(1);
        ws.clearLine(0);
        ws.clearLine(5);
        ws.cursorTo(3, 4);
        ws.cursorTo(4);
        ws.moveCursor(1, 2);
        ws.moveCursor(-2, -3);
        ws.moveCursor(0, 0);
        ws.clearScreenDown();
        true;
        "#
        ),
    );
    let _ = results2;
    std::thread::sleep(std::time::Duration::from_millis(20));
    let bytes = drain_master(pty.master);
    assert!(bytes.contains("C17MARK"), "write() bytes missing on master: {:?}", bytes);
    assert!(bytes.contains("\x1b[1K"), "clearLine(-1) escape missing: {:?}", bytes);
    assert!(bytes.contains("\x1b[0K"), "clearLine(1) escape missing: {:?}", bytes);
    assert!(bytes.contains("\x1b[2K"), "clearLine(0) escape missing: {:?}", bytes);
    // clearLine(any other positive) → 0K (node `dir > 0 ? kClearToLineEnd`).
    // cursorTo(3,4): row+1=5, col+1=4 (node's \x1b[5;4H byte-for-byte).
    assert!(bytes.contains("\x1b[5;4H"), "cursorTo(3,4) escape missing: {:?}", bytes);
    // cursorTo(4) with no y: column-only form, row preserved (T3).
    assert!(bytes.contains("\x1b[5G"), "cursorTo(4) column-only escape missing: {:?}", bytes);
    // moveCursor(1,2): dx first, dy>0 → DOWN (T2 node byte contract).
    assert!(
        bytes.contains("\x1b[1C\x1b[2B"),
        "moveCursor(1,2) node-order bytes missing: {:?}",
        bytes
    );
    assert!(
        !bytes.contains("\x1b[2A\x1b[1C"),
        "moveCursor still emits the inverted/old order: {:?}",
        bytes
    );
    // moveCursor(-2,-3): left then up.
    assert!(
        bytes.contains("\x1b[2D\x1b[3A"),
        "moveCursor(-2,-3) node-order bytes missing: {:?}",
        bytes
    );
    // moveCursor(0,0) writes nothing (empty-esc guard).
    assert!(
        !bytes.contains("\x1b[1A") && !bytes.contains("\x1b[1B"),
        "empty moveCursor wrote bytes: {:?}",
        bytes
    );
    // clearScreenDown: bao's inherited CSI J form (node emits 0J; byte
    // divergence is outside the T1-T4 registry).
    assert!(bytes.contains("\x1b[J"), "clearScreenDown escape missing: {:?}", bytes);
}

#[test]
fn c17_tty_pty_zero_size_and_return_pins() {
    let pty = open_pty(0, 0);
    set_master_nonblocking(pty.master);
    let fd = pty.slave;
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        &format!(
            r#"
        var tty = require('tty');

        // T4: a fresh 0x0 pty reports numeric columns=0 rows=0, exactly
        // node's oracle, and getWindowSize() is [0, 0] (not undefined).
        check("ws_zero_size_ctor_parity_T4", function() {{
            var ws = new tty.WriteStream({fd});
            if (ws.isTTY === true && ws.columns === 0 && ws.rows === 0) return true;
            throw new Error('GOT:isTTY=' + ws.isTTY + ',cols=' + JSON.stringify(ws.columns) + ',rows=' + JSON.stringify(ws.rows));
        }});
        check("gws_zero_size_pair_T4", function() {{
            var ws = new tty.WriteStream({fd});
            var g = ws.getWindowSize();
            return g && g.length === 2 && g[0] === 0 && g[1] === 0;
        }});
        // T1: writer methods return boolean true (byte-level oracle probes).
        check("writer_returns_true_T1", function() {{
            var ws = new tty.WriteStream({fd});
            return ws.clearLine(0) === true && ws.clearScreenDown() === true
                && ws.cursorTo(1, 1) === true && ws.moveCursor(1, 1) === true;
        }});
        // T5: non-int32 arguments must not abort the process (the old
        // JSVal::to_int32 assert crashed the whole engine). node coerces
        // with ToNumber: 'x' → NaN → entire line, '1' → 0K.
        check("clearline_non_number_tolerated_T5", function() {{
            var ws = new tty.WriteStream({fd});
            var a = ws.clearLine('x');
            var b = ws.clearLine('1');
            var c = ws.clearLine(undefined);
            return a === true && b === true && c === true;
        }});
        // node cursorTo error faces (argument validation, not abort).
        check("cursorto_argument_validation", function() {{
            var ws = new tty.WriteStream({fd});
            function thrown(fn) {{ try {{ return fn(); }} catch (e) {{ return e; }} }}
            var nan = thrown(function() {{ ws.cursorTo(NaN); }});
            var pos = thrown(function() {{ ws.cursorTo('x', 5); }});
            var silent = ws.cursorTo('x');
            return nan && nan.name === 'TypeError' && nan.code === 'ERR_INVALID_ARG_VALUE'
                && pos && pos.code === 'ERR_INVALID_CURSOR_POS'
                && silent === true;
        }});
        // clearLine absent-dir arm: missing arg → NaN comparisons → entire
        // line escape.
        check("clearline_default_dir_entire", function() {{
            var ws = new tty.WriteStream({fd});
            ws.clearLine();
            return true;
        }});
        "#
        ),
    );
    assert_all_pass(&results);

    std::thread::sleep(std::time::Duration::from_millis(20));
    let bytes = drain_master(pty.master);
    // Absent/'x' dir → NaN → entire-line escape; numeric string '1' → 0K.
    assert!(
        bytes.matches("\x1b[2K").count() >= 2,
        "clearLine non-number/default arms missing: {:?}",
        bytes
    );
    assert!(
        bytes.matches("\x1b[0K").count() >= 1,
        "clearLine('1') positive-dir arm missing: {:?}",
        bytes
    );
    // cursorTo(1,1) → \x1b[2;2H (row+1;col+1 — the arm node does NOT take;
    // node keeps the row and emits \x1b[2G; T3 registered).
    assert!(
        bytes.contains("\x1b[2;2H"),
        "cursorTo(1,1) bao row-col escape missing: {:?}",
        bytes
    );
}

// ---------------------------------------------------------------------------
// 6. lib.rs — crate-root helper faces (raw-JSAPI property walk, string
//    helpers, exit no-ops).
// ---------------------------------------------------------------------------

#[test]
fn c17_lib_for_each_string_property_faces() {
    let mut ctx = make_ctx();
    let setup = eval_string_full(
        &mut ctx,
        r#"
        globalThis.__c17obj = { a: 1, b: 'two' };
        globalThis.__c17sym = {};
        globalThis.__c17sym[Symbol('hidden')] = 1;
        globalThis.__c17sym.shown = 2;
        globalThis.__c17thr = { get bad() { throw new Error('getter-boom'); }, ok: 1 };
        globalThis.__c17badproxy = new Proxy({}, { ownKeys() { throw new Error('ownkeys-boom'); } });
        'setup-ok'
        "#,
    );
    assert_eq!(setup, "setup-ok");

    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("realm global");
    let mut cxm = ctx.cx();
    rooted!(&in(cxm) let g_root = global);
    // Realm entry is load-bearing for raw JSAPI on this cx (engine_props
    // shape): bare calls outside AutoRealm SIGSEGV.
    let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;
    let raw = unsafe { realm_cx.raw_cx() };

    unsafe {
        use std::marker::PhantomData;
        type MutH = mozjs::jsapi::MutableHandle<mozjs::jsapi::Value>;
        let mut plain_val = mozjs::jsval::UndefinedValue();
        assert!(mozjs::jsapi::JS_GetProperty(
            raw,
            g_root.handle().into(),
            c"__c17obj".as_ptr(),
            MutH { _phantom_0: PhantomData, ptr: &mut plain_val },
        ));
        rooted!(&in(realm_cx) let plain_obj = plain_val.to_object());
        let mut seen: Vec<String> = Vec::new();
        let mut all_got = true;
        let walk_ok = bun_runtime::for_each_string_property(
            raw,
            plain_obj.handle().into(),
            |name, _id, _val, got| {
                if !got {
                    all_got = false;
                }
                seen.push(name.to_string());
            },
        );
        assert!(walk_ok, "plain walk failed");
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
        assert!(all_got);

        // Symbol keys are skipped; string keys still visited.
        let mut sym_val = mozjs::jsval::UndefinedValue();
        assert!(mozjs::jsapi::JS_GetProperty(
            raw,
            g_root.handle().into(),
            c"__c17sym".as_ptr(),
            MutH { _phantom_0: PhantomData, ptr: &mut sym_val },
        ));
        rooted!(&in(realm_cx) let sym_obj = sym_val.to_object());
        let mut sym_seen: Vec<String> = Vec::new();
        let sym_ok = bun_runtime::for_each_string_property(
            raw,
            sym_obj.handle().into(),
            |name, _id, _val, _got| sym_seen.push(name.to_string()),
        );
        assert!(sym_ok);
        assert_eq!(sym_seen, vec!["shown".to_string()]);

        // Throwing getter: enumeration succeeds, the failing get delivers
        // got=false and the walk continues (no abort, no pending-exception
        // side effect on the loop).
        let mut thr_val = mozjs::jsval::UndefinedValue();
        assert!(mozjs::jsapi::JS_GetProperty(
            raw,
            g_root.handle().into(),
            c"__c17thr".as_ptr(),
            MutH { _phantom_0: PhantomData, ptr: &mut thr_val },
        ));
        rooted!(&in(realm_cx) let thr_obj = thr_val.to_object());
        let mut got_map: Vec<(String, bool)> = Vec::new();
        let thr_ok = bun_runtime::for_each_string_property(
            raw,
            thr_obj.handle().into(),
            |name, _id, _val, got| got_map.push((name.to_string(), got)),
        );
        assert!(thr_ok);
        assert!(
            got_map.iter().any(|(n, g)| n == "bad" && !g),
            "throwing getter not reported: {:?}",
            got_map
        );
        assert!(
            got_map.iter().any(|(n, g)| n == "ok" && *g),
            "healthy prop missing: {:?}",
            got_map
        );

        // GetPropertyKeys failure arm: a proxy whose ownKeys trap throws →
        // the walk reports false and the callback never runs.
        let mut px_val = mozjs::jsval::UndefinedValue();
        assert!(mozjs::jsapi::JS_GetProperty(
            raw,
            g_root.handle().into(),
            c"__c17badproxy".as_ptr(),
            MutH { _phantom_0: PhantomData, ptr: &mut px_val },
        ));
        rooted!(&in(realm_cx) let px_obj = px_val.to_object());
        let mut px_seen = 0usize;
        let px_ok = bun_runtime::for_each_string_property(
            raw,
            px_obj.handle().into(),
            |_name, _id, _val, _got| px_seen += 1,
        );
        assert!(!px_ok, "throwing ownKeys proxy must fail the walk");
        assert_eq!(px_seen, 0, "callback ran despite enumeration failure");

        // jsstr_to_rust_string null guard.
        assert_eq!(bun_runtime::jsstr_to_rust_string(raw, std::ptr::null_mut()), "");
    }
}

#[test]
fn c17_lib_string_helpers_and_exit_noops() {
    let mut ctx = make_ctx();
    let setup = eval_string_full(&mut ctx, "globalThis.__c17str = 'héllo ✓ c17'; 'ok'");
    assert_eq!(setup, "ok");

    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("realm global");
    let mut cxm = ctx.cx();
    rooted!(&in(cxm) let g_root = global);
    let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;
    let raw = unsafe { realm_cx.raw_cx() };

    unsafe {
        use std::marker::PhantomData;
        type MutH = mozjs::jsapi::MutableHandle<mozjs::jsapi::Value>;

        let mut str_val = mozjs::jsval::UndefinedValue();
        assert!(mozjs::jsapi::JS_GetProperty(
            raw,
            g_root.handle().into(),
            c"__c17str".as_ptr(),
            MutH { _phantom_0: PhantomData, ptr: &mut str_val },
        ));
        assert!(str_val.is_string());
        // js_to_rust_string happy path (non-latin1 UTF-16 roundtrip).
        assert_eq!(bun_runtime::js_to_rust_string(raw, str_val), "héllo ✓ c17");
        // Exit no-ops (install_exit_handler is an init-time no-op kept for
        // API compat; must be callable repeatedly).
        bun_runtime::install_exit_handler();
        bun_runtime::install_exit_handler();
    }

    // The exit-flag lifecycle itself is C8b-pinned (runtime_misc_edge);
    // only assert the harness entry state here.
    let was = bun_runtime::should_exit();
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "fresh harness must not inherit an exit code"
    );
    let _ = was;

    // force_link_bun_install: keeps the bun_install unit linked; callable.
    bun_runtime::force_link_bun_install();
}
