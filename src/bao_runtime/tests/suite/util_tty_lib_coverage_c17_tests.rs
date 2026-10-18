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
// ── 产品缺陷登记(本波只登记不修,逐项 = node oracle 实证)────────────────
// | # | 面 | bao 现行为 | node v24.19.0 oracle | 触达测试 |
// |---|---|---|---|---|
// | U1 | util.parseArgs | Rust stub 返回 `{}` | `parseArgs(["-f","--bar","b","pos"])` → `{values:{f:true,bar:"b"},positionals:["pos"]}` | c17_util_stub_divergence_pins |
// | U2 | util.getSystemErrorName | 恒 undefined | `-1`→"EPERM" / `-13`→"EACCES";`0`/正数 → throw ERR_OUT_OF_RANGE;非数字 → ERR_INVALID_ARG_TYPE | c17_util_stub_divergence_pins |
// | U3 | util.deprecate | 原样返回 fn,零包装零告警 | 返回包装 fn(`w !== g`),首次调用发 DeprecationWarning | c17_util_stub_divergence_pins |
// | U4 | util.inherits | 只设 `Child.super_` | 另接原型链(`C.prototype.__proto__ = P.prototype` → `new C() instanceof P` 为 true) | c17_util_stub_divergence_pins |
// | U5 | util.inspect 类族 | Map/Set/Date/RegExp/TypedArray/WeakMap/Promise/Symbol/BigInt/class/Error 全部落到 plain-object 路径(Error 连 message 自有属性也非枚举 own → `{}`;`{}`/空串/无类名前缀) | `Map(1) { 1 => 'a' }` / `TypeError: boom` / `Symbol(s)` / `9n` / `Point { x: 1 }` / `[Object: null prototype] {}` 等 | c17_util_inspect_class_family |
// | U6 | util.format `%i`/`%d` | 直接走 display(2.9 → "2.9") | `%i` parseInt 截断(2.9 → "2") | c17_util_misc_oracle_faces |
// | U7 | util.types.isProxy | `v && typeof v==='object' && !v.constructor` — 有 constructor 的 proxy 返 false | 真 proxy 恒 true | c17_util_stub_divergence_pins |
// | U8 | assert loose deepEqual 失败文案 | "Expected values to be loosely deeply equal:" | "Expected values to be loosely deep-equal:"(diff 主体形状等价) | c17_util_assert_diff_faces |
// | U9 | isDeepStrictEqual Rust native | 注册后被 JS 扩展同名覆盖 = 死注册臂,JS 侧运行时永不可达(可达性死臂,非行为缺陷) | — | (登记,无测试可达) |
// | T1 | tty WriteStream clearLine/clearScreenDown/cursorTo/moveCursor 返回值 | 返回 `this` | 返回 boolean true(字节级 pty 实证) | c17_tty_pty_zero_size_and_return_pins |
// | T2 | tty moveCursor(dx,dy) | dy 符号反转(dy>0→`[nA` 上移)+ 臂序 dy 先 dx 后 | dy>0→`[nB` 下移,臂序 dx 先(`moveCursor(1,2)` → `\x1b[1C\x1b[2B`) | c17_tty_pty_true_size_arms |
// | T3 | tty cursorTo(x) 缺 y 臂 | 强制行归位(`\x1b[1;1H`) | 仅列移动(`cursorTo(4)` → `\x1b[5G`) | c17_tty_pty_true_size_arms |
// | T4 | tty 零尺寸窗臂(ws_col==0) | getWindowSize()=undefined(ctor columns/rows=数值 0/0 与 node 平价,实证非分歧) | ctor 数值 0/0;getWindowSize()=[0,0] | c17_tty_pty_zero_size_and_return_pins |
// | U10 | assert fail()/equal() 消息文本 | e.message 携带 `AssertionError: ` 前缀(e.g. `AssertionError: boom`;equal 文案 `Expected values to be loosely equal:`) | fail message=裸参数 `boom`;equal=双侧渲染 `==` 联结 | c17_util_assert_diff_faces |
// | L1 | lib.rs seam 体(~300 行:force_link_native_c_libs + test_link_seams no_mangle 组) | 链接期 face,运行时不可达(同 cli.rs 类测量法伪影,入分母治理账) | — | (登记,归分母治理) |
// | L2 | lib.rs shutdown_engine | 进程级 JS_ShutDown,suite 内调用毒化后续测试(plain cargo test --test-threads=1 同进程) | — | (登记:harness 边界面) |

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

        // node oracle: Map(1) { 1 => 'a' } — bao has no Map branch; the
        // instance has no own enumerable props → plain-object `{}` face
        // (divergence U5, registered).
        check("inspect_map_plain_object_face", function() {
            return util.inspect(new Map([[1, 'a']])) === '{}';
        });
        check("inspect_set_plain_object_face", function() {
            return util.inspect(new Set([1, 2])) === '{}';
        });
        // node oracle: 1970-01-01T00:00:00.000Z — bao: Date instances carry
        // no own enumerable props → '{}'.
        check("inspect_date_plain_object_face", function() {
            return util.inspect(new Date(0)) === '{}';
        });
        check("inspect_regexp_plain_object_face", function() {
            return util.inspect(/ab/g) === '{}';
        });
        // node oracle: TypeError: boom — bao: the message own-prop is not
        // enumerable-own under GetPropertyKeys, so the instance renders {}
        // (U5 family, registered).
        check("inspect_error_plain_object_face", function() {
            return util.inspect(new TypeError('boom')) === '{}';
        });
        // node oracle: Uint8Array(3) [ 1, 2, 3 ] — bao: typed arrays are
        // not Buffer views via the _isBuffer marker, so the byte-view
        // branch is skipped and the integer-index loop renders the
        // elements as an object body (ints first, ascending).
        check("inspect_uint8array_index_face", function() {
            var s = util.inspect(new Uint8Array([1, 2, 3]));
            return s === '{ 0: 1, 1: 2, 2: 3 }';
        });
        // node oracle: WeakMap { <items unknown> } — bao: no own props.
        check("inspect_weakmap_plain_object_face", function() {
            return util.inspect(new WeakMap()) === '{}';
        });
        // node oracle: Promise { <pending> } — bao: no own enumerable props.
        check("inspect_promise_plain_object_face", function() {
            return util.inspect(new Promise(function() {})) === '{}';
        });
        // node oracle: Symbol(s) — bao: symbol JSVal is neither object nor
        // the covered primitives → the fall-through empty-string arm.
        check("inspect_symbol_empty_arm", function() {
            return util.inspect(Symbol('s')) === '';
        });
        // node oracle: 9n — BigInt falls through to the same empty arm.
        check("inspect_bigint_empty_arm", function() {
            return util.inspect(BigInt(9)) === '';
        });
        // node oracle: Point { x: 1 } (class-name prefix) — bao: no class
        // branch, own-prop render only.
        check("inspect_class_instance_no_prefix", function() {
            var s = util.inspect(new (function Point() { this.x = 1; })());
            return s === '{ x: 1 }';
        });
        // node oracle: [Object: null prototype] {} — bao: plain {} face.
        check("inspect_null_proto_plain_face", function() {
            return util.inspect(Object.create(null)) === '{}';
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
            var outer = { m: new Map() };
            var s = util.inspect(outer, { depth: 0 });
            return s === '{ m: [Object] }';
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
        // + diff body; bao: "loosely deeply equal" (U8, registered). The
        // diff body shape (object lines, +/- markers on the differing key)
        // is the contract under test.
        check("deep_equal_loose_diff_body", function() {
            var msg = '';
            try { assert.deepEqual({ a: 1, b: 2 }, { a: 1, b: 3 }); }
            catch (e) {
                if (e.name !== 'AssertionError') return false;
                msg = e.message;
            }
            return msg.indexOf('loosely deeply equal') !== -1
                && msg.indexOf('a: 1') !== -1
                && msg.indexOf('b: 2') !== -1 && msg.indexOf('b: 3') !== -1
                && msg.indexOf('+') !== -1 && msg.indexOf('-') !== -1;
        });
        // equal() on objects: multi-line diff instead of the collapsed
        // "[object Object] == [object Object]" pre-fix form.
        // U10 (registered): bao's equal() failure message is prefixed
        // "AssertionError: " and worded "Expected values to be loosely
        // equal:" with a diff body; node renders both sides joined by ==.
        check("equal_objects_diff_not_object_object", function() {
            var msg = '';
            try { assert.equal({ a: 1 }, { a: 1 }); }
            catch (e) { msg = e.message; }
            if (msg.indexOf('loosely equal') !== -1 && msg.length > 40
                && msg.indexOf('[object Object]') === -1) return true;
            throw new Error('GOT:' + msg.substring(0, 120));
        });
        // U10 (registered): node's message is the bare argument; bao's
        // carries the "AssertionError: " prefix inside message.
        check("fail_throws_assertion_error", function() {
            try { assert.fail('boom'); }
            catch (e) {
                if (e && e.name === 'AssertionError'
                    && e.message.indexOf('boom') !== -1
                    && e.message.indexOf('AssertionError') === 0) return true;
                throw new Error('GOT:name=' + (e && e.name) + ',msg=' + (e && e.message) + ',kind=' + typeof e);
            }
            throw new Error('GOT:no-throw');
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
fn c17_util_stub_divergence_pins() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var util = require('util');

        // U1: node → {values:{f:true,bar:'b'},positionals:['pos']}; bao stub → {}.
        check("parseargs_stub_empty_object_KNOWN_DIVERGE_U1", function() {
            var r = util.parseArgs(['-f', '--bar', 'b', 'pos']);
            return r !== null && typeof r === 'object'
                && r.values === undefined && r.positionals === undefined;
        });
        // U2: node → getSystemErrorName(-1)==='EPERM' etc.; bao → undefined.
        check("getsystemerrorname_stub_undefined_KNOWN_DIVERGE_U2", function() {
            return util.getSystemErrorName(-1) === undefined
                && util.getSystemErrorName(1) === undefined
                && util.getSystemErrorName() === undefined;
        });
        // U3: node wraps (w !== g) + DeprecationWarning once; bao passes the
        // original reference through with no warning side effect.
        check("deprecate_verbatim_passthrough_KNOWN_DIVERGE_U3", function() {
            var g = function() { return 41; };
            var w = util.deprecate(g, 'gone soon');
            return w === g && w() === 41;
        });
        check("deprecate_noargs_returns_undefined", function() {
            return util.deprecate() === undefined;
        });
        // U4: node also links the prototype chain (instanceof P true); bao
        // only sets super_.
        check("inherits_super_only_KNOWN_DIVERGE_U4", function() {
            function P() {}
            function C() {}
            util.inherits(C, P);
            var protoLinked = C.prototype.__proto__ === P.prototype;
            return C.super_ === P && protoLinked === false
                && !((new C()) instanceof P);
        });
        // inherits argc/argument guards (native arms).
        check("inherits_guards", function() {
            function C() {}
            return util.inherits(C) === undefined
                && util.inherits(5, {}) === undefined
                && util.inherits({}, 5) === undefined;
        });
        // U7: node types.isProxy(real proxy) === true; bao's constructor
        // probe answers false for any proxy whose target has a constructor.
        check("types_isproxy_constructor_probe_KNOWN_DIVERGE_U7", function() {
            var p = new Proxy({}, {});
            return util.types.isProxy(p) === false
                && util.types.isProxy({}) === false;
        });
        check("types_is_module_namespace_object", function() {
            // Math is not a module namespace; a real namespace needs ESM, so
            // only the negative face is harness-reachable.
            return util.types.isModuleNamespaceObject(Math) === false
                && util.types.isModuleNamespaceObject({}) === false;
        });
        // The JS-level isDeepStrictEqual extension shadows the registered
        // Rust native (U9 dead-registration arm); its object semantics are
        // the JS face C8b pinned — here the primitive/double arm.
        check("is_deep_strict_equal_double_arm", function() {
            return util.isDeepStrictEqual(2.5, 2.5) === true
                && util.isDeepStrictEqual(2.5, 2) === false
                && util.isDeepStrictEqual(true, 1) === false;
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
        // U6: node truncates (%i → parseInt); bao displays the double as-is.
        check("format_i_double_not_truncated_KNOWN_DIVERGE_U6", function() {
            return util.format('%i', 2.9) === '2.9';
        });
        // %d with a double renders the double (same display path).
        check("format_d_double_display", function() {
            return util.format('%d', 2.5) === '2.5';
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
        ws.cursorTo(3, 4);
        ws.moveCursor(1, 2);
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
    // cursorTo(3,4): row+1=5, col+1=4 (matches node's \x1b[5;4H byte-for-byte).
    assert!(bytes.contains("\x1b[5;4H"), "cursorTo(3,4) escape missing: {:?}", bytes);
    // moveCursor(1,2): bao emits dy-then-dx with dy>0 → UP ([2A) — node emits
    // \x1b[1C\x1b[2B (dx-first, dy>0 → DOWN). Divergence T2, registered; the
    // pin below is bao's actual byte contract.
    assert!(
        bytes.contains("\x1b[2A\x1b[1C"),
        "moveCursor(1,2) bao-order bytes missing (node order is [1C[2B): {:?}",
        bytes
    );
    // moveCursor(0,0) writes nothing (empty-esc guard).
    assert!(
        !bytes.contains("\x1b[1A") && !bytes.contains("\x1b[1B"),
        "empty moveCursor wrote bytes: {:?}",
        bytes
    );
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

        // T4 ctor half: OBSERVED PARITY — bao on a fresh 0x0 pty reports
        // numeric columns=0 rows=0, exactly node's oracle (the has_size arm
        // is true whenever tcgetwinsize succeeds, zero included).
        check("ws_zero_size_ctor_parity_T4", function() {{
            var ws = new tty.WriteStream({fd});
            if (ws.isTTY === true && ws.columns === 0 && ws.rows === 0) return true;
            throw new Error('GOT:isTTY=' + ws.isTTY + ',cols=' + JSON.stringify(ws.columns) + ',rows=' + JSON.stringify(ws.rows));
        }});
        check("gws_zero_size_undefined_KNOWN_DIVERGE_T4", function() {{
            var ws = new tty.WriteStream({fd});
            return ws.getWindowSize() === undefined;
        }});
        // T1 (registered): writer methods return `this`; node returns
        // boolean true (byte-level oracle probes).
        check("writer_returns_this_KNOWN_DIVERGE_T1", function() {{
            var ws = new tty.WriteStream({fd});
            return ws.clearLine(0) === ws && ws.clearScreenDown() === ws
                && ws.cursorTo(1, 1) === ws && ws.moveCursor(1, 1) === ws;
        }});
        // clearLine absent-dir arm: missing arg → dir 0 → entire line
        // escape. (Non-numeric dir args are NOT exercised here: the native
        // reads them via JSVal::to_int32, which debug-asserts is_int32 —
        // a string arg aborts the process in dev builds. Defect T5,
        // registered; node tolerates 'x' → NaN → entire-line.)
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
    // Absent dir → entire-line escape (the T5-aborted string-dir arm is
    // registered, not exercised).
    assert!(
        bytes.matches("\x1b[2K").count() >= 1,
        "clearLine absent-dir arm missing: {:?}",
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
